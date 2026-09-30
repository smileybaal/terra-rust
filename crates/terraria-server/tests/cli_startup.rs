//! Startup and CLI handling, driven through the REAL binary.
//!
//! `boot::run` serves once it starts listening, so it cannot be called in-process:
//! these tests start the binary, read the port it says it bound, and talk to it. Every
//! socket has a timeout (see `common`), so a silent server fails in seconds rather than
//! hanging the suite.
//!
//! The C# these mirror is `LaunchInitializer.LoadSharedParameters` /
//! `LoadServerParameters` (`re/exports/ilspy_server/Terraria/Initializers/LaunchInitializer.cs`):
//!
//!     TryParameter("-p", "-port")        // -p is consulted FIRST, so -p wins
//!     TryParameter("-pass", "-password") // -pass is consulted FIRST, so -pass wins
//!
//! `TryParameter` returns the value of the FIRST key it finds, so when both `-p` and
//! `-port` are given it is `-p` that decides, and when both `-pass` and `-password`
//! are given it is `-pass` that decides.

mod common;

use common::*;

/// `-port 0` asks the OS for an ephemeral port. The server must PRINT the port it
/// actually bound, not the `0` it was asked for, or the operator has no way to know
/// where to connect. This reads the printed port, connects to it, and completes the
/// handshake, which is the only proof the printed port is the real one.
#[test]
fn port_zero_reports_the_real_bound_port() {
    let server = start_server(&["-port", "0"]);
    assert_ne!(
        server.port, 0,
        "`-port 0` bound an ephemeral port but reported `0`: the operator cannot learn the port"
    );

    let mut c = connect(server.port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    let (id, _body) = read_packet(&mut c).expect("no answer on the printed port");
    assert_eq!(
        id,
        MessageID::PlayerInfo,
        "a client on the printed port should be answered, not kicked (got id {id})"
    );
}

/// The C# consults `-p` before `-port` (`TryParameter("-p", "-port")`), so `-p` wins
/// when both are present. Before the fix `boot.rs` read `-port` first, so this
/// listened on the `-port` value instead.
#[test]
fn the_short_p_flag_wins_over_port() {
    let p_port = free_port();
    let server = start_server(&["-p", &p_port.to_string(), "-port", "0"]);
    assert_eq!(
        server.port, p_port,
        "both -p and -port given: the C# uses -p first, so -p must win"
    );
    // and it is really listening there
    let mut c = connect(p_port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    let (id, _) = read_packet(&mut c).unwrap();
    assert_eq!(id, MessageID::PlayerInfo);
}

/// The C# consults `-pass` before `-password` (`TryParameter("-pass", "-password")`).
/// With `-pass ""` present but empty, the C# sets an EMPTY password, which
/// `MessageBuffer.cs:205` (`string.IsNullOrEmpty(Netplay.ServerPassword)`) treats as
/// "no password": the client is served without being asked. Before the fix `boot.rs`
/// read `-password` first, so the non-empty `-password` won and the client was asked
/// for a password the C# would not have required.
#[test]
fn the_short_pass_flag_wins_over_password() {
    let port = free_port();
    let server = start_server(&[
        "-port",
        &port.to_string(),
        "-pass",
        "",
        "-password",
        "secret",
    ]);
    assert_eq!(server.port, port, "the server should listen on the requested port");

    let mut c = connect(port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    let (id, _) = read_packet(&mut c).expect("no answer after Hello");
    assert_eq!(
        id,
        MessageID::PlayerInfo,
        "`-pass` (empty) wins over `-password` in the C#, so no password should be asked; \
         a RequestPassword (id {}) means `-password` was preferred instead",
        MessageID::RequestPassword
    );
}

/// An empty password means no password (`MessageBuffer.cs:205`), and a non-empty one
/// is requested. This pins both halves of the meaning of `-password`.
#[test]
fn an_empty_password_means_no_password_and_a_set_one_is_asked_for() {
    let empty_port = free_port();
    let s1 = start_server(&["-port", &empty_port.to_string(), "-password", ""]);
    let mut c = connect(s1.port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    assert_eq!(
        read_packet(&mut c).unwrap().0,
        MessageID::PlayerInfo,
        "an empty -password must mean no password"
    );

    let set_port = free_port();
    let s2 = start_server(&["-port", &set_port.to_string(), "-password", "hunter2"]);
    let mut c = connect(s2.port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    assert_eq!(
        read_packet(&mut c).unwrap().0,
        MessageID::RequestPassword,
        "a non-empty -password must be requested"
    );
}

/// A password made of several tokens is joined with spaces (as `ParseArguements`
/// does), and is still a password.
#[test]
fn a_multi_token_password_is_joined_and_still_a_password() {
    let port = free_port();
    let server = start_server(&["-port", &port.to_string(), "-password", "two", "words"]);
    let mut c = connect(server.port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    assert_eq!(read_packet(&mut c).unwrap().0, MessageID::RequestPassword);
}

/// A port that cannot be used must be refused with exit 1 and a sentence, never a
/// panic (a Rust panic is exit 101).
#[test]
fn unusable_ports_are_refused_with_exit_one_and_no_panic() {
    for args in [
        &["-port", "65536"][..],   // too large for u16
        &["-port", "-1"][..],      // negative; `-1` is itself read as a flag
        &["-port", "not-a-port"][..],
        &["-port", ""][..],        // an empty value is not a port
        &["-port", "0x1e61"][..],  // hex is not what int.TryParse reads
    ] {
        let (code, _out, err) = run_to_exit(args);
        assert_eq!(
            code,
            Some(1),
            "{args:?} should refuse to start with exit 1 (a panic is 101); stderr:\n{err}"
        );
        assert!(
            err.contains("not a port number"),
            "{args:?} should say why on stderr, got:\n{err}"
        );
    }
}

/// A port already in use is the C# "Tried to run two servers on the same PC" path.
/// Tested for real by holding the socket before the server starts.
#[test]
fn a_port_already_in_use_is_refused_with_exit_one() {
    let held = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("hold a port");
    let port = held.local_addr().unwrap().port();
    let port = port.to_string();
    let (code, _out, err) = run_to_exit(&["-port", &port]);
    drop(held);
    assert_eq!(code, Some(1), "an in-use port must exit 1, not panic; stderr:\n{err}");
    assert!(
        err.contains("Tried to run two servers on the same PC"),
        "the C# sentence for this path should be printed, got:\n{err}"
    );
}

/// Whitespace around a port number is accepted, as `int.TryParse` accepts it.
#[test]
fn a_padded_port_number_is_accepted() {
    let port = free_port();
    let padded = format!("  {port}  ");
    let server = start_server(&["-port", &padded]);
    assert_eq!(server.port, port);
    let mut c = connect(port);
    let (id, body) = hello_packet(&connect_string());
    write_packet(&mut c, id, &body).unwrap();
    assert_eq!(read_packet(&mut c).unwrap().0, MessageID::PlayerInfo);
}

/// A repeated `-port` is recorded and reported (dec020), and the first value wins.
#[test]
fn a_repeated_port_flag_is_reported_and_the_first_wins() {
    let first = free_port();
    let second = free_port();
    let server = start_server(&["-port", &first.to_string(), "-port", &second.to_string()]);
    assert_eq!(
        server.port, first,
        "a repeated flag keeps the first value (dec020)"
    );
}
