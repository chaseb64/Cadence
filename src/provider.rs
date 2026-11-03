//! Provider-independent control and playback events for the application layer.
use anyhow::Result;
use async_trait::async_trait;

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub name: String,
    pub artist: Option<String>,
    pub duration_ms: Option<u64>,
    pub artwork_url: Option<String>,
    pub spotify_url: Option<String>,
}

pub enum PlaybackEvent {
    Track(TrackInfo),
    Position {
        position_ms: u64,
        timestamp_ms: u64,
        speed: f64,
    },
    Playing,
    Paused,
    Stopped,
    DeviceActive,
    DeviceInactive,
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
