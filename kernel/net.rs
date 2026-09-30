//! The serving half of the kernel: the first piece of the port's BEHAVIOUR.
//!
//! Until this module existed the binary printed its shape and exited, because every
//! projected body is a stub (D4: the sheets carry the server's shape, not its
//! behaviour). What is implemented here is the C# server's ENTRY PATH, taken from the
//! decompiled source in `re/exports/ilspy_server` and cited per item:
//!
//!   accept and log        `Netplay.OnConnectionAccepted`   (Netplay.cs:221)
//!   server-is-full kick   `Netplay.KickClient`             (Netplay.cs:237)
//!   packet framing        `NetMessage.SendData`            (NetMessage.cs:118-142)
//!   the handshake         `MessageBuffer` case 1           (MessageBuffer.cs:181-222)
//!   the state guard       `MessageBuffer.cs:163-173`
//!   the listen port       `LaunchInitializer.LoadSharedParameters` (LaunchInitializer.cs:30)
//!   the kick texts        `Lang.mp[1..4]` = `LegacyMultiplayer.1..4`
//!                         (Terraria.Localization.Content.en-US.Legacy.json)
//!
//! Message ids come from the projection (`port::terraria::id::MessageID`), which reads
//! them from the sheets; they are not retyped here. The protocol version, 326, is the
//! one literal in this file that has no sheet row: it appears in the C# as
//! `writer.Write("Terraria" + 326)` (NetMessage.cs:142) and as the comparison in
//! `reader.ReadString() == "Terraria" + 326` (MessageBuffer.cs:203).
//!
//! WHAT IS NOT IMPLEMENTED, and is therefore named rather than hidden: everything
//! after the handshake. There is no world loading, no tile sending and no game loop,
//! so a client that gets past `Hello` is kicked with a LITERAL message that says this
//! port has not implemented that message yet. That is a DELIBERATE divergence from the
//! C#, where such a client would be served, and it is reported out loud for the same
//! reason `boot` used to refuse to print a banner it could not back up.

use std::io::{self, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::port;

/// The protocol version, from the projection: `Main.curRelease` is a ROW in
/// `sheets/re/server/fields.tsv` (`terraria.main.currelease`, value 326), and the port
/// emits it as `port::terraria::Main::curRelease`. It is not typed here, so the same
/// value is visible in the sheet and in this binary, which is the point of the
/// projection (D1). The C# uses the literal 326 at `NetMessage.cs:142`.
pub const PROTOCOL_VERSION: i32 = port::terraria::Main::curRelease;

/// The greeting a client must send in `Hello` to be accepted.
///
/// Rust cannot build a `&str` const from another const, so this is the one value in
/// the module that is written out. The test below binds it to the projected
/// `curRelease`, so it cannot drift from the sheet row.
pub const CONNECT_STRING: &str = "Terraria326";

/// `Lang.mp` keys. The client resolves them from its own localization, which is why a
/// key travels instead of a sentence.
mod mp {
    pub const INVALID_STATE: &str = "LegacyMultiplayer.2";
    pub const VERSION_MISMATCH: &str = "LegacyMultiplayer.4";
}

/// `Netplay.cs:237`: the key the C# sends when there is no free client slot.
const SERVER_IS_FULL: &str = "CLI.ServerIsFull";

/// `MessageBuffer.cs:165`: ids above 12 that the C# tolerates while a client is still
/// below `State == 10`. Named from the projection rather than typed as numbers, so each
/// one is a sheet value and the list cannot drift from the id table.
const EARLY_ALLOWED: [u8; 8] = [
    port::terraria::id::MessageID::SocialHandshake, // 93
    port::terraria::id::MessageID::PlayerLifeMana,  // 16
    port::terraria::id::MessageID::Unknown42,       // 42
    port::terraria::id::MessageID::PlayerBuffs,     // 50
    port::terraria::id::MessageID::SendPassword,    // 38
    port::terraria::id::MessageID::Unknown68,       // 68
    port::terraria::id::MessageID::SyncLoadout,     // 147
    port::terraria::id::MessageID::HostToken,       // 161
];

/// Live connections, for the `MaxConnections` limit (`Netplay.InitializeServer`).
static CONNECTED: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// the wire: `BinaryWriter` strings and the 2-byte length prefix
// ---------------------------------------------------------------------------

/// `BinaryWriter.Write(string)`: a 7-bit encoded byte length, then UTF-8.
///
/// Get this wrong and nothing else in the protocol can be right, so it is written out
/// rather than delegated to a crate: the format is a contract with a C# reader.
fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
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

/// The write half of `read_string`.
fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
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

/// `NetworkText.Serialize` with `Mode.LocalizationKey` (`NetworkText.cs`): the mode
/// byte, the key, then a zero substitution count. A kick that travels as a key lets
/// the client localize it, which is why the C# never sends the sentence.
fn write_kick_key<W: Write>(w: &mut W, key: &str) -> io::Result<()> {
    w.write_all(&[2])?; // Mode.LocalizationKey
    write_string(w, key)?;
    w.write_all(&[0])?; // no substitutions
    Ok(())
}

/// `NetworkText.Serialize` with `Mode.Literal`. Used only for this port's own
/// not-implemented notice, which is not a C# string and must not pretend to be one.
fn write_kick_literal<W: Write>(w: &mut W, text: &str) -> io::Result<()> {
    w.write_all(&[0])?; // Mode.Literal
    write_string(w, text)?;
    Ok(())
}

/// `NetMessage.SendData`: a 2-byte little-endian length covering the id byte plus the
/// body, then the id, then the body. The length EXCLUDES the prefix itself.
fn write_packet<W: Write>(w: &mut W, id: u8, body: &[u8]) -> io::Result<()> {
    let len = 1 + body.len();
    if len > u16::MAX as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "packet is too large for its 2-byte length"));
    }
    w.write_all(&(len as u16).to_le_bytes())?;
    w.write_all(&[id])?;
    w.write_all(body)?;
    w.flush()
}

/// The read half of `write_packet`.
fn read_packet<R: Read>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)?;
    let len = u16::from_le_bytes(len) as usize;
    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a packet must carry at least its id"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let id = buf[0];
    buf.remove(0);
    Ok((id, buf))
}

// ---------------------------------------------------------------------------
// the server
// ---------------------------------------------------------------------------

/// Bind the listen socket. Separate from `serve` so a test can take port 0 and learn
/// the real port, rather than hard-coding 7777 and racing the machine.
pub fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
}

/// Accept forever, one thread per client.
///
/// The C# runs one loop over 256 async slots (`Netplay.ServerLoop`, Netplay.cs:331)
/// instead of a thread per client. That is a scheduling difference, not a protocol
/// one: nothing on the wire depends on it, and at this stage of the port a thread per
/// connection is the smaller thing to be right about.
pub fn serve(listener: TcpListener, password: Option<String>) -> io::Result<()> {
    println!("Server started"); // CLI.ServerStarted
    let max = port::terraria::Netplay::MaxConnections as usize;
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept failed: {e}");
                continue;
            }
        };
        let addr = stream
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        println!("{addr} is connecting..."); // Net.ClientConnecting

        if CONNECTED.load(Ordering::SeqCst) >= max {
            let mut s = stream;
            let mut body = Vec::new();
            let _ = write_kick_key(&mut body, SERVER_IS_FULL);
            let _ = write_packet(&mut s, port::terraria::id::MessageID::Kick, &body);
            println!("{addr} was booted: ServerIsFull");
            continue;
        }
        CONNECTED.fetch_add(1, Ordering::SeqCst);
        let pw = password.clone();
        thread::spawn(move || {
            let r = client_loop(stream, pw);
            CONNECTED.fetch_sub(1, Ordering::SeqCst);
            if let Err(e) = r {
                eprintln!("{addr}: {e}");
            }
        });
    }
    Ok(())
}

/// One connection, from accept to disconnect.
///
/// The state machine is `MessageBuffer.HandleMessage` as far as the handshake, and the
/// two guards in front of it are copied from `MessageBuffer.cs:163-173` rather than
/// invented, because they are what stops a client skipping the handshake.
fn client_loop(mut stream: TcpStream, password: Option<String>) -> io::Result<()> {
    let mut state: i32 = 0;
    loop {
        let (id, body) = match read_packet(&mut stream) {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()), // client left
            Err(e) => return Err(e),
        };

        // MessageBuffer.cs:169: before the handshake, only Hello is legal.
        if state == 0 && id != port::terraria::id::MessageID::Hello {
            return boot(&mut stream, mp::INVALID_STATE, "not Hello while State == 0");
        }
        // MessageBuffer.cs:165: ids above 12 are refused until the client is settled.
        if state < 10 && id > 12 && !EARLY_ALLOWED.contains(&id) {
            return boot(&mut stream, mp::INVALID_STATE, "id above 12 before State >= 10");
        }

        if id == port::terraria::id::MessageID::Hello {
            if state != 0 {
                continue; // MessageBuffer.cs:199
            }
            let greeting = read_string(&mut Cursor::new(&body[..]))?;
            if greeting != CONNECT_STRING {
                return boot(&mut stream, mp::VERSION_MISMATCH, "wrong greeting");
            }
            match &password {
                None => {
                    state = 1;
                    send_player_info(&mut stream)?;
                }
                Some(_) => {
                    state = -1;
                    send_password_request(&mut stream)?;
                }
            }
            continue;
        }

        // Everything past the handshake is the port's remaining work.
        return not_implemented(&mut stream, id);
    }
}

/// `MessageBuffer.cs:207-208`: `State = 1` then `TrySendData(3, whoAmI)`, which per
/// `SendData` case 3 writes the slot and a false bool. The slot is 0 here because the
/// C# assigns slots from `FindNextOpenClientSlot` and this port accepts the first
/// client into 0; a second concurrent client is a known gap, reported rather than
/// papered over.
fn send_player_info(stream: &mut TcpStream) -> io::Result<()> {
    write_packet(stream, port::terraria::id::MessageID::PlayerInfo, &[0u8, 0u8])
}

/// `MessageBuffer.cs:212-213`: `State = -1` then `TrySendData(37, whoAmI)`, which is
/// `MessageID.RequestPassword`. `SendData` has no `case 37`, so nothing follows the id:
/// the request carries no body.
fn send_password_request(stream: &mut TcpStream) -> io::Result<()> {
    write_packet(stream, port::terraria::id::MessageID::RequestPassword, &[])
}

/// `NetMessage.TrySendData(2, ...)`: kick this client and close. The text travels as a
/// localization key, exactly where the C# passes `Lang.mp[n].ToNetworkText()`.
fn boot(stream: &mut TcpStream, key: &str, why: &str) -> io::Result<()> {
    let mut body = Vec::new();
    write_kick_key(&mut body, key)?;
    write_packet(stream, port::terraria::id::MessageID::Kick, &body)?;
    println!("kicked ({key}) because {why}");
    Ok(())
}

/// This port's own notice, and the only message it sends that the C# would not: the
/// message is named so a client is told what is missing instead of hanging.
fn not_implemented(stream: &mut TcpStream, id: u8) -> io::Result<()> {
    let mut body = Vec::new();
    write_kick_literal(
        &mut body,
        &format!("terra-rust: message {id} is not implemented yet; only the handshake is"),
    )?;
    write_packet(stream, port::terraria::id::MessageID::Kick, &body)?;
    println!("kicked (literal) because message {id} is not implemented");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The greeting is built from the version, so they cannot drift apart. The version
    /// is the sheet's `terraria.main.currelease`, so this test ties the one literal in
    /// the module to the row it came from.
    #[test]
    fn the_greeting_matches_the_projected_protocol_version() {
        assert_eq!(PROTOCOL_VERSION, 326, "the sheet row terraria.main.currelease");
        assert_eq!(CONNECT_STRING, format!("Terraria{PROTOCOL_VERSION}"));
    }

    /// The 2-byte prefix excludes itself and covers the id, and a body long enough to
    /// need a two-byte 7-bit length round-trips. This is the framing the C# writes with
    /// `writer.BaseStream.Position += 2L` and then back-patches.
    #[test]
    fn framing_and_dotnet_strings_round_trip() {
        let long = "T".repeat(300); // forces a 2-byte 7-bit length
        let mut body = Vec::new();
        write_string(&mut body, &long).unwrap();
        assert_eq!(read_string(&mut Cursor::new(&body[..])).unwrap(), long);

        let mut wire = Vec::new();
        write_packet(&mut wire, 42, &body).unwrap();
        assert_eq!(wire[0..2].to_vec(), (1 + body.len() as u16).to_le_bytes().to_vec());
        assert_eq!(wire[2], 42);
        let (id, got) = read_packet(&mut Cursor::new(&wire[..])).unwrap();
        assert_eq!(id, 42);
        assert_eq!(got, body);
    }

    /// A real socket: `Hello` with the right greeting is answered with `PlayerInfo`.
    /// The C# is `TrySendData(3, whoAmI)` writing the slot and a false bool.
    #[test]
    fn hello_is_answered_with_player_info() {
        let addr = start(None);
        let mut c = connect(addr);
        send_hello(&mut c, CONNECT_STRING);
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::PlayerInfo);
        assert_eq!(body, vec![0u8, 0u8]);
    }

    /// A wrong greeting is kicked with the localization key the C# sends, so a client
    /// localizes the sentence itself.
    #[test]
    fn a_wrong_greeting_is_kicked_with_the_version_key() {
        let addr = start(None);
        let mut c = connect(addr);
        send_hello(&mut c, "Terraria999");
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), mp::VERSION_MISMATCH);
    }

    /// MessageBuffer.cs:169 again, from the outside: a client that asks for world data
    /// before saying hello is booted, not served.
    #[test]
    fn a_message_before_hello_is_booted() {
        let addr = start(None);
        let mut c = connect(addr);
        write_packet(&mut c, port::terraria::id::MessageID::RequestWorldData, &[]).unwrap();
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), mp::INVALID_STATE);
    }

    /// Past the handshake the port has nothing, and says so with a literal rather than
    /// leaving the client waiting.
    #[test]
    fn an_unimplemented_message_is_kicked_by_name() {
        let addr = start(None);
        let mut c = connect(addr);
        send_hello(&mut c, CONNECT_STRING);
        let _ = read_packet(&mut c).unwrap(); // PlayerInfo
        write_packet(&mut c, port::terraria::id::MessageID::SyncPlayer, &[]).unwrap();
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        assert_eq!(read_mode_of(&body), 0, "a literal, not a key: it is not a C# string");
        let text = read_string(&mut Crsr::new(&body[1..])).unwrap();
        assert!(text.contains("not implemented"), "got {text:?}");
    }

    /// A password set means the C# goes to `State = -1` and asks, and this port does the
    /// same rather than quietly letting the client in.
    #[test]
    fn a_password_is_requested_before_player_info() {
        let addr = start(Some("hunter2".to_string()));
        let mut c = connect(addr);
        send_hello(&mut c, CONNECT_STRING);
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::RequestPassword);
        assert!(body.is_empty(), "SendData has no case 37, so nothing follows the id");
    }

    // -- helpers ------------------------------------------------------------

    use std::net::SocketAddr;
    use std::time::Duration;

    /// A `Cursor` alias that reads the way the tests talk.
    type Crsr<'a> = Cursor<&'a [u8]>;

    /// Connect with timeouts, so a missing response FAILS in seconds rather than
    /// hanging the suite. A test that can hang proves nothing and blocks every other,
    /// which is how this one behaved before the timeouts were added.
    fn connect(addr: SocketAddr) -> TcpStream {
        let s = TcpStream::connect(addr).unwrap();
        let d = Some(Duration::from_secs(5));
        s.set_read_timeout(d).unwrap();
        s.set_write_timeout(d).unwrap();
        s
    }

    fn start(password: Option<String>) -> SocketAddr {
        let l = bind(0).unwrap();
        // 0.0.0.0 is a listen address, not a destination: dialling it is
        // WSAEADDRNOTAVAIL (10049) on Windows. Take the port and dial loopback.
        let port = l.local_addr().unwrap().port();
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        thread::spawn(move || {
            let _ = serve(l, password);
        });
        addr
    }

    fn send_hello(c: &mut TcpStream, greeting: &str) {
        let mut body = Vec::new();
        write_string(&mut body, greeting).unwrap();
        write_packet(c, port::terraria::id::MessageID::Hello, &body).unwrap();
    }

    fn read_mode_of(body: &[u8]) -> u8 {
        body[0]
    }
}
