// SPDX-License-Identifier: GPL-3.0-or-later
//! Talking to the Discord app on this computer, to set the "Playing …" status.
//!
//! Discord listens on a Unix socket, `discord-ipc-0` … `discord-ipc-9`, in
//! `$XDG_RUNTIME_DIR` (or a subfolder for the Flatpak and Snap versions). Each
//! message is a frame: two little-endian 32-bit numbers, the *opcode* and the
//! payload length, followed by that many bytes of JSON.
//!
//! ```text
//! opcode 0  handshake  {"v": 1, "client_id": "<application id>"}
//! opcode 1  frame      {"cmd": "SET_ACTIVITY", "args": {...}, "nonce": "1"}
//! opcode 2  close      sent by Discord when it refuses or ends the connection
//! ```
//!
//! The application ID belongs to Feline's own Discord application, which
//! decides the "Playing Feline" name and holds the `feline` logo image.

use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// Feline's Discord application (registered by the Feline project).
pub const APPLICATION_ID: &str = "1567489166411833405";

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;

/// Larger replies than this mean something is wrong; don't buffer them.
const MAX_FRAME: u32 = 64 * 1024;

#[derive(Debug)]
pub enum DiscordError {
    /// No Discord socket was found: Discord (or a client such as Vesktop) isn't running.
    NotRunning,
    Io(io::Error),
    /// Discord closed the connection, with its reason.
    Closed(String),
    /// Discord answered with something unexpected.
    Protocol(String),
}

impl fmt::Display for DiscordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("Discord isn't running"),
            Self::Io(e) => write!(f, "lost the connection to Discord ({e})"),
            Self::Closed(why) => write!(f, "Discord closed the connection: {why}"),
            Self::Protocol(what) => write!(f, "unexpected answer from Discord: {what}"),
        }
    }
}

impl std::error::Error for DiscordError {}

impl From<io::Error> for DiscordError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The sockets to try, in order.
pub fn socket_candidates(runtime_dir: &Path) -> Vec<PathBuf> {
    // The plain folder is used by native Discord, Vesktop, Equibop and others.
    let dirs = [
        runtime_dir.to_path_buf(),
        runtime_dir.join("app/com.discordapp.Discord"),
        runtime_dir.join("app/com.discordapp.DiscordCanary"),
        runtime_dir.join("app/dev.vencord.Vesktop"),
        runtime_dir.join("snap.discord"),
    ];
    dirs.iter()
        .flat_map(|dir| (0..10).map(move |n| dir.join(format!("discord-ipc-{n}"))))
        .collect()
}

/// An open connection to Discord.
pub struct Discord {
    stream: UnixStream,
    nonce: u64,
}

impl Discord {
    /// Connects to the first Discord socket that answers, and says hello.
    pub fn connect() -> Result<Self, DiscordError> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let stream = socket_candidates(&runtime_dir)
            .iter()
            .find_map(|path| UnixStream::connect(path).ok())
            .ok_or(DiscordError::NotRunning)?;
        Self::handshake(stream)
    }

    /// Sends the handshake on an already open stream (separate for tests).
    pub fn handshake(stream: UnixStream) -> Result<Self, DiscordError> {
        // Bounded waits, so a stuck Discord can't hang Feline's thread.
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut discord = Self { stream, nonce: 0 };
        discord.send(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": APPLICATION_ID }),
        )?;
        // Discord answers with a READY event, or closes with a reason.
        let reply = discord.receive()?;
        if reply.get("evt").and_then(Value::as_str) != Some("READY") {
            return Err(DiscordError::Protocol(reply.to_string()));
        }
        Ok(discord)
    }

    /// Shows `activity` as the status. `None` clears it.
    pub fn set_activity(&mut self, activity: Option<Value>) -> Result<(), DiscordError> {
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            // Discord clears the status itself when this process exits.
            "args": { "pid": std::process::id(), "activity": activity },
            "nonce": self.nonce.to_string(),
        });
        self.send(OP_FRAME, &payload)?;
        let reply = self.receive()?;
        if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
            let message = reply
                .pointer("/data/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(DiscordError::Protocol(message.to_owned()));
        }
        Ok(())
    }

    fn send(&mut self, opcode: u32, payload: &Value) -> Result<(), DiscordError> {
        let body = payload.to_string();
        let len = u32::try_from(body.len())
            .map_err(|_| DiscordError::Protocol("message too large".to_owned()))?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.stream.write_all(&frame)?;
        Ok(())
    }

    fn receive(&mut self) -> Result<Value, DiscordError> {
        let mut header = [0u8; 8];
        self.stream.read_exact(&mut header)?;
        let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if len > MAX_FRAME {
            return Err(DiscordError::Protocol(format!("{len}-byte reply")));
        }
        let mut body = vec![0u8; len as usize];
        self.stream.read_exact(&mut body)?;
        let value: Value = serde_json::from_slice(&body)
            .map_err(|e| DiscordError::Protocol(format!("invalid JSON ({e})")))?;
        if opcode == OP_CLOSE {
            let why = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            return Err(DiscordError::Closed(why.to_owned()));
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads one frame on the "Discord" side of a test socket pair.
    fn read_frame(stream: &mut UnixStream) -> (u32, Value) {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).unwrap();
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let len = u32::from_le_bytes(header[4..].try_into().unwrap());
        let mut body = vec![0u8; len as usize];
        stream.read_exact(&mut body).unwrap();
        (op, serde_json::from_slice(&body).unwrap())
    }

    fn write_frame(stream: &mut UnixStream, op: u32, value: &Value) {
        let body = value.to_string();
        stream.write_all(&op.to_le_bytes()).unwrap();
        stream
            .write_all(&(body.len() as u32).to_le_bytes())
            .unwrap();
        stream.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn handshake_and_set_activity_with_a_fake_discord() {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let fake = std::thread::spawn(move || {
            let (op, hello) = read_frame(&mut theirs);
            write_frame(
                &mut theirs,
                OP_FRAME,
                &json!({"cmd": "DISPATCH", "evt": "READY"}),
            );
            let (op2, set) = read_frame(&mut theirs);
            write_frame(
                &mut theirs,
                OP_FRAME,
                &json!({"cmd": "SET_ACTIVITY", "evt": null}),
            );
            (op, hello, op2, set)
        });

        let mut discord = Discord::handshake(ours).unwrap();
        discord
            .set_activity(Some(json!({"details": "Fishing"})))
            .unwrap();
        let (op, hello, op2, set) = fake.join().unwrap();

        assert_eq!(op, OP_HANDSHAKE);
        assert_eq!(hello, json!({"v": 1, "client_id": APPLICATION_ID}));
        assert_eq!(op2, OP_FRAME);
        assert_eq!(set["cmd"], "SET_ACTIVITY");
        assert_eq!(set["args"]["activity"]["details"], "Fishing");
        assert_eq!(set["args"]["pid"], std::process::id());
    }

    #[test]
    fn a_refused_handshake_is_an_error() {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let fake = std::thread::spawn(move || {
            read_frame(&mut theirs);
            write_frame(
                &mut theirs,
                OP_CLOSE,
                &json!({"code": 4000, "message": "Invalid Client ID"}),
            );
        });
        let result = Discord::handshake(ours);
        fake.join().unwrap();
        assert!(matches!(result, Err(DiscordError::Closed(why)) if why == "Invalid Client ID"));
    }

    #[test]
    fn candidates_cover_flatpak_and_snap() {
        let list = socket_candidates(Path::new("/run/user/1000"));
        assert_eq!(list[0], Path::new("/run/user/1000/discord-ipc-0"));
        assert!(list.contains(&PathBuf::from(
            "/run/user/1000/app/com.discordapp.Discord/discord-ipc-0"
        )));
        assert_eq!(list.len(), 50);
    }
}
