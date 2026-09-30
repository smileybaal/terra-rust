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

    /// A struct exists as a real Rust type, sized from the fields the sheet declares.
    #[test]
    fn a_struct_carries_its_declared_fields() {
        assert!(core::mem::size_of::<port::terraria::Player>() > 0);
        assert!(core::mem::size_of::<port::terraria::Netplay>() > 0);
    }
}
