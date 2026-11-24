use crate::i18n::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaybackMode {
    #[default]
    Normal,
    Shuffle,
    DjMix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RepeatMode {
    Off,
    #[default]
    All,
    One,
}

impl RepeatMode {
    #[allow(dead_code)]
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EqPreset {
    #[default]
    Flat,
    BassBoost,
    HardstyleKick,
    VocalClarity,
    RockPop,
    Custom,
}

impl EqPreset {
    #[allow(dead_code)]
    pub fn label(self, lang: Language) -> &'static str {
        match (self, lang) {
            (EqPreset::Flat, _) => "Flat",
            (EqPreset::BassBoost, _) => "Bass Boost",
            (EqPreset::HardstyleKick, _) => "Hardstyle Kick",
            (EqPreset::VocalClarity, Language::De) => "Stimmen & Klarheit",
            (EqPreset::VocalClarity, Language::En) => "Vocal Clarity",
            (EqPreset::RockPop, _) => "Rock / Pop",
            (EqPreset::Custom, Language::De) => "Benutzerdefiniert",
            (EqPreset::Custom, Language::En) => "Custom",
        }
    }

    #[allow(dead_code)]
    pub fn bands_db(self) -> [f32; 5] {
        match self {
            EqPreset::Flat => [0.0, 0.0, 0.0, 0.0, 0.0],
            EqPreset::BassBoost => [7.5, 5.0, 0.0, 1.0, 2.0],
            EqPreset::HardstyleKick => [8.5, 4.5, -1.5, 3.0, 4.5],
            EqPreset::VocalClarity => [-1.5, -0.5, 3.5, 5.0, 2.5],
            EqPreset::RockPop => [4.5, 2.0, -1.0, 2.5, 4.0],
            EqPreset::Custom => [0.0, 0.0, 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub setup_completed: bool,
    pub language: Language,
    pub music_dir: Option<PathBuf>,
    pub volume: f32,
    pub muted: bool,
    pub playback_mode: PlaybackMode,
    pub repeat_mode: RepeatMode,
    pub last_playlist: Option<String>,
    pub favorites: HashSet<String>,

    // DJ Mix settings
    pub dj_crossfade_secs: f32,
    pub dj_tempo_sync: bool,
    pub dj_bass_swap: bool,
    pub dj_trim_silence: bool,
    pub normal_short_fade: bool,
    pub auto_analyze_bpm: bool,

    // 5-Band Equalizer settings
    #[serde(default)]
    pub eq_preset: EqPreset,
    #[serde(default)]
    pub eq_bands_db: [f32; 5],

    // Loudness normalization (ITU-R BS.1770-4, -14.0 LUFS target)
    #[serde(default = "default_true")]
    pub loudness_normalization: bool,

    // Discord Rich Presence
    #[serde(default = "default_true")]
    pub discord_rpc_enabled: bool,
    #[serde(default)]
    pub discord_client_id: String,

    // Auto-Updater settings
    #[serde(default = "default_true")]
    pub update_popup_enabled: bool,
    #[serde(default)]
    pub last_known_release_size: u64,

    // YouTube / yt-dlp settings
    pub ytdlp_path: Option<PathBuf>,
    pub ffmpeg_path: Option<PathBuf>,
    pub yt_mp3_bitrate: u32, // 320, 256, 192

    // Visual settings
    pub animated_glass_backdrop: bool,

    // Visualizer Lyrics settings
    #[serde(default = "default_true")]
    pub visualizer_show_lyrics: bool,
    #[serde(default)]
    pub visualizer_lyrics_top: bool, // false = Bottom (default), true = Top
}

fn default_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            setup_completed: false,
            language: Language::De,
            music_dir: detect_suggested_music_dir(),
            volume: 0.78,
            muted: false,
            playback_mode: PlaybackMode::Normal,
            repeat_mode: RepeatMode::All,
            last_playlist: None,
            favorites: HashSet::new(),
            dj_crossfade_secs: 8.0,
            dj_tempo_sync: true,
            dj_bass_swap: true,
            dj_trim_silence: true,
            normal_short_fade: false,
            auto_analyze_bpm: true,
            eq_preset: EqPreset::Flat,
            eq_bands_db: [0.0; 5],
            loudness_normalization: true,
            discord_rpc_enabled: true,
            discord_client_id: String::new(),
            update_popup_enabled: true,
            last_known_release_size: 0,
            ytdlp_path: None,
            ffmpeg_path: None,
            yt_mp3_bitrate: 320,
            animated_glass_backdrop: true,
            visualizer_show_lyrics: true,
            visualizer_lyrics_top: false,
        }
    }
}

impl AppConfig {
    pub fn app_data_dir() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("Rustify")
    }

    pub fn config_path() -> PathBuf {
        Self::app_data_dir().join("config.json")
    }

    pub fn bpm_cache_path() -> PathBuf {
        Self::app_data_dir().join("bpm_cache.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&data) {
                cfg.volume = cfg.volume.clamp(0.0, 1.0);
                cfg.dj_crossfade_secs = cfg.dj_crossfade_secs.clamp(3.0, 16.0);
                if cfg.music_dir.is_none() {
                    cfg.music_dir = detect_suggested_music_dir();
                }
                return cfg;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let dir = Self::app_data_dir();
        let _ = fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::config_path(), json);
        }
    }

    #[allow(dead_code)]
    pub fn is_favorite(&self, path: &Path) -> bool {
        let key = normalize_path_key(path);
        self.favorites.contains(&key)
    }

    #[allow(dead_code)]
    pub fn toggle_favorite(&mut self, path: &Path) -> bool {
        let key = normalize_path_key(path);
        if self.favorites.contains(&key) {
            self.favorites.remove(&key);
            self.save();
            false
        } else {
            self.favorites.insert(key);
            self.save();
            true
        }
    }
}

pub fn normalize_path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

pub fn validate_http_url(url: &str) -> Result<&str, String> {
    if url.chars().any(|c| c.is_control()) {
        return Err("URL contains invalid control characters".to_string());
    }
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL must not be empty".to_string());
    }
    if trimmed.chars().any(|c| c.is_whitespace()) {
        return Err("URL must not contain whitespace".to_string());
    }
    if trimmed.starts_with('-') {
        return Err("URL must not start with '-'".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err("URL must use http:// or https://".to_string());
    }
    Ok(trimmed)
}

pub fn is_audio_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac"
    )
}

fn dir_has_audio_playlists(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower.starts_with('.') || matches!(lower.as_str(), "src" | "target" | "assets" | "node_modules") {
                continue;
            }
        }
        if let Ok(sub) = fs::read_dir(&p) {
            for f in sub.flatten() {
                let fp = f.path();
                if fp.is_file() {
                    if let Some(ext) = fp.extension().and_then(|e| e.to_str()) {
                        if is_audio_extension(ext) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

pub fn detect_suggested_music_dir() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if dir_has_audio_playlists(exe_dir) {
                return Some(exe_dir.to_path_buf());
            }
            // Also check two levels up (when running from target/release/Rustify.exe)
            if let Some(parent) = exe_dir.parent().and_then(|p| p.parent()) {
                if dir_has_audio_playlists(parent) {
                    return Some(parent.to_path_buf());
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if dir_has_audio_playlists(&cwd) {
            return Some(cwd);
        }
    }
    if let Some(music) = dirs::audio_dir() {
        if dir_has_audio_playlists(&music) {
            return Some(music);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_http_url() {
        assert!(validate_http_url("https://example.com/path").is_ok());
        assert!(validate_http_url("HTTP://example.com").is_ok());
        assert!(validate_http_url("  https://open.spotify.com/playlist/123  ").is_ok());

        assert!(validate_http_url("").is_err());
        assert!(validate_http_url("   ").is_err());
        assert!(validate_http_url("--upload-file=/etc/passwd").is_err());
        assert!(validate_http_url("-o").is_err());
        assert!(validate_http_url("file:///C:/Windows/System32").is_err());
        assert!(validate_http_url("javascript:alert(1)").is_err());
        assert!(validate_http_url("https://example.com/\n--evil").is_err());
        assert!(validate_http_url("https://example.com/\r").is_err());
    }
}
