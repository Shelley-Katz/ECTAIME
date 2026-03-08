use anyhow::{anyhow, Context, Result};
use midir::{MidiOutput, MidiOutputConnection};
use mixct_control::ControlBackend;
use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;

#[derive(Debug, Clone)]
pub struct MidiTargetSpec {
    pub channel: u8,             // 1..=16
    pub cc: u8,                  // 0..=127
    pub mcu_channel: Option<u8>, // 1..=16
    pub min_db: f32,
    pub max_db: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum WriteProtocol {
    CcLearn,
    McuFader,
}

#[derive(Debug, Clone)]
pub struct UndoTrigger {
    pub channel: u8, // 1..=16
    pub cc: u8,      // 0..=127
    pub value: u8,   // 0..=127
}

pub struct MidiBackend {
    conn: MidiOutputConnection,
    targets: HashMap<String, MidiTargetSpec>,
    protocol: WriteProtocol,
    touch_started_at: Option<Instant>,
    undo_primary: Option<UndoTrigger>,
    undo_fallback: Option<UndoTrigger>,
}

impl MidiBackend {
    pub fn connect(
        midi_out_name: Option<&str>,
        targets: HashMap<String, MidiTargetSpec>,
        protocol: WriteProtocol,
        undo_primary: Option<UndoTrigger>,
        undo_fallback: Option<UndoTrigger>,
    ) -> Result<Self> {
        let out = MidiOutput::new("mixct-midi-out")?;
        let ports = out.ports();
        if ports.is_empty() {
            return Err(anyhow!("no_midi_output_ports_found"));
        }

        let selected_idx = select_port_index(&out, midi_out_name)
            .with_context(|| "failed to select MIDI output port")?;
        let port = ports
            .get(selected_idx)
            .ok_or_else(|| anyhow!("invalid_midi_port_index"))?;
        let port_name = out
            .port_name(port)
            .unwrap_or_else(|_| "<unknown>".to_string());
        let conn = out
            .connect(port, "mixct-mcu-cc")
            .with_context(|| format!("failed to connect MIDI out: {port_name}"))?;

        info!("midi_backend_connected port={port_name}");
        Ok(Self {
            conn,
            targets,
            protocol,
            touch_started_at: None,
            undo_primary,
            undo_fallback,
        })
    }

    fn send_cc(&mut self, channel: u8, cc: u8, value: u8) -> Result<()> {
        if !(1..=16).contains(&channel) {
            return Err(anyhow!("invalid_midi_channel:{channel}"));
        }
        let status = 0xB0 | ((channel - 1) & 0x0F);
        self.conn
            .send(&[status, cc, value])
            .with_context(|| format!("failed to send CC ch={channel} cc={cc} value={value}"))?;
        Ok(())
    }

    fn send_pitch_bend(&mut self, channel: u8, value_14bit: u16) -> Result<()> {
        if !(1..=16).contains(&channel) {
            return Err(anyhow!("invalid_midi_channel:{channel}"));
        }
        let status = 0xE0 | ((channel - 1) & 0x0F);
        let v = value_14bit.min(16383);
        let lsb = (v & 0x7F) as u8;
        let msb = ((v >> 7) & 0x7F) as u8;
        self.conn
            .send(&[status, lsb, msb])
            .with_context(|| format!("failed to send pitchbend ch={channel} value={v}"))?;
        Ok(())
    }

    fn resolve_target(&self, target: &str) -> Result<&MidiTargetSpec> {
        let (target_id, _lane) = target
            .split_once("::")
            .ok_or_else(|| anyhow!("invalid_target_format:{target}"))?;
        let spec = self
            .targets
            .get(target_id)
            .ok_or_else(|| anyhow!("unknown_target:{target_id}"))?;
        Ok(spec)
    }
}

pub fn list_output_ports() -> Result<Vec<String>> {
    let out = MidiOutput::new("mixct-midi-list")?;
    let ports = out.ports();
    let names = ports
        .iter()
        .map(|p| out.port_name(p).unwrap_or_else(|_| "<unknown>".to_string()))
        .collect();
    Ok(names)
}

pub fn run_cc_sweep(
    midi_out_name: Option<&str>,
    channel: u8,
    cc: u8,
    min: u8,
    max: u8,
    step: u8,
    interval_ms: u64,
    cycles: Option<u32>,
) -> Result<()> {
    if !(1..=16).contains(&channel) {
        return Err(anyhow!("invalid_midi_channel:{channel}"));
    }
    if min >= max {
        return Err(anyhow!("invalid_range:min_must_be_less_than_max"));
    }
    if step == 0 {
        return Err(anyhow!("invalid_step:step_must_be_nonzero"));
    }

    let out = MidiOutput::new("mixct-cc-sweep")?;
    let ports = out.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_output_ports_found"));
    }
    let selected_idx = select_port_index(&out, midi_out_name)
        .with_context(|| "failed to select MIDI output port")?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_port_index"))?;
    let port_name = out
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());
    let mut conn = out
        .connect(port, "mixct-cc-sweep")
        .with_context(|| format!("failed to connect MIDI out: {port_name}"))?;

    info!(
        "cc_sweep_started port={} ch={} cc={} min={} max={} step={} interval_ms={} cycles={:?}",
        port_name, channel, cc, min, max, step, interval_ms, cycles
    );
    println!(
        "cc_sweep: started port='{}' ch={} cc={} range={}..{} step={} interval_ms={} cycles={:?}",
        port_name, channel, cc, min, max, step, interval_ms, cycles
    );

    let mut completed = 0u32;
    loop {
        sweep_once(&mut conn, channel, cc, min, max, step, interval_ms)?;
        completed += 1;
        if let Some(limit) = cycles {
            if completed >= limit {
                break;
            }
        }
    }

    println!("cc_sweep: finished cycles={completed}");
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum McuTransportAction {
    Play,
    Stop,
    Rewind,
    FastForward,
}

pub fn run_mcu_transport(
    midi_out_name: Option<&str>,
    action: McuTransportAction,
    hold_ms: u64,
) -> Result<()> {
    let out = MidiOutput::new("mixct-mcu-transport")?;
    let ports = out.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_output_ports_found"));
    }
    let selected_idx = select_port_index(&out, midi_out_name)
        .with_context(|| "failed to select MIDI output port")?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_port_index"))?;
    let port_name = out
        .port_name(port)
        .unwrap_or_else(|_| "<unknown>".to_string());
    let mut conn = out
        .connect(port, "mixct-mcu-transport")
        .with_context(|| format!("failed to connect MIDI out: {port_name}"))?;

    let (note, mmc_cmd, label) = match action {
        McuTransportAction::Play => (0x5E_u8, 0x02_u8, "play"), // MCU Play, MMC Play
        McuTransportAction::Stop => (0x5D_u8, 0x01_u8, "stop"), // MCU Stop, MMC Stop
        McuTransportAction::Rewind => (0x5B_u8, 0x05_u8, "rewind"), // MCU Rewind, MMC Rewind
        McuTransportAction::FastForward => (0x5C_u8, 0x04_u8, "fast-forward"), // MCU FF, MMC FF
    };

    // Mackie Control transport button press/release (ch1 note-on).
    conn.send(&[0x90, note, 0x7F])?;
    thread::sleep(Duration::from_millis(hold_ms.max(30)));
    conn.send(&[0x90, note, 0x00])?;

    // Also emit MMC as compatibility fallback for DAW transport.
    // Universal realtime SysEx: F0 7F <dev> 06 <cmd> F7
    conn.send(&[0xF0, 0x7F, 0x7F, 0x06, mmc_cmd, 0xF7])?;

    println!("mcu_transport: {} sent on '{}'", label, port_name);
    Ok(())
}

fn sweep_once(
    conn: &mut MidiOutputConnection,
    channel: u8,
    cc: u8,
    min: u8,
    max: u8,
    step: u8,
    interval_ms: u64,
) -> Result<()> {
    let status = 0xB0 | ((channel - 1) & 0x0F);

    let mut v = min;
    loop {
        conn.send(&[status, cc, v])?;
        thread::sleep(Duration::from_millis(interval_ms));
        if v >= max {
            break;
        }
        let next = v.saturating_add(step);
        v = if next > max { max } else { next };
    }

    let mut v = max;
    loop {
        conn.send(&[status, cc, v])?;
        thread::sleep(Duration::from_millis(interval_ms));
        if v <= min {
            break;
        }
        let next = v.saturating_sub(step);
        v = if next < min { min } else { next };
    }

    Ok(())
}

impl ControlBackend for MidiBackend {
    fn begin_touch(&mut self, _target: &str) -> Result<()> {
        self.touch_started_at = Some(Instant::now());
        Ok(())
    }

    fn write_value(&mut self, target: &str, value: f32, at_ms: u64) -> Result<()> {
        let spec = self.resolve_target(target)?;

        if let Some(start) = self.touch_started_at {
            let due = start + Duration::from_millis(at_ms);
            let now = Instant::now();
            if due > now {
                thread::sleep(due - now);
            }
        }

        let db = value.clamp(spec.min_db, spec.max_db);
        let span = (spec.max_db - spec.min_db).max(0.0001);
        let norm = (db - spec.min_db) / span;
        match self.protocol {
            WriteProtocol::CcLearn => {
                let midi_value = (norm * 127.0).round().clamp(0.0, 127.0) as u8;
                self.send_cc(spec.channel, spec.cc, midi_value)?;
            }
            WriteProtocol::McuFader => {
                let ch = spec
                    .mcu_channel
                    .ok_or_else(|| anyhow!("mcu_channel_missing_for_target"))?;
                let bend = (norm * 16383.0).round().clamp(0.0, 16383.0) as u16;
                self.send_pitch_bend(ch, bend)?;
            }
        }
        Ok(())
    }

    fn end_touch(&mut self, _target: &str) -> Result<()> {
        self.touch_started_at = None;
        Ok(())
    }

    fn trigger_undo_primary(&mut self) -> Result<()> {
        let trig = self
            .undo_primary
            .clone()
            .ok_or_else(|| anyhow!("undo_primary_cc_not_configured"))?;
        self.send_cc(trig.channel, trig.cc, trig.value)
    }

    fn trigger_undo_fallback(&mut self) -> Result<()> {
        let trig = self
            .undo_fallback
            .clone()
            .ok_or_else(|| anyhow!("undo_fallback_cc_not_configured"))?;
        self.send_cc(trig.channel, trig.cc, trig.value)
    }
}

fn select_port_index(out: &MidiOutput, hint: Option<&str>) -> Result<usize> {
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
