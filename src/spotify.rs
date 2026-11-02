//! Experimental provider. Spotify remains the authority for playback and its queue.
use crate::audio::AudioPipe;
use crate::provider::{AudioProvider, PlaybackEvent};
use crate::session::Cancellation;
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use librespot::{
    connect::{ConnectConfig, Spirc},
    core::{authentication::Credentials, cache::Cache, config::SessionConfig, session::Session},
    playback::{
        audio_backend,
        audio_backend::{Sink, SinkError, SinkResult},
        config::{Bitrate, PlayerConfig},
        convert::Converter,
        decoder::AudioPacket,
        mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
        player::{Player, PlayerEvent},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

struct PcmSink(AudioPipe);
// Declared after Player during startup so cancellation closes the pipe and
// invalidates network loaders before librespot joins its synchronous thread.
struct StartupCleanup {
    pipe: AudioPipe,
    session: Session,
    armed: bool,
}
impl Drop for StartupCleanup {
    fn drop(&mut self) {
        if self.armed {
            self.pipe.close();
            self.session.shutdown();
        }
    }
}
impl Sink for PcmSink {
    fn stop(&mut self) -> SinkResult<()> {
        self.0.clear();
        Ok(())
    }
    fn write(&mut self, packet: AudioPacket, _: &mut Converter) -> SinkResult<()> {
        match packet {
            AudioPacket::Samples(samples) => self
                .0
                .write_samples(&samples)
                .map_err(|_| SinkError::OnWrite("PCM bridge closed".into())),
            AudioPacket::Raw(_) => Err(SinkError::OnWrite("Expected decoded PCM samples".into())),
        }
    }
}

// Never derive Debug for credentials or token responses.
#[derive(Serialize, Deserialize)]
pub struct AuthFile {
    pub client_id: String,
    pub refresh_token: String,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}
struct Token {
    value: String,
    until: Instant,
}
pub struct Spotify {
    spirc: Spirc,
    session: Session,
    pub player: Arc<Player>,
    http: reqwest::Client,
    auth_path: PathBuf,
    token: Mutex<Option<Token>>,
    device_id: String,
    pipe: AudioPipe,
    cancellation: Cancellation,
}

fn protect(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}
fn save_auth(path: &Path, auth: &AuthFile) -> Result<()> {
    let tmp = path.with_extension("tmp");
    // Data directory is private (0700) and only one process owns this file.
    std::fs::write(&tmp, serde_json::to_vec(auth)?)?;
    protect(&tmp)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

impl Spotify {
    pub async fn start(
        data_dir: &Path,
        auth_seed: &Path,
        name: String,
        pipe: AudioPipe,
    ) -> Result<(
        Arc<Self>,
        tokio::sync::mpsc::Receiver<PlaybackEvent>,
        tokio::task::JoinHandle<()>,
    )> {
        std::fs::create_dir_all(data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let auth_path = data_dir.join("oauth.json");
        if !auth_path.exists() {
            let seed: AuthFile = serde_json::from_slice(
                &std::fs::read(auth_seed)
                    .context("Read Spotify OAuth secret; run scripts/spotify_login.py first")?,
            )?;
            ensure!(
                !seed.client_id.is_empty() && !seed.refresh_token.is_empty(),
                "OAuth secret is incomplete"
            );
            save_auth(&auth_path, &seed)?;
        }
        let auth: AuthFile = serde_json::from_slice(&std::fs::read(&auth_path)?)?;
        let device_path = data_dir.join("device-id");
        let mut config = SessionConfig {
            client_id: auth.client_id,
            ..Default::default()
        };
        if device_path.exists() {
            config.device_id = std::fs::read_to_string(&device_path)?.trim().to_owned();
        } else {
            std::fs::write(&device_path, &config.device_id)?;
        }
        let device_id = config.device_id.clone();
        let cache = Cache::new(Some(data_dir), Some(data_dir), None, None)?;
        let cached = cache.credentials();
        let session = Session::new(config, Some(cache));
        let mixer = Arc::new(SoftMixer::open(MixerConfig::default())?);
        let sink_pipe = pipe.clone();
        let player = Player::new(
            PlayerConfig {
                bitrate: Bitrate::Bitrate320,
                ..Default::default()
            },
            session.clone(),
            mixer.get_soft_volume(),
            move || Box::new(PcmSink(sink_pipe)) as Box<dyn audio_backend::Sink>,
        );
        let mut cleanup = StartupCleanup {
            pipe: pipe.clone(),
            session: session.clone(),
            armed: true,
        };
        let mut events = player.get_player_event_channel();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let token = Self::refresh(&http, &auth_path).await?;
        let credentials = cached.unwrap_or_else(|| Credentials::with_access_token(&token.value));
        let (spirc, task) = tokio::time::timeout(
            Duration::from_secs(60),
            Spirc::new(
                ConnectConfig {
                    name,
                    ..Default::default()
                },
                session.clone(),
                credentials,
                player.clone(),
                mixer,
            ),
        )
        .await
        .context("Spotify connection timed out")?
        .context("Spotify session failed; authentication does not prove playback support")?;
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(32);
        let task = tokio::spawn(async move {
            let forward = async move {
                while let Some(event) = events.recv().await {
                    let event = match event {
                        PlayerEvent::TrackChanged { audio_item } => PlaybackEvent::Track {
                            name: audio_item.name,
                        },
                        PlayerEvent::Playing { .. } => PlaybackEvent::Playing,
                        PlayerEvent::Paused { .. } => PlaybackEvent::Paused,
                        PlayerEvent::Stopped { .. } => PlaybackEvent::Stopped,
                        PlayerEvent::Unavailable { .. } => PlaybackEvent::Unavailable,
                        _ => continue,
                    };
                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
            };
            tokio::select! { _ = task => {}, _ = forward => {} }
        });
        cleanup.armed = false;
        Ok((
            Arc::new(Self {
                spirc,
                session,
                player,
                http,
                auth_path,
                token: Mutex::new(Some(token)),
                device_id,
                pipe,
                cancellation: Cancellation::default(),
            }),
            event_rx,
            task,
        ))
    }
    async fn refresh(http: &reqwest::Client, path: &Path) -> Result<Token> {
        let mut auth: AuthFile = serde_json::from_slice(&std::fs::read(path)?)?;
        let response = http
            .post("https://accounts.spotify.com/api/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", auth.refresh_token.as_str()),
                ("client_id", auth.client_id.as_str()),
            ])
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "Spotify token refresh failed (HTTP {}); repeat browser login if revoked",
            response.status()
        );
        let token: TokenResponse = response.json().await?;
        if let Some(refresh) = token.refresh_token {
            auth.refresh_token = refresh;
            save_auth(path, &auth)?;
        }
        Ok(Token {
            value: token.access_token,
            until: Instant::now() + Duration::from_secs(token.expires_in.saturating_sub(60)),
        })
    }
    async fn access_token(&self) -> Result<String> {
        let mut guard = self.token.lock().await;
        if guard.as_ref().is_none_or(|t| Instant::now() >= t.until) {
            *guard = Some(Self::refresh(&self.http, &self.auth_path).await?);
        }
        Ok(guard.as_ref().unwrap().value.clone())
    }
    async fn active_token(&self) -> Result<String> {
        self.ensure_running()?;
        let token = self.access_token().await?;
        let r = self
            .http
            .get("https://api.spotify.com/v1/me/player")
            .bearer_auth(&token)
            .send()
            .await?;
        ensure!(
            r.status() == reqwest::StatusCode::OK,
            "Select Cadence in Spotify first (playback status HTTP {})",
            r.status()
        );
        let v: serde_json::Value = r.json().await?;
        ensure!(
            v["device"]["id"].as_str() == Some(&self.device_id),
            "Cadence is not the active Spotify device; select it in Spotify first"
        );
        Ok(token)
    }
    pub fn shutdown(&self) {
        self.cancellation.cancel();
        self.pipe.close();
        let _ = self.spirc.shutdown();
        self.player.stop();
        self.session.shutdown();
    }
    fn ensure_running(&self) -> Result<()> {
        ensure!(
            !self.cancellation.is_cancelled(),
            "This session has ended; wait for Cadence to reconnect"
        );
        Ok(())
    }
}
#[async_trait]
impl AudioProvider for Spotify {
    async fn pause(&self) -> Result<()> {
        self.ensure_running()?;
        self.spirc.pause()?;
        Ok(())
    }
    async fn resume(&self) -> Result<()> {
        self.ensure_running()?;
        self.spirc.play()?;
        Ok(())
    }
    async fn skip(&self) -> Result<()> {
        self.ensure_running()?;
        self.spirc.next()?;
        Ok(())
    }
    async fn queue(&self) -> Result<Vec<String>> {
        self.cancellation
            .run(async {
                let token = self.active_token().await?;
                let r = self
                    .http
                    .get("https://api.spotify.com/v1/me/player/queue")
                    .bearer_auth(token)
                    .send()
                    .await?;
                ensure!(
                    r.status().is_success(),
                    "Spotify queue unavailable (HTTP {})",
                    r.status()
                );
                let v: serde_json::Value = r.json().await?;
                let queue = v["queue"].as_array().context("Spotify returned no queue")?;
                Ok(queue
                    .iter()
                    .take(10)
                    .map(|t| t["name"].as_str().unwrap_or("Unknown track").to_owned())
                    .collect())
            })
            .await
    }
    async fn enqueue(&self, input: &str) -> Result<()> {
        self.cancellation
            .run(async {
                let uri = track_uri(input)?;
                let token = self.active_token().await?;
                // Never automatically retry a mutation: a timeout may have succeeded remotely.
                let r = self
                    .http
                    .post("https://api.spotify.com/v1/me/player/queue")
                    .query(&[
                        ("uri", uri.as_str()),
                        ("device_id", self.device_id.as_str()),
                    ])
                    .bearer_auth(token)
                    .send()
                    .await?;
                ensure!(
                    r.status().is_success(),
                    "Spotify could not add the track (HTTP {})",
                    r.status()
                );
                Ok(())
            })
            .await
    }
}
pub fn track_uri(input: &str) -> Result<String> {
    let input = input.trim();
    let id = if let Some(id) = input.strip_prefix("spotify:track:") {
        id
    } else if let Some(id) = input.strip_prefix("https://open.spotify.com/track/") {
        id.split('?').next().unwrap_or("")
    } else {
        bail!("Use a Spotify track link or spotify:track: URI");
    };
    ensure!(
        id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid Spotify track ID"
    );
    Ok(format!("spotify:track:{id}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_are_normalized_without_fetching_user_urls() {
        let id = "1234567890123456789012";
        assert_eq!(
            track_uri(&format!("https://open.spotify.com/track/{id}?si=test")).unwrap(),
            format!("spotify:track:{id}")
        );
        for bad in [
            "http://localhost/track/foo",
            "https://open.spotify.com.evil/track/1234567890123456789012",
            "spotify:album:1234567890123456789012",
            "spotify:track:short",
        ] {
            assert!(track_uri(bad).is_err());
        }
    }
}
