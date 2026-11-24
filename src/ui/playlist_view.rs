use crate::config::{AppConfig, PlaybackMode};
use crate::covers::CoverManager;
use crate::i18n::{tr, Language};
use crate::library::{format_mm_ss, format_total_duration, Track};
use crate::ui::icons::{draw_icon, draw_live_equalizer, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, glass_icon_button,
    glass_pill_button, lerp_color, rgba, GlassLevel, FROST_CTA_IDLE, FROST_CTA_TEXT,
    GLASS_PILL_IDLE, RIM_BRIGHT, RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::path::PathBuf;

pub struct PlaylistViewInfo<'a> {
    pub title: &'a str,
    pub is_real_playlist: bool,
    pub tracks: &'a [Track],
    pub display_indices: &'a [usize],
    pub total_duration_sec: f32,
    pub sync_source_label: Option<&'a str>,
    pub is_syncing: bool,
}

#[allow(dead_code)]
pub enum PlaylistAction {
    PlayTrackAt(usize),
    TogglePlayPlaylist,
    SetMode(PlaybackMode),
    OpenYouTubeModal,
    OpenImportModal,
    UpdateSyncedPlaylist,
    FetchMissingCovers,
    PickCustomCover(PathBuf),
    RefreshCoverOnline(PathBuf),
    PlayNext(PathBuf),
    AddToQueue(PathBuf),
    OpenRename(PathBuf, String, String),
    ReanalyzeBpm(PathBuf),
    RevealInExplorer(PathBuf),
    RequestDelete(PathBuf, String),
}

#[allow(clippy::too_many_arguments)]
pub fn show_playlist_view(
    ui: &mut Ui,
    rect: Rect,
    config: &AppConfig,
    info: &PlaylistViewInfo<'_>,
    covers: &mut CoverManager,
    search_query: &mut String,
    focus_search: &mut bool,
    current_playing_path: Option<&PathBuf>,
    is_playing: bool,
    time: f32,
    bass_energy: f32,
    dragging_files: bool,
) -> Option<PlaylistAction> {
    let s = tr(config.language);
    let mut action = None;

    draw_liquid_glass_panel(ui.painter(), rect, 14, GlassLevel::Surface);

    let inner = rect.shrink2(Vec2::new(26.0, 20.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    // === A. Spotify Hero Banner (136px height) ===
    let hero_h = 136.0;
    let (hero_rect, _) =
        child.allocate_exact_size(Vec2::new(inner.width(), hero_h), Sense::hover());

    let cover_rect = Rect::from_min_size(
        Pos2::new(hero_rect.left(), hero_rect.top() + 4.0),
        Vec2::splat(124.0),
    );

    // Collect up to 4 available cover textures from playlist tracks
    let mut cover_textures = Vec::with_capacity(4);
    for t in info.tracks {
        if let Some(tex) = covers.get_texture(child.ctx(), &t.path) {
            cover_textures.push(tex);
            if cover_textures.len() >= 4 {
                break;
            }
        }
    }

    {
        let p = child.painter();
        // Soft ambient drop shadow behind the 124x124 playlist cover
        p.rect_filled(
            cover_rect.expand(8.0).translate(Vec2::new(0.0, 5.0)),
            CornerRadius::same(13),
            Color32::from_rgba_unmultiplied(0, 0, 0, 42),
        );
        p.rect_filled(
            cover_rect.expand(3.0).translate(Vec2::new(0.0, 2.5)),
            CornerRadius::same(10),
            Color32::from_rgba_unmultiplied(0, 0, 0, 75),
        );

        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        if cover_textures.len() >= 4 {
            // Crisp 2x2 mosaic (62x62 each) inside the rounded 124x124 cover rect
            p.rect_filled(cover_rect, CornerRadius::same(8), Color32::from_rgb(18, 20, 26));
            let clipped = p.with_clip_rect(cover_rect.shrink(0.5));
            let mid = cover_rect.center();
            let q0 = Rect::from_min_max(cover_rect.min, mid);
            let q1 = Rect::from_min_max(Pos2::new(mid.x, cover_rect.top()), Pos2::new(cover_rect.right(), mid.y));
            let q2 = Rect::from_min_max(Pos2::new(cover_rect.left(), mid.y), Pos2::new(mid.x, cover_rect.bottom()));
            let q3 = Rect::from_min_max(mid, cover_rect.max);

            clipped.image(cover_textures[0].id(), q0, uv, Color32::WHITE);
            clipped.image(cover_textures[1].id(), q1, uv, Color32::WHITE);
            clipped.image(cover_textures[2].id(), q2, uv, Color32::WHITE);
            clipped.image(cover_textures[3].id(), q3, uv, Color32::WHITE);

            p.rect_stroke(
                cover_rect,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_BRIGHT),
                StrokeKind::Inside,
            );
        } else if let Some(first_tex) = cover_textures.first() {
            p.image(first_tex.id(), cover_rect, uv, Color32::WHITE);
            p.rect_stroke(
                cover_rect,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_BRIGHT),
                StrokeKind::Inside,
            );
        } else {
            p.rect_filled(
                cover_rect,
                CornerRadius::same(8),
                Color32::from_rgb(18, 20, 26),
            );
            p.rect_stroke(
                cover_rect,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
            draw_icon(p, cover_rect.center(), 36.0, Icon::MusicNote, TEXT_MUTED);
        }

        // Right of cover: Spotify bottom-aligned hero typography
        let text_x = cover_rect.right() + 22.0;
        let eyebrow = if info.is_real_playlist {
            s.playlist_label
        } else {
            s.smart_playlist_label
        };
        p.text(
            Pos2::new(text_x, cover_rect.top() + 18.0),
            Align2::LEFT_CENTER,
            eyebrow,
            font_semibold(11.5),
            TEXT_SECONDARY,
        );

        let title_rect = Rect::from_min_max(
            Pos2::new(text_x, cover_rect.top() + 36.0),
            Pos2::new(hero_rect.right(), cover_rect.top() + 88.0),
        );
        draw_truncated_text(
            p,
            title_rect,
            info.title,
            font_semibold(40.0),
            TEXT_PRIMARY,
            Align2::LEFT_CENTER,
        );

        let dur_str = format_total_duration(
            info.total_duration_sec,
            config.language == Language::De,
        );
        let source_prefix = match info.sync_source_label {
            Some(src) => format!("Rustify · {src}"),
            None => "Rustify".to_string(),
        };
        let meta_line = format!(
            "{}  •  {} {}, {}",
            source_prefix,
            info.tracks.len(),
            s.songs_word,
            dur_str
        );
        p.text(
            Pos2::new(text_x, cover_rect.bottom() - 18.0),
            Align2::LEFT_CENTER,
            meta_line,
            font_regular(12.5),
            TEXT_SECONDARY,
        );
    }

    // === B. Spotify Sub-Header Action Bar (46px height) ===
    let playlist_is_active = current_playing_path
        .map(|cp| info.tracks.iter().any(|t| &t.path == cp))
        .unwrap_or(false);

    let (controls_rect, _) =
        child.allocate_exact_size(Vec2::new(inner.width(), 46.0), Sense::hover());
    let mut ctrl_ui = child.new_child(
        egui::UiBuilder::new()
            .max_rect(controls_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );

    let play_icon = if playlist_is_active && is_playing {
        Icon::Pause
    } else {
        Icon::Play
    };
    if glass_icon_button(&mut ctrl_ui, play_icon, 44.0, 17.0, true, true).clicked() {
        action = Some(PlaylistAction::TogglePlayPlaylist);
    }

    ctrl_ui.add_space(12.0);

    // Uniform [ Normal | Shuffle | DJ-Mix ] mode switcher (32px height, 8px radius)
    if let Some(new_mode) =
        draw_mode_segmented_control(&mut ctrl_ui, config.playback_mode, config.language)
    {
        action = Some(PlaylistAction::SetMode(new_mode));
    }

    if info.is_real_playlist {
        ctrl_ui.add_space(8.0);
        if glass_pill_button(&mut ctrl_ui, Some(Icon::Plus), s.add_youtube, 32.0, false).clicked()
        {
            action = Some(PlaylistAction::OpenYouTubeModal);
        }

        if let Some(src_lbl) = info.sync_source_label {
            ctrl_ui.add_space(8.0);
            let upd_lbl = if info.is_syncing {
                "Sync..."
            } else {
                "Update"
            };
            let upd_tip = if config.language == Language::De {
                format!("Neue Songs aus {src_lbl}-Playlist automatisch über YouTube laden")
            } else {
                format!("Fetch newly added songs from {src_lbl} playlist via YouTube")
            };
            if glass_pill_button(
                &mut ctrl_ui,
                Some(Icon::Refresh),
                upd_lbl,
                32.0,
                info.is_syncing,
            )
            .on_hover_text(upd_tip)
            .clicked()
            {
                action = Some(PlaylistAction::UpdateSyncedPlaylist);
            }
        }
    }

    ctrl_ui.add_space(8.0);
    let cover_prog = covers.progress();
    let cover_btn_label = if let Some((done, total)) = cover_prog {
        format!("{} {done}/{total}", s.fetching_covers)
    } else {
        s.fetch_covers.to_string()
    };
    if glass_pill_button(
        &mut ctrl_ui,
        Some(Icon::Image),
        &cover_btn_label,
        32.0,
        cover_prog.is_some(),
    )
    .clicked()
    {
        action = Some(PlaylistAction::FetchMissingCovers);
    }

    // Right-aligned uniform 32px / 8px-radius Search box
    ctrl_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let search_w = (ui.available_width() - 6.0).clamp(115.0, 200.0);
        let (srect, _) = ui.allocate_exact_size(Vec2::new(search_w, 32.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(srect, CornerRadius::same(8), GLASS_PILL_IDLE);
        p.rect_stroke(
            srect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_SUBTLE),
            StrokeKind::Inside,
        );
        draw_icon(
            p,
            Pos2::new(srect.left() + 14.0, srect.center().y),
            12.5,
            Icon::Search,
            TEXT_MUTED,
        );

        let clear_space = if search_query.is_empty() { 8.0 } else { 26.0 };
        let input_rect = Rect::from_min_max(
            Pos2::new(srect.left() + 28.0, srect.top() + 6.0),
            Pos2::new(srect.right() - clear_space, srect.bottom() - 4.0),
        );
        let mut inp_ui = ui.new_child(egui::UiBuilder::new().max_rect(input_rect));
        let resp = inp_ui.add(
            egui::TextEdit::singleline(search_query)
                .id(Id::new("playlist_search_box"))
                .hint_text(egui::RichText::new(s.filter_placeholder).color(TEXT_MUTED))
                .frame(false)
                .desired_width(f32::INFINITY)
                .font(font_regular(12.5)),
        );
        if *focus_search {
            resp.request_focus();
            *focus_search = false;
        }
        if !search_query.is_empty() {
            let clr_rect = Rect::from_center_size(
                Pos2::new(srect.right() - 14.0, srect.center().y),
                Vec2::splat(20.0),
            );
            let mut clr_ui = ui.new_child(egui::UiBuilder::new().max_rect(clr_rect));
            if glass_icon_button(&mut clr_ui, Icon::Close, 20.0, 9.0, false, false).clicked() {
                search_query.clear();
            }
        }
    });

    child.add_space(10.0);

    // === C. Spotify Track Table Header (#, Titel, Clock icon) ===
    let col_h = 24.0;
    let (col_rect, _) = child.allocate_exact_size(Vec2::new(inner.width(), col_h), Sense::hover());
    {
        let p = child.painter();
        p.text(
            Pos2::new(col_rect.left() + 18.0, col_rect.center().y),
            Align2::CENTER_CENTER,
            "#",
            font_semibold(11.0),
            TEXT_MUTED,
        );
        p.text(
            Pos2::new(col_rect.left() + 38.0, col_rect.center().y),
            Align2::LEFT_CENTER,
            s.col_title,
            font_semibold(10.5),
            TEXT_MUTED,
        );
        draw_icon(
            p,
            Pos2::new(col_rect.right() - 56.0, col_rect.center().y),
            13.0,
            Icon::Clock,
            TEXT_MUTED,
        );
        p.line_segment(
            [
                Pos2::new(col_rect.left(), col_rect.bottom()),
                Pos2::new(col_rect.right(), col_rect.bottom()),
            ],
            Stroke::new(1.0_f32, rgba(255, 255, 255, 14)),
        );
    }

    child.add_space(4.0);

    // === D. Spotify Track Rows (54px row height) ===
    let list_h = (inner.bottom() - child.cursor().min.y).max(60.0);

    if info.tracks.is_empty() {
        let empty_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), list_h));
        let p = child.painter();
        p.text(
            Pos2::new(empty_rect.center().x, empty_rect.center().y - 10.0),
            Align2::CENTER_CENTER,
            s.empty_playlist_title,
            font_semibold(15.0),
            TEXT_PRIMARY,
        );
        p.text(
            Pos2::new(empty_rect.center().x, empty_rect.center().y + 14.0),
            Align2::CENTER_CENTER,
            s.empty_playlist_hint,
            font_regular(12.5),
            TEXT_SECONDARY,
        );
    } else if info.display_indices.is_empty() {
        let empty_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), list_h));
        child.painter().text(
            empty_rect.center(),
            Align2::CENTER_CENTER,
            s.no_search_results,
            font_regular(13.5),
            TEXT_SECONDARY,
        );
    } else {
        egui::ScrollArea::vertical()
            .id_salt(("track_list", info.title))
            .max_height(list_h)
            .auto_shrink([false, false])
            .show_rows(
                &mut child,
                54.0,
                info.display_indices.len(),
                |ui, row_range| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for row_pos in row_range {
                        let track_idx = info.display_indices[row_pos];
                        let track = &info.tracks[track_idx];
                        let is_current = current_playing_path
                            .map(|cp| cp == &track.path)
                            .unwrap_or(false);

                        let cover_tex = covers.get_texture(ui.ctx(), &track.path);

                        if let Some(act) = draw_track_row(
                            ui,
                            row_pos + 1,
                            track_idx,
                            track,
                            cover_tex.as_ref(),
                            is_current,
                            is_playing && is_current,
                            config.language,
                            time,
                            bass_energy,
                        ) {
                            action = Some(act);
                        }
                    }
                },
            );
    }

    if dragging_files && info.is_real_playlist {
        let p = ui.painter();
        p.rect_filled(
            rect,
            CornerRadius::same(14),
            Color32::from_rgba_unmultiplied(8, 9, 12, 220),
        );
        p.rect_stroke(
            rect.shrink(12.0),
            CornerRadius::same(10),
            Stroke::new(1.5_f32, RIM_BRIGHT),
            StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            s.drop_files_hint,
            font_semibold(14.5),
            TEXT_PRIMARY,
        );
    }

    action
}

fn draw_mode_segmented_control(
    ui: &mut Ui,
    current: PlaybackMode,
    lang: Language,
) -> Option<PlaybackMode> {
    let s = tr(lang);
    let items = [
        (PlaybackMode::Normal, s.mode_normal, s.mode_normal_tip),
        (PlaybackMode::Shuffle, s.mode_shuffle, s.mode_shuffle_tip),
        (PlaybackMode::DjMix, s.mode_dj, s.mode_dj_tip),
    ];

    let seg_w = 72.0;
    let total_w = seg_w * 3.0 + 6.0;
    let h = 32.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(total_w, h), Sense::hover());

    // Uniform 8px-rounded dark glass container matching buttons & search box
    ui.painter()
        .rect_filled(outer, CornerRadius::same(8), GLASS_PILL_IDLE);
    ui.painter().rect_stroke(
        outer,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, RIM_SUBTLE),
        StrokeKind::Inside,
    );

    let mut chosen = None;
    for (i, (mode, label, tip)) in items.iter().enumerate() {
        let seg_rect = Rect::from_min_size(
            Pos2::new(outer.left() + 3.0 + i as f32 * seg_w, outer.top() + 3.0),
            Vec2::new(seg_w, h - 6.0),
        );
        let resp = ui
            .interact(seg_rect, ui.id().with(("mode_seg", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(*tip);

        let selected = current == *mode;
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
        let p = ui.painter();

        if selected {
            p.rect_filled(seg_rect, CornerRadius::same(6), FROST_CTA_IDLE);
        } else if hover > 0.01 {
            p.rect_filled(
                seg_rect,
                CornerRadius::same(6),
                Color32::from_rgba_unmultiplied(255, 255, 255, (hover * 14.0) as u8),
            );
        }

        let col = if selected {
            FROST_CTA_TEXT
        } else {
            lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hover)
        };

        p.text(
            seg_rect.center(),
            Align2::CENTER_CENTER,
            *label,
            font_semibold(12.0),
            col,
        );

        if resp.clicked() && !selected {
            chosen = Some(*mode);
        }
    }

    chosen
}

#[allow(clippy::too_many_arguments)]
fn draw_track_row(
    ui: &mut Ui,
    display_num: usize,
    track_idx: usize,
    track: &Track,
    cover_tex: Option<&egui::TextureHandle>,
    is_current: bool,
    is_playing: bool,
    lang: Language,
    time: f32,
    bass_energy: f32,
) -> Option<PlaylistAction> {
    let s = tr(lang);
    let mut action = None;
    let row_w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(row_w, 54.0), Sense::click());

    let more_rect = Rect::from_center_size(
        Pos2::new(rect.right() - 20.0, rect.center().y),
        Vec2::splat(28.0),
    );
    let more_resp = ui
        .interact(
            more_rect,
            ui.id().with(("more_btn", &track.path)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    let active_menu_key = Id::new("rustify_open_track_menu");
    let open_path: Option<PathBuf> = ui.ctx().data(|d| d.get_temp(active_menu_key));
    let popup_open = open_path.as_deref() == Some(track.path.as_path());

    let row_hovered = resp.hovered() || more_resp.hovered() || popup_open;
    let hover = ui.ctx().animate_bool_with_time(resp.id, row_hovered, 0.12);
    let more_hover = ui
        .ctx()
        .animate_bool_with_time(more_resp.id, more_resp.hovered() || popup_open, 0.12);

    if ui.is_rect_visible(rect) {
        let p = ui.painter();

        if is_current {
            p.rect_filled(
                rect,
                CornerRadius::same(8),
                rgba(255, 255, 255, 16),
            );
            p.rect_stroke(
                rect,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_BRIGHT),
                StrokeKind::Inside,
            );
            let pill = Rect::from_min_max(
                Pos2::new(rect.left() + 3.5, rect.top() + 12.0),
                Pos2::new(rect.left() + 6.0, rect.bottom() - 12.0),
            );
            p.rect_filled(pill, CornerRadius::same(2), Color32::WHITE);
        } else if hover > 0.01 {
            p.rect_filled(
                rect,
                CornerRadius::same(8),
                rgba(255, 255, 255, (hover * 11.0) as u8),
            );
        }

        // Column 1 (x = rect.left() + 18.0): Track number 1, 2, 3 / Play-Pause on hover / Live Equalizer
        let idx_center = Pos2::new(rect.left() + 18.0, rect.center().y);
        if is_current && !row_hovered {
            draw_live_equalizer(
                p,
                idx_center,
                13.5,
                time,
                is_playing,
                bass_energy,
                Color32::WHITE,
            );
        } else if row_hovered {
            draw_icon(
                p,
                idx_center,
                12.5,
                if is_playing { Icon::Pause } else { Icon::Play },
                TEXT_PRIMARY,
            );
        } else {
            p.text(
                idx_center,
                Align2::CENTER_CENTER,
                display_num.to_string(),
                font_regular(12.5),
                TEXT_MUTED,
            );
        }

        // Column 2 (x = rect.left() + 38.0 .. 78.0): 40x40 Square Track Cover Thumbnail
        let art_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 58.0, rect.center().y),
            Vec2::splat(40.0),
        );
        if let Some(tex) = cover_tex {
            p.image(
                tex.id(),
                art_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            p.rect_stroke(
                art_rect,
                CornerRadius::same(4),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
        } else {
            p.rect_filled(
                art_rect,
                CornerRadius::same(4),
                Color32::from_rgb(18, 20, 25),
            );
            p.rect_stroke(
                art_rect,
                CornerRadius::same(4),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
            draw_icon(p, art_rect.center(), 14.0, Icon::MusicNote, TEXT_MUTED);
        }

        // Column 3 (x = rect.left() + 90.0 .. rect.right() - 96.0): Song Title + Artist below
        let text_left = rect.left() + 90.0;
        let text_right = (rect.right() - 96.0).max(text_left + 80.0);

        let title_rect = Rect::from_min_max(
            Pos2::new(text_left, rect.top() + 8.0),
            Pos2::new(text_right, rect.top() + 28.0),
        );
        draw_truncated_text(
            p,
            title_rect,
            &track.title,
            font_semibold(14.0),
            if is_current {
                Color32::WHITE
            } else {
                TEXT_PRIMARY
            },
            Align2::LEFT_CENTER,
        );

        let artist_rect = Rect::from_min_max(
            Pos2::new(text_left, rect.top() + 28.0),
            Pos2::new(text_right, rect.bottom() - 8.0),
        );
        draw_truncated_text(
            p,
            artist_rect,
            track.display_artist(s.unknown_artist),
            font_regular(12.0),
            if is_current {
                Color32::from_rgb(190, 195, 205)
            } else {
                TEXT_SECONDARY
            },
            Align2::LEFT_CENTER,
        );

        // Column 4 (x = rect.right() - 48.0): Duration (right-aligned)
        let dur_text = if track.effective_duration() > 0.0 {
            format_mm_ss(track.effective_duration())
        } else {
            "--:--".to_string()
        };
        p.text(
            Pos2::new(rect.right() - 48.0, rect.center().y),
            Align2::RIGHT_CENTER,
            dur_text,
            font_regular(12.0),
            if is_current {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            },
        );

        // Column 5 (x = rect.right() - 20.0): 3-Dots Menu Button (•••, Icon::MoreHorizontal)
        if hover > 0.02 || popup_open {
            if more_hover > 0.01 {
                p.rect_filled(
                    more_rect,
                    CornerRadius::same(6),
                    rgba(255, 255, 255, (more_hover * 18.0) as u8),
                );
            }
            let more_col = lerp_color(
                TEXT_SECONDARY,
                TEXT_PRIMARY,
                more_hover,
            );
            draw_icon(p, more_rect.center(), 15.0, Icon::MoreHorizontal, more_col);
        }
    }

    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if more_resp.clicked() || resp.secondary_clicked() {
        ui.ctx().data_mut(|d| {
            if popup_open {
                d.remove::<PathBuf>(active_menu_key);
            } else {
                d.insert_temp(active_menu_key, track.path.clone());
            }
        });
    } else if resp.clicked() {
        action = Some(PlaylistAction::PlayTrackAt(track_idx));
    }

    if popup_open {
        show_track_dropdown_menu(
            ui.ctx(),
            active_menu_key,
            more_rect,
            track,
            s,
            &mut action,
        );
    }

    action
}

fn show_track_dropdown_menu(
    ctx: &egui::Context,
    active_menu_key: Id,
    anchor_rect: Rect,
    track: &Track,
    s: &crate::i18n::Strings,
    action: &mut Option<PlaylistAction>,
) {
    let menu_w = 232.0;
    let item_h = 32.0;
    // 7 items + 2 separators (10px each) + 12px vertical padding
    let menu_h = 7.0 * item_h + 2.0 * 10.0 + 12.0;
    let screen = ctx.screen_rect();

    let x = (anchor_rect.right() - menu_w).clamp(12.0, (screen.right() - menu_w - 12.0).max(12.0));
    let y = if anchor_rect.bottom() + 6.0 + menu_h <= screen.bottom() - 84.0 {
        anchor_rect.bottom() + 4.0
    } else {
        (anchor_rect.top() - menu_h - 4.0).max(42.0)
    };
    let card_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(menu_w, menu_h));

    let mut close_menu = false;

    egui::Area::new(Id::new(("track_dropdown_area", &track.path)))
        .order(egui::Order::Foreground)
        .fixed_pos(card_rect.min)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(card_rect.size(), Sense::click());
            let p = ui.painter();

            // Multi-pass drop shadow
            p.rect_filled(
                rect.expand(10.0).translate(Vec2::new(0.0, 6.0)),
                CornerRadius::same(14),
                Color32::from_rgba_unmultiplied(0, 0, 0, 65),
            );
            p.rect_filled(
                rect.expand(4.0).translate(Vec2::new(0.0, 2.0)),
                CornerRadius::same(12),
                Color32::from_rgba_unmultiplied(0, 0, 0, 110),
            );
            // Solid opaque dark obsidian menu card (zero see-through bleed!)
            p.rect_filled(rect, CornerRadius::same(10), Color32::from_rgb(15, 16, 21));
            p.rect_stroke(
                rect,
                CornerRadius::same(10),
                Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 57)),
                StrokeKind::Inside,
            );

            let inner = rect.shrink2(Vec2::new(6.0, 6.0));
            let mut m_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            m_ui.spacing_mut().item_spacing.y = 0.0;

            if menu_row(&mut m_ui, Icon::Next, s.ctx_play_next, false) {
                *action = Some(PlaylistAction::PlayNext(track.path.clone()));
                close_menu = true;
            }
            if menu_row(&mut m_ui, Icon::Queue, s.ctx_add_queue, false) {
                *action = Some(PlaylistAction::AddToQueue(track.path.clone()));
                close_menu = true;
            }

            menu_separator(&mut m_ui);

            if menu_row(&mut m_ui, Icon::Image, s.ctx_custom_cover, false) {
                *action = Some(PlaylistAction::PickCustomCover(track.path.clone()));
                close_menu = true;
            }
            if menu_row(&mut m_ui, Icon::Refresh, s.ctx_fetch_cover_online, false) {
                *action = Some(PlaylistAction::RefreshCoverOnline(track.path.clone()));
                close_menu = true;
            }
            if menu_row(&mut m_ui, Icon::Edit, s.ctx_rename, false) {
                *action = Some(PlaylistAction::OpenRename(
                    track.path.clone(),
                    track.title.clone(),
                    track.artist.clone(),
                ));
                close_menu = true;
            }
            if menu_row(&mut m_ui, Icon::Folder, s.ctx_reveal_explorer, false) {
                *action = Some(PlaylistAction::RevealInExplorer(track.path.clone()));
                close_menu = true;
            }

            menu_separator(&mut m_ui);

            if menu_row(&mut m_ui, Icon::Trash, s.ctx_delete, true) {
                *action = Some(PlaylistAction::RequestDelete(
                    track.path.clone(),
                    track.title.clone(),
                ));
                close_menu = true;
            }
        });

    let clicked_outside = ctx.input(|i| {
        if i.key_pressed(egui::Key::Escape) {
            return true;
        }
        if i.pointer.any_pressed() {
            if let Some(pos) = i.pointer.interact_pos() {
                if !card_rect.contains(pos) && !anchor_rect.contains(pos) {
                    return true;
                }
            }
        }
        false
    });

    if close_menu || clicked_outside {
        ctx.data_mut(|d| d.remove::<PathBuf>(active_menu_key));
    }
}

fn menu_row(ui: &mut Ui, icon: Icon, label: &str, danger: bool) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();

    if hovered {
        let fill = if danger {
            Color32::from_rgb(48, 22, 24)
        } else {
            Color32::from_rgb(28, 31, 39)
        };
        p.rect_filled(rect, CornerRadius::same(6), fill);
    }

    let fg = if danger {
        if hovered {
            Color32::from_rgb(255, 130, 130)
        } else {
            Color32::from_rgb(235, 108, 108)
        }
    } else if hovered {
        TEXT_PRIMARY
    } else {
        Color32::from_rgb(214, 218, 226)
    };

    draw_icon(
        p,
        Pos2::new(rect.left() + 15.0, rect.center().y),
        13.0,
        icon,
        fg,
    );
    p.text(
        Pos2::new(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font_regular(12.5),
        fg,
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn menu_separator(ui: &mut Ui) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.left() + 8.0, rect.center().y),
            Pos2::new(rect.right() - 8.0, rect.center().y),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(34, 37, 46)),
    );
}
