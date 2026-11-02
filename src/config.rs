use anyhow::{Context, Result, ensure};
use std::{env, path::PathBuf};

// Deliberately no Debug implementation: configuration contains a bot token.
pub struct Config {
    pub discord_token: String,
    pub guild_id: u64,
    pub text_channel_id: u64,
    pub data_dir: PathBuf,
    pub spotify_auth_file: PathBuf,
    pub device_name: String,
}

pub fn snowflake(name: &str, value: &str) -> Result<u64> {
    let id: u64 = value
        .parse()
        .with_context(|| format!("{name} must be a Discord ID"))?;
    ensure!(id != 0, "{name} must be nonzero");
    Ok(id)
}

impl Config {
    pub fn load() -> Result<Self> {
        let id = |name| {
            snowflake(
                name,
                &env::var(name).with_context(|| format!("Set {name}"))?,
            )
        };
        let token_path =
            env::var("DISCORD_TOKEN_FILE").unwrap_or_else(|_| "/run/secrets/discord_token".into());
        let discord_token = std::fs::read_to_string(token_path)
            .context("Read DISCORD_TOKEN_FILE")?
            .trim()
            .to_owned();
        ensure!(!discord_token.is_empty(), "Discord token file is empty");
        let device_name = env::var("SPOTIFY_DEVICE_NAME").unwrap_or_else(|_| "Cadence".into());
        ensure!(
            (1..=64).contains(&device_name.chars().count()),
            "Device name must be 1–64 characters"
        );
        Ok(Self {
            discord_token,
            guild_id: id("DISCORD_GUILD_ID")?,
            text_channel_id: id("DISCORD_TEXT_CHANNEL_ID")?,
            data_dir: env::var("CADENCE_DATA_DIR")
                .unwrap_or_else(|_| "/data".into())
                .into(),
            spotify_auth_file: env::var("SPOTIFY_AUTH_FILE")
                .unwrap_or_else(|_| "/run/secrets/spotify_auth".into())
                .into(),
            device_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ids_are_rejected() {
        for s in ["", "0", "-1", "abc", "18446744073709551616"] {
            assert!(snowflake("test", s).is_err());
        }
        assert_eq!(snowflake("test", "123").unwrap(), 123);
    }
}
