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
    /// what it says it wants (X-Chlorum-Permissions=network;sound;)
    pub permissions: Option<String>,
    /// the kinds of files it opens (MimeType=)
    pub mime: Vec<String>,
}

impl App {
    /// The command line that starts this app with only what it may use
    /// (see sandbox.rs).
    pub fn launch(&self) -> String {
        format!("chlorum run {} {}", quote(&self.id), quote(&self.exec))
    }

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
pub const DEFAULT_PINNED: &[&str] = &["librewolf", "chlorum-library", "chlorum-typewriter", "com.system76.CosmicTerm"];
/// Apps never shown on Home or in the app list.
pub const HIDDEN: &[&str] = &[
    "nixos-manual",
    "com.system76.CosmicEdit",
    "chlorum-shell",
    // the parts of an app that only start it in another way
    "foot-server",
    "footclient",
    // players and viewers that only show a file they are given: Library
    // and Gallery open them with one
    "mpv",
    "umpv",
    "imv",
    "imv-dir",
    "org.pwmt.zathura",
    "display-im6.q16",
    "display-im7.q16",
    "texdoctk",
    "xterm",
    "nvim",
    "cups",
];

/// Apps that are one program started in different ways (LibreOffice's
/// Writer, Calc…) share a family: the program's file name.
pub fn family(a: &App) -> String {
    // the program itself: past "env VAR=x", not a shell or a launcher
    let words: Vec<&str> = a.exec.split_whitespace().collect();
    let mut prog = "";
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let base = Path::new(w).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if base == "env" || (w.contains('=') && !w.starts_with('/')) || w.starts_with('-') {
            i += 1;
            continue;
        }
        prog = w;
        break;
    }
    let base = Path::new(prog).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let base = base.trim_start_matches('.').trim_end_matches("-wrapped").to_string();
    match base.as_str() {
        "footclient" => "foot".into(),
        "soffice" | "libreoffice" | "loffice" | "lowriter" | "localc" | "loimpress" | "lodraw" | "lomath" | "lobase" => "libreoffice".into(),
        // a shell, a launcher or nothing says nothing: the app is its own
        "" | "sh" | "bash" | "true" | "flatpak" | "steam" | "wine" | "java" | "python3" | "python" | "chlorum" | "xdg-open" | "gtk-launch" => format!("id:{}", a.id),
        _ => base,
    }
}

/// Hidden by what it is called (ids differ between versions).
pub fn hidden_by_name(a: &App) -> bool {
    a.name.starts_with("ImageMagick")
}
/// Apps opened side by side when a session starts, unless the owner chose others.
pub const DEFAULT_STARTUP: &[&str] = &["librewolf", "com.system76.CosmicTerm"];

/// One shell word, whatever it contains.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

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
    scan(false)
}

/// Every app that can open files, also the ones that keep out of menus
/// (an image viewer's "open the folder" entry…); not terminal ones.
pub fn handlers() -> HashMap<String, App> {
    scan(true)
}

fn scan(with_hidden: bool) -> HashMap<String, App> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(h).join("applications"));
    } else if let Ok(h) = std::env::var("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/applications"));
    }
    // apps installed for this profile (Center → Get apps), whatever the
    // session's search path says
    dirs.extend(crate::store::profile_dirs());
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
            if let Some(app) = parse(id, &e.path(), with_hidden) {
                out.insert(id.to_string(), app);
            }
        }
    }
    out
}

fn parse(id: &str, path: &Path, with_hidden: bool) -> Option<App> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut name, mut icon, mut exec, mut wm, mut perms, mut mime) = (None, None, None, None, None, None);
    let mut hidden = false;
    let mut not_an_app = false;
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
        } else if let Some(v) = line.strip_prefix("X-Chlorum-Permissions=") {
            perms.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("StartupWMClass=") {
            wm.get_or_insert(v.to_string());
        } else if let Some(v) = line.strip_prefix("MimeType=") {
            mime.get_or_insert(v.split(';').filter(|m| !m.is_empty()).map(str::to_string).collect::<Vec<_>>());
        } else if line == "NoDisplay=true" {
            hidden = true;
        } else if line == "Hidden=true" || line == "Terminal=true" {
            not_an_app = true;
        } else if let Some(v) = line.strip_prefix("Type=") {
            if v != "Application" {
                not_an_app = true;
            }
        }
    }
    let exec = exec?
        .split_whitespace()
        .filter(|a| !(a.len() == 2 && a.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ");
    if not_an_app || (hidden && !with_hidden && id != "chlorum-library") {
        return None;
    }
    Some(App {
        id: id.to_string(),
        name: name?,
        icon: icon.unwrap_or_else(|| "application-x-executable".into()),
        exec,
        wm_class: wm,
        permissions: perms,
        mime: mime.unwrap_or_default(),
    })
}


#[cfg(test)]
mod launch_tests {
    use super::*;

    fn app(id: &str, exec: &str) -> App {
        App { id: id.into(), name: id.into(), icon: String::new(), exec: exec.into(), wm_class: None, permissions: None, mime: vec![] }
    }

    #[test]
    fn families() {
        assert_eq!(family(&app("writer", "libreoffice --writer")), "libreoffice");
        assert_eq!(family(&app("calc", "/nix/store/x-libreoffice/bin/libreoffice --calc")), "libreoffice");
        assert_eq!(family(&app("foot", "foot")), family(&app("footclient", "footclient")));
        assert_eq!(family(&app("a", "env GDK_BACKEND=x11 /nix/store/y/bin/gimp")), "gimp");
        assert_ne!(family(&app("lib", "chlorum library")), family(&app("tw", "chlorum typewriter")));
        assert_ne!(family(&app("x", "sh -c 'a'")), family(&app("y", "sh -c 'b'")));
    }

    #[test]
    fn quoting() {
        assert_eq!(super::quote("it's"), "'it'\\''s'");
        let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", super::quote("a 'b' \"c\" $d"))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a 'b' \"c\" $d");
    }
}
