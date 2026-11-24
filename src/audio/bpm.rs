use crate::audio::decoder::decode_mono_for_analysis;
use crate::audio::TrackAnalysis;
use crate::config::{normalize_path_key, AppConfig};
use beat_this::{calculate_bpm, BeatThis, Runtime, RtenRuntime};
use crossbeam_channel::{unbounded, Receiver, Sender};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::UNIX_EPOCH;

const CACHE_VERSION: u32 = 10;

const MEL_ONNX_BYTES: &[u8] = include_bytes!("../../models/mel_spectrogram.onnx");
const BEAT_ONNX_BYTES: &[u8] = include_bytes!("../../models/beat_this_small.onnx");

type RtenModel = <RtenRuntime as Runtime>::Model;

fn ensure_beat_this_models() -> Result<(PathBuf, PathBuf), String> {
    let local_dir = Path::new("models");
    let local_mel = local_dir.join("mel_spectrogram.onnx");
    let local_beat = local_dir.join("beat_this_small.onnx");
    if local_mel.is_file() && local_beat.is_file() {
        return Ok((local_mel, local_beat));
    }

    let app_models = AppConfig::app_data_dir().join("models");
    let _ = fs::create_dir_all(&app_models);
    let mel_path = app_models.join("mel_spectrogram.onnx");
    let beat_path = app_models.join("beat_this_small.onnx");

    if !mel_path.is_file() {
        fs::write(&mel_path, MEL_ONNX_BYTES)
            .map_err(|e| format!("Failed to write mel_spectrogram.onnx: {e}"))?;
    }
    if !beat_path.is_file() {
        fs::write(&beat_path, BEAT_ONNX_BYTES)
            .map_err(|e| format!("Failed to write beat_this_small.onnx: {e}"))?;
    }
    Ok((mel_path, beat_path))
}

pub fn create_beat_this_tracker() -> Option<BeatThis<RtenModel>> {
    let (mel_path, beat_path) = ensure_beat_this_models().ok()?;
    BeatThis::new(&RtenRuntime, &mel_path, &beat_path).ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedEntry {
    file_size: u64,
    mtime_secs: u64,
    analysis: TrackAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BpmCacheFile {
    version: u32,
    entries: HashMap<String, CachedEntry>,
}

fn file_signature(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((size, mtime))
}

#[allow(dead_code)]
/// Analyze a single track from disk using the `beat-this` AI beat & downbeat tracker.
pub fn analyze_track(path: &Path) -> Result<TrackAnalysis, String> {
    let mut tracker = create_beat_this_tracker();
    analyze_track_with_tracker(path, tracker.as_mut())
}

pub fn analyze_track_with_tracker(
    path: &Path,
    tracker: Option<&mut BeatThis<RtenModel>>,
) -> Result<TrackAnalysis, String> {
    let (mono, sr, duration_sec) = decode_mono_for_analysis(path)?;
    Ok(analyze_mono_pcm(&mono, sr, duration_sec, tracker))
}

fn analyze_mono_pcm(
    mono: &[f32],
    sr: u32,
    duration_sec: f32,
    tracker: Option<&mut BeatThis<RtenModel>>,
) -> TrackAnalysis {
    let total_samples = mono.len();
    if total_samples < sr as usize * 2 {
        return TrackAnalysis {
            bpm: 120.0,
            confidence: 0.0,
            first_beat_sec: 0.0,
            mix_in_sec: 0.0,
            mix_out_sec: duration_sec.max(0.0),
            energy: 0.5,
            intro_boost: 1.15,
            duration_sec,
            beats: Vec::new(),
            downbeats: Vec::new(),
            camelot_key: String::new(),
            camelot_num: 0,
            camelot_major: false,
            lufs: -14.0,
            norm_gain: 1.0,
        };
    }

    let (lufs, norm_gain) = measure_lufs_and_norm_gain(mono, sr);
    let (camelot_key, camelot_num, camelot_major) = detect_camelot_key(mono, sr);

    // 1. Compute 50ms RMS energy envelope + 500ms smoothed contour for structural phrase cue detection
    let block_len = ((sr as f32 * 0.05) as usize).max(256);
    let mut rms_blocks = Vec::with_capacity(total_samples / block_len + 1);
    for chunk in mono.chunks(block_len) {
        let mut sum_sq = 0.0f64;
        for &s in chunk {
            sum_sq += (s as f64) * (s as f64);
        }
        rms_blocks.push((sum_sq / chunk.len() as f64).sqrt() as f32);
    }

    // 500ms (10-block) moving average so transient dips don't fool phrase boundary detection
    let n_blocks = rms_blocks.len();
    let mut smooth_rms = vec![0.0f32; n_blocks];
    for (i, slot) in smooth_rms.iter_mut().enumerate() {
        let lo = i.saturating_sub(5);
        let hi = (i + 5).min(n_blocks);
        let sum: f32 = rms_blocks[lo..hi].iter().sum();
        *slot = sum / (hi - lo).max(1) as f32;
    }

    let mut sorted_rms = smooth_rms.clone();
    sorted_rms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let body_rms = sorted_rms
        .get((sorted_rms.len() * 75) / 100)
        .copied()
        .unwrap_or(0.1)
        .max(1e-4);

    // Smart Mix-In: skip dead silence AND quiet atmospheric intro fade-in (up to first 14s)
    // so incoming tracks enter the DJ mix with immediate musical presence!
    let max_intro_blocks = ((14.0f32.min(duration_sec * 0.14)) / 0.05) as usize;
    let search_in_end = max_intro_blocks.min(n_blocks);
    let presence_thresh = body_rms * 0.30;
    let audible_thresh = body_rms * 0.07;

    let first_presence_block = smooth_rms[..search_in_end]
        .iter()
        .position(|&r| r >= presence_thresh)
        .or_else(|| {
            rms_blocks[..search_in_end]
                .iter()
                .position(|&r| r >= audible_thresh)
        })
        .unwrap_or(0);
    let raw_mix_in_sec = (first_presence_block as f32 * 0.05).max(0.0);

    // Measure average RMS in the 6s window following raw_mix_in_sec to calculate intro makeup gain
    let intro_end_block = (first_presence_block + 120).min(n_blocks);
    let intro_avg_rms = if intro_end_block > first_presence_block {
        let sum: f32 = smooth_rms[first_presence_block..intro_end_block].iter().sum();
        sum / (intro_end_block - first_presence_block) as f32
    } else {
        body_rms
    };
    let intro_boost = ((body_rms * 0.88) / intro_avg_rms.max(body_rms * 0.42)).clamp(1.0, 1.55);

    // Smart Mix-Out: find the last moment the track still has solid rhythmic body (>= 46% of body_rms)
    // BEFORE the quiet fade-out or reverb tail begins, plus a 2.2s natural tail allowance.
    let min_out_block = ((duration_sec * 0.72) / 0.05) as usize;
    let body_out_thresh = body_rms * 0.46;
    let tail_out_thresh = body_rms * 0.12;

    let last_tail_block = rms_blocks
        .iter()
        .rposition(|&r| r > tail_out_thresh)
        .unwrap_or(n_blocks.saturating_sub(1));
    let tail_end_sec = ((last_tail_block + 1) as f32 * 0.05).min(duration_sec);

    let last_body_block = smooth_rms
        .iter()
        .enumerate()
        .rev()
        .find(|&(idx, &r)| idx >= min_out_block.min(n_blocks.saturating_sub(1)) && r >= body_out_thresh)
        .map(|(idx, _)| idx)
        .unwrap_or(last_tail_block);
    let energetic_end_sec = (last_body_block as f32 * 0.05) + 2.2;
    let mix_out_sec = energetic_end_sec
        .min(tail_end_sec)
        .max(duration_sec * 0.72)
        .min(duration_sec);

    let energy = (body_rms * 3.2).clamp(0.05, 1.0);

    // 2. Primary AI Beat & Downbeat Tracking via `beat-this` (ISMIR 2024 pure-Rust port)
    if let Some(bt) = tracker {
        if let Ok(bt_res) = bt.analyze_audio(mono, sr) {
            if let Some(raw_bpm) = calculate_bpm(&bt_res) {
                if (50.0..=240.0).contains(&raw_bpm) && bt_res.beats.len() >= 4 {
                    let mut bpm = raw_bpm;
                    let nearest_int = bpm.round();
                    if (bpm - nearest_int).abs() < 0.22 {
                        bpm = nearest_int;
                    } else {
                        bpm = (bpm * 10.0).round() / 10.0;
                    }

                    let period_sec = 60.0 / bpm;
                    let first_beat_sec = bt_res
                        .downbeats
                        .first()
                        .or_else(|| bt_res.beats.first())
                        .copied()
                        .unwrap_or(0.0)
                        .rem_euclid(period_sec);

                    let max_in = (duration_sec * 0.20).max(2.0);
                    let snapped_in = bt_res
                        .downbeats
                        .iter()
                        .copied()
                        .find(|&d| d >= raw_mix_in_sec - 0.15 && d <= max_in)
                        .or_else(|| {
                            bt_res
                                .beats
                                .iter()
                                .copied()
                                .find(|&b| b >= raw_mix_in_sec - 0.10 && b <= max_in)
                        })
                        .unwrap_or(raw_mix_in_sec);

                    return TrackAnalysis {
                        bpm,
                        confidence: 0.98,
                        first_beat_sec,
                        mix_in_sec: snapped_in.max(0.0),
                        mix_out_sec,
                        energy,
                        intro_boost,
                        duration_sec,
                        beats: bt_res.beats,
                        downbeats: bt_res.downbeats,
                        camelot_key,
                        camelot_num,
                        camelot_major,
                        lufs,
                        norm_gain,
                    };
                }
            }
        }
    }

    // 3. Fallback: Analyze up to 160s across the core of the track (10% .. 90%)
    let max_window_sec = 160.0f32;
    let (win_start_sec, win_len_sec) = if duration_sec <= max_window_sec {
        (0.0, duration_sec)
    } else {
        let start = (duration_sec * 0.12).min(duration_sec - max_window_sec);
        (start, max_window_sec)
    };
    let start_idx = ((win_start_sec * sr as f32) as usize).min(total_samples);
    let end_idx = (((win_start_sec + win_len_sec) * sr as f32) as usize).min(total_samples);
    let slice = &mono[start_idx..end_idx];

    // 3. Multi-band log-compressed Spectral Flux (FFT 1024, hop 256)
    const FFT_SIZE: usize = 1024;
    const HOP_SIZE: usize = 256;
    if slice.len() < FFT_SIZE * 4 {
        return TrackAnalysis {
            bpm: 120.0,
            confidence: 0.0,
            first_beat_sec: 0.0,
            mix_in_sec: raw_mix_in_sec,
            mix_out_sec,
            energy,
            intro_boost,
            duration_sec,
            beats: Vec::new(),
            downbeats: Vec::new(),
            camelot_key,
            camelot_num,
            camelot_major,
            lufs,
            norm_gain,
        };
    }

    let num_frames = (slice.len() - FFT_SIZE) / HOP_SIZE + 1;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

    let hz_per_bin = sr as f32 / FFT_SIZE as f32;
    let bin_low_min = (35.0 / hz_per_bin).floor() as usize;
    let bin_low_max = (180.0 / hz_per_bin).ceil() as usize;
    let bin_mid_max = (2400.0 / hz_per_bin).ceil() as usize;
    let bin_hi_max = ((8000.0 / hz_per_bin).ceil() as usize).min(FFT_SIZE / 2);

    let mut buf = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
    let mut prev_log_mag = vec![0.0f32; FFT_SIZE / 2 + 1];
    let mut onset_total = Vec::with_capacity(num_frames);
    let mut onset_kick = Vec::with_capacity(num_frames);

    for f in 0..num_frames {
        let base = f * HOP_SIZE;
        for i in 0..FFT_SIZE {
            buf[i] = Complex::new(slice[base + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);

        let mut flux_low = 0.0f32;
        let mut flux_mid = 0.0f32;
        let mut flux_hi = 0.0f32;

        for b in bin_low_min..=bin_hi_max {
            let mag = (buf[b].re * buf[b].re + buf[b].im * buf[b].im).sqrt();
            let log_m = (1.0 + 12.0 * mag).ln();
            let diff = (log_m - prev_log_mag[b]).max(0.0);
            prev_log_mag[b] = log_m;

            if b <= bin_low_max {
                flux_low += diff;
            } else if b <= bin_mid_max {
                flux_mid += diff;
            } else {
                flux_hi += diff;
            }
        }

        let norm_low = flux_low / (bin_low_max.saturating_sub(bin_low_min).max(1) as f32);
        let norm_mid = flux_mid / (bin_mid_max.saturating_sub(bin_low_max).max(1) as f32);
        let norm_hi = flux_hi / (bin_hi_max.saturating_sub(bin_mid_max).max(1) as f32);

        onset_kick.push(norm_low);
        onset_total.push(2.4 * norm_low + 1.15 * norm_mid + 0.55 * norm_hi);
    }

    // Adaptive local mean subtraction + normalization
    let onset = adaptive_whiten(&onset_total, 8);
    let kick_onset = adaptive_whiten(&onset_kick, 8);

    // 4. Short-Time Windowed Tempogram Autocorrelation (Ellis 2007 / Librosa approach)
    // Averaging autocorrelation over 8-second overlapping windows (hop 2s) preserves sharp beat peaks
    // even on 60s/70s/80s live-drummer recordings with slight human tempo drift!
    let frame_rate = sr as f32 / HOP_SIZE as f32; // ~86.1328 Hz
    let ac = windowed_tempogram_autocorrelation(&onset, 688, 172);
    let kick_ac = windowed_tempogram_autocorrelation(&kick_onset, 688, 172);

    if ac.len() < 300 {
        return TrackAnalysis {
            bpm: 120.0,
            confidence: 0.0,
            first_beat_sec: 0.0,
            mix_in_sec: raw_mix_in_sec,
            mix_out_sec,
            energy,
            intro_boost,
            duration_sec,
            beats: Vec::new(),
            downbeats: Vec::new(),
            camelot_key,
            camelot_num,
            camelot_major,
            lufs,
            norm_gain,
        };
    }

    let sample_ac = |arr: &[f32], lag: f32| -> f32 {
        if lag < 1.0 || (lag as usize) + 1 >= arr.len() {
            return 0.0;
        }
        let i = lag.floor() as usize;
        let f = lag - i as f32;
        arr[i] * (1.0 - f) + arr[i + 1] * f
    };

    // Continuous fractional-lag 4/4 harmonic comb over [70.0 .. 206.0] BPM.
    let eval_comb = |bpm_cand: f32| -> f32 {
        let t = frame_rate * 60.0 / bpm_cand;
        let a1 = sample_ac(&ac, t);
        let a2 = sample_ac(&ac, t * 2.0);
        let a4 = sample_ac(&ac, t * 4.0);

        let k1 = sample_ac(&kick_ac, t);
        let k2 = sample_ac(&kick_ac, t * 2.0);
        let k4 = sample_ac(&kick_ac, t * 4.0);

        let comb_all = 1.15 * a1 + 0.65 * a2 + 0.45 * a4;
        let comb_kick = 1.25 * k1 + 0.60 * k2 + 0.40 * k4;
        let raw = comb_all + 0.65 * comb_kick;

        // Broad psychoacoustic tempo prior centered at 126 BPM (supports 75..205 BPM cleanly)
        let log_dist = (bpm_cand / 126.0).ln();
        let prior = 0.80 + 0.20 * (-0.5 * (log_dist / 0.46).powi(2)).exp();
        raw * prior
    };

    let mut best_bpm = 120.0f32;
    let mut best_score = -1.0f32;
    let mut cand = 74.0f32;
    while cand <= 215.0 {
        let score = eval_comb(cand);
        if score > best_score {
            best_score = score;
            best_bpm = cand;
        }
        cand += 0.25;
    }

    // 5. Bidirectional Octave Disambiguation (74..106 BPM vs 148..215 BPM)
    if best_bpm > 154.0 {
        let half_bpm = best_bpm * 0.5;
        let t_fast = frame_rate * 60.0 / best_bpm;
        let t_slow = frame_rate * 60.0 / half_bpm;
        let fast_kick = sample_ac(&kick_ac, t_fast);
        let slow_kick = sample_ac(&kick_ac, t_slow);
        let fast_all = sample_ac(&ac, t_fast);
        let slow_all = sample_ac(&ac, t_slow);
        if (slow_kick >= fast_kick * 0.96 || slow_all >= fast_all * 1.02) && energy < 0.84 {
            best_bpm = half_bpm;
        }
    } else if best_bpm < 98.0 {
        let dbl_bpm = best_bpm * 2.0;
        let t_slow = frame_rate * 60.0 / best_bpm;

        if dbl_bpm <= 215.0 {
            let t_fast = frame_rate * 60.0 / dbl_bpm;
            let fast_kick = sample_ac(&kick_ac, t_fast);
            let slow_kick = sample_ac(&kick_ac, t_slow);
            let fast_all = sample_ac(&ac, t_fast);
            let slow_all = sample_ac(&ac, t_slow);
            if (fast_all >= slow_all * 0.54 && energy > 0.84)
                || (fast_kick >= slow_kick * 0.80 && fast_all >= slow_all * 0.72)
            {
                best_bpm = dbl_bpm;
            }
        }
    }

    // Fine micro-sweep around best_bpm (±0.5 BPM in 0.05 BPM steps)
    let mut refined_bpm = best_bpm;
    let mut refined_score = -1.0f32;
    let mut c = (best_bpm - 0.5).max(74.0);
    let hi = (best_bpm + 0.5).min(215.0);
    while c <= hi {
        let sc = eval_comb(c);
        if sc > refined_score {
            refined_score = sc;
            refined_bpm = c;
        }
        c += 0.05;
    }

    let mut bpm = refined_bpm.clamp(74.0, 215.0);
    let nearest_int = bpm.round();
    if (bpm - nearest_int).abs() < 0.22 {
        bpm = nearest_int;
    } else {
        bpm = (bpm * 10.0).round() / 10.0;
    }

    let final_lag = frame_rate * 60.0 / bpm;
    let confidence = (sample_ac(&ac, final_lag).clamp(0.0, 1.0) * 1.4).min(1.0);

    // 6. Beat Phase Alignment: find exact phase phi in [0, T) aligning with kicks/onsets
    let period_frames = frame_rate * 60.0 / bpm;
    let steps = 48usize;
    let mut best_phase_frames = 0.0f32;
    let mut best_phase_val = -1.0f32;
    let n_f = onset.len();

    for s in 0..steps {
        let phi = (s as f32 / steps as f32) * period_frames;
        let mut sum = 0.0f32;
        let mut count = 0u32;
        let mut pos = phi;
        while (pos as usize) < n_f {
            let idx = pos as usize;
            // Weight kick onset heavily so phase locks right on the downbeat kick!
            let val = onset[idx] + 1.5 * kick_onset[idx];
            sum += val;
            count += 1;
            pos += period_frames;
        }
        if count > 0 {
            let avg = sum / count as f32;
            if avg > best_phase_val {
                best_phase_val = avg;
                best_phase_frames = phi;
            }
        }
    }

    // Convert window-relative beat phase back to track-absolute phase in [0, period_sec)
    let period_sec = 60.0 / bpm;
    let abs_beat_sec = win_start_sec + best_phase_frames / frame_rate;
    let first_beat_sec = abs_beat_sec.rem_euclid(period_sec);

    // Snap mix_in_sec cleanly to the nearest beat at or just before raw_mix_in_sec
    let beat_idx = ((raw_mix_in_sec - first_beat_sec) / period_sec).floor().max(0.0);
    let mix_in_sec = (first_beat_sec + beat_idx * period_sec)
        .max(0.0)
        .min((duration_sec * 0.16).max(0.0));

    let mut synth_beats = Vec::new();
    let mut synth_downbeats = Vec::new();
    let mut t_b = first_beat_sec;
    let mut idx_b = 0usize;
    while t_b < duration_sec {
        synth_beats.push(t_b);
        if idx_b.is_multiple_of(4) {
            synth_downbeats.push(t_b);
        }
        t_b += period_sec;
        idx_b += 1;
    }

    TrackAnalysis {
        bpm,
        confidence,
        first_beat_sec,
        mix_in_sec,
        mix_out_sec,
        energy,
        intro_boost,
        duration_sec,
        beats: synth_beats,
        downbeats: synth_downbeats,
        camelot_key,
        camelot_num,
        camelot_major,
        lufs,
        norm_gain,
    }
}

/// Measures ITU-R BS.1770-4 integrated loudness (LUFS) with 2-stage K-weighting biquad cascade
/// and dual absolute/relative gating, returning `(lufs, peak_safe_norm_gain)` targeting `-14.0 LUFS`.
fn measure_lufs_and_norm_gain(mono: &[f32], sr: u32) -> (f32, f32) {
    if sr == 0 || mono.is_empty() {
        return (-14.0, 1.0);
    }

    let fs = sr as f64;

    // Stage 1: RBJ High-Shelf +4.0 dB at 1500.0 Hz, Q = 0.7071
    let f1 = 1500.0f64.clamp(20.0, fs * 0.45);
    let a_shelf = 10.0f64.powf(4.0 / 40.0);
    let w1 = 2.0 * std::f64::consts::PI * f1 / fs;
    let cos1 = w1.cos();
    let sin1 = w1.sin();
    let alpha1 = sin1 / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
    let two_sqrt_a_alpha1 = 2.0 * a_shelf.sqrt() * alpha1;
    let s1_a0 = (a_shelf + 1.0) - (a_shelf - 1.0) * cos1 + two_sqrt_a_alpha1;
    let s1_b0 = (a_shelf * ((a_shelf + 1.0) + (a_shelf - 1.0) * cos1 + two_sqrt_a_alpha1)) / s1_a0;
    let s1_b1 = (-2.0 * a_shelf * ((a_shelf - 1.0) + (a_shelf + 1.0) * cos1)) / s1_a0;
    let s1_b2 = (a_shelf * ((a_shelf + 1.0) + (a_shelf - 1.0) * cos1 - two_sqrt_a_alpha1)) / s1_a0;
    let s1_a1 = (2.0 * ((a_shelf - 1.0) - (a_shelf + 1.0) * cos1)) / s1_a0;
    let s1_a2 = ((a_shelf + 1.0) - (a_shelf - 1.0) * cos1 - two_sqrt_a_alpha1) / s1_a0;

    // Stage 2: RBJ High-Pass at 38.135 Hz, Q = 0.5003
    let f2 = 38.135f64.clamp(10.0, fs * 0.45);
    let w2 = 2.0 * std::f64::consts::PI * f2 / fs;
    let cos2 = w2.cos();
    let sin2 = w2.sin();
    let alpha2 = sin2 / (2.0 * 0.5003);
    let s2_a0 = 1.0 + alpha2;
    let s2_b0 = ((1.0 + cos2) * 0.5) / s2_a0;
    let s2_b1 = -(1.0 + cos2) / s2_a0;
    let s2_b2 = ((1.0 + cos2) * 0.5) / s2_a0;
    let s2_a1 = (-2.0 * cos2) / s2_a0;
    let s2_a2 = (1.0 - alpha2) / s2_a0;

    // 100 ms hop sub-blocks; 4 consecutive sub-blocks form a 400 ms block (75% overlap).
    let hop_len = ((fs * 0.1).round() as usize).max(1);
    let block_len = hop_len * 4;
    let mut sub_sums: Vec<f64> = Vec::with_capacity(mono.len() / hop_len + 1);
    let mut sub_acc = 0.0f64;
    let mut sub_cnt = 0usize;
    let mut total_sq = 0.0f64;

    // Direct Form II Transposed filter states for both stages
    let (mut z1_1, mut z1_2) = (0.0f64, 0.0f64);
    let (mut z2_1, mut z2_2) = (0.0f64, 0.0f64);

    for &sample in mono {
        let x = sample as f64;
        // Stage 1: High-Shelf
        let y1 = s1_b0 * x + z1_1;
        z1_1 = s1_b1 * x - s1_a1 * y1 + z1_2;
        z1_2 = s1_b2 * x - s1_a2 * y1;
        // Stage 2: High-Pass
        let y2 = s2_b0 * y1 + z2_1;
        z2_1 = s2_b1 * y1 - s2_a1 * y2 + z2_2;
        z2_2 = s2_b2 * y1 - s2_a2 * y2;

        let sq = y2 * y2;
        total_sq += sq;
        sub_acc += sq;
        sub_cnt += 1;
        if sub_cnt == hop_len {
            sub_sums.push(sub_acc);
            sub_acc = 0.0;
            sub_cnt = 0;
        }
    }

    let mut blocks: Vec<f64> = Vec::new();
    if sub_sums.len() >= 4 {
        blocks.reserve(sub_sums.len() - 3);
        let inv_block = 1.0 / (block_len as f64);
        for w in sub_sums.windows(4) {
            let z_j = (w[0] + w[1] + w[2] + w[3]) * inv_block;
            blocks.push(z_j);
        }
    } else {
        blocks.push(total_sq / (mono.len() as f64));
    }

    // Absolute gate: > -70.0 LUFS
    let abs_gated: Vec<f64> = blocks
        .into_iter()
        .filter(|&z_j| -0.691 + 10.0 * z_j.max(1e-12).log10() > -70.0)
        .collect();

    if abs_gated.is_empty() {
        return (-14.0, 1.0);
    }

    // Relative gate: > gamma_r = L_abs - 10.0 LU
    let z_mean = abs_gated.iter().sum::<f64>() / (abs_gated.len() as f64);
    let gamma_r = -0.691 + 10.0 * z_mean.max(1e-12).log10() - 10.0;

    let mut rel_sum = 0.0f64;
    let mut rel_cnt = 0usize;
    for z_j in abs_gated {
        if -0.691 + 10.0 * z_j.max(1e-12).log10() > gamma_r {
            rel_sum += z_j;
            rel_cnt += 1;
        }
    }

    let z_final = if rel_cnt > 0 {
        rel_sum / (rel_cnt as f64)
    } else {
        z_mean
    };

    let lufs = (-0.691 + 10.0 * z_final.max(1e-12).log10()).clamp(-45.0, -2.0) as f32;
    let peak = mono
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max)
        .max(0.05);
    let raw_gain = 10.0f32.powf((-14.0 - lufs) / 20.0);
    let max_safe = (0.96 / peak).max(0.35);
    let norm_gain = raw_gain.clamp(0.35, 2.2).min(max_safe);

    (lufs, norm_gain)
}

/// Detects the musical key of mono PCM audio using a 4096-point STFT Pitch Class Profile
/// correlated against the Krumhansl-Kessler Major & Minor key profiles, mapped to Camelot Wheel notation.
fn detect_camelot_key(mono: &[f32], sr: u32) -> (String, u8, bool) {
    const FFT_SIZE: usize = 4096;
    const HOP_SIZE: usize = 2048;

    if sr == 0 || mono.len() < FFT_SIZE {
        return (String::new(), 0, false);
    }

    // Take up to 60 seconds from the middle of the track
    let max_samples = (sr as usize).saturating_mul(60);
    let slice = if mono.len() <= max_samples {
        mono
    } else {
        let start = (mono.len() - max_samples) / 2;
        &mono[start..start + max_samples]
    };

    if slice.len() < FFT_SIZE {
        return (String::new(), 0, false);
    }

    let num_frames = (slice.len() - FFT_SIZE) / HOP_SIZE + 1;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

    let hz_per_bin = sr as f32 / FFT_SIZE as f32;
    let bin_min = ((65.0 / hz_per_bin).ceil() as usize).max(1);
    let bin_max = ((1800.0 / hz_per_bin).floor() as usize).min(FFT_SIZE / 2);
    if bin_min > bin_max {
        return (String::new(), 0, false);
    }

    let bin_pcs: Vec<usize> = (bin_min..=bin_max)
        .map(|b| {
            let freq = b as f32 * hz_per_bin;
            let midi = (12.0 * (freq / 440.0).log2() + 69.0).round() as i32;
            midi.rem_euclid(12) as usize
        })
        .collect();

    let mut buf = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
    let mut chroma = [0.0f64; 12];

    for f in 0..num_frames {
        let base = f * HOP_SIZE;
        for i in 0..FFT_SIZE {
            buf[i] = Complex::new(slice[base + i] * hann[i], 0.0);
        }
        fft.process(&mut buf);

        for (offset, b) in (bin_min..=bin_max).enumerate() {
            let mag = (buf[b].re * buf[b].re + buf[b].im * buf[b].im).sqrt();
            let pc = bin_pcs[offset];
            chroma[pc] += (mag as f64).sqrt();
        }
    }

    let c_mean = chroma.iter().sum::<f64>() / 12.0;
    let c_var: f64 = chroma.iter().map(|&v| (v - c_mean).powi(2)).sum();
    if c_var <= 1e-12 {
        return (String::new(), 0, false);
    }

    const MAJOR_PROFILE: [f64; 12] = [
        6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
    ];
    const MINOR_PROFILE: [f64; 12] = [
        6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
    ];

    let correlate = |profile: &[f64; 12], root: usize| -> f64 {
        let p_mean = profile.iter().sum::<f64>() / 12.0;
        let p_var: f64 = profile.iter().map(|&v| (v - p_mean).powi(2)).sum();
        let denom = (c_var * p_var).sqrt();
        if denom <= 1e-12 {
            return f64::NEG_INFINITY;
        }
        let mut num = 0.0f64;
        for i in 0..12 {
            num += (chroma[(root + i) % 12] - c_mean) * (profile[i] - p_mean);
        }
        num / denom
    };

    let mut best_root = 0usize;
    let mut best_major = true;
    let mut best_corr = f64::NEG_INFINITY;

    for root in 0..12 {
        let maj_c = correlate(&MAJOR_PROFILE, root);
        if maj_c > best_corr {
            best_corr = maj_c;
            best_root = root;
            best_major = true;
        }
        let min_c = correlate(&MINOR_PROFILE, root);
        if min_c > best_corr {
            best_corr = min_c;
            best_root = root;
            best_major = false;
        }
    }

    const CAMELOT_MAJOR: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
    const CAMELOT_MINOR: [u8; 12] = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

    let num = if best_major {
        CAMELOT_MAJOR[best_root]
    } else {
        CAMELOT_MINOR[best_root]
    };
    let label = format!("{}{}", num, if best_major { "B" } else { "A" });
    (label, num, best_major)
}

fn adaptive_whiten(signal: &[f32], half_win: usize) -> Vec<f32> {
    let n = signal.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + signal[i] as f64;
    }

    let mut max_val = 1e-6f32;
    for i in 0..n {
        let lo = i.saturating_sub(half_win);
        let hi = (i + half_win + 1).min(n);
        let mean = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
        let v = (signal[i] - mean * 1.05).max(0.0);
        out[i] = v;
        if v > max_val {
            max_val = v;
        }
    }
    for v in &mut out {
        *v /= max_val;
    }
    out
}

fn windowed_tempogram_autocorrelation(signal: &[f32], win_len: usize, hop: usize) -> Vec<f32> {
    if signal.len() <= win_len {
        return normalized_autocorrelation(signal);
    }
    let mut windows: Vec<(f32, usize)> = Vec::new();
    let mut start = 0usize;
    while start + win_len <= signal.len() {
        let slice = &signal[start..start + win_len];
        let energy: f32 = slice.iter().sum::<f32>() / win_len as f32;
        if energy > 0.03 {
            windows.push((energy, start));
        }
        start += hop;
    }
    if windows.is_empty() {
        return normalized_autocorrelation(signal);
    }
    // Sort descending by percussive onset density and keep the top 45% most rhythmic windows (drops & choruses!)
    windows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let keep = ((windows.len() * 45) / 100).max(2).min(windows.len());

    let mut avg = vec![0.0f32; win_len];
    let mut weight_sum = 0.0f32;
    for &(w, st) in &windows[..keep] {
        let slice = &signal[st..st + win_len];
        let ac = normalized_autocorrelation(slice);
        for i in 0..win_len.min(ac.len()) {
            avg[i] += ac[i] * w;
        }
        weight_sum += w;
    }
    if weight_sum > 1e-6 {
        for v in &mut avg {
            *v /= weight_sum;
        }
    }
    avg
}

fn normalized_autocorrelation(signal: &[f32]) -> Vec<f32> {
    let n = signal.len();
    if n < 8 {
        return vec![0.0; n];
    }
    let mut prefix_sq = vec![0.0f64; n + 1];
    for i in 0..n {
        let s = signal[i] as f64;
        prefix_sq[i + 1] = prefix_sq[i] + s * s;
    }

    let fft_len = (n * 2).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(fft_len);
    let inv = planner.plan_fft_inverse(fft_len);

    let mut buf = vec![Complex::new(0.0f32, 0.0); fft_len];
    for (i, &v) in signal.iter().enumerate() {
        buf[i] = Complex::new(v, 0.0);
    }
    fwd.process(&mut buf);
    for c in &mut buf {
        let mag2 = c.re * c.re + c.im * c.im;
        *c = Complex::new(mag2, 0.0);
    }
    inv.process(&mut buf);

    let scale = 1.0 / fft_len as f64;
    let mut out = vec![0.0f32; n];
    for lag in 0..n {
        let e_left = prefix_sq[n - lag];
        let e_right = prefix_sq[n] - prefix_sq[lag];
        let denom = (e_left * e_right).sqrt();
        if denom > 1e-9 {
            let corr = (buf[lag].re as f64 * scale) / denom;
            out[lag] = (corr as f32).clamp(0.0, 1.0);
        }
    }
    out
}

/// Result message pushed from background BPM analyzer workers to UI.
pub struct BpmUpdate {
    pub path: PathBuf,
    pub analysis: TrackAnalysis,
}

/// Manages persistent BPM cache + multi-threaded background analysis workers.
pub struct BpmManager {
    cache: BpmCacheFile,
    job_tx: Sender<PathBuf>,
    result_rx: Receiver<BpmUpdate>,
    in_flight: HashSet<String>,
    pending_count: Arc<AtomicUsize>,
    total_batch: usize,
    completed_batch: usize,
    dirty_since_save: usize,
}

impl BpmManager {
    pub fn new() -> Self {
        let cache = Self::load_cache();
        let (job_tx, job_rx) = unbounded::<PathBuf>();
        let (result_tx, result_rx) = unbounded::<BpmUpdate>();
        let pending_count = Arc::new(AtomicUsize::new(0));

        // Spawn 2 background beat-this analyzer threads (each loads the ONNX models once!)
        let workers = 2usize;

        for idx in 0..workers {
            let rx = job_rx.clone();
            let tx = result_tx.clone();
            let pending = pending_count.clone();
            let _ = thread::Builder::new()
                .name(format!("rustify-bpm-{idx}"))
                .spawn(move || {
                    let mut tracker = create_beat_this_tracker();
                    while let Ok(path) = rx.recv() {
                        if let Ok(analysis) = analyze_track_with_tracker(&path, tracker.as_mut()) {
                            let _ = tx.send(BpmUpdate { path, analysis });
                        }
                        pending.fetch_sub(1, Ordering::Relaxed);
                    }
                });
        }

        Self {
            cache,
            job_tx,
            result_rx,
            in_flight: HashSet::new(),
            pending_count,
            total_batch: 0,
            completed_batch: 0,
            dirty_since_save: 0,
        }
    }

    fn load_cache() -> BpmCacheFile {
        let path = AppConfig::bpm_cache_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(file) = serde_json::from_str::<BpmCacheFile>(&raw) {
                if file.version == CACHE_VERSION {
                    return file;
                }
            }
        }
        BpmCacheFile {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        }
    }

    pub fn save_cache(&mut self) {
        let dir = AppConfig::app_data_dir();
        let _ = fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string(&self.cache) {
            let _ = fs::write(AppConfig::bpm_cache_path(), json);
        }
        self.dirty_since_save = 0;
    }

    pub fn get_cached(&self, path: &Path) -> Option<TrackAnalysis> {
        let key = normalize_path_key(path);
        let entry = self.cache.entries.get(&key)?;
        let (size, mtime) = file_signature(path)?;
        if entry.file_size == size && entry.mtime_secs == mtime {
            Some(entry.analysis.clone())
        } else {
            None
        }
    }

    pub fn migrate_track_path(&mut self, old_path: &Path, new_path: &Path) {
        let old_key = normalize_path_key(old_path);
        let new_key = normalize_path_key(new_path);
        if let Some(mut entry) = self.cache.entries.remove(&old_key) {
            if let Some((size, mtime)) = file_signature(new_path) {
                entry.file_size = size;
                entry.mtime_secs = mtime;
            }
            self.cache.entries.insert(new_key, entry);
            self.dirty_since_save += 1;
        }
        self.in_flight.remove(&old_key);
    }

    pub fn remove_track(&mut self, path: &Path) {
        let key = normalize_path_key(path);
        if self.cache.entries.remove(&key).is_some() {
            self.dirty_since_save += 1;
        }
        self.in_flight.remove(&key);
    }

    pub fn enqueue_if_needed(&mut self, path: &Path) {
        if self.get_cached(path).is_some() {
            return;
        }
        self.force_enqueue(path);
    }

    pub fn force_enqueue(&mut self, path: &Path) {
        let key = normalize_path_key(path);
        if self.in_flight.contains(&key) {
            return;
        }
        self.in_flight.insert(key);
        self.pending_count.fetch_add(1, Ordering::Relaxed);
        self.total_batch += 1;
        let _ = self.job_tx.send(path.to_path_buf());
    }

    pub fn clear_cache_and_reanalyze(&mut self, paths: &[PathBuf]) {
        self.cache.entries.clear();
        self.in_flight.clear();
        self.total_batch = 0;
        self.completed_batch = 0;
        self.save_cache();
        for p in paths {
            self.force_enqueue(p);
        }
    }

    pub fn poll_updates(&mut self) -> Vec<BpmUpdate> {
        let mut updates = Vec::new();
        while let Ok(upd) = self.result_rx.try_recv() {
            let key = normalize_path_key(&upd.path);
            self.in_flight.remove(&key);
            if let Some((size, mtime)) = file_signature(&upd.path) {
                self.cache.entries.insert(
                    key,
                    CachedEntry {
                        file_size: size,
                        mtime_secs: mtime,
                        analysis: upd.analysis.clone(),
                    },
                );
                self.dirty_since_save += 1;
            }
            self.completed_batch += 1;
            updates.push(upd);
        }

        if self.dirty_since_save >= 8 || (self.dirty_since_save > 0 && !self.is_analyzing()) {
            self.save_cache();
        }
        if !self.is_analyzing() && self.total_batch > 0 {
            self.total_batch = 0;
            self.completed_batch = 0;
        }
        updates
    }

    pub fn is_analyzing(&self) -> bool {
        self.pending_count.load(Ordering::Relaxed) > 0
    }

    /// Returns `(completed, total)` while an analysis batch is active.
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.is_analyzing() && self.total_batch > 0 {
            Some((self.completed_batch.min(self.total_batch), self.total_batch))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dj::{build_dj_sorted_indices, compute_tempo_match};
    use crate::library::parse_title_artist;

    #[test]
    fn test_filename_parsing_and_nfc() {
        let (t, a) = parse_title_artist("3 Haselnu\u{0308}sse - Jaques Raupe\u{0301} x Felix Harrer");
        assert_eq!(t, "3 Haselnüsse");
        assert_eq!(a, "Jaques Raupé x Felix Harrer");

        let (t2, a2) = parse_title_artist("Krach Fiesta");
        assert_eq!(t2, "Krach Fiesta");
        assert_eq!(a2, "");
    }

    #[test]
    fn test_tempo_match_and_dj_sort() {
        // 150 -> 156 BPM is +4% (syncable!)
        let m1 = compute_tempo_match(150.0, 156.0);
        assert!(m1.can_tempo_sync);
        // Half/double time: 75 -> 150 BPM should match ~1.0x!
        let m2 = compute_tempo_match(150.0, 75.0);
        assert!(m2.can_tempo_sync);
        assert!((m2.incoming_start_rate - 1.0).abs() < 0.01);

        let fake = |bpm: f32, energy: f32| {
            Some(TrackAnalysis {
                bpm,
                confidence: 0.9,
                first_beat_sec: 0.1,
                mix_in_sec: 0.2,
                mix_out_sec: 180.0,
                energy,
                intro_boost: 1.15,
                duration_sec: 185.0,
                beats: vec![0.1, 0.6, 1.1, 1.6],
                downbeats: vec![0.1],
                camelot_key: String::new(),
                camelot_num: 0,
                camelot_major: false,
                lufs: -14.0,
                norm_gain: 1.0,
            })
        };
        let order = build_dj_sorted_indices(&[fake(174.0, 0.8), fake(128.0, 0.6), fake(150.0, 0.9)]);
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn test_lufs_measurement_and_peak_safety() {
        let sr = 44100u32;
        let len = sr as usize * 3;
        // 1 kHz sine at -20 dBFS RMS (amplitude ~0.1414) -> quiet track should be boosted (> 1.0)
        let quiet: Vec<f32> = (0..len)
            .map(|i| 0.1414 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin())
            .collect();
        let (lufs_q, gain_q) = measure_lufs_and_norm_gain(&quiet, sr);
        assert!(lufs_q < -16.0 && lufs_q > -24.0, "unexpected quiet LUFS: {lufs_q}");
        assert!(gain_q > 1.2 && gain_q <= 2.2, "unexpected quiet gain: {gain_q}");

        // Loud 1 kHz sine at near-0 dBFS (amplitude 0.95) -> loud track should be attenuated (< 1.0)
        let loud: Vec<f32> = (0..len)
            .map(|i| 0.95 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin())
            .collect();
        let (lufs_l, gain_l) = measure_lufs_and_norm_gain(&loud, sr);
        assert!(lufs_l > -8.0, "unexpected loud LUFS: {lufs_l}");
        assert!((0.35..0.6).contains(&gain_l), "unexpected loud gain: {gain_l}");
        assert!(0.95 * gain_l <= 0.961, "peak safety violated: {}", 0.95 * gain_l);
    }

    #[test]
    fn test_real_tracks_bpm_calibration() {
        let samples = [
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Irgendwie · Irgendwo · Irgendwann - NENA.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Come and Get Your Love - Guardian's of the Galaxy.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Gangsta's Paradise - Coolio.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\I'm Still Standing - Elton John.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Wake Me Up Before You Go-Go - Wham!.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Völlig losgelöst - Major Tom.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Alles neu - Peter Fox.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Blue Monday 88 - New Order.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Baddadan - Chase & Status, Bou.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Breakaway - Martin Garrix & Mesto.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\The Real Slim Shady - Eminem.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Ilay\Push Up - Creeds.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\Lose My Mind - Brennan Heart & Wildstylez.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\HALO - Sub Zero Project.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\Judgement Day - Hardwell & Sub Zero Project.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\Die Young - Lil Texas.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\Feel The Ground Shake - Dimitri K.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\FA TI CO but it's 2026 - Lekkerfaces & Mind Compressor.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\Steve Aoki & Tony Junior - Kids.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\WHAT THA HELLY - Sickmode & Krowdexx x Levenkhan.mp3",
            r"c:\Users\Ilay\Desktop\Musik\Rustify\Hardstyle\REAL INSANITY - Lekkerfaces & TOZA & Teal.mp3",
        ];
        for p in samples {
            let path = Path::new(p);
            if path.exists() {
                let res = analyze_track(path).expect("analysis should succeed");
                println!(
                    "{:55} => {:5.1} BPM (conf={:.2}, beat0={:.3}s, in={:.2}s, out={:.2}s/{:.1}s, E={:.2})",
                    path.file_name().unwrap().to_string_lossy(),
                    res.bpm,
                    res.confidence,
                    res.first_beat_sec,
                    res.mix_in_sec,
                    res.mix_out_sec,
                    res.duration_sec,
                    res.energy
                );
                assert!(res.bpm >= 68.0 && res.bpm <= 215.0);
                assert!(res.duration_sec > 30.0);
            }
        }
    }
}
