use anyhow::{Context as _, Result, bail};
use cadence::{
    audio::{AudioPipe, CHANNELS, SAMPLE_RATE},
    config::Config,
    provider::{AudioProvider, PlaybackEvent},
    session::{Session, can_control, join_target},
    spotify::Spotify,
};
use serenity::{all::*, async_trait};
use songbird::{SerenityInit, input::RawAdapter};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::{
    sync::{Mutex, Semaphore, mpsc},
    time::{Duration, timeout},
};

struct Handler {
    config: Arc<Config>,
    provider: Arc<dyn AudioProvider>,
    pipe: AudioPipe,
    initialized: AtomicBool,
    failures: mpsc::Sender<&'static str>,
    display: Arc<Mutex<Option<MessageId>>>,
    control_slot: Semaphore,
    stopping: tokio::sync::watch::Receiver<bool>,
    requested_voice: Arc<AtomicU64>,
    audio_started: AtomicBool,
}

struct VoiceFailure(mpsc::Sender<&'static str>);
#[async_trait]
impl songbird::EventHandler for VoiceFailure {
    async fn act(&self, event: &songbird::EventContext<'_>) -> Option<songbird::Event> {
        // Changing channels deliberately also emits a disconnect event.
        if let songbird::EventContext::DriverDisconnect(data) = event
            && data.reason.is_none()
        {
            return None;
        }
        match event {
            songbird::EventContext::DriverDisconnect(data) => {
                tracing::warn!(reason = ?data.reason, kind = ?data.kind, "Discord voice connection failed");
            }
            songbird::EventContext::Track(tracks) => {
                for (state, _) in *tracks {
                    tracing::warn!(mode = ?state.playing, "PCM audio track failed");
                }
            }
            _ => {}
        }
        let _ = self.0.try_send("Discord voice/audio disconnected");
        None
    }
}

impl Handler {
    async fn setup(&self, ctx: &serenity::client::Context) -> Result<()> {
        let _permit = self.control_slot.acquire().await?;
        let cfg = &self.config;
        let text = ChannelId::new(cfg.text_channel_id)
            .to_channel(&ctx.http)
            .await?
            .guild()
            .context("Configured text channel is not a server channel")?;
        anyhow::ensure!(
            text.guild_id.get() == cfg.guild_id && text.kind == ChannelType::Text,
            "Configure a normal text channel in DISCORD_GUILD_ID"
        );
        let command = CreateCommand::new("cadence")
            .description("Control Cadence in its voice channel")
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "pause",
                "Pause Spotify",
            ))
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "resume",
                "Resume Spotify",
            ))
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "skip",
                "Skip the current song",
            ))
            .add_option(CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "queue",
                "View up to ten upcoming songs",
            ))
            .add_option(
                CreateCommandOption::new(
                    CommandOptionType::SubCommand,
                    "add",
                    "Add a Spotify track to the shared queue",
                )
                .add_sub_option(
                    CreateCommandOption::new(
                        CommandOptionType::String,
                        "track",
                        "Spotify track URL or URI",
                    )
                    .required(true),
                ),
            );
        GuildId::new(cfg.guild_id)
            .create_command(&ctx.http, command)
            .await?;
        GuildId::new(cfg.guild_id)
            .create_command(
                &ctx.http,
                CreateCommand::new("join").description("Join your current voice channel"),
            )
            .await?;
        let requested_voice = self.requested_voice.load(Ordering::SeqCst);
        if requested_voice != 0 {
            self.connect_voice(ctx, ChannelId::new(requested_voice))
                .await?;
        }
        let channel = ChannelId::new(cfg.text_channel_id);
        let record = cfg.data_dir.join("now-playing.json");
        let previous = std::fs::read(&record)
            .ok()
            .and_then(|data| serde_json::from_slice::<(u64, u64)>(&data).ok());
        let content =
            "Cadence ready. Join a voice channel and use /join, then select Cadence in Spotify.";
        let mut message_id = None;
        if let Some((channel_id, id)) = previous
            && channel_id == cfg.text_channel_id
            && let Ok(message) = channel.message(&ctx.http, MessageId::new(id)).await
            && message.author.id == ctx.cache.current_user().id
        {
            channel
                .edit_message(
                    &ctx.http,
                    message.id,
                    EditMessage::new()
                        .content(content)
                        .allowed_mentions(CreateAllowedMentions::new())
                        .components(buttons()),
                )
                .await?;
            message_id = Some(message.id);
        }
        let id = match message_id {
            Some(id) => id,
            None => {
                channel
                    .send_message(
                        &ctx.http,
                        CreateMessage::new()
                            .content(content)
                            .allowed_mentions(CreateAllowedMentions::new())
                            .components(buttons()),
                    )
                    .await?
                    .id
            }
        };
        std::fs::write(
            record,
            serde_json::to_vec(&(cfg.text_channel_id, id.get()))?,
        )?;
        *self.display.lock().await = Some(id);
        tracing::info!("Discord ready; use /join from a voice channel");
        Ok(())
    }
    async fn connect_voice(
        &self,
        ctx: &serenity::client::Context,
        target: ChannelId,
    ) -> Result<()> {
        let cfg = &self.config;
        let voice = target
            .to_channel(&ctx.http)
            .await?
            .guild()
            .context("Target is not a server voice channel")?;
        anyhow::ensure!(
            voice.guild_id.get() == cfg.guild_id && voice.kind == ChannelType::Voice,
            "Join a normal voice channel in this server; Stage channels are not supported"
        );
        let manager = songbird::get(ctx)
            .await
            .context("Voice manager unavailable")?;
        self.pipe.clear();
        let joined = timeout(
            Duration::from_secs(30),
            manager.join(GuildId::new(cfg.guild_id), target),
        )
        .await;
        let call = match joined {
            Ok(Ok(call)) => call,
            error => {
                self.requested_voice.store(0, Ordering::SeqCst);
                let _ = timeout(
                    Duration::from_secs(3),
                    manager.remove(GuildId::new(cfg.guild_id)),
                )
                .await;
                self.audio_started.store(false, Ordering::SeqCst);
                self.pipe.clear();
                return match error {
                    Err(_) => Err(anyhow::anyhow!(
                        "Voice connection timed out; check Connect/Speak permissions and try /join again"
                    )),
                    Ok(Err(error)) => Err(anyhow::anyhow!(
                        "Could not join your voice channel: {error}. Check View Channel, Connect and Speak permissions"
                    )),
                    Ok(Ok(_)) => unreachable!(),
                };
            }
        };
        let mut call = call.lock().await;
        if !self.audio_started.swap(true, Ordering::SeqCst) {
            call.add_global_event(
                songbird::Event::Core(songbird::CoreEvent::DriverDisconnect),
                VoiceFailure(self.failures.clone()),
            );
            call.add_global_event(
                songbird::Event::Track(songbird::TrackEvent::Error),
                VoiceFailure(self.failures.clone()),
            );
            call.play_only_input(RawAdapter::new(self.pipe.clone(), SAMPLE_RATE, CHANNELS).into());
        }
        drop(call);
        self.requested_voice.store(target.get(), Ordering::SeqCst);
        tracing::info!("Discord voice joined; audible playback still needs verification");
        Ok(())
    }
    fn user_voice(
        &self,
        ctx: &serenity::client::Context,
        guild: Option<GuildId>,
        user: UserId,
    ) -> Option<u64> {
        guild
            .and_then(|id| {
                ctx.cache
                    .guild(id)
                    .and_then(|g| g.voice_states.get(&user).and_then(|v| v.channel_id))
            })
            .map(|id| id.get())
    }
    async fn dispatch(
        &self,
        ctx: &serenity::client::Context,
        guild: Option<GuildId>,
        user: UserId,
        action: &str,
        track: Option<&str>,
    ) -> Result<String> {
        let _permit = self
            .control_slot
            .try_acquire()
            .context("Another control request is running; try again shortly")?;
        anyhow::ensure!(
            !*self.stopping.borrow(),
            "Cadence is reconnecting; try again shortly"
        );
        let user_voice = self.user_voice(ctx, guild, user);
        if action == "join" {
            let target = join_target(self.config.guild_id, guild.map(|id| id.get()), user_voice)?;
            let mut stopping = self.stopping.clone();
            tokio::select! {
                result = self.connect_voice(ctx, ChannelId::new(target)) => result?,
                _ = stopping.changed() => bail!("Cadence is reconnecting; try /join again shortly"),
            }
            return Ok(format!(
                "Joined <#{target}>. Select Cadence in Spotify to play music."
            ));
        }
        let manager = songbird::get(ctx)
            .await
            .context("Voice manager unavailable")?;
        let bot_voice = if let Some(call) = manager.get(GuildId::new(self.config.guild_id)) {
            call.lock().await.current_channel().map(|id| id.0.get())
        } else {
            None
        };
        anyhow::ensure!(
            self.audio_started.load(Ordering::SeqCst)
                && can_control(
                    self.config.guild_id,
                    bot_voice,
                    guild.map(|id| id.get()),
                    user_voice
                ),
            "Join Cadence's voice channel to use these controls. Use /join to bring Cadence to you"
        );
        match action {
            "pause" => {
                self.provider.pause().await?;
                Ok("Pause requested.".into())
            }
            "resume" => {
                self.provider.resume().await?;
                Ok("Resume requested.".into())
            }
            "skip" => {
                self.provider.skip().await?;
                Ok("Skip requested.".into())
            }
            "add" => {
                self.provider
                    .enqueue(track.context("Supply a Spotify track link")?)
                    .await?;
                Ok("Track added to Spotify’s queue.".into())
            }
            "queue" => {
                let tracks = self.provider.queue().await?;
                if tracks.is_empty() {
                    return Ok("Spotify’s upcoming queue is empty.".into());
                }
                Ok(tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("{}. {}", i + 1, safe_text(t)))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            _ => bail!("Unknown Cadence control"),
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: serenity::client::Context, _: Ready) {
        if self.initialized.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut stopping = self.stopping.clone();
        if *stopping.borrow() {
            return;
        }
        let result = tokio::select! { result = self.setup(&ctx) => result, _ = stopping.changed() => { return; } };
        if let Err(error) = result {
            tracing::error!(%error, "Discord setup failed");
            let _ = self
                .failures
                .try_send("Discord setup failed; check bot permissions and channel configuration");
        }
    }
    async fn interaction_create(&self, ctx: serenity::client::Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(command)
                if matches!(command.data.name.as_str(), "cadence" | "join") =>
            {
                if command.defer_ephemeral(&ctx.http).await.is_err() {
                    return;
                }
                let action = command.data.options.first();
                let name = if command.data.name == "join" {
                    "join"
                } else {
                    action.map(|o| o.name.as_str()).unwrap_or("")
                };
                let track = action.and_then(|o| match &o.value {
                    CommandDataOptionValue::SubCommand(options) => {
                        options.first().and_then(|v| v.value.as_str())
                    }
                    _ => None,
                });
                let result = self.dispatch(&ctx, command.guild_id, command.user.id, name, track).await.unwrap_or_else(|e| format!("Couldn’t complete that request: {e}. If an add timed out, check Spotify before trying again."));
                let _ = command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(result)
                            .allowed_mentions(CreateAllowedMentions::new()),
                    )
                    .await;
            }
            Interaction::Component(component)
                if component.data.custom_id.starts_with("cadence:") =>
            {
                if component.defer_ephemeral(&ctx.http).await.is_err() {
                    return;
                }
                let result = self
                    .dispatch(
                        &ctx,
                        component.guild_id,
                        component.user.id,
                        component.data.custom_id.trim_start_matches("cadence:"),
                        None,
                    )
                    .await
                    .unwrap_or_else(|e| format!("Request failed: {e}"));
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(result)
                            .allowed_mentions(CreateAllowedMentions::new()),
                    )
                    .await;
            }
            _ => {}
        }
    }
}

fn buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("cadence:pause")
            .label("Pause")
            .style(ButtonStyle::Secondary),
        CreateButton::new("cadence:resume")
            .label("Resume")
            .style(ButtonStyle::Success),
        CreateButton::new("cadence:skip")
            .label("Skip")
            .style(ButtonStyle::Primary),
        CreateButton::new("cadence:queue")
            .label("Queue")
            .style(ButtonStyle::Secondary),
    ])]
}
fn safe_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '@' | '`' | '*' | '_' | '~' | '[' | ']' | '<' | '>' | '\n' | '\r'
            )
        })
        .take(140)
        .collect()
}

async fn run_session(
    config: Arc<Config>,
    stop: &mut tokio::sync::watch::Receiver<bool>,
    requested_voice: Arc<AtomicU64>,
) -> Result<()> {
    if *stop.borrow() {
        return Ok(());
    }
    let pipe = AudioPipe::default();
    let started = tokio::select! {
        result = Spotify::start(&config.data_dir, &config.spotify_auth_file, config.device_name.clone(), pipe.clone()) => result,
        _ = stop.changed() => { pipe.close(); return Ok(()); }
    };
    let (spotify, mut events, mut spotify_task) = match started {
        Ok(v) => v,
        Err(e) => {
            pipe.close();
            return Err(e);
        }
    };
    let display = Arc::new(Mutex::new(None));
    let (tx, mut failures) = mpsc::channel(8);
    let (stopping_tx, stopping_rx) = tokio::sync::watch::channel(false);
    let handler = Handler {
        config: config.clone(),
        provider: spotify.clone(),
        pipe: pipe.clone(),
        initialized: AtomicBool::new(false),
        failures: tx,
        display: display.clone(),
        control_slot: Semaphore::new(1),
        stopping: stopping_rx,
        requested_voice,
        audio_started: AtomicBool::new(false),
    };
    let intents = GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES;
    let voice_manager = songbird::Songbird::serenity();
    let client_result = tokio::select! {
        result = Client::builder(&config.discord_token, intents).event_handler(handler).register_songbird_with(voice_manager.clone()) => result,
        _ = stop.changed() => { spotify.shutdown(); spotify_task.abort(); return Ok(()); }
    };
    let mut client = match client_result {
        Ok(c) => c,
        Err(e) => {
            spotify.shutdown();
            spotify_task.abort();
            return Err(e.into());
        }
    };
    let http = client.http.clone();
    let shards = client.shard_manager.clone();
    let mut discord_task = tokio::spawn(async move { client.start().await });
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    let mut title = "Waiting for Spotify".to_owned();
    let mut state = "Idle";
    let mut previous = String::new();
    let mut discord_done = false;
    let mut spotify_done = false;
    let result = loop {
        tokio::select! {
            _ = stop.changed() => { break Ok(()); }
            result = &mut discord_task => { discord_done = true; break Err(anyhow::anyhow!("Discord gateway stopped: {}", if result.is_ok() { "client ended" } else { "task failed" })); }
            _ = &mut spotify_task => { spotify_done = true; break Err(anyhow::anyhow!("Spotify connection stopped")); }
            Some(reason) = failures.recv() => { break Err(anyhow::anyhow!(reason)); }
            _ = ticker.tick() => {
                let content = format!("**Cadence · {state}**\n{}\nUse `/join` from your voice channel. Manage songs in Spotify or use `/cadence add` and `/cadence queue`.", safe_text(&title));
                if previous != content && let Some(id) = *display.lock().await {
                        match ChannelId::new(config.text_channel_id).edit_message(&http,id,EditMessage::new().content(&content).allowed_mentions(CreateAllowedMentions::new()).components(buttons())).await {
                            Ok(_) => previous = content,
                            Err(_) => tracing::warn!("Now-playing update failed; will retry"),
                        }
                }
            }
            event = events.recv() => {
                match event {
                    Some(PlaybackEvent::Track { name }) => { title = name; }
                    Some(PlaybackEvent::Playing) => { state = "Playing"; tracing::info!("Spotify reports playing; confirm audibility in Discord"); }
                    Some(PlaybackEvent::Paused) => { state = "Paused"; pipe.clear(); }
                    Some(PlaybackEvent::Stopped) => { state = "Idle"; pipe.clear(); }
                    Some(PlaybackEvent::Unavailable) => { state = "Track unavailable"; pipe.clear(); }
                    None => { break Err(anyhow::anyhow!("Spotify player event stream closed")); }
                }
            }
        }
    };
    let _ = stopping_tx.send(true);
    spotify.shutdown();
    let _ = timeout(
        Duration::from_secs(3),
        voice_manager.remove(GuildId::new(config.guild_id)),
    )
    .await;
    if let Some(id) = *display.lock().await {
        let _ = timeout(
            Duration::from_secs(3),
            ChannelId::new(config.text_channel_id).edit_message(
                &http,
                id,
                EditMessage::new()
                    .content("Cadence is offline or reconnecting.")
                    .components(Vec::new()),
            ),
        )
        .await;
    }
    let _ = timeout(Duration::from_secs(5), shards.shutdown_all()).await;
    discord_task.abort();
    spotify_task.abort();
    if !discord_done {
        let _ = discord_task.await;
    }
    if !spotify_done {
        let _ = spotify_task.await;
    }
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    // Keep dependency debug/trace disabled: some clients log credential-bearing data.
    tracing_subscriber::fmt()
        .with_env_filter("cadence=info,serenity=warn,songbird=warn,librespot=off")
        .init();
    let config = Arc::new(Config::load()?);
    let _session_lock = cadence::session::claim(&config.data_dir)?;
    let mut session = Session::default();
    let requested_voice = Arc::new(AtomicU64::new(0));
    // Signal handling covers both local Ctrl+C and Docker's SIGTERM.
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        let _ = stop_tx.send(true);
    });
    loop {
        session.begin();
        let result = run_session(config.clone(), &mut stop_rx, requested_voice.clone()).await;
        session.end();
        if *stop_rx.borrow() {
            return Ok(());
        }
        if let Err(error) = result {
            tracing::warn!(%error, "Session ended");
        }
        let Some(delay) = session.next_retry_seconds() else {
            bail!("Reconnect budget exhausted; inspect logs and restart manually");
        };
        tracing::info!(delay_seconds = delay, "Retrying session");
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(delay)) => {}, _ = stop_rx.changed() => { return Ok(()); } }
    }
}
