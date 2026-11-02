//! A one-second bounded PCM bridge. Closing wakes blocked readers and writers.
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};
use symphonia::core::io::MediaSource;

pub const SAMPLE_RATE: u32 = 44_100;
pub const CHANNELS: u32 = 2;
const CAPACITY: usize = SAMPLE_RATE as usize * CHANNELS as usize * 4;

#[derive(Default)]
struct State {
    bytes: VecDeque<u8>,
    closed: bool,
    epoch: u64,
}
#[derive(Clone)]
pub struct AudioPipe(Arc<(Mutex<State>, Condvar)>);

impl Default for AudioPipe {
    fn default() -> Self {
        Self(Arc::new((Mutex::new(State::default()), Condvar::new())))
    }
}
impl AudioPipe {
    pub fn clear(&self) {
        let (lock, changed) = &*self.0;
        let mut s = lock.lock().unwrap();
        s.bytes.clear();
        s.epoch += 1;
        changed.notify_all();
    }
    pub fn close(&self) {
        let (lock, changed) = &*self.0;
        let mut s = lock.lock().unwrap();
        s.closed = true;
        s.bytes.clear();
        changed.notify_all();
    }
    pub fn write_samples(&self, samples: &[f64]) -> io::Result<()> {
        let (lock, changed) = &*self.0;
        let epoch = lock.lock().unwrap().epoch;
        for block in samples.chunks(1024) {
            let bytes: Vec<u8> = block
                .iter()
                .flat_map(|s| {
                    let sample = if s.is_finite() {
                        s.clamp(-1.0, 1.0) as f32
                    } else {
                        0.0
                    };
                    sample.to_le_bytes()
                })
                .collect();
            let mut s = lock.lock().unwrap();
            while !s.closed && s.epoch == epoch && s.bytes.len() + bytes.len() > CAPACITY {
                s = changed.wait(s).unwrap();
            }
            if s.closed {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if s.epoch != epoch {
                return Ok(());
            }
            s.bytes.extend(bytes);
            changed.notify_all();
        }
        Ok(())
    }
}
impl Read for AudioPipe {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let (lock, changed) = &*self.0;
        let mut s = lock.lock().unwrap();
        if s.bytes.is_empty() && !s.closed {
            s = changed
                .wait_timeout(s, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        if s.closed {
            return Ok(0);
        }
        if s.bytes.is_empty() {
            // Silence on underrun, never end-of-stream. Limit to 20ms of audio.
            let n = out.len().min(7056);
            out[..n].fill(0);
            return Ok(n);
        }
        let n = out.len().min(s.bytes.len());
        for b in &mut out[..n] {
            *b = s.bytes.pop_front().unwrap();
        }
        changed.notify_all();
        Ok(n)
    }
}
impl Seek for AudioPipe {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
impl MediaSource for AudioPipe {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread};
    #[test]
    fn songbird_can_decode_the_live_pcm_bridge() {
        use symphonia::core::{
            formats::{FormatOptions, FormatReader},
            io::{MediaSourceStream, MediaSourceStreamOptions},
        };
        let pipe = AudioPipe::default();
        pipe.write_samples(&vec![0.25; 1764]).unwrap();
        let source = songbird::input::RawAdapter::new(pipe, SAMPLE_RATE, CHANNELS);
        let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        let mut reader =
            songbird::input::codecs::RawReader::try_new(stream, &FormatOptions::default()).unwrap();
        let params = reader.default_track().unwrap().codec_params.clone();
        let mut decoder = songbird::input::codecs::get_codec_registry()
            .make(&params, &Default::default())
            .expect("PCM decoder must be enabled for the live bridge");
        let packet = reader.next_packet().unwrap();
        let decoded = decoder.decode(&packet).unwrap();
        assert_eq!(decoded.spec().rate, SAMPLE_RATE);
        assert!(decoded.frames() > 0);
    }
    #[test]
    fn close_releases_a_blocked_producer_and_stops_output() {
        let pipe = AudioPipe::default();
        let producer = pipe.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            tx.send(producer.write_samples(&vec![0.25; CAPACITY]))
                .unwrap();
        });
        // Wait until the producer fills the buffer; no unbounded sleep.
        let (lock, changed) = &*pipe.0;
        let state = lock.lock().unwrap();
        let (state, timeout) = changed
            .wait_timeout_while(state, Duration::from_secs(2), |s| {
                s.bytes.len() < CAPACITY - 4096
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(state.bytes.len() <= CAPACITY);
        drop(state);
        pipe.close();
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        worker.join().unwrap();
        assert_eq!(pipe.clone().read(&mut [0; 8]).unwrap(), 0);
    }
    #[test]
    fn clear_discards_queued_audio() {
        let mut pipe = AudioPipe::default();
        pipe.write_samples(&[0.5, -0.5]).unwrap();
        pipe.clear();
        let mut bytes = [1; 8];
        assert_eq!(pipe.read(&mut bytes).unwrap(), 8);
        assert_eq!(bytes, [0; 8]);
    }
    #[test]
    fn samples_have_explicit_little_endian_format() {
        let mut pipe = AudioPipe::default();
        pipe.write_samples(&[0.5, f64::NAN]).unwrap();
        let mut b = [0; 8];
        pipe.read_exact(&mut b).unwrap();
        assert_eq!(&b[..4], &0.5_f32.to_le_bytes());
        assert_eq!(&b[4..], &[0; 4]);
    }
    #[test]
    fn clearing_a_full_buffer_releases_the_old_writer_without_stale_audio() {
        let mut pipe = AudioPipe::default();
        let writer = pipe.clone();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            tx.send(writer.write_samples(&vec![0.75; CAPACITY]))
                .unwrap();
        });
        let (lock, changed) = &*pipe.0;
        let state = lock.lock().unwrap();
        let (state, timed) = changed
            .wait_timeout_while(state, Duration::from_secs(2), |s| {
                s.bytes.len() < CAPACITY - 4096
            })
            .unwrap();
        assert!(!timed.timed_out());
        drop(state);
        pipe.clear();
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
        worker.join().unwrap();
        let mut bytes = [1; 8];
        pipe.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
    }
}
