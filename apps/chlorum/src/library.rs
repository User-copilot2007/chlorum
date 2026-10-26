//! Library: the person's files without folders to manage.
//!
//! Everything lives in collections (~/Library/<collection>). New files land
//! in Downloads; Library asks once where each belongs and files it there,
//! and can remember the answer for every file of the same type. Views by
//! kind (photos, music, documents…) gather files from all collections.

use crate::ui::*;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{self, Alignment, Color, Length, Subscription, event, keyboard};
use cosmic::widget::text_input;
use cosmic::{Application, Element, widget};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn run() -> cosmic::iced::Result {
    let settings = Settings::default()
        .default_font(SANS)
        .client_decorations(false)
        .default_icon_theme("Papirus-Dark")
        .size(iced::Size::new(1600.0, 1000.0));
    cosmic::app::run::<Library>(settings, Mode::Browse)
}

/// What Library is open for.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    /// the person's own Library
    #[default]
    Browse,
    /// an app asks for files to open
    Open { multiple: bool, folders: bool, exts: Vec<String>, title: String },
    /// an app asks where to save
    Save { name: String, title: String },
}

/// What was chosen in a picker; read after the window closes.
static PICKED: std::sync::Mutex<Option<Vec<PathBuf>>> = std::sync::Mutex::new(None);

/// Library as the window apps use to open and save files: the person picks
/// from collections, never from folders. Returns the chosen files, or None
/// when the person backs out.
pub fn pick(mode: Mode) -> Option<Vec<PathBuf>> {
    let settings = Settings::default()
        .default_font(SANS)
        .client_decorations(false)
        .default_icon_theme("Papirus-Dark")
        .size(iced::Size::new(1500.0, 900.0));
    let _ = cosmic::app::run::<Picker>(settings, mode);
    PICKED.lock().ok().and_then(|mut p| p.take())
}

/// `chlorum pick open [--multiple] [--folders] [--ext pdf]… [--title T]`
/// `chlorum pick save [--name N] [--title T]`: prints one path per line;
/// exits 1 when nothing was chosen.
pub fn pick_cli(args: &[String]) -> Result<(), String> {
    let mut it = args.iter();
    let what = it.next().map(String::as_str).unwrap_or("open");
    let (mut multiple, mut folders, mut exts, mut title, mut name) = (false, false, vec![], String::new(), String::new());
    while let Some(a) = it.next() {
        match a.as_str() {
            "--multiple" => multiple = true,
            "--folders" => folders = true,
            "--ext" => exts.extend(it.next().map(|e| e.trim_start_matches('.').to_lowercase())),
            "--title" => title = it.next().cloned().unwrap_or_default(),
            "--name" => name = it.next().cloned().unwrap_or_default(),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let mode = if what == "save" { Mode::Save { name, title } } else { Mode::Open { multiple, folders, exts, title } };
    match pick(mode) {
        Some(paths) if !paths.is_empty() => {
            for p in paths {
                println!("{}", p.display());
            }
            Ok(())
        }
        _ => std::process::exit(1),
    }
}

/// The picker is Library under its own name, so niri can float it above
/// the app that asked.
pub struct Picker(Library);

impl Application for Picker {
    type Executor = cosmic::executor::Default;
    type Flags = Mode;
    type Message = Message;
    const APP_ID: &'static str = "chlorum-picker";

    fn core(&self) -> &Core {
        &self.0.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.0.core
    }
    fn init(core: Core, mode: Mode) -> (Self, Task<Message>) {
        let (l, t) = Library::init(core, mode);
        (Picker(l), t)
    }
    fn subscription(&self) -> Subscription<Message> {
        self.0.subscription()
    }
    fn update(&mut self, m: Message) -> Task<Message> {
        self.0.update(m)
    }
    fn view(&self) -> Element<'_, Message> {
        self.0.view()
    }
}

const COLS: usize = 4;
const DEFAULT_COLLECTIONS: &[&str] = &["Documents", "Photos", "Music", "Videos"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Photo,
    Video,
    Music,
    Document,
    Code,
    Archive,
    Other,
}

impl Kind {
    const VIEWS: [Kind; 4] = [Kind::Photo, Kind::Video, Kind::Music, Kind::Document];
    fn of(path: &Path) -> Kind {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "avif" | "bmp" | "tif" | "tiff" | "raw" | "dng" | "svg" => Kind::Photo,
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
            "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => Kind::Music,
            "pdf" | "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "xls" | "xlsx" | "ods" | "csv" | "ppt" | "pptx" | "odp" | "epub" => Kind::Document,
            "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "go" | "java" | "nix" | "sh" | "json" | "toml" | "yaml" | "yml" | "html" | "css" => Kind::Code,
            "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" | "iso" => Kind::Archive,
            _ => Kind::Other,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Photo => "Photos",
            Kind::Video => "Videos",
            Kind::Music => "Music",
            Kind::Document => "Documents",
            Kind::Code => "Code",
            Kind::Archive => "Archives",
            Kind::Other => "Other",
        }
    }
    fn badge(self) -> &'static str {
        match self {
            Kind::Photo => "IMG",
            Kind::Video => "VID",
            Kind::Music => "AUD",
            Kind::Document => "DOC",
            Kind::Code => "CODE",
            Kind::Archive => "ZIP",
            Kind::Other => "FILE",
        }
    }
}

#[derive(Clone, Debug)]
struct Item {
    path: PathBuf,
    name: String,
    kind: Kind,
    /// collection it is filed in; None while it waits for an answer
    collection: Option<String>,
    modified: SystemTime,
}

#[derive(Clone, Debug, PartialEq)]
enum View {
    /// a flash drive or other removable disk (its UDisks id)
    Drive(String),
    Inbox,
    Recent,
    Collection(String),
    Kind(Kind),
}

impl View {
    fn label(&self) -> String {
        match self {
            View::Inbox => "Needs an answer".into(),
            View::Recent => "Recent".into(),
            View::Collection(c) => c.clone(),
            View::Kind(k) => k.label().into(),
            View::Drive(_) => "Drive".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Focus {
    Side,
    Grid,
    Ask(usize),
    NewName,
    Confirm,
    /// picker: typing the name to save under
    SaveName,
    /// typing the password of an encrypted drive
    DrivePassword,
}

#[derive(Clone, Debug)]
pub enum Message {
    Rescan,
    Key(keyboard::Key, event::Status),
    NewName(String),
    NewSubmit,
    SaveName(String),
    SaveSubmit,
    Drives(Vec<crate::drives::Drive>),
    DriveOpened(String, Result<PathBuf, String>),
    DrivePassword(String),
    DriveSubmit,
    Ejected(String, Result<(), String>),
}

pub struct Library {
    core: Core,
    items: Vec<Item>,
    collections: Vec<String>,
    rules: HashMap<String, String>, // extension -> collection
    views: Vec<View>,
    view: usize,
    sel: usize,
    focus: Focus,
    always: bool,
    new_name: String,
    input_id: widget::Id,
    note: Option<String>,
    mode: Mode,
    /// picker: files marked so far (several at once)
    marked: Vec<PathBuf>,
    save_name: String,
    /// picker: Enter once more replaces a file of the same name
    replace_armed: bool,
    drives: Vec<crate::drives::Drive>,
    /// the files on the drive being looked at
    drive_items: Vec<Item>,
    drive_pw: String,
    drive_busy: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}
fn library_dir() -> PathBuf {
    home().join("Library")
}
/// New files wait here for an answer: downloads, and what apps without
/// personal files saved into their drop boxes.
fn inbox_dirs() -> Vec<PathBuf> {
    let mut v = vec![home().join("Downloads"), home().join("Desktop")];
    v.extend(crate::sandbox::drop_boxes());
    v
}
fn rules_path() -> PathBuf {
    crate::apps::config_dir().join("chlorum/library-rules")
}

impl Application for Library {
    type Executor = cosmic::executor::Default;
    type Flags = Mode;
    type Message = Message;
    const APP_ID: &'static str = "chlorum-library";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, mode: Mode) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let lib = library_dir();
        if !lib.exists() {
            for c in DEFAULT_COLLECTIONS {
                let _ = std::fs::create_dir_all(lib.join(c));
            }
        }
        let mut l = Library {
            core,
            items: vec![],
            collections: vec![],
            rules: load_rules(),
            views: vec![],
            view: 0,
            sel: 0,
            focus: Focus::Side,
            always: false,
            new_name: String::new(),
            input_id: widget::Id::unique(),
            note: None,
            save_name: match &mode {
                Mode::Save { name, .. } => name.clone(),
                _ => String::new(),
            },
            mode,
            marked: vec![],
            replace_armed: false,
            drives: vec![],
            drive_items: vec![],
            drive_pw: String::new(),
            drive_busy: false,
        };
        l.scan();
        l.view = if l.picking() { 0 } else if l.inbox().is_empty() { 1 } else { 0 };
        (l, Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d))))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_secs(3)).map(|_| Message::Rescan),
            event::listen_with(|e, status, _| match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => Some(Message::Key(key, status)),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Rescan => {
                if !matches!(self.focus, Focus::NewName | Focus::SaveName | Focus::DrivePassword) {
                    self.scan();
                }
                return Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d)));
            }
            Message::Drives(d) => {
                if d != self.drives {
                    self.drives = d;
                    self.scan();
                }
            }
            Message::DrivePassword(s) => self.drive_pw = s,
            Message::DriveSubmit => return self.open_drive(),
            Message::DriveOpened(name, r) => {
                self.drive_busy = false;
                self.drive_pw.clear();
                match r {
                    Ok(_) => {
                        self.note = Some(format!("{name} is open."));
                        self.focus = Focus::Side;
                    }
                    Err(e) => {
                        self.note = Some(e);
                        if self.focus == Focus::DrivePassword {
                            return text_input::focus(self.input_id.clone());
                        }
                    }
                }
                return Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d)));
            }
            Message::Ejected(name, r) => {
                self.drive_busy = false;
                self.note = Some(match r {
                    Ok(()) => format!("{name} can be taken out now."),
                    Err(e) => e,
                });
                self.focus = Focus::Side;
                return Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d)));
            }
            Message::SaveName(s) => {
                self.save_name = s;
                self.replace_armed = false;
            }
            Message::SaveSubmit => return self.save_here(),
            Message::NewName(s) => self.new_name = s,
            Message::NewSubmit => return self.create_and_file(),
            Message::Key(k, status) => return self.key(k, status),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut side = widget::Column::new().spacing(4).width(Length::Fixed(330.0));
        side = side.push(
            widget::Row::new()
                .push(widget::icon::from_name("com.chlorum.Logo").size(56))
                .push(text(self.heading(), SANS_BOLD, 34.0, TEXT))
                .spacing(16)
                .align_y(Alignment::Center),
        );
        if let Mode::Open { title, .. } | Mode::Save { title, .. } = &self.mode {
            if !title.is_empty() {
                side = side.push(space(1.0, 8.0)).push(text(short(title, 30), SANS, 18.0, TEXT_2));
            }
        }
        side = side.push(space(1.0, 26.0));
        for (i, v) in self.views.iter().enumerate() {
            if matches!(v, View::Kind(Kind::Photo)) {
                side = side.push(space(1.0, 10.0)).push(text("BY KIND", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
            }
            if matches!(v, View::Drive(_)) && !matches!(self.views.get(i.wrapping_sub(1)), Some(View::Drive(_))) {
                side = side.push(space(1.0, 10.0)).push(text("DRIVES", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
            }
            if matches!(v, View::Collection(_)) && !matches!(self.views.get(i.wrapping_sub(1)), Some(View::Collection(_))) {
                side = side.push(space(1.0, 10.0)).push(text("COLLECTIONS", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
            }
            let on = i == self.view;
            let active = on && self.focus == Focus::Side;
            let count = self.items_in(v).len();
            let inbox = *v == View::Inbox && count > 0;
            let fg = if active { BLACK } else if on || inbox { YELLOW } else { TEXT_2 };
            let bg = if active { YELLOW } else if on { PANEL_2 } else { Color::TRANSPARENT };
            side = side.push(
                fill(
                    widget::Row::new()
                        .push(text(self.view_label(v), SANS_BOLD, 21.0, fg))
                        .push(hfill())
                        .push(text(self.view_count(v, count), MONO, 15.0, fg))
                        .align_y(Alignment::Center),
                    bg,
                )
                .padding([14, 22])
                .width(Length::Fill),
            );
        }

        let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
        let items = self.items_in(&view);
        let mut grid = widget::Column::new().spacing(22);
        for (r, chunk) in items.chunks(COLS).enumerate() {
            let mut row = widget::Row::new().spacing(22);
            for (c, it) in chunk.iter().enumerate() {
                let i = r * COLS + c;
                let on = i == self.sel && matches!(self.focus, Focus::Grid | Focus::Ask(_) | Focus::NewName | Focus::Confirm);
                let marked = self.marked.contains(&it.path);
                let thumb = fill(
                    widget::container(fill(text(it.kind.badge(), MONO_BOLD, 24.0, BLACK), YELLOW).padding([4, 12]))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                    PANEL_2,
                )
                .width(Length::Fill)
                .height(Length::Fixed(130.0));
                let meta = format!(
                    "{}{} · {}",
                    if marked { "■ " } else { "" },
                    it.collection.clone().unwrap_or_else(|| "NEW".into()).to_uppercase(),
                    when(it.modified)
                );
                let card = fill(
                    widget::Column::new()
                        .push(thumb)
                        .push(
                            widget::Column::new()
                                .push(text(short(&it.name, 26), SANS_BOLD, 18.0, if on { BLACK } else { TEXT }))
                                .push(text(meta, MONO, 13.0, if on { BLACK } else { TEXT_2 }))
                                .spacing(6)
                                .padding([14, 16]),
                        ),
                    if on { YELLOW } else if marked { PANEL_2 } else { PANEL },
                )
                .width(Length::Fixed(250.0));
                row = row.push(bracket(card, if on && self.focus == Focus::Grid { 1.0 } else { 0.0 }, 8.0));
            }
            grid = grid.push(row);
        }
        if items.is_empty() {
            let msg = match (&self.mode, &view) {
                (Mode::Save { .. }, _) => "Choose where it goes, then press → to name it.",
                (_, View::Inbox) => "Nothing waiting. New downloads appear here.",
                _ => "Nothing here yet.",
            };
            grid = grid.push(text(msg, SANS, 22.0, TEXT_2));
        }
        let main = widget::Column::new()
            .push(text(self.view_label(&view), SANS_BOLD, 44.0, TEXT))
            .push(space(1.0, 26.0))
            .push(widget::scrollable(grid).height(Length::Fill))
            .width(Length::Fill);

        let mut row = widget::Row::new().push(side).push(space(56.0, 1.0)).push(main).height(Length::Fill);
        if let Some(panel) = self.drive_panel(&view) {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.save_panel(&view) {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.ask_panel(&items) {
            row = row.push(space(30.0, 1.0)).push(panel);
        }

        let on_drive = matches!(view, View::Drive(_));
        let mut h: Vec<(&str, &str, bool)> = match (&self.mode, self.focus) {
            (_, Focus::DrivePassword) => vec![("ENTER", "Unlock", true), ("ESC", "Back", false)],
            (Mode::Browse, Focus::Side) if on_drive => vec![("↑ ↓", "View", false), ("→", "Open", true), ("E", "Take out", false)],
            (Mode::Browse, Focus::Grid) if on_drive => {
                vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("C", "Copy into Library", false), ("E", "Take out", false)]
            }
            (Mode::Open { folders: true, .. }, Focus::Side) => vec![("↑ ↓", "Collection", false), ("ENTER", "Choose this one", true), ("ESC", "Cancel", false)],
            (Mode::Open { .. }, Focus::Side) => vec![("↑ ↓", "View", false), ("→", "Files", true), ("ESC", "Cancel", false)],
            (Mode::Open { multiple: true, .. }, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("SPACE", "Mark", false), ("ENTER", "Open", true), ("ESC", "Back", false)],
            (Mode::Open { .. }, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("ESC", "Back", false)],
            (Mode::Save { .. }, Focus::Side) => vec![("↑ ↓", "Where", false), ("→", "Name it", true), ("ESC", "Cancel", false)],
            (Mode::Save { .. }, _) => vec![("ENTER", "Save", true), ("ESC", "Back", false)],
            (_, Focus::SaveName) => vec![("ENTER", "Save", true), ("ESC", "Back", false)],
            (_, Focus::Side) => vec![("↑ ↓", "View", false), ("→", "Files", true)],
            (_, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("M", "Move to…", false), ("DEL", "Delete", false)],
            (_, Focus::Ask(_)) => vec![("↑ ↓", "Choose", false), ("1–9", "Pick", false), ("ENTER", "File it", true), ("A", "Always", false), ("ESC", "Back", false)],
            (_, Focus::NewName) => vec![("ENTER", "Create", true), ("ESC", "Back", false)],
            (_, Focus::Confirm) => vec![("ENTER", "Delete", true), ("ESC", "Keep", false)],
        };
        if !self.picking() {
            h.push(("SUPER", "System menu", false));
        }
        let mut body = widget::Column::new().push(row).push(space(1.0, 18.0));
        if let Some(n) = &self.note {
            body = body.push(text(n.clone(), MONO, 16.0, YELLOW)).push(space(1.0, 10.0));
        }
        body = body.push(hints(&h, 1.0)).padding([40, 48]);
        fill(body, BLACK).width(Length::Fill).height(Length::Fill).into()
    }
}

impl Library {
    fn scan(&mut self) {
        let mut items = Vec::new();
        let lib = library_dir();
        let mut collections: Vec<String> = std::fs::read_dir(&lib)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        collections.sort();
        for c in &collections {
            walk(&lib.join(c), 0, &mut |p| items.push(item(p, Some(c.clone()))));
        }
        for d in inbox_dirs() {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().to_string();
                    // skip hidden files and downloads still in progress
                    // a browser keeps an empty file under the final name while
                    // the real one downloads next to it as .part
                    let busy = ["part", "crdownload", "download"]
                        .iter()
                        .any(|ext| d.join(format!("{name}.{ext}")).exists() || p.with_extension(ext).exists());
                    let empty = std::fs::metadata(&p).map(|m| m.len() == 0).unwrap_or(true);
                    if p.is_file() && !name.starts_with('.') && !name.ends_with(".part") && !name.ends_with(".crdownload") && !busy && !empty {
                        items.push(item(&p, None));
                    }
                }
            }
        }
        items.sort_by(|a, b| b.modified.cmp(&a.modified));
        // files with a remembered rule are filed without asking
        let mut moved = false;
        for it in items.iter().filter(|i| i.collection.is_none()) {
            let ext = ext_of(&it.path);
            if let Some(c) = self.rules.get(&ext) {
                if file_into(&it.path, c).is_ok() {
                    moved = true;
                }
            }
        }
        if moved {
            return self.scan();
        }
        let old_view = self.views.get(self.view).cloned();
        self.items = items;
        self.collections = collections.clone();
        let mut views = match &self.mode {
            Mode::Browse => vec![View::Inbox, View::Recent],
            Mode::Open { folders: false, .. } => vec![View::Recent, View::Inbox],
            _ => vec![],
        };
        views.extend(collections.into_iter().map(View::Collection));
        if matches!(self.mode, Mode::Browse | Mode::Open { folders: false, .. }) {
            views.extend(Kind::VIEWS.iter().map(|k| View::Kind(*k)));
        }
        if matches!(self.mode, Mode::Browse | Mode::Open { folders: false, .. }) {
            views.extend(self.drives.iter().map(|d| View::Drive(d.id.clone())));
        }
        if views.is_empty() {
            views.push(View::Collection(DEFAULT_COLLECTIONS[0].into()));
        }
        self.views = views;
        // the files on the drive being looked at
        self.drive_items.clear();
        let current = old_view.clone().or_else(|| self.views.get(self.view).cloned());
        if let Some(View::Drive(id)) = &current {
            if let Some(d) = self.drives.iter().find(|d| &d.id == id) {
                if let crate::drives::State::Open(root) = &d.state {
                    let tag = d.name.clone();
                    let mut n = 0;
                    walk(root, 0, &mut |p| {
                        if n < 3000 {
                            self.drive_items.push(item(p, Some(tag.clone())));
                            n += 1;
                        }
                    });
                    self.drive_items.sort_by(|a, b| b.modified.cmp(&a.modified));
                }
            }
        }
        if let Some(v) = old_view {
            self.view = self.views.iter().position(|x| *x == v).unwrap_or(0);
        }
        let n = self.items_in(&self.views[self.view].clone()).len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn inbox(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.collection.is_none()).collect()
    }

    fn items_in(&self, v: &View) -> Vec<Item> {
        if let View::Drive(id) = v {
            let looking = matches!(self.views.get(self.view), Some(View::Drive(x)) if x == id);
            if !looking {
                return vec![];
            }
            let exts: &[String] = match &self.mode {
                Mode::Open { exts, .. } => exts,
                _ => &[],
            };
            return self.drive_items.iter().filter(|i| exts.is_empty() || exts.contains(&ext_of(&i.path))).cloned().collect();
        }
        if matches!(self.mode, Mode::Save { .. } | Mode::Open { folders: true, .. }) {
            // choosing a place, not a file: show what is already there
            return self.items.iter().filter(|i| matches!(v, View::Collection(c) if i.collection.as_deref() == Some(c))).take(40).cloned().collect();
        }
        let exts: &[String] = match &self.mode {
            Mode::Open { exts, .. } => exts,
            _ => &[],
        };
        self.items
            .iter()
            .filter(|i| exts.is_empty() || exts.contains(&ext_of(&i.path)))
            .filter(|i| match v {
                View::Drive(_) => false,
                View::Inbox => i.collection.is_none(),
                View::Recent => true,
                View::Collection(c) => i.collection.as_deref() == Some(c),
                View::Kind(k) => i.kind == *k,
            })
            .take(if *v == View::Recent { 60 } else { usize::MAX })
            .cloned()
            .collect()
    }

    /// The answers offered for a file: collections, best guess first.
    fn choices(&self, it: &Item) -> Vec<String> {
        let guess = match it.kind {
            Kind::Photo => "Photos",
            Kind::Video => "Videos",
            Kind::Music => "Music",
            _ => "Documents",
        };
        let mut v: Vec<String> = self.collections.clone();
        if let Some(p) = v.iter().position(|c| c == guess) {
            let g = v.remove(p);
            v.insert(0, g);
        }
        v
    }

    fn ask_panel(&self, items: &[Item]) -> Option<Element<'_, Message>> {
        let it = items.get(self.sel)?;
        let asking = matches!(self.focus, Focus::Ask(_) | Focus::NewName) || it.collection.is_none();
        if !asking && self.focus != Focus::Confirm {
            return None;
        }
        let mut col = widget::Column::new().spacing(14).width(Length::Fixed(400.0));
        if self.focus == Focus::Confirm {
            col = col
                .push(text("DELETE", MONO, 14.0, RED))
                .push(text(it.name.clone(), SANS_BOLD, 22.0, TEXT))
                .push(text("It goes to the bin and can be brought back for 30 days.", SANS, 18.0, TEXT_2))
                .push(fill(text("ENTER  Delete", SANS_BOLD, 20.0, BLACK), RED).padding([14, 22]));
            return Some(fill(col.padding(30), PANEL).into());
        }
        let title = if it.collection.is_none() { "Where does this belong?" } else { "Move to…" };
        col = col
            .push(text(if it.collection.is_none() { "NEEDS AN ANSWER".to_string() } else { "MOVE".to_string() }, MONO, 14.0, YELLOW))
            .push(text(short(&it.name, 34), SANS_BOLD, 22.0, TEXT))
            .push(text(format!("{} · {}", it.kind.label().to_uppercase(), when(it.modified)), MONO, 13.0, TEXT_2))
            .push(space(1.0, 8.0))
            .push(text(title, SANS_BOLD, 28.0, TEXT));
        let choices = self.choices(it);
        let sel = match self.focus {
            Focus::Ask(i) => Some(i),
            _ => None,
        };
        for (i, c) in choices.iter().enumerate() {
            let on = sel == Some(i);
            col = col.push(
                fill(
                    widget::Row::new()
                        .push(boxed(text((i + 1).to_string(), MONO_BOLD, 15.0, if on { BLACK } else { TEXT }), Color::TRANSPARENT, if on { BLACK } else { TEXT_2 }, 1.5).padding([2, 8]))
                        .push(text(c.clone(), SANS_BOLD, 21.0, if on { BLACK } else { TEXT }))
                        .spacing(16)
                        .align_y(Alignment::Center),
                    if on { YELLOW } else { PANEL_2 },
                )
                .padding([14, 18])
                .width(Length::Fill),
            );
        }
        let new_on = sel == Some(choices.len()) || self.focus == Focus::NewName;
        let new_row: Element<'_, Message> = if self.focus == Focus::NewName {
            text_input::text_input("Name of the new collection", self.new_name.as_str())
                .id(self.input_id.clone())
                .on_input(Message::NewName)
                .on_submit(|_| Message::NewSubmit)
                .style(field_style())
                .size(21)
                .padding([14, 18])
                .into()
        } else {
            fill(
                widget::Row::new()
                    .push(boxed(text("N", MONO_BOLD, 15.0, if new_on { BLACK } else { TEXT }), Color::TRANSPARENT, if new_on { BLACK } else { TEXT_2 }, 1.5).padding([2, 8]))
                    .push(text("New collection…", SANS_BOLD, 21.0, if new_on { BLACK } else { TEXT }))
                    .spacing(16)
                    .align_y(Alignment::Center),
                if new_on { YELLOW } else { PANEL_2 },
            )
            .padding([14, 18])
            .width(Length::Fill)
            .into()
        };
        col = col.push(new_row);
        let ext = ext_of(&it.path);
        if !ext.is_empty() {
            let mark = if self.always { "■" } else { "□" };
            col = col.push(space(1.0, 6.0)).push(text(
                format!("{mark}  A  Always file .{ext} files there"),
                MONO,
                15.0,
                if self.always { YELLOW } else { TEXT_2 },
            ));
        }
        Some(fill(col.padding(30), PANEL).into())
    }

    fn current_item(&self) -> Option<Item> {
        let v = self.views.get(self.view)?.clone();
        self.items_in(&v).get(self.sel).cloned()
    }

    fn key(&mut self, key: keyboard::Key, status: event::Status) -> Task<Message> {
        use keyboard::key::Named;
        let named = match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        };
        let ch = match key.as_ref() {
            keyboard::Key::Character(c) => c.to_lowercase(),
            _ => String::new(),
        };
        self.note = None;
        let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
        let n = self.items_in(&view).len();
        if let View::Drive(id) = &view {
            if let Some(t) = self.drive_key(id.clone(), named, &ch, n) {
                return t;
            }
        }
        if self.picking() {
            if let Some(t) = self.pick_key(named, &ch, &view, n) {
                return t;
            }
        }
        match self.focus {
            Focus::SaveName | Focus::DrivePassword => {}
            Focus::NewName => {
                if named == Some(Named::Escape) {
                    self.focus = Focus::Ask(0);
                }
                let _ = status;
            }
            Focus::Side => match named {
                Some(Named::ArrowDown) => {
                    self.view = (self.view + 1).min(self.views.len() - 1);
                    self.sel = 0;
                }
                Some(Named::ArrowUp) => {
                    self.view = self.view.saturating_sub(1);
                    self.sel = 0;
                }
                Some(Named::ArrowRight) | Some(Named::Enter) if n > 0 => {
                    self.focus = Focus::Grid;
                    if view == View::Inbox {
                        self.focus = Focus::Ask(0);
                    }
                }
                _ => {}
            },
            Focus::Grid => match named {
                Some(Named::ArrowRight) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                Some(Named::ArrowLeft) => {
                    if self.sel % COLS == 0 {
                        self.focus = Focus::Side;
                    } else {
                        self.sel -= 1;
                    }
                }
                Some(Named::ArrowDown) => {
                    if self.sel + COLS < n {
                        self.sel += COLS;
                    }
                }
                Some(Named::ArrowUp) => {
                    if self.sel >= COLS {
                        self.sel -= COLS;
                    }
                }
                Some(Named::Escape) => self.focus = Focus::Side,
                Some(Named::Enter) => {
                    if let Some(it) = self.current_item() {
                        if it.collection.is_none() {
                            self.focus = Focus::Ask(0);
                        } else {
                            open(&it.path);
                        }
                    }
                }
                Some(Named::Delete) => self.focus = Focus::Confirm,
                _ => {
                    if ch == "m" {
                        self.focus = Focus::Ask(0);
                    }
                }
            },
            Focus::Confirm => match named {
                Some(Named::Enter) => {
                    if let Some(it) = self.current_item() {
                        self.note = Some(match trash(&it.path) {
                            Ok(()) => format!("{} is in the bin.", it.name),
                            Err(e) => format!("Could not delete: {e}"),
                        });
                    }
                    self.focus = Focus::Grid;
                    self.scan();
                }
                _ => self.focus = Focus::Grid,
            },
            Focus::Ask(i) => {
                let Some(it) = self.current_item() else {
                    self.focus = Focus::Side;
                    return Task::none();
                };
                let choices = self.choices(&it);
                let last = choices.len(); // "New collection…"
                match named {
                    Some(Named::ArrowDown) => self.focus = Focus::Ask((i + 1).min(last)),
                    Some(Named::ArrowUp) => self.focus = Focus::Ask(i.saturating_sub(1)),
                    Some(Named::Escape) | Some(Named::ArrowLeft) => {
                        self.focus = if view == View::Inbox { Focus::Side } else { Focus::Grid };
                    }
                    Some(Named::Enter) => {
                        if i == last {
                            return self.ask_new_name();
                        }
                        return self.answer(&it, &choices[i]);
                    }
                    _ => {}
                }
                if let Some(d) = ch.chars().next().and_then(|c| c.to_digit(10)) {
                    if let Some(c) = choices.get((d as usize).wrapping_sub(1)) {
                        return self.answer(&it, &c.clone());
                    }
                }
                match ch.as_str() {
                    "n" => return self.ask_new_name(),
                    "a" => self.always = !self.always,
                    _ => {}
                }
            }
        }
        Task::none()
    }

    fn drive(&self, id: &str) -> Option<crate::drives::Drive> {
        self.drives.iter().find(|d| d.id == id).cloned()
    }

    fn view_label(&self, v: &View) -> String {
        match v {
            View::Drive(id) => self.drive(id).map(|d| d.name).unwrap_or_else(|| "Drive".into()),
            other => other.label(),
        }
    }

    fn view_count(&self, v: &View, n: usize) -> String {
        use crate::drives::State;
        match v {
            View::Drive(id) => match self.drive(id).map(|d| d.state) {
                Some(State::Locked) => "LOCKED".into(),
                Some(State::Closed) => "USB".into(),
                Some(State::Open(_)) if n > 0 => n.to_string(),
                _ => "OPEN".into(),
            },
            _ => n.to_string(),
        }
    }

    /// Keys on a drive. None: the usual meaning.
    fn drive_key(&mut self, id: String, named: Option<keyboard::key::Named>, ch: &str, n: usize) -> Option<Task<Message>> {
        use crate::drives::State;
        use keyboard::key::Named;
        let d = self.drive(&id)?;
        if self.drive_busy {
            return Some(Task::none());
        }
        if self.focus == Focus::DrivePassword {
            if named == Some(Named::Escape) {
                self.focus = Focus::Side;
                self.drive_pw.clear();
            }
            return Some(Task::none());
        }
        if ch == "e" && !self.picking() {
            self.drive_busy = true;
            self.note = Some(format!("Closing {}…", d.name));
            let name = d.name.clone();
            return Some(Task::perform(crate::drives::eject(d), move |r| cosmic::Action::App(Message::Ejected(name.clone(), r))));
        }
        match self.focus {
            Focus::Side if matches!(named, Some(Named::ArrowRight) | Some(Named::Enter)) => match d.state {
                State::Locked => {
                    self.focus = Focus::DrivePassword;
                    self.drive_pw.clear();
                    Some(text_input::focus(self.input_id.clone()))
                }
                State::Closed => Some(self.open_drive()),
                State::Open(_) => {
                    if n > 0 {
                        self.focus = Focus::Grid;
                        self.sel = 0;
                    }
                    Some(Task::none())
                }
            },
            Focus::Grid if !self.picking() => {
                if ch == "c" {
                    if let Some(it) = self.current_item() {
                        // a copy into Downloads: Library then asks where it belongs
                        let dl = home().join("Downloads");
                        let _ = std::fs::create_dir_all(&dl);
                        let mut target = dl.join(&it.name);
                        let mut k = 2;
                        while target.exists() {
                            target = dl.join(format!("{k}-{}", it.name));
                            k += 1;
                        }
                        self.note = Some(match std::fs::copy(&it.path, &target) {
                            Ok(_) => format!("{} copied: it waits in \"Needs an answer\".", it.name),
                            Err(e) => format!("Could not copy: {e}"),
                        });
                    }
                    return Some(Task::none());
                }
                match named {
                    Some(Named::Enter) => {
                        if let Some(it) = self.current_item() {
                            open(&it.path);
                        }
                        Some(Task::none())
                    }
                    // nothing on a drive is deleted or moved from here
                    Some(Named::Delete) => Some(Task::none()),
                    _ if ch == "m" => Some(Task::none()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn open_drive(&mut self) -> Task<Message> {
        let Some(View::Drive(id)) = self.views.get(self.view).cloned() else { return Task::none() };
        let Some(d) = self.drive(&id) else { return Task::none() };
        let pw = if d.state == crate::drives::State::Locked {
            if self.drive_pw.is_empty() {
                return Task::none();
            }
            Some(self.drive_pw.clone())
        } else {
            None
        };
        self.drive_busy = true;
        self.note = Some(format!("Opening {}…", d.name));
        let name = d.name.clone();
        Task::perform(crate::drives::open(d, pw), move |r| cosmic::Action::App(Message::DriveOpened(name.clone(), r)))
    }

    fn drive_panel(&self, view: &View) -> Option<Element<'_, Message>> {
        use crate::drives::{State, size_label};
        let View::Drive(id) = view else { return None };
        let d = self.drive(id)?;
        let mut col = widget::Column::new()
            .spacing(12)
            .width(Length::Fixed(420.0))
            .push(text("DRIVE", MONO, 14.0, YELLOW))
            .push(text(short(&d.name, 30), SANS_BOLD, 28.0, TEXT))
            .push(text(format!("{} · {}{}", size_label(d.size), d.kind, if d.encrypted { " · ENCRYPTED" } else { "" }), MONO, 14.0, TEXT_2))
            .push(space(1.0, 8.0));
        match d.state {
            State::Locked if self.focus == Focus::DrivePassword => {
                col = col.push(text("Its password", SANS_BOLD, 20.0, TEXT)).push(
                    text_input::text_input("Password", self.drive_pw.as_str())
                        .id(self.input_id.clone())
                        .password()
                        .on_input(Message::DrivePassword)
                        .on_submit(|_| Message::DriveSubmit)
                        .style(field_style())
                        .size(21)
                        .padding([14, 18]),
                );
            }
            State::Locked => col = col.push(text("Locked. Press → to type its password.", SANS, 19.0, TEXT_2)),
            State::Closed => col = col.push(text("Press → to open it.", SANS, 19.0, TEXT_2)),
            State::Open(_) => {
                col = col.push(text("Open. C copies a file into Library; E closes the drive so it can be taken out.", SANS, 18.0, TEXT_2));
            }
        }
        Some(fill(col.padding(30), PANEL).into())
    }

    fn picking(&self) -> bool {
        self.mode != Mode::Browse
    }

    fn heading(&self) -> &'static str {
        match self.mode {
            Mode::Browse => "Library",
            Mode::Open { folders: true, .. } => "Choose",
            Mode::Open { .. } => "Open",
            Mode::Save { .. } => "Save",
        }
    }

    /// Keys that mean something else in a picker. None: the usual meaning.
    fn pick_key(&mut self, named: Option<keyboard::key::Named>, ch: &str, view: &View, n: usize) -> Option<Task<Message>> {
        use keyboard::key::Named;
        let save = matches!(self.mode, Mode::Save { .. });
        let folders = matches!(self.mode, Mode::Open { folders: true, .. });
        let multiple = matches!(self.mode, Mode::Open { multiple: true, .. });
        match self.focus {
            Focus::Side => match named {
                Some(Named::Escape) => Some(finish(None)),
                Some(Named::ArrowRight) | Some(Named::Enter) => {
                    if folders {
                        if let View::Collection(c) = view {
                            return Some(finish(Some(vec![library_dir().join(c)])));
                        }
                        return Some(Task::none());
                    }
                    if save {
                        if matches!(view, View::Collection(_)) {
                            self.focus = Focus::SaveName;
                            self.replace_armed = false;
                            return Some(text_input::focus(self.input_id.clone()));
                        }
                        return Some(Task::none());
                    }
                    if n > 0 {
                        self.focus = Focus::Grid;
                    }
                    Some(Task::none())
                }
                _ => None,
            },
            Focus::SaveName => {
                if named == Some(Named::Escape) {
                    self.focus = Focus::Side;
                }
                Some(Task::none())
            }
            Focus::Grid if save || folders => {
                self.focus = Focus::Side;
                Some(Task::none())
            }
            Focus::Grid => match named {
                Some(Named::Enter) => {
                    let mut chosen = std::mem::take(&mut self.marked);
                    if chosen.is_empty() {
                        chosen.extend(self.current_item().map(|i| i.path));
                    }
                    if chosen.is_empty() {
                        return Some(Task::none());
                    }
                    Some(finish(Some(chosen)))
                }
                _ if multiple && ch == " " => {
                    if let Some(it) = self.current_item() {
                        if let Some(p) = self.marked.iter().position(|m| *m == it.path) {
                            self.marked.remove(p);
                        } else {
                            self.marked.push(it.path);
                        }
                    }
                    Some(Task::none())
                }
                // no moving or deleting from inside another app's window
                Some(Named::Delete) => Some(Task::none()),
                _ if ch == "m" => Some(Task::none()),
                _ => None,
            },
            _ => {
                self.focus = Focus::Side;
                Some(Task::none())
            }
        }
    }

    fn save_panel(&self, view: &View) -> Option<Element<'_, Message>> {
        if !matches!(self.mode, Mode::Save { .. }) {
            return None;
        }
        let place = match view {
            View::Collection(c) => c.clone(),
            _ => return None,
        };
        let naming = self.focus == Focus::SaveName;
        let mut col = widget::Column::new()
            .spacing(14)
            .width(Length::Fixed(440.0))
            .push(text("SAVE INTO", MONO, 14.0, YELLOW))
            .push(text(place, SANS_BOLD, 30.0, TEXT))
            .push(space(1.0, 8.0))
            .push(text("NAME", MONO, 14.0, TEXT_2));
        if naming {
            col = col.push(
                text_input::text_input("File name", self.save_name.as_str())
                    .id(self.input_id.clone())
                    .on_input(Message::SaveName)
                    .on_submit(|_| Message::SaveSubmit)
                    .style(field_style())
                    .size(21)
                    .padding([14, 18]),
            );
        } else {
            col = col.push(fill(text(if self.save_name.is_empty() { "—".to_string() } else { self.save_name.clone() }, SANS_BOLD, 21.0, TEXT), PANEL_2).padding([14, 18]).width(Length::Fill));
        }
        if self.replace_armed {
            col = col.push(text("A file with this name is already there. ENTER again replaces it.", SANS, 17.0, RED));
        }
        Some(fill(col.padding(30), PANEL).into())
    }

    fn save_here(&mut self) -> Task<Message> {
        let Some(View::Collection(c)) = self.views.get(self.view).cloned() else { return Task::none() };
        let name = self.save_name.trim().replace('/', "-");
        if name.is_empty() || name.starts_with('.') {
            self.note = Some("Give it a name first.".into());
            return Task::none();
        }
        let dir = library_dir().join(&c);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(&name);
        if path.exists() && !self.replace_armed {
            self.replace_armed = true;
            return Task::none();
        }
        finish(Some(vec![path]))
    }

    fn ask_new_name(&mut self) -> Task<Message> {
        self.focus = Focus::NewName;
        self.new_name.clear();
        text_input::focus(self.input_id.clone())
    }

    fn create_and_file(&mut self) -> Task<Message> {
        let name = self.new_name.trim().replace('/', "-");
        if name.is_empty() || name.starts_with('.') {
            return Task::none();
        }
        let _ = std::fs::create_dir_all(library_dir().join(&name));
        self.collections.push(name.clone());
        if let Some(it) = self.current_item() {
            return self.answer(&it, &name);
        }
        Task::none()
    }

    fn answer(&mut self, it: &Item, collection: &str) -> Task<Message> {
        match file_into(&it.path, collection) {
            Ok(_) => {
                if self.always {
                    let ext = ext_of(&it.path);
                    if !ext.is_empty() {
                        self.rules.insert(ext, collection.to_string());
                        save_rules(&self.rules);
                    }
                }
                self.note = Some(format!("{} → {}", it.name, collection));
            }
            Err(e) => self.note = Some(format!("Could not move it: {e}")),
        }
        self.always = false;
        self.scan();
        let view = self.views[self.view].clone();
        let left = self.items_in(&view).len();
        self.focus = if left == 0 { Focus::Side } else if view == View::Inbox { Focus::Ask(0) } else { Focus::Grid };
        Task::none()
    }
}

/// Close the picker with what was chosen.
fn finish(chosen: Option<Vec<PathBuf>>) -> Task<Message> {
    if let Ok(mut p) = PICKED.lock() {
        *p = chosen;
    }
    cosmic::iced::exit()
}

fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, depth + 1, f);
        } else if p.is_file() {
            f(&p);
        }
    }
}

fn item(p: &Path, collection: Option<String>) -> Item {
    Item {
        path: p.to_path_buf(),
        name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        kind: Kind::of(p),
        collection,
        modified: std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH),
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Moves a file into a collection without ever overwriting another file.
fn file_into(path: &Path, collection: &str) -> std::io::Result<PathBuf> {
    let dir = library_dir().join(collection);
    std::fs::create_dir_all(&dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut target = dir.join(&name);
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    if std::fs::rename(path, &target).is_err() {
        std::fs::copy(path, &target)?;
        std::fs::remove_file(path)?;
    }
    Ok(target)
}

/// Into the freedesktop bin, so it can be restored.
fn trash(path: &Path) -> std::io::Result<()> {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    let bin = data.join("Trash");
    std::fs::create_dir_all(bin.join("files"))?;
    std::fs::create_dir_all(bin.join("info"))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut target = name.clone();
    let mut n = 2;
    while bin.join("files").join(&target).exists() {
        target = format!("{n}-{name}");
        n += 1;
    }
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    std::fs::write(
        bin.join("info").join(format!("{target}.trashinfo")),
        format!("[Trash Info]\nPath={}\nDeletionDate={now}\n", url_path(path)),
    )?;
    std::fs::rename(path, bin.join("files").join(target))
}

/// The bin wants the original place written like a URL path.
fn url_path(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn open(path: &Path) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn load_rules() -> HashMap<String, String> {
    std::fs::read_to_string(rules_path())
        .map(|s| {
            s.lines()
                .filter_map(|l| l.split_once('=').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn save_rules(r: &HashMap<String, String>) {
    let p = rules_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut lines: Vec<String> = r.iter().map(|(a, b)| format!("{a}={b}")).collect();
    lines.sort();
    let _ = std::fs::write(p, lines.join("\n") + "\n");
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

fn when(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    let now = chrono::Local::now();
    if dt.date_naive() == now.date_naive() {
        dt.format("TODAY %H:%M").to_string()
    } else if (now.date_naive() - dt.date_naive()).num_days() < 7 {
        dt.format("%a").to_string().to_uppercase()
    } else {
        dt.format("%-d %b").to_string().to_uppercase()
    }
}
