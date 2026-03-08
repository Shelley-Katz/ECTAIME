use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncInputs {
    pub session_hash_matches: bool,
    pub timeline_hash_matches: bool,
    pub avb_clock_lock_valid: bool,
    pub transport_state_matches_plan: bool,
    pub drift_ms: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub confidence: f32,
    pub can_execute: bool,
    pub reasons: Vec<String>,
}

pub fn evaluate_sync(inputs: &SyncInputs, min_confidence: f32) -> SyncResult {
    let mut reasons = Vec::new();
    let mut confidence: f32 = 1.0;

    if !inputs.session_hash_matches {
        confidence -= 0.35;
        reasons.push("session_hash_mismatch".to_string());
    }
    if !inputs.timeline_hash_matches {
        confidence -= 0.3;
        reasons.push("timeline_hash_mismatch".to_string());
    }
    if !inputs.avb_clock_lock_valid {
        confidence -= 0.25;
        reasons.push("avb_clock_unlock".to_string());
    }
    if !inputs.transport_state_matches_plan {
        confidence -= 0.2;
        reasons.push("transport_state_mismatch".to_string());
    }

    if inputs.drift_ms > 12.0 {
        confidence -= 0.2;
        reasons.push("drift_exceeds_threshold".to_string());
    }

    confidence = confidence.clamp(0.0, 1.0);
    let can_execute = confidence >= min_confidence && reasons.is_empty();

    SyncResult {
        confidence,
        can_execute,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_on_clock_unlock() {
        let r = evaluate_sync(
            &SyncInputs {
                session_hash_matches: true,
                timeline_hash_matches: true,
                avb_clock_lock_valid: false,
                transport_state_matches_plan: true,
                drift_ms: 2.0,
            },
            0.85,
        );
        assert!(!r.can_execute);
        assert!(r.reasons.contains(&"avb_clock_unlock".to_string()));
    }
}
