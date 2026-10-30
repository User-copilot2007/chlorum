//! `chlorum gallery [file]` — Gallery: every photo and video, newest first,
//! by month, as pictures (not names). Enter shows one on the whole screen;
//! ← → go to the one before and after; Enter on a video plays it. Opened
//! with a file (from Library), it shows that one, among the others of its
//! folder.
//!
//! Keys, grid: arrows move, Enter views, Tab shows all / photos / videos /
//! screenshots, A shows the albums (the folders they are in), S sorts
//! (date, name, resolution, file size), T shows the
//! timeline (jump to any year and month), PageUp/PageDown go a month back
//! or on, typing a year (2019) or, sorted by name, a name jumps there,
//! Delete puts in the bin (Enter to confirm), Esc closes.
//! Viewer: ← → before / after, Space slideshow, I details, Enter plays a
//! video, Delete bins it, Esc back to the grid.

use crate::thumbs;
use crate::ui::*;
use chrono::Datelike;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{self, Alignment, ContentFit, Length, Subscription, event, keyboard, mouse};
use cosmic::{Application, Element, widget};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub fn run(args: &[String]) -> Result<(), String> {
    let start = args.first().map(PathBuf::from).filter(|p| p.is_file());
    let settings = Settings::default().scale_factor(crate::ui::screen_zoom())
        .default_font(SANS)
        .client_decorations(false)
        .default_icon_theme("Papirus-Dark")
        .size(iced::Size::new(1600.0, 1000.0));
    cosmic::app::run::<Gallery>(settings, start).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub video: bool,
    /// width and height, once read (in the background)
    pub dims: Option<(u32, u32)>,
}

impl Media {
    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }
    fn screenshot(&self) -> bool {
        self.path.components().any(|c| c.as_os_str() == "Screenshots")
    }
}

fn media_of(p: &Path) -> Option<Media> {
    let src = thumbs::source(p)?;
    if src == thumbs::Source::Pdf {
        return None;
    }
    let m = std::fs::metadata(p).ok()?;
    Some(Media { path: p.to_path_buf(), size: m.len(), modified: m.modified().unwrap_or(SystemTime::UNIX_EPOCH), video: src == thumbs::Source::Video, dims: None })
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<Media>) {
    if depth > 8 || out.len() > 200_000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk(&p, depth + 1, out);
        } else if t.is_file() {
            out.extend(media_of(&p));
        }
    }
}

/// Every photo and video in Library, Downloads and the drives that are
/// open, newest first.
fn everything() -> Vec<Media> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut out = vec![];
    for d in [home.join("Library"), home.join("Downloads"), home.join("Pictures"), home.join("Videos")] {
        walk(&d, 0, &mut out);
    }
    let user = std::env::var("USER").unwrap_or_default();
    walk(&PathBuf::from(format!("/run/media/{user}")), 0, &mut out);
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// The photos and videos of one folder, newest first.
fn folder_of(file: &Path) -> Vec<Media> {
    let mut out = vec![];
    if let Some(d) = file.parent() {
        walk(d, 8, &mut out);
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Show {
    All,
    Photos,
    Videos,
    Screenshots,
}

impl Show {
    const ALL: [Show; 4] = [Show::All, Show::Photos, Show::Videos, Show::Screenshots];
    fn label(self) -> &'static str {
        match self {
            Show::All => "All",
            Show::Photos => "Photos",
            Show::Videos => "Videos",
            Show::Screenshots => "Screenshots",
        }
    }
    fn keeps(self, m: &Media) -> bool {
        match self {
            Show::All => true,
            Show::Photos => !m.video && !m.screenshot(),
            Show::Videos => m.video,
            Show::Screenshots => m.screenshot(),
        }
    }
}

/// The order of the grid (S).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Sort {
    Newest,
    Oldest,
    NameAz,
    NameZa,
    Largest,
    Smallest,
    BiggestFile,
}

impl Sort {
    const ALL: [Sort; 7] = [Sort::Newest, Sort::Oldest, Sort::NameAz, Sort::NameZa, Sort::Largest, Sort::Smallest, Sort::BiggestFile];
    fn label(self) -> &'static str {
        match self {
            Sort::Newest => "Newest first",
            Sort::Oldest => "Oldest first",
            Sort::NameAz => "Name, A to Z",
            Sort::NameZa => "Name, Z to A",
            Sort::Largest => "Highest resolution first",
            Sort::Smallest => "Lowest resolution first",
            Sort::BiggestFile => "Biggest files first",
        }
    }
    fn by_date(self) -> bool {
        matches!(self, Sort::Newest | Sort::Oldest)
    }
    fn key(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::NameAz => "name",
            Sort::NameZa => "name-desc",
            Sort::Largest => "resolution",
            Sort::Smallest => "resolution-asc",
            Sort::BiggestFile => "size",
        }
    }
    fn path() -> PathBuf {
        crate::apps::config_dir().join("chlorum/gallery-sort")
    }
    fn load() -> Sort {
        let k = std::fs::read_to_string(Sort::path()).unwrap_or_default();
        Sort::ALL.into_iter().find(|s| s.key() == k.trim()).unwrap_or(Sort::Newest)
    }
    fn save(self) {
        let p = Sort::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, self.key());
    }
}

/// The order of the photos shown: indexes into the list, filtered (Tab)
/// and sorted (S).
fn order(all: &[Media], show: Show, sort: Sort, album: Option<&Path>) -> Vec<usize> {
    let mut v: Vec<usize> = (0..all.len()).filter(|i| show.keeps(&all[*i]) && album.is_none_or(|a| all[*i].path.parent() == Some(a))).collect();
    let pixels = |i: &usize| all[*i].dims.map(|(w, h)| w as u64 * h as u64);
    let name = |i: &usize| all[*i].name().to_lowercase();
    match sort {
        Sort::Newest => v.sort_by(|a, b| all[*b].modified.cmp(&all[*a].modified)),
        Sort::Oldest => v.sort_by(|a, b| all[*a].modified.cmp(&all[*b].modified)),
        Sort::NameAz => v.sort_by_cached_key(name),
        Sort::NameZa => {
            v.sort_by_cached_key(name);
            v.reverse();
        }
        // not read yet: at the end
        Sort::Largest => v.sort_by_key(|i| std::cmp::Reverse(pixels(i).unwrap_or(0))),
        Sort::Smallest => v.sort_by_key(|i| pixels(i).unwrap_or(u64::MAX)),
        Sort::BiggestFile => v.sort_by_key(|i| std::cmp::Reverse(all[*i].size)),
    }
    v
}

/// What the grid's sections are called: months for a date order, the first
/// letter for names, the resolution for resolution, the size for size.
fn section_of(m: &Media, sort: Sort) -> String {
    match sort {
        Sort::Newest | Sort::Oldest => month_label(m.modified),
        Sort::NameAz | Sort::NameZa => m.name().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
        Sort::Largest | Sort::Smallest => match m.dims {
            None => "Resolution not read yet".into(),
            Some((w, h)) => {
                let mp = w as f64 * h as f64 / 1e6;
                if mp >= 40.0 {
                    "40 megapixels and more".into()
                } else if mp >= 20.0 {
                    "20 to 40 megapixels".into()
                } else if mp >= 12.0 {
                    "12 to 20 megapixels".into()
                } else if mp >= 8.0 {
                    "8 to 12 megapixels (4K)".into()
                } else if mp >= 2.0 {
                    "2 to 8 megapixels (Full HD)".into()
                } else {
                    "Under 2 megapixels".into()
                }
            }
        },
        Sort::BiggestFile => {
            let mb = m.size / 1_000_000;
            if mb >= 1000 {
                "1 GB and more".into()
            } else if mb >= 100 {
                "100 MB to 1 GB".into()
            } else if mb >= 10 {
                "10 to 100 MB".into()
            } else if mb >= 1 {
                "1 to 10 MB".into()
            } else {
                "Under 1 MB".into()
            }
        }
    }
}

/// One line of the grid: a month's title or a row of pictures.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    Month(String),
    Row(Vec<usize>),
}

fn month_label(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    let now = chrono::Local::now();
    if dt.year() == now.year() && dt.month() == now.month() {
        "This month".into()
    } else if dt.year() == now.year() {
        dt.format("%B").to_string()
    } else {
        dt.format("%B %Y").to_string()
    }
}

/// The grid's lines: each section (a month, when by date) starts a new
/// row under its title. The numbers are positions in the order shown.
fn lines(all: &[Media], order: &[usize], sort: Sort, cols: usize) -> Vec<Line> {
    let mut out = vec![];
    let mut month = String::new();
    let mut row: Vec<usize> = vec![];
    for (i, k) in order.iter().enumerate() {
        let label = section_of(&all[*k], sort);
        if label != month {
            if !row.is_empty() {
                out.push(Line::Row(std::mem::take(&mut row)));
            }
            out.push(Line::Month(label.clone()));
            month = label;
        }
        row.push(i);
        if row.len() == cols {
            out.push(Line::Row(std::mem::take(&mut row)));
        }
    }
    if !row.is_empty() {
        out.push(Line::Row(row));
    }
    out
}

#[derive(Clone, Debug)]
pub enum Message {
    /// the photos and videos, and which one to start on
    Loaded(Vec<Media>, usize),
    Key(keyboard::Key, keyboard::Modifiers),
    Resized(iced::Size),
    Tick,
    /// a big picture for the viewer is ready
    Big(PathBuf, Option<PathBuf>),
    Click(usize),
    Scroll(f32),
    /// widths and heights read in the background: (file, dims), how many
    /// read so far, of how many
    Dims(Vec<(PathBuf, (u32, u32))>, usize, usize),
}

/// A panel over the grid: choosing the order, or the timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Panel {
    Sort(usize),
    Timeline(usize),
    Albums(usize),
}

pub struct Gallery {
    core: Core,
    all: Vec<Media>,
    loaded: bool,
    show: Show,
    sel: usize,
    /// the viewer is open
    viewing: bool,
    details: bool,
    slideshow: Option<Instant>,
    /// Delete was pressed: Enter puts it in the bin
    confirm: bool,
    note: Option<(String, Instant)>,
    size: iced::Size,
    thumbs: thumbs::Maker,
    /// big pictures made for the viewer (file → picture)
    big: std::collections::HashMap<PathBuf, Option<PathBuf>>,
    /// opened with a file: only its folder
    folder: bool,
    last_click: Option<(usize, Instant)>,
    /// the grid's lines, made again only when what is shown changes
    lines_cache: std::cell::RefCell<Option<((Show, Sort, usize, u64), std::rc::Rc<Vec<Line>>)>>,
    sort: Sort,
    /// what is shown, in order (indexes into `all`)
    order: Vec<usize>,
    /// counts changes of `order`, for the caches
    version: u64,
    panel: Option<Panel>,
    /// typed to jump (a year, or the start of a name) and when
    typed: (String, Instant),
    /// resolutions read so far, of how many
    dims_read: (usize, usize),
    /// mouse wheel: when it last moved, for speeding up
    last_wheel: Instant,
    /// only the photos and videos of this folder (an album), when chosen
    album: Option<PathBuf>,
}

impl Gallery {
    fn len(&self) -> usize {
        self.order.len()
    }
    fn at(&self, i: usize) -> Option<&Media> {
        self.order.get(i).map(|k| &self.all[*k])
    }
    /// The order again (after a change of what is shown or how), staying
    /// on the same photo.
    fn reorder(&mut self) {
        let keep = self.at(self.sel).map(|m| m.path.clone());
        self.order = order(&self.all, self.show, self.sort, self.album.as_deref());
        self.version += 1;
        self.sel = keep.and_then(|p| self.order.iter().position(|k| self.all[*k].path == p)).unwrap_or(0).min(self.order.len().saturating_sub(1));
    }
    /// The albums: every folder holding photos or videos, with how many
    /// (of the kind shown), by name. The first line is "everything".
    fn albums(&self) -> Vec<(Option<PathBuf>, String, usize)> {
        let mut count: std::collections::HashMap<&Path, usize> = std::collections::HashMap::new();
        for m in self.all.iter().filter(|m| self.show.keeps(m)) {
            if let Some(d) = m.path.parent() {
                *count.entry(d).or_default() += 1;
            }
        }
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let mut v: Vec<(Option<PathBuf>, String, usize)> = count
            .into_iter()
            .map(|(d, n)| {
                // "Photos › Rome": where it is, without the home (or Library)
                let inside = d.strip_prefix(home.join("Library")).or_else(|_| d.strip_prefix(&home)).unwrap_or(d);
                let shown = inside.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(" › ");
                (Some(d.to_path_buf()), shown, n)
            })
            .collect();
        v.sort_by_key(|(_, l, _)| l.to_lowercase());
        let total = self.all.iter().filter(|m| self.show.keeps(m)).count();
        v.insert(0, (None, "Everything".into(), total));
        v
    }

    /// The start of each section (month, letter…) with its name and size.
    fn sections(&self) -> Vec<(String, usize, usize)> {
        let mut out: Vec<(String, usize, usize)> = vec![];
        for (i, k) in self.order.iter().enumerate() {
            let label = section_of(&self.all[*k], self.sort);
            match out.last_mut() {
                Some((l, _, n)) if *l == label => *n += 1,
                _ => out.push((label, i, 1)),
            }
        }
        out
    }
    fn cols(&self) -> usize {
        // cells of about 300 pixels, at least three
        (((self.size.width - 120.0) / 316.0).floor() as usize).clamp(3, 12)
    }
    /// The grid's lines for what is shown now (kept until it changes).
    fn lines_now(&self) -> std::rc::Rc<Vec<Line>> {
        let key = (self.show, self.sort, self.cols(), self.version);
        if let Some((k, l)) = &*self.lines_cache.borrow() {
            if *k == key {
                return l.clone();
            }
        }
        let l = std::rc::Rc::new(lines(&self.all, &self.order, self.sort, key.2));
        *self.lines_cache.borrow_mut() = Some((key, l.clone()));
        l
    }

    fn current(&self) -> Option<Media> {
        self.at(self.sel).cloned()
    }
    fn say(&mut self, s: impl Into<String>) {
        self.note = Some((s.into(), Instant::now()));
    }

    /// Makes the big pictures of the shown one and its neighbours.
    fn prefetch(&mut self) -> Task<Message> {
        let width = ((self.size.width * 2.0) as u32).clamp(1920, 3840);
        let mut tasks = vec![];
        for d in [0i64, 1, -1] {
            let i = self.sel as i64 + d;
            if i < 0 {
                continue;
            }
            let Some(m) = self.at(i as usize).cloned() else { continue };
            if self.big.contains_key(&m.path) {
                continue;
            }
            // JPEG and PNG photos are shown as they are; the rest (HEIC,
            // WebP, videos) as a picture made from them
            let ext = m.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !m.video && matches!(ext.as_str(), "jpg" | "jpeg" | "png") && m.size < 40_000_000 {
                self.big.insert(m.path.clone(), Some(m.path.clone()));
                continue;
            }
            self.big.insert(m.path.clone(), None);
            tasks.push(Task::perform(
                async move {
                    let p = m.path.clone();
                    let r = tokio::task::spawn_blocking(move || thumbs::make_sized(&m.path, m.size, m.modified, width)).await.ok().flatten();
                    (p, r)
                },
                |(p, r)| cosmic::Action::App(Message::Big(p, r)),
            ));
        }
        Task::batch(tasks)
    }

    fn play(&mut self, m: &Media) {
        match crate::library::open_file(&m.path) {
            Ok(app) => self.say(format!("Playing in {app}…")),
            Err(e) => self.say(e),
        }
    }

    fn bin(&mut self) {
        let Some(m) = self.current() else { return };
        match crate::library::trash_file(&m.path) {
            Ok(()) => {
                self.all.retain(|x| x.path != m.path);
                // the old order points into the old list: made afresh, the
                // next photo taking this one's place
                self.order = order(&self.all, self.show, self.sort, self.album.as_deref());
                self.version += 1;
                let n = self.len();
                if n == 0 {
                    self.viewing = false;
                }
                self.sel = self.sel.min(n.saturating_sub(1));
                self.say(format!("{} is in the bin.", m.name()));
            }
            Err(e) => self.say(format!("Could not put it in the bin: {e}")),
        }
    }

    fn grid_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        if let Some(p) = self.panel {
            return self.panel_key(p, named);
        }
        let n = self.len();
        // typing jumps: a year when by date, the start of a name when by name
        let typing = !self.typed.0.is_empty() && self.typed.1.elapsed() < Duration::from_millis(2500);
        let command = ch == "s" || ch == "t" || ch == "a";
        if named.is_none() && ch.chars().count() == 1 && ch != " " && (!command || typing) {
            if self.typed.1.elapsed() > Duration::from_millis(2500) {
                self.typed.0.clear();
            }
            self.typed = (format!("{}{}", self.typed.0, ch), Instant::now());
            let t = self.typed.0.clone();
            let found = if self.sort.by_date() {
                // 2019, or 2019-03 / 201903
                let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
                (digits.len() >= 4).then(|| {
                    let year: i32 = digits[..4].parse().unwrap_or(0);
                    let month: u32 = digits.get(4..6).and_then(|m| m.parse().ok()).unwrap_or(0);
                    self.order.iter().position(|k| {
                        let dt: chrono::DateTime<chrono::Local> = self.all[*k].modified.into();
                        dt.year() == year && (month == 0 || dt.month() == month)
                    })
                }).flatten()
            } else {
                let t = t.to_lowercase();
                self.order.iter().position(|k| self.all[*k].name().to_lowercase().starts_with(&t))
            };
            match found {
                Some(i) => {
                    self.sel = i;
                    self.say(format!("Jumped to “{t}”."));
                }
                None => self.say(format!("“{t}”… {}", if self.sort.by_date() { "type a year, like 2019" } else { "no name starts like that" })),
            }
            return Task::none();
        }
        if ch == "s" {
            self.panel = Some(Panel::Sort(Sort::ALL.iter().position(|x| *x == self.sort).unwrap_or(0)));
            return Task::none();
        }
        if ch == "a" {
            let at = self.albums().iter().position(|(d, _, _)| *d == self.album).unwrap_or(0);
            self.panel = Some(Panel::Albums(at));
            return Task::none();
        }
        if ch == "t" {
            let secs = self.sections();
            let at = secs.iter().rposition(|(_, start, _)| *start <= self.sel).unwrap_or(0);
            self.panel = Some(Panel::Timeline(at));
            return Task::none();
        }
        let ls = self.lines_now();
        // where the chosen one is: its line and column
        let (li, col) = ls
            .iter()
            .enumerate()
            .find_map(|(l, line)| match line {
                Line::Row(r) => r.iter().position(|i| *i == self.sel).map(|c| (l, c)),
                _ => None,
            })
            .unwrap_or((0, 0));
        let row_at = |l: usize| match &ls[l] {
            Line::Row(r) => Some(r.clone()),
            _ => None,
        };
        match named {
            Some(Named::ArrowRight) if n > 0 => self.sel = (self.sel + 1).min(n - 1),
            Some(Named::ArrowLeft) => self.sel = self.sel.saturating_sub(1),
            Some(Named::ArrowDown) => {
                if let Some(r) = (li + 1..ls.len()).find_map(row_at) {
                    self.sel = r[col.min(r.len() - 1)];
                }
            }
            Some(Named::ArrowUp) => {
                if let Some(r) = (0..li).rev().find_map(row_at) {
                    self.sel = r[col.min(r.len() - 1)];
                }
            }
            // a section (a month, when by date) on or back
            Some(Named::PageDown) => {
                let secs = self.sections();
                if let Some((_, start, _)) = secs.iter().find(|(_, start, _)| *start > self.sel) {
                    self.sel = *start;
                } else {
                    self.sel = n.saturating_sub(1);
                }
            }
            Some(Named::PageUp) => {
                let secs = self.sections();
                let cur = secs.iter().rposition(|(_, start, _)| *start <= self.sel).unwrap_or(0);
                // the start of this section, or of the one before when at it
                let target = if secs.get(cur).is_some_and(|(_, start, _)| *start < self.sel) { cur } else { cur.saturating_sub(1) };
                self.sel = secs.get(target).map(|(_, start, _)| *start).unwrap_or(0);
            }
            Some(Named::Home) => self.sel = 0,
            Some(Named::End) => self.sel = n.saturating_sub(1),
            Some(Named::Tab) => {
                let i = Show::ALL.iter().position(|s| *s == self.show).unwrap_or(0);
                self.show = Show::ALL[(i + 1) % Show::ALL.len()];
                self.reorder();
                self.sel = 0;
            }
            Some(Named::Enter) if n > 0 => {
                self.viewing = true;
                return self.prefetch();
            }
            // out of an album first, then closed
            Some(Named::Escape) if self.album.is_some() => {
                self.album = None;
                self.reorder();
            }
            Some(Named::Escape) => std::process::exit(0),
            _ => {}
        }
        Task::none()
    }

    /// Keys in the order or timeline panel.
    fn panel_key(&mut self, p: Panel, named: Option<keyboard::key::Named>) -> Task<Message> {
        use keyboard::key::Named;
        let len = match p {
            Panel::Sort(_) => Sort::ALL.len(),
            Panel::Timeline(_) => self.sections().len(),
            Panel::Albums(_) => self.albums().len(),
        };
        let (i, make): (usize, fn(usize) -> Panel) = match p {
            Panel::Sort(i) => (i, Panel::Sort),
            Panel::Timeline(i) => (i, Panel::Timeline),
            Panel::Albums(i) => (i, Panel::Albums),
        };
        match named {
            Some(Named::ArrowDown) => self.panel = Some(make((i + 1).min(len.saturating_sub(1)))),
            Some(Named::ArrowUp) => self.panel = Some(make(i.saturating_sub(1))),
            Some(Named::PageDown) => self.panel = Some(make((i + 12).min(len.saturating_sub(1)))),
            Some(Named::PageUp) => self.panel = Some(make(i.saturating_sub(12))),
            Some(Named::Home) => self.panel = Some(make(0)),
            Some(Named::End) => self.panel = Some(make(len.saturating_sub(1))),
            Some(Named::Escape) => self.panel = None,
            Some(Named::Enter) => {
                self.panel = None;
                match p {
                    Panel::Sort(i) => {
                        self.sort = Sort::ALL[i];
                        self.sort.save();
                        self.reorder();
                        self.sel = 0;
                        if matches!(self.sort, Sort::Largest | Sort::Smallest) && self.dims_read.0 < self.dims_read.1 {
                            self.say(format!("Reading the resolutions: {} of {}. The order fills in as they come.", self.dims_read.0, self.dims_read.1));
                        }
                    }
                    Panel::Timeline(i) => {
                        if let Some((_, start, _)) = self.sections().get(i) {
                            self.sel = *start;
                        }
                    }
                    Panel::Albums(i) => {
                        if let Some((d, label, _)) = self.albums().get(i).cloned() {
                            self.album = d;
                            self.reorder();
                            self.sel = 0;
                            if self.album.is_some() {
                                self.say(format!("{label}. Esc shows everything again."));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn panel_view(&self, p: Panel) -> Element<'_, Message> {
        let row = |label: String, detail: String, on: bool| -> Element<'_, Message> {
            fill(
                widget::Row::new()
                    .push(text(label, SANS_BOLD, 19.0, if on { BLACK } else { TEXT }))
                    .push(hfill())
                    .push(text(detail, MONO, 14.0, if on { BLACK } else { TEXT_2 }))
                    .align_y(Alignment::Center),
                if on { YELLOW } else { PANEL },
            )
            .padding([10, 16])
            .width(Length::Fill)
            .into()
        };
        let mut col = widget::Column::new().spacing(4).width(Length::Fixed(440.0));
        match p {
            Panel::Sort(at) => {
                col = col.push(text("ORDER", MONO, 14.0, YELLOW)).push(space(1.0, 6.0));
                for (i, s) in Sort::ALL.iter().enumerate() {
                    let detail = if *s == self.sort { "NOW".to_string() } else if matches!(s, Sort::Largest | Sort::Smallest) && self.dims_read.0 < self.dims_read.1 { format!("{}%", self.dims_read.0 * 100 / self.dims_read.1.max(1)) } else { String::new() };
                    col = col.push(row(s.label().to_string(), detail, i == at));
                }
            }
            Panel::Albums(at) => {
                col = col.push(text("ALBUMS", MONO, 14.0, YELLOW)).push(space(1.0, 6.0));
                let all = self.albums();
                let shown = ((self.size.height - 300.0) / 46.0).max(5.0) as usize;
                let first = at.saturating_sub(shown / 2).min(all.len().saturating_sub(shown));
                for (i, (_, label, n)) in all.iter().enumerate().skip(first).take(shown) {
                    let short: String = if label.chars().count() > 34 { format!("…{}", label.chars().rev().take(33).collect::<Vec<_>>().into_iter().rev().collect::<String>()) } else { label.clone() };
                    col = col.push(row(short, format!("{n}"), i == at));
                }
            }
            Panel::Timeline(at) => {
                col = col.push(text(if self.sort.by_date() { "TIMELINE" } else { "SECTIONS" }, MONO, 14.0, YELLOW)).push(space(1.0, 6.0));
                let secs = self.sections();
                // the ones around the chosen one
                let shown = ((self.size.height - 300.0) / 46.0).max(5.0) as usize;
                let first = at.saturating_sub(shown / 2).min(secs.len().saturating_sub(shown));
                for (i, (label, _, n)) in secs.iter().enumerate().skip(first).take(shown) {
                    col = col.push(row(label.clone(), format!("{n}"), i == at));
                }
            }
        }
        col = col.push(space(1.0, 8.0)).push(hints(&[("↑ ↓", "Choose", false), ("ENTER", "Go", true), ("ESC", "Close", false)], 1.0));
        fill(col.padding(20), BLACK).into()
    }

    fn viewer_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        let n = self.len();
        match named {
            Some(Named::ArrowRight) | Some(Named::PageDown) if n > 0 => {
                self.sel = (self.sel + 1).min(n - 1);
                return self.prefetch();
            }
            Some(Named::ArrowLeft) | Some(Named::PageUp) => {
                self.sel = self.sel.saturating_sub(1);
                return self.prefetch();
            }
            Some(Named::Home) => {
                self.sel = 0;
                return self.prefetch();
            }
            Some(Named::End) => {
                self.sel = n.saturating_sub(1);
                return self.prefetch();
            }
            Some(Named::Enter) => {
                if let Some(m) = self.current() {
                    if m.video {
                        self.play(&m);
                    } else {
                        self.details = !self.details;
                    }
                }
            }
            Some(Named::Escape) => {
                if self.folder && self.slideshow.is_none() {
                    std::process::exit(0);
                }
                self.viewing = false;
                self.slideshow = None;
            }
            _ => {}
        }
        match ch {
            "i" => self.details = !self.details,
            " " => {
                self.slideshow = if self.slideshow.is_some() { None } else { Some(Instant::now()) };
                self.say(if self.slideshow.is_some() { "Slideshow: a new one every 5 seconds. Space stops it." } else { "Slideshow stopped." });
            }
            _ => {}
        }
        Task::none()
    }

    fn cell<'a>(&self, i: usize, m: &Media, w: f32) -> Element<'a, Message> {
        let on = i == self.sel;
        let h = w * 0.75;
        let pic: Element<'a, Message> = match self.thumbs.get(&m.path, m.size, m.modified) {
            Some(p) => widget::image(widget::image::Handle::from_path(p)).content_fit(ContentFit::Cover).width(Length::Fixed(w)).height(Length::Fixed(h)).into(),
            None => widget::container(text(
                if thumbs::failed(&m.path, m.size, m.modified) { if m.video { "VIDEO · NO PICTURE" } else { "NO PICTURE" } } else if m.video { "VIDEO" } else { "…" },
                MONO,
                14.0,
                TEXT_2,
            ))
                .center_x(Length::Fixed(w))
                .center_y(Length::Fixed(h))
                .into(),
        };
        let mut stack = widget::Column::new().push(pic);
        if m.video {
            stack = stack.push(text("▶ VIDEO", MONO_BOLD, 13.0, if on { BLACK } else { YELLOW }));
        }
        let card = fill(stack.spacing(4), if on { YELLOW } else { PANEL }).padding(if on { 4 } else { 4 });
        let b = widget::mouse_area(bracket(card, if on { 1.0 } else { 0.0 }, 8.0)).on_press(Message::Click(i));
        b.into()
    }

    fn grid_view(&self) -> Element<'_, Message> {
        let n = self.len();
        let cols = self.cols();
        // with a panel open the same rows, smaller, beside it
        let panel = if self.panel.is_some() { 464.0 } else { 0.0 };
        let w = ((self.size.width - 144.0 - panel - (cols as f32 - 1.0) * 16.0) / cols as f32).max(80.0);
        let ls = self.lines_now();
        // the lines around the chosen one that fit on the screen
        let line_h = |l: &Line| match l {
            Line::Month(_) => 64.0,
            Line::Row(_) => w * 0.75 + 30.0,
        };
        let sel_line = ls.iter().position(|l| matches!(l, Line::Row(r) if r.contains(&self.sel))).unwrap_or(0);
        let room = (self.size.height - 260.0).max(300.0);
        let mut first = sel_line;
        let mut used = line_h(&ls.get(sel_line).cloned().unwrap_or(Line::Month(String::new())));
        // a little above the chosen line, as much below as fits
        while first > 0 && used + line_h(&ls[first - 1]) < room * 0.4 {
            first -= 1;
            used += line_h(&ls[first]);
        }
        if first > 0 && matches!(ls[first], Line::Row(_)) && matches!(ls[first - 1], Line::Month(_)) {
            first -= 1;
        }
        let mut used = 0.0;
        let mut grid = widget::Column::new().spacing(16);
        for l in ls.iter().skip(first) {
            used += line_h(l);
            if used > room && !matches!(l, Line::Month(_)) && grid_has_rows(&ls, first, l) {
                break;
            }
            grid = match l {
                Line::Month(m) => grid.push(text(m.clone(), SANS_BOLD, 28.0, TEXT)),
                Line::Row(r) => {
                    let mut row = widget::Row::new().spacing(16);
                    for i in r {
                        if let Some(m) = self.at(*i) {
                            row = row.push(self.cell(*i, m, w));
                        }
                    }
                    grid.push(row)
                }
            };
            if used > room {
                break;
            }
        }
        let mut tabs = widget::Row::new().spacing(10);
        for s in Show::ALL {
            let on = s == self.show;
            tabs = tabs.push(fill(text(s.label(), SANS_BOLD, 18.0, if on { BLACK } else { TEXT_2 }), if on { YELLOW } else { PANEL_2 }).padding([10, 18]));
        }
        let count = if n == 0 {
            String::new()
        } else {
            let place = self.at(self.sel).map(|m| section_of(m, self.sort)).unwrap_or_default();
            let album = self.album.as_ref().and_then(|a| a.file_name()).map(|a| format!("ALBUM {}  ·  ", a.to_string_lossy().to_uppercase())).unwrap_or_default();
            format!("{album}{} of {}  ·  {}  ·  {}", self.sel + 1, n, place, self.sort.label().to_uppercase())
        };
        let mut col = widget::Column::new()
            .push(widget::Row::new().push(text("Gallery", SANS_BOLD, 44.0, TEXT)).push(hfill()).push(tabs).align_y(Alignment::Center))
            .push(text(count, MONO, 14.0, TEXT_2))
            .push(space(1.0, 18.0));
        if !self.loaded {
            col = col.push(text("Looking for photos and videos…", SANS, 22.0, TEXT_2));
        } else if n == 0 {
            col = col.push(text(
                match self.show {
                    Show::All => "No photos or videos yet. Photos from a phone or a USB drive: open the drive in Library, then they show here too.",
                    Show::Videos => "No videos.",
                    Show::Screenshots => "No screenshots. Print takes one.",
                    Show::Photos => "No photos.",
                },
                SANS,
                22.0,
                TEXT_2,
            ));
        }
        let grid: Element<'_, Message> = match self.panel {
            Some(p) => widget::Row::new().push(widget::container(grid).width(Length::Fill)).push(space(24.0, 1.0)).push(self.panel_view(p)).into(),
            None => grid.into(),
        };
        col = col.push(widget::container(grid).height(Length::Fill));
        if let Some((n, _)) = &self.note {
            col = col.push(text(n.clone(), SANS_BOLD, 19.0, YELLOW));
        }
        col = col.push(hints(
            &[
                ("← ↑ ↓ →", "Move", false),
                ("ENTER", "View", true),
                ("PG UP · DOWN", if self.sort.by_date() { "Month" } else { "Section" }, false),
                ("T", "Timeline", false),
                ("A", "Albums", false),
                ("S", "Order", false),
                ("TYPE", if self.sort.by_date() { "A year" } else { "A name" }, false),
                ("TAB", "Photos · Videos", false),
                ("DEL", "Bin", false),
                ("ESC", if self.album.is_some() { "Everything" } else { "Close" }, false),
            ],
            1.0,
        ));
        fill(col.padding([40, 72]), BLACK).width(Length::Fill).height(Length::Fill).into()
    }

    fn viewer(&self) -> Element<'_, Message> {
        let Some(m) = self.current() else { return space(1.0, 1.0) };
        let pic: Element<'_, Message> = match self.big.get(&m.path) {
            Some(Some(p)) => widget::image(widget::image::Handle::from_path(p.clone())).content_fit(ContentFit::Contain).width(Length::Fill).height(Length::Fill).into(),
            Some(None) if !self.big.contains_key(&m.path) => space(1.0, 1.0),
            _ => match thumbs::ready(&m.path, m.size, m.modified) {
                // the small one until the big one is ready
                Some(p) => widget::image(widget::image::Handle::from_path(p)).content_fit(ContentFit::Contain).width(Length::Fill).height(Length::Fill).into(),
                None => widget::container(text("…", SANS, 40.0, TEXT_2)).center(Length::Fill).into(),
            },
        };
        let n = self.len();
        let mut top = widget::Row::new().push(text(format!("{} / {}", self.sel + 1, n), MONO, 16.0, TEXT_2)).push(hfill());
        if m.video {
            top = top.push(text("▶ VIDEO · ENTER PLAYS", MONO_BOLD, 16.0, YELLOW));
        }
        let mut col = widget::Column::new().push(top).push(space(1.0, 10.0)).push(widget::container(pic).center(Length::Fill));
        if self.details {
            let dt: chrono::DateTime<chrono::Local> = m.modified.into();
            let res = m.dims.map(|(w, h)| format!("{w} × {h}  ·  ")).unwrap_or_default();
            col = col.push(text(m.name(), SANS_BOLD, 22.0, TEXT)).push(text(
                format!("{}  ·  {res}{}  ·  {}", dt.format("%-d %B %Y, %H:%M"), crate::drives::size_label(m.size), m.path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
                MONO,
                15.0,
                TEXT_2,
            ));
        }
        if self.confirm {
            col = col.push(text(format!("Put {} in the bin? Enter: yes · Esc: no", m.name()), SANS_BOLD, 20.0, RED));
        } else if let Some((note, _)) = &self.note {
            col = col.push(text(note.clone(), SANS_BOLD, 19.0, YELLOW));
        }
        col = col.push(space(1.0, 8.0)).push(hints(
            &[("← →", "Before · After", false), (if m.video { "ENTER" } else { "I" }, if m.video { "Play" } else { "Details" }, true), ("SPACE", "Slideshow", false), ("DEL", "Bin", false), ("ESC", "Back", false)],
            1.0,
        ));
        fill(col.padding([28, 48]), BLACK).width(Length::Fill).height(Length::Fill).into()
    }
}

/// Whether a row was already put on screen (one row always shows, however
/// small the window).
fn grid_has_rows(ls: &[Line], first: usize, upto: &Line) -> bool {
    ls.iter().skip(first).take_while(|l| *l != upto).any(|l| matches!(l, Line::Row(_)))
}

impl Application for Gallery {
    type Executor = cosmic::executor::Default;
    type Flags = Option<PathBuf>;
    type Message = Message;
    const APP_ID: &'static str = "chlorum-gallery";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, start: Option<PathBuf>) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let folder = start.is_some();
        let g = Gallery {
            core,
            all: vec![],
            loaded: false,
            show: Show::All,
            sel: 0,
            viewing: folder,
            details: false,
            slideshow: None,
            confirm: false,
            note: None,
            size: iced::Size::new(1600.0, 1000.0),
            thumbs: thumbs::Maker::new(),
            big: Default::default(),
            folder,
            last_click: None,
            lines_cache: Default::default(),
            sort: Sort::load(),
            order: vec![],
            version: 0,
            panel: None,
            typed: (String::new(), Instant::now()),
            dims_read: (0, 0),
            last_wheel: Instant::now(),
            album: None,
        };
        let load = Task::perform(
            async move {
                let start2 = start.clone();
                let list = tokio::task::spawn_blocking(move || match &start2 {
                    Some(f) => folder_of(f),
                    None => everything(),
                })
                .await
                .unwrap_or_default();
                // the file it was opened with comes first in the order
                match start {
                    Some(f) => {
                        let mut list = list;
                        if !list.iter().any(|m| m.path == f) {
                            list.extend(media_of(&f));
                        }
                        let at = list.iter().position(|m| m.path == f).unwrap_or(0);
                        (list, at)
                    }
                    None => (list, 0),
                }
            },
            |(list, at)| cosmic::Action::App(Message::Loaded(list, at)),
        );
        let size = match g.core.main_window_id() {
            Some(id) => cosmic::iced::window::size(id).map(|sz| cosmic::Action::App(Message::Resized(sz))),
            None => Task::none(),
        };
        (g, Task::batch([load, size]))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut s = vec![event::listen_with(|e, status, _| match e {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) if status == event::Status::Ignored => Some(Message::Key(key, modifiers)),
            iced::Event::Window(iced::window::Event::Resized(size)) => Some(Message::Resized(size)),
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => Some(Message::Scroll(match delta {
                mouse::ScrollDelta::Lines { y, .. } => y,
                mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
            })),
            _ => None,
        })];
        // new grid pictures show as they are made; the slideshow moves on;
        // a note goes away
        if self.thumbs.busy() || self.slideshow.is_some() || self.note.is_some() {
            s.push(iced::time::every(Duration::from_millis(400)).map(|_| Message::Tick));
        }
        Subscription::batch(s)
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Loaded(list, at) => {
                // opened with a file: that one, wherever the order puts it
                let start = list.get(at).map(|m| m.path.clone());
                self.all = list;
                self.loaded = true;
                self.reorder();
                if let Some(p) = start.filter(|_| self.folder) {
                    self.sel = self.order.iter().position(|k| self.all[*k].path == p).unwrap_or(0);
                }
                // the resolutions, for that order and the details
                let files: Vec<(PathBuf, u64, SystemTime)> = self.all.iter().map(|m| (m.path.clone(), m.size, m.modified)).collect();
                self.dims_read = (0, files.len());
                let read = Task::run(read_dims(files), |(batch, done, total)| cosmic::Action::App(Message::Dims(batch, done, total)));
                if self.viewing {
                    return Task::batch([read, self.prefetch()]);
                }
                return read;
            }
            Message::Dims(batch, done, total) => {
                self.dims_read = (done, total);
                if !batch.is_empty() {
                    let at: std::collections::HashMap<&Path, usize> = self.all.iter().enumerate().map(|(i, m)| (m.path.as_path(), i)).collect();
                    let found: Vec<(usize, (u32, u32))> = batch.iter().filter_map(|(p, d)| at.get(p.as_path()).map(|i| (*i, *d))).collect();
                    for (i, d) in found {
                        self.all[i].dims = Some(d);
                    }
                    // the order by resolution fills in as they come
                    if matches!(self.sort, Sort::Largest | Sort::Smallest) {
                        self.reorder();
                    }
                }
            }
            Message::Resized(s) => self.size = s,
            Message::Tick => {
                let _ = self.thumbs.take_done();
                if self.note.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(5)) {
                    self.note = None;
                }
                if let Some(t) = self.slideshow {
                    if t.elapsed() > Duration::from_secs(5) {
                        let n = self.len();
                        self.sel = if self.sel + 1 >= n { 0 } else { self.sel + 1 };
                        self.slideshow = Some(Instant::now());
                        return self.prefetch();
                    }
                }
            }
            Message::Big(p, r) => {
                self.big.insert(p, r);
            }
            Message::Click(i) => {
                let double = self.last_click.is_some_and(|(j, t)| j == i && t.elapsed() < Duration::from_millis(450));
                self.sel = i;
                self.last_click = Some((i, Instant::now()));
                if double {
                    self.viewing = true;
                    return self.prefetch();
                }
            }
            Message::Scroll(y) => {
                let n = self.len();
                if self.viewing {
                    if y < 0.0 && n > 0 {
                        self.sel = (self.sel + 1).min(n - 1);
                    } else if y > 0.0 {
                        self.sel = self.sel.saturating_sub(1);
                    }
                    return self.prefetch();
                }
                // a quick spin of the wheel goes further: up to 10 rows a notch
                let cols = self.cols();
                let quick = self.last_wheel.elapsed() < Duration::from_millis(120);
                self.last_wheel = Instant::now();
                let rows = ((y.abs().max(1.0)) * if quick { 4.0 } else { 1.0 }).min(10.0) as usize;
                if y < 0.0 && n > 0 {
                    self.sel = (self.sel + cols * rows).min(n - 1);
                } else if y > 0.0 {
                    self.sel = self.sel.saturating_sub(cols * rows);
                }
            }
            Message::Key(key, _mods) => {
                use keyboard::key::Named;
                let named = match key.as_ref() {
                    keyboard::Key::Named(n) => Some(n),
                    _ => None,
                };
                let ch = match key.as_ref() {
                    keyboard::Key::Character(c) => c.to_lowercase(),
                    _ => String::new(),
                };
                if self.confirm {
                    self.confirm = false;
                    if named == Some(Named::Enter) {
                        self.bin();
                        if self.viewing {
                            return self.prefetch();
                        }
                    }
                    return Task::none();
                }
                if named == Some(Named::Delete) && self.current().is_some() {
                    self.confirm = true;
                    if !self.viewing {
                        if let Some(m) = self.current() {
                            self.say(format!("Put {} in the bin? Enter: yes · Esc: no", m.name()));
                        }
                    }
                    return Task::none();
                }
                return if self.viewing { self.viewer_key(named, &ch) } else { self.grid_key(named, &ch) };
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.viewing { self.viewer() } else { self.grid_view() }
    }
}

fn dims_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("chlorum/dims.tsv")
}

/// Reads the width and height of every photo and video in the background
/// (remembered in ~/.cache/chlorum/dims.tsv, so only new ones are read the
/// next time), telling in batches.
fn read_dims(files: Vec<(PathBuf, u64, SystemTime)>) -> impl cosmic::iced::futures::Stream<Item = (Vec<(PathBuf, (u32, u32))>, usize, usize)> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(4, async move |mut out: cosmic::iced::futures::channel::mpsc::Sender<(Vec<(PathBuf, (u32, u32))>, usize, usize)>| {
        let total = files.len();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(Vec<(PathBuf, (u32, u32))>, usize)>(4);
        std::thread::spawn(move || {
            let secs = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            // what was read before: path → (size, date, w, h)
            let mut known: std::collections::HashMap<PathBuf, (u64, u64, u32, u32)> = std::fs::read_to_string(dims_cache_path())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    if f.len() != 5 {
                        return None;
                    }
                    Some((PathBuf::from(f[0]), (f[1].parse().ok()?, f[2].parse().ok()?, f[3].parse().ok()?, f[4].parse().ok()?)))
                })
                .collect();
            let mut batch = vec![];
            let mut last = Instant::now();
            let mut changed = false;
            for (n, (p, size, modified)) in files.into_iter().enumerate() {
                let d = match known.get(&p) {
                    Some((s, m, w, h)) if *s == size && *m == secs(modified) => Some((*w, *h)),
                    _ => {
                        let d = thumbs::dims(&p);
                        if let Some((w, h)) = d {
                            known.insert(p.clone(), (size, secs(modified), w, h));
                            changed = true;
                        }
                        d
                    }
                };
                if let Some(d) = d {
                    batch.push((p, d));
                }
                if last.elapsed() > Duration::from_millis(700) || batch.len() >= 5000 {
                    if tx.blocking_send((std::mem::take(&mut batch), n + 1)).is_err() {
                        return;
                    }
                    last = Instant::now();
                }
            }
            let _ = tx.blocking_send((batch, total));
            if changed {
                let mut text = String::new();
                for (p, (s, m, w, h)) in &known {
                    if let Some(ps) = p.to_str().filter(|x| !x.contains('\t') && !x.contains('\n')) {
                        text.push_str(&format!("{ps}\t{s}\t{m}\t{w}\t{h}\n"));
                    }
                }
                let path = dims_cache_path();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = path.with_extension("new");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(tmp, path);
                }
            }
        });
        while let Some((batch, done)) = rx.recv().await {
            let _ = out.send((batch, done, total)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, days_ago: u64) -> Media {
        Media { path: PathBuf::from(name), size: 1, modified: SystemTime::now() - Duration::from_secs(days_ago * 86400), video: false, dims: None }
    }

    #[test]
    fn months_start_new_rows() {
        let list = vec![m("a", 0), m("b", 0), m("c", 0), m("d", 400), m("e", 400)];
        let ord = order(&list, Show::All, Sort::Newest, None);
        let ls = lines(&list, &ord, Sort::Newest, 2);
        assert_eq!(ls[0], Line::Month("This month".into()));
        assert_eq!(ls[1], Line::Row(vec![0, 1]));
        assert_eq!(ls[2], Line::Row(vec![2]));
        assert!(matches!(&ls[3], Line::Month(_)));
        assert_eq!(ls[4], Line::Row(vec![3, 4]));
        // by resolution: the biggest first, the ones not read at the end
        let mut list = list;
        list[0].dims = Some((100, 100));
        list[3].dims = Some((4000, 3000));
        let ord = order(&list, Show::All, Sort::Largest, None);
        assert_eq!(ord[0], 3);
        assert_eq!(ord[1], 0);
        // by name, Z to A
        let ord = order(&list, Show::All, Sort::NameZa, None);
        assert_eq!(list[ord[0]].name(), "e");
    }
}
