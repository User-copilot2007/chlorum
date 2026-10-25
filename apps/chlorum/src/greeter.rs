//! `chlorum greeter` — the first start and the login screen, inside niri
//! under greetd.
//!
//! First start: keyboard → internet → profile, then straight into Chlorum.
//! Afterwards: profiles in a row, the password under them, and under that
//! "Show" and "Power". Arrows move where they point; nothing needs a mouse.

use crate::atom::Atom;
use crate::niri::{self, Action};
use crate::setupd;
use crate::ui::*;
use chrono::{Local, Timelike};
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{self, Alignment, Background, Border, Length, Subscription, event, keyboard};
use cosmic::widget::text_input;
use cosmic::{Application, Element, widget};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run() -> cosmic::iced::Result {
    let settings = Settings::default()
        .default_font(SANS)
        .client_decorations(false)
        .default_icon_theme("Papirus-Dark")
        .size(iced::Size::new(1920.0, 1080.0));
    cosmic::app::run::<Greeter>(settings, ())
}

const SESSION: &str = "chlorum-session";

#[derive(Clone, Debug)]
pub struct Profile {
    login: String,
    name: String,
}

#[derive(Clone, Debug)]
struct Net {
    ssid: Option<String>, // None: continue without internet
    signal: u8,
    secure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Retry {
    Keyboard,
    Network,
    Profile,
    Create,
}

#[derive(Clone, Debug, PartialEq)]
enum Screen {
    Login,
    Keyboard,
    Checking,
    Networks,
    WifiPassword,
    Connecting,
    Proposal,
    Own,
    Preparing,
    Creating,
    Error(String, Retry),
    Starting,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Profiles,
    Field,
    Buttons(usize), // 0 Show, 1 Power
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Login,
    Wifi,
    OwnName,
    OwnPass,
    OwnPass2,
}

#[derive(Clone, Debug)]
pub enum Message {
    Tick,
    Frame,
    Key(keyboard::Key, event::Status),
    Input(Field, String),
    Submit(Field),
    Reply(&'static str, Value),
    Profiles(Vec<Profile>),
    Done(Result<(), String>),
}

pub struct Greeter {
    core: Core,
    screen: Screen,
    // login
    profiles: Vec<Profile>,
    sel: usize,
    zone: Zone,
    password: String,
    show: bool,
    error: Option<String>,
    power: Option<usize>,
    armed: bool,
    // first start
    layout_sel: usize,
    nets: Vec<Net>,
    net_sel: usize,
    wifi_ssid: String,
    wifi_pw: String,
    proposal: (String, String),
    choice: usize, // 0 use these, 1 choose my own
    own: [String; 3],
    own_focus: usize,
    creating: Option<(String, String)>,
    // inputs
    ids: [widget::Id; 5],
    started: Instant,
    appear: f32,
    last: Instant,
}

fn setup_done() -> bool {
    Path::new(&format!("{}/setup-done", setupd::STATE)).exists() || std::env::var_os("CHLORUM_TEST_LOGIN").is_some()
}

fn ask(op: &'static str, body: Value) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let mut req = body;
                req["op"] = json!(op);
                setupd::call(req)
            })
            .await
            .unwrap_or_else(|e| json!({"error": e.to_string()}))
        },
        move |v| cosmic::Action::App(Message::Reply(op, v)),
    )
}

impl Application for Greeter {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "com.chlorum.Greeter";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, _: ()) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.content_container = false;
        let current = std::fs::read_to_string(format!("{}/xkb-layout", setupd::STATE)).unwrap_or_default();
        let mut g = Greeter {
            core,
            screen: Screen::Login,
            profiles: vec![],
            sel: 0,
            zone: Zone::Field,
            password: String::new(),
            show: false,
            error: None,
            power: None,
            armed: false,
            layout_sel: setupd::LAYOUTS.iter().position(|l| l.0 == current.trim()).unwrap_or(0),
            nets: vec![],
            net_sel: 0,
            wifi_ssid: String::new(),
            wifi_pw: String::new(),
            proposal: (String::new(), String::new()),
            choice: 0,
            own: Default::default(),
            own_focus: 0,
            creating: None,
            ids: std::array::from_fn(|_| widget::Id::unique()),
            started: Instant::now(),
            appear: 0.0,
            last: Instant::now(),
        };
        let mut tasks = vec![];
        if setup_done() {
            // homed may take a moment to answer: show the screen first
            tasks.push(Task::perform(
                async { tokio::task::spawn_blocking(profiles).await.unwrap_or_default() },
                |p| cosmic::Action::App(Message::Profiles(p)),
            ));
            tasks.push(g.focus(Field::Login));
        } else {
            g.screen = Screen::Keyboard;
        }
        if let Some(id) = g.core.main_window_id() {
            tasks.push(iced::window::set_mode(id, iced::window::Mode::Fullscreen));
        }
        (g, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut s = vec![
            iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick),
            event::listen_with(|e, status, _| match e {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => Some(Message::Key(key, status)),
                _ => None,
            }),
        ];
        if self.busy() || self.appear < 1.0 {
            s.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::Frame));
        }
        Subscription::batch(s)
    }

    fn update(&mut self, m: Message) -> Task<Message> {
        match m {
            Message::Tick => {}
            Message::Frame => {
                let dt = self.last.elapsed().as_secs_f32().min(0.05);
                self.last = Instant::now();
                self.appear = (self.appear + dt / 0.4).min(1.0);
            }
            Message::Input(f, s) => {
                self.error = None;
                match f {
                    Field::Login => self.password = s,
                    Field::Wifi => self.wifi_pw = s,
                    Field::OwnName => self.own[0] = s.to_lowercase(),
                    Field::OwnPass => self.own[1] = s,
                    Field::OwnPass2 => self.own[2] = s,
                }
            }
            Message::Submit(f) => return self.submit(f),
            Message::Reply(op, v) => return self.reply(op, v),
            Message::Profiles(p) => {
                self.profiles = p;
                self.sel = self.sel.min(self.profiles.len().saturating_sub(1));
            }
            Message::Done(Ok(())) => {
                // greetd starts the session once this screen is gone
                return Task::perform(
                    async {
                        if std::env::var_os("NIRI_SOCKET").is_some() {
                            niri::run(vec![Action::Quit { skip_confirmation: true }]).await;
                        }
                    },
                    |_| std::process::exit(0),
                );
            }
            Message::Done(Err(e)) => {
                if setup_done() && self.profiles.is_empty() {
                    self.profiles = profiles();
                }
                self.screen = Screen::Login;
                self.error = Some(e);
                self.password.clear();
                self.zone = Zone::Field;
                return self.focus(Field::Login);
            }
            Message::Key(key, status) => return self.key(key, status),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let f = self.appear;
        let now = Local::now();
        let left = widget::Column::new()
            .push(vfill())
            .push(widget::icon::from_name("com.chlorum.Logo").size(440))
            .push(space(1.0, 30.0))
            .push(
                widget::Row::new()
                    .push(text(format!("{:02}:{:02}", now.hour(), now.minute()), MONO_BOLD, 80.0, alpha(TEXT, f)))
                    .push(text(format!(":{:02}", now.second()), MONO_BOLD, 80.0, alpha(YELLOW, f))),
            )
            .push(text(now.format("%A  ·  %-d %B").to_string().to_uppercase(), MONO, 21.0, alpha(TEXT_2, f)))
            .push(vfill())
            .width(Length::Fixed(600.0))
            .align_x(Alignment::Center);

        let mut panel = widget::Column::new().push(vfill()).push(match &self.screen {
            Screen::Login => self.login_view(),
            _ => self.setup_view(),
        });
        if let Some(sel) = self.power {
            let mut r = widget::Row::new().spacing(18);
            for (i, l) in ["Restart", "Shut down", "Cancel"].iter().enumerate() {
                let on = i == sel;
                let armed = on && self.armed && i < 2;
                let label = if armed { format!("Enter again to {}", l.to_lowercase()) } else { l.to_string() };
                let b = fill(text(label, SANS_BOLD, 21.0, if on { BLACK } else { TEXT }), if armed { RED } else if on { YELLOW } else { PANEL_2 })
                    .padding([16, 26]);
                r = r.push(bracket(b, if on { 1.0 } else { 0.0 }, 8.0));
            }
            panel = panel.push(space(1.0, 30.0)).push(r);
        }
        panel = panel.push(vfill());

        let body = widget::Column::new()
            .push(widget::Row::new().push(left).push(space(90.0, 1.0)).push(panel.width(Length::Fill)).height(Length::Fill))
            .push(hints(&self.hint_list(), f))
            .padding([48, 80]);
        fill(body, BLACK).width(Length::Fill).height(Length::Fill).into()
    }
}

impl Greeter {
    fn busy(&self) -> bool {
        matches!(self.screen, Screen::Checking | Screen::Connecting | Screen::Preparing | Screen::Creating | Screen::Starting)
    }

    fn id(&self, f: Field) -> widget::Id {
        self.ids[f as usize].clone()
    }

    fn focus(&self, f: Field) -> Task<Message> {
        text_input::focus(self.id(f))
    }

    fn hint_list(&self) -> Vec<(&'static str, &'static str, bool)> {
        if self.power.is_some() {
            return vec![("← →", "Choose", false), ("ENTER", "Twice to confirm", true), ("ESC", "Back", false)];
        }
        let mut h = match &self.screen {
            Screen::Login => match self.zone {
                Zone::Profiles => vec![("← →", "Profile", false), ("↓", "Password", false), ("ENTER", "Choose", true)],
                Zone::Field => vec![("↑", "Profile", false), ("↓", "Show password", false), ("ENTER", "Unlock", true)],
                Zone::Buttons(_) => vec![("← →", "Move", false), ("↑", "Password", false), ("ENTER", "Press", true)],
            },
            Screen::Keyboard | Screen::Networks => vec![("↑ ↓", "Choose", false), ("ENTER", "Next", true)],
            Screen::WifiPassword => vec![("ENTER", "Connect", true), ("ESC", "Back", false)],
            Screen::Proposal => vec![("← →", "Choose", false), ("ENTER", "Press", true)],
            Screen::Own => vec![("↑ ↓", "Field", false), ("ENTER", "Next / Create", true), ("ESC", "Back", false)],
            Screen::Error(..) => vec![("ENTER", "Try again", true)],
            _ => vec![],
        };
        if !self.busy() {
            h.push(("F10", "Power", false));
        }
        h
    }

    // ---------------------------------------------------------------- login

    fn login_view(&self) -> Element<'_, Message> {
        let mut who = widget::Row::new().spacing(34);
        for (i, p) in self.profiles.iter().enumerate() {
            let on = i == self.sel;
            let initial = p.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            let av = widget::container(text(initial, MONO_BOLD, 54.0, if on { BLACK } else { TEXT }))
                .width(Length::Fixed(124.0))
                .height(Length::Fixed(124.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .class(cosmic::theme::Container::custom(move |_| iced::widget::container::Style {
                    background: Some(Background::Color(if on { YELLOW } else { PANEL_2 })),
                    border: Border { radius: 62.0.into(), ..Default::default() },
                    ..Default::default()
                }));
            let card = fill(
                widget::Column::new()
                    .push(av)
                    .push(text(p.name.clone(), SANS_BOLD, 26.0, if on { TEXT } else { TEXT_2 }))
                    .spacing(22)
                    .align_x(Alignment::Center),
                PANEL,
            )
            .width(Length::Fixed(230.0))
            .height(Length::Fixed(270.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center);
            who = who.push(bracket(card, if on && self.zone == Zone::Profiles { 1.0 } else { 0.0 }, 10.0));
        }
        let field_on = self.zone == Zone::Field;
        let field: Element<'_, Message> = if self.screen == Screen::Starting || self.busy() {
            busy_row("Unlocking your profile…", self.started)
        } else {
            self.input(Field::Login, "Password", &self.password, !self.show, 640.0)
        };
        let button = |i: usize, label: String, sub: &'static str| -> Element<'_, Message> {
            let on = self.zone == Zone::Buttons(i) && self.power.is_none();
            let b = fill(
                widget::Column::new()
                    .push(text(label, SANS_BOLD, 22.0, if on { BLACK } else { TEXT }))
                    .push(text(sub, MONO, 14.0, if on { BLACK } else { TEXT_2 }))
                    .spacing(4)
                    .align_x(Alignment::Center),
                if on { YELLOW } else { PANEL_2 },
            )
            .width(Length::Fixed(200.0))
            .padding([16, 10])
            .align_x(Alignment::Center);
            bracket(b, if on { 1.0 } else { 0.0 }, 8.0)
        };
        let mut col = widget::Column::new()
            .push(who)
            .push(space(1.0, 56.0))
            .push(bracket(field, if field_on && self.power.is_none() { 1.0 } else { 0.0 }, 10.0))
            .push(space(1.0, 26.0))
            .push(
                widget::Row::new()
                    .push(button(0, if self.show { "Hide".into() } else { "Show".into() }, "PASSWORD"))
                    .push(button(1, "Power".into(), "F10"))
                    .spacing(22),
            );
        if let Some(e) = &self.error {
            col = col.push(space(1.0, 24.0)).push(text(e.clone(), SANS_BOLD, 22.0, RED));
        }
        if self.profiles.is_empty() {
            col = col.push(space(1.0, 24.0)).push(text("No profiles found.", SANS, 22.0, TEXT_2));
        }
        col.into()
    }

    fn input<'a>(&self, f: Field, placeholder: &'a str, value: &'a str, secret: bool, width: f32) -> Element<'a, Message> {
        let mut t = text_input::text_input(placeholder, value)
            .id(self.id(f))
            .size(30)
            .padding([22, 26])
            .width(Length::Fixed(width))
            .style(field_style())
            .on_input(move |s| Message::Input(f, s))
            .on_submit(move |_| Message::Submit(f));
        if secret {
            t = t.password();
        }
        t.into()
    }

    // ---------------------------------------------------------------- first start

    fn setup_view(&self) -> Element<'_, Message> {
        let (step, eyebrow) = match self.screen {
            Screen::Keyboard => (1, "KEYBOARD"),
            Screen::Checking | Screen::Networks | Screen::WifiPassword | Screen::Connecting => (2, "INTERNET"),
            _ => (3, "YOUR PROFILE"),
        };
        let mut steps = widget::Row::new().spacing(10);
        for i in 1..=3 {
            steps = steps.push(fill(space(70.0, 6.0), if i <= step { YELLOW } else { LINE }));
        }
        let mut col = widget::Column::new()
            .spacing(18)
            .push(steps)
            .push(text(format!("STEP {step} OF 3 · {eyebrow}"), MONO, 16.0, YELLOW));
        let title = |s: String| text(s, SANS_BOLD, 46.0, TEXT);
        let lead = |s: &str| text(s.to_string(), SANS, 22.0, TEXT_2);
        match &self.screen {
            Screen::Keyboard => {
                col = col.push(title("Which keyboard do you have?".into())).push(lead(
                    "Pick the layout printed on your keys, so what you type matches what you see.",
                ));
                let rows = setupd::LAYOUTS.iter().map(|l| (l.2.to_string(), l.0.to_uppercase())).collect();
                col = col.push(list(rows, self.layout_sel));
            }
            Screen::Checking => col = col.push(title("Internet".into())).push(busy_row("Checking the connection…", self.started)),
            Screen::Networks => {
                col = col
                    .push(title("Connect to Wi-Fi".into()))
                    .push(lead("There is no internet over a cable. Choose your network."));
                let rows = self
                    .nets
                    .iter()
                    .map(|n| match &n.ssid {
                        Some(s) => (s.clone(), format!("{}{}%", if n.secure { "LOCKED · " } else { "" }, n.signal)),
                        None => ("Continue without internet".into(), "SKIP".into()),
                    })
                    .collect();
                col = col.push(list(rows, self.net_sel));
            }
            Screen::WifiPassword => {
                col = col
                    .push(title(self.wifi_ssid.clone()))
                    .push(text("WI-FI PASSWORD", MONO, 15.0, TEXT_2))
                    .push(bracket(self.input(Field::Wifi, "Password", &self.wifi_pw, true, 640.0), 1.0, 10.0));
                if let Some(e) = &self.error {
                    col = col.push(text(e.clone(), SANS_BOLD, 21.0, RED));
                }
            }
            Screen::Connecting => {
                col = col.push(title(self.wifi_ssid.clone())).push(busy_row("Connecting…", self.started));
            }
            Screen::Proposal => {
                col = col.push(title("Write these down on paper".into()));
                let secret = |k: &'static str, v: String| -> Element<'_, Message> {
                    fill(
                        widget::Column::new()
                            .push(text(k, MONO, 15.0, TEXT_2))
                            .push(text(v, MONO_BOLD, 38.0, YELLOW))
                            .spacing(10),
                        PANEL,
                    )
                    .padding([22, 28])
                    .width(Length::Fixed(900.0))
                    .into()
                };
                col = col
                    .push(secret("PROFILE NAME", self.proposal.0.clone()))
                    .push(secret("PASSWORD", self.proposal.1.clone()))
                    .push(lead(
                        "They are shown only once. Never photograph them or send them to anyone. \
                         Your profile is encrypted with this password; without it nobody can open it, not even you.",
                    ).apply_width(900.0));
                let mut r = widget::Row::new().spacing(20);
                for (i, l) in ["I wrote them down", "Choose my own"].iter().enumerate() {
                    let on = i == self.choice && self.power.is_none();
                    let b = fill(text(l.to_string(), SANS_BOLD, 22.0, if on { BLACK } else { TEXT }), if on { YELLOW } else { PANEL_2 })
                        .padding([18, 30]);
                    r = r.push(bracket(b, if on { 1.0 } else { 0.0 }, 8.0));
                }
                col = col.push(r);
            }
            Screen::Own => {
                col = col.push(title("Choose your own".into()));
                let labels = [
                    "PROFILE NAME · lowercase letters, digits and -",
                    "PASSWORD · at least 12 characters",
                    "REPEAT PASSWORD",
                ];
                let fields = [Field::OwnName, Field::OwnPass, Field::OwnPass2];
                for i in 0..3 {
                    let input = self.input(fields[i], "", &self.own[i], i > 0, 640.0);
                    col = col
                        .push(text(labels[i], MONO, 15.0, if i == self.own_focus { TEXT } else { TEXT_2 }))
                        .push(bracket(input, if i == self.own_focus { 1.0 } else { 0.0 }, 8.0));
                }
                if let Some(e) = &self.error {
                    col = col.push(text(e.clone(), SANS_BOLD, 21.0, RED));
                }
            }
            Screen::Preparing => col = col.push(title("Your profile".into())).push(busy_row("Preparing…", self.started)),
            Screen::Creating => {
                col = col
                    .push(title("Creating your profile".into()))
                    .push(busy_row("Encrypting your space. This takes a minute…", self.started));
            }
            Screen::Starting => col = col.push(title("All set".into())).push(busy_row("Starting Chlorum…", self.started)),
            Screen::Error(e, _) => {
                col = col
                    .push(text("SOMETHING WENT WRONG", MONO, 16.0, RED))
                    .push(title("Setup could not continue".into()))
                    .push(text(e.clone(), SANS, 22.0, TEXT).apply_width(900.0));
                col = col.push(bracket(fill(text("Try again", SANS_BOLD, 22.0, BLACK), YELLOW).padding([18, 30]), 1.0, 8.0));
            }
            Screen::Login => {}
        }
        col.into()
    }

    fn submit(&mut self, f: Field) -> Task<Message> {
        match f {
            Field::Login => self.login(),
            Field::Wifi => {
                self.screen = Screen::Connecting;
                self.started = Instant::now();
                ask("wifi", json!({"ssid": self.wifi_ssid, "password": self.wifi_pw}))
            }
            Field::OwnName | Field::OwnPass => {
                self.own_focus += 1;
                self.focus([Field::OwnName, Field::OwnPass, Field::OwnPass2][self.own_focus])
            }
            Field::OwnPass2 => self.create_own(),
        }
    }

    fn create_own(&mut self) -> Task<Message> {
        let name = self.own[0].trim().to_string();
        let ok_name = name.len() >= 2
            && name.len() <= 31
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok_name {
            self.error = Some("Use 2 to 31 lowercase letters, digits or -, starting with a letter.".into());
            self.own_focus = 0;
            return self.focus(Field::OwnName);
        }
        if self.own[1].chars().count() < 12 {
            self.error = Some("The password must be at least 12 characters.".into());
            self.own_focus = 1;
            return self.focus(Field::OwnPass);
        }
        if self.own[1] != self.own[2] {
            self.error = Some("The passwords do not match.".into());
            self.own[2].clear();
            self.own_focus = 2;
            return self.focus(Field::OwnPass2);
        }
        self.create(name, self.own[1].clone())
    }

    fn create(&mut self, name: String, pw: String) -> Task<Message> {
        self.screen = Screen::Creating;
        self.started = Instant::now();
        self.creating = Some((name.clone(), pw.clone()));
        ask("create", json!({"name": name, "password": pw}))
    }

    fn start(&mut self, step: Retry) -> Task<Message> {
        self.error = None;
        self.started = Instant::now();
        match step {
            Retry::Keyboard => {
                self.screen = Screen::Keyboard;
                Task::none()
            }
            Retry::Network => {
                self.screen = Screen::Checking;
                ask("network", json!({}))
            }
            Retry::Profile => {
                self.screen = Screen::Preparing;
                ask("proposal", json!({}))
            }
            Retry::Create => match self.creating.clone() {
                Some((n, p)) => self.create(n, p),
                None => self.start(Retry::Profile),
            },
        }
    }

    fn reply(&mut self, op: &'static str, v: Value) -> Task<Message> {
        if let Some(e) = v["error"].as_str() {
            let e = e.to_string();
            return match op {
                "wifi" => {
                    self.screen = Screen::WifiPassword;
                    self.error = Some(e);
                    self.wifi_pw.clear();
                    self.focus(Field::Wifi)
                }
                "create" if self.own_focus > 0 || !self.own[0].is_empty() => {
                    self.screen = Screen::Own;
                    self.error = Some(e);
                    self.focus(Field::OwnName)
                }
                _ => {
                    let retry = match op {
                        "set_layout" => Retry::Keyboard,
                        "network" => Retry::Network,
                        "proposal" => Retry::Profile,
                        _ => Retry::Create,
                    };
                    self.screen = Screen::Error(e, retry);
                    Task::none()
                }
            };
        }
        match op {
            "set_layout" => {
                // the new layout works at once: niri reloads its config
                if let Ok(path) = std::env::var("CHLORUM_NIRI_CONFIG") {
                    crate::system::set_niri_layout(Path::new(&path), setupd::LAYOUTS[self.layout_sel].0);
                }
                self.start(Retry::Network)
            }
            "network" => {
                if v["online"].as_bool() == Some(true) {
                    return self.start(Retry::Profile);
                }
                self.nets = v["wifi"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|n| Net {
                                ssid: n["ssid"].as_str().map(str::to_string),
                                signal: n["signal"].as_u64().unwrap_or(0) as u8,
                                secure: n["secure"].as_bool().unwrap_or(true),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.nets.push(Net { ssid: None, signal: 0, secure: false });
                self.net_sel = 0;
                self.screen = Screen::Networks;
                Task::none()
            }
            "wifi" => self.start(Retry::Profile),
            "proposal" => {
                self.proposal = (
                    v["name"].as_str().unwrap_or("").to_string(),
                    v["password"].as_str().unwrap_or("").to_string(),
                );
                self.choice = 0;
                self.screen = Screen::Proposal;
                Task::none()
            }
            "create" => ask("finish", json!({})),
            "finish" => {
                // straight in: no need to type what was just chosen
                self.screen = Screen::Starting;
                self.started = Instant::now();
                let (user, pw) = self.creating.clone().unwrap_or_default();
                Task::perform(
                    async move { tokio::task::spawn_blocking(move || greetd_login(&user, &pw)).await.unwrap_or_else(|e| Err(e.to_string())) },
                    |r| cosmic::Action::App(Message::Done(r)),
                )
            }
            _ => Task::none(),
        }
    }

    // ---------------------------------------------------------------- keys

    fn key(&mut self, key: keyboard::Key, status: event::Status) -> Task<Message> {
        use keyboard::key::Named;
        if self.busy() {
            return Task::none();
        }
        let named = match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        };
        // Power, from anywhere
        if named == Some(Named::F10) {
            self.power = Some(0);
            self.armed = false;
            return Task::none();
        }
        if let Some(i) = self.power {
            match named {
                Some(Named::ArrowLeft) => {
                    self.power = Some(i.saturating_sub(1));
                    self.armed = false;
                }
                Some(Named::ArrowRight) => {
                    self.power = Some((i + 1).min(2));
                    self.armed = false;
                }
                Some(Named::Escape) | Some(Named::ArrowUp) => self.power = None,
                Some(Named::Enter) => {
                    if i == 2 {
                        self.power = None;
                    } else if self.armed {
                        let _ = std::process::Command::new("systemctl").arg(if i == 0 { "reboot" } else { "poweroff" }).spawn();
                    } else {
                        self.armed = true;
                    }
                }
                _ => {}
            }
            return Task::none();
        }
        let n = |len: usize| len.max(1) - 1;
        match self.screen.clone() {
            Screen::Login => return self.login_key(key, named, status),
            Screen::Keyboard => match named {
                Some(Named::ArrowDown) => self.layout_sel = (self.layout_sel + 1).min(n(setupd::LAYOUTS.len())),
                Some(Named::ArrowUp) => self.layout_sel = self.layout_sel.saturating_sub(1),
                Some(Named::Enter) => {
                    self.screen = Screen::Checking;
                    self.started = Instant::now();
                    return ask("set_layout", json!({"id": setupd::LAYOUTS[self.layout_sel].0}));
                }
                _ => {}
            },
            Screen::Networks => match named {
                Some(Named::ArrowDown) => self.net_sel = (self.net_sel + 1).min(n(self.nets.len())),
                Some(Named::ArrowUp) => self.net_sel = self.net_sel.saturating_sub(1),
                Some(Named::Escape) => self.screen = Screen::Keyboard,
                Some(Named::Enter) => {
                    let net = self.nets[self.net_sel].clone();
                    match net.ssid {
                        None => return self.start(Retry::Profile),
                        Some(s) => {
                            self.wifi_ssid = s;
                            self.wifi_pw.clear();
                            if net.secure {
                                self.screen = Screen::WifiPassword;
                                return self.focus(Field::Wifi);
                            }
                            return self.submit(Field::Wifi);
                        }
                    }
                }
                _ => {}
            },
            Screen::WifiPassword => {
                if named == Some(Named::Escape) {
                    self.screen = Screen::Networks;
                    self.error = None;
                }
            }
            Screen::Proposal => match named {
                Some(Named::ArrowLeft) => self.choice = 0,
                Some(Named::ArrowRight) | Some(Named::Tab) => self.choice = 1,
                Some(Named::Enter) => {
                    if self.choice == 0 {
                        let (n, p) = self.proposal.clone();
                        return self.create(n, p);
                    }
                    self.screen = Screen::Own;
                    self.own_focus = 0;
                    return self.focus(Field::OwnName);
                }
                _ => {}
            },
            Screen::Own => {
                let fields = [Field::OwnName, Field::OwnPass, Field::OwnPass2];
                match named {
                    Some(Named::Escape) => {
                        self.screen = Screen::Proposal;
                        self.error = None;
                    }
                    Some(Named::ArrowDown) | Some(Named::Tab) => {
                        self.own_focus = (self.own_focus + 1).min(2);
                        return self.focus(fields[self.own_focus]);
                    }
                    Some(Named::ArrowUp) => {
                        self.own_focus = self.own_focus.saturating_sub(1);
                        return self.focus(fields[self.own_focus]);
                    }
                    _ => {}
                }
            }
            Screen::Error(_, retry) => {
                if named == Some(Named::Enter) {
                    return self.start(retry);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn login_key(&mut self, key: keyboard::Key, named: Option<keyboard::key::Named>, status: event::Status) -> Task<Message> {
        use keyboard::key::Named;
        let Some(named) = named else {
            // typing anywhere goes to the password
            let _ = key;
            if self.zone != Zone::Field {
                self.zone = Zone::Field;
                return self.focus(Field::Login);
            }
            return Task::none();
        };
        let n = self.profiles.len();
        match (self.zone, named) {
            (Zone::Profiles, Named::ArrowLeft) => self.sel = self.sel.saturating_sub(1),
            (Zone::Profiles, Named::ArrowRight) if n > 0 => self.sel = (self.sel + 1).min(n - 1),
            (Zone::Profiles, Named::ArrowDown | Named::Enter | Named::Tab) => {
                self.zone = Zone::Field;
                return self.focus(Field::Login);
            }
            (Zone::Field, Named::ArrowUp) => self.zone = Zone::Profiles,
            (Zone::Field, Named::ArrowDown | Named::Tab) => self.zone = Zone::Buttons(0),
            (Zone::Field, Named::Escape) => self.password.clear(),
            (Zone::Field, Named::Enter) if status == event::Status::Ignored => return self.login(),
            (Zone::Buttons(i), Named::ArrowLeft) => self.zone = Zone::Buttons(i.saturating_sub(1)),
            (Zone::Buttons(i), Named::ArrowRight) => self.zone = Zone::Buttons((i + 1).min(1)),
            (Zone::Buttons(_), Named::ArrowUp | Named::Escape) => {
                self.zone = Zone::Field;
                return self.focus(Field::Login);
            }
            (Zone::Buttons(0), Named::Enter) => {
                self.show = !self.show;
                self.zone = Zone::Field;
                return self.focus(Field::Login);
            }
            (Zone::Buttons(1), Named::Enter) => {
                self.power = Some(0);
                self.armed = false;
            }
            _ => {}
        }
        Task::none()
    }

    fn login(&mut self) -> Task<Message> {
        let Some(p) = self.profiles.get(self.sel) else { return Task::none() };
        if self.password.is_empty() {
            self.error = Some("Type your password first.".into());
            return Task::none();
        }
        self.screen = Screen::Starting;
        self.started = Instant::now();
        self.error = None;
        let (user, pw) = (p.login.clone(), self.password.clone());
        Task::perform(
            async move { tokio::task::spawn_blocking(move || greetd_login(&user, &pw)).await.unwrap_or_else(|e| Err(e.to_string())) },
            |r| cosmic::Action::App(Message::Done(r)),
        )
    }
}

trait Width<'a> {
    fn apply_width(self, w: f32) -> Element<'a, Message>;
}
impl<'a> Width<'a> for Element<'a, Message> {
    fn apply_width(self, w: f32) -> Element<'a, Message> {
        widget::container(self).width(Length::Fixed(w)).into()
    }
}

fn busy_row<'a>(label: &str, started: Instant) -> Element<'a, Message> {
    widget::Row::new()
        .push(Atom::view(96.0, started))
        .push(text(label.to_string(), SANS_BOLD, 26.0, TEXT))
        .spacing(26)
        .align_y(Alignment::Center)
        .height(Length::Fixed(110.0))
        .into()
}

/// A vertical list with one chosen row (name left, detail right).
fn list<'a>(rows: Vec<(String, String)>, sel: usize) -> Element<'a, Message> {
    let mut col = widget::Column::new().spacing(6).width(Length::Fixed(760.0));
    // keep the chosen row in view on short screens: show a window of 8 rows
    let start = sel.saturating_sub(5).min(rows.len().saturating_sub(8));
    for (i, (a, b)) in rows.into_iter().enumerate().skip(start).take(8) {
        let on = i == sel;
        col = col.push(
            fill(
                widget::Row::new()
                    .push(text(a, SANS_BOLD, 23.0, if on { BLACK } else { TEXT }))
                    .push(hfill())
                    .push(text(b, MONO, 15.0, if on { BLACK } else { TEXT_2 }))
                    .align_y(Alignment::Center),
                if on { YELLOW } else { PANEL },
            )
            .padding([16, 24])
            .width(Length::Fill),
        );
    }
    col.into()
}


fn profiles() -> Vec<Profile> {
    let mut v: Vec<Profile> = setupd::profiles()
        .into_iter()
        .map(|login| {
            let name = std::process::Command::new("getent")
                .args(["passwd", &login])
                .output()
                .ok()
                .and_then(|o| {
                    let l = String::from_utf8_lossy(&o.stdout).to_string();
                    l.split(':').nth(4).map(|g| g.split(',').next().unwrap_or("").trim().to_string())
                })
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| login.clone());
            Profile { login, name }
        })
        .collect();
    if let Ok(extra) = std::env::var("CHLORUM_TEST_PROFILES") {
        for p in extra.split(',') {
            v.push(Profile { login: p.to_lowercase(), name: p.to_string() });
        }
    }
    v
}

// ---------------------------------------------------------------- greetd

struct Greetd(UnixStream);

impl Greetd {
    fn connect() -> Result<Greetd, String> {
        let path = std::env::var("GREETD_SOCK").map_err(|_| "The login service is not running.".to_string())?;
        UnixStream::connect(path).map(Greetd).map_err(|e| e.to_string())
    }

    fn call(&mut self, msg: Value) -> Result<Value, String> {
        let data = msg.to_string().into_bytes();
        self.0.write_all(&(data.len() as u32).to_ne_bytes()).map_err(|e| e.to_string())?;
        self.0.write_all(&data).map_err(|e| e.to_string())?;
        let mut len = [0u8; 4];
        self.0.read_exact(&mut len).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; u32::from_ne_bytes(len) as usize];
        self.0.read_exact(&mut buf).map_err(|e| e.to_string())?;
        serde_json::from_slice(&buf).map_err(|e| e.to_string())
    }
}

fn greetd_login(user: &str, password: &str) -> Result<(), String> {
    let mut g = Greetd::connect()?;
    let mut r = g.call(json!({"type": "create_session", "username": user}))?;
    let mut answered = false;
    let mut message = String::new();
    while r["type"] == "auth_message" {
        let kind = r["auth_message_type"].as_str().unwrap_or("");
        if kind == "secret" || kind == "visible" {
            if answered {
                // asked again: the password was wrong; don't spend more tries
                let _ = g.call(json!({"type": "cancel_session"}));
                return Err(if message.is_empty() { "Wrong password. Try again.".into() } else { message });
            }
            answered = true;
            r = g.call(json!({"type": "post_auth_message_response", "response": password}))?;
        } else {
            if kind == "error" {
                if let Some(m) = r["auth_message"].as_str() {
                    message = m.trim().to_string();
                }
            }
            r = g.call(json!({"type": "post_auth_message_response", "response": null}))?;
        }
    }
    if r["type"] != "success" {
        let _ = g.call(json!({"type": "cancel_session"}));
        if !message.is_empty() {
            return Err(message);
        }
        if r["error_type"] == "auth_error" {
            return Err("Wrong password. Try again.".into());
        }
        return Err(r["description"].as_str().unwrap_or("Could not log in.").to_string());
    }
    let r = g.call(json!({"type": "start_session", "cmd": [SESSION], "env": []}))?;
    if r["type"] != "success" {
        let _ = g.call(json!({"type": "cancel_session"}));
        return Err(r["description"].as_str().unwrap_or("Could not start Chlorum.").to_string());
    }
    Ok(())
}
