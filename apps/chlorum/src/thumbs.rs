//! Small pictures of files: photos, a frame of each video, the first page
//! of a PDF. Made once in the background and kept in
//! ~/.cache/chlorum/thumbs (named after the file's path, size and date, so
//! a changed file gets a new one).
//!
//! The pictures are made by ffmpeg and pdftoppm, each run in a sandbox with
//! no network and nothing writable but the cache: a file from anywhere (a
//! USB drive) is read by them, so a broken or hostile file cannot do harm.

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// Width of the pictures, in pixels (sharp at 200% on a grid cell).
pub const SIZE: u32 = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Image,
    Video,
    Pdf,
}

/// What kind of picture a file can have, by its name.
pub fn source(path: &Path) -> Option<Source> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "avif" | "jxl" | "tga" | "ico" => Source::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "mpg" | "mpeg" | "wmv" | "flv" | "3gp" | "ts" | "mts" | "m2ts" | "ogv" => Source::Video,
        "pdf" => Source::Pdf,
        _ => return None,
    })
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("chlorum/thumbs")
}

/// Where the picture of this file is (or will be), `width` pixels wide.
pub fn path_for(file: &Path, size: u64, modified: SystemTime, width: u32) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut h);
    size.hash(&mut h);
    modified.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).hash(&mut h);
    let dir = if width == SIZE { cache_dir() } else { cache_dir().join(format!("w{width}")) };
    dir.join(format!("{:016x}.jpg", h.finish()))
}

/// The picture, if it is made already.
pub fn ready(file: &Path, size: u64, modified: SystemTime) -> Option<PathBuf> {
    let p = path_for(file, size, modified, SIZE);
    p.exists().then_some(p)
}

/// The file could not be pictured (broken, or a kind ffmpeg cannot read).
pub fn failed(file: &Path, size: u64, modified: SystemTime) -> bool {
    path_for(file, size, modified, SIZE).with_extension("none").exists()
}

/// Makes the grid picture now (slow: call it off the screen's thread).
pub fn make(file: &Path, size: u64, modified: SystemTime) -> Option<PathBuf> {
    make_sized(file, size, modified, SIZE)
}

/// Makes a picture at most `width` pixels wide (the viewer's are as big as
/// the screen).
pub fn make_sized(file: &Path, size: u64, modified: SystemTime, width: u32) -> Option<PathBuf> {
    let src = source(file)?;
    let out = path_for(file, size, modified, width);
    if out.exists() {
        return Some(out);
    }
    let dir = out.parent()?.to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    // a file that could not be pictured is remembered, not tried again
    let failed = out.with_extension("none");
    if failed.exists() {
        return None;
    }
    let tmp = out.with_extension("part.jpg");
    let input = file.to_string_lossy().to_string();
    let tmp_s = tmp.to_string_lossy().to_string();
    let scale = format!("scale='min({width},iw)':-2:flags=lanczos");
    let ok = match src {
        Source::Image => run(&dir, file, &["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-i", &input, "-frames:v", "1", "-vf", &scale, "-q:v", "4", &tmp_s]),
        // a frame a little way in (the first is often black), the most
        // typical of a few
        Source::Video => {
            let vf = format!("thumbnail=60,{scale}");
            run(&dir, file, &["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-ss", "3", "-i", &input, "-frames:v", "1", "-vf", &vf, "-q:v", "4", &tmp_s])
                || run(&dir, file, &["ffmpeg", "-nostdin", "-loglevel", "error", "-y", "-i", &input, "-frames:v", "1", "-vf", &scale, "-q:v", "4", &tmp_s])
        }
        Source::Pdf => {
            // pdftoppm adds ".jpg" itself: X.part → X.part.jpg, the tmp file
            let prefix = tmp.with_extension("");
            let prefix_s = prefix.to_string_lossy().to_string();
            run(&dir, file, &["pdftoppm", "-jpeg", "-singlefile", "-f", "1", "-l", "1", "-scale-to", &width.to_string(), &input, &prefix_s])
        }
    };
    if ok && tmp.exists() {
        std::fs::rename(&tmp, &out).ok()?;
        Some(out)
    } else {
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::write(failed, b"");
        None
    }
}

/// Runs a picture maker in a sandbox: the whole system read-only, no
/// network, no other processes, only the cache writable, killed after 20 s.
fn run(cache: &Path, file: &Path, cmd: &[&str]) -> bool {
    let cache_s = cache.to_string_lossy().to_string();
    let parent = file.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    let mut c = std::process::Command::new("timeout");
    c.arg("20");
    if which("bwrap") {
        c.args(["bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp"]);
        // the folder of the file (it may be under a hidden home or a drive)
        c.args(["--ro-bind", &parent, &parent]);
        c.args(["--bind", &cache_s, &cache_s]);
        c.args(["--unshare-all", "--die-with-parent", "--new-session", "--"]);
    }
    c.args(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).exists()))
}

/// A few workers that make pictures for the files asked for, newest ask
/// first (what is on screen now), and tell when one is ready.
#[derive(Clone)]
pub struct Maker {
    inner: Arc<(Mutex<Queue>, Condvar)>,
}

struct Queue {
    todo: VecDeque<(PathBuf, u64, SystemTime)>,
    asked: HashSet<PathBuf>,
    /// made since the screen last looked
    done: usize,
    /// being made right now
    working: usize,
}

impl Default for Maker {
    fn default() -> Self {
        Self::new()
    }
}

impl Maker {
    pub fn new() -> Maker {
        let inner = Arc::new((Mutex::new(Queue { todo: VecDeque::new(), asked: HashSet::new(), done: 0, working: 0 }), Condvar::new()));
        for _ in 0..3 {
            let inner = inner.clone();
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let (lock, cv) = &*inner;
                        let mut q = lock.lock().unwrap();
                        while q.todo.is_empty() {
                            q = cv.wait(q).unwrap();
                        }
                        let j = q.todo.pop_front();
                        if j.is_some() {
                            q.working += 1;
                        }
                        j
                    };
                    if let Some((p, size, m)) = job {
                        let made = make(&p, size, m).is_some();
                        let mut q = inner.0.lock().unwrap();
                        q.working -= 1;
                        // a failed one is also news: its card says so
                        let _ = made;
                        q.done += 1;
                    }
                }
            });
        }
        Maker { inner }
    }

    /// The picture of a file: there already, or asked for (None until made).
    pub fn get(&self, file: &Path, size: u64, modified: SystemTime) -> Option<PathBuf> {
        source(file)?;
        if let Some(p) = ready(file, size, modified) {
            return Some(p);
        }
        let (lock, cv) = &*self.inner;
        let mut q = lock.lock().unwrap();
        if q.asked.insert(file.to_path_buf()) {
            // what is on screen now goes first
            q.todo.push_front((file.to_path_buf(), size, modified));
            q.todo.truncate(400);
            cv.notify_one();
        }
        None
    }

    /// Pictures are being made (the screen should look again soon).
    pub fn busy(&self) -> bool {
        let q = self.inner.0.lock().unwrap();
        !q.todo.is_empty() || q.done > 0 || q.working > 0
    }

    /// How many were made since the last call (the screen redraws then).
    pub fn take_done(&self) -> usize {
        std::mem::take(&mut self.inner.0.lock().unwrap().done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_names() {
        assert_eq!(source(Path::new("/a/IMG_1.JPG")), Some(Source::Image));
        assert_eq!(source(Path::new("/a/v.MKV")), Some(Source::Video));
        assert_eq!(source(Path::new("/a/b.pdf")), Some(Source::Pdf));
        assert_eq!(source(Path::new("/a/b.txt")), None);
        let t = SystemTime::UNIX_EPOCH;
        assert_eq!(path_for(Path::new("/a"), 1, t, SIZE), path_for(Path::new("/a"), 1, t, SIZE));
        assert_ne!(path_for(Path::new("/a"), 1, t, SIZE), path_for(Path::new("/a"), 2, t, SIZE));
        assert_ne!(path_for(Path::new("/a"), 1, t, SIZE), path_for(Path::new("/a"), 1, t, 3840));
    }
}
