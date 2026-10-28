//! Bluetooth through BlueZ (the system's Bluetooth service, on D-Bus):
//! switch it on or off, look for devices, pair, connect, disconnect,
//! forget. Center → Bluetooth.
//!
//! Pairing some devices needs an answer (a keyboard shows a number to type
//! on it, headphones or a phone show a number to compare): the shell
//! registers an agent that BlueZ asks. It answers only about the one device
//! a pairing started from Center is for, never to a device that asks on its
//! own, and a number to compare is put to the user, who says yes or no.

use cosmic::iced::futures::channel::mpsc::Sender;
use cosmic::iced::futures::channel::oneshot;
use cosmic::iced::futures::{SinkExt, Stream};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const PROPS: &str = "org.freedesktop.DBus.Properties";
const AGENT_PATH: &str = "/org/chlorum/bluetooth";

/// The device a pairing started from Center is for: the only one the agent
/// answers about.
static PAIRING: Mutex<Option<String>> = Mutex::new(None);
/// The user's yes or no to "does the device show this number?"
static ANSWER: Mutex<Option<oneshot::Sender<bool>>> = Mutex::new(None);
/// One connection to the system bus for the whole session: BlueZ stops
/// looking for devices when the connection that asked goes away.
static BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();

fn pairing() -> Option<String> {
    PAIRING.lock().map(|p| p.clone()).unwrap_or(None)
}

/// While it lives, the agent may answer about this device.
struct Pairing;
impl Pairing {
    fn start(path: &str) -> Pairing {
        if let Ok(mut p) = PAIRING.lock() {
            *p = Some(path.to_string());
        }
        Pairing
    }
}
impl Drop for Pairing {
    fn drop(&mut self) {
        if let Ok(mut p) = PAIRING.lock() {
            *p = None;
        }
        // a question still on screen has no one to answer to any more
        if let Ok(mut a) = ANSWER.lock() {
            a.take();
        }
    }
}

/// The user's answer to the number on screen (Enter yes, Esc no).
pub fn answer(yes: bool) {
    if let Some(tx) = ANSWER.lock().ok().and_then(|mut a| a.take()) {
        let _ = tx.send(yes);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub address: String,
    /// "audio-headset", "input-keyboard", "input-mouse", "phone"…
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    /// battery, when the device tells (percent)
    pub battery: Option<u8>,
}

impl Device {
    pub fn kind(&self) -> &'static str {
        match self.icon.as_str() {
            "audio-headset" | "audio-headphones" => "Headphones",
            "audio-card" | "audio-speakers" => "Speaker",
            "input-keyboard" => "Keyboard",
            "input-mouse" => "Mouse",
            "input-gaming" => "Game controller",
            "phone" => "Phone",
            "computer" => "Computer",
            _ => "Device",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// there is a Bluetooth adapter
    pub present: bool,
    pub powered: bool,
    pub discovering: bool,
    /// paired ones first, then the ones nearby, by name
    pub devices: Vec<Device>,
    adapter: String,
}

type Props = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

fn s(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}
fn b(p: &Props, k: &str) -> bool {
    p.get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

async fn conn() -> Result<zbus::Connection, String> {
    BUS.get_or_try_init(zbus::Connection::system).await.cloned().map_err(|e| e.to_string())
}

async fn objects(c: &zbus::Connection) -> zbus::Result<Objects> {
    let m = c.call_method(Some(BLUEZ), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &()).await?;
    m.body().deserialize::<Objects>()
}

/// Bluetooth as it is now.
pub async fn state() -> State {
    let Ok(c) = conn().await else { return State::default() };
    let Ok(objs) = objects(&c).await else { return State::default() };
    let mut st = State::default();
    let mut paths: Vec<&OwnedObjectPath> = objs.keys().collect();
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for path in paths {
        let ifaces = &objs[path];
        if let Some(a) = ifaces.get(ADAPTER) {
            if !st.present {
                st.present = true;
                st.powered = b(a, "Powered");
                st.discovering = b(a, "Discovering");
                st.adapter = path.to_string();
            }
        }
        if let Some(d) = ifaces.get(DEVICE) {
            let name = s(d, "Alias");
            let address = s(d, "Address");
            let paired = b(d, "Paired");
            // nearby devices without a real name are noise
            if !paired && (name.is_empty() || name.replace('-', ":") == address) {
                continue;
            }
            let battery = ifaces.get("org.bluez.Battery1").and_then(|p| p.get("Percentage")).and_then(|v| u8::try_from(v).ok());
            st.devices.push(Device { path: path.to_string(), name, address, icon: s(d, "Icon"), paired, connected: b(d, "Connected"), battery });
        }
    }
    st.devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    st
}

/// Plain words for what went wrong.
fn why(e: zbus::Error) -> String {
    let m = e.to_string();
    if m.contains("AuthenticationFailed") || m.contains("AuthenticationRejected") || m.contains("AuthenticationCanceled") {
        "Pairing did not work. Put the device in pairing mode and try again.".into()
    } else if m.contains("AuthenticationTimeout") || m.contains("Timeout") || m.contains("timed out") {
        "The device did not answer. Put it in pairing mode, close to the computer.".into()
    } else if m.contains("Failed") && m.contains("br-connection") {
        "The device did not connect. Is it on and close by?".into()
    } else if m.contains("NotReady") || m.contains("Powered") {
        "Bluetooth is off.".into()
    } else if m.contains("AlreadyExists") {
        "It is already paired.".into()
    } else if m.contains("InProgress") {
        "Already busy with it. Wait a moment.".into()
    } else {
        m.rsplit(": ").next().unwrap_or(&m).to_string()
    }
}

async fn set_prop(c: &zbus::Connection, path: &str, iface: &str, name: &str, v: Value<'_>) -> Result<(), String> {
    c.call_method(Some(BLUEZ), path, Some(PROPS), "Set", &(iface, name, v)).await.map(|_| ()).map_err(why)
}

pub async fn set_powered(on: bool) -> Result<(), String> {
    let c = conn().await?;
    let st = state().await;
    if !st.present {
        return Err("There is no Bluetooth on this computer.".into());
    }
    set_prop(&c, &st.adapter, ADAPTER, "Powered", Value::from(on)).await
}

/// Looks for devices nearby for a while (BlueZ stops by itself when no
/// one is looking any more).
pub async fn scan(on: bool) -> Result<(), String> {
    let c = conn().await?;
    let st = state().await;
    if !st.powered {
        return Err("Bluetooth is off.".into());
    }
    let method = if on { "StartDiscovery" } else { "StopDiscovery" };
    match c.call_method(Some(BLUEZ), st.adapter.as_str(), Some(ADAPTER), method, &()).await {
        Ok(_) => Ok(()),
        // already looking, or already stopped
        Err(e) if e.to_string().contains("InProgress") || e.to_string().contains("NotReady") && !on => Ok(()),
        Err(e) => Err(why(e)),
    }
}

/// Pairs (if needed), trusts and connects.
pub async fn connect(path: String) -> Result<String, String> {
    let c = conn().await?;
    let st = state().await;
    let d = st.devices.iter().find(|d| d.path == path).cloned().ok_or("The device is not there any more.")?;
    if !d.paired {
        let guard = Pairing::start(&path);
        let r = c.call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), "Pair", &()).await;
        drop(guard);
        r.map_err(why)?;
        // it may connect again by itself later (headphones switched on)
        set_prop(&c, &path, DEVICE, "Trusted", Value::from(true)).await?;
    }
    c.call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), "Connect", &()).await.map_err(why)?;
    Ok(format!("{} is connected.", d.name))
}

pub async fn disconnect(path: String) -> Result<String, String> {
    let c = conn().await?;
    c.call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), "Disconnect", &()).await.map_err(why)?;
    Ok("Disconnected.".into())
}

/// Forgets a paired device: it has to be paired again to be used.
pub async fn forget(path: String) -> Result<String, String> {
    let c = conn().await?;
    let st = state().await;
    let dev = OwnedObjectPath::try_from(path.as_str()).map_err(|e| e.to_string())?;
    c.call_method(Some(BLUEZ), st.adapter.as_str(), Some(ADAPTER), "RemoveDevice", &(dev,)).await.map_err(why)?;
    Ok("Forgotten.".into())
}

// ------------------------------------------------------------ agent

#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// type this on the device (a keyboard), then Enter there
    Show(String),
    /// does the device show this number? (answer with `answer`)
    Confirm(String),
    Done,
}

struct Agent {
    out: Mutex<Sender<AgentEvent>>,
}

impl Agent {
    fn tell(&self, e: AgentEvent) {
        if let Ok(mut o) = self.out.lock() {
            let _ = o.try_send(e);
        }
    }
    fn allowed(&self, device: &OwnedObjectPath) -> zbus::fdo::Result<()> {
        if pairing().as_deref() == Some(device.as_str()) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied("Pairing is started from Chlorum's Center.".into()))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_pin_code(&self, device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        self.allowed(&device)?;
        // old devices with a fixed code: almost always this one
        self.tell(AgentEvent::Show("0000".into()));
        Ok("0000".into())
    }

    fn display_pin_code(&self, device: OwnedObjectPath, pincode: String) -> zbus::fdo::Result<()> {
        self.allowed(&device)?;
        self.tell(AgentEvent::Show(pincode));
        Ok(())
    }

    fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        // we cannot type a number in: the device has to show it to us
        Err(zbus::fdo::Error::NotSupported("No way to type a passkey.".into()))
    }

    fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, _entered: u16) -> zbus::fdo::Result<()> {
        self.allowed(&device)?;
        self.tell(AgentEvent::Show(format!("{passkey:06}")));
        Ok(())
    }

    /// The device shows a number too: the user compares and says yes or no
    /// (no answer in 30 seconds is a no).
    async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32) -> zbus::fdo::Result<()> {
        self.allowed(&device)?;
        let (tx, rx) = oneshot::channel();
        if let Ok(mut a) = ANSWER.lock() {
            *a = Some(tx);
        }
        self.tell(AgentEvent::Confirm(format!("{passkey:06}")));
        let yes = tokio::time::timeout(Duration::from_secs(30), rx).await.ok().and_then(|r| r.ok()).unwrap_or(false);
        self.tell(AgentEvent::Done);
        if yes && pairing().as_deref() == Some(device.as_str()) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied("The numbers were not confirmed.".into()))
        }
    }

    fn request_authorization(&self, device: OwnedObjectPath) -> zbus::fdo::Result<()> {
        self.allowed(&device)
    }

    /// A device wants to use a service (sound, input…): only a device that
    /// is paired and trusted (paired from Center) may.
    async fn authorize_service(&self, device: OwnedObjectPath, _uuid: String) -> zbus::fdo::Result<()> {
        let ok = async {
            let c = conn().await.ok()?;
            let objs = objects(&c).await.ok()?;
            let d = objs.get(&device)?.get(DEVICE)?;
            Some(b(d, "Paired") && b(d, "Trusted"))
        }
        .await
        .unwrap_or(false);
        if ok { Ok(()) } else { Err(zbus::fdo::Error::AccessDenied("Only devices paired from Center.".into())) }
    }

    fn cancel(&self) {
        self.tell(AgentEvent::Done);
    }
}

/// The agent, for the whole session.
pub fn agent() -> impl Stream<Item = AgentEvent> {
    cosmic::iced::stream::channel(8, async |out: Sender<AgentEvent>| {
        let run = async {
            let c = zbus::connection::Builder::system()?.serve_at(AGENT_PATH, Agent { out: Mutex::new(out.clone()) })?.build().await?;
            let path = OwnedObjectPath::try_from(AGENT_PATH).map_err(zbus::Error::from)?;
            c.call_method(Some(BLUEZ), "/org/bluez", Some("org.bluez.AgentManager1"), "RegisterAgent", &(path.clone(), "DisplayYesNo")).await?;
            c.call_method(Some(BLUEZ), "/org/bluez", Some("org.bluez.AgentManager1"), "RequestDefaultAgent", &(path,)).await?;
            Ok::<zbus::Connection, zbus::Error>(c)
        };
        match run.await {
            // keep the connection (and the agent) for the session
            Ok(_c) => std::future::pending::<()>().await,
            Err(e) => {
                eprintln!("chlorum: bluetooth agent: {e}");
                let mut out = out;
                let _ = out.send(AgentEvent::Done).await;
                std::future::pending::<()>().await
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        let d = Device { path: "/x".into(), name: "WH-1000XM5".into(), address: "AA".into(), icon: "audio-headset".into(), paired: true, connected: false, battery: None };
        assert_eq!(d.kind(), "Headphones");
        // nobody may pair without Center starting it, and only that device
        assert!(pairing().is_none());
        {
            let _g = Pairing::start("/org/bluez/hci0/dev_AA");
            assert_eq!(pairing().as_deref(), Some("/org/bluez/hci0/dev_AA"));
        }
        assert!(pairing().is_none());
    }
}
