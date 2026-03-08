use anyhow::{anyhow, Context, Result};
use midir::{Ignore, MidiInput, MidiOutput};
use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub enum HotkeyAction {
    Home,
    BarBack,
    BarForward,
    BeatBack,
    BeatForward,
}

impl HotkeyAction {
    pub fn default_note(self) -> u8 {
        match self {
            // Dedicated trigger notes (non-musical low range).
            HotkeyAction::Home => 36,
            HotkeyAction::BarBack => 37,
            HotkeyAction::BarForward => 38,
            HotkeyAction::BeatBack => 39,
            HotkeyAction::BeatForward => 40,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TriggerEvent {
    channel: u8,
    note: u8,
    velocity: u8,
}

pub fn run_hotkey_send(
    midi_out_hint: Option<&str>,
    channel: u8,
    note: u8,
    velocity: u8,
    hold_ms: u64,
) -> Result<()> {
    if !(1..=16).contains(&channel) {
        return Err(anyhow!("invalid_midi_channel:{channel}"));
    }
    let out = MidiOutput::new("mixct-hotkey-send")?;
    let ports = out.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_output_ports_found"));
    }
    let selected_idx = select_output_port_index(&out, midi_out_hint)?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_output_port_index"))?;
    let port_name = out
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());
    let mut conn = out
        .connect(port, "mixct-hotkey-send")
        .with_context(|| format!("failed to connect MIDI out: {port_name}"))?;
    let status = 0x90 | ((channel - 1) & 0x0F);
    conn.send(&[status, note, velocity])?;
    std::thread::sleep(Duration::from_millis(hold_ms.max(10)));
    conn.send(&[status, note, 0x00])?;
    println!(
        "hotkey_send: sent note={} vel={} ch={} on '{}'",
        note, velocity, channel, port_name
    );
    Ok(())
}

#[derive(Debug, Clone)]
pub struct HotkeySeekReport {
    pub home_sent: bool,
    pub bar_steps: u32,
    pub beat_steps: u32,
    pub target_bar: u32,
}

pub fn run_transport_seek_hotkey(
    midi_out_hint: Option<&str>,
    channel: u8,
    target_bar: u32,
    target_beat: f64,
    target_tick: u32,
    do_home: bool,
    velocity: u8,
    hold_ms: u64,
    interval_ms: u64,
    settle_ms: u64,
) -> Result<HotkeySeekReport> {
    if !(1..=16).contains(&channel) {
        return Err(anyhow!("invalid_midi_channel:{channel}"));
    }
    if target_bar == 0 {
        return Err(anyhow!("target_bar_must_be_1_based"));
    }
    if target_beat < 1.0 {
        return Err(anyhow!("target_beat_must_be_at_least_1"));
    }
    if target_tick > 0 {
        return Err(anyhow!(
            "target_tick_not_supported_in_hotkey_seek_use_0_or_ltc_trim"
        ));
    }
    let beat_int = target_beat.round();
    if (target_beat - beat_int).abs() > 1e-6 {
        return Err(anyhow!(
            "fractional_beats_not_supported_in_hotkey_seek target_beat={}",
            target_beat
        ));
    }
    let beat_int = beat_int as u32;
    if beat_int == 0 {
        return Err(anyhow!("target_beat_must_be_at_least_1"));
    }

    let out = MidiOutput::new("mixct-hotkey-seek")?;
    let ports = out.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_output_ports_found"));
    }
    let selected_idx = select_output_port_index(&out, midi_out_hint)?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_output_port_index"))?;
    let port_name = out
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());
    let mut conn = out
        .connect(port, "mixct-hotkey-seek")
        .with_context(|| format!("failed to connect MIDI out: {port_name}"))?;
    let status = 0x90 | ((channel - 1) & 0x0F);

    let bar_steps = target_bar.saturating_sub(1);
    let beat_steps = beat_int.saturating_sub(1);

    if do_home {
        send_note_once(
            &mut conn,
            status,
            HotkeyAction::Home.default_note(),
            velocity,
            hold_ms,
        )?;
        std::thread::sleep(Duration::from_millis(settle_ms.max(20)));
    }

    send_note_repeat(
        &mut conn,
        status,
        HotkeyAction::BarForward.default_note(),
        velocity,
        hold_ms,
        interval_ms,
        bar_steps,
    )?;
    send_note_repeat(
        &mut conn,
        status,
        HotkeyAction::BeatForward.default_note(),
        velocity,
        hold_ms,
        interval_ms,
        beat_steps,
    )?;
    std::thread::sleep(Duration::from_millis(settle_ms.max(20)));

    println!(
        "transport_seek_hotkey: port='{}' home={} bar_steps={} beat_steps={} -> target {}|{}|{}",
        port_name, do_home, bar_steps, beat_steps, target_bar, target_beat, target_tick
    );

    Ok(HotkeySeekReport {
        home_sent: do_home,
        bar_steps,
        beat_steps,
        target_bar,
    })
}

pub fn run_hotkey_bridge(
    midi_in_hint: Option<&str>,
    channel_filter: Option<u8>,
    note_map_spec: Option<&str>,
    target_app: Option<&str>,
    duration_sec: Option<u64>,
    verbose: bool,
) -> Result<()> {
    if let Some(ch) = channel_filter {
        if !(1..=16).contains(&ch) {
            return Err(anyhow!("invalid_midi_channel_filter:{ch}"));
        }
    }
    let key_map = parse_note_map_spec(note_map_spec)?;
    if key_map.is_empty() {
        return Err(anyhow!("empty_note_map"));
    }

    let mut inp = MidiInput::new("mixct-hotkey-bridge")?;
    inp.ignore(Ignore::None);
    let ports = inp.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_input_ports_found"));
    }
    let selected_idx = select_input_port_index(&inp, midi_in_hint)?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_input_port_index"))?;
    let port_name = inp
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());

    let (tx, rx) = mpsc::channel::<TriggerEvent>();
    let _conn = inp.connect(
        port,
        "mixct-hotkey-bridge",
        move |_stamp, message, _| {
            if let Some((ch, note, vel)) = parse_note_on(message) {
                let _ = tx.send(TriggerEvent {
                    channel: ch,
                    note,
                    velocity: vel,
                });
            }
        },
        (),
    )?;

    println!(
        "hotkey_bridge: listening on '{}' target_app={:?} channel_filter={:?} map={:?}",
        port_name, target_app, channel_filter, key_map
    );

    let start = Instant::now();
    let mut trig_count = 0u64;
    loop {
        if let Some(limit) = duration_sec {
            if start.elapsed() >= Duration::from_secs(limit) {
                break;
            }
        }
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(120)) {
            if let Some(ch_filter) = channel_filter {
                if ev.channel != ch_filter {
                    continue;
                }
            }
            let Some(keycode) = key_map.get(&ev.note).copied() else {
                continue;
            };
            let fired = dispatch_keycode(keycode, target_app)?;
            if fired {
                trig_count += 1;
            }
            if verbose {
                println!(
                    "hotkey_bridge: note={} vel={} ch={} -> keycode={} fired={}",
                    ev.note, ev.velocity, ev.channel, keycode, fired
                );
            }
        }
    }

    println!("hotkey_bridge: done triggers={}", trig_count);
    Ok(())
}

fn parse_note_on(msg: &[u8]) -> Option<(u8, u8, u8)> {
    if msg.len() < 3 {
        return None;
    }
    let status = msg[0];
    if status & 0xF0 != 0x90 {
        return None;
    }
    let vel = msg[2];
    if vel == 0 {
        return None;
    }
    let ch = (status & 0x0F) + 1;
    Some((ch, msg[1], vel))
}

fn send_note_once(
    conn: &mut midir::MidiOutputConnection,
    status: u8,
    note: u8,
    velocity: u8,
    hold_ms: u64,
) -> Result<()> {
    conn.send(&[status, note, velocity])?;
    std::thread::sleep(Duration::from_millis(hold_ms.max(10)));
    conn.send(&[status, note, 0x00])?;
    Ok(())
}

fn send_note_repeat(
    conn: &mut midir::MidiOutputConnection,
    status: u8,
    note: u8,
    velocity: u8,
    hold_ms: u64,
    interval_ms: u64,
    repeats: u32,
) -> Result<()> {
    for i in 0..repeats {
        send_note_once(conn, status, note, velocity, hold_ms)?;
        if i + 1 < repeats {
            std::thread::sleep(Duration::from_millis(interval_ms.max(1)));
        }
    }
    Ok(())
}

fn parse_note_map_spec(spec: Option<&str>) -> Result<HashMap<u8, u16>> {
    if spec.is_none() {
        // Default mapping -> DP numeric keypad shortcuts.
        // 1=start, 5=bar back, 6=bar forward, 2=beat back, 3=beat forward.
        let mut m = HashMap::new();
        m.insert(36, 83); // kp1
        m.insert(37, 87); // kp5
        m.insert(38, 88); // kp6
        m.insert(39, 84); // kp2
        m.insert(40, 85); // kp3
        return Ok(m);
    }
    let mut out = HashMap::new();
    let raw = spec.unwrap_or_default();
    for pair in raw.split(',') {
        let p = pair.trim();
        if p.is_empty() {
            continue;
        }
        let Some((note_s, key_s)) = p.split_once(':') else {
            return Err(anyhow!("invalid_note_map_pair:{p} expected NOTE:KEYCODE"));
        };
        let note = note_s
            .trim()
            .parse::<u8>()
            .with_context(|| format!("invalid_note_in_pair:{p}"))?;
        let keycode = key_s
            .trim()
            .parse::<u16>()
            .with_context(|| format!("invalid_keycode_in_pair:{p}"))?;
        out.insert(note, keycode);
    }
    Ok(out)
}

fn dispatch_keycode(keycode: u16, target_app: Option<&str>) -> Result<bool> {
    let script = if let Some(app) = target_app {
        let app_esc = app.replace('\"', "\\\"");
        format!(
            "set didSend to false\n\
             tell application \"System Events\"\n\
             if exists process \"{app_esc}\" then\n\
               if frontmost of process \"{app_esc}\" then\n\
                 key code {keycode}\n\
                 set didSend to true\n\
               end if\n\
             end if\n\
             end tell\n\
             return didSend"
        )
    } else {
        format!(
            "tell application \"System Events\" to key code {keycode}\n\
             return true"
        )
    };
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("failed_to_run_osascript")?;
    if !output.status.success() {
        return Err(anyhow!(
            "osascript_failed:{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().eq_ignore_ascii_case("true"))
}

fn select_input_port_index(inp: &MidiInput, hint: Option<&str>) -> Result<usize> {
    let ports = inp.ports();
    let names: Vec<String> = ports
        .iter()
        .map(|p| inp.port_name(p).unwrap_or_else(|_| "<unknown>".to_string()))
        .collect();

    if let Some(h) = hint {
        let lower = h.to_lowercase();
        if let Some((idx, _)) = names
            .iter()
            .enumerate()
            .find(|(_, n)| n.to_lowercase().contains(&lower))
        {
            return Ok(idx);
        }
        return Err(anyhow!(
            "requested_midi_input_not_found:{} available={:?}",
            h,
            names
        ));
    }

    if let Some((idx, _)) = names
        .iter()
        .enumerate()
        .find(|(_, n)| n.to_lowercase().contains("network"))
    {
        return Ok(idx);
    }
    Ok(0)
}

fn select_output_port_index(out: &MidiOutput, hint: Option<&str>) -> Result<usize> {
    let ports = out.ports();
    let names: Vec<String> = ports
        .iter()
        .map(|p| out.port_name(p).unwrap_or_else(|_| "<unknown>".to_string()))
        .collect();

    if let Some(h) = hint {
        let lower = h.to_lowercase();
        if let Some((idx, _)) = names
            .iter()
            .enumerate()
            .find(|(_, n)| n.to_lowercase().contains(&lower))
        {
            return Ok(idx);
        }
        return Err(anyhow!(
            "requested_midi_output_not_found:{} available={:?}",
            h,
            names
        ));
    }
    if let Some((idx, _)) = names
        .iter()
        .enumerate()
        .find(|(_, n)| n.to_lowercase().contains("network"))
    {
        return Ok(idx);
    }
    Ok(0)
}
