#[derive(Debug, Clone, Copy)]
pub struct RiskInputs {
    pub target_count: usize,
    pub bar_span: u32,
    pub strength: f32,
    pub confidence: f32,
    pub sync_margin: f32,
}

pub fn compute_risk_score(input: RiskInputs) -> f32 {
    let target_factor = (input.target_count as f32 / 4.0).min(1.0);
    let span_factor = (input.bar_span as f32 / 16.0).min(1.0);
    let strength_factor = input.strength.clamp(0.0, 1.0);
    let confidence_penalty = (1.0 - input.confidence.clamp(0.0, 1.0)).max(0.0);
    let sync_penalty = (1.0 - input.sync_margin.clamp(0.0, 1.0)).max(0.0);

    let score = 0.25 * target_factor
        + 0.2 * span_factor
        + 0.2 * strength_factor
        + 0.2 * confidence_penalty
        + 0.15 * sync_penalty;

    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_score_increases_with_size() {
        let low = compute_risk_score(RiskInputs {
            target_count: 1,
            bar_span: 2,
            strength: 0.2,
            confidence: 0.95,
            sync_margin: 0.95,
        });
        let high = compute_risk_score(RiskInputs {
            target_count: 5,
            bar_span: 24,
            strength: 0.9,
            confidence: 0.4,
            sync_margin: 0.3,
        });
        assert!(high > low);
    }
}
