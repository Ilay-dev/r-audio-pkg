use crate::config::AppConfig;
use crate::covers::{has_cover_on_disk, save_cover_bytes_for_track};
use crate::i18n::{tr, Language};
use crate::library::{format_mm_ss, sanitize_filename_component};
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, frosted_cta_button,
    glass_icon_button, glass_pill_button, rgba, GlassLevel, GLASS_PILL_IDLE, RIM_BRIGHT,
    RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use crate::youtube::{
    resolve_ffmpeg, resolve_ytdlp, spawn_download, spawn_fetch_info, spawn_youtube_search,
    DownloadEvent, InfoEvent, SearchEvent, YouTubeSearchResult,
};
use crossbeam_channel::Receiver;
use egui::{
    Align2, Color32, ColorImage, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use std::path::PathBuf;

pub struct YouTubeModalState {
    pub open: bool,
    pub target_playlist_name: String,
    pub target_playlist_dir: PathBuf,

    // YouTube top-5 search state
    pub search_query: String,
    pub searching: bool,
    pub search_rx: Option<Receiver<SearchEvent>>,
    pub search_results: Vec<YouTubeSearchResult>,
    pub search_textures: Vec<Option<TextureHandle>>,
    pub selected_thumbnail_bytes: Option<Vec<u8>>,

    // Direct URL & metadata state
    pub url: String,
    pub title: String,
    pub artist: String,
    pub fetching_info: bool,
    pub info_rx: Option<Receiver<InfoEvent>>,
    pub downloading: bool,
    pub status_text: String,
    pub progress: f32,
    pub converting: bool,
    pub error: Option<String>,
    pub download_rx: Option<Receiver<DownloadEvent>>,
}

impl Default for YouTubeModalState {
    fn default() -> Self {
        Self {
            open: false,
            target_playlist_name: String::new(),
            target_playlist_dir: PathBuf::new(),
            search_query: String::new(),
            searching: false,
            search_rx: None,
            search_results: Vec::new(),
            search_textures: Vec::new(),
            selected_thumbnail_bytes: None,
            url: String::new(),
            title: String::new(),
            artist: String::new(),
            fetching_info: false,
            info_rx: None,
            downloading: false,
            status_text: String::new(),
            progress: 0.0,
            converting: false,
            error: None,
            download_rx: None,
        }
    }
}

impl YouTubeModalState {
    pub fn open_for(&mut self, playlist_name: String, playlist_dir: PathBuf) {
        self.open = true;
        self.target_playlist_name = playlist_name;
        self.target_playlist_dir = playlist_dir;
        if !self.downloading {
            self.url.clear();
            self.title.clear();
            self.artist.clear();
            self.error = None;
            self.progress = 0.0;
            self.converting = false;
            self.status_text.clear();
            self.selected_thumbnail_bytes = None;
        }
    }

    pub fn poll(&mut self, config: &AppConfig) -> Option<(PathBuf, PathBuf, String)> {
        let s = tr(config.language);

        if let Some(ref rx) = self.search_rx {
            while let Ok(ev) = rx.try_recv() {
                self.searching = false;
                match ev {
                    SearchEvent::Results(results) => {
                        self.search_textures = vec![None; results.len()];
                        self.search_results = results;
                        self.error = None;
                    }
                    SearchEvent::Error(e) => {
                        self.error = Some(e);
                    }
                }
            }
        }
        if !self.searching {
            self.search_rx = None;
        }

        if let Some(ref rx) = self.info_rx {
            while let Ok(ev) = rx.try_recv() {
                self.fetching_info = false;
                match ev {
                    InfoEvent::Loaded { title, artist } => {
                        self.title = title;
                        self.artist = artist;
                        self.error = None;
                    }
                    InfoEvent::Error(e) => {
                        self.error = Some(e);
                    }
                }
            }
        }
        if !self.fetching_info {
            self.info_rx = None;
        }

        let mut completed = None;
        let mut finished_dl = false;
        if let Some(ref rx) = self.download_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    DownloadEvent::FetchingInfo => {
                        self.status_text = s.yt_fetching.to_string();
                        self.converting = false;
                    }
                    DownloadEvent::Downloading(p) => {
                        self.progress = p;
                        self.converting = false;
                        self.status_text = format!("{} {:.0}%", s.yt_downloading, p * 100.0);
                    }
                    DownloadEvent::Converting => {
                        self.progress = 0.96;
                        self.converting = true;
                        self.status_text = s.yt_converting.to_string();
                    }
                    DownloadEvent::Completed {
                        playlist_dir,
                        saved_path,
                        display_name,
                    } => {
                        if let Some(ref thumb_bytes) = self.selected_thumbnail_bytes {
                            if !has_cover_on_disk(&saved_path) {
                                let _ = save_cover_bytes_for_track(&saved_path, thumb_bytes);
                            }
                        }
                        self.selected_thumbnail_bytes = None;
                        self.downloading = false;
                        self.converting = false;
                        self.progress = 1.0;
                        self.error = None;
                        self.open = false;
                        self.url.clear();
                        self.title.clear();
                        self.artist.clear();
                        finished_dl = true;
                        completed = Some((playlist_dir, saved_path, display_name));
                    }
                    DownloadEvent::Error(e) => {
                        self.downloading = false;
                        self.converting = false;
                        self.error = Some(e);
                        finished_dl = true;
                    }
                }
            }
        }
        if finished_dl {
            self.download_rx = None;
        }
        completed
    }
}

pub fn show_youtube_modal(
    ui: &mut Ui,
    screen: Rect,
    config: &AppConfig,
    state: &mut YouTubeModalState,
) {
    if !state.open {
        return;
    }
    let s = tr(config.language);
    let is_de = config.language == Language::De;

    // Upload decoded search thumbnails to GPU textures on demand
    for (idx, item) in state.search_results.iter().enumerate() {
        if state.search_textures.get(idx).and_then(|t| t.as_ref()).is_none() {
            if let Some((w, h, ref rgba_bytes)) = item.thumbnail_rgba {
                let color_img =
                    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba_bytes);
                let tex = ui.ctx().load_texture(
                    format!("yt_thumb_{}", item.id),
                    color_img,
                    TextureOptions::LINEAR,
                );
                if idx < state.search_textures.len() {
                    state.search_textures[idx] = Some(tex);
                }
            }
        }
    }

    let scrim_resp = ui.interact(screen, ui.id().with("yt_modal_scrim"), Sense::click());
    ui.painter().rect_filled(
        screen,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(0, 0, 0, 175),
    );

    let has_results = !state.search_results.is_empty() || state.searching;
    let base_h: f32 = if has_results { 660.0 } else { 510.0 };
    let card_w = 550.0f32.min(screen.width() - 40.0);
    let card_h = base_h.min(screen.height() - 36.0);
    let card_rect = Rect::from_center_size(screen.center(), Vec2::new(card_w, card_h));

    if scrim_resp.clicked()
        && !card_rect.contains(scrim_resp.interact_pointer_pos().unwrap_or(Pos2::ZERO))
    {
        state.open = false;
        return;
    }

    draw_liquid_glass_panel(ui.painter(), card_rect, 18, GlassLevel::Modal);

    let inner = card_rect.shrink2(Vec2::new(26.0, 20.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    // Header
    child.horizontal(|ui| {
        ui.label(
            egui::RichText::new(s.yt_modal_title)
                .font(font_semibold(17.5))
                .color(TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if glass_icon_button(ui, Icon::Close, 28.0, 11.5, false, false).clicked() {
                state.open = false;
            }
        });
    });

    child.add_space(2.0);
    child.label(
        egui::RichText::new(format!(
            "{}: {}",
            s.yt_target_playlist, state.target_playlist_name
        ))
        .font(font_regular(12.0))
        .color(TEXT_SECONDARY),
    );
    child.add_space(12.0);

    let ytdlp_bin = resolve_ytdlp(config.ytdlp_path.as_ref());
    let ffmpeg_bin = resolve_ffmpeg(config.ffmpeg_path.as_ref());

    if ytdlp_bin.is_none() {
        let warn_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), 44.0));
        child.allocate_exact_size(warn_rect.size(), Sense::hover());
        child.painter().rect_filled(
            warn_rect,
            CornerRadius::same(10),
            Color32::from_rgba_unmultiplied(150, 45, 45, 70),
        );
        child.painter().rect_stroke(
            warn_rect,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(240, 110, 110, 90)),
            StrokeKind::Inside,
        );
        let mut w_ui = child.new_child(egui::UiBuilder::new().max_rect(warn_rect.shrink(10.0)));
        w_ui.label(
            egui::RichText::new(s.yt_missing_tool)
                .font(font_regular(11.5))
                .color(TEXT_PRIMARY),
        );
        child.add_space(8.0);
    }

    // === 1. YOUTUBE SEARCH BAR + TOP 5 RESULTS WITH COVERS ===
    child.label(
        egui::RichText::new(if is_de {
            "AUF YOUTUBE SUCHEN (TOP 5 ERGEBNISSE)"
        } else {
            "SEARCH ON YOUTUBE (TOP 5 RESULTS)"
        })
        .font(font_semibold(10.0))
        .color(TEXT_MUTED),
    );
    child.add_space(5.0);

    let mut trigger_search = false;
    child.horizontal(|ui| {
        let input_w = (inner.width() - 98.0).max(160.0);
        let (box_r, _) = ui.allocate_exact_size(Vec2::new(input_w, 34.0), Sense::hover());
        ui.painter()
            .rect_filled(box_r, CornerRadius::same(8), GLASS_PILL_IDLE);
        ui.painter().rect_stroke(
            box_r,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_BRIGHT),
            StrokeKind::Inside,
        );
        draw_icon(
            ui.painter(),
            Pos2::new(box_r.left() + 14.0, box_r.center().y),
            12.5,
            Icon::Search,
            TEXT_MUTED,
        );

        let edit_rect = Rect::from_min_max(
            Pos2::new(box_r.left() + 28.0, box_r.top() + 6.0),
            Pos2::new(box_r.right() - 8.0, box_r.bottom() - 6.0),
        );
        let mut inp_ui = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
        let hint = if is_de {
            "Songtitel oder Interpret suchen..."
        } else {
            "Search song title or artist..."
        };
        let te = inp_ui.add_enabled(
            !state.downloading,
            egui::TextEdit::singleline(&mut state.search_query)
                .hint_text(egui::RichText::new(hint).color(TEXT_MUTED))
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(font_regular(12.5)),
        );
        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            trigger_search = true;
        }

        ui.add_space(6.0);
        let can_search = !state.downloading
            && !state.searching
            && !state.search_query.trim().is_empty()
            && ytdlp_bin.is_some();
        ui.add_enabled_ui(can_search, |ui| {
            let btn_lbl = if state.searching {
                "..."
            } else if is_de {
                "Suchen"
            } else {
                "Search"
            };
            if glass_pill_button(ui, None, btn_lbl, 34.0, false).clicked() {
                trigger_search = true;
            }
        });
    });

    if trigger_search && !state.searching && !state.search_query.trim().is_empty() {
        if let Some(ref bin) = ytdlp_bin {
            state.searching = true;
            state.error = None;
            state.search_rx = Some(spawn_youtube_search(
                bin.clone(),
                state.search_query.trim().to_string(),
            ));
        }
    }

    if state.searching {
        child.add_space(8.0);
        let wait_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), 38.0));
        child.allocate_exact_size(wait_rect.size(), Sense::hover());
        child.painter().rect_filled(
            wait_rect,
            CornerRadius::same(8),
            rgba(255, 255, 255, 8),
        );
        child.painter().text(
            wait_rect.center(),
            Align2::CENTER_CENTER,
            if is_de {
                "Suche Top 5 Ergebnisse & Covers auf YouTube..."
            } else {
                "Searching Top 5 results & covers on YouTube..."
            },
            font_regular(12.0),
            TEXT_SECONDARY,
        );
    } else if !state.search_results.is_empty() {
        child.add_space(8.0);
        let results_h = (state.search_results.len() as f32 * 48.0).min(242.0);
        let results_box =
            Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), results_h));
        let mut clicked_result_idx: Option<usize> = None;

        for (i, item) in state.search_results.iter().enumerate() {
            let (row_r, row_resp) = child.allocate_exact_size(
                Vec2::new(inner.width(), 46.0),
                if state.downloading {
                    Sense::hover()
                } else {
                    Sense::click()
                },
            );
            let hovered = row_resp.hovered() && !state.downloading;
            let is_selected = state.url == item.url;
            let p = child.painter();

            let row_bg = if is_selected {
                rgba(255, 255, 255, 20)
            } else if hovered {
                rgba(255, 255, 255, 13)
            } else {
                rgba(255, 255, 255, 6)
            };
            p.rect_filled(row_r, CornerRadius::same(8), row_bg);
            p.rect_stroke(
                row_r,
                CornerRadius::same(8),
                Stroke::new(
                    1.0_f32,
                    if is_selected || hovered {
                        RIM_BRIGHT
                    } else {
                        RIM_SUBTLE
                    },
                ),
                StrokeKind::Inside,
            );

            // Cover Thumbnail (36x36)
            let thumb_rect = Rect::from_center_size(
                Pos2::new(row_r.left() + 23.0, row_r.center().y),
                Vec2::splat(36.0),
            );
            if let Some(Some(ref tex)) = state.search_textures.get(i) {
                p.image(
                    tex.id(),
                    thumb_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                p.rect_stroke(
                    thumb_rect,
                    CornerRadius::same(5),
                    Stroke::new(1.0_f32, RIM_SUBTLE),
                    StrokeKind::Inside,
                );
            } else {
                p.rect_filled(
                    thumb_rect,
                    CornerRadius::same(5),
                    Color32::from_rgb(18, 20, 26),
                );
                draw_icon(p, thumb_rect.center(), 14.0, Icon::YouTube, TEXT_MUTED);
            }

            // Title & Uploader/Channel
            let text_left = thumb_rect.right() + 10.0;
            let text_right = (row_r.right() - 62.0).max(text_left + 80.0);

            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, row_r.top() + 5.0),
                    Pos2::new(text_right, row_r.center().y + 1.0),
                ),
                &item.raw_title,
                font_semibold(12.5),
                TEXT_PRIMARY,
                Align2::LEFT_CENTER,
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, row_r.center().y + 1.0),
                    Pos2::new(text_right, row_r.bottom() - 4.0),
                ),
                &item.channel,
                font_regular(11.0),
                TEXT_SECONDARY,
                Align2::LEFT_CENTER,
            );

            // Duration on the right
            if let Some(d) = item.duration_sec {
                if d > 0.0 {
                    p.text(
                        Pos2::new(row_r.right() - 12.0, row_r.center().y),
                        Align2::RIGHT_CENTER,
                        format_mm_ss(d),
                        font_regular(11.5),
                        if hovered { TEXT_PRIMARY } else { TEXT_MUTED },
                    );
                }
            }

            if !state.downloading
                && row_resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            {
                clicked_result_idx = Some(i);
            }
            child.add_space(2.0);
        }

        // Clicking a search result selects it AND immediately starts downloading it as MP3!
        if let Some(idx) = clicked_result_idx {
            if let Some(item) = state.search_results.get(idx).cloned() {
                state.url = item.url.clone();
                state.title = item.parsed_title.clone();
                state.artist = item.parsed_artist.clone();
                state.selected_thumbnail_bytes = item.thumbnail_raw_bytes.clone();

                if let Some(ref bin) = ytdlp_bin {
                    state.downloading = true;
                    state.error = None;
                    state.progress = 0.02;
                    state.status_text = s.yt_downloading.to_string();
                    state.download_rx = Some(spawn_download(
                        bin.clone(),
                        ffmpeg_bin.clone(),
                        state.url.trim().to_string(),
                        state.target_playlist_dir.clone(),
                        state.title.clone(),
                        state.artist.clone(),
                        config.yt_mp3_bitrate,
                    ));
                }
            }
        }
        let _ = results_box;
    }

    child.add_space(10.0);

    // === 2. DIRECT YOUTUBE LINK & OPTIONAL TITLE/ARTIST NAMING ===
    child.label(
        egui::RichText::new(s.yt_url_label.to_uppercase())
            .font(font_semibold(10.0))
            .color(TEXT_MUTED),
    );
    child.add_space(4.0);

    child.horizontal(|ui| {
        let input_w = (inner.width() - 98.0).max(160.0);
        let (box_r, _) = ui.allocate_exact_size(Vec2::new(input_w, 34.0), Sense::hover());
        ui.painter()
            .rect_filled(box_r, CornerRadius::same(8), GLASS_PILL_IDLE);
        ui.painter().rect_stroke(
            box_r,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_SUBTLE),
            StrokeKind::Inside,
        );

        let mut inp_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(box_r.shrink2(Vec2::new(10.0, 6.0))));
        inp_ui.add_enabled(
            !state.downloading,
            egui::TextEdit::singleline(&mut state.url)
                .hint_text(egui::RichText::new(s.yt_url_placeholder).color(TEXT_MUTED))
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(font_regular(12.5)),
        );

        ui.add_space(6.0);
        let can_fetch = !state.downloading
            && !state.fetching_info
            && !state.url.trim().is_empty()
            && ytdlp_bin.is_some();
        ui.add_enabled_ui(can_fetch, |ui| {
            let lbl = if state.fetching_info {
                "..."
            } else {
                s.yt_fetch_info
            };
            if glass_pill_button(ui, None, lbl, 34.0, false).clicked() {
                if let Some(ref bin) = ytdlp_bin {
                    state.fetching_info = true;
                    state.error = None;
                    state.info_rx = Some(spawn_fetch_info(bin.clone(), state.url.trim().to_string()));
                }
            }
        });
    });

    child.add_space(10.0);

    let half_w = (inner.width() - 10.0) * 0.5;
    child.horizontal(|ui| {
        draw_labeled_input(ui, half_w, s.field_title, &mut state.title, !state.downloading);
        ui.add_space(10.0);
        draw_labeled_input(ui, half_w, s.field_artist, &mut state.artist, !state.downloading);
    });

    child.add_space(10.0);

    // Live Filename Preview
    let clean_t = sanitize_filename_component(&state.title);
    let clean_a = sanitize_filename_component(&state.artist);
    let preview_name = if !clean_t.is_empty() && !clean_a.is_empty() {
        format!("{clean_t} - {clean_a}.mp3")
    } else if !clean_t.is_empty() {
        format!("{clean_t}.mp3")
    } else if is_de {
        "Automatisch (Titel - Interpret.mp3)".to_string()
    } else {
        "Auto (Title - Artist.mp3)".to_string()
    };
    let prev_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), 28.0));
    child.allocate_exact_size(prev_rect.size(), Sense::hover());
    child
        .painter()
        .rect_filled(prev_rect, CornerRadius::same(8), rgba(255, 255, 255, 6));
    child.painter().rect_stroke(
        prev_rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, RIM_SUBTLE),
        StrokeKind::Inside,
    );
    draw_truncated_text(
        child.painter(),
        prev_rect.shrink2(Vec2::new(10.0, 0.0)),
        &format!("{}:  {preview_name}", s.yt_filename_preview),
        font_regular(11.5),
        TEXT_SECONDARY,
        Align2::LEFT_CENTER,
    );

    // Dedicated Progress / Error zone strictly ABOVE the bottom button bar (never collides!)
    let status_zone = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - 82.0),
        Pos2::new(inner.right(), inner.bottom() - 46.0),
    );
    if state.downloading {
        let p = ui.painter();
        p.text(
            Pos2::new(status_zone.left(), status_zone.top() + 8.0),
            Align2::LEFT_CENTER,
            &state.status_text,
            font_semibold(12.0),
            TEXT_PRIMARY,
        );
        let bar_r = Rect::from_min_max(
            Pos2::new(status_zone.left(), status_zone.bottom() - 12.0),
            Pos2::new(status_zone.right(), status_zone.bottom() - 6.0),
        );
        p.rect_filled(bar_r, CornerRadius::same(3), rgba(255, 255, 255, 22));
        let fill_w = bar_r.width() * state.progress.clamp(0.03, 1.0);
        let fill_r = Rect::from_min_size(bar_r.min, Vec2::new(fill_w, bar_r.height()));
        p.rect_filled(fill_r, CornerRadius::same(3), Color32::WHITE);
    } else if let Some(ref err) = state.error {
        draw_truncated_text(
            ui.painter(),
            status_zone,
            err,
            font_regular(11.5),
            Color32::from_rgb(235, 115, 115),
            Align2::LEFT_CENTER,
        );
    }

    // Bottom Buttons (cleanly separated at the bottom)
    let footer = Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - 36.0), inner.max);
    let mut f_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(footer)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );

    if !state.downloading {
        let can_dl = !state.url.trim().is_empty() && ytdlp_bin.is_some();
        f_ui.add_enabled_ui(can_dl, |ui| {
            if frosted_cta_button(ui, None, s.yt_download, 34.0).clicked() {
                if let Some(bin) = ytdlp_bin {
                    state.downloading = true;
                    state.error = None;
                    state.progress = 0.02;
                    state.status_text = s.yt_downloading.to_string();
                    state.download_rx = Some(spawn_download(
                        bin,
                        ffmpeg_bin,
                        state.url.trim().to_string(),
                        state.target_playlist_dir.clone(),
                        state.title.clone(),
                        state.artist.clone(),
                        config.yt_mp3_bitrate,
                    ));
                }
            }
        });
        f_ui.add_space(10.0);
    }

    if glass_pill_button(&mut f_ui, None, s.yt_close, 34.0, false).clicked() {
        state.open = false;
    }
}

fn draw_labeled_input(ui: &mut Ui, width: f32, label: &str, value: &mut String, enabled: bool) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .font(font_semibold(10.0))
                .color(TEXT_MUTED),
        );
        ui.add_space(4.0);
        let (box_r, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::hover());
        ui.painter()
            .rect_filled(box_r, CornerRadius::same(8), GLASS_PILL_IDLE);
        ui.painter().rect_stroke(
            box_r,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_SUBTLE),
            StrokeKind::Inside,
        );
        let mut inp =
            ui.new_child(egui::UiBuilder::new().max_rect(box_r.shrink2(Vec2::new(10.0, 5.0))));
        inp.add_enabled(
            enabled,
            egui::TextEdit::singleline(value)
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(font_regular(12.5)),
        );
    });
}
