use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    pub enabled: bool,
    pub feature_flag: String,
    pub engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackTranscript {
    pub text: String,
    pub confidence: f32,
    pub backend: String,
}

pub fn transcribe_with_local_fallback(
    _audio_hint: Option<&str>,
    config: &FallbackConfig,
) -> Result<FallbackTranscript> {
    if !config.enabled {
        return Err(anyhow!("fallback_disabled"));
    }
    if config.engine != "mlx_whisper_local_only" {
        return Err(anyhow!("unsupported_fallback_engine"));
    }

    Ok(FallbackTranscript {
        text: "fallback transcript".to_string(),
        confidence: 0.82,
        backend: "mlx_whisper_local_only".to_string(),
    })
}
