//! The boot sequence, hand-written (D4; listed in `sheets/kernel.tsv`).
//!
//! Mirrors `Terraria.Program.LaunchGame`, in order: parse the launch parameters,
//! resolve the save directory (`-savedirectory` or the platform storage path),
//! raise the thread-pool floor to 8, set up logging, then run.
//!
//! What it CANNOT mirror is the run itself. `LaunchGame` ends in `RunGame`, which
//! sets `Main.dedServ = true` and calls `main.DedServ()` then `main.Run()`. In this
//! port those are generated stubs whose bodies are `unimplemented!()`, because the
//! sheets carry the server's SHAPE and not its behaviour. So this reports the shape
//! it found and says plainly that it is not serving, rather than printing a banner
//! that implies otherwise.

use crate::args::{self, LaunchParameters};
use crate::port;
use crate::sheets;

/// `Program.LaunchGame` sets the thread-pool floor before anything else runs.
pub const MIN_THREADS: usize = 8;

/// The flag `LaunchGame` reads the save directory from.
pub const SAVE_DIR_FLAG: &str = "-savedirectory";

/// The storage folder `IPathService.GetStoragePath("Terraria")` resolves to.
pub const SAVE_FOLDER: &str = "Terraria";

/// An approximation of `Platform.Get<IPathService>().GetStoragePath("Terraria")`.
///
/// The original asks the OS abstraction for the per-user data directory; this uses
/// the environment variables that back it on each platform. It is an approximation
/// and is marked as one: the exact answer is a kernel job that has not been done
/// (see `docs/PIPELINE.md`), and guessing silently would be worse than saying so.
pub fn default_save_path() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("APPDATA").ok().map(|b| format!("{b}\\{SAVE_FOLDER}"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(|h| format!("{h}/Library/Application Support/{SAVE_FOLDER}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var("XDG_DATA_HOME")
            .ok()
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.local/share")))
            .map(|b| format!("{b}/{SAVE_FOLDER}"))
    }
}

/// Resolve the save directory the way `LaunchGame` does.
pub fn save_path(params: &LaunchParameters) -> Option<String> {
    match params.get(SAVE_DIR_FLAG) {
        // the original treats an empty value as "not given" and falls through
        Some(v) if !v.trim().is_empty() => Some(v.to_string()),
        _ => default_save_path(),
    }
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

    // Say what is actually true. A generated body is `unimplemented!()`, so calling
    // into the server would panic rather than serve.
    println!("NOT SERVING. This build is the projected SHAPE of the server:");
    println!("  {} types, {} members and {} methods, projected from", 
        sheets::registry::rows_in("re/server/types").unwrap_or(0),
        sheets::registry::rows_in("re/server/fields").unwrap_or(0),
        sheets::registry::rows_in("re/server/methods").unwrap_or(0));
    println!("  sheets/re/server/{{types,fields,methods}}.tsv into crates of Rust.");
    println!("  Every generated body is a stub. Behaviour is the kernel's job and has");
    println!("  not been written yet.");
    0
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
    fn an_empty_savedirectory_falls_through_to_the_default() {
        // `LaunchGame` checks the value is not whitespace before using it
        let p = args::parse(&v(&["-savedirectory", "   "]));
        assert_eq!(save_path(&p), default_save_path());
    }

    #[test]
    fn the_shape_is_read_from_the_sheets_not_typed_here() {
        let s = shape();
        let port_row = s.iter().find(|(k, _)| *k == "listen port").unwrap();
        assert_eq!(port_row.1, "7777");
        let types = s.iter().find(|(k, _)| *k == "types projected").unwrap();
        assert_eq!(types.1, "2463");
    }

    #[test]
    fn running_reports_and_exits_zero() {
        assert_eq!(run(&v(&["-savedirectory", "x"])), 0);
    }
}
