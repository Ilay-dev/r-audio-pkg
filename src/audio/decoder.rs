use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Fast header-only probe returning duration in seconds (and optional embedded artwork PNG/JPEG bytes).
pub struct ProbeInfo {
    pub duration_sec: Option<f32>,
    pub embedded_art: Option<Vec<u8>>,
}

pub fn probe_track_info(path: &Path) -> ProbeInfo {
    let Ok(file) = File::open(path) else {
        return ProbeInfo {
            duration_sec: None,
            embedded_art: None,
        };
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let Ok(mut probed) = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) else {
        return ProbeInfo {
            duration_sec: None,
            embedded_art: None,
        };
    };

    let mut embedded_art = None;
    if let Some(meta) = probed.metadata.get() {
        if let Some(rev) = meta.current() {
            if let Some(vis) = rev.visuals().first() {
                if !vis.data.is_empty() && vis.data.len() < 4 * 1024 * 1024 {
                    embedded_art = Some(vis.data.to_vec());
                }
            }
        }
    }

    let format = &mut probed.format;
    if embedded_art.is_none() {
        if let Some(rev) = format.metadata().current() {
            if let Some(vis) = rev.visuals().first() {
                if !vis.data.is_empty() && vis.data.len() < 4 * 1024 * 1024 {
                    embedded_art = Some(vis.data.to_vec());
                }
            }
        }
    }

    let duration_sec = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .and_then(|track| {
            let sr = track.codec_params.sample_rate? as f64;
            if sr <= 0.0 {
                return None;
            }
            track.codec_params.n_frames.map(|n_frames| (n_frames as f64 / sr) as f32)
        });

    ProbeInfo {
        duration_sec,
        embedded_art,
    }
}

/// Decoded stereo audio buffer resampled to the output device's sample rate.
/// Stored as interleaved stereo `i16` (`[L0, R0, L1, R1, ...]`) to keep memory footprint low
/// while allowing instant sample-accurate seeking & real-time cubic tempo interpolation.
pub struct DecodedAudio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub total_frames: usize,
    pub duration_sec: f32,
}

impl DecodedAudio {
    /// Fetch stereo sample at fractional frame index using 4-point Catmull-Rom cubic interpolation.
    #[inline]
    pub fn sample_cubic(&self, pos: f64) -> (f32, f32) {
        if self.total_frames == 0 || pos < 0.0 {
            return (0.0, 0.0);
        }
        let max_f = self.total_frames.saturating_sub(1);
        let i1 = pos.floor() as usize;
        if i1 >= self.total_frames {
            return (0.0, 0.0);
        }
        let frac = (pos - i1 as f64) as f32;

        let i0 = i1.saturating_sub(1);
        let i2 = (i1 + 1).min(max_f);
        let i3 = (i1 + 2).min(max_f);

        const SCALE: f32 = 1.0 / 32768.0;
        let l0 = self.samples[i0 * 2] as f32 * SCALE;
        let l1 = self.samples[i1 * 2] as f32 * SCALE;
        let l2 = self.samples[i2 * 2] as f32 * SCALE;
        let l3 = self.samples[i3 * 2] as f32 * SCALE;

        let r0 = self.samples[i0 * 2 + 1] as f32 * SCALE;
        let r1 = self.samples[i1 * 2 + 1] as f32 * SCALE;
        let r2 = self.samples[i2 * 2 + 1] as f32 * SCALE;
        let r3 = self.samples[i3 * 2 + 1] as f32 * SCALE;

        (
            catmull_rom(l0, l1, l2, l3, frac),
            catmull_rom(r0, r1, r2, r3, frac),
        )
    }
}

#[inline]
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * y1)
        + (-y0 + y2) * t
        + (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3) * t2
        + (-y0 + 3.0 * y1 - 3.0 * y2 + y3) * t3)
}

#[inline]
fn f32_to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

fn append_stereo_f32(buf: &AudioBufferRef<'_>, out: &mut Vec<f32>) {
    let channels = buf.spec().channels.count();
    let frames = buf.frames();
    if channels == 0 || frames == 0 {
        return;
    }
    out.reserve(frames * 2);

    match buf {
        AudioBufferRef::F32(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i]);
                out.push(r[i]);
            }
        }
        AudioBufferRef::F64(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i] as f32);
                out.push(r[i] as f32);
            }
        }
        AudioBufferRef::S16(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i] as f32 / 32768.0);
                out.push(r[i] as f32 / 32768.0);
            }
        }
        AudioBufferRef::S32(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i] as f32 / 2147483648.0);
                out.push(r[i] as f32 / 2147483648.0);
            }
        }
        AudioBufferRef::S24(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i].inner() as f32 / 8388608.0);
                out.push(r[i].inner() as f32 / 8388608.0);
            }
        }
        AudioBufferRef::U8(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push((l[i] as f32 - 128.0) / 128.0);
                out.push((r[i] as f32 - 128.0) / 128.0);
            }
        }
        AudioBufferRef::U16(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push((l[i] as f32 - 32768.0) / 32768.0);
                out.push((r[i] as f32 - 32768.0) / 32768.0);
            }
        }
        AudioBufferRef::U24(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push((l[i].inner() as f32 - 8388608.0) / 8388608.0);
                out.push((r[i].inner() as f32 - 8388608.0) / 8388608.0);
            }
        }
        AudioBufferRef::U32(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(((l[i] as f64 - 2147483648.0) / 2147483648.0) as f32);
                out.push(((r[i] as f64 - 2147483648.0) / 2147483648.0) as f32);
            }
        }
        AudioBufferRef::S8(b) => {
            let l = b.chan(0);
            let r = if channels > 1 { b.chan(1) } else { l };
            for i in 0..frames {
                out.push(l[i] as f32 / 128.0);
                out.push(r[i] as f32 / 128.0);
            }
        }
    }
}

/// Decode an entire audio file to stereo interleaved `i16` at `target_sr`.
pub fn decode_to_stereo_i16(path: &Path, target_sr: u32) -> Result<DecodedAudio, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("Unsupported audio format: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "No decodable audio track found".to_string())?;

    let track_id = track.id;
    let src_sr = track.codec_params.sample_rate.unwrap_or(target_sr);
    let est_frames = track.codec_params.n_frames.unwrap_or(0) as usize;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Failed to create audio decoder: {e}"))?;

    let mut raw_stereo: Vec<f32> = Vec::with_capacity(est_frames.min(48000 * 600) * 2);
    let mut detected_sr = src_sr;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(ref err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                detected_sr = audio_buf.spec().rate;
                append_stereo_f32(&audio_buf, &mut raw_stereo);
            }
            Err(Error::DecodeError(_)) => continue, // skip corrupt frame gracefully
            Err(_) => break,
        }
    }

    let src_frames = raw_stereo.len() / 2;
    if src_frames == 0 {
        return Err("Audio file produced no samples".to_string());
    }

    let samples: Vec<i16> = if detected_sr == target_sr {
        raw_stereo.into_iter().map(f32_to_i16).collect()
    } else {
        // High-quality cubic resampling to device native sample rate
        let ratio = detected_sr as f64 / target_sr as f64;
        let out_frames = ((src_frames as f64) / ratio).round() as usize;
        let mut resampled = Vec::with_capacity(out_frames * 2);
        let max_f = src_frames.saturating_sub(1);
        for out_i in 0..out_frames {
            let pos = out_i as f64 * ratio;
            let i1 = (pos.floor() as usize).min(max_f);
            let frac = (pos - i1 as f64) as f32;
            let i0 = i1.saturating_sub(1);
            let i2 = (i1 + 1).min(max_f);
            let i3 = (i1 + 2).min(max_f);

            let l = catmull_rom(
                raw_stereo[i0 * 2],
                raw_stereo[i1 * 2],
                raw_stereo[i2 * 2],
                raw_stereo[i3 * 2],
                frac,
            );
            let r = catmull_rom(
                raw_stereo[i0 * 2 + 1],
                raw_stereo[i1 * 2 + 1],
                raw_stereo[i2 * 2 + 1],
                raw_stereo[i3 * 2 + 1],
                frac,
            );
            resampled.push(f32_to_i16(l));
            resampled.push(f32_to_i16(r));
        }
        resampled
    };

    let total_frames = samples.len() / 2;
    let duration_sec = total_frames as f32 / target_sr as f32;

    Ok(DecodedAudio {
        samples,
        sample_rate: target_sr,
        total_frames,
        duration_sec,
    })
}

/// Fast mono downsampled decode (~22050 Hz) for BPM, beat-grid & mix-point analysis.
pub fn decode_mono_for_analysis(path: &Path) -> Result<(Vec<f32>, u32, f32), String> {
    const ANALYSIS_SR: u32 = 22050;
    let file = File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("Probe error: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "No audio track".to_string())?;

    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Decoder error: {e}"))?;

    let mut stereo_buf: Vec<f32> = Vec::with_capacity(4096);
    let mut mono_out: Vec<f32> = Vec::with_capacity(ANALYSIS_SR as usize * 240);
    let mut detected_sr = track.codec_params.sample_rate.unwrap_or(44100);
    let mut total_src_frames: usize = 0;

    // Decimate on the fly with simple box averaging (fast anti-aliasing for 44.1/48kHz -> ~22kHz)
    let mut phase: f64 = 0.0;
    let mut acc: f32 = 0.0;
    let mut acc_n: u32 = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(ref err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                detected_sr = audio_buf.spec().rate;
                stereo_buf.clear();
                append_stereo_f32(&audio_buf, &mut stereo_buf);
                let frames = stereo_buf.len() / 2;
                total_src_frames += frames;

                let step = ANALYSIS_SR as f64 / detected_sr.max(8000) as f64;
                for i in 0..frames {
                    let m = 0.5 * (stereo_buf[i * 2] + stereo_buf[i * 2 + 1]);
                    acc += m;
                    acc_n += 1;
                    phase += step;
                    if phase >= 1.0 {
                        mono_out.push(acc / acc_n as f32);
                        acc = 0.0;
                        acc_n = 0;
                        phase -= 1.0;
                    }
                }
            }
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        }
    }

    if mono_out.is_empty() {
        return Err("Empty audio".to_string());
    }

    let duration_sec = total_src_frames as f32 / detected_sr.max(1) as f32;
    Ok((mono_out, ANALYSIS_SR, duration_sec))
}
