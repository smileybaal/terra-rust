//! terraria-demo: the smallest crate that proves the strut equation.
//!
//! Nothing here is hand-written data. Every `Def` and every field value comes
//! from a sheet row via `build.rs`. If a sheet changes, this crate changes; if
//! preflight fails, this crate does not build at all (MDD D5).
//!
//! The client and server are separate modules because they are separate sheet
//! sets over separate binaries, even though they share 1,544 type ids.

pub mod generated {
    //! One module per emitted sheet (MDD 5.9: never one giant module).

    /// Managed types recovered from Terraria.exe (client). 1549 rows.
    pub mod re_client_types {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_client_types.rs"));
    }

    /// Managed types recovered from TerrariaServer.exe (server). 1551 rows.
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
    #[test]
    fn plan_status_comes_from_the_sheet() {
        let core = plan::by_id("core_math").expect("core_math must exist");
        assert_eq!(core.status, "todo");
        assert_eq!(core.priority, 1);
        assert_eq!(core.target, "crate::core::math");
    }
}
