use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub confidence: f32,
    pub backend: String,
}

pub trait SpeechBackend {
    fn transcribe_push_to_talk(&self, _audio_hint: Option<&str>) -> Result<Transcript>;
}

pub struct MockAppleSpeech;

impl SpeechBackend for MockAppleSpeech {
    fn transcribe_push_to_talk(&self, _audio_hint: Option<&str>) -> Result<Transcript> {
        Ok(Transcript {
            text: "mock transcript".to_string(),
            confidence: 0.9,
            backend: "apple_speech_mock".to_string(),
        })
    }
}

pub struct AppleSpeechUnavailable;

impl SpeechBackend for AppleSpeechUnavailable {
    fn transcribe_push_to_talk(&self, _audio_hint: Option<&str>) -> Result<Transcript> {
        Err(anyhow!(
            "speech sidecar is not attached; use mock or integrate SpeechDetector/SpeechAnalyzer/SpeechTranscriber"
        ))
    }
}

#[derive(Debug, Clone)]
pub struct CommandAppleSpeech {
    command: String,
}

impl CommandAppleSpeech {
    pub fn from_env() -> Result<Self> {
        let command = std::env::var("MIXCT_APPLE_STT_CMD")
            .map_err(|_| anyhow!("MIXCT_APPLE_STT_CMD_not_set"))?;
        let command = command.trim().to_string();
        if command.is_empty() {
            return Err(anyhow!("MIXCT_APPLE_STT_CMD_empty"));
        }
        Ok(Self { command })
    }
}

impl SpeechBackend for CommandAppleSpeech {
    fn transcribe_push_to_talk(&self, audio_hint: Option<&str>) -> Result<Transcript> {
        let mut cmd = Command::new("sh");
        cmd.arg("-lc").arg(&self.command);
        if let Some(hint) = audio_hint {
            cmd.env("MIXCT_AUDIO_HINT", hint);
        }
        let out = cmd
            .output()
            .with_context(|| "failed to spawn MIXCT_APPLE_STT_CMD")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!(
                "MIXCT_APPLE_STT_CMD_failed status={} stderr={}",
                out.status,
                stderr.trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if stdout.is_empty() {
            return Err(anyhow!("MIXCT_APPLE_STT_CMD_empty_output"));
        }
        parse_transcript_output(&stdout, "apple_speech_cmd")
    }
}

fn parse_transcript_output(raw: &str, default_backend: &str) -> Result<Transcript> {
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
                return Err(anyhow!("speech_output_text_empty"));
            }
            return Ok(Transcript {
                text,
                confidence: parsed.confidence.unwrap_or(0.80).clamp(0.0, 1.0),
                backend: parsed
                    .backend
                    .unwrap_or_else(|| default_backend.to_string()),
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
        return Err(anyhow!("speech_plaintext_output_empty"));
    }
    Ok(Transcript {
        text,
        confidence: 0.78,
        backend: default_backend.to_string(),
    })
}
