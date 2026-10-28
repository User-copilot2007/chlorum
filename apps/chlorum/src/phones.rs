//! Phones and cameras plugged in by USB: Android phones (MTP), iPhones
//! (AFC) and cameras (PTP), through GVFS. Library shows them under Drives;
//! their photos and videos can be looked at and copied into Library.
//!
//! GVFS notices the phone; `gio mount` opens it (the phone has to be
//! unlocked, and an Android phone set to "File transfer" when it asks).
//! Its files are then under $XDG_RUNTIME_DIR/gvfs.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Phone {
    pub name: String,
    /// mtp://SAMSUNG_Galaxy_S23_R5CW…/
    pub uri: String,
    /// where its files are, once open
    pub path: Option<PathBuf>,
}

fn gvfs_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/run/user/1000".into()).join("gvfs")
}

/// The phones and cameras there are (`gio mount -li`).
pub fn list() -> Vec<Phone> {
    let Ok(out) = std::process::Command::new("gio").args(["mount", "-li"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else {
        return vec![];
    };
    parse(&String::from_utf8_lossy(&out.stdout), &mounted())
}

/// The open ones: their folder names under gvfs ("mtp:host=…").
fn mounted() -> Vec<String> {
    std::fs::read_dir(gvfs_dir()).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
}

fn parse(text: &str, open: &[String]) -> Vec<Phone> {
    let mut out: Vec<Phone> = vec![];
    let mut name = String::new();
    for line in text.lines() {
        let t = line.trim();
        // "Volume(0): Galaxy S23" (also listed under its Drive(…))
        if let Some(rest) = t.strip_prefix("Volume(").and_then(|r| r.split_once("): ")) {
            name = rest.1.trim().to_string();
        } else if let Some(rest) = t.strip_prefix("Mount(").and_then(|r| r.split_once("): ")) {
            name = rest.1.split(" -> ").next().unwrap_or("").trim().to_string();
        }
        let uri = t.strip_prefix("activation_root=").or_else(|| t.strip_prefix("default_location=")).unwrap_or("");
        let scheme = uri.split("://").next().unwrap_or("");
        if !matches!(scheme, "mtp" | "afc" | "gphoto2") || out.iter().any(|p| p.uri == uri) {
            continue;
        }
        let host = uri.split("://").nth(1).unwrap_or("").trim_end_matches('/').to_string();
        let path = open.iter().find(|d| d.starts_with(&format!("{scheme}:host=")) && host_matches(d, &host)).map(|d| gvfs_dir().join(d));
        let shown = if name.is_empty() { host.replace('_', " ") } else { name.clone() };
        out.push(Phone { name: shown, uri: uri.to_string(), path });
    }
    out
}

/// Whether a gvfs folder ("mtp:host=SAMSUNG_Galaxy") is this phone's.
fn host_matches(dir: &str, host: &str) -> bool {
    let h = dir.split("host=").nth(1).unwrap_or("");
    let unescape = |s: &str| s.replace("%5B", "[").replace("%5D", "]").replace("%2C", ",").replace("%3A", ":");
    unescape(h) == unescape(host) || h.starts_with(&host.replace([',', '[', ']', ':'], "")) || host.starts_with(h)
}

/// Opens the phone (it may ask on its screen first). Returns its folder.
pub fn open(p: &Phone) -> Result<PathBuf, String> {
    if let Some(path) = &p.path {
        return Ok(path.clone());
    }
    let out = std::process::Command::new("gio").args(["mount", &p.uri]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(if err.contains("locked") || err.contains("Unable to open MTP") || err.contains("No MTP") {
            "Unlock the phone, choose \"File transfer\" on it, then press → again.".into()
        } else if err.contains("not trusted") || err.contains("Trust") || err.contains("pair") {
            "On the iPhone, choose \"Trust\", then press → again.".into()
        } else {
            format!("The phone did not open: {}", err.trim().rsplit(": ").next().unwrap_or("").trim())
        });
    }
    list().into_iter().find(|x| x.uri == p.uri).and_then(|x| x.path).ok_or_else(|| "The phone opened, but its files did not show. Press → again.".into())
}

/// Where a phone keeps photos, videos and downloads (the rest is apps'
/// own data), below its storage ("Internal shared storage", "Card").
pub fn media_dirs(root: &Path) -> Vec<PathBuf> {
    const KEEP: &[&str] = &["DCIM", "Pictures", "Movies", "Download", "Downloads", "Music", "Documents", "Recordings", "Screenshots", "Camera"];
    let mut out = vec![];
    // an iPhone shows DCIM right away; an Android phone a storage first
    let storages: Vec<PathBuf> = if root.join("DCIM").is_dir() {
        vec![root.to_path_buf()]
    } else {
        std::fs::read_dir(root).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
    };
    for s in storages {
        for k in KEEP {
            let d = s.join(k);
            if d.is_dir() {
                out.push(d);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_from_gio() {
        let text = "Drive(0): SAMSUNG Android\n  Type: GProxyDrive (GProxyVolumeMonitorMTP)\n  Volume(0): Galaxy S23\n    Type: GProxyVolume (GProxyVolumeMonitorMTP)\n    activation_root=mtp://SAMSUNG_SAMSUNG_Android_R5CW12/\nVolume(0): Galaxy S23\n  activation_root=mtp://SAMSUNG_SAMSUNG_Android_R5CW12/\n";
        let p = parse(text, &["mtp:host=SAMSUNG_SAMSUNG_Android_R5CW12".into()]);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "Galaxy S23");
        assert!(p[0].path.as_ref().unwrap().ends_with("mtp:host=SAMSUNG_SAMSUNG_Android_R5CW12"));
        let q = parse(text, &[]);
        assert!(q[0].path.is_none());
        assert!(parse("Volume(0): USB\n  activation_root=file:///x\n", &[]).is_empty());
    }
}
