use anyhow::Result;
use std::process::Command;

pub trait TtsSpeaker {
    fn speak(&self, text: &str) -> Result<()>;
}

pub struct ConsoleTts;

impl TtsSpeaker for ConsoleTts {
    fn speak(&self, text: &str) -> Result<()> {
        if std::env::var("MIXCT_TTS_CONSOLE_ONLY").ok().as_deref() != Some("1") {
            if let Ok(status) = Command::new("say").arg(text).status() {
                if status.success() {
                    return Ok(());
                }
            }
        }
        println!("[TTS] {text}");
        Ok(())
    }
}
