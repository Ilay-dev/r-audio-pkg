use crate::config::AppConfig;
use crate::i18n::Language;
use crate::library::Track;
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, glass_icon_button,
    glass_pill_button, rgba, GlassLevel, RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_sec: f32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackLyrics {
    pub synced: Vec<LyricLine>,
    pub plain: Vec<String>,
    pub instrumental: bool,
}

impl TrackLyrics {
    fn has_content(&self) -> bool {
        self.instrumental || !self.synced.is_empty() || self.plain.iter().any(|l| !l.trim().is_empty())
    }
}

#[derive(Debug, Clone)]
pub enum LyricsState {
    Loading,
    Loaded(TrackLyrics),
    NotFound,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisualizerLyricsFrame {
    pub prev: Option<String>,
    pub current: Option<String>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum LyricsAction {
    SeekTo(f32),
    Close,
    RetryFetch,
}

pub struct LyricsManager {
    cache: HashMap<PathBuf, LyricsState>,
    in_flight: HashSet<PathBuf>,
    job_tx: Sender<(PathBuf, String, String, f32, bool)>,
    done_rx: Receiver<(PathBuf, Option<TrackLyrics>)>,
    last_active_idx: Option<usize>,
    last_track_path: Option<PathBuf>,
}

impl Default for LyricsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LyricsManager {
    pub fn new() -> Self {
        let (job_tx, job_rx) = unbounded::<(PathBuf, String, String, f32, bool)>();
        let (done_tx, done_rx) = unbounded::<(PathBuf, Option<TrackLyrics>)>();

        let _ = thread::Builder::new()
            .name("rustify-lyrics".to_string())
            .spawn(move || {
                while let Ok((path, title, artist, duration_sec, force)) = job_rx.recv() {
                    let result = fetch_or_load_lyrics(&path, &title, &artist, duration_sec, force);
                    let _ = done_tx.send((path, result));
                }
            });

        Self {
            cache: HashMap::new(),
            in_flight: HashSet::new(),
            job_tx,
            done_rx,
            last_active_idx: None,
            last_track_path: None,
        }
    }

    pub fn poll(&mut self) {
        while let Ok((path, result)) = self.done_rx.try_recv() {
            self.in_flight.remove(&path);
            let state = match result {
                Some(lyrics) if lyrics.has_content() => LyricsState::Loaded(lyrics),
                _ => LyricsState::NotFound,
            };
            self.cache.insert(path, state);
        }
    }

    pub fn is_loading(&self) -> bool {
        !self.in_flight.is_empty()
    }

    pub fn ensure_fetched(&mut self, track: &Track) {
        if self.cache.contains_key(&track.path) || self.in_flight.contains(&track.path) {
            return;
        }
        self.cache.insert(track.path.clone(), LyricsState::Loading);
        self.in_flight.insert(track.path.clone());
        let _ = self.job_tx.send((
            track.path.clone(),
            track.title.clone(),
            track.artist.clone(),
            track.effective_duration(),
            false,
        ));
    }

    pub fn force_refetch(&mut self, track: &Track) {
        self.cache.insert(track.path.clone(), LyricsState::Loading);
        self.in_flight.insert(track.path.clone());
        self.last_active_idx = None;
        let _ = self.job_tx.send((
            track.path.clone(),
            track.title.clone(),
            track.artist.clone(),
            track.effective_duration(),
            true,
        ));
    }

    pub fn migrate_track_path(&mut self, old_path: &Path, new_path: &Path) {
        let old_cache = lyrics_cache_path(old_path);
        let new_cache = lyrics_cache_path(new_path);
        if old_cache != new_cache && old_cache.is_file() && !new_cache.is_file() {
            if let Some(parent) = new_cache.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::rename(&old_cache, &new_cache);
        }
        if let Some(entry) = self.cache.remove(old_path) {
            self.cache.insert(new_path.to_path_buf(), entry);
        }
        if self.in_flight.remove(old_path) {
            self.in_flight.insert(new_path.to_path_buf());
        }
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.cache.remove(path);
        self.in_flight.remove(path);
    }

    pub fn active_lines_for(
        &self,
        track_path: &Path,
        position_sec: f32,
    ) -> Option<VisualizerLyricsFrame> {
        let LyricsState::Loaded(lyrics) = self.cache.get(track_path)? else {
            return None;
        };
        if lyrics.instrumental || lyrics.synced.is_empty() {
            return None;
        }

        let active_idx = lyrics
            .synced
            .iter()
            .rposition(|line| line.time_sec <= position_sec + 0.16);

        let clean = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };

        let frame = match active_idx {
            Some(idx) => VisualizerLyricsFrame {
                prev: idx.checked_sub(1).and_then(|i| clean(&lyrics.synced[i].text)),
                current: clean(&lyrics.synced[idx].text),
                next: lyrics.synced.get(idx + 1).and_then(|l| clean(&l.text)),
            },
            None => VisualizerLyricsFrame {
                prev: None,
                current: None,
                next: lyrics.synced.first().and_then(|l| clean(&l.text)),
            },
        };

        if frame.prev.is_some() || frame.current.is_some() || frame.next.is_some() {
            Some(frame)
        } else {
            None
        }
    }
}

fn lyrics_cache_path(track_path: &Path) -> PathBuf {
    let stem = track_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let normalized: String = stem.nfc().collect::<String>().trim().to_string();
    let safe_stem: String = normalized
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let final_stem = if safe_stem.is_empty() {
        "unknown".to_string()
    } else {
        safe_stem
    };

    AppConfig::app_data_dir()
        .join("lyrics")
        .join(format!("{final_stem}.json"))
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
                "hq" | "hd" | "4k" | "lyrics" | "audio" | "official" | "video"
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

fn curl_get_json(url: &str) -> Option<serde_json::Value> {
    let safe_url = crate::config::validate_http_url(url).ok()?;
    #[cfg(target_os = "windows")]
    let program = "curl.exe";
    #[cfg(not(target_os = "windows"))]
    let program = "curl";

    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .args(["-s", "-L", "--max-time", "8", "-A", "Rustify/1.0", "--", safe_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    serde_json::from_slice(&output.stdout).ok()
}

fn parse_lrc_timestamp(tag: &str) -> Option<f32> {
    let (min_str, rest) = tag.trim().split_once(':')?;
    let mins: u32 = min_str.parse().ok()?;
    if rest.is_empty() {
        return None;
    }

    let (sec_str, frac_sec) = if let Some((s, f)) = rest.split_once('.').or_else(|| rest.split_once(':')) {
        let frac_digits = f.trim();
        if frac_digits.is_empty() || !frac_digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let frac_val: u32 = frac_digits.parse().ok()?;
        let denom = 10_f32.powi(frac_digits.len() as i32);
        (s, frac_val as f32 / denom)
    } else {
        (rest, 0.0_f32)
    };

    let secs: u32 = sec_str.parse().ok()?;
    Some(mins as f32 * 60.0 + secs as f32 + frac_sec)
}

fn parse_synced_lrc(raw: &str) -> Vec<LyricLine> {
    let mut lines: Vec<LyricLine> = Vec::new();

    for raw_line in raw.lines() {
        let mut slice = raw_line.trim();
        if slice.is_empty() {
            continue;
        }

        let mut timestamps: Vec<f32> = Vec::new();
        while slice.starts_with('[') {
            let Some(end_idx) = slice.find(']') else {
                break;
            };
            let inside = &slice[1..end_idx];
            if let Some(t) = parse_lrc_timestamp(inside) {
                timestamps.push(t);
                slice = slice[end_idx + 1..].trim_start();
            } else {
                break;
            }
        }

        if timestamps.is_empty() {
            continue;
        }

        let text = slice.trim();
        if text.is_empty() {
            continue;
        }

        for time_sec in timestamps {
            lines.push(LyricLine {
                time_sec,
                text: text.to_string(),
            });
        }
    }

    lines.sort_by(|a, b| a.time_sec.total_cmp(&b.time_sec));
    lines
}

fn parse_plain_lyrics(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = raw.lines().map(|l| l.trim().to_string()).collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

fn parse_lrclib_entry(val: &serde_json::Value) -> Option<(TrackLyrics, Option<f32>)> {
    let obj = val.as_object()?;
    let instrumental = obj
        .get("instrumental")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let synced = obj
        .get("syncedLyrics")
        .and_then(|v| v.as_str())
        .map(parse_synced_lrc)
        .unwrap_or_default();

    let plain = obj
        .get("plainLyrics")
        .and_then(|v| v.as_str())
        .map(parse_plain_lyrics)
        .unwrap_or_default();

    let duration = obj
        .get("duration")
        .and_then(|v| v.as_f64())
        .map(|d| d as f32);

    let lyrics = TrackLyrics {
        synced,
        plain,
        instrumental,
    };

    if lyrics.has_content() {
        Some((lyrics, duration))
    } else {
        None
    }
}

fn pick_best_candidate(
    candidates: Vec<(TrackLyrics, Option<f32>)>,
    duration_sec: f32,
) -> Option<TrackLyrics> {
    if candidates.is_empty() {
        return None;
    }

    let has_dur = duration_sec > 1.0;

    let best_by_duration = |items: Vec<(TrackLyrics, Option<f32>)>| -> Option<TrackLyrics> {
        if items.is_empty() {
            return None;
        }
        if !has_dur {
            return items.into_iter().next().map(|(l, _)| l);
        }
        items
            .into_iter()
            .min_by(|(_, d1), (_, d2)| {
                let diff1 = d1.map(|d| (d - duration_sec).abs()).unwrap_or(f32::INFINITY);
                let diff2 = d2.map(|d| (d - duration_sec).abs()).unwrap_or(f32::INFINITY);
                diff1.total_cmp(&diff2)
            })
            .map(|(l, _)| l)
    };

    let synced_only: Vec<_> = candidates
        .iter()
        .filter(|(l, _)| !l.synced.is_empty())
        .cloned()
        .collect();
    if let Some(best) = best_by_duration(synced_only) {
        return Some(best);
    }

    let plain_only: Vec<_> = candidates
        .iter()
        .filter(|(l, _)| l.plain.iter().any(|s| !s.is_empty()))
        .cloned()
        .collect();
    if let Some(best) = best_by_duration(plain_only) {
        return Some(best);
    }

    best_by_duration(candidates)
}

fn fetch_from_lrclib(title: &str, artist: &str, duration_sec: f32) -> Option<TrackLyrics> {
    let trimmed_title = title.trim();
    let trimmed_artist = artist.trim();

    // 1. Direct exact endpoint (/api/get)
    if !trimmed_title.is_empty() && !trimmed_artist.is_empty() {
        let get_url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            url_encode(trimmed_title),
            url_encode(trimmed_artist)
        );
        if let Some(val) = curl_get_json(&get_url) {
            if let Some((lyrics, _)) = parse_lrclib_entry(&val) {
                return Some(lyrics);
            }
        }
    }

    // 2. Fallback search endpoint (/api/search)
    let clean_title = strip_search_tags(trimmed_title);
    let query = if trimmed_artist.is_empty() {
        clean_title
    } else {
        format!("{clean_title} {trimmed_artist}")
    };
    let query = query.trim();
    if query.is_empty() {
        return None;
    }

    let search_url = format!("https://lrclib.net/api/search?q={}", url_encode(query));
    let arr = curl_get_json(&search_url)?.as_array()?.clone();

    let candidates: Vec<(TrackLyrics, Option<f32>)> = arr
        .iter()
        .filter_map(parse_lrclib_entry)
        .collect();

    pick_best_candidate(candidates, duration_sec)
}

fn fetch_or_load_lyrics(
    track_path: &Path,
    title: &str,
    artist: &str,
    duration_sec: f32,
    force: bool,
) -> Option<TrackLyrics> {
    let cache_file = lyrics_cache_path(track_path);

    if !force && cache_file.is_file() {
        if let Ok(bytes) = fs::read(&cache_file) {
            if let Ok(cached) = serde_json::from_slice::<TrackLyrics>(&bytes) {
                if cached.has_content() {
                    return Some(cached);
                }
            }
        }
    }

    let fetched = fetch_from_lrclib(title, artist, duration_sec)?;

    if let Some(parent) = cache_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&fetched) {
        let _ = fs::write(&cache_file, json);
    }

    Some(fetched)
}

struct LyricsCopy {
    header_tag: &'static str,
    no_track: &'static str,
    loading: &'static str,
    instrumental: &'static str,
    not_found: &'static str,
    retry: &'static str,
    refresh_tip: &'static str,
    close_tip: &'static str,
    unknown_artist: &'static str,
    unsynced_note: &'static str,
}

fn copy_for(lang: Language) -> LyricsCopy {
    match lang {
        Language::De => LyricsCopy {
            header_tag: "SONGTEXT",
            no_track: "Kein Titel wird abgespielt",
            loading: "Songtext wird geladen…",
            instrumental: "Instrumental — kein Gesangstext",
            not_found: "Kein Songtext gefunden",
            retry: "Erneut suchen",
            refresh_tip: "Songtext neu laden",
            close_tip: "Schließen",
            unknown_artist: "Unbekannter Künstler",
            unsynced_note: "Unsynchronisierter Songtext",
        },
        Language::En => LyricsCopy {
            header_tag: "LYRICS",
            no_track: "No track currently playing",
            loading: "Loading lyrics…",
            instrumental: "Instrumental — no lyrics",
            not_found: "No lyrics found",
            retry: "Search again",
            refresh_tip: "Refetch lyrics",
            close_tip: "Close",
            unknown_artist: "Unknown Artist",
            unsynced_note: "Unsynced lyrics",
        },
    }
}

pub fn show_lyrics_panel(
    ui: &mut Ui,
    rect: Rect,
    lang: Language,
    current_track: Option<&Track>,
    position_sec: f32,
    lyrics_mgr: &mut LyricsManager,
) -> Option<LyricsAction> {
    let copy = copy_for(lang);
    let mut action: Option<LyricsAction> = None;

    // Reset active scroll tracking when switching tracks and kick off fetch if needed
    let current_path = current_track.map(|t| t.path.clone());
    if lyrics_mgr.last_track_path != current_path {
        lyrics_mgr.last_track_path = current_path;
        lyrics_mgr.last_active_idx = None;
    }
    if let Some(track) = current_track {
        lyrics_mgr.ensure_fetched(track);
    }

    draw_liquid_glass_panel(ui.painter(), rect, 14, GlassLevel::Surface);

    let inner = rect.shrink2(Vec2::new(16.0, 14.0));
    if inner.width() <= 24.0 || inner.height() <= 48.0 {
        return None;
    }

    let mut content = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::LEFT)),
    );

    // === Top Header Bar ===
    let header_h = 54.0_f32;
    let (hdr_rect, _) =
        content.allocate_exact_size(Vec2::new(inner.width(), header_h), Sense::hover());

    // Right buttons (Refresh + Close)
    let mut hdr_right = content.new_child(
        UiBuilder::new()
            .max_rect(hdr_rect)
            .layout(Layout::right_to_left(Align::Center)),
    );

    if glass_icon_button(&mut hdr_right, Icon::Close, 28.0, 11.0, false, false)
        .on_hover_text(copy.close_tip)
        .clicked()
    {
        action = Some(LyricsAction::Close);
    }
    hdr_right.add_space(6.0);
    if glass_icon_button(&mut hdr_right, Icon::Refresh, 28.0, 13.0, false, false)
        .on_hover_text(copy.refresh_tip)
        .clicked()
    {
        action = Some(LyricsAction::RetryFetch);
    }

    // Left text block (SONGTEXT / LYRICS + Title + Artist)
    let text_max_x = (hdr_rect.right() - 72.0).max(hdr_rect.left() + 40.0);
    let tag_rect = Rect::from_min_max(
        Pos2::new(hdr_rect.left(), hdr_rect.top()),
        Pos2::new(text_max_x, hdr_rect.top() + 14.0),
    );
    draw_truncated_text(
        content.painter(),
        tag_rect,
        copy.header_tag,
        font_semibold(10.0),
        TEXT_MUTED,
        Align2::LEFT_CENTER,
    );

    if let Some(track) = current_track {
        let title_rect = Rect::from_min_max(
            Pos2::new(hdr_rect.left(), hdr_rect.top() + 16.0),
            Pos2::new(text_max_x, hdr_rect.top() + 36.0),
        );
        draw_truncated_text(
            content.painter(),
            title_rect,
            &track.title,
            font_semibold(14.5),
            TEXT_PRIMARY,
            Align2::LEFT_CENTER,
        );

        let artist_rect = Rect::from_min_max(
            Pos2::new(hdr_rect.left(), hdr_rect.top() + 36.0),
            Pos2::new(text_max_x, hdr_rect.bottom()),
        );
        draw_truncated_text(
            content.painter(),
            artist_rect,
            track.display_artist(copy.unknown_artist),
            font_regular(11.5),
            TEXT_SECONDARY,
            Align2::LEFT_CENTER,
        );
    } else {
        let sub_rect = Rect::from_min_max(
            Pos2::new(hdr_rect.left(), hdr_rect.top() + 20.0),
            Pos2::new(text_max_x, hdr_rect.bottom()),
        );
        draw_truncated_text(
            content.painter(),
            sub_rect,
            copy.no_track,
            font_regular(12.5),
            TEXT_SECONDARY,
            Align2::LEFT_CENTER,
        );
    }

    content.add_space(10.0);
    let div_y = content.cursor().top();
    content.painter().line_segment(
        [
            Pos2::new(inner.left(), div_y),
            Pos2::new(inner.right(), div_y),
        ],
        Stroke::new(1.0_f32, RIM_SUBTLE),
    );
    content.add_space(10.0);

    // === Body Area ===
    let body_rect = Rect::from_min_max(content.cursor().min, inner.max);
    if body_rect.height() <= 16.0 {
        return action;
    }

    let Some(track) = current_track else {
        draw_centered_message(&mut content, body_rect, Some(Icon::MusicNote), copy.no_track, None);
        return action;
    };

    let state = lyrics_mgr.cache.get(&track.path).cloned();
    match state {
        None | Some(LyricsState::Loading) => {
            content
                .ctx()
                .request_repaint_after(Duration::from_millis(60));
            draw_loading_state(&mut content, body_rect, copy.loading);
        }
        Some(LyricsState::NotFound) => {
            if draw_centered_message(
                &mut content,
                body_rect,
                Some(Icon::MusicNote),
                copy.not_found,
                Some(copy.retry),
            ) {
                action = Some(LyricsAction::RetryFetch);
            }
        }
        Some(LyricsState::Loaded(lyrics)) => {
            if lyrics.instrumental {
                draw_centered_message(
                    &mut content,
                    body_rect,
                    Some(Icon::Disc),
                    copy.instrumental,
                    None,
                );
            } else if !lyrics.synced.is_empty() {
                let active_idx = lyrics
                    .synced
                    .iter()
                    .rposition(|line| line.time_sec <= position_sec + 0.16);
                let active_changed = active_idx != lyrics_mgr.last_active_idx;
                lyrics_mgr.last_active_idx = active_idx;

                egui::ScrollArea::vertical()
                    .id_salt(("lyrics_synced_scroll", &track.path))
                    .max_height(body_rect.height())
                    .auto_shrink([false, false])
                    .show(&mut content, |scroll| {
                        let row_w = (scroll.available_width() - 6.0).max(40.0);
                        scroll.add_space(16.0);

                        for (idx, line) in lyrics.synced.iter().enumerate() {
                            let is_active = active_idx == Some(idx);
                            let is_past = active_idx.is_some_and(|a| idx < a);

                            let (font, base_color, pad_x, pad_y) = if is_active {
                                (font_semibold(21.0), TEXT_PRIMARY, 12.0_f32, 10.0_f32)
                            } else if is_past {
                                (font_semibold(16.0), rgba(245, 246, 248, 88), 12.0_f32, 6.0_f32)
                            } else {
                                (font_semibold(16.5), TEXT_SECONDARY, 12.0_f32, 6.0_f32)
                            };

                            let wrap_w = (row_w - pad_x * 2.0).max(32.0);
                            let measure_galley = scroll.painter().layout(
                                line.text.clone(),
                                font.clone(),
                                base_color,
                                wrap_w,
                            );
                            let row_h = (measure_galley.size().y + pad_y * 2.0).max(28.0);

                            let (row_rect, resp) = scroll
                                .allocate_exact_size(Vec2::new(row_w, row_h), Sense::click());
                            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

                            if scroll.is_rect_visible(row_rect) {
                                let p = scroll.painter();
                                if is_active {
                                    p.rect_filled(
                                        row_rect,
                                        CornerRadius::same(10),
                                        rgba(255, 255, 255, 12),
                                    );
                                    p.rect_stroke(
                                        row_rect,
                                        CornerRadius::same(10),
                                        Stroke::new(1.0_f32, rgba(255, 255, 255, 20)),
                                        StrokeKind::Inside,
                                    );
                                } else if resp.hovered() {
                                    p.rect_filled(
                                        row_rect,
                                        CornerRadius::same(8),
                                        rgba(255, 255, 255, 8),
                                    );
                                }

                                let draw_color = if !is_active && resp.hovered() {
                                    TEXT_PRIMARY
                                } else {
                                    base_color
                                };
                                let galley = if draw_color == base_color {
                                    measure_galley
                                } else {
                                    p.layout(line.text.clone(), font, draw_color, wrap_w)
                                };

                                p.galley(
                                    Pos2::new(row_rect.left() + pad_x, row_rect.top() + pad_y),
                                    galley,
                                    draw_color,
                                );
                            }

                            if is_active && active_changed {
                                resp.scroll_to_me(Some(Align::Center));
                            }

                            if resp.clicked() {
                                action = Some(LyricsAction::SeekTo(line.time_sec));
                            }

                            scroll.add_space(6.0);
                        }

                        scroll.add_space((body_rect.height() * 0.35).max(40.0));
                    });
            } else if lyrics.plain.iter().any(|l| !l.trim().is_empty()) {
                egui::ScrollArea::vertical()
                    .id_salt(("lyrics_plain_scroll", &track.path))
                    .max_height(body_rect.height())
                    .auto_shrink([false, false])
                    .show(&mut content, |scroll| {
                        let wrap_w = (scroll.available_width() - 18.0).max(40.0);
                        scroll.add_space(4.0);
                        scroll.label(
                            egui::RichText::new(copy.unsynced_note)
                                .font(font_regular(11.0))
                                .color(TEXT_MUTED),
                        );
                        scroll.add_space(12.0);

                        for line in &lyrics.plain {
                            if line.trim().is_empty() {
                                scroll.add_space(12.0);
                            } else {
                                let galley = scroll.painter().layout(
                                    line.clone(),
                                    font_semibold(15.5),
                                    TEXT_PRIMARY,
                                    wrap_w,
                                );
                                let (r, _) = scroll.allocate_exact_size(
                                    Vec2::new(wrap_w, galley.size().y + 4.0),
                                    Sense::hover(),
                                );
                                if scroll.is_rect_visible(r) {
                                    scroll.painter().galley(
                                        Pos2::new(r.left() + 4.0, r.top() + 2.0),
                                        galley,
                                        TEXT_PRIMARY,
                                    );
                                }
                                scroll.add_space(4.0);
                            }
                        }

                        scroll.add_space(32.0);
                    });
            } else if draw_centered_message(
                &mut content,
                body_rect,
                Some(Icon::MusicNote),
                copy.not_found,
                Some(copy.retry),
            ) {
                action = Some(LyricsAction::RetryFetch);
            }
        }
    }

    action
}

fn draw_loading_state(ui: &mut Ui, rect: Rect, label: &str) {
    let center = rect.center();
    let t = ui.ctx().input(|i| i.time) as f32;
    let p = ui.painter();

    // Subtle monochrome spinning arc
    let radius = 14.0_f32;
    let ring_center = Pos2::new(center.x, center.y - 16.0);
    p.circle_stroke(ring_center, radius, Stroke::new(2.0_f32, rgba(255, 255, 255, 20)));

    let start_angle = t * 4.2;
    let arc_span = std::f32::consts::PI * 1.15;
    let segments = 18usize;
    let pts: Vec<Pos2> = (0..=segments)
        .map(|i| {
            let a = start_angle + arc_span * (i as f32 / segments as f32);
            Pos2::new(ring_center.x + radius * a.cos(), ring_center.y + radius * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(
        pts,
        Stroke::new(2.0_f32, Color32::from_rgb(235, 238, 245)),
    ));

    p.text(
        Pos2::new(center.x, center.y + 14.0),
        Align2::CENTER_CENTER,
        label,
        font_regular(13.0),
        TEXT_SECONDARY,
    );
}

fn draw_centered_message(
    ui: &mut Ui,
    rect: Rect,
    icon: Option<Icon>,
    message: &str,
    retry_label: Option<&str>,
) -> bool {
    let center = rect.center();
    let mut clicked_retry = false;

    let y_shift = if retry_label.is_some() { -20.0_f32 } else { 0.0_f32 };

    if let Some(ic) = icon {
        let icon_center = Pos2::new(center.x, center.y + y_shift - 20.0);
        ui.painter()
            .circle_filled(icon_center, 22.0, rgba(255, 255, 255, 10));
        ui.painter().circle_stroke(
            icon_center,
            22.0,
            Stroke::new(1.0_f32, RIM_SUBTLE),
        );
        draw_icon(ui.painter(), icon_center, 18.0, ic, TEXT_SECONDARY);
    }

    let wrap_w = (rect.width() - 32.0).max(80.0);
    let galley = ui.painter().layout(
        message.to_string(),
        font_regular(13.5),
        TEXT_SECONDARY,
        wrap_w,
    );
    let text_top = center.y + y_shift + 14.0;
    let text_w = galley.size().x;
    let text_h = galley.size().y;
    ui.painter().galley(
        Pos2::new(center.x - text_w * 0.5, text_top),
        galley,
        TEXT_SECONDARY,
    );

    if let Some(btn_label) = retry_label {
        let btn_y = text_top + text_h + 16.0;
        let btn_area = Rect::from_min_max(
            Pos2::new(rect.left(), btn_y),
            Pos2::new(rect.right(), btn_y + 36.0),
        );
        let mut btn_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(btn_area)
                .layout(Layout::top_down(Align::Center)),
        );
        if glass_pill_button(&mut btn_ui, Some(Icon::Refresh), btn_label, 32.0, false).clicked() {
            clicked_retry = true;
        }
    }

    clicked_retry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_lines_for_synced_progression() {
        let mut mgr = LyricsManager::new();
        let path = PathBuf::from("test_track.mp3");
        mgr.cache.insert(
            path.clone(),
            LyricsState::Loaded(TrackLyrics {
                synced: vec![
                    LyricLine {
                        time_sec: 10.0,
                        text: "  First line  ".to_string(),
                    },
                    LyricLine {
                        time_sec: 20.0,
                        text: "Second line".to_string(),
                    },
                    LyricLine {
                        time_sec: 30.0,
                        text: "Third line".to_string(),
                    },
                ],
                plain: Vec::new(),
                instrumental: false,
            }),
        );

        // Before first lyric timestamp (with 0.16s lead tolerance)
        assert_eq!(
            mgr.active_lines_for(&path, 5.0),
            Some(VisualizerLyricsFrame {
                prev: None,
                current: None,
                next: Some("First line".to_string()),
            })
        );

        // Line 0 active
        assert_eq!(
            mgr.active_lines_for(&path, 10.0),
            Some(VisualizerLyricsFrame {
                prev: None,
                current: Some("First line".to_string()),
                next: Some("Second line".to_string()),
            })
        );

        // Middle line active (prev + current + next)
        assert_eq!(
            mgr.active_lines_for(&path, 22.5),
            Some(VisualizerLyricsFrame {
                prev: Some("First line".to_string()),
                current: Some("Second line".to_string()),
                next: Some("Third line".to_string()),
            })
        );

        // Last line active (prev + current + None)
        assert_eq!(
            mgr.active_lines_for(&path, 45.0),
            Some(VisualizerLyricsFrame {
                prev: Some("Second line".to_string()),
                current: Some("Third line".to_string()),
                next: None,
            })
        );

        // Instrumental returns None
        let inst_path = PathBuf::from("inst.mp3");
        mgr.cache.insert(
            inst_path.clone(),
            LyricsState::Loaded(TrackLyrics {
                synced: vec![LyricLine {
                    time_sec: 1.0,
                    text: "Ignored".to_string(),
                }],
                plain: Vec::new(),
                instrumental: true,
            }),
        );
        assert_eq!(mgr.active_lines_for(&inst_path, 5.0), None);
    }
}
