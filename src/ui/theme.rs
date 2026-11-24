use crate::ui::icons::{draw_icon, Icon};
use egui::{
    epaint::{Mesh, Vertex},
    Align2, Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use std::f32::consts::PI;

/// Compile-time premultiplied RGBA helper (ensures translucent rims & glass fills never turn additive!).
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let pr = ((r as u16 * a as u16 + 127) / 255) as u8;
    let pg = ((g as u16 * a as u16 + 127) / 255) as u8;
    let pb = ((b as u16 * a as u16 + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(pr, pg, pb, a)
}

// === Monochrome Liquid Glass Palette (Ilay-dev Modern Obsidian & Silver) ===
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(245, 246, 248);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(142, 147, 158);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(92, 96, 106);

pub const GLASS_SIDEBAR: Color32 = rgba(10, 11, 14, 210);
pub const GLASS_SURFACE: Color32 = rgba(11, 12, 15, 205);
pub const GLASS_ELEVATED: Color32 = rgba(13, 14, 18, 235);
pub const GLASS_MODAL: Color32 = rgba(12, 13, 17, 246);

pub const GLASS_PILL_IDLE: Color32 = rgba(255, 255, 255, 10);
pub const GLASS_PILL_HOVER: Color32 = rgba(255, 255, 255, 20);
pub const GLASS_PILL_ACTIVE: Color32 = rgba(255, 255, 255, 28);

pub const FROST_CTA_IDLE: Color32 = Color32::from_rgb(242, 243, 247);
pub const FROST_CTA_HOVER: Color32 = Color32::from_rgb(255, 255, 255);
pub const FROST_CTA_TEXT: Color32 = Color32::from_rgb(9, 10, 13);

pub const RIM_SUBTLE: Color32 = rgba(255, 255, 255, 18);
pub const RIM_BRIGHT: Color32 = rgba(255, 255, 255, 32);
pub const RIM_ACTIVE: Color32 = rgba(255, 255, 255, 64);

pub const DANGER_GLASS: Color32 = rgba(196, 62, 62, 215);

pub fn install_fonts_and_style(ctx: &Context) {
    let mut fonts = FontDefinitions::default();

    // Load native Windows Segoe UI + Semibold for crisp German/Unicode typography
    #[cfg(target_os = "windows")]
    {
        let regular_candidates = [
            r"C:\Windows\Fonts\segoeui.ttf",
            r"C:\Windows\Fonts\SegUIVar.ttf",
        ];
        for p in regular_candidates {
            if let Ok(bytes) = std::fs::read(p) {
                fonts
                    .font_data
                    .insert("segoe_ui".to_owned(), FontData::from_owned(bytes).into());
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "segoe_ui".to_owned());
                }
                break;
            }
        }

        let bold_candidates = [
            r"C:\Windows\Fonts\seguisb.ttf",
            r"C:\Windows\Fonts\segoeuib.ttf",
        ];
        for p in bold_candidates {
            if let Ok(bytes) = std::fs::read(p) {
                fonts
                    .font_data
                    .insert("segoe_sb".to_owned(), FontData::from_owned(bytes).into());
                fonts.families.insert(
                    FontFamily::Name("SemiBold".into()),
                    vec!["segoe_sb".to_owned(), "segoe_ui".to_owned()],
                );
                break;
            }
        }
    }

    // Load native Linux system fonts (Arch Linux & Linux Mint: Inter, Ubuntu, Noto Sans, DejaVu Sans, Liberation Sans)
    #[cfg(target_os = "linux")]
    {
        let regular_candidates = [
            "/usr/share/fonts/inter/Inter-Regular.otf",
            "/usr/share/fonts/TTF/Inter-Regular.ttf",
            "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
            "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            "/usr/share/fonts/TTF/Ubuntu-R.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        ];
        let mut has_regular = false;
        for p in regular_candidates {
            if let Ok(bytes) = std::fs::read(p) {
                fonts
                    .font_data
                    .insert("linux_ui".to_owned(), FontData::from_owned(bytes).into());
                if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "linux_ui".to_owned());
                }
                has_regular = true;
                break;
            }
        }

        let bold_candidates = [
            "/usr/share/fonts/inter/Inter-SemiBold.otf",
            "/usr/share/fonts/TTF/Inter-SemiBold.ttf",
            "/usr/share/fonts/truetype/inter/Inter-SemiBold.ttf",
            "/usr/share/fonts/opentype/inter/Inter-SemiBold.otf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf",
            "/usr/share/fonts/TTF/Ubuntu-M.ttf",
            "/usr/share/fonts/TTF/Ubuntu-B.ttf",
            "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Bold.ttf",
        ];
        for p in bold_candidates {
            if let Ok(bytes) = std::fs::read(p) {
                fonts
                    .font_data
                    .insert("linux_sb".to_owned(), FontData::from_owned(bytes).into());
                let mut family = vec!["linux_sb".to_owned()];
                if has_regular {
                    family.push("linux_ui".to_owned());
                }
                if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
                    for f in prop {
                        if !family.contains(f) {
                            family.push(f.clone());
                        }
                    }
                }
                fonts
                    .families
                    .insert(FontFamily::Name("SemiBold".into()), family);
                break;
            }
        }
    }

    if !fonts.families.contains_key(&FontFamily::Name("SemiBold".into())) {
        let fallback = fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(FontFamily::Name("SemiBold".into()), fallback);
    }

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = Color32::from_rgb(14, 15, 19);
    style.visuals.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(38, 41, 50));
    style.visuals.window_corner_radius = CornerRadius::same(10);
    style.visuals.menu_corner_radius = CornerRadius::same(10);
    style.visuals.panel_fill = Color32::TRANSPARENT;
    style.visuals.extreme_bg_color = Color32::from_rgb(10, 11, 14);
    style.visuals.faint_bg_color = Color32::from_rgb(20, 22, 28);
    style.visuals.override_text_color = Some(TEXT_PRIMARY);
    style.visuals.selection.bg_fill = rgba(220, 224, 234, 42);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, RIM_BRIGHT);

    style.visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, RIM_SUBTLE);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);

    style.visuals.widgets.inactive.bg_fill = GLASS_PILL_IDLE;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, RIM_SUBTLE);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(10);

    style.visuals.widgets.hovered.bg_fill = GLASS_PILL_HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, RIM_BRIGHT);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(10);

    style.visuals.widgets.active.bg_fill = GLASS_PILL_ACTIVE;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, RIM_ACTIVE);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(10);

    // Slim minimalist monochrome scrollbars
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        bar_width: 6.0,
        handle_min_length: 28.0,
        bar_inner_margin: 4.0,
        bar_outer_margin: 4.0,
        floating_width: 3.0,
        floating_allocated_width: 0.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.12,
        interact_background_opacity: 0.22,
        dormant_handle_opacity: 0.18,
        active_handle_opacity: 0.48,
        interact_handle_opacity: 0.78,
    };

    style.interaction.tooltip_delay = 0.35;
    ctx.set_style(style);
}

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("SemiBold".into()))
}

pub fn font_regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

#[derive(Debug, Clone, Copy)]
pub enum GlassLevel {
    Sidebar,
    Surface,
    Elevated,
    Modal,
}

/// Renders an authentic modern dark Monochrome Liquid Glass panel (Ilay-dev obsidian aesthetic):
/// soft ambient drop shadow + deep matte obsidian translucent fill + 1px dark rim + delicate top specular edge.
pub fn draw_liquid_glass_panel(painter: &Painter, rect: Rect, radius: u8, level: GlassLevel) {
    let (fill, rim, shadow_layers) = match level {
        GlassLevel::Sidebar => (GLASS_SIDEBAR, RIM_SUBTLE, 2),
        GlassLevel::Surface => (GLASS_SURFACE, RIM_SUBTLE, 2),
        GlassLevel::Elevated => (GLASS_ELEVATED, RIM_BRIGHT, 3),
        GlassLevel::Modal => (GLASS_MODAL, RIM_BRIGHT, 4),
    };

    // 1. Soft multi-pass elevation drop shadow
    for i in (1..=shadow_layers).rev() {
        let spread = i as f32 * 4.0;
        let y_off = i as f32 * 2.5;
        let alpha = (26 / i as u8).max(4);
        let shadow_rect = rect.expand(spread).translate(Vec2::new(0.0, y_off));
        painter.rect_filled(
            shadow_rect,
            CornerRadius::same(radius.saturating_add(i as u8 * 2)),
            Color32::from_rgba_unmultiplied(0, 0, 0, alpha),
        );
    }

    // 2. Translucent obsidian glass body
    let cr = CornerRadius::same(radius);
    painter.rect_filled(rect, cr, fill);

    // 3. Perimeter 1px graphite glass border
    painter.rect_stroke(rect, cr, Stroke::new(1.0_f32, rim), StrokeKind::Inside);

    // 4. Delicate 1px top specular catchlight
    let inset = (radius as f32 * 0.95).max(10.0).min(rect.width() * 0.35);
    let spec_y = rect.top() + 0.8;
    painter.line_segment(
        [
            Pos2::new(rect.left() + inset, spec_y),
            Pos2::new(rect.right() - inset, spec_y),
        ],
        Stroke::new(1.0_f32, rgba(255, 255, 255, 28)),
    );
}

#[allow(dead_code)]
/// Draws a silky vertical white-to-transparent refractive sheen strictly respecting the top corner radius.
fn draw_rounded_top_sheen(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    sheen_h: f32,
    top_alpha: u8,
) {
    if rect.width() <= 2.0 || rect.height() <= 2.0 || top_alpha == 0 {
        return;
    }
    let r = radius.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let h = sheen_h.min(rect.height());
    let bottom_y = rect.top() + h;

    // Build horizontal scanline strips smoothly following the circular top corner arcs
    let arc_steps = 8usize;
    let mut ys: Vec<f32> = Vec::with_capacity(arc_steps + 2);
    for i in 0..=arc_steps {
        let t = i as f32 / arc_steps as f32;
        let y = rect.top() + r * (1.0 - (t * PI * 0.5).cos());
        if y <= bottom_y {
            ys.push(y);
        }
    }
    if ys.last().copied().unwrap_or(rect.top()) < bottom_y - 0.5 {
        ys.push(bottom_y);
    }

    let mut mesh = Mesh::default();
    for (idx, &y) in ys.iter().enumerate() {
        let dy = (y - rect.top()).max(0.0);
        let inset_x = if dy < r {
            let cy = r - dy;
            (r - (r * r - cy * cy).max(0.0).sqrt()).max(0.0)
        } else {
            0.0
        };
        let frac = (dy / h.max(1.0)).clamp(0.0, 1.0);
        // Quadratic ease-out falloff looks like genuine curved optical glass
        let a = ((1.0 - frac).powi(2) * top_alpha as f32).round() as u8;
        let col = Color32::from_rgba_unmultiplied(255, 255, 255, a);

        mesh.vertices.push(Vertex {
            pos: Pos2::new(rect.left() + inset_x, y),
            uv: egui::epaint::WHITE_UV,
            color: col,
        });
        mesh.vertices.push(Vertex {
            pos: Pos2::new(rect.right() - inset_x, y),
            uv: egui::epaint::WHITE_UV,
            color: col,
        });

        if idx > 0 {
            let prev = ((idx - 1) * 2) as u32;
            let curr = (idx * 2) as u32;
            mesh.indices.extend_from_slice(&[prev, prev + 1, curr + 1, prev, curr + 1, curr]);
        }
    }

    if !mesh.indices.is_empty() {
        painter.add(mesh);
    }
}

/// Primary frosted platinum CTA button (uniform 8px modern corner radius matching Ilay-dev).
pub fn frosted_cta_button(ui: &mut Ui, icon: Option<Icon>, label: &str, height: f32) -> Response {
    let font = font_semibold(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), FROST_CTA_TEXT);
    let icon_w = if icon.is_some() { 18.0 } else { 0.0 };
    let width = galley.size().x + icon_w + 26.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
        let fill = lerp_color(FROST_CTA_IDLE, FROST_CTA_HOVER, hover);
        let radius = 8u8;

        let draw_rect = if resp.is_pointer_button_down_on() {
            rect.shrink(0.6)
        } else {
            rect
        };

        p.rect_filled(draw_rect, CornerRadius::same(radius), fill);

        let total_content = galley.size().x + icon_w;
        let mut cursor_x = draw_rect.center().x - total_content * 0.5;
        if let Some(ic) = icon {
            draw_icon(
                p,
                Pos2::new(cursor_x + 6.0, draw_rect.center().y),
                13.0,
                ic,
                FROST_CTA_TEXT,
            );
            cursor_x += icon_w;
        }
        p.galley(
            Pos2::new(cursor_x, draw_rect.center().y - galley.size().y * 0.5),
            galley,
            FROST_CTA_TEXT,
        );
    }

    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Secondary monochrome dark-glass button (uniform 8px corner radius & 1px border matching Ilay-dev).
pub fn glass_pill_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    height: f32,
    active: bool,
) -> Response {
    let font = font_semibold(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, TEXT_PRIMARY);
    let icon_w = if icon.is_some() { 18.0 } else { 0.0 };
    let pad = if label.is_empty() { 12.0 } else { 24.0 };
    let width = (galley.size().x + icon_w + pad).max(height);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.14);
        let base_fill = if active {
            GLASS_PILL_ACTIVE
        } else {
            GLASS_PILL_IDLE
        };
        let fill = lerp_color(base_fill, GLASS_PILL_HOVER, hover * if active { 0.35 } else { 1.0 });
        let rim = if active {
            RIM_ACTIVE
        } else {
            lerp_color(RIM_SUBTLE, RIM_BRIGHT, hover)
        };
        let radius = 8u8;
        let draw_rect = if resp.is_pointer_button_down_on() {
            rect.shrink(0.6)
        } else {
            rect
        };

        p.rect_filled(draw_rect, CornerRadius::same(radius), fill);
        p.rect_stroke(
            draw_rect,
            CornerRadius::same(radius),
            Stroke::new(1.0_f32, rim),
            StrokeKind::Inside,
        );

        let text_col = if active || hover > 0.3 {
            TEXT_PRIMARY
        } else {
            Color32::from_rgb(212, 216, 224)
        };

        let total_w = galley.size().x + if icon.is_some() && !label.is_empty() { icon_w } else if icon.is_some() { 13.0 } else { 0.0 };
        let mut cx = draw_rect.center().x - total_w * 0.5;
        if let Some(ic) = icon {
            draw_icon(p, Pos2::new(cx + 6.0, draw_rect.center().y), 13.0, ic, text_col);
            cx += icon_w;
        }
        if !label.is_empty() {
            p.galley(
                Pos2::new(cx, draw_rect.center().y - galley.size().y * 0.5),
                galley,
                text_col,
            );
        }
    }

    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Circular glass icon button (used for transport controls, queue, window controls, etc.).
pub fn glass_icon_button(
    ui: &mut Ui,
    icon: Icon,
    diameter: f32,
    icon_size: f32,
    active: bool,
    primary_circle: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.13);
        let center = rect.center();
        let r = diameter * 0.5 - if resp.is_pointer_button_down_on() { 0.8 } else { 0.0 };

        if primary_circle {
            if hover > 0.01 {
                p.circle_filled(
                    center,
                    r + 2.5,
                    rgba(255, 255, 255, (hover * 20.0) as u8),
                );
            }
            let bg = lerp_color(FROST_CTA_IDLE, FROST_CTA_HOVER, hover);
            p.circle_filled(center, r, bg);
            draw_icon(p, center, icon_size, icon, FROST_CTA_TEXT);
        } else {
            let bg_alpha = if active {
                26.0 + hover * 12.0
            } else {
                hover * 18.0
            };
            if bg_alpha > 1.0 {
                p.circle_filled(
                    center,
                    r,
                    rgba(255, 255, 255, bg_alpha as u8),
                );
                if active {
                    p.circle_stroke(center, r, Stroke::new(1.0_f32, RIM_BRIGHT));
                }
            }
            let col = if active {
                TEXT_PRIMARY
            } else {
                lerp_color(TEXT_SECONDARY, TEXT_PRIMARY, hover)
            };
            draw_icon(p, center, icon_size, icon, col);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Sleek Liquid Glass interactive horizontal slider (for Seek bar & Volume).
/// Optionally draws a subtle DJ mix trigger marker on the track.
pub fn glass_slider(
    ui: &mut Ui,
    id_salt: &str,
    width: f32,
    height: f32,
    value: f32,
    mix_marker_frac: Option<f32>,
    hover_formatter: Option<&dyn Fn(f32) -> String>,
) -> (Option<f32>, Response) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width.max(40.0), height), Sense::click_and_drag());
    let mut new_val = None;

    let interacted = resp.dragged() || resp.clicked();
    let pointer_frac = resp
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.hover_pos()))
        .map(|pos| ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0));

    if interacted {
        if let Some(f) = pointer_frac {
            new_val = Some(f);
        }
    }

    let shown_val = new_val.unwrap_or(value).clamp(0.0, 1.0);
    let hover = ui.ctx().animate_bool_with_time(
        ui.make_persistent_id(id_salt),
        resp.hovered() || resp.dragged(),
        0.14,
    );

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let cy = rect.center().y;
        let track_h = 3.5 + hover * 1.5;
        let track_rect = Rect::from_min_max(
            Pos2::new(rect.left(), cy - track_h * 0.5),
            Pos2::new(rect.right(), cy + track_h * 0.5),
        );
        let cr = CornerRadius::same((track_h * 0.5).ceil() as u8);

        // Background glass rail
        p.rect_filled(track_rect, cr, Color32::from_rgba_unmultiplied(255, 255, 255, 26));

        // Optional subtle DJ mix zone highlight on the right side of the rail
        if let Some(mf) = mix_marker_frac {
            let mf = mf.clamp(0.05, 0.98);
            let mx = rect.left() + rect.width() * mf;
            let mix_rect = Rect::from_min_max(
                Pos2::new(mx, track_rect.top()),
                Pos2::new(rect.right(), track_rect.bottom()),
            );
            p.rect_filled(
                mix_rect,
                cr,
                Color32::from_rgba_unmultiplied(210, 218, 232, 34),
            );
            // Crisp DJ cue tick
            p.line_segment(
                [Pos2::new(mx, cy - 5.0), Pos2::new(mx, cy + 5.0)],
                Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(235, 238, 245, 110)),
            );
        }

        // Filled progress track
        let fill_x = rect.left() + rect.width() * shown_val;
        let fill_rect = Rect::from_min_max(
            track_rect.left_top(),
            Pos2::new(fill_x.max(track_rect.left()), track_rect.bottom()),
        );
        let fill_col = lerp_color(
            Color32::from_rgb(206, 210, 220),
            Color32::from_rgb(248, 250, 255),
            hover,
        );
        p.rect_filled(fill_rect, cr, fill_col);

        // Luminous glass thumb knob (smoothly expands on hover/drag)
        let knob_r = 3.5 + hover * 3.0;
        if hover > 0.02 || resp.dragged() {
            p.circle_filled(
                Pos2::new(fill_x, cy),
                knob_r + 3.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, (hover * 34.0) as u8),
            );
            p.circle_filled(Pos2::new(fill_x, cy), knob_r, Color32::WHITE);
        } else {
            p.circle_filled(Pos2::new(fill_x, cy), 3.0, Color32::from_rgb(230, 234, 242));
        }

        // Hover timestamp pill tooltip directly above pointer
        if (resp.hovered() || resp.dragged()) && hover_formatter.is_some() {
            if let (Some(fmt), Some(f)) = (hover_formatter, pointer_frac) {
                let text = fmt(f);
                let tx = rect.left() + rect.width() * f;
                let galley = p.layout_no_wrap(text, font_regular(11.0), TEXT_PRIMARY);
                let tip_size = galley.size() + Vec2::new(12.0, 6.0);
                let tip_rect = Rect::from_center_size(
                    Pos2::new(
                        tx.clamp(rect.left() + tip_size.x * 0.5, rect.right() - tip_size.x * 0.5),
                        cy - 18.0,
                    ),
                    tip_size,
                );
                p.rect_filled(
                    tip_rect,
                    CornerRadius::same(6),
                    Color32::from_rgba_premultiplied(20, 22, 27, 235),
                );
                p.rect_stroke(tip_rect, CornerRadius::same(6), Stroke::new(1.0_f32, RIM_BRIGHT), StrokeKind::Inside);
                p.galley(
                    Pos2::new(
                        tip_rect.center().x - galley.size().x * 0.5,
                        tip_rect.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    TEXT_PRIMARY,
                );
            }
        }
    }

    (new_val, resp.on_hover_cursor(egui::CursorIcon::PointingHand))
}

/// Sleek Liquid Glass interactive horizontal seek slider that tracks the pointer
/// visually while dragging without interrupting playback, and only commits the
/// seek position (`Some(target_frac)`) when the mouse button is released.
pub fn glass_seek_slider(
    ui: &mut Ui,
    id_salt: &str,
    width: f32,
    height: f32,
    value: f32,
    mix_marker_frac: Option<f32>,
    hover_formatter: Option<&dyn Fn(f32) -> String>,
) -> (Option<f32>, f32) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(width.max(40.0), height), Sense::click_and_drag());
    let base_id = ui.make_persistent_id(id_salt);
    let scrub_id = base_id.with("scrub_frac");
    let hold_id = base_id.with("hold_frac");
    let now = ui.input(|i| i.time);

    let pointer_frac = resp
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.hover_pos()))
        .map(|pos| ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0));

    let is_scrubbing = resp.dragged() || resp.is_pointer_button_down_on();
    let mut committed_val = None;

    if is_scrubbing {
        if let Some(f) = pointer_frac {
            ui.ctx().data_mut(|d| d.insert_temp(scrub_id, f));
        }
    }

    if resp.drag_stopped() || resp.clicked() {
        let final_frac = pointer_frac
            .or_else(|| ui.ctx().data_mut(|d| d.get_temp::<f32>(scrub_id)))
            .unwrap_or(value)
            .clamp(0.0, 1.0);
        ui.ctx().data_mut(|d| {
            d.remove::<f32>(scrub_id);
            d.insert_temp(hold_id, (final_frac, now));
        });
        committed_val = Some(final_frac);
    }

    let shown_val = if is_scrubbing {
        pointer_frac
            .or_else(|| ui.ctx().data_mut(|d| d.get_temp::<f32>(scrub_id)))
            .unwrap_or(value)
            .clamp(0.0, 1.0)
    } else if let Some(c) = committed_val {
        c
    } else if let Some((held_f, held_at)) =
        ui.ctx().data_mut(|d| d.get_temp::<(f32, f64)>(hold_id))
    {
        if (now - held_at) < 0.16 && (value - held_f).abs() > 0.015 {
            held_f
        } else {
            ui.ctx().data_mut(|d| d.remove::<(f32, f64)>(hold_id));
            value.clamp(0.0, 1.0)
        }
    } else {
        value.clamp(0.0, 1.0)
    };

    let hover = ui.ctx().animate_bool_with_time(
        base_id,
        resp.hovered() || is_scrubbing,
        0.14,
    );

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let cy = rect.center().y;
        let track_h = 3.5 + hover * 1.5;
        let track_rect = Rect::from_min_max(
            Pos2::new(rect.left(), cy - track_h * 0.5),
            Pos2::new(rect.right(), cy + track_h * 0.5),
        );
        let cr = CornerRadius::same((track_h * 0.5).ceil() as u8);

        p.rect_filled(
            track_rect,
            cr,
            Color32::from_rgba_unmultiplied(255, 255, 255, 26),
        );

        if let Some(mf) = mix_marker_frac {
            let mf = mf.clamp(0.05, 0.98);
            let mx = rect.left() + rect.width() * mf;
            let mix_rect = Rect::from_min_max(
                Pos2::new(mx, track_rect.top()),
                Pos2::new(rect.right(), track_rect.bottom()),
            );
            p.rect_filled(
                mix_rect,
                cr,
                Color32::from_rgba_unmultiplied(210, 218, 232, 34),
            );
            p.line_segment(
                [Pos2::new(mx, cy - 5.0), Pos2::new(mx, cy + 5.0)],
                Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(235, 238, 245, 110)),
            );
        }

        let fill_x = rect.left() + rect.width() * shown_val;
        let fill_rect = Rect::from_min_max(
            track_rect.left_top(),
            Pos2::new(fill_x.max(track_rect.left()), track_rect.bottom()),
        );
        let fill_col = lerp_color(
            Color32::from_rgb(206, 210, 220),
            Color32::from_rgb(248, 250, 255),
            hover,
        );
        p.rect_filled(fill_rect, cr, fill_col);

        let knob_r = 3.5 + hover * 3.0;
        if hover > 0.02 || is_scrubbing {
            p.circle_filled(
                Pos2::new(fill_x, cy),
                knob_r + 3.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, (hover * 34.0) as u8),
            );
            p.circle_filled(Pos2::new(fill_x, cy), knob_r, Color32::WHITE);
        } else {
            p.circle_filled(Pos2::new(fill_x, cy), 3.0, Color32::from_rgb(230, 234, 242));
        }

        if (resp.hovered() || is_scrubbing) && hover_formatter.is_some() {
            let tip_frac = if is_scrubbing {
                Some(shown_val)
            } else {
                pointer_frac
            };
            if let (Some(fmt), Some(f)) = (hover_formatter, tip_frac) {
                let text = fmt(f);
                let tx = rect.left() + rect.width() * f;
                let galley = p.layout_no_wrap(text, font_regular(11.0), TEXT_PRIMARY);
                let tip_size = galley.size() + Vec2::new(12.0, 6.0);
                let tip_rect = Rect::from_center_size(
                    Pos2::new(
                        tx.clamp(
                            rect.left() + tip_size.x * 0.5,
                            rect.right() - tip_size.x * 0.5,
                        ),
                        cy - 18.0,
                    ),
                    tip_size,
                );
                p.rect_filled(
                    tip_rect,
                    CornerRadius::same(6),
                    Color32::from_rgba_premultiplied(20, 22, 27, 235),
                );
                p.rect_stroke(
                    tip_rect,
                    CornerRadius::same(6),
                    Stroke::new(1.0_f32, RIM_BRIGHT),
                    StrokeKind::Inside,
                );
                p.galley(
                    Pos2::new(
                        tip_rect.center().x - galley.size().x * 0.5,
                        tip_rect.center().y - galley.size().y * 0.5,
                    ),
                    galley,
                    TEXT_PRIMARY,
                );
            }
        }
    }

    let _ = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (committed_val, shown_val)
}

/// Minimalist Liquid Glass toggle switch for Settings & Setup.
pub fn glass_toggle(ui: &mut Ui, checked: &mut bool) -> Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *checked, 0.16);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let bg = lerp_color(
            Color32::from_rgba_premultiplied(36, 39, 46, 180),
            Color32::from_rgb(224, 228, 236),
            t,
        );
        let cr = CornerRadius::same(11);
        p.rect_filled(rect, cr, bg);
        p.rect_stroke(
            rect,
            cr,
            Stroke::new(1.0_f32, lerp_color(RIM_SUBTLE, RIM_ACTIVE, t)),
            StrokeKind::Inside,
        );
        let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
        let knob_col = lerp_color(Color32::from_rgb(170, 175, 186), Color32::from_rgb(14, 15, 19), t);
        p.circle_filled(Pos2::new(knob_x, rect.center().y), 7.5, knob_col);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(dead_code)]
/// Deterministic, refined Monochrome Liquid Glass procedural cover artwork for playlists & tracks.
pub fn draw_procedural_cover(
    painter: &Painter,
    rect: Rect,
    seed: u64,
    radius: u8,
    time: f32,
    spinning: bool,
) {
    let cr = CornerRadius::same(radius);
    // Subtle dark gradient base varied deterministically per track/playlist
    let v1 = ((seed & 0xFF) as f32) / 255.0;
    let v2 = (((seed >> 8) & 0xFF) as f32) / 255.0;
    let pattern = (seed >> 16) % 3;

    let base_val = (22.0 + v1 * 18.0) as u8;
    let base_col = Color32::from_rgb(base_val, base_val + 1, base_val + 4);
    painter.rect_filled(rect, cr, base_col);

    // Clipped subtle monochrome liquid glass geometry inside cover tile
    let clipped = painter.with_clip_rect(rect);
    let c = rect.center();
    let s = rect.width().min(rect.height());

    // Ambient silver-mist highlight orb
    let orb_x = c.x + (v1 - 0.5) * s * 0.48;
    let orb_y = c.y + (v2 - 0.5) * s * 0.48;
    clipped.circle_filled(
        Pos2::new(orb_x, orb_y),
        s * 0.55,
        Color32::from_rgba_unmultiplied(255, 255, 255, 18),
    );
    clipped.circle_filled(
        Pos2::new(orb_x, orb_y),
        s * 0.30,
        Color32::from_rgba_unmultiplied(255, 255, 255, 16),
    );

    match pattern {
        0 => {
            // Minimalist monochrome vinyl disc grooves
            let angle = if spinning { time * 1.4 } else { v1 * PI };
            clipped.circle_stroke(
                c,
                s * 0.36,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 30)),
            );
            clipped.circle_stroke(
                c,
                s * 0.24,
                Stroke::new(0.8_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 18)),
            );
            // Rotating subtle sheen ray
            let dir = Vec2::new(angle.cos(), angle.sin());
            clipped.line_segment(
                [c + dir * (s * 0.14), c + dir * (s * 0.36)],
                Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 48)),
            );
            clipped.circle_filled(c, s * 0.08, Color32::from_rgba_unmultiplied(235, 238, 245, 130));
        }
        1 => {
            // Acoustic monochrome liquid wave lines
            let phase = if spinning { time * 2.4 } else { v2 * 6.0 };
            for row in 0..3 {
                let mut pts = Vec::with_capacity(14);
                let y_base = c.y + (row as f32 - 1.0) * s * 0.17;
                for i in 0..=13 {
                    let t = i as f32 / 13.0;
                    let x = rect.left() + t * rect.width();
                    let y = y_base + ((t * PI * 2.2 + phase + row as f32 * 0.9).sin()) * s * 0.07;
                    pts.push(Pos2::new(x, y));
                }
                let a = 32 + row as u8 * 14;
                clipped.add(egui::Shape::line(
                    pts,
                    Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(240, 244, 250, a)),
                ));
            }
        }
        _ => {
            // Minimalist frosted diagonal prism + music emblem
            let p1 = Pos2::new(rect.left() - s * 0.1, rect.bottom());
            let p2 = Pos2::new(rect.right(), rect.top() - s * 0.1);
            clipped.line_segment(
                [p1, p2],
                Stroke::new(s * 0.22, Color32::from_rgba_unmultiplied(255, 255, 255, 12)),
            );
            draw_icon(
                &clipped,
                c,
                s * 0.42,
                Icon::MusicNote,
                Color32::from_rgba_unmultiplied(235, 238, 245, 115),
            );
        }
    }

    // Crisp outer glass specular rim
    painter.rect_stroke(rect, cr, Stroke::new(1.0_f32, RIM_BRIGHT), StrokeKind::Inside);
}

pub fn draw_truncated_text(
    painter: &Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    color: Color32,
    align: Align2,
) {
    let clipped = painter.with_clip_rect(rect);
    let galley = clipped.layout_no_wrap(text.to_string(), font.clone(), color);
    if galley.size().x <= rect.width() {
        let pos = align.pos_in_rect(&rect);
        clipped.text(pos, align, text, font, color);
        return;
    }

    // Binary search clean ellipsis truncation
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    let mut best = String::from("...");
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let cand: String = chars[..mid].iter().collect::<String>() + "…";
        let w = clipped
            .layout_no_wrap(cand.clone(), font.clone(), color)
            .size()
            .x;
        if w <= rect.width() {
            best = cand;
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    let pos = align.pos_in_rect(&rect);
    clipped.text(pos, align, best, font, color);
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
        (a.a() as f32 + (b.a() as f32 - a.a() as f32) * t).round() as u8,
    )
}
