use crate::audio::decoder::{decode_to_stereo_i16, DecodedAudio};
use crate::audio::dj::{plan_dj_crossfade, CrossfadePlan};
use crate::audio::{AudioCommand, AudioTrackSpec, DjSettings, EngineSnapshot, MixStatus};
use crate::config::PlaybackMode;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{unbounded, Sender};
use parking_lot::Mutex;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;
use std::thread;

/// Phase-coherent 2-band Linkwitz-Riley / 2nd-order Butterworth crossover (~220 Hz)
/// Splits stereo signal into `(low, high)` where `low + high == input` exactly.
#[derive(Clone, Copy)]
struct StereoCrossover {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    zl1: f32,
    zl2: f32,
    zr1: f32,
    zr2: f32,
}

impl StereoCrossover {
    fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let omega = 2.0 * PI * cutoff_hz / sample_rate.max(8000.0);
        let sin_w = omega.sin();
        let cos_w = omega.cos();
        let q = std::f32::consts::FRAC_1_SQRT_2; // Butterworth Q
        let alpha = sin_w / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w) * 0.5) / a0,
            b1: (1.0 - cos_w) / a0,
            b2: ((1.0 - cos_w) * 0.5) / a0,
            a1: (-2.0 * cos_w) / a0,
            a2: (1.0 - alpha) / a0,
            zl1: 0.0,
            zl2: 0.0,
            zr1: 0.0,
            zr2: 0.0,
        }
    }

    fn reset(&mut self) {
        self.zl1 = 0.0;
        self.zl2 = 0.0;
        self.zr1 = 0.0;
        self.zr2 = 0.0;
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> ((f32, f32), (f32, f32)) {
        // Direct Form II Transposed for numerical stability
        let low_l = self.b0 * l + self.zl1;
        self.zl1 = self.b1 * l - self.a1 * low_l + self.zl2;
        self.zl2 = self.b2 * l - self.a2 * low_l;

        let low_r = self.b0 * r + self.zr1;
        self.zr1 = self.b1 * r - self.a1 * low_r + self.zr2;
        self.zr2 = self.b2 * r - self.a2 * low_r;

        ((low_l, low_r), (l - low_l, r - low_r))
    }
}

/// Stereo biquad filter (Direct Form II Transposed) using RBJ Audio EQ Cookbook coefficients.
#[derive(Clone, Copy)]
struct StereoBiquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    zl1: f32,
    zl2: f32,
    zr1: f32,
    zr2: f32,
}

impl StereoBiquad {
    fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            zl1: 0.0,
            zl2: 0.0,
            zr1: 0.0,
            zr2: 0.0,
        }
    }

    fn set_identity_coeffs(&mut self) {
        self.b0 = 1.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
        self.a1 = 0.0;
        self.a2 = 0.0;
    }

    fn reset_state(&mut self) {
        self.zl1 = 0.0;
        self.zl2 = 0.0;
        self.zr1 = 0.0;
        self.zr2 = 0.0;
    }

    fn set_low_shelf(&mut self, sample_rate: f32, freq_hz: f32, slope: f32, gain_db: f32) {
        let db = gain_db.clamp(-18.0, 18.0);
        if db.abs() < 0.05 {
            self.set_identity_coeffs();
            return;
        }
        let fs = sample_rate.max(8000.0) as f64;
        let freq = (freq_hz as f64).clamp(20.0, fs * 0.45);
        let s = (slope as f64).max(0.1);
        let a = 10.0f64.powf(db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / fs;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = (sin_w0 * 0.5) * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    fn set_peaking(&mut self, sample_rate: f32, freq_hz: f32, q: f32, gain_db: f32) {
        let db = gain_db.clamp(-18.0, 18.0);
        if db.abs() < 0.05 {
            self.set_identity_coeffs();
            return;
        }
        let fs = sample_rate.max(8000.0) as f64;
        let freq = (freq_hz as f64).clamp(20.0, fs * 0.45);
        let q = (q as f64).max(0.1);
        let a = 10.0f64.powf(db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / fs;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    fn set_high_shelf(&mut self, sample_rate: f32, freq_hz: f32, slope: f32, gain_db: f32) {
        let db = gain_db.clamp(-18.0, 18.0);
        if db.abs() < 0.05 {
            self.set_identity_coeffs();
            return;
        }
        let fs = sample_rate.max(8000.0) as f64;
        let freq = (freq_hz as f64).clamp(20.0, fs * 0.45);
        let s = (slope as f64).max(0.1);
        let a = 10.0f64.powf(db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq / fs;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = (sin_w0 * 0.5) * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.0).sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;

        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let out_l = self.b0 * l + self.zl1;
        self.zl1 = self.b1 * l - self.a1 * out_l + self.zl2;
        self.zl2 = self.b2 * l - self.a2 * out_l;

        let out_r = self.b0 * r + self.zr1;
        self.zr1 = self.b1 * r - self.a1 * out_r + self.zr2;
        self.zr2 = self.b2 * r - self.a2 * out_r;

        (out_l, out_r)
    }
}

/// Real-time 5-band stereo RBJ biquad equalizer (60 Hz, 230 Hz, 910 Hz, 3.6 kHz, 14 kHz)
/// with zero-coloration flat bypass and gentle headroom compensation.
struct StereoEqualizer {
    sample_rate: f32,
    bands: [StereoBiquad; 5],
    active: bool,
    headroom_gain: f32,
}

impl StereoEqualizer {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            bands: [StereoBiquad::identity(); 5],
            active: false,
            headroom_gain: 1.0,
        }
    }

    fn set_bands_db(&mut self, bands_db: [f32; 5]) {
        let is_active = bands_db.iter().any(|&db| db.abs() >= 0.05);
        if !is_active {
            self.active = false;
            self.headroom_gain = 1.0;
            for band in &mut self.bands {
                band.set_identity_coeffs();
                band.reset_state();
            }
            return;
        }

        self.active = true;
        self.bands[0].set_low_shelf(self.sample_rate, 60.0, 0.85, bands_db[0]);
        self.bands[1].set_peaking(self.sample_rate, 230.0, 1.0, bands_db[1]);
        self.bands[2].set_peaking(self.sample_rate, 910.0, 1.0, bands_db[2]);
        self.bands[3].set_peaking(self.sample_rate, 3600.0, 1.0, bands_db[3]);
        self.bands[4].set_high_shelf(self.sample_rate, 14000.0, 0.85, bands_db[4]);

        let max_boost_db = bands_db
            .iter()
            .copied()
            .map(|db| db.clamp(-18.0, 18.0))
            .fold(0.0f32, f32::max);
        self.headroom_gain = 10.0f32.powf(-max_boost_db * 0.025);
    }

    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if !self.active {
            return (l, r);
        }
        let (mut ol, mut or) = (l, r);
        for band in &mut self.bands {
            (ol, or) = band.process(ol, or);
        }
        (ol * self.headroom_gain, or * self.headroom_gain)
    }
}

struct Deck {
    spec: AudioTrackSpec,
    audio: Arc<DecodedAudio>,
    pos_frames: f64,
    crossover: StereoCrossover,
    smooth_norm_gain: f32,
}

impl Deck {
    fn new(
        spec: AudioTrackSpec,
        audio: Arc<DecodedAudio>,
        sr: f32,
        start_sec: f64,
        loudness_norm: bool,
    ) -> Self {
        let pos_frames = (start_sec * sr as f64).clamp(0.0, audio.total_frames as f64);
        let initial_gain = if loudness_norm {
            spec.analysis
                .as_ref()
                .map(|a| a.norm_gain.clamp(0.35, 2.2))
                .unwrap_or(1.0)
        } else {
            1.0
        };
        Self {
            spec,
            audio,
            pos_frames,
            crossover: StereoCrossover::new(sr, 220.0),
            smooth_norm_gain: initial_gain,
        }
    }

    #[inline]
    fn pos_sec(&self) -> f64 {
        self.pos_frames / self.audio.sample_rate as f64
    }

    #[inline]
    fn is_finished(&self) -> bool {
        self.pos_frames >= self.audio.total_frames as f64
    }
}

struct ActiveTransition {
    plan: CrossfadePlan,
    elapsed_sec: f64,
    start_beat_a: f64,
    start_beat_b: f64,
}

struct MixerState {
    sample_rate: u32,
    deck_a: Option<Deck>,
    deck_b: Option<Deck>,
    preloaded_next: Option<(AudioTrackSpec, Arc<DecodedAudio>)>,
    transition: Option<ActiveTransition>,
    planned_mix: Option<CrossfadePlan>,
    mode: PlaybackMode,
    dj_settings: DjSettings,

    is_playing: bool,
    is_loading: bool,
    loudness_norm: bool,
    target_volume: f32,
    smooth_volume: f32,
    play_gate: f32, // smooth 0..1 envelope to eliminate play/pause clicks

    track_ended_counter: u64,
    transitioned_to_next_counter: u64,
    last_error: Option<String>,
    generation: u64,

    // 4096-sample ring buffer of recent mono output samples for live UI spectrum & AudioVizuallizer window
    scope_ring: Vec<f32>,
    scope_write: usize,
    bass_env: f32,
    rms_env: f32,
    bass_filter: StereoCrossover,
    eq: StereoEqualizer,
}

impl MixerState {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            deck_a: None,
            deck_b: None,
            preloaded_next: None,
            transition: None,
            planned_mix: None,
            mode: PlaybackMode::Normal,
            dj_settings: DjSettings::default(),
            is_playing: false,
            is_loading: false,
            loudness_norm: true,
            target_volume: 0.78,
            smooth_volume: 0.78,
            play_gate: 0.0,
            track_ended_counter: 0,
            transitioned_to_next_counter: 0,
            last_error: None,
            generation: 0,
            scope_ring: vec![0.0; 4096],
            scope_write: 0,
            bass_env: 0.0,
            rms_env: 0.0,
            bass_filter: StereoCrossover::new(sample_rate as f32, 150.0),
            eq: StereoEqualizer::new(sample_rate as f32),
        }
    }

    fn recompute_plan(&mut self) {
        self.planned_mix = None;
        let Some(ref a) = self.deck_a else {
            return;
        };
        let Some((ref next_spec, ref next_audio)) = self.preloaded_next else {
            return;
        };

        if self.mode == PlaybackMode::DjMix {
            let plan = plan_dj_crossfade(
                a.spec.analysis.as_ref(),
                next_spec.analysis.as_ref(),
                a.audio.duration_sec,
                next_audio.duration_sec,
                &self.dj_settings,
            );
            // Only schedule if we haven't already passed the trigger point
            if a.pos_sec() < plan.trigger_at_sec + 1.0 {
                self.planned_mix = Some(plan);
            }
        } else if self.dj_settings.normal_short_fade {
            let fade = 2.5f64.min((a.audio.duration_sec * 0.2) as f64).max(0.8);
            let trigger = (a.audio.duration_sec as f64 - fade).max(0.5);
            if a.pos_sec() < trigger + 0.5 {
                self.planned_mix = Some(CrossfadePlan {
                    trigger_at_sec: trigger,
                    duration_sec: fade,
                    incoming_start_sec: 0.0,
                    tempo_match: crate::audio::dj::TempoMatch {
                        incoming_start_rate: 1.0,
                        beat_scale: 1.0,
                        percent_diff: 0.0,
                        can_tempo_sync: false,
                    },
                    intro_boost: 1.05,
                    use_bass_swap: false,
                });
            }
        }
    }

    fn start_transition_now(&mut self, next_spec: AudioTrackSpec, next_audio: Arc<DecodedAudio>, quick_fade: bool) {
        let sr = self.sample_rate as f32;
        if let Some(ref mut a) = self.deck_a {
            let mut plan = plan_dj_crossfade(
                a.spec.analysis.as_ref(),
                next_spec.analysis.as_ref(),
                a.audio.duration_sec,
                next_audio.duration_sec,
                &self.dj_settings,
            );
            if quick_fade {
                plan.duration_sec = plan.duration_sec.clamp(4.0, 6.0);
            }
            // Match incoming track's Takt & beat phase (`bar_phase_at`) to Deck A's exact live position!
            if let (Some(an_a), Some(an_b)) = (&a.spec.analysis, &next_spec.analysis) {
                let live_bar_phase_a = an_a.bar_phase_at(a.pos_sec());
                let max_in = (next_audio.duration_sec * 0.24).max(2.0);
                let base_downbeat_b = an_b.snap_to_nearest_downbeat(plan.incoming_start_sec as f32, 0.0, max_in);
                let aligned_b = an_b.match_bar_phase_near(base_downbeat_b as f64, live_bar_phase_a);
                if aligned_b < next_audio.duration_sec as f64 * 0.28 {
                    plan.incoming_start_sec = aligned_b.max(0.0);
                }
            }
            plan.trigger_at_sec = a.pos_sec();
            let start_beat_a = a
                .spec
                .analysis
                .as_ref()
                .map(|an| an.continuous_beat_at(a.pos_sec()))
                .unwrap_or(0.0);
            let start_beat_b = next_spec
                .analysis
                .as_ref()
                .map(|an| an.continuous_beat_at(plan.incoming_start_sec))
                .unwrap_or(0.0);

            a.crossover.reset();
            let deck_b = Deck::new(
                next_spec,
                next_audio,
                sr,
                plan.incoming_start_sec,
                self.loudness_norm,
            );
            self.deck_b = Some(deck_b);
            self.transition = Some(ActiveTransition {
                plan,
                elapsed_sec: 0.0,
                start_beat_a,
                start_beat_b,
            });
            self.planned_mix = None;
            self.is_playing = true;
        } else {
            self.deck_a = Some(Deck::new(
                next_spec,
                next_audio,
                sr,
                0.0,
                self.loudness_norm,
            ));
            self.deck_b = None;
            self.transition = None;
            self.is_playing = true;
        }
    }

    /// Render `out_frames` stereo samples into `out` (interleaved L, R).
    fn render_stereo(&mut self, out: &mut [(f32, f32)]) {
        let sr = self.sample_rate as f64;
        let inv_sr = 1.0 / sr;
        // 12ms smooth ramp coeff for volume & play gate; 250ms smooth ramp for loudness normalization
        let ramp_coeff = (1.0 / (0.012 * sr as f32)).min(0.05);
        let norm_ramp_coeff = (1.0 / (0.250 * sr as f32)).min(0.01);

        for slot in out.iter_mut() {
            // Smooth master volume & play/pause gate
            self.smooth_volume += (self.target_volume - self.smooth_volume) * ramp_coeff;
            let target_gate = if self.is_playing && self.deck_a.is_some() {
                1.0
            } else {
                0.0
            };
            self.play_gate += (target_gate - self.play_gate) * ramp_coeff;

            if self.play_gate < 1e-5 {
                *slot = (0.0, 0.0);
                self.bass_env *= 0.9992;
                self.rms_env *= 0.9992;
                self.scope_ring[self.scope_write] = 0.0;
                self.scope_write = (self.scope_write + 1) & 4095;
                continue;
            }

            // Check if automatic scheduled DJ transition should trigger right now!
            if self.transition.is_none() {
                if let (Some(ref deck_a), Some(mut plan)) = (&self.deck_a, self.planned_mix) {
                    if deck_a.pos_sec() >= plan.trigger_at_sec {
                        if let Some((next_spec, next_audio)) = self.preloaded_next.take() {
                            if let (Some(an_a), Some(an_b)) = (&deck_a.spec.analysis, &next_spec.analysis) {
                                let live_phase_a = an_a.bar_phase_at(deck_a.pos_sec());
                                let max_in = (next_audio.duration_sec * 0.24).max(2.0);
                                let base_db = an_b.snap_to_nearest_downbeat(plan.incoming_start_sec as f32, 0.0, max_in);
                                let matched = an_b.match_bar_phase_near(base_db as f64, live_phase_a);
                                if matched < next_audio.duration_sec as f64 * 0.28 {
                                    plan.incoming_start_sec = matched;
                                }
                            }
                            let start_beat_a = deck_a
                                .spec
                                .analysis
                                .as_ref()
                                .map(|an| an.continuous_beat_at(deck_a.pos_sec()))
                                .unwrap_or(0.0);
                            let start_beat_b = next_spec
                                .analysis
                                .as_ref()
                                .map(|an| an.continuous_beat_at(plan.incoming_start_sec))
                                .unwrap_or(0.0);

                            let deck_b = Deck::new(
                                next_spec,
                                next_audio,
                                self.sample_rate as f32,
                                plan.incoming_start_sec,
                                self.loudness_norm,
                            );
                            if let Some(ref mut da) = self.deck_a {
                                da.crossover.reset();
                            }
                            self.deck_b = Some(deck_b);
                            self.transition = Some(ActiveTransition {
                                plan,
                                elapsed_sec: 0.0,
                                start_beat_a,
                                start_beat_b,
                            });
                            self.planned_mix = None;
                        }
                    }
                }
            }

            let (mix_l, mix_r) = if let Some(ref mut tr) = self.transition {
                let p = (tr.elapsed_sec / tr.plan.duration_sec.max(0.05)).clamp(0.0, 1.0) as f32;
                let s = p * p * (3.0 - 2.0 * p); // smoothstep S-curve

                // Mathematically exact Beat-Grid Phase-Locked Loop (PLL) Overlay!
                // 1) Base tempo glide between Deck A and Deck B
                let (rate_a, mut rate_b) = if tr.plan.tempo_match.can_tempo_sync {
                    let ratio = tr.plan.tempo_match.incoming_start_rate;
                    let ra = (1.0 / ratio).powf(s as f64);
                    let rb = ratio * ra;
                    (ra, rb)
                } else {
                    (1.0, 1.0)
                };

                // 2) Continuous AI Beat-Grid Phase Locking: keeps Deck B's exact beats and Takte
                // overlaid on top of Deck A's beats throughout the audible overlap (p < 0.85)!
                if tr.plan.tempo_match.can_tempo_sync {
                    if let (Some(ref da), Some(ref db)) = (&self.deck_a, &self.deck_b) {
                        if let (Some(ref an_a), Some(ref an_b)) = (&da.spec.analysis, &db.spec.analysis) {
                            let cur_beat_a = an_a.continuous_beat_at(da.pos_sec());
                            let elapsed_beats_a = (cur_beat_a - tr.start_beat_a).max(0.0);
                            let target_beat_b =
                                tr.start_beat_b + elapsed_beats_a * tr.plan.tempo_match.beat_scale;
                            let target_sec_b = an_b.time_at_continuous_beat(target_beat_b);
                            let err_sec = target_sec_b - db.pos_sec();

                            // Smoothly taper PLL authority to 0 over the final 18% after Deck A has faded out
                            let pll_weight = if p < 0.80 {
                                1.0
                            } else {
                                ((1.0 - p as f64) / 0.20).clamp(0.0, 1.0)
                            };
                            let pll_nudge = (err_sec * 3.8 * pll_weight).clamp(-0.07, 0.07);
                            rate_b = (rate_b + pll_nudge).clamp(0.80, 1.24);
                        }
                    }
                }

                let (la, ra_s) = if let Some(ref mut da) = self.deck_a {
                    let target_g = if self.loudness_norm {
                        da.spec
                            .analysis
                            .as_ref()
                            .map(|a| a.norm_gain.clamp(0.35, 2.2))
                            .unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    da.smooth_norm_gain += (target_g - da.smooth_norm_gain) * norm_ramp_coeff;
                    let (sl, sr_s) = da.audio.sample_cubic(da.pos_frames);
                    da.pos_frames += rate_a;
                    (sl * da.smooth_norm_gain, sr_s * da.smooth_norm_gain)
                } else {
                    (0.0, 0.0)
                };

                let (lb_raw, rb_raw) = if let Some(ref mut db) = self.deck_b {
                    let target_g = if self.loudness_norm {
                        db.spec
                            .analysis
                            .as_ref()
                            .map(|a| a.norm_gain.clamp(0.35, 2.2))
                            .unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    db.smooth_norm_gain += (target_g - db.smooth_norm_gain) * norm_ramp_coeff;
                    let (sl, sr_s) = db.audio.sample_cubic(db.pos_frames);
                    db.pos_frames += rate_b;
                    (sl * db.smooth_norm_gain, sr_s * db.smooth_norm_gain)
                } else {
                    (0.0, 0.0)
                };

                // Dynamic intro makeup gain so softer intros enter with full clarity, gliding to 1.0x
                let b_boost = tr.plan.intro_boost + (1.0 - tr.plan.intro_boost) * s;
                let lb = lb_raw * b_boost;
                let rb_s = rb_raw * b_boost;

                // Club DJ Constant-Energy Overlapping Curve (eliminates any mid-transition volume dip!):
                // - Deck A holds 100% presence through p=0..0.30, then smoothly fades out over 0.30..1.0
                // - Deck B rises to 100% presence over p=0..0.65, then holds full power over 0.65..1.0
                let hi_gain_a = if p <= 0.30 {
                    1.0f32
                } else {
                    (((p - 0.30) / 0.70) * PI * 0.5).cos()
                };
                let hi_gain_b = if p >= 0.65 {
                    1.0f32
                } else {
                    ((p / 0.65) * PI * 0.5).sin()
                };

                let (out_l, out_r) = if tr.plan.use_bass_swap {
                    // Clean phrase-aligned DJ Bass-Swap EQ crossover at 220 Hz around [0.36..0.58]
                    let (lo_gain_a, lo_gain_b) = if p < 0.36 {
                        (1.0f32, 0.12f32)
                    } else if p > 0.58 {
                        (0.0f32, 1.0f32)
                    } else {
                        let bp = ((p - 0.36) / 0.22) * PI * 0.5;
                        (bp.cos(), bp.sin())
                    };

                    let ((lo_al, lo_ar), (hi_al, hi_ar)) = if let Some(ref mut da) = self.deck_a {
                        da.crossover.process(la, ra_s)
                    } else {
                        ((0.0, 0.0), (0.0, 0.0))
                    };
                    let ((lo_bl, lo_br), (hi_bl, hi_br)) = if let Some(ref mut db) = self.deck_b {
                        db.crossover.process(lb, rb_s)
                    } else {
                        ((0.0, 0.0), (0.0, 0.0))
                    };

                    (
                        lo_al * lo_gain_a + hi_al * hi_gain_a + lo_bl * lo_gain_b + hi_bl * hi_gain_b,
                        lo_ar * lo_gain_a + hi_ar * hi_gain_a + lo_br * lo_gain_b + hi_br * hi_gain_b,
                    )
                } else {
                    (
                        la * hi_gain_a + lb * hi_gain_b,
                        ra_s * hi_gain_a + rb_s * hi_gain_b,
                    )
                };

                tr.elapsed_sec += inv_sr;
                let finished = tr.elapsed_sec >= tr.plan.duration_sec
                    || self.deck_a.as_ref().map(|d| d.is_finished()).unwrap_or(true);

                if finished {
                    // Promote Deck B cleanly to Deck A!
                    self.deck_a = self.deck_b.take();
                    self.transition = None;
                    self.transitioned_to_next_counter += 1;
                }

                (out_l, out_r)
            } else if let Some(ref mut da) = self.deck_a {
                let target_g = if self.loudness_norm {
                    da.spec
                        .analysis
                        .as_ref()
                        .map(|a| a.norm_gain.clamp(0.35, 2.2))
                        .unwrap_or(1.0)
                } else {
                    1.0
                };
                da.smooth_norm_gain += (target_g - da.smooth_norm_gain) * norm_ramp_coeff;
                let (sl, sr_s) = da.audio.sample_cubic(da.pos_frames);
                let sample = (sl * da.smooth_norm_gain, sr_s * da.smooth_norm_gain);
                da.pos_frames += 1.0;

                if da.is_finished() {
                    // Gapless handoff if next track is preloaded!
                    if let Some((next_spec, next_audio)) = self.preloaded_next.take() {
                        let start = if self.mode == PlaybackMode::DjMix && self.dj_settings.trim_silence {
                            next_spec.analysis.as_ref().map(|a| a.mix_in_sec as f64).unwrap_or(0.0)
                        } else {
                            0.0
                        };
                        self.deck_a = Some(Deck::new(
                            next_spec,
                            next_audio,
                            self.sample_rate as f32,
                            start,
                            self.loudness_norm,
                        ));
                        self.transitioned_to_next_counter += 1;
                    } else {
                        self.is_playing = false;
                        self.track_ended_counter += 1;
                    }
                }
                sample
            } else {
                (0.0, 0.0)
            };

            let (eq_l, eq_r) = self.eq.process(mix_l, mix_r);
            let gain = self.smooth_volume * self.play_gate;
            let l = soft_limit(eq_l * gain);
            let r = soft_limit(eq_r * gain);
            *slot = (l, r);

            // Update lightweight metering & 4096-sample scope ring for UI & AudioVizuallizer
            let mono = 0.5 * (l + r);
            self.scope_ring[self.scope_write] = mono;
            self.scope_write = (self.scope_write + 1) & 4095;

            let ((bass_l, _), _) = self.bass_filter.process(mono, 0.0);
            let abs_b = bass_l.abs();
            let abs_m = mono.abs();
            self.bass_env = if abs_b > self.bass_env {
                self.bass_env * 0.96 + abs_b * 0.04
            } else {
                self.bass_env * 0.9994 + abs_b * 0.0006
            };
            self.rms_env = if abs_m > self.rms_env {
                self.rms_env * 0.97 + abs_m * 0.03
            } else {
                self.rms_env * 0.9994 + abs_m * 0.0006
            };
        }
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    if x.abs() <= 0.92 {
        x
    } else {
        let sign = x.signum();
        let excess = x.abs() - 0.92;
        sign * (0.92 + 0.08 * (excess / 0.08).tanh())
    }
}

enum LoadJob {
    PlayNow {
        generation: u64,
        track: AudioTrackSpec,
    },
    CrossfadeNow {
        generation: u64,
        track: AudioTrackSpec,
    },
    PreloadNext {
        generation: u64,
        track: AudioTrackSpec,
    },
}

pub struct AudioEngine {
    state: Arc<Mutex<MixerState>>,
    loader_tx: Sender<LoadJob>,
    _stream: Option<Stream>,
    fft_buf: Vec<Complex<f32>>,
    fft_plan: Arc<dyn rustfft::Fft<f32>>,
    smooth_spectrum: [f32; 16],
}

impl AudioEngine {
    pub fn new(initial_volume: f32, dj_settings: DjSettings) -> Self {
        let host = cpal::default_host();
        let device = host.default_output_device();

        let (sample_rate, stream, state) = if let Some(dev) = device {
            let default_cfg = dev.default_output_config().ok();
            let sr = default_cfg
                .as_ref()
                .map(|c| c.sample_rate().0)
                .unwrap_or(48000);
            let state = Arc::new(Mutex::new(MixerState::new(sr)));
            {
                let mut st = state.lock();
                st.target_volume = initial_volume.clamp(0.0, 1.0);
                st.smooth_volume = st.target_volume;
                st.dj_settings = dj_settings;
            }

            let stream = default_cfg.and_then(|cfg| {
                let channels = cfg.channels() as usize;
                let stream_cfg: StreamConfig = cfg.clone().into();
                let st_clone = state.clone();
                let err_st = state.clone();
                let err_fn = move |err| {
                    err_st.lock().last_error = Some(format!("Audio stream error: {err}"));
                };

                let mut scratch: Vec<(f32, f32)> = Vec::with_capacity(2048);
                let built = match cfg.sample_format() {
                    SampleFormat::F32 => dev.build_output_stream(
                        &stream_cfg,
                        move |data: &mut [f32], _| {
                            let frames = data.len() / channels.max(1);
                            scratch.resize(frames, (0.0, 0.0));
                            st_clone.lock().render_stereo(&mut scratch);
                            for (i, &(l, r)) in scratch.iter().enumerate() {
                                let base = i * channels;
                                if channels == 1 {
                                    data[base] = 0.5 * (l + r);
                                } else {
                                    data[base] = l;
                                    data[base + 1] = r;
                                    for c in 2..channels {
                                        data[base + c] = 0.0;
                                    }
                                }
                            }
                        },
                        err_fn,
                        None,
                    ),
                    SampleFormat::I16 => dev.build_output_stream(
                        &stream_cfg,
                        move |data: &mut [i16], _| {
                            let frames = data.len() / channels.max(1);
                            scratch.resize(frames, (0.0, 0.0));
                            st_clone.lock().render_stereo(&mut scratch);
                            for (i, &(l, r)) in scratch.iter().enumerate() {
                                let base = i * channels;
                                let li = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
                                let ri = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
                                if channels == 1 {
                                    data[base] = ((li as i32 + ri as i32) / 2) as i16;
                                } else {
                                    data[base] = li;
                                    data[base + 1] = ri;
                                    for c in 2..channels {
                                        data[base + c] = 0;
                                    }
                                }
                            }
                        },
                        err_fn,
                        None,
                    ),
                    SampleFormat::U16 => dev.build_output_stream(
                        &stream_cfg,
                        move |data: &mut [u16], _| {
                            let frames = data.len() / channels.max(1);
                            scratch.resize(frames, (0.0, 0.0));
                            st_clone.lock().render_stereo(&mut scratch);
                            for (i, &(l, r)) in scratch.iter().enumerate() {
                                let base = i * channels;
                                let lu = ((l.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16;
                                let ru = ((r.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16;
                                if channels == 1 {
                                    data[base] = ((lu as u32 + ru as u32) / 2) as u16;
                                } else {
                                    data[base] = lu;
                                    data[base + 1] = ru;
                                    for c in 2..channels {
                                        data[base + c] = 32768;
                                    }
                                }
                            }
                        },
                        err_fn,
                        None,
                    ),
                    _ => return None,
                };

                match built {
                    Ok(s) => {
                        let _ = s.play();
                        Some(s)
                    }
                    Err(e) => {
                        state.lock().last_error = Some(format!("Audio output init failed: {e}"));
                        None
                    }
                }
            });

            (sr, stream, state)
        } else {
            let state = Arc::new(Mutex::new(MixerState::new(48000)));
            state.lock().last_error = Some("No audio output device found".to_string());
            (48000, None, state)
        };

        // Background audio file decoder/pre-loader thread
        let (loader_tx, loader_rx) = unbounded::<LoadJob>();
        let state_bg = state.clone();
        let _ = thread::Builder::new()
            .name("rustify-loader".to_string())
            .spawn(move || {
                while let Ok(job) = loader_rx.recv() {
                    match job {
                        LoadJob::PlayNow { generation, track } => {
                            // Skip stale job if user already clicked another track
                            if state_bg.lock().generation != generation {
                                continue;
                            }
                            match decode_to_stereo_i16(&track.path, sample_rate) {
                                Ok(decoded) => {
                                    let audio = Arc::new(decoded);
                                    let mut st = state_bg.lock();
                                    if st.generation != generation {
                                        continue;
                                    }
                                    let loudness_norm = st.loudness_norm;
                                    let sr = st.sample_rate as f32;
                                    st.deck_a = Some(Deck::new(
                                        track,
                                        audio,
                                        sr,
                                        0.0,
                                        loudness_norm,
                                    ));
                                    st.deck_b = None;
                                    st.transition = None;
                                    st.is_loading = false;
                                    st.is_playing = true;
                                    st.last_error = None;
                                    st.recompute_plan();
                                }
                                Err(e) => {
                                    let mut st = state_bg.lock();
                                    if st.generation == generation {
                                        st.is_loading = false;
                                        st.last_error = Some(e);
                                    }
                                }
                            }
                        }
                        LoadJob::CrossfadeNow { generation, track } => {
                            if state_bg.lock().generation != generation {
                                continue;
                            }
                            match decode_to_stereo_i16(&track.path, sample_rate) {
                                Ok(decoded) => {
                                    let audio = Arc::new(decoded);
                                    let mut st = state_bg.lock();
                                    if st.generation != generation {
                                        continue;
                                    }
                                    st.is_loading = false;
                                    st.start_transition_now(track, audio, true);
                                    st.last_error = None;
                                }
                                Err(e) => {
                                    let mut st = state_bg.lock();
                                    if st.generation == generation {
                                        st.is_loading = false;
                                        st.last_error = Some(e);
                                    }
                                }
                            }
                        }
                        LoadJob::PreloadNext { generation, track } => {
                            if state_bg.lock().generation != generation {
                                continue;
                            }
                            // Check if already preloaded
                            {
                                let st = state_bg.lock();
                                if let Some((ref existing, _)) = st.preloaded_next {
                                    if existing.path == track.path {
                                        continue;
                                    }
                                }
                            }
                            if let Ok(decoded) = decode_to_stereo_i16(&track.path, sample_rate) {
                                let audio = Arc::new(decoded);
                                let mut st = state_bg.lock();
                                if st.generation == generation {
                                    st.preloaded_next = Some((track, audio));
                                    st.recompute_plan();
                                }
                            }
                        }
                    }
                }
            });

        let mut planner = FftPlanner::<f32>::new();
        let fft_plan = planner.plan_fft_forward(512);

        Self {
            state,
            loader_tx,
            _stream: stream,
            fft_buf: vec![Complex::new(0.0, 0.0); 512],
            fft_plan,
            smooth_spectrum: [0.0; 16],
        }
    }

    pub fn send(&self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::PlayNow { track, next, mode } => {
                let gen = {
                    let mut st = self.state.lock();
                    st.generation += 1;
                    st.mode = mode;
                    st.is_loading = true;
                    st.preloaded_next = None;
                    st.planned_mix = None;
                    st.transition = None;
                    st.deck_b = None;
                    st.generation
                };
                let _ = self.loader_tx.send(LoadJob::PlayNow {
                    generation: gen,
                    track,
                });
                if let Some(n) = next {
                    let _ = self.loader_tx.send(LoadJob::PreloadNext {
                        generation: gen,
                        track: n,
                    });
                }
            }
            AudioCommand::CrossfadeTo { track, next, mode } => {
                // If track is already preloaded, transition instantaneously with ZERO wait!
                let (already_loaded, gen) = {
                    let mut st = self.state.lock();
                    st.generation += 1;
                    st.mode = mode;
                    let found = if let Some((ref pre_spec, _)) = st.preloaded_next {
                        if pre_spec.path == track.path {
                            st.preloaded_next.take()
                        } else {
                            st.preloaded_next = None;
                            None
                        }
                    } else {
                        None
                    };
                    if let Some((_, audio)) = found {
                        st.start_transition_now(track.clone(), audio, true);
                        (true, st.generation)
                    } else {
                        st.is_loading = true;
                        (false, st.generation)
                    }
                };
                if !already_loaded {
                    let _ = self.loader_tx.send(LoadJob::CrossfadeNow {
                        generation: gen,
                        track,
                    });
                }
                if let Some(n) = next {
                    let _ = self.loader_tx.send(LoadJob::PreloadNext {
                        generation: gen,
                        track: n,
                    });
                }
            }
            AudioCommand::UpdateNext { next, mode } => {
                let gen = {
                    let mut st = self.state.lock();
                    st.mode = mode;
                    if let Some(ref n) = next {
                        let same = st
                            .preloaded_next
                            .as_ref()
                            .map(|(s, _)| s.path == n.path)
                            .unwrap_or(false);
                        if same {
                            if let Some((ref mut s, _)) = st.preloaded_next {
                                s.analysis = n.analysis.clone();
                            }
                            st.recompute_plan();
                            return;
                        }
                        st.preloaded_next = None;
                        st.planned_mix = None;
                        st.generation
                    } else {
                        st.preloaded_next = None;
                        st.planned_mix = None;
                        return;
                    }
                };
                if let Some(n) = next {
                    let _ = self.loader_tx.send(LoadJob::PreloadNext {
                        generation: gen,
                        track: n,
                    });
                }
            }
            AudioCommand::UpdateCurrentAnalysis { path, analysis } => {
                let mut st = self.state.lock();
                let mut changed = false;
                if let Some(ref mut da) = st.deck_a {
                    if da.spec.path == path {
                        da.spec.analysis = Some(analysis.clone());
                        changed = true;
                    }
                }
                if let Some(ref mut db) = st.deck_b {
                    if db.spec.path == path {
                        db.spec.analysis = Some(analysis.clone());
                        changed = true;
                    }
                }
                if let Some((ref mut pre, _)) = st.preloaded_next {
                    if pre.path == path {
                        pre.analysis = Some(analysis);
                        changed = true;
                    }
                }
                if changed {
                    st.recompute_plan();
                }
            }
            AudioCommand::MigrateTrackPath { old_path, new_path } => {
                let mut st = self.state.lock();
                if let Some(ref mut da) = st.deck_a {
                    if da.spec.path == old_path {
                        da.spec.path = new_path.clone();
                    }
                }
                if let Some(ref mut db) = st.deck_b {
                    if db.spec.path == old_path {
                        db.spec.path = new_path.clone();
                    }
                }
                if let Some((ref mut pre, _)) = st.preloaded_next {
                    if pre.path == old_path {
                        pre.path = new_path;
                    }
                }
            }
            AudioCommand::InvalidatePreload(path) => {
                let mut st = self.state.lock();
                if st
                    .preloaded_next
                    .as_ref()
                    .is_some_and(|(pre, _)| pre.path == path)
                {
                    st.preloaded_next = None;
                    st.recompute_plan();
                }
            }
            AudioCommand::SetPlaying(p) => {
                let mut st = self.state.lock();
                if st.deck_a.is_some() {
                    st.is_playing = p;
                }
            }
            AudioCommand::TogglePlay => {
                let mut st = self.state.lock();
                if st.deck_a.is_some() {
                    st.is_playing = !st.is_playing;
                }
            }
            AudioCommand::Seek(sec) => {
                let mut st = self.state.lock();
                // Cancel any active crossfade when user manually seeks
                if st.transition.is_some() {
                    st.transition = None;
                    st.deck_b = None;
                }
                if let Some(ref mut da) = st.deck_a {
                    let target = (sec.max(0.0) as f64 * da.audio.sample_rate as f64)
                        .clamp(0.0, (da.audio.total_frames.saturating_sub(1)) as f64);
                    da.pos_frames = target;
                    da.crossover.reset();
                }
                st.recompute_plan();
            }
            AudioCommand::SetVolume(vol) => {
                self.state.lock().target_volume = vol.clamp(0.0, 1.0);
            }
            AudioCommand::SetEq(bands) => {
                self.state.lock().eq.set_bands_db(bands);
            }
            AudioCommand::SetLoudnessNorm(enabled) => {
                self.state.lock().loudness_norm = enabled;
            }
            AudioCommand::SetDjSettings(settings) => {
                let mut st = self.state.lock();
                st.dj_settings = settings;
                st.recompute_plan();
            }
            AudioCommand::Stop => {
                let mut st = self.state.lock();
                st.generation += 1;
                st.is_playing = false;
                st.is_loading = false;
                st.deck_a = None;
                st.deck_b = None;
                st.preloaded_next = None;
                st.transition = None;
                st.planned_mix = None;
            }
        }
    }

    pub fn copy_recent_mono_4096(&self, out: &mut [f32; 4096]) -> (u32, bool) {
        let st = self.state.lock();
        let w = st.scope_write;
        for (i, item) in out.iter_mut().enumerate() {
            *item = st.scope_ring[(w + i) & 4095];
        }
        (st.sample_rate, st.is_playing)
    }

    pub fn snapshot(&mut self) -> EngineSnapshot {
        let mut scope = [0.0f32; 512];
        let snap_base = {
            let st = self.state.lock();
            let w = st.scope_write;
            for (i, item) in scope.iter_mut().enumerate() {
                *item = st.scope_ring[(w + 4096 - 512 + i) & 4095];
            }
            let (current_path, pos, dur) = if let Some(ref da) = st.deck_a {
                (
                    Some(da.spec.path.clone()),
                    da.pos_sec() as f32,
                    da.audio.duration_sec,
                )
            } else {
                (None, 0.0, 0.0)
            };

            let mix = if let Some(ref tr) = st.transition {
                let progress =
                    (tr.elapsed_sec / tr.plan.duration_sec.max(0.05)).clamp(0.0, 1.0) as f32;
                MixStatus {
                    active: true,
                    progress,
                    outgoing_title: st
                        .deck_a
                        .as_ref()
                        .map(|d| d.spec.title.clone())
                        .unwrap_or_default(),
                    incoming_title: st
                        .deck_b
                        .as_ref()
                        .map(|d| d.spec.title.clone())
                        .unwrap_or_default(),
                    outgoing_bpm: st
                        .deck_a
                        .as_ref()
                        .and_then(|d| d.spec.analysis.as_ref().map(|a| a.bpm)),
                    incoming_bpm: st
                        .deck_b
                        .as_ref()
                        .and_then(|d| d.spec.analysis.as_ref().map(|a| a.bpm)),
                    outgoing_key: st.deck_a.as_ref().and_then(|d| {
                        d.spec
                            .analysis
                            .as_ref()
                            .filter(|a| !a.camelot_key.is_empty())
                            .map(|a| a.camelot_key.clone())
                    }),
                    incoming_key: st.deck_b.as_ref().and_then(|d| {
                        d.spec
                            .analysis
                            .as_ref()
                            .filter(|a| !a.camelot_key.is_empty())
                            .map(|a| a.camelot_key.clone())
                    }),
                    tempo_synced: tr.plan.tempo_match.can_tempo_sync,
                    tempo_delta_pct: tr.plan.tempo_match.percent_diff,
                }
            } else {
                MixStatus::default()
            };

            EngineSnapshot {
                is_playing: st.is_playing,
                is_loading: st.is_loading,
                current_path,
                position_sec: pos,
                duration_sec: dur,
                planned_mix_out_sec: st.planned_mix.map(|p| p.trigger_at_sec as f32),
                mix,
                bass_energy: (st.bass_env * 2.6).clamp(0.0, 1.0),
                rms_level: (st.rms_env * 2.4).clamp(0.0, 1.0),
                spectrum: [0.0; 16],
                track_ended_counter: st.track_ended_counter,
                transitioned_to_next_counter: st.transitioned_to_next_counter,
                last_error: st.last_error.clone(),
            }
        };

        // Compute fast 16-band logarithmic spectrum on UI thread from 512-sample ring (<0.02ms)
        let mut snap = snap_base;
        if snap.is_playing {
            for (i, &sample) in scope.iter().enumerate() {
                let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / 511.0).cos());
                self.fft_buf[i] = Complex::new(sample * w, 0.0);
            }
            self.fft_plan.process(&mut self.fft_buf);

            // Logarithmic bin ranges across 1..200
            const EDGES: [usize; 17] = [1, 2, 3, 5, 7, 10, 14, 19, 26, 35, 47, 63, 84, 112, 148, 192, 240];
            for band in 0..16 {
                let lo = EDGES[band];
                let hi = EDGES[band + 1];
                let mut peak = 0.0f32;
                for &c in &self.fft_buf[lo..hi] {
                    let mag = (c.re * c.re + c.im * c.im).sqrt() / 32.0;
                    if mag > peak {
                        peak = mag;
                    }
                }
                let val = (peak.sqrt() * 0.92).clamp(0.0, 1.0);
                // Smooth attack/decay for fluid liquid glass visualizer
                if val > self.smooth_spectrum[band] {
                    self.smooth_spectrum[band] = self.smooth_spectrum[band] * 0.45 + val * 0.55;
                } else {
                    self.smooth_spectrum[band] = self.smooth_spectrum[band] * 0.84 + val * 0.16;
                }
            }
        } else {
            for v in &mut self.smooth_spectrum {
                *v *= 0.86;
            }
        }

        snap.spectrum = self.smooth_spectrum;
        snap
    }
}
