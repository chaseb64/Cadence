//! Local Soloist control plus a bounded PulseAudio monitor bridge.
//! The unauthenticated WebSocket must remain on loopback.
use crate::{
    audio::AudioPipe,
    provider::{AudioProvider, PlaybackEvent},
    spotify::track_uri,
};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Request = (Value, oneshot::Sender<Result<Value>>);
pub struct Soloist {
    requests: mpsc::Sender<Request>,
    stop: watch::Sender<bool>,
    pipe: AudioPipe,
}

fn title(item: &Value) -> String {
    item.pointer("/decorations/identity/name")
        .and_then(Value::as_str)
        .or_else(|| item.get("uri").and_then(Value::as_str))
        .unwrap_or("Unknown track")
        .to_owned()
}

// Incremental updates must retain active-device ownership: a remote device playing
// must never open this device's Discord audio gate.
#[derive(Default)]
struct Playback {
    active: bool,
    playing: bool,
}
impl Playback {
    fn activity_change(&mut self, value: &Value) -> (bool, Option<bool>) {
        let before = self.active;
        let playing = self.update(value);
        (playing, (before != self.active).then_some(self.active))
    }
    fn update(&mut self, value: &Value) -> bool {
        if let Some(active) = value.get("is_active").and_then(Value::as_bool) {
            self.active = active;
        }
        if value.get("logged_in") == Some(&Value::Bool(false)) {
            self.active = false;
            self.playing = false;
        }
        if let Some(status) = value.get("status").and_then(Value::as_str) {
            self.playing = status == "playing";
        }
        self.active && self.playing
    }
}

impl Soloist {
    pub async fn start(
        pipe: AudioPipe,
    ) -> Result<(Arc<Self>, mpsc::Receiver<PlaybackEvent>, JoinHandle<()>)> {
        let endpoint = std::env::var("SOLOIST_WS").unwrap_or_else(|_| "ws://127.0.0.1:9090".into());
        ensure!(
            endpoint.starts_with("ws://127.0.0.1:"),
            "SOLOIST_WS must use IPv4 loopback"
        );
        let (mut ws, _) = timeout(Duration::from_secs(10), connect_async(endpoint))
            .await
            .context("Soloist connection timed out")??;
        let mut child = Command::new("parec")
            .args([
                "--raw",
                "--format=float32le",
                "--rate=48000",
                "--channels=2",
                "--device=cadence_test.monitor",
                "--latency-msec=40",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("Start PulseAudio monitor capture")?;
        let mut stdout = child.stdout.take().context("Capture stdout missing")?;
        let (requests, mut incoming) = mpsc::channel::<Request>(8);
        let (events, receiver) = mpsc::channel(32);
        let (stop, mut stopping) = watch::channel(false);
        let enabled = Arc::new(AtomicBool::new(false));
        let gate = enabled.clone();
        let capture_pipe = pipe.clone();
        let provider = Arc::new(Self {
            requests,
            stop,
            pipe: pipe.clone(),
        });
        let task = tokio::spawn(async move {
            let capture = async move {
                // Always drain the live monitor, even while paused. Never replay old audio.
                let mut bytes = [0u8; 8192];
                let mut tail = Vec::new();
                loop {
                    let n = stdout.read(&mut bytes).await?;
                    if n == 0 {
                        bail!("PulseAudio monitor ended");
                    }
                    tail.extend_from_slice(&bytes[..n]);
                    let length = tail.len() / 4 * 4;
                    let samples: Vec<f64> = tail[..length]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|s| f32::from_le_bytes(*s) as f64)
                        .collect();
                    tail.drain(..length);
                    if gate.load(Ordering::SeqCst) {
                        let output = capture_pipe.clone();
                        tokio::task::spawn_blocking(move || output.write_samples(&samples))
                            .await??;
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), anyhow::Error>(())
            };
            tokio::pin!(capture);
            let mut state = Playback::default();
            let mut pending: Option<(String, oneshot::Sender<Result<Value>>)> = None;
            let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(86400)));
            loop {
                tokio::select! {
                    _ = stopping.changed() => break,
                    result = &mut capture => { let _ = result; tracing::error!("Soloist audio capture ended"); break; },
                    _ = &mut deadline, if pending.is_some() => {
                        if let Some((_, reply)) = pending.take() { let _ = reply.send(Err(anyhow::anyhow!("Soloist command timed out; check Spotify before retrying"))); }
                        // No request IDs: discard this socket after timeout, never misattribute a late reply.
                        break;
                    },
                    request = incoming.recv(), if pending.is_none() => {
                        let Some((value, reply)) = request else { break; };
                        let command = value["command"].as_str().unwrap_or_default().to_owned();
                        if ws.send(Message::Text(value.to_string().into())).await.is_err() { break; }
                        pending = Some((command, reply));
                        deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(8));
                    },
                    message = ws.next() => {
                        let value = match message {
                            Some(Ok(Message::Text(text))) => match serde_json::from_str::<Value>(&text) { Ok(v) => v, Err(_) => break },
                            Some(Ok(Message::Ping(data))) => { if ws.send(Message::Pong(data)).await.is_err() { break; } continue; },
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                            _ => continue,
                        };
                        let kind = value["type"].as_str().unwrap_or_default();
                        if matches!(kind, "playback_state" | "playback_changed" | "device_changed" | "auth_state") {
                            let (playing, activity) = state.activity_change(&value);
                            if let Some(active) = activity && events.send(if active { PlaybackEvent::DeviceActive } else { PlaybackEvent::DeviceInactive }).await.is_err() { break; }
                            enabled.store(playing, Ordering::SeqCst);
                            if !playing { pipe.clear(); }
                            if events.send(if playing { PlaybackEvent::Playing } else { PlaybackEvent::Paused }).await.is_err() { break; }
                        }
                        if matches!(kind, "track_changed" | "playback_state") && value.get("item").is_some_and(Value::is_object)
                            && events.send(PlaybackEvent::Track { name: title(&value["item"]) }).await.is_err() { break; }
                        if let Some((command, _)) = &pending {
                            let matched = kind == "error" || (kind == "command_result" && value["command"] == *command) || (command == "get_queue" && kind == "queue_changed");
                            if matched {
                                let (_, reply) = pending.take().unwrap();
                                let result = if kind == "error" { Err(anyhow::anyhow!("Soloist rejected the command")) } else { Ok(value) };
                                let _ = reply.send(result);
                            }
                        }
                    }
                }
            }
            enabled.store(false, Ordering::SeqCst);
            pipe.close(); // Wake a blocked bounded-buffer writer before cancelling capture.
            let _ = child.kill().await;
            let _ = child.wait().await;
        });
        Ok((provider, receiver, task))
    }
    async fn command(&self, value: Value) -> Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.requests
            .try_send((value, reply))
            .context("Soloist is busy or disconnected")?;
        timeout(Duration::from_secs(10), receiver)
            .await
            .context("Soloist request timed out")?
            .context("Soloist disconnected")?
    }
}
#[async_trait]
impl AudioProvider for Soloist {
    fn shutdown(&self) {
        let _ = self.stop.send(true);
        self.pipe.close();
    }
    async fn pause(&self) -> Result<()> {
        self.command(json!({"type":"command","command":"pause"}))
            .await?;
        Ok(())
    }
    async fn resume(&self) -> Result<()> {
        self.command(json!({"type":"command","command":"play"}))
            .await?;
        Ok(())
    }
    async fn skip(&self) -> Result<()> {
        self.command(json!({"type":"command","command":"skip_next"}))
            .await?;
        Ok(())
    }
    async fn queue(&self) -> Result<Vec<String>> {
        let value = self
            .command(json!({"type":"command","command":"get_queue","limit":10}))
            .await?;
        Ok(value["upcoming"]
            .as_array()
            .context("Invalid Soloist queue")?
            .iter()
            .map(|entry| title(&entry["item"]))
            .collect())
    }
    async fn enqueue(&self, input: &str) -> Result<()> {
        self.command(json!({"type":"command","command":"add_to_queue","uri":track_uri(input)?}))
            .await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_transfer_and_buffering_close_audio_gate() {
        let mut state = Playback::default();
        assert!(state.update(&json!({"is_active":true,"status":"playing"})));
        assert!(!state.update(&json!({"is_active":false})));
        assert!(!state.update(&json!({"status":"playing"})));
        assert!(state.update(&json!({"is_active":true})));
        assert!(!state.update(&json!({"status":"buffering"})));
        assert!(state.update(&json!({"status":"playing"})));
        assert!(!state.update(&json!({"logged_in":false})));
    }
    #[test]
    fn pause_does_not_transfer_device_but_deactivation_and_logout_do() {
        let mut state = Playback::default();
        assert_eq!(
            state.activity_change(&json!({"is_active":false,"status":"paused"})),
            (false, None)
        );
        assert_eq!(
            state.activity_change(&json!({"is_active":true,"status":"playing"})),
            (true, Some(true))
        );
        assert_eq!(
            state.activity_change(&json!({"status":"paused"})),
            (false, None)
        );
        assert_eq!(
            state.activity_change(&json!({"status":"buffering"})),
            (false, None)
        );
        assert_eq!(
            state.activity_change(&json!({"is_active":false})),
            (false, Some(false))
        );
        assert_eq!(
            state.activity_change(&json!({"is_active":false})),
            (false, None)
        );
        assert_eq!(
            state.activity_change(&json!({"is_active":true,"status":"playing"})),
            (true, Some(true))
        );
        assert_eq!(
            state.activity_change(&json!({"logged_in":false})),
            (false, Some(false))
        );
    }
    #[test]
    fn queue_titles_use_documented_entity_shape() {
        assert_eq!(
            title(&json!({"decorations":{"identity":{"name":"Test song"}},"uri":"fallback"})),
            "Test song"
        );
        assert_eq!(
            title(&json!({"uri":"spotify:track:test"})),
            "spotify:track:test"
        );
    }
}
