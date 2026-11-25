use crate::config::is_audio_extension;
use crate::covers::save_cover_bytes_for_track;
use crate::library::{import_audio_files, parse_title_artist, sanitize_filename_component};
use crate::youtube::{cleanup_partial_downloads, pick_ytdlp_error_line, resolve_ytdlp};
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_normalization::UnicodeNormalization;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const SYNC_META_FILENAME: &str = ".rustify-sync.json";
const PLAYLIST_MARKER_FILENAME: &str = ".rustify-playlist";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// Data Structures & Sync Metadata Persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistSourceKind {
    Spotify,
    AmazonMusic,
    YouTube,
}

impl PlaylistSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            PlaylistSourceKind::Spotify => "Spotify",
            PlaylistSourceKind::AmazonMusic => "Amazon Music",
            PlaylistSourceKind::YouTube => "YouTube",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTrack {
    pub title: String,
    pub artist: String,
    pub youtube_url: Option<String>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePlaylist {
    pub name: String,
    pub source_kind: PlaylistSourceKind,
    pub source_url: String,
    pub tracks: Vec<RemoteTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistSyncMeta {
    pub source_kind: PlaylistSourceKind,
    pub source_url: String,
    pub playlist_name: String,
}

pub fn read_sync_meta(playlist_dir: &Path) -> Option<PlaylistSyncMeta> {
    let meta_path = playlist_dir.join(SYNC_META_FILENAME);
    let content = fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_sync_meta(playlist_dir: &Path, meta: &PlaylistSyncMeta) -> Result<(), String> {
    let meta_path = playlist_dir.join(SYNC_META_FILENAME);
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize sync metadata: {e}"))?;
    fs::write(&meta_path, json).map_err(|e| format!("Failed to write sync metadata: {e}"))
}

#[allow(dead_code)]
pub fn has_sync_meta(playlist_dir: &Path) -> bool {
    playlist_dir.join(SYNC_META_FILENAME).is_file()
}

// ---------------------------------------------------------------------------
// Process Helpers
// ---------------------------------------------------------------------------

fn hidden_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    cmd.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn hidden_command_name(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn curl_bin() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "curl.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "curl"
    }
}

fn curl_get_bytes(url: &str, timeout_secs: u32, user_agent: &str) -> Result<Vec<u8>, String> {
    let safe_url = crate::config::validate_http_url(url)?;
    let timeout_str = timeout_secs.to_string();
    let bin = curl_bin();
    let output = hidden_command_name(bin)
        .args(["-s", "-L", "--max-time", &timeout_str, "-A", user_agent, "--", safe_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run {bin}: {e}"))?;

    if !output.status.success() || output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().unwrap_or("Empty response").trim();
        return Err(format!("HTTP request failed: {detail}"));
    }
    Ok(output.stdout)
}

fn curl_get_text(url: &str, timeout_secs: u32, user_agent: &str) -> Result<String, String> {
    let bytes = curl_get_bytes(url, timeout_secs, user_agent)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn curl_resolve_redirect(url: &str) -> Result<String, String> {
    let safe_url = crate::config::validate_http_url(url)?;
    let bin = curl_bin();
    let null_sink = if cfg!(target_os = "windows") {
        "NUL"
    } else {
        "/dev/null"
    };
    let output = hidden_command_name(bin)
        .args([
            "-s",
            "-L",
            "--max-time",
            "15",
            "-A",
            "Mozilla/5.0",
            "-o",
            null_sink,
            "-w",
            "%{url_effective}",
            "--",
            safe_url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run {bin}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().unwrap_or("Redirect failed").trim();
        return Err(format!("HTTP redirect resolution failed: {detail}"));
    }
    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if resolved.is_empty() {
        return Err("HTTP redirect resolved to empty URL".to_string());
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Source Detection & Public Playlist Extraction
// ---------------------------------------------------------------------------

fn is_known_playlist_authority(authority_and_rest: &str) -> bool {
    let auth_end = authority_and_rest
        .find(['/', '?', '#'])
        .unwrap_or(authority_and_rest.len());
    let authority = &authority_and_rest[..auth_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host_lower = authority.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        host_lower.as_str(),
        "open.spotify.com"
            | "spotify.link"
            | "spoti.fi"
            | "youtube.com"
            | "www.youtube.com"
            | "m.youtube.com"
            | "music.youtube.com"
            | "youtu.be"
    ) || is_valid_amazon_music_host(&host_lower)
}

pub fn normalize_playlist_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("http://") {
        let rest = &trimmed[7..];
        if is_known_playlist_authority(rest) {
            return format!("https://{rest}");
        }
        return trimmed.to_string();
    }
    if !trimmed.contains("://") && is_known_playlist_authority(trimmed) {
        return format!("https://{trimmed}");
    }
    trimmed.to_string()
}

fn extract_https_host(url: &str) -> Option<(&str, &str)> {
    let trimmed = crate::config::validate_http_url(url).ok()?;
    if trimmed.len() < 8 || !trimmed[..8].eq_ignore_ascii_case("https://") {
        return None;
    }
    let after_scheme = &trimmed[8..];
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        return None;
    }
    Some((host, &after_scheme[authority_end..]))
}

fn is_valid_amazon_music_host(host_lower: &str) -> bool {
    if host_lower == "music.amazon.com" || host_lower == "music.amazon.de" {
        return true;
    }
    if let Some(suffix) = host_lower.strip_prefix("music.amazon.") {
        return !suffix.is_empty()
            && !suffix.starts_with('.')
            && !suffix.ends_with('.')
            && !suffix.contains("..")
            && suffix.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
    }
    false
}

pub fn detect_source_kind(url: &str) -> Option<PlaylistSourceKind> {
    let normalized = normalize_playlist_url(url);
    let (host, rest) = extract_https_host(&normalized)?;
    let host_lower = host.to_ascii_lowercase();
    let rest_lower = rest.to_ascii_lowercase();

    if (host_lower == "open.spotify.com" && rest_lower.contains("/playlist/"))
        || ((host_lower == "spotify.link" || host_lower == "spoti.fi") && rest.len() > 1)
    {
        return Some(PlaylistSourceKind::Spotify);
    }

    if is_valid_amazon_music_host(&host_lower) {
        return Some(PlaylistSourceKind::AmazonMusic);
    }

    if (matches!(
        host_lower.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    ) && (rest_lower.contains("/playlist") || rest_lower.contains("list=")))
        || (host_lower == "youtu.be" && rest_lower.contains("list="))
    {
        return Some(PlaylistSourceKind::YouTube);
    }

    None
}

pub fn fetch_remote_playlist(url: &str, ytdlp: Option<&Path>) -> Result<RemotePlaylist, String> {
    let normalized = normalize_playlist_url(url);
    if normalized.is_empty() {
        return Err("Bitte gib eine gültige Playlist-URL ein.".to_string());
    }

    let kind = detect_source_kind(&normalized).ok_or_else(|| {
        "Nicht unterstützter Playlist-Link. Unterstützt werden Spotify-, Amazon-Music- und YouTube-Playlists."
            .to_string()
    })?;

    match kind {
        PlaylistSourceKind::Spotify => fetch_spotify_playlist(&normalized),
        PlaylistSourceKind::YouTube => fetch_youtube_playlist(&normalized, ytdlp),
        PlaylistSourceKind::AmazonMusic => fetch_amazon_music_playlist(&normalized),
    }
}

// ---------------------------------------------------------------------------
// A) Public Spotify Playlist Extractor
// ---------------------------------------------------------------------------

fn extract_spotify_playlist_id(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let idx = lower.find("/playlist/")?;
    let rest = &url[idx + "/playlist/".len()..];
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn extract_next_data_json(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let marker = lower
        .find("id=\"__next_data__\"")
        .or_else(|| lower.find("id='__next_data__'"))?;
    let open_end = lower[marker..].find('>')? + marker + 1;
    let close_start = lower[open_end..].find("</script>")? + open_end;
    Some(html[open_end..close_start].trim())
}

fn clean_spotify_text(raw: &str) -> String {
    raw.replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn pick_best_image_url(arr: &[serde_json::Value]) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    for item in arr {
        let Some(u) = item.get("url").and_then(|v| v.as_str()) else {
            continue;
        };
        if !u.starts_with("http") {
            continue;
        }
        let width = item
            .get("maxWidth")
            .or_else(|| item.get("width"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if best.as_ref().is_none_or(|(bw, _)| width > *bw) {
            best = Some((width, u.to_string()));
        }
    }
    best.map(|(_, u)| u)
}

fn extract_spotify_image_url(val: &serde_json::Value) -> Option<String> {
    if let Some(arr) = val
        .get("visualIdentity")
        .and_then(|v| v.get("image"))
        .and_then(|v| v.as_array())
    {
        if let Some(u) = pick_best_image_url(arr) {
            return Some(u);
        }
    }
    if let Some(arr) = val
        .get("coverArt")
        .and_then(|v| v.get("sources"))
        .and_then(|v| v.as_array())
    {
        if let Some(u) = pick_best_image_url(arr) {
            return Some(u);
        }
    }
    if let Some(u) = val
        .get("image")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
    {
        if u.starts_with("http") {
            return Some(u.to_string());
        }
    }
    None
}

fn fetch_spotify_playlist(url: &str) -> Result<RemotePlaylist, String> {
    let playlist_id = if let Some(id) = extract_spotify_playlist_id(url) {
        id
    } else {
        let resolved = curl_resolve_redirect(url).map_err(|_| {
            "Ungültige Spotify-Playlist-URL. Erwartet wird z. B. https://open.spotify.com/playlist/..."
                .to_string()
        })?;
        let resolved_host = extract_https_host(&resolved)
            .map(|(h, _)| h.to_ascii_lowercase())
            .unwrap_or_default();
        if resolved_host != "open.spotify.com" {
            return Err(
                "Kurzlink konnte nicht zu einer Spotify-Playlist aufgelöst werden."
                    .to_string(),
            );
        }
        extract_spotify_playlist_id(&resolved).ok_or_else(|| {
            "Keine Spotify-Playlist im Link gefunden. Stelle sicher, dass es ein Playlist-Link ist."
                .to_string()
        })?
    };

    let canonical_url = format!("https://open.spotify.com/playlist/{playlist_id}");
    let embed_url = format!("https://open.spotify.com/embed/playlist/{playlist_id}");
    let html = curl_get_text(&embed_url, 15, "Mozilla/5.0")
        .map_err(|e| format!("Spotify-Playlist konnte nicht abgerufen werden: {e}"))?;

    let json_str = extract_next_data_json(&html).ok_or_else(|| {
        "Spotify-Playlist-Daten wurden nicht gefunden. Stelle sicher, dass die Playlist auf Spotify öffentlich (Public) ist."
            .to_string()
    })?;

    let root: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| format!("Spotify-Antwort konnte nicht gelesen werden: {e}"))?;

    let entity = root
        .get("props")
        .and_then(|v| v.get("pageProps"))
        .and_then(|v| v.get("state"))
        .and_then(|v| v.get("data"))
        .and_then(|v| v.get("entity"))
        .ok_or_else(|| {
            "Spotify-Playlist konnte nicht ausgelesen werden. Stelle sicher, dass die Playlist öffentlich (Public) ist."
                .to_string()
        })?;

    let name = entity
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| entity.get("title").and_then(|v| v.as_str()))
        .map(clean_spotify_text)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Spotify Playlist".to_string());

    let playlist_cover = extract_spotify_image_url(entity);

    let track_list = entity
        .get("trackList")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            "Diese Spotify-Playlist enthält keine sichtbaren Titel. Bitte stelle die Playlist auf Spotify auf 'Öffentlich' (Public)."
                .to_string()
        })?;

    let mut tracks = Vec::with_capacity(track_list.len());
    for item in track_list {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(clean_spotify_text)
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        let artist = item
            .get("subtitle")
            .and_then(|v| v.as_str())
            .map(clean_spotify_text)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                item.get("artists").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                        .map(clean_spotify_text)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
            })
            .unwrap_or_default();

        let cover_url = extract_spotify_image_url(item).or_else(|| playlist_cover.clone());

        tracks.push(RemoteTrack {
            title,
            artist,
            youtube_url: None,
            cover_url,
        });
    }

    let tracks = deduplicate_remote_tracks(tracks);
    if tracks.is_empty() {
        return Err(
            "Keine Titel in der Spotify-Playlist gefunden. Bitte stelle sicher, dass die Playlist auf Spotify auf 'Öffentlich' (Public) gestellt ist."
                .to_string(),
        );
    }

    Ok(RemotePlaylist {
        name,
        source_kind: PlaylistSourceKind::Spotify,
        source_url: canonical_url,
        tracks,
    })
}

// ---------------------------------------------------------------------------
// B) Public YouTube / YouTube Music Playlist Extractor
// ---------------------------------------------------------------------------

fn remove_ascii_case_insensitive(s: &mut String, pat: &str) {
    let lower_pat = pat.to_ascii_lowercase();
    if lower_pat.is_empty() {
        return;
    }
    loop {
        let lower_s = s.to_ascii_lowercase();
        let Some(idx) = lower_s.find(&lower_pat) else {
            break;
        };
        let end = idx + lower_pat.len();
        if s.is_char_boundary(idx) && s.is_char_boundary(end) {
            s.replace_range(idx..end, "");
        } else {
            break;
        }
    }
}

fn strip_youtube_noise(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    const PATTERNS: &[&str] = &[
        "(Official Music Video)",
        "[Official Music Video]",
        "(Official Video)",
        "[Official Video]",
        "(Official Audio)",
        "[Official Audio]",
        "(Official Lyric Video)",
        "[Official Lyric Video]",
        "(Lyric Video)",
        "[Lyric Video]",
        "(Lyrics)",
        "[Lyrics]",
        "(Audio)",
        "[Audio]",
        "(HD)",
        "[HD]",
        "(HQ)",
        "[HQ]",
        "(4K)",
        "[4K]",
        "(4K Remaster)",
        "(Official Visualizer)",
        "[Official Visualizer]",
        "(Visualizer)",
        "(Music Video)",
        "[Music Video]",
    ];

    for pat in PATTERNS {
        remove_ascii_case_insensitive(&mut s, pat);
    }

    if let Some((left, _)) = s.split_once(" | ") {
        if left.trim().len() >= 3 {
            s = left.trim().to_string();
        }
    }

    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_uploader(raw: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_suffix(" - Topic")
        .or_else(|| trimmed.strip_suffix(" - topic"))
        .or_else(|| trimmed.strip_suffix("VEVO"))
        .unwrap_or(trimmed);
    stripped.trim().to_string()
}

fn split_youtube_title_artist(raw_title: &str, raw_channel: &str) -> (String, String) {
    let cleaned = strip_youtube_noise(raw_title);
    for sep in [" - ", " – ", " — "] {
        if let Some((left, right)) = cleaned.split_once(sep) {
            let a = left.trim();
            let t = right.trim();
            if !t.is_empty() && !a.is_empty() {
                return (t.to_string(), a.to_string());
            }
        }
    }

    let title = if cleaned.is_empty() {
        raw_title.trim().to_string()
    } else {
        cleaned
    };
    let artist = clean_uploader(raw_channel);
    (title, artist)
}

fn extract_youtube_list_id(url: &str) -> Option<String> {
    let (_, query_and_frag) = url.split_once('?')?;
    let query = query_and_frag.split('#').next().unwrap_or(query_and_frag);
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k.eq_ignore_ascii_case("list") {
                let id: String = v
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                if !id.is_empty() {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn fetch_youtube_playlist(url: &str, ytdlp: Option<&Path>) -> Result<RemotePlaylist, String> {
    let resolved_owned;
    let ytdlp_path = match ytdlp {
        Some(p) => p,
        None => {
            resolved_owned = resolve_ytdlp(None).ok_or_else(|| {
                "yt-dlp wurde nicht gefunden. Bitte konfiguriere yt-dlp in den Einstellungen."
                    .to_string()
            })?;
            resolved_owned.as_path()
        }
    };

    let fetch_url = if !url.to_ascii_lowercase().contains("/playlist") {
        if let Some(list_id) = extract_youtube_list_id(url) {
            format!("https://www.youtube.com/playlist?list={list_id}")
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    };

    let safe_url = crate::config::validate_http_url(&fetch_url)?;
    let output = hidden_command(ytdlp_path)
        .args(["--yes-playlist", "--flat-playlist", "-J", "--no-warnings", "--", safe_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("yt-dlp konnte nicht gestartet werden: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("YouTube-Playlist konnte nicht abgerufen werden")
            .trim();
        return Err(msg.to_string());
    }

    let root: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Ungültige yt-dlp-Antwort: {e}"))?;

    let name = root
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("YouTube Playlist")
        .to_string();

    let entries = root
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            "Keine Einträge in der YouTube-Playlist gefunden. Stelle sicher, dass die Playlist öffentlich oder nicht gelistet ist."
                .to_string()
        })?;

    let mut tracks = Vec::with_capacity(entries.len());
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if id.is_empty() {
            continue;
        }

        let raw_title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("");
        if raw_title.is_empty()
            || raw_title.eq_ignore_ascii_case("[Private video]")
            || raw_title.eq_ignore_ascii_case("[Deleted video]")
        {
            continue;
        }

        let youtube_url = entry
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| s.starts_with("http"))
            .map(str::to_string)
            .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));

        let raw_channel = entry
            .get("channel")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                entry
                    .get("uploader")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                entry
                    .get("artist")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                entry
                    .get("creator")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or("");

        let (title, artist) = if let (Some(track_meta), Some(artist_meta)) = (
            entry
                .get("track")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "NA"),
            entry
                .get("artist")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "NA"),
        ) {
            (strip_youtube_noise(track_meta), artist_meta.to_string())
        } else {
            split_youtube_title_artist(raw_title, raw_channel)
        };

        let cover_url = Some(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"));

        tracks.push(RemoteTrack {
            title,
            artist,
            youtube_url: Some(youtube_url),
            cover_url,
        });
    }

    let tracks = deduplicate_remote_tracks(tracks);
    if tracks.is_empty() {
        return Err(
            "Keine verfügbaren Titel in der YouTube-Playlist gefunden. Stelle sicher, dass die Playlist nicht privat ist."
                .to_string(),
        );
    }

    Ok(RemotePlaylist {
        name,
        source_kind: PlaylistSourceKind::YouTube,
        source_url: url.to_string(),
        tracks,
    })
}

// ---------------------------------------------------------------------------
// C) Public Amazon Music Playlist Extractor (Headless Edge/Chrome + Fallback)
// ---------------------------------------------------------------------------

fn find_chromium_browser() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut candidates: Vec<PathBuf> = vec![
            PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
            PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
            PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
        ];

        if let Some(local) = dirs::data_local_dir() {
            candidates.push(local.join(r"Google\Chrome\Application\chrome.exe"));
            candidates.push(local.join(r"Microsoft\Edge\Application\msedge.exe"));
        }

        candidates.into_iter().find(|p| p.is_file())
    }

    #[cfg(not(target_os = "windows"))]
    {
        const NAMES: &[&str] = &[
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "brave",
            "brave-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "vivaldi",
            "vivaldi-stable",
            "opera",
            "opera-stable",
        ];

        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                for name in NAMES {
                    let cand = dir.join(name);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }

        for dir in ["/usr/bin", "/usr/local/bin"] {
            for name in NAMES {
                let cand = Path::new(dir).join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }

        None
    }
}

fn dump_dom_with_chromium(browser: &Path, url: &str) -> Result<String, String> {
    let safe_url = crate::config::validate_http_url(url)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let profile_dir = std::env::temp_dir().join(format!("rustify-dom-{}-{nanos}", std::process::id()));
    let _ = fs::create_dir_all(&profile_dir);
    let user_data_arg = format!("--user-data-dir={}", profile_dir.to_string_lossy());

    let output = hidden_command(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            &user_data_arg,
            "--virtual-time-budget=18000",
            "--dump-dom",
            "--",
            safe_url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to start browser for DOM dump: {e}"));

    let _ = fs::remove_dir_all(&profile_dir);

    let output = output?;
    if output.stdout.is_empty() {
        return Err("Headless browser returned empty DOM".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn unescape_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'&' {
            if let Some(rel_semi) = input[i..].find(';') {
                if rel_semi <= 12 {
                    let entity = &input[i + 1..i + rel_semi];
                    let decoded = match entity {
                        "amp" => Some("&".to_string()),
                        "quot" => Some("\"".to_string()),
                        "apos" => Some("'".to_string()),
                        "lt" => Some("<".to_string()),
                        "gt" => Some(">".to_string()),
                        "nbsp" => Some(" ".to_string()),
                        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                            u32::from_str_radix(&entity[2..], 16)
                                .ok()
                                .and_then(char::from_u32)
                                .map(|c| c.to_string())
                        }
                        _ if entity.starts_with('#') => entity[1..]
                            .parse::<u32>()
                            .ok()
                            .and_then(char::from_u32)
                            .map(|c| c.to_string()),
                        _ => None,
                    };
                    if let Some(rep) = decoded {
                        out.push_str(&rep);
                        i += rel_semi + 1;
                        continue;
                    }
                }
            }
        }

        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    out.replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

struct ParsedTag {
    name: String,
    attrs: Vec<(String, String)>,
}

impl ParsedTag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

fn parse_html_attributes(raw: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        let name_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'>'
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let name = raw[name_start..i].to_ascii_lowercase();

        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let val = if bytes[i] == b'"' || bytes[i] == b'\'' {
                let quote = bytes[i];
                i += 1;
                let val_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                let slice = &raw[val_start..i];
                if i < bytes.len() && bytes[i] == quote {
                    i += 1;
                }
                unescape_html_entities(slice)
            } else {
                let val_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                unescape_html_entities(&raw[val_start..i])
            };
            attrs.push((name, val));
        } else {
            attrs.push((name, String::new()));
        }
    }

    attrs
}

fn scan_opening_tags(html: &str) -> Vec<ParsedTag> {
    let mut tags = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Skip comments, doctype, closing tags
        if i + 1 >= bytes.len() || matches!(bytes[i + 1], b'/' | b'!' | b'?') {
            i += 1;
            continue;
        }

        let name_start = i + 1;
        let mut j = name_start;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
        {
            j += 1;
        }
        if j == name_start {
            i += 1;
            continue;
        }
        let name = html[name_start..j].to_ascii_lowercase();

        // Advance to closing '>' while respecting quoted attribute values
        let attr_start = j;
        let mut in_quote: Option<u8> = None;
        while j < bytes.len() {
            let b = bytes[j];
            if let Some(q) = in_quote {
                if b == q {
                    in_quote = None;
                }
            } else if b == b'"' || b == b'\'' {
                in_quote = Some(b);
            } else if b == b'>' {
                break;
            }
            j += 1;
        }

        if j < bytes.len() && bytes[j] == b'>' {
            let interested = matches!(
                name.as_str(),
                "music-detail-header" | "music-image-row" | "music-text-row" | "meta"
            ) || html[attr_start..j]
                .to_ascii_lowercase()
                .contains("primary-text");

            if interested {
                let attrs = parse_html_attributes(&html[attr_start..j]);
                tags.push(ParsedTag { name, attrs });
            }
            i = j + 1;
        } else {
            break;
        }
    }

    tags
}

fn strip_html_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    unescape_html_entities(&out)
}

fn extract_tag_inner_text(html: &str, tag_name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open_pat = format!("<{tag_name}");
    let close_pat = format!("</{tag_name}>");
    let mut search_from = 0usize;

    while let Some(rel) = lower[search_from..].find(&open_pat) {
        let start = search_from + rel;
        let after_name = start + open_pat.len();
        if let Some(next_b) = lower.as_bytes().get(after_name) {
            if !next_b.is_ascii_whitespace() && *next_b != b'>' {
                search_from = after_name;
                continue;
            }
        }
        let open_end = lower[after_name..].find('>')? + after_name + 1;
        let close_start = lower[open_end..].find(&close_pat)? + open_end;
        let text = strip_html_tags(&html[open_end..close_start]);
        if !text.is_empty() {
            return Some(text);
        }
        search_from = close_start + close_pat.len();
    }
    None
}

fn clean_amazon_title(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    const SUFFIXES: &[&str] = &[
        " on Amazon Music",
        " bei Amazon Music",
        " - Amazon Music",
        " | Amazon Music",
        " – Amazon Music",
        " — Amazon Music",
        " on Amazon Music Unlimited",
        " bei Amazon Music Unlimited",
    ];
    for suf in SUFFIXES {
        if s.len() > suf.len()
            && s.as_bytes()[s.len() - suf.len()..].eq_ignore_ascii_case(suf.as_bytes())
        {
            s.truncate(s.len() - suf.len());
            s = s.trim().to_string();
        }
    }
    s
}

fn extract_json_ld_blocks(html: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut pos = 0usize;

    while let Some(rel) = lower[pos..].find("<script") {
        let script_start = pos + rel;
        let Some(open_end_rel) = lower[script_start..].find('>') else {
            break;
        };
        let open_end = script_start + open_end_rel + 1;
        let header = &lower[script_start..open_end];
        let Some(close_rel) = lower[open_end..].find("</script>") else {
            break;
        };
        let close_start = open_end + close_rel;
        if header.contains("application/ld+json") {
            let body = html[open_end..close_start].trim();
            if !body.is_empty() {
                blocks.push(body);
            }
        }
        pos = close_start + "</script>".len();
    }
    blocks
}

fn extract_json_ld_artist(val: &serde_json::Value) -> String {
    if let Some(s) = val.as_str() {
        return unescape_html_entities(s);
    }
    if let Some(obj) = val.as_object() {
        if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
            return unescape_html_entities(name);
        }
    }
    if let Some(arr) = val.as_array() {
        let names: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    Some(unescape_html_entities(s))
                } else {
                    item.get("name")
                        .and_then(|n| n.as_str())
                        .map(unescape_html_entities)
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        return names.join(", ");
    }
    String::new()
}

fn extract_json_ld_image(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        if s.starts_with("http") {
            return Some(s.to_string());
        }
    }
    if let Some(u) = val.get("url").and_then(|v| v.as_str()) {
        if u.starts_with("http") {
            return Some(u.to_string());
        }
    }
    if let Some(arr) = val.as_array() {
        for item in arr {
            if let Some(u) = extract_json_ld_image(item) {
                return Some(u);
            }
        }
    }
    None
}

fn collect_tracks_from_json_ld_node(
    node: &serde_json::Value,
    playlist_name: &mut Option<String>,
    out: &mut Vec<RemoteTrack>,
) {
    if let Some(arr) = node.as_array() {
        for item in arr {
            collect_tracks_from_json_ld_node(item, playlist_name, out);
        }
        return;
    }

    if let Some(graph) = node.get("@graph") {
        collect_tracks_from_json_ld_node(graph, playlist_name, out);
    }

    let is_playlist = node
        .get("@type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.eq_ignore_ascii_case("MusicPlaylist"));

    if is_playlist && playlist_name.is_none() {
        if let Some(n) = node.get("name").and_then(|v| v.as_str()) {
            let cleaned = clean_amazon_title(&unescape_html_entities(n));
            if !cleaned.is_empty() {
                *playlist_name = Some(cleaned);
            }
        }
    }

    let track_items: Option<&Vec<serde_json::Value>> = node
        .get("track")
        .and_then(|t| {
            t.as_array()
                .or_else(|| t.get("itemListElement").and_then(|v| v.as_array()))
        })
        .or_else(|| node.get("itemListElement").and_then(|v| v.as_array()));

    if let Some(items) = track_items {
        for entry in items {
            let rec = entry.get("item").unwrap_or(entry);
            let title = rec
                .get("name")
                .and_then(|v| v.as_str())
                .map(unescape_html_entities)
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let artist = rec
                .get("byArtist")
                .or_else(|| rec.get("author"))
                .map(extract_json_ld_artist)
                .unwrap_or_default();
            let cover_url = rec.get("image").and_then(extract_json_ld_image);

            out.push(RemoteTrack {
                title,
                artist,
                youtube_url: None,
                cover_url,
            });
        }
    }
}

fn parse_amazon_music_html(html: &str) -> (String, Vec<RemoteTrack>) {
    let tags = scan_opening_tags(html);
    let mut playlist_name: Option<String> = None;
    let mut tracks: Vec<RemoteTrack> = Vec::new();

    // 1. Extract playlist title from <music-detail-header headline="...">
    for tag in &tags {
        if tag.name == "music-detail-header" {
            if let Some(headline) = tag
                .attr("headline")
                .or_else(|| tag.attr("primary-text"))
                .map(clean_amazon_title)
                .filter(|s| !s.is_empty())
            {
                playlist_name = Some(headline);
                break;
            }
        }
    }

    // 2. Extract tracks from <music-image-row> and <music-text-row>
    for tag in &tags {
        if tag.name == "music-image-row" || tag.name == "music-text-row" {
            let title = tag.attr("primary-text").unwrap_or("").trim().to_string();
            if title.is_empty() {
                continue;
            }
            let artist = tag
                .attr("secondary-text-1")
                .or_else(|| tag.attr("secondary-text"))
                .or_else(|| tag.attr("secondary-text-2"))
                .unwrap_or("")
                .trim()
                .to_string();
            let cover_url = tag
                .attr("image-src")
                .map(str::trim)
                .filter(|s| s.starts_with("http"))
                .map(str::to_string);

            tracks.push(RemoteTrack {
                title,
                artist,
                youtube_url: None,
                cover_url,
            });
        }
    }

    // 3. Extract from JSON-LD <script type="application/ld+json">
    for block in extract_json_ld_blocks(html) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(block) {
            collect_tracks_from_json_ld_node(&val, &mut playlist_name, &mut tracks);
        }
    }

    // 4. Fallback: generic elements with primary-text="..." and secondary-text*="..."
    if tracks.is_empty() {
        for tag in &tags {
            if tag.name == "music-detail-header" || tag.name == "meta" {
                continue;
            }
            let Some(title) = tag.attr("primary-text").map(str::trim).filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(artist) = tag
                .attr("secondary-text-1")
                .or_else(|| tag.attr("secondary-text"))
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let cover_url = tag
                .attr("image-src")
                .map(str::trim)
                .filter(|s| s.starts_with("http"))
                .map(str::to_string);

            tracks.push(RemoteTrack {
                title: title.to_string(),
                artist: artist.to_string(),
                youtube_url: None,
                cover_url,
            });
        }
    }

    // 5. Fallback title sources: <h1>, <meta property="og:title">, <title>
    if playlist_name.is_none() {
        if let Some(h1) = extract_tag_inner_text(html, "h1") {
            let cleaned = clean_amazon_title(&h1);
            if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case("Amazon Music") {
                playlist_name = Some(cleaned);
            }
        }
    }
    if playlist_name.is_none() {
        for tag in &tags {
            if tag.name == "meta" {
                let is_og_title = tag
                    .attr("property")
                    .or_else(|| tag.attr("name"))
                    .is_some_and(|p| p.eq_ignore_ascii_case("og:title"));
                if is_og_title {
                    if let Some(content) = tag.attr("content") {
                        let cleaned = clean_amazon_title(content);
                        if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case("Amazon Music") {
                            playlist_name = Some(cleaned);
                            break;
                        }
                    }
                }
            }
        }
    }
    if playlist_name.is_none() {
        if let Some(t) = extract_tag_inner_text(html, "title") {
            let cleaned = clean_amazon_title(&t);
            if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case("Amazon Music") {
                playlist_name = Some(cleaned);
            }
        }
    }

    let final_name = playlist_name.unwrap_or_else(|| "Amazon Music Playlist".to_string());
    (final_name, deduplicate_remote_tracks(tracks))
}

fn fetch_amazon_music_playlist(url: &str) -> Result<RemotePlaylist, String> {
    let mut best_name = "Amazon Music Playlist".to_string();
    let mut best_tracks: Vec<RemoteTrack> = Vec::new();

    // Stage 1: Headless Chromium/Chrome/Edge DOM dump (renders Amazon Music's JS web components)
    let browser = find_chromium_browser();
    let found_browser = browser.is_some();
    if let Some(browser) = browser {
        if let Ok(dom_html) = dump_dom_with_chromium(&browser, url) {
            let (name, tracks) = parse_amazon_music_html(&dom_html);
            best_name = name;
            best_tracks = tracks;
        }
    }

    // Stage 2: Fallback to curl if no Chromium browser was found or 0 tracks extracted
    if best_tracks.is_empty() {
        if let Ok(raw_html) = curl_get_text(url, 15, BROWSER_USER_AGENT) {
            let (name, tracks) = parse_amazon_music_html(&raw_html);
            if !tracks.is_empty() || best_name == "Amazon Music Playlist" {
                best_name = name;
            }
            if !tracks.is_empty() {
                best_tracks = tracks;
            }
        }
    }

    if best_tracks.is_empty() {
        if !found_browser {
            return Err(
                "Für Amazon-Music-Playlists wird ein Chromium-Browser (z. B. Chromium oder Chrome) benötigt. Auf Arch Linux: sudo pacman -S chromium"
                    .to_string(),
            );
        }
        return Err(
            "Keine Titel in der Amazon-Music-Playlist gefunden. Bitte stelle sicher, dass die Playlist auf Amazon Music öffentlich geteilt ('Öffentlich geteilt' / 'Publicly shared') ist."
                .to_string(),
        );
    }

    Ok(RemotePlaylist {
        name: best_name,
        source_kind: PlaylistSourceKind::AmazonMusic,
        source_url: url.to_string(),
        tracks: best_tracks,
    })
}

fn deduplicate_remote_tracks(tracks: Vec<RemoteTrack>) -> Vec<RemoteTrack> {
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(tracks.len());
    for track in tracks {
        let key = format!(
            "{}|{}",
            normalize_match_key(&track.title),
            normalize_match_key(&track.artist)
        );
        if key == "|" || !seen.insert(key) {
            continue;
        }
        result.push(track);
    }
    result
}

// ---------------------------------------------------------------------------
// PC Folder Import Helper
// ---------------------------------------------------------------------------

pub fn import_pc_folder_as_playlist(
    music_dir: &Path,
    source_folder: &Path,
) -> Result<(String, usize), String> {
    if !source_folder.is_dir() {
        return Err("Der ausgewählte Quellordner existiert nicht.".to_string());
    }

    let raw_name = source_folder
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Playlist");
    let base_name = sanitize_filename_component(raw_name);
    let base_name = if base_name.is_empty() {
        "Imported Playlist".to_string()
    } else {
        base_name
    };

    // Collect supported audio files non-recursively
    let mut audio_files = Vec::new();
    let entries = fs::read_dir(source_folder)
        .map_err(|e| format!("Quellordner konnte nicht gelesen werden: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if is_audio_extension(ext) {
                audio_files.push(p);
            }
        }
    }

    // Check if source_folder is already directly inside music_dir
    let canon_music = fs::canonicalize(music_dir).ok();
    let canon_src = fs::canonicalize(source_folder).ok();
    let already_in_music_dir = match (&canon_music, &canon_src) {
        (Some(cm), Some(cs)) => cs.parent() == Some(cm.as_path()),
        _ => source_folder.parent() == Some(music_dir),
    };

    if already_in_music_dir {
        let _ = fs::write(source_folder.join(PLAYLIST_MARKER_FILENAME), b"rustify");
        return Ok((base_name, audio_files.len()));
    }

    if audio_files.is_empty() {
        return Err(
            "Der ausgewählte Ordner enthält keine unterstützten Audiodateien (.mp3, .wav, .flac, .ogg, .m4a, .aac)."
                .to_string(),
        );
    }

    let (playlist_name, target_dir) = allocate_unique_playlist_dir(music_dir, &base_name);
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Playlist-Ordner konnte nicht erstellt werden: {e}"))?;
    let _ = fs::write(target_dir.join(PLAYLIST_MARKER_FILENAME), b"rustify");

    let imported_count = import_audio_files(&target_dir, &audio_files);
    Ok((playlist_name, imported_count))
}

fn allocate_unique_playlist_dir(music_dir: &Path, base_name: &str) -> (String, PathBuf) {
    let mut name = base_name.to_string();
    let mut dir = music_dir.join(&name);
    let mut n = 2u32;
    while dir.exists() {
        name = format!("{base_name} ({n})");
        dir = music_dir.join(&name);
        n += 1;
    }
    (name, dir)
}

// ---------------------------------------------------------------------------
// Track Comparison & Single-Track Downloader
// ---------------------------------------------------------------------------

fn normalize_match_key(raw: &str) -> String {
    let nfc: String = raw.nfc().collect();
    let mut cleaned = String::with_capacity(nfc.len());
    for ch in nfc.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                cleaned.push(lc);
            }
        } else {
            cleaned.push(' ');
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct LocalTrackIndex {
    full_stems: HashSet<String>,
    title_artist_pairs: Vec<(String, String)>,
}

impl LocalTrackIndex {
    fn from_playlist_dir(playlist_dir: &Path) -> Self {
        let mut idx = Self {
            full_stems: HashSet::new(),
            title_artist_pairs: Vec::new(),
        };

        let Ok(entries) = fs::read_dir(playlist_dir) else {
            return idx;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !is_audio_extension(ext) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            idx.insert_stem(stem);
        }

        idx
    }

    fn insert_stem(&mut self, stem: &str) {
        let full = normalize_match_key(stem);
        if !full.is_empty() {
            self.full_stems.insert(full);
        }
        let (title, artist) = parse_title_artist(stem);
        let norm_t = normalize_match_key(&title);
        let norm_a = normalize_match_key(&artist);
        if !norm_t.is_empty() {
            if !norm_a.is_empty() {
                self.full_stems.insert(format!("{norm_t} {norm_a}"));
                self.full_stems.insert(format!("{norm_a} {norm_t}"));
            }
            self.title_artist_pairs.push((norm_t, norm_a));
        }
    }

    fn contains_remote(&self, track: &RemoteTrack) -> bool {
        let norm_t = normalize_match_key(&track.title);
        let norm_a = normalize_match_key(&track.artist);
        if norm_t.is_empty() {
            return true;
        }

        if !norm_a.is_empty() {
            let ta = format!("{norm_t} {norm_a}");
            let at = format!("{norm_a} {norm_t}");
            if self.full_stems.contains(&ta) || self.full_stems.contains(&at) {
                return true;
            }
            for (lt, la) in &self.title_artist_pairs {
                if lt == &norm_t
                    && (la.is_empty()
                        || la == &norm_a
                        || la.contains(&norm_a)
                        || norm_a.contains(la))
                {
                    return true;
                }
            }
            false
        } else {
            if self.full_stems.contains(&norm_t) {
                return true;
            }
            self.title_artist_pairs.iter().any(|(lt, _)| lt == &norm_t)
        }
    }
}

fn download_remote_track(
    ytdlp: &Path,
    ffmpeg: Option<&Path>,
    playlist_dir: &Path,
    track: &RemoteTrack,
    bitrate_kbps: u32,
) -> Result<PathBuf, String> {
    let mut clean_title = sanitize_filename_component(&track.title);
    let clean_artist = sanitize_filename_component(&track.artist);
    if clean_title.is_empty() {
        clean_title = "Downloaded Track".to_string();
    }

    let base_stem = if clean_artist.is_empty() {
        clean_title.clone()
    } else {
        format!("{clean_title} - {clean_artist}")
    };

    let mut stem = base_stem.clone();
    let mut final_mp3 = playlist_dir.join(format!("{stem}.mp3"));
    let mut n = 2u32;
    while final_mp3.exists() {
        stem = format!("{base_stem} ({n})");
        final_mp3 = playlist_dir.join(format!("{stem}.mp3"));
        n += 1;
    }

    let escaped_stem = stem.replace('%', "%%");
    let output_template = playlist_dir.join(format!("{escaped_stem}.%(ext)s"));

    let source_arg = match track.youtube_url.as_deref().map(str::trim) {
        Some(u) if !u.is_empty() => crate::config::validate_http_url(u)?.to_string(),
        _ => {
            if !track.artist.trim().is_empty() {
                format!("ytsearch1:{} - {} audio", track.artist.trim(), track.title.trim())
            } else {
                format!("ytsearch1:{} audio", track.title.trim())
            }
        }
    };

    let mut cmd = hidden_command(ytdlp);
    cmd.args([
        "--no-playlist",
        "--no-warnings",
        "-x",
        "--audio-format",
        "mp3",
        "--audio-quality",
        &format!("{}K", bitrate_kbps.clamp(128, 320)),
    ]);

    if let Some(ff) = ffmpeg {
        if let Some(parent) = ff.parent() {
            cmd.arg("--ffmpeg-location").arg(parent);
        }
    }

    cmd.arg("-o")
        .arg(&output_template)
        .arg("--")
        .arg(&source_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .map_err(|e| format!("yt-dlp konnte nicht gestartet werden: {e}"))?;

    if !output.status.success() || !final_mp3.exists() {
        cleanup_partial_downloads(playlist_dir, &stem);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = pick_ytdlp_error_line(&stderr)
            .unwrap_or_else(|| "Download fehlgeschlagen — prüfe yt-dlp & ffmpeg".to_string());
        return Err(msg);
    }

    // Download and save track cover if provided by the source playlist
    if let Some(ref cover_url) = track.cover_url {
        if cover_url.starts_with("http") {
            if let Ok(bytes) = curl_get_bytes(cover_url, 8, "Rustify/1.0") {
                let _ = save_cover_bytes_for_track(&final_mp3, &bytes);
            }
        }
    }

    Ok(final_mp3)
}

// ---------------------------------------------------------------------------
// Background Batch Importer & Sync Manager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub playlist_name: String,
    #[allow(dead_code)]
    pub playlist_dir: PathBuf,
    pub completed: usize,
    pub total: usize,
    #[allow(dead_code)]
    pub downloaded_new: usize,
    pub current_track: String,
}

#[derive(Debug, Clone)]
pub enum SyncEvent {
    FetchingList(String),
    Progress(SyncProgress),
    TrackDownloaded {
        #[allow(dead_code)]
        playlist_dir: PathBuf,
        saved_path: PathBuf,
    },
    Finished {
        playlist_name: String,
        #[allow(dead_code)]
        playlist_dir: PathBuf,
        downloaded_new: usize,
        already_present: usize,
    },
    Error(String),
}

pub struct PlaylistSyncManager {
    pub active: bool,
    pub status_text: String,
    pub progress: Option<SyncProgress>,
    pub last_error: Option<String>,
    rx: Option<Receiver<SyncEvent>>,
    cancel_flag: Arc<AtomicBool>,
}

impl Default for PlaylistSyncManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaylistSyncManager {
    pub fn new() -> Self {
        Self {
            active: false,
            status_text: String::new(),
            progress: None,
            last_error: None,
            rx: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    fn begin_job(&mut self, initial_status: &str) -> (Sender<SyncEvent>, Arc<AtomicBool>) {
        // Signal any previous worker to stop cleanly
        self.cancel_flag.store(true, Ordering::SeqCst);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let (tx, rx) = unbounded::<SyncEvent>();

        self.active = true;
        self.status_text = initial_status.to_string();
        self.progress = None;
        self.last_error = None;
        self.rx = Some(rx);
        self.cancel_flag = cancel_flag.clone();

        (tx, cancel_flag)
    }

    pub fn start_import_new_playlist(
        &mut self,
        music_dir: PathBuf,
        url: String,
        custom_name: Option<String>,
        ytdlp: PathBuf,
        ffmpeg: Option<PathBuf>,
        bitrate_kbps: u32,
    ) {
        let (tx, cancel_flag) = self.begin_job("Playlist wird geladen...");

        let _ = thread::Builder::new()
            .name("rustify-playlist-sync".to_string())
            .spawn(move || {
                let _ = tx.send(SyncEvent::FetchingList(
                    "Playlist-Titel werden abgerufen...".to_string(),
                ));

                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = tx.send(SyncEvent::Error("Import abgebrochen.".to_string()));
                    return;
                }

                let remote = match fetch_remote_playlist(&url, Some(&ytdlp)) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(SyncEvent::Error(e));
                        return;
                    }
                };

                let chosen_raw = custom_name
                    .as_deref()
                    .map(sanitize_filename_component)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| sanitize_filename_component(&remote.name));

                let base_name = if chosen_raw.is_empty() {
                    format!("{} Playlist", remote.source_kind.label())
                } else {
                    chosen_raw
                };

                let (playlist_name, playlist_dir) =
                    allocate_unique_playlist_dir(&music_dir, &base_name);

                if let Err(e) = fs::create_dir_all(&playlist_dir) {
                    let _ = tx.send(SyncEvent::Error(format!(
                        "Playlist-Ordner konnte nicht erstellt werden: {e}"
                    )));
                    return;
                }
                let _ = fs::write(playlist_dir.join(PLAYLIST_MARKER_FILENAME), b"rustify");

                let meta = PlaylistSyncMeta {
                    source_kind: remote.source_kind,
                    source_url: remote.source_url.clone(),
                    playlist_name: playlist_name.clone(),
                };
                if let Err(e) = write_sync_meta(&playlist_dir, &meta) {
                    let _ = tx.send(SyncEvent::Error(e));
                    return;
                }

                run_batch_download_loop(
                    &tx,
                    &cancel_flag,
                    &playlist_name,
                    &playlist_dir,
                    &remote.tracks,
                    &ytdlp,
                    ffmpeg.as_deref(),
                    bitrate_kbps,
                );
            });
    }

    pub fn start_import_into_existing(
        &mut self,
        playlist_dir: PathBuf,
        playlist_name: String,
        url: String,
        ytdlp: PathBuf,
        ffmpeg: Option<PathBuf>,
        bitrate_kbps: u32,
    ) {
        let (tx, cancel_flag) = self.begin_job("Playlist wird geladen...");

        let _ = thread::Builder::new()
            .name("rustify-playlist-sync".to_string())
            .spawn(move || {
                let _ = tx.send(SyncEvent::FetchingList(format!(
                    "Lade Online-Playlist für '{playlist_name}'..."
                )));

                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = tx.send(SyncEvent::Error("Import abgebrochen.".to_string()));
                    return;
                }

                let remote = match fetch_remote_playlist(&url, Some(&ytdlp)) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(SyncEvent::Error(e));
                        return;
                    }
                };

                if let Err(e) = fs::create_dir_all(&playlist_dir) {
                    let _ = tx.send(SyncEvent::Error(format!(
                        "Playlist-Ordner konnte nicht geöffnet werden: {e}"
                    )));
                    return;
                }
                let _ = fs::write(playlist_dir.join(PLAYLIST_MARKER_FILENAME), b"rustify");

                let meta = PlaylistSyncMeta {
                    source_kind: remote.source_kind,
                    source_url: remote.source_url.clone(),
                    playlist_name: playlist_name.clone(),
                };
                if let Err(e) = write_sync_meta(&playlist_dir, &meta) {
                    let _ = tx.send(SyncEvent::Error(e));
                    return;
                }

                run_batch_download_loop(
                    &tx,
                    &cancel_flag,
                    &playlist_name,
                    &playlist_dir,
                    &remote.tracks,
                    &ytdlp,
                    ffmpeg.as_deref(),
                    bitrate_kbps,
                );
            });
    }

    pub fn start_update_synced_playlist(
        &mut self,
        playlist_dir: PathBuf,
        ytdlp: PathBuf,
        ffmpeg: Option<PathBuf>,
        bitrate_kbps: u32,
    ) {
        let (tx, cancel_flag) = self.begin_job("Synchronisiere Playlist...");

        let _ = thread::Builder::new()
            .name("rustify-playlist-sync".to_string())
            .spawn(move || {
                let Some(meta) = read_sync_meta(&playlist_dir) else {
                    let _ = tx.send(SyncEvent::Error(
                        "Für diese Playlist ist keine Online-Sync-Quelle (.rustify-sync.json) hinterlegt."
                            .to_string(),
                    ));
                    return;
                };

                let playlist_name = playlist_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.nfc().collect::<String>())
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| meta.playlist_name.clone());

                let _ = tx.send(SyncEvent::FetchingList(format!(
                    "Prüfe '{}' ({}) auf neue Titel...",
                    playlist_name,
                    meta.source_kind.label()
                )));

                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = tx.send(SyncEvent::Error("Sync abgebrochen.".to_string()));
                    return;
                }

                let remote = match fetch_remote_playlist(&meta.source_url, Some(&ytdlp)) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(SyncEvent::Error(e));
                        return;
                    }
                };

                run_batch_download_loop(
                    &tx,
                    &cancel_flag,
                    &playlist_name,
                    &playlist_dir,
                    &remote.tracks,
                    &ytdlp,
                    ffmpeg.as_deref(),
                    bitrate_kbps,
                );
            });
    }

    pub fn cancel(&mut self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        if self.active {
            self.status_text = "Wird nach dem aktuellen Titel abgebrochen...".to_string();
        }
    }

    pub fn poll(&mut self) -> Vec<SyncEvent> {
        let mut events = Vec::new();
        let mut disconnected = false;

        if let Some(ref rx) = self.rx {
            loop {
                match rx.try_recv() {
                    Ok(ev) => events.push(ev),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        for ev in &events {
            match ev {
                SyncEvent::FetchingList(msg) => {
                    self.active = true;
                    self.status_text = msg.clone();
                }
                SyncEvent::Progress(prog) => {
                    self.active = true;
                    self.progress = Some(prog.clone());
                    self.status_text = format!(
                        "{} ({}/{}) — {}",
                        prog.playlist_name, prog.completed, prog.total, prog.current_track
                    );
                }
                SyncEvent::TrackDownloaded { .. } => {}
                SyncEvent::Finished {
                    playlist_name,
                    downloaded_new,
                    already_present,
                    ..
                } => {
                    self.active = false;
                    self.progress = None;
                    self.rx = None;
                    self.status_text = if *downloaded_new > 0 {
                        format!(
                            "{playlist_name}: {downloaded_new} neu geladen ({already_present} bereits vorhanden)"
                        )
                    } else {
                        format!("{playlist_name}: Bereits aktuell ({already_present} Titel)")
                    };
                }
                SyncEvent::Error(err) => {
                    self.active = false;
                    self.progress = None;
                    self.rx = None;
                    self.last_error = Some(err.clone());
                    self.status_text = err.clone();
                }
            }
        }

        if disconnected && self.active {
            self.active = false;
            self.progress = None;
            self.rx = None;
        }

        events
    }
}

#[allow(clippy::too_many_arguments)]
fn run_batch_download_loop(
    tx: &Sender<SyncEvent>,
    cancel_flag: &AtomicBool,
    playlist_name: &str,
    playlist_dir: &Path,
    tracks: &[RemoteTrack],
    ytdlp: &Path,
    ffmpeg: Option<&Path>,
    bitrate_kbps: u32,
) {
    let mut local_index = LocalTrackIndex::from_playlist_dir(playlist_dir);
    let total = tracks.len();
    let mut completed = 0usize;
    let mut downloaded_new = 0usize;
    let mut already_present = 0usize;
    let mut last_download_error: Option<String> = None;

    for track in tracks {
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        let display_label = if track.artist.trim().is_empty() {
            track.title.clone()
        } else {
            format!("{} — {}", track.title, track.artist)
        };

        if local_index.contains_remote(track) {
            already_present += 1;
            completed += 1;
            let _ = tx.send(SyncEvent::Progress(SyncProgress {
                playlist_name: playlist_name.to_string(),
                playlist_dir: playlist_dir.to_path_buf(),
                completed,
                total,
                downloaded_new,
                current_track: format!("Bereits vorhanden: {display_label}"),
            }));
            continue;
        }

        let _ = tx.send(SyncEvent::Progress(SyncProgress {
            playlist_name: playlist_name.to_string(),
            playlist_dir: playlist_dir.to_path_buf(),
            completed,
            total,
            downloaded_new,
            current_track: format!("Lade: {display_label}"),
        }));

        match download_remote_track(ytdlp, ffmpeg, playlist_dir, track, bitrate_kbps) {
            Ok(saved_path) => {
                if let Some(stem) = saved_path.file_stem().and_then(|s| s.to_str()) {
                    local_index.insert_stem(stem);
                }
                downloaded_new += 1;
                completed += 1;
                let _ = tx.send(SyncEvent::TrackDownloaded {
                    playlist_dir: playlist_dir.to_path_buf(),
                    saved_path,
                });
                let _ = tx.send(SyncEvent::Progress(SyncProgress {
                    playlist_name: playlist_name.to_string(),
                    playlist_dir: playlist_dir.to_path_buf(),
                    completed,
                    total,
                    downloaded_new,
                    current_track: display_label,
                }));
            }
            Err(err) => {
                last_download_error = Some(err.clone());
                completed += 1;
                // Abort immediately if yt-dlp itself could not be started
                if err.starts_with("yt-dlp konnte nicht gestartet werden") {
                    let _ = tx.send(SyncEvent::Error(err));
                    return;
                }
            }
        }
    }

    if downloaded_new == 0 && already_present == 0 {
        if let Some(err) = last_download_error {
            let _ = tx.send(SyncEvent::Error(format!(
                "Kein Titel konnte heruntergeladen werden: {err}"
            )));
            return;
        }
    }

    let _ = tx.send(SyncEvent::Finished {
        playlist_name: playlist_name.to_string(),
        playlist_dir: playlist_dir.to_path_buf(),
        downloaded_new,
        already_present,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_source_kind_strict_hosts() {
        assert_eq!(
            detect_source_kind("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some(PlaylistSourceKind::Spotify)
        );
        assert_eq!(
            detect_source_kind("https://music.amazon.de/playlists/B08XYZ"),
            Some(PlaylistSourceKind::AmazonMusic)
        );
        assert_eq!(
            detect_source_kind("https://music.amazon.co.uk/playlists/B08XYZ"),
            Some(PlaylistSourceKind::AmazonMusic)
        );
        assert_eq!(
            detect_source_kind("https://www.youtube.com/playlist?list=PL123"),
            Some(PlaylistSourceKind::YouTube)
        );
        assert_eq!(
            detect_source_kind("https://music.youtube.com/playlist?list=PL123"),
            Some(PlaylistSourceKind::YouTube)
        );
        assert_eq!(
            detect_source_kind("https://youtu.be/dQw4w9WgXcQ?list=PL123"),
            Some(PlaylistSourceKind::YouTube)
        );
        assert_eq!(
            detect_source_kind("https://spotify.link/abc123"),
            Some(PlaylistSourceKind::Spotify)
        );
        assert_eq!(
            detect_source_kind("https://spoti.fi/abc123"),
            Some(PlaylistSourceKind::Spotify)
        );
        assert_eq!(
            detect_source_kind("open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            Some(PlaylistSourceKind::Spotify)
        );
        assert_eq!(
            detect_source_kind("music.amazon.de/playlists/B08XYZ"),
            Some(PlaylistSourceKind::AmazonMusic)
        );

        // Reject unknown http:// hosts, userinfo @, or spoofed substrings
        assert_eq!(
            detect_source_kind("http://evil.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            None
        );
        assert_eq!(
            detect_source_kind("file://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            None
        );
        assert_eq!(
            detect_source_kind("https://evil.com@music.amazon.com/playlists/123"),
            None
        );
        assert_eq!(
            detect_source_kind("https://evil.com/?q=music.amazon.com"),
            None
        );
        assert_eq!(
            detect_source_kind("https://music.amazon..com/playlists/123"),
            None
        );
        assert_eq!(
            detect_source_kind("https://evil.com/?list=PL123"),
            None
        );
    }

    #[test]
    fn test_normalize_playlist_url_and_extract_youtube_list_id() {
        assert_eq!(
            normalize_playlist_url("  open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M "),
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
        );
        assert_eq!(
            normalize_playlist_url("http://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"),
            "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
        );
        assert_eq!(
            normalize_playlist_url("http://evil.com/open.spotify.com/playlist/123"),
            "http://evil.com/open.spotify.com/playlist/123"
        );
        assert_eq!(
            extract_youtube_list_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLMC9KNkIncKtPzgY-5rmhvj7fax8fdxoj&index=1"),
            Some("PLMC9KNkIncKtPzgY-5rmhvj7fax8fdxoj".to_string())
        );
        assert_eq!(
            extract_youtube_list_id("https://youtu.be/dQw4w9WgXcQ?list=PL_123-abc"),
            Some("PL_123-abc".to_string())
        );
    }
}
