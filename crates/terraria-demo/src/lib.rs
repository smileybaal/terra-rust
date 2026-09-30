//! terraria-demo: the smallest crate that proves the strut equation.
//!
//! Nothing here is hand-written data. Every `Def` and every field value comes
//! from a sheet row via `build.rs`. If a sheet changes, this crate changes; if
//! preflight fails, this crate does not build at all (MDD D5).
//!
//! The client and server are separate modules because they are separate sheet
//! sets over separate binaries, even though they share 2,458 type ids.

pub mod generated {
    //! One module per emitted sheet (MDD 5.9: never one giant module).

    /// Managed types recovered from Terraria.exe (client). 2464 rows.
    pub mod re_client_types {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_client_types.rs"));
    }

    /// Managed types recovered from TerrariaServer.exe (server). 2463 rows.
    pub mod re_server_types {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_server_types.rs"));
    }

    /// The porting plan: one strut per subsystem.
    pub mod plan {
        include!(concat!(env!("OUT_DIR"), "/sheets/plan.rs"));
    }

    /// Identifiers and lengths only, so its rebuild is cheap (MDD 5.9 item 4).
    pub mod registry {
        include!(concat!(env!("OUT_DIR"), "/registry.rs"));
    }
}

#[cfg(test)]
mod tests {
    use super::generated::{plan, re_client_types, re_server_types, registry};

    /// The strut exists and carries the row's data.
    #[test]
    fn a_row_projects_to_a_def() {
        let p = re_client_types::by_id("terraria.player").expect("terraria.player must exist");
        assert_eq!(p.name, "Player");
        assert_eq!(p.namespace, "Terraria");
        assert_eq!(p.kind, "class");
        // measured by ILSpy from the decompiled source, not guessed. It is the
        // count of this type's OWN members: 1,316 included the members of its 21
        // nested types, which now live in their own rows (dec011, dec018).
        assert_eq!(p.fields, 1313);
    }

    /// Both platforms are emitted, and both resolve the same shared type.
    #[test]
    fn both_platforms_are_present() {
        let c = re_client_types::by_id("terraria.player").expect("client Player");
        let s = re_server_types::by_id("terraria.player").expect("server Player");
        assert_eq!(c.name, s.name);
        assert_eq!(c.namespace, s.namespace);
    }

    /// The platform split, asserted from data rather than from a hand-written list.
    /// These are the only two types whose field counts differ between the two
    /// builds: conditional compilation, which the entity-name inventory could not
    /// see and which the sheets surfaced only once they were separated and
    /// overlapped.
    #[test]
    fn the_two_conditional_compilation_divergences_are_visible() {
        let sc = re_client_types::by_id("terraria.netplay").expect("client NetPlay");
        let ss = re_server_types::by_id("terraria.netplay").expect("server NetPlay");
        assert_eq!(sc.fields, 28);
        assert_eq!(ss.fields, 30);

        let cc = re_client_types::by_id("terraria.initializers.chromainitializer")
            .expect("client ChromaInitializer");
        let cs = re_server_types::by_id("terraria.initializers.chromainitializer")
            .expect("server ChromaInitializer");
        assert_eq!(cc.fields, 15);
        assert_eq!(cs.fields, 11);
    }

    /// Nested types are rows, under their parent's dotted path, and own their own
    /// members rather than lending them to the parent (dec011, dec018).
    #[test]
    fn nested_types_are_rows_that_own_their_members() {
        let outer = re_server_types::by_id("terraria.player").expect("Player");
        assert_eq!(outer.name, "Player");
        assert_eq!(outer.namespace, "Terraria");

        let inner = re_server_types::by_id("terraria.player.settings").expect("Player.Settings");
        assert_eq!(inner.name, "Settings");
        // the namespace of a nested type is its parent's full path, which is what
        // makes the id derivable from the row
        assert_eq!(inner.namespace, "Terraria.Player");
        assert_eq!(inner.kind, "class");
        // and it owns its OWN members rather than the parent's: 8, not Player's 1313
        assert_eq!(inner.fields, 8);

        // a nested type can itself contain a nested type
        assert!(re_server_types::by_id("terraria.main.currentframeflags.hacks").is_some());
    }

    /// Server-only types exist in the server sheet and not the client one.
    #[test]
    fn server_only_types_are_absent_from_the_client() {
        assert!(re_server_types::by_id("natupnplib.upnpnat").is_some());
        assert!(re_client_types::by_id("natupnplib.upnpnat").is_none());
        // ...and the reverse, for a client-only type.
        assert!(re_client_types::by_id("terraria.audio.mp3audiotrack").is_some());
        assert!(re_server_types::by_id("terraria.audio.mp3audiotrack").is_none());
    }

    /// The dense registry is complete and self-consistent.
    #[test]
    fn the_registry_matches_the_sheet() {
        assert_eq!(re_client_types::COUNT, re_client_types::ALL.len());
        assert_eq!(re_server_types::COUNT, re_server_types::ALL.len());
        assert_eq!(plan::COUNT, plan::ALL.len());
        assert_eq!(registry::rows_in("re/client/types"), Some(re_client_types::COUNT));
        assert_eq!(registry::rows_in("re/server/types"), Some(re_server_types::COUNT));
        assert_eq!(registry::rows_in("02-plan"), Some(plan::COUNT));
    }

    /// The row counts written in this crate's own module docs are claims about the
    /// data, so they are read back and checked against the registry rather than
    /// left as prose that can drift. They did drift: the client/server split
    /// changed the counts and the doc lines were never updated, so the crate
    /// advertised 1549/1551 rows while emitting 2464/2463.
    #[test]
    fn documented_counts_match_the_sheets() {
        let src = include_str!("lib.rs");

        let row_count = |marker: &str| -> usize {
            let line = src
                .lines()
                .find(|l| l.contains(marker) && l.contains("rows."))
                .unwrap_or_else(|| panic!("no module doc stating a row count near '{marker}'"));
            line.split_whitespace()
                .find_map(|w| w.trim_end_matches("rows.").parse::<usize>().ok())
                .unwrap_or_else(|| panic!("no row count in doc line: {line}"))
        };
        assert_eq!(registry::rows_in("re/client/types"), Some(row_count("(client).")));
        assert_eq!(registry::rows_in("re/server/types"), Some(row_count("(server).")));

        // ...and the same for the claim about how many type ids the two platforms
        // share, recomputed from the emitted data rather than trusted.
        let shared_line = src
            .lines()
            .find(|l| l.contains("type ids."))
            .expect("the module doc must state how many ids the platforms share");
        let documented: usize = shared_line
            .split_whitespace()
            .find_map(|w| w.trim_end_matches("type ids.").replace(',', "").parse().ok())
            .expect("a number before 'type ids.'");
        let shared = re_client_types::ALL.iter().filter(|d| re_server_types::by_id(d.id).is_some()).count();
        assert_eq!(documented, shared, "stale doc line: {shared_line}");
    }

    /// Rows are emitted sorted, which is what makes the lookup a binary search.
    #[test]
    fn rows_are_sorted_by_key() {
        for w in re_client_types::ALL.windows(2) {
            assert!(w[0].id <= w[1].id, "re/client/types unsorted at {}", w[0].id);
        }
        for w in re_server_types::ALL.windows(2) {
            assert!(w[0].id <= w[1].id, "re/server/types unsorted at {}", w[0].id);
        }
        for w in plan::ALL.windows(2) {
            assert!(w[0].id <= w[1].id, "02-plan unsorted at {}", w[0].id);
        }
    }

    /// A missing key is a miss, not a panic.
    #[test]
    fn unknown_keys_miss_cleanly() {
        assert!(re_client_types::by_id("no.such.type").is_none());
        assert!(re_server_types::by_id("no.such.type").is_none());
    }

    /// The plan is the specification relation, and status comes from the sheet.
    ///
    /// The priority is the WORKLOAD rank, so it moved when the rank was measured
    /// rather than guessed (dec021): `core_math` is the dependency of everything
    /// above it and is therefore ranked last of the units a server actually runs.
    /// This test failing on that change is the point of it - the value is read
    /// from `sheets/02-plan.tsv`, not from the code under test.
    #[test]
    fn plan_status_comes_from_the_sheet() {
        let core = plan::by_id("core_math").expect("core_math must exist");
        assert_eq!(core.status, "todo");
        assert_eq!(core.priority, 13, "the measured workload rank");
        assert_eq!(core.target, "crate::core::math");
        // A unit the dedicated server never executes says so, and that is a sheet
        // value like any other.
        let render = plan::by_id("render").expect("render must exist");
        assert_eq!(render.status, "n/a");
        assert_eq!(render.priority, 99);
    }
}
