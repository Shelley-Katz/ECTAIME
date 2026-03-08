use anyhow::Result;

pub trait TtsSpeaker {
    fn speak(&self, text: &str) -> Result<()>;
}

pub struct ConsoleTts;

impl TtsSpeaker for ConsoleTts {
    fn speak(&self, text: &str) -> Result<()> {
        println!("[TTS] {text}");
        Ok(())
    }
}
