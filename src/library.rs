use crate::audio::decoder::probe_track_info;
use crate::audio::TrackAnalysis;
use crate::config::is_audio_extension;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone)]
pub struct Track {
    pub path: PathBuf,
    pub playlist_name: String,
    pub title: String,
    pub artist: String,
    pub duration_sec: Option<f32>,
    pub analysis: Option<TrackAnalysis>,
    #[allow(dead_code)]
    pub art_seed: u64,
    #[allow(dead_code)]
    pub embedded_art: Option<Vec<u8>>,
}

impl Track {
    pub fn display_artist<'a>(&'a self, unknown_label: &'a str) -> &'a str {
        if self.artist.trim().is_empty() {
            unknown_label
        } else {
            &self.artist
        }
    }

    pub fn effective_duration(&self) -> f32 {
        self.analysis
            .as_ref()
            .map(|a| a.duration_sec)
            .or(self.duration_sec)
            .unwrap_or(0.0)
    }

    pub fn bpm(&self) -> Option<f32> {
        self.analysis.as_ref().map(|a| a.bpm)
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub name: String,
    pub dir: PathBuf,
    pub tracks: Vec<Track>,
    #[allow(dead_code)]
    pub art_seed: u64,
}

#[allow(dead_code)]
impl Playlist {
    pub fn total_duration_sec(&self) -> f32 {
        self.tracks.iter().map(|t| t.effective_duration()).sum()
    }

    pub fn avg_bpm(&self) -> Option<f32> {
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for t in &self.tracks {
            if let Some(b) = t.bpm() {
                sum += b;
                count += 1;
            }
        }
        if count > 0 {
            Some(sum / count as f32)
        } else {
            None
        }
    }
}

/// Parses `"Title - Artist"` from a filename stem with Unicode NFC normalization.
pub fn parse_title_artist(stem: &str) -> (String, String) {
    let normalized: String = stem.nfc().collect();
    let trimmed = normalized.trim();
    if let Some((title, artist)) = trimmed.rsplit_once(" - ") {
        let t = title.trim();
        let a = artist.trim();
        if !t.is_empty() {
            return (t.to_string(), a.to_string());
        }
    }
    (trimmed.to_string(), String::new())
}

pub fn compute_seed(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

pub fn scan_library(music_dir: &Path) -> Vec<Playlist> {
    let Ok(entries) = fs::read_dir(music_dir) else {
        return Vec::new();
    };

    let mut playlists = Vec::new();

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(raw_name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let lower = raw_name.to_ascii_lowercase();
        if lower.starts_with('.')
            || matches!(
                lower.as_str(),
                "src" | "target" | "assets" | "covers" | "models" | "node_modules" | ".git"
            )
        {
            continue;
        }

        let name: String = raw_name.nfc().collect();
        let has_marker = dir.join(".rustify-playlist").exists();
        let tracks = scan_playlist_dir(&dir, &name);

        if !tracks.is_empty() || has_marker {
            let art_seed = compute_seed(&name);
            playlists.push(Playlist {
                name,
                dir,
                tracks,
                art_seed,
            });
        }
    }

    playlists.sort_by_key(|a| a.name.to_lowercase());
    playlists
}

pub fn scan_playlist_dir(dir: &Path, playlist_name: &str) -> Vec<Track> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut tracks = Vec::new();
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
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown");
        let (title, artist) = parse_title_artist(stem);
        let probe = probe_track_info(&path);
        let art_seed = compute_seed(&format!("{title}|{artist}"));

        tracks.push(Track {
            path,
            playlist_name: playlist_name.to_string(),
            title,
            artist,
            duration_sec: probe.duration_sec,
            analysis: None,
            art_seed,
            embedded_art: probe.embedded_art,
        });
    }

    tracks.sort_by_key(|a| a.title.to_lowercase());
    tracks
}

pub fn sanitize_filename_component(input: &str) -> String {
    let cleaned: String = input
        .nfc()
        .collect::<String>()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    // Collapse multiple whitespace and strip leading/trailing dots and spaces
    let s = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();

    if s.is_empty() {
        return s;
    }

    let base = s.split('.').next().unwrap_or(&s).trim();
    let upper = base.to_ascii_uppercase();
    let is_reserved = matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if is_reserved {
        format!("_{s}")
    } else {
        s
    }
}

pub fn create_playlist(music_dir: &Path, raw_name: &str) -> Result<PathBuf, String> {
    let clean = sanitize_filename_component(raw_name);
    if clean.is_empty() {
        return Err("Playlist name cannot be empty".to_string());
    }
    let dir = music_dir.join(&clean);
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create folder: {e}"))?;
    let _ = fs::write(dir.join(".rustify-playlist"), b"rustify");
    Ok(dir)
}

pub fn rename_track(path: &Path, new_title: &str, new_artist: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid track path".to_string())?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");

    let t = sanitize_filename_component(new_title);
    let a = sanitize_filename_component(new_artist);
    if t.is_empty() {
        return Err("Title cannot be empty".to_string());
    }

    let stem = if a.is_empty() {
        t
    } else {
        format!("{t} - {a}")
    };
    let target = parent.join(format!("{stem}.{ext}"));
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err("A track with this name already exists".to_string());
    }
    fs::rename(path, &target).map_err(|e| format!("Rename failed: {e}"))?;
    Ok(target)
}

pub fn delete_track(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(|e| format!("Delete failed: {e}"))
}

pub fn import_audio_files(playlist_dir: &Path, files: &[PathBuf]) -> usize {
    let mut imported = 0usize;
    for src in files {
        if !src.is_file() {
            continue;
        }
        let Some(ext) = src.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !is_audio_extension(ext) {
            continue;
        }
        let Some(file_name) = src.file_name() else {
            continue;
        };
        let mut dest = playlist_dir.join(file_name);
        if dest == *src {
            continue;
        }
        if dest.exists() {
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("track");
            let mut n = 2u32;
            while dest.exists() {
                dest = playlist_dir.join(format!("{stem} ({n}).{ext}"));
                n += 1;
            }
        }
        if fs::copy(src, &dest).is_ok() {
            imported += 1;
        }
    }
    imported
}

pub fn format_mm_ss(seconds: f32) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0:00".to_string();
    }
    let total = seconds.round() as u64;
    let mins = total / 60;
    let secs = total % 60;
    format!("{mins}:{secs:02}")
}

#[allow(dead_code)]
pub fn format_total_duration(seconds: f32, is_german: bool) -> String {
    let total_mins = (seconds.max(0.0) / 60.0).round() as u64;
    let hours = total_mins / 60;
    let mins = total_mins % 60;
    if hours > 0 {
        if is_german {
            format!("{hours} Std. {mins} Min.")
        } else {
            format!("{hours} hr {mins} min")
        }
    } else if is_german {
        format!("{mins} Min.")
    } else {
        format!("{mins} min")
    }
}

pub fn reveal_in_explorer(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        if path.is_dir() {
            let _ = std::process::Command::new("explorer")
                .arg(path)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        } else {
            let _ = std::process::Command::new("explorer")
                .arg("/select,")
                .arg(path)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_component_leading_dots_and_reserved_names() {
        assert_eq!(sanitize_filename_component("...hidden"), "hidden");
        assert_eq!(sanitize_filename_component("..hidden.."), "hidden");
        assert_eq!(sanitize_filename_component(".gitignore"), "gitignore");
        assert_eq!(sanitize_filename_component(". . .Leading Dots.."), "Leading Dots");
        assert_eq!(sanitize_filename_component("..."), "");
        assert_eq!(sanitize_filename_component("CON"), "_CON");
        assert_eq!(sanitize_filename_component("con"), "_con");
        assert_eq!(sanitize_filename_component("prn.mp3"), "_prn.mp3");
        assert_eq!(sanitize_filename_component("AUX .txt"), "_AUX .txt");
        assert_eq!(sanitize_filename_component("NUL"), "_NUL");
        assert_eq!(sanitize_filename_component("nul"), "_nul");
        assert_eq!(sanitize_filename_component("com1"), "_com1");
        assert_eq!(sanitize_filename_component("LPT9"), "_LPT9");
        assert_eq!(sanitize_filename_component("LPT9.flac"), "_LPT9.flac");
        assert_eq!(sanitize_filename_component("Con Air"), "Con Air");
        assert_eq!(sanitize_filename_component("Normal Track"), "Normal Track");
        assert_eq!(sanitize_filename_component("Song: <Best> / *Track*?"), "Song Best Track");
    }
}
