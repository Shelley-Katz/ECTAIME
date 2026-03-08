use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Decision {
    Execute,
    Clarify,
    Suggest,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OperationClass {
    WriteNewCurve,
    SetFlatRange,
    TrimExistingRange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LaneKind {
    Volume,
    EqLowGain,
    EqPresenceGain,
    EqAirGain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeRange {
    pub start_bar: u32,
    pub end_bar: u32,
}

impl TimeRange {
    pub fn is_valid(&self) -> bool {
        self.start_bar > 0 && self.end_bar >= self.start_bar
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Intent {
    pub source_text: String,
    pub decision: Decision,
    pub operation_class: Option<OperationClass>,
    pub targets: Vec<String>,
    pub time_range: Option<TimeRange>,
    pub strength: Option<String>,
    pub confidence: f32,
    pub requires_confirmation: bool,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedTarget {
    pub canonical_name: String,
    pub lane: LaneKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CurvePoint {
    pub offset_ms: u64,
    pub value: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PassPlan {
    pub plan_id: String,
    pub source_text: String,
    pub operation_class: OperationClass,
    pub target_lanes: Vec<ResolvedTarget>,
    pub target_strips: Vec<u32>,
    pub time_range: TimeRange,
    pub control_rate_hz: u32,
    pub curve_shape: String,
    pub curve_points: Vec<CurvePoint>,
    pub pre_roll_bars: u32,
    pub post_roll_beats: u32,
    pub boundary_smoothing_ms: u32,
    pub undo_anchor_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafetyBounds {
    pub volume_clamp_min_db: f32,
    pub volume_clamp_max_db: f32,
    pub volume_slew_db_per_beat: f32,
    pub eq_gain_min_db: f32,
    pub eq_gain_max_db: f32,
}

impl Default for SafetyBounds {
    fn default() -> Self {
        Self {
            volume_clamp_min_db: -6.0,
            volume_clamp_max_db: 6.0,
            volume_slew_db_per_beat: 3.0,
            eq_gain_min_db: -4.0,
            eq_gain_max_db: 4.0,
        }
    }
}
