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
    cosmic::app::run::<Library>(settings, ())
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
}

#[derive(Clone, Debug)]
pub enum Message {
    Rescan,
    Key(keyboard::Key, event::Status),
    NewName(String),
    NewSubmit,
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
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}
fn library_dir() -> PathBuf {
    home().join("Library")
}
fn inbox_dirs() -> Vec<PathBuf> {
    vec![home().join("Downloads"), home().join("Desktop")]
}
fn rules_path() -> PathBuf {
    crate::apps::config_dir().join("chlorum/library-rules")
}

impl Application for Library {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "chlorum-library";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, _: ()) -> (Self, Task<Message>) {
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
        };
        l.scan();
        l.view = if l.inbox().is_empty() { 1 } else { 0 };
        (l, Task::none())
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
                if self.focus != Focus::NewName {
                    self.scan();
                }
            }
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
                .push(text("Library", SANS_BOLD, 34.0, TEXT))
                .spacing(16)
                .align_y(Alignment::Center),
        );
        side = side.push(space(1.0, 26.0));
        for (i, v) in self.views.iter().enumerate() {
            if matches!(v, View::Kind(Kind::Photo)) {
                side = side.push(space(1.0, 10.0)).push(text("BY KIND", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
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
                        .push(text(v.label(), SANS_BOLD, 21.0, fg))
                        .push(hfill())
                        .push(text(count.to_string(), MONO, 15.0, fg))
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
                    "{} · {}",
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
                    if on { YELLOW } else { PANEL },
                )
                .width(Length::Fixed(250.0));
                row = row.push(bracket(card, if on && self.focus == Focus::Grid { 1.0 } else { 0.0 }, 8.0));
            }
            grid = grid.push(row);
        }
        if items.is_empty() {
            let msg = match view {
                View::Inbox => "Nothing waiting. New downloads appear here.",
                _ => "Nothing here yet.",
            };
            grid = grid.push(text(msg, SANS, 22.0, TEXT_2));
        }
        let main = widget::Column::new()
            .push(text(view.label(), SANS_BOLD, 44.0, TEXT))
            .push(space(1.0, 26.0))
            .push(widget::scrollable(grid).height(Length::Fill))
            .width(Length::Fill);

        let mut row = widget::Row::new().push(side).push(space(56.0, 1.0)).push(main).height(Length::Fill);
        if let Some(panel) = self.ask_panel(&items) {
            row = row.push(space(30.0, 1.0)).push(panel);
        }

        let mut h: Vec<(&str, &str, bool)> = match self.focus {
            Focus::Side => vec![("↑ ↓", "View", false), ("→", "Files", true)],
            Focus::Grid => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("M", "Move to…", false), ("DEL", "Delete", false)],
            Focus::Ask(_) => vec![("↑ ↓", "Choose", false), ("1–9", "Pick", false), ("ENTER", "File it", true), ("A", "Always", false), ("ESC", "Back", false)],
            Focus::NewName => vec![("ENTER", "Create", true), ("ESC", "Back", false)],
            Focus::Confirm => vec![("ENTER", "Delete", true), ("ESC", "Keep", false)],
        };
        h.push(("SUPER", "System menu", false));
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
        let mut views = vec![View::Inbox, View::Recent];
        views.extend(collections.into_iter().map(View::Collection));
        views.extend(Kind::VIEWS.iter().map(|k| View::Kind(*k)));
        self.views = views;
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
        self.items
            .iter()
            .filter(|i| match v {
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
        match self.focus {
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
