//! Installed apps, read from their .desktop entries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct App {
    /// desktop id without ".desktop", e.g. "firefox"
    pub id: String,
    pub name: String,
    pub icon: String,
    pub exec: String,
    /// what the window reports as its app id, when it differs from `id`
    pub wm_class: Option<String>,
}

impl App {
    /// Does a window with this app id belong to this app?
    pub fn owns(&self, app_id: &str) -> bool {
        let a = app_id.to_ascii_lowercase();
        a == self.id.to_ascii_lowercase()
            || self.wm_class.as_deref().is_some_and(|w| w.to_ascii_lowercase() == a)
            || self.exec.split_whitespace().next().is_some_and(|e| {
                Path::new(e).file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == a)
            })
    }
}

/// Apps on Home when the owner has not chosen their own list.
pub const DEFAULT_PINNED: &[&str] = &["librewolf", "chlorum-library", "com.system76.CosmicTerm", "com.system76.CosmicEdit"];
/// Apps opened side by side when a session starts, unless the owner chose others.
pub const DEFAULT_STARTUP: &[&str] = &["librewolf", "com.system76.CosmicTerm"];

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
}

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into()).join("chlorum")
}

/// One id per line in ~/.config/chlorum/<name>, or the default list.
pub fn id_list(name: &str, default: &[&str]) -> Vec<String> {
    match std::fs::read_to_string(config_dir().join("chlorum").join(name)) {
        Ok(s) => s
            .lines()
            .map(|l| l.trim().trim_end_matches(".desktop").to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect(),
        Err(_) => default.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn installed() -> HashMap<String, App> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(h).join("applications"));
    } else if let Ok(h) = std::env::var("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/applications"));
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/run/current-system/sw/share:/usr/share".into());
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("applications")));

    let mut out = HashMap::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            let Some(id) = file.strip_suffix(".desktop") else { continue };
            if out.contains_key(id) {
                continue;
            }
            if let Some(app) = parse(id, &e.path()) {
                out.insert(id.to_string(), app);
            }
        }
    }
    out
}

fn parse(id: &str, path: &Path) -> Option<App> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut name, mut icon, mut exec, mut wm) = (None, None, None, None);
    let mut hidden = false;
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("Icon=") {
            icon.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
            wm.get_or_insert(v.to_string());
        } else if line == "NoDisplay=true" || line == "Hidden=true" || line == "Terminal=true" {
            hidden = true;
        } else if let Some(v) = line.strip_prefix("Type=") {
            if v != "Application" {
                hidden = true;
            }
        }
    }
    let exec = exec?
        .split_whitespace()
        .filter(|a| !(a.len() == 2 && a.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ");
    if hidden && id != "chlorum-library" {
        return None;
    }
    Some(App {
        id: id.to_string(),
        name: name?,
        icon: icon.unwrap_or_else(|| "application-x-executable".into()),
        exec,
        wm_class: wm,
    })
}

