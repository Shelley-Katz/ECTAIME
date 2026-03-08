use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

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
