//! `chlorum portal`: the open/save window every app gets, through
//! xdg-desktop-portal (org.freedesktop.impl.portal.FileChooser). Apps ask the
//! portal; the portal asks us; we show Library as a picker, so people choose
//! from their collections instead of walking folders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.chlorum";

struct FileChooser;

type Results = HashMap<String, OwnedValue>;

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    async fn open_file(
        &self,
        _handle: ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        let multiple = flag(&options, "multiple");
        let folders = flag(&options, "directory");
        let mut args = vec!["open".to_string(), "--title".into(), title];
        if multiple {
            args.push("--multiple".into());
        }
        if folders {
            args.push("--folders".into());
        }
        for e in filter_exts(&options) {
            args.push("--ext".into());
            args.push(e);
        }
        respond(run_picker(args).await)
    }

    async fn save_file(
        &self,
        _handle: ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        let mut name = text(&options, "current_name").unwrap_or_default();
        if name.is_empty() {
            if let Some(f) = bytes_path(&options, "current_file") {
                name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            }
        }
        let args = vec!["save".to_string(), "--title".into(), title, "--name".into(), name];
        respond(run_picker(args).await)
    }

    /// Several files at once (e.g. "save all attachments"): choose the
    /// collection, keep the names.
    async fn save_files(
        &self,
        _handle: ObjectPath<'_>,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        let names: Vec<String> = options
            .get("files")
            .and_then(|v| Vec::<Vec<u8>>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|b| {
                let s = String::from_utf8_lossy(&b).trim_end_matches('\0').to_string();
                Path::new(&s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(s)
            })
            .collect();
        let args = vec!["open".to_string(), "--folders".into(), "--title".into(), title];
        match run_picker(args).await {
            Some(dirs) if !dirs.is_empty() => respond(Some(names.iter().map(|n| dirs[0].join(n)).collect())),
            _ => respond(None),
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }
}

fn flag(o: &HashMap<String, OwnedValue>, key: &str) -> bool {
    o.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

fn text(o: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    o.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

fn bytes_path(o: &HashMap<String, OwnedValue>, key: &str) -> Option<PathBuf> {
    let b = Vec::<u8>::try_from(o.get(key)?.try_clone().ok()?).ok()?;
    let s = String::from_utf8_lossy(&b).trim_end_matches('\0').to_string();
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// File types the app asked for, as extensions (glob filters like "*.pdf").
/// Media-type filters are left out: then every file is shown.
fn filter_exts(o: &HashMap<String, OwnedValue>) -> Vec<String> {
    let mut out = Vec::new();
    let filters = o
        .get("current_filter")
        .and_then(|v| <(String, Vec<(u32, String)>)>::try_from(v.try_clone().ok()?).ok())
        .map(|f| vec![f])
        .or_else(|| o.get("filters").and_then(|v| Vec::<(String, Vec<(u32, String)>)>::try_from(v.try_clone().ok()?).ok()))
        .unwrap_or_default();
    for (_, rules) in filters {
        for (kind, pattern) in rules {
            if kind != 0 {
                // a media type: no extension list can stand for it
                return vec![];
            }
            match glob_ext(&pattern) {
                Some(e) => out.push(e),
                // "*" or anything we cannot read: show everything
                None => return vec![],
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn glob_ext(pattern: &str) -> Option<String> {
    let e = pattern.strip_prefix("*.")?;
    // "*.[pP][dD][fF]" → "pdf"
    let plain: String = if e.contains('[') {
        let mut s = String::new();
        let mut chars = e.chars();
        while let Some(c) = chars.next() {
            if c == '[' {
                let inner: String = chars.by_ref().take_while(|c| *c != ']').collect();
                s.extend(inner.chars().next());
            } else {
                s.push(c);
            }
        }
        s
    } else {
        e.to_string()
    };
    (!plain.is_empty() && plain.chars().all(|c| c.is_ascii_alphanumeric())).then(|| plain.to_lowercase())
}

/// Runs `chlorum pick …` and reads the chosen paths.
async fn run_picker(args: Vec<String>) -> Option<Vec<PathBuf>> {
    let exe = std::env::current_exe().unwrap_or_else(|_| "chlorum".into());
    let app = crate::sandbox::focused_app().await;
    let out = tokio::process::Command::new(exe).arg("pick").args(&args).stdin(std::process::Stdio::null()).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    // an app kept away from personal files sees only what was handed to it
    Some(crate::sandbox::hand_over(app.as_ref(), paths, args.first().map(String::as_str) == Some("save")))
}

fn respond(chosen: Option<Vec<PathBuf>>) -> (u32, Results) {
    let Some(paths) = chosen.filter(|p| !p.is_empty()) else { return (1, HashMap::new()) };
    let uris: Vec<String> = paths.iter().map(|p| file_uri(p)).collect();
    let mut r = HashMap::new();
    if let Ok(v) = OwnedValue::try_from(Value::from(uris)) {
        r.insert("uris".to_string(), v);
    }
    (0, r)
}

fn file_uri(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

pub fn run() -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let _conn = zbus::connection::Builder::session()
            .and_then(|b| b.name(BUS_NAME))
            .and_then(|b| b.serve_at("/org/freedesktop/portal/desktop", FileChooser))
            .map_err(|e| e.to_string())?
            .build()
            .await
            .map_err(|e| e.to_string())?;
        eprintln!("chlorum: portal ready");
        std::future::pending::<()>().await;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert_eq!(glob_ext("*.pdf").as_deref(), Some("pdf"));
        assert_eq!(glob_ext("*.[pP][nN][gG]").as_deref(), Some("png"));
        assert_eq!(glob_ext("*"), None);
        assert_eq!(glob_ext("*.tar.gz"), None);
    }

    #[test]
    fn uri() {
        assert_eq!(file_uri(Path::new("/home/a/My File.pdf")), "file:///home/a/My%20File.pdf");
    }
}
