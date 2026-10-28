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
    /// the first error Nix reported, for a failed update
    pub error: String,
    /// what Nix said, as plain text (the last few hundred lines), kept
    /// next to the progress file for when something goes wrong
    pub log: std::collections::VecDeque<String>,
}

/// The cause in one of Nix's error messages. An evaluation error is a
/// trace ("… while calling the 'head' builtin", "at /nix/store/…") ending
/// in the real "error: …" line; other errors start with it.
fn cause(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).collect();
    let traced = lines.iter().any(|l| l.starts_with('…') || l.starts_with("..."));
    if traced {
        if let Some(l) = lines.iter().rev().filter_map(|l| l.strip_prefix("error:")).map(|l| l.trim()).find(|l| !l.is_empty()) {
            return Some(l.to_string());
        }
    }
    lines
        .iter()
        .map(|l| l.trim_start_matches("error:").trim())
        .find(|l| !l.is_empty() && !l.starts_with('…') && !l.starts_with("...") && !l.starts_with("at "))
        .map(|l| l.to_string())
}

/// The cause in Nix's error output (stderr of a plain command).
pub fn first_error(text: &str) -> Option<String> {
    cause(&plain(text))
}

/// Without terminal colours.
fn plain(m: &str) -> String {
    let mut out = String::new();
    let mut esc = false;
    for c in m.chars() {
        match (esc, c) {
            (false, '\x1b') => esc = true,
            (true, 'm') => esc = false,
            (true, _) => {}
            (false, c) => out.push(c),
        }
    }
    out
}

const RES_BUILD_LOG_LINE: u64 = 101;

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
            Some("msg") => {
                let text = plain(v.get("msg").and_then(|m| m.as_str()).unwrap_or(""));
                for l in text.lines() {
                    self.keep(l);
                }
                if v.get("level").and_then(|l| l.as_u64()) == Some(0) && self.error.is_empty() {
                    // the first real line of the first error: the cause; later
                    // ones are often only "failed dependency"
                    if let Some(l) = cause(&text) {
                        self.error = l.chars().take(200).collect();
                    }
                }
                false
            }
            Some("result") if ty == RES_BUILD_LOG_LINE => {
                if let Some(l) = v.get("fields").and_then(|f| f.get(0)).and_then(|x| x.as_str()) {
                    self.keep(&plain(l));
                }
                false
            }
            _ => false,
        }
    }

    fn keep(&mut self, line: &str) {
        if self.log.len() >= 400 {
            self.log.pop_front();
        }
        self.log.push_back(line.to_string());
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
    // everything Nix said, readable without root (journalctl needs it)
    let text: Vec<&str> = tally.log.iter().map(String::as_str).collect();
    let _ = std::fs::write(path.with_extension("log"), text.join("\n") + "\n");
    // and the reason in the journal too, for a failed update
    if !tally.error.is_empty() {
        eprintln!("chlorum-update: {}", tally.error);
        for l in text.iter().rev().take(30).rev() {
            eprintln!("  {l}");
        }
    }
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

    #[test]
    fn finds_the_cause_under_a_trace() {
        let t = "error:\n       … while calling the 'head' builtin\n         at /nix/store/x/lib.nix:12:3:\n\n       … while fetching the input 'git+https://github.com/x'\n\n       error: executing 'git': No such file or directory";
        assert_eq!(cause(t).as_deref(), Some("executing 'git': No such file or directory"));
        assert_eq!(cause("error: Build failed due to failed dependency").as_deref(), Some("Build failed due to failed dependency"));
        assert_eq!(cause("error:\n       hash mismatch").as_deref(), Some("hash mismatch"));
    }

    #[test]
    fn keeps_the_first_real_error() {
        let mut t = Tally::default();
        for l in [
            r#"@nix {"action":"msg","level":0,"msg":"\u001b[31;1merror:\u001b[0m\n       hash mismatch in fixed-output derivation 'x.drv'"}"#,
            r#"@nix {"action":"result","id":5,"type":101,"fields":["cargo: compiling chlorum"]}"#,
            r#"@nix {"action":"msg","level":0,"msg":"\u001b[31;1merror:\u001b[0m Build failed due to failed dependency"}"#,
        ] {
            t.line(l);
        }
        assert_eq!(t.error, "hash mismatch in fixed-output derivation 'x.drv'");
        assert!(t.log.iter().any(|l| l.contains("compiling chlorum")));
        assert!(t.log.iter().any(|l| l.contains("failed dependency")));
    }
}
