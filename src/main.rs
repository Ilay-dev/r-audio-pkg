#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod covers;
mod discord_rpc;
mod i18n;
mod library;
mod lyrics;
mod media_controls;
mod playlist_sync;
mod ui;
mod updater;
mod youtube;

use crate::ui::RustifyApp;
use egui::{IconData, ViewportBuilder};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

const ICON_PNG: &[u8] = include_bytes!("../assets/rustify_256.png");

fn load_window_icon() -> Option<IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

#[cfg(target_os = "windows")]
fn apply_windows_dwm_styling(hwnd: *mut c_void) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    unsafe {
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const _ as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
        let corner: i32 = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
        // Subtle graphite native border matching Liquid Glass aesthetic (COLORREF = 0x00BBGGRR)
        let border_color: u32 = 0x002A2724;
        let _ = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_BORDER_COLOR as u32,
            &border_color as *const _ as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn main() -> eframe::Result<()> {
    let mut viewport = ViewportBuilder::default()
        .with_title("Rustify")
        .with_app_id("rustify")
        .with_inner_size([1240.0, 780.0])
        .with_min_inner_size([340.0, 86.0])
        .with_decorations(false)
        .with_resizable(true)
        .with_drag_and_drop(true);

    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: true,
        vsync: true,
        // On Linux (X11/Wayland/Mesa), requesting 4x MSAA fails with NoGlutinConfigs on many drivers/compositors;
        // egui already applies smooth per-primitive feathering AA natively.
        multisampling: if cfg!(target_os = "windows") { 4 } else { 0 },
        ..Default::default()
    };

    eframe::run_native(
        "Rustify",
        options,
        Box::new(|cc| {
            let hwnd = cc
                .window_handle()
                .ok()
                .and_then(|wh| match wh.as_raw() {
                    RawWindowHandle::Win32(handle) => {
                        let ptr = handle.hwnd.get() as *mut c_void;
                        #[cfg(target_os = "windows")]
                        apply_windows_dwm_styling(ptr);
                        Some(ptr)
                    }
                    _ => None,
                });

            Ok(Box::new(RustifyApp::new(cc, hwnd)))
        }),
    )
}
