use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig, SupportedStreamConfigRange};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFeatures {
    pub rms_db: f32,
    pub peak_db: f32,
    pub crest_db: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioBusMap {
    pub id: String,
    pub channels: Vec<usize>, // 1-based channel indexes
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioVirtualGroupMap {
    pub id: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMonitorRequest {
    pub device_hint: Option<String>,
    pub duration_sec: u64,
    pub sample_rate_hz: Option<u32>,
    pub window_ms: u64,
    pub hop_ms: u64,
    pub calibrate_sec: u64,
    pub buses: Vec<AudioBusMap>,
    pub virtual_groups: Vec<AudioVirtualGroupMap>,
    pub include_default_virtual_groups: bool,
    pub print_every_ms: u64,
    pub jsonl_out: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralMetrics {
    pub low_db: f32,
    pub mid_db: f32,
    pub high_db: f32,
    pub centroid_hz: f32,
    pub rolloff_hz: f32,
    pub flux: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CepstralMetrics {
    pub peak_quefrency_ms: f32,
    pub spread: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseMetrics {
    pub correlation: f32,
    pub decorrelation: f32,
    pub phasing_risk: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusFrameMetrics {
    pub bus_id: String,
    pub is_virtual_group: bool,
    pub members: Option<Vec<String>>,
    pub rms_db: f32,
    pub normalized_rms_db: f32,
    pub peak_db: f32,
    pub crest_db: f32,
    pub noise_floor_db: f32,
    pub gain_norm_db: f32,
    pub transient_strength: f32,
    pub transient_density_hz: f32,
    pub rt60_proxy_ms: Option<f32>,
    pub spectral: SpectralMetrics,
    pub cepstral: CepstralMetrics,
    pub phase: Option<PhaseMetrics>,
    pub combination_coherence: Option<f32>,
    pub combination_gain_db: Option<f32>,
    pub confidence: f32,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrameMetrics {
    pub ts_utc: String,
    pub elapsed_ms: u64,
    pub buses: Vec<BusFrameMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusSummary {
    pub bus_id: String,
    pub avg_rms_db: f32,
    pub avg_normalized_rms_db: f32,
    pub avg_peak_db: f32,
    pub avg_centroid_hz: f32,
    pub avg_transient_density_hz: f32,
    pub avg_confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMonitorReport {
    pub device_name: String,
    pub sample_rate_hz: u32,
    pub channels: usize,
    pub duration_sec: u64,
    pub frame_count: usize,
    pub windows_processed: usize,
    pub bus_summaries: Vec<BusSummary>,
}

#[derive(Debug, Clone, Default)]
struct RunningSummary {
    n: usize,
    rms_sum: f32,
    rms_norm_sum: f32,
    peak_sum: f32,
    centroid_sum: f32,
    transient_density_sum: f32,
    confidence_sum: f32,
}

#[derive(Debug, Clone)]
struct BusState {
    prev_mags: Vec<f32>,
    onset_times_sec: VecDeque<f64>,
    prev_rms: Option<(f32, f64)>,
    calibration_rms: Vec<f32>,
    noise_floor_db: f32,
    gain_norm_db: f32,
    calibrated: bool,
}

impl Default for BusState {
    fn default() -> Self {
        Self {
            prev_mags: Vec::new(),
            onset_times_sec: VecDeque::new(),
            prev_rms: None,
            calibration_rms: Vec::new(),
            noise_floor_db: -90.0,
            gain_norm_db: 0.0,
            calibrated: false,
        }
    }
}

pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    20.0 * mean_sq.sqrt().max(1e-9).log10()
}

pub fn peak_db(samples: &[f32]) -> f32 {
    let peak = samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
        .max(1e-9);
    20.0 * peak.log10()
}

pub fn analyze(samples: &[f32]) -> AudioFeatures {
    let rms = rms_db(samples);
    let peak = peak_db(samples);
    AudioFeatures {
        rms_db: rms,
        peak_db: peak,
        crest_db: peak - rms,
    }
}

pub fn list_input_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .context("failed to enumerate input devices")?;
    let mut out = Vec::new();
    for device in devices {
        out.push(device.name().unwrap_or_else(|_| "<unknown>".to_string()));
    }
    Ok(out)
}

pub fn run_realtime_monitor(req: AudioMonitorRequest) -> Result<AudioMonitorReport> {
    if req.duration_sec == 0 {
        return Err(anyhow!("duration_sec_must_be_positive"));
    }
    if req.window_ms == 0 || req.hop_ms == 0 {
        return Err(anyhow!("window_ms_and_hop_ms_must_be_positive"));
    }
    if req.hop_ms > req.window_ms {
        return Err(anyhow!("hop_ms_must_be_lte_window_ms"));
    }

    let host = cpal::default_host();
    let device = select_input_device(&host, req.device_hint.as_deref())?;
    let device_name = device.name().unwrap_or_else(|_| "<unknown>".to_string());
    let (stream_config, sample_format) = choose_stream_config(&device, req.sample_rate_hz)?;
    let channels = stream_config.channels as usize;
    let sample_rate_hz = stream_config.sample_rate.0;

    let mut buses = if req.buses.is_empty() {
        vec![AudioBusMap {
            id: "MASTER".to_string(),
            channels: (1..=channels).collect(),
        }]
    } else {
        req.buses.clone()
    };
    let requested_bus_count = buses.len();
    sanitize_bus_channels(&mut buses, channels)?;
    if buses.len() < requested_bus_count {
        eprintln!(
            "audio_monitor: dropped {} bus(es) due unavailable input channels (available={})",
            requested_bus_count - buses.len(),
            channels
        );
    }
    let mut virtual_groups = req.virtual_groups.clone();
    if req.include_default_virtual_groups {
        merge_virtual_groups(&mut virtual_groups, default_virtual_groups(&buses));
    }
    sanitize_virtual_groups(&mut virtual_groups, &buses);

    let window_frames =
        (((req.window_ms as f32 / 1000.0) * sample_rate_hz as f32).round() as usize).max(256);
    let hop_frames =
        (((req.hop_ms as f32 / 1000.0) * sample_rate_hz as f32).round() as usize).max(64);
    let cal_windows = ((req.calibrate_sec.max(1) as f32 * sample_rate_hz as f32)
        / hop_frames as f32)
        .ceil() as usize;

    let nfft = window_frames.next_power_of_two();
    let hann = hann_window(window_frames);
    let mut planner = FftPlanner::<f32>::new();
    let fft_forward = planner.plan_fft_forward(nfft);
    let fft_inverse = planner.plan_fft_inverse(nfft);

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let err_fn = |err| eprintln!("audio_monitor_stream_error={err}");

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
        "audio_monitor: device='{}' sr={} ch={} window={}ms hop={}ms calibrate={}s virtual_groups={}",
        device_name,
        sample_rate_hz,
        channels,
        req.window_ms,
        req.hop_ms,
        req.calibrate_sec,
        virtual_groups.len()
    );

    stream.play()?;

    let mut bus_state: HashMap<String, BusState> = HashMap::new();
    let mut summary_state: HashMap<String, RunningSummary> = HashMap::new();
    for bus in &buses {
        bus_state.insert(bus.id.clone(), BusState::default());
        summary_state.insert(bus.id.clone(), RunningSummary::default());
    }
    for vg in &virtual_groups {
        bus_state.insert(vg.id.clone(), BusState::default());
        summary_state.insert(vg.id.clone(), RunningSummary::default());
    }

    let mut writer = if let Some(path) = &req.jsonl_out {
        Some(BufWriter::new(File::create(path)?))
    } else {
        None
    };

    let mut interleaved = Vec::<f32>::new();
    let mut cursor = 0usize;
    let mut windows_processed = 0usize;
    let mut frame_count = 0usize;
    let mut last_print = Instant::now();
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(req.duration_sec) {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(30)) {
            interleaved.extend_from_slice(&chunk);
        }

        while interleaved.len() >= cursor + (window_frames * channels) {
            let window = &interleaved[cursor..cursor + (window_frames * channels)];
            let elapsed_sec = start.elapsed().as_secs_f64();
            let frame = analyze_frame(
                window,
                channels,
                sample_rate_hz,
                &buses,
                &virtual_groups,
                &mut bus_state,
                &hann,
                &fft_forward,
                &fft_inverse,
                nfft,
                windows_processed,
                cal_windows,
                elapsed_sec,
            )?;

            if let Some(w) = writer.as_mut() {
                serde_json::to_writer(&mut *w, &frame)?;
                w.write_all(b"\n")?;
            }

            for bus in &frame.buses {
                if let Some(s) = summary_state.get_mut(&bus.bus_id) {
                    s.n += 1;
                    s.rms_sum += bus.rms_db;
                    s.rms_norm_sum += bus.normalized_rms_db;
                    s.peak_sum += bus.peak_db;
                    s.centroid_sum += bus.spectral.centroid_hz;
                    s.transient_density_sum += bus.transient_density_hz;
                    s.confidence_sum += bus.confidence;
                }
            }

            if last_print.elapsed() >= Duration::from_millis(req.print_every_ms.max(60)) {
                print_frame_snapshot(&frame);
                last_print = Instant::now();
            }

            frame_count += 1;
            windows_processed += 1;
            cursor += hop_frames * channels;
        }

        if cursor > 0 && cursor > interleaved.len() / 2 {
            interleaved.drain(0..cursor);
            cursor = 0;
        }
    }

    drop(stream);

    if let Some(w) = writer.as_mut() {
        w.flush()?;
    }

    let mut bus_summaries = Vec::new();
    let mut ids: Vec<String> = buses.iter().map(|b| b.id.clone()).collect();
    ids.extend(virtual_groups.iter().map(|g| g.id.clone()));
    for id in ids {
        if let Some(s) = summary_state.get(&id) {
            if s.n == 0 {
                continue;
            }
            let n = s.n as f32;
            bus_summaries.push(BusSummary {
                bus_id: id,
                avg_rms_db: s.rms_sum / n,
                avg_normalized_rms_db: s.rms_norm_sum / n,
                avg_peak_db: s.peak_sum / n,
                avg_centroid_hz: s.centroid_sum / n,
                avg_transient_density_hz: s.transient_density_sum / n,
                avg_confidence: s.confidence_sum / n,
            });
        }
    }

    Ok(AudioMonitorReport {
        device_name,
        sample_rate_hz,
        channels,
        duration_sec: req.duration_sec,
        frame_count,
        windows_processed,
        bus_summaries,
    })
}

fn print_frame_snapshot(frame: &AudioFrameMetrics) {
    let mut parts = Vec::new();
    for bus in &frame.buses {
        parts.push(format!(
            "{} rms:{:+.1} norm:{:+.1} conf:{:.2} tr:{:.2}",
            bus.bus_id, bus.rms_db, bus.normalized_rms_db, bus.confidence, bus.transient_density_hz
        ));
    }
    println!(
        "audio_monitor: t={}ms {}",
        frame.elapsed_ms,
        parts.join(" | ")
    );
}

#[allow(clippy::too_many_arguments)]
fn analyze_frame(
    window: &[f32],
    channels: usize,
    sample_rate_hz: u32,
    buses: &[AudioBusMap],
    virtual_groups: &[AudioVirtualGroupMap],
    bus_state: &mut HashMap<String, BusState>,
    hann: &[f32],
    fft_forward: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    nfft: usize,
    window_index: usize,
    calibrate_windows: usize,
    elapsed_sec: f64,
) -> Result<AudioFrameMetrics> {
    let mut out = Vec::with_capacity(buses.len() + virtual_groups.len());
    let mut mono_by_id: HashMap<String, Vec<f32>> = HashMap::new();
    for bus in buses {
        mono_by_id.insert(
            bus.id.clone(),
            extract_bus_mono(window, channels, &bus.channels),
        );
    }

    for bus in buses {
        let mono = mono_by_id
            .get(&bus.id)
            .ok_or_else(|| anyhow!("internal_missing_mono_for_bus:{}", bus.id))?;
        let st = bus_state
            .get_mut(&bus.id)
            .ok_or_else(|| anyhow!("internal_missing_bus_state"))?;
        let phase = if bus.channels.len() >= 2 {
            let a = extract_single_channel(window, channels, bus.channels[0]);
            let b = extract_single_channel(window, channels, bus.channels[1]);
            Some(analyze_phase(&a, &b))
        } else {
            None
        };
        out.push(build_bus_metrics(
            &bus.id,
            mono,
            false,
            None,
            phase,
            None,
            None,
            st,
            sample_rate_hz,
            hann,
            nfft,
            fft_forward,
            fft_inverse,
            window_index,
            calibrate_windows,
            elapsed_sec,
        ));
    }

    for group in virtual_groups {
        let mut member_refs = Vec::new();
        for m in &group.members {
            if let Some(sig) = mono_by_id.get(m) {
                member_refs.push(sig.as_slice());
            }
        }
        if member_refs.is_empty() {
            continue;
        }
        let (combined, coherence, combination_gain_db) = combine_member_monos(&member_refs);
        let phase = if member_refs.len() >= 2 {
            let phasing_risk = if coherence < -0.2 {
                ((-coherence - 0.2) / 0.8).clamp(0.0, 1.0)
            } else {
                0.0
            };
            Some(PhaseMetrics {
                correlation: coherence,
                decorrelation: 1.0 - coherence.abs(),
                phasing_risk,
            })
        } else {
            None
        };
        let st = bus_state
            .get_mut(&group.id)
            .ok_or_else(|| anyhow!("internal_missing_virtual_group_state"))?;
        out.push(build_bus_metrics(
            &group.id,
            &combined,
            true,
            Some(group.members.clone()),
            phase,
            Some(coherence),
            Some(combination_gain_db),
            st,
            sample_rate_hz,
            hann,
            nfft,
            fft_forward,
            fft_inverse,
            window_index,
            calibrate_windows,
            elapsed_sec,
        ));
    }

    Ok(AudioFrameMetrics {
        ts_utc: Utc::now().to_rfc3339(),
        elapsed_ms: (elapsed_sec * 1000.0).round() as u64,
        buses: out,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_bus_metrics(
    bus_id: &str,
    mono: &[f32],
    is_virtual_group: bool,
    members: Option<Vec<String>>,
    phase: Option<PhaseMetrics>,
    combination_coherence: Option<f32>,
    combination_gain_db: Option<f32>,
    st: &mut BusState,
    sample_rate_hz: u32,
    hann: &[f32],
    nfft: usize,
    fft_forward: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    window_index: usize,
    calibrate_windows: usize,
    elapsed_sec: f64,
) -> BusFrameMetrics {
    let feats = analyze(mono);
    let spectral = analyze_spectrum(
        mono,
        sample_rate_hz,
        hann,
        nfft,
        fft_forward,
        fft_inverse,
        &mut st.prev_mags,
    );

    if window_index < calibrate_windows {
        st.calibration_rms.push(feats.rms_db);
    } else if !st.calibrated {
        let (noise_floor, gain_norm) = derive_calibration(&st.calibration_rms);
        st.noise_floor_db = noise_floor;
        st.gain_norm_db = gain_norm;
        st.calibrated = true;
    }

    let normalized_rms_db = feats.rms_db + st.gain_norm_db;
    let transient_strength = spectral.flux.max(0.0);
    if transient_strength > 0.005 {
        st.onset_times_sec.push_back(elapsed_sec);
    }
    while let Some(front) = st.onset_times_sec.front().copied() {
        if elapsed_sec - front > 1.0 {
            let _ = st.onset_times_sec.pop_front();
        } else {
            break;
        }
    }
    let transient_density_hz = st.onset_times_sec.len() as f32;

    let rt60_proxy_ms = estimate_rt60_proxy(st.prev_rms, feats.rms_db, elapsed_sec);
    st.prev_rms = Some((feats.rms_db, elapsed_sec));

    let confidence = confidence_score(
        feats.rms_db,
        feats.peak_db,
        st.noise_floor_db,
        phase.as_ref().map(|p| p.phasing_risk).unwrap_or(0.0),
    );
    let recommendation = recommendation_for(
        normalized_rms_db,
        feats.peak_db,
        phase.as_ref().map(|p| p.phasing_risk).unwrap_or(0.0),
        confidence,
    );

    BusFrameMetrics {
        bus_id: bus_id.to_string(),
        is_virtual_group,
        members,
        rms_db: feats.rms_db,
        normalized_rms_db,
        peak_db: feats.peak_db,
        crest_db: feats.crest_db,
        noise_floor_db: st.noise_floor_db,
        gain_norm_db: st.gain_norm_db,
        transient_strength,
        transient_density_hz,
        rt60_proxy_ms,
        spectral: SpectralMetrics {
            low_db: spectral.low_db,
            mid_db: spectral.mid_db,
            high_db: spectral.high_db,
            centroid_hz: spectral.centroid_hz,
            rolloff_hz: spectral.rolloff_hz,
            flux: spectral.flux,
        },
        cepstral: CepstralMetrics {
            peak_quefrency_ms: spectral.cepstral_peak_quefrency_ms,
            spread: spectral.cepstral_spread,
        },
        phase,
        combination_coherence,
        combination_gain_db,
        confidence,
        recommendation,
    }
}

#[derive(Debug, Clone)]
struct SpectralInternal {
    low_db: f32,
    mid_db: f32,
    high_db: f32,
    centroid_hz: f32,
    rolloff_hz: f32,
    flux: f32,
    cepstral_peak_quefrency_ms: f32,
    cepstral_spread: f32,
}

#[allow(clippy::too_many_arguments)]
fn analyze_spectrum(
    mono: &[f32],
    sample_rate_hz: u32,
    hann: &[f32],
    nfft: usize,
    fft_forward: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    fft_inverse: &std::sync::Arc<dyn rustfft::Fft<f32>>,
    prev_mags: &mut Vec<f32>,
) -> SpectralInternal {
    let eps = 1e-9_f32;
    let mut buffer = vec![Complex::new(0.0, 0.0); nfft];
    for (i, sample) in mono.iter().enumerate().take(hann.len()) {
        buffer[i].re = *sample * hann[i];
    }
    fft_forward.process(&mut buffer);

    let half = nfft / 2;
    let mut mags = vec![0.0_f32; half + 1];
    let mut centroid_num = 0.0_f32;
    let mut low = 0.0_f32;
    let mut mid = 0.0_f32;
    let mut high = 0.0_f32;

    for k in 0..=half {
        let c = buffer[k];
        let mag = (c.re * c.re + c.im * c.im).sqrt();
        let pow = mag * mag;
        mags[k] = mag;
        let freq = (k as f32 * sample_rate_hz as f32) / nfft as f32;
        centroid_num += freq * mag;
        if freq < 200.0 {
            low += pow;
        } else if freq < 4000.0 {
            mid += pow;
        } else {
            high += pow;
        }
    }

    let centroid = if mags.iter().sum::<f32>() > eps {
        centroid_num / mags.iter().sum::<f32>().max(eps)
    } else {
        0.0
    };

    let rolloff = spectral_rolloff_hz(&mags, sample_rate_hz, nfft, 0.85);

    let mut flux = 0.0_f32;
    if prev_mags.len() == mags.len() {
        for (m, p) in mags.iter().zip(prev_mags.iter()) {
            flux += (*m - *p).max(0.0);
        }
        flux /= mags.len() as f32;
    }
    *prev_mags = mags.clone();

    let (cep_peak_ms, cep_spread) =
        cepstral_metrics_from_mag(&mags, sample_rate_hz, nfft, fft_inverse);

    SpectralInternal {
        low_db: 10.0 * low.max(eps).log10(),
        mid_db: 10.0 * mid.max(eps).log10(),
        high_db: 10.0 * high.max(eps).log10(),
        centroid_hz: centroid,
        rolloff_hz: rolloff,
        flux,
        cepstral_peak_quefrency_ms: cep_peak_ms,
        cepstral_spread: cep_spread,
    }
}

fn cepstral_metrics_from_mag(
    mags: &[f32],
    sample_rate_hz: u32,
    nfft: usize,
    fft_inverse: &std::sync::Arc<dyn rustfft::Fft<f32>>,
) -> (f32, f32) {
    let eps = 1e-9_f32;
    let half = nfft / 2;
    let mut spec = vec![Complex::new(0.0_f32, 0.0_f32); nfft];
    for k in 0..=half.min(mags.len().saturating_sub(1)) {
        spec[k].re = mags[k].max(eps).ln();
    }
    for k in 1..half {
        let mirror = nfft - k;
        spec[mirror].re = spec[k].re;
    }
    fft_inverse.process(&mut spec);

    let mut peak_idx = 0usize;
    let mut peak_val = 0.0_f32;
    let q_min = ((0.001 * sample_rate_hz as f32).round() as usize).max(1);
    let q_max = ((0.025 * sample_rate_hz as f32).round() as usize).min(nfft.saturating_sub(1));

    let mut mean = 0.0_f32;
    let mut n = 0usize;
    for (i, c) in spec.iter().enumerate().take(q_max + 1).skip(q_min) {
        let v = (c.re / nfft as f32).abs();
        mean += v;
        n += 1;
        if v > peak_val {
            peak_val = v;
            peak_idx = i;
        }
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    mean /= n as f32;
    let mut var = 0.0_f32;
    for c in spec.iter().take(q_max + 1).skip(q_min) {
        let v = (c.re / nfft as f32).abs();
        let d = v - mean;
        var += d * d;
    }
    var /= n as f32;

    let peak_ms = (peak_idx as f32 * 1000.0) / sample_rate_hz as f32;
    (peak_ms, var.sqrt())
}

fn spectral_rolloff_hz(mags: &[f32], sample_rate_hz: u32, nfft: usize, pct: f32) -> f32 {
    let total = mags.iter().sum::<f32>().max(1e-9);
    let target = total * pct.clamp(0.01, 0.99);
    let mut acc = 0.0_f32;
    for (k, m) in mags.iter().enumerate() {
        acc += *m;
        if acc >= target {
            return (k as f32 * sample_rate_hz as f32) / nfft as f32;
        }
    }
    sample_rate_hz as f32 / 2.0
}

fn analyze_phase(a: &[f32], b: &[f32]) -> PhaseMetrics {
    let n = a.len().min(b.len()).max(1);
    let mut sum_a = 0.0_f32;
    let mut sum_b = 0.0_f32;
    for i in 0..n {
        sum_a += a[i];
        sum_b += b[i];
    }
    let mean_a = sum_a / n as f32;
    let mean_b = sum_b / n as f32;

    let mut cov = 0.0_f32;
    let mut var_a = 0.0_f32;
    let mut var_b = 0.0_f32;
    for i in 0..n {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }

    let denom = (var_a.sqrt() * var_b.sqrt()).max(1e-9);
    let corr = (cov / denom).clamp(-1.0, 1.0);
    let decor = 1.0 - corr.abs();
    let phasing_risk = if corr < -0.2 {
        ((-corr - 0.2) / 0.8).clamp(0.0, 1.0)
    } else {
        0.0
    };

    PhaseMetrics {
        correlation: corr,
        decorrelation: decor,
        phasing_risk,
    }
}

fn estimate_rt60_proxy(prev: Option<(f32, f64)>, now_rms_db: f32, now_sec: f64) -> Option<f32> {
    let (prev_db, prev_t) = prev?;
    if now_sec <= prev_t || now_rms_db >= prev_db {
        return None;
    }
    let drop_db = prev_db - now_rms_db;
    if drop_db < 0.5 {
        return None;
    }
    let dt = now_sec - prev_t;
    let rt60_ms = ((60.0 * dt) / drop_db as f64 * 1000.0) as f32;
    Some(rt60_ms.clamp(100.0, 12000.0))
}

fn derive_calibration(calibration_rms: &[f32]) -> (f32, f32) {
    if calibration_rms.is_empty() {
        return (-90.0, 0.0);
    }
    let mut sorted = calibration_rms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p20_idx = ((sorted.len() as f32) * 0.2).floor() as usize;
    let median_idx = sorted.len() / 2;
    let noise_floor = sorted[p20_idx.min(sorted.len() - 1)];
    let median = sorted[median_idx];
    let gain_norm_db = (-18.0 - median).clamp(-24.0, 24.0);
    (noise_floor, gain_norm_db)
}

fn confidence_score(rms_db: f32, peak_db: f32, noise_floor_db: f32, phasing_risk: f32) -> f32 {
    let snr_db = rms_db - noise_floor_db;
    let snr_score = ((snr_db - 3.0) / 24.0).clamp(0.0, 1.0);
    let headroom_score = ((0.0 - peak_db) / 12.0).clamp(0.0, 1.0);
    let phase_score = (1.0 - phasing_risk * 0.6).clamp(0.0, 1.0);
    (0.6 * snr_score + 0.3 * headroom_score + 0.1 * phase_score).clamp(0.0, 1.0)
}

fn recommendation_for(norm_rms: f32, peak_db: f32, phasing_risk: f32, confidence: f32) -> String {
    if confidence < 0.35 {
        return "low_confidence_check_input_gain_or_noise_floor".to_string();
    }
    if peak_db > -0.5 {
        return "clipping_risk_reduce_level".to_string();
    }
    if phasing_risk > 0.55 {
        return "phasing_risk_check_width_or_alignment".to_string();
    }
    if norm_rms < -28.0 {
        return "very_soft_consider_gain_up".to_string();
    }
    if norm_rms > -10.0 {
        return "very_hot_consider_gain_down".to_string();
    }
    "ok".to_string()
}

fn extract_bus_mono(window: &[f32], channels: usize, bus_channels: &[usize]) -> Vec<f32> {
    if bus_channels.is_empty() || channels == 0 {
        return Vec::new();
    }
    let frames = window.len() / channels;
    let mut out = vec![0.0_f32; frames];
    let n = bus_channels.len() as f32;
    for i in 0..frames {
        let base = i * channels;
        let mut sum = 0.0_f32;
        for ch in bus_channels {
            let idx0 = ch.saturating_sub(1);
            sum += window.get(base + idx0).copied().unwrap_or(0.0);
        }
        out[i] = sum / n;
    }
    out
}

fn extract_single_channel(window: &[f32], channels: usize, channel_1_based: usize) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }
    let frames = window.len() / channels;
    let idx0 = channel_1_based.saturating_sub(1);
    let mut out = vec![0.0_f32; frames];
    for i in 0..frames {
        let base = i * channels;
        out[i] = window.get(base + idx0).copied().unwrap_or(0.0);
    }
    out
}

fn hann_window(len: usize) -> Vec<f32> {
    if len <= 1 {
        return vec![1.0; len.max(1)];
    }
    let m = (len - 1) as f32;
    (0..len)
        .map(|i| 0.5 - 0.5 * ((2.0 * std::f32::consts::PI * i as f32) / m).cos())
        .collect()
}

fn sanitize_bus_channels(buses: &mut Vec<AudioBusMap>, available_channels: usize) -> Result<()> {
    for bus in buses.iter_mut() {
        let mut filtered = Vec::new();
        for ch in &bus.channels {
            if *ch > 0 && *ch <= available_channels {
                filtered.push(*ch);
            }
        }
        filtered.sort_unstable();
        filtered.dedup();
        bus.channels = filtered;
    }
    buses.retain(|b| !b.channels.is_empty());
    if buses.is_empty() {
        return Err(anyhow!("no_valid_audio_buses_after_channel_filtering"));
    }
    Ok(())
}

fn default_virtual_groups(buses: &[AudioBusMap]) -> Vec<AudioVirtualGroupMap> {
    let has = |id: &str| buses.iter().any(|b| b.id == id);
    let mut out = Vec::new();

    let str_members = vec!["STR_HI", "STR_MID", "STR_LO"]
        .into_iter()
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if str_members.len() >= 2 {
        out.push(AudioVirtualGroupMap {
            id: "STR_ALL".to_string(),
            members: str_members,
        });
    }

    let ww_members = vec!["WW_HI", "WW_MID", "WW_LO"]
        .into_iter()
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if ww_members.len() >= 2 {
        out.push(AudioVirtualGroupMap {
            id: "WW_ALL".to_string(),
            members: ww_members,
        });
    }

    let brass_members = vec!["HN", "TPT", "BR_LO"]
        .into_iter()
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if brass_members.len() >= 2 {
        out.push(AudioVirtualGroupMap {
            id: "BRASS_ALL".to_string(),
            members: brass_members,
        });
    }

    let rhythm_members = vec!["TIMP", "PERC", "HARP"]
        .into_iter()
        .filter(|m| has(m))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if rhythm_members.len() >= 2 {
        out.push(AudioVirtualGroupMap {
            id: "RHYTHM_ALL".to_string(),
            members: rhythm_members,
        });
    }

    let orch_members = buses.iter().map(|b| b.id.clone()).collect::<Vec<_>>();
    if orch_members.len() >= 2 {
        out.push(AudioVirtualGroupMap {
            id: "ORCH_ALL".to_string(),
            members: orch_members,
        });
    }

    out
}

fn merge_virtual_groups(dst: &mut Vec<AudioVirtualGroupMap>, mut src: Vec<AudioVirtualGroupMap>) {
    for g in src.drain(..) {
        if dst.iter().any(|x| x.id == g.id) {
            continue;
        }
        dst.push(g);
    }
}

fn sanitize_virtual_groups(groups: &mut Vec<AudioVirtualGroupMap>, buses: &[AudioBusMap]) {
    let ids = buses.iter().map(|b| b.id.as_str()).collect::<Vec<_>>();
    for g in groups.iter_mut() {
        let mut m = g
            .members
            .iter()
            .filter(|id| ids.iter().any(|x| *x == id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        m.sort();
        m.dedup();
        g.members = m;
    }
    groups.retain(|g| !g.members.is_empty());
}

fn combine_member_monos(member_signals: &[&[f32]]) -> (Vec<f32>, f32, f32) {
    if member_signals.is_empty() {
        return (Vec::new(), 0.0, 0.0);
    }
    let min_len = member_signals.iter().map(|s| s.len()).min().unwrap_or(0);
    if min_len == 0 {
        return (Vec::new(), 0.0, 0.0);
    }

    let mut out = vec![0.0_f32; min_len];
    for sig in member_signals {
        for i in 0..min_len {
            out[i] += sig[i];
        }
    }

    let mut corr_sum = 0.0_f32;
    let mut pairs = 0usize;
    for i in 0..member_signals.len() {
        for j in (i + 1)..member_signals.len() {
            corr_sum += corrcoef(member_signals[i], member_signals[j], min_len);
            pairs += 1;
        }
    }
    let coherence = if pairs > 0 {
        corr_sum / pairs as f32
    } else {
        1.0
    };

    let combined_rms = db_to_amp(rms_db(&out));
    let power_sum = member_signals
        .iter()
        .map(|s| {
            let r = db_to_amp(rms_db(&s[..min_len]));
            r * r
        })
        .sum::<f32>()
        .max(1e-12);
    let combined_power = (combined_rms * combined_rms).max(1e-12);
    let combination_gain_db = 10.0 * (combined_power / power_sum).log10();

    (out, coherence, combination_gain_db)
}

fn corrcoef(a: &[f32], b: &[f32], n: usize) -> f32 {
    if n == 0 {
        return 0.0;
    }
    let mut sum_a = 0.0_f32;
    let mut sum_b = 0.0_f32;
    for i in 0..n {
        sum_a += a[i];
        sum_b += b[i];
    }
    let mean_a = sum_a / n as f32;
    let mean_b = sum_b / n as f32;

    let mut cov = 0.0_f32;
    let mut var_a = 0.0_f32;
    let mut var_b = 0.0_f32;
    for i in 0..n {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    let denom = (var_a.sqrt() * var_b.sqrt()).max(1e-9);
    (cov / denom).clamp(-1.0, 1.0)
}

fn db_to_amp(db: f32) -> f32 {
    10_f32.powf(db / 20.0)
}

fn select_input_device(host: &cpal::Host, hint: Option<&str>) -> Result<cpal::Device> {
    if let Some(h) = hint {
        let needle = h.to_lowercase();
        let devices = host
            .input_devices()
            .context("failed to enumerate input devices")?;
        for d in devices {
            let name = d.name().unwrap_or_else(|_| "<unknown>".to_string());
            if name.to_lowercase().contains(&needle) {
                return Ok(d);
            }
        }
        return Err(anyhow!("audio_input_device_not_found:{}", h));
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no_default_input_device"))
}

fn choose_stream_config(
    device: &cpal::Device,
    preferred_sample_rate: Option<u32>,
) -> Result<(StreamConfig, SampleFormat)> {
    let ranges = device
        .supported_input_configs()
        .context("failed to query supported input configs")?;
    let ranges: Vec<SupportedStreamConfigRange> = ranges.collect();
    if ranges.is_empty() {
        return Err(anyhow!("no_supported_input_configs"));
    }

    if let Some(sr) = preferred_sample_rate {
        for r in &ranges {
            if r.min_sample_rate().0 <= sr && r.max_sample_rate().0 >= sr {
                let cfg = r.with_sample_rate(SampleRate(sr));
                return Ok((cfg.config(), cfg.sample_format()));
            }
        }
    }

    let default_cfg = device.default_input_config()?;
    Ok((default_cfg.config(), default_cfg.sample_format()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_has_crest() {
        let data = vec![0.0, 0.1, 0.3, 0.0, -0.2, 0.6, -0.1];
        let f = analyze(&data);
        assert!(f.crest_db.is_finite());
    }

    #[test]
    fn calibration_derives_values() {
        let samples = vec![-60.0, -50.0, -40.0, -35.0, -30.0, -25.0, -20.0];
        let (noise, gain) = derive_calibration(&samples);
        assert!(noise <= -40.0);
        assert!(gain.is_finite());
    }

    #[test]
    fn phase_metrics_detect_anti_phase() {
        let a = vec![1.0, -1.0, 1.0, -1.0];
        let b = vec![-1.0, 1.0, -1.0, 1.0];
        let m = analyze_phase(&a, &b);
        assert!(m.correlation < -0.9);
        assert!(m.phasing_risk > 0.8);
    }
}
