use crate::config::AppConfig;
use crate::i18n::{tr, Language};
use crate::library::scan_library;
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, frosted_cta_button,
    glass_pill_button, glass_toggle, GlassLevel, GLASS_PILL_IDLE, RIM_ACTIVE, RIM_BRIGHT,
    RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, TextureHandle, Ui, Vec2};
use std::path::PathBuf;

pub struct SetupState {
    pub detected_playlists: Vec<(String, usize)>,
    pub scanned_dir: Option<PathBuf>,
}

impl SetupState {
    pub fn new(initial_dir: Option<&PathBuf>) -> Self {
        let mut s = Self {
            detected_playlists: Vec::new(),
            scanned_dir: None,
        };
        s.refresh_preview(initial_dir);
        s
    }

    pub fn refresh_preview(&mut self, dir: Option<&PathBuf>) {
        if self.scanned_dir.as_ref() == dir {
            return;
        }
        self.scanned_dir = dir.cloned();
        self.detected_playlists.clear();
        if let Some(d) = dir {
            let pls = scan_library(d);
            for pl in pls {
                self.detected_playlists.push((pl.name, pl.tracks.len()));
            }
        }
    }
}

/// Renders the First-Run Liquid Glass Setup Wizard. Returns `true` when the user completes setup.
pub fn show_setup_wizard(
    ui: &mut Ui,
    config: &mut AppConfig,
    setup: &mut SetupState,
    logo_tex: Option<&TextureHandle>,
) -> bool {
    setup.refresh_preview(config.music_dir.as_ref());
    let s = tr(config.language);
    let mut completed = false;

    let avail = ui.available_rect_before_wrap();
    let card_w = 540.0f32.min(avail.width() - 40.0);
    let card_h = 540.0f32.min(avail.height() - 36.0);
    let card_rect = Rect::from_center_size(avail.center(), Vec2::new(card_w, card_h));

    draw_liquid_glass_panel(ui.painter(), card_rect, 24, GlassLevel::Modal);

    let inner = card_rect.shrink2(Vec2::new(34.0, 28.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    // Header: App Logo + Welcome Title
    child.horizontal(|ui| {
        if let Some(tex) = logo_tex {
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
            ui.painter().image(
                tex.id(),
                logo_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            ui.add_space(12.0);
        }
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(s.setup_welcome)
                    .font(font_semibold(23.0))
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(s.setup_subtitle)
                    .font(font_regular(13.0))
                    .color(TEXT_SECONDARY),
            );
        });
    });

    child.add_space(22.0);

    // Section 1: Language selection (DE / EN)
    child.label(
        egui::RichText::new(s.setup_lang_title.to_uppercase())
            .font(font_semibold(11.0))
            .color(TEXT_MUTED),
    );
    child.add_space(8.0);
    child.horizontal(|ui| {
        let pill_w = (inner.width() - 10.0) * 0.5;
        for lang in [Language::De, Language::En] {
            let selected = config.language == lang;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(pill_w, 42.0), egui::Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
            let p = ui.painter();

            let fill = if selected {
                Color32::from_rgb(236, 238, 244)
            } else {
                Color32::from_rgba_unmultiplied(
                    255,
                    255,
                    255,
                    (14.0 + hover * 14.0) as u8,
                )
            };
            let rim = if selected { RIM_ACTIVE } else { RIM_SUBTLE };
            p.rect_filled(rect, CornerRadius::same(12), fill);
            p.rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0_f32, rim), StrokeKind::Inside);

            let text_col = if selected {
                Color32::from_rgb(12, 13, 17)
            } else {
                TEXT_PRIMARY
            };
            let badge = match lang {
                Language::De => "DE  ·  Deutsch",
                Language::En => "EN  ·  English",
            };
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                badge,
                font_semibold(13.5),
                text_col,
            );
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                config.language = lang;
            }
            ui.add_space(10.0);
        }
    });

    child.add_space(22.0);

    // Section 2: Music Folder Picker
    child.label(
        egui::RichText::new(s.setup_folder_title.to_uppercase())
            .font(font_semibold(11.0))
            .color(TEXT_MUTED),
    );
    child.add_space(4.0);
    child.label(
        egui::RichText::new(s.setup_folder_desc)
            .font(font_regular(12.5))
            .color(TEXT_SECONDARY),
    );
    child.add_space(10.0);

    // Folder display glass box + Browse button
    let folder_box_h = 46.0;
    let (box_rect, _) = child.allocate_exact_size(Vec2::new(inner.width(), folder_box_h), egui::Sense::hover());
    {
        let p = child.painter();
        p.rect_filled(box_rect, CornerRadius::same(12), GLASS_PILL_IDLE);
        p.rect_stroke(box_rect, CornerRadius::same(12), Stroke::new(1.0_f32, RIM_BRIGHT), StrokeKind::Inside);

        draw_icon(
            p,
            Pos2::new(box_rect.left() + 20.0, box_rect.center().y),
            16.0,
            Icon::Folder,
            TEXT_SECONDARY,
        );

        let path_str = config
            .music_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| s.setup_no_folder.to_string());

        let text_rect = Rect::from_min_max(
            Pos2::new(box_rect.left() + 38.0, box_rect.top()),
            Pos2::new(box_rect.right() - 160.0, box_rect.bottom()),
        );
        draw_truncated_text(
            p,
            text_rect,
            &path_str,
            font_regular(13.0),
            if config.music_dir.is_some() {
                TEXT_PRIMARY
            } else {
                TEXT_MUTED
            },
            Align2::LEFT_CENTER,
        );
    }

    // Place Browse button right inside the folder box
    let btn_rect = Rect::from_min_max(
        Pos2::new(box_rect.right() - 154.0, box_rect.top() + 6.0),
        Pos2::new(box_rect.right() - 6.0, box_rect.bottom() - 6.0),
    );
    let mut btn_ui = child.new_child(egui::UiBuilder::new().max_rect(btn_rect));
    if glass_pill_button(&mut btn_ui, None, s.setup_choose_folder, 34.0, false).clicked() {
        let mut dialog = rfd::FileDialog::new();
        if let Some(ref cur) = config.music_dir {
            dialog = dialog.set_directory(cur);
        }
        if let Some(picked) = dialog.pick_folder() {
            config.music_dir = Some(picked);
            setup.scanned_dir = None; // force preview refresh
            setup.refresh_preview(config.music_dir.as_ref());
        }
    }

    child.add_space(10.0);

    // Live Preview of Detected Playlists
    let preview_rect = Rect::from_min_size(
        child.cursor().min,
        Vec2::new(inner.width(), 52.0),
    );
    child.allocate_exact_size(preview_rect.size(), egui::Sense::hover());
    {
        let p = child.painter();
        p.rect_filled(
            preview_rect,
            CornerRadius::same(11),
            Color32::from_rgba_premultiplied(24, 26, 32, 130),
        );
        p.rect_stroke(
            preview_rect,
            CornerRadius::same(11),
            Stroke::new(1.0_f32, RIM_SUBTLE),
            StrokeKind::Inside,
        );

        if !setup.detected_playlists.is_empty() {
            let total_tracks: usize = setup.detected_playlists.iter().map(|(_, n)| *n).sum();
            let names: Vec<String> = setup
                .detected_playlists
                .iter()
                .take(4)
                .map(|(name, n)| format!("{name} ({n})"))
                .collect();
            let summary = format!(
                "{} {}  ·  {} {}",
                setup.detected_playlists.len(),
                s.setup_found_playlists,
                total_tracks,
                s.tracks_count
            );
            draw_icon(
                p,
                Pos2::new(preview_rect.left() + 18.0, preview_rect.top() + 18.0),
                13.0,
                Icon::Check,
                TEXT_PRIMARY,
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(preview_rect.left() + 34.0, preview_rect.top() + 6.0),
                    Pos2::new(preview_rect.right() - 12.0, preview_rect.top() + 28.0),
                ),
                &summary,
                font_semibold(12.5),
                TEXT_PRIMARY,
                Align2::LEFT_CENTER,
            );
            draw_truncated_text(
                p,
                Rect::from_min_max(
                    Pos2::new(preview_rect.left() + 34.0, preview_rect.top() + 26.0),
                    Pos2::new(preview_rect.right() - 12.0, preview_rect.bottom() - 4.0),
                ),
                &names.join("  ·  "),
                font_regular(11.5),
                TEXT_SECONDARY,
                Align2::LEFT_CENTER,
            );
        } else {
            draw_truncated_text(
                p,
                preview_rect.shrink2(Vec2::new(14.0, 0.0)),
                s.setup_no_playlists_warn,
                font_regular(12.0),
                TEXT_MUTED,
                Align2::LEFT_CENTER,
            );
        }
    }

    child.add_space(20.0);

    // Section 3: Quick DJ Engine Option
    child.label(
        egui::RichText::new(s.setup_dj_title.to_uppercase())
            .font(font_semibold(11.0))
            .color(TEXT_MUTED),
    );
    child.add_space(8.0);
    child.horizontal(|ui| {
        ui.label(
            egui::RichText::new(s.setup_auto_bpm)
                .font(font_regular(13.0))
                .color(TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            glass_toggle(ui, &mut config.auto_analyze_bpm);
        });
    });

    // Bottom CTA button pinned cleanly at card bottom
    let footer_rect = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - 46.0),
        inner.max,
    );
    let mut footer_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(footer_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );

    let can_start = config.music_dir.as_ref().map(|d| d.exists()).unwrap_or(false);
    footer_ui.add_enabled_ui(can_start, |ui| {
        if frosted_cta_button(ui, Some(Icon::Play), s.setup_start, 42.0).clicked() {
            config.setup_completed = true;
            config.save();
            completed = true;
        }
    });

    completed
}
