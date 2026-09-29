//! terraria-demo: the smallest crate that proves the strut equation.
//!
//! Nothing here is hand-written data. Every `Def` and every field value comes
//! from a sheet row via `build.rs`. If a sheet changes, this crate changes; if
//! preflight fails, this crate does not build at all (MDD D5).
//!
//! The point of the method is that the author of these files never writes them.
//! A 40-token row becomes a struct, a registry entry and a lookup, and adding
//! the 1500th type costs the same as the 3rd.

pub mod generated {
    //! One module per emitted sheet (MDD 5.9: never one giant module).

    /// Managed types recovered by ILSpy. 1549 rows.
    pub mod re_types {
        include!(concat!(env!("OUT_DIR"), "/sheets/re_types.rs"));
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
    use super::generated::{plan, re_types, registry};

    /// The strut exists and carries the row's data.
    #[test]
    fn a_row_projectes_to_a_def() {
        let p = re_types::by_id("terraria.player").expect("terraria.player must exist");
        assert_eq!(p.name, "Player");
        assert_eq!(p.namespace, "Terraria");
        assert_eq!(p.kind, "class");
        // measured by ILSpy from the decompiled source, not guessed
        assert_eq!(p.fields, 1316);
    }

    /// The dense registry is complete and self-consistent.
    #[test]
    fn the_registry_matches_the_sheet() {
        assert_eq!(re_types::COUNT, re_types::ALL.len());
        assert_eq!(plan::COUNT, plan::ALL.len());
        assert_eq!(registry::rows_in("re/types"), Some(re_types::COUNT));
        assert_eq!(registry::rows_in("02-plan"), Some(plan::COUNT));
    }

    /// Rows are emitted sorted, which is what makes the lookup a binary search.
    #[test]
    fn rows_are_sorted_by_key() {
        for w in re_types::ALL.windows(2) {
            assert!(w[0].id <= w[1].id, "re/types is not sorted at {}", w[0].id);
        }
        for w in plan::ALL.windows(2) {
            assert!(w[0].id <= w[1].id, "02-plan is not sorted at {}", w[0].id);
        }
    }

    /// A missing key is a miss, not a panic.
    #[test]
    fn unknown_keys_miss_cleanly() {
        assert!(re_types::by_id("no.such.type").is_none());
    }

    /// The plan is the specification relation: every row starts at `todo`, and
    /// the emitted status must reflect the sheet rather than a default.
    #[test]
    fn plan_status_comes_from_the_sheet() {
        let core = plan::by_id("core_math").expect("core_math must exist");
        assert_eq!(core.status, "todo");
        assert_eq!(core.priority, 1);
        assert_eq!(core.target, "crate::core::math");
    }
}
