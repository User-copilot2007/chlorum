//! What kind of file this is, and which app opens it: read straight from
//! the system's MIME database (shared-mime-info's globs2 and subclasses)
//! and the mimeapps.list files, so nothing depends on `file`, `xdg-mime`
//! or a desktop environment being there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Where the MIME database and app lists live, most important first.
fn data_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    match std::env::var_os("XDG_DATA_HOME") {
        Some(d) => v.push(PathBuf::from(d)),
        None => v.extend(home().map(|h| h.join(".local/share"))),
    }
    v.extend(crate::store::profile_dirs().into_iter().filter_map(|d| d.parent().map(Path::to_path_buf)));
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    v.extend(sys.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    for d in ["/run/current-system/sw/share", "/usr/share"] {
        let p = PathBuf::from(d);
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

/// (weight, pattern, mime) from every globs2 file.
fn globs() -> Vec<(u32, String, String)> {
    let mut out = vec![];
    for d in data_dirs() {
        let Ok(s) = std::fs::read_to_string(d.join("mime/globs2")) else { continue };
        out.extend(parse_globs(&s));
    }
    out
}

fn parse_globs(s: &str) -> Vec<(u32, String, String)> {
    s.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.splitn(4, ':');
            let w = f.next()?.parse().ok()?;
            let mime = f.next()?.to_string();
            let pat = f.next()?.to_string();
            let case_sensitive = f.next().is_some_and(|o| o.contains("cs"));
            Some((w, if case_sensitive { pat } else { pat.to_lowercase() }, mime))
        })
        .collect()
}

/// A glob as shared-mime-info uses them: "*.tar.gz", "Makefile", "*.[ch]"
/// (only * and ? and simple [..] sets).
fn glob_match(pat: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&p[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some('[') => {
                let Some(end) = p.iter().position(|c| *c == ']') else { return false };
                !n.is_empty() && p[1..end].contains(&n[0]) && go(&p[end + 1..], &n[1..])
            }
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

fn by_name(name: &str, globs: &[(u32, String, String)]) -> Option<String> {
    let lower = name.to_lowercase();
    globs
        .iter()
        .filter(|(_, pat, _)| glob_match(pat, if pat.chars().any(|c| c.is_uppercase()) { name } else { &lower }))
        // the heaviest, then the longest (most precise) pattern wins
        .max_by_key(|(w, pat, _)| (*w, pat.len()))
        .map(|(_, _, m)| m.clone())
}

/// The file's type: by its name, then by a look at its first bytes.
pub fn of(path: &Path) -> String {
    if path.is_dir() {
        return "inode/directory".into();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if let Some(m) = by_name(&name, &globs()) {
        return m;
    }
    let mut head = [0u8; 512];
    let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    sniff(&head[..n])
}

fn sniff(b: &[u8]) -> String {
    let starts = |m: &[u8]| b.starts_with(m);
    let m = if starts(b"%PDF") {
        "application/pdf"
    } else if starts(b"\x89PNG") {
        "image/png"
    } else if starts(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if starts(b"GIF8") {
        "image/gif"
    } else if starts(b"PK\x03\x04") {
        "application/zip"
    } else if starts(b"\x7fELF") {
        "application/x-executable"
    } else if b.len() > 11 && &b[4..8] == b"ftyp" {
        "video/mp4"
    } else if starts(b"\x1aE\xdf\xa3") {
        "video/x-matroska"
    } else if starts(b"ID3") || starts(b"\xff\xfb") {
        "audio/mpeg"
    } else if starts(b"OggS") {
        "audio/ogg"
    } else if starts(b"fLaC") {
        "audio/flac"
    } else if b.is_empty() || std::str::from_utf8(b).is_ok() || !b.contains(&0) {
        // nothing special and no zero bytes: text (an empty file too)
        "text/plain"
    } else {
        "application/octet-stream"
    };
    m.into()
}

/// The types this one is a kind of (text/x-rust → text/plain), nearest first.
fn parents(mime: &str) -> Vec<String> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for d in data_dirs() {
        let Ok(s) = std::fs::read_to_string(d.join("mime/subclasses")) else { continue };
        for l in s.lines() {
            if let Some((a, b)) = l.split_once(' ') {
                map.entry(a.to_string()).or_default().push(b.to_string());
            }
        }
    }
    let mut out = vec![];
    let mut todo = vec![mime.to_string()];
    while let Some(m) = todo.pop() {
        for p in map.get(&m).cloned().unwrap_or_default() {
            if !out.contains(&p) && p != mime {
                out.push(p.clone());
                todo.insert(0, p);
            }
        }
    }
    // every text is text/plain, and everything is octet-stream at heart
    if mime.starts_with("text/") && !out.iter().any(|p| p == "text/plain") {
        out.push("text/plain".into());
    }
    out
}

/// What mimeapps.list and defaults.list say, in the order the XDG spec
/// gives them weight.
fn chosen() -> Vec<(String, Vec<String>)> {
    let mut files = vec![];
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => files.push(PathBuf::from(d).join("mimeapps.list")),
        None => files.extend(home().map(|h| h.join(".config/mimeapps.list"))),
    }
    files.push(PathBuf::from("/etc/xdg/mimeapps.list"));
    for d in data_dirs() {
        files.push(d.join("applications/mimeapps.list"));
        files.push(d.join("applications/defaults.list"));
    }
    let mut out = vec![];
    for f in files {
        let Ok(s) = std::fs::read_to_string(&f) else { continue };
        out.extend(parse_defaults(&s));
    }
    out
}

fn parse_defaults(s: &str) -> Vec<(String, Vec<String>)> {
    let mut out = vec![];
    let mut on = false;
    for l in s.lines().map(str::trim) {
        if l.starts_with('[') {
            on = l == "[Default Applications]";
            continue;
        }
        if !on {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let ids = v.split(';').map(|x| x.trim().trim_end_matches(".desktop").to_string()).filter(|x| !x.is_empty()).collect();
            out.push((k.trim().to_string(), ids));
        }
    }
    out
}

/// The app (desktop id, without ".desktop") that opens this type: the one
/// chosen for it or for a type it is a kind of, otherwise any installed app
/// that says it handles it.
pub fn app_for(mime: &str, installed: &HashMap<String, crate::apps::App>) -> Option<String> {
    let mut kinds = vec![mime.to_string()];
    kinds.extend(parents(mime));
    let chosen = chosen();
    for k in &kinds {
        for (m, ids) in &chosen {
            if m == k {
                if let Some(id) = ids.iter().find(|id| installed.contains_key(id.as_str())) {
                    return Some(id.clone());
                }
            }
        }
    }
    for k in &kinds {
        let mut v: Vec<&String> = installed.iter().filter(|(_, a)| a.mime.iter().any(|m| m == k)).map(|(id, _)| id).collect();
        v.sort();
        if let Some(id) = v.first() {
            return Some((*id).clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_defaults() {
        let g = parse_globs("# x\n50:text/x-rust:*.rs\n50:application/gzip:*.gz\n55:application/x-compressed-tar:*.tar.gz\n50:text/x-csrc:*.[ch]\n50:text/x-makefile:Makefile:cs\n");
        assert_eq!(by_name("main.RS", &g).as_deref(), Some("text/x-rust"));
        assert_eq!(by_name("a.tar.gz", &g).as_deref(), Some("application/x-compressed-tar"));
        assert_eq!(by_name("x.h", &g).as_deref(), Some("text/x-csrc"));
        assert_eq!(by_name("Makefile", &g).as_deref(), Some("text/x-makefile"));
        assert_eq!(by_name("notes", &g), None);
        assert_eq!(sniff(b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff(b"hello\n"), "text/plain");
        let d = parse_defaults("[Added Associations]\nx/y=a.desktop\n[Default Applications]\nvideo/mp4=mpv.desktop;vlc.desktop;\n");
        assert_eq!(d, vec![("video/mp4".to_string(), vec!["mpv".to_string(), "vlc".to_string()])]);
        assert!(parents("text/x-rust").contains(&"text/plain".to_string()));
    }
}
