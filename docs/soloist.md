# Soloist on Docker / Portainer

Cadence uses the official Soloist engine as an independent source. The public image contains PulseAudio and the Cadence bridge, **not Spotify's binary or your key**. Soloist's local WebSocket controls playback and queue; the monitor captures stereo float32 PCM at 48 kHz (PulseAudio converts the source as needed). No audio files are recorded. The bridge holds at most one second, drains while paused, and cancels capture on shutdown.

## Setup

1. Download the Linux amd64 Soloist archive directly from [Spotify's downloads page](https://developer.spotify.com/documentation/soloist/reference/downloads-and-updates). Extract its `soloist` executable. Do not commit or redistribute it. Releases expire; replace the mounted binary before expiry, preserving the data volume.
2. Generate your own key in the Spotify Soloist dashboard. Place `soloist`, `soloist-key`, and `discord-token` in your private NAS secrets directory. The keys must be readable only by UID 10001 (0400); the downloaded binary needs read access for UID 10001. Cadence copies it into its private writable data volume and marks that copy executable. Do not put keys in Compose, GitHub, or chat.
3. In Portainer, create/update the stack using `compose.soloist.yaml`. Set `CADENCE_IMAGE` to the tested immutable image tag, `CADENCE_SECRETS_DIR` to the absolute private directory, and the two Discord IDs. Stop any older Cadence/Soloist test instance first; do not run duplicate players against the same data volume.
4. Deploy. Select **Cadence** from Spotify's device picker and complete any account pairing that Spotify requests. The Web API developer-app credentials used by the old provider are not needed for Soloist queue/control.
5. Join your Discord voice channel and run `/join`. Start a song on Cadence from Spotify. Verify audible music, then pause/resume/skip, queue view/add, device transfer to PC, and silence while paused. An advancing Spotify timer is insufficient proof of Discord audio.

This Compose uses Linux host networking for Spotify LAN discovery. The unauthenticated WebSocket binds **127.0.0.1:9090 only**; never change it to 0.0.0.0, publish it, or tunnel it. Other processes on the NAS can access loopback. PulseAudio uses a local per-user Unix socket; no sound hardware or privileged container is required. The root filesystem is read-only; temporary storage and memory are bounded. `init: true` reaps child processes. Stopping the container terminates Cadence, Soloist, and PulseAudio.

The `soloist-data` volume contains the paired session and must be treated as a credential. Keep it private and preserve it when replacing images. For the existing feasibility deployment, explicitly map this volume to `cadence-soloist-test_soloist-test-data` as an external volume to retain pairing. The Discord display volume can similarly retain its existing external volume.

## Failure behavior and limitations

The provider serializes local commands through a bounded channel. An eight-second command timeout ends its socket/session because this protocol has no request IDs; a late reply cannot satisfy another request. Do not blindly retry timed-out queue additions. Connection/capture failures use Cadence's bounded session reconnects; exhausting the budget exits visibly with automatic container restart disabled during feasibility. Capture process arguments and logs do not contain account credentials. Soloist itself receives its key through its supported CLI argument; NAS administrators can inspect container processes.

The audio gate opens only while this device is active and playing. Paused, buffering, logged-out, and transferred sessions clear queued audio. Discord tracks start paused when idle. Verify the visible Discord speaking indicator on the actual deployment; automated PCM/session tests do not prove its UI behavior.

The official player does not grant permission to redistribute Spotify audio. Spotify's platform-policy restrictions still apply. Follow the [Soloist documentation](https://developer.spotify.com/documentation/soloist) and [Spotify policy](https://developer.spotify.com/policy).

Feasibility record: Soloist 1.3.10.33 (2026-11-02) on the DXP2800 accepted the user's paired Premium account and advanced tracks without the librespot skip loop. On the integrated image, the user confirmed audible Spotify music in Discord. The user confirmed pause, skip, and queue controls work and the speaking indicator turns off when paused. Audio quality remains under investigation.
