use crate::i18n::{tr, Language};
use crate::library::Playlist;
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, lerp_color, rgba,
    GlassLevel, GLASS_PILL_ACTIVE, GLASS_PILL_IDLE, RIM_BRIGHT, TEXT_MUTED, TEXT_PRIMARY,
    TEXT_SECONDARY,
};
use crate::ui::visualizer::{draw_sidebar_visualizer_settings, AudioVisualizerState};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewSelection {
    Playlist(usize),
}

#[allow(dead_code)]
pub enum SidebarAction {
    Select(ViewSelection),
    CreatePlaylist(String),
    OpenImportModal,
    ToggleVisualizer,
    VisualizerConfigChanged,
    OpenSettings,
}

#[allow(clippy::too_many_arguments)]
pub fn show_sidebar(
    ui: &mut Ui,
    rect: Rect,
    lang: Language,
    playlists: &[Playlist],
    selection: &ViewSelection,
    bpm_progress: Option<(usize, usize)>,
    creating_playlist: &mut bool,
    new_playlist_name: &mut String,
    visualizer: &mut AudioVisualizerState,
) -> Option<SidebarAction> {
    let s = tr(lang);
    let mut action = None;

    draw_liquid_glass_panel(ui.painter(), rect, 16, GlassLevel::Sidebar);

    let inner = rect.shrink2(Vec2::new(14.0, 16.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    // PLAYLISTS Header + prominent "+ Importieren" / "+ Import" pill button
    let (hdr_rect, _) = child.allocate_exact_size(Vec2::new(inner.width(), 28.0), Sense::hover());
    {
        let p = child.painter();
        p.text(
            Pos2::new(hdr_rect.left() + 4.0, hdr_rect.center().y),
            Align2::LEFT_CENTER,
            s.playlists,
            font_semibold(10.5),
            TEXT_MUTED,
        );

        let btn_label = if lang == Language::De {
            "Importieren"
        } else {
            "Import"
        };
        let galley = p.layout_no_wrap(btn_label.to_string(), font_semibold(11.5), TEXT_PRIMARY);
        let btn_w = (galley.size().x + 30.0).min((hdr_rect.width() - 68.0).max(76.0));
        let plus_rect = Rect::from_min_size(
            Pos2::new(hdr_rect.right() - btn_w, hdr_rect.center().y - 13.0),
            Vec2::new(btn_w, 26.0),
        );
        let plus_resp = child
            .interact(plus_rect, child.id().with("sidebar_plus_btn"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if lang == Language::De {
                "Playlist erstellen oder importieren"
            } else {
                "Create or import playlist"
            });
        let hover = child
            .ctx()
            .animate_bool_with_time(plus_resp.id, plus_resp.hovered(), 0.13);

        let bg = lerp_color(GLASS_PILL_IDLE, GLASS_PILL_ACTIVE, hover);
        let rim = lerp_color(
            rgba(255, 255, 255, 22),
            RIM_BRIGHT,
            hover,
        );
        p.rect_filled(plus_rect, CornerRadius::same(7), bg);
        p.rect_stroke(
            plus_rect,
            CornerRadius::same(7),
            Stroke::new(1.0_f32, rim),
            StrokeKind::Inside,
        );

        let fg = lerp_color(Color32::from_rgb(212, 216, 224), TEXT_PRIMARY, hover);
        draw_icon(
            p,
            Pos2::new(plus_rect.left() + 12.0, plus_rect.center().y),
            13.5,
            Icon::Plus,
            fg,
        );
        p.galley(
            Pos2::new(
                plus_rect.left() + 23.0,
                plus_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            fg,
        );

        if plus_resp.clicked() {
            action = Some(SidebarAction::OpenImportModal);
        }
    }
    child.add_space(10.0);

    // Inline Create New Playlist input
    if *creating_playlist {
        let box_rect = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), 66.0));
        child.painter().rect_filled(box_rect, CornerRadius::same(10), GLASS_PILL_IDLE);
        child.painter().rect_stroke(box_rect, CornerRadius::same(10), Stroke::new(1.0_f32, RIM_BRIGHT), StrokeKind::Inside);

        let mut form_ui = child.new_child(egui::UiBuilder::new().max_rect(box_rect.shrink(8.0)));
        let te = form_ui.add(
            egui::TextEdit::singleline(new_playlist_name)
                .hint_text(s.new_playlist_placeholder)
                .desired_width(f32::INFINITY)
                .frame(false)
                .font(font_regular(12.5)),
        );
        let enter_pressed = te.lost_focus() && form_ui.input(|i| i.key_pressed(egui::Key::Enter));
        form_ui.add_space(6.0);
        form_ui.horizontal(|ui| {
            let can_create = !new_playlist_name.trim().is_empty();
            if ui
                .add_enabled(
                    can_create,
                    egui::Button::new(egui::RichText::new(s.create).font(font_semibold(11.5)))
                        .corner_radius(CornerRadius::same(6)),
                )
                .clicked()
                || (enter_pressed && can_create)
            {
                action = Some(SidebarAction::CreatePlaylist(new_playlist_name.trim().to_string()));
                new_playlist_name.clear();
                *creating_playlist = false;
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(s.cancel).font(font_regular(11.5)))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE),
                )
                .clicked()
            {
                *creating_playlist = false;
                new_playlist_name.clear();
            }
        });
        child.advance_cursor_after_rect(box_rect);
        child.add_space(8.0);
    }

    let footer_h = if bpm_progress.is_some() { 102.0 } else { 78.0 };
    let list_h = (inner.bottom() - child.cursor().min.y - footer_h).max(60.0);

    // Clean Playlist List (+ Visualizer Settings below playlists ONLY when Visualizer is launched!)
    egui::ScrollArea::vertical()
        .id_salt("sidebar_playlists")
        .max_height(list_h)
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            for (idx, pl) in playlists.iter().enumerate() {
                let selected = *selection == ViewSelection::Playlist(idx);
                if draw_playlist_item(ui, &pl.name, pl.tracks.len(), selected) {
                    action = Some(SidebarAction::Select(ViewSelection::Playlist(idx)));
                }
                ui.add_space(4.0);
            }

            if visualizer.open {
                ui.add_space(8.0);
                let sep_y = ui.cursor().min.y;
                ui.painter().line_segment(
                    [
                        Pos2::new(ui.max_rect().left() + 2.0, sep_y),
                        Pos2::new(ui.max_rect().right() - 2.0, sep_y),
                    ],
                    Stroke::new(1.0_f32, rgba(255, 255, 255, 14)),
                );
                ui.add_space(10.0);
                if draw_sidebar_visualizer_settings(ui, lang, visualizer) {
                    action = Some(SidebarAction::VisualizerConfigChanged);
                }
                ui.add_space(6.0);
            }
        });

    // Bottom pinned footer: subtle 1-line BPM indicator (only while analyzing) + Visualizer button + Settings button
    let footer_rect = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - footer_h + 4.0),
        inner.max,
    );
    let mut footer = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(footer_rect)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    if let Some((done, total)) = bpm_progress {
        footer.label(
            egui::RichText::new(format!("{} · {done}/{total}", s.analyzing_bpm))
                .font(font_regular(11.0))
                .color(TEXT_MUTED),
        );
        footer.add_space(6.0);
    }

    let row_y = footer.cursor().min.y;
    footer.painter().line_segment(
        [
            Pos2::new(inner.left() + 2.0, row_y),
            Pos2::new(inner.right() - 2.0, row_y),
        ],
        Stroke::new(1.0_f32, rgba(255, 255, 255, 14)),
    );
    footer.add_space(6.0);

    // 1. Visualizer Button (directly above Settings, styled identically!)
    let viz_label = "Visualizer";
    if draw_footer_nav_button(&mut footer, Icon::Visualizer, viz_label, visualizer.open) {
        action = Some(SidebarAction::ToggleVisualizer);
    }
    footer.add_space(4.0);

    // 2. Settings Button
    if draw_footer_nav_button(&mut footer, Icon::Settings, s.settings, false) {
        action = Some(SidebarAction::OpenSettings);
    }

    action
}

fn draw_footer_nav_button(ui: &mut Ui, icon: Icon, label: &str, active: bool) -> bool {
    let btn_w = ui.available_width().max(80.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(btn_w, 30.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
    let p = ui.painter();

    if active {
        p.rect_filled(rect, CornerRadius::same(8), GLASS_PILL_ACTIVE);
        p.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, RIM_BRIGHT),
            StrokeKind::Inside,
        );
    } else if hover > 0.01 {
        p.rect_filled(
            rect,
            CornerRadius::same(8),
            rgba(255, 255, 255, (hover * 14.0) as u8),
        );
    }

    let col = if active {
        TEXT_PRIMARY
    } else {
        lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hover)
    };
    draw_icon(
        p,
        Pos2::new(rect.left() + 12.0, rect.center().y),
        13.5,
        icon,
        col,
    );
    p.text(
        Pos2::new(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if active {
            font_semibold(12.5)
        } else {
            font_regular(12.5)
        },
        col,
    );
    if active {
        p.circle_filled(
            Pos2::new(rect.right() - 12.0, rect.center().y),
            3.0,
            Color32::WHITE,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn draw_playlist_item(ui: &mut Ui, label: &str, count: usize, selected: bool) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 36.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);

        if selected {
            p.rect_filled(rect, CornerRadius::same(9), GLASS_PILL_ACTIVE);
            p.rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0_f32, RIM_BRIGHT), StrokeKind::Inside);
            let indicator = Rect::from_min_max(
                Pos2::new(rect.left() + 4.0, rect.top() + 9.0),
                Pos2::new(rect.left() + 6.5, rect.bottom() - 9.0),
            );
            p.rect_filled(indicator, CornerRadius::same(2), Color32::WHITE);
        } else if hover > 0.01 {
            p.rect_filled(
                rect,
                CornerRadius::same(9),
                rgba(255, 255, 255, (hover * 14.0) as u8),
            );
        }

        let fg = if selected {
            TEXT_PRIMARY
        } else {
            lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hover)
        };

        let text_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 14.0, rect.top()),
            Pos2::new(rect.right() - 32.0, rect.bottom()),
        );
        draw_truncated_text(
            p,
            text_rect,
            label,
            if selected {
                font_semibold(13.0)
            } else {
                font_regular(13.0)
            },
            fg,
            Align2::LEFT_CENTER,
        );

        p.text(
            Pos2::new(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            font_regular(11.5),
            if selected { TEXT_SECONDARY } else { TEXT_MUTED },
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
