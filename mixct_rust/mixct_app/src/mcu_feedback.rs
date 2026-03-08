use anyhow::{anyhow, Result};
use chrono::Utc;
use midir::{Ignore, MidiInput};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct FeedbackState {
    pub target: String,
    pub mcu_channel: u8,
    pub value_14bit: u16,
    pub normalized: f32,
    pub db_estimate: f32,
    pub updated_at_utc: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedbackReport {
    pub port_name: String,
    pub duration_sec: u64,
    pub event_count: usize,
    pub targets: Vec<FeedbackState>,
}

#[derive(Debug, Clone)]
struct FeedbackEvent {
    channel: u8, // 1..=16
    value_14bit: u16,
    received_at: Instant,
}

pub fn list_input_ports() -> Result<Vec<String>> {
    let inp = MidiInput::new("mixct-midi-input-list")?;
    let ports = inp.ports();
    Ok(ports
        .iter()
        .map(|p| inp.port_name(p).unwrap_or_else(|_| "<unknown>".to_string()))
        .collect())
}

pub fn monitor_mcu_feedback(
    midi_in_hint: Option<&str>,
    duration_sec: u64,
    poll_ms: u64,
    mcu_channel_to_target: HashMap<u8, String>,
    target_db_ranges: HashMap<String, (f32, f32)>,
    json_out: Option<&Path>,
) -> Result<()> {
    let mut inp = MidiInput::new("mixct-mcu-feedback")?;
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

    let (tx, rx) = mpsc::channel::<FeedbackEvent>();
    let _conn = inp.connect(
        port,
        "mixct-mcu-feedback-monitor",
        move |_stamp, message, _| {
            if let Some(ev) = parse_pitch_bend(message) {
                let _ = tx.send(FeedbackEvent {
                    channel: ev.0,
                    value_14bit: ev.1,
                    received_at: Instant::now(),
                });
            }
        },
        (),
    )?;

    println!(
        "mcu_monitor: listening on '{}' for {}s",
        port_name, duration_sec
    );

    let mut states: HashMap<String, FeedbackState> = HashMap::new();
    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut event_count = 0usize;

    while start.elapsed() < Duration::from_secs(duration_sec) {
        while let Ok(ev) = rx.try_recv() {
            event_count += 1;
            if let Some(target) = mcu_channel_to_target.get(&ev.channel) {
                let normalized = ev.value_14bit as f32 / 16383.0;
                let (min_db, max_db) = target_db_ranges
                    .get(target)
                    .copied()
                    .unwrap_or((-18.0, 6.0));
                let db_estimate = min_db + normalized * (max_db - min_db);
                states.insert(
                    target.clone(),
                    FeedbackState {
                        target: target.clone(),
                        mcu_channel: ev.channel,
                        value_14bit: ev.value_14bit,
                        normalized,
                        db_estimate,
                        updated_at_utc: Utc::now().to_rfc3339(),
                    },
                );
            }
            let _ = ev.received_at;
        }

        if last_print.elapsed() >= Duration::from_millis(poll_ms) {
            print_snapshot(&states);
            last_print = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(8));
    }

    println!("mcu_monitor: finished events={event_count}");

    if let Some(path) = json_out {
        let mut out: Vec<FeedbackState> = states.into_values().collect();
        out.sort_by(|a, b| a.mcu_channel.cmp(&b.mcu_channel));
        let report = FeedbackReport {
            port_name,
            duration_sec,
            event_count,
            targets: out,
        };
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("mcu_monitor: report written {}", path.display());
    }

    Ok(())
}

fn parse_pitch_bend(msg: &[u8]) -> Option<(u8, u16)> {
    if msg.len() < 3 {
        return None;
    }
    let status = msg[0];
    if status & 0xF0 != 0xE0 {
        return None;
    }
    let channel = (status & 0x0F) + 1;
    let lsb = msg[1] as u16;
    let msb = msg[2] as u16;
    let value = (msb << 7) | lsb;
    Some((channel, value))
}

fn print_snapshot(states: &HashMap<String, FeedbackState>) {
    if states.is_empty() {
        return;
    }
    let mut items: Vec<&FeedbackState> = states.values().collect();
    items.sort_by(|a, b| a.mcu_channel.cmp(&b.mcu_channel));
    let line = items
        .iter()
        .map(|s| format!("{}(ch{}):{:+.2}dB", s.target, s.mcu_channel, s.db_estimate))
        .collect::<Vec<_>>()
        .join(" | ");
    println!("mcu_monitor: {line}");
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
