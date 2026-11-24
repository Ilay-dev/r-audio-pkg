pub mod bpm;
pub mod decoder;
pub mod dj;
pub mod engine;

use crate::config::PlaybackMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent analysis result for a single track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackAnalysis {
    pub bpm: f32,
    pub confidence: f32,
    /// Detected beat phase offset in seconds (in [0, 60/bpm)).
    pub first_beat_sec: f32,
    /// Energetic phrase entry cue point in seconds (downbeat-aligned).
    pub mix_in_sec: f32,
    /// Energetic phrase exit cue point in seconds (before quiet outro tail).
    pub mix_out_sec: f32,
    /// Perceived track energy [0.0, 1.0].
    pub energy: f32,
    /// Makeup gain for softer intros during DJ crossfade [1.0, 1.55].
    #[serde(default = "default_intro_boost")]
    pub intro_boost: f32,
    /// Exact decoded duration in seconds.
    pub duration_sec: f32,
    /// AI-detected beat timestamps in seconds across the track (from `beat-this`).
    #[serde(default)]
    pub beats: Vec<f32>,
    /// AI-detected bar/measure downbeat ("Takt 1") timestamps in seconds (from `beat-this`).
    #[serde(default)]
    pub downbeats: Vec<f32>,
    /// Musical key in Camelot Wheel notation (e.g. "8A", "11B").
    #[serde(default)]
    pub camelot_key: String,
    /// Camelot wheel number 1..=12 (0 if unknown).
    #[serde(default)]
    pub camelot_num: u8,
    /// True for Major ('B'), false for Minor ('A').
    #[serde(default)]
    pub camelot_major: bool,
    /// ITU-R BS.1770-4 integrated loudness in LUFS (e.g. -14.0).
    #[serde(default = "default_lufs")]
    pub lufs: f32,
    /// Peak-safe linear makeup gain to reach -14.0 LUFS target.
    #[serde(default = "default_norm_gain")]
    pub norm_gain: f32,
}

fn default_intro_boost() -> f32 {
    1.15
}

fn default_lufs() -> f32 {
    -14.0
}

fn default_norm_gain() -> f32 {
    1.0
}

impl TrackAnalysis {
    pub fn beat_period(&self) -> f32 {
        if self.bpm > 10.0 {
            60.0 / self.bpm
        } else {
            0.5
        }
    }

    /// Returns the continuous beat index at playback time `t_sec` using the AI `beats` grid.
    pub fn continuous_beat_at(&self, t_sec: f64) -> f64 {
        let period = (self.beat_period() as f64).max(0.15);
        if self.beats.len() < 2 {
            return (t_sec - self.first_beat_sec as f64) / period;
        }
        let first = self.beats[0] as f64;
        if t_sec <= first {
            return (t_sec - first) / period;
        }
        let last_idx = self.beats.len() - 1;
        let last = self.beats[last_idx] as f64;
        if t_sec >= last {
            return last_idx as f64 + (t_sec - last) / period;
        }
        let t_f32 = t_sec as f32;
        let idx = self.beats.partition_point(|&b| b <= t_f32).saturating_sub(1);
        let i = idx.min(last_idx - 1);
        let b0 = self.beats[i] as f64;
        let b1 = self.beats[i + 1] as f64;
        let span = (b1 - b0).max(0.05);
        i as f64 + ((t_sec - b0) / span).clamp(0.0, 1.0)
    }

    /// Inverse of `continuous_beat_at`: maps a continuous beat index back to exact track time in seconds.
    pub fn time_at_continuous_beat(&self, beat_pos: f64) -> f64 {
        let period = (self.beat_period() as f64).max(0.15);
        if self.beats.len() < 2 {
            return self.first_beat_sec as f64 + beat_pos * period;
        }
        if beat_pos <= 0.0 {
            return self.beats[0] as f64 + beat_pos * period;
        }
        let last_idx = self.beats.len() - 1;
        if beat_pos >= last_idx as f64 {
            return self.beats[last_idx] as f64 + (beat_pos - last_idx as f64) * period;
        }
        let i = (beat_pos.floor() as usize).min(last_idx - 1);
        let frac = beat_pos - i as f64;
        let b0 = self.beats[i] as f64;
        let b1 = self.beats[i + 1] as f64;
        b0 + frac * (b1 - b0)
    }

    /// Returns the continuous Takt / bar phase in `[0.0, 4.0)` at `t_sec`,
    /// where `0.0` is the Downbeat ("Takt 1"), `1.0` is Beat 2, `2.0` is Beat 3, `3.0` is Beat 4.
    pub fn bar_phase_at(&self, t_sec: f64) -> f64 {
        let cur_beat = self.continuous_beat_at(t_sec);
        if !self.downbeats.is_empty() {
            let t_f32 = (t_sec + 0.03) as f32;
            let db_pos = self.downbeats.partition_point(|&d| d <= t_f32);
            let anchor_db = if db_pos > 0 {
                self.downbeats[db_pos - 1] as f64
            } else {
                self.downbeats[0] as f64
            };
            let anchor_beat = self.continuous_beat_at(anchor_db).round();
            (cur_beat - anchor_beat).rem_euclid(4.0)
        } else {
            cur_beat.rem_euclid(4.0)
        }
    }

    /// Snaps `target_sec` to the nearest AI-detected Downbeat ("Takt 1") within `[min_sec, max_sec]`.
    pub fn snap_to_nearest_downbeat(&self, target_sec: f32, min_sec: f32, max_sec: f32) -> f32 {
        let lo = min_sec.min(max_sec);
        let hi = max_sec.max(min_sec);
        if !self.downbeats.is_empty() {
            let mut best = None;
            let mut best_dist = f32::INFINITY;
            for &db in &self.downbeats {
                if db >= lo && db <= hi {
                    let dist = (db - target_sec).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        best = Some(db);
                    }
                }
            }
            if let Some(d) = best {
                return d;
            }
        }
        if !self.beats.is_empty() {
            let mut best = None;
            let mut best_dist = f32::INFINITY;
            for (idx, &b) in self.beats.iter().enumerate() {
                if idx % 4 == 0 && b >= lo && b <= hi {
                    let dist = (b - target_sec).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        best = Some(b);
                    }
                }
            }
            if let Some(b) = best {
                return b;
            }
        }
        let bar_len = (self.beat_period() * 4.0).max(0.6);
        let bars = ((target_sec - self.first_beat_sec) / bar_len).round().max(0.0);
        (self.first_beat_sec + bars * bar_len).clamp(lo, hi)
    }

    /// Given a base downbeat timestamp in this track and an outgoing track's Takt phase `target_bar_phase` in `[0.0, 4.0)`,
    /// returns the exact timestamp in this track with the identical Takt & beat phase.
    pub fn match_bar_phase_near(&self, base_downbeat_sec: f64, target_bar_phase: f64) -> f64 {
        let downbeat_idx = self.continuous_beat_at(base_downbeat_sec).round();
        let target_beat = downbeat_idx + target_bar_phase.rem_euclid(4.0);
        self.time_at_continuous_beat(target_beat).max(0.0)
    }

    /// Computes harmonic compatibility on the Camelot Wheel between two tracks (`1..=4`).
    /// - `4`: exact same key
    /// - `3`: relative major/minor or adjacent fifth (±1 on the wheel, same mode)
    /// - `2`: ±2 steps on the wheel, same mode
    /// - `1`: otherwise (or if either key is unknown)
    pub fn camelot_compatibility(&self, other: &TrackAnalysis) -> u8 {
        if self.camelot_num == 0
            || other.camelot_num == 0
            || self.camelot_num > 12
            || other.camelot_num > 12
        {
            return 1;
        }
        let diff = self.camelot_num.abs_diff(other.camelot_num);
        let wheel_dist = diff.min(12 - diff);
        if wheel_dist == 0 && self.camelot_major == other.camelot_major {
            4
        } else if (wheel_dist == 0 && self.camelot_major != other.camelot_major)
            || (wheel_dist == 1 && self.camelot_major == other.camelot_major)
        {
            3
        } else if wheel_dist == 2 && self.camelot_major == other.camelot_major {
            2
        } else {
            1
        }
    }
}

#[derive(Debug, Clone)]
pub struct DjSettings {
    pub crossfade_secs: f32,
    pub tempo_sync: bool,
    pub bass_swap: bool,
    pub trim_silence: bool,
    pub normal_short_fade: bool,
}

impl Default for DjSettings {
    fn default() -> Self {
        Self {
            crossfade_secs: 8.0,
            tempo_sync: true,
            bass_swap: true,
            trim_silence: true,
            normal_short_fade: false,
        }
    }
}

/// Information about a track passed to the audio engine.
#[derive(Debug, Clone)]
pub struct AudioTrackSpec {
    pub path: PathBuf,
    pub title: String,
    #[allow(dead_code)]
    pub artist: String,
    pub analysis: Option<TrackAnalysis>,
}

/// Live crossfade status for the Liquid Glass DJ HUD.
#[derive(Debug, Clone, Default)]
pub struct MixStatus {
    pub active: bool,
    pub progress: f32, // 0.0 -> 1.0
    pub outgoing_title: String,
    pub incoming_title: String,
    pub outgoing_bpm: Option<f32>,
    pub incoming_bpm: Option<f32>,
    #[allow(dead_code)]
    pub outgoing_key: Option<String>,
    #[allow(dead_code)]
    pub incoming_key: Option<String>,
    #[allow(dead_code)]
    pub tempo_synced: bool,
    #[allow(dead_code)]
    pub tempo_delta_pct: f32,
}

/// Snapshot sent from the audio engine to the UI every frame.
#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    pub is_playing: bool,
    #[allow(dead_code)]
    pub is_loading: bool,
    pub current_path: Option<PathBuf>,
    pub position_sec: f32,
    pub duration_sec: f32,
    pub planned_mix_out_sec: Option<f32>,
    pub mix: MixStatus,
    pub bass_energy: f32,
    #[allow(dead_code)]
    pub rms_level: f32,
    pub spectrum: [f32; 16],
    pub track_ended_counter: u64,
    pub transitioned_to_next_counter: u64,
    pub last_error: Option<String>,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            is_playing: false,
            is_loading: false,
            current_path: None,
            position_sec: 0.0,
            duration_sec: 0.0,
            planned_mix_out_sec: None,
            mix: MixStatus::default(),
            bass_energy: 0.0,
            rms_level: 0.0,
            spectrum: [0.0; 16],
            track_ended_counter: 0,
            transitioned_to_next_counter: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    PlayNow {
        track: AudioTrackSpec,
        next: Option<AudioTrackSpec>,
        mode: PlaybackMode,
    },
    /// Smoothly crossfade immediately (e.g., when user presses Next in DJ Mix mode or automatic pre-cue).
    CrossfadeTo {
        track: AudioTrackSpec,
        next: Option<AudioTrackSpec>,
        mode: PlaybackMode,
    },
    UpdateNext {
        next: Option<AudioTrackSpec>,
        mode: PlaybackMode,
    },
    UpdateCurrentAnalysis {
        path: PathBuf,
        analysis: TrackAnalysis,
    },
    MigrateTrackPath {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    InvalidatePreload(PathBuf),
    SetPlaying(bool),
    TogglePlay,
    Seek(f32),
    SetVolume(f32),
    SetEq([f32; 5]),
    SetLoudnessNorm(bool),
    SetDjSettings(DjSettings),
    Stop,
}
