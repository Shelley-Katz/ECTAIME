#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Boot,
    Diagnostics,
    Ready,
    Listening,
    Transcribing,
    Normalizing,
    Parsing,
    Clarifying,
    Suggesting,
    Planning,
    PrepassSnapshot,
    Executing,
    Verifying,
    Summarizing,
    Restoring,
    SafeStop,
    Error,
}

pub fn is_transition_allowed(from: RuntimeState, to: RuntimeState) -> bool {
    use RuntimeState::*;
    if from == Transcribing && to == Executing {
        return false;
    }
    if from == Parsing && to == Executing {
        return false;
    }
    if from == Executing && to == SafeStop {
        return true;
    }
    if from == SafeStop && to == Ready {
        return true;
    }
    from != Error || to == Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_transcribe_to_execute() {
        assert!(!is_transition_allowed(
            RuntimeState::Transcribing,
            RuntimeState::Executing
        ));
    }
}
