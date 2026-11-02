//! Completes a distinct Connect grant and stores reusable credentials in the
//! existing Spotify secret. Does not join Discord or print any credentials.
use anyhow::{Context, Result, ensure};
use librespot::core::{
    authentication::Credentials, cache::Cache, config::SessionConfig, session::Session,
};
use serde::Deserialize;
use std::{path::Path, process::Command, time::Duration};

#[derive(Deserialize)]
struct Grant {
    client_id: String,
    refresh_token: String,
}
#[derive(Deserialize)]
struct AccessToken {
    access_token: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let secret = Path::new("secrets/spotify.json");
    ensure!(secret.exists(), "Complete developer-app sign-in first");
    let mut seed: serde_json::Value = serde_json::from_slice(&std::fs::read(secret)?)?;
    let oauth_path = Path::new(".tools/connect-oauth.json");
    if !oauth_path.exists() {
        let client_id = SessionConfig::default().client_id;
        println!("Opening the separate Spotify Connect sign-in. Complete it in your browser.");
        let status = tokio::task::spawn_blocking(move || {
            Command::new("python")
                .args([
                    "scripts/spotify_login.py",
                    "--client-id",
                    &client_id,
                    "--redirect-uri",
                    "http://127.0.0.1:5588/login",
                    "--scopes",
                    "streaming",
                    "--output",
                    ".tools/connect-oauth.json",
                ])
                .status()
        })
        .await??;
        ensure!(status.success(), "Connect sign-in did not complete");
    }
    let grant: Grant = serde_json::from_slice(&std::fs::read(oauth_path)?)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    let response = http
        .post("https://accounts.spotify.com/api/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", grant.client_id.as_str()),
            ("refresh_token", grant.refresh_token.as_str()),
        ])
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "Connect token refresh failed (HTTP {})",
        response.status()
    );
    let token: AccessToken = response.json().await?;
    let dir = Path::new(".tools/connect-login-cache");
    let cache = Cache::new(Some(dir), None, None, None)?;
    let session = Session::new(SessionConfig::default(), Some(cache.clone()));
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        session.connect(Credentials::with_access_token(token.access_token), true),
    )
    .await;
    session.shutdown();
    result
        .context("Connect authentication timed out")?
        .map_err(|error| anyhow::anyhow!("Connect authentication failed ({})", error.kind))?;
    let credentials = cache
        .credentials()
        .context("Spotify did not return reusable Connect credentials")?;
    seed["connect_credentials"] = serde_json::to_value(credentials)?;
    let temporary = secret.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec(&seed)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, secret)?;
    println!(
        "Connect sign-in verified. Updated secrets/spotify.json privately; upload that file to the NAS again."
    );
    println!("This verifies account authentication, not audible Discord playback.");
    Ok(())
}
