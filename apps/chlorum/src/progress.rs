//! How far an update has got. `chlorum-update install` runs Nix with its
//! machine-readable log (`--log-format internal-json`) and pipes it into
//! `chlorum update-progress <file>`, which keeps <file> (JSON) up to date:
//!
//!   {"phase":"downloading","version":"0.10.0","done":12,"expected":80,"percent":15}
//!
//! The shell reads that file and draws the line.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

// Nix activity and result types (src/libutil/logging.hh)
const ACT_COPY_PATHS: u64 = 103;
const ACT_BUILDS: u64 = 104;
const RES_PROGRESS: u64 = 105;
const RES_SET_EXPECTED: u64 = 106;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub phase: String,
    pub version: String,
    pub done: u64,
    pub expected: u64,
    pub message: String,
}

impl Progress {
    pub fn percent(&self) -> u32 {
        if self.expected == 0 { 0 } else { ((self.done * 100) / self.expected).min(100) as u32 }
    }

    fn to_json(&self) -> String {
        serde_json::json!({
            "phase": self.phase,
            "version": self.version,
            "done": self.done,
            "expected": self.expected,
            "percent": self.percent(),
            "message": self.message,
        })
        .to_string()
    }
}

/// What the update is doing now, if one has run since the computer started.
pub fn read(path: &Path) -> Option<Progress> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    Some(Progress { phase: s("phase"), version: s("version"), done: n("done"), expected: n("expected"), message: s("message") })
}

/// Counts what Nix copies (downloads) and builds.
#[derive(Default)]
pub struct Tally {
    /// activity id → its type
    kinds: HashMap<u64, u64>,
    /// activity id → (done, expected) from progress results
    progress: HashMap<u64, (u64, u64)>,
    /// activity id → expected count announced for a kind of work
    expected: HashMap<(u64, u64), u64>,
    pub building: bool,
    /// the last error Nix reported, for a failed update
    pub error: String,
}

impl Tally {
    /// One line of Nix's log. Returns true when the numbers changed.
    pub fn line(&mut self, line: &str) -> bool {
        let Some(json) = line.strip_prefix("@nix ") else { return false };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return false };
        let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
        let ty = v.get("type").and_then(|x| x.as_u64()).unwrap_or(0);
        let fields: Vec<u64> = v.get("fields").and_then(|f| f.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64()).collect()).unwrap_or_default();
        match v.get("action").and_then(|a| a.as_str()) {
            Some("start") => {
                self.kinds.insert(id, ty);
                if ty == ACT_BUILDS {
                    self.building = true;
                }
                false
            }
            // a finished step is complete, whatever its last count said
            Some("stop") => {
                if let Some(p) = self.progress.get_mut(&id) {
                    p.0 = p.1.max(p.0);
                    return true;
                }
                false
            }
            Some("result") if ty == RES_PROGRESS && fields.len() >= 2 => {
                if matches!(self.kinds.get(&id), Some(&ACT_COPY_PATHS) | Some(&ACT_BUILDS)) {
                    self.progress.insert(id, (fields[0], fields[1]));
                    return true;
                }
                false
            }
            Some("result") if ty == RES_SET_EXPECTED && fields.len() >= 2 => {
                if fields[0] == ACT_COPY_PATHS || fields[0] == ACT_BUILDS {
                    self.expected.insert((id, fields[0]), fields[1]);
                    return true;
                }
                false
            }
            Some("msg") if v.get("level").and_then(|l| l.as_u64()) == Some(0) => {
                let m = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                // without colours, first line only
                let mut plain = String::new();
                let mut esc = false;
                for c in m.chars() {
                    match (esc, c) {
                        (false, '\x1b') => esc = true,
                        (true, 'm') => esc = false,
                        (true, _) => {}
                        (false, '\n') => break,
                        (false, c) => plain.push(c),
                    }
                }
                self.error = plain.trim().trim_start_matches("error:").trim().chars().take(160).collect();
                false
            }
            _ => false,
        }
    }

    /// (done, expected) over everything Nix has said so far.
    pub fn totals(&self) -> (u64, u64) {
        let done: u64 = self.progress.values().map(|p| p.0).sum();
        let from_progress: u64 = self.progress.values().map(|p| p.1).sum();
        let announced: u64 = self.expected.values().sum();
        (done, from_progress.max(announced).max(done))
    }
}

fn write(path: &Path, p: &Progress) {
    let tmp = path.with_extension("new");
    if std::fs::write(&tmp, p.to_json()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `chlorum update-progress <file> <phase> <version> [message]`: with
/// "downloading" it reads Nix's log from stdin and keeps the numbers;
/// any other phase just records it.
pub fn cli(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(args.first().ok_or("usage: chlorum update-progress <file> <phase> <version> [message]")?);
    let phase = args.get(1).cloned().unwrap_or_default();
    let version = args.get(2).cloned().unwrap_or_default();
    let message = args.get(3).cloned().unwrap_or_default();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut p = Progress { phase, version, message, ..Default::default() };
    write(&path, &p);
    if p.phase != "downloading" {
        return Ok(());
    }
    let mut tally = Tally::default();
    let mut last = std::time::Instant::now();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if tally.line(&line) && last.elapsed().as_millis() > 300 {
            (p.done, p.expected) = tally.totals();
            p.phase = if tally.building { "building".into() } else { "downloading".into() };
            write(&path, &p);
            last = std::time::Instant::now();
        }
    }
    (p.done, p.expected) = tally.totals();
    p.message = tally.error.clone();
    write(&path, &p);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_downloads_and_builds() {
        let mut t = Tally::default();
        for l in [
            r#"@nix {"action":"start","id":1,"level":0,"type":103,"text":"","fields":[]}"#,
            r#"@nix {"action":"result","id":1,"type":106,"fields":[103,40]}"#,
            r#"@nix {"action":"start","id":2,"level":0,"type":104,"text":"","fields":[]}"#,
            r#"@nix {"action":"result","id":2,"type":105,"fields":[1,4,1,0]}"#,
            r#"@nix {"action":"result","id":1,"type":105,"fields":[10,40,2,0]}"#,
            "not json at all",
            r#"@nix {"action":"msg","level":3,"msg":"hello"}"#,
        ] {
            t.line(l);
        }
        assert_eq!(t.totals(), (11, 44));
        assert!(t.building);
        let p = Progress { done: 11, expected: 44, ..Default::default() };
        assert_eq!(p.percent(), 25);
        assert_eq!(Progress::default().percent(), 0);
    }
}
