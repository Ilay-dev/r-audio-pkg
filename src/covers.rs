use crate::config::AppConfig;
use crate::library::Track;
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use unicode_normalization::UnicodeNormalization;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn legacy_covers_dir() -> PathBuf {
    AppConfig::app_data_dir().join("covers")
}

/// Returns the `<music_dir>/Covers` directory for a track (e.g. `C:\Users\Ilay\Desktop\Musik\Rustify\Covers`),
/// falling back to `%APPDATA%\Rustify\Covers`.
pub fn covers_dir_for_track(track_path: &Path) -> PathBuf {
    if let Some(music_dir) = track_path.parent().and_then(|p| p.parent()) {
        if music_dir.is_dir() {
            return music_dir.join("Covers");
        }
    }
    AppConfig::app_data_dir().join("Covers")
}

/// Deterministic cover PNG path in `<music_dir>/Covers/<Track Stem>.png`.
/// Automatically migrates any legacy hashed cover from `%APPDATA%\Rustify\covers` if present.
pub fn cover_path_for_track(track_path: &Path) -> PathBuf {
    let stem = track_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let normalized_stem: String = stem.nfc().collect::<String>().trim().to_string();
    let safe_stem: String = normalized_stem
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();

    let dir = covers_dir_for_track(track_path);
    let _ = fs::create_dir_all(&dir);
    let target_path = dir.join(format!("{safe_stem}.png"));

    if !target_path.is_file() {
        let lower_norm = normalized_stem.to_lowercase();
        let mut hasher = DefaultHasher::new();
        lower_norm.hash(&mut hasher);
        let hash = hasher.finish();
        let legacy_path = legacy_covers_dir().join(format!("{hash:016x}.png"));
        if legacy_path.is_file() {
            let _ = fs::copy(&legacy_path, &target_path);
        }
    }

    target_path
}

pub fn has_cover_on_disk(track_path: &Path) -> bool {
    cover_path_for_track(track_path).is_file()
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            encoded.push(b as char);
        } else if b == b' ' {
            encoded.push_str("%20");
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}

/// Strips parenthetical/bracketed tags like `(Extended Mix)`, `[Official Video]`, `HQ`, `HD`, etc.
fn strip_search_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for ch in raw.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if paren_depth == 0 && bracket_depth == 0 => out.push(ch),
            _ => {}
        }
    }

    let cleaned = out
        .split_whitespace()
        .filter(|tok| {
            !matches!(
                tok.to_ascii_lowercase().as_str(),
                "hq" | "hd" | "4k" | "lyrics" | "audio" | "official"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        raw.trim().to_string()
    } else {
        trimmed.to_string()
    }
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

fn curl_get_bytes(url: &str) -> Result<Vec<u8>, String> {
    let safe_url = crate::config::validate_http_url(url)?;
    let bin = curl_bin();
    let mut cmd = Command::new(bin);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .args(["-s", "-L", "--max-time", "10", "-A", "Rustify/1.0", "--", safe_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run {bin}: {e}"))?;

    if !output.status.success() || output.stdout.is_empty() {
        return Err("curl request failed or returned empty data".to_string());
    }
    Ok(output.stdout)
}

fn query_deezer(query: &str) -> Option<String> {
    let encoded = url_encode(query);
    let url = format!("https://api.deezer.com/search?q={encoded}&limit=1");
    let bytes = curl_get_bytes(&url).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let album = v.get("data")?.get(0)?.get("album")?;

    for key in ["cover_xl", "cover_big", "cover_medium"] {
        if let Some(s) = album.get(key).and_then(|val| val.as_str()) {
            if s.starts_with("http") {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn query_itunes(query: &str) -> Option<String> {
    let encoded = url_encode(query);
    let url = format!("https://itunes.apple.com/search?term={encoded}&entity=song&limit=1");
    let bytes = curl_get_bytes(&url).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let artwork = v
        .get("results")?
        .get(0)?
        .get("artworkUrl100")?
        .as_str()?;
    if artwork.starts_with("http") {
        Some(artwork.replace("100x100bb", "500x500bb"))
    } else {
        None
    }
}

fn search_cover_url(query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    // Step A: Deezer public API
    if let Some(url) = query_deezer(q) {
        return Some(url);
    }
    // Step B: iTunes Search API
    query_itunes(q)
}

fn decode_crop_and_save_png(bytes: &[u8], dest_png: &Path) -> Result<(), String> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| format!("Failed to decode cover image: {e}"))?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err("Cover image has zero dimensions".to_string());
    }
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let cropped = img.crop_imm(x, y, side, side);
    let resized = cropped.resize_exact(256, 256, image::imageops::FilterType::Triangle);

    if let Some(parent) = dest_png.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut png_bytes = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode cover PNG: {e}"))?;
    fs::write(dest_png, &png_bytes).map_err(|e| format!("Failed to write cover PNG: {e}"))
}

pub fn fetch_cover_from_internet(title: &str, artist: &str, dest_png: &Path) -> Result<(), String> {
    let t = title.trim();
    let a = artist.trim();
    if t.is_empty() && a.is_empty() {
        return Err("Empty title and artist".to_string());
    }

    let primary_query = if a.is_empty() {
        t.to_string()
    } else {
        format!("{t} {a}")
    };

    // Step A & B: Query Deezer then iTunes with "{title} {artist}"
    let mut img_url = search_cover_url(&primary_query);

    // Step C: If neither returned an image URL, retry with stripped title
    if img_url.is_none() {
        let cleaned_title = strip_search_tags(t);
        if !a.is_empty() && cleaned_title != t {
            img_url = search_cover_url(&format!("{cleaned_title} {a}"));
        }
        if img_url.is_none() && (!a.is_empty() || cleaned_title != t) {
            img_url = search_cover_url(&cleaned_title);
        }
    }

    let url = img_url.ok_or_else(|| "No cover found online".to_string())?;
    let img_bytes = curl_get_bytes(&url)?;
    decode_crop_and_save_png(&img_bytes, dest_png)
}

pub fn set_custom_cover(track_path: &Path, source_image_path: &Path) -> Result<(), String> {
    let bytes =
        fs::read(source_image_path).map_err(|e| format!("Failed to read image file: {e}"))?;
    let dest = cover_path_for_track(track_path);
    decode_crop_and_save_png(&bytes, &dest)
}

pub fn save_cover_bytes_for_track(track_path: &Path, image_bytes: &[u8]) -> Result<(), String> {
    let dest = cover_path_for_track(track_path);
    decode_crop_and_save_png(image_bytes, &dest)
}

pub fn migrate_cover_on_rename(old_track_path: &Path, new_track_path: &Path) {
    let old_cover = cover_path_for_track(old_track_path);
    let new_cover = cover_path_for_track(new_track_path);
    if old_cover != new_cover && old_cover.is_file() && !new_cover.is_file() {
        if let Some(parent) = new_cover.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::rename(&old_cover, &new_cover);
    }
}

pub struct CoverManager {
    textures: HashMap<PathBuf, Option<TextureHandle>>,
    in_flight: HashSet<PathBuf>,
    job_tx: Sender<(PathBuf, String, String, bool)>,
    done_rx: Receiver<PathBuf>,
    pending_count: Arc<AtomicUsize>,
    total_batch: usize,
    completed_batch: usize,
}

impl Default for CoverManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverManager {
    pub fn new() -> Self {
        let _ = fs::create_dir_all(legacy_covers_dir());
        let (job_tx, job_rx) = unbounded::<(PathBuf, String, String, bool)>();
        let (done_tx, done_rx) = unbounded::<PathBuf>();
        let pending_count = Arc::new(AtomicUsize::new(0));

        for idx in 0..4 {
            let rx = job_rx.clone();
            let tx = done_tx.clone();
            let pending = pending_count.clone();
            let _ = thread::Builder::new()
                .name(format!("rustify-cover-{idx}"))
                .spawn(move || {
                    while let Ok((track_path, title, artist, force)) = rx.recv() {
                        if force || !has_cover_on_disk(&track_path) {
                            let dest = cover_path_for_track(&track_path);
                            let _ = fetch_cover_from_internet(&title, &artist, &dest);
                        }
                        pending.fetch_sub(1, Ordering::Relaxed);
                        let _ = tx.send(track_path);
                    }
                });
        }

        Self {
            textures: HashMap::new(),
            in_flight: HashSet::new(),
            job_tx,
            done_rx,
            pending_count,
            total_batch: 0,
            completed_batch: 0,
        }
    }

    pub fn poll(&mut self) {
        while let Ok(track_path) = self.done_rx.try_recv() {
            self.in_flight.remove(&track_path);
            self.textures.remove(&track_path);
            self.completed_batch = (self.completed_batch + 1).min(self.total_batch);
        }
        if self.in_flight.is_empty() && self.pending_count.load(Ordering::Relaxed) == 0 {
            self.total_batch = 0;
            self.completed_batch = 0;
        }
    }

    pub fn get_texture(&mut self, ctx: &Context, track_path: &Path) -> Option<TextureHandle> {
        if let Some(cached) = self.textures.get(track_path) {
            return cached.clone();
        }

        let cover_path = cover_path_for_track(track_path);
        let handle = if cover_path.is_file() {
            fs::read(&cover_path)
                .ok()
                .and_then(|bytes| image::load_from_memory(&bytes).ok())
                .map(|img| {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_img = ColorImage::from_rgba_unmultiplied(
                        [w as usize, h as usize],
                        rgba.as_raw(),
                    );
                    let tex_name = format!("cover_{}", cover_path.to_string_lossy());
                    ctx.load_texture(tex_name, color_img, TextureOptions::LINEAR)
                })
        } else {
            None
        };

        self.textures.insert(track_path.to_path_buf(), handle.clone());
        handle
    }

    pub fn invalidate(&mut self, track_path: &Path) {
        self.textures.remove(track_path);
    }

    pub fn fetch_missing_for_tracks(&mut self, tracks: &[Track]) -> usize {
        if self.in_flight.is_empty() && self.pending_count.load(Ordering::Relaxed) == 0 {
            self.total_batch = 0;
            self.completed_batch = 0;
        }

        let mut enqueued = 0usize;
        for track in tracks {
            if has_cover_on_disk(&track.path) || self.in_flight.contains(&track.path) {
                continue;
            }
            self.in_flight.insert(track.path.clone());
            self.pending_count.fetch_add(1, Ordering::Relaxed);
            self.total_batch += 1;
            let _ = self.job_tx.send((
                track.path.clone(),
                track.title.clone(),
                track.artist.clone(),
                false,
            ));
            enqueued += 1;
        }
        enqueued
    }

    pub fn force_fetch_one(&mut self, track: &Track) {
        if self.in_flight.contains(&track.path) {
            return;
        }
        if self.in_flight.is_empty() && self.pending_count.load(Ordering::Relaxed) == 0 {
            self.total_batch = 0;
            self.completed_batch = 0;
        }
        self.in_flight.insert(track.path.clone());
        self.pending_count.fetch_add(1, Ordering::Relaxed);
        self.total_batch += 1;
        let _ = self.job_tx.send((
            track.path.clone(),
            track.title.clone(),
            track.artist.clone(),
            true,
        ));
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        let pending = self.pending_count.load(Ordering::Relaxed);
        if pending > 0 && self.total_batch > 0 {
            Some((self.completed_batch.min(self.total_batch), self.total_batch))
        } else {
            None
        }
    }
}
