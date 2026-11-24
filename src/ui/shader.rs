use egui_glow::glow::{self, HasContext};
use parking_lot::Mutex;
use std::sync::Arc;

const VERT_SRC: &str = r#"#version 330 core
out vec2 v_uv;
void main() {
    vec2 pos = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    v_uv = pos;
    gl_Position = vec4(pos * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAG_SRC: &str = r#"#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform float u_time;
uniform float u_energy;
uniform float u_mix;
uniform vec2 u_resolution;

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

void main() {
    vec2 uv = v_uv;
    float aspect = u_resolution.x / max(u_resolution.y, 1.0);
    vec2 p = (uv - 0.5) * vec2(aspect, 1.0);

    float t = u_time * 0.15;

    // Deep pitch-black obsidian base (#050507)
    vec3 col = vec3(0.020, 0.021, 0.025);

    // Subtle slow-moving dark liquid-glass ambient glow lobes (monochrome silver-graphite)
    vec2 c1 = vec2(-0.32 * aspect + 0.12 * sin(t * 0.9), 0.24 + 0.08 * cos(t * 0.7));
    vec2 c2 = vec2(0.35 * aspect + 0.10 * cos(t * 0.8), -0.22 + 0.09 * sin(t * 0.6));

    float lobe1 = exp(-dot(p - c1, p - c1) * 1.9);
    float lobe2 = exp(-dot(p - c2, p - c2) * 2.1);
    float center_glow = exp(-dot(p, p) * 1.2);

    float reactive = 0.028 + 0.022 * u_energy + 0.025 * u_mix;
    col += vec3(0.92, 0.95, 1.00) * (lobe1 * 0.032 + lobe2 * 0.026 + center_glow * reactive);

    // Subtle architectural dark technical grid (32px spacing, fades softly toward edges — matches Ilay-dev!)
    vec2 px = uv * u_resolution;
    vec2 grid_dist = abs(fract(px / 32.0 - 0.5) - 0.5) * 32.0;
    float line = 1.0 - clamp(min(grid_dist.x, grid_dist.y), 0.0, 1.0);
    float grid_mask = clamp(1.0 - dot(uv - 0.5, uv - 0.5) * 1.6, 0.15, 1.0);
    col += vec3(1.0) * (line * 0.016 * grid_mask);

    // Ultra-fine dithering to prevent any 8-bit dark banding
    float dither = (hash(gl_FragCoord.xy + fract(u_time)) - 0.5) / 255.0;
    col += dither;

    frag_color = vec4(clamp(col, 0.0, 1.0), 1.0);
}
"#;

struct LiquidGlResources {
    program: glow::Program,
    vao: glow::VertexArray,
    u_time: Option<glow::UniformLocation>,
    u_energy: Option<glow::UniformLocation>,
    u_mix: Option<glow::UniformLocation>,
    u_resolution: Option<glow::UniformLocation>,
}

pub struct LiquidBackdrop {
    gl_res: Arc<Mutex<Option<LiquidGlResources>>>,
    available: bool,
}

impl LiquidBackdrop {
    pub fn new(gl: &glow::Context) -> Self {
        let res = unsafe { compile_program(gl) };
        let available = res.is_some();
        Self {
            gl_res: Arc::new(Mutex::new(res)),
            available,
        }
    }

    pub fn paint(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        time: f32,
        energy: f32,
        mix_intensity: f32,
    ) {
        if !self.available {
            painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_rgb(5, 5, 7));
            return;
        }

        let res_arc = self.gl_res.clone();
        let cb = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let gl = glow_painter.gl();
            let guard = res_arc.lock();
            if let Some(ref res) = *guard {
                unsafe {
                    gl.use_program(Some(res.program));
                    if let Some(loc) = res.u_time {
                        gl.uniform_1_f32(Some(&loc), time);
                    }
                    if let Some(loc) = res.u_energy {
                        gl.uniform_1_f32(Some(&loc), energy);
                    }
                    if let Some(loc) = res.u_mix {
                        gl.uniform_1_f32(Some(&loc), mix_intensity);
                    }
                    if let Some(loc) = res.u_resolution {
                        let vp = info.viewport_in_pixels();
                        gl.uniform_2_f32(Some(&loc), vp.width_px as f32, vp.height_px as f32);
                    }
                    gl.bind_vertex_array(Some(res.vao));
                    gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    gl.bind_vertex_array(None);
                    gl.use_program(None);
                }
            }
        });

        painter.add(egui::PaintCallback {
            rect,
            callback: Arc::new(cb),
        });
    }

    pub fn destroy(&self, gl: &glow::Context) {
        let mut guard = self.gl_res.lock();
        if let Some(res) = guard.take() {
            unsafe {
                gl.delete_program(res.program);
                gl.delete_vertex_array(res.vao);
            }
        }
    }
}

unsafe fn compile_program(gl: &glow::Context) -> Option<LiquidGlResources> {
    let program = gl.create_program().ok()?;
    let vs = gl.create_shader(glow::VERTEX_SHADER).ok()?;
    gl.shader_source(vs, VERT_SRC);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        gl.delete_shader(vs);
        gl.delete_program(program);
        return None;
    }

    let fs = gl.create_shader(glow::FRAGMENT_SHADER).ok()?;
    gl.shader_source(fs, FRAG_SRC);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        gl.delete_program(program);
        return None;
    }

    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);

    if !gl.get_program_link_status(program) {
        gl.delete_program(program);
        return None;
    }

    let vao = gl.create_vertex_array().ok()?;
    let u_time = gl.get_uniform_location(program, "u_time");
    let u_energy = gl.get_uniform_location(program, "u_energy");
    let u_mix = gl.get_uniform_location(program, "u_mix");
    let u_resolution = gl.get_uniform_location(program, "u_resolution");

    Some(LiquidGlResources {
        program,
        vao,
        u_time,
        u_energy,
        u_mix,
        u_resolution,
    })
}
