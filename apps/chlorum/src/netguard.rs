//! The network guard: which app may reach which place on the internet.
//!
//! An app given "Internet and network" (sandbox.rs) can reach the network;
//! this decides *where*. The OpenSnitch daemon (opensnitchd, root) holds
//! every new connection; when no rule says yes or no, it asks its UI, and
//! Chlorum's shell is that UI: a gRPC server on a socket in
//! /run/chlorum-netui that the daemon connects to.
//!
//! The first time an app reaches a new place, the shell asks:
//!   Firefox wants to reach google.com (port 443)
//!   Allow google.com · Allow everything for Firefox · Block google.com · Not now
//! The answers are kept by the daemon as rules (in /var/lib/opensnitch/rules),
//! named chlorum-…, so they hold even while nobody is logged in; Center →
//! Internet lists them and forgets one with Delete.
//!
//! Apps are told apart by the CHLORUM_APP variable `chlorum run` gives
//! them (not by their file, which changes with every update). Programs not
//! started as an app (the system itself, Chlorum, services) are let through
//! without asking. No answer within 30 seconds blocks that one connection.
//!
//! Known gap: a program inside an app can start another one with a
//! different CHLORUM_APP and so pass for another app (the sandbox's own
//! "network off" still holds; this layer only narrows down places).

#[allow(clippy::all, warnings)]
pub mod proto {
    include!("netguard_proto.rs");
    #[cfg(test)]
    include!("netguard_proto_client.rs");
}

use proto::ui_server::{Ui, UiServer};
use proto::{Action, ClientConfig, Connection, MsgResponse, Notification, NotificationReply, Operator, PingReply, PingRequest, Rule};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status};

/// Where the shell listens and the daemon connects. The daemon is told
/// "unix:osui.sock" and runs in /run/chlorum-netui: with a full path, its
/// gRPC client sends the path as the HTTP/2 authority, which is refused.
pub const SOCK: &str = "/run/chlorum-netui/osui.sock";
/// How long a question waits for an answer before the connection is blocked.
const WAIT: Duration = Duration::from_secs(30);

/// An app wants to reach a place nobody has decided about yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub id: u64,
    /// the app's id (what `chlorum run` was given)
    pub app: String,
    /// the name looked up for the address, when there was one
    pub host: String,
    pub ip: String,
    pub port: u32,
    pub protocol: String,
}

impl Ask {
    /// "google.com", or the address when no name was looked up.
    pub fn place(&self) -> String {
        if self.host.is_empty() { self.ip.clone() } else { self.host.clone() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// this place, from now on
    AllowPlace,
    /// every place, for this app
    AllowApp,
    /// never this place
    BlockPlace,
    /// not this time (asked again next time)
    BlockOnce,
}

/// A rule Chlorum made and the daemon keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub name: String,
    pub app: String,
    /// None: every place
    pub place: Option<String>,
    pub allow: bool,
}

/// A connection an app made lately, and whether it went through.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub app: String,
    pub place: String,
    pub port: u32,
    pub allowed: bool,
    /// "14:03:22"
    pub time: String,
}

#[derive(Clone, Debug)]
pub enum Event {
    Ask(Ask),
    /// the rules Chlorum made, all of them
    Kept(Vec<Kept>),
    /// the latest connections of apps, newest first
    Seen(Vec<Seen>),
    /// the daemon is connected (or went away)
    Connected(bool),
}

type Out = cosmic::iced::futures::channel::mpsc::Sender<Event>;

static OUT: Mutex<Option<Out>> = Mutex::new(None);
static PENDING: Mutex<Option<HashMap<u64, oneshot::Sender<Answer>>>> = Mutex::new(None);
static TO_DAEMON: Mutex<Option<mpsc::Sender<Result<Notification, Status>>>> = Mutex::new(None);
static RULES: Mutex<Vec<Rule>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn tell(e: Event) {
    if let Ok(mut o) = OUT.lock() {
        if let Some(o) = o.as_mut() {
            let _ = o.try_send(e);
        }
    }
}

fn tell_kept() {
    let kept = RULES.lock().map(|r| r.iter().filter_map(kept_of).collect()).unwrap_or_default();
    tell(Event::Kept(kept));
}

/// The shell's answer to a question.
pub fn answer(id: u64, a: Answer) {
    if let Ok(mut p) = PENDING.lock() {
        if let Some(tx) = p.get_or_insert_with(HashMap::new).remove(&id) {
            let _ = tx.send(a);
        }
    }
}

/// Forgets a rule Chlorum made (Center → Internet, Delete): the app is
/// asked again the next time. False when the daemon is not connected.
pub fn forget(name: &str) -> bool {
    let rule = RULES.lock().ok().and_then(|r| r.iter().find(|x| x.name == name).cloned());
    let Some(rule) = rule else { return false };
    let tx = TO_DAEMON.lock().ok().and_then(|t| t.clone());
    let Some(tx) = tx else { return false };
    let n = Notification {
        id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        client_name: String::new(),
        server_name: "chlorum".into(),
        r#type: Action::DeleteRule as i32,
        data: String::new(),
        rules: vec![rule],
        sys_firewall: None,
    };
    if tx.try_send(Ok(n)).is_err() {
        return false;
    }
    if let Ok(mut r) = RULES.lock() {
        r.retain(|x| x.name != name);
    }
    tell_kept();
    true
}

// ---------------------------------------------------------------- rules

/// Rule names are file names for the daemon: plain letters only.
fn clean(s: &str) -> String {
    let mut out: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' }).collect();
    out.truncate(120);
    out.trim_matches(['-', '.']).to_string()
}

fn simple(operand: &str, data: &str) -> Operator {
    Operator { r#type: "simple".into(), operand: operand.into(), data: data.into(), sensitive: false, list: vec![] }
}

const APP_OPERAND: &str = "process.env.CHLORUM_APP";

fn app_is(app: &str) -> Operator {
    simple(APP_OPERAND, app)
}

fn place_is(ask: &Ask) -> Operator {
    if ask.host.is_empty() { simple("dest.ip", &ask.ip) } else { simple("dest.host", &ask.host) }
}

fn both(a: Operator, b: Operator) -> Operator {
    Operator { r#type: "list".into(), operand: "list".into(), data: String::new(), sensitive: false, list: vec![a, b] }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The rule an answer becomes.
pub fn rule_for(ask: &Ask, a: Answer) -> Rule {
    let place = ask.place();
    let (name, action, duration, operator) = match a {
        Answer::AllowPlace => (format!("chlorum-allow-{}", clean(&format!("{}-{place}", ask.app))), "allow", "always", both(app_is(&ask.app), place_is(ask))),
        Answer::AllowApp => (format!("chlorum-allow-{}", clean(&ask.app)), "allow", "always", app_is(&ask.app)),
        Answer::BlockPlace => (format!("chlorum-block-{}", clean(&format!("{}-{place}", ask.app))), "deny", "always", both(app_is(&ask.app), place_is(ask))),
        Answer::BlockOnce => (format!("chlorum-once-{}", clean(&format!("{}-{place}", ask.app))), "deny", "once", both(app_is(&ask.app), place_is(ask))),
    };
    Rule {
        created: now_secs(),
        name,
        description: format!("Chlorum: {} for {}", if action == "allow" { "allowed" } else { "blocked" }, ask.app),
        enabled: true,
        precedence: false,
        nolog: false,
        action: action.into(),
        duration: duration.into(),
        operator: Some(operator),
    }
}

/// A program that is not an app (the system, Chlorum itself) goes through:
/// until the daemon restarts, for that program file.
fn system_rule(conn: &Connection) -> Rule {
    let (name, duration, operator) = if conn.process_path.is_empty() {
        ("chlorum-system-once".to_string(), "once", simple("true", ""))
    } else {
        (format!("chlorum-system-{}", clean(&conn.process_path)), "until restart", simple("process.path", &conn.process_path))
    };
    Rule {
        created: now_secs(),
        name,
        description: "Chlorum: part of the system".into(),
        enabled: true,
        precedence: false,
        nolog: true,
        action: "allow".into(),
        duration: duration.into(),
        operator: Some(operator),
    }
}

/// What a rule Chlorum made says, for Center.
pub fn kept_of(r: &Rule) -> Option<Kept> {
    if !(r.name.starts_with("chlorum-allow-") || r.name.starts_with("chlorum-block-")) || r.duration != "always" {
        return None;
    }
    let op = r.operator.as_ref()?;
    let ops: Vec<&Operator> = if op.r#type == "list" { op.list.iter().collect() } else { vec![op] };
    let app = ops.iter().find(|o| o.operand == APP_OPERAND)?.data.clone();
    let place = ops.iter().find(|o| o.operand == "dest.host" || o.operand == "dest.ip").map(|o| o.data.clone());
    Some(Kept { name: r.name.clone(), app, place, allow: r.action == "allow" })
}

// ---------------------------------------------------------------- the UI service

struct Guard {
    /// the newest event last passed on, so the same list is not sent again
    last: Mutex<i64>,
}

#[tonic::async_trait]
impl Ui for Guard {
    async fn ping(&self, req: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let req = req.into_inner();
        if let Some(stats) = req.stats {
            let newest = stats.events.iter().map(|e| e.unixnano).max().unwrap_or(0);
            let changed = self.last.lock().map(|mut l| std::mem::replace(&mut *l, newest) != newest).unwrap_or(false);
            if changed {
                let mut seen: Vec<(i64, Seen)> = stats
                    .events
                    .iter()
                    .filter_map(|e| {
                        let c = e.connection.as_ref()?;
                        let app = c.process_env.get("CHLORUM_APP")?.clone();
                        let allowed = e.rule.as_ref().is_some_and(|r| r.action == "allow");
                        let place = if c.dst_host.is_empty() { c.dst_ip.clone() } else { c.dst_host.clone() };
                        let time = e.time.rsplit(' ').next().unwrap_or(&e.time).chars().take(8).collect();
                        Some((e.unixnano, Seen { app, place, port: c.dst_port, allowed, time }))
                    })
                    .collect();
                seen.sort_by(|a, b| b.0.cmp(&a.0));
                tell(Event::Seen(seen.into_iter().map(|(_, s)| s).take(60).collect()));
            }
        }
        Ok(Response::new(PingReply { id: req.id }))
    }

    async fn ask_rule(&self, req: Request<Connection>) -> Result<Response<Rule>, Status> {
        let conn = req.into_inner();
        let Some(app) = conn.process_env.get("CHLORUM_APP").cloned().filter(|a| !a.is_empty()) else {
            return Ok(Response::new(system_rule(&conn)));
        };
        let ask = Ask {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            app,
            host: conn.dst_host.clone(),
            ip: conn.dst_ip.clone(),
            port: conn.dst_port,
            protocol: conn.protocol.clone(),
        };
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = PENDING.lock() {
            p.get_or_insert_with(HashMap::new).insert(ask.id, tx);
        }
        tell(Event::Ask(ask.clone()));
        let a = tokio::time::timeout(WAIT, rx).await.ok().and_then(|r| r.ok()).unwrap_or(Answer::BlockOnce);
        if let Ok(mut p) = PENDING.lock() {
            p.get_or_insert_with(HashMap::new).remove(&ask.id);
        }
        let rule = rule_for(&ask, a);
        if rule.duration == "always" {
            if let Ok(mut r) = RULES.lock() {
                r.retain(|x| x.name != rule.name);
                r.push(rule.clone());
            }
            tell_kept();
        }
        Ok(Response::new(rule))
    }

    async fn subscribe(&self, req: Request<ClientConfig>) -> Result<Response<ClientConfig>, Status> {
        let cfg = req.into_inner();
        if let Ok(mut r) = RULES.lock() {
            *r = cfg.rules.iter().filter(|x| x.name.starts_with("chlorum-")).cloned().collect();
        }
        tell_kept();
        tell(Event::Connected(true));
        Ok(Response::new(ClientConfig { id: cfg.id, name: "chlorum".into(), version: env!("CARGO_PKG_VERSION").into(), ..Default::default() }))
    }

    type NotificationsStream = tokio_stream::wrappers::ReceiverStream<Result<Notification, Status>>;

    async fn notifications(&self, req: Request<tonic::Streaming<NotificationReply>>) -> Result<Response<Self::NotificationsStream>, Status> {
        let mut replies = req.into_inner();
        let (tx, rx) = mpsc::channel(16);
        if let Ok(mut t) = TO_DAEMON.lock() {
            *t = Some(tx);
        }
        // the daemon's replies; when they end, it has gone away
        tokio::spawn(async move {
            while let Ok(Some(_)) = replies.message().await {}
            if let Ok(mut t) = TO_DAEMON.lock() {
                *t = None;
            }
            tell(Event::Connected(false));
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    async fn post_alert(&self, req: Request<proto::Alert>) -> Result<Response<MsgResponse>, Status> {
        Ok(Response::new(MsgResponse { id: req.into_inner().id }))
    }
}

/// The guard, for the whole session: listens for the daemon and passes
/// its questions and news to the shell.
pub fn events() -> impl cosmic::iced::futures::Stream<Item = Event> {
    cosmic::iced::stream::channel(32, async |out: Out| {
        if let Ok(mut o) = OUT.lock() {
            *o = Some(out);
        }
        serve(std::path::Path::new(SOCK)).await;
        std::future::pending::<()>().await
    })
}

/// Listens on `path` for the daemon until it fails.
async fn serve(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    match tokio::net::UnixListener::bind(path) {
        Ok(listener) => {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
            let guard = Guard { last: Mutex::new(0) };
            if let Err(e) = tonic::transport::Server::builder().add_service(UiServer::new(guard)).serve_with_incoming(incoming).await {
                eprintln!("chlorum: network guard stopped: {e}");
            }
        }
        // an older system without the guard, or a test bench
        Err(e) => eprintln!("chlorum: network guard not started ({}): {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(host: &str) -> Ask {
        Ask { id: 1, app: "librewolf".into(), host: host.into(), ip: "93.184.216.34".into(), port: 443, protocol: "tcp".into() }
    }

    #[test]
    fn answers_become_rules() {
        let r = rule_for(&ask("example.com"), Answer::AllowPlace);
        assert_eq!(r.name, "chlorum-allow-librewolf-example.com");
        assert_eq!((r.action.as_str(), r.duration.as_str()), ("allow", "always"));
        let op = r.operator.as_ref().unwrap();
        assert_eq!(op.r#type, "list");
        assert_eq!(op.list[0].operand, "process.env.CHLORUM_APP");
        assert_eq!(op.list[1].operand, "dest.host");
        assert_eq!(kept_of(&r), Some(Kept { name: r.name.clone(), app: "librewolf".into(), place: Some("example.com".into()), allow: true }));

        // no name looked up: the address
        let r = rule_for(&ask(""), Answer::BlockPlace);
        assert_eq!(r.action, "deny");
        assert_eq!(r.operator.as_ref().unwrap().list[1].operand, "dest.ip");
        assert_eq!(kept_of(&r).unwrap().place.as_deref(), Some("93.184.216.34"));

        let r = rule_for(&ask("example.com"), Answer::AllowApp);
        assert_eq!(r.name, "chlorum-allow-librewolf");
        assert_eq!(kept_of(&r).unwrap().place, None);

        // "not now" is not kept and not listed
        let r = rule_for(&ask("example.com"), Answer::BlockOnce);
        assert_eq!(r.duration, "once");
        assert!(kept_of(&r).is_none());
    }

    /// The daemon's side, played by a client: it subscribes, asks, gets a
    /// rule back, and is told to delete one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn talks_to_the_daemon() {
        use cosmic::iced::futures::StreamExt;
        use proto::ui_client::UiClient;
        let dir = std::env::temp_dir().join(format!("chlorum-netguard-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("osui.sock");
        let (out, mut events) = cosmic::iced::futures::channel::mpsc::channel(32);
        *OUT.lock().unwrap() = Some(out);
        let s2 = sock.clone();
        tokio::spawn(async move { serve(&s2).await });
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let s3 = sock.clone();
        let channel = tonic::transport::Endpoint::try_from("http://[::]:50051")
            .unwrap()
            .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
                let s = s3.clone();
                async move { Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(tokio::net::UnixStream::connect(s).await?)) }
            }))
            .await
            .unwrap();
        let mut c = UiClient::new(channel);

        // subscribing: Chlorum's rules are kept, others ignored
        let kept = rule_for(&ask("example.org"), Answer::BlockPlace);
        let other = Rule { name: "someone-elses".into(), ..kept.clone() };
        c.subscribe(ClientConfig { id: 7, rules: vec![kept.clone(), other], ..Default::default() }).await.unwrap();
        match events.next().await {
            Some(Event::Kept(k)) => assert_eq!(k.len(), 1),
            e => panic!("expected the rules, got {e:?}"),
        }
        assert!(matches!(events.next().await, Some(Event::Connected(true))));

        // the notification channel, for deleting rules
        let (_replies_tx, replies_rx) = mpsc::channel::<NotificationReply>(4);
        let mut notes = c.notifications(tokio_stream::wrappers::ReceiverStream::new(replies_rx)).await.unwrap().into_inner();

        // an app asks: the shell answers, the daemon gets the rule
        let mut c2 = c.clone();
        let asking = tokio::spawn(async move {
            let mut env = HashMap::new();
            env.insert("CHLORUM_APP".to_string(), "librewolf".to_string());
            c2.ask_rule(Connection { protocol: "tcp".into(), dst_ip: "93.184.216.34".into(), dst_host: "example.com".into(), dst_port: 443, process_env: env, ..Default::default() })
                .await
                .unwrap()
                .into_inner()
        });
        let id = match events.next().await {
            Some(Event::Ask(a)) => {
                assert_eq!((a.app.as_str(), a.place(), a.port), ("librewolf", "example.com".to_string(), 443));
                a.id
            }
            e => panic!("expected a question, got {e:?}"),
        };
        answer(id, Answer::AllowPlace);
        let rule = asking.await.unwrap();
        assert_eq!(rule.name, "chlorum-allow-librewolf-example.com");
        assert_eq!(rule.duration, "always");
        match events.next().await {
            Some(Event::Kept(k)) => assert_eq!(k.len(), 2),
            e => panic!("expected the rules, got {e:?}"),
        }

        // the system goes through without a question
        let r = c.ask_rule(Connection { process_path: "/run/current-system/sw/bin/nix".into(), ..Default::default() }).await.unwrap().into_inner();
        assert_eq!((r.action.as_str(), r.duration.as_str()), ("allow", "until restart"));

        // forgetting a rule tells the daemon to delete it
        assert!(forget(&kept.name));
        let n = tokio::time::timeout(Duration::from_secs(5), notes.message()).await.unwrap().unwrap().unwrap();
        assert_eq!(n.r#type, Action::DeleteRule as i32);
        assert_eq!(n.rules[0].name, kept.name);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_plain() {
        assert_eq!(clean("com.App/../x y"), "com.app-..-x-y");
        assert!(!clean("/nix/store/abc-firefox/bin/firefox").contains('/'));
        // the system is let through, for its own file only
        let c = Connection { process_path: "/nix/store/x-nix/bin/nix".into(), ..Default::default() };
        let r = system_rule(&c);
        assert_eq!(r.duration, "until restart");
        assert_eq!(r.operator.unwrap().operand, "process.path");
    }
}
