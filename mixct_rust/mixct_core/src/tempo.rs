use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoEvent {
    pub bar: u32,
    pub beat: u32,
    pub bpm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeterEvent {
    pub bar: u32,
    pub numerator: u32,
    pub denominator: u32,
}

pub fn bar_beat_to_seconds(
    bar: u32,
    beat: u32,
    tempo_events: &[TempoEvent],
    meter_events: &[MeterEvent],
) -> Option<f64> {
    if bar == 0 {
        return None;
    }
    let bpm = tempo_events
        .iter()
        .filter(|e| e.bar <= bar)
        .max_by_key(|e| (e.bar, e.beat))
        .map(|e| e.bpm)
        .unwrap_or(120.0);

    let beats_per_bar = meter_events
        .iter()
        .filter(|m| m.bar <= bar)
        .max_by_key(|m| m.bar)
        .map(|m| m.numerator)
        .unwrap_or(4);

    let total_beats = (bar.saturating_sub(1) as f64) * beats_per_bar as f64 + beat as f64;
    Some(total_beats * 60.0 / bpm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bar_beat() {
        let t = bar_beat_to_seconds(
            5,
            0,
            &[TempoEvent {
                bar: 1,
                beat: 0,
                bpm: 120.0,
            }],
            &[MeterEvent {
                bar: 1,
                numerator: 4,
                denominator: 4,
            }],
        )
        .unwrap();
        assert!((t - 8.0).abs() < 1e-6);
    }
}
