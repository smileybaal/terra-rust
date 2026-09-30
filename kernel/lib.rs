//! kernel: the hand-written part of the Rust server (D4, `sheets/kernel.tsv`).
//!
//! Everything the server *is* comes from the sheet book and arrives here as
//! generated code, included at build time from `$OUT_DIR`. Everything the server
//! *does* that is not a row lives in these modules, and every one of them is listed
//! in `sheets/kernel.tsv`, because D4 allows exactly two kinds of file: rows, and
//! listed kernel modules.
//!
//! `port` is the projection of three sheets joined together - a type row is an
//! item, its field rows are the struct body, its method rows are the `impl`. Its
//! bodies are stubs: what it carries is the server's SHAPE, at a fidelity the
//! sheets can prove. Turning that shape into behaviour is the kernel's job, and
//! that job has barely started, which is why `boot` says so out loud rather than
//! pretending to serve.

/// The generated server. Nothing here is written by hand, and nothing here may be
/// edited by hand: change the sheets (D6).
///
/// The lint allows live HERE rather than in the generated file, because an inner
/// attribute is not permitted inside `include!`. The port keeps C# casing on
/// purpose (a field named `_serverIP` should read as it does in the source), which
/// is what `non_snake_case` would otherwise complain about on every line.
pub mod port {
    #![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
    #![allow(dead_code, unused_variables, unused_mut, clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/port.rs"));
}

/// The per-sheet projections of the server's evidence, for querying the book from
/// Rust rather than only from the CLI.
pub mod sheets {
    pub mod re_server_types {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_server_types.rs"));
    }
    /// The server's own localization table (`sheets/re/server/localization.tsv`).
    ///
    /// The sheets carry no strings at all (dec015), so before this relation every
    /// protocol string in `net` was hand-cited from the C# and the game's
    /// localization file. Now they are rows, and `by_id` reads them: the lookup is
    /// a binary search over a static table sorted by id.
    pub mod localization {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_server_localization.rs"));
    }
    pub mod registry {
        include!(concat!(env!("OUT_DIR"), "/registry.rs"));
    }
}

pub mod args;
pub mod boot;
pub mod net;

#[cfg(test)]
mod tests {
    use super::*;

    /// The port is present and carries the whole server, not a sample of it.
    #[test]
    fn the_port_covers_the_server() {
        assert!(sheets::registry::rows_in("re/server/types") == Some(2463));
        assert!(sheets::registry::rows_in("re/server/fields") == Some(30040));
        assert!(sheets::registry::rows_in("re/server/methods") == Some(14486));
    }

    /// A constant from an ID table carries its real value. This is the end of the
    /// chain: C# source -> ILSpy -> sheet row -> generated Rust.
    #[test]
    fn a_projected_constant_carries_its_value() {
        assert_eq!(port::terraria::id::ItemID::DirtBlock, 2);
        assert_eq!(port::terraria::Netplay::DefaultPort, 7777);
        assert_eq!(port::terraria::Netplay::MaxConnections, 256);
    }

    /// A nested type projects as a sibling MODULE, not as a nested item: the
    /// namespace `Terraria.Player` becomes `terraria::player`, and `Settings` is an
    /// item in it. Module names are lowercased precisely so that `mod player` and
    /// `struct Player` can coexist in `terraria`, which is what a nested type needs.
    #[test]
    fn a_nested_type_is_a_real_item() {
        assert!(core::mem::size_of::<port::terraria::player::Settings>() > 0);
        assert!(core::mem::size_of::<port::terraria::player::selectionradial::SelectionMode>() > 0);
    }

    /// An enum member carries its discriminant, including the negative one, and a
    /// member with no explicit value is numbered from the previous member.
    #[test]
    fn enum_discriminants_survive_the_projection() {
        use port::terraria::achievements::AchievementCategory;
        assert_eq!(AchievementCategory::None as i64, -1);
        assert_eq!(AchievementCategory::Slayer as i64, 0);
        assert_eq!(AchievementCategory::Collector as i64, 1);
    }

    /// The localization relation carries the CLI section, and every row is a
    /// lookupable key. This is the chain the port now depends on: the game's JSON
    /// -> sheet row -> generated `Def` -> the string the server prints.
    #[test]
    fn the_localization_relation_carries_the_cli_section() {
        use sheets::localization::{by_id, COUNT};
        assert_eq!(COUNT, 108);
        let prompt = by_id("cli.chooseworld").expect("the world-select prompt is a row");
        assert_eq!(prompt.key, "CLI.ChooseWorld");
        assert_eq!(prompt.text, "Choose World: ");
        assert_eq!(prompt.section, "CLI");
        // The row the connection-limit kick needs, so a typo in the id cannot
        // silently produce a kick with no text.
        assert_eq!(by_id("cli.serverisfull").unwrap().key, "CLI.ServerIsFull");
        assert_eq!(by_id("cli.serverisfull").unwrap().text,
                   "This server is full right now, please try again later.");
        // `length` is the UTF-8 byte length, so a multi-byte string would disagree
        // with `.len()` and this is where that would surface.
        for d in sheets::localization::ALL {
            assert_eq!(d.length as usize, d.text.len(), "length disagrees for {}", d.id);
        }
    }

    /// A struct exists as a real Rust type, sized from the fields the sheet declares.
    #[test]
    fn a_struct_carries_its_declared_fields() {
        assert!(core::mem::size_of::<port::terraria::Player>() > 0);
        assert!(core::mem::size_of::<port::terraria::Netplay>() > 0);
    }

    /// A row that carries NO value must not be quietly hardcoded instead.
    ///
    /// `terraria.netplay.serverpassword` is a real row, but the extractor left its value
    /// as `-` and its status as `todo`, so the projection gives `Netplay::ServerPassword`
    /// a FIELD and no constant, and the port's "no password by default" is cited from the
    /// C# (`Netplay.cs:38`) rather than dressed up as projected. That is the honest state
    /// of the book, and this test is here to FAIL the day it stops being true: the moment
    /// the extractor records a value, `""` becomes a stale hardcode and the fix is to read
    /// the projected constant. A check that cannot fire is not a check, so this one can.
    #[test]
    fn a_value_less_row_is_not_silently_hardcoded() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("kernel/ has a parent: the repo root");
        let book =
            std::fs::read_to_string(root.join("sheets/re/server/fields.tsv")).expect("the book");
        // The header names its columns (`value:string?`, `status:string`), so the columns
        // are found by name rather than by a position typed here.
        let header: Vec<&str> = book
            .lines()
            .find(|l| !l.starts_with('#'))
            .expect("a header line")
            .split('\t')
            .collect();
        let col = |prefix: &str| {
            header
                .iter()
                .position(|c| c.starts_with(prefix))
                .unwrap_or_else(|| panic!("the header has a {prefix} column"))
        };
        let (value, status) = (col("value"), col("status"));
        let row: Vec<&str> = book
            .lines()
            .find(|l| l.starts_with("terraria.netplay.serverpassword"))
            .expect("the row exists")
            .split('\t')
            .collect();
        assert_eq!(
            row[value], "-",
            "the extractor now records a value for ServerPassword: consume the projected \
             constant in boot.rs instead of citing Netplay.cs:38, and drop this exemption"
        );
        assert_eq!(row[status], "todo");
    }
}
