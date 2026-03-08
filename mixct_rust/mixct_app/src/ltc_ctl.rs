use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, SupportedStreamConfigRange};
use ltc::{FramerateEstimate, LTCDecoder};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct LtcFrameSnapshot {
    pub elapsed_ms: u64,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub frame: u8,
    pub fps_estimate: f32,
    pub drop_frame: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LtcMonitorReport {
    pub device_name: String,
    pub channel_1_based: usize,
    pub sample_rate_hz: u32,
    pub duration_sec: u64,
    pub frame_count: usize,
    pub unique_positions: usize,
    pub first: Option<LtcFrameSnapshot>,
    pub last: Option<LtcFrameSnapshot>,
}

pub fn run_ltc_monitor(
    device_hint: Option<&str>,
    channel_1_based: usize,
    duration_sec: u64,
    sample_rate_hz: Option<u32>,
    expected_fps: f32,
    json_out: Option<&Path>,
) -> Result<LtcMonitorReport> {
    if duration_sec == 0 {
        return Err(anyhow!("duration_sec_must_be_positive"));
    }
    if !(1..=512).contains(&channel_1_based) {
        return Err(anyhow!("invalid_channel_1_based"));
    }
    if expected_fps < 1.0 {
        return Err(anyhow!("invalid_expected_fps"));
    }

    let host = cpal::default_host();
    let device = select_input_device(&host, device_hint)?;
    let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    let (stream_config, sample_format) = choose_stream_config(&device, sample_rate_hz)?;
    let channels = stream_config.channels as usize;
    if channel_1_based > channels {
        return Err(anyhow!(
            "requested_channel_out_of_range requested={} available={}",
            channel_1_based,
            channels
        ));
    }
    let sr = stream_config.sample_rate.0;
    let channel_idx = channel_1_based - 1;
    let samples_per_frame = sr as f32 / expected_fps;
    let mut decoder = LTCDecoder::with_capacity(samples_per_frame, 128);

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let err_fn = |err| eprintln!("ltc_monitor_stream_error={err}");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                let _ = tx.send(data.to_vec());
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let mapped: Vec<f32> = data.iter().map(|v| *v as f32 / i16::MAX as f32).collect();
                let _ = tx.send(mapped);
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
                let _ = tx.send(mapped);
            },
            err_fn,
            None,
        )?,
        _ => return Err(anyhow!("unsupported_sample_format")),
    };

    println!(
        "ltc_monitor: device='{}' sr={} ch={} source_ch={} expected_fps={}",
        device_name, sr, channels, channel_1_based, expected_fps
    );
    stream.play()?;

    let start = Instant::now();
    let mut frame_count = 0usize;
    let mut unique_positions = 0usize;
    let mut last_tuple: Option<(u8, u8, u8, u8)> = None;
    let mut first: Option<LtcFrameSnapshot> = None;
    let mut last: Option<LtcFrameSnapshot> = None;
    let mut last_print = Instant::now();

    while start.elapsed() < Duration::from_secs(duration_sec) {
        let Ok(chunk) = rx.recv_timeout(Duration::from_millis(40)) else {
            continue;
        };
        if chunk.len() < channels {
            continue;
        }
        let frame_n = chunk.len() / channels;
        if frame_n == 0 {
            continue;
        }
        let mut mono = Vec::with_capacity(frame_n);
        for i in 0..frame_n {
            let idx = i * channels + channel_idx;
            mono.push(chunk.get(idx).copied().unwrap_or(0.0));
        }

        if decoder.write_samples(&mono) {
            for frame in &mut decoder {
                let fps_estimate = match frame.estimate_framerate(sr as f32) {
                    FramerateEstimate::F24 => 24.0,
                    FramerateEstimate::F25 => 25.0,
                    FramerateEstimate::F30 => 30.0,
                    FramerateEstimate::Unknown(v) => v,
                };
                let snap = LtcFrameSnapshot {
                    elapsed_ms: start.elapsed().as_millis() as u64,
                    hour: frame.hour,
                    minute: frame.minute,
                    second: frame.second,
                    frame: frame.frame,
                    fps_estimate,
                    drop_frame: frame.drop_frame,
                };
                frame_count += 1;
                let tuple = (snap.hour, snap.minute, snap.second, snap.frame);
                if last_tuple != Some(tuple) {
                    unique_positions += 1;
                    last_tuple = Some(tuple);
                }
                if first.is_none() {
                    first = Some(snap.clone());
                }
                last = Some(snap.clone());
                if last_print.elapsed() >= Duration::from_millis(120) {
                    println!(
                        "ltc: {:02}:{:02}:{:02}:{:02} fps~{:.2} drop={}",
                        snap.hour,
                        snap.minute,
                        snap.second,
                        snap.frame,
                        snap.fps_estimate,
                        snap.drop_frame
                    );
                    last_print = Instant::now();
                }
            }
        }
    }

    drop(stream);

    let report = LtcMonitorReport {
        device_name,
        channel_1_based,
        sample_rate_hz: sr,
        duration_sec,
        frame_count,
        unique_positions,
        first,
        last,
    };

    if let Some(path) = json_out {
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("ltc_monitor: report written {}", path.display());
    }
    if report.frame_count == 0 {
        println!("ltc_monitor: no LTC frames decoded");
    }
    Ok(report)
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
