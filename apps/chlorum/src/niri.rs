//! Talking to niri, the compositor: it draws and animates the windows; the
//! shell tells it where every window goes.

use cosmic::iced::futures::{SinkExt, Stream};
pub use niri_ipc::{Action, Event, Request, Response, SizeChange, Window, Workspace, WorkspaceReferenceArg};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn socket() -> Option<PathBuf> {
    std::env::var_os("NIRI_SOCKET").map(PathBuf::from)
}

pub async fn send(req: Request) -> Result<Response, String> {
    let path = socket().ok_or("not running under niri")?;
    let mut stream = UnixStream::connect(path).await.map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).await.map_err(|e| e.to_string())?;
    let reply: niri_ipc::Reply = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    reply
}

/// Runs the actions one after another, in order.
pub async fn run(actions: Vec<Action>) {
    for a in actions {
        if let Err(e) = send(Request::Action(a)).await {
            eprintln!("chlorum: niri: {e}");
        }
    }
}

/// Every change niri reports, for as long as the session runs.
pub fn events() -> impl Stream<Item = Event> {
    cosmic::iced::stream::channel(64, async |mut out| {
        loop {
            if let Some(path) = socket() {
                if let Ok(mut stream) = UnixStream::connect(&path).await {
                    let req = serde_json::to_string(&Request::EventStream).unwrap() + "\n";
                    if stream.write_all(req.as_bytes()).await.is_ok() {
                        let mut lines = BufReader::new(stream).lines();
                        let _ = lines.next_line().await; // the "Handled" reply
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                                if out.send(ev).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    })
}
