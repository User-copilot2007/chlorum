//! Bluetooth through BlueZ (the system's Bluetooth service, on D-Bus):
//! switch it on or off, look for devices, pair, connect, disconnect,
//! forget. Center → Bluetooth.
//!
//! Pairing some devices needs an answer (a keyboard shows a number to type
//! on it): the shell registers an agent that BlueZ asks. It says yes only
//! while a pairing started from Center is under way, never to a device
//! that asks on its own.

use cosmic::iced::futures::channel::mpsc::Sender;
use cosmic::iced::futures::{SinkExt, Stream};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const PROPS: &str = "org.freedesktop.DBus.Properties";
const AGENT_PATH: &str = "/org/chlorum/bluetooth";

/// A pairing started from Center is under way: the agent may say yes.
static PAIRING: AtomicBool = AtomicBool::new(false);

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
    zbus::Connection::system().await.map_err(|e| e.to_string())
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
        PAIRING.store(true, Ordering::SeqCst);
        let r = c.call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), "Pair", &()).await;
        PAIRING.store(false, Ordering::SeqCst);
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
    /// the device shows this number too: it is pairing (accepted)
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
    fn allowed(&self) -> zbus::fdo::Result<()> {
        if PAIRING.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AccessDenied("Pairing is started from Chlorum's Center.".into()))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_pin_code(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        self.allowed()?;
        // old devices with a fixed code: almost always this one
        self.tell(AgentEvent::Show("0000".into()));
        Ok("0000".into())
    }

    fn display_pin_code(&self, _device: OwnedObjectPath, pincode: String) -> zbus::fdo::Result<()> {
        self.allowed()?;
        self.tell(AgentEvent::Show(pincode));
        Ok(())
    }

    fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        // we cannot type a number in: the device has to show it to us
        Err(zbus::fdo::Error::NotSupported("No way to type a passkey.".into()))
    }

    fn display_passkey(&self, _device: OwnedObjectPath, passkey: u32, _entered: u16) -> zbus::fdo::Result<()> {
        self.allowed()?;
        self.tell(AgentEvent::Show(format!("{passkey:06}")));
        Ok(())
    }

    fn request_confirmation(&self, _device: OwnedObjectPath, passkey: u32) -> zbus::fdo::Result<()> {
        self.allowed()?;
        self.tell(AgentEvent::Confirm(format!("{passkey:06}")));
        Ok(())
    }

    fn request_authorization(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<()> {
        self.allowed()
    }

    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) -> zbus::fdo::Result<()> {
        // only devices that are paired already ask this
        Ok(())
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
        // nobody may pair without Center starting it
        assert!(!PAIRING.load(Ordering::SeqCst));
    }
}
