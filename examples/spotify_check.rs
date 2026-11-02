//! Checks Connect authentication once without joining Discord or playing audio.
use cadence::{audio::AudioPipe, spotify::Spotify};
use std::path::Path;

#[tokio::main]
async fn main() {
    let pipe = AudioPipe::default();
    let result = Spotify::start(
        Path::new(".tools/spotify-check-data"),
        Path::new("secrets/spotify.json"),
        "Cadence setup check".into(),
        pipe.clone(),
    )
    .await;
    match result {
        Ok((provider, _, task)) => {
            provider.shutdown();
            task.abort();
            println!("Connect session initialized. Audio playback has NOT been verified.");
        }
        Err(error) => {
            pipe.close();
            for cause in error.chain() {
                // No credential-bearing debug logging. Redact long values, URLs,
                // and account identifiers from the human-readable error chain.
                let text = cause
                    .to_string()
                    .split_whitespace()
                    .map(|word| {
                        if word.len() > 48 || word.contains('@') || word.contains("http") {
                            "<redacted>".to_string()
                        } else {
                            word.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("{text}");
            }
        }
    }
}
