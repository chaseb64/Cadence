//! Pure session rules; no Discord or Spotify network operations.
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

#[derive(Clone)]
pub struct Cancellation(tokio::sync::watch::Sender<bool>);
impl Default for Cancellation {
    fn default() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    pub async fn run<T>(&self, work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        let mut stop = self.0.subscribe();
        if *stop.borrow() {
            bail!("This session has ended; wait for Cadence to reconnect");
        }
        tokio::select! {
            biased;
            _ = stop.changed() => bail!("This session has ended; wait for Cadence to reconnect"),
            result = work => result,
        }
    }
}

pub fn claim(data_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("session.lock"))?;
    lock.try_lock()
        .context("Another Cadence process owns this data volume")?;
    Ok(lock)
}
#[derive(Default)]
pub struct Session {
    generation: u64,
    connected: bool,
    attempts: u8,
}

impl Session {
    pub fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.connected = true;
        self.generation
    }
    pub fn end(&mut self) {
        self.connected = false;
        self.generation += 1;
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.connected && generation == self.generation
    }
    pub fn next_retry_seconds(&mut self) -> Option<u64> {
        if self.attempts >= 5 {
            return None;
        }
        let delay = 1 << self.attempts;
        self.attempts += 1;
        Some(delay)
    }
}

pub fn can_control(
    configured_guild: u64,
    bot_voice: Option<u64>,
    guild: Option<u64>,
    voice: Option<u64>,
) -> bool {
    guild == Some(configured_guild) && bot_voice.is_some() && voice == bot_voice
}

pub fn join_target(
    configured_guild: u64,
    guild: Option<u64>,
    user_voice: Option<u64>,
) -> Result<u64> {
    anyhow::ensure!(
        guild == Some(configured_guild),
        "Use /join in Cadence's configured server"
    );
    user_voice.context("Join a voice channel first, then use /join")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ending_a_session_cancels_an_inflight_control() {
        let cancel = Cancellation::default();
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = started.clone();
        let owner = cancel.clone();
        let task = tokio::spawn(async move {
            owner
                .run(async move {
                    signal.notify_one();
                    std::future::pending::<Result<()>>().await
                })
                .await
        });
        started.notified().await;
        cancel.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(
            cancel
                .run(async {
                    panic!("a cancelled session must not execute another request");
                    #[allow(unreachable_code)]
                    Ok(())
                })
                .await
                .is_err()
        );
    }
    #[test]
    fn a_second_owner_is_rejected_and_shutdown_releases_ownership() {
        let dir = std::env::temp_dir().join(format!(
            "cadence-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = claim(&dir).unwrap();
        assert!(claim(&dir).is_err());
        drop(first);
        drop(claim(&dir).unwrap());
        std::fs::remove_file(dir.join("session.lock")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn stale_callbacks_cannot_revive_ended_sessions() {
        let mut s = Session::default();
        let old = s.begin();
        s.end();
        assert!(!s.accepts(old));
        let current = s.begin();
        assert!(s.accepts(current));
        assert!(!s.accepts(old));
    }
    #[test]
    fn retries_are_bounded() {
        let mut s = Session::default();
        assert_eq!(
            (0..7).map(|_| s.next_retry_seconds()).collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(4), Some(8), Some(16), None, None]
        );
    }
    #[test]
    fn controls_move_with_the_bot_and_require_a_voice_connection() {
        assert!(can_control(1, Some(2), Some(1), Some(2)));
        assert!(!can_control(1, Some(2), Some(3), Some(2)));
        assert!(!can_control(1, Some(2), Some(1), None));
        assert!(!can_control(1, Some(2), Some(1), Some(4)));
        assert!(!can_control(1, None, Some(1), None));
        assert!(!can_control(1, Some(4), Some(1), Some(2)));
        assert!(can_control(1, Some(4), Some(1), Some(4)));
    }
    #[test]
    fn join_uses_the_callers_channel_and_rejects_dms_or_other_servers() {
        assert_eq!(join_target(1, Some(1), Some(2)).unwrap(), 2);
        assert_eq!(join_target(1, Some(1), Some(3)).unwrap(), 3);
        assert!(join_target(1, Some(1), None).is_err());
        assert!(join_target(1, None, Some(2)).is_err());
        assert!(join_target(1, Some(7), Some(2)).is_err());
    }
}
