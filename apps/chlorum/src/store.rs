//! Get apps (Center → Get apps): a chosen set of apps, installed for this
//! profile with `nix profile` from the same package set the system is
//! built from (the `nixpkgs` the system pins), so they are ready-built and
//! fit the system. No administrator password, nothing changes for other
//! profiles, and each app still asks for what it may use (sandbox.rs).
//!
//! Which desktop entries an install added is kept in
//! ~/.config/chlorum/store-apps ("attr id id…"), so Chlorum knows what the
//! app asks for when it first starts.

use crate::sandbox::Perm;
use cosmic::iced::futures::channel::mpsc::Sender;
use cosmic::iced::futures::{SinkExt, Stream};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

pub struct Entry {
    /// nixpkgs attribute
    pub attr: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub about: &'static str,
    /// not free software: installed only when asked for (it says so)
    pub unfree: bool,
    /// what it asks for on its first start
    pub needs: &'static [Perm],
}

use Perm::{Camera as C, Files as F, Network as N, Sound as S};

pub const CATALOG: &[Entry] = &[
    Entry { attr: "telegram-desktop", name: "Telegram", kind: "Chat", about: "Messages, groups and calls", unfree: false, needs: &[N, S, C, F] },
    Entry { attr: "signal-desktop", name: "Signal", kind: "Chat", about: "Private messages and calls", unfree: false, needs: &[N, S, C] },
    Entry { attr: "vesktop", name: "Vesktop (Discord)", kind: "Chat", about: "Discord, with screen sharing that works on Wayland", unfree: false, needs: &[N, S, C] },
    Entry { attr: "discord", name: "Discord", kind: "Chat", about: "Voice and text chat (official app)", unfree: true, needs: &[N, S, C] },
    Entry { attr: "element-desktop", name: "Element", kind: "Chat", about: "Matrix chat", unfree: false, needs: &[N, S, C] },
    Entry { attr: "zoom-us", name: "Zoom", kind: "Chat", about: "Video meetings", unfree: true, needs: &[N, S, C] },
    Entry { attr: "thunderbird", name: "Thunderbird", kind: "Chat", about: "E-mail and calendar", unfree: false, needs: &[N] },
    Entry { attr: "obs-studio", name: "OBS Studio", kind: "Video", about: "Recording and live streaming", unfree: false, needs: &[N, S, C, F] },
    Entry { attr: "kdePackages.kdenlive", name: "Kdenlive", kind: "Video", about: "Video editor", unfree: false, needs: &[S, F] },
    Entry { attr: "shotcut", name: "Shotcut", kind: "Video", about: "Simple video editor", unfree: false, needs: &[S, F] },
    Entry { attr: "handbrake", name: "HandBrake", kind: "Video", about: "Convert videos", unfree: false, needs: &[F] },
    Entry { attr: "vlc", name: "VLC", kind: "Video", about: "Plays every video and stream", unfree: false, needs: &[N, S] },
    Entry { attr: "spotify", name: "Spotify", kind: "Music", about: "Music streaming", unfree: true, needs: &[N, S] },
    Entry { attr: "audacity", name: "Audacity", kind: "Music", about: "Record and edit sound", unfree: false, needs: &[S, F] },
    Entry { attr: "musescore", name: "MuseScore", kind: "Music", about: "Write sheet music", unfree: false, needs: &[S, F] },
    Entry { attr: "ardour", name: "Ardour", kind: "Music", about: "Studio for recording music", unfree: false, needs: &[S, F] },
    Entry { attr: "libreoffice-fresh", name: "LibreOffice", kind: "Office", about: "Documents, spreadsheets, slides (Word, Excel, PowerPoint files)", unfree: false, needs: &[F] },
    Entry { attr: "onlyoffice-desktopeditors", name: "ONLYOFFICE", kind: "Office", about: "Office suite close to Microsoft Office", unfree: false, needs: &[F] },
    Entry { attr: "obsidian", name: "Obsidian", kind: "Office", about: "Notes in a folder of Markdown files", unfree: true, needs: &[N, F] },
    Entry { attr: "xournalpp", name: "Xournal++", kind: "Office", about: "Handwritten notes, write on PDFs", unfree: false, needs: &[F] },
    Entry { attr: "anki", name: "Anki", kind: "Office", about: "Flash cards for learning", unfree: false, needs: &[N, S] },
    Entry { attr: "calibre", name: "Calibre", kind: "Office", about: "E-book library", unfree: false, needs: &[F] },
    Entry { attr: "foliate", name: "Foliate", kind: "Office", about: "Read e-books", unfree: false, needs: &[F] },
    Entry { attr: "gimp", name: "GIMP", kind: "Images", about: "Edit photos and images", unfree: false, needs: &[F] },
    Entry { attr: "krita", name: "Krita", kind: "Images", about: "Painting and drawing", unfree: false, needs: &[F] },
    Entry { attr: "inkscape", name: "Inkscape", kind: "Images", about: "Vector drawing", unfree: false, needs: &[F] },
    Entry { attr: "darktable", name: "darktable", kind: "Images", about: "Develop camera RAW photos", unfree: false, needs: &[F] },
    Entry { attr: "loupe", name: "Image Viewer", kind: "Images", about: "Look at photos", unfree: false, needs: &[] },
    Entry { attr: "blender", name: "Blender", kind: "Images", about: "3D modelling and animation", unfree: false, needs: &[S, F] },
    Entry { attr: "vscodium", name: "VSCodium", kind: "Code", about: "VS Code without Microsoft's tracking", unfree: false, needs: &[N, F] },
    Entry { attr: "vscode", name: "Visual Studio Code", kind: "Code", about: "Microsoft's code editor", unfree: true, needs: &[N, F] },
    Entry { attr: "zed-editor", name: "Zed", kind: "Code", about: "Fast code editor", unfree: false, needs: &[N, F] },
    Entry { attr: "godot", name: "Godot", kind: "Code", about: "Make games", unfree: false, needs: &[S, F] },
    Entry { attr: "steam", name: "Steam", kind: "Games", about: "Games store and library", unfree: true, needs: &[N, S, C, F] },
    Entry { attr: "heroic", name: "Heroic", kind: "Games", about: "Epic, GOG and Amazon games", unfree: false, needs: &[N, S, F] },
    Entry { attr: "prismlauncher", name: "Prism Launcher", kind: "Games", about: "Minecraft", unfree: false, needs: &[N, S] },
    Entry { attr: "lutris", name: "Lutris", kind: "Games", about: "Run games from everywhere", unfree: false, needs: &[N, S, F] },
    Entry { attr: "keepassxc", name: "KeePassXC", kind: "Tools", about: "Password manager (offline)", unfree: false, needs: &[F] },
    Entry { attr: "bitwarden-desktop", name: "Bitwarden", kind: "Tools", about: "Password manager (in the cloud)", unfree: false, needs: &[N] },
    Entry { attr: "qbittorrent", name: "qBittorrent", kind: "Tools", about: "Download torrents", unfree: false, needs: &[N] },
    Entry { attr: "chromium", name: "Chromium", kind: "Tools", about: "Second browser (and for keyboard set-up sites like VIA)", unfree: false, needs: &[N, S, C] },
    Entry { attr: "qalculate-gtk", name: "Qalculate!", kind: "Tools", about: "Calculator that understands units", unfree: false, needs: &[] },
];

pub fn entry(attr: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.attr == attr)
}

/// The catalog in the order it is shown: by kind, then name.
pub fn listed() -> Vec<&'static Entry> {
    const ORDER: &[&str] = &["Chat", "Office", "Video", "Music", "Images", "Code", "Games", "Tools"];
    let mut v: Vec<&Entry> = CATALOG.iter().collect();
    v.sort_by_key(|e| (ORDER.iter().position(|k| *k == e.kind).unwrap_or(99), e.name.to_lowercase()));
    v
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// Where this profile's installed apps put their desktop entries.
pub fn profile_dirs() -> Vec<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    vec![home().join(".nix-profile/share/applications"), state.join("nix/profile/share/applications")]
}

fn desktop_ids() -> BTreeSet<String> {
    profile_dirs()
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".desktop").map(String::from))
        .collect()
}

fn record_path() -> PathBuf {
    crate::apps::config_dir().join("chlorum/store-apps")
}

/// attr → desktop ids it added
fn records() -> HashMap<String, Vec<String>> {
    std::fs::read_to_string(record_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((w.next()?.to_string(), w.map(String::from).collect()))
        })
        .collect()
}

fn save_records(r: &HashMap<String, Vec<String>>) {
    let mut lines: Vec<String> = r.iter().map(|(a, ids)| format!("{a} {}", ids.join(" "))).collect();
    lines.sort();
    let p = record_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, lines.join("\n") + "\n");
}

/// What an app from the catalog asks for, by its desktop id.
pub fn needs_of(desktop_id: &str) -> Option<Vec<Perm>> {
    let r = records();
    let attr = r.iter().find(|(_, ids)| ids.iter().any(|i| i == desktop_id)).map(|(a, _)| a.clone())?;
    entry(&attr).map(|e| e.needs.to_vec())
}

fn nix() -> String {
    ["/run/current-system/sw/bin/nix"].iter().find(|p| std::path::Path::new(p).exists()).map(|p| p.to_string()).unwrap_or_else(|| "nix".into())
}

/// The catalog apps installed for this profile (by attr).
pub async fn installed() -> BTreeSet<String> {
    let out = tokio::process::Command::new(nix()).args(["profile", "list", "--json"]).output().await;
    let Ok(out) = out else { return BTreeSet::new() };
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let mut set = BTreeSet::new();
    if let Some(els) = v.get("elements").and_then(|e| e.as_object()) {
        for el in els.values() {
            let path = el.get("attrPath").and_then(|a| a.as_str()).unwrap_or("");
            for e in CATALOG {
                if path.ends_with(&format!(".{}", e.attr)) {
                    set.insert(e.attr.to_string());
                }
            }
        }
    }
    set
}

/// The profile element of an installed app, for removing it.
async fn element(attr: &str) -> Option<String> {
    let out = tokio::process::Command::new(nix()).args(["profile", "list", "--json"]).output().await.ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let els = v.get("elements")?.as_object()?;
    els.iter().find(|(_, el)| el.get("attrPath").and_then(|a| a.as_str()).is_some_and(|p| p.ends_with(&format!(".{attr}")))).map(|(k, _)| k.clone())
}

#[derive(Clone, Debug)]
pub enum Event {
    /// done, expected (things to download and build)
    Progress(u64, u64),
    Done(Result<String, String>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Job {
    Install,
    Remove,
    /// every installed app to the version that fits the system now
    Update,
}

/// Runs one job, telling how far it is.
pub fn run(job: Job, attr: String) -> impl Stream<Item = Event> {
    cosmic::iced::stream::channel(16, async move |mut out: Sender<Event>| {
        let result = work(job, &attr, &mut out).await;
        let _ = out.send(Event::Done(result)).await;
    })
}

async fn work(job: Job, attr: &str, out: &mut Sender<Event>) -> Result<String, String> {
    use tokio::io::AsyncBufReadExt;
    let name = entry(attr).map(|e| e.name).unwrap_or(attr);
    let before = desktop_ids();
    let mut cmd = tokio::process::Command::new(nix());
    match job {
        Job::Install => {
            let e = entry(attr).ok_or("Unknown app.")?;
            cmd.args(["profile", "add", "--log-format", "internal-json"]);
            if e.unfree {
                // asked for by name: the owner chose this non-free app
                cmd.arg("--impure").env("NIXPKGS_ALLOW_UNFREE", "1");
            }
            cmd.arg(format!("nixpkgs#{attr}"));
        }
        Job::Remove => {
            let el = element(attr).await.ok_or(format!("{name} is not installed."))?;
            cmd.args(["profile", "remove", "--log-format", "internal-json", &el]);
        }
        Job::Update => {
            // non-free apps need the same permission to be rebuilt
            cmd.args(["profile", "upgrade", "--all", "--impure", "--log-format", "internal-json"]).env("NIXPKGS_ALLOW_UNFREE", "1");
        }
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start Nix: {e}"))?;
    let mut tally = crate::progress::Tally::default();
    let mut lines = tokio::io::BufReader::new(child.stderr.take().ok_or("no output")?).lines();
    let mut last = std::time::Instant::now();
    while let Ok(Some(l)) = lines.next_line().await {
        if tally.line(&l) && last.elapsed().as_millis() > 300 {
            let (d, e) = tally.totals();
            let _ = out.send(Event::Progress(d, e)).await;
            last = std::time::Instant::now();
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(if tally.error.is_empty() { format!("{name}: it did not work.") } else { tally.error.clone() });
    }
    // remember which desktop entries it brought
    let mut r = records();
    match job {
        Job::Install => {
            let added: Vec<String> = desktop_ids().difference(&before).cloned().collect();
            r.insert(attr.to_string(), added);
        }
        Job::Remove => {
            r.remove(attr);
        }
        Job::Update => {}
    }
    save_records(&r);
    Ok(match job {
        Job::Install => format!("{name} is installed. It is on Home's list of apps."),
        Job::Remove => format!("{name} is removed."),
        Job::Update => "Your apps are up to date.".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog() {
        let mut seen = BTreeSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.attr), "twice: {}", e.attr);
            assert!(!e.name.is_empty() && !e.about.is_empty());
        }
        assert_eq!(listed().first().unwrap().kind, "Chat");
        assert!(entry("steam").unwrap().unfree);
        assert!(!entry("obs-studio").unwrap().unfree);
    }
}
