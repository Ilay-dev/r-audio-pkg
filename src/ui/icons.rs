use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::f32::consts::PI;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
    Order,
    Shuffle,
    DjMix,
    Repeat,
    RepeatOne,
    HeartOutline,
    HeartFilled,
    Search,
    Plus,
    YouTube,
    Settings,
    Queue,
    VolumeHigh,
    VolumeLow,
    VolumeMute,
    Folder,
    Refresh,
    Check,
    Close,
    Minimize,
    Maximize,
    Restore,
    MusicNote,
    Disc,
    Trash,
    Edit,
    MiniPlayer,
    Expand,
    MoreHorizontal,
    Clock,
    Image,
    Visualizer,
    Lyrics,
    Link,
}

pub fn draw_icon(painter: &Painter, center: Pos2, size: f32, icon: Icon, color: Color32) {
    let r = size * 0.5;
    let sw = (size * 0.105).clamp(1.25, 2.2);
    let stroke = Stroke::new(sw, color);

    match icon {
        Icon::Play => {
            // Optical right-offset triangle for crisp balanced visual center
            let pts = vec![
                Pos2::new(center.x - r * 0.34, center.y - r * 0.58),
                Pos2::new(center.x + r * 0.62, center.y),
                Pos2::new(center.x - r * 0.34, center.y + r * 0.58),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            let bw = (r * 0.34).max(2.5);
            let bh = r * 1.12;
            let gap = r * 0.22;
            let cr = CornerRadius::same((bw * 0.4).round() as u8);
            let left = Rect::from_center_size(
                Pos2::new(center.x - gap - bw * 0.5, center.y),
                Vec2::new(bw, bh),
            );
            let right = Rect::from_center_size(
                Pos2::new(center.x + gap + bw * 0.5, center.y),
                Vec2::new(bw, bh),
            );
            painter.rect_filled(left, cr, color);
            painter.rect_filled(right, cr, color);
        }
        Icon::Next => {
            let tri = vec![
                Pos2::new(center.x - r * 0.58, center.y - r * 0.52),
                Pos2::new(center.x + r * 0.26, center.y),
                Pos2::new(center.x - r * 0.58, center.y + r * 0.52),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
            let bar = Rect::from_center_size(
                Pos2::new(center.x + r * 0.48, center.y),
                Vec2::new((r * 0.24).max(2.0), r * 1.04),
            );
            painter.rect_filled(bar, CornerRadius::same(1), color);
        }
        Icon::Prev => {
            let bar = Rect::from_center_size(
                Pos2::new(center.x - r * 0.48, center.y),
                Vec2::new((r * 0.24).max(2.0), r * 1.04),
            );
            painter.rect_filled(bar, CornerRadius::same(1), color);
            let tri = vec![
                Pos2::new(center.x + r * 0.58, center.y - r * 0.52),
                Pos2::new(center.x - r * 0.26, center.y),
                Pos2::new(center.x + r * 0.58, center.y + r * 0.52),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::Order => {
            // Three clean horizontal playlist/order bars + subtle arrow
            for (i, w) in [1.25f32, 1.05, 0.78].iter().enumerate() {
                let y = center.y + (i as f32 - 1.0) * r * 0.46;
                painter.line_segment(
                    [
                        Pos2::new(center.x - r * 0.65, y),
                        Pos2::new(center.x - r * 0.65 + r * w, y),
                    ],
                    stroke,
                );
            }
            // Small downward chevron on right
            let ax = center.x + r * 0.52;
            painter.line_segment(
                [Pos2::new(ax, center.y - r * 0.10), Pos2::new(ax, center.y + r * 0.48)],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(ax - r * 0.22, center.y + r * 0.26),
                    Pos2::new(ax, center.y + r * 0.50),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(ax + r * 0.22, center.y + r * 0.26),
                    Pos2::new(ax, center.y + r * 0.50),
                ],
                stroke,
            );
        }
        Icon::Shuffle => {
            // Classic crossing shuffle arrows
            let x0 = center.x - r * 0.68;
            let x1 = center.x - r * 0.16;
            let x2 = center.x + r * 0.16;
            let x3 = center.x + r * 0.62;
            let yt = center.y - r * 0.36;
            let yb = center.y + r * 0.36;

            painter.add(Shape::line(
                vec![
                    Pos2::new(x0, yt),
                    Pos2::new(x1, yt),
                    Pos2::new(x2, yb),
                    Pos2::new(x3, yb),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![
                    Pos2::new(x0, yb),
                    Pos2::new(x1, yb),
                    Pos2::new(x2, yt),
                    Pos2::new(x3, yt),
                ],
                stroke,
            ));
            // Arrowheads
            for ay in [yt, yb] {
                painter.line_segment(
                    [Pos2::new(x3 - r * 0.22, ay - r * 0.20), Pos2::new(x3 + r * 0.04, ay)],
                    stroke,
                );
                painter.line_segment(
                    [Pos2::new(x3 - r * 0.22, ay + r * 0.20), Pos2::new(x3 + r * 0.04, ay)],
                    stroke,
                );
            }
        }
        Icon::DjMix => {
            // Twin overlapping crossfade waves (outgoing fading down, incoming rising up!)
            let steps = 16;
            let mut wave_a = Vec::with_capacity(steps + 1);
            let mut wave_b = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let x = center.x + (t - 0.5) * r * 1.45;
                // Smooth crossing S-curves
                let s = t * t * (3.0 - 2.0 * t);
                let ya = center.y + (s - 0.5) * r * 0.85;
                let yb = center.y + (0.5 - s) * r * 0.85;
                wave_a.push(Pos2::new(x, ya));
                wave_b.push(Pos2::new(x, yb));
            }
            painter.add(Shape::line(wave_a, Stroke::new(sw * 1.05, color)));
            painter.add(Shape::line(
                wave_b,
                Stroke::new(sw * 1.05, color.gamma_multiply(0.68)),
            ));
            // Crisp beat-sync diamond dot right at intersection
            painter.circle_filled(center, (r * 0.16).max(1.8), color);
        }
        Icon::Repeat | Icon::RepeatOne => {
            let xl = center.x - r * 0.58;
            let xr = center.x + r * 0.58;
            let yt = center.y - r * 0.36;
            let yb = center.y + r * 0.36;
            painter.add(Shape::line(
                vec![
                    Pos2::new(xl, center.y - r * 0.08),
                    Pos2::new(xl, yt),
                    Pos2::new(xr, yt),
                ],
                stroke,
            ));
            painter.line_segment(
                [Pos2::new(xr - r * 0.22, yt - r * 0.20), Pos2::new(xr + r * 0.02, yt)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(xr - r * 0.22, yt + r * 0.20), Pos2::new(xr + r * 0.02, yt)],
                stroke,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(xr, center.y + r * 0.08),
                    Pos2::new(xr, yb),
                    Pos2::new(xl, yb),
                ],
                stroke,
            ));
            painter.line_segment(
                [Pos2::new(xl + r * 0.22, yb - r * 0.20), Pos2::new(xl - r * 0.02, yb)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(xl + r * 0.22, yb + r * 0.20), Pos2::new(xl - r * 0.02, yb)],
                stroke,
            );
            if icon == Icon::RepeatOne {
                painter.circle_filled(center, (r * 0.17).max(1.8), color);
            }
        }
        Icon::HeartOutline | Icon::HeartFilled => {
            let mut pts = Vec::with_capacity(28);
            for i in 0..28 {
                let t = (i as f32 / 28.0) * 2.0 * PI;
                let hx = 16.0 * t.sin().powi(3);
                let hy = -(13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos());
                pts.push(Pos2::new(
                    center.x + (hx / 18.0) * r * 0.78,
                    center.y + (hy / 18.0) * r * 0.78 - r * 0.04,
                ));
            }
            if icon == Icon::HeartFilled {
                painter.add(Shape::Path(egui::epaint::PathShape::convex_polygon(
                    pts.clone(),
                    color,
                    Stroke::NONE,
                )));
                // Also fill heart lobes cleanly so concave top notch stays sharp
                painter.circle_filled(
                    Pos2::new(center.x - r * 0.27, center.y - r * 0.18),
                    r * 0.31,
                    color,
                );
                painter.circle_filled(
                    Pos2::new(center.x + r * 0.27, center.y - r * 0.18),
                    r * 0.31,
                    color,
                );
                painter.add(Shape::convex_polygon(
                    vec![
                        Pos2::new(center.x - r * 0.54, center.y - r * 0.06),
                        Pos2::new(center.x + r * 0.54, center.y - r * 0.06),
                        Pos2::new(center.x, center.y + r * 0.62),
                    ],
                    color,
                    Stroke::NONE,
                ));
            } else {
                painter.add(Shape::closed_line(pts, stroke));
            }
        }
        Icon::Search => {
            let c = Pos2::new(center.x - r * 0.12, center.y - r * 0.12);
            let rad = r * 0.46;
            painter.circle_stroke(c, rad, stroke);
            let dir = Vec2::new(0.707, 0.707);
            painter.line_segment(
                [c + dir * rad, Pos2::new(center.x + r * 0.62, center.y + r * 0.62)],
                Stroke::new(sw * 1.15, color),
            );
        }
        Icon::Plus => {
            let cx = center.x.round();
            let cy = center.y.round();
            let l = (r * 0.62).round().max(4.0);
            let s = Stroke::new(1.5_f32, color);
            painter.line_segment([Pos2::new(cx - l, cy), Pos2::new(cx + l, cy)], s);
            painter.line_segment([Pos2::new(cx, cy - l), Pos2::new(cx, cy + l)], s);
        }
        Icon::YouTube => {
            // Minimalist rounded play-screen icon
            let box_rect = Rect::from_center_size(center, Vec2::new(r * 1.48, r * 1.06));
            painter.rect_stroke(
                box_rect,
                CornerRadius::same((r * 0.30).round() as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            let tri = vec![
                Pos2::new(center.x - r * 0.16, center.y - r * 0.25),
                Pos2::new(center.x + r * 0.28, center.y),
                Pos2::new(center.x - r * 0.16, center.y + r * 0.25),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::Settings => {
            // Clean 8-tooth precision gear (subtle Rust echo!)
            painter.circle_stroke(center, r * 0.44, stroke);
            painter.circle_stroke(center, r * 0.17, Stroke::new(sw * 0.9, color));
            for k in 0..8 {
                let a = k as f32 * PI * 0.25;
                let dir = Vec2::new(a.cos(), a.sin());
                painter.line_segment(
                    [center + dir * (r * 0.48), center + dir * (r * 0.72)],
                    Stroke::new(sw * 1.15, color),
                );
            }
        }
        Icon::Queue => {
            for i in 0..3 {
                let y = center.y + (i as f32 - 1.0) * r * 0.46;
                let x0 = if i == 0 {
                    center.x - r * 0.22
                } else {
                    center.x - r * 0.64
                };
                painter.line_segment([Pos2::new(x0, y), Pos2::new(center.x + r * 0.64, y)], stroke);
            }
            // Small play triangle next to first line
            let ty = center.y - r * 0.46;
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(center.x - r * 0.64, ty - r * 0.20),
                    Pos2::new(center.x - r * 0.36, ty),
                    Pos2::new(center.x - r * 0.64, ty + r * 0.20),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::VolumeHigh | Icon::VolumeLow | Icon::VolumeMute => {
            // Speaker trapezoid
            let speaker = vec![
                Pos2::new(center.x - r * 0.64, center.y - r * 0.24),
                Pos2::new(center.x - r * 0.32, center.y - r * 0.24),
                Pos2::new(center.x + r * 0.04, center.y - r * 0.56),
                Pos2::new(center.x + r * 0.04, center.y + r * 0.56),
                Pos2::new(center.x - r * 0.32, center.y + r * 0.24),
                Pos2::new(center.x - r * 0.64, center.y + r * 0.24),
            ];
            painter.add(Shape::convex_polygon(speaker, color, Stroke::NONE));

            match icon {
                Icon::VolumeMute => {
                    let x0 = center.x + r * 0.26;
                    let x1 = center.x + r * 0.66;
                    let dy = r * 0.22;
                    painter.line_segment(
                        [Pos2::new(x0, center.y - dy), Pos2::new(x1, center.y + dy)],
                        stroke,
                    );
                    painter.line_segment(
                        [Pos2::new(x0, center.y + dy), Pos2::new(x1, center.y - dy)],
                        stroke,
                    );
                }
                Icon::VolumeLow => {
                    draw_arc(painter, Pos2::new(center.x + r * 0.06, center.y), r * 0.34, -0.7, 0.7, stroke);
                }
                Icon::VolumeHigh => {
                    draw_arc(painter, Pos2::new(center.x + r * 0.06, center.y), r * 0.32, -0.7, 0.7, stroke);
                    draw_arc(painter, Pos2::new(center.x + r * 0.06, center.y), r * 0.56, -0.78, 0.78, stroke);
                }
                _ => {}
            }
        }
        Icon::Folder => {
            let pts = vec![
                Pos2::new(center.x - r * 0.66, center.y - r * 0.44),
                Pos2::new(center.x - r * 0.14, center.y - r * 0.44),
                Pos2::new(center.x + r * 0.02, center.y - r * 0.24),
                Pos2::new(center.x + r * 0.66, center.y - r * 0.24),
                Pos2::new(center.x + r * 0.66, center.y + r * 0.48),
                Pos2::new(center.x - r * 0.66, center.y + r * 0.48),
            ];
            painter.add(Shape::closed_line(pts, stroke));
        }
        Icon::Refresh => {
            draw_arc(painter, center, r * 0.54, -2.3, 1.9, stroke);
            let tip = Pos2::new(
                center.x + r * 0.54 * (-2.3f32).cos(),
                center.y + r * 0.54 * (-2.3f32).sin(),
            );
            painter.line_segment([tip, tip + Vec2::new(r * 0.28, -r * 0.06)], stroke);
            painter.line_segment([tip, tip + Vec2::new(r * 0.04, r * 0.28)], stroke);
        }
        Icon::Check => {
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - r * 0.54, center.y + r * 0.04),
                    Pos2::new(center.x - r * 0.14, center.y + r * 0.44),
                    Pos2::new(center.x + r * 0.58, center.y - r * 0.40),
                ],
                Stroke::new(sw * 1.15, color),
            ));
        }
        Icon::Close => {
            let l = r * 0.46;
            painter.line_segment(
                [Pos2::new(center.x - l, center.y - l), Pos2::new(center.x + l, center.y + l)],
                stroke,
            );
            painter.line_segment(
                [Pos2::new(center.x + l, center.y - l), Pos2::new(center.x - l, center.y + l)],
                stroke,
            );
        }
        Icon::Minimize => {
            let l = r * 0.48;
            painter.line_segment(
                [Pos2::new(center.x - l, center.y), Pos2::new(center.x + l, center.y)],
                stroke,
            );
        }
        Icon::Maximize => {
            let rect = Rect::from_center_size(center, Vec2::splat(r * 0.96));
            painter.rect_stroke(rect, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        }
        Icon::Restore => {
            let front = Rect::from_center_size(
                Pos2::new(center.x - r * 0.12, center.y + r * 0.12),
                Vec2::splat(r * 0.78),
            );
            painter.rect_stroke(front, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - r * 0.24, center.y - r * 0.28),
                    Pos2::new(center.x - r * 0.24, center.y - r * 0.48),
                    Pos2::new(center.x + r * 0.50, center.y - r * 0.48),
                    Pos2::new(center.x + r * 0.50, center.y + r * 0.24),
                    Pos2::new(center.x + r * 0.28, center.y + r * 0.24),
                ],
                stroke,
            ));
        }
        Icon::MusicNote => {
            // Crisp beamed eighth note
            let x1 = center.x - r * 0.24;
            let x2 = center.x + r * 0.36;
            painter.circle_filled(Pos2::new(x1 - r * 0.12, center.y + r * 0.44), r * 0.20, color);
            painter.circle_filled(Pos2::new(x2 - r * 0.12, center.y + r * 0.32), r * 0.20, color);
            painter.line_segment([Pos2::new(x1 + r * 0.04, center.y + r * 0.42), Pos2::new(x1 + r * 0.04, center.y - r * 0.42)], stroke);
            painter.line_segment([Pos2::new(x2 + r * 0.04, center.y + r * 0.30), Pos2::new(x2 + r * 0.04, center.y - r * 0.54)], stroke);
            painter.line_segment([Pos2::new(x1 + r * 0.04, center.y - r * 0.42), Pos2::new(x2 + r * 0.04, center.y - r * 0.54)], Stroke::new(sw * 1.7, color));
        }
        Icon::Disc => {
            painter.circle_stroke(center, r * 0.66, stroke);
            painter.circle_stroke(center, r * 0.20, stroke);
        }
        Icon::Trash => {
            painter.line_segment(
                [Pos2::new(center.x - r * 0.56, center.y - r * 0.38), Pos2::new(center.x + r * 0.56, center.y - r * 0.38)],
                stroke,
            );
            let body = Rect::from_min_max(
                Pos2::new(center.x - r * 0.40, center.y - r * 0.38),
                Pos2::new(center.x + r * 0.40, center.y + r * 0.56),
            );
            painter.rect_stroke(body, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(center.x - r * 0.20, center.y - r * 0.56), Pos2::new(center.x + r * 0.20, center.y - r * 0.56)],
                stroke,
            );
        }
        Icon::Edit => {
            painter.line_segment(
                [Pos2::new(center.x - r * 0.46, center.y + r * 0.46), Pos2::new(center.x + r * 0.44, center.y - r * 0.44)],
                Stroke::new(sw * 1.4, color),
            );
            painter.line_segment(
                [Pos2::new(center.x - r * 0.54, center.y + r * 0.54), Pos2::new(center.x - r * 0.30, center.y + r * 0.46)],
                stroke,
            );
        }
        Icon::MiniPlayer => {
            // Outer window frame + filled bottom-right mini-widget rectangle (Picture-in-Picture icon)
            let outer = Rect::from_center_size(center, Vec2::new(r * 1.32, r * 1.00));
            painter.rect_stroke(outer, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            let pip = Rect::from_min_max(
                Pos2::new(center.x + r * 0.04, center.y - r * 0.02),
                Pos2::new(outer.right() - r * 0.16, outer.bottom() - r * 0.16),
            );
            painter.rect_filled(pip, CornerRadius::same(1), color);
        }
        Icon::Expand => {
            // Diagonal expand arrows (bottom-left to top-right)
            let p1 = Pos2::new(center.x - r * 0.48, center.y + r * 0.48);
            let p2 = Pos2::new(center.x + r * 0.48, center.y - r * 0.48);
            painter.line_segment([p1, p2], stroke);
            // Top-right arrowhead
            painter.line_segment([p2, Pos2::new(p2.x - r * 0.42, p2.y)], stroke);
            painter.line_segment([p2, Pos2::new(p2.x, p2.y + r * 0.42)], stroke);
            // Bottom-left arrowhead
            painter.line_segment([p1, Pos2::new(p1.x + r * 0.42, p1.y)], stroke);
            painter.line_segment([p1, Pos2::new(p1.x, p1.y - r * 0.42)], stroke);
        }
        Icon::MoreHorizontal => {
            let dot_r = (r * 0.14).max(1.5);
            for dx in [-0.52f32, 0.0, 0.52] {
                painter.circle_filled(Pos2::new(center.x + r * dx, center.y), dot_r, color);
            }
        }
        Icon::Clock => {
            painter.circle_stroke(center, r * 0.62, stroke);
            painter.line_segment(
                [center, Pos2::new(center.x, center.y - r * 0.36)],
                stroke,
            );
            painter.line_segment(
                [center, Pos2::new(center.x + r * 0.26, center.y + r * 0.10)],
                stroke,
            );
        }
        Icon::Image => {
            let frame = Rect::from_center_size(center, Vec2::splat(r * 1.24));
            painter.rect_stroke(
                frame,
                CornerRadius::same((r * 0.22).round().max(2.0) as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.circle_filled(
                Pos2::new(center.x - r * 0.26, center.y - r * 0.24),
                (r * 0.13).max(1.4),
                color,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(frame.left() + r * 0.14, frame.bottom() - r * 0.18),
                    Pos2::new(center.x - r * 0.12, center.y + r * 0.04),
                    Pos2::new(center.x + r * 0.10, center.y + r * 0.24),
                    Pos2::new(center.x + r * 0.26, center.y + r * 0.08),
                    Pos2::new(frame.right() - r * 0.14, frame.bottom() - r * 0.18),
                ],
                stroke,
            ));
        }
        Icon::Visualizer => {
            // Symmetrical 5-bar acoustic waveform icon
            let heights = [0.32f32, 0.64, 0.96, 0.56, 0.28];
            for (i, &h) in heights.iter().enumerate() {
                let dx = (i as f32 - 2.0) * r * 0.34;
                let x = (center.x + dx).round();
                let dy = r * h * 0.68;
                painter.line_segment(
                    [Pos2::new(x, center.y - dy), Pos2::new(x, center.y + dy)],
                    Stroke::new(sw * 1.05, color),
                );
            }
        }
        Icon::Lyrics => {
            // Minimalist rounded lyric speech-bubble with crisp text lines
            let bubble = Rect::from_center_size(
                Pos2::new(center.x, center.y - r * 0.12),
                Vec2::new(r * 1.44, r * 1.08),
            );
            painter.rect_stroke(
                bubble,
                CornerRadius::same((r * 0.30).round().max(2.0) as u8),
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - r * 0.34, bubble.bottom()),
                    Pos2::new(center.x - r * 0.50, center.y + r * 0.68),
                    Pos2::new(center.x - r * 0.08, bubble.bottom()),
                ],
                stroke,
            ));
            let line_l = bubble.left() + r * 0.28;
            painter.line_segment(
                [
                    Pos2::new(line_l, bubble.center().y - r * 0.16),
                    Pos2::new(bubble.right() - r * 0.28, bubble.center().y - r * 0.16),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(line_l, bubble.center().y + r * 0.16),
                    Pos2::new(bubble.right() - r * 0.48, bubble.center().y + r * 0.16),
                ],
                stroke,
            );
        }
        Icon::Link => {
            // Clean playlist-link / import icon (two overlapping rounded links)
            let left_box = Rect::from_center_size(
                Pos2::new(center.x - r * 0.26, center.y + r * 0.12),
                Vec2::new(r * 0.76, r * 0.44),
            );
            let right_box = Rect::from_center_size(
                Pos2::new(center.x + r * 0.26, center.y - r * 0.12),
                Vec2::new(r * 0.76, r * 0.44),
            );
            let cr = CornerRadius::same((r * 0.22).round().max(2.0) as u8);
            painter.rect_stroke(left_box, cr, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(right_box, cr, stroke, egui::StrokeKind::Inside);
        }
    }
}

fn draw_arc(painter: &Painter, center: Pos2, radius: f32, start: f32, end: f32, stroke: Stroke) {
    let steps = 12;
    let mut pts = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let a = start + (end - start) * (i as f32 / steps as f32);
        pts.push(Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin()));
    }
    painter.add(Shape::line(pts, stroke));
}

/// Animated minimalist 3-bar liquid equalizer for the currently playing track row.
pub fn draw_live_equalizer(painter: &Painter, center: Pos2, size: f32, time: f32, active: bool, energy: f32, color: Color32) {
    let bar_w = (size * 0.17).max(2.2);
    let gap = bar_w * 0.9;
    let max_h = size * 0.85;
    let base_y = center.y + max_h * 0.5;

    for i in 0..3 {
        let phase = time * (5.2 + i as f32 * 1.7) + i as f32 * 2.1;
        let h_frac = if active {
            (0.24 + (0.48 + 0.28 * energy) * (0.5 + 0.5 * phase.sin())).clamp(0.18, 1.0)
        } else {
            0.22 + i as f32 * 0.08
        };
        let h = max_h * h_frac;
        let x = center.x + (i as f32 - 1.0) * (bar_w + gap);
        let rect = Rect::from_min_max(
            Pos2::new(x - bar_w * 0.5, base_y - h),
            Pos2::new(x + bar_w * 0.5, base_y),
        );
        painter.rect_filled(rect, CornerRadius::same(1), color);
    }
}
