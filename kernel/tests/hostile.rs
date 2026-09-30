//! The wire layer under hostile input, over real sockets.
//!
//! These began as print-only probes, which is a way to find out what happens but not a
//! way to keep it happening: a probe with no assertion passes for every behaviour,
//! including the broken ones, and this repository's own doctrine says a check that
//! cannot fail is not a check. Each case below asserts the two things that matter:
//!
//!   1. the hostile client must never be SERVED (`assert_not_served`), and
//!   2. the server must still be alive and still serve a fresh, well-formed client
//!      afterwards (`assert_still_served`).
//!
//! Every socket has a read/write timeout, because a test that can hang blocks the whole
//! suite and proves nothing while it does.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::Duration;

use terraria_kernel::net::{bind, serve, CONNECT_STRING};

fn start(password: Option<String>) -> SocketAddr {
    let l = bind(0).unwrap();
    let port = l.local_addr().unwrap().port();
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    thread::spawn(move || {
        let _ = serve(l, password, None);
    });
    addr
}

fn connect(addr: SocketAddr) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    let d = Some(Duration::from_secs(2));
    s.set_read_timeout(d).unwrap();
    s.set_write_timeout(d).unwrap();
    s
}

fn pkt(id: u8, body: &[u8]) -> Vec<u8> {
    let mut v = ((1 + body.len()) as u16).to_le_bytes().to_vec();
    v.push(id);
    v.extend_from_slice(body);
    v
}

fn str7(s: &str) -> Vec<u8> {
    let mut len = s.len();
    let mut v = Vec::new();
    loop {
        let mut b = (len & 0x7f) as u8;
        len >>= 7;
        if len != 0 {
            b |= 0x80;
        }
        v.push(b);
        if len == 0 {
            break;
        }
    }
    v.extend_from_slice(s.as_bytes());
    v
}

/// Read one packet; None on EOF, reset or timeout.
fn recv(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut l = [0u8; 2];
    if s.read_exact(&mut l).is_err() {
        return None;
    }
    let n = u16::from_le_bytes(l) as usize;
    if n == 0 {
        return None;
    }
    let mut buf = vec![0u8; n];
    if s.read_exact(&mut buf).is_err() {
        return None;
    }
    Some((buf[0], buf[1..].to_vec()))
}

/// `MessageID::PlayerInfo` is 3: being handed it is being SERVED, which is the one
/// outcome a hostile input must never get. A kick (2), a close, or silence are all
/// acceptable; silence is what a timeout yields.
fn assert_not_served(what: &str, got: Option<(u8, Vec<u8>)>) {
    if let Some((id, body)) = got {
        assert_ne!(id, 3, "{what} was SERVED PlayerInfo {body:?}");
    }
}

/// The invariant the whole file exists for: after whatever the hostile client did, a
/// fresh well-formed client is still accepted.
fn assert_still_served(addr: SocketAddr) {
    let mut c = connect(addr);
    c.write_all(&pkt(1, &str7(CONNECT_STRING))).unwrap();
    match recv(&mut c) {
        Some((id, body)) => {
            assert_eq!(id, 3, "a fresh client must be handed PlayerInfo");
            assert_eq!(body, vec![0u8, 0u8], "slot 0 and a false bool");
        }
        None => panic!("the server stopped serving a fresh client after a hostile one"),
    }
}

fn hello() -> Vec<u8> {
    pkt(1, &str7(CONNECT_STRING))
}

// -- framing ---------------------------------------------------------------

#[test]
fn a_zero_length_frame_is_refused() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&[0, 0]).unwrap();
    assert_not_served("a zero-length frame", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_length_longer_than_the_body_followed_by_close_is_refused() {
    let a = start(None);
    {
        let mut c = connect(a);
        // claims 100 bytes of body, sends 3, then closes
        c.write_all(&[100, 0, 1, 2, 3]).unwrap();
    }
    thread::sleep(Duration::from_millis(200));
    assert_still_served(a);
}

#[test]
fn a_huge_length_with_no_body_does_not_stop_the_server() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&[0xff, 0xff]).unwrap();
    thread::sleep(Duration::from_millis(200));
    assert_still_served(a);
    drop(c);
}

#[test]
fn a_single_byte_then_silence_does_not_stop_the_server() {
    let a = start(None);
    let c = connect(a);
    let mut c = c;
    c.write_all(&[0x01]).unwrap();
    thread::sleep(Duration::from_millis(200));
    assert_still_served(a);
    drop(c);
}

// -- the claimed string length ---------------------------------------------

#[test]
fn a_claimed_256_mib_string_is_refused() {
    let a = start(None);
    let mut c = connect(a);
    // 7-bit length = 2^28, then one byte of the claimed 256 MiB
    let mut body = vec![0x80u8, 0x80, 0x80, 0x80, 0x01];
    body.push(b'T');
    c.write_all(&pkt(1, &body)).unwrap();
    assert_not_served("a 256 MiB string claim", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_claimed_34_gib_string_is_refused_and_the_process_lives() {
    let a = start(None);
    let mut c = connect(a);
    // 7-bit length = 127*(1+2^7+2^14+2^21+2^28) ~ 34 GiB, no payload at all
    c.write_all(&pkt(1, &[0xff, 0xff, 0xff, 0xff, 0x7f])).unwrap();
    assert_not_served("a 34 GiB string claim", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_truncated_string_is_refused() {
    let a = start(None);
    let mut c = connect(a);
    let mut body = vec![10u8]; // declares 10 bytes
    body.extend_from_slice(b"Ter"); // sends 3
    c.write_all(&pkt(1, &body)).unwrap();
    assert_not_served("a truncated string", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_hello_with_an_empty_body_is_refused() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&pkt(1, &[])).unwrap();
    assert_not_served("a Hello with no body", recv(&mut c));
    assert_still_served(a);
}

// -- handshake state -------------------------------------------------------

#[test]
fn a_second_hello_on_a_settled_connection_is_ignored() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&hello()).unwrap();
    assert_eq!(recv(&mut c).map(|p| p.0), Some(3), "the first Hello is served");
    // MessageBuffer.cs:199 refuses a second Hello once State != 0, so it is ignored
    // rather than answered. Silence here IS the correct behaviour.
    c.write_all(&hello()).unwrap();
    assert_not_served("a second Hello", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_message_before_hello_is_booted_with_the_state_key() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&pkt(6, &[])).unwrap(); // RequestWorldData while State == 0
    match recv(&mut c) {
        Some((id, body)) => {
            assert_eq!(id, 2, "booted, not served");
            assert_eq!(body[0], 2, "Mode.LocalizationKey");
        }
        None => panic!("a message before Hello must be answered with a kick, not silence"),
    }
    assert_still_served(a);
}

#[test]
fn a_wrong_greeting_closes_the_connection_to_further_traffic() {
    let a = start(None);
    let mut c = connect(a);
    c.write_all(&pkt(1, &str7("Terraria999"))).unwrap();
    assert_eq!(recv(&mut c).map(|p| p.0), Some(2), "kicked");
    // The kick is the end of the connection: a second Hello cannot be answered.
    let _ = c.write_all(&hello());
    assert_not_served("a Hello after a kick", recv(&mut c));
    assert_still_served(a);
}

#[test]
fn a_non_minimal_zero_length_string_is_not_the_greeting() {
    let a = start(None);
    let mut c = connect(a);
    // 0x80 0x00 is a legal but non-minimal encoding of length 0: the string is empty,
    // which is not the greeting, so it must be kicked.
    c.write_all(&pkt(1, &[0x80, 0x00])).unwrap();
    assert_eq!(recv(&mut c).map(|p| p.0), Some(2), "an empty greeting is not the greeting");
    assert_still_served(a);
}

#[test]
fn a_password_makes_the_server_ask_instead_of_admitting() {
    let a = start(Some("hunter2".to_string()));
    let mut c = connect(a);
    c.write_all(&hello()).unwrap();
    let (id, body) = recv(&mut c).expect("the server must respond to a correct greeting");
    assert_ne!(id, 3, "a password must not admit the client");
    assert_eq!(id, 37, "MessageID::RequestPassword");
    assert!(body.is_empty());
}

#[test]
fn trailing_bytes_after_the_greeting_do_not_prevent_being_served() {
    let a = start(None);
    let mut c = connect(a);
    let mut body = str7(CONNECT_STRING);
    body.extend_from_slice(b"trailing junk the reader never asked for");
    c.write_all(&pkt(1, &body)).unwrap();
    // `read_string` consumes exactly its claim; the rest of the body is not its business,
    // which is what the C# BinaryReader does too. Being served is correct here.
    assert_eq!(recv(&mut c).map(|p| p.0), Some(3), "served, as the C# would serve it");
}

#[test]
fn every_id_before_hello_is_booted_with_the_state_key() {
    // MessageBuffer.cs:169 boots anything but Hello while State == 0, and that guard is
    // what stops a client skipping the handshake. 16 and 38 are in the EARLY_ALLOWED set
    // that the id > 12 guard tolerates, so they exercise the OTHER guard and are still
    // booted: State is 0 either way.
    let a = start(None);
    let mut expected = vec![2u8, 19]; // Mode.LocalizationKey, then the key's length
    expected.extend_from_slice(b"LegacyMultiplayer.2");
    expected.push(0); // no substitutions
    for id in [2u8, 3, 4, 5, 7, 12, 13, 16, 38] {
        let mut c = connect(a);
        c.write_all(&pkt(id, &[])).unwrap();
        match recv(&mut c) {
            Some((kick, body)) => {
                assert_eq!(kick, 2, "id {id} must be kicked, not served");
                assert_eq!(body, expected, "id {id} must carry the state key");
            }
            None => panic!("id {id} was met with silence instead of a kick"),
        }
    }
    assert_still_served(a);
}

// -- volume ----------------------------------------------------------------
#[test]
fn many_connections_in_a_row_are_all_served() {
    let a = start(None);
    let mut served = 0;
    for _ in 0..40 {
        let mut c = connect(a);
        c.write_all(&hello()).unwrap();
        if recv(&mut c).map(|p| p.0) == Some(3) {
            served += 1;
        }
    }
    assert_eq!(served, 40, "every well-formed client should be served");
    assert_still_served(a);
}
