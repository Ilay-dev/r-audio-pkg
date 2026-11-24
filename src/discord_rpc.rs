use crossbeam_channel::{unbounded, RecvTimeoutError, Sender};
use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1574490783363506296";
const RUSTIFY_ICON_URL: &str =
    "https://raw.githubusercontent.com/Ilay-dev/Rustify/main/assets/rustify_256.png";
const RUSTIFY_DOWNLOAD_URL: &str = "https://ilay-dev.github.io/Rustify-Download/";
const RETRY_INTERVAL: Duration = Duration::from_secs(5);

fn effective_client_id(custom: &str) -> &str {
    let t = custom.trim();
    if t.is_empty() {
        DEFAULT_DISCORD_CLIENT_ID
    } else {
        t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RpcStatus {
    Disabled = 0,
    WaitingForDiscord = 1,
    Connected = 2,
}

impl RpcStatus {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => RpcStatus::WaitingForDiscord,
            2 => RpcStatus::Connected,
            _ => RpcStatus::Disabled,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RpcTrackInfo {
    pub title: String,
    pub artist: String,
    pub playlist: String,
    pub position_sec: f32,
    pub duration_sec: f32,
    pub is_playing: bool,
}

enum RpcMsg {
    SetConfig { enabled: bool, client_id: String },
    UpdateTrack(Option<RpcTrackInfo>),
    Shutdown,
}

pub struct DiscordRpcManager {
    tx: Sender<RpcMsg>,
    status: Arc<AtomicU8>,
    last_sent: Option<(RpcTrackInfo, Instant)>,
}

impl DiscordRpcManager {
    pub fn new(enabled: bool, client_id: String) -> Self {
        let initial_status = if !enabled {
            RpcStatus::Disabled
        } else {
            RpcStatus::WaitingForDiscord
        };

        let status = Arc::new(AtomicU8::new(initial_status as u8));
        let status_bg = Arc::clone(&status);
        let (tx, rx) = unbounded::<RpcMsg>();

        let _ = thread::Builder::new()
            .name("rustify-discord-rpc".to_string())
            .spawn(move || {
                rpc_worker_loop(enabled, client_id, rx, status_bg);
            });

        Self {
            tx,
            status,
            last_sent: None,
        }
    }

    pub fn set_config(&mut self, enabled: bool, client_id: String) {
        let _ = self.tx.send(RpcMsg::SetConfig { enabled, client_id });
    }

    pub fn update(&mut self, info: Option<RpcTrackInfo>) {
        let now = Instant::now();
        let changed = match (&self.last_sent, &info) {
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
            (Some((prev, prev_at)), Some(curr)) => {
                let meta_changed = prev.title != curr.title
                    || prev.artist != curr.artist
                    || prev.playlist != curr.playlist
                    || prev.is_playing != curr.is_playing
                    || (prev.duration_sec - curr.duration_sec).abs() > 1.5;

                let seek_jumped = if curr.is_playing && prev.is_playing {
                    let expected_pos = prev.position_sec + prev_at.elapsed().as_secs_f32();
                    (curr.position_sec - expected_pos).abs() > 2.5
                } else {
                    false
                };

                meta_changed || seek_jumped
            }
        };

        if changed {
            self.last_sent = info.clone().map(|i| (i, now));
            let _ = self.tx.send(RpcMsg::UpdateTrack(info));
        }
    }

    pub fn status(&self) -> RpcStatus {
        RpcStatus::from_u8(self.status.load(Ordering::Relaxed))
    }
}

impl Drop for DiscordRpcManager {
    fn drop(&mut self) {
        let _ = self.tx.send(RpcMsg::Shutdown);
    }
}

fn set_atomic_status(status: &AtomicU8, val: RpcStatus) {
    status.store(val as u8, Ordering::Relaxed);
}

fn sanitize_rpc_text(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim();
    let s = if trimmed.is_empty() { fallback } else { trimmed };
    let mut out: String = s.chars().take(124).collect();
    if out.chars().count() < 2 {
        out.push(' ');
    }
    out
}

fn push_activity(
    client: &mut DiscordIpcClient,
    track: &RpcTrackInfo,
    received_at: Instant,
) -> Result<(), String> {
    let title = sanitize_rpc_text(&track.title, "Rustify");
    let state_fallback = if !track.playlist.trim().is_empty() {
        track.playlist.as_str()
    } else {
        "Rustify"
    };
    let artist = sanitize_rpc_text(&track.artist, state_fallback);

    let live_pos = (track.position_sec + received_at.elapsed().as_secs_f32()).max(0.0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let start_ms = now_ms.saturating_sub((live_pos * 1000.0).round() as i64);

    let mut timestamps = activity::Timestamps::new().start(start_ms);
    if track.duration_sec > 1.0 {
        let end_ms = start_ms.saturating_add((track.duration_sec * 1000.0).round() as i64);
        timestamps = timestamps.end(end_ms);
    }

    let large_text = if !track.playlist.trim().is_empty() {
        sanitize_rpc_text(&format!("Rustify · {}", track.playlist.trim()), "Rustify")
    } else {
        "Rustify".to_string()
    };

    let assets = activity::Assets::new()
        .large_image(RUSTIFY_ICON_URL)
        .large_text(&large_text);

    let buttons = vec![activity::Button::new("Get Rustify", RUSTIFY_DOWNLOAD_URL)];

    let payload = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .status_display_type(activity::StatusDisplayType::Name)
        .details(&title)
        .state(&artist)
        .timestamps(timestamps)
        .assets(assets)
        .buttons(buttons);

    client.set_activity(payload).map_err(|e| e.to_string())?;
    let (_op, resp) = client.recv().map_err(|e| e.to_string())?;
    if resp.get("evt").and_then(|v| v.as_str()) == Some("ERROR") {
        return Err(format!("Discord IPC error: {resp}"));
    }
    Ok(())
}

fn disconnect_client(client: &mut Option<DiscordIpcClient>, activity_active: &mut bool) {
    if let Some(mut c) = client.take() {
        if *activity_active {
            if c.clear_activity().is_ok() {
                let _ = c.recv();
            }
            *activity_active = false;
        }
        let _ = c.close();
    } else {
        *activity_active = false;
    }
}

fn sync_current_track(
    client: &mut Option<DiscordIpcClient>,
    activity_active: &mut bool,
    current_track: &Option<(RpcTrackInfo, Instant)>,
    status: &AtomicU8,
    next_retry: &mut Option<Instant>,
) {
    let Some(ref mut c) = client else {
        return;
    };

    match current_track {
        Some((track, received_at)) if track.is_playing => {
            if push_activity(c, track, *received_at).is_ok() {
                *activity_active = true;
            } else {
                *client = None;
                *activity_active = false;
                set_atomic_status(status, RpcStatus::WaitingForDiscord);
                *next_retry = Some(Instant::now() + RETRY_INTERVAL);
            }
        }
        _ => {
            if *activity_active {
                if c.clear_activity().is_ok() {
                    let _ = c.recv();
                    *activity_active = false;
                } else {
                    *client = None;
                    *activity_active = false;
                    set_atomic_status(status, RpcStatus::WaitingForDiscord);
                    *next_retry = Some(Instant::now() + RETRY_INTERVAL);
                }
            }
        }
    }
}

fn rpc_worker_loop(
    mut enabled: bool,
    mut client_id: String,
    rx: crossbeam_channel::Receiver<RpcMsg>,
    status: Arc<AtomicU8>,
) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut current_track: Option<(RpcTrackInfo, Instant)> = None;
    let mut activity_active = false;
    let mut next_retry: Option<Instant> = None;

    loop {
        if !enabled {
            disconnect_client(&mut client, &mut activity_active);
            next_retry = None;
            set_atomic_status(&status, RpcStatus::Disabled);
        } else if client.is_none() {
            set_atomic_status(&status, RpcStatus::WaitingForDiscord);
            let should_try = next_retry.is_none_or(|t| Instant::now() >= t);
            if should_try {
                let mut c = DiscordIpcClient::new(effective_client_id(&client_id));
                if c.connect().is_ok() {
                    client = Some(c);
                    next_retry = None;
                    set_atomic_status(&status, RpcStatus::Connected);
                    sync_current_track(
                        &mut client,
                        &mut activity_active,
                        &current_track,
                        &status,
                        &mut next_retry,
                    );
                } else {
                    next_retry = Some(Instant::now() + RETRY_INTERVAL);
                }
            }
        }

        let msg = if enabled && client.is_none() {
            let wait = next_retry
                .map(|t| t.saturating_duration_since(Instant::now()))
                .unwrap_or(RETRY_INTERVAL);
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };

        let Some(first_msg) = msg else {
            continue;
        };

        // Drain queued messages so we always process the newest state immediately
        let mut pending = vec![first_msg];
        while let Ok(m) = rx.try_recv() {
            pending.push(m);
        }

        let mut track_dirty = false;
        for m in pending {
            match m {
                RpcMsg::Shutdown => {
                    disconnect_client(&mut client, &mut activity_active);
                    return;
                }
                RpcMsg::SetConfig {
                    enabled: new_enabled,
                    client_id: new_id,
                } => {
                    let id_changed =
                        effective_client_id(&client_id) != effective_client_id(&new_id);
                    if !new_enabled || id_changed {
                        disconnect_client(&mut client, &mut activity_active);
                    }
                    enabled = new_enabled;
                    client_id = new_id;
                    next_retry = None;
                    track_dirty = true;
                }
                RpcMsg::UpdateTrack(info) => {
                    current_track = info.map(|i| (i, Instant::now()));
                    track_dirty = true;
                }
            }
        }

        if track_dirty && enabled && client.is_some() {
            sync_current_track(
                &mut client,
                &mut activity_active,
                &current_track,
                &status,
                &mut next_retry,
            );
        }
    }

    disconnect_client(&mut client, &mut activity_active);
}
