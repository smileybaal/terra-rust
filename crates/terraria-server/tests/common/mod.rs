//! Shared harness for the `terraria-server` integration tests.
//!
//! The server SERVES now (`kernel/net.rs`), so `boot::run` does not return once it
//! starts listening: a test cannot call it in-process. Every test here therefore
//! starts the REAL binary, learns the port it actually bound by reading its own
//! `listening on ...` line, and talks the wire protocol over a socket.
//!
//! Two rules are load-bearing:
//!   - every socket has a read/write timeout, so a missing response FAILS in seconds
//!     instead of hanging the suite (the mistake this repo made once already);
//!   - the server's stdout is drained continuously, because a full OS pipe buffer
//!     would block the server's accept loop and hang the test from the other side.
//!
//! Message ids come from the projection (`terraria_kernel::port`), never retyped.

#![allow(dead_code)]

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub use terraria_kernel::port::terraria::id::MessageID;

/// A running server process plus the port it actually bound.
pub struct Server {
    pub child: Child,
    pub port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        // never leave a server process holding a port
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start the real binary with `args`, wait for it to say which port it is listening
/// on, and return that port.
///
/// The port is read from the server's own output rather than assumed, which is the
/// point: `-port 0` asks the OS for an ephemeral port and the ONLY way a caller can
/// learn it is if the server prints the address it really bound.
pub fn start_server(args: &[&str]) -> Server {
    let exe = env!("CARGO_BIN_EXE_terraria-server");
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn terraria-server");

    let stdout = child.stdout.take().expect("stdout was piped");
    let (tx, rx) = mpsc::channel::<u16>();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if let Some(addr) = line.trim().strip_prefix("listening on ") {
                        let port = addr
                            .rsplit(':')
                            .next()
                            .and_then(|p| p.trim().parse::<u16>().ok())
                            .unwrap_or(0);
                        let _ = tx.send(port);
                        // Keep draining forever: a full pipe would block the server's
                        // accept loop, which under hundreds of connections would hang
                        // the test. The pipe closes when the child is killed.
                        let mut sink = String::new();
                        loop {
                            sink.clear();
                            if reader.read_line(&mut sink).unwrap_or(0) == 0 {
                                return;
                            }
                        }
                    }
                }
            }
        }
    });

    let port = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the server never printed a `listening on` line");
    Server { child, port }
}

/// Run the binary to completion and return `(exit_code, stdout, stderr)`.
///
/// Only used for the refusal paths, which exit immediately; if the process were to
/// serve instead, `wait_with_output` would block, so callers must pass an argument
/// that makes it refuse. A hard cap is applied by the caller's own timeout.
pub fn run_to_exit(args: &[&str]) -> (Option<i32>, String, String) {
    let exe = env!("CARGO_BIN_EXE_terraria-server");
    let out = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn terraria-server");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A port that was free a moment ago. Bound, read, then released, so a test can name
/// a concrete port instead of asking for an ephemeral one. The window between release
/// and the server's bind is tiny and, if it ever loses the race, the test fails loudly
/// rather than passing wrongly.
pub fn free_port() -> u16 {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind for free_port");
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// Connect to a port with read/write timeouts, so a silent server fails the test
/// rather than hanging it.
pub fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    let d = Some(Duration::from_secs(10));
    s.set_read_timeout(d).expect("set_read_timeout");
    s.set_write_timeout(d).expect("set_write_timeout");
    s
}

/// Try to connect without panicking, for a port that may legitimately be unreachable.
pub fn try_connect(port: u16) -> Option<TcpStream> {
    match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => {
            let d = Some(Duration::from_secs(10));
            let _ = s.set_read_timeout(d);
            let _ = s.set_write_timeout(d);
            Some(s)
        }
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// the wire, mirroring `kernel/net.rs` (a contract with a C# reader)
// ---------------------------------------------------------------------------

/// `BinaryWriter.Write(string)`: 7-bit encoded byte length, then UTF-8.
pub fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    let mut len = s.len();
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        w.write_all(&[byte])?;
        if len == 0 {
            break;
        }
    }
    w.write_all(s.as_bytes())
}

pub fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    let mut len = 0usize;
    let mut shift = 0u32;
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        len |= ((b[0] & 0x7F) as usize) << shift;
        if b[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "7-bit length is too long"));
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// `NetMessage.SendData`: a 2-byte little-endian length covering the id plus body.
pub fn write_packet<W: Write>(w: &mut W, id: u8, body: &[u8]) -> io::Result<()> {
    let len = 1 + body.len();
    w.write_all(&(len as u16).to_le_bytes())?;
    w.write_all(&[id])?;
    w.write_all(body)?;
    w.flush()
}

pub fn read_packet<R: Read>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)?;
    let len = u16::from_le_bytes(len) as usize;
    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty packet"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let id = buf[0];
    buf.remove(0);
    Ok((id, buf))
}

/// `Hello` with the given greeting, ready to send.
pub fn hello_packet(greeting: &str) -> (u8, Vec<u8>) {
    let mut body = Vec::new();
    write_string(&mut body, greeting).unwrap();
    (MessageID::Hello, body)
}

/// The greeting a compliant client sends (`Terraria` + the current release).
pub fn connect_string() -> String {
    format!("Terraria{}", terraria_kernel::port::terraria::Main::curRelease)
}
