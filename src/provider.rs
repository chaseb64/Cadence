//! Provider-independent control and playback events for the application layer.
use anyhow::Result;
use async_trait::async_trait;

pub enum PlaybackEvent {
    Track { name: String },
    Playing,
    Paused,
    Stopped,
    Unavailable,
}

#[async_trait]
pub trait AudioProvider: Send + Sync {
    fn shutdown(&self);
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn skip(&self) -> Result<()>;
    async fn queue(&self) -> Result<Vec<String>>;
    async fn enqueue(&self, input: &str) -> Result<()>;
}
