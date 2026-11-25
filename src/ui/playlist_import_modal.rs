use crate::config::AppConfig;
use crate::i18n::Language;
use crate::playlist_sync::{detect_source_kind, PlaylistSourceKind, PlaylistSyncManager};
use crate::ui::icons::{draw_icon, Icon};
use crate::ui::theme::{
    draw_liquid_glass_panel, draw_truncated_text, font_regular, font_semibold, frosted_cta_button,
    glass_icon_button, glass_pill_button, rgba, GlassLevel, FROST_CTA_IDLE, FROST_CTA_TEXT,
    GLASS_PILL_IDLE, RIM_BRIGHT, RIM_SUBTLE, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};
use crate::youtube::resolve_ytdlp;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportTab {
    #[default]
    OnlineLink,
    PcFolder,
    EmptyLocal,
}

pub struct PlaylistImportModalState {
    pub open: bool,
    pub tab: ImportTab,
    pub url: String,
    pub custom_name: String,
    pub new_playlist_name: String,
    /// When opened from inside a playlist ("Import" next to YouTube), allows importing into that playlist.
    pub target_playlist: Option<(String, PathBuf)>,
    pub import_into_current: bool,
    pub error: Option<String>,
}

impl Default for PlaylistImportModalState {
    fn default() -> Self {
        Self {
            open: false,
            tab: ImportTab::OnlineLink,
            url: String::new(),
            custom_name: String::new(),
            new_playlist_name: String::new(),
            target_playlist: None,
            import_into_current: false,
            error: None,
        }
    }
}

impl PlaylistImportModalState {
    pub fn open_from_sidebar(&mut self) {
        self.open = true;
        self.tab = ImportTab::OnlineLink;
        self.target_playlist = None;
        self.import_into_current = false;
        self.error = None;
    }

    pub fn open_for_playlist(&mut self, name: String, dir: PathBuf) {
        self.open = true;
        self.tab = ImportTab::OnlineLink;
        self.target_playlist = Some((name, dir));
        self.import_into_current = true;
        self.error = None;
    }
}

pub enum PlaylistImportAction {
    ImportOnlineNew {
        url: String,
        custom_name: Option<String>,
    },
    ImportOnlineIntoExisting {
        url: String,
        playlist_name: String,
        playlist_dir: PathBuf,
    },
    ImportPcFolder(PathBuf),
    CreateEmptyPlaylist(String),
    CancelSync,
}

pub fn show_playlist_import_modal(
    ui: &mut Ui,
    screen: Rect,
    config: &AppConfig,
    state: &mut PlaylistImportModalState,
    sync_mgr: &PlaylistSyncManager,
) -> Option<PlaylistImportAction> {
    if !state.open {
        return None;
    }
    let is_de = config.language == Language::De;
    let mut action = None;

    let scrim_resp = ui.interact(screen, ui.id().with("import_modal_scrim"), Sense::click());
    ui.painter().rect_filled(
        screen,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(0, 0, 0, 180),
    );

    let card_w = 530.0f32.min(screen.width() - 40.0);
    let card_h = 430.0f32.min(screen.height() - 40.0);
    let card_rect = Rect::from_center_size(screen.center(), Vec2::new(card_w, card_h));

    if scrim_resp.clicked()
        && !card_rect.contains(scrim_resp.interact_pointer_pos().unwrap_or(Pos2::ZERO))
    {
        state.open = false;
        return None;
    }

    draw_liquid_glass_panel(ui.painter(), card_rect, 16, GlassLevel::Modal);

    let inner = card_rect.shrink2(Vec2::new(26.0, 22.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    // Header
    child.horizontal(|ui| {
        ui.label(
            egui::RichText::new(if is_de {
                "Playlist erstellen & importieren"
            } else {
                "Create & Import Playlist"
            })
            .font(font_semibold(17.5))
            .color(TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if glass_icon_button(ui, Icon::Close, 28.0, 11.5, false, false).clicked() {
                state.open = false;
            }
        });
    });

    child.add_space(12.0);

    // Segmented 3-Tab Switcher: [ Online-Link | Ordner vom PC | Neu erstellen ]
    let tabs = [
        (ImportTab::OnlineLink, "Spotify / Amazon / YT"),
        (
            ImportTab::PcFolder,
            if is_de { "Ordner vom PC" } else { "PC Folder" },
        ),
        (
            ImportTab::EmptyLocal,
            if is_de { "Neue Playlist" } else { "New Playlist" },
        ),
    ];
    let tab_bar_h = 34.0;
    let (tab_outer, _) =
        child.allocate_exact_size(Vec2::new(inner.width(), tab_bar_h), Sense::hover());
    child
        .painter()
        .rect_filled(tab_outer, CornerRadius::same(9), GLASS_PILL_IDLE);
    child.painter().rect_stroke(
        tab_outer,
        CornerRadius::same(9),
        Stroke::new(1.0_f32, RIM_SUBTLE),
        StrokeKind::Inside,
    );
    let seg_w = (tab_outer.width() - 6.0) / 3.0;
    for (i, (tab, lbl)) in tabs.iter().enumerate() {
        let seg_r = Rect::from_min_size(
            Pos2::new(tab_outer.left() + 3.0 + i as f32 * seg_w, tab_outer.top() + 3.0),
            Vec2::new(seg_w, tab_bar_h - 6.0),
        );
        let resp = child
            .interact(seg_r, child.id().with(("imp_tab", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = state.tab == *tab;
        if active {
            child
                .painter()
                .rect_filled(seg_r, CornerRadius::same(6), FROST_CTA_IDLE);
        } else if resp.hovered() {
            child
                .painter()
                .rect_filled(seg_r, CornerRadius::same(6), rgba(255, 255, 255, 14));
        }
        child.painter().text(
            seg_r.center(),
            Align2::CENTER_CENTER,
            *lbl,
            font_semibold(11.5),
            if active {
                FROST_CTA_TEXT
            } else {
                TEXT_SECONDARY
            },
        );
        if resp.clicked() && !sync_mgr.active {
            state.tab = *tab;
            state.error = None;
        }
    }

    child.add_space(16.0);

    let ytdlp_ok = resolve_ytdlp(config.ytdlp_path.as_ref()).is_some();

    match state.tab {
        ImportTab::OnlineLink => {
            child.label(
                egui::RichText::new(if is_de {
                    "ÖFFENTLICHER PLAYLIST-LINK (SPOTIFY, AMAZON MUSIC ODER YOUTUBE)"
                } else {
                    "PUBLIC PLAYLIST LINK (SPOTIFY, AMAZON MUSIC OR YOUTUBE)"
                })
                .font(font_semibold(10.0))
                .color(TEXT_MUTED),
            );
            child.add_space(6.0);

            let (box_r, _) =
                child.allocate_exact_size(Vec2::new(inner.width(), 36.0), Sense::hover());
            child
                .painter()
                .rect_filled(box_r, CornerRadius::same(8), GLASS_PILL_IDLE);
            child.painter().rect_stroke(
                box_r,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_BRIGHT),
                StrokeKind::Inside,
            );
            let mut inp_ui = child
                .new_child(egui::UiBuilder::new().max_rect(box_r.shrink2(Vec2::new(10.0, 7.0))));
            inp_ui.add_enabled(
                !sync_mgr.active,
                egui::TextEdit::singleline(&mut state.url)
                    .hint_text(
                        egui::RichText::new("https://open.spotify.com/playlist/...  |  music.amazon.de/playlists/...")
                            .color(TEXT_MUTED),
                    )
                    .frame(false)
                    .desired_width(f32::INFINITY)
                    .font(font_regular(12.0)),
            );

            child.add_space(8.0);

            // Live platform detection badge
            let detected = detect_source_kind(state.url.trim());
            let badge_text = match detected {
                Some(PlaylistSourceKind::Spotify) => {
                    if is_de {
                        "● Spotify-Playlist erkannt — Songs werden automatisch über YouTube geladen"
                    } else {
                        "● Spotify playlist detected — tracks will be downloaded via YouTube"
                    }
                }
                Some(PlaylistSourceKind::AmazonMusic) => {
                    if is_de {
                        "● Amazon Music-Playlist erkannt — Songs werden automatisch über YouTube geladen"
                    } else {
                        "● Amazon Music playlist detected — tracks will be downloaded via YouTube"
                    }
                }
                Some(PlaylistSourceKind::YouTube) => {
                    if is_de {
                        "● YouTube-Playlist erkannt — alle Titel werden als MP3 geladen"
                    } else {
                        "● YouTube playlist detected — all tracks will be downloaded as MP3"
                    }
                }
                None => {
                    if is_de {
                        "Unterstützt öffentliche Spotify-, Amazon Music- und YouTube-Playlist-Links."
                    } else {
                        "Supports public Spotify, Amazon Music, and YouTube playlist links."
                    }
                }
            };
            child.label(
                egui::RichText::new(badge_text)
                    .font(font_regular(11.5))
                    .color(if detected.is_some() {
                        TEXT_PRIMARY
                    } else {
                        TEXT_SECONDARY
                    }),
            );

            child.add_space(12.0);

            // Optional destination toggle when opened from inside an existing playlist
            if let Some((ref pl_name, _)) = state.target_playlist {
                child.horizontal(|ui| {
                    let lbl_cur = if is_de {
                        format!("In \"{pl_name}\" importieren")
                    } else {
                        format!("Import into \"{pl_name}\"")
                    };
                    let lbl_new = if is_de {
                        "Als neue Playlist anlegen"
                    } else {
                        "Create as new playlist"
                    };
                    if glass_pill_button(ui, None, &lbl_cur, 28.0, state.import_into_current)
                        .clicked()
                    {
                        state.import_into_current = true;
                    }
                    ui.add_space(6.0);
                    if glass_pill_button(ui, None, lbl_new, 28.0, !state.import_into_current)
                        .clicked()
                    {
                        state.import_into_current = false;
                    }
                });
                child.add_space(10.0);
            }

            if !state.import_into_current || state.target_playlist.is_none() {
                child.label(
                    egui::RichText::new(if is_de {
                        "OPTIONALER PLAYLIST-NAME (LEER = ORIGINALNAME)"
                    } else {
                        "OPTIONAL PLAYLIST NAME (EMPTY = ORIGINAL NAME)"
                    })
                    .font(font_semibold(10.0))
                    .color(TEXT_MUTED),
                );
                child.add_space(5.0);
                let (n_r, _) =
                    child.allocate_exact_size(Vec2::new(inner.width(), 34.0), Sense::hover());
                child
                    .painter()
                    .rect_filled(n_r, CornerRadius::same(8), GLASS_PILL_IDLE);
                child.painter().rect_stroke(
                    n_r,
                    CornerRadius::same(8),
                    Stroke::new(1.0_f32, RIM_SUBTLE),
                    StrokeKind::Inside,
                );
                let mut n_ui = child
                    .new_child(egui::UiBuilder::new().max_rect(n_r.shrink2(Vec2::new(10.0, 6.0))));
                n_ui.add_enabled(
                    !sync_mgr.active,
                    egui::TextEdit::singleline(&mut state.custom_name)
                        .hint_text(
                            egui::RichText::new(if is_de {
                                "Automatisch übernehmen..."
                            } else {
                                "Use original playlist title..."
                            })
                            .color(TEXT_MUTED),
                        )
                        .frame(false)
                        .desired_width(f32::INFINITY)
                        .font(font_regular(12.5)),
                );
            }

            if !ytdlp_ok {
                child.add_space(8.0);
                child.label(
                    egui::RichText::new(if is_de {
                        "Hinweis: yt-dlp wurde nicht gefunden und wird für den Song-Download benötigt."
                    } else {
                        "Note: yt-dlp was not found and is required for downloading songs."
                    })
                    .font(font_regular(11.5))
                    .color(Color32::from_rgb(235, 115, 115)),
                );
            }
        }
        ImportTab::PcFolder => {
            let box_r = Rect::from_min_size(child.cursor().min, Vec2::new(inner.width(), 165.0));
            child.allocate_exact_size(box_r.size(), Sense::hover());
            child
                .painter()
                .rect_filled(box_r, CornerRadius::same(10), rgba(255, 255, 255, 6));
            child.painter().rect_stroke(
                box_r,
                CornerRadius::same(10),
                Stroke::new(1.0_f32, RIM_SUBTLE),
                StrokeKind::Inside,
            );
            draw_icon(
                child.painter(),
                Pos2::new(box_r.center().x, box_r.top() + 38.0),
                26.0,
                Icon::Folder,
                TEXT_PRIMARY,
            );
            child.painter().text(
                Pos2::new(box_r.center().x, box_r.top() + 74.0),
                Align2::CENTER_CENTER,
                if is_de {
                    "Lokalen Musikordner vom PC als Playlist importieren"
                } else {
                    "Import a local music folder from your PC as a playlist"
                },
                font_semibold(13.5),
                TEXT_PRIMARY,
            );
            child.painter().text(
                Pos2::new(box_r.center().x, box_r.top() + 96.0),
                Align2::CENTER_CENTER,
                if is_de {
                    "Alle MP3-, WAV-, FLAC-, OGG- und M4A-Dateien werden sauber kopiert."
                } else {
                    "All MP3, WAV, FLAC, OGG, and M4A files will be cleanly copied."
                },
                font_regular(11.5),
                TEXT_SECONDARY,
            );

            let btn_area = Rect::from_center_size(
                Pos2::new(box_r.center().x, box_r.bottom() - 32.0),
                Vec2::new(220.0, 36.0),
            );
            let mut b_ui = child.new_child(
                egui::UiBuilder::new()
                    .max_rect(btn_area)
                    .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
            );
            if frosted_cta_button(
                &mut b_ui,
                Some(Icon::Folder),
                if is_de {
                    "Ordner auswählen"
                } else {
                    "Choose Folder"
                },
                36.0,
            )
            .clicked()
            {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    action = Some(PlaylistImportAction::ImportPcFolder(folder));
                    state.open = false;
                }
            }
        }
        ImportTab::EmptyLocal => {
            child.label(
                egui::RichText::new(if is_de {
                    "NAME DER NEUEN PLAYLIST"
                } else {
                    "NEW PLAYLIST NAME"
                })
                .font(font_semibold(10.0))
                .color(TEXT_MUTED),
            );
            child.add_space(6.0);
            let (box_r, _) =
                child.allocate_exact_size(Vec2::new(inner.width(), 36.0), Sense::hover());
            child
                .painter()
                .rect_filled(box_r, CornerRadius::same(8), GLASS_PILL_IDLE);
            child.painter().rect_stroke(
                box_r,
                CornerRadius::same(8),
                Stroke::new(1.0_f32, RIM_BRIGHT),
                StrokeKind::Inside,
            );
            let mut inp_ui = child
                .new_child(egui::UiBuilder::new().max_rect(box_r.shrink2(Vec2::new(10.0, 7.0))));
            let te = inp_ui.add(
                egui::TextEdit::singleline(&mut state.new_playlist_name)
                    .hint_text(
                        egui::RichText::new(if is_de {
                            "z. B. Gym, Chill, Autofahrt..."
                        } else {
                            "e.g. Gym, Chill, Roadtrip..."
                        })
                        .color(TEXT_MUTED),
                    )
                    .frame(false)
                    .desired_width(f32::INFINITY)
                    .font(font_regular(13.0)),
            );
            if te.lost_focus()
                && child.input(|i| i.key_pressed(egui::Key::Enter))
                && !state.new_playlist_name.trim().is_empty()
            {
                action = Some(PlaylistImportAction::CreateEmptyPlaylist(
                    state.new_playlist_name.trim().to_string(),
                ));
                state.new_playlist_name.clear();
                state.open = false;
            }
        }
    }

    // Dedicated Live Sync Progress / Error Zone above bottom buttons
    let status_zone = Rect::from_min_max(
        Pos2::new(inner.left(), inner.bottom() - 88.0),
        Pos2::new(inner.right(), inner.bottom() - 46.0),
    );
    if sync_mgr.active {
        let p = ui.painter();
        let (title_txt, frac) = if let Some(ref prog) = sync_mgr.progress {
            let f = if prog.total > 0 {
                prog.completed as f32 / prog.total as f32
            } else {
                0.08
            };
            (
                format!(
                    "{} ({}/{}) — {}",
                    prog.playlist_name, prog.completed, prog.total, prog.current_track
                ),
                f.clamp(0.04, 1.0),
            )
        } else {
            (sync_mgr.status_text.clone(), 0.12)
        };
        draw_truncated_text(
            p,
            Rect::from_min_max(
                status_zone.min,
                Pos2::new(status_zone.right(), status_zone.top() + 20.0),
            ),
            &title_txt,
            font_semibold(11.5),
            TEXT_PRIMARY,
            Align2::LEFT_CENTER,
        );
        let bar_r = Rect::from_min_max(
            Pos2::new(status_zone.left(), status_zone.bottom() - 12.0),
            Pos2::new(status_zone.right(), status_zone.bottom() - 6.0),
        );
        p.rect_filled(bar_r, CornerRadius::same(3), rgba(255, 255, 255, 22));
        p.rect_filled(
            Rect::from_min_size(bar_r.min, Vec2::new(bar_r.width() * frac, bar_r.height())),
            CornerRadius::same(3),
            Color32::WHITE,
        );
    } else if let Some(err) = state.error.as_deref().or(sync_mgr.last_error.as_deref()) {
        let mut err_ui = ui.new_child(egui::UiBuilder::new().max_rect(status_zone));
        err_ui.add(
            egui::Label::new(
                egui::RichText::new(err)
                    .font(font_regular(11.0))
                    .color(Color32::from_rgb(235, 115, 115)),
            )
            .wrap(),
        );
    }

    // Bottom Action Bar
    let footer = Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - 36.0), inner.max);
    let mut f_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(footer)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );

    if sync_mgr.active {
        if glass_pill_button(
            &mut f_ui,
            None,
            if is_de { "Im Hintergrund" } else { "Run in Background" },
            34.0,
            false,
        )
        .clicked()
        {
            state.open = false;
        }
        f_ui.add_space(8.0);
        if glass_pill_button(
            &mut f_ui,
            None,
            if is_de { "Stoppen" } else { "Stop" },
            34.0,
            false,
        )
        .clicked()
        {
            action = Some(PlaylistImportAction::CancelSync);
        }
    } else {
        match state.tab {
            ImportTab::OnlineLink => {
                let can_import = detect_source_kind(state.url.trim()).is_some() && ytdlp_ok;
                f_ui.add_enabled_ui(can_import, |ui| {
                    if frosted_cta_button(
                        ui,
                        Some(Icon::Link),
                        if is_de {
                            "Playlist importieren"
                        } else {
                            "Import Playlist"
                        },
                        34.0,
                    )
                    .clicked()
                    {
                        let url = state.url.trim().to_string();
                        if state.import_into_current {
                            if let Some((ref name, ref dir)) = state.target_playlist {
                                action = Some(PlaylistImportAction::ImportOnlineIntoExisting {
                                    url,
                                    playlist_name: name.clone(),
                                    playlist_dir: dir.clone(),
                                });
                            }
                        } else {
                            let cname = if state.custom_name.trim().is_empty() {
                                None
                            } else {
                                Some(state.custom_name.trim().to_string())
                            };
                            action = Some(PlaylistImportAction::ImportOnlineNew {
                                url,
                                custom_name: cname,
                            });
                        }
                    }
                });
                f_ui.add_space(10.0);
            }
            ImportTab::EmptyLocal => {
                let can_create = !state.new_playlist_name.trim().is_empty();
                f_ui.add_enabled_ui(can_create, |ui| {
                    if frosted_cta_button(
                        ui,
                        None,
                        if is_de { "Erstellen" } else { "Create" },
                        34.0,
                    )
                    .clicked()
                    {
                        action = Some(PlaylistImportAction::CreateEmptyPlaylist(
                            state.new_playlist_name.trim().to_string(),
                        ));
                        state.new_playlist_name.clear();
                        state.open = false;
                    }
                });
                f_ui.add_space(10.0);
            }
            ImportTab::PcFolder => {}
        }

        if glass_pill_button(
            &mut f_ui,
            None,
            if is_de { "Schließen" } else { "Close" },
            34.0,
            false,
        )
        .clicked()
        {
            state.open = false;
        }
    }

    action
}
