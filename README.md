# Cadence

A self-hosted Spotify Connect → Discord music bot inspired by [Aoede](https://github.com/codetheweb/aoede). Join a Discord voice channel, use `/join`, then select **Cadence** in Spotify. Cadence stays in that channel until another `/join`; it never follows a person automatically.

**Status: experimental feasibility implementation. Soloist playback is audible in Discord on the DXP2800 NAS with the intended Premium account; control and pause-indicator checks are ongoing.** The original librespot provider failed audio-key requests with this account. Login, device discovery, or a voice connection alone does not prove audio works.

For the Soloist integration, follow [Soloist setup](docs/soloist.md) and use `compose.soloist.yaml`. The older librespot setup below remains available for comparison.

## First version

- One Spotify Premium account, one Discord server, one listening session at a time. `/join` finds your current voice channel and joins or moves Cadence there.
- One shared Spotify queue. Add tracks from Spotify or `/cadence add` in Discord. `/cadence queue` lists up to ten upcoming tracks. Removal/reordering and track-name search are outside this version.
- One updating Discord now-playing message with pause, resume, skip, and queue buttons.
- Everyone currently in Cadence's voice channel can control it. Anyone in the configured server can use `/join` to summon it to their voice channel. Other channels and DMs are rejected. Controls are serialized; concurrent requests receive a busy response.
- Docker images for **linux/amd64**, targeting the Intel N100 UGreen DXP2800 and Portainer on standalone Docker.

## Spotify limitations

Spotify's Web API provides metadata/control rather than raw audio. Cadence supports a local Spotify Soloist audio provider and the original unofficial librespot provider. Soloist audio is captured from a private PulseAudio null-sink monitor, then re-encoded for Discord. It requires Premium, and its authors warn about Spotify's terms. Spotify's Developer Policy prohibits relevant redistribution patterns, including one source played to simultaneous listeners. Premium does not establish redistribution permission. See [Spotify policy](https://developer.spotify.com/policy) and [librespot's disclaimer](https://github.com/librespot-org/librespot#disclaimer).

The developer-app Web API grant and the Connect streaming grant are separate. OAuth with your developer app may succeed while Connect/audio fails; record that failure rather than bypassing it or declaring playback supported. Account, market, app access, and service changes affect availability. Queue read/add also require Web API access and playback scopes. There is no documented public operation for removing/reordering the live playback queue.

The librespot provider requests 320 kbps source audio. Soloist manages its own source quality. Discord receives re-encoded Opus. This is not lossless. Unavailable songs remain unavailable; the integration does not remove ads for free accounts.

## Setup

### 1. Discord

Create an application and bot in the [Discord Developer Portal](https://discord.com/developers/applications). Invite it with `bot` and `applications.commands` scopes. Grant View Channel, Connect, Speak, Send Messages, Read Message History, and Use Application Commands as appropriate. No privileged intents or Message Content intent are needed. Never use a user token.

Choose one **normal text channel** for now-playing messages. Cadence joins a **normal voice channel** when you use `/join`. Stages and threads are outside this version. Enable Discord Developer Mode to copy the server ID and now-playing text-channel ID. No voice-channel ID is required. Give the bot View Channel, Connect, and Speak permissions in any voice channel where you plan to summon it. Store the bot token in a private file named `discord-token`, containing only the token. Do not put it in GitHub, Compose, or chat.

### 2. Browser sign-in on your PC

Create an application in the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) under your Premium account with Web API access. Register this exact redirect URI:

```text
http://127.0.0.1:5588/callback
```

From this folder, using Python 3.10 or newer:

```powershell
python scripts/spotify_login.py --client-id YOUR_SPOTIFY_APP_CLIENT_ID
```

The client ID is public configuration; no client secret is needed. The helper opens your browser, binds only to PC loopback, verifies OAuth state, uses PKCE, and saves a refresh token to `secrets/spotify.json`. It expires after five minutes. Stop it with Ctrl+C if you decline sign-in. It does not log callback URLs or tokens. Required scopes: `streaming`, `user-read-playback-state`, and `user-modify-playback-state`.

If Spotify refuses your app, scopes, or account, record a feasibility failure. Do not supply account passwords or substitute browser-session tokens. Development mode may require adding your account to the app's user allowlist; see [Spotify quota modes](https://developer.spotify.com/documentation/web-api/concepts/quota-modes).

The developer-app grant above supports Web API controls. Spotify Connect audio requires a separate sign-in. With Rust and the build dependencies installed on your PC, run:

```sh
cargo run --example spotify_connect_login --locked
```

Complete the new Spotify browser approval. Librespot presents its supported Spotify desktop client identity, so the page can say **Spotify for Desktop**. Cadence is still unofficial. The helper authenticates once, obtains reusable Connect credentials, and adds them to `secrets/spotify.json` without printing them. Upload the updated JSON to the NAS. On later starts Cadence uses its protected cached Connect session separately from your developer-app OAuth token. If revoked, repeat this flow; do not substitute passwords or browser-session tokens.

`secrets/` is ignored by Git and Docker. Restrict these files to your Windows account before transferring them. Sign-in alone does not establish audio support.

### 3. GitHub image build

Push this folder to your repository on `main` or `master`. The included workflow runs format checks, Clippy, Rust tests, and Python OAuth tests on Linux, then builds an amd64 image. Successful branch/tag/manual builds publish to `ghcr.io/<owner>/<repository>` using the workflow's `GITHUB_TOKEN`. Pull requests do not publish. The project is published at https://github.com/chaseb64/Cadence; pushes trigger the image workflow.

Allow Actions to write packages. Making the container package public simplifies pulling it from Portainer; package visibility is separate from repository visibility. For a private package, configure Portainer's GHCR registry credentials with package read access. Prefer an immutable generated `sha-…` tag over the changing `main` tag.

Local Linux alternative:

```sh
docker build -t cadence:local .
```

The development workspace has no Docker daemon. Confirm the Linux image build through Actions or on your NAS.

### 4. Portainer on the NAS

Transfer `discord-token` and `spotify.json` into a private NAS directory outside the Git checkout. Find its actual absolute path in UGOS. Cadence runs as **UID/GID 10001:10001**; grant that user read access to both files and traversal access to their directory. If using SSH, an example is:

```sh
# Substitute only the private directory you created for Cadence.
sudo chown 10001:10001 /your/private/cadence-secrets
sudo chown 10001:10001 /your/private/cadence-secrets/discord-token /your/private/cadence-secrets/spotify.json
sudo chmod 700 /your/private/cadence-secrets
sudo chmod 400 /your/private/cadence-secrets/discord-token /your/private/cadence-secrets/spotify.json
```

Alternatively grant equivalent permissions in UGOS. These are setup instructions, not commands run by Cadence. Standalone Compose secrets are file mounts, not encrypted storage; NAS permissions matter.

In Portainer's **local Docker environment → Stacks → Add stack**, paste `compose.yaml` into the editor. Set these stack variables (also shown in `.env.example`):

| Variable | Value |
| --- | --- |
| `CADENCE_IMAGE` | `ghcr.io/<lowercase-owner>/<lowercase-repository>:<tested-tag>` |
| `CADENCE_SECRETS_DIR` | Absolute NAS directory containing the two private files |
| `DISCORD_GUILD_ID` | Server ID |
| `DISCORD_TEXT_CHANNEL_ID` | Now-playing text channel ID |

Deploy and inspect container logs. No inbound ports, host networking, NAS audio device, or privileged container is required. Allow outbound HTTPS/WebSocket and Discord voice UDP. This implementation authenticates Connect to the account and does not advertise using LAN mDNS; discovery in your intended Spotify clients is a feasibility check.

A named volume stores the stable device ID, Connect credentials, and working OAuth token. Keep backups private. The OAuth seed is copied only on first start; its writable copy updates when the token rotates. To reauthenticate, stop the stack and deliberately replace `/data/oauth.json` with the new seed; stale `/data/credentials.json` may also need removal. Do not delete the volume to update the image.

The container runs without root, with a read-only root filesystem, dropped capabilities, and bounded temporary storage. The data directory is private. Dependency debug logs are disabled. Docker restart is off during feasibility: Cadence allows five session reconnects (1/2/4/8/16-second backoff), then exits visibly. A reconnect within the running process tries the last channel requested through `/join`; a full container restart waits for a new `/join`. Inspect errors before restarting. SIGTERM and Ctrl+C cancel audio and clean up the gateway.

## Required feasibility test

Run these checks on your NAS and record results in [docs/feasibility.md](docs/feasibility.md):

1. Start Cadence, join a Discord voice channel, run `/join` in a server text channel, then select Cadence in Spotify and play several normal tracks. **Confirm audible, correctly paced music.** A Playing log is insufficient.
2. Pause/resume and skip from Spotify and Discord. Check the now-playing display through track transitions.
3. Add track A from Spotify and B via `/cadence add track:<Spotify URL>`. Inspect both queues; confirm each plays once in order. Discord shows a queue snapshot rather than maintaining another queue.
4. Transfer Spotify playback to another device. Cadence must stop emitting old audio. Discord queue requests must reject until Cadence is selected again. Transfer back and test playback.
5. Restart the container, including during a track. Check persistent identity and cached login.
6. Interrupt networking briefly, test voice disconnect/reconnect, and stop during audio. Check bounded recovery, no hung process, and no old audio after reconnect. PCM buffering is capped at one second.
7. Use `/join` from a different voice channel and confirm Cadence moves there once, with no duplicated audio. Verify listeners in the new channel can control it and people in the old channel cannot. Leave voice and verify `/join` gives a helpful error. A container restart should wait for `/join` again.

If audio fails, keep this experimental regardless of UI or authentication success.

## Development and architecture

Rust 1.89+, CMake, a C/C++ toolchain, and Opus build dependencies are needed. The image build uses Rust 1.99.1 on Debian Bookworm. On Debian/Ubuntu install `build-essential cmake pkg-config libopus-dev`.

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
```

For native execution, set the server and text-channel IDs and private local paths for `DISCORD_TOKEN_FILE`, `SPOTIFY_AUTH_FILE`, and `CADENCE_DATA_DIR`, then `cargo run --locked`. Defaults use container paths.

- `src/main.rs`: Discord validation, controls/display, task ownership, cancellation, and reconnect supervision. Reconnect creates fresh per-session handlers and event channels.
- `src/provider.rs`: provider-independent controls and playback events. A replacement implements this interface and plugs into session construction; there is no plugin framework.
- `src/spotify.rs`: experimental librespot provider, credentials, token refresh, and Web API queue. Native events are translated into a bounded application event channel.
- `src/audio.rs`: bounded PCM bridge, producer backpressure, underrun silence, flush, and cancellation that wakes blocked threads.
- `src/session.rs`: volume ownership lock, cancellation of pending requests, control authorization, and bounded retry rules.
- `scripts/spotify_login.py`: PC-side OAuth PKCE bootstrap.
- `examples/spotify_connect_login.rs`: separate Connect sign-in and secure credential packaging.
- `examples/spotify_check.rs`: a single Connect-session check without Discord audio.

Commit `Cargo.lock`. Librespot 0.8.0 currently needs `vergen` 9.0.6 locked to avoid an upstream trait-version conflict with 9.1.0. Songbird 0.6.1 includes DAVE support introduced in 0.6.0; live compatibility needs the NAS test. This pipeline uses no FFmpeg or NAS audio device.

## Attribution and sources

No Aoede code was copied; preserve MIT attribution if reuse is added later. Cadence is MIT licensed; dependencies retain their licenses. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

- [Songbird 0.6.1](https://github.com/serenity-rs/songbird/releases/tag/v0.6.1)
- [Serenity 0.12.5](https://github.com/serenity-rs/serenity/releases/tag/v0.12.5)
- [librespot 0.8.0](https://github.com/librespot-org/librespot/releases/tag/v0.8.0)
- [Discord voice/DAVE](https://github.com/discord/discord-api-docs/blob/main/developers/topics/voice-connections.mdx)
- [Spotify queue read](https://developer.spotify.com/documentation/web-api/reference/get-queue) / [add](https://developer.spotify.com/documentation/web-api/reference/add-to-queue)
