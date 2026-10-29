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
use std::time::{Duration, Instant, SystemTime};

pub fn run() -> cosmic::iced::Result {
    let settings = Settings::default().scale_factor(crate::ui::screen_zoom())
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
    let settings = Settings::default().scale_factor(crate::ui::screen_zoom())
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

const DEFAULT_COLLECTIONS: &[&str] = &["Documents", "Photos", "Music", "Videos"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Photo,
    Video,
    Music,
    Document,
    Code,
    /// a Workshop project: a folder of code with its own tools
    Project,
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
            Kind::Project => "Projects",
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
            Kind::Project => "PROJECT",
            Kind::Archive => "ZIP",
            Kind::Other => "FILE",
        }
    }
}

/// The order files are shown in (S changes it, everywhere but Recent).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sort {
    Newest,
    Oldest,
    Name,
    Size,
    Kind,
}

impl Sort {
    const ALL: [Sort; 5] = [Sort::Newest, Sort::Oldest, Sort::Name, Sort::Size, Sort::Kind];
    fn key(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::Name => "name",
            Sort::Size => "size",
            Sort::Kind => "kind",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Sort::Newest => "Newest first",
            Sort::Oldest => "Oldest first",
            Sort::Name => "By name",
            Sort::Size => "Largest first",
            Sort::Kind => "By kind",
        }
    }
    fn next(self) -> Sort {
        let i = Sort::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Sort::ALL[(i + 1) % Sort::ALL.len()]
    }
    fn path() -> PathBuf {
        crate::apps::config_dir().join("chlorum/library-sort")
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
    fn apply(self, items: &mut [&Item]) {
        match self {
            Sort::Newest => items.sort_by(|a, b| b.modified.cmp(&a.modified)),
            Sort::Oldest => items.sort_by(|a, b| a.modified.cmp(&b.modified)),
            Sort::Name => items.sort_by_cached_key(|i| i.name.to_lowercase()),
            Sort::Size => items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
            Sort::Kind => items.sort_by(|a, b| a.kind.label().cmp(b.kind.label()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
        }
    }
}

/// Which files are shown (F changes it, in every view): all of them, or
/// only one kind.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Show {
    All,
    Photos,
    Videos,
    Audio,
    Text,
    Technical,
    Office,
    Other,
}

impl Show {
    const ALL: [Show; 8] = [Show::All, Show::Photos, Show::Videos, Show::Audio, Show::Text, Show::Technical, Show::Office, Show::Other];
    fn key(self) -> &'static str {
        match self {
            Show::All => "all",
            Show::Photos => "photos",
            Show::Videos => "videos",
            Show::Audio => "audio",
            Show::Text => "text",
            Show::Technical => "technical",
            Show::Office => "office",
            Show::Other => "other",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Show::All => "All files",
            Show::Photos => "Only photos",
            Show::Videos => "Only videos",
            Show::Audio => "Only audio",
            Show::Text => "Only text",
            Show::Technical => "Only technical files",
            Show::Office => "Only office files",
            Show::Other => "Only other files",
        }
    }
    /// Is this file of the kind shown? (Folders show only with All, except
    /// Workshop projects, which are technical.)
    fn keeps(self, i: &Item) -> bool {
        let ext = i.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let e = ext.as_str();
        match self {
            Show::All => true,
            Show::Other => ![Show::Photos, Show::Videos, Show::Audio, Show::Text, Show::Technical, Show::Office].iter().any(|k| k.keeps(i)),
            Show::Photos => i.kind == Kind::Photo,
            Show::Videos => i.kind == Kind::Video,
            Show::Audio => i.kind == Kind::Music,
            Show::Text => matches!(e, "txt" | "md" | "markdown" | "rst" | "org" | "log" | "tex" | "rtf" | "nfo"),
            Show::Office => matches!(
                e,
                "pdf" | "epub" | "doc" | "docx" | "odt" | "ott" | "xls" | "xlsx" | "ods" | "ots" | "csv" | "tsv" | "ppt" | "pptx" | "odp" | "otp" | "odg" | "pages" | "numbers" | "key"
            ),
            Show::Technical => {
                matches!(i.kind, Kind::Code | Kind::Project | Kind::Archive)
                    || matches!(
                        e,
                        "rs" | "py" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "c" | "h" | "cc" | "cpp" | "hpp" | "go" | "java" | "kt" | "swift" | "rb" | "php" | "lua" | "zig" | "hs"
                            | "nix" | "sh" | "bash" | "fish" | "ps1" | "bat" | "json" | "toml" | "yaml" | "yml" | "xml" | "ini" | "conf" | "cfg" | "env" | "html" | "css" | "scss" | "sql"
                            | "kdl" | "lock" | "diff" | "patch" | "iso" | "img" | "deb" | "rpm" | "appimage" | "exe" | "msi" | "bin" | "elf" | "so" | "o" | "a" | "wasm" | "ipynb"
                    )
                    || matches!(i.name.as_str(), "Makefile" | "Dockerfile" | "CMakeLists.txt" | "justfile" | "flake.lock")
            }
        }
    }
}

/// When a file was last changed, for the filter.
#[derive(Clone, Copy, Debug, PartialEq)]
enum When {
    Any,
    Today,
    Week,
    Month,
    Year,
    LastYear,
    Between,
}

impl When {
    const ALL: [When; 7] = [When::Any, When::Today, When::Week, When::Month, When::Year, When::LastYear, When::Between];
    fn key(self) -> &'static str {
        match self {
            When::Any => "any",
            When::Today => "today",
            When::Week => "week",
            When::Month => "month",
            When::Year => "year",
            When::LastYear => "lastyear",
            When::Between => "between",
        }
    }
    fn label(self) -> &'static str {
        match self {
            When::Any => "Any time",
            When::Today => "Today",
            When::Week => "Last 7 days",
            When::Month => "Last 30 days",
            When::Year => "This year",
            When::LastYear => "Last year",
            When::Between => "Between two dates",
        }
    }
}

/// What Library shows: some kinds of files (none chosen: all), from some
/// time, in some order. Set in the filter panel (F); kept between uses.
#[derive(Clone, Debug, PartialEq)]
struct Filter {
    kinds: Vec<Show>,
    when: When,
    /// YYYY-MM-DD, typed in the panel
    from: String,
    to: String,
}

impl Default for Filter {
    fn default() -> Self {
        Filter { kinds: vec![], when: When::Any, from: String::new(), to: String::new() }
    }
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

impl Filter {
    fn path() -> PathBuf {
        crate::apps::config_dir().join("chlorum/library-filter")
    }
    fn load() -> Filter {
        let mut f = Filter::default();
        for l in std::fs::read_to_string(Filter::path()).unwrap_or_default().lines() {
            let Some((k, v)) = l.split_once(' ') else { continue };
            match k {
                "kinds" => f.kinds = v.split(',').filter_map(|x| Show::ALL.into_iter().find(|s| s.key() == x && *s != Show::All)).collect(),
                "when" => f.when = When::ALL.into_iter().find(|w| w.key() == v).unwrap_or(When::Any),
                "from" => f.from = v.to_string(),
                "to" => f.to = v.to_string(),
                _ => {}
            }
        }
        f
    }
    fn save(&self) {
        let p = Filter::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let kinds: Vec<&str> = self.kinds.iter().map(|k| k.key()).collect();
        let _ = std::fs::write(p, format!("kinds {}\nwhen {}\nfrom {}\nto {}\n", kinds.join(","), self.when.key(), self.from, self.to));
    }
    fn is_all(&self) -> bool {
        self.kinds.is_empty() && self.when == When::Any
    }
    fn keeps(&self, i: &Item) -> bool {
        if !self.kinds.is_empty() && !self.kinds.iter().any(|k| k.keeps(i)) {
            return false;
        }
        let dt: chrono::DateTime<chrono::Local> = i.modified.into();
        let day = dt.date_naive();
        let today = chrono::Local::now().date_naive();
        use chrono::Datelike;
        match self.when {
            When::Any => true,
            When::Today => day == today,
            When::Week => (today - day).num_days() < 7,
            When::Month => (today - day).num_days() < 30,
            When::Year => day.year() == today.year(),
            When::LastYear => day.year() == today.year() - 1,
            When::Between => {
                let from = parse_date(&self.from);
                let to = parse_date(&self.to);
                from.is_none_or(|f| day >= f) && to.is_none_or(|t| day <= t)
            }
        }
    }
    /// "Photos, Videos · Last 30 days"
    fn summary(&self) -> String {
        let kinds = if self.kinds.is_empty() {
            "All kinds".to_string()
        } else {
            self.kinds.iter().map(|k| k.label().trim_start_matches("Only ").to_string()).map(|s| {
                let mut c = s.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }).collect::<Vec<_>>().join(", ")
        };
        let when = match self.when {
            When::Between => format!("{} – {}", if self.from.is_empty() { "…" } else { &self.from }, if self.to.is_empty() { "…" } else { &self.to }),
            w => w.label().to_string(),
        };
        format!("{kinds} · {when}")
    }
}

/// A line of the filter panel.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FilterRow {
    Kind(Show),
    When,
    From,
    To,
    Order,
    Clear,
}

#[derive(Clone, Debug)]
pub struct Item {
    path: PathBuf,
    name: String,
    kind: Kind,
    /// collection it is filed in; None while it waits for an answer
    collection: Option<String>,
    modified: SystemTime,
    /// bytes (0 for a folder)
    size: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum View {
    /// a flash drive or other removable disk (its UDisks id)
    Drive(String),
    Inbox,
    Recent,
    Collection(String),
    Kind(Kind),
    /// what was deleted, for 30 days
    Bin,
    /// a phone or camera plugged in by USB (its GVFS address)
    Phone(String),
}

impl View {
    fn label(&self) -> String {
        match self {
            View::Inbox => "Needs an answer".into(),
            View::Recent => "Recent".into(),
            View::Collection(c) => c.clone(),
            View::Kind(k) => k.label().into(),
            View::Drive(_) => "Drive".into(),
            View::Bin => "Bin".into(),
            View::Phone(_) => "Phone".into(),
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
    /// opening an encrypted drive with a security key: typing the key's PIN
    KeyPin,
    /// adding a security key to a drive: its password, then the key's PIN
    AddPassword,
    AddPin,
    /// naming a new Workshop project; the number is the language
    NewProject(usize),
    /// the filter panel, on one of its lines
    Filter(usize),
    /// typing in the search field
    Search,
    /// typing a file's new name
    Rename,
    /// choosing the drive to copy files to
    SendTo(usize),
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
    KeyPin(String),
    KeySubmit,
    AddNext,
    AddSubmit,
    KeyAdded(String, Result<(), String>),
    ProjectName(String),
    ProjectSubmit,
    Ejected(String, Result<(), String>),
    Resized(iced::Size),
    Query(String),
    RenameInput(String),
    /// files copied (to a drive, or from one into Library)
    Copied(Result<String, String>),
    /// the disk was read in the background
    Scanned(Box<Option<Gathered>>),
    /// a phone's photos and videos were read (its address, the files)
    PhoneLoaded(String, Result<Vec<Item>, String>),
    RenameSubmit,
    QuerySubmit,
    /// a file was clicked (twice quickly: opened)
    Click(usize),
    Scroll(f32),
    /// a moment passed: new pictures of files may be ready
    Frame,
}

pub struct Library {
    core: Core,
    /// the window's size: how many files fit on a row and on the screen
    size: iced::Size,
    thumbs: crate::thumbs::Maker,
    last_click: Option<(usize, Instant)>,
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
    /// the file grid, so it can scroll to follow the selection
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
    /// the security key's PIN, only while it is being typed
    key_pin: String,
    drive_busy: bool,
    project_name: String,
    sort: Sort,
    filter: Filter,
    /// reading the disk in the background
    scanning: bool,
    /// the drive whose files are read (they are not read again each time)
    drive_read_for: Option<String>,
    /// what is in the bin (as files there)
    bin_items: Vec<Item>,
    phones: Vec<crate::phones::Phone>,
    /// the phone being looked at and its files
    phone_items: Option<(String, Vec<Item>)>,
    rename: String,
    /// typed in the search field (/): files from everywhere whose name has it
    query: String,
    search_id: widget::Id,
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
        // Workshop's projects always have their place
        let _ = std::fs::create_dir_all(lib.join("Projects"));
        let mut l = Library {
            core,
            size: iced::Size::new(1600.0, 1000.0),
            thumbs: crate::thumbs::Maker::new(),
            last_click: None,
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
            key_pin: String::new(),
            drive_busy: false,
            project_name: String::new(),
            sort: Sort::load(),
            filter: Filter::load(),
            scanning: false,
            drive_read_for: None,
            bin_items: vec![],
            phones: vec![],
            phone_items: None,
            rename: String::new(),
            query: String::new(),
            search_id: widget::Id::unique(),
        };
        l.scan();
        l.view = if l.picking() { 0 } else if l.inbox().is_empty() { 1 } else { 0 };
        // the window's real size, for how many files fit on a row
        let size = match l.core.main_window_id() {
            Some(id) => cosmic::iced::window::size(id).map(|sz| cosmic::Action::App(Message::Resized(sz))),
            None => Task::none(),
        };
        (l, Task::batch([size, Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d)))]))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_secs(3)).map(|_| Message::Rescan),
            if self.thumbs.busy() { iced::time::every(Duration::from_millis(500)).map(|_| Message::Frame) } else { Subscription::none() },
            event::listen_with(|e, status, _| match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => Some(Message::Key(key, status)),
                iced::Event::Window(iced::window::Event::Resized(size)) => Some(Message::Resized(size)),
                iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => Some(Message::Scroll(match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => y,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
                })),
                _ => None,
            }),
        ])
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Resized(sz) => self.size = sz,
            Message::RenameInput(v) => self.rename = v,
            Message::PhoneLoaded(uri, r) => {
                self.drive_busy = false;
                match r {
                    Ok(items) => {
                        self.note = Some(if items.is_empty() { "No photos or videos on it.".into() } else { format!("{} photos, videos and files. SPACE marks, C copies into Library.", items.len()) });
                        self.phone_items = Some((uri, items));
                        self.focus = Focus::Grid;
                        self.sel = 0;
                    }
                    Err(e) => self.note = Some(e),
                }
            }
            Message::Copied(r) => {
                self.drive_busy = false;
                self.note = Some(match r {
                    Ok(m) => m,
                    Err(e) => e,
                });
                self.marked.clear();
                self.scan();
            }
            Message::RenameSubmit => {
                if let Some(it) = self.current_item() {
                    let new = self.rename.trim().to_string();
                    let to = it.path.with_file_name(&new);
                    self.note = Some(if new.is_empty() || new.contains('/') || new == "." || new == ".." {
                        "A name cannot be empty or have / in it.".to_string()
                    } else if new == it.name {
                        "The name is the same.".to_string()
                    } else if to.exists() {
                        format!("There is already a file called {new} there.")
                    } else {
                        match std::fs::rename(&it.path, &to) {
                            Ok(()) => format!("Now called {new}."),
                            Err(e) => format!("Could not rename: {e}"),
                        }
                    });
                    self.focus = Focus::Grid;
                    self.scan();
                    // stay on the file under its new name
                    let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
                    if let Some(i) = self.items_in(&view).iter().position(|x| x.path == to) {
                        self.sel = i;
                    }
                }
            }
            Message::Query(q) => {
                self.query = q;
                self.sel = 0;
            }
            Message::QuerySubmit => {
                self.sel = 0;
                self.focus = Focus::Grid;
            }
            Message::Frame => {
                let _ = self.thumbs.take_done();
            }
            Message::Click(i) => {
                let double = self.last_click.is_some_and(|(j, t)| j == i && t.elapsed() < Duration::from_millis(450));
                self.last_click = Some((i, Instant::now()));
                self.sel = i;
                if !matches!(self.focus, Focus::Grid) && matches!(self.focus, Focus::Side | Focus::Ask(_) | Focus::Confirm) {
                    self.focus = Focus::Grid;
                }
                if double {
                    if let Some(it) = self.current_item() {
                        if it.collection.is_none() && !matches!(self.views.get(self.view), Some(View::Drive(_) | View::Phone(_))) {
                            self.focus = Focus::Ask(0);
                        } else {
                            self.opened(&it.path);
                        }
                    }
                }
            }
            Message::Scroll(y) => {
                if matches!(self.focus, Focus::Grid | Focus::Side) {
                    let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
                    let n = self.items_in(&view).len();
                    let cols = self.cols();
                    self.focus = Focus::Grid;
                    if y < 0.0 && n > 0 {
                        self.sel = (self.sel + cols).min(n - 1);
                    } else if y > 0.0 {
                        self.sel = self.sel.saturating_sub(cols);
                    }
                }
            }
            Message::Rescan => {
                let read = if !matches!(self.focus, Focus::NewName | Focus::SaveName | Focus::DrivePassword | Focus::KeyPin | Focus::AddPassword | Focus::AddPin | Focus::NewProject(_) | Focus::Rename | Focus::Confirm | Focus::Ask(_) | Focus::SendTo(_)) {
                    self.scan_later()
                } else {
                    Task::none()
                };
                return Task::batch([read, Task::perform(crate::drives::list(), |d| cosmic::Action::App(Message::Drives(d)))]);
            }
            Message::Scanned(g) => {
                self.scanning = false;
                if let Some(g) = *g {
                    self.apply(g);
                }
            }
            Message::Drives(d) => {
                if d != self.drives {
                    self.drives = d;
                    // a drive opened or closed: its files are read in the
                    // background (a big one takes a while)
                    self.drive_read_for = None;
                    return self.scan_later();
                }
            }
            Message::DrivePassword(s) => self.drive_pw = s,
            Message::ProjectName(s) => self.project_name = s,
            Message::ProjectSubmit => return self.make_project(),
            Message::DriveSubmit => return self.open_drive(),
            Message::KeyPin(s) => self.key_pin = s,
            Message::KeySubmit => return self.open_with_key(),
            Message::AddNext => {
                if !self.drive_pw.is_empty() {
                    self.focus = Focus::AddPin;
                    self.key_pin.clear();
                    return text_input::focus(self.input_id.clone());
                }
            }
            Message::AddSubmit => return self.add_key(),
            Message::KeyAdded(name, r) => {
                self.drive_busy = false;
                self.drive_pw.clear();
                self.key_pin.clear();
                self.focus = Focus::Side;
                self.note = Some(match r {
                    Ok(()) => format!("{name} now also opens with your security key: press K."),
                    Err(e) => e,
                });
            }
            Message::DriveOpened(name, r) => {
                self.drive_busy = false;
                self.drive_pw.clear();
                self.key_pin.clear();
                match r {
                    Ok(_) => {
                        self.note = Some(format!("{name} is open."));
                        self.focus = Focus::Side;
                    }
                    Err(e) => {
                        self.note = Some(e);
                        if matches!(self.focus, Focus::DrivePassword | Focus::KeyPin) {
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
            Message::Key(k, status) => {
                let before = (self.view, self.sel);
                let t = self.key(k, status);
                if self.view != before.0 && self.mode == Mode::Browse {
                    self.marked.clear();
                }
                if (self.view, self.sel) != before {
                    return Task::batch([t, self.follow()]);
                }
                return t;
            }
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
            if matches!(v, View::Drive(_) | View::Phone(_)) && !matches!(self.views.get(i.wrapping_sub(1)), Some(View::Drive(_) | View::Phone(_))) {
                side = side.push(space(1.0, 10.0)).push(text("DRIVES", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
            }
            if matches!(v, View::Collection(_)) && !matches!(self.views.get(i.wrapping_sub(1)), Some(View::Collection(_))) {
                side = side.push(space(1.0, 10.0)).push(text("COLLECTIONS", MONO, 13.0, MUTED)).push(space(1.0, 6.0));
            }
            let on = i == self.view;
            let active = on && self.focus == Focus::Side;
            // what each view holds, whatever is being searched for
            let count = self.count_in(v);
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
        let cols = self.cols();
        let card_w = self.card_width();
        let thumb_h = (card_w * 0.66).round();
        // the rows around the chosen file: it never goes off screen
        let fit = self.rows_shown(thumb_h);
        let rows = items.len().div_ceil(cols);
        let first = (self.sel / cols).saturating_sub(fit.saturating_sub(1) / 3).min(rows.saturating_sub(fit));
        for (r, chunk) in items.chunks(cols).enumerate().skip(first).take(fit) {
            let mut row = widget::Row::new().spacing(22);
            for (c, it) in chunk.iter().enumerate() {
                let i = r * cols + c;
                let on = i == self.sel && matches!(self.focus, Focus::Grid | Focus::Ask(_) | Focus::NewName | Focus::Confirm);
                let marked = self.marked.contains(&it.path);
                // the file itself when it can be pictured, its kind otherwise
                let picture = self.thumbs.get(&it.path, it.size, it.modified);
                let thumb: Element<'_, Message> = match picture {
                    Some(p) => widget::image(widget::image::Handle::from_path(p)).content_fit(cosmic::iced::ContentFit::Cover).width(Length::Fill).height(Length::Fixed(thumb_h)).into(),
                    None => fill(
                        widget::container(fill(text(it.kind.badge(), MONO_BOLD, 24.0, BLACK), YELLOW).padding([4, 12]))
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center),
                        PANEL_2,
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(thumb_h))
                    .into(),
                };
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
                .width(Length::Fixed(card_w));
                row = row.push(widget::mouse_area(bracket(card, if on && self.focus == Focus::Grid { 1.0 } else { 0.0 }, 8.0)).on_press(Message::Click(i)));
            }
            grid = grid.push(row);
        }
        if items.is_empty() {
            let msg = match (&self.mode, &view) {
                (Mode::Save { .. }, _) => "Choose where it goes, then press → to name it.",
                (_, View::Inbox) => "Nothing waiting. New downloads appear here.",
                _ if !self.query.trim().is_empty() && !self.filter.is_all() => "Nothing called like that passes the filter. F changes the filter, Esc clears the search.",
                _ if !self.query.trim().is_empty() => "No file is called like that. Esc clears the search.",
                _ if !self.filter.is_all() => "Nothing here passes the filter. F changes it.",
                _ => "Nothing here yet.",
            };
            grid = grid.push(text(msg, SANS, 22.0, TEXT_2));
        }
        let order = if view == View::Recent && self.query.is_empty() { "Newest first".to_string() } else { self.sort.label().to_string() };
        let title = if self.query.trim().is_empty() { self.view_label(&view) } else { format!("Results for “{}”", self.query.trim()) };
        let search = text_input::text_input("Search all files ( / )", self.query.as_str())
            .id(self.search_id.clone())
            .on_input(Message::Query)
            .on_submit(|_| Message::QuerySubmit)
            .size(20)
            .padding([10, 16])
            .width(Length::Fixed(520.0));
        let main = widget::Column::new()
            .push(widget::Row::new().push(text(title, SANS_BOLD, 44.0, TEXT)).push(hfill()).push(bracket(search, if self.focus == Focus::Search { 1.0 } else { 0.0 }, 6.0)).align_y(Alignment::Center))
            .push(space(1.0, 6.0))
            .push(text(
                format!("{} · S      {} · F", order.to_uppercase(), self.filter.summary().to_uppercase()),
                MONO,
                14.0,
                if self.filter.is_all() { TEXT_2 } else { YELLOW },
            ))
            .push(space(1.0, 20.0))
            .push(if self.focus == Focus::Rename {
                widget::Column::new()
                    .push(text("NEW NAME", MONO, 13.0, YELLOW))
                    .push(bracket(
                        text_input::text_input("Name", self.rename.as_str())
                            .id(self.input_id.clone())
                            .on_input(Message::RenameInput)
                            .on_submit(|_| Message::RenameSubmit)
                            .size(22)
                            .padding([12, 16])
                            .width(Length::Fixed(640.0)),
                        1.0,
                        6.0,
                    ))
                    .push(space(1.0, 16.0))
                    .spacing(6)
                    .into()
            } else {
                space(1.0, 1.0)
            })
            .push(widget::container(grid).height(Length::Fill))
            .push(text(if rows > fit { format!("{} of {} · scroll or arrows", (self.sel + 1).min(items.len()), items.len()) } else { String::new() }, MONO, 13.0, TEXT_2))
            .width(Length::Fill);

        let mut row = widget::Row::new().push(side).push(space(56.0, 1.0)).push(main).height(Length::Fill);
        if let Some(panel) = self.filter_panel() {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.send_panel() {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.project_panel() {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.drive_panel(&view) {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.save_panel(&view) {
            row = row.push(space(30.0, 1.0)).push(panel);
        } else if let Some(panel) = self.ask_panel(&items) {
            row = row.push(space(30.0, 1.0)).push(panel);
        }

        let on_drive = matches!(view, View::Drive(_) | View::Phone(_));
        let mut h: Vec<(&str, &str, bool)> = match (&self.mode, self.focus) {
            (_, Focus::DrivePassword) => vec![("ENTER", "Unlock", true), ("ESC", "Back", false)],
            (_, Focus::KeyPin) => vec![("ENTER", "Open with the key", true), ("ESC", "Back", false)],
            (_, Focus::AddPassword) => vec![("ENTER", "Next", true), ("ESC", "Back", false)],
            (_, Focus::AddPin) => vec![("ENTER", "Add the key", true), ("ESC", "Back", false)],
            (_, Focus::NewProject(_)) => vec![("↑ ↓", "Language", false), ("ENTER", "Create", true), ("ESC", "Back", false)],
            (_, Focus::Filter(_)) => vec![("↑ ↓", "Choose", false), ("ENTER", "On · Off", true), ("← →", "Change", false), ("ESC", "Done", false)],
            (_, Focus::Rename) => vec![("ENTER", "Rename", true), ("ESC", "Back", false)],
            (_, Focus::SendTo(_)) => vec![("↑ ↓", "Drive", false), ("ENTER", "Copy there", true), ("ESC", "Back", false)],
            (Mode::Browse, Focus::Grid) if view == View::Bin => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Put back", true), ("DEL", "Delete for good", false)],
            (Mode::Browse, Focus::Confirm) if view == View::Bin => vec![("ENTER", "Delete for good", true), ("ESC", "Keep", false)],
            (_, Focus::Search) => vec![("TYPE", "Name", false), ("ENTER", "Results", true), ("ESC", "Clear", false)],
            (Mode::Browse, Focus::Side | Focus::Grid) if matches!(&view, View::Collection(c) if c == "Projects") => {
                vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open in Typewriter", true), ("N", "New project", false), ("DEL", "Delete", false)]
            }
            (Mode::Browse, Focus::Side) if on_drive => {
                let luks_locked = matches!(&view, View::Drive(id) if self.drive(id).is_some_and(|d| d.kind == "LUKS" && d.state == crate::drives::State::Locked));
                if luks_locked {
                    vec![("↑ ↓", "View", false), ("→", "Password", true), ("K", "Security key", false), ("A", "Add a key", false), ("E", "Take out", false)]
                } else {
                    vec![("↑ ↓", "View", false), ("→", "Open", true), ("E", "Take out", false)]
                }
            }
            (Mode::Browse, Focus::Grid) if on_drive => {
                vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("SPACE", "Mark", false), ("C", "Copy into Library", false), ("F", "Filter", false), ("/", "Search", false), ("E", "Take out", false)]
            }
            (Mode::Open { folders: true, .. }, Focus::Side) => vec![("↑ ↓", "Collection", false), ("ENTER", "Choose this one", true), ("ESC", "Cancel", false)],
            (Mode::Open { .. }, Focus::Side) => vec![("↑ ↓", "View", false), ("→", "Files", true), ("ESC", "Cancel", false)],
            (Mode::Open { multiple: true, .. }, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("SPACE", "Mark", false), ("ENTER", "Open", true), ("ESC", "Back", false)],
            (Mode::Open { .. }, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("ESC", "Back", false)],
            (Mode::Save { .. }, Focus::Side) => vec![("↑ ↓", "Where", false), ("→", "Name it", true), ("ESC", "Cancel", false)],
            (Mode::Save { .. }, _) => vec![("ENTER", "Save", true), ("ESC", "Back", false)],
            (_, Focus::SaveName) => vec![("ENTER", "Save", true), ("ESC", "Back", false)],
            (_, Focus::Side) => vec![("↑ ↓", "View", false), ("→", "Files", true), ("F", "Filter", false), ("/", "Search", false)],
            (_, Focus::Grid) => vec![("← ↑ ↓ →", "Move", false), ("ENTER", "Open", true), ("SPACE", "Mark", false), ("C", "Copy to drive", false), ("M", "Move", false), ("R", "Rename", false), ("F", "Filter", false), ("/", "Search", false), ("DEL", "Delete", false)],
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
    /// Scrolls the grid so the chosen file is on screen. Rows are all the
    /// same height, so scrolling to (row / last row) of the way down always
    /// shows that row, without knowing the window's size.
    fn follow(&self) -> Task<Message> {
        // the grid shows the rows around the chosen file by itself
        Task::none()
    }

    /// Keys on a phone. None: the usual meaning.
    fn phone_key(&mut self, uri: String, named: Option<keyboard::key::Named>, ch: &str, n: usize) -> Option<Task<Message>> {
        use keyboard::key::Named;
        if self.drive_busy {
            return Some(Task::none());
        }
        let loaded = matches!(&self.phone_items, Some((u, _)) if *u == uri);
        match self.focus {
            Focus::Side if matches!(named, Some(Named::ArrowRight) | Some(Named::Enter)) => {
                if loaded && n > 0 {
                    self.focus = Focus::Grid;
                    self.sel = 0;
                    return Some(Task::none());
                }
                let phone = self.phones.iter().find(|p| p.uri == uri).cloned()?;
                self.drive_busy = true;
                self.note = Some(format!("Opening {}… (unlock it if it asks)", phone.name));
                Some(Task::perform(
                    async move {
                        let u = phone.uri.clone();
                        let r = tokio::task::spawn_blocking(move || {
                            let root = crate::phones::open(&phone)?;
                            let mut items = vec![];
                            for d in crate::phones::media_dirs(&root) {
                                walk(&d, 0, &mut |p| {
                                    if items.len() < 20000 {
                                        items.push(item(p, Some("PHONE".into())));
                                    }
                                });
                            }
                            items.sort_by(|a: &Item, b: &Item| b.modified.cmp(&a.modified));
                            Ok::<Vec<Item>, String>(items)
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                        (u, r)
                    },
                    |(u, r)| cosmic::Action::App(Message::PhoneLoaded(u, r)),
                ))
            }
            Focus::Grid => {
                if ch == " " {
                    if let Some(it) = self.current_item() {
                        if let Some(p) = self.marked.iter().position(|m| *m == it.path) {
                            self.marked.remove(p);
                        } else {
                            self.marked.push(it.path);
                        }
                        self.note = Some(format!("{} marked. C copies them into Library.", self.marked.len()));
                    }
                    return Some(Task::none());
                }
                if ch == "c" {
                    let files: Vec<PathBuf> = if self.marked.is_empty() { self.current_item().map(|it| vec![it.path]).unwrap_or_default() } else { self.marked.clone() };
                    if files.is_empty() {
                        return Some(Task::none());
                    }
                    let dl = home().join("Downloads");
                    self.drive_busy = true;
                    self.note = Some(format!("Copying {} from the phone…", if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) }));
                    return Some(Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || copy_all(&files, &dl).map(|n| format!("{n} copied: they wait in \"Needs an answer\".")))
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        |r| cosmic::Action::App(Message::Copied(r)),
                    ));
                }
                match named {
                    Some(Named::Enter) => {
                        if let Some(it) = self.current_item() {
                            self.opened(&it.path);
                        }
                        Some(Task::none())
                    }
                    // nothing on a phone is deleted or moved from here
                    Some(Named::Delete) => Some(Task::none()),
                    _ if ch == "m" || ch == "r" => Some(Task::none()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The drives that are open: (name, where their files are).
    fn open_drives(&self) -> Vec<(String, PathBuf)> {
        self.drives
            .iter()
            .filter_map(|d| match &d.state {
                crate::drives::State::Open(p) => Some((d.name.clone(), p.clone())),
                _ => None,
            })
            .collect()
    }

    fn send_panel(&self) -> Option<Element<'_, Message>> {
        let Focus::SendTo(at) = self.focus else { return None };
        let n = if self.marked.is_empty() { 1 } else { self.marked.len() };
        let mut col = widget::Column::new().spacing(6).width(Length::Fixed(430.0));
        col = col
            .push(text("COPY TO A DRIVE", MONO, 14.0, YELLOW))
            .push(text(if n == 1 { "1 file".to_string() } else { format!("{n} files") }, SANS_BOLD, 26.0, TEXT))
            .push(text("A copy goes to the drive; the file stays here too.", SANS, 15.0, TEXT_2))
            .push(space(1.0, 6.0));
        for (i, (name, root)) in self.open_drives().into_iter().enumerate() {
            let on = i == at;
            col = col.push(
                fill(
                    widget::Column::new().push(text(name, SANS_BOLD, 19.0, if on { BLACK } else { TEXT })).push(text(root.display().to_string(), MONO, 12.0, if on { BLACK } else { TEXT_2 })),
                    if on { YELLOW } else { PANEL },
                )
                .padding([12, 18])
                .width(Length::Fill),
            );
        }
        Some(col.into())
    }

    fn filter_rows(&self) -> Vec<FilterRow> {
        let mut v: Vec<FilterRow> = Show::ALL.iter().filter(|s| **s != Show::All).map(|s| FilterRow::Kind(*s)).collect();
        v.push(FilterRow::When);
        if self.filter.when == When::Between {
            v.push(FilterRow::From);
            v.push(FilterRow::To);
        }
        v.push(FilterRow::Order);
        v.push(FilterRow::Clear);
        v
    }

    fn filter_key(&mut self, row: usize, named: Option<keyboard::key::Named>, ch: &str) -> Task<Message> {
        use keyboard::key::Named;
        let rows = self.filter_rows();
        let Some(r) = rows.get(row).copied() else {
            self.focus = Focus::Filter(0);
            return Task::none();
        };
        let step = match named {
            Some(Named::ArrowRight) => 1,
            Some(Named::ArrowLeft) => -1,
            _ => 0,
        };
        match named {
            Some(Named::ArrowDown) => self.focus = Focus::Filter((row + 1).min(rows.len() - 1)),
            Some(Named::ArrowUp) => self.focus = Focus::Filter(row.saturating_sub(1)),
            Some(Named::Escape) => {
                self.focus = Focus::Grid;
                self.sel = 0;
                return Task::none();
            }
            _ => {}
        }
        let press = named == Some(Named::Enter) || ch == " ";
        match r {
            FilterRow::Kind(k) if press => {
                if let Some(i) = self.filter.kinds.iter().position(|x| *x == k) {
                    self.filter.kinds.remove(i);
                } else {
                    self.filter.kinds.push(k);
                    self.filter.kinds.sort_by_key(|x| Show::ALL.iter().position(|y| y == x));
                }
            }
            FilterRow::When if step != 0 || press => {
                let i = When::ALL.iter().position(|w| *w == self.filter.when).unwrap_or(0) as i32;
                let d = if step == 0 { 1 } else { step };
                self.filter.when = When::ALL[(i + d).rem_euclid(When::ALL.len() as i32) as usize];
            }
            FilterRow::From | FilterRow::To => {
                let field = if r == FilterRow::From { &mut self.filter.from } else { &mut self.filter.to };
                if named == Some(Named::Backspace) {
                    field.pop();
                } else if ch.len() == 1 && (ch.chars().all(|c| c.is_ascii_digit()) || ch == "-") && field.len() < 10 {
                    field.push_str(ch);
                    // the dashes come by themselves: 20261001 → 2026-10-01
                    if field.len() == 4 || field.len() == 7 {
                        field.push('-');
                    }
                    *field = field.replace("--", "-");
                }
            }
            FilterRow::Order if step != 0 || press => {
                let i = Sort::ALL.iter().position(|x| *x == self.sort).unwrap_or(0) as i32;
                let d = if step == 0 { 1 } else { step };
                self.sort = Sort::ALL[(i + d).rem_euclid(Sort::ALL.len() as i32) as usize];
                self.sort.save();
            }
            FilterRow::Clear if press => {
                self.filter = Filter::default();
                self.focus = Focus::Filter(0);
            }
            _ => {}
        }
        self.filter.save();
        self.sel = 0;
        Task::none()
    }

    fn filter_panel(&self) -> Option<Element<'_, Message>> {
        let Focus::Filter(at) = self.focus else { return None };
        let line = |i: usize, label: String, value: String, on_value: bool| -> Element<'_, Message> {
            let on = i == at;
            fill(
                widget::Row::new()
                    .push(text(label, SANS_BOLD, 19.0, if on { BLACK } else { TEXT }))
                    .push(hfill())
                    .push(text(value, MONO_BOLD, 15.0, if on { BLACK } else if on_value { YELLOW } else { TEXT_2 }))
                    .align_y(Alignment::Center),
                if on { YELLOW } else { PANEL },
            )
            .padding([12, 18])
            .width(Length::Fill)
            .into()
        };
        let mut col = widget::Column::new().spacing(6).width(Length::Fixed(430.0));
        col = col.push(text("FILTER", MONO, 14.0, YELLOW)).push(text("Show only", SANS_BOLD, 26.0, TEXT)).push(text("Choose one kind or several. None chosen: everything.", SANS, 15.0, TEXT_2)).push(space(1.0, 6.0));
        for (i, r) in self.filter_rows().into_iter().enumerate() {
            let el = match r {
                FilterRow::Kind(k) => {
                    let on = self.filter.kinds.contains(&k);
                    let name = k.label().trim_start_matches("Only ").to_string();
                    let name = name[..1].to_uppercase() + &name[1..];
                    line(i, name, if on { "■ SHOWN".into() } else { "□".into() }, on)
                }
                FilterRow::When => {
                    col = col.push(space(1.0, 10.0)).push(text("CHANGED", MONO, 13.0, MUTED));
                    line(i, "When".into(), format!("‹ {} ›", self.filter.when.label()), self.filter.when != When::Any)
                }
                FilterRow::From => line(i, "From (YYYY-MM-DD)".into(), if self.filter.from.is_empty() { "type the date".into() } else { self.filter.from.clone() }, parse_date(&self.filter.from).is_some()),
                FilterRow::To => line(i, "To (YYYY-MM-DD)".into(), if self.filter.to.is_empty() { "type the date".into() } else { self.filter.to.clone() }, parse_date(&self.filter.to).is_some()),
                FilterRow::Order => {
                    col = col.push(space(1.0, 10.0)).push(text("ORDER", MONO, 13.0, MUTED));
                    line(i, "Order".into(), format!("‹ {} ›", self.sort.label()), false)
                }
                FilterRow::Clear => {
                    col = col.push(space(1.0, 10.0));
                    line(i, "Show everything again".into(), "ENTER".into(), false)
                }
            };
            col = col.push(el);
        }
        let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
        col = col.push(space(1.0, 10.0)).push(text(format!("{} files shown", self.items_in(&view).len()), MONO, 14.0, TEXT_2));
        Some(col.into())
    }

    /// Files on a row: as many cards of about 280 pixels as fit beside the
    /// collections (and the panel on the right, when one shows).
    fn cols(&self) -> usize {
        let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
        let panel = if self.filter_panel().is_some() || self.send_panel().is_some() || self.project_panel().is_some() || self.drive_panel(&view).is_some() || self.save_panel(&view).is_some() { 460.0 } else { 0.0 };
        let room = self.size.width - 330.0 - 56.0 - 144.0 - panel;
        ((room / 302.0).floor() as usize).clamp(2, 10)
    }
    fn card_width(&self) -> f32 {
        let view = self.views.get(self.view).cloned().unwrap_or(View::Recent);
        let panel = if self.filter_panel().is_some() || self.send_panel().is_some() || self.project_panel().is_some() || self.drive_panel(&view).is_some() || self.save_panel(&view).is_some() { 460.0 } else { 0.0 };
        let room = self.size.width - 330.0 - 56.0 - 144.0 - panel;
        let cols = self.cols() as f32;
        ((room - (cols - 1.0) * 22.0) / cols).clamp(200.0, 420.0)
    }
    /// Rows that fit on the screen.
    fn rows_shown(&self, thumb_h: f32) -> usize {
        let row_h = thumb_h + 80.0 + 22.0;
        (((self.size.height - 300.0) / row_h).floor() as usize).max(1)
    }

    /// Reads everything again now (after something was done here); the
    /// phones are asked about, and a drive is read, only in the background.
    fn scan(&mut self) {
        let g = gather(&self.rules, None, self.mode == Mode::Browse, false);
        self.apply(g);
    }

    /// Reads everything again without stopping the screen (every few
    /// seconds): a big photo collection or a slow drive takes a while.
    fn scan_later(&mut self) -> Task<Message> {
        if self.scanning {
            return Task::none();
        }
        self.scanning = true;
        // a drive's files are read once when it is looked at, not every time
        let root = self.drive_root().filter(|(id, _, _)| self.drive_read_for.as_ref() != Some(id));
        let (rules, browse) = (self.rules.clone(), self.mode == Mode::Browse);
        Task::perform(
            async move { tokio::task::spawn_blocking(move || gather(&rules, root, browse, true)).await.ok() },
            |g| cosmic::Action::App(Message::Scanned(Box::new(g))),
        )
    }

    /// The drive being looked at: its id, name and where its files are.
    fn drive_root(&self) -> Option<(String, String, PathBuf)> {
        let Some(View::Drive(id)) = self.views.get(self.view) else { return None };
        let d = self.drives.iter().find(|d| &d.id == id)?;
        match &d.state {
            crate::drives::State::Open(root) => Some((d.id.clone(), d.name.clone(), root.clone())),
            _ => None,
        }
    }

    fn apply(&mut self, g: Gathered) {
        let old_view = self.views.get(self.view).cloned();
        // the file chosen stays chosen, wherever new files push it
        let keep = self.current_item().map(|i| i.path);
        self.items = g.items;
        self.collections = g.collections.clone();
        let mut views = match &self.mode {
            Mode::Browse => vec![View::Inbox, View::Recent],
            Mode::Open { folders: false, .. } => vec![View::Recent, View::Inbox],
            _ => vec![],
        };
        views.extend(g.collections.into_iter().map(View::Collection));
        if matches!(self.mode, Mode::Browse | Mode::Open { folders: false, .. }) {
            views.extend(Kind::VIEWS.iter().map(|k| View::Kind(*k)));
        }
        if self.mode == Mode::Browse {
            views.push(View::Bin);
            self.bin_items = g.bin;
        }
        if matches!(self.mode, Mode::Browse | Mode::Open { folders: false, .. }) {
            views.extend(self.drives.iter().map(|d| View::Drive(d.id.clone())));
        }
        if self.mode == Mode::Browse {
            if g.with_phones {
                self.phones = g.phones;
            }
            views.extend(self.phones.iter().map(|p| View::Phone(p.uri.clone())));
        }
        if views.is_empty() {
            views.push(View::Collection(DEFAULT_COLLECTIONS[0].into()));
        }
        self.views = views;
        // the files on the drive being looked at (read for the one that
        // was looked at when reading started)
        match &old_view {
            Some(View::Drive(id)) if g.drive_for.as_ref() == Some(id) => {
                self.drive_items = g.drive;
                self.drive_read_for = Some(id.clone());
            }
            // not read this time (read already), or read for another drive
            Some(View::Drive(_)) => {}
            _ => {
                self.drive_items.clear();
                self.drive_read_for = None;
            }
        }
        if let Some(v) = old_view {
            self.view = self.views.iter().position(|x| *x == v).unwrap_or(0);
        }
        let view = self.views[self.view].clone();
        let list = self.items_in(&view);
        let found = keep.as_ref().map(|p| list.iter().position(|x| x.path == *p));
        let n = list.len();
        if let Some(found) = found {
            if let Some(i) = found {
                self.sel = i;
            } else if matches!(self.focus, Focus::Confirm | Focus::Ask(_) | Focus::Rename | Focus::NewName) {
                // the file went away meanwhile: nothing is done to another
                self.focus = Focus::Grid;
                self.note = Some("That file is not there any more.".into());
            }
        }
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn inbox(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.collection.is_none()).collect()
    }

    fn items_in(&self, v: &View) -> Vec<&Item> {
        // a search looks through everything, whatever view is open
        let mut items = if self.query.trim().is_empty() {
            self.unsorted_items_in(v)
        } else if matches!(v, View::Bin | View::Phone(_) | View::Drive(_)) {
            // the bin, a drive or a phone: searched on its own, so what is
            // done there (delete for good, put back) only ever reaches it
            let q = self.query.trim().to_lowercase();
            self.unsorted_items_in(v).into_iter().filter(|i| i.name.to_lowercase().contains(&q)).collect()
        } else {
            let q = self.query.trim().to_lowercase();
            let mut v: Vec<&Item> = self.items.iter().filter(|i| i.name.to_lowercase().contains(&q)).collect();
            v.sort_by_key(|i| !i.name.to_lowercase().starts_with(&q));
            v.dedup_by(|a, b| a.path == b.path);
            v
        };
        if !self.filter.is_all() {
            items.retain(|i| self.filter.keeps(i));
        }
        // Recent is the newest files, whatever the order chosen
        if *v != View::Recent {
            self.sort.apply(&mut items);
        }
        items
    }

    /// How many files a view holds (with the filter), without copying them.
    fn count_in(&self, v: &View) -> usize {
        let ok = |i: &Item| self.filter.is_all() || self.filter.keeps(i);
        match v {
            View::Bin => self.bin_items.iter().filter(|i| ok(i)).count(),
            View::Phone(_) | View::Drive(_) => self.unsorted_items_in(v).iter().filter(|i| ok(i)).count(),
            _ if !matches!(self.mode, Mode::Browse) => self.unsorted_items_in(v).len(),
            View::Inbox => self.items.iter().filter(|i| i.collection.is_none() && ok(i)).count(),
            View::Recent => self.items.iter().filter(|i| ok(i)).take(60).count(),
            View::Collection(c) => self.items.iter().filter(|i| i.collection.as_deref() == Some(c.as_str()) && ok(i)).count(),
            View::Kind(k) => self.items.iter().filter(|i| i.kind == *k && ok(i)).count(),
        }
    }

    fn unsorted_items_in(&self, v: &View) -> Vec<&Item> {
        if *v == View::Bin {
            return self.bin_items.iter().collect();
        }
        if let View::Phone(uri) = v {
            return match &self.phone_items {
                Some((u, items)) if u == uri => items.iter().collect(),
                _ => vec![],
            };
        }
        if let View::Drive(id) = v {
            let looking = matches!(self.views.get(self.view), Some(View::Drive(x)) if x == id);
            if !looking {
                return vec![];
            }
            let exts: &[String] = match &self.mode {
                Mode::Open { exts, .. } => exts,
                _ => &[],
            };
            return self.drive_items.iter().filter(|i| exts.is_empty() || exts.contains(&ext_of(&i.path))).collect();
        }
        if matches!(self.mode, Mode::Save { .. } | Mode::Open { folders: true, .. }) {
            // choosing a place, not a file: show what is already there
            return self.items.iter().filter(|i| matches!(v, View::Collection(c) if i.collection.as_deref() == Some(c))).take(40).collect();
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
                View::Bin | View::Phone(_) => false,
            })
            .take(if *v == View::Recent { 60 } else { usize::MAX })
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

    fn ask_panel(&self, items: &[&Item]) -> Option<Element<'_, Message>> {
        let it = *items.get(self.sel)?;
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
        self.items_in(&v).get(self.sel).map(|i| (*i).clone())
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
        if let View::Phone(uri) = &view {
            if let Some(t) = self.phone_key(uri.clone(), named, &ch, n) {
                return t;
            }
        }
        if self.picking() {
            if let Some(t) = self.pick_key(named, &ch, &view, n) {
                return t;
            }
        }
        if let Focus::NewProject(lang) = self.focus {
            let n = crate::workshop::Lang::ALL.len();
            match named {
                Some(Named::Escape) => self.focus = Focus::Side,
                Some(Named::ArrowDown) => self.focus = Focus::NewProject((lang + 1).min(n - 1)),
                Some(Named::ArrowUp) => self.focus = Focus::NewProject(lang.saturating_sub(1)),
                _ => {}
            }
            return Task::none();
        }
        if ch == "n" && !self.picking() && matches!(&view, View::Collection(c) if c == "Projects") && matches!(self.focus, Focus::Side | Focus::Grid) {
            self.focus = Focus::NewProject(0);
            self.project_name.clear();
            return text_input::focus(self.input_id.clone());
        }
        if let Focus::Filter(row) = self.focus {
            return self.filter_key(row, named, &ch);
        }
        if self.focus == Focus::Search {
            match named {
                Some(Named::Escape) => {
                    self.query.clear();
                    self.focus = Focus::Side;
                }
                Some(Named::ArrowDown) | Some(Named::Enter) => {
                    self.sel = 0;
                    self.focus = Focus::Grid;
                }
                _ => {}
            }
            return Task::none();
        }
        if ch == "f" && matches!(self.focus, Focus::Side | Focus::Grid) && !self.picking() {
            self.focus = Focus::Filter(0);
            return Task::none();
        }
        if ch == "/" && matches!(self.focus, Focus::Side | Focus::Grid) {
            self.focus = Focus::Search;
            return text_input::focus(self.search_id.clone());
        }
        // out of a search: Esc in the results clears it
        if named == Some(Named::Escape) && !self.query.is_empty() && matches!(self.focus, Focus::Grid | Focus::Side) {
            self.query.clear();
            self.sel = 0;
            self.focus = Focus::Side;
            return Task::none();
        }
        if ch == "s" && matches!(self.focus, Focus::Side | Focus::Grid) {
            if view == View::Recent {
                self.note = Some("Recent is always newest first. S sorts the other views.".into());
                return Task::none();
            }
            self.sort = self.sort.next();
            self.sort.save();
            self.sel = 0;
            self.note = Some(format!("{}. S changes the order.", self.sort.label()));
            return self.follow();
        }
        match self.focus {
            Focus::SaveName | Focus::DrivePassword | Focus::KeyPin | Focus::AddPassword | Focus::AddPin | Focus::NewProject(_) | Focus::Filter(_) | Focus::Search => {}
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
            Focus::Grid if view == View::Bin => match named {
                Some(Named::ArrowRight) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                Some(Named::ArrowLeft) => {
                    if self.sel % self.cols() == 0 {
                        self.focus = Focus::Side;
                    } else {
                        self.sel -= 1;
                    }
                }
                Some(Named::ArrowDown) if self.sel + self.cols() < n => self.sel += self.cols(),
                Some(Named::ArrowUp) if self.sel >= self.cols() => self.sel -= self.cols(),
                Some(Named::Escape) => self.focus = Focus::Side,
                Some(Named::Enter) => {
                    if let Some(it) = self.current_item() {
                        self.note = Some(match restore(&it.path) {
                            Ok(to) => format!("{} is back in {}.", it.name, to.parent().map(|p| p.display().to_string()).unwrap_or_default()),
                            Err(e) => format!("Could not put it back: {e}"),
                        });
                        self.scan();
                    }
                }
                Some(Named::Delete) => self.focus = Focus::Confirm,
                _ => {}
            },
            Focus::Grid => match named {
                Some(Named::ArrowRight) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                Some(Named::ArrowLeft) => {
                    if self.sel % self.cols() == 0 {
                        self.focus = Focus::Side;
                    } else {
                        self.sel -= 1;
                    }
                }
                Some(Named::ArrowDown) => {
                    if self.sel + self.cols() < n {
                        self.sel += self.cols();
                    }
                }
                Some(Named::ArrowUp) => {
                    if self.sel >= self.cols() {
                        self.sel -= self.cols();
                    }
                }
                Some(Named::Escape) => self.focus = Focus::Side,
                Some(Named::Enter) => {
                    if let Some(it) = self.current_item() {
                        if it.collection.is_none() {
                            self.focus = Focus::Ask(0);
                        } else {
                            self.opened(&it.path);
                        }
                    }
                }
                Some(Named::Delete) => self.focus = Focus::Confirm,
                _ => {
                    if ch == "m" {
                        self.focus = Focus::Ask(0);
                    }
                    if ch == " " && self.mode == Mode::Browse {
                        if let Some(it) = self.current_item() {
                            if let Some(p) = self.marked.iter().position(|m| *m == it.path) {
                                self.marked.remove(p);
                            } else {
                                self.marked.push(it.path);
                            }
                            self.note = Some(format!("{} marked. C copies them to a drive.", self.marked.len()));
                        }
                    }
                    if ch == "c" && self.mode == Mode::Browse {
                        if self.open_drives().is_empty() {
                            self.note = Some("No drive is open. Plug one in, choose it under Drives, then →.".into());
                        } else {
                            self.focus = Focus::SendTo(0);
                        }
                    }
                    if ch == "r" {
                        if let Some(it) = self.current_item() {
                            self.rename = it.name.clone();
                            self.focus = Focus::Rename;
                            return text_input::focus(self.input_id.clone());
                        }
                    }
                }
            },
            Focus::Rename => {
                if named == Some(Named::Escape) {
                    self.focus = Focus::Grid;
                }
            }
            Focus::SendTo(i) => {
                let drives = self.open_drives();
                match named {
                    Some(Named::ArrowDown) => self.focus = Focus::SendTo((i + 1).min(drives.len().saturating_sub(1))),
                    Some(Named::ArrowUp) => self.focus = Focus::SendTo(i.saturating_sub(1)),
                    Some(Named::Escape) => self.focus = Focus::Grid,
                    Some(Named::Enter) => {
                        let Some((name, root)) = drives.get(i).cloned() else { return Task::none() };
                        let files: Vec<PathBuf> = if self.marked.is_empty() { self.current_item().map(|it| vec![it.path]).unwrap_or_default() } else { self.marked.clone() };
                        self.focus = Focus::Grid;
                        self.drive_busy = true;
                        self.note = Some(format!("Copying {} to {name}…", if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) }));
                        return Task::perform(
                            async move {
                                let name2 = name.clone();
                                tokio::task::spawn_blocking(move || copy_all(&files, &root).map(|n| format!("{n} copied to {name2}. Press E on the drive before taking it out.")))
                                    .await
                                    .unwrap_or_else(|e| Err(e.to_string()))
                            },
                            |r| cosmic::Action::App(Message::Copied(r)),
                        );
                    }
                    _ => {}
                }
            }
            Focus::Confirm if view == View::Bin => {
                if named == Some(Named::Enter) {
                    if let Some(it) = self.current_item() {
                        self.note = Some(match forget(&it.path) {
                            Ok(()) => format!("{} is gone for good.", it.name),
                            Err(e) => format!("Could not delete: {e}"),
                        });
                    }
                    self.scan();
                }
                self.focus = Focus::Grid;
            }
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

    fn make_project(&mut self) -> Task<Message> {
        use crate::workshop::{Lang, create, tidy_name};
        let Focus::NewProject(i) = self.focus else { return Task::none() };
        let name = tidy_name(&self.project_name);
        if name.is_empty() {
            return Task::none();
        }
        match create(&name, Lang::ALL[i]) {
            Ok(dir) => {
                self.note = Some(format!("{name} is ready. It opens in Typewriter; its terminal has the project's tools."));
                self.focus = Focus::Side;
                self.scan();
                if let Err(e) = open(&dir) {
                    self.note = Some(e);
                }
            }
            Err(e) => self.note = Some(e),
        }
        Task::none()
    }

    fn project_panel(&self) -> Option<Element<'_, Message>> {
        let Focus::NewProject(sel) = self.focus else { return None };
        let mut col = widget::Column::new()
            .spacing(12)
            .width(Length::Fixed(440.0))
            .push(text("NEW PROJECT", MONO, 14.0, YELLOW))
            .push(text("Its name", SANS_BOLD, 22.0, TEXT))
            .push(
                text_input::text_input("my-project", self.project_name.as_str())
                    .id(self.input_id.clone())
                    .on_input(Message::ProjectName)
                    .on_submit(|_| Message::ProjectSubmit)
                    .style(field_style())
                    .size(21)
                    .padding([14, 18]),
            )
            .push(space(1.0, 6.0))
            .push(text("Language  ↑ ↓", SANS_BOLD, 22.0, TEXT));
        for (i, l) in crate::workshop::Lang::ALL.iter().enumerate() {
            let on = i == sel;
            col = col.push(
                fill(text(l.label(), SANS_BOLD, 19.0, if on { BLACK } else { TEXT }), if on { YELLOW } else { PANEL_2 }).padding([12, 18]).width(Length::Fill),
            );
        }
        col = col.push(text("Its tools stay inside the project; nothing is installed on the system.", SANS, 16.0, TEXT_2));
        Some(fill(col.padding(30), PANEL).into())
    }

    fn drive(&self, id: &str) -> Option<crate::drives::Drive> {
        self.drives.iter().find(|d| d.id == id).cloned()
    }

    fn view_label(&self, v: &View) -> String {
        match v {
            View::Drive(id) => self.drive(id).map(|d| d.name).unwrap_or_else(|| "Drive".into()),
            View::Phone(uri) => self.phones.iter().find(|p| &p.uri == uri).map(|p| p.name.clone()).unwrap_or_else(|| "Phone".into()),
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
            View::Phone(uri) => match &self.phone_items {
                Some((u, _)) if u == uri => n.to_string(),
                _ => "PHONE".into(),
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
        if matches!(self.focus, Focus::DrivePassword | Focus::KeyPin | Focus::AddPassword | Focus::AddPin) {
            if named == Some(Named::Escape) {
                self.focus = Focus::Side;
                self.drive_pw.clear();
                self.key_pin.clear();
            }
            return Some(Task::none());
        }
        // LUKS drives can be opened with a security key, and get one added
        if self.focus == Focus::Side && d.state == State::Locked && d.kind == "LUKS" && (ch == "k" || ch == "a") {
            self.focus = if ch == "k" { Focus::KeyPin } else { Focus::AddPassword };
            self.drive_pw.clear();
            self.key_pin.clear();
            return Some(text_input::focus(self.input_id.clone()));
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
                        return Some(Task::none());
                    }
                    if self.drive_read_for.as_ref() != Some(&id) {
                        self.note = Some(format!("Reading what is on {}…", d.name));
                        return Some(self.scan_later());
                    }
                    self.note = Some(format!("{} is open, and empty.", d.name));
                    Some(Task::none())
                }
            },
            Focus::Grid if !self.picking() => {
                if ch == " " {
                    if let Some(it) = self.current_item() {
                        if let Some(p) = self.marked.iter().position(|m| *m == it.path) {
                            self.marked.remove(p);
                        } else {
                            self.marked.push(it.path);
                        }
                        self.note = Some(format!("{} marked. C copies them into Library.", self.marked.len()));
                    }
                    return Some(Task::none());
                }
                if ch == "c" {
                    // copies into Downloads: Library then asks where they belong
                    let files: Vec<PathBuf> = if self.marked.is_empty() { self.current_item().map(|it| vec![it.path]).unwrap_or_default() } else { self.marked.clone() };
                    if files.is_empty() {
                        return Some(Task::none());
                    }
                    let dl = home().join("Downloads");
                    self.drive_busy = true;
                    self.note = Some(format!("Copying {} into Library…", if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) }));
                    return Some(Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || copy_all(&files, &dl).map(|n| format!("{n} copied: they wait in \"Needs an answer\".")))
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        |r| cosmic::Action::App(Message::Copied(r)),
                    ));
                }
                match named {
                    Some(Named::Enter) => {
                        if let Some(it) = self.current_item() {
                            self.opened(&it.path);
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

    /// Opens a file and says so (or why not).
    fn opened(&mut self, path: &Path) {
        self.note = Some(match open(path) {
            Ok(app) => format!("Opening in {app}…"),
            Err(e) => e,
        });
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

    /// Opens the locked drive with the security key, then as usual.
    fn open_with_key(&mut self) -> Task<Message> {
        let Some(View::Drive(id)) = self.views.get(self.view).cloned() else { return Task::none() };
        let Some(d) = self.drive(&id) else { return Task::none() };
        self.drive_busy = true;
        self.note = Some("Touch your security key when it blinks…".into());
        let (name, pin) = (d.name.clone(), std::mem::take(&mut self.key_pin));
        Task::perform(
            async move {
                crate::keyd::unlock(d.device.clone(), pin).await?;
                // now an unlocked drive like any other: open it, once UDisks
                // has noticed it is unlocked
                for _ in 0..20 {
                    let now = crate::drives::list().await.into_iter().find(|x| x.id == d.id).ok_or("The drive went away.")?;
                    if now.state != crate::drives::State::Locked {
                        return crate::drives::open(now, None).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Err("The drive is unlocked, but it did not appear. Press → to open it.".to_string())
            },
            move |r| cosmic::Action::App(Message::DriveOpened(name.clone(), r)),
        )
    }

    /// Adds the security key to the drive (its password proves you may).
    fn add_key(&mut self) -> Task<Message> {
        let Some(View::Drive(id)) = self.views.get(self.view).cloned() else { return Task::none() };
        let Some(d) = self.drive(&id) else { return Task::none() };
        self.drive_busy = true;
        self.note = Some("Touch your security key when it blinks (maybe twice)…".into());
        let (name, pw, pin) = (d.name.clone(), std::mem::take(&mut self.drive_pw), std::mem::take(&mut self.key_pin));
        Task::perform(crate::keyd::enroll(d.device.clone(), pw, pin), move |r| cosmic::Action::App(Message::KeyAdded(name.clone(), r)))
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
            State::Locked if matches!(self.focus, Focus::KeyPin | Focus::AddPassword | Focus::AddPin) => {
                let (title, hint, value, msg): (&str, &str, &str, fn(String) -> Message) = match self.focus {
                    Focus::KeyPin => ("Your security key's PIN", "PIN (leave empty if it has none)", self.key_pin.as_str(), Message::KeyPin),
                    Focus::AddPassword => ("Add a security key: the drive's password", "Password", self.drive_pw.as_str(), Message::DrivePassword),
                    _ => ("Your security key's PIN", "PIN (leave empty if it has none)", self.key_pin.as_str(), Message::KeyPin),
                };
                let submit = match self.focus {
                    Focus::KeyPin => Message::KeySubmit,
                    Focus::AddPassword => Message::AddNext,
                    _ => Message::AddSubmit,
                };
                col = col.push(text(title, SANS_BOLD, 20.0, TEXT)).push(
                    text_input::text_input(hint, value)
                        .id(self.input_id.clone())
                        .password()
                        .on_input(msg)
                        .on_submit(move |_| submit.clone())
                        .style(field_style())
                        .size(21)
                        .padding([14, 18]),
                );
                col = col.push(text("Keep the key plugged in. Touch it when it blinks.", SANS, 17.0, TEXT_2));
            }
            State::Locked if d.kind == "LUKS" => {
                col = col.push(text("Locked. → types its password. K opens it with your security key (YubiKey). A adds a security key to it.", SANS, 18.0, TEXT_2));
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
        if p.is_dir() && crate::workshop::is_project(&p) {
            // a project is one thing in Library, opened in Typewriter
            f(&p);
        } else if p.is_dir() {
            walk(&p, depth + 1, f);
        } else if p.is_file() {
            f(&p);
        }
    }
}

/// Like `walk`, but stops when `f` says so (big drives).
fn walk_until(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path) -> bool) -> bool {
    if depth > 12 {
        return true;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    for e in rd.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if !walk_until(&p, depth + 1, f) {
                return false;
            }
        } else if t.is_file() && !f(&p) {
            return false;
        }
    }
    true
}

fn item(p: &Path, collection: Option<String>) -> Item {
    Item {
        path: p.to_path_buf(),
        name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        kind: if p.is_dir() { Kind::Project } else { Kind::of(p) },
        collection,
        modified: std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH),
        size: std::fs::metadata(p).map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0),
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
/// For Gallery: the same bin as Library's.
pub(crate) fn trash_file(path: &Path) -> std::io::Result<()> {
    trash(path)
}

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
    let to = bin.join("files").join(&target);
    match std::fs::rename(path, &to) {
        Ok(()) => {}
        // on another drive (a USB stick): copied into the bin, then removed
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            if let Err(e) = copy_all(&[path.to_path_buf()], &bin.join("files")).map_err(std::io::Error::other) {
                return Err(e);
            }
            // copy_all keeps the name, or numbers it as the bin's name above
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        Err(e) => return Err(e),
    }
    std::fs::write(bin.join("info").join(format!("{target}.trashinfo")), format!("[Trash Info]\nPath={}\nDeletionDate={now}\n", url_path(path)))
}

/// What reading the disk found.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    /// the drive whose files `drive` are (its id)
    drive_for: Option<String>,
    /// the phones were asked about (they are not, when reading right away)
    with_phones: bool,
    collections: Vec<String>,
    items: Vec<Item>,
    drive: Vec<Item>,
    bin: Vec<Item>,
    phones: Vec<crate::phones::Phone>,
}

/// Reads Library, the waiting files, the drive looked at, the bin and the
/// phones (slow: off the screen's thread when it can be). Files with a
/// remembered rule are filed on the way.
fn gather(rules: &HashMap<String, String>, drive: Option<(String, String, PathBuf)>, browse: bool, phones: bool) -> Gathered {
    for _ in 0..2 {
        let mut items = Vec::new();
        let lib = library_dir();
        let mut collections: Vec<String> = std::fs::read_dir(&lib)
            .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| !n.starts_with('.')).collect())
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
                    // skip hidden files and downloads still in progress: a
                    // browser keeps an empty file under the final name while
                    // the real one downloads next to it as .part
                    let busy = ["part", "crdownload", "download"].iter().any(|ext| d.join(format!("{name}.{ext}")).exists() || p.with_extension(ext).exists());
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
            if let Some(c) = rules.get(&ext_of(&it.path)) {
                if file_into(&it.path, c).is_ok() {
                    moved = true;
                }
            }
        }
        if moved {
            continue;
        }
        let mut drive_items = vec![];
        if let Some((_, tag, root)) = &drive {
            walk_until(root, 0, &mut |p| {
                drive_items.push(item(p, Some(tag.clone())));
                drive_items.len() < 50_000
            });
            drive_items.sort_by(|a: &Item, b: &Item| b.modified.cmp(&a.modified));
        }
        let bin = if browse { bin_list() } else { vec![] };
        let with_phones = browse && phones;
        let phones = if with_phones { crate::phones::list() } else { vec![] };
        return Gathered { drive_for: drive.map(|d| d.0), with_phones, collections, items, drive: drive_items, bin, phones };
    }
    Gathered::default()
}

fn bin_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("Trash")
}

/// What is in the bin, newest first; what has been there 30 days goes.
fn bin_list() -> Vec<Item> {
    let bin = bin_dir();
    let now = chrono::Local::now().naive_local();
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(bin.join("files")) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let info = std::fs::read_to_string(bin.join("info").join(format!("{name}.trashinfo"))).unwrap_or_default();
        let deleted = info
            .lines()
            .find_map(|l| l.strip_prefix("DeletionDate="))
            .and_then(|d| chrono::NaiveDateTime::parse_from_str(d.trim(), "%Y-%m-%dT%H:%M:%S").ok());
        if deleted.is_some_and(|d| (now - d).num_days() >= 30) {
            let _ = forget(&p);
            continue;
        }
        let mut it = item(&p, Some("Bin".into()));
        // shown under the name it had
        if let Some(orig) = original_of(&info) {
            it.name = orig.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(it.name);
        }
        if let Some(d) = deleted {
            it.modified = d.and_local_timezone(chrono::Local).single().map(SystemTime::from).unwrap_or(it.modified);
        }
        out.push(it);
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// The original place, from a .trashinfo ("Path=/home/x/My%20file.txt").
fn original_of(info: &str) -> Option<PathBuf> {
    let raw = info.lines().find_map(|l| l.strip_prefix("Path="))?.trim();
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    let p = PathBuf::from(std::ffi::OsString::from_vec(out));
    // only a plain place in the home: nothing relative, nothing with ..
    let plain = p.is_absolute() && !p.components().any(|c| matches!(c, std::path::Component::ParentDir));
    plain.then_some(p)
}

/// Puts a file from the bin back where it was (under a new name if that
/// place is taken now).
/// Is this a file in the bin (and not anywhere else)?
fn in_bin(p: &Path) -> bool {
    p.parent() == Some(bin_dir().join("files").as_path())
}

fn restore(in_bin_path: &Path) -> std::io::Result<PathBuf> {
    let in_bin_ok = in_bin(in_bin_path);
    let in_bin = in_bin_path;
    if !in_bin_ok {
        return Err(std::io::Error::other("that is not in the bin"));
    }
    let bin = bin_dir();
    let name = in_bin.file_name().unwrap_or_default().to_string_lossy().to_string();
    let info_path = bin.join("info").join(format!("{name}.trashinfo"));
    let info = std::fs::read_to_string(&info_path).unwrap_or_default();
    let mut to = original_of(&info).unwrap_or_else(|| library_dir().join(&name));
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut n = 2;
    while to.exists() {
        let stem = to.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let ext = to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        to = to.with_file_name(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(in_bin, &to)?;
    let _ = std::fs::remove_file(info_path);
    Ok(to)
}

/// Deletes a file in the bin for good.
fn forget(path: &Path) -> std::io::Result<()> {
    if !in_bin(path) {
        return Err(std::io::Error::other("only what is in the bin is deleted for good"));
    }
    let in_bin = path;
    let name = in_bin.file_name().unwrap_or_default().to_string_lossy().to_string();
    if in_bin.is_dir() {
        std::fs::remove_dir_all(in_bin)?;
    } else {
        std::fs::remove_file(in_bin)?;
    }
    let _ = std::fs::remove_file(bin_dir().join("info").join(format!("{name}.trashinfo")));
    Ok(())
}

/// Copies files (and folders, with what is in them) into a folder; a name
/// already there gets a number. Returns "3 files".
fn copy_all(files: &[PathBuf], into: &Path) -> Result<String, String> {
    fn copy_one(from: &Path, to: &Path) -> std::io::Result<u64> {
        if from.is_dir() {
            std::fs::create_dir_all(to)?;
            let mut n = 0;
            for e in std::fs::read_dir(from)?.flatten() {
                n += copy_one(&e.path(), &to.join(e.file_name()))?;
            }
            Ok(n)
        } else {
            std::fs::copy(from, to)?;
            Ok(1)
        }
    }
    std::fs::create_dir_all(into).map_err(|e| format!("Could not copy: {e}"))?;
    let mut count = 0;
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let mut target = into.join(&name);
        let mut k = 2;
        while target.exists() {
            target = into.join(format!("{k}-{name}"));
            k += 1;
        }
        count += copy_one(f, &target).map_err(|e| {
            if e.raw_os_error() == Some(libc::ENOSPC) {
                "The drive is full.".to_string()
            } else if e.raw_os_error() == Some(libc::EFBIG) {
                format!("{name} is too big for this drive (FAT32 takes files up to 4 GB).")
            } else {
                format!("Could not copy {name}: {e}")
            }
        })?;
    }
    // written out before saying it is done
    unsafe { libc::sync() };
    Ok(if count == 1 { "1 file".into() } else { format!("{count} files") })
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

/// Opens a file with the app made for its type, through `chlorum run`: the
/// app gets what the owner allows it, and this one file even when it may
/// not see personal files. A folder opens in Typewriter. Says which app,
/// or why nothing opens it.
/// For Gallery: opens a file with its app, as Library does.
pub(crate) fn open_file(path: &Path) -> Result<String, String> {
    open(path)
}

fn open(path: &Path) -> Result<String, String> {
    let quoted = crate::apps::quote(&path.to_string_lossy());
    let apps = crate::apps::handlers();
    let (line, name) = if path.is_dir() {
        match apps.get("chlorum-typewriter") {
            Some(a) => (format!("chlorum run {} --grant {} {}", crate::apps::quote(&a.id), quoted, crate::apps::quote(&format!("{} {}", a.exec, quoted))), a.name.clone()),
            None => (format!("typewriter {quoted}"), "Typewriter".to_string()),
        }
    } else {
        let mime = crate::mime::of(path);
        let Some(app) = crate::mime::app_for(&mime, &apps).and_then(|id| apps.get(&id).cloned()) else {
            let what = path.extension().map(|e| format!(".{} files", e.to_string_lossy())).unwrap_or_else(|| format!("files of this kind ({mime})"));
            let office = mime.starts_with("application/vnd.oasis") || mime.starts_with("application/vnd.openxmlformats") || mime.starts_with("application/vnd.ms-") || mime == "application/msword" || mime == "text/csv";
            if office {
                return Err(format!("No app here opens {what} yet. Get LibreOffice in Center → Get apps (type “libre”)."));
            }
            return Err(format!("No app here opens {what}. Center → Get apps may have one."));
        };
        // the command line is one word for `chlorum run`, which runs it through sh
        let line = format!("chlorum run {} --grant {} {}", crate::apps::quote(&app.id), quoted, crate::apps::quote(&format!("{} {}", app.exec, quoted)));
        (line, app.name)
    };
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(line)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start {name}: {e}"))?;
    // reaped in the background
    std::thread::spawn(move || child.wait());
    Ok(name)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showing_one_kind() {
        let it = |name: &str| {
            let path = PathBuf::from(format!("/x/{name}"));
            Item { kind: Kind::of(&path), path, name: name.into(), collection: None, modified: SystemTime::UNIX_EPOCH, size: 0 }
        };
        assert!(Show::Photos.keeps(&it("a.JPG")) && !Show::Photos.keeps(&it("a.mp4")));
        assert!(Show::Videos.keeps(&it("a.mkv")));
        assert!(Show::Audio.keeps(&it("a.flac")));
        assert!(Show::Text.keeps(&it("notes.md")) && !Show::Text.keeps(&it("cv.docx")));
        assert!(Show::Office.keeps(&it("cv.docx")) && Show::Office.keeps(&it("book.pdf")) && !Show::Office.keeps(&it("notes.txt")));
        assert!(Show::Technical.keeps(&it("main.rs")) && Show::Technical.keeps(&it("Makefile")) && Show::Technical.keeps(&it("x.zip")));
        assert!(Show::All.keeps(&it("whatever")));
        assert!(Show::Other.keeps(&it("whatever")) && !Show::Other.keeps(&it("a.jpg")));
        // several kinds at once, and a time
        let f = Filter { kinds: vec![Show::Photos, Show::Videos], when: When::Any, from: String::new(), to: String::new() };
        assert!(f.keeps(&it("a.jpg")) && f.keeps(&it("b.mp4")) && !f.keeps(&it("c.txt")));
        let old = Filter { kinds: vec![], when: When::Today, from: String::new(), to: String::new() };
        assert!(!old.keeps(&it("a.jpg")));
        let range = Filter { kinds: vec![], when: When::Between, from: "1969-12-31".into(), to: "1970-01-02".into() };
        assert!(range.keeps(&it("a.jpg")));
        assert_eq!(f.summary(), "Photos, Videos · Any time");
    }
}

#[cfg(test)]
mod bin_tests {
    #[test]
    fn original_place() {
        let p = super::original_of("[Trash Info]\nPath=/home/r/Library/Docs/my%20file%C3%A9.txt\nDeletionDate=2026-10-28T10:00:00\n").unwrap();
        assert_eq!(p, std::path::PathBuf::from("/home/r/Library/Docs/my fileé.txt"));
        assert_eq!(super::url_path(&p), "/home/r/Library/Docs/my%20file%C3%A9.txt");
        // a bin record cannot send a file outside a plain place
        assert!(super::original_of("Path=../../etc/x\n").is_none());
        assert!(super::original_of("Path=/home/r/../../etc/x\n").is_none());
        // only what is in the bin can be deleted for good or put back
        assert!(super::forget(std::path::Path::new("/home/r/Library/Documents/cv.pdf")).is_err());
        assert!(super::restore(std::path::Path::new("/home/r/Library/Documents/cv.pdf")).is_err());
    }
}
