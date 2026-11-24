use crate::audio::engine::AudioEngine;
use crate::i18n::Language;
use crate::lyrics::VisualizerLyricsFrame;
use crate::ui::theme::{
    font_regular, font_semibold, glass_slider, glass_toggle, rgba, FROST_CTA_IDLE, FROST_CTA_TEXT,
    GLASS_PILL_ACTIVE, GLASS_PILL_IDLE, RIM_BRIGHT, RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use egui::{
    Align2, Color32, Context, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, ViewportBuilder, ViewportCommand, ViewportId,
};
use parking_lot::Mutex;
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::f32::consts::PI;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizerAudioSource {
    #[default]
    RustifyApp,
    SystemAudio,
    Microphone,
}

impl VisualizerAudioSource {
    pub fn label(self, lang: Language) -> &'static str {
        match (self, lang) {
            (VisualizerAudioSource::RustifyApp, Language::De) => "Rustify (Musik-App)",
            (VisualizerAudioSource::RustifyApp, Language::En) => "Rustify (Music App)",
            (VisualizerAudioSource::SystemAudio, Language::De) => "System-Audio (Desktop)",
            (VisualizerAudioSource::SystemAudio, Language::En) => "System Audio (Desktop)",
            (VisualizerAudioSource::Microphone, Language::De) => "Mikrofon",
            (VisualizerAudioSource::Microphone, Language::En) => "Microphone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizerPreset {
    #[default]
    Classic,
    NeonPulse,
    OceanWaves,
    Custom,
}

impl VisualizerPreset {
    pub fn label(self) -> &'static str {
        match self {
            VisualizerPreset::Classic => "Classic",
            VisualizerPreset::NeonPulse => "Neon Pulse",
            VisualizerPreset::OceanWaves => "Ocean Waves",
            VisualizerPreset::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizerShape {
    #[default]
    LineMirrored,
    LineSingle,
    Bars,
}

impl VisualizerShape {
    pub fn label(self) -> &'static str {
        match self {
            VisualizerShape::LineMirrored => "Line (Mirrored)",
            VisualizerShape::LineSingle => "Line (Single)",
            VisualizerShape::Bars => "Bars",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FreqScale {
    #[default]
    Dynamic,
    Fixed,
}

impl FreqScale {
    pub fn label(self) -> &'static str {
        match self {
            FreqScale::Dynamic => "Dynamic Tracking",
            FreqScale::Fixed => "Fixed (80Hz - 24kHz)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizerLyricsPosition {
    Top,
    #[default]
    Bottom,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerConfig {
    #[serde(default)]
    pub audio_source: VisualizerAudioSource,
    pub preset: VisualizerPreset,
    pub color: [u8; 3],
    pub bg_color: [u8; 3],
    pub shape: VisualizerShape,
    pub freq_scale: FreqScale,
    pub line_width: f32,
    pub history_limit: usize,
    #[serde(default = "default_true")]
    pub show_lyrics: bool,
    #[serde(default)]
    pub lyrics_position: VisualizerLyricsPosition,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            audio_source: VisualizerAudioSource::RustifyApp,
            preset: VisualizerPreset::Classic,
            color: [255, 255, 255],
            bg_color: [0, 0, 0],
            shape: VisualizerShape::LineMirrored,
            freq_scale: FreqScale::Dynamic,
            line_width: 2.5,
            history_limit: 15,
            show_lyrics: true,
            lyrics_position: VisualizerLyricsPosition::Bottom,
        }
    }
}

impl VisualizerConfig {
    pub fn apply_preset(&mut self, preset: VisualizerPreset) {
        self.preset = preset;
        match preset {
            VisualizerPreset::Classic => {
                self.color = [255, 255, 255];
                self.bg_color = [0, 0, 0];
                self.shape = VisualizerShape::LineMirrored;
                self.freq_scale = FreqScale::Dynamic;
                self.line_width = 2.5;
                self.history_limit = 15;
            }
            VisualizerPreset::NeonPulse => {
                self.color = [255, 0, 255];
                self.bg_color = [10, 0, 26];
                self.shape = VisualizerShape::Bars;
                self.freq_scale = FreqScale::Fixed;
                self.line_width = 4.0;
                self.history_limit = 15;
            }
            VisualizerPreset::OceanWaves => {
                self.color = [0, 204, 255];
                self.bg_color = [0, 10, 20];
                self.shape = VisualizerShape::LineSingle;
                self.freq_scale = FreqScale::Dynamic;
                self.line_width = 2.0;
                self.history_limit = 15;
            }
            VisualizerPreset::Custom => {}
        }
    }
}

struct CaptureRing {
    ring: Vec<f32>,
    write_idx: usize,
    sample_rate: u32,
}

impl CaptureRing {
    fn new(sample_rate: u32) -> Self {
        Self {
            ring: vec![0.0; 4096],
            write_idx: 0,
            sample_rate,
        }
    }

    #[inline]
    fn push_sample(&mut self, mono: f32) {
        self.ring[self.write_idx] = mono;
        self.write_idx = (self.write_idx + 1) & 4095;
    }

    fn copy_recent_4096(&self, out: &mut [f32; 4096]) -> u32 {
        let w = self.write_idx;
        for (i, item) in out.iter_mut().enumerate() {
            *item = self.ring[(w + i) & 4095];
        }
        self.sample_rate
    }
}

#[cfg(target_os = "linux")]
struct LinuxPipeCapture {
    stop: Arc<std::sync::atomic::AtomicBool>,
    child: Option<std::process::Child>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Drop for LinuxPipeCapture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[allow(dead_code)]
enum ExternalCapture {
    Cpal(Stream),
    #[cfg(target_os = "linux")]
    LinuxPipe(LinuxPipeCapture),
}

/// 1:1 pure-Rust implementation of https://ilay-dev.github.io/AudioVizuallizer/
/// Supports Rustify internal audio, Desktop System Audio Loopback (Windows WASAPI / Linux PipeWire & PulseAudio), and Microphone input.
pub struct AudioVisualizerState {
    pub open: bool,
    pub paused: bool,
    pub fullscreen: bool,
    pub config: VisualizerConfig,

    // External capture state for SystemAudio (WASAPI / PipeWire / PulseAudio loopback) & Microphone
    active_capture_source: VisualizerAudioSource,
    capture_stream: Option<ExternalCapture>,
    capture_ring: Arc<Mutex<CaptureRing>>,
    pub capture_error: Option<String>,

    // Web Audio API AnalyserNode (fftSize = 4096, smoothingTimeConstant = 0.85)
    fft_plan: Arc<dyn rustfft::Fft<f32>>,
    blackman_win: Vec<f32>,
    fft_buf: Vec<Complex<f32>>,
    mono_scratch: Box<[f32; 4096]>,
    smoothed_mag: Vec<f32>,
    current_data: Vec<f32>, // 2048 frequency bins in [0.0 .. 255.0]
    history: VecDeque<Vec<f32>>,

    // Dynamic frequency & amplitude tracking state from original JS
    all_time_max_amp: f32,
    dynamic_end_bin: f32,
}

impl AudioVisualizerState {
    pub fn new(config: VisualizerConfig) -> Self {
        const FFT_SIZE: usize = 4096;
        let mut planner = FftPlanner::<f32>::new();
        let fft_plan = planner.plan_fft_forward(FFT_SIZE);

        // Standard Web Audio API Blackman window (alpha = 0.16)
        let a0 = 0.42f32;
        let a1 = 0.50f32;
        let a2 = 0.08f32;
        let blackman_win: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let phase = 2.0 * PI * (i as f32) / (FFT_SIZE as f32);
                a0 - a1 * phase.cos() + a2 * (2.0 * phase).cos()
            })
            .collect();

        Self {
            open: false,
            paused: false,
            fullscreen: false,
            config,
            active_capture_source: VisualizerAudioSource::RustifyApp,
            capture_stream: None,
            capture_ring: Arc::new(Mutex::new(CaptureRing::new(48000))),
            capture_error: None,
            fft_plan,
            blackman_win,
            fft_buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            mono_scratch: Box::new([0.0; FFT_SIZE]),
            smoothed_mag: vec![0.0; FFT_SIZE / 2],
            current_data: vec![0.0; FFT_SIZE / 2],
            history: VecDeque::with_capacity(64),
            all_time_max_amp: 100.0,
            dynamic_end_bin: 1024.0,
        }
    }

    pub fn trim_history(&mut self) {
        while self.history.len() > self.config.history_limit {
            self.history.pop_front();
        }
    }

    fn sync_capture_stream(&mut self) {
        if !self.open || self.config.audio_source == VisualizerAudioSource::RustifyApp {
            if self.capture_stream.is_some() {
                self.capture_stream = None;
                self.active_capture_source = VisualizerAudioSource::RustifyApp;
                self.capture_error = None;
            }
            return;
        }

        if self.active_capture_source == self.config.audio_source && self.capture_stream.is_some() {
            return;
        }

        // Rebuild external capture stream for SystemAudio (WASAPI loopback) or Microphone
        self.capture_stream = None;
        self.capture_error = None;
        self.active_capture_source = self.config.audio_source;

        match build_external_capture_stream(self.config.audio_source, self.capture_ring.clone()) {
            Ok(stream) => {
                self.capture_stream = Some(stream);
            }
            Err(err) => {
                self.capture_error = Some(err);
            }
        }
    }

    /// Updates the 4096-point FFT frequency spectrum from the selected audio source
    /// and renders the secondary AudioVisualizer viewport window.
    /// Renders one frame of the AudioVisualizer directly into an arbitrary painter/rect
    /// (used both by the secondary window and headless screenshot capture).
    pub fn render_frame(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        engine: &AudioEngine,
        lyrics: Option<&crate::lyrics::VisualizerLyricsFrame>,
    ) {
        let sample_rate = self.update_fft_and_history(engine);
        let bg_col = Color32::from_rgb(
            self.config.bg_color[0],
            self.config.bg_color[1],
            self.config.bg_color[2],
        );
        painter.rect_filled(rect, CornerRadius::ZERO, bg_col);

        let limit = self.config.history_limit.max(1) as f32;
        let bin_width = (sample_rate.max(8000) as f32) / 4096.0;
        let (start_bin, active_range) = match self.config.freq_scale {
            FreqScale::Fixed => {
                let sb = ((80.0 / bin_width).floor() as usize).min(2040);
                let eb = ((24000.0 / bin_width).floor() as usize).min(2047);
                (sb, eb.saturating_sub(sb).max(16))
            }
            FreqScale::Dynamic => {
                let mut active = 0usize;
                for (i, &val) in self.current_data.iter().enumerate() {
                    if val > 15.0 {
                        active = i;
                    }
                }
                let mut target_range = ((active as f32 * 1.15) as usize).max(150);
                target_range = target_range.min(self.current_data.len());
                let diff = target_range as f32 - self.dynamic_end_bin;
                if diff > 0.0 {
                    self.dynamic_end_bin += diff * 0.20;
                } else {
                    self.dynamic_end_bin += diff * 0.03;
                }
                (0, (self.dynamic_end_bin.round() as usize).clamp(150, 2048))
            }
        };

        let cur_max = self.current_data[start_bin..(start_bin + active_range).min(2048)]
            .iter()
            .copied()
            .fold(0.0f32, f32::max);
        if cur_max > self.all_time_max_amp {
            self.all_time_max_amp = cur_max;
        } else {
            self.all_time_max_amp = (self.all_time_max_amp * 0.999).max(120.0);
        }

        let hist_len = self.history.len();
        for (i, hist_data) in self.history.iter().enumerate() {
            let progress = (i as f32) / hist_len.max(1) as f32;
            let alpha = progress.powi(2) * 0.6;
            let scale = 0.5 + progress * 0.5;
            let depth_offset = limit - (i as f32);
            draw_visualizer_path(
                painter,
                rect,
                &self.config,
                hist_data,
                start_bin,
                active_range,
                self.all_time_max_amp,
                alpha,
                scale,
                depth_offset,
                false,
            );
        }

        draw_visualizer_path(
            painter,
            rect,
            &self.config,
            &self.current_data,
            start_bin,
            active_range,
            self.all_time_max_amp,
            1.0,
            1.0,
            0.0,
            true,
        );

        if self.config.show_lyrics {
            if let Some(lyr) = lyrics {
                draw_visualizer_lyrics(painter, rect, &self.config, lyr);
            }
        }
    }

    fn update_fft_and_history(&mut self, engine: &AudioEngine) -> u32 {
        let sample_rate = match self.config.audio_source {
            VisualizerAudioSource::RustifyApp => {
                let (sr, _) = engine.copy_recent_mono_4096(&mut self.mono_scratch);
                if std::env::var("RUSTIFY_CAPTURE_VIEW").as_deref() == Ok("visualizer_only")
                    && self.mono_scratch.iter().all(|&x| x.abs() < 1e-4)
                {
                    let phase_off = self.history.len() as f32 * 0.35;
                    for (i, slot) in self.mono_scratch.iter_mut().enumerate() {
                        let t = i as f32 / sr.max(8000) as f32;
                        let env = (0.65 + 0.35 * (phase_off + t * 8.0).sin()).max(0.2);
                        *slot = env
                            * (0.45 * (2.0 * PI * 62.0 * t + phase_off).sin()
                                + 0.32 * (2.0 * PI * 145.0 * t + phase_off * 1.3).sin()
                                + 0.24 * (2.0 * PI * 330.0 * t - phase_off).sin()
                                + 0.16 * (2.0 * PI * 740.0 * t + phase_off * 0.7).sin()
                                + 0.10 * (2.0 * PI * 1480.0 * t).sin()
                                + 0.06 * (2.0 * PI * 2800.0 * t).sin());
                    }
                }
                sr
            }
            VisualizerAudioSource::SystemAudio | VisualizerAudioSource::Microphone => {
                self.capture_ring.lock().copy_recent_4096(&mut self.mono_scratch)
            }
        };

        if !self.paused {
            for i in 0..4096 {
                self.fft_buf[i] = Complex::new(self.mono_scratch[i] * self.blackman_win[i], 0.0);
            }
            self.fft_plan.process(&mut self.fft_buf);

            const SMOOTHING: f32 = 0.85;
            const MIN_DB: f32 = -100.0;
            const MAX_DB: f32 = -30.0;
            const INV_DB_RANGE: f32 = 255.0 / (MAX_DB - MIN_DB);

            for k in 0..2048 {
                let c = self.fft_buf[k];
                let mag = (c.re * c.re + c.im * c.im).sqrt() / 4096.0;
                let smoothed = SMOOTHING * self.smoothed_mag[k] + (1.0 - SMOOTHING) * mag;
                self.smoothed_mag[k] = smoothed;

                let db = 20.0 * smoothed.max(1e-7).log10();
                let byte_val = ((db - MIN_DB) * INV_DB_RANGE).clamp(0.0, 255.0);
                self.current_data[k] = byte_val;
            }

            self.history.push_back(self.current_data.clone());
            while self.history.len() > self.config.history_limit {
                self.history.pop_front();
            }
        }
        sample_rate
    }

    pub fn show_window(
        &mut self,
        ctx: &Context,
        engine: &AudioEngine,
        lyrics: Option<&VisualizerLyricsFrame>,
    ) {
        self.sync_capture_stream();

        if !self.open {
            return;
        }

        let sample_rate = match self.config.audio_source {
            VisualizerAudioSource::RustifyApp => {
                let (sr, _) = engine.copy_recent_mono_4096(&mut self.mono_scratch);
                if std::env::var("RUSTIFY_CAPTURE_VIEW").as_deref() == Ok("visualizer_only")
                    && self.mono_scratch.iter().all(|&x| x.abs() < 1e-4)
                {
                    let phase_off = self.history.len() as f32 * 0.35;
                    for (i, slot) in self.mono_scratch.iter_mut().enumerate() {
                        let t = i as f32 / sr.max(8000) as f32;
                        let env = (0.65 + 0.35 * (phase_off + t * 8.0).sin()).max(0.2);
                        *slot = env
                            * (0.45 * (2.0 * PI * 62.0 * t + phase_off).sin()
                                + 0.32 * (2.0 * PI * 145.0 * t + phase_off * 1.3).sin()
                                + 0.24 * (2.0 * PI * 330.0 * t - phase_off).sin()
                                + 0.16 * (2.0 * PI * 740.0 * t + phase_off * 0.7).sin()
                                + 0.10 * (2.0 * PI * 1480.0 * t).sin()
                                + 0.06 * (2.0 * PI * 2800.0 * t).sin());
                    }
                }
                sr
            }
            VisualizerAudioSource::SystemAudio | VisualizerAudioSource::Microphone => {
                self.capture_ring.lock().copy_recent_4096(&mut self.mono_scratch)
            }
        };

        if !self.paused {
            // Compute 4096-point FFT exactly like Web Audio API AnalyserNode
            for i in 0..4096 {
                self.fft_buf[i] = Complex::new(self.mono_scratch[i] * self.blackman_win[i], 0.0);
            }
            self.fft_plan.process(&mut self.fft_buf);

            // smoothingTimeConstant = 0.85, minDecibels = -100, maxDecibels = -30
            const SMOOTHING: f32 = 0.85;
            const MIN_DB: f32 = -100.0;
            const MAX_DB: f32 = -30.0;
            const INV_DB_RANGE: f32 = 255.0 / (MAX_DB - MIN_DB);

            for k in 0..2048 {
                let c = self.fft_buf[k];
                let mag = (c.re * c.re + c.im * c.im).sqrt() / 4096.0;
                let smoothed = SMOOTHING * self.smoothed_mag[k] + (1.0 - SMOOTHING) * mag;
                self.smoothed_mag[k] = smoothed;

                let db = 20.0 * smoothed.max(1e-7).log10();
                let byte_val = ((db - MIN_DB) * INV_DB_RANGE).clamp(0.0, 255.0);
                self.current_data[k] = byte_val;
            }

            self.history.push_back(self.current_data.clone());
            while self.history.len() > self.config.history_limit {
                self.history.pop_front();
            }
        }

        let viewport_id = ViewportId::from_hash_of("rustify_audiovisuallizer_window");
        let builder = ViewportBuilder::default()
            .with_title("Rustify — AudioVisualizer")
            .with_inner_size([980.0, 560.0])
            .with_min_inner_size([420.0, 260.0]);

        ctx.show_viewport_immediate(viewport_id, builder, |vp_ctx, _class| {
            if let Some(viz_out) = std::env::var_os("RUSTIFY_CAPTURE_VIZ_PNG") {
                let mut viz_saved = false;
                vp_ctx.input(|i| {
                    for ev in &i.events {
                        if let egui::Event::Screenshot { image, .. } = ev {
                            let w = image.width() as u32;
                            let h = image.height() as u32;
                            let rgba_bytes: Vec<u8> = image
                                .pixels
                                .iter()
                                .flat_map(|c| c.to_srgba_unmultiplied())
                                .collect();
                            if let Some(buf) = image::RgbaImage::from_raw(w, h, rgba_bytes) {
                                let _ = buf.save(&viz_out);
                            }
                            viz_saved = true;
                        }
                    }
                });
                let frame_id = egui::Id::new("rustify_viz_capture_frames");
                let count = vp_ctx.data_mut(|d| {
                    let c = d.get_temp::<u32>(frame_id).unwrap_or(0) + 1;
                    d.insert_temp(frame_id, c);
                    c
                });
                if count == 30 {
                    // Seek playing track into active lyrics section
                    engine.send(crate::audio::AudioCommand::Seek(24.0));
                }
                if count == 75 {
                    vp_ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                }
                vp_ctx.request_repaint();
                if viz_saved {
                    std::process::exit(0);
                }
                if count > 140 {
                    std::process::exit(0);
                }
            }

            if vp_ctx.input(|i| i.viewport().close_requested()) {
                self.open = false;
                self.fullscreen = false;
                self.capture_stream = None;
                self.active_capture_source = VisualizerAudioSource::RustifyApp;
                return;
            }

            // Keyboard controls matching website: "f" for fullscreen, "Space" to pause
            let (toggle_fs, toggle_pause) = vp_ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::Space),
                )
            });
            if toggle_fs {
                self.fullscreen = !self.fullscreen;
                vp_ctx.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
            }
            if toggle_pause {
                self.paused = !self.paused;
            }

            let bg_col = Color32::from_rgb(
                self.config.bg_color[0],
                self.config.bg_color[1],
                self.config.bg_color[2],
            );

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(bg_col))
                .show(vp_ctx, |ui| {
                    let rect = ui.max_rect();
                    let resp = ui.interact(rect, ui.id().with("viz_canvas"), Sense::click());
                    if resp.double_clicked() {
                        self.fullscreen = !self.fullscreen;
                        vp_ctx.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
                    } else if resp.clicked() {
                        self.paused = !self.paused;
                    }

                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::ZERO, bg_col);

                    let limit = self.config.history_limit.max(1) as f32;

                    // Update dynamic frequency range & allTimeMaxAmp from current_data
                    let bin_width = (sample_rate.max(8000) as f32) / 4096.0;
                    let (start_bin, active_range) = match self.config.freq_scale {
                        FreqScale::Fixed => {
                            let sb = ((80.0 / bin_width).floor() as usize).min(2040);
                            let eb = ((24000.0 / bin_width).floor() as usize).min(2047);
                            (sb, eb.saturating_sub(sb).max(16))
                        }
                        FreqScale::Dynamic => {
                            let mut last_active = 0usize;
                            for i in (0..self.current_data.len()).rev() {
                                if self.current_data[i] > 3.0 {
                                    last_active = i;
                                    break;
                                }
                            }
                            self.dynamic_end_bin =
                                self.dynamic_end_bin * 0.99 + (last_active as f32) * 0.01;
                            let sb = ((40.0 / bin_width).floor() as usize).min(2000);
                            let ar = ((self.dynamic_end_bin - sb as f32).max(100.0) as usize)
                                .min(2048usize.saturating_sub(sb))
                                .max(16);
                            (sb, ar)
                        }
                    };

                    let mut current_frame_max = 0.0f32;
                    for &v in &self.current_data {
                        if v > current_frame_max {
                            current_frame_max = v;
                        }
                    }
                    if current_frame_max > self.all_time_max_amp {
                        self.all_time_max_amp = current_frame_max;
                    } else {
                        self.all_time_max_amp = (self.all_time_max_amp * 0.9999).max(35.0);
                    }

                    // 1. Render 3D receding history trails
                    for (i, hist_data) in self.history.iter().enumerate() {
                        let progress = (i as f32) / limit;
                        let alpha = progress * 0.25;
                        let scale = 0.5 + progress * 0.5;
                        let depth_offset = limit - (i as f32);
                        draw_visualizer_path(
                            painter,
                            rect,
                            &self.config,
                            hist_data,
                            start_bin,
                            active_range,
                            self.all_time_max_amp,
                            alpha,
                            scale,
                            depth_offset,
                            false,
                        );
                    }

                    // 2. Render main foreground spectrum path with luminous shadow glow
                    draw_visualizer_path(
                        painter,
                        rect,
                        &self.config,
                        &self.current_data,
                        start_bin,
                        active_range,
                        self.all_time_max_amp,
                        1.0,
                        1.0,
                        0.0,
                        true,
                    );

                    // 3. Synchronized 3-line lyrics overlay (prev / current / next)
                    if self.config.show_lyrics {
                        if let Some(lyr) = lyrics {
                            draw_visualizer_lyrics(painter, rect, &self.config, lyr);
                        }
                    }

                    // Subtle pause indicator when paused (matching website behavior)
                    if self.paused {
                        let badge = Rect::from_min_size(
                            Pos2::new(rect.right() - 110.0, rect.top() + 18.0),
                            Vec2::new(92.0, 26.0),
                        );
                        painter.rect_filled(
                            badge,
                            CornerRadius::same(6),
                            Color32::from_rgba_premultiplied(18, 19, 24, 210),
                        );
                        painter.rect_stroke(
                            badge,
                            CornerRadius::same(6),
                            Stroke::new(1.0_f32, RIM_BRIGHT),
                            StrokeKind::Inside,
                        );
                        painter.text(
                            badge.center(),
                            Align2::CENTER_CENTER,
                            "PAUSED",
                            font_semibold(11.0),
                            TEXT_PRIMARY,
                        );
                    }
                });
        });
    }
}

#[cfg(target_os = "linux")]
fn try_spawn_linux_pipe(
    program: &str,
    args: &[&str],
    ring: Arc<Mutex<CaptureRing>>,
) -> Result<LinuxPipeCapture, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program} konnte nicht gestartet werden: {e}"))?;

    // Brief check to catch immediate exit (e.g. PulseAudio/PipeWire daemon not running)
    std::thread::sleep(std::time::Duration::from_millis(40));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("{program} wurde sofort beendet ({status})"));
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program} stdout nicht verfügbar"))?;

    {
        let mut r = ring.lock();
        r.sample_rate = 48_000;
        r.ring.fill(0.0);
        r.write_idx = 0;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    let handle = match std::thread::Builder::new()
        .name(format!("rustify-{program}-cap"))
        .spawn(move || {
            let mut buf = [0u8; 1024];
            let mut carry = [0u8; 4];
            let mut carry_len = 0usize;
            while !stop_clone.load(Ordering::Relaxed) {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut offset = 0usize;
                        let mut r = ring.lock();
                        if carry_len > 0 {
                            let need = 4 - carry_len;
                            if n >= need {
                                carry[carry_len..4].copy_from_slice(&buf[..need]);
                                let sample = f32::from_le_bytes(carry);
                                if sample.is_finite() {
                                    r.push_sample(sample);
                                }
                                carry_len = 0;
                                offset = need;
                            } else {
                                carry[carry_len..carry_len + n].copy_from_slice(&buf[..n]);
                                carry_len += n;
                                continue;
                            }
                        }
                        let slice = &buf[offset..n];
                        let (chunks, rem) = slice.as_chunks::<4>();
                        for &chunk in chunks {
                            let sample = f32::from_le_bytes(chunk);
                            if sample.is_finite() {
                                r.push_sample(sample);
                            }
                        }
                        if !rem.is_empty() {
                            carry[..rem.len()].copy_from_slice(rem);
                            carry_len = rem.len();
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }) {
        Ok(h) => h,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("Audio-Capture-Thread fehlgeschlagen: {e}"));
        }
    };

    Ok(LinuxPipeCapture {
        stop,
        child: Some(child),
        thread: Some(handle),
    })
}

fn build_external_capture_stream(
    source: VisualizerAudioSource,
    ring: Arc<Mutex<CaptureRing>>,
) -> Result<ExternalCapture, String> {
    #[cfg(target_os = "linux")]
    if source == VisualizerAudioSource::SystemAudio {
        if let Ok(cap) = try_spawn_linux_pipe(
            "parec",
            &[
                "--format=float32le",
                "--channels=1",
                "--rate=48000",
                "--latency-msec=20",
                "-d",
                "@DEFAULT_MONITOR@",
            ],
            Arc::clone(&ring),
        ) {
            return Ok(ExternalCapture::LinuxPipe(cap));
        }
        if let Ok(cap) = try_spawn_linux_pipe(
            "pw-record",
            &[
                "--raw",
                "--format=f32",
                "--rate=48000",
                "--channels=1",
                "-P",
                "stream.capture.sink=true",
                "-",
            ],
            Arc::clone(&ring),
        ) {
            return Ok(ExternalCapture::LinuxPipe(cap));
        }
    }

    let host = cpal::default_host();
    let (device, supported_cfg) = match source {
        VisualizerAudioSource::SystemAudio => {
            #[cfg(target_os = "linux")]
            {
                let monitor_dev = host.input_devices().ok().and_then(|mut devs| {
                    devs.find(|d| {
                        d.name()
                            .is_ok_and(|n| n.to_ascii_lowercase().contains("monitor"))
                    })
                });
                let dev = monitor_dev.ok_or_else(|| {
                    "Kein System-Audio-Monitor gefunden (installiere pipewire-pulse / parec)"
                        .to_string()
                })?;
                let cfg = dev
                    .default_input_config()
                    .map_err(|e| format!("System-Audio-Konfiguration fehlgeschlagen: {e}"))?;
                (dev, cfg)
            }
            #[cfg(not(target_os = "linux"))]
            {
                // On Windows WASAPI, building an input stream on an output device automatically activates WASAPI Loopback!
                let dev = host
                    .default_output_device()
                    .ok_or_else(|| "Kein System-Audio-Ausgabegerät gefunden".to_string())?;
                let cfg = dev
                    .default_output_config()
                    .map_err(|e| format!("System-Audio-Konfiguration fehlgeschlagen: {e}"))?;
                (dev, cfg)
            }
        }
        VisualizerAudioSource::Microphone => {
            let dev = host
                .default_input_device()
                .ok_or_else(|| "Kein Mikrofon gefunden".to_string())?;
            let cfg = dev
                .default_input_config()
                .map_err(|e| format!("Mikrofon-Konfiguration fehlgeschlagen: {e}"))?;
            (dev, cfg)
        }
        VisualizerAudioSource::RustifyApp => {
            return Err("Invalid external source".to_string());
        }
    };

    let channels = supported_cfg.channels().max(1) as usize;
    let sample_rate = supported_cfg.sample_rate().0;
    let sample_format = supported_cfg.sample_format();
    let stream_cfg: StreamConfig = supported_cfg.into();

    {
        let mut r = ring.lock();
        r.sample_rate = sample_rate;
        r.ring.fill(0.0);
        r.write_idx = 0;
    }

    let err_fn = |_err| {};

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_cfg,
            move |data: &[f32], _| {
                let mut r = ring.lock();
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().copied().sum();
                    r.push_sample(sum / (frame.len().max(1) as f32));
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &stream_cfg,
            move |data: &[i16], _| {
                let mut r = ring.lock();
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|&s| s as f32 / 32768.0).sum();
                    r.push_sample(sum / (frame.len().max(1) as f32));
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &stream_cfg,
            move |data: &[u16], _| {
                let mut r = ring.lock();
                for frame in data.chunks(channels) {
                    let sum: f32 = frame
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .sum();
                    r.push_sample(sum / (frame.len().max(1) as f32));
                }
            },
            err_fn,
            None,
        ),
        _ => return Err("Nicht unterstütztes Audio-Sample-Format".to_string()),
    }
    .map_err(|e| format!("Audio-Stream konnte nicht gestartet werden: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Audio-Stream Startfehler: {e}"))?;
    Ok(ExternalCapture::Cpal(stream))
}

#[allow(clippy::too_many_arguments)]
fn draw_visualizer_path(
    painter: &egui::Painter,
    rect: Rect,
    config: &VisualizerConfig,
    data: &[f32],
    start_bin: usize,
    active_range: usize,
    all_time_max_amp: f32,
    alpha: f32,
    scale: f32,
    depth_offset: f32,
    is_main: bool,
) {
    if data.is_empty() || active_range == 0 || rect.width() <= 4.0 || rect.height() <= 4.0 {
        return;
    }

    let width = rect.width();
    let height = rect.height();
    let slice_width = width / (active_range as f32);
    let gain_factor = (height * 0.35) / all_time_max_amp.max(1.0);

    let [r, g, b] = config.color;
    let a_u8 = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let main_col = rgba(r, g, b, a_u8);

    let center_y = rect.center().y;
    let half_width = width * 0.5;
    let depth_factor = depth_offset * 0.04;

    // Smart path decimation algorithm from AudioVizuallizer
    let target_points = if is_main {
        active_range.min(width as usize)
    } else {
        active_range.min((width / 3.0) as usize)
    }
    .max(1);
    let mut step = (active_range / target_points).max(1);
    if config.shape == VisualizerShape::Bars {
        step = 1;
    }
    let actual_slice_width = slice_width * (step as f32);
    let end_bin = (start_bin + active_range).min(data.len());

    if config.shape == VisualizerShape::Bars {
        let bar_width = (actual_slice_width - 1.0).max(1.0);
        let draw_w = (config.line_width * (bar_width * 0.5)).max(1.0);
        let glow_col = rgba(r, g, b, (a_u8 as u16 * 55 / 255) as u8);

        let mut x = 0.0f32;
        let mut i = start_bin;
        while i < end_bin {
            let magnitude = data[i] * gain_factor * scale;
            if magnitude > 0.3 {
                let draw_x = rect.left() + x + depth_factor * (half_width - x);
                let bar_rect = Rect::from_min_size(
                    Pos2::new(draw_x, center_y - magnitude),
                    Vec2::new(draw_w, magnitude * 2.0),
                );
                if is_main {
                    painter.rect_filled(bar_rect.expand(1.8), CornerRadius::ZERO, glow_col);
                }
                painter.rect_filled(bar_rect, CornerRadius::ZERO, main_col);
            }
            x += actual_slice_width;
            i += step;
        }
    } else {
        let cap = (end_bin.saturating_sub(start_bin)) / step + 2;
        let mut upper_pts = Vec::with_capacity(cap);
        let mut lower_pts = if config.shape == VisualizerShape::LineMirrored {
            Vec::with_capacity(cap)
        } else {
            Vec::new()
        };

        let mut x = 0.0f32;
        let mut i = start_bin;
        while i < end_bin {
            let mut magnitude = data[i];
            if is_main && step > 1 {
                let max_j = (i + step).min(data.len());
                for &val in &data[(i + 1)..max_j] {
                    if val > magnitude {
                        magnitude = val;
                    }
                }
            }
            magnitude = magnitude * gain_factor * scale;
            let draw_x = rect.left() + x + depth_factor * (half_width - x);
            upper_pts.push(Pos2::new(draw_x, center_y - magnitude));
            if config.shape == VisualizerShape::LineMirrored {
                lower_pts.push(Pos2::new(draw_x, center_y + magnitude));
            }
            x += actual_slice_width;
            i += step;
        }

        let line_w = if is_main {
            config.line_width
        } else {
            (config.line_width * 0.5).max(0.5)
        };

        if upper_pts.len() >= 2 {
            if is_main {
                // Canvas shadowBlur = 15 emulation
                let glow_outer = rgba(r, g, b, 34);
                let glow_inner = rgba(r, g, b, 76);
                painter.add(Shape::line(
                    upper_pts.clone(),
                    Stroke::new(line_w * 3.4, glow_outer),
                ));
                painter.add(Shape::line(
                    upper_pts.clone(),
                    Stroke::new(line_w * 1.9, glow_inner),
                ));
            }
            painter.add(Shape::line(upper_pts, Stroke::new(line_w, main_col)));
        }

        if config.shape == VisualizerShape::LineMirrored && lower_pts.len() >= 2 {
            if is_main {
                let glow_outer = rgba(r, g, b, 34);
                let glow_inner = rgba(r, g, b, 76);
                painter.add(Shape::line(
                    lower_pts.clone(),
                    Stroke::new(line_w * 3.4, glow_outer),
                ));
                painter.add(Shape::line(
                    lower_pts.clone(),
                    Stroke::new(line_w * 1.9, glow_inner),
                ));
            }
            painter.add(Shape::line(lower_pts, Stroke::new(line_w, main_col)));
        }
    }
}

fn draw_visualizer_lyrics(
    painter: &egui::Painter,
    rect: Rect,
    config: &VisualizerConfig,
    lyrics: &VisualizerLyricsFrame,
) {
    if rect.width() < 120.0 || rect.height() < 100.0 {
        return;
    }

    let max_w = (rect.width() * 0.78)
        .clamp(240.0, 960.0)
        .min((rect.width() - 32.0).max(80.0));
    let s = (rect.height() / 640.0).clamp(0.76, 1.45);

    let side_font = font_regular((16.0 * s).clamp(12.0, 22.0));
    let cur_font = font_semibold((25.0 * s).clamp(18.0, 36.0));

    let prev_col = rgba(210, 214, 224, 138);
    let cur_col = rgba(255, 255, 255, 248);
    let next_col = rgba(210, 214, 224, 152);
    let shadow_col = rgba(0, 0, 0, 175);

    let make_galley = |text: &str, font: egui::FontId, color: Color32| {
        let mut job = egui::text::LayoutJob::single_section(
            text.to_string(),
            egui::text::TextFormat {
                font_id: font,
                color,
                ..Default::default()
            },
        );
        job.halign = egui::Align::Center;
        job.wrap.max_width = max_w;
        painter.layout_job(job)
    };

    let prev_galleys = lyrics.prev.as_deref().map(|t| {
        (
            make_galley(t, side_font.clone(), shadow_col),
            make_galley(t, side_font.clone(), prev_col),
            prev_col,
        )
    });
    let cur_galleys = lyrics.current.as_deref().map(|t| {
        (
            make_galley(t, cur_font.clone(), shadow_col),
            make_galley(t, cur_font.clone(), cur_col),
            cur_col,
        )
    });
    let next_galleys = lyrics.next.as_deref().map(|t| {
        (
            make_galley(t, side_font.clone(), shadow_col),
            make_galley(t, side_font.clone(), next_col),
            next_col,
        )
    });

    if prev_galleys.is_none() && cur_galleys.is_none() && next_galleys.is_none() {
        return;
    }

    let side_min_h = (22.0 * s).clamp(16.0, 32.0);
    let cur_min_h = (34.0 * s).clamp(24.0, 48.0);
    let gap = (7.0 * s).clamp(5.0, 11.0);

    let prev_h = prev_galleys
        .as_ref()
        .map(|(_, g, _)| g.rect.height())
        .unwrap_or(0.0)
        .max(side_min_h);
    let cur_h = cur_galleys
        .as_ref()
        .map(|(_, g, _)| g.rect.height())
        .unwrap_or(0.0)
        .max(cur_min_h);
    let next_h = next_galleys
        .as_ref()
        .map(|(_, g, _)| g.rect.height())
        .unwrap_or(0.0)
        .max(side_min_h);

    let total_h = prev_h + gap + cur_h + gap + next_h;

    let block_top = match config.lyrics_position {
        VisualizerLyricsPosition::Top => rect.top() + (rect.height() * 0.055).clamp(24.0, 54.0),
        VisualizerLyricsPosition::Bottom => {
            rect.bottom() - (rect.height() * 0.06).clamp(26.0, 56.0) - total_h
        }
    };

    let center_x = rect.center().x;
    let prev_y = block_top;
    let cur_y = prev_y + prev_h + gap;
    let next_y = cur_y + cur_h + gap;

    let slots = [
        (prev_galleys, prev_y, prev_h),
        (cur_galleys, cur_y, cur_h),
        (next_galleys, next_y, next_h),
    ];

    let mut placed = Vec::with_capacity(3);
    let mut bounds: Option<Rect> = None;

    for (item, slot_top, slot_h) in slots {
        if let Some((shadow_g, fg_g, fg_col)) = item {
            let gx = center_x - fg_g.rect.center().x;
            let gy = slot_top + (slot_h - fg_g.rect.height()) * 0.5 - fg_g.rect.top();
            let pos = Pos2::new(gx, gy);
            let drawn = fg_g.rect.translate(pos.to_vec2());
            bounds = Some(match bounds {
                Some(b) => b.union(drawn),
                None => drawn,
            });
            placed.push((pos, shadow_g, fg_g, fg_col));
        }
    }

    if let Some(b) = bounds {
        let pill = b.expand2(Vec2::new(24.0, 12.0));
        let bg = config.bg_color;
        let backdrop_col =
            Color32::from_rgba_unmultiplied(bg[0] / 2, bg[1] / 2, bg[2] / 2, 118);
        painter.rect_filled(pill, CornerRadius::same(16), backdrop_col);
    }

    for (pos, shadow_g, fg_g, fg_col) in placed {
        painter.galley(Pos2::new(pos.x, pos.y + 1.5), shadow_g, shadow_col);
        painter.galley(pos, fg_g, fg_col);
    }
}

/// Renders the AudioVisualizer burger-menu settings directly inside the left Sidebar
/// whenever the Visualizer window is open!
pub fn draw_sidebar_visualizer_settings(
    ui: &mut Ui,
    lang: Language,
    viz: &mut AudioVisualizerState,
) -> bool {
    let mut changed = false;
    let is_de = lang == Language::De;

    ui.label(
        egui::RichText::new("VISUALIZER")
            .font(font_semibold(10.5))
            .color(TEXT_MUTED),
    );
    ui.add_space(6.0);

    // 0. Audio Source selector (Rustify App Audio vs. System Audio vs. Microphone)
    ui.label(
        egui::RichText::new(if is_de {
            "AUDIO-QUELLE"
        } else {
            "AUDIO SOURCE"
        })
        .font(font_semibold(9.5))
        .color(TEXT_SECONDARY),
    );
    ui.add_space(3.0);
    let sources = [
        VisualizerAudioSource::RustifyApp,
        VisualizerAudioSource::SystemAudio,
        VisualizerAudioSource::Microphone,
    ];
    egui::ComboBox::from_id_salt("viz_audio_source_combo")
        .width(ui.available_width())
        .selected_text(
            egui::RichText::new(viz.config.audio_source.label(lang))
                .font(font_regular(12.0))
                .color(TEXT_PRIMARY),
        )
        .show_ui(ui, |ui| {
            for src in sources {
                if ui
                    .selectable_label(viz.config.audio_source == src, src.label(lang))
                    .clicked()
                {
                    viz.config.audio_source = src;
                    changed = true;
                }
            }
        });

    if let Some(ref err) = viz.capture_error {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(err)
                .font(font_regular(10.5))
                .color(Color32::from_rgb(235, 115, 115)),
        );
    }

    ui.add_space(8.0);

    // 1. Preset selector
    ui.label(
        egui::RichText::new("PRESET")
            .font(font_semibold(9.5))
            .color(TEXT_SECONDARY),
    );
    ui.add_space(3.0);
    let presets = [
        VisualizerPreset::Classic,
        VisualizerPreset::NeonPulse,
        VisualizerPreset::OceanWaves,
        VisualizerPreset::Custom,
    ];
    egui::ComboBox::from_id_salt("viz_preset_combo")
        .width(ui.available_width())
        .selected_text(
            egui::RichText::new(viz.config.preset.label())
                .font(font_regular(12.0))
                .color(TEXT_PRIMARY),
        )
        .show_ui(ui, |ui| {
            for p in presets {
                if ui
                    .selectable_label(viz.config.preset == p, p.label())
                    .clicked()
                {
                    viz.config.apply_preset(p);
                    viz.trim_history();
                    changed = true;
                }
            }
        });

    // 2. Custom Controls (visible when Preset == Custom, matching #custom-controls on the website)
    if viz.config.preset == VisualizerPreset::Custom {
        ui.add_space(8.0);

        // Line Color & Background Color side by side to save vertical sidebar space
        ui.horizontal(|ui| {
            let half_w = (ui.available_width() - 8.0) * 0.5;
            ui.vertical(|ui| {
                ui.set_width(half_w);
                ui.label(
                    egui::RichText::new(if is_de { "LINIENFARBE" } else { "LINE COLOR" })
                        .font(font_semibold(9.0))
                        .color(TEXT_SECONDARY),
                );
                if ui.color_edit_button_srgb(&mut viz.config.color).changed() {
                    changed = true;
                }
            });
            ui.vertical(|ui| {
                ui.set_width(half_w);
                ui.label(
                    egui::RichText::new(if is_de { "HINTERGRUND" } else { "BG COLOR" })
                        .font(font_semibold(9.0))
                        .color(TEXT_SECONDARY),
                );
                if ui
                    .color_edit_button_srgb(&mut viz.config.bg_color)
                    .changed()
                {
                    changed = true;
                }
            });
        });

        ui.add_space(6.0);

        // Thickness / Size (0.5 .. 8.0, step 0.5)
        draw_label_with_badge(
            ui,
            if is_de { "DICKE / GRÖSSE" } else { "THICKNESS / SIZE" },
            &format!("{:.1}", viz.config.line_width),
        );
        let norm_w = ((viz.config.line_width - 0.5) / 7.5).clamp(0.0, 1.0);
        let (new_w, _) = glass_slider(
            ui,
            "viz_thickness_slider",
            ui.available_width(),
            16.0,
            norm_w,
            None,
            None,
        );
        if let Some(f) = new_w {
            let raw = 0.5 + f * 7.5;
            viz.config.line_width = (raw * 2.0).round() * 0.5;
            changed = true;
        }

        ui.add_space(6.0);

        // Trail Amount (Delay) (0 .. 50)
        draw_label_with_badge(
            ui,
            if is_de {
                "SPUREN (DELAY)"
            } else {
                "TRAIL AMOUNT"
            },
            &viz.config.history_limit.to_string(),
        );
        let norm_h = (viz.config.history_limit as f32 / 50.0).clamp(0.0, 1.0);
        let (new_h, _) = glass_slider(
            ui,
            "viz_history_slider",
            ui.available_width(),
            16.0,
            norm_h,
            None,
            None,
        );
        if let Some(f) = new_h {
            viz.config.history_limit = (f * 50.0).round() as usize;
            viz.trim_history();
            changed = true;
        }

        ui.add_space(6.0);

        // Shape selector
        ui.label(
            egui::RichText::new("SHAPE")
                .font(font_semibold(9.5))
                .color(TEXT_SECONDARY),
        );
        ui.add_space(3.0);
        let shapes = [
            VisualizerShape::LineMirrored,
            VisualizerShape::LineSingle,
            VisualizerShape::Bars,
        ];
        egui::ComboBox::from_id_salt("viz_shape_combo")
            .width(ui.available_width())
            .selected_text(
                egui::RichText::new(viz.config.shape.label())
                    .font(font_regular(12.0))
                    .color(TEXT_PRIMARY),
            )
            .show_ui(ui, |ui| {
                for sh in shapes {
                    if ui
                        .selectable_label(viz.config.shape == sh, sh.label())
                        .clicked()
                    {
                        viz.config.shape = sh;
                        changed = true;
                    }
                }
            });
    }

    ui.add_space(8.0);

    // 3. Frequency Scale selector
    ui.label(
        egui::RichText::new(if is_de {
            "FREQUENZ-SKALA"
        } else {
            "FREQUENCY SCALE"
        })
        .font(font_semibold(9.5))
        .color(TEXT_SECONDARY),
    );
    ui.add_space(3.0);
    let scales = [FreqScale::Dynamic, FreqScale::Fixed];
    egui::ComboBox::from_id_salt("viz_freq_scale_combo")
        .width(ui.available_width())
        .selected_text(
            egui::RichText::new(viz.config.freq_scale.label())
                .font(font_regular(12.0))
                .color(TEXT_PRIMARY),
        )
        .show_ui(ui, |ui| {
            for sc in scales {
                if ui
                    .selectable_label(viz.config.freq_scale == sc, sc.label())
                    .clicked()
                {
                    viz.config.freq_scale = sc;
                    changed = true;
                }
            }
        });

    ui.add_space(10.0);

    // 4. Synchronized Lyrics Overlay toggle + Top / Bottom placement selector
    let lyrics_row_w = ui.available_width();
    let (lyrics_row, _) = ui.allocate_exact_size(Vec2::new(lyrics_row_w, 24.0), Sense::hover());
    ui.painter().text(
        Pos2::new(lyrics_row.left(), lyrics_row.center().y),
        Align2::LEFT_CENTER,
        "LYRICS",
        font_semibold(9.5),
        TEXT_SECONDARY,
    );
    let tog_rect = Rect::from_min_size(
        Pos2::new(lyrics_row.right() - 40.0, lyrics_row.center().y - 11.0),
        Vec2::new(40.0, 22.0),
    );
    let mut tog_ui = ui.new_child(egui::UiBuilder::new().max_rect(tog_rect));
    if glass_toggle(&mut tog_ui, &mut viz.config.show_lyrics).changed() {
        changed = true;
    }

    if viz.config.show_lyrics {
        ui.add_space(6.0);
        let pill_w = ui.available_width();
        let (group_rect, _) = ui.allocate_exact_size(Vec2::new(pill_w, 28.0), Sense::hover());
        ui.painter().rect_filled(
            group_rect,
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(0, 0, 0, 90),
        );
        ui.painter().rect_stroke(
            group_rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_SUBTLE),
            StrokeKind::Inside,
        );

        let seg_w = ((group_rect.width() - 4.0) * 0.5).max(20.0);
        let options = [
            (
                VisualizerLyricsPosition::Top,
                if is_de { "Oben" } else { "Top" },
            ),
            (
                VisualizerLyricsPosition::Bottom,
                if is_de { "Unten" } else { "Bottom" },
            ),
        ];
        for (i, (pos_opt, label)) in options.iter().enumerate() {
            let sel = viz.config.lyrics_position == *pos_opt;
            let sr = Rect::from_min_size(
                Pos2::new(
                    group_rect.left() + 2.0 + i as f32 * seg_w,
                    group_rect.top() + 2.0,
                ),
                Vec2::new(seg_w, 24.0),
            );
            let resp = ui
                .interact(sr, ui.id().with(("viz_lyrics_pos", i)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if sel {
                ui.painter()
                    .rect_filled(sr, CornerRadius::same(6), FROST_CTA_IDLE);
            } else if resp.hovered() {
                ui.painter()
                    .rect_filled(sr, CornerRadius::same(6), rgba(255, 255, 255, 14));
            }
            ui.painter().text(
                sr.center(),
                Align2::CENTER_CENTER,
                *label,
                font_semibold(11.0),
                if sel { FROST_CTA_TEXT } else { TEXT_SECONDARY },
            );
            if resp.clicked() && !sel {
                viz.config.lyrics_position = *pos_opt;
                changed = true;
            }
        }
    }

    changed
}

fn draw_label_with_badge(ui: &mut Ui, label: &str, badge: &str) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 18.0), Sense::hover());
    let p = ui.painter();
    p.text(
        Pos2::new(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font_semibold(9.0),
        TEXT_SECONDARY,
    );
    let galley = p.layout_no_wrap(badge.to_string(), font_semibold(10.0), TEXT_PRIMARY);
    let b_rect = Rect::from_min_max(
        Pos2::new(rect.right() - galley.size().x - 10.0, rect.top() + 1.0),
        Pos2::new(rect.right(), rect.bottom() - 1.0),
    );
    p.rect_filled(b_rect, CornerRadius::same(4), GLASS_PILL_ACTIVE);
    p.rect_stroke(
        b_rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, RIM_SUBTLE),
        StrokeKind::Inside,
    );
    p.galley(
        Pos2::new(
            b_rect.center().x - galley.size().x * 0.5,
            b_rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        TEXT_PRIMARY,
    );
    let _ = GLASS_PILL_IDLE;
}
