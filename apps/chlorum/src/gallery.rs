//! `chlorum gallery [file]` — Gallery: every photo and video, newest first,
//! by month, as pictures (not names). Enter shows one on the whole screen;
//! ← → go to the one before and after; Enter on a video plays it. Opened
//! with a file (from Library), it shows that one, among the others of its
//! folder.
//!
//! Keys, grid: arrows move, Enter views, Tab shows all / photos / videos /
//! screenshots, Delete puts in the bin (Enter to confirm), Esc closes.
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
    let settings = Settings::default()
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
    Some(Media { path: p.to_path_buf(), size: m.len(), modified: m.modified().unwrap_or(SystemTime::UNIX_EPOCH), video: src == thumbs::Source::Video })
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

/// The grid's lines: each month starts a new row under its title.
fn lines(list: &[Media], cols: usize) -> Vec<Line> {
    let mut out = vec![];
    let mut month = String::new();
    let mut row: Vec<usize> = vec![];
    for (i, m) in list.iter().enumerate() {
        let label = month_label(m.modified);
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
    lines_cache: std::cell::RefCell<Option<((Show, usize, usize), std::rc::Rc<Vec<Line>>)>>,
}

impl Gallery {
    fn list(&self) -> Vec<&Media> {
        self.all.iter().filter(|m| self.show.keeps(m)).collect()
    }
    fn cols(&self) -> usize {
        // cells of about 300 pixels, at least three
        (((self.size.width - 120.0) / 316.0).floor() as usize).clamp(3, 12)
    }
    /// The grid's lines for what is shown now (kept until it changes).
    fn lines_now(&self) -> std::rc::Rc<Vec<Line>> {
        let key = (self.show, self.cols(), self.all.len());
        if let Some((k, l)) = &*self.lines_cache.borrow() {
            if *k == key {
                return l.clone();
            }
        }
        let list: Vec<Media> = self.list().into_iter().cloned().collect();
        let l = std::rc::Rc::new(lines(&list, key.1));
        *self.lines_cache.borrow_mut() = Some((key, l.clone()));
        l
    }

    fn current(&self) -> Option<Media> {
        self.list().get(self.sel).map(|m| (*m).clone())
    }
    fn say(&mut self, s: impl Into<String>) {
        self.note = Some((s.into(), Instant::now()));
    }

    /// Makes the big pictures of the shown one and its neighbours.
    fn prefetch(&mut self) -> Task<Message> {
        let list: Vec<Media> = self.list().into_iter().cloned().collect();
        let width = ((self.size.width * 2.0) as u32).clamp(1920, 3840);
        let mut tasks = vec![];
        for d in [0i64, 1, -1] {
            let i = self.sel as i64 + d;
            if i < 0 {
                continue;
            }
            let Some(m) = list.get(i as usize).cloned() else { continue };
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
                let n = self.list().len();
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
        let n = self.list().len();
        let cols = self.cols();
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
            Some(Named::PageDown) => self.sel = (self.sel + cols * 4).min(n.saturating_sub(1)),
            Some(Named::PageUp) => self.sel = self.sel.saturating_sub(cols * 4),
            Some(Named::Home) => self.sel = 0,
            Some(Named::End) => self.sel = n.saturating_sub(1),
            Some(Named::Tab) => {
                let i = Show::ALL.iter().position(|s| *s == self.show).unwrap_or(0);
                self.show = Show::ALL[(i + 1) % Show::ALL.len()];
                self.sel = 0;
            }
            Some(Named::Enter) if n > 0 => {
                self.viewing = true;
                return self.prefetch();
            }
            Some(Named::Escape) => std::process::exit(0),
            _ => {}
        }
        let _ = ch;
        Task::none()
    }

    fn viewer_key(&mut self, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        let n = self.list().len();
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
        let list: Vec<Media> = self.list().into_iter().cloned().collect();
        let cols = self.cols();
        let w = ((self.size.width - 144.0 - (cols as f32 - 1.0) * 16.0) / cols as f32).max(160.0);
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
                        row = row.push(self.cell(*i, &list[*i], w));
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
        let count = if list.is_empty() { String::new() } else { format!("{} of {}", self.sel + 1, list.len()) };
        let mut col = widget::Column::new()
            .push(widget::Row::new().push(text("Gallery", SANS_BOLD, 44.0, TEXT)).push(hfill()).push(tabs).align_y(Alignment::Center))
            .push(text(count, MONO, 14.0, TEXT_2))
            .push(space(1.0, 18.0));
        if !self.loaded {
            col = col.push(text("Looking for photos and videos…", SANS, 22.0, TEXT_2));
        } else if list.is_empty() {
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
        col = col.push(widget::container(grid).height(Length::Fill));
        if let Some((n, _)) = &self.note {
            col = col.push(text(n.clone(), SANS_BOLD, 19.0, YELLOW));
        }
        col = col.push(hints(&[("← ↑ ↓ →", "Move", false), ("ENTER", "View", true), ("TAB", "Photos · Videos", false), ("DEL", "Bin", false), ("ESC", "Close", false)], 1.0));
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
        let n = self.list().len();
        let mut top = widget::Row::new().push(text(format!("{} / {}", self.sel + 1, n), MONO, 16.0, TEXT_2)).push(hfill());
        if m.video {
            top = top.push(text("▶ VIDEO · ENTER PLAYS", MONO_BOLD, 16.0, YELLOW));
        }
        let mut col = widget::Column::new().push(top).push(space(1.0, 10.0)).push(widget::container(pic).center(Length::Fill));
        if self.details {
            let dt: chrono::DateTime<chrono::Local> = m.modified.into();
            col = col.push(text(m.name(), SANS_BOLD, 22.0, TEXT)).push(text(
                format!("{}  ·  {}  ·  {}", dt.format("%-d %B %Y, %H:%M"), crate::drives::size_label(m.size), m.path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
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
                self.all = list;
                self.loaded = true;
                self.sel = at;
                if self.viewing {
                    return self.prefetch();
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
                        let n = self.list().len();
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
                let n = self.list().len();
                if self.viewing {
                    if y < 0.0 && n > 0 {
                        self.sel = (self.sel + 1).min(n - 1);
                    } else if y > 0.0 {
                        self.sel = self.sel.saturating_sub(1);
                    }
                    return self.prefetch();
                }
                let cols = self.cols();
                if y < 0.0 && n > 0 {
                    self.sel = (self.sel + cols).min(n - 1);
                } else if y > 0.0 {
                    self.sel = self.sel.saturating_sub(cols);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, days_ago: u64) -> Media {
        Media { path: PathBuf::from(name), size: 1, modified: SystemTime::now() - Duration::from_secs(days_ago * 86400), video: false }
    }

    #[test]
    fn months_start_new_rows() {
        let list = vec![m("a", 0), m("b", 0), m("c", 0), m("d", 400), m("e", 400)];
        let ls = lines(&list, 2);
        assert_eq!(ls[0], Line::Month("This month".into()));
        assert_eq!(ls[1], Line::Row(vec![0, 1]));
        assert_eq!(ls[2], Line::Row(vec![2]));
        assert!(matches!(&ls[3], Line::Month(_)));
        assert_eq!(ls[4], Line::Row(vec![3, 4]));
    }
}
