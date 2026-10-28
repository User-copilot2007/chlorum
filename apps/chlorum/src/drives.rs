//! Flash drives and other removable disks, through UDisks2 (the system's
//! disk service): find them, unlock encrypted ones (LUKS, BitLocker), open
//! them, and take them out safely. File systems: whatever the system can
//! mount — FAT, exFAT, NTFS, ext4, Btrfs, XFS, F2FS, ISO 9660, UDF.

use std::collections::HashMap;
use std::path::PathBuf;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const UDISKS: &str = "org.freedesktop.UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const FS: &str = "org.freedesktop.UDisks2.Filesystem";
const CRYPT: &str = "org.freedesktop.UDisks2.Encrypted";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// encrypted and locked: needs its password
    Locked,
    /// ready to open
    Closed,
    Open(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    /// the UDisks object of the partition (the encrypted one, when it is)
    pub id: String,
    pub name: String,
    /// "exFAT", "NTFS", "LUKS"…
    pub kind: String,
    pub encrypted: bool,
    pub size: u64,
    pub state: State,
    /// the partition as a device file (/dev/sdb1), for the security-key helper
    pub device: String,
    drive: String,
    /// the unlocked partition, when an encrypted one is open
    clear: Option<String>,
}

type Props = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

fn s(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}
fn b(p: &Props, k: &str) -> bool {
    p.get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}
fn t(p: &Props, k: &str) -> u64 {
    p.get(k).and_then(|v| u64::try_from(v).ok()).unwrap_or(0)
}
fn o(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok()).map(|p| p.to_string()).unwrap_or_default()
}
fn bytes(p: &Props, k: &str) -> String {
    let v: Vec<u8> = p.get(k).and_then(|v| Vec::<u8>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
    String::from_utf8_lossy(&v).trim_end_matches('\0').to_string()
}
fn mounts(p: &Props) -> Vec<PathBuf> {
    p.get("MountPoints")
        .and_then(|v| Vec::<Vec<u8>>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|m| PathBuf::from(String::from_utf8_lossy(&m).trim_end_matches('\0').to_string()))
        .collect()
}

/// A readable name for a file system type.
pub fn kind_label(id_type: &str) -> String {
    match id_type {
        "vfat" => "FAT".into(),
        "exfat" => "exFAT".into(),
        "ntfs" => "NTFS".into(),
        "crypto_LUKS" => "LUKS".into(),
        "BitLocker" => "BitLocker".into(),
        "iso9660" => "Disc image".into(),
        "udf" => "UDF".into(),
        "" => "Unknown".into(),
        other => other.to_uppercase(),
    }
}

/// "16 GB", "512 MB".
pub fn size_label(n: u64) -> String {
    let gb = n as f64 / 1e9;
    if gb >= 1.0 { format!("{:.0} GB", gb.round().max(1.0)) } else { format!("{:.0} MB", (n as f64 / 1e6).max(1.0)) }
}

async fn objects(conn: &zbus::Connection) -> zbus::Result<Objects> {
    let msg = conn
        .call_method(Some(UDISKS), "/org/freedesktop/UDisks2", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
        .await?;
    msg.body().deserialize::<Objects>()
}

/// The removable drives people plug in (never the computer's own disks).
fn pick(objs: &Objects) -> Vec<Drive> {
    let mut out = Vec::new();
    for (path, ifaces) in objs {
        let Some(block) = ifaces.get(BLOCK) else { continue };
        if b(block, "HintSystem") || b(block, "HintIgnore") {
            continue;
        }
        // an unlocked partition is shown through its encrypted parent
        if o(block, "CryptoBackingDevice").len() > 1 {
            continue;
        }
        let drive_path = o(block, "Drive");
        let Some(drive) = objs.iter().find(|(p, _)| p.as_str() == drive_path).and_then(|(_, i)| i.get(DRIVE)) else { continue };
        if !(b(drive, "Removable") || b(drive, "MediaRemovable") || s(drive, "ConnectionBus") == "usb") {
            continue;
        }
        let usage = s(block, "IdUsage");
        let id_type = s(block, "IdType");
        let encrypted = ifaces.contains_key(CRYPT);
        if usage != "filesystem" && !encrypted {
            continue;
        }
        let label = s(block, "IdLabel");
        let model = format!("{} {}", s(drive, "Vendor"), s(drive, "Model")).trim().to_string();
        let name = if !label.is_empty() {
            label
        } else if !model.is_empty() {
            model
        } else {
            bytes(block, "Device").trim_start_matches("/dev/").to_string()
        };
        let (state, clear) = if encrypted {
            let c = ifaces.get(CRYPT).map(|p| o(p, "CleartextDevice")).unwrap_or_default();
            if c.len() > 1 {
                let fs = objs.iter().find(|(p, _)| p.as_str() == c).and_then(|(_, i)| i.get(FS));
                match fs.map(mounts).and_then(|m| m.into_iter().next()) {
                    Some(m) => (State::Open(m), Some(c)),
                    None => (State::Closed, Some(c)),
                }
            } else {
                (State::Locked, None)
            }
        } else {
            match ifaces.get(FS).map(mounts).and_then(|m| m.into_iter().next()) {
                Some(m) => (State::Open(m), None),
                None => (State::Closed, None),
            }
        };
        out.push(Drive {
            id: path.to_string(),
            name,
            kind: kind_label(&id_type),
            encrypted,
            size: t(block, "Size"),
            state,
            device: bytes(block, "Device"),
            drive: drive_path,
            clear,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

pub async fn list() -> Vec<Drive> {
    let Ok(conn) = zbus::Connection::system().await else { return vec![] };
    objects(&conn).await.map(|o| pick(&o)).unwrap_or_default()
}

fn no_opts() -> HashMap<&'static str, Value<'static>> {
    HashMap::new()
}

/// Plain words for what went wrong.
fn why(e: zbus::Error) -> String {
    let m = e.to_string();
    if m.contains("passphrase") || m.contains("Failed to activate") || m.contains("No key available") {
        "That password does not open this drive.".into()
    } else if m.contains("NotAuthorized") {
        "Chlorum is not allowed to open this drive.".into()
    } else if m.contains("busy") || m.contains("target is busy") {
        "Something still uses this drive. Close it there first.".into()
    } else {
        m.rsplit(": ").next().unwrap_or(&m).to_string()
    }
}

/// Opens the drive (unlocking it first with `password` when it is
/// encrypted) and returns where its files are.
pub async fn open(d: Drive, password: Option<String>) -> Result<PathBuf, String> {
    if let State::Open(p) = &d.state {
        return Ok(p.clone());
    }
    let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;
    let fs_path = if d.encrypted {
        match d.clear.clone() {
            Some(c) => c,
            None => {
                let pw = password.ok_or("This drive is locked: it needs its password.")?;
                let msg = conn
                    .call_method(Some(UDISKS), d.id.as_str(), Some(CRYPT), "Unlock", &(pw.as_str(), no_opts()))
                    .await
                    .map_err(why)?;
                msg.body().deserialize::<OwnedObjectPath>().map_err(|e| e.to_string())?.to_string()
            }
        }
    } else {
        d.id.clone()
    };
    let msg = conn.call_method(Some(UDISKS), fs_path.as_str(), Some(FS), "Mount", &(no_opts(),)).await.map_err(why)?;
    let mount: String = msg.body().deserialize().map_err(|e| e.to_string())?;
    Ok(PathBuf::from(mount))
}

/// Closes everything on the drive and switches it off, so it can be pulled
/// out.
pub async fn eject(d: Drive) -> Result<(), String> {
    let conn = zbus::Connection::system().await.map_err(|e| e.to_string())?;
    let objs = objects(&conn).await.map_err(|e| e.to_string())?;
    // every mounted partition of this drive, unlocked ones included
    for (path, ifaces) in &objs {
        let Some(block) = ifaces.get(BLOCK) else { continue };
        let backing = o(block, "CryptoBackingDevice");
        let on_drive = o(block, "Drive") == d.drive
            || objs.iter().any(|(p, i)| p.as_str() == backing && i.get(BLOCK).is_some_and(|bb| o(bb, "Drive") == d.drive));
        if !on_drive {
            continue;
        }
        if let Some(fs) = ifaces.get(FS) {
            if !mounts(fs).is_empty() {
                conn.call_method(Some(UDISKS), path.as_str(), Some(FS), "Unmount", &(no_opts(),)).await.map_err(why)?;
            }
        }
    }
    for (path, ifaces) in &objs {
        let Some(block) = ifaces.get(BLOCK) else { continue };
        if o(block, "Drive") != d.drive {
            continue;
        }
        if let Some(c) = ifaces.get(CRYPT) {
            let clear = o(c, "CleartextDevice");
            if clear.len() > 1 {
                // opened with a security key (by chlorum keyd, as root):
                // closed by the same helper
                let by_key = objs
                    .iter()
                    .find(|(p, _)| p.as_str() == clear)
                    .and_then(|(_, i)| i.get(BLOCK))
                    .is_some_and(|b| bytes(b, "PreferredDevice").contains("chlorum-key-") || s(b, "Id").contains("chlorum-key-"));
                if by_key {
                    crate::keyd::lock(bytes(block, "Device")).await?;
                } else {
                    conn.call_method(Some(UDISKS), path.as_str(), Some(CRYPT), "Lock", &(no_opts(),)).await.map_err(why)?;
                }
            }
        }
    }
    // some readers cannot switch off; then it is safe to pull out anyway
    let _ = conn.call_method(Some(UDISKS), d.drive.as_str(), Some(DRIVE), "PowerOff", &(no_opts(),)).await;
    Ok(())
}

/// One event each time a drive is plugged in (its name).
pub fn plugged() -> impl cosmic::iced::futures::Stream<Item = String> {
    use cosmic::iced::futures::{SinkExt, StreamExt};
    cosmic::iced::stream::channel(8, async |mut out: cosmic::iced::futures::channel::mpsc::Sender<String>| {
        let run = async {
            let conn = zbus::Connection::system().await?;
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.DBus.ObjectManager")?
                .member("InterfacesAdded")?
                .path("/org/freedesktop/UDisks2")?
                .build();
            let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, Some(16)).await?;
            let mut known: Vec<String> = pick(&objects(&conn).await?).into_iter().map(|d| d.id).collect();
            while let Some(Ok(_)) = stream.next().await {
                // partitions show up a moment after the drive itself
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                let now = pick(&objects(&conn).await?);
                for d in &now {
                    if !known.contains(&d.id) {
                        let _ = out.send(d.name.clone()).await;
                    }
                }
                known = now.into_iter().map(|d| d.id).collect();
            }
            Ok::<(), zbus::Error>(())
        };
        if let Err(e) = run.await {
            eprintln!("chlorum: drives: {e}");
        }
        std::future::pending::<()>().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(kind_label("vfat"), "FAT");
        assert_eq!(kind_label("crypto_LUKS"), "LUKS");
        assert_eq!(kind_label("ext4"), "EXT4");
        assert_eq!(size_label(16_000_000_000), "16 GB");
        assert_eq!(size_label(512_000_000), "512 MB");
    }
}
