use mixct_core::{Decision, Intent, OperationClass, TimeRange};
use regex::Regex;
use std::collections::HashMap;

pub fn parse_utterance(text: &str, aliases: &HashMap<String, String>) -> Intent {
    let lower = text.to_lowercase();
    let mut reason_codes = Vec::new();

    let time_range = parse_time_range(&lower);
    if time_range.is_none() {
        reason_codes.push("missing_time_window".to_string());
    }

    let mut targets = resolve_targets(&lower, aliases);
    if targets.is_empty() {
        if lower.contains("violin") {
            targets.push("STR_HI".to_string());
        }
    }
    if targets.is_empty() {
        reason_codes.push("missing_target".to_string());
    }

    let suggest_mode = lower.contains("what do you suggest") || lower.contains("suggest");
    let restore_mode = lower.contains("restore") && lower.contains("previous");

    let operation_class = if restore_mode {
        Some(OperationClass::TrimExistingRange)
    } else if lower.contains("flat") || lower.contains("hold") {
        Some(OperationClass::SetFlatRange)
    } else if lower.contains("trim") {
        Some(OperationClass::TrimExistingRange)
    } else if lower.contains("primary")
        || lower.contains("secondary")
        || lower.contains("counterpoint")
        || lower.contains("accompaniment")
        || lower.contains("rest of orchestra")
        || lower.contains("rest of the orchestra")
    {
        Some(OperationClass::WriteNewCurve)
    } else if lower.contains("soft")
        || lower.contains("loud")
        || lower.contains("presence")
        || lower.contains("air")
        || lower.contains("covered")
    {
        Some(OperationClass::WriteNewCurve)
    } else {
        None
    };

    if operation_class.is_none() {
        reason_codes.push("missing_operation".to_string());
    }

    let ambiguous_words = ["there", "them", "something", "fix it", "do something"];
    if ambiguous_words.iter().any(|w| lower.contains(w)) && targets.is_empty() {
        reason_codes.push("ambiguous_target".to_string());
    }

    let decision = if suggest_mode {
        Decision::Suggest
    } else if restore_mode {
        if targets.is_empty() {
            Decision::Clarify
        } else {
            Decision::Execute
        }
    } else if !reason_codes.is_empty() {
        Decision::Clarify
    } else {
        Decision::Execute
    };

    let confidence = match decision {
        Decision::Execute => 0.92,
        Decision::Suggest => 0.9,
        Decision::Clarify => 0.55,
        Decision::Reject => 0.2,
    };

    let requires_confirmation = matches!(decision, Decision::Suggest) || targets.len() > 2;

    Intent {
        source_text: text.to_string(),
        decision,
        operation_class,
        targets,
        time_range,
        strength: parse_strength(&lower),
        confidence,
        requires_confirmation,
        reason_codes,
    }
}

fn parse_time_range(lower: &str) -> Option<TimeRange> {
    let patterns = [
        r"bars?\s*(\d+)\s*(?:-|to|through)\s*(\d+)",
        r"measures?\s*(\d+)\s*(?:-|to|through)\s*(\d+)",
    ];
    for p in patterns {
        let re = Regex::new(p).ok()?;
        if let Some(c) = re.captures(lower) {
            let start = c.get(1)?.as_str().parse::<u32>().ok()?;
            let end = c.get(2)?.as_str().parse::<u32>().ok()?;
            return Some(TimeRange {
                start_bar: start,
                end_bar: end,
            });
        }
    }
    None
}

fn resolve_targets(lower: &str, aliases: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in aliases {
        if lower.contains(k) && !out.contains(v) {
            out.push(v.clone());
        }
    }
    out
}

fn parse_strength(lower: &str) -> Option<String> {
    if lower.contains("slight") || lower.contains("a bit") {
        Some("slight".to_string())
    } else if lower.contains("much") || lower.contains("strong") {
        Some("strong".to_string())
    } else if lower.contains("too soft") || lower.contains("not loud enough") {
        Some("medium".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actionable_command_executes() {
        let mut aliases = HashMap::new();
        aliases.insert("violins".to_string(), "STR_HI".to_string());
        let i = parse_utterance("Violins are too soft in bars 26-29", &aliases);
        assert_eq!(i.decision, Decision::Execute);
        assert_eq!(i.time_range.unwrap().start_bar, 26);
    }

    #[test]
    fn suggest_command_routes_to_suggest() {
        let aliases = HashMap::new();
        let i = parse_utterance("What do you suggest for bars 26-29?", &aliases);
        assert_eq!(i.decision, Decision::Suggest);
    }

    #[test]
    fn vague_command_clarifies() {
        let aliases = HashMap::new();
        let i = parse_utterance("Bring them up a bit there", &aliases);
        assert_eq!(i.decision, Decision::Clarify);
    }
}
