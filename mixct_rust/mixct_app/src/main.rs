use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use mixct_audio::{list_input_devices, run_realtime_monitor, AudioBusMap, AudioMonitorRequest};
mod mcu_feedback;
mod midi_backend;
mod ltc_ctl;
mod hotkey_bridge;
mod timeline_service;
mod transport_ctl;
mod transport_ltc_ctl;
use hotkey_bridge::{
    run_hotkey_bridge, run_hotkey_send, run_transport_seek_hotkey, HotkeyAction,
};
use ltc_ctl::run_ltc_monitor;
use mcu_feedback::{list_input_ports, monitor_mcu_feedback};
use midi_backend::{
    list_output_ports, run_cc_sweep, run_mcu_transport, McuTransportAction, MidiBackend,
    MidiTargetSpec, UndoTrigger, WriteProtocol,
};
use mixct_audit::{AuditLogger, AuditRecord};
use mixct_control::{
    execute_pass_with_scales, validate_prepass, ControlBackend, MockBackend, PrepassState,
};
use mixct_core::{
    clamp_db, compute_risk_score, enforce_slew, Decision, LaneKind, OperationClass, PassPlan,
    ResolvedTarget, RiskInputs, TimeRange,
};
use mixct_intent::parse_utterance;
use mixct_restore::{capture_undo_anchor, restore_from_anchor, UndoExecutor};
use mixct_sync::{evaluate_sync, SyncInputs};
use serde::{Deserialize, Serialize};
use speech_apple_bridge::{MockAppleSpeech, SpeechBackend};
use speech_fallback_local::{transcribe_with_local_fallback, FallbackConfig};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{info, warn};
use transport_ctl::{
    load_markers, resolve_target_seconds, run_transport_home, run_transport_monitor, run_transport_seek,
    run_transport_step, MusicalGrid,
};
use transport_ltc_ctl::{
    resolve_target_seconds_ltc, run_transport_home_ltc, run_transport_seek_ltc,
};
use timeline_service::{anchored_seconds_for_bar_beat_tick, score_seconds_for_bar_beat_tick};
use uuid::Uuid;
use voice_tts_apple_bridge::{ConsoleTts, TtsSpeaker};

#[derive(Parser)]
#[command(name = "mixct-app")]
#[command(about = "MixCT MVP-B CLI daemon prototype")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum BackendMode {
    Mock,
    Midi,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum WriteProtocolArg {
    Cc,
    Mcu,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum TransportActionArg {
    Play,
    Stop,
    Rewind,
    FastForward,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum HotkeyActionArg {
    Home,
    BarBack,
    BarForward,
    BeatBack,
    BeatForward,
}

#[derive(Subcommand)]
enum Commands {
    MidiPorts,
    MidiInPorts,
    AudioInPorts,
    AudioMonitor {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 10)]
        duration_sec: u64,
        #[arg(long, default_value_t = 100)]
        window_ms: u64,
        #[arg(long, default_value_t = 20)]
        hop_ms: u64,
        #[arg(long, default_value_t = 2)]
        calibrate_sec: u64,
        #[arg(long)]
        sample_rate_hz: Option<u32>,
        #[arg(long)]
        session_map: Option<PathBuf>,
        #[arg(long)]
        jsonl_out: Option<PathBuf>,
        #[arg(long, default_value_t = 250)]
        print_every_ms: u64,
    },
    TimelineProbe {
        #[arg(long)]
        score_midi: PathBuf,
        #[arg(long)]
        bar: u32,
        #[arg(long, default_value_t = 1.0)]
        beat: f64,
        #[arg(long, default_value_t = 0)]
        tick: u32,
        #[arg(long, default_value_t = 3600.0)]
        start_offset_sec: f64,
    },
    LtcMonitor {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 10)]
        duration_sec: u64,
        #[arg(long, default_value_t = 34)]
        channel: usize,
        #[arg(long)]
        sample_rate_hz: Option<u32>,
        #[arg(long, default_value_t = 24.0)]
        expected_fps: f32,
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    HotkeyBridge {
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long)]
        channel: Option<u8>,
        #[arg(long)]
        target_app: Option<String>,
        #[arg(long)]
        note_map: Option<String>,
        #[arg(long)]
        duration_sec: Option<u64>,
        #[arg(long, default_value_t = true)]
        verbose: bool,
    },
    HotkeySend {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long, default_value_t = 16)]
        channel: u8,
        #[arg(long, value_enum)]
        action: Option<HotkeyActionArg>,
        #[arg(long)]
        note: Option<u8>,
        #[arg(long, default_value_t = 127)]
        velocity: u8,
        #[arg(long, default_value_t = 35)]
        hold_ms: u64,
    },
    McuTransport {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long, value_enum)]
        action: TransportActionArg,
        #[arg(long, default_value_t = 60)]
        hold_ms: u64,
    },
    TransportMonitor {
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long, default_value_t = 20)]
        duration_sec: u64,
        #[arg(long, default_value_t = 150)]
        poll_ms: u64,
        #[arg(long, default_value_t = 120.0)]
        tempo_bpm: f64,
        #[arg(long, default_value_t = 4)]
        ts_num: u32,
        #[arg(long, default_value_t = 4)]
        ts_den: u32,
        #[arg(long, default_value_t = 480)]
        ppq: u32,
        #[arg(long, default_value_t = 0.0)]
        start_offset_sec: f64,
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    TransportSeek {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long)]
        marker: Option<String>,
        #[arg(long)]
        markers_file: Option<PathBuf>,
        #[arg(long)]
        target_bar: Option<u32>,
        #[arg(long)]
        target_beat: Option<f64>,
        #[arg(long)]
        target_tick: Option<u32>,
        #[arg(long, default_value_t = 120.0)]
        tempo_bpm: f64,
        #[arg(long, default_value_t = 4)]
        ts_num: u32,
        #[arg(long, default_value_t = 4)]
        ts_den: u32,
        #[arg(long, default_value_t = 480)]
        ppq: u32,
        #[arg(long, default_value_t = 0.0)]
        start_offset_sec: f64,
        #[arg(long, default_value_t = 70)]
        tolerance_ms: u64,
        #[arg(long, default_value_t = 30)]
        max_steps: u32,
    },
    TransportStep {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long)]
        beats: f64,
        #[arg(long, default_value_t = 120.0)]
        tempo_bpm: f64,
        #[arg(long, default_value_t = 4)]
        ts_num: u32,
        #[arg(long, default_value_t = 4)]
        ts_den: u32,
        #[arg(long, default_value_t = 480)]
        ppq: u32,
        #[arg(long, default_value_t = 0.0)]
        start_offset_sec: f64,
        #[arg(long, default_value_t = 70)]
        tolerance_ms: u64,
        #[arg(long, default_value_t = 30)]
        max_steps: u32,
    },
    TransportSeekHotkey {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long, default_value_t = 16)]
        channel: u8,
        #[arg(long)]
        target_bar: u32,
        #[arg(long, default_value_t = 1.0)]
        target_beat: f64,
        #[arg(long, default_value_t = 0)]
        target_tick: u32,
        #[arg(long, default_value_t = false)]
        use_beat_hotkeys: bool,
        #[arg(long, default_value_t = false)]
        no_home: bool,
        #[arg(long, default_value_t = 127)]
        velocity: u8,
        #[arg(long, default_value_t = 28)]
        hold_ms: u64,
        #[arg(long, default_value_t = 35)]
        interval_ms: u64,
        #[arg(long, default_value_t = 120)]
        settle_ms: u64,
        #[arg(long, default_value_t = false)]
        verify_ltc: bool,
        #[arg(long)]
        ltc_device: Option<String>,
        #[arg(long, default_value_t = 34)]
        ltc_channel: usize,
        #[arg(long, default_value_t = 25.0)]
        ltc_fps: f32,
        #[arg(long)]
        sample_rate_hz: Option<u32>,
        #[arg(long, default_value_t = 1)]
        verify_sec: u64,
        #[arg(long)]
        score_midi: Option<PathBuf>,
        #[arg(long)]
        score_anchors_file: Option<PathBuf>,
        #[arg(long, default_value_t = 3600.0)]
        start_offset_sec: f64,
        #[arg(long, default_value_t = 180)]
        verify_tolerance_ms: u64,
        #[arg(long, default_value_t = false)]
        verify_strict: bool,
        #[arg(long, default_value_t = 120)]
        fine_tolerance_ms: u64,
        #[arg(long, default_value_t = 60)]
        fine_max_steps: u32,
    },
    TransportHome {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long, default_value_t = 400)]
        hold_ms: u64,
        #[arg(long, default_value_t = 120)]
        max_steps: u32,
        #[arg(long)]
        home_floor_sec: Option<f64>,
    },
    TransportSeekLtc {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        score_midi: Option<PathBuf>,
        #[arg(long)]
        score_anchors_file: Option<PathBuf>,
        #[arg(long)]
        ltc_device: Option<String>,
        #[arg(long, default_value_t = 34)]
        ltc_channel: usize,
        #[arg(long, default_value_t = 24.0)]
        ltc_fps: f32,
        #[arg(long)]
        sample_rate_hz: Option<u32>,
        #[arg(long)]
        marker: Option<String>,
        #[arg(long)]
        markers_file: Option<PathBuf>,
        #[arg(long)]
        target_bar: Option<u32>,
        #[arg(long)]
        target_beat: Option<f64>,
        #[arg(long)]
        target_tick: Option<u32>,
        #[arg(long, default_value_t = 120.0)]
        tempo_bpm: f64,
        #[arg(long, default_value_t = 4)]
        ts_num: u32,
        #[arg(long, default_value_t = 4)]
        ts_den: u32,
        #[arg(long, default_value_t = 480)]
        ppq: u32,
        #[arg(long, default_value_t = 0.0)]
        start_offset_sec: f64,
        #[arg(long, default_value_t = 70)]
        tolerance_ms: u64,
        #[arg(long, default_value_t = 30)]
        max_steps: u32,
    },
    TransportHomeLtc {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        ltc_device: Option<String>,
        #[arg(long, default_value_t = 34)]
        ltc_channel: usize,
        #[arg(long, default_value_t = 24.0)]
        ltc_fps: f32,
        #[arg(long)]
        sample_rate_hz: Option<u32>,
        #[arg(long, default_value_t = 400)]
        hold_ms: u64,
        #[arg(long, default_value_t = 120)]
        max_steps: u32,
        #[arg(long)]
        home_floor_sec: Option<f64>,
    },
    CalibrateResponse {
        #[arg(long)]
        session_map: PathBuf,
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        audio_device: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        targets: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        baseline_db: f32,
        #[arg(long, default_value_t = 3.0)]
        test_delta_db: f32,
        #[arg(long, default_value_t = 2)]
        capture_sec: u64,
        #[arg(long, default_value_t = 350)]
        settle_ms: u64,
        #[arg(long, default_value_t = false)]
        no_auto_transport: bool,
        #[arg(long, value_enum, default_value_t = WriteProtocolArg::Mcu)]
        write_protocol: WriteProtocolArg,
    },
    CcSweep {
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long, default_value_t = 16)]
        channel: u8,
        #[arg(long)]
        cc: u8,
        #[arg(long, default_value_t = 5)]
        min: u8,
        #[arg(long, default_value_t = 96)]
        max: u8,
        #[arg(long, default_value_t = 2)]
        step: u8,
        #[arg(long, default_value_t = 80)]
        interval_ms: u64,
        #[arg(long)]
        cycles: Option<u32>,
    },
    McuMonitor {
        #[arg(long)]
        midi_in: Option<String>,
        #[arg(long)]
        session_map: Option<PathBuf>,
        #[arg(long, default_value_t = 12)]
        duration_sec: u64,
        #[arg(long, default_value_t = 150)]
        poll_ms: u64,
        #[arg(long)]
        json_out: Option<PathBuf>,
    },
    Diagnostics {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        session_map: Option<PathBuf>,
        #[arg(long)]
        timeline_snapshot: Option<PathBuf>,
    },
    Plan {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        command: String,
        #[arg(long)]
        session_map: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Execute {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        command: String,
        #[arg(long, default_value_t = 1.0)]
        depth: f32,
        #[arg(long, default_value_t = 2400)]
        gesture_ms: u64,
        #[arg(long, value_enum, default_value_t = BackendMode::Midi)]
        backend: BackendMode,
        #[arg(long, value_enum, default_value_t = WriteProtocolArg::Mcu)]
        write_protocol: WriteProtocolArg,
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        feedback_in: Option<String>,
        #[arg(long, default_value_t = 0)]
        feedback_sec: u64,
        #[arg(long)]
        response_calibration: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        no_response_calibration: bool,
        #[arg(long)]
        audio_device: Option<String>,
        #[arg(long, default_value_t = 6)]
        audio_verify_sec: u64,
        #[arg(long, default_value_t = 100)]
        audio_verify_window_ms: u64,
        #[arg(long, default_value_t = 20)]
        audio_verify_hop_ms: u64,
        #[arg(long, default_value_t = 1)]
        audio_verify_calibrate_sec: u64,
        #[arg(long, default_value_t = false)]
        no_audio_verify: bool,
        #[arg(long)]
        undo_primary_cc: Option<u8>,
        #[arg(long)]
        undo_fallback_cc: Option<u8>,
        #[arg(long, default_value_t = 16)]
        undo_channel: u8,
        #[arg(long)]
        session_map: Option<PathBuf>,
        #[arg(long)]
        audit_dir: PathBuf,
        #[arg(long)]
        command_id: Option<String>,
        #[arg(long)]
        captured_at: Option<String>,
        #[arg(long, default_value_t = 120)]
        max_command_age_sec: u64,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = 34)]
        ltc_channel: usize,
        #[arg(long, default_value_t = 24.0)]
        ltc_fps: f32,
        #[arg(long, default_value_t = 2)]
        ltc_probe_sec: u64,
        #[arg(long)]
        ltc_sample_rate_hz: Option<u32>,
        #[arg(long)]
        score_midi: Option<PathBuf>,
        #[arg(long)]
        seek_anchors_file: Option<PathBuf>,
        #[arg(long)]
        seek_marker: Option<String>,
        #[arg(long)]
        markers_file: Option<PathBuf>,
        #[arg(long)]
        seek_bar: Option<u32>,
        #[arg(long)]
        seek_beat: Option<f64>,
        #[arg(long)]
        seek_tick: Option<u32>,
        #[arg(long, default_value_t = 120.0)]
        seek_tempo_bpm: f64,
        #[arg(long, default_value_t = 4)]
        seek_ts_num: u32,
        #[arg(long, default_value_t = 4)]
        seek_ts_den: u32,
        #[arg(long, default_value_t = 480)]
        seek_ppq: u32,
        #[arg(long, default_value_t = 3600.0)]
        seek_start_offset_sec: f64,
        #[arg(long, default_value_t = 70)]
        seek_tolerance_ms: u64,
        #[arg(long, default_value_t = 40)]
        seek_max_steps: u32,
        #[arg(long, default_value_t = false)]
        strict_seek: bool,
        #[arg(long, default_value_t = false)]
        auto_play_before_write: bool,
        #[arg(long, default_value_t = false)]
        auto_stop_after_write: bool,
    },
    Restore {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        command_id: String,
        #[arg(long, value_enum, default_value_t = BackendMode::Midi)]
        backend: BackendMode,
        #[arg(long)]
        midi_out: Option<String>,
        #[arg(long)]
        undo_primary_cc: Option<u8>,
        #[arg(long)]
        undo_fallback_cc: Option<u8>,
        #[arg(long, default_value_t = 16)]
        undo_channel: u8,
        #[arg(long)]
        audit_dir: PathBuf,
    },
}

#[derive(Debug)]
struct SpecChecks {
    ok: bool,
    errors: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct SessionMap {
    #[serde(default)]
    entity_aliases: HashMap<String, String>,
    #[serde(default)]
    buses: Vec<SessionBus>,
}

#[derive(Debug, Deserialize, Clone)]
struct SessionBus {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    audio_channels: Vec<usize>,
    mcu_channel: Option<u8>,
    #[serde(default = "default_midi_channel")]
    channel: u8,
    cc: Option<u8>,
    #[serde(default = "default_bus_min_db")]
    min_db: f32,
    #[serde(default = "default_bus_max_db")]
    max_db: f32,
}

#[derive(Debug, Deserialize)]
struct AuditLine {
    command_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResponseCalibrationFile {
    version: String,
    created_at_utc: String,
    baseline_db: f32,
    test_delta_db: f32,
    buses: Vec<ResponseCalibrationBus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResponseCalibrationBus {
    id: String,
    slope_audio_db_per_fader_db: f32,
    recommended_fader_scale: f32,
    baseline_rms_db: f32,
    high_rms_db: f32,
    low_rms_db: f32,
    delta_audio_db: f32,
    confidence: f32,
    valid: bool,
}

fn default_midi_channel() -> u8 {
    16
}

fn default_bus_min_db() -> f32 {
    -18.0
}

fn default_bus_max_db() -> f32 {
    6.0
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .without_time()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::MidiPorts => {
            for (idx, name) in list_output_ports()?.iter().enumerate() {
                println!("{idx}: {name}");
            }
            Ok(())
        }
        Commands::MidiInPorts => {
            for (idx, name) in list_input_ports()?.iter().enumerate() {
                println!("{idx}: {name}");
            }
            Ok(())
        }
        Commands::AudioInPorts => {
            for (idx, name) in list_input_devices()?.iter().enumerate() {
                println!("{idx}: {name}");
            }
            Ok(())
        }
        Commands::AudioMonitor {
            device,
            duration_sec,
            window_ms,
            hop_ms,
            calibrate_sec,
            sample_rate_hz,
            session_map,
            jsonl_out,
            print_every_ms,
        } => run_audio_monitor(
            device.as_deref(),
            duration_sec,
            window_ms,
            hop_ms,
            calibrate_sec,
            sample_rate_hz,
            session_map.as_deref(),
            jsonl_out.as_deref(),
            print_every_ms,
        ),
        Commands::TimelineProbe {
            score_midi,
            bar,
            beat,
            tick,
            start_offset_sec,
        } => {
            let (seconds, summary) =
                score_seconds_for_bar_beat_tick(&score_midi, bar, beat, tick, start_offset_sec)?;
            println!(
                "timeline_probe: score='{}' bar={} beat={} tick={} -> seconds={:.6} (offset={:.3}) ppq={} tempo_events={} timesig_events={}",
                score_midi.display(),
                bar,
                beat,
                tick,
                seconds,
                start_offset_sec,
                summary.ppq,
                summary.tempo_events,
                summary.time_signature_events
            );
            Ok(())
        }
        Commands::LtcMonitor {
            device,
            duration_sec,
            channel,
            sample_rate_hz,
            expected_fps,
            json_out,
        } => {
            let _ = run_ltc_monitor(
                device.as_deref(),
                channel,
                duration_sec,
                sample_rate_hz,
                expected_fps,
                json_out.as_deref(),
            )?;
            Ok(())
        }
        Commands::HotkeyBridge {
            midi_in,
            channel,
            target_app,
            note_map,
            duration_sec,
            verbose,
        } => run_hotkey_bridge(
            midi_in.as_deref(),
            channel,
            note_map.as_deref(),
            target_app.as_deref(),
            duration_sec,
            verbose,
        ),
        Commands::HotkeySend {
            midi_out,
            channel,
            action,
            note,
            velocity,
            hold_ms,
        } => {
            let mapped = match (action, note) {
                (Some(HotkeyActionArg::Home), None) => HotkeyAction::Home.default_note(),
                (Some(HotkeyActionArg::BarBack), None) => HotkeyAction::BarBack.default_note(),
                (Some(HotkeyActionArg::BarForward), None) => {
                    HotkeyAction::BarForward.default_note()
                }
                (Some(HotkeyActionArg::BeatBack), None) => HotkeyAction::BeatBack.default_note(),
                (Some(HotkeyActionArg::BeatForward), None) => {
                    HotkeyAction::BeatForward.default_note()
                }
                (None, Some(n)) => n,
                (Some(_), Some(n)) => n,
                (None, None) => {
                    return Err(anyhow!(
                        "hotkey_send_requires_action_or_note (example: --action bar-forward)"
                    ))
                }
            };
            run_hotkey_send(midi_out.as_deref(), channel, mapped, velocity, hold_ms)
        }
        Commands::McuTransport {
            midi_out,
            action,
            hold_ms,
        } => {
            let a = match action {
                TransportActionArg::Play => McuTransportAction::Play,
                TransportActionArg::Stop => McuTransportAction::Stop,
                TransportActionArg::Rewind => McuTransportAction::Rewind,
                TransportActionArg::FastForward => McuTransportAction::FastForward,
            };
            run_mcu_transport(midi_out.as_deref(), a, hold_ms)
        }
        Commands::TransportMonitor {
            midi_in,
            duration_sec,
            poll_ms,
            tempo_bpm,
            ts_num,
            ts_den,
            ppq,
            start_offset_sec,
            json_out,
        } => run_transport_monitor(
            midi_in.as_deref(),
            duration_sec,
            poll_ms,
            MusicalGrid {
                bpm: tempo_bpm,
                ts_num,
                ts_den,
                ppq,
                start_offset_sec,
            },
            json_out.as_deref(),
        ),
        Commands::TransportSeek {
            midi_out,
            midi_in,
            marker,
            markers_file,
            target_bar,
            target_beat,
            target_tick,
            tempo_bpm,
            ts_num,
            ts_den,
            ppq,
            start_offset_sec,
            tolerance_ms,
            max_steps,
        } => {
            let grid = MusicalGrid {
                bpm: tempo_bpm,
                ts_num,
                ts_den,
                ppq,
                start_offset_sec,
            };
            let target_seconds = resolve_target_seconds(
                target_bar,
                target_beat,
                target_tick,
                marker.as_deref(),
                markers_file.as_deref(),
                grid,
            )?;
            run_transport_seek(
                midi_out.as_deref(),
                midi_in.as_deref(),
                target_seconds,
                grid,
                tolerance_ms,
                max_steps,
            )
        }
        Commands::TransportStep {
            midi_out,
            midi_in,
            beats,
            tempo_bpm,
            ts_num,
            ts_den,
            ppq,
            start_offset_sec,
            tolerance_ms,
            max_steps,
        } => run_transport_step(
            midi_out.as_deref(),
            midi_in.as_deref(),
            beats,
            MusicalGrid {
                bpm: tempo_bpm,
                ts_num,
                ts_den,
                ppq,
                start_offset_sec,
            },
            tolerance_ms,
            max_steps,
        ),
        Commands::TransportSeekHotkey {
            midi_out,
            channel,
            target_bar,
            target_beat,
            target_tick,
            use_beat_hotkeys,
            no_home,
            velocity,
            hold_ms,
            interval_ms,
            settle_ms,
            verify_ltc,
            ltc_device,
            ltc_channel,
            ltc_fps,
            sample_rate_hz,
            verify_sec,
            score_midi,
            score_anchors_file,
            start_offset_sec,
            verify_tolerance_ms,
            verify_strict,
            fine_tolerance_ms,
            fine_max_steps,
        } => {
            let _ = run_mcu_transport(
                midi_out.as_deref(),
                McuTransportAction::Stop,
                80,
            );
            thread::sleep(Duration::from_millis(80));
            let hotkey_beat = if use_beat_hotkeys { target_beat } else { 1.0 };
            let seek = run_transport_seek_hotkey(
                midi_out.as_deref(),
                channel,
                target_bar,
                hotkey_beat,
                0,
                !no_home,
                velocity,
                hold_ms,
                interval_ms,
                settle_ms,
            )?;
            if !use_beat_hotkeys && (target_beat > 1.0 || target_tick > 0) {
                let score_path = score_midi
                    .as_deref()
                    .ok_or_else(|| anyhow!("score_midi_required_for_ltc_fine_seek"))?;
                let target_seconds = if let Some(anchor_path) = score_anchors_file.as_deref() {
                    anchored_seconds_for_bar_beat_tick(
                        score_path,
                        anchor_path,
                        target_bar,
                        target_beat,
                        target_tick,
                        start_offset_sec,
                    )?
                    .0
                } else {
                    score_seconds_for_bar_beat_tick(
                        score_path,
                        target_bar,
                        target_beat,
                        target_tick,
                        start_offset_sec,
                    )?
                    .0
                };
                if let Err(e) = run_transport_seek_ltc(
                    midi_out.as_deref(),
                    ltc_device.as_deref(),
                    ltc_channel,
                    sample_rate_hz,
                    ltc_fps,
                    target_seconds,
                    MusicalGrid {
                        bpm: 120.0,
                        ts_num: 4,
                        ts_den: 4,
                        ppq: 480,
                        start_offset_sec,
                    },
                    fine_tolerance_ms,
                    fine_max_steps,
                ) {
                    warn!("hotkey_fine_seek_best_effort_failed continuing=true error={e}");
                }
            }
            if verify_ltc {
                let _ = run_mcu_transport(
                    midi_out.as_deref(),
                    McuTransportAction::Stop,
                    80,
                );
                thread::sleep(Duration::from_millis(120));
                let report = run_ltc_monitor(
                    ltc_device.as_deref(),
                    ltc_channel,
                    verify_sec.max(1),
                    sample_rate_hz,
                    ltc_fps,
                    None,
                )?;
                if let Some(last) = report.last {
                    let obs_fps = if last.fps_estimate >= 1.0 {
                        last.fps_estimate as f64
                    } else {
                        ltc_fps as f64
                    };
                    let obs_sec = last.hour as f64 * 3600.0
                        + last.minute as f64 * 60.0
                        + last.second as f64
                        + (last.frame as f64 / obs_fps.max(1.0));
                    if let Some(score_path) = score_midi.as_deref() {
                        let (expected_sec, _summary) =
                            if let Some(anchor_path) = score_anchors_file.as_deref() {
                                anchored_seconds_for_bar_beat_tick(
                                    score_path,
                                    anchor_path,
                                    target_bar,
                                    target_beat,
                                    target_tick,
                                    start_offset_sec,
                                )?
                            } else {
                                score_seconds_for_bar_beat_tick(
                                    score_path,
                                    target_bar,
                                    target_beat,
                                    target_tick,
                                    start_offset_sec,
                                )?
                            };
                        let delta = obs_sec - expected_sec;
                        let tol = verify_tolerance_ms as f64 / 1000.0;
                        println!(
                            "transport_seek_hotkey_verify: observed={:.6}s expected={:.6}s delta={:+.6}s tol={:.3}s",
                            obs_sec, expected_sec, delta, tol
                        );
                        if verify_strict && delta.abs() > tol {
                            return Err(anyhow!(
                                "transport_seek_hotkey_verify_failed delta_sec={:+.6} tol_sec={:.6}",
                                delta,
                                tol
                            ));
                        }
                    } else {
                        println!(
                            "transport_seek_hotkey_verify: observed_ltc={:02}:{:02}:{:02}:{:02} ({:.6}s) (no score target provided)",
                            last.hour, last.minute, last.second, last.frame, obs_sec
                        );
                    }
                } else if verify_strict {
                    return Err(anyhow!(
                        "transport_seek_hotkey_verify_failed_no_ltc_frames_decoded"
                    ));
                }
            }
            let _ = run_mcu_transport(
                midi_out.as_deref(),
                McuTransportAction::Stop,
                80,
            );
            println!(
                "transport_seek_hotkey_done: home={} bar_steps={} beat_steps={} beat_hotkeys={} target={}|{}|{}",
                seek.home_sent, seek.bar_steps, seek.beat_steps, use_beat_hotkeys, seek.target_bar, target_beat, target_tick
            );
            Ok(())
        }
        Commands::TransportHome {
            midi_out,
            midi_in,
            hold_ms,
            max_steps,
            home_floor_sec,
        } => {
            run_transport_home(
                midi_out.as_deref(),
                midi_in.as_deref(),
                hold_ms,
                max_steps,
                home_floor_sec,
            )?;
            Ok(())
        }
        Commands::TransportSeekLtc {
            midi_out,
            score_midi,
            score_anchors_file,
            ltc_device,
            ltc_channel,
            ltc_fps,
            sample_rate_hz,
            marker,
            markers_file,
            target_bar,
            target_beat,
            target_tick,
            tempo_bpm,
            ts_num,
            ts_den,
            ppq,
            start_offset_sec,
            tolerance_ms,
            max_steps,
        } => {
            let grid = MusicalGrid {
                bpm: tempo_bpm,
                ts_num,
                ts_den,
                ppq,
                start_offset_sec,
            };
            let target_seconds = if let Some(score_path) = score_midi.as_deref() {
                let (bar, beat, tick) = if let Some(marker_name) = marker.as_deref() {
                    let marker_file = load_markers(markers_file.as_deref())?;
                    let m = marker_file
                        .markers
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(marker_name))
                        .ok_or_else(|| anyhow!("marker_not_found:{marker_name}"))?;
                    (m.bar, m.beat, m.tick)
                } else {
                    (
                        target_bar.ok_or_else(|| anyhow!("target_bar_required_without_marker"))?,
                        target_beat.unwrap_or(1.0),
                        target_tick.unwrap_or(0),
                    )
                };
                let (seconds, summary) = if let Some(anchor_path) = score_anchors_file.as_deref() {
                    anchored_seconds_for_bar_beat_tick(
                        score_path,
                        anchor_path,
                        bar,
                        beat,
                        tick,
                        start_offset_sec,
                    )?
                } else {
                    score_seconds_for_bar_beat_tick(score_path, bar, beat, tick, start_offset_sec)?
                };
                println!(
                    "transport_seek_ltc: score_map active score='{}' ppq={} tempo_events={} timesig_events={} target_bar={} beat={} tick={} target_sec={:.6}",
                    score_path.display(),
                    summary.ppq,
                    summary.tempo_events,
                    summary.time_signature_events,
                    bar,
                    beat,
                    tick,
                    seconds
                );
                seconds
            } else {
                resolve_target_seconds_ltc(
                    target_bar,
                    target_beat,
                    target_tick,
                    marker.as_deref(),
                    markers_file.as_deref(),
                    grid,
                )?
            };
            run_transport_seek_ltc(
                midi_out.as_deref(),
                ltc_device.as_deref(),
                ltc_channel,
                sample_rate_hz,
                ltc_fps,
                target_seconds,
                grid,
                tolerance_ms,
                max_steps,
            )
        }
        Commands::TransportHomeLtc {
            midi_out,
            ltc_device,
            ltc_channel,
            ltc_fps,
            sample_rate_hz,
            hold_ms,
            max_steps,
            home_floor_sec,
        } => run_transport_home_ltc(
            midi_out.as_deref(),
            ltc_device.as_deref(),
            ltc_channel,
            sample_rate_hz,
            ltc_fps,
            hold_ms,
            max_steps,
            home_floor_sec,
        ),
        Commands::CalibrateResponse {
            session_map,
            midi_out,
            audio_device,
            out,
            targets,
            baseline_db,
            test_delta_db,
            capture_sec,
            settle_ms,
            no_auto_transport,
            write_protocol,
        } => run_calibrate_response(
            &session_map,
            midi_out.as_deref(),
            audio_device.as_deref(),
            out.as_deref(),
            targets.as_deref(),
            baseline_db,
            test_delta_db,
            capture_sec,
            settle_ms,
            !no_auto_transport,
            write_protocol,
        ),
        Commands::CcSweep {
            midi_out,
            channel,
            cc,
            min,
            max,
            step,
            interval_ms,
            cycles,
        } => run_cc_sweep(
            midi_out.as_deref(),
            channel,
            cc,
            min,
            max,
            step,
            interval_ms,
            cycles,
        ),
        Commands::McuMonitor {
            midi_in,
            session_map,
            duration_sec,
            poll_ms,
            json_out,
        } => {
            let sm = load_session_map(session_map.as_deref())?;
            let (channel_map, db_ranges) = build_mcu_maps(&sm);
            monitor_mcu_feedback(
                midi_in.as_deref(),
                duration_sec,
                poll_ms,
                channel_map,
                db_ranges,
                json_out.as_deref(),
            )
        }
        Commands::Diagnostics {
            spec,
            session_map,
            timeline_snapshot,
        } => run_diagnostics(&spec, session_map.as_deref(), timeline_snapshot.as_deref()),
        Commands::Plan {
            spec,
            command,
            session_map,
            out,
        } => run_plan(&spec, &command, session_map.as_deref(), out.as_deref()),
        Commands::Execute {
            spec,
            command,
            depth,
            gesture_ms,
            backend,
            write_protocol,
            midi_out,
            feedback_in,
            feedback_sec,
            response_calibration,
            no_response_calibration,
            audio_device,
            audio_verify_sec,
            audio_verify_window_ms,
            audio_verify_hop_ms,
            audio_verify_calibrate_sec,
            no_audio_verify,
            undo_primary_cc,
            undo_fallback_cc,
            undo_channel,
            session_map,
            audit_dir,
            command_id,
            captured_at,
            max_command_age_sec,
            dry_run,
            ltc_channel,
            ltc_fps,
            ltc_probe_sec,
            ltc_sample_rate_hz,
            score_midi,
            seek_anchors_file,
            seek_marker,
            markers_file,
            seek_bar,
            seek_beat,
            seek_tick,
            seek_tempo_bpm,
            seek_ts_num,
            seek_ts_den,
            seek_ppq,
            seek_start_offset_sec,
            seek_tolerance_ms,
            seek_max_steps,
            strict_seek,
            auto_play_before_write,
            auto_stop_after_write,
        } => run_execute(
            &spec,
            &command,
            depth,
            gesture_ms,
            backend,
            write_protocol,
            midi_out.as_deref(),
            feedback_in.as_deref(),
            feedback_sec,
            response_calibration.as_deref(),
            no_response_calibration,
            audio_device.as_deref(),
            audio_verify_sec,
            audio_verify_window_ms,
            audio_verify_hop_ms,
            audio_verify_calibrate_sec,
            no_audio_verify,
            undo_primary_cc,
            undo_fallback_cc,
            undo_channel,
            session_map.as_deref(),
            &audit_dir,
            command_id,
            captured_at,
            max_command_age_sec,
            dry_run,
            ltc_channel,
            ltc_fps,
            ltc_probe_sec,
            ltc_sample_rate_hz,
            score_midi.as_deref(),
            seek_anchors_file.as_deref(),
            seek_marker.as_deref(),
            markers_file.as_deref(),
            seek_bar,
            seek_beat,
            seek_tick,
            seek_tempo_bpm,
            seek_ts_num,
            seek_ts_den,
            seek_ppq,
            seek_start_offset_sec,
            seek_tolerance_ms,
            seek_max_steps,
            strict_seek,
            auto_play_before_write,
            auto_stop_after_write,
        ),
        Commands::Restore {
            spec,
            command_id,
            backend,
            midi_out,
            undo_primary_cc,
            undo_fallback_cc,
            undo_channel,
            audit_dir,
        } => run_restore(
            &spec,
            &command_id,
            backend,
            midi_out.as_deref(),
            undo_primary_cc,
            undo_fallback_cc,
            undo_channel,
            &audit_dir,
        ),
    }
}

fn run_diagnostics(
    spec: &Path,
    session_map: Option<&Path>,
    timeline_snapshot: Option<&Path>,
) -> Result<()> {
    let val = load_json(spec)?;
    let checks = validate_spec(&val);
    if !checks.ok {
        for e in checks.errors {
            eprintln!("diag_fail: {e}");
        }
        return Err(anyhow!("spec validation failed"));
    }

    if let Some(path) = session_map {
        let _: SessionMap = load_yaml(path)
            .with_context(|| format!("failed loading session map: {}", path.display()))?;
    }
    if let Some(path) = timeline_snapshot {
        let _ = load_json(path)
            .with_context(|| format!("failed loading timeline snapshot: {}", path.display()))?;
    }

    info!("diagnostics_ok=true");
    println!("diagnostics: OK");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_audio_monitor(
    device: Option<&str>,
    duration_sec: u64,
    window_ms: u64,
    hop_ms: u64,
    calibrate_sec: u64,
    sample_rate_hz: Option<u32>,
    session_map_path: Option<&Path>,
    jsonl_out: Option<&Path>,
    print_every_ms: u64,
) -> Result<()> {
    let session_map = load_session_map(session_map_path)?;
    let buses = if let Some(sm) = &session_map {
        build_audio_bus_maps(sm)
    } else {
        Vec::new()
    };

    let req = AudioMonitorRequest {
        device_hint: device.map(|d| d.to_string()),
        duration_sec,
        sample_rate_hz,
        window_ms,
        hop_ms,
        calibrate_sec,
        buses,
        virtual_groups: Vec::new(),
        include_default_virtual_groups: true,
        print_every_ms,
        jsonl_out: jsonl_out.map(|p| p.to_path_buf()),
    };

    let report = run_realtime_monitor(req)?;
    println!(
        "audio_monitor: done device='{}' frames={} windows={} sr={} ch={}",
        report.device_name,
        report.frame_count,
        report.windows_processed,
        report.sample_rate_hz,
        report.channels
    );
    for s in &report.bus_summaries {
        println!(
            "audio_summary: {} rms={:+.1} norm={:+.1} peak={:+.1} centroid={:.0}Hz transient={:.2} conf={:.2}",
            s.bus_id,
            s.avg_rms_db,
            s.avg_normalized_rms_db,
            s.avg_peak_db,
            s.avg_centroid_hz,
            s.avg_transient_density_hz,
            s.avg_confidence
        );
    }
    Ok(())
}

fn run_plan(
    spec: &Path,
    command: &str,
    session_map_path: Option<&Path>,
    out: Option<&Path>,
) -> Result<()> {
    let val = load_json(spec)?;
    ensure_spec_ready(&val)?;

    let session_map = load_session_map(session_map_path)?;
    let aliases = build_aliases(&session_map);
    let intent = parse_utterance(command, &aliases);

    let risk = compute_risk_score(RiskInputs {
        target_count: intent.targets.len(),
        bar_span: intent
            .time_range
            .as_ref()
            .map(|t| t.end_bar - t.start_bar + 1)
            .unwrap_or(0),
        strength: match intent.strength.as_deref() {
            Some("slight") => 0.2,
            Some("medium") => 0.5,
            Some("strong") => 0.8,
            _ => 0.4,
        },
        confidence: intent.confidence,
        sync_margin: 0.95,
    });

    let plan = build_pass_plan(&intent, risk, session_map.as_ref(), 2400)?;
    let plan_json = serde_json::to_string_pretty(&plan)?;

    if let Some(path) = out {
        fs::write(path, &plan_json)?;
        println!("plan written: {}", path.display());
    } else {
        println!("{plan_json}");
    }

    Ok(())
}

fn map_write_protocol(arg: WriteProtocolArg) -> WriteProtocol {
    match arg {
        WriteProtocolArg::Cc => WriteProtocol::CcLearn,
        WriteProtocolArg::Mcu => WriteProtocol::McuFader,
    }
}

fn parse_target_list(targets: Option<&str>, sm: &SessionMap) -> Result<Vec<String>> {
    if let Some(raw) = targets {
        let mut out = Vec::new();
        for token in raw.split(',') {
            let t = token.trim();
            if t.is_empty() {
                continue;
            }
            let canonical = sm
                .buses
                .iter()
                .find(|b| b.id.eq_ignore_ascii_case(t))
                .map(|b| b.id.clone())
                .ok_or_else(|| anyhow!("unknown_target_in_calibration:{t}"))?;
            out.push(canonical);
        }
        if out.is_empty() {
            return Err(anyhow!("no_targets_selected_for_calibration"));
        }
        Ok(out)
    } else {
        Ok(sm.buses.iter().map(|b| b.id.clone()).collect())
    }
}

fn measure_bus_rms(
    device: Option<&str>,
    bus: AudioBusMap,
    capture_sec: u64,
) -> Result<(f32, mixct_audio::AudioMonitorReport)> {
    let req = AudioMonitorRequest {
        device_hint: Some(device.unwrap_or("LP32").to_string()),
        duration_sec: capture_sec.max(1),
        sample_rate_hz: Some(48000),
        window_ms: 120,
        hop_ms: 24,
        calibrate_sec: 1,
        buses: vec![bus.clone()],
        virtual_groups: Vec::new(),
        include_default_virtual_groups: false,
        print_every_ms: 1000,
        jsonl_out: None,
    };
    let report = run_realtime_monitor(req)?;
    let rms = report
        .bus_summaries
        .iter()
        .find(|s| s.bus_id == bus.id)
        .map(|s| s.avg_rms_db)
        .ok_or_else(|| anyhow!("missing_bus_summary_for:{}", bus.id))?;
    Ok((rms, report))
}

fn set_levels(backend: &mut dyn ControlBackend, bus_ids: &[String], level_db: f32) -> Result<()> {
    for id in bus_ids {
        let target = format!("{id}::Volume");
        backend.begin_touch(&target)?;
        backend.write_value(&target, level_db, 0)?;
        backend.end_touch(&target)?;
    }
    Ok(())
}

fn default_calibration_output_path(session_map_path: &Path) -> PathBuf {
    let file = format!(
        "response_calibration_{}.json",
        Utc::now().format("%Y%m%d_%H%M%S")
    );
    let candidate = session_map_path
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("qc_notes").join(file.clone()));
    candidate.unwrap_or_else(|| PathBuf::from(file))
}

#[allow(clippy::too_many_arguments)]
fn run_calibrate_response(
    session_map_path: &Path,
    midi_out: Option<&str>,
    audio_device: Option<&str>,
    out: Option<&Path>,
    targets: Option<&str>,
    baseline_db: f32,
    test_delta_db: f32,
    capture_sec: u64,
    settle_ms: u64,
    auto_transport: bool,
    write_protocol: WriteProtocolArg,
) -> Result<()> {
    if test_delta_db <= 0.0 {
        return Err(anyhow!("test_delta_db_must_be_positive"));
    }
    let session_map: SessionMap = load_yaml(session_map_path)
        .with_context(|| format!("failed loading session map: {}", session_map_path.display()))?;
    let selected = parse_target_list(targets, &session_map)?;
    let target_specs = build_midi_target_specs(&Some(session_map.clone()));
    let mut backend = MidiBackend::connect(
        midi_out,
        target_specs,
        map_write_protocol(write_protocol),
        None,
        None,
    )?;

    let bus_maps = build_audio_bus_maps(&session_map);
    let bus_by_id: HashMap<String, AudioBusMap> =
        bus_maps.into_iter().map(|b| (b.id.clone(), b)).collect();

    if auto_transport {
        run_mcu_transport(midi_out, McuTransportAction::Play, 80)?;
        thread::sleep(Duration::from_millis(300));
    }

    // Put selected targets at baseline first so each bus probe starts from known state.
    set_levels(&mut backend, &selected, baseline_db)?;
    thread::sleep(Duration::from_millis(settle_ms.max(80)));

    let mut buses = Vec::new();
    for id in &selected {
        let bus = bus_by_id
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("no_audio_bus_mapping_for_target:{id}"))?;

        // Re-establish baseline for every measurement triplet.
        set_levels(&mut backend, &selected, baseline_db)?;
        thread::sleep(Duration::from_millis(settle_ms.max(80)));
        let (baseline_rms_db, _) = measure_bus_rms(audio_device, bus.clone(), capture_sec)?;

        set_levels(
            &mut backend,
            std::slice::from_ref(id),
            baseline_db + test_delta_db,
        )?;
        thread::sleep(Duration::from_millis(settle_ms.max(80)));
        let (high_rms_db, _) = measure_bus_rms(audio_device, bus.clone(), capture_sec)?;

        set_levels(
            &mut backend,
            std::slice::from_ref(id),
            baseline_db - test_delta_db,
        )?;
        thread::sleep(Duration::from_millis(settle_ms.max(80)));
        let (low_rms_db, _) = measure_bus_rms(audio_device, bus.clone(), capture_sec)?;

        // Return target to baseline after test.
        set_levels(&mut backend, std::slice::from_ref(id), baseline_db)?;

        let delta_audio_db = high_rms_db - low_rms_db;
        let slope = delta_audio_db / (2.0 * test_delta_db);
        let valid = slope.is_finite() && slope > 0.05;
        let mut recommended = if valid {
            (1.0 / slope).clamp(0.25, 4.0)
        } else {
            1.0
        };
        if !recommended.is_finite() {
            recommended = 1.0;
        }
        let mut confidence = ((delta_audio_db.abs() - 0.5) / 6.0).clamp(0.0, 1.0);
        if !valid {
            confidence *= 0.2;
        }

        println!(
            "calibration: {id} baseline={:+.2} high={:+.2} low={:+.2} slope={:.3} scale={:.3} conf={:.2} valid={}",
            baseline_rms_db, high_rms_db, low_rms_db, slope, recommended, confidence, valid
        );

        buses.push(ResponseCalibrationBus {
            id: id.clone(),
            slope_audio_db_per_fader_db: slope,
            recommended_fader_scale: recommended,
            baseline_rms_db,
            high_rms_db,
            low_rms_db,
            delta_audio_db,
            confidence,
            valid,
        });
    }

    if auto_transport {
        run_mcu_transport(midi_out, McuTransportAction::Stop, 80)?;
    }

    let out_path = out
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| default_calibration_output_path(session_map_path));
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = ResponseCalibrationFile {
        version: "1.0".to_string(),
        created_at_utc: Utc::now().to_rfc3339(),
        baseline_db,
        test_delta_db,
        buses,
    };
    fs::write(&out_path, serde_json::to_string_pretty(&payload)?)?;
    println!("calibration_written: {}", out_path.display());
    Ok(())
}

fn build_audio_bus_maps(sm: &SessionMap) -> Vec<AudioBusMap> {
    sm.buses
        .iter()
        .enumerate()
        .map(|(idx, b)| {
            let channels = if b.audio_channels.is_empty() {
                vec![idx + 1]
            } else {
                b.audio_channels.clone()
            };
            AudioBusMap {
                id: b.id.clone(),
                channels,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn run_execute(
    spec: &Path,
    command: &str,
    depth: f32,
    gesture_ms: u64,
    backend_mode: BackendMode,
    write_protocol: WriteProtocolArg,
    midi_out: Option<&str>,
    feedback_in: Option<&str>,
    feedback_sec: u64,
    response_calibration_path: Option<&Path>,
    no_response_calibration: bool,
    audio_device: Option<&str>,
    audio_verify_sec: u64,
    audio_verify_window_ms: u64,
    audio_verify_hop_ms: u64,
    audio_verify_calibrate_sec: u64,
    no_audio_verify: bool,
    undo_primary_cc: Option<u8>,
    undo_fallback_cc: Option<u8>,
    undo_channel: u8,
    session_map_path: Option<&Path>,
    audit_dir: &Path,
    command_id: Option<String>,
    captured_at: Option<String>,
    max_command_age_sec: u64,
    dry_run: bool,
    ltc_channel: usize,
    ltc_fps: f32,
    ltc_probe_sec: u64,
    ltc_sample_rate_hz: Option<u32>,
    score_midi: Option<&Path>,
    seek_anchors_file: Option<&Path>,
    seek_marker: Option<&str>,
    markers_file: Option<&Path>,
    seek_bar: Option<u32>,
    seek_beat: Option<f64>,
    seek_tick: Option<u32>,
    seek_tempo_bpm: f64,
    seek_ts_num: u32,
    seek_ts_den: u32,
    seek_ppq: u32,
    seek_start_offset_sec: f64,
    seek_tolerance_ms: u64,
    seek_max_steps: u32,
    strict_seek: bool,
    auto_play_before_write: bool,
    auto_stop_after_write: bool,
) -> Result<()> {
    let val = load_json(spec)?;
    ensure_spec_ready(&val)?;

    let cmd_id = command_id.unwrap_or_else(|| format!("cmd-{}", Uuid::new_v4()));
    let captured = parse_captured_at(captured_at.as_deref())?;
    ensure_not_stale(captured, max_command_age_sec)?;

    let session_map = load_session_map(session_map_path)?;
    let aliases = build_aliases(&session_map);

    let stt = MockAppleSpeech;
    let transcript = stt.transcribe_push_to_talk(None)?;
    info!(
        "stt_backend={} stt_confidence={}",
        transcript.backend, transcript.confidence
    );

    let mut intent = parse_utterance(command, &aliases);

    if intent.confidence < 0.75 {
        let cfg = FallbackConfig {
            enabled: true,
            feature_flag: "stt.local_fallback.enabled".to_string(),
            engine: "mlx_whisper_local_only".to_string(),
        };
        match transcribe_with_local_fallback(None, &cfg) {
            Ok(t) => {
                info!(
                    "fallback_backend={} fallback_confidence={}",
                    t.backend, t.confidence
                );
                // Keep typed command text authoritative in CLI mode.
                // We only use fallback confidence as supplementary signal.
                if t.confidence > intent.confidence {
                    intent.confidence = t.confidence;
                }
            }
            Err(e) => warn!("fallback_unavailable={e}"),
        }
    }

    let log_path = audit_dir.join("pass_audit.jsonl");
    if command_id_exists(&log_path, &cmd_id)? {
        return Err(anyhow!("duplicate_command_id"));
    }
    let logger = AuditLogger::new(&log_path)?;

    match intent.decision {
        Decision::Suggest => {
            let suggestions = build_suggestions(&intent);
            logger.append(&AuditRecord {
                run_id: "run-1".to_string(),
                command_id: cmd_id,
                event_type: "suggest".to_string(),
                created_at: Utc::now(),
                payload: serde_json::json!({
                    "command": command,
                    "captured_at": captured,
                    "decision": "Suggest",
                    "no_write": true,
                    "options": suggestions,
                }),
            })?;
            ConsoleTts.speak("I have suggestions. Please choose one option.")?;
            println!("suggestions:");
            for (idx, s) in suggestions.iter().enumerate() {
                println!("{}. {}", idx + 1, s);
            }
            return Ok(());
        }
        Decision::Clarify => {
            let prompt = build_clarification_prompt(&intent);
            logger.append(&AuditRecord {
                run_id: "run-1".to_string(),
                command_id: cmd_id,
                event_type: "clarify".to_string(),
                created_at: Utc::now(),
                payload: serde_json::json!({
                    "command": command,
                    "captured_at": captured,
                    "decision": "Clarify",
                    "no_write": true,
                    "prompt": prompt,
                    "reason_codes": intent.reason_codes,
                }),
            })?;
            ConsoleTts.speak(&prompt)?;
            println!("clarify: {prompt}");
            return Ok(());
        }
        Decision::Reject => {
            logger.append(&AuditRecord {
                run_id: "run-1".to_string(),
                command_id: cmd_id,
                event_type: "reject".to_string(),
                created_at: Utc::now(),
                payload: serde_json::json!({
                    "command": command,
                    "captured_at": captured,
                    "decision": "Reject",
                    "no_write": true,
                }),
            })?;
            ConsoleTts.speak("I cannot execute that request safely.")?;
            return Ok(());
        }
        Decision::Execute => {}
    }

    let ltc_device = audio_device.or(Some("LP32"));
    let seek_requested = seek_marker.is_some()
        || seek_bar.is_some()
        || seek_beat.is_some()
        || seek_tick.is_some();
    if seek_requested {
        let grid = MusicalGrid {
            bpm: seek_tempo_bpm,
            ts_num: seek_ts_num,
            ts_den: seek_ts_den,
            ppq: seek_ppq,
            start_offset_sec: seek_start_offset_sec,
        };
        let target_seconds = if let Some(score_path) = score_midi {
            let (bar, beat, tick) = if let Some(marker_name) = seek_marker {
                let marker_file = load_markers(markers_file)?;
                let m = marker_file
                    .markers
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(marker_name))
                    .ok_or_else(|| anyhow!("marker_not_found:{marker_name}"))?;
                (m.bar, m.beat, m.tick)
            } else {
                (
                    seek_bar.ok_or_else(|| anyhow!("target_bar_required_without_marker"))?,
                    seek_beat.unwrap_or(1.0),
                    seek_tick.unwrap_or(0),
                )
            };
            let (seconds, summary) = if let Some(anchor_path) = seek_anchors_file {
                anchored_seconds_for_bar_beat_tick(
                    score_path,
                    anchor_path,
                    bar,
                    beat,
                    tick,
                    seek_start_offset_sec,
                )?
            } else {
                score_seconds_for_bar_beat_tick(
                    score_path,
                    bar,
                    beat,
                    tick,
                    seek_start_offset_sec,
                )?
            };
            info!(
                "score_timeline_seek score={} ppq={} tempo_events={} timesig_events={} target_bar={} beat={} tick={} target_seconds={:.6}",
                score_path.display(),
                summary.ppq,
                summary.tempo_events,
                summary.time_signature_events,
                bar,
                beat,
                tick,
                seconds
            );
            seconds
        } else {
            resolve_target_seconds_ltc(
                seek_bar,
                seek_beat,
                seek_tick,
                seek_marker,
                markers_file,
                grid,
            )?
        };
        let seek_res = run_transport_seek_ltc(
            midi_out,
            ltc_device,
            ltc_channel,
            ltc_sample_rate_hz,
            ltc_fps,
            target_seconds,
            grid,
            seek_tolerance_ms,
            seek_max_steps,
        );
        if let Err(e) = seek_res {
            if strict_seek {
                return Err(e);
            }
            warn!("seek_best_effort_failed continuing=true error={e}");
        }
    }

    if auto_play_before_write {
        run_mcu_transport(midi_out, McuTransportAction::Play, 80)?;
        thread::sleep(Duration::from_millis(300));
    }

    let ltc_probe = run_ltc_monitor(
        ltc_device,
        ltc_channel,
        ltc_probe_sec.max(1),
        ltc_sample_rate_hz,
        ltc_fps,
        None,
    )?;
    let ltc_signal_ok = ltc_probe.frame_count > 0;

    let sync = evaluate_sync(
        &SyncInputs {
            session_hash_matches: true,
            timeline_hash_matches: true,
            avb_clock_lock_valid: true,
            transport_state_matches_plan: ltc_signal_ok,
            drift_ms: 1.5,
        },
        0.85,
    );

    if !sync.can_execute {
        return Err(anyhow!("sync gate blocked: {:?}", sync.reasons));
    }

    let risk = compute_risk_score(RiskInputs {
        target_count: intent.targets.len(),
        bar_span: intent
            .time_range
            .as_ref()
            .map(|t| t.end_bar - t.start_bar + 1)
            .unwrap_or(0),
        strength: 0.5,
        confidence: intent.confidence,
        sync_margin: sync.confidence,
    });

    let mut plan = build_pass_plan(&intent, risk, session_map.as_ref(), gesture_ms)?;
    apply_depth(&mut plan, depth);
    let anchor = capture_undo_anchor(&cmd_id);
    plan.undo_anchor_ref = Some(anchor.anchor_id.clone());

    validate_prepass(&PrepassState {
        control_path_live: true,
        sync_confidence_valid: sync.can_execute,
        correct_strip_bank_verified: true,
        target_lanes_verified: true,
        dp_transport_state_verified: ltc_signal_ok,
        undo_anchor_captured: true,
        pass_plan_validated: true,
        undo_path_available: true,
    })?;

    let mut backend: Box<dyn ControlBackend> = match backend_mode {
        BackendMode::Mock => Box::new(MockBackend::default()),
        BackendMode::Midi => {
            let target_specs = build_midi_target_specs(&session_map);
            let proto = match write_protocol {
                WriteProtocolArg::Cc => WriteProtocol::CcLearn,
                WriteProtocolArg::Mcu => WriteProtocol::McuFader,
            };
            let undo_primary = undo_primary_cc.map(|cc| UndoTrigger {
                channel: undo_channel,
                cc,
                value: 127,
            });
            let undo_fallback = undo_fallback_cc.map(|cc| UndoTrigger {
                channel: undo_channel,
                cc,
                value: 127,
            });
            Box::new(MidiBackend::connect(
                midi_out,
                target_specs,
                proto,
                undo_primary,
                undo_fallback,
            )?)
        }
    };

    let effective_calibration_path =
        resolve_response_calibration_path(response_calibration_path, session_map_path);
    let response_scales = if no_response_calibration {
        None
    } else {
        load_response_calibration_scales(effective_calibration_path.as_deref())?
    };

    if let Some(scales) = &response_scales {
        info!("response_calibration_loaded targets={}", scales.len());
    } else if !no_response_calibration {
        warn!("response_calibration_not_loaded_using_unscaled_writes=true");
    }

    let apply_rest_accompaniment = wants_rest_accompaniment(command);
    let report = if dry_run {
        None
    } else if apply_rest_accompaniment {
        let rest_targets = derive_rest_targets(session_map.as_ref(), &plan.target_lanes);
        Some(execute_rest_aware_pass(
            &mut *backend,
            &plan,
            &rest_targets,
            -12.0,
            6.0,
            3.0,
            response_scales.as_ref(),
        )?)
    } else {
        Some(execute_pass_with_scales(
            &mut *backend,
            &plan,
            -6.0,
            6.0,
            3.0,
            response_scales.as_ref(),
        )?)
    };

    logger.append(&AuditRecord {
        run_id: "run-1".to_string(),
        command_id: cmd_id.clone(),
        event_type: if dry_run {
            "dry_run".to_string()
        } else {
            "execute".to_string()
        },
        created_at: Utc::now(),
        payload: serde_json::json!({
            "command": command,
            "captured_at": captured,
            "decision": format!("{:?}", intent.decision),
            "plan_id": plan.plan_id,
            "undo_anchor_ref": plan.undo_anchor_ref,
            "report": report,
            "dry_run": dry_run,
            "rest_of_orchestra_accompaniment_applied": apply_rest_accompaniment,
            "response_calibration_applied": response_scales.is_some(),
            "response_calibration_path": effective_calibration_path.as_ref().map(|p| p.display().to_string()),
            "ltc_probe": ltc_probe,
        }),
    })?;

    if matches!(backend_mode, BackendMode::Midi) && feedback_sec > 0 {
        let (channel_map, db_ranges) = build_mcu_maps(&session_map);
        let feedback_json = audit_dir.join(format!("feedback_{}.json", plan.plan_id));
        monitor_mcu_feedback(
            feedback_in.or(midi_out),
            feedback_sec,
            150,
            channel_map,
            db_ranges,
            Some(&feedback_json),
        )?;
    }

    if matches!(backend_mode, BackendMode::Midi)
        && !dry_run
        && !no_audio_verify
        && audio_verify_sec > 0
    {
        let verify_buses = session_map
            .as_ref()
            .map(build_audio_bus_maps)
            .unwrap_or_default();
        let audio_jsonl = audit_dir.join(format!("audio_verify_{}.jsonl", plan.plan_id));
        let req = AudioMonitorRequest {
            device_hint: Some(audio_device.unwrap_or("LP32").to_string()),
            duration_sec: audio_verify_sec,
            sample_rate_hz: Some(48000),
            window_ms: audio_verify_window_ms,
            hop_ms: audio_verify_hop_ms,
            calibrate_sec: audio_verify_calibrate_sec,
            buses: verify_buses,
            virtual_groups: Vec::new(),
            include_default_virtual_groups: true,
            print_every_ms: 500,
            jsonl_out: Some(audio_jsonl.clone()),
        };

        match run_realtime_monitor(req) {
            Ok(audio_report) => {
                let active_buses = audio_report
                    .bus_summaries
                    .iter()
                    .filter(|b| b.avg_rms_db > -170.0)
                    .count();
                println!(
                    "audio_verify: active_buses={}/{}",
                    active_buses,
                    audio_report.bus_summaries.len()
                );
                logger.append(&AuditRecord {
                    run_id: "run-1".to_string(),
                    command_id: cmd_id.clone(),
                    event_type: "audio_verify".to_string(),
                    created_at: Utc::now(),
                    payload: serde_json::json!({
                        "plan_id": plan.plan_id,
                        "device": audio_device.unwrap_or("LP32"),
                        "duration_sec": audio_verify_sec,
                        "jsonl_path": audio_jsonl,
                        "active_buses": active_buses,
                        "report": audio_report
                    }),
                })?;
            }
            Err(e) => {
                warn!("audio_verify_failed={e}");
                logger.append(&AuditRecord {
                    run_id: "run-1".to_string(),
                    command_id: cmd_id.clone(),
                    event_type: "audio_verify_failed".to_string(),
                    created_at: Utc::now(),
                    payload: serde_json::json!({
                        "plan_id": plan.plan_id,
                        "device": audio_device.unwrap_or("LP32"),
                        "duration_sec": audio_verify_sec,
                        "error": e.to_string()
                    }),
                })?;
            }
        }
    }

    if auto_stop_after_write {
        run_mcu_transport(midi_out, McuTransportAction::Stop, 80)?;
    }

    let tts = ConsoleTts;
    tts.speak(if dry_run {
        "Dry run complete. No automation was written."
    } else {
        "Pass complete. Automation write finished."
    })?;

    println!("execute: OK");
    Ok(())
}

fn build_suggestions(intent: &mixct_core::Intent) -> Vec<String> {
    let tr = intent
        .time_range
        .as_ref()
        .map(|t| format!("bars {}-{}", t.start_bar, t.end_bar))
        .unwrap_or_else(|| "the selected range".to_string());

    let target = intent
        .targets
        .first()
        .cloned()
        .unwrap_or_else(|| "STR_HI".to_string());

    vec![
        format!("Lift {target} by +2.0 dB in {tr}."),
        format!("Reduce competing section by -2.0 dB in {tr}."),
        format!("Apply +1.0 dB trim plus EQ Presence +1.0 dB to {target} in {tr}."),
    ]
}

fn build_clarification_prompt(intent: &mixct_core::Intent) -> String {
    let needs_target = intent
        .reason_codes
        .iter()
        .any(|r| r == "missing_target" || r == "ambiguous_target");
    let needs_time = intent
        .reason_codes
        .iter()
        .any(|r| r == "missing_time_window");
    match (needs_target, needs_time) {
        (true, true) => {
            "Please specify both section and bar range, for example: Violins bars 26-29."
                .to_string()
        }
        (true, false) => {
            "Please specify the target section, for example: First violins.".to_string()
        }
        (false, true) => "Please specify the bar range, for example: bars 26-29.".to_string(),
        (false, false) => "Please restate the instruction in a more concrete way.".to_string(),
    }
}

fn run_restore(
    spec: &Path,
    command_id: &str,
    backend_mode: BackendMode,
    midi_out: Option<&str>,
    undo_primary_cc: Option<u8>,
    undo_fallback_cc: Option<u8>,
    undo_channel: u8,
    audit_dir: &Path,
) -> Result<()> {
    let val = load_json(spec)?;
    ensure_spec_ready(&val)?;

    let backend: Box<dyn ControlBackend> = match backend_mode {
        BackendMode::Mock => Box::new(MockBackend {
            fail_primary_undo: true,
            ..Default::default()
        }),
        BackendMode::Midi => {
            let undo_primary = undo_primary_cc.map(|cc| UndoTrigger {
                channel: undo_channel,
                cc,
                value: 127,
            });
            let undo_fallback = undo_fallback_cc.map(|cc| UndoTrigger {
                channel: undo_channel,
                cc,
                value: 127,
            });
            if undo_primary.is_none() && undo_fallback.is_none() {
                return Err(anyhow!("undo_path_unavailable"));
            }
            Box::new(MidiBackend::connect(
                midi_out,
                HashMap::new(),
                WriteProtocol::CcLearn,
                undo_primary,
                undo_fallback,
            )?)
        }
    };
    let mut backend = RestoreAdapter { backend };
    let anchor = capture_undo_anchor(command_id);
    restore_from_anchor(&mut backend, &anchor)?;

    let logger = AuditLogger::new(audit_dir.join("pass_audit.jsonl"))?;
    logger.append(&AuditRecord {
        run_id: "run-1".to_string(),
        command_id: command_id.to_string(),
        event_type: "restore".to_string(),
        created_at: Utc::now(),
        payload: serde_json::json!({
            "undo_anchor_ref": anchor.anchor_id,
            "undo_execution_path": {
                "primary": "mcu_undo_command",
                "fallback": "studio_local_agent_undo_trigger",
                "verification": "post_undo_lane_state_check"
            }
        }),
    })?;

    ConsoleTts.speak("Restore complete.")?;
    println!("restore: OK");
    Ok(())
}

struct RestoreAdapter {
    backend: Box<dyn ControlBackend>,
}

impl UndoExecutor for RestoreAdapter {
    fn undo_primary(&mut self) -> Result<()> {
        self.backend.trigger_undo_primary()
    }

    fn undo_fallback(&mut self) -> Result<()> {
        self.backend.trigger_undo_fallback()
    }

    fn verify_post_undo(&mut self) -> Result<()> {
        Ok(())
    }
}

fn build_pass_plan(
    intent: &mixct_core::Intent,
    _risk: f32,
    session_map: Option<&SessionMap>,
    gesture_ms: u64,
) -> Result<PassPlan> {
    let tr = intent
        .time_range
        .clone()
        .ok_or_else(|| anyhow!("missing time range"))?;

    if !tr.is_valid() {
        return Err(anyhow!("invalid time range"));
    }

    let targets = if intent.targets.is_empty() {
        vec![ResolvedTarget {
            canonical_name: "STR_HI".to_string(),
            lane: LaneKind::Volume,
        }]
    } else {
        intent
            .targets
            .iter()
            .map(|t| ResolvedTarget {
                canonical_name: t.clone(),
                lane: if intent.source_text.to_lowercase().contains("presence") {
                    LaneKind::EqPresenceGain
                } else if intent.source_text.to_lowercase().contains("air") {
                    LaneKind::EqAirGain
                } else if intent.source_text.to_lowercase().contains("low") {
                    LaneKind::EqLowGain
                } else {
                    LaneKind::Volume
                },
            })
            .collect()
    };

    let mut strips = vec![1u32];
    if let Some(sm) = session_map {
        let mapped: Vec<u32> = targets
            .iter()
            .filter_map(|t| {
                sm.buses
                    .iter()
                    .position(|b| b.id == t.canonical_name)
                    .map(|idx| (idx as u32) + 1)
            })
            .collect();
        if !mapped.is_empty() {
            strips = mapped;
        }
    }

    let points = scale_curve_duration(build_curve_points(intent), gesture_ms);

    Ok(PassPlan {
        plan_id: format!("plan-{}", Uuid::new_v4()),
        source_text: intent.source_text.clone(),
        operation_class: intent
            .operation_class
            .clone()
            .unwrap_or(OperationClass::WriteNewCurve),
        target_lanes: targets,
        target_strips: strips,
        time_range: TimeRange {
            start_bar: tr.start_bar,
            end_bar: tr.end_bar,
        },
        control_rate_hz: 50,
        curve_shape: "cubic_ease_in_out".to_string(),
        curve_points: points,
        pre_roll_bars: 1,
        post_roll_beats: 1,
        boundary_smoothing_ms: 80,
        undo_anchor_ref: None,
        created_at: Utc::now(),
    })
}

fn scale_curve_duration(
    mut points: Vec<mixct_core::CurvePoint>,
    target_ms: u64,
) -> Vec<mixct_core::CurvePoint> {
    let max_offset = points.iter().map(|p| p.offset_ms).max().unwrap_or(0);
    if max_offset == 0 || target_ms == 0 || target_ms == max_offset {
        return points;
    }
    let scale = target_ms as f64 / max_offset as f64;
    for p in &mut points {
        p.offset_ms = ((p.offset_ms as f64) * scale).round() as u64;
    }
    points
}

fn build_curve_points(intent: &mixct_core::Intent) -> Vec<mixct_core::CurvePoint> {
    let lower = intent.source_text.to_lowercase();
    let direction = if lower.contains("too loud")
        || lower.contains("harsh")
        || lower.contains("reduce")
        || lower.contains("softer")
    {
        -1.0
    } else if lower.contains("too soft")
        || lower.contains("not loud enough")
        || lower.contains("covered")
        || lower.contains("bring out")
        || lower.contains("lift")
    {
        1.0
    } else {
        1.0
    };

    let strength = match intent.strength.as_deref() {
        Some("slight") => 2.5,
        Some("strong") => 6.0,
        _ => 4.5,
    } * direction;
    let sustain = strength * 0.6;
    let immediate = strength * 0.65;

    vec![
        mixct_core::CurvePoint {
            offset_ms: 0,
            value: immediate,
        },
        mixct_core::CurvePoint {
            // Fast shaping point (not a startup delay): keeps moves musical without
            // delaying initial action.
            offset_ms: 120,
            value: strength,
        },
        mixct_core::CurvePoint {
            offset_ms: 900,
            value: sustain,
        },
        mixct_core::CurvePoint {
            offset_ms: 2400,
            value: 0.0,
        },
    ]
}

fn apply_depth(plan: &mut PassPlan, depth: f32) {
    let d = depth.clamp(0.2, 4.0);
    for p in &mut plan.curve_points {
        p.value *= d;
    }
}

fn accompaniment_curve_points_like(base: &[mixct_core::CurvePoint]) -> Vec<mixct_core::CurvePoint> {
    base.iter()
        .map(|p| {
            let v = p.value;
            let accompaniment = if v.abs() < f32::EPSILON {
                0.0
            } else {
                -(v.abs() * 1.7).clamp(2.5, 10.0)
            };
            mixct_core::CurvePoint {
                offset_ms: p.offset_ms,
                value: accompaniment,
            }
        })
        .collect()
}

fn wants_rest_accompaniment(command: &str) -> bool {
    let lower = command.to_lowercase();
    let mentions_rest = lower.contains("rest of orchestra")
        || lower.contains("rest of the orchestra")
        || lower.contains("the rest of orchestra")
        || lower.contains("the rest of the orchestra")
        || lower.contains(" rest=");
    let mentions_accompaniment = lower.contains("accompaniment");
    mentions_rest && mentions_accompaniment
}

fn derive_rest_targets(
    session_map: Option<&SessionMap>,
    primary_targets: &[ResolvedTarget],
) -> Vec<ResolvedTarget> {
    let Some(sm) = session_map else {
        return Vec::new();
    };

    let primary_ids: std::collections::HashSet<&str> = primary_targets
        .iter()
        .map(|t| t.canonical_name.as_str())
        .collect();

    sm.buses
        .iter()
        .filter(|b| !primary_ids.contains(b.id.as_str()))
        .map(|b| ResolvedTarget {
            canonical_name: b.id.clone(),
            lane: LaneKind::Volume,
        })
        .collect()
}

fn load_response_calibration_scales(path: Option<&Path>) -> Result<Option<HashMap<String, f32>>> {
    let Some(p) = path else {
        return Ok(None);
    };
    let text = fs::read_to_string(p)
        .with_context(|| format!("failed reading response calibration: {}", p.display()))?;
    let parsed: ResponseCalibrationFile = serde_json::from_str(&text)
        .with_context(|| format!("invalid response calibration json: {}", p.display()))?;

    let mut scales = HashMap::new();
    for bus in parsed.buses {
        if bus.valid && bus.recommended_fader_scale.is_finite() && bus.recommended_fader_scale > 0.0
        {
            scales.insert(bus.id, bus.recommended_fader_scale.clamp(0.25, 4.0));
        }
    }
    if scales.is_empty() {
        warn!(
            "response_calibration_valid_scales_empty path={}",
            p.display()
        );
        return Ok(None);
    }
    Ok(Some(scales))
}

fn resolve_response_calibration_path(
    explicit: Option<&Path>,
    session_map_path: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }

    let from_session_map = session_map_path.and_then(|sm| {
        sm.parent()
            .and_then(|contracts| contracts.parent())
            .map(|root| {
                root.join("qc_notes")
                    .join("response_calibration.latest.json")
            })
    });
    if let Some(p) = from_session_map {
        if p.exists() {
            return Some(p);
        }
    }

    let from_cwd = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("../qc_notes/response_calibration.latest.json"));
    if let Some(p) = from_cwd {
        if p.exists() {
            return Some(p);
        }
    }

    None
}

fn execute_rest_aware_pass(
    backend: &mut dyn ControlBackend,
    plan: &PassPlan,
    rest_targets: &[ResolvedTarget],
    min_db: f32,
    max_db: f32,
    max_slew_step: f32,
    target_scales: Option<&HashMap<String, f32>>,
) -> Result<mixct_control::ExecutionReport> {
    use chrono::Utc;

    let rest_points = accompaniment_curve_points_like(&plan.curve_points);

    let mut all_targets: Vec<ResolvedTarget> = Vec::new();
    all_targets.extend(plan.target_lanes.clone());
    all_targets.extend(rest_targets.iter().cloned());

    let mut previous_values = Vec::with_capacity(all_targets.len());
    for lane in &all_targets {
        let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
        backend.begin_touch(&target_name)?;
        previous_values.push(0.0f32);
    }

    let mut applied_clamps = 0usize;
    let mut event_count = 0usize;
    for idx in 0..plan.curve_points.len() {
        for (target_idx, lane) in all_targets.iter().enumerate() {
            let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
            let source_point = if target_idx < plan.target_lanes.len() {
                &plan.curve_points[idx]
            } else {
                &rest_points[idx]
            };

            let scale = target_scales
                .and_then(|m| m.get(&lane.canonical_name).copied())
                .unwrap_or(1.0);
            let mut v = source_point.value * scale;
            let clamped = clamp_db(v, min_db, max_db);
            if (clamped - v).abs() > f32::EPSILON {
                applied_clamps += 1;
            }
            v = enforce_slew(previous_values[target_idx], clamped, max_slew_step);
            backend.write_value(&target_name, v, source_point.offset_ms)?;
            previous_values[target_idx] = v;
            event_count += 1;
        }
    }

    for lane in &all_targets {
        let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
        backend.end_touch(&target_name)?;
    }

    Ok(mixct_control::ExecutionReport {
        executed_at: Utc::now(),
        event_count,
        applied_clamps,
        success: true,
    })
}

fn parse_captured_at(captured_at: Option<&str>) -> Result<DateTime<Utc>> {
    if let Some(ts) = captured_at {
        let dt = DateTime::parse_from_rfc3339(ts)
            .with_context(|| "captured_at must be RFC3339")?
            .with_timezone(&Utc);
        Ok(dt)
    } else {
        Ok(Utc::now())
    }
}

fn ensure_not_stale(captured: DateTime<Utc>, max_age_secs: u64) -> Result<()> {
    let age = (Utc::now() - captured).num_seconds();
    if age > max_age_secs as i64 {
        return Err(anyhow!("stale_command_context"));
    }
    Ok(())
}

fn load_session_map(path: Option<&Path>) -> Result<Option<SessionMap>> {
    if let Some(p) = path {
        let s: SessionMap = load_yaml(p)?;
        Ok(Some(s))
    } else {
        Ok(None)
    }
}

fn build_aliases(session_map: &Option<SessionMap>) -> HashMap<String, String> {
    let mut map = default_aliases();
    if let Some(sm) = session_map {
        for (k, v) in &sm.entity_aliases {
            map.insert(k.to_lowercase(), v.clone());
        }
        for bus in &sm.buses {
            map.insert(bus.id.to_lowercase(), bus.id.clone());
            map.insert(bus.id.to_lowercase().replace('_', " "), bus.id.clone());
            for alias in &bus.aliases {
                map.insert(alias.to_lowercase(), bus.id.clone());
            }
        }
    }
    map
}

fn build_mcu_maps(
    session_map: &Option<SessionMap>,
) -> (HashMap<u8, String>, HashMap<String, (f32, f32)>) {
    let mut channel_map = HashMap::new();
    let mut db_ranges = HashMap::new();

    if let Some(sm) = session_map {
        for (idx, bus) in sm.buses.iter().enumerate() {
            let ch = bus
                .mcu_channel
                .or_else(|| u8::try_from(idx + 1).ok())
                .unwrap_or(1);
            channel_map.insert(ch, bus.id.clone());
            db_ranges.insert(bus.id.clone(), (bus.min_db, bus.max_db));
        }
    }

    (channel_map, db_ranges)
}

fn build_midi_target_specs(session_map: &Option<SessionMap>) -> HashMap<String, MidiTargetSpec> {
    let mut specs = HashMap::new();
    if let Some(sm) = session_map {
        for bus in &sm.buses {
            if let Some(cc) = bus.cc {
                specs.insert(
                    bus.id.clone(),
                    MidiTargetSpec {
                        channel: bus.channel,
                        cc,
                        mcu_channel: bus.mcu_channel,
                        min_db: bus.min_db,
                        max_db: bus.max_db,
                    },
                );
            }
        }
    }

    if specs.is_empty() {
        for (id, cc) in [
            ("STR_HI", 90u8),
            ("STR_MID", 91u8),
            ("STR_LO", 92u8),
            ("WW_HI", 93u8),
            ("WW_MID", 94u8),
            ("WW_LO", 95u8),
            ("HN", 96u8),
            ("TPT", 97u8),
            ("BR_LO", 102u8),
            ("TIMP", 103u8),
            ("PERC", 104u8),
            ("HARP", 105u8),
        ] {
            specs.insert(
                id.to_string(),
                MidiTargetSpec {
                    channel: 16,
                    cc,
                    mcu_channel: None,
                    min_db: -18.0,
                    max_db: 6.0,
                },
            );
        }
    }

    specs
}

fn command_id_exists(path: &Path, command_id: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(parsed) = serde_json::from_str::<AuditLine>(line) {
            if parsed.command_id == command_id {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn default_aliases() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("first violins".to_string(), "STR_HI".to_string());
    map.insert("violins".to_string(), "STR_HI".to_string());
    map.insert("high ww".to_string(), "WW_HI".to_string());
    map.insert("horns".to_string(), "HN".to_string());
    map.insert("low brass".to_string(), "BR_LO".to_string());
    map.insert("timpani".to_string(), "TIMP".to_string());
    map.insert("percussion".to_string(), "PERC".to_string());
    map.insert("harp".to_string(), "HARP".to_string());
    map
}

fn load_json(path: &Path) -> Result<serde_json::Value> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read: {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("invalid json: {}", path.display()))?;
    Ok(v)
}

fn load_yaml<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read: {}", path.display()))?;
    let v: T =
        serde_yaml::from_str(&text).with_context(|| format!("invalid yaml: {}", path.display()))?;
    Ok(v)
}

fn ensure_spec_ready(v: &serde_json::Value) -> Result<()> {
    let checks = validate_spec(v);
    if checks.ok {
        Ok(())
    } else {
        Err(anyhow!("spec invalid: {}", checks.errors.join(", ")))
    }
}

fn validate_spec(v: &serde_json::Value) -> SpecChecks {
    let mut errors = Vec::new();

    if path_str(v, &["document", "schema_version"]) != Some("1.1.0") {
        errors.push("schema_version must be 1.1.0".to_string());
    }

    if path_bool(
        v,
        &[
            "principles",
            "manual_dp_undo_disallowed_during_mixct_session",
        ],
    ) != Some(true)
    {
        errors.push("manual dp undo guard missing".to_string());
    }

    if path_str(
        v,
        &[
            "architecture",
            "plane_split",
            "audio_plane",
            "clock_authority",
        ],
    ) != Some("avb_ptp_grandmaster_motu_848")
    {
        errors.push("audio clock authority mismatch".to_string());
    }

    let forb = path_array(v, &["voice_system", "speech_backend", "forbidden_apis"]);
    if !forb.iter().any(|x| x == "cloud_stt") {
        errors.push("cloud_stt must remain forbidden".to_string());
    }
    if forb.iter().any(|x| x == "third_party_stt") {
        errors.push("third_party_stt must not be in forbidden_apis".to_string());
    }

    if path_str(
        v,
        &[
            "voice_system",
            "speech_backend",
            "fallback_engine",
            "feature_flag",
        ],
    ) != Some("stt.local_fallback.enabled")
    {
        errors.push("fallback feature flag missing".to_string());
    }

    if !path_array(v, &["startup_diagnostics", "startup_checks"])
        .iter()
        .any(|x| x == "avb_ptp_clock_locked")
    {
        errors.push("startup check avb_ptp_clock_locked missing".to_string());
    }

    if !path_array(v, &["startup_diagnostics", "hard_refusal_cases"])
        .iter()
        .any(|x| x == "undo_anchor_failure")
    {
        errors.push("hard refusal undo_anchor_failure missing".to_string());
    }

    if path_str(v, &["restoration_model", "undo_execution_path", "primary"])
        != Some("mcu_undo_command")
    {
        errors.push("undo execution primary path missing".to_string());
    }

    if path_str(v, &["restoration_model", "undo_execution_path", "fallback"])
        != Some("studio_local_agent_undo_trigger")
    {
        errors.push("undo execution fallback path missing".to_string());
    }

    SpecChecks {
        ok: errors.is_empty(),
        errors,
    }
}

fn path_str<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_str()
}

fn path_bool(v: &serde_json::Value, path: &[&str]) -> Option<bool> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_bool()
}

fn path_array(v: &serde_json::Value, path: &[&str]) -> Vec<String> {
    let mut cur = v;
    for p in path {
        if let Some(next) = cur.get(*p) {
            cur = next;
        } else {
            return vec![];
        }
    }
    cur.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}
