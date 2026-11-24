use crossbeam_channel::{unbounded, Receiver, Sender};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const IS_PRIVATE_BUILD: bool = true;
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/Ilay-dev/Rustify/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateReason {
    NewerVersion,
    SizeChanged,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    #[allow(dead_code)]
    pub tag: String,
    pub download_url: String,
    #[allow(dead_code)]
    pub html_url: String,
    pub asset_size: u64,
    pub sha256: Option<String>,
    pub notes: String,
    pub reason: UpdateReason,
}

#[derive(Debug, Clone)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate {
        #[allow(dead_code)]
        latest_version: String,
        #[allow(dead_code)]
        asset_size: u64,
    },
    Available(UpdateInfo),
    Downloading {
        info: UpdateInfo,
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    ReadyToInstall,
    Error(String),
}

#[derive(Debug, Default)]
pub struct UpdatePollResult {
    pub open_startup_popup: bool,
    pub save_baseline_size: Option<u64>,
    pub should_exit_for_install: bool,
    pub error_toast: Option<String>,
}

enum UpdaterEvent {
    UpToDate {
        latest_version: String,
        asset_size: u64,
        baseline_size: Option<u64>,
    },
    Available {
        info: UpdateInfo,
        is_startup: bool,
    },
    CheckError {
        message: String,
        is_startup: bool,
    },
    DownloadProgress {
        downloaded_bytes: u64,
    },
    ReadyToInstall,
    DownloadError(String),
}

pub fn parse_semver(s: &str) -> (u32, u32, u32) {
    let clean = s
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    let mut parts = clean.split('.');
    let parse_part = |p: Option<&str>| -> u32 {
        p.and_then(|seg| {
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().ok()
        })
        .unwrap_or(0)
    };
    let major = parse_part(parts.next());
    let minor = parse_part(parts.next());
    let patch = parse_part(parts.next());
    (major, minor, patch)
}

pub fn detect_update(
    current_ver: &str,
    remote_tag: &str,
    remote_size: u64,
    last_known_size: u64,
) -> Option<UpdateReason> {
    let cur = parse_semver(current_ver);
    let rem = parse_semver(remote_tag);
    if rem > cur {
        Some(UpdateReason::NewerVersion)
    } else if rem == cur
        && remote_size > 0
        && last_known_size > 0
        && remote_size != last_known_size
    {
        Some(UpdateReason::SizeChanged)
    } else {
        None
    }
}

fn clamp_release_notes(raw: &str, max_chars: usize) -> String {
    let cleaned: String = raw
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let count = cleaned.chars().count();
    if count <= max_chars {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub struct UpdateManager {
    status: UpdateStatus,
    tx: Sender<UpdaterEvent>,
    rx: Receiver<UpdaterEvent>,
}

impl UpdateManager {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            status: UpdateStatus::Idle,
            tx,
            rx,
        }
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        )
    }

    pub fn check_now(&mut self, last_known_size: u64, is_startup: bool) {
        if self.is_busy() {
            return;
        }
        self.status = UpdateStatus::Checking;
        let tx = self.tx.clone();

        let _ = thread::Builder::new()
            .name("rustify-updater-check".to_string())
            .spawn(move || {
                match fetch_latest_release() {
                    Ok(rel) => {
                        let version = rel
                            .tag
                            .trim()
                            .trim_start_matches(['v', 'V'])
                            .to_string();

                        if let Some(reason) = detect_update(
                            CURRENT_VERSION,
                            &rel.tag,
                            rel.asset_size,
                            last_known_size,
                        ) {
                            if rel.download_url.is_empty() {
                                let _ = tx.send(UpdaterEvent::CheckError {
                                    message: "Installer-Download-URL fehlt im Release.".to_string(),
                                    is_startup,
                                });
                            } else {
                                let info = UpdateInfo {
                                    version,
                                    tag: rel.tag,
                                    download_url: rel.download_url,
                                    html_url: rel.html_url,
                                    asset_size: rel.asset_size,
                                    sha256: rel.sha256,
                                    notes: rel.notes,
                                    reason,
                                };
                                let _ = tx.send(UpdaterEvent::Available { info, is_startup });
                            }
                        } else {
                            let baseline_size = if last_known_size == 0 && rel.asset_size > 0 {
                                Some(rel.asset_size)
                            } else {
                                None
                            };
                            let _ = tx.send(UpdaterEvent::UpToDate {
                                latest_version: version,
                                asset_size: rel.asset_size,
                                baseline_size,
                            });
                        }
                    }
                    Err(message) => {
                        let _ = tx.send(UpdaterEvent::CheckError {
                            message,
                            is_startup,
                        });
                    }
                }
            });
    }

    pub fn start_download_and_install(&mut self, info: UpdateInfo) {
        if IS_PRIVATE_BUILD {
            let _ = self.tx.send(UpdaterEvent::DownloadError(
                "Private Edition ist geschützt — das öffentliche Clean-Update wird nicht installiert."
                    .to_string(),
            ));
            return;
        }
        if matches!(self.status, UpdateStatus::Downloading { .. }) {
            return;
        }
        let total_bytes = info.asset_size;
        self.status = UpdateStatus::Downloading {
            info: info.clone(),
            downloaded_bytes: 0,
            total_bytes,
        };
        let tx = self.tx.clone();

        let _ = thread::Builder::new()
            .name("rustify-updater-download".to_string())
            .spawn(move || {
                if let Err(err) = download_and_launch_installer(&info, &tx) {
                    let _ = tx.send(UpdaterEvent::DownloadError(err));
                }
            });
    }

    pub fn poll(&mut self) -> UpdatePollResult {
        let mut res = UpdatePollResult::default();
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                UpdaterEvent::UpToDate {
                    latest_version,
                    asset_size,
                    baseline_size,
                } => {
                    self.status = UpdateStatus::UpToDate {
                        latest_version,
                        asset_size,
                    };
                    if baseline_size.is_some() {
                        res.save_baseline_size = baseline_size;
                    }
                }
                UpdaterEvent::Available { info, is_startup } => {
                    self.status = UpdateStatus::Available(info);
                    if is_startup {
                        res.open_startup_popup = true;
                    }
                }
                UpdaterEvent::CheckError {
                    message,
                    is_startup,
                } => {
                    self.status = UpdateStatus::Error(message.clone());
                    if !is_startup {
                        res.error_toast = Some(message);
                    }
                }
                UpdaterEvent::DownloadProgress { downloaded_bytes } => {
                    if let UpdateStatus::Downloading {
                        downloaded_bytes: ref mut dl,
                        ..
                    } = self.status
                    {
                        *dl = downloaded_bytes;
                    }
                }
                UpdaterEvent::ReadyToInstall => {
                    self.status = UpdateStatus::ReadyToInstall;
                    res.should_exit_for_install = true;
                }
                UpdaterEvent::DownloadError(msg) => {
                    self.status = UpdateStatus::Error(msg.clone());
                    res.error_toast = Some(msg);
                }
            }
        }
        res
    }
}

fn curl_bin() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "curl.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "curl"
    }
}

struct ReleaseMetadata {
    tag: String,
    html_url: String,
    notes: String,
    download_url: String,
    asset_size: u64,
    sha256: Option<String>,
}

fn extract_sha256_from_body(body: &str) -> Option<String> {
    for token in body.split(|c: char| !c.is_ascii_hexdigit()) {
        if token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(token.to_ascii_lowercase());
        }
    }
    None
}

fn fetch_latest_release() -> Result<ReleaseMetadata, String> {
    let api_url = crate::config::validate_http_url(LATEST_RELEASE_API)?;
    let mut cmd = Command::new(curl_bin());
    cmd.args([
        "-sSL",
        "--max-time",
        "8",
        "-H",
        "User-Agent: Rustify-Updater",
        "-H",
        "Accept: application/vnd.github+json",
        "--",
        api_url,
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .map_err(|e| format!("Update-Check fehlgeschlagen: {e}"))?;
    if !output.status.success() {
        return Err("Update-Server nicht erreichbar.".to_string());
    }

    let val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Ungültige Antwort vom Update-Server.".to_string())?;

    let tag = val
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Kein Release gefunden.".to_string())?;

    let html_url = val
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/Ilay-dev/Rustify/releases/latest")
        .to_string();

    let raw_body = val.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let notes = clamp_release_notes(raw_body, 320);

    let mut download_url = String::new();
    let mut asset_size: u64 = 0;
    let mut sha256: Option<String> = None;

    if let Some(assets) = val.get("assets").and_then(|v| v.as_array()) {
        // Prefer exact "Rustify-Setup.exe", fallback to any ".exe"
        let chosen = assets
            .iter()
            .find(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.eq_ignore_ascii_case("Rustify-Setup.exe"))
                    .unwrap_or(false)
            })
            .or_else(|| {
                assets.iter().find(|a| {
                    a.get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| n.to_ascii_lowercase().ends_with(".exe"))
                        .unwrap_or(false)
                })
            });

        if let Some(asset) = chosen {
            download_url = asset
                .get("browser_download_url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            asset_size = asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            sha256 = asset
                .get("digest")
                .and_then(|d| d.as_str())
                .and_then(|s| s.trim().strip_prefix("sha256:"))
                .map(|hex| hex.trim().to_ascii_lowercase())
                .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
                .or_else(|| extract_sha256_from_body(raw_body));
        }
    }

    Ok(ReleaseMetadata {
        tag,
        html_url,
        notes,
        download_url,
        asset_size,
        sha256,
    })
}

fn validate_release_download_url(url: &str) -> Result<&str, String> {
    let safe_url = crate::config::validate_http_url(url)?;
    if !safe_url.starts_with("https://github.com/")
        && !safe_url.starts_with("https://objects.githubusercontent.com/")
        && !safe_url.starts_with("https://release-assets.githubusercontent.com/")
    {
        return Err("Ungültige Download-Domain für das Update.".to_string());
    }
    Ok(safe_url)
}

fn compute_file_sha256_hex(path: &std::path::Path) -> Result<String, String> {
    let mut f =
        File::open(path).map_err(|e| format!("Installer konnte nicht geprüft werden: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65_536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|_| "Installer-Datei konnte nicht gelesen werden.".to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn download_and_launch_installer(
    info: &UpdateInfo,
    tx: &Sender<UpdaterEvent>,
) -> Result<(), String> {
    let safe_url = validate_release_download_url(&info.download_url)?;
    let expected_sha256 = info
        .sha256
        .as_deref()
        .ok_or_else(|| "Fehlende SHA-256-Prüfsumme im Release.".to_string())?;

    let tmp_path = std::env::temp_dir().join("Rustify-Update-Setup.exe");
    let _ = fs::remove_file(&tmp_path);

    let tmp_str = tmp_path.to_string_lossy().to_string();
    let mut cmd = Command::new(curl_bin());
    cmd.args([
        "-fSL",
        "--max-time",
        "180",
        "-o",
        &tmp_str,
        "--",
        safe_url,
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Download konnte nicht gestartet werden: {e}"))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = fs::remove_file(&tmp_path);
                    return Err("Installer-Download fehlgeschlagen.".to_string());
                }
                break;
            }
            Ok(None) => {
                let dl = fs::metadata(&tmp_path).map(|m| m.len()).unwrap_or(0);
                let _ = tx.send(UpdaterEvent::DownloadProgress {
                    downloaded_bytes: dl,
                });
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(format!("Download-Fehler: {e}"));
            }
        }
    }

    // Validate downloaded file: exists, >= 500 KB, and starts with PE header "MZ"
    let meta = fs::metadata(&tmp_path)
        .map_err(|_| "Heruntergeladene Datei wurde nicht gefunden.".to_string())?;
    let file_len = meta.len();
    if file_len < 500_000 {
        let _ = fs::remove_file(&tmp_path);
        return Err("Heruntergeladener Installer ist unvollständig.".to_string());
    }
    let _ = tx.send(UpdaterEvent::DownloadProgress {
        downloaded_bytes: file_len,
    });

    let mut header = [0u8; 2];
    let mut f = File::open(&tmp_path)
        .map_err(|e| format!("Installer konnte nicht geprüft werden: {e}"))?;
    f.read_exact(&mut header)
        .map_err(|_| "Installer-Datei konnte nicht gelesen werden.".to_string())?;
    drop(f);

    if &header != b"MZ" {
        let _ = fs::remove_file(&tmp_path);
        return Err("Ungültige Windows-Installer-Datei.".to_string());
    }

    let actual_sha256 = match compute_file_sha256_hex(&tmp_path) {
        Ok(h) => h,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };
    if actual_sha256 != expected_sha256 {
        let _ = fs::remove_file(&tmp_path);
        return Err("SHA-256-Prüfsumme des Installers stimmt nicht überein.".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        Command::new(&tmp_path)
            .spawn()
            .map_err(|e| format!("Installer konnte nicht gestartet werden: {e}"))?;
    }

    let _ = tx.send(UpdaterEvent::ReadyToInstall);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver() {
        assert_eq!(parse_semver("1.2.0"), (1, 2, 0));
        assert_eq!(parse_semver("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_semver("V2.10.4-beta.1"), (2, 10, 4));
        assert_eq!(parse_semver("  v1.5-rc1  "), (1, 5, 0));
        assert_eq!(parse_semver("3"), (3, 0, 0));
        assert_eq!(parse_semver("invalid"), (0, 0, 0));
    }

    #[test]
    fn test_detect_update_newer_version() {
        assert_eq!(
            detect_update("1.2.0", "v1.3.0", 14_000_000, 14_000_000),
            Some(UpdateReason::NewerVersion)
        );
        assert_eq!(
            detect_update("1.2.0", "v2.0.0", 10_000_000, 0),
            Some(UpdateReason::NewerVersion)
        );
        assert_eq!(
            detect_update("1.2.0", "1.2.1", 14_000_000, 14_000_000),
            Some(UpdateReason::NewerVersion)
        );
    }

    #[test]
    fn test_detect_update_equal_version_same_size() {
        assert_eq!(
            detect_update("1.2.0", "v1.2.0", 14_500_000, 14_500_000),
            None
        );
        // Baseline not yet known (0) should NOT trigger false-positive SizeChanged
        assert_eq!(detect_update("1.2.0", "v1.2.0", 14_500_000, 0), None);
    }

    #[test]
    fn test_detect_update_equal_version_size_changed() {
        // Larger remote installer
        assert_eq!(
            detect_update("1.2.0", "v1.2.0", 15_200_000, 14_500_000),
            Some(UpdateReason::SizeChanged)
        );
        // Smaller remote installer
        assert_eq!(
            detect_update("1.2.0", "v1.2.0", 13_800_000, 14_500_000),
            Some(UpdateReason::SizeChanged)
        );
    }

    #[test]
    fn test_detect_update_older_remote_version() {
        assert_eq!(
            detect_update("1.2.0", "v1.1.9", 15_000_000, 14_500_000),
            None
        );
        assert_eq!(
            detect_update("1.2.0", "v1.0.0", 12_000_000, 14_500_000),
            None
        );
    }
}
