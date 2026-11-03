//! Presentation state for the current track, independent of Discord networking.
use crate::provider::TrackInfo;
use std::time::Instant;

#[derive(Default)]
pub struct NowPlaying {
    pub track: Option<TrackInfo>,
    position: Option<(u64, Instant)>,
    running: bool,
    speed: f64,
}
impl NowPlaying {
    pub fn track(&mut self, track: TrackInfo) {
        self.track = Some(track);
        self.position = None;
    }
    pub fn position(
        &mut self,
        position_ms: u64,
        timestamp_ms: u64,
        speed: f64,
        now_ms: u64,
        now: Instant,
    ) {
        // Soloist's timestamp is an epoch anchor. Bound stale/invalid values and overflow.
        let elapsed = now_ms
            .checked_sub(timestamp_ms)
            .filter(|age| *age <= 86_400_000)
            .unwrap_or(0);
        let advance = if self.running {
            (elapsed as f64 * speed) as u64
        } else {
            0
        };
        self.position = Some((position_ms.saturating_add(advance), now));
        if speed > 0.0 {
            self.speed = speed;
        }
    }
    pub fn set_running(&mut self, running: bool, now: Instant) {
        if self.running != running {
            if let Some(position) = self.position_ms(now) {
                self.position = Some((position, now));
            }
            self.running = running;
        }
    }
    pub fn clear(&mut self) {
        self.track = None;
        self.position = None;
        self.running = false;
        self.speed = 0.0;
    }
    pub fn position_ms(&self, now: Instant) -> Option<u64> {
        let (base, anchor) = self.position?;
        let advance = if self.running {
            let elapsed = now
                .saturating_duration_since(anchor)
                .as_millis()
                .min(u64::MAX as u128) as f64;
            (elapsed * if self.speed > 0.0 { self.speed } else { 1.0 }) as u64
        } else {
            0
        };
        let position = base.saturating_add(advance);
        Some(
            self.track
                .as_ref()
                .and_then(|t| t.duration_ms)
                .map_or(position, |duration| position.min(duration)),
        )
    }
    pub fn timer(&self, now: Instant) -> Option<String> {
        fn format_time(ms: u64) -> String {
            let s = ms / 1000;
            format!("{}:{:02}", s / 60, s % 60)
        }
        let position = format_time(self.position_ms(now)?);
        Some(match self.track.as_ref().and_then(|t| t.duration_ms) {
            Some(duration) => format!("{position} / {}", format_time(duration)),
            None => position,
        })
    }
    pub fn activity(&self, now: Instant) -> Option<String> {
        let track = self.track.as_ref()?;
        let name: String = track
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(45)
            .collect();
        if name.is_empty() {
            return None;
        }
        let mut text = if self.running {
            name
        } else {
            format!("Paused · {name}")
        };
        if let Some(timer) = self.timer(now) {
            text.push_str(" · ");
            text.push_str(&timer);
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn advances_pauses_resumes_seeks_and_resets() {
        let t = Instant::now();
        let mut p = NowPlaying::default();
        p.track(TrackInfo {
            name: "Song".into(),
            duration_ms: Some(90_000),
            ..Default::default()
        });
        assert_eq!(p.timer(t), None);
        p.set_running(true, t);
        p.position(10_000, 1_000, 1.0, 2_000, t);
        assert_eq!(
            p.timer(t + Duration::from_secs(4)).as_deref(),
            Some("0:15 / 1:30")
        );
        p.set_running(false, t + Duration::from_secs(4));
        assert_eq!(
            p.timer(t + Duration::from_secs(20)).as_deref(),
            Some("0:15 / 1:30")
        );
        p.set_running(true, t + Duration::from_secs(20));
        assert_eq!(
            p.timer(t + Duration::from_secs(25)).as_deref(),
            Some("0:20 / 1:30")
        );
        p.position(89_000, 0, 1.0, 0, t + Duration::from_secs(25));
        assert_eq!(
            p.timer(t + Duration::from_secs(40)).as_deref(),
            Some("1:30 / 1:30")
        );
        p.track(TrackInfo {
            name: "Next".into(),
            ..Default::default()
        });
        assert_eq!(p.timer(t + Duration::from_secs(40)), None);
        p.clear();
        assert_eq!(p.activity(t), None);
    }
}
