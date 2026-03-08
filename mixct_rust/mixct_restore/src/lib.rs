use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoAnchor {
    pub anchor_id: String,
    pub captured_at: DateTime<Utc>,
    pub command_id: String,
}

pub trait UndoExecutor {
    fn undo_primary(&mut self) -> Result<()>;
    fn undo_fallback(&mut self) -> Result<()>;
    fn verify_post_undo(&mut self) -> Result<()>;
}

pub fn capture_undo_anchor(command_id: &str) -> UndoAnchor {
    UndoAnchor {
        anchor_id: Uuid::new_v4().to_string(),
        captured_at: Utc::now(),
        command_id: command_id.to_string(),
    }
}

pub fn restore_from_anchor<E: UndoExecutor>(executor: &mut E, anchor: &UndoAnchor) -> Result<()> {
    let primary = executor.undo_primary();
    if primary.is_err() {
        executor.undo_fallback().map_err(|e| {
            anyhow!(
                "undo failed: primary_and_fallback_failed for anchor {}: {e}",
                anchor.anchor_id
            )
        })?;
    }
    executor.verify_post_undo().map_err(|e| {
        anyhow!(
            "undo verification failed for anchor {}: {e}",
            anchor.anchor_id
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExec {
        fail_primary: bool,
    }

    impl UndoExecutor for MockExec {
        fn undo_primary(&mut self) -> Result<()> {
            if self.fail_primary {
                return Err(anyhow!("primary_failed"));
            }
            Ok(())
        }
        fn undo_fallback(&mut self) -> Result<()> {
            Ok(())
        }
        fn verify_post_undo(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn uses_fallback_when_primary_fails() {
        let anchor = capture_undo_anchor("c1");
        let mut ex = MockExec { fail_primary: true };
        assert!(restore_from_anchor(&mut ex, &anchor).is_ok());
    }
}
