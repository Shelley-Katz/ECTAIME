use anyhow::{anyhow, Context, Result};
use midly::{MetaMessage, Smf, Timing, TrackEventKind};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct TempoPoint {
    pub tick: u64,
    pub us_per_quarter: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimeSigPoint {
    pub tick: u64,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Debug, Clone)]
struct TempoSegment {
    start_tick: u64,
    end_tick: Option<u64>,
    start_seconds: f64,
    us_per_tick: f64,
}

#[derive(Debug, Clone)]
struct MeterSegment {
    start_tick: u64,
    start_bar: u32, // 1-based
    ticks_per_beat: f64,
    ticks_per_bar: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoreTimelineSummary {
    pub ppq: u16,
    pub tempo_events: usize,
    pub time_signature_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreAnchorPoint {
    pub bar: u32,
    #[serde(default = "default_anchor_beat")]
    pub beat: f64,
    #[serde(default)]
    pub tick: u32,
    pub ltc_seconds: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreAnchorMap {
    pub anchors: Vec<ScoreAnchorPoint>,
}

#[derive(Debug, Clone)]
pub struct ScoreTimeline {
    ppq: u16,
    tempo_points: Vec<TempoPoint>,
    time_sig_points: Vec<TimeSigPoint>,
    tempo_segments: Vec<TempoSegment>,
    meter_segments: Vec<MeterSegment>,
}

impl ScoreTimeline {
    pub fn from_midi_file(path: &Path) -> Result<Self> {
        let data = fs::read(path)
            .with_context(|| format!("failed reading score midi: {}", path.display()))?;
        let smf =
            Smf::parse(&data).with_context(|| format!("invalid midi: {}", path.display()))?;

        let ppq = match smf.header.timing {
            Timing::Metrical(t) => t.as_int(),
            Timing::Timecode(_fps, _subframe) => {
                return Err(anyhow!(
                    "unsupported_smpte_timing_in_score_midi (expected PPQ metrical timing)"
                ));
            }
        };

        let mut tempo_points = collect_tempo_points(&smf);
        let mut time_sig_points = collect_time_sig_points(&smf);

        if tempo_points.is_empty() || tempo_points[0].tick != 0 {
            tempo_points.insert(
                0,
                TempoPoint {
                    tick: 0,
                    us_per_quarter: 500_000, // 120 BPM default MIDI
                },
            );
        }
        if time_sig_points.is_empty() || time_sig_points[0].tick != 0 {
            time_sig_points.insert(
                0,
                TimeSigPoint {
                    tick: 0,
                    numerator: 4,
                    denominator: 4,
                },
            );
        }

        let tempo_segments = build_tempo_segments(ppq, &tempo_points);
        let meter_segments = build_meter_segments(ppq, &time_sig_points);

        Ok(Self {
            ppq,
            tempo_points,
            time_sig_points,
            tempo_segments,
            meter_segments,
        })
    }

    pub fn summary(&self) -> ScoreTimelineSummary {
        ScoreTimelineSummary {
            ppq: self.ppq,
            tempo_events: self.tempo_points.len(),
            time_signature_events: self.time_sig_points.len(),
        }
    }

    pub fn tick_at_bar_beat_tick(&self, bar: u32, beat: f64, tick: u32) -> Result<u64> {
        if bar == 0 {
            return Err(anyhow!("bar_must_be_1_based"));
        }
        let beat_clamped = beat.max(1.0);
        let segment_idx = self.find_meter_segment_for_bar(bar)?;
        let seg = &self.meter_segments[segment_idx];
        let bar_offset = bar.saturating_sub(seg.start_bar) as f64;
        let tick_f = seg.start_tick as f64
            + bar_offset * seg.ticks_per_bar
            + (beat_clamped - 1.0) * seg.ticks_per_beat
            + tick as f64;
        Ok(tick_f.round().max(0.0) as u64)
    }

    pub fn seconds_at_tick(&self, tick: u64) -> f64 {
        let idx = self.find_tempo_segment_for_tick(tick);
        let seg = &self.tempo_segments[idx];
        seg.start_seconds + (tick.saturating_sub(seg.start_tick) as f64) * seg.us_per_tick / 1e6
    }

    pub fn seconds_at_bar_beat_tick(
        &self,
        bar: u32,
        beat: f64,
        tick: u32,
        timeline_offset_seconds: f64,
    ) -> Result<f64> {
        let abs_tick = self.tick_at_bar_beat_tick(bar, beat, tick)?;
        Ok(timeline_offset_seconds + self.seconds_at_tick(abs_tick))
    }

    fn find_tempo_segment_for_tick(&self, tick: u64) -> usize {
        let mut idx = 0usize;
        for (i, seg) in self.tempo_segments.iter().enumerate() {
            let in_seg = if let Some(end_tick) = seg.end_tick {
                tick >= seg.start_tick && tick < end_tick
            } else {
                tick >= seg.start_tick
            };
            if in_seg {
                idx = i;
                break;
            }
        }
        idx
    }

    fn find_meter_segment_for_bar(&self, bar: u32) -> Result<usize> {
        for (i, seg) in self.meter_segments.iter().enumerate() {
            let next_bar = self
                .meter_segments
                .get(i + 1)
                .map(|s| s.start_bar)
                .unwrap_or(u32::MAX);
            if bar >= seg.start_bar && bar < next_bar {
                return Ok(i);
            }
        }
        Err(anyhow!("bar_out_of_range_in_timeline:{}", bar))
    }
}

pub fn score_seconds_for_bar_beat_tick(
    score_midi: &Path,
    bar: u32,
    beat: f64,
    tick: u32,
    timeline_offset_seconds: f64,
) -> Result<(f64, ScoreTimelineSummary)> {
    let timeline = ScoreTimeline::from_midi_file(score_midi)?;
    let seconds = timeline.seconds_at_bar_beat_tick(bar, beat, tick, timeline_offset_seconds)?;
    Ok((seconds, timeline.summary()))
}

pub fn anchored_seconds_for_bar_beat_tick(
    score_midi: &Path,
    anchors_file: &Path,
    bar: u32,
    beat: f64,
    tick: u32,
    timeline_offset_seconds: f64,
) -> Result<(f64, ScoreTimelineSummary)> {
    let timeline = ScoreTimeline::from_midi_file(score_midi)?;
    let target_score_seconds =
        timeline.seconds_at_bar_beat_tick(bar, beat, tick, timeline_offset_seconds)?;
    let anchor_map = load_anchor_map(anchors_file)?;
    if anchor_map.anchors.len() < 2 {
        return Err(anyhow!("anchor_map_requires_at_least_2_points"));
    }

    let mut points: Vec<(f64, f64)> = Vec::new(); // (score_seconds, ltc_seconds)
    for a in &anchor_map.anchors {
        let score_sec =
            timeline.seconds_at_bar_beat_tick(a.bar, a.beat, a.tick, timeline_offset_seconds)?;
        points.push((score_sec, a.ltc_seconds));
    }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    points.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
    if points.len() < 2 {
        return Err(anyhow!("anchor_map_collapsed_to_less_than_2_unique_points"));
    }

    let (x1, y1, x2, y2) = if target_score_seconds <= points[0].0 {
        (points[0].0, points[0].1, points[1].0, points[1].1)
    } else if target_score_seconds >= points[points.len() - 1].0 {
        let n = points.len();
        (
            points[n - 2].0,
            points[n - 2].1,
            points[n - 1].0,
            points[n - 1].1,
        )
    } else {
        let mut seg = (points[0].0, points[0].1, points[1].0, points[1].1);
        for i in 0..(points.len() - 1) {
            let a = points[i];
            let b = points[i + 1];
            if target_score_seconds >= a.0 && target_score_seconds <= b.0 {
                seg = (a.0, a.1, b.0, b.1);
                break;
            }
        }
        seg
    };

    let dx = (x2 - x1).abs();
    let target_ltc_seconds = if dx < 1e-9 {
        y1
    } else {
        let t = (target_score_seconds - x1) / (x2 - x1);
        y1 + t * (y2 - y1)
    };

    Ok((target_ltc_seconds, timeline.summary()))
}

fn default_anchor_beat() -> f64 {
    1.0
}

fn load_anchor_map(path: &Path) -> Result<ScoreAnchorMap> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed reading {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let parsed = if ext == "json" {
        serde_json::from_str::<ScoreAnchorMap>(&content)
            .with_context(|| format!("invalid JSON anchor map: {}", path.display()))?
    } else {
        serde_yaml::from_str::<ScoreAnchorMap>(&content)
            .with_context(|| format!("invalid YAML anchor map: {}", path.display()))?
    };
    Ok(parsed)
}

fn collect_tempo_points(smf: &Smf<'_>) -> Vec<TempoPoint> {
    let mut out: Vec<TempoPoint> = Vec::new();
    for track in &smf.tracks {
        let mut tick_acc = 0u64;
        for ev in track {
            tick_acc = tick_acc.saturating_add(ev.delta.as_int() as u64);
            if let TrackEventKind::Meta(MetaMessage::Tempo(v)) = ev.kind {
                out.push(TempoPoint {
                    tick: tick_acc,
                    us_per_quarter: v.as_int(),
                });
            }
        }
    }
    out.sort_by_key(|t| t.tick);
    dedup_tempo_points(out)
}

fn collect_time_sig_points(smf: &Smf<'_>) -> Vec<TimeSigPoint> {
    let mut out: Vec<TimeSigPoint> = Vec::new();
    for track in &smf.tracks {
        let mut tick_acc = 0u64;
        for ev in track {
            tick_acc = tick_acc.saturating_add(ev.delta.as_int() as u64);
            if let TrackEventKind::Meta(MetaMessage::TimeSignature(num, den_pow, _, _)) = ev.kind {
                let denominator = 2u8.saturating_pow(den_pow as u32);
                out.push(TimeSigPoint {
                    tick: tick_acc,
                    numerator: num,
                    denominator: denominator.max(1),
                });
            }
        }
    }
    out.sort_by_key(|t| t.tick);
    dedup_time_sig_points(out)
}

fn dedup_tempo_points(points: Vec<TempoPoint>) -> Vec<TempoPoint> {
    let mut out: Vec<TempoPoint> = Vec::new();
    for p in points {
        if let Some(last) = out.last_mut() {
            if last.tick == p.tick {
                *last = p;
                continue;
            }
        }
        out.push(p);
    }
    out
}

fn dedup_time_sig_points(points: Vec<TimeSigPoint>) -> Vec<TimeSigPoint> {
    let mut out: Vec<TimeSigPoint> = Vec::new();
    for p in points {
        if let Some(last) = out.last_mut() {
            if last.tick == p.tick {
                *last = p;
                continue;
            }
        }
        out.push(p);
    }
    out
}

fn build_tempo_segments(ppq: u16, points: &[TempoPoint]) -> Vec<TempoSegment> {
    let mut out = Vec::<TempoSegment>::new();
    let mut start_seconds = 0.0_f64;
    for (i, tp) in points.iter().enumerate() {
        if let Some(prev) = out.last() {
            let prev_end_tick = tp.tick;
            start_seconds = prev.start_seconds
                + (prev_end_tick.saturating_sub(prev.start_tick) as f64) * prev.us_per_tick / 1e6;
        }
        let us_per_tick = tp.us_per_quarter as f64 / ppq as f64;
        let end_tick = points.get(i + 1).map(|n| n.tick);
        out.push(TempoSegment {
            start_tick: tp.tick,
            end_tick,
            start_seconds,
            us_per_tick,
        });
    }
    out
}

fn build_meter_segments(ppq: u16, points: &[TimeSigPoint]) -> Vec<MeterSegment> {
    let mut out = Vec::<MeterSegment>::new();
    let mut start_bar = 1u32;
    for (i, ts) in points.iter().enumerate() {
        let ticks_per_beat = ppq as f64 * (4.0 / ts.denominator as f64);
        let ticks_per_bar = ticks_per_beat * ts.numerator as f64;
        out.push(MeterSegment {
            start_tick: ts.tick,
            start_bar,
            ticks_per_beat,
            ticks_per_bar,
        });

        if let Some(next) = points.get(i + 1) {
            let delta_ticks = next.tick.saturating_sub(ts.tick) as f64;
            let bars_f = if ticks_per_bar > 0.0 {
                delta_ticks / ticks_per_bar
            } else {
                0.0
            };
            let nearest = bars_f.round();
            let bars_inc = if (bars_f - nearest).abs() < 1e-6 {
                nearest.max(0.0) as u32
            } else {
                bars_f.floor().max(0.0) as u32
            };
            start_bar = start_bar.saturating_add(bars_inc);
        }
    }
    out
}
