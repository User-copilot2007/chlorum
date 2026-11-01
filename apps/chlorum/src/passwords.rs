//! `chlorum passwords` — Passwords: a vault of logins, kept in one file in
//! the KeePass format (KDBX 4, Argon2id, ChaCha20), so KeePassXC on a
//! computer or KeePassDX on a phone open it too.
//!
//! It is an app with nothing allowed: no internet, no sound, not your
//! files. The vault lives in its own home ($HOME/.local/share/chlorum/
//! Passwords.kdbx as it sees it); X puts a copy in Library ("Needs an
//! answer") to take to a phone.
//!
//! Keys, list: type to find; ↑ ↓ choose; Enter copies the password (the
//! clipboard is emptied 30 seconds later); Ctrl+B copies the user name;
//! Ctrl+N new; Ctrl+E or F2 changes; Delete deletes (Enter to confirm);
//! Ctrl+L locks; X exports a copy; Esc clears the search, then closes.
//! Change: Tab or ↑ ↓ between fields; Ctrl+G makes a strong password;
//! Ctrl+R shows or hides it; Enter saves; Esc goes back.
//! Five minutes without a key, and it locks by itself.

use crate::ui::*;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{self, Alignment, Length, Subscription, event, keyboard};
use cosmic::widget::text_input;
use cosmic::{Application, Element, widget};
use keepass::db::{Database, EntryId, fields};
use keepass::DatabaseKey;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const LOCK_AFTER: Duration = Duration::from_secs(5 * 60);
const CLEAR_AFTER: Duration = Duration::from_secs(30);
const MIN_MASTER: usize = 10;

pub fn run() -> Result<(), String> {
    let settings = Settings::default()
        .scale_factor(crate::ui::screen_zoom())
        .default_font(SANS)
        .client_decorations(false)
        .default_icon_theme("Papirus-Dark")
        .size(iced::Size::new(1400.0, 900.0));
    cosmic::app::run::<Passwords>(settings, ()).map_err(|e| e.to_string())
}

fn vault_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("chlorum/Passwords.kdbx")
}

/// One login, as the list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Login {
    pub id: EntryId,
    pub title: String,
    pub user: String,
    pub url: String,
}

/// The vault open in memory, with the key it was opened with.
struct Open {
    db: Database,
    master: String,
}

/// A good new password: 20 letters, digits and signs, from the kernel's
/// random numbers, with at least one of each kind.
pub fn generate() -> String {
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGIT: &[u8] = b"23456789";
    const SIGN: &[u8] = b"!#$%&*+-=?@_~";
    let all: Vec<u8> = [LOWER, UPPER, DIGIT, SIGN].concat();
    loop {
        let mut raw = [0u8; 64];
        if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut raw)).is_err() {
            // no randomness: refuse rather than make a guessable password
            return String::new();
        }
        // only bytes below the largest multiple of the alphabet: no bias
        let limit = 256 - (256 % all.len());
        let pw: String = raw.iter().filter(|b| (**b as usize) < limit).map(|b| all[*b as usize % all.len()] as char).take(20).collect();
        let has = |set: &[u8]| pw.bytes().any(|c| set.contains(&c));
        if pw.len() == 20 && has(LOWER) && has(UPPER) && has(DIGIT) && has(SIGN) {
            return pw;
        }
    }
}

fn config() -> keepass::config::DatabaseConfig {
    use keepass::config::{DatabaseConfig, KdfConfig};
    let mut c = DatabaseConfig::default();
    // KeePassXC's strength: 64 MiB, 10 rounds of Argon2id
    let version = match c.kdf_config {
        KdfConfig::Argon2 { version, .. } | KdfConfig::Argon2id { version, .. } => version,
        _ => return c,
    };
    c.kdf_config = KdfConfig::Argon2id { iterations: 10, memory: 64 * 1024 * 1024, parallelism: 2, version };
    c
}

/// Writes the vault safely: a new file next to it, then swapped in; the
/// one before stays as .bak.
fn save(db: &Database, master: &str, path: &std::path::Path) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("kdbx.new");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        db.save(&mut f, DatabaseKey::new().with_password(master)).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let _ = std::fs::copy(path, path.with_extension("kdbx.bak"));
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn open(master: &str, path: &std::path::Path) -> Result<Database, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    Database::open(&mut f, DatabaseKey::new().with_password(master)).map_err(|e| {
        let s = e.to_string();
        if s.contains("Incorrect key") || s.contains("key") || s.contains("HMAC") || s.contains("checksum") {
            "Wrong password.".to_string()
        } else {
            format!("Could not open the vault: {s}")
        }
    })
}

fn logins(db: &Database) -> Vec<Login> {
    let bin = db.recycle_bin().map(|g| g.id());
    let mut v: Vec<Login> = db
        .iter_all_entries()
        .filter(|e| bin.is_none_or(|b| e.parent().id() != b))
        .map(|e| Login {
            id: e.id(),
            title: e.get_title().unwrap_or("").to_string(),
            user: e.get_username().unwrap_or("").to_string(),
            url: e.get_url().unwrap_or("").to_string(),
        })
        .collect();
    v.sort_by_key(|l| l.title.to_lowercase());
    v
}

/// The logins that match what was typed (title, user name or website).
pub fn find<'a>(all: &'a [Login], q: &str) -> Vec<&'a Login> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return all.iter().collect();
    }
    let mut v: Vec<&Login> = all.iter().filter(|l| [&l.title, &l.user, &l.url].iter().any(|s| s.to_lowercase().contains(&q))).collect();
    // names that start with it first
    v.sort_by_key(|l| !l.title.to_lowercase().starts_with(&q));
    v
}

#[derive(Clone, Debug, PartialEq)]
enum Screen {
    /// no vault yet: choose its password (twice)
    Create,
    Locked,
    List,
    /// a login being made (None) or changed
    Edit(Option<EntryId>),
    /// Delete pressed on this login: Enter deletes
    Confirm(EntryId),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Form {
    title: String,
    user: String,
    password: String,
    url: String,
    notes: String,
}

const FIELDS: [&str; 5] = ["Name", "User name", "Password", "Website", "Notes"];

#[derive(Clone, Debug)]
pub enum Message {
    Key(keyboard::Key, keyboard::Modifiers, event::Status),
    Master(String),
    Master2(String),
    Submit,
    Opened(Result<(Database, String), String>),
    Query(String),
    Field(usize, String),
    Click(usize),
    Tick,
}

pub struct Passwords {
    core: Core,
    screen: Screen,
    vault: Option<Open>,
    all: Vec<Login>,
    master: String,
    master2: String,
    busy: bool,
    note: Option<(String, Instant)>,
    query: String,
    sel: usize,
    form: Form,
    field: usize,
    reveal: bool,
    last_key: Instant,
    /// the clipboard holds something of ours since then
    copied_at: Option<Instant>,
    ids: [widget::Id; 6],
}

impl Passwords {
    fn say(&mut self, s: impl Into<String>) {
        self.note = Some((s.into(), Instant::now()));
    }

    fn shown(&self) -> Vec<&Login> {
        find(&self.all, &self.query)
    }

    fn current(&self) -> Option<Login> {
        self.shown().get(self.sel).map(|l| (*l).clone())
    }

    fn refresh(&mut self) {
        self.all = self.vault.as_ref().map(|o| logins(&o.db)).unwrap_or_default();
        self.sel = self.sel.min(self.shown().len().saturating_sub(1));
    }

    fn focus(&self, i: usize) -> Task<Message> {
        text_input::focus(self.ids[i].clone())
    }

    fn lock(&mut self) -> Task<Message> {
        self.vault = None;
        self.all.clear();
        self.query.clear();
        self.master.clear();
        self.form = Form::default();
        self.screen = Screen::Locked;
        self.focus(0)
    }

    fn copy(&mut self, what: &str, value: String) -> Task<Message> {
        if value.is_empty() {
            self.say(format!("This login has no {what}."));
            return Task::none();
        }
        self.copied_at = Some(Instant::now());
        self.say(format!("{} copied. The clipboard is emptied in 30 seconds.", capital(what)));
        cosmic::iced::clipboard::write(value)
    }

    fn secret(&self, id: EntryId, key: &str) -> String {
        self.vault.as_ref().and_then(|o| o.db.entry(id)).and_then(|e| e.get(key).map(String::from)).unwrap_or_default()
    }

    fn store(&mut self) -> Result<(), String> {
        let o = self.vault.as_ref().ok_or("The vault is locked.")?;
        save(&o.db, &o.master, &vault_path())
    }

    fn start_edit(&mut self, id: Option<EntryId>) -> Task<Message> {
        self.form = match id {
            Some(id) => Form {
                title: self.secret(id, fields::TITLE),
                user: self.secret(id, fields::USERNAME),
                password: self.secret(id, fields::PASSWORD),
                url: self.secret(id, fields::URL),
                notes: self.secret(id, fields::NOTES),
            },
            None => Form { password: generate(), ..Form::default() },
        };
        self.screen = Screen::Edit(id);
        self.field = 0;
        self.reveal = false;
        self.focus(1)
    }

    fn save_form(&mut self) -> Task<Message> {
        let Screen::Edit(id) = self.screen.clone() else { return Task::none() };
        if self.form.title.trim().is_empty() {
            self.say("Give it a name first.");
            return self.focus(1);
        }
        let Some(o) = self.vault.as_mut() else { return Task::none() };
        let f = self.form.clone();
        let fill_in = |e: &mut keepass::db::Entry| {
            e.set_unprotected(fields::TITLE, f.title.trim());
            e.set_unprotected(fields::USERNAME, f.user.trim());
            e.set_protected(fields::PASSWORD, f.password.clone());
            e.set_unprotected(fields::URL, f.url.trim());
            e.set_unprotected(fields::NOTES, f.notes.clone());
        };
        let saved_id = match id {
            Some(id) => match o.db.entry_mut(id) {
                Some(mut e) => {
                    fill_in(&mut *e);
                    id
                }
                None => return Task::none(),
            },
            None => {
                let mut root = o.db.root_mut();
                let mut e = root.add_entry();
                fill_in(&mut *e);
                e.id()
            }
        };
        match self.store() {
            Ok(()) => {
                self.say(format!("{} saved.", f.title.trim()));
                self.screen = Screen::List;
                self.query.clear();
                self.refresh();
                self.sel = self.shown().iter().position(|l| l.id == saved_id).unwrap_or(0);
            }
            Err(e) => self.say(format!("Could not save: {e}")),
        }
        self.focus(0)
    }

    fn delete(&mut self, id: EntryId) {
        let title = self.secret(id, fields::TITLE);
        if let Some(o) = self.vault.as_mut() {
            if let Some(e) = o.db.entry_mut(id) {
                e.remove();
            }
        }
        match self.store() {
            Ok(()) => self.say(format!("{title} deleted.")),
            Err(e) => self.say(format!("Could not save: {e}")),
        }
        self.screen = Screen::List;
        self.refresh();
    }

    /// A copy for a phone or another computer, into Library.
    fn export(&mut self) {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        // in its sandbox, ~/Library is this app's drop box, which Library shows
        let to = home.join("Library").join(format!("Passwords {}.kdbx", chrono::Local::now().format("%Y-%m-%d")));
        let r = std::fs::create_dir_all(home.join("Library")).and_then(|_| std::fs::copy(vault_path(), &to));
        match r {
            Ok(_) => self.say("A copy is in Library, under Needs an answer. It opens with the same password in KeePassXC or KeePassDX."),
            Err(e) => self.say(format!("Could not make a copy: {e}")),
        }
    }

    fn key(&mut self, key: keyboard::Key, mods: keyboard::Modifiers, status: event::Status) -> Task<Message> {
        use keyboard::key::Named;
        self.last_key = Instant::now();
        let named = match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        };
        let ch = match key.as_ref() {
            keyboard::Key::Character(c) => c.to_lowercase(),
            _ => String::new(),
        };
        let ctrl = mods.control();
        // Enter in a field is its submit
        if named == Some(Named::Enter) && status == event::Status::Captured {
            return Task::none();
        }
        match self.screen.clone() {
            Screen::Create | Screen::Locked => {
                if named == Some(Named::Escape) {
                    std::process::exit(0);
                }
                if self.screen == Screen::Create && matches!(named, Some(Named::Tab) | Some(Named::ArrowDown) | Some(Named::ArrowUp)) {
                    return self.focus(if matches!(named, Some(Named::ArrowUp)) { 0 } else { 1 });
                }
            }
            Screen::List => {
                let n = self.shown().len();
                match named {
                    Some(Named::ArrowDown) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                    Some(Named::ArrowUp) => self.sel = self.sel.saturating_sub(1),
                    Some(Named::PageDown) => self.sel = (self.sel + 8).min(n.saturating_sub(1)),
                    Some(Named::PageUp) => self.sel = self.sel.saturating_sub(8),
                    Some(Named::Escape) => {
                        if self.query.is_empty() {
                            std::process::exit(0);
                        }
                        self.query.clear();
                        self.sel = 0;
                    }
                    Some(Named::Delete) => {
                        if let Some(l) = self.current() {
                            self.screen = Screen::Confirm(l.id);
                        }
                    }
                    Some(Named::F2) => return self.current().map(|l| self.start_edit(Some(l.id))).unwrap_or(Task::none()),
                    _ => {}
                }
                if ctrl {
                    match ch.as_str() {
                        "n" => return self.start_edit(None),
                        "e" => return self.current().map(|l| self.start_edit(Some(l.id))).unwrap_or(Task::none()),
                        "b" => {
                            if let Some(l) = self.current() {
                                return self.copy("user name", l.user);
                            }
                        }
                        "l" => return self.lock(),
                        "x" => self.export(),
                        _ => {}
                    }
                }
            }
            Screen::Edit(_) => {
                match named {
                    Some(Named::Escape) => {
                        self.screen = Screen::List;
                        self.form = Form::default();
                        return self.focus(0);
                    }
                    Some(Named::Tab) if mods.shift() => {
                        self.field = (self.field + FIELDS.len() - 1) % FIELDS.len();
                        return self.focus(self.field + 1);
                    }
                    Some(Named::Tab) | Some(Named::ArrowDown) => {
                        self.field = (self.field + 1) % FIELDS.len();
                        return self.focus(self.field + 1);
                    }
                    Some(Named::ArrowUp) => {
                        self.field = (self.field + FIELDS.len() - 1) % FIELDS.len();
                        return self.focus(self.field + 1);
                    }
                    _ => {}
                }
                if ctrl && ch == "g" {
                    let pw = generate();
                    if pw.is_empty() {
                        self.say("No random numbers to make a password with.");
                    } else {
                        self.form.password = pw;
                        self.reveal = true;
                        self.say("A new strong password. Enter saves it.");
                    }
                }
                if ctrl && ch == "r" {
                    self.reveal = !self.reveal;
                }
            }
            Screen::Confirm(id) => match named {
                Some(Named::Enter) => self.delete(id),
                _ => {
                    self.screen = Screen::List;
                    return self.focus(0);
                }
            },
        }
        Task::none()
    }

    fn submit(&mut self) -> Task<Message> {
        match self.screen.clone() {
            Screen::Create => {
                if self.master.chars().count() < MIN_MASTER {
                    self.say(format!("Use at least {MIN_MASTER} characters: a few words you will remember make a good one."));
                    return self.focus(0);
                }
                if self.master != self.master2 {
                    self.say("The two passwords are not the same.");
                    self.master2.clear();
                    return self.focus(1);
                }
                self.busy = true;
                let master = self.master.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let db = Database::with_config(config());
                            save(&db, &master, &vault_path()).map(|_| (db, master))
                        })
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    |r| cosmic::Action::App(Message::Opened(r)),
                )
            }
            Screen::Locked => {
                if self.master.is_empty() {
                    return Task::none();
                }
                self.busy = true;
                let master = self.master.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || open(&master, &vault_path()).map(|db| (db, master)))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    },
                    |r| cosmic::Action::App(Message::Opened(r)),
                )
            }
            Screen::List => match self.current() {
                Some(l) => {
                    let pw = self.secret(l.id, fields::PASSWORD);
                    self.copy("password", pw)
                }
                None => Task::none(),
            },
            Screen::Edit(_) => self.save_form(),
            Screen::Confirm(id) => {
                self.delete(id);
                self.focus(0)
            }
        }
    }

    fn field_input(&self, i: usize) -> Element<'_, Message> {
        let value = match i {
            0 => &self.form.title,
            1 => &self.form.user,
            2 => &self.form.password,
            3 => &self.form.url,
            _ => &self.form.notes,
        };
        let mut t = text_input::text_input(FIELDS[i], value.as_str())
            .id(self.ids[i + 1].clone())
            .on_input(move |s| Message::Field(i, s))
            .on_submit(|_| Message::Submit)
            .style(field_style())
            .size(21)
            .padding([12, 16]);
        if i == 2 && !self.reveal {
            t = t.password();
        }
        let on = i == self.field;
        widget::Column::new()
            .push(text(FIELDS[i].to_uppercase(), MONO, 13.0, if on { YELLOW } else { TEXT_2 }))
            .push(bracket(t, if on { 1.0 } else { 0.0 }, 6.0))
            .spacing(6)
            .into()
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Application for Passwords {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "chlorum-passwords";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, _: ()) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let p = Passwords {
            core,
            screen: if vault_path().exists() { Screen::Locked } else { Screen::Create },
            vault: None,
            all: vec![],
            master: String::new(),
            master2: String::new(),
            busy: false,
            note: None,
            query: String::new(),
            sel: 0,
            form: Form::default(),
            field: 0,
            reveal: false,
            last_key: Instant::now(),
            copied_at: None,
            ids: std::array::from_fn(|_| widget::Id::unique()),
        };
        let t = p.focus(0);
        (p, t)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            event::listen_with(|e, status, _| match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => Some(Message::Key(key, modifiers, status)),
                _ => None,
            }),
            iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
        ])
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Key(k, mods, status) => return self.key(k, mods, status),
            Message::Master(s) => self.master = s,
            Message::Master2(s) => self.master2 = s,
            Message::Submit => return self.submit(),
            Message::Opened(r) => {
                self.busy = false;
                self.master2.clear();
                match r {
                    Ok((db, master)) => {
                        let new = self.screen == Screen::Create;
                        self.vault = Some(Open { db, master });
                        self.master.clear();
                        self.screen = Screen::List;
                        self.refresh();
                        self.last_key = Instant::now();
                        if new {
                            self.say("Your vault is ready. Ctrl+N adds a login.");
                        }
                        return self.focus(0);
                    }
                    Err(e) => {
                        self.master.clear();
                        self.say(e);
                        return self.focus(0);
                    }
                }
            }
            Message::Query(q) => {
                self.query = q;
                self.sel = 0;
            }
            Message::Field(i, s) => {
                self.field = i;
                match i {
                    0 => self.form.title = s,
                    1 => self.form.user = s,
                    2 => self.form.password = s,
                    3 => self.form.url = s,
                    _ => self.form.notes = s,
                }
            }
            Message::Click(i) => {
                self.sel = i;
                self.last_key = Instant::now();
            }
            Message::Tick => {
                // the copied password does not stay in the clipboard
                if self.copied_at.is_some_and(|t| t.elapsed() >= CLEAR_AFTER) {
                    self.copied_at = None;
                    self.say("The clipboard is empty again.");
                    return cosmic::iced::clipboard::write(String::new());
                }
                if self.vault.is_some() && self.last_key.elapsed() >= LOCK_AFTER {
                    let t = self.lock();
                    self.say("Locked after five minutes without a key.");
                    return t;
                }
                if self.note.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(8)) {
                    self.note = None;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(14).width(Length::Fill);
        col = col.push(
            widget::Row::new()
                .push(widget::icon::from_name("dialog-password").size(56))
                .push(text("Passwords", SANS_BOLD, 40.0, TEXT))
                .spacing(18)
                .align_y(Alignment::Center),
        );
        let hints_now: Vec<(&str, &str, bool)>;
        match &self.screen {
            Screen::Create | Screen::Locked => {
                let create = self.screen == Screen::Create;
                let mut inner = widget::Column::new().spacing(14).width(Length::Fixed(620.0));
                inner = inner
                    .push(text(if create { "Make your vault" } else { "Locked" }, SANS_BOLD, 30.0, TEXT))
                    .push(text(
                        if create {
                            "Choose one password for all your other passwords. Nobody can bring it back if it is forgotten, so make it long and easy for you to remember: four or five words work well."
                        } else {
                            "Type the vault's password."
                        },
                        SANS,
                        19.0,
                        TEXT_2,
                    ))
                    .push(bracket(
                        text_input::text_input(if create { "Vault password" } else { "Password" }, self.master.as_str())
                            .id(self.ids[0].clone())
                            .on_input(Message::Master)
                            .on_submit(|_| Message::Submit)
                            .password()
                            .style(field_style())
                            .size(22)
                            .padding([14, 18]),
                        1.0,
                        8.0,
                    ));
                if create {
                    inner = inner.push(bracket(
                        text_input::text_input("The same again", self.master2.as_str())
                            .id(self.ids[1].clone())
                            .on_input(Message::Master2)
                            .on_submit(|_| Message::Submit)
                            .password()
                            .style(field_style())
                            .size(22)
                            .padding([14, 18]),
                        0.0,
                        8.0,
                    ));
                }
                if self.busy {
                    inner = inner.push(text(if create { "Making the vault…" } else { "Opening…" }, SANS_BOLD, 20.0, YELLOW));
                }
                col = col.push(vfill()).push(widget::container(inner).width(Length::Fill).align_x(Alignment::Center)).push(vfill());
                hints_now = vec![("ENTER", if create { "Make it" } else { "Open" }, true), ("ESC", "Close", false)];
            }
            Screen::List | Screen::Confirm(_) => {
                let shown = self.shown();
                col = col.push(bracket(
                    text_input::text_input("Type to find a login", self.query.as_str())
                        .id(self.ids[0].clone())
                        .on_input(Message::Query)
                        .on_submit(|_| Message::Submit)
                        .style(field_style())
                        .size(22)
                        .padding([14, 18])
                        .width(Length::Fixed(720.0)),
                    1.0,
                    8.0,
                ));
                if shown.is_empty() {
                    col = col.push(text(
                        if self.all.is_empty() { "No logins yet. Ctrl+N adds one." } else { "Nothing is called like that. Esc clears the search." },
                        SANS,
                        21.0,
                        TEXT_2,
                    ));
                }
                let fit = 9usize;
                let first = self.sel.saturating_sub(fit / 2).min(shown.len().saturating_sub(fit));
                let mut list = widget::Column::new().spacing(6).width(Length::Fill);
                for (i, l) in shown.iter().enumerate().skip(first).take(fit) {
                    let on = i == self.sel;
                    let row = fill(
                        widget::Row::new()
                            .push(widget::Column::new().push(text(l.title.clone(), SANS_BOLD, 21.0, if on { BLACK } else { TEXT })).push(text(
                                if l.user.is_empty() { l.url.clone() } else { l.user.clone() },
                                SANS,
                                16.0,
                                if on { BLACK } else { TEXT_2 },
                            )))
                            .push(hfill())
                            .push(text(l.url.clone(), MONO, 14.0, if on { BLACK } else { MUTED }))
                            .align_y(Alignment::Center),
                        if on { YELLOW } else { PANEL },
                    )
                    .padding([12, 20])
                    .width(Length::Fill);
                    list = list.push(widget::mouse_area(row).on_press(Message::Click(i)));
                }
                col = col.push(list);
                if let Screen::Confirm(id) = &self.screen {
                    col = col.push(
                        fill(text(format!("Delete {}? Enter deletes, any other key keeps it.", self.secret(*id, fields::TITLE)), SANS_BOLD, 20.0, BLACK), RED)
                            .padding([14, 20]),
                    );
                }
                col = col.push(vfill());
                hints_now = vec![
                    ("TYPE", "Find", false),
                    ("↑ ↓", "Choose", false),
                    ("ENTER", "Copy password", true),
                    ("CTRL B", "Copy user", false),
                    ("CTRL N", "New", false),
                    ("CTRL E", "Change", false),
                    ("DEL", "Delete", false),
                    ("CTRL X", "Copy to Library", false),
                    ("CTRL L", "Lock", false),
                ];
            }
            Screen::Edit(id) => {
                col = col.push(text(if id.is_some() { "Change the login" } else { "New login" }, SANS_BOLD, 28.0, TEXT));
                let mut form = widget::Column::new().spacing(12).width(Length::Fixed(720.0));
                for i in 0..FIELDS.len() {
                    form = form.push(self.field_input(i));
                }
                col = col.push(form).push(vfill());
                hints_now = vec![
                    ("TAB ↑ ↓", "Field", false),
                    ("CTRL G", "Strong password", false),
                    ("CTRL R", if self.reveal { "Hide" } else { "Show" }, false),
                    ("ENTER", "Save", true),
                    ("ESC", "Back", false),
                ];
            }
        }
        if let Some((n, _)) = &self.note {
            col = col.push(text(n.clone(), SANS_BOLD, 18.0, YELLOW));
        }
        col = col.push(hints(&hints_now, 1.0));
        fill(col.padding([40, 56]), BLACK).width(Length::Fill).height(Length::Fill).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_are_strong() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), 20);
        assert_ne!(a, b);
        assert!(a.chars().any(|c| c.is_ascii_digit()) && a.chars().any(|c| c.is_ascii_uppercase()));
    }

    #[test]
    fn vault_round_trip() {
        let dir = std::env::temp_dir().join(format!("chlorum-pw-{}", std::process::id()));
        let path = dir.join("v.kdbx");
        let mut db = Database::with_config(config());
        {
            let mut root = db.root_mut();
            let mut e = root.add_entry();
            e.set_unprotected(fields::TITLE, "Mail");
            e.set_unprotected(fields::USERNAME, "roman");
            e.set_protected(fields::PASSWORD, "s3cret-words-here");
            e.set_unprotected(fields::URL, "https://mail.example");
        }
        save(&db, "correct horse battery", &path).unwrap();
        assert_eq!(open("wrong password", &path).unwrap_err(), "Wrong password.");
        let back = open("correct horse battery", &path).unwrap();
        let l = logins(&back);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].title.as_str(), l[0].user.as_str()), ("Mail", "roman"));
        assert_eq!(back.entry(l[0].id).unwrap().get_password(), Some("s3cret-words-here"));
        // saved again: the one before is kept
        save(&back, "correct horse battery", &path).unwrap();
        assert!(path.with_extension("kdbx.bak").exists());
        let found = find(&l, "MAIL");
        assert_eq!(found.len(), 1);
        assert!(find(&l, "nothing").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
