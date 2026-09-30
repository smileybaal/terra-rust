//! Many concurrent clients, through the real binary.
//!
//! `MaxConnections` is 256 (`Netplay.cs:30`). The two things worth proving are that the
//! limit is enforced with the C# "server is full" kick, and that the live-connection
//! counter comes back to zero once clients leave: a leak there would refuse every
//! future client forever, which is the failure mode this file exists to catch.
//!
//! Every socket has a timeout (see `common`), and the whole file is bounded work: 256
//! handshakes on loopback, then a fresh one. Nothing waits on a silent server.

mod common;

use common::*;
use std::io::Cursor;
use std::net::TcpStream;
use std::time::{Duration, Instant};

const MAX: usize = 256;

/// Complete the handshake, which proves the server accepted this socket, counted it,
/// and ran its thread. Returns the still-open socket so the client stays "live".
fn full_handshake(port: u16) -> TcpStream {
    let mut c = connect(port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    let (id, _) = read_packet(&mut c).expect("no answer to Hello");
    assert_eq!(
        id,
        MessageID::PlayerInfo,
        "expected PlayerInfo for a live client, got id {id}"
    );
    c
}

/// Read a kick body that travels as a localization key and return the key.
fn kick_key(body: &[u8]) -> String {
    assert_eq!(body[0], 2, "a kick text travels as Mode.LocalizationKey");
    read_string(&mut Cursor::new(&body[1..])).unwrap()
}

/// Fill every slot, then the next client must be kicked with `CLI.ServerIsFull` (not
/// dropped silently), and after all of them leave the counter must be zero again so a
/// fresh client is accepted.
#[test]
fn the_257th_client_is_kicked_and_the_counter_returns_to_zero() {
    let server = start_server(&["-port", "0"]);
    let port = server.port;
    assert_ne!(port, 0, "the server must report its real port");

    let mut live = Vec::with_capacity(MAX);
    for _ in 0..MAX {
        live.push(full_handshake(port));
    }

    // ~300 connections in total: every one past the first 256 must be kicked with the
    // server-is-full key, not dropped silently and not served.
    for _ in 0..44 {
        let mut extra = connect(port);
        let (id, body) = read_packet(&mut extra).expect("an over-capacity client got no answer");
        assert_eq!(
            id,
            MessageID::Kick,
            "a client past MaxConnections should be kicked, not served (got id {id})"
        );
        assert_eq!(kick_key(&body), "CLI.ServerIsFull");
    }

    // everyone leaves; the server's threads should notice and release their slots
    drop(live);

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last: String;
    loop {
        let mut c = connect(port);
        let (id, body) = hello_packet(&connect_string());
        write_packet(&mut c, id, &body).unwrap();
        match read_packet(&mut c) {
            Ok((id, _)) if id == MessageID::PlayerInfo => return, // a fresh client is accepted
            Ok((id, body)) => last = format!("id {id}, body {body:?}"),
            Err(e) => last = format!("read error {e}"),
        }
        if Instant::now() >= deadline {
            panic!(
                "after all {MAX} clients disconnected a fresh client was still refused ({last}); \
                 the live-connection counter leaked"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A client that connects and disconnects before it ever says Hello must release its
/// slot. If it leaked, a burst of connect-and-go would permanently exhaust the server.
#[test]
fn clients_that_leave_before_hello_leak_nothing() {
    let server = start_server(&["-port", "0"]);
    let port = server.port;
    assert_ne!(port, 0);

    for _ in 0..(MAX + 16) {
        let c = connect(port);
        drop(c);
    }

    // a real client must still be able to complete a handshake
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let mut c = connect(port);
        let (id, body) = hello_packet(&connect_string());
        write_packet(&mut c, id, &body).unwrap();
        match read_packet(&mut c) {
            Ok((id, _)) if id == MessageID::PlayerInfo => return,
            Ok((id, _)) if id == MessageID::Kick => {
                assert!(
                    Instant::now() < deadline,
                    "clients that disconnected before Hello exhausted the slots: a fresh client was kicked"
                );
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            panic!("a fresh client could not be served after connect-and-disconnect churn");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
