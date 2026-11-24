use crate::audio::{DjSettings, TrackAnalysis};

#[derive(Debug, Clone, Copy)]
pub struct TempoMatch {
    /// Initial playback rate for incoming Deck B so its beats match outgoing Deck A's tempo.
    pub incoming_start_rate: f64,
    /// Beat count multiplier (`1.0` for 1:1 BPM match, `2.0` if Deck B is double-time, `0.5` if half-time).
    pub beat_scale: f64,
    /// Relative tempo change in percent (e.g. +2.4 means Deck B speeds up 2.4% at transition start).
    pub percent_diff: f32,
    /// True when the two BPMs (or half/double-time equivalents) are within club DJ pitch range (±16%).
    pub can_tempo_sync: bool,
}

/// Computes the cleanest DJ tempo-sync ratio between outgoing `bpm_a` and incoming `bpm_b`,
/// accounting for 1x, 2x (double-time) and 0.5x (half-time) harmonic tempo locks.
pub fn compute_tempo_match(bpm_a: f32, bpm_b: f32) -> TempoMatch {
    if bpm_a < 40.0 || bpm_b < 40.0 {
        return TempoMatch {
            incoming_start_rate: 1.0,
            beat_scale: 1.0,
            percent_diff: 0.0,
            can_tempo_sync: false,
        };
    }

    let candidates = [
        (bpm_a as f64 / bpm_b as f64, 1.0),
        (bpm_a as f64 / (bpm_b as f64 * 2.0), 0.5),
        ((bpm_a as f64 * 2.0) / bpm_b as f64, 2.0),
    ];

    let mut best_rate = candidates[0].0;
    let mut best_scale = candidates[0].1;
    let mut best_err = (best_rate - 1.0).abs();
    for &(rate, scale) in &candidates[1..] {
        let err = (rate - 1.0).abs();
        if err < best_err {
            best_rate = rate;
            best_scale = scale;
            best_err = err;
        }
    }

    let can_sync = best_err <= 0.16;
    let pct = ((best_rate - 1.0) * 100.0) as f32;

    TempoMatch {
        incoming_start_rate: if can_sync { best_rate } else { 1.0 },
        beat_scale: if can_sync { best_scale } else { 1.0 },
        percent_diff: pct,
        can_tempo_sync: can_sync,
    }
}

/// Computes a smooth BPM + Camelot harmonic-sorted DJ ordering of track indices.
/// Tracks with known BPM are initially sorted ascending by BPM, then greedily chained
/// within a 5.0 BPM lookahead window (up to 6 candidates) to maximize Camelot Wheel
/// harmonic compatibility while keeping BPM progression smooth.
pub fn build_dj_sorted_indices(analyses: &[Option<TrackAnalysis>]) -> Vec<usize> {
    let mut known: Vec<(usize, &TrackAnalysis)> = Vec::new();
    let mut unknown: Vec<usize> = Vec::new();

    for (idx, opt) in analyses.iter().enumerate() {
        if let Some(a) = opt {
            known.push((idx, a));
        } else {
            unknown.push(idx);
        }
    }

    known.sort_by(|a, b| {
        a.1.bpm
            .partial_cmp(&b.1.bpm)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.1.energy
                    .partial_cmp(&b.1.energy)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut result = Vec::with_capacity(analyses.len());
    if !known.is_empty() {
        let mut remaining = known;
        let (first_idx, mut current) = remaining.remove(0);
        result.push(first_idx);

        while !remaining.is_empty() {
            let min_rem_bpm = remaining[0].1.bpm;
            let mut best_pos = 0usize;
            let mut best_score = f32::NEG_INFINITY;

            for (pos, &(_, cand)) in remaining.iter().enumerate().take(6) {
                if (cand.bpm - min_rem_bpm).abs() > 5.0 {
                    break;
                }
                let score = (current.camelot_compatibility(cand) as f32 * 3.0)
                    - (cand.bpm - current.bpm).abs() * 0.45;
                if score > best_score {
                    best_score = score;
                    best_pos = pos;
                }
            }

            let (next_idx, next_analysis) = remaining.remove(best_pos);
            result.push(next_idx);
            current = next_analysis;
        }
    }

    result.extend(unknown);
    result
}

#[derive(Debug, Clone, Copy)]
pub struct CrossfadePlan {
    /// Track A position (seconds) where the crossfade begins.
    pub trigger_at_sec: f64,
    /// Crossfade duration in seconds (bar-aligned when BPM is available).
    pub duration_sec: f64,
    /// Track B start offset in seconds (beat-phase aligned to Track A's trigger beat!).
    pub incoming_start_sec: f64,
    /// Tempo match details for smooth pitch-sync glide.
    pub tempo_match: TempoMatch,
    /// Makeup gain for softer intros on incoming track [1.0, 1.55].
    pub intro_boost: f32,
    /// Whether to apply bass-swap EQ crossover.
    pub use_bass_swap: bool,
}

/// Plans a phrase-aligned, energy-matched club DJ crossfade between Track A (outgoing) and Track B (incoming).
pub fn plan_dj_crossfade(
    out_analysis: Option<&TrackAnalysis>,
    in_analysis: Option<&TrackAnalysis>,
    out_dur: f32,
    in_dur: f32,
    settings: &DjSettings,
) -> CrossfadePlan {
    let out_dur = out_dur.max(1.0);
    let in_dur = in_dur.max(1.0);

    // 1. Determine energetic exit cue of Deck A & energetic entry cue of Deck B
    let raw_out_end = if settings.trim_silence {
        out_analysis
            .map(|a| a.mix_out_sec.min(out_dur))
            .unwrap_or(out_dur)
    } else {
        out_dur
    };

    let raw_in_start = if settings.trim_silence {
        in_analysis
            .map(|a| a.mix_in_sec.min((in_dur * 0.18).max(0.0)))
            .unwrap_or(0.0)
    } else {
        0.0
    };

    let intro_boost = in_analysis.map(|a| a.intro_boost).unwrap_or(1.18);

    // 2. Compute tempo compatibility
    let tempo_match = match (out_analysis, in_analysis) {
        (Some(a), Some(b)) if settings.tempo_sync => compute_tempo_match(a.bpm, b.bpm),
        _ => TempoMatch {
            incoming_start_rate: 1.0,
            beat_scale: 1.0,
            percent_diff: 0.0,
            can_tempo_sync: false,
        },
    };

    // 3. Snap crossfade duration to an exact integer number of 4-beat bars (Takte)
    let max_safe_fade = (out_dur * 0.30).min(in_dur * 0.30).clamp(2.5, 16.0);
    let desired = settings.crossfade_secs.clamp(3.0, max_safe_fade);

    let fade_dur = if let Some(a) = out_analysis {
        let bar_sec = a.beat_period() * 4.0; // 1 bar = 4 beats
        if bar_sec > 0.6 && bar_sec < 6.0 {
            let bars = (desired / bar_sec).round().max(1.0);
            (bars * bar_sec).clamp(3.0, max_safe_fade)
        } else {
            desired
        }
    } else {
        desired
    };

    // 4. Snap Deck A trigger time to an exact AI-detected Downbeat ("Takt 1") before `raw_out_end - fade_dur`
    let target_trigger = (raw_out_end - fade_dur).max(1.0);
    let trigger_at_sec = if let Some(a) = out_analysis {
        let lo = (target_trigger - a.beat_period() * 4.5).max(1.0);
        let hi = (target_trigger + a.beat_period() * 2.5).min((out_dur - 2.0).max(1.0));
        a.snap_to_nearest_downbeat(target_trigger, lo, hi)
    } else {
        target_trigger
    };

    // 5. Snap Deck B start time to an exact AI-detected Downbeat ("Takt 1") near `raw_in_start`,
    // and match Deck A's exact Takt phase (`bar_phase_at`) so both tracks overlay in the exact same Takt!
    let incoming_start_sec = if let Some(b) = in_analysis {
        let max_in = (in_dur * 0.22).max(2.0);
        let base_downbeat_b = b.snap_to_nearest_downbeat(raw_in_start, 0.0, max_in);
        if let Some(a) = out_analysis {
            let phase_a = a.bar_phase_at(trigger_at_sec as f64);
            let matched = b.match_bar_phase_near(base_downbeat_b as f64, phase_a) as f32;
            if matched <= max_in + 2.0 {
                matched
            } else {
                base_downbeat_b
            }
        } else {
            base_downbeat_b
        }
    } else {
        raw_in_start
    };

    CrossfadePlan {
        trigger_at_sec: trigger_at_sec as f64,
        duration_sec: fade_dur as f64,
        incoming_start_sec: incoming_start_sec as f64,
        tempo_match,
        intro_boost,
        use_bass_swap: settings.bass_swap,
    }
}
