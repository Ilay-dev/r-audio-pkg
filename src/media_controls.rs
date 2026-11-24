use crossbeam_channel::{unbounded, Receiver};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::ffi::c_void;
use std::time::{Duration, Instant};

pub struct SystemMediaController {
    controls: Option<MediaControls>,
    event_rx: Receiver<MediaControlEvent>,
    last_track_key: String,
    last_playing: Option<bool>,
    last_pos_sync: Instant,
}

impl SystemMediaController {
    pub fn new(hwnd: Option<*mut c_void>) -> Self {
        let (tx, rx) = unbounded::<MediaControlEvent>();

        let config = PlatformConfig {
            dbus_name: "rustify",
            display_name: "Rustify",
            hwnd,
        };

        let controls = match MediaControls::new(config) {
            Ok(mut ctrl) => {
                if ctrl
                    .attach(move |event: MediaControlEvent| {
                        let _ = tx.send(event);
                    })
                    .is_ok()
                {
                    let _ = ctrl.set_playback(MediaPlayback::Stopped);
                    Some(ctrl)
                } else {
                    None
                }
            }
            Err(_) => None,
        };

        Self {
            controls,
            event_rx: rx,
            last_track_key: String::new(),
            last_playing: None,
            last_pos_sync: Instant::now(),
        }
    }

    pub fn poll_events(&self) -> Vec<MediaControlEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            out.push(ev);
        }
        out
    }

    pub fn update_state(
        &mut self,
        title: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        duration_sec: f32,
        position_sec: f32,
        is_playing: bool,
    ) {
        let Some(ref mut ctrl) = self.controls else {
            return;
        };

        let Some(title) = title else {
            if !self.last_track_key.is_empty() || self.last_playing.is_some() {
                self.last_track_key.clear();
                self.last_playing = None;
                let _ = ctrl.set_playback(MediaPlayback::Stopped);
            }
            return;
        };

        let artist_str = artist.unwrap_or("");
        let album_str = album.unwrap_or("Rustify");
        let dur_int = duration_sec.round() as u64;
        let track_key = format!("{title}|{artist_str}|{album_str}|{dur_int}");

        if track_key != self.last_track_key {
            self.last_track_key = track_key;
            let meta = MediaMetadata {
                title: Some(title),
                artist: if artist_str.is_empty() {
                    None
                } else {
                    Some(artist_str)
                },
                album: Some(album_str),
                duration: if duration_sec > 0.5 {
                    Some(Duration::from_secs_f32(duration_sec))
                } else {
                    None
                },
                cover_url: None,
            };
            let _ = ctrl.set_metadata(meta);
            self.last_playing = None; // force playback sync
        }

        let need_playback_sync = self.last_playing != Some(is_playing)
            || self.last_pos_sync.elapsed() >= Duration::from_secs(4);

        if need_playback_sync {
            self.last_playing = Some(is_playing);
            self.last_pos_sync = Instant::now();
            let progress = Some(MediaPosition(Duration::from_secs_f32(position_sec.max(0.0))));
            let pb = if is_playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            };
            let _ = ctrl.set_playback(pb);
        }
    }
}
