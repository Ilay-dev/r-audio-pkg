use crate::library::sanitize_filename_component;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone)]
pub enum InfoEvent {
    Loaded { title: String, artist: String },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct YouTubeSearchResult {
    pub id: String,
    pub url: String,
    pub raw_title: String,
    pub channel: String,
    pub parsed_title: String,
    pub parsed_artist: String,
    pub duration_sec: Option<f32>,
    pub thumbnail_rgba: Option<(u32, u32, Vec<u8>)>,
    pub thumbnail_raw_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub enum SearchEvent {
    Results(Vec<YouTubeSearchResult>),
    Error(String),
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    FetchingInfo,
    Downloading(f32), // 0.0..1.0
    Converting,
    Completed {
        playlist_dir: PathBuf,
        saved_path: PathBuf,
        display_name: String,
    },
    Error(String),
}

fn find_executable(custom: Option<&PathBuf>, names: &[&str]) -> Option<PathBuf> {
    if let Some(p) = custom {
        if p.is_file() {
            return Some(p.clone());
        }
    }

    // 1. Check alongside Rustify.exe (portable deployment!)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }

    // 2. Check PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in names {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }

    // 3. Common Windows Python / Scoop / Winget locations
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = dirs::data_local_dir() {
            let py_base = local.join("Programs").join("Python");
            if let Ok(entries) = std::fs::read_dir(&py_base) {
                for entry in entries.flatten() {
                    let scripts = entry.path().join("Scripts");
                    for name in names {
                        let cand = scripts.join(name);
                        if cand.is_file() {
                            return Some(cand);
                        }
                    }
                }
            }
        }
    }

    // 4. Common Linux / Unix locations (pacman, apt, pipx, ~/.local/bin)
    #[cfg(not(target_os = "windows"))]
    {
        let mut unix_dirs: Vec<PathBuf> = Vec::new();
        if let Some(home) = dirs::home_dir() {
            unix_dirs.push(home.join(".local").join("bin"));
        }
        unix_dirs.push(PathBuf::from("/usr/local/bin"));
        unix_dirs.push(PathBuf::from("/usr/bin"));
        for dir in unix_dirs {
            for name in names {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }

    None
}

pub fn resolve_ytdlp(custom: Option<&PathBuf>) -> Option<PathBuf> {
    find_executable(custom, &["yt-dlp.exe", "yt-dlp"])
}

pub fn resolve_ffmpeg(custom: Option<&PathBuf>) -> Option<PathBuf> {
    find_executable(custom, &["ffmpeg.exe", "ffmpeg"])
}

fn hidden_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

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

/// Clean noisy YouTube tags like "(Official Video)", "[Official Audio]", "(Lyrics)", etc.
fn strip_youtube_noise(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    let patterns = [
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

    for pat in patterns {
        remove_ascii_case_insensitive(&mut s, pat);
    }

    // Trim trailing separators like " | ..."
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

/// Synchronously fetch & parse `(Title, Artist)` from a YouTube link via `yt-dlp`.
fn fetch_metadata_sync(ytdlp: &Path, url: &str) -> Result<(String, String), String> {
    let safe_url = crate::config::validate_http_url(url)?;
    let output = hidden_command(ytdlp)
        .args([
            "--no-playlist",
            "--no-warnings",
            "--skip-download",
            "--print",
            "%(title)s",
            "--print",
            "%(track)s",
            "--print",
            "%(artist)s",
            "--print",
            "%(uploader)s",
            "--",
            safe_url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to start yt-dlp: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let first_line = err.lines().next().unwrap_or("yt-dlp failed").trim();
        return Err(first_line.to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let raw_title = lines.first().copied().unwrap_or("").trim();
    let meta_track = lines.get(1).copied().unwrap_or("NA").trim();
    let meta_artist = lines.get(2).copied().unwrap_or("NA").trim();
    let raw_uploader = lines.get(3).copied().unwrap_or("").trim();

    // 1. Prefer official YouTube Music track + artist metadata when available
    if !meta_track.is_empty() && meta_track != "NA" && !meta_artist.is_empty() && meta_artist != "NA" {
        let t = sanitize_filename_component(&strip_youtube_noise(meta_track));
        let a = sanitize_filename_component(meta_artist);
        if !t.is_empty() {
            return Ok((t, a));
        }
    }

    // 2. Otherwise parse standard YouTube "Artist - Song Title" -> Rustify's "Title - Artist"
    let cleaned = strip_youtube_noise(raw_title);
    if let Some((left, right)) = cleaned.split_once(" - ") {
        let a = sanitize_filename_component(left);
        let t = sanitize_filename_component(right);
        if !t.is_empty() && !a.is_empty() {
            return Ok((t, a));
        }
    }

    // 3. Fallback: video title + channel uploader
    let t = sanitize_filename_component(&cleaned);
    let a = sanitize_filename_component(&clean_uploader(raw_uploader));
    Ok((
        if t.is_empty() {
            "Downloaded Track".to_string()
        } else {
            t
        },
        a,
    ))
}

pub fn spawn_fetch_info(ytdlp: PathBuf, url: String) -> Receiver<InfoEvent> {
    let (tx, rx) = unbounded();
    thread::spawn(move || match fetch_metadata_sync(&ytdlp, &url) {
        Ok((title, artist)) => {
            let _ = tx.send(InfoEvent::Loaded { title, artist });
        }
        Err(e) => {
            let _ = tx.send(InfoEvent::Error(e));
        }
    });
    rx
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

fn fetch_thumbnail_bytes(url: &str) -> Option<Vec<u8>> {
    let safe_url = crate::config::validate_http_url(url).ok()?;
    let mut cmd = Command::new(curl_bin());
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .args(["-s", "-L", "--max-time", "6", "-A", "Rustify/1.0", "--", safe_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() && !out.stdout.is_empty() {
        Some(out.stdout)
    } else {
        None
    }
}

fn decode_square_thumbnail(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    // Crop center square (eliminating 16:9 letterbox borders if present)
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let cropped = img.crop_imm(x, y, side, side);
    let resized = cropped.resize_exact(112, 112, image::imageops::FilterType::Triangle);
    let rgba = resized.to_rgba8();
    Some((112, 112, rgba.into_raw()))
}

fn split_title_and_artist_from_search(raw_title: &str, raw_channel: &str) -> (String, String) {
    let cleaned = strip_youtube_noise(raw_title);
    for sep in [" - ", " – ", " — "] {
        if let Some((left, right)) = cleaned.split_once(sep) {
            let a = sanitize_filename_component(left);
            let t = sanitize_filename_component(right);
            if !t.is_empty() && !a.is_empty() {
                return (t, a);
            }
        }
    }
    let t = sanitize_filename_component(&cleaned);
    let a = sanitize_filename_component(&clean_uploader(raw_channel));
    (
        if t.is_empty() {
            raw_title.trim().to_string()
        } else {
            t
        },
        a,
    )
}

fn search_youtube_top5_sync(ytdlp: &Path, query: &str) -> Result<Vec<YouTubeSearchResult>, String> {
    let q: String = query
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let q = q.trim().trim_start_matches('-').trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let search_spec = format!("ytsearch5:{q}");
    let output = hidden_command(ytdlp)
        .args(["--flat-playlist", "-J", "--no-warnings", "--", &search_spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to start yt-dlp search: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let first_line = err.lines().next().unwrap_or("YouTube-Suche fehlgeschlagen").trim();
        return Err(first_line.to_string());
    }

    let root: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Ungültige Suchantwort: {e}"))?;
    let entries = root
        .get("entries")
        .and_then(|e| e.as_array())
        .ok_or_else(|| "Keine Ergebnisse gefunden".to_string())?;

    let mut items = Vec::with_capacity(5);
    for entry in entries.iter().take(5) {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let url = entry
            .get("url")
            .and_then(|v| v.as_str())
            .filter(|s| s.starts_with("http"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));

        let raw_title = entry
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unbekannter Titel")
            .trim()
            .to_string();

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
            .or_else(|| {
                entry
                    .get("uploader_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or("")
            .trim()
            .to_string();

        let duration_sec = entry
            .get("duration")
            .and_then(|v| v.as_f64())
            .map(|d| d as f32);

        let (parsed_title, parsed_artist) =
            split_title_and_artist_from_search(&raw_title, &raw_channel);

        let channel = if !raw_channel.is_empty() {
            raw_channel
        } else if !parsed_artist.is_empty() {
            parsed_artist.clone()
        } else {
            "YouTube".to_string()
        };

        items.push(YouTubeSearchResult {
            id,
            url,
            raw_title,
            channel,
            parsed_title,
            parsed_artist,
            duration_sec,
            thumbnail_rgba: None,
            thumbnail_raw_bytes: None,
        });
    }

    // Download all 5 thumbnails in parallel (~100ms total)
    thread::scope(|s| {
        let mut handles = Vec::with_capacity(items.len());
        for item in &items {
            let vid = item.id.clone();
            handles.push(s.spawn(move || {
                let thumb_url = format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg");
                let bytes = fetch_thumbnail_bytes(&thumb_url)?;
                let decoded = decode_square_thumbnail(&bytes);
                Some((bytes, decoded))
            }));
        }
        for (item, handle) in items.iter_mut().zip(handles) {
            if let Ok(Some((raw_bytes, rgba_opt))) = handle.join() {
                item.thumbnail_raw_bytes = Some(raw_bytes);
                item.thumbnail_rgba = rgba_opt;
            }
        }
    });

    Ok(items)
}

pub fn spawn_youtube_search(ytdlp: PathBuf, query: String) -> Receiver<SearchEvent> {
    let (tx, rx) = unbounded();
    thread::spawn(move || match search_youtube_top5_sync(&ytdlp, &query) {
        Ok(results) => {
            let _ = tx.send(SearchEvent::Results(results));
        }
        Err(e) => {
            let _ = tx.send(SearchEvent::Error(e));
        }
    });
    rx
}

struct DownloadJob {
    ytdlp: PathBuf,
    ffmpeg: Option<PathBuf>,
    url: String,
    playlist_dir: PathBuf,
    custom_title: String,
    custom_artist: String,
    bitrate_kbps: u32,
}

pub fn spawn_download(
    ytdlp: PathBuf,
    ffmpeg: Option<PathBuf>,
    url: String,
    playlist_dir: PathBuf,
    custom_title: String,
    custom_artist: String,
    bitrate_kbps: u32,
) -> Receiver<DownloadEvent> {
    let (tx, rx) = unbounded();
    let job = DownloadJob {
        ytdlp,
        ffmpeg,
        url,
        playlist_dir,
        custom_title,
        custom_artist,
        bitrate_kbps,
    };
    thread::spawn(move || {
        run_download_worker(tx, job);
    });
    rx
}

fn run_download_worker(tx: Sender<DownloadEvent>, job: DownloadJob) {
    let DownloadJob {
        ytdlp,
        ffmpeg,
        url,
        playlist_dir,
        custom_title,
        custom_artist,
        bitrate_kbps,
    } = job;
    let safe_url = match crate::config::validate_http_url(&url) {
        Ok(u) => u.to_string(),
        Err(e) => {
            let _ = tx.send(DownloadEvent::Error(e));
            return;
        }
    };
    let mut title = sanitize_filename_component(&custom_title);
    let mut artist = sanitize_filename_component(&custom_artist);

    if title.is_empty() {
        let _ = tx.send(DownloadEvent::FetchingInfo);
        match fetch_metadata_sync(&ytdlp, &safe_url) {
            Ok((t, a)) => {
                title = t;
                if artist.is_empty() {
                    artist = a;
                }
            }
            Err(e) => {
                let _ = tx.send(DownloadEvent::Error(e));
                return;
            }
        }
    }

    let base_stem = if artist.is_empty() {
        title.clone()
    } else {
        format!("{title} - {artist}")
    };

    // Ensure unique filename so we never overwrite an existing song
    let mut stem = base_stem.clone();
    let mut final_mp3 = playlist_dir.join(format!("{stem}.mp3"));
    let mut n = 2u32;
    while final_mp3.exists() {
        stem = format!("{base_stem} ({n})");
        final_mp3 = playlist_dir.join(format!("{stem}.mp3"));
        n += 1;
    }

    let output_template = playlist_dir.join(format!("{stem}.%(ext)s"));

    let mut cmd = hidden_command(&ytdlp);
    cmd.args([
        "--no-playlist",
        "--no-warnings",
        "--newline",
        "--progress",
        "-x",
        "--audio-format",
        "mp3",
        "--audio-quality",
        &format!("{}K", bitrate_kbps.clamp(128, 320)),
    ]);

    if let Some(ref ff) = ffmpeg {
        if let Some(parent) = ff.parent() {
            cmd.arg("--ffmpeg-location").arg(parent);
        }
    }

    cmd.arg("-o")
        .arg(&output_template)
        .arg("--")
        .arg(&safe_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(DownloadEvent::Error(format!("Failed to run yt-dlp: {e}")));
            return;
        }
    };

    let _ = tx.send(DownloadEvent::Downloading(0.02));

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if line.contains("[ExtractAudio]") || line.contains("[Fixup") {
                let _ = tx.send(DownloadEvent::Converting);
            } else if let Some(pct) = parse_ytdlp_percent(&line) {
                let _ = tx.send(DownloadEvent::Downloading((pct / 100.0).clamp(0.0, 1.0)));
            }
        }
    }

    let output = child.wait_with_output();
    match output {
        Ok(out) if out.status.success() && final_mp3.exists() => {
            let _ = tx.send(DownloadEvent::Completed {
                playlist_dir,
                saved_path: final_mp3,
                display_name: stem,
            });
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("Download failed — check link & ffmpeg")
                .trim()
                .to_string();
            let _ = tx.send(DownloadEvent::Error(msg));
        }
        Err(e) => {
            let _ = tx.send(DownloadEvent::Error(format!("yt-dlp error: {e}")));
        }
    }
}

fn parse_ytdlp_percent(line: &str) -> Option<f32> {
    if !line.contains("[download]") {
        return None;
    }
    let pct_idx = line.find('%')?;
    let before = &line[..pct_idx];
    let num_str = before.split_whitespace().last()?;
    num_str.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_youtube_noise_multibyte_utf8_no_panic() {
        // Uppercase Turkish 'İ' (2 bytes in UTF-8, 3 bytes when lowercased with full Unicode)
        // and German 'ß', plus emojis — must never panic on byte boundary mismatches!
        let input = "İİİ İstanbul Straßen 🎵 (Official Music Video) [HD] (Lyrics) | Official Channel";
        let cleaned = strip_youtube_noise(input);
        assert_eq!(cleaned, "İİİ İstanbul Straßen 🎵");

        let mixed_case = "Große Freiheit 🔥 [oFfIcIaL vIdEo] (4k remaster) [AUDIO]";
        assert_eq!(strip_youtube_noise(mixed_case), "Große Freiheit 🔥");

        let s = "İstanbul Şarkısı (Official Video) [HD]";
        assert_eq!(strip_youtube_noise(s), "İstanbul Şarkısı");
    }
}
