use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct TransportPosition {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fps: f32,
    pub source: String,
    pub updated_at_utc: String,
}

impl TransportPosition {
    pub fn to_seconds(&self) -> f64 {
        let base = self.hours as f64 * 3600.0 + self.minutes as f64 * 60.0 + self.seconds as f64;
        base + (self.frames as f64 / self.fps.max(1.0) as f64)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TransportSnapshot {
    pub elapsed_ms: u64,
    pub position: TransportPosition,
    pub bar: u32,
    pub beat: f64,
    pub tick: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransportMonitorReport {
    pub midi_in_port: String,
    pub duration_sec: u64,
    pub snapshot_count: usize,
    pub last: Option<TransportSnapshot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Marker {
    pub name: String,
    pub bar: u32,
    #[serde(default = "default_marker_beat")]
    pub beat: f64,
    #[serde(default)]
    pub tick: u32,
}

fn default_marker_beat() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarkerFile {
    #[serde(default)]
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, Copy)]
pub enum NavAction {
    Rewind,
    FastForward,
}

#[derive(Debug, Clone, Copy)]
pub struct MusicalGrid {
    pub bpm: f64,
    pub ts_num: u32,
    pub ts_den: u32,
    pub ppq: u32,
    pub start_offset_sec: f64,
}

#[derive(Debug)]
enum InEvent {
    Position(TransportPosition),
    Start,
    Continue,
    Stop,
    Clock,
    SongPosition(u16),
}

#[derive(Debug, Default)]
struct QfParser {
    nibbles: [u8; 8],
    seen: [bool; 8],
}

impl QfParser {
    fn ingest_qf(&mut self, data: u8) -> Option<TransportPosition> {
        let idx = ((data >> 4) & 0x07) as usize;
        let nibble = data & 0x0F;
        self.nibbles[idx] = nibble;
        self.seen[idx] = true;
        if idx != 7 || self.seen.iter().any(|v| !v) {
            return None;
        }

        let frames = self.nibbles[0] | ((self.nibbles[1] & 0x01) << 4);
        let seconds = self.nibbles[2] | ((self.nibbles[3] & 0x03) << 4);
        let minutes = self.nibbles[4] | ((self.nibbles[5] & 0x03) << 4);
        let hours = self.nibbles[6] | ((self.nibbles[7] & 0x01) << 4);
        let rate_code = (self.nibbles[7] >> 1) & 0x03;
        let fps = rate_code_to_fps(rate_code);

        Some(TransportPosition {
            hours,
            minutes,
            seconds,
            frames,
            fps,
            source: "mtc_qf".to_string(),
            updated_at_utc: Utc::now().to_rfc3339(),
        })
    }
}

pub fn run_transport_monitor(
    midi_in_hint: Option<&str>,
    duration_sec: u64,
    poll_ms: u64,
    grid: MusicalGrid,
    json_out: Option<&Path>,
) -> Result<()> {
    if duration_sec == 0 {
        return Err(anyhow!("duration_sec_must_be_positive"));
    }
    let mut input = MidiInput::new("mixct-transport-monitor")?;
    input.ignore(Ignore::None);
    let ports = input.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_input_ports_found"));
    }
    let selected_idx = select_input_port_index(&input, midi_in_hint)?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_input_port_index"))?;
    let port_name = input
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());

    let (tx, rx) = std::sync::mpsc::channel::<InEvent>();
    let _conn = connect_input_for_transport(input, port, tx)?;

    println!(
        "transport_monitor: listening on '{}' duration={}s",
        port_name, duration_sec
    );

    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut snapshots = Vec::<TransportSnapshot>::new();
    let mut latest: Option<TransportPosition> = None;
    let mut latest_mtc_at: Option<Instant> = None;
    let mut playing = false;
    let mut last_clock_at: Option<Instant> = None;
    let mut clock_count: u64 = 0;
    let mut spp_16th: Option<u32> = None;
    let mut spp_anchor_clock_count: u64 = 0;
    let mut signal_events: u64 = 0;

    while start.elapsed() < Duration::from_secs(duration_sec) {
        while let Ok(ev) = rx.try_recv() {
            signal_events += 1;
            match ev {
                InEvent::Position(pos) => {
                    latest = Some(pos);
                    latest_mtc_at = Some(Instant::now());
                }
                InEvent::Start | InEvent::Continue => {
                    playing = true;
                    if spp_16th.is_none() {
                        spp_16th = Some(0);
                        spp_anchor_clock_count = clock_count;
                    }
                }
                InEvent::Stop => {
                    playing = false;
                }
                InEvent::Clock => {
                    clock_count = clock_count.saturating_add(1);
                    last_clock_at = Some(Instant::now());
                }
                InEvent::SongPosition(spp) => {
                    spp_16th = Some(spp as u32);
                    spp_anchor_clock_count = clock_count;
                }
            }
        }

        if last_print.elapsed() >= Duration::from_millis(poll_ms.max(40)) {
            let mut display_pos = None;
            if let Some(pos) = latest.clone() {
                if latest_mtc_at
                    .map(|t| t.elapsed() <= Duration::from_secs(2))
                    .unwrap_or(false)
                {
                    display_pos = Some(pos);
                }
            }

            if display_pos.is_none() {
                if let Some(spp) = spp_16th {
                    let qn_base = spp as f64 / 4.0;
                    let qn_delta = if playing {
                        (clock_count.saturating_sub(spp_anchor_clock_count)) as f64 / 24.0
                    } else {
                        0.0
                    };
                    let sec =
                        grid.start_offset_sec + ((qn_base + qn_delta) * 60.0 / grid.bpm.max(1.0));
                    display_pos = Some(seconds_to_transport_position(
                        sec,
                        24.0,
                        "clock_spp_estimate",
                    ));
                } else if let Some(last_clock) = last_clock_at {
                    if last_clock.elapsed() <= Duration::from_secs(1) {
                        let sec = grid.start_offset_sec
                            + ((clock_count as f64 / 24.0) * 60.0 / grid.bpm.max(1.0));
                        display_pos =
                            Some(seconds_to_transport_position(sec, 24.0, "clock_estimate"));
                    }
                }
            }

            if let Some(pos) = display_pos {
                let (bar, beat, tick) = seconds_to_bar_beat_tick(pos.to_seconds(), grid);
                let moving = last_clock_at
                    .map(|t| t.elapsed() <= Duration::from_millis(350))
                    .unwrap_or(false);
                let state = if playing || moving { "PLAY" } else { "STOP" };
                println!(
                    "transport[{state}]: {:02}:{:02}:{:02}:{:02} @ {}fps [{}] | bar {} beat {:.3} tick {}",
                    pos.hours,
                    pos.minutes,
                    pos.seconds,
                    pos.frames,
                    pos.fps,
                    pos.source,
                    bar,
                    beat,
                    tick
                );
                snapshots.push(TransportSnapshot {
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    position: pos,
                    bar,
                    beat,
                    tick,
                });
            }
            last_print = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(8));
    }

    let report = TransportMonitorReport {
        midi_in_port: port_name,
        duration_sec,
        snapshot_count: snapshots.len(),
        last: snapshots.last().cloned(),
    };

    if report.snapshot_count == 0 {
        if signal_events == 0 {
            println!(
                "transport_monitor: no transport MIDI received (check DAW transmit + network port)"
            );
        } else {
            println!("transport_monitor: transport MIDI received but no position could be derived");
        }
    }

    if let Some(path) = json_out {
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("transport_monitor: report written {}", path.display());
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_transport_seek(
    midi_out_hint: Option<&str>,
    midi_in_hint: Option<&str>,
    target_seconds: f64,
    grid: MusicalGrid,
    tolerance_ms: u64,
    max_steps: u32,
) -> Result<()> {
    let mut io = TransportIo::connect(midi_out_hint, midi_in_hint)?;
    let tolerance_sec = (tolerance_ms as f64 / 1000.0).max(0.01);

    let mut current = io.wait_for_position(Duration::from_secs(2))?;
    let mut delta = target_seconds - current.to_seconds();
    let mut previous_delta = delta;

    println!(
        "transport_seek: current={:.3}s target={:.3}s delta={:+.3}s",
        current.to_seconds(),
        target_seconds,
        delta
    );

    for _ in 0..max_steps.max(1) {
        if delta.abs() <= tolerance_sec {
            let (bar, beat, tick) = seconds_to_bar_beat_tick(current.to_seconds(), grid);
            println!(
                "transport_seek: locked at {:02}:{:02}:{:02}:{:02} | bar {} beat {:.3} tick {}",
                current.hours, current.minutes, current.seconds, current.frames, bar, beat, tick
            );
            return Ok(());
        }

        // Adaptive pulse sizing:
        // - proportional on large deltas
        // - very short near target
        // - damp hard if we crossed target (sign flip)
        let mut hold_ms = (delta.abs() * 40.0).round().clamp(14.0, 900.0) as u64;
        if delta.abs() <= 2.0 {
            hold_ms = (delta.abs() * 20.0).round().clamp(10.0, 80.0) as u64;
        }
        if delta.signum() != previous_delta.signum() {
            hold_ms = hold_ms.min(20);
        }
        let action = if delta > 0.0 {
            NavAction::FastForward
        } else {
            NavAction::Rewind
        };
        io.pulse_nav(action, hold_ms)?;
        std::thread::sleep(Duration::from_millis(60));
        current = io.wait_for_position(Duration::from_millis(900))?;
        previous_delta = delta;
        delta = target_seconds - current.to_seconds();
    }

    Err(anyhow!(
        "transport_seek_not_converged final_delta_sec={:+.3}",
        delta
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn run_transport_step(
    midi_out_hint: Option<&str>,
    midi_in_hint: Option<&str>,
    beats_delta: f64,
    grid: MusicalGrid,
    tolerance_ms: u64,
    max_steps: u32,
) -> Result<()> {
    let mut io = TransportIo::connect(midi_out_hint, midi_in_hint)?;
    let current = io.wait_for_position(Duration::from_secs(2))?;
    let current_secs = current.to_seconds();
    let quarter_notes_delta = beats_delta * (4.0 / grid.ts_den as f64);
    let target_seconds = current_secs + (quarter_notes_delta * 60.0 / grid.bpm.max(1.0));

    run_transport_seek(
        midi_out_hint,
        midi_in_hint,
        target_seconds.max(0.0),
        grid,
        tolerance_ms,
        max_steps,
    )
}

pub fn run_transport_home(
    midi_out_hint: Option<&str>,
    midi_in_hint: Option<&str>,
    hold_ms: u64,
    max_steps: u32,
    home_floor_sec: Option<f64>,
) -> Result<TransportPosition> {
    let mut io = TransportIo::connect(midi_out_hint, midi_in_hint)?;
    let mut current = io.wait_for_position(Duration::from_secs(2))?;
    let mut stalled = 0u32;
    for _ in 0..max_steps.max(1) {
        let prev_sec = current.to_seconds();
        io.pulse_nav(NavAction::Rewind, hold_ms.max(60))?;
        std::thread::sleep(Duration::from_millis(80));
        current = io.wait_for_position(Duration::from_millis(900))?;
        let now_sec = current.to_seconds();
        if let Some(floor) = home_floor_sec {
            if now_sec <= floor + 0.20 {
                println!(
                    "transport_home: floor lock at {:02}:{:02}:{:02}:{:02} @ {}fps",
                    current.hours, current.minutes, current.seconds, current.frames, current.fps
                );
                return Ok(current);
            }
        }
        if now_sec >= prev_sec - 0.02 {
            stalled += 1;
        } else {
            stalled = 0;
        }
        if stalled >= 3 {
            println!(
                "transport_home: locked at {:02}:{:02}:{:02}:{:02} @ {}fps",
                current.hours, current.minutes, current.seconds, current.frames, current.fps
            );
            return Ok(current);
        }
    }
    Err(anyhow!("transport_home_not_converged"))
}

pub fn resolve_target_seconds(
    target_bar: Option<u32>,
    target_beat: Option<f64>,
    target_tick: Option<u32>,
    target_marker: Option<&str>,
    markers_file: Option<&Path>,
    grid: MusicalGrid,
) -> Result<f64> {
    if let Some(name) = target_marker {
        let mf = load_markers(markers_file)?;
        let marker = mf
            .markers
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow!("marker_not_found:{name}"))?;
        return Ok(bar_beat_tick_to_seconds(
            marker.bar,
            marker.beat,
            marker.tick,
            grid,
        ));
    }

    let bar = target_bar.ok_or_else(|| anyhow!("target_bar_required_without_marker"))?;
    let beat = target_beat.unwrap_or(1.0);
    let tick = target_tick.unwrap_or(0);
    Ok(bar_beat_tick_to_seconds(bar, beat, tick, grid))
}

pub fn load_markers(path: Option<&Path>) -> Result<MarkerFile> {
    let p = path.ok_or_else(|| anyhow!("markers_file_required_for_marker_lookup"))?;
    let text =
        fs::read_to_string(p).with_context(|| format!("cannot read markers: {}", p.display()))?;
    let mf: MarkerFile = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid marker yaml: {}", p.display()))?;
    Ok(mf)
}

pub fn bar_beat_tick_to_seconds(bar: u32, beat: f64, tick: u32, grid: MusicalGrid) -> f64 {
    let bars = bar.saturating_sub(1) as f64;
    let beats = (beat - 1.0).max(0.0);
    let score_beats = bars * grid.ts_num as f64 + beats;
    let quarter_notes = score_beats * (4.0 / grid.ts_den as f64) + tick as f64 / grid.ppq as f64;
    grid.start_offset_sec + quarter_notes * 60.0 / grid.bpm.max(1.0)
}

pub fn seconds_to_bar_beat_tick(seconds: f64, grid: MusicalGrid) -> (u32, f64, u32) {
    let rel_sec = (seconds - grid.start_offset_sec).max(0.0);
    let quarter_notes = rel_sec * grid.bpm.max(1.0) / 60.0;
    let score_beats_total = quarter_notes / (4.0 / grid.ts_den as f64);
    let bar_index = (score_beats_total / grid.ts_num as f64).floor();
    let beat_in_bar = score_beats_total - (bar_index * grid.ts_num as f64);
    let beat_floor = beat_in_bar.floor();
    let beat = 1.0 + beat_in_bar;
    let tick = ((beat_in_bar - beat_floor) * grid.ppq as f64).round() as u32;
    ((bar_index as u32) + 1, beat, tick)
}

fn rate_code_to_fps(rate_code: u8) -> f32 {
    match rate_code {
        0 => 24.0,
        1 => 25.0,
        2 => 29.97,
        _ => 30.0,
    }
}

fn parse_full_frame_sysex(msg: &[u8]) -> Option<TransportPosition> {
    if msg.len() < 10 {
        return None;
    }
    if msg[0] != 0xF0 || msg[msg.len() - 1] != 0xF7 {
        return None;
    }
    if msg.get(3).copied()? != 0x01 || msg.get(4).copied()? != 0x01 {
        return None;
    }

    let hr = *msg.get(5)?;
    let minutes = *msg.get(6)?;
    let seconds = *msg.get(7)?;
    let frames = *msg.get(8)?;
    let rate_code = (hr >> 5) & 0x03;
    let fps = rate_code_to_fps(rate_code);
    let hours = hr & 0x1F;

    Some(TransportPosition {
        hours,
        minutes,
        seconds,
        frames,
        fps,
        source: "mtc_full_frame".to_string(),
        updated_at_utc: Utc::now().to_rfc3339(),
    })
}

fn parse_midi_message(msg: &[u8], qf: &mut QfParser) -> Option<TransportPosition> {
    if msg.is_empty() {
        return None;
    }
    match msg[0] {
        0xF1 if msg.len() >= 2 => qf.ingest_qf(msg[1]),
        0xF0 => parse_full_frame_sysex(msg),
        _ => None,
    }
}

fn parse_transport_event(msg: &[u8], qf: &mut QfParser) -> Option<InEvent> {
    if msg.is_empty() {
        return None;
    }
    if let Some(pos) = parse_midi_message(msg, qf) {
        return Some(InEvent::Position(pos));
    }
    match msg[0] {
        0xFA => Some(InEvent::Start),
        0xFB => Some(InEvent::Continue),
        0xFC => Some(InEvent::Stop),
        0xF8 => Some(InEvent::Clock),
        0xF2 if msg.len() >= 3 => {
            let lsb = msg[1] as u16;
            let msb = msg[2] as u16;
            Some(InEvent::SongPosition((msb << 7) | lsb))
        }
        _ => None,
    }
}

fn seconds_to_transport_position(seconds: f64, fps: f32, source: &str) -> TransportPosition {
    let sec = seconds.max(0.0);
    let hours = (sec / 3600.0).floor() as u8;
    let rem1 = sec - (hours as f64 * 3600.0);
    let minutes = (rem1 / 60.0).floor() as u8;
    let rem2 = rem1 - (minutes as f64 * 60.0);
    let whole_seconds = rem2.floor() as u8;
    let frac = rem2 - whole_seconds as f64;
    let frames = (frac * fps.max(1.0) as f64).round().clamp(0.0, 255.0) as u8;
    TransportPosition {
        hours,
        minutes,
        seconds: whole_seconds,
        frames,
        fps,
        source: source.to_string(),
        updated_at_utc: Utc::now().to_rfc3339(),
    }
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
        .find(|(_, n)| n.to_lowercase().contains("network session"))
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
        .find(|(_, n)| n.to_lowercase().contains("network session"))
    {
        return Ok(idx);
    }

    Ok(0)
}

fn connect_input_for_transport(
    mut input: MidiInput,
    port: &midir::MidiInputPort,
    tx: mpsc::Sender<InEvent>,
) -> Result<MidiInputConnection<()>> {
    input.ignore(Ignore::None);
    let mut qf = QfParser::default();
    let conn = input.connect(
        port,
        "mixct-transport-in",
        move |_stamp, message, _| {
            if let Some(ev) = parse_transport_event(message, &mut qf) {
                let _ = tx.send(ev);
            }
        },
        (),
    )?;
    Ok(conn)
}

struct TransportIo {
    _in_conn: MidiInputConnection<()>,
    out_conn: MidiOutputConnection,
    rx: mpsc::Receiver<InEvent>,
    last_pos: Option<TransportPosition>,
}

impl TransportIo {
    fn connect(midi_out_hint: Option<&str>, midi_in_hint: Option<&str>) -> Result<Self> {
        let input = MidiInput::new("mixct-transport-io-in")?;
        let in_ports = input.ports();
        if in_ports.is_empty() {
            return Err(anyhow!("no_midi_input_ports_found"));
        }
        let in_idx = select_input_port_index(&input, midi_in_hint)?;
        let in_port = in_ports
            .get(in_idx)
            .ok_or_else(|| anyhow!("invalid_midi_input_port_index"))?;

        let output = MidiOutput::new("mixct-transport-io-out")?;
        let out_ports = output.ports();
        if out_ports.is_empty() {
            return Err(anyhow!("no_midi_output_ports_found"));
        }
        let out_idx = select_output_port_index(&output, midi_out_hint)?;
        let out_port = out_ports
            .get(out_idx)
            .ok_or_else(|| anyhow!("invalid_midi_output_port_index"))?;

        let (tx, rx) = mpsc::channel::<InEvent>();
        let in_conn = connect_input_for_transport(input, in_port, tx)?;
        let out_conn = output.connect(out_port, "mixct-transport-out")?;
        Ok(Self {
            _in_conn: in_conn,
            out_conn,
            rx,
            last_pos: None,
        })
    }

    fn wait_for_position(&mut self, timeout: Duration) -> Result<TransportPosition> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(ev) = self.rx.recv_timeout(Duration::from_millis(60)) {
                if let InEvent::Position(pos) = ev {
                    self.last_pos = Some(pos.clone());
                    return Ok(pos);
                }
            }
        }
        self.last_pos
            .clone()
            .ok_or_else(|| anyhow!("transport_position_timeout"))
    }

    fn pulse_nav(&mut self, action: NavAction, hold_ms: u64) -> Result<()> {
        let note = match action {
            NavAction::Rewind => 0x5B_u8,
            NavAction::FastForward => 0x5C_u8,
        };
        self.out_conn.send(&[0x90, note, 0x7F])?;
        std::thread::sleep(Duration::from_millis(hold_ms.max(20)));
        self.out_conn.send(&[0x90, note, 0x00])?;
        Ok(())
    }
}
