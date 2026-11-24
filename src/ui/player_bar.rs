use crate::audio::EngineSnapshot;
use crate::config::{AppConfig, PlaybackMode, RepeatMode};
use crate::i18n::tr;
use crate::library::{format_mm_ss, Track};
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, glass_icon_button,
    glass_seek_slider, glass_slider, GlassLevel, RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};

pub enum PlayerAction {
    TogglePlay,
    Prev,
    Next,
    Seek(f32),
    CycleMode,
    CycleRepeat,
    ToggleLyrics,
    ToggleQueuePopup,
    EnterMiniMode,
    SetVolume(f32),
    ToggleMute,
}

#[allow(clippy::too_many_arguments)]
pub fn show_player_bar(
    ui: &mut Ui,
    rect: Rect,
    config: &AppConfig,
    current_track: Option<&Track>,
    next_track: Option<&Track>,
    snap: &EngineSnapshot,
    current_cover: Option<&egui::TextureHandle>,
    lyrics_open: bool,
    queue_open: bool,
    user_queue_len: usize,
) -> Option<PlayerAction> {
    let s = tr(config.language);
    let mut action = None;

    draw_liquid_glass_panel(ui.painter(), rect, 16, GlassLevel::Elevated);

    let body_rect = rect.shrink2(Vec2::new(16.0, 8.0));
    let total_w = body_rect.width();
    let left_w = (total_w * 0.28).clamp(210.0, 330.0);
    let right_w = (total_w * 0.25).clamp(228.0, 265.0);
    let center_w = (total_w - left_w - right_w - 28.0).max(240.0);

    // --- LEFT: 52x52 Cover Art + Song Title & Artist / Live DJ Mix Status ---
    let left_rect = Rect::from_min_size(body_rect.min, Vec2::new(left_w, body_rect.height()));
    {
        let p = ui.painter();
        let art_rect = Rect::from_center_size(
            Pos2::new(left_rect.left() + 26.0, left_rect.center().y),
            Vec2::splat(52.0),
        );
        if let Some(tex) = current_cover {
            p.image(
                tex.id(),
                art_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            p.rect_stroke(
                art_rect,
                CornerRadius::same(6),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
        } else {
            p.rect_filled(
                art_rect,
                CornerRadius::same(6),
                Color32::from_rgb(18, 20, 26),
            );
            p.rect_stroke(
                art_rect,
                CornerRadius::same(6),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
            draw_icon(p, art_rect.center(), 18.0, Icon::MusicNote, TEXT_MUTED);
        }

        let text_left = art_rect.right() + 12.0;
        let text_right = left_rect.right();

        if snap.mix.active {
            let mix_title = format!(
                "{}  ⇄  {}",
                snap.mix.outgoing_title, snap.mix.incoming_title
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y - 18.0),
                    Pos2::new(text_right, left_rect.center().y + 1.0),
                ),
                &mix_title,
                font_semibold(14.0),
                TEXT_PRIMARY,
                Align2::LEFT_CENTER,
            );
            let key_part = match (&snap.mix.outgoing_key, &snap.mix.incoming_key) {
                (Some(ka), Some(kb)) if !ka.is_empty() && !kb.is_empty() => {
                    format!(" · {ka} → {kb}")
                }
                _ => String::new(),
            };
            let bpm_part = match (snap.mix.outgoing_bpm, snap.mix.incoming_bpm) {
                (Some(a), Some(b)) => format!(" · {:.0} → {:.0} BPM", a, b),
                _ => String::new(),
            };
            let mix_sub = format!(
                "{} {:.0}%{}{}",
                s.dj_mixing,
                snap.mix.progress * 100.0,
                key_part,
                bpm_part
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y + 2.0),
                    Pos2::new(text_right, left_rect.center().y + 19.0),
                ),
                &mix_sub,
                font_regular(12.0),
                TEXT_SECONDARY,
                Align2::LEFT_CENTER,
            );
        } else if let Some(track) = current_track {
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y - 18.0),
                    Pos2::new(text_right, left_rect.center().y + 1.0),
                ),
                &track.title,
                font_semibold(14.0),
                TEXT_PRIMARY,
                Align2::LEFT_CENTER,
            );

            let artist = track.display_artist(s.unknown_artist);
            let sub = if config.playback_mode == PlaybackMode::DjMix {
                match (track.bpm(), next_track.and_then(|n| n.bpm())) {
                    (Some(cb), Some(nb)) => format!("{artist}  ·  {cb:.0} → {nb:.0} BPM"),
                    (Some(cb), None) => format!("{artist}  ·  {cb:.0} BPM"),
                    _ => artist.to_string(),
                }
            } else {
                artist.to_string()
            };

            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y + 2.0),
                    Pos2::new(text_right, left_rect.center().y + 19.0),
                ),
                &sub,
                font_regular(12.0),
                TEXT_SECONDARY,
                Align2::LEFT_CENTER,
            );
        } else {
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y - 17.0),
                    Pos2::new(text_right, left_rect.center().y + 1.0),
                ),
                s.nothing_playing,
                font_semibold(13.5),
                TEXT_SECONDARY,
                Align2::LEFT_CENTER,
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(text_left, left_rect.center().y + 2.0),
                    Pos2::new(text_right, left_rect.center().y + 18.0),
                ),
                s.select_track_hint,
                font_regular(11.5),
                TEXT_MUTED,
                Align2::LEFT_CENTER,
            );
        }
    }

    // --- CENTER: 5 Spotify Transport Buttons + Slim Progress Bar ---
    let center_rect = Rect::from_min_size(
        Pos2::new(left_rect.right() + 14.0, body_rect.top()),
        Vec2::new(center_w, body_rect.height()),
    );
    {
        let btn_row_h = 36.0;
        let btn_strip_w = 198.0;
        let btn_rect = Rect::from_center_size(
            Pos2::new(center_rect.center().x, center_rect.top() + btn_row_h * 0.5 - 1.0),
            Vec2::new(btn_strip_w, btn_row_h),
        );
        let mut btn_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(btn_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        btn_ui.spacing_mut().item_spacing.x = 12.0;

        // 1. Mode / Shuffle button (28.0 diameter) + 2px active dot underneath
        let (mode_icon, mode_active, mode_tip) = match config.playback_mode {
            PlaybackMode::Normal => (Icon::Order, false, s.mode_normal_tip),
            PlaybackMode::Shuffle => (Icon::Shuffle, true, s.mode_shuffle_tip),
            PlaybackMode::DjMix => (Icon::DjMix, true, s.mode_dj_tip),
        };
        let mode_resp =
            glass_icon_button(&mut btn_ui, mode_icon, 28.0, 13.5, mode_active, false)
                .on_hover_text(mode_tip);
        if mode_resp.clicked() {
            action = Some(PlayerAction::CycleMode);
        }
        if mode_active {
            btn_ui.painter().circle_filled(
                Pos2::new(mode_resp.rect.center().x, mode_resp.rect.bottom() + 2.0),
                2.0,
                Color32::WHITE,
            );
        }

        // 2. Prev (30.0)
        if glass_icon_button(&mut btn_ui, Icon::Prev, 30.0, 13.5, false, false).clicked() {
            action = Some(PlayerAction::Prev);
        }

        // 3. Play / Pause (36.0, primary white circle with dark icon)
        let play_icon = if snap.is_playing {
            Icon::Pause
        } else {
            Icon::Play
        };
        if glass_icon_button(&mut btn_ui, play_icon, 36.0, 15.0, true, true).clicked() {
            action = Some(PlayerAction::TogglePlay);
        }

        // 4. Next (30.0)
        if glass_icon_button(&mut btn_ui, Icon::Next, 30.0, 13.5, false, false).clicked() {
            action = Some(PlayerAction::Next);
        }

        // 5. Repeat (28.0) + 2px active dot underneath when RepeatAll or RepeatOne
        let (rep_icon, rep_active, rep_tip) = match config.repeat_mode {
            RepeatMode::Off => (Icon::Repeat, false, s.repeat_off),
            RepeatMode::All => (Icon::Repeat, true, s.repeat_all),
            RepeatMode::One => (Icon::RepeatOne, true, s.repeat_one),
        };
        let rep_resp = glass_icon_button(&mut btn_ui, rep_icon, 28.0, 13.5, rep_active, false)
            .on_hover_text(rep_tip);
        if rep_resp.clicked() {
            action = Some(PlayerAction::CycleRepeat);
        }
        if rep_active {
            btn_ui.painter().circle_filled(
                Pos2::new(rep_resp.rect.center().x, rep_resp.rect.bottom() + 2.0),
                2.0,
                Color32::WHITE,
            );
        }

        // Bottom row: Seek slider
        let dur = if snap.duration_sec > 0.1 {
            snap.duration_sec
        } else {
            current_track
                .map(|t| t.effective_duration())
                .unwrap_or(0.0)
        };
        let pos = snap.position_sec.clamp(0.0, dur.max(0.01));
        let frac = if dur > 0.1 { pos / dur } else { 0.0 };
        let mix_frac = snap.planned_mix_out_sec.and_then(|m| {
            if dur > 1.0 && config.playback_mode == PlaybackMode::DjMix {
                Some((m / dur).clamp(0.1, 0.99))
            } else {
                None
            }
        });

        let seek_y = center_rect.bottom() - 9.0;
        let time_w = 36.0;
        let slider_rect = Rect::from_min_max(
            Pos2::new(center_rect.left() + time_w + 4.0, seek_y - 9.0),
            Pos2::new(center_rect.right() - time_w - 4.0, seek_y + 9.0),
        );
        let mut s_ui = ui.new_child(egui::UiBuilder::new().max_rect(slider_rect));
        let fmt = |f: f32| format_mm_ss(f * dur);
        let (seeked, shown_frac) = glass_seek_slider(
            &mut s_ui,
            "main_seek_slider",
            slider_rect.width(),
            18.0,
            frac,
            mix_frac,
            if dur > 0.5 { Some(&fmt) } else { None },
        );
        let shown_pos = (shown_frac * dur).clamp(0.0, dur.max(0.0));
        let p = ui.painter();
        p.text(
            Pos2::new(center_rect.left() + time_w - 4.0, seek_y),
            Align2::RIGHT_CENTER,
            format_mm_ss(shown_pos),
            font_regular(11.0),
            TEXT_SECONDARY,
        );
        p.text(
            Pos2::new(center_rect.right() - time_w + 4.0, seek_y),
            Align2::LEFT_CENTER,
            format_mm_ss(dur),
            font_regular(11.0),
            TEXT_MUTED,
        );
        if let Some(new_frac) = seeked {
            if dur > 0.5 {
                action = Some(PlayerAction::Seek(new_frac * dur));
            }
        }
    }

    // --- RIGHT: Queue + Mute + Volume Slider + Mini-Player (right-to-left) ---
    let right_rect = Rect::from_min_max(
        Pos2::new(body_rect.right() - right_w, body_rect.top()),
        body_rect.max,
    );
    {
        let mut r_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(right_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        r_ui.spacing_mut().item_spacing.x = 8.0;

        // 1. Far right: Mini-Player button (Icon::MiniPlayer, 28.0)
        if glass_icon_button(&mut r_ui, Icon::MiniPlayer, 28.0, 13.0, false, false)
            .on_hover_text("Mini-Player")
            .clicked()
        {
            action = Some(PlayerAction::EnterMiniMode);
        }

        // 2. Volume Slider (88.0 width)
        let effective_vol = if config.muted { 0.0 } else { config.volume };
        let (vol_new, _) = glass_slider(
            &mut r_ui,
            "volume_slider",
            88.0,
            20.0,
            effective_vol,
            None,
            None,
        );
        if let Some(v) = vol_new {
            action = Some(PlayerAction::SetVolume(v));
        }

        // 3. Volume Mute/Speaker icon (28.0)
        let vol_icon = if config.muted || config.volume <= 0.005 {
            Icon::VolumeMute
        } else if config.volume < 0.5 {
            Icon::VolumeLow
        } else {
            Icon::VolumeHigh
        };
        if glass_icon_button(&mut r_ui, vol_icon, 28.0, 14.0, config.muted, false).clicked() {
            action = Some(PlayerAction::ToggleMute);
        }

        // 4. Queue / Warteschlange button (Icon::Queue, 28.0, active = queue_open)
        let q_resp = glass_icon_button(&mut r_ui, Icon::Queue, 28.0, 13.5, queue_open, false)
            .on_hover_text(s.queue_title);
        if q_resp.clicked() {
            action = Some(PlayerAction::ToggleQueuePopup);
        }
        if user_queue_len > 0 {
            r_ui.painter().circle_filled(
                Pos2::new(q_resp.rect.right() - 4.0, q_resp.rect.top() + 4.0),
                2.5,
                Color32::WHITE,
            );
        }

        // 5. Synchronized Lyrics button (Icon::Lyrics, 28.0, active = lyrics_open)
        let lyrics_tip = if config.language == crate::i18n::Language::De {
            "Songtext (Lyrics)"
        } else {
            "Lyrics"
        };
        let lyr_resp = glass_icon_button(&mut r_ui, Icon::Lyrics, 28.0, 13.5, lyrics_open, false)
            .on_hover_text(lyrics_tip);
        if lyr_resp.clicked() {
            action = Some(PlayerAction::ToggleLyrics);
        }
        if lyrics_open {
            r_ui.painter().circle_filled(
                Pos2::new(lyr_resp.rect.center().x, lyr_resp.rect.bottom() + 2.0),
                2.0,
                Color32::WHITE,
            );
        }
    }

    action
}
