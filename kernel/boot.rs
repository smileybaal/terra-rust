//! The boot sequence, hand-written (D4; listed in `sheets/kernel.tsv`).
//!
//! Mirrors `Terraria.Program.LaunchGame`, in order: parse the launch parameters,
//! resolve the save directory (`-savedirectory` or the platform storage path),
//! raise the thread-pool floor to 8, set up logging, then run.
//!
//! What it COULD NOT mirror was the run itself: `LaunchGame` ends in `RunGame`, which
//! sets `Main.dedServ = true` and calls `main.DedServ()` then `main.Run()`, and those
//! are generated stubs whose bodies are `unimplemented!()`, because the sheets carry
//! the server's SHAPE and not its behaviour.
//!
//! So this reports the shape it found and then hands the machine to `net`, where the
//! behaviour now begins: the listener, the packet framing and the C# handshake. It
//! says plainly how far that goes, because a banner implying more than the port can do
//! is worse than no banner at all.

use crate::args::{self, LaunchParameters};
use crate::port;
use crate::sheets;

/// `Program.LaunchGame` sets the thread-pool floor before anything else runs.
pub const MIN_THREADS: usize = 8;

/// The flag `LaunchGame` reads the save directory from.
pub const SAVE_DIR_FLAG: &str = "-savedirectory";

/// The storage folder `IPathService.GetStoragePath("Terraria")` resolves to.
pub const SAVE_FOLDER: &str = "Terraria";

/// `ReLogic.OS.Windows.PathService.GetStoragePath("Terraria")`.
///
/// C# is `Path.Combine(Environment.GetFolderPath(SpecialFolder.Personal), "My Games",
/// "Terraria")`. `SpecialFolder.Personal` is the user's Documents folder, whose default
/// location is `%USERPROFILE%\Documents`. This is the closest a Rust process can get to
/// that shell API, so it is still an approximation of a redirected Documents folder, but
/// it is the same folder the C# server uses - NOT `%APPDATA%`.
#[allow(dead_code)]
fn windows_save_path(user_profile: Option<&str>) -> Option<String> {
    user_profile
        .filter(|p| !p.is_empty())
        .map(|p| format!("{p}\\Documents\\My Games\\{SAVE_FOLDER}"))
}

/// `ReLogic.OS.OSX.PathService.GetStoragePath("Terraria")`.
///
/// C# treats an unset OR EMPTY `HOME` as absent and returns `"."`.
#[allow(dead_code)]
fn macos_save_path(home: Option<&str>) -> String {
    match home {
        Some(h) if !h.is_empty() => format!("{h}/Library/Application Support/{SAVE_FOLDER}"),
        _ => format!("./{SAVE_FOLDER}"),
    }
}

/// `ReLogic.OS.Linux.PathService.GetStoragePath("Terraria")`.
///
/// C# treats an unset OR EMPTY `XDG_DATA_HOME` as absent (as the XDG spec requires) and
/// falls back to `$HOME/.local/share`, then to `"."` when `HOME` is unset or empty too.
#[allow(dead_code)]
fn unix_save_path(xdg_data_home: Option<&str>, home: Option<&str>) -> String {
    let base = match xdg_data_home {
        Some(x) if !x.is_empty() => x.to_string(),
        _ => match home {
            Some(h) if !h.is_empty() => format!("{h}/.local/share"),
            _ => ".".to_string(),
        },
    };
    format!("{base}/{SAVE_FOLDER}")
}

/// `Platform.Get<IPathService>().GetStoragePath("Terraria")`, per platform.
///
/// The folder name is the argument `Program.LaunchGame` passes, verbatim at
/// `re/exports/ilspy/Terraria/Program.cs:187`. The Windows shape is corroborated twice
/// in the evidence tree: the user-facing text in
/// `re/exports/ilspy/Terraria/UI/FancyErrorPrinter.cs:65,99` names the folder
/// `Documents/My Games/Terraria`, and `sheets/re/client/strings.tsv` records the
/// `GetStoragePath` string found in the binary at `0x00e0d11f`.
///
/// The macOS and Linux shapes and the `IsNullOrEmpty` handling are INFERRED, not
/// decompiled: the export carries `Terraria.Libraries.ReLogic.ReLogic.dll` as a binary
/// with no `.cs` tree, so only the Windows path is evidence-backed. Treat the other two
/// as a stated approximation, and do not cite them as a decompilation.
pub fn default_save_path() -> Option<String> {
    #[cfg(windows)]
    {
        windows_save_path(std::env::var("USERPROFILE").ok().as_deref())
    }
    #[cfg(target_os = "macos")]
    {
        Some(macos_save_path(std::env::var("HOME").ok().as_deref()))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(unix_save_path(
            std::env::var("XDG_DATA_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        ))
    }
}

/// Resolve the save directory the way `LaunchGame` does.
///
/// C# is `LaunchParameters.ContainsKey("-savedirectory") ? LaunchParameters["-savedirectory"]
/// : GetStoragePath("Terraria")`, so the flag's value is used verbatim whenever the flag is
/// present, even when that value is empty or only whitespace.
pub fn save_path(params: &LaunchParameters) -> Option<String> {
    params
        .get(SAVE_DIR_FLAG)
        .map(str::to_string)
        .or_else(default_save_path)
}

/// The facts the port can state about the server, read from the projected sheets.
///
/// Every number here comes from a sheet row, not from a constant typed into this
/// file, which is the point of the projection: the same value is visible in
/// `sheets/re/server/fields.tsv` and in the compiled binary.
pub fn shape() -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    v.push(("listen port", port::terraria::Netplay::DefaultPort.to_string()));
    v.push(("max connections", port::terraria::Netplay::MaxConnections.to_string()));
    v.push(("net buffer size", port::terraria::Netplay::NetBufferSize.to_string()));
    v.push(("first item id (DirtBlock)", port::terraria::id::ItemID::DirtBlock.to_string()));
    v.push((
        "types projected",
        sheets::registry::rows_in("re/server/types").unwrap_or(0).to_string(),
    ));
    v.push((
        "members projected",
        sheets::registry::rows_in("re/server/fields").unwrap_or(0).to_string(),
    ));
    v.push((
        "methods projected",
        sheets::registry::rows_in("re/server/methods").unwrap_or(0).to_string(),
    ));
    v.push((
        "inventory rows",
        sheets::re_server_types::COUNT.to_string(),
    ));
    v
}

/// `Program.LaunchGame`, as far as the port can take it.
pub fn run(argv: &[String]) -> i32 {
    let params = args::parse(argv);

    println!("terraria-server (Rust port of TerrariaServer.exe)");
    println!();

    println!("launch parameters: {}", params.len());
    for (k, v) in params.iter() {
        println!("  {k} = {v}");
    }
    for d in params.duplicates() {
        println!("  WARNING repeated flag {d}: the C# server would have thrown here (dec020)");
    }
    if !params.is_empty() {
        println!();
    }

    match save_path(&params) {
        Some(p) => println!("save directory: {p}"),
        None => println!("save directory: unresolved (no -savedirectory and no platform data dir)"),
    }
    println!("thread pool floor: {MIN_THREADS}");
    println!();

    println!("the port knows:");
    for (k, v) in shape() {
        println!("  {k:28} {v}");
    }
    println!();

    // The entry path is implemented now (`kernel/net.rs`), so this serves instead of
    // reporting. What is implemented is everything up to and including the handshake;
    // what is not is everything after it, and the module says which message is missing
    // on every kick rather than leaving a client to wait for one that will not come.
    println!("serving: the entry path is implemented (accept, framing, Hello handshake).");
    println!("  after the handshake this build has nothing: world loading, tile sending");
    println!("  and the game loop are not written, so such a client is kicked by name");
    println!("  rather than left hanging (kernel/net.rs).");
    println!();

    // `LaunchInitializer.LoadSharedParameters` reads the port from `-p` / `-port`
    // (LaunchInitializer.cs:30); `-pass` / `-password` sets `Netplay.ServerPassword`
    // (LaunchInitializer.cs:44).
    let listen_port = match params.get("-port").or_else(|| params.get("-p")) {
        Some(v) => match v.trim().parse::<u16>() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("-port {v:?} is not a port number");
                return 1;
            }
        },
        None => port::terraria::Netplay::DefaultPort as u16,
    };
    let password = params
        .get("-password")
        .or_else(|| params.get("-pass"))
        .map(str::to_string)
        .filter(|p| !p.is_empty());

    let listener = match crate::net::bind(listen_port) {
        Ok(l) => l,
        Err(e) => {
            // `Netplay.InitializeServer` reports exactly this when the bind fails.
            eprintln!("Tried to run two servers on the same PC ({e})");
            return 1;
        }
    };
    println!("listening on 0.0.0.0:{listen_port}");
    match crate::net::serve(listener, password) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("server loop stopped: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: &[&str]) -> Vec<String> {
        x.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn savedirectory_flag_wins_over_the_platform_default() {
        let p = args::parse(&v(&["-savedirectory", "D:\\terraria"]));
        assert_eq!(save_path(&p), Some("D:\\terraria".to_string()));
    }

    #[test]
    fn a_savedirectory_flag_is_used_verbatim_even_when_empty() {
        // C# is `LaunchParameters.ContainsKey("-savedirectory") ? LaunchParameters["-savedirectory"]
        // : GetStoragePath("Terraria")`: the flag's value is used as-is whenever it is present.
        // Before the fix this fell through to the platform default for whitespace/empty values.
        assert_eq!(save_path(&args::parse(&v(&["-savedirectory"]))), Some(String::new()));
        assert_eq!(
            save_path(&args::parse(&v(&["-savedirectory", "   "]))),
            Some("   ".to_string())
        );
    }

    #[test]
    fn the_windows_storage_path_is_my_games_not_appdata() {
        // ReLogic.OS.Windows.PathService.GetStoragePath() is
        // Path.Combine(SpecialFolder.Personal, "My Games"), and Personal is Documents.
        assert_eq!(
            windows_save_path(Some("C:\\Users\\x")),
            Some("C:\\Users\\x\\Documents\\My Games\\Terraria".to_string())
        );
        // an unset OR empty USERPROFILE must not yield a relative "Documents\\My Games\\Terraria"
        assert_eq!(windows_save_path(Some("")), None);
        assert_eq!(windows_save_path(None), None);
    }

    #[test]
    fn the_macos_storage_path_treats_an_empty_home_as_unset() {
        assert_eq!(
            macos_save_path(Some("/Users/x")),
            "/Users/x/Library/Application Support/Terraria"
        );
        assert_eq!(macos_save_path(Some("")), "./Terraria");
        assert_eq!(macos_save_path(None), "./Terraria");
    }

    #[test]
    fn the_linux_storage_path_treats_an_empty_xdg_home_as_unset() {
        assert_eq!(unix_save_path(Some("/data"), Some("/home/x")), "/data/Terraria");
        // empty XDG_DATA_HOME must be treated as unset, as the XDG spec (and the C#
        // IsNullOrEmpty check) requires
        assert_eq!(
            unix_save_path(Some(""), Some("/home/x")),
            "/home/x/.local/share/Terraria"
        );
        assert_eq!(unix_save_path(None, Some("/home/x")), "/home/x/.local/share/Terraria");
        assert_eq!(unix_save_path(Some(""), Some("")), "./Terraria");
        assert_eq!(unix_save_path(None, None), "./Terraria");
    }

    #[test]
    fn the_shape_is_read_from_the_sheets_not_typed_here() {
        let s = shape();
        let port_row = s.iter().find(|(k, _)| *k == "listen port").unwrap();
        assert_eq!(port_row.1, "7777");
        let types = s.iter().find(|(k, _)| *k == "types projected").unwrap();
        assert_eq!(types.1, "2463");
    }

    /// `run` now SERVES, so it has no "returns zero" case left to assert: the process
    /// that starts listening does not come back, which is the point. What can be
    /// asserted is every branch that REFUSES to start, and those are checked here. The
    /// serving path itself is covered by `net`'s socket tests, which drive it over a
    /// real socket on a port the OS chose.
    #[test]
    fn an_unusable_port_refuses_to_start() {
        assert_eq!(run(&v(&["-port", "not-a-port"])), 1);
    }

    /// A port already in use is the C# "Tried to run two servers on the same PC" path.
    /// It must report and return, not panic and not hang.
    #[test]
    fn a_port_already_in_use_refuses_to_start() {
        let taken = crate::net::bind(0).unwrap();
        let port = taken.local_addr().unwrap().port().to_string();
        assert_eq!(run(&v(&["-port", port.as_str()])), 1);
        drop(taken);
    }
}
