//! The Chlorum login screen (runs in cage under greetd).
//!
//! Profiles in a row, the password under them, and under that "Show" and
//! "Power". Arrows move between them in the direction pressed; nothing needs
//! a mouse.

use crate::atom::Atom;
use crate::ui::*;
use chrono::{Local, Timelike};
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{self, Alignment, Background, Border, Color, Length, Subscription, event, keyboard};
use cosmic::widget::text_input;
use cosmic::{Application, Element, widget};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
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
struct Profile {
    login: String,
    name: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Profiles,
    Field,
    Buttons(usize), // 0 Show, 1 Power
    Power(usize),   // 0 Restart, 1 Shut down, 2 Cancel
}

#[derive(Clone, Debug)]
pub enum Message {
    Tick,
    Frame,
    Key(keyboard::Key, event::Status),
    Input(String),
    Submit,
    Done(Result<(), String>),
}

pub struct Greeter {
    core: Core,
    profiles: Vec<Profile>,
    sel: usize,
    zone: Zone,
    password: String,
    show: bool,
    busy: bool,
    error: Option<String>,
    armed: bool,
    input_id: widget::Id,
    started: Instant,
    appear: f32,
    last: Instant,
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
        let input_id = widget::Id::unique();
        let g = Greeter {
            core,
            profiles: profiles(),
            sel: 0,
            zone: Zone::Field,
            password: String::new(),
            show: false,
            busy: false,
            error: None,
            armed: false,
            input_id: input_id.clone(),
            started: Instant::now(),
            appear: 0.0,
            last: Instant::now(),
        };
        let mut tasks = vec![text_input::focus(input_id)];
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
        if self.busy || self.appear < 1.0 {
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
            Message::Input(s) => {
                if !self.busy {
                    self.password = s;
                    self.error = None;
                }
            }
            Message::Submit => return self.login(),
            Message::Done(Ok(())) => std::process::exit(0),
            Message::Done(Err(e)) => {
                self.busy = false;
                self.error = Some(e);
                self.password.clear();
                self.zone = Zone::Field;
                return text_input::focus(self.input_id.clone());
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
            .push(widget::icon::from_name("com.chlorum.Logo").size(460))
            .push(space(1.0, 30.0))
            .push(
                widget::Row::new()
                    .push(text(format!("{:02}:{:02}", now.hour(), now.minute()), MONO_BOLD, 84.0, alpha(TEXT, f)))
                    .push(text(format!(":{:02}", now.second()), MONO_BOLD, 84.0, alpha(YELLOW, f))),
            )
            .push(text(now.format("%A  ·  %-d %B").to_string().to_uppercase(), MONO, 22.0, alpha(TEXT_2, f)))
            .push(vfill())
            .width(Length::Fixed(620.0))
            .align_x(Alignment::Center);

        let mut who = widget::Row::new().spacing(34);
        for (i, p) in self.profiles.iter().enumerate() {
            let on = i == self.sel;
            let initial = p.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            let av = boxed(
                widget::container(text(initial, MONO_BOLD, 54.0, if on { BLACK } else { TEXT }))
                    .width(Length::Fixed(124.0))
                    .height(Length::Fixed(124.0))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
                if on { YELLOW } else { PANEL_2 },
                Color::TRANSPARENT,
                0.0,
            )
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
                Color::from_rgba(0.07, 0.07, 0.07, 0.9),
            )
            .width(Length::Fixed(230.0))
            .height(Length::Fixed(270.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center);
            who = who.push(bracket(card, if on && self.zone == Zone::Profiles { 1.0 } else { 0.0 }, 10.0));
        }

        let field_on = self.zone == Zone::Field;
        let input = {
            let mut t = text_input::text_input("Password", self.password.as_str())
                .id(self.input_id.clone())
                .size(34)
                .padding([24, 28])
                .width(Length::Fixed(640.0))
                .style(field_style(field_on));
            if !self.show {
                t = t.password();
            }
            if !self.busy {
                t = t.on_input(Message::Input).on_submit(|_| Message::Submit);
            }
            t
        };
        let field: Element<'_, Message> = if self.busy {
            widget::Row::new()
                .push(Atom::view(92.0, self.started))
                .push(text("Unlocking your profile…", SANS_BOLD, 26.0, TEXT))
                .spacing(26)
                .align_y(Alignment::Center)
                .height(Length::Fixed(96.0))
                .into()
        } else {
            input.into()
        };

        let button = |i: usize, label: String, sub: &str| -> Element<'_, Message> {
            let on = self.zone == Zone::Buttons(i);
            let b = fill(
                widget::Column::new()
                    .push(text(label, SANS_BOLD, 22.0, if on { BLACK } else { TEXT }))
                    .push(text(sub.to_string(), MONO, 14.0, if on { BLACK } else { TEXT_2 }))
                    .spacing(4)
                    .align_x(Alignment::Center),
                if on { YELLOW } else { PANEL_2 },
            )
            .width(Length::Fixed(200.0))
            .padding([16, 10])
            .align_x(Alignment::Center);
            bracket(b, if on { 1.0 } else { 0.0 }, 8.0)
        };
        let buttons = widget::Row::new()
            .push(button(0, if self.show { "Hide".into() } else { "Show".into() }, "PASSWORD"))
            .push(button(1, "Power".into(), "F10"))
            .spacing(22);

        let mut panel = widget::Column::new()
            .push(vfill())
            .push(who)
            .push(space(1.0, 56.0))
            .push(bracket(field, if field_on && !self.busy { 1.0 } else { 0.0 }, 10.0))
            .push(space(1.0, 26.0))
            .push(buttons);
        if let Zone::Power(sel) = self.zone {
            let mut r = widget::Row::new().spacing(18);
            for (i, l) in ["Restart", "Shut down", "Cancel"].iter().enumerate() {
                let on = i == sel;
                let armed = on && self.armed && i < 2;
                let label = if armed { format!("Enter again to {}", l.to_lowercase()) } else { l.to_string() };
                let b = fill(text(label, SANS_BOLD, 21.0, if on { BLACK } else { TEXT }), if armed { RED } else if on { YELLOW } else { PANEL_2 })
                    .padding([16, 26]);
                r = r.push(bracket(b, if on { 1.0 } else { 0.0 }, 8.0));
            }
            panel = panel.push(space(1.0, 24.0)).push(r);
        }
        if let Some(e) = &self.error {
            panel = panel.push(space(1.0, 24.0)).push(text(e.clone(), SANS_BOLD, 22.0, RED));
        }
        if self.profiles.is_empty() {
            panel = panel.push(space(1.0, 24.0)).push(text("No profiles yet.", SANS, 22.0, TEXT_2));
        }
        panel = panel.push(vfill());

        let h: &[(&str, &str, bool)] = match self.zone {
            Zone::Profiles => &[("← →", "Profile", false), ("↓", "Password", false), ("ENTER", "Choose", true), ("F10", "Power", false)],
            Zone::Field => &[("↑", "Profile", false), ("↓", "Show password", false), ("ENTER", "Unlock", true), ("F10", "Power", false)],
            Zone::Buttons(_) => &[("← →", "Move", false), ("↑", "Password", false), ("ENTER", "Press", true)],
            Zone::Power(_) => &[("← →", "Choose", false), ("ENTER", "Twice to confirm", true), ("ESC", "Back", false)],
        };
        let body = widget::Column::new()
            .push(
                widget::Row::new()
                    .push(left)
                    .push(space(90.0, 1.0))
                    .push(panel.width(Length::Fill))
                    .height(Length::Fill),
            )
            .push(hints(h, f))
            .padding([48, 80]);
        fill(body, Color::from_rgb(0.0, 0.0, 0.0)).width(Length::Fill).height(Length::Fill).into()
    }
}

fn field_style(on: bool) -> cosmic::theme::TextInput {
    let look = move |_: &cosmic::Theme| text_input::Appearance {
        background: Background::Color(Color::from_rgb(0.05, 0.05, 0.05)),
        border_radius: 0.0.into(),
        border_offset: None,
        border_width: 0.0,
        border_color: Color::TRANSPARENT,
        label_color: TEXT_2,
        placeholder_color: MUTED,
        selected_text_color: BLACK,
        icon_color: Some(TEXT),
        text_color: Some(TEXT),
        selected_fill: YELLOW,
    };
    let _ = on;
    cosmic::theme::TextInput::Custom {
        active: Box::new(look),
        error: Box::new(look),
        hovered: Box::new(look),
        focused: Box::new(look),
        disabled: Box::new(look),
    }
}

impl Greeter {
    fn key(&mut self, key: keyboard::Key, status: event::Status) -> Task<Message> {
        use keyboard::key::Named;
        if self.busy {
            return Task::none();
        }
        let Some(named) = (match key.as_ref() {
            keyboard::Key::Named(n) => Some(n),
            _ => None,
        }) else {
            // typing anywhere goes to the password
            if self.zone != Zone::Field {
                self.zone = Zone::Field;
                return text_input::focus(self.input_id.clone());
            }
            return Task::none();
        };
        let n = self.profiles.len();
        match (self.zone, named) {
            (_, Named::F10) => {
                self.zone = Zone::Power(0);
                self.armed = false;
            }
            (Zone::Profiles, Named::ArrowLeft) => self.sel = self.sel.saturating_sub(1),
            (Zone::Profiles, Named::ArrowRight) if n > 0 => self.sel = (self.sel + 1).min(n - 1),
            (Zone::Profiles, Named::ArrowDown | Named::Enter | Named::Tab) => {
                self.zone = Zone::Field;
                return text_input::focus(self.input_id.clone());
            }
            (Zone::Field, Named::ArrowUp) => self.zone = Zone::Profiles,
            (Zone::Field, Named::ArrowDown | Named::Tab) => self.zone = Zone::Buttons(0),
            (Zone::Field, Named::Escape) => self.password.clear(),
            (Zone::Field, Named::Enter) if status == event::Status::Ignored => return self.login(),
            (Zone::Buttons(i), Named::ArrowLeft) => self.zone = Zone::Buttons(i.saturating_sub(1)),
            (Zone::Buttons(i), Named::ArrowRight) => self.zone = Zone::Buttons((i + 1).min(1)),
            (Zone::Buttons(_), Named::ArrowUp | Named::Escape) => {
                self.zone = Zone::Field;
                return text_input::focus(self.input_id.clone());
            }
            (Zone::Buttons(0), Named::Enter) => {
                self.show = !self.show;
                self.zone = Zone::Field;
                return text_input::focus(self.input_id.clone());
            }
            (Zone::Buttons(1), Named::Enter) => {
                self.zone = Zone::Power(0);
                self.armed = false;
            }
            (Zone::Power(i), Named::ArrowLeft) => {
                self.zone = Zone::Power(i.saturating_sub(1));
                self.armed = false;
            }
            (Zone::Power(i), Named::ArrowRight) => {
                self.zone = Zone::Power((i + 1).min(2));
                self.armed = false;
            }
            (Zone::Power(_), Named::Escape | Named::ArrowUp) => self.zone = Zone::Buttons(1),
            (Zone::Power(i), Named::Enter) => {
                if i == 2 {
                    self.zone = Zone::Buttons(1);
                } else if self.armed {
                    let _ = std::process::Command::new("systemctl").arg(if i == 0 { "reboot" } else { "poweroff" }).spawn();
                } else {
                    self.armed = true;
                }
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
        self.busy = true;
        self.started = Instant::now();
        self.error = None;
        let (user, pw) = (p.login.clone(), self.password.clone());
        Task::perform(
            async move { tokio::task::spawn_blocking(move || greetd_login(&user, &pw)).await.unwrap_or_else(|e| Err(e.to_string())) },
            |r| cosmic::Action::App(Message::Done(r)),
        )
    }
}

fn profiles() -> Vec<Profile> {
    let out = std::process::Command::new("homectl").args(["list", "--no-legend"]).output();
    let mut v = Vec::new();
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let Some(login) = line.split_whitespace().next() else { continue };
            let name = std::process::Command::new("getent")
                .args(["passwd", login])
                .output()
                .ok()
                .and_then(|o| {
                    let l = String::from_utf8_lossy(&o.stdout).to_string();
                    l.split(':').nth(4).map(|g| g.split(',').next().unwrap_or("").trim().to_string())
                })
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| login.to_string());
            v.push(Profile { login: login.to_string(), name });
        }
    }
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

    fn call(&mut self, msg: serde_json::Value) -> Result<serde_json::Value, String> {
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
    use serde_json::json;
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
