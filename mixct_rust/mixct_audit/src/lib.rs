use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub run_id: String,
    pub command_id: String,
    pub event_type: String,
    pub created_at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

pub struct AuditLogger {
    path: PathBuf,
}

impl AuditLogger {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        Ok(Self { path })
    }

    pub fn append(&self, record: &AuditRecord) -> Result<()> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(record)?;
        writeln!(f, "{line}")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_jsonl() {
        let path = std::env::temp_dir().join("mixct_audit_test.jsonl");
        let logger = AuditLogger::new(&path).unwrap();
        let rec = AuditRecord {
            run_id: "r1".into(),
            command_id: "c1".into(),
            event_type: "test".into(),
            created_at: Utc::now(),
            payload: serde_json::json!({"ok":true}),
        };
        logger.append(&rec).unwrap();
    }
}
