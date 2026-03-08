use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use mixct_core::{clamp_db, enforce_slew, PassPlan};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEvent {
    pub target: String,
    pub at_ms: u64,
    pub value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub executed_at: DateTime<Utc>,
    pub event_count: usize,
    pub applied_clamps: usize,
    pub success: bool,
}

pub trait ControlBackend {
    fn begin_touch(&mut self, target: &str) -> Result<()>;
    fn write_value(&mut self, target: &str, value: f32, at_ms: u64) -> Result<()>;
    fn end_touch(&mut self, target: &str) -> Result<()>;
    fn trigger_undo_primary(&mut self) -> Result<()>;
    fn trigger_undo_fallback(&mut self) -> Result<()>;
}

#[derive(Default)]
pub struct MockBackend {
    pub log: Vec<String>,
    pub fail_primary_undo: bool,
}

impl ControlBackend for MockBackend {
    fn begin_touch(&mut self, target: &str) -> Result<()> {
        self.log.push(format!("begin_touch:{target}"));
        Ok(())
    }

    fn write_value(&mut self, target: &str, value: f32, at_ms: u64) -> Result<()> {
        self.log.push(format!("write:{target}:{value:.3}:{at_ms}"));
        Ok(())
    }

    fn end_touch(&mut self, target: &str) -> Result<()> {
        self.log.push(format!("end_touch:{target}"));
        Ok(())
    }

    fn trigger_undo_primary(&mut self) -> Result<()> {
        self.log.push("undo_primary".to_string());
        if self.fail_primary_undo {
            return Err(anyhow!("primary_undo_failed"));
        }
        Ok(())
    }

    fn trigger_undo_fallback(&mut self) -> Result<()> {
        self.log.push("undo_fallback".to_string());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PrepassState {
    pub control_path_live: bool,
    pub sync_confidence_valid: bool,
    pub correct_strip_bank_verified: bool,
    pub target_lanes_verified: bool,
    pub dp_transport_state_verified: bool,
    pub undo_anchor_captured: bool,
    pub pass_plan_validated: bool,
    pub undo_path_available: bool,
}

pub fn validate_prepass(state: &PrepassState) -> Result<()> {
    if !state.control_path_live {
        return Err(anyhow!("control_path_live=false"));
    }
    if !state.sync_confidence_valid {
        return Err(anyhow!("sync_confidence_valid=false"));
    }
    if !state.correct_strip_bank_verified {
        return Err(anyhow!("correct_strip_bank_verified=false"));
    }
    if !state.target_lanes_verified {
        return Err(anyhow!("target_lanes_verified=false"));
    }
    if !state.dp_transport_state_verified {
        return Err(anyhow!("dp_transport_state_verified=false"));
    }
    if !state.undo_anchor_captured {
        return Err(anyhow!("undo_anchor_captured=false"));
    }
    if !state.pass_plan_validated {
        return Err(anyhow!("pass_plan_validated=false"));
    }
    if !state.undo_path_available {
        return Err(anyhow!("undo_path_available=false"));
    }
    Ok(())
}

pub fn execute_pass<B: ControlBackend + ?Sized>(
    backend: &mut B,
    plan: &PassPlan,
    min_db: f32,
    max_db: f32,
    max_slew_step: f32,
) -> Result<ExecutionReport> {
    execute_pass_with_scales(backend, plan, min_db, max_db, max_slew_step, None)
}

pub fn execute_pass_with_scales<B: ControlBackend + ?Sized>(
    backend: &mut B,
    plan: &PassPlan,
    min_db: f32,
    max_db: f32,
    max_slew_step: f32,
    target_scales: Option<&HashMap<String, f32>>,
) -> Result<ExecutionReport> {
    let mut applied_clamps = 0usize;
    let mut event_count = 0usize;
    let mut previous_values = Vec::with_capacity(plan.target_lanes.len());

    for lane in &plan.target_lanes {
        let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
        backend.begin_touch(&target_name)?;
        previous_values.push(0.0f32);
    }

    for point in &plan.curve_points {
        for (idx, lane) in plan.target_lanes.iter().enumerate() {
            let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
            let scale = target_scales
                .and_then(|m| m.get(&lane.canonical_name).copied())
                .unwrap_or(1.0);
            let mut v = point.value * scale;
            let clamped = clamp_db(v, min_db, max_db);
            if (clamped - v).abs() > f32::EPSILON {
                applied_clamps += 1;
            }
            v = enforce_slew(previous_values[idx], clamped, max_slew_step);
            backend.write_value(&target_name, v, point.offset_ms)?;
            previous_values[idx] = v;
            event_count += 1;
        }
    }

    for lane in &plan.target_lanes {
        let target_name = format!("{}::{:?}", lane.canonical_name, lane.lane);
        backend.end_touch(&target_name)?;
    }

    Ok(ExecutionReport {
        executed_at: Utc::now(),
        event_count,
        applied_clamps,
        success: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixct_core::{LaneKind, OperationClass, ResolvedTarget, TimeRange};

    #[test]
    fn pass_execution_records_events() {
        let mut backend = MockBackend::default();
        let plan = PassPlan {
            plan_id: "p1".to_string(),
            source_text: "test".to_string(),
            operation_class: OperationClass::WriteNewCurve,
            target_lanes: vec![ResolvedTarget {
                canonical_name: "STR_HI".to_string(),
                lane: LaneKind::Volume,
            }],
            target_strips: vec![1],
            time_range: TimeRange {
                start_bar: 1,
                end_bar: 2,
            },
            control_rate_hz: 50,
            curve_shape: "cubic_ease_in_out".to_string(),
            curve_points: vec![
                mixct_core::CurvePoint {
                    offset_ms: 0,
                    value: 1.0,
                },
                mixct_core::CurvePoint {
                    offset_ms: 20,
                    value: 2.0,
                },
            ],
            pre_roll_bars: 1,
            post_roll_beats: 1,
            boundary_smoothing_ms: 80,
            undo_anchor_ref: Some("u1".to_string()),
            created_at: Utc::now(),
        };
        let report = execute_pass(&mut backend, &plan, -6.0, 6.0, 3.0).unwrap();
        assert!(report.event_count >= 2);
        assert!(backend.log.iter().any(|l| l.starts_with("begin_touch")));
    }
}
