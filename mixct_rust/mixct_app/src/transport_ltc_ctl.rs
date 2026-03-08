use crate::transport_ctl::{resolve_target_seconds, seconds_to_bar_beat_tick, MusicalGrid};
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, SupportedStreamConfigRange};
use ltc::LTCDecoder;
use midir::{MidiOutput, MidiOutputConnection};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct LtcPosition {
    hour: u8,
    minute: u8,
    second: u8,
    frame: u8,
    fps: f32,
}

impl LtcPosition {
    fn to_seconds(&self) -> f64 {
        let base = self.hour as f64 * 3600.0 + self.minute as f64 * 60.0 + self.second as f64;
        base + (self.frame as f64 / self.fps.max(1.0) as f64)
    }
}

#[derive(Debug, Clone, Copy)]
enum NavAction {
    Rewind,
    FastForward,
}

pub fn run_transport_seek_ltc(
    midi_out_hint: Option<&str>,
    ltc_device_hint: Option<&str>,
    ltc_channel_1_based: usize,
    sample_rate_hz: Option<u32>,
    fps: f32,
    target_seconds: f64,
    grid: MusicalGrid,
    tolerance_ms: u64,
    max_steps: u32,
) -> Result<()> {
    let mut io = TransportLtcIo::connect(
        midi_out_hint,
        ltc_device_hint,
        ltc_channel_1_based,
        sample_rate_hz,
        fps,
    )?;
    let tolerance_sec = (tolerance_ms as f64 / 1000.0).max(0.01);

    let mut current = io.wait_for_position(Duration::from_secs(2))?;
    let mut delta = target_seconds - current.to_seconds();
    let mut previous_delta = delta;

    println!(
        "transport_seek_ltc: current={:.3}s target={:.3}s delta={:+.3}s",
        current.to_seconds(),
        target_seconds,
        delta
    );

    // Coarse forward travel is more reliable via PLAY/STOP than FF nudges on DP.
    if delta > tolerance_sec {
        // Empirical DP + network transport stop latency compensation.
        // We stop slightly before target to avoid systematic late lock.
        let stop_lead_sec = 0.26_f64;
        io.send_play()?;
        let forward_deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < forward_deadline {
            current = io.wait_for_position(Duration::from_millis(800))?;
            delta = target_seconds - current.to_seconds();
            if delta <= stop_lead_sec.max(tolerance_sec) {
                break;
            }
        }
        io.send_stop()?;
        std::thread::sleep(Duration::from_millis(120));
        current = io.wait_for_position(Duration::from_millis(900))?;
        delta = target_seconds - current.to_seconds();
    }

    for _ in 0..max_steps.max(1) {
        if delta.abs() <= tolerance_sec {
            let (bar, beat, tick) = seconds_to_bar_beat_tick(current.to_seconds(), grid);
            println!(
                "transport_seek_ltc: locked at {:02}:{:02}:{:02}:{:02} delta_sec={:+.3} | bar {} beat {:.3} tick {}",
                current.hour, current.minute, current.second, current.frame, delta, bar, beat, tick
            );
            return Ok(());
        }

        // DP transport FF/REW is non-linear and can jump far on long pulses.
        // Keep pulses short and bounded; use many nudges instead of big holds.
        let mut hold_ms = if delta.abs() > 40.0 {
            75
        } else if delta.abs() > 20.0 {
            65
        } else if delta.abs() > 10.0 {
            55
        } else if delta.abs() > 3.0 {
            40
        } else if delta.abs() > 1.0 {
            28
        } else {
            20
        };
        if delta.signum() != previous_delta.signum() {
            // Crossed target: immediately switch to tiny nudges.
            hold_ms = 18;
        } else if delta.abs() < 0.6 {
            hold_ms = 16;
        } else if delta.abs() < 0.25 {
            hold_ms = 12;
        } else if delta.abs() > 120.0 {
            // Very large reposition request: allow stronger coarse pulses.
            hold_ms = 90;
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
        "transport_seek_ltc_not_converged final_delta_sec={:+.3}",
        delta
    ))
}

pub fn run_transport_home_ltc(
    midi_out_hint: Option<&str>,
    ltc_device_hint: Option<&str>,
    ltc_channel_1_based: usize,
    sample_rate_hz: Option<u32>,
    fps: f32,
    hold_ms: u64,
    max_steps: u32,
    home_floor_sec: Option<f64>,
) -> Result<()> {
    let mut io = TransportLtcIo::connect(
        midi_out_hint,
        ltc_device_hint,
        ltc_channel_1_based,
        sample_rate_hz,
        fps,
    )?;
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
                    "transport_home_ltc: floor lock at {:02}:{:02}:{:02}:{:02}",
                    current.hour, current.minute, current.second, current.frame
                );
                return Ok(());
            }
            // In explicit floor mode, do not early-exit on transport stall;
            // continue pushing rewind until floor (or max steps) is reached.
            continue;
        }
        if now_sec >= prev_sec - 0.02 {
            stalled += 1;
        } else {
            stalled = 0;
        }
        if stalled >= 3 {
            println!(
                "transport_home_ltc: locked at {:02}:{:02}:{:02}:{:02}",
                current.hour, current.minute, current.second, current.frame
            );
            return Ok(());
        }
    }
    Err(anyhow!("transport_home_ltc_not_converged"))
}

pub fn resolve_target_seconds_ltc(
    target_bar: Option<u32>,
    target_beat: Option<f64>,
    target_tick: Option<u32>,
    target_marker: Option<&str>,
    markers_file: Option<&Path>,
    grid: MusicalGrid,
) -> Result<f64> {
    resolve_target_seconds(
        target_bar,
        target_beat,
        target_tick,
        target_marker,
        markers_file,
        grid,
    )
}

struct TransportLtcIo {
    _audio_stream: cpal::Stream,
    rx: mpsc::Receiver<LtcPosition>,
    out_conn: MidiOutputConnection,
    last_pos: Option<LtcPosition>,
}

impl TransportLtcIo {
    fn connect(
        midi_out_hint: Option<&str>,
        ltc_device_hint: Option<&str>,
        ltc_channel_1_based: usize,
        sample_rate_hz: Option<u32>,
        fps: f32,
    ) -> Result<Self> {
        let (audio_stream, rx) = connect_ltc_reader(
            ltc_device_hint,
            ltc_channel_1_based,
            sample_rate_hz,
            fps.max(1.0),
        )?;
        let out_conn = connect_midi_out(midi_out_hint)?;
        Ok(Self {
            _audio_stream: audio_stream,
            rx,
            out_conn,
            last_pos: None,
        })
    }

    fn wait_for_position(&mut self, timeout: Duration) -> Result<LtcPosition> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(pos) = self.rx.recv_timeout(Duration::from_millis(60)) {
                self.last_pos = Some(pos.clone());
                return Ok(pos);
            }
        }
        self.last_pos
            .clone()
            .ok_or_else(|| anyhow!("ltc_position_timeout"))
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

    fn send_play(&mut self) -> Result<()> {
        self.out_conn.send(&[0x90, 0x5E, 0x7F])?;
        self.out_conn.send(&[0x90, 0x5E, 0x00])?;
        Ok(())
    }

    fn send_stop(&mut self) -> Result<()> {
        self.out_conn.send(&[0x90, 0x5D, 0x7F])?;
        self.out_conn.send(&[0x90, 0x5D, 0x00])?;
        Ok(())
    }
}

fn connect_midi_out(midi_out_hint: Option<&str>) -> Result<MidiOutputConnection> {
    let out = MidiOutput::new("mixct-transport-ltc-out")?;
    let ports = out.ports();
    if ports.is_empty() {
        return Err(anyhow!("no_midi_output_ports_found"));
    }
    let selected_idx = select_output_port_index(&out, midi_out_hint)?;
    let port = ports
        .get(selected_idx)
        .ok_or_else(|| anyhow!("invalid_midi_output_port_index"))?;
    let conn = out.connect(port, "mixct-transport-ltc-out")?;
    Ok(conn)
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

fn connect_ltc_reader(
    device_hint: Option<&str>,
    channel_1_based: usize,
    sample_rate_hz: Option<u32>,
    fps: f32,
) -> Result<(cpal::Stream, mpsc::Receiver<LtcPosition>)> {
    let host = cpal::default_host();
    let device = select_input_device(&host, device_hint)?;
    let (stream_config, sample_format) = choose_stream_config(&device, sample_rate_hz)?;
    let channels = stream_config.channels as usize;
    if channel_1_based == 0 || channel_1_based > channels {
        return Err(anyhow!(
            "ltc_channel_out_of_range requested={} available={}",
            channel_1_based,
            channels
        ));
    }
    let sr = stream_config.sample_rate.0;
    let channel_idx = channel_1_based - 1;
    let mut decoder = LTCDecoder::with_capacity(sr as f32 / fps.max(1.0), 128);
    let (tx_pcm, rx_pcm) = mpsc::channel::<Vec<f32>>();
    let (tx_pos, rx_pos) = mpsc::channel::<LtcPosition>();
    let err_fn = |err| eprintln!("transport_ltc_stream_error={err}");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                let _ = tx_pcm.send(data.to_vec());
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let mapped: Vec<f32> = data.iter().map(|v| *v as f32 / i16::MAX as f32).collect();
                let _ = tx_pcm.send(mapped);
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                let mapped: Vec<f32> = data
                    .iter()
                    .map(|v| (*v as f32 / u16::MAX as f32) * 2.0 - 1.0)
                    .collect();
                let _ = tx_pcm.send(mapped);
            },
            err_fn,
            None,
        )?,
        _ => return Err(anyhow!("unsupported_sample_format")),
    };
    stream.play()?;

    std::thread::spawn(move || {
        while let Ok(chunk) = rx_pcm.recv() {
            if chunk.len() < channels {
                continue;
            }
            let frames = chunk.len() / channels;
            let mut mono = Vec::with_capacity(frames);
            for i in 0..frames {
                let idx = i * channels + channel_idx;
                mono.push(chunk.get(idx).copied().unwrap_or(0.0));
            }
            if decoder.write_samples(&mono) {
                for f in &mut decoder {
                    let _ = tx_pos.send(LtcPosition {
                        hour: f.hour,
                        minute: f.minute,
                        second: f.second,
                        frame: f.frame,
                        fps,
                    });
                }
            }
        }
    });

    Ok((stream, rx_pos))
}

fn select_input_device(host: &cpal::Host, hint: Option<&str>) -> Result<cpal::Device> {
    let mut devices = host
        .input_devices()
        .context("failed to enumerate input devices")?;
    if let Some(h) = hint {
        let needle = h.to_lowercase();
        for d in devices.by_ref() {
            let name = d.name().unwrap_or_default();
            if name.to_lowercase().contains(&needle) {
                return Ok(d);
            }
        }
        return Err(anyhow!("requested_audio_input_not_found:{h}"));
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no_default_input_device"))
}

fn choose_stream_config(
    device: &cpal::Device,
    requested_sample_rate_hz: Option<u32>,
) -> Result<(StreamConfig, SampleFormat)> {
    let ranges: Vec<SupportedStreamConfigRange> = device
        .supported_input_configs()
        .context("failed to query supported input configs")?
        .collect();
    if ranges.is_empty() {
        return Err(anyhow!("no_supported_input_configs"));
    }
    let mut candidates = ranges;
    candidates.sort_by_key(|r| {
        let is_f32 = r.sample_format() == SampleFormat::F32;
        let channels = r.channels();
        (!is_f32, std::cmp::Reverse(channels))
    });
    if let Some(req_sr) = requested_sample_rate_hz {
        for r in &candidates {
            if req_sr >= r.min_sample_rate().0 && req_sr <= r.max_sample_rate().0 {
                return Ok((
                    StreamConfig {
                        channels: r.channels(),
                        sample_rate: SampleRate(req_sr),
                        buffer_size: cpal::BufferSize::Default,
                    },
                    r.sample_format(),
                ));
            }
        }
    }
    let r = &candidates[0];
    let cfg = r.with_max_sample_rate();
    Ok((cfg.config(), cfg.sample_format()))
}
