use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    pub enabled: bool,
    pub feature_flag: String,
    pub engine: String,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackTranscript {
    pub text: String,
    pub confidence: f32,
    pub backend: String,
}

pub fn transcribe_with_local_fallback(
    audio_hint: Option<&str>,
    config: &FallbackConfig,
) -> Result<FallbackTranscript> {
    if !config.enabled {
        return Err(anyhow!("fallback_disabled"));
    }
    if config.engine != "mlx_whisper_local_only" {
        return Err(anyhow!("unsupported_fallback_engine"));
    }

    let command = config
        .command
        .as_ref()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .or_else(|| std::env::var("MIXCT_LOCAL_STT_CMD").ok())
        .ok_or_else(|| anyhow!("local_fallback_command_not_configured"))?;

    let mut cmd = Command::new("sh");
    cmd.arg("-lc").arg(&command);
    if let Some(hint) = audio_hint {
        cmd.env("MIXCT_AUDIO_HINT", hint);
    }
    let out = cmd
        .output()
        .with_context(|| "failed to spawn MIXCT_LOCAL_STT_CMD")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "MIXCT_LOCAL_STT_CMD_failed status={} stderr={}",
            out.status,
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(anyhow!("MIXCT_LOCAL_STT_CMD_empty_output"));
    }
    parse_fallback_output(&stdout)
}

fn parse_fallback_output(raw: &str) -> Result<FallbackTranscript> {
    #[derive(Debug, Deserialize)]
    struct JsonOut {
        text: String,
        confidence: Option<f32>,
        backend: Option<String>,
    }

    if raw.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<JsonOut>(raw) {
            let text = parsed.text.trim().to_string();
            if text.is_empty() {
                return Err(anyhow!("fallback_output_text_empty"));
            }
            return Ok(FallbackTranscript {
                text,
                confidence: parsed.confidence.unwrap_or(0.75).clamp(0.0, 1.0),
                backend: parsed
                    .backend
                    .unwrap_or_else(|| "mlx_whisper_local_only".to_string()),
            });
        }
    }

    let text = raw
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(anyhow!("fallback_plaintext_output_empty"));
    }
    Ok(FallbackTranscript {
        text,
        confidence: 0.72,
        backend: "mlx_whisper_local_only".to_string(),
    })
}
