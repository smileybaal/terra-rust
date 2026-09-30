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
///
/// These are the one class of value in this module the sheets do NOT carry: searching
/// every sheet for `LegacyMultiplayer` and for the CLI/Net keys finds only an unrelated
/// UI class, and the server has no `strings` sheet at all (dec015). So they are cited
/// from the localization table the game ships, and which key belongs where is cited
/// from the C# line that passes it.
mod mp {
    /// `Lang.mp[n]`, the table the C# indexes for its multiplayer kick texts
    /// (`MessageBuffer.cs:160` passes `mp[1]`, `:167` and `:171` pass `mp[2]`, `:218`
    /// passes `mp[4]`).
    ///
    /// These were three hand-typed strings until the localization sheet grew the
    /// `LegacyMultiplayer` section; now they are rows, and a wrong index is a preflight
    /// problem rather than a typo nobody can see. `Lang.mp` is 1-BASED, which is why the
    /// ids start at `legacymultiplayer.1`.
    fn row(index: u32) -> &'static str {
        let id = format!("legacymultiplayer.{index}");
        crate::sheets::localization::by_id(&id)
            .unwrap_or_else(|| panic!("{id} is a row of re/server/localization"))
            .key
    }

    /// "Incorrect password".
    pub fn incorrect_password() -> &'static str {
        row(1)
    }

    /// "Invalid operation at this state."
    pub fn invalid_state() -> &'static str {
        row(2)
    }

    /// "You are not using the same version as this server."
    pub fn version_mismatch() -> &'static str {
        row(4)
    }
}

/// `Netplay.cs:233`: the key the C# sends when there is no free client slot.
///
/// Read from the BOOK rather than typed. The row is `cli.serverisfull`, and it is
/// carried by `sheets::localization` because the CLI section is the first section
/// of the localization relation. This is the point of that sheet: the string is
/// now a row like every other value here, so a wrong key is a preflight problem
/// rather than a hand-typing one, and the test in `lib` pins the lookup so it
/// cannot quietly degrade into an empty kick.
fn server_is_full() -> &'static str {
    crate::sheets::localization::by_id("cli.serverisfull")
        .expect("cli.serverisfull is a row of re/server/localization")
        .key
}

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

/// The most a legitimate string can be, and the initial allocation cap.
///
/// Two bounds, both derived rather than invented. A frame's length is a `u16`
/// (`write_packet`), so no body and therefore no string inside one can exceed 65535
/// bytes: a larger claim is not a big string, it is a lie. And the capacity the buffer
/// starts at is `Netplay.NetBufferSize`, which is the buffer the C# gives each client
/// (`Clients[i].ReadBuffer = new byte[1024]`, Netplay.cs:305) and a ROW in
/// `sheets/re/server/fields.tsv` (`terraria.netplay.netbuffersize`).
const MAX_STRING: usize = u16::MAX as usize;
const STRING_CHUNK: usize = port::terraria::Netplay::NetBufferSize as usize;

/// The 7-bit encoded length `BinaryWriter.Write(string)` puts in front of the bytes.
fn read_7bit_len<R: Read>(r: &mut R) -> io::Result<usize> {
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
    Ok(len)
}

/// `BinaryWriter.Write(string)`: a 7-bit encoded byte length, then UTF-8.
///
/// The claimed length is NOT trusted to size the buffer. A hostile client can send five
/// bytes that claim ~34 GiB, and allocating that before one payload byte arrives is a
/// resource vector the C# does not have: its `BinaryReader` reads from a buffer it
/// already holds and grows its own string as it goes. So the claim is checked against
/// the frame bound and the bytes are read in bounded chunks.
pub(crate) fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    let len = read_7bit_len(r)?;
    if len > MAX_STRING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a string of {len} bytes cannot be inside a {MAX_STRING}-byte frame"),
        ));
    }
    let mut buf = Vec::with_capacity(len.min(STRING_CHUNK));
    let mut chunk = [0u8; 4096];
    let mut remaining = len;
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        r.read_exact(&mut chunk[..take])?;
        buf.extend_from_slice(&chunk[..take]);
        remaining -= take;
    }
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The write half of `read_string`.
pub(crate) fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
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
    // The ordinal is the sheet's: `terraria.localization.networktext.mode`
    // (LocalizationKey = 2), projected as `NetworkText::Mode`. Not typed here, so
    // the mode byte and the enum cannot drift (D1/D6).
    w.write_all(&[port::terraria::localization::networktext::Mode::LocalizationKey as u8])?;
    write_string(w, key)?;
    w.write_all(&[0])?; // no substitutions
    Ok(())
}

/// `NetworkText.Serialize` with `Mode.Literal`. Used only for this port's own
/// not-implemented notice, which is not a C# string and must not pretend to be one.
fn write_kick_literal<W: Write>(w: &mut W, text: &str) -> io::Result<()> {
    w.write_all(&[port::terraria::localization::networktext::Mode::Literal as u8])?; // Mode.Literal
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
            let _ = write_kick_key(&mut body, server_is_full());
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
    // The name from `SyncPlayer` (4). The C# stores it on the client's slot
    // (`Netplay.Clients[whoAmI].Name`) and uses it for the duplicate-name check; there is
    // no client table here yet, so it is held for this connection and logged.
    let mut name: Option<String> = None;
    loop {
        let (id, body) = match read_packet(&mut stream) {
            Ok(p) => p,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()), // client left
            Err(e) => return Err(e),
        };

        // MessageBuffer.cs:158-162, and this one runs BEFORE the two below it: once a
        // password has been asked for, anything other than `SendPassword` is answered
        // with the "Incorrect password" kick. Without it a client could sit at State -1
        // and be told nothing, and a repeated Hello would be ignored rather than refused.
        if state == -1 && id != port::terraria::id::MessageID::SendPassword {
            return boot(&mut stream, mp::incorrect_password(), "not SendPassword while State == -1");
        }

        // MessageBuffer.cs:169: before the handshake, only Hello is legal.
        if state == 0 && id != port::terraria::id::MessageID::Hello {
            return boot(&mut stream, mp::invalid_state(), "not Hello while State == 0");
        }
        // MessageBuffer.cs:165: ids above 12 are refused until the client is settled.
        if state < 10 && id > 12 && !EARLY_ALLOWED.contains(&id) {
            return boot(&mut stream, mp::invalid_state(), "id above 12 before State >= 10");
        }

        if id == port::terraria::id::MessageID::Hello {
            if state != 0 {
                continue; // MessageBuffer.cs:199
            }
            let greeting = read_string(&mut Cursor::new(&body[..]))?;
            if greeting != CONNECT_STRING {
                return boot(&mut stream, mp::version_mismatch(), "wrong greeting");
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

        // `MessageBuffer.cs:284-400`: `SyncPlayer` (4), the first thing a client sends
        // after `PlayerInfo`. The reader does the sanitising, and its refusals carry the
        // book's own keys, so a kick names the row the C# names.
        if id == port::terraria::id::MessageID::SyncPlayer {
            match crate::player::read_sync_player(&body) {
                Ok(p) => {
                    // The slot on the wire is DISCARDED, as the C# discards it
                    // (`if (Main.netMode == 2) num188 = whoAmI;`): a client does not get to
                    // say which player it is. There is one connection per client here, so
                    // the connection's own identity is the only one it can have.
                    println!(
                        "sync player: {:?} (slot {} on the wire, ignored - one connection per client)",
                        p.name, p.slot
                    );
                    name = Some(p.name);
                }
                Err(e) => {
                    return match e.key() {
                        Some(key) => boot(&mut stream, key, "SyncPlayer was refused"),
                        // `TooShort` has no key: the C# has no message for a body that ends
                        // early, so it is this port's own notice rather than a fake key.
                        None => not_implemented(&mut stream, id),
                    };
                }
            }
            continue;
        }

        // `MessageBuffer.cs:1095`: `PlayerLifeMana` (16), the second thing the client
        // sends. The rules are the ones `player::sanitise_life` already reproduces.
        if id == port::terraria::id::MessageID::PlayerLifeMana {
            let mut r = Cursor::new(&body[..]);
            let mut two = [0u8; 2];
            if r.read_exact(&mut two).is_ok() {
                let life = i16::from_le_bytes(two);
                if r.read_exact(&mut two).is_ok() {
                    let max = i16::from_le_bytes(two);
                    let lm = crate::player::sanitise_life(life, max);
                    println!(
                        "player life: {} of {} (dead: {})",
                        lm.stat_life, lm.stat_life_max, lm.dead
                    );
                    continue;
                }
            }
            return not_implemented(&mut stream, id);
        }

        // The password path is NOT written: the C# compares the string against
        // `Netplay.ServerPassword` here (`MessageBuffer.cs:2218`). Accepting it without
        // that comparison would let anyone in, so it is refused by name instead. This is
        // deliberately checked BEFORE the early-burst acceptance below, because
        // `SendPassword` (38) is itself in that list.
        if id == port::terraria::id::MessageID::SendPassword {
            return not_implemented(&mut stream, id);
        }

        // The rest of the client's opening burst (`MessageBuffer.cs:248-253`): ids the C#
        // handles and this port does not yet. They are ACCEPTED so the conversation
        // continues to the message that actually blocks the load, and the log says they
        // were not applied - accepting them silently would look like they worked.
        if EARLY_ALLOWED.contains(&id) {
            println!("accepted message {id} (early player data; not applied yet)");
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

    /// The kick's mode byte is `NetworkText.Mode`, which is a sheet row
    /// (`terraria.localization.networktext.mode`: Literal = 0, LocalizationKey = 2)
    /// projected as `port::terraria::localization::networktext::Mode`. The writers take
    /// the ordinal from that projection instead of retyping 0 and 2, so a sheet that
    /// moved the ordinal moves the wire byte with it (D1/D6). The two agree today,
    /// which is why this is a coupling check and not a difference.
    #[test]
    fn kick_modes_come_from_the_projected_networktext_mode() {
        use port::terraria::localization::networktext::Mode;
        assert_eq!(Mode::Literal as u8, 0, "the sheet row networktext.mode.literal");
        assert_eq!(Mode::LocalizationKey as u8, 2, "the sheet row networktext.mode.localizationkey");
        let mut key = Vec::new();
        write_kick_key(&mut key, "K").unwrap();
        assert_eq!(key[0], Mode::LocalizationKey as u8);
        let mut lit = Vec::new();
        write_kick_literal(&mut lit, "L").unwrap();
        assert_eq!(lit[0], Mode::Literal as u8);
    }

    /// A claimed length that cannot be legitimate is refused rather than believed.
    ///
    /// The frame's own `u16` length is the bound, so a five-byte claim of ~34 GiB is a
    /// lie, and the old code answered it by asking the allocator for 34 GiB before a
    /// single payload byte arrived. Measured against the running binary the virtual size
    /// did not move, because the allocation is freed as soon as the read fails; the
    /// waste is real all the same, and this is the test that pins the refusal.
    #[test]
    fn an_impossible_string_length_is_refused_not_allocated() {
        // 7-bit 2^28 followed by one byte of payload: what a hostile client sends.
        let claimed_256_mib = [0x80u8, 0x80, 0x80, 0x80, 0x01, b'T'];
        let err = read_string(&mut Cursor::new(&claimed_256_mib[..])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("cannot be inside"), "got {err}");

        // 7-bit ~34 GiB, no payload at all.
        let claimed_34_gib = [0xFFu8, 0xFF, 0xFF, 0xFF, 0x7F];
        let err = read_string(&mut Cursor::new(&claimed_34_gib[..])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        // The largest claim that could be real is still read, so the bound does not
        // reject a legitimate value: a 65535-byte string inside a 65535-byte frame.
        let mut wire = Vec::new();
        write_string(&mut wire, &"x".repeat(MAX_STRING)).unwrap();
        assert_eq!(read_string(&mut Cursor::new(&wire[..])).unwrap().len(), MAX_STRING);
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
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), mp::version_mismatch());
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
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), mp::invalid_state());
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
    /// same rather than quietly letting the client in. It also means MessageBuffer.cs:158
    /// now applies: anything but `SendPassword` is refused with the password key.
    #[test]
    fn a_password_is_requested_and_then_anything_else_is_refused() {
        let addr = start(Some("hunter2".to_string()));
        let mut c = connect(addr);
        send_hello(&mut c, CONNECT_STRING);
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::RequestPassword);
        assert!(body.is_empty(), "SendData has no case 37, so nothing follows the id");

        // A second Hello while State == -1 is not ignored: it is the "Incorrect
        // password" kick, which is what the C# does at MessageBuffer.cs:160.
        send_hello(&mut c, CONNECT_STRING);
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), mp::incorrect_password());
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

    /// The exact `SyncPlayer` (4) body this port's writer produces, so a test never
    /// rejects a message for being malformed when it means to test something else.
    fn sync_player_body(name: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&[0, 11, 2]);
        b.extend_from_slice(&0.5f32.to_le_bytes());
        b.push(3);
        write_string(&mut b, name).unwrap();
        b.push(0); // hairDye is ONE byte, not a string
        b.extend_from_slice(&0u16.to_le_bytes());
        b.push(0); // hideMisc
        for _ in 0..7 {
            b.extend_from_slice(&[1, 2, 3]);
        }
        b.extend_from_slice(&[0, 0, 0]); // difficulty, biome torches, crystals
        b
    }

    /// `SyncPlayer` (4) is ACCEPTED, and the proof is that the next message is the one
    /// that gets refused. A test that only checked "no kick arrived" would pass even if
    /// the server had simply stopped reading.
    #[test]
    fn sync_player_is_accepted_and_the_next_message_is_the_one_refused() {
        let addr = start(None);
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        send_hello(&mut c, CONNECT_STRING);
        let (id, _) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::PlayerInfo);

        write_packet(&mut c, port::terraria::id::MessageID::SyncPlayer, &sync_player_body("Probe")).unwrap();
        // Then ask for the world, which IS unimplemented. If message 4 had been refused
        // the connection would already be gone and this read would fail.
        write_packet(&mut c, port::terraria::id::MessageID::RequestWorldData, &[]).unwrap();
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        let text = read_string(&mut Crsr::new(&body[1..])).unwrap();
        assert!(
            text.contains("message 6"),
            "the refusal must be for message 6, not 4: {text:?}"
        );
    }

    /// The client's whole opening burst is accepted, so the conversation reaches the
    /// message that actually blocks the load.
    #[test]
    fn the_early_burst_is_accepted_in_the_clients_order() {
        let addr = start(None);
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        send_hello(&mut c, CONNECT_STRING);
        let _ = read_packet(&mut c).unwrap();

        write_packet(&mut c, port::terraria::id::MessageID::SyncPlayer, &sync_player_body("Burst")).unwrap();
        for id in [68u8, 42, 50, 147] {
            write_packet(&mut c, id, &[0]).unwrap();
        }
        // Life and mana, the one the port already sanitises.
        write_packet(&mut c, port::terraria::id::MessageID::PlayerLifeMana, &[100, 0, 100, 0]).unwrap();
        // None of that may have produced a reply; the world request is what does.
        write_packet(&mut c, port::terraria::id::MessageID::RequestWorldData, &[]).unwrap();
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        let text = read_string(&mut Crsr::new(&body[1..])).unwrap();
        assert!(text.contains("message 6"), "got {text:?}");
    }

    /// A name the book refuses is refused with the book's OWN key, not a literal.
    #[test]
    fn a_too_long_name_is_refused_with_the_books_key() {
        let addr = start(None);
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        send_hello(&mut c, CONNECT_STRING);
        let _ = read_packet(&mut c).unwrap();

        let long = "x".repeat(crate::player::NAME_LEN + 1);
        write_packet(&mut c, port::terraria::id::MessageID::SyncPlayer, &sync_player_body(&long)).unwrap();
        let (id, body) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick);
        assert_eq!(read_string(&mut Crsr::new(&body[1..])).unwrap(), "Net.NameTooLong");
    }

    /// The password path is NOT written, and this is the test that keeps it that way:
    /// `SendPassword` is in the early-burst list, so without the explicit check above it
    /// would be accepted without ever being compared, which would let anyone in.
    #[test]
    fn send_password_is_not_silently_accepted() {
        let addr = start(Some("hunter2".to_string()));
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        send_hello(&mut c, CONNECT_STRING);
        let (id, _) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::RequestPassword);

        let mut body = Vec::new();
        write_string(&mut body, "hunter2").unwrap();
        write_packet(&mut c, port::terraria::id::MessageID::SendPassword, &body).unwrap();
        let (id, _) = read_packet(&mut c).unwrap();
        assert_eq!(id, port::terraria::id::MessageID::Kick, "a password must not be accepted unchecked");
    }
}
