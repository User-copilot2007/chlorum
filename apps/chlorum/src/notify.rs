//! Notifications from apps (org.freedesktop.Notifications). The shell shows
//! each one briefly in the corner and keeps the recent ones in the system
//! menu.

use cosmic::iced::futures::channel::mpsc::Sender;
use cosmic::iced::futures::{SinkExt, Stream};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use zbus::zvariant::OwnedValue;

#[derive(Clone, Debug)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub icon: String,
    pub summary: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub enum Event {
    Show(Note),
    Close(u32),
}

struct Server {
    out: Mutex<Sender<Event>>,
    next: AtomicU32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 { replaces_id } else { self.next.fetch_add(1, Ordering::Relaxed) };
        let note = Note { id, app: app_name, icon: app_icon, summary, body: strip_markup(&body) };
        if let Ok(mut out) = self.out.lock() {
            let _ = out.try_send(Event::Show(note));
        }
        id
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) {
        if let Ok(mut out) = self.out.lock() {
            let _ = out.try_send(Event::Close(id));
        }
        // 3 = closed by a call to CloseNotification
        let _ = Self::notification_closed(&emitter, id, 3).await;
    }

    #[zbus(signal)]
    async fn notification_closed(emitter: &zbus::object_server::SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("Chlorum".into(), "Chlorum".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

/// Bodies may carry simple markup; show plain text.
fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

pub fn events() -> impl Stream<Item = Event> {
    cosmic::iced::stream::channel(32, async |out: Sender<Event>| {
        let server = Server { out: Mutex::new(out.clone()), next: AtomicU32::new(1) };
        let conn = async {
            zbus::connection::Builder::session()?
                .name("org.freedesktop.Notifications")?
                .serve_at("/org/freedesktop/Notifications", server)?
                .build()
                .await
        }
        .await;
        match conn {
            Ok(_conn) => {
                // keep the connection (and the name) for the whole session
                std::future::pending::<()>().await;
            }
            Err(e) => {
                eprintln!("chlorum: notifications: {e}");
                let mut out = out;
                let _ = out.send(Event::Close(0)).await;
                std::future::pending::<()>().await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn markup() {
        assert_eq!(super::strip_markup("<b>Done</b> &amp; saved"), "Done & saved");
    }
}

/// One event each time the computer is about to sleep (logind), so the
/// screen is locked when it wakes up.
pub fn sleeps() -> impl Stream<Item = ()> {
    use cosmic::iced::futures::StreamExt;
    cosmic::iced::stream::channel(4, async |mut out: Sender<()>| {
        let run = async {
            let conn = zbus::Connection::system().await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.login1.Manager")?
                .member("PrepareForSleep")?
                .build();
            let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, Some(16)).await?;
            // logind waits (up to its InhibitDelayMaxSec) while we hold this,
            // so the lock screen is up before the computer sleeps
            let inhibit = |conn: zbus::Connection| async move {
                let m = conn
                    .call_method(
                        Some("org.freedesktop.login1"),
                        "/org/freedesktop/login1",
                        Some("org.freedesktop.login1.Manager"),
                        "Inhibit",
                        &("sleep", "Chlorum", "Locking the screen first", "delay"),
                    )
                    .await
                    .ok()?;
                m.body().deserialize::<zbus::zvariant::OwnedFd>().ok()
            };
            let mut held = inhibit(conn.clone()).await;
            while let Some(Ok(msg)) = stream.next().await {
                if msg.body().deserialize::<bool>().unwrap_or(false) {
                    let _ = out.send(()).await;
                    // the lock screen needs a moment to show on every screen
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    held = None;
                } else if held.is_none() {
                    // woke up: ready for the next time
                    held = inhibit(conn.clone()).await;
                }
            }
            drop(held);
            Ok::<(), zbus::Error>(())
        };
        if let Err(e) = run.await {
            eprintln!("chlorum: sleep watch: {e}");
        }
        std::future::pending::<()>().await;
    })
}
