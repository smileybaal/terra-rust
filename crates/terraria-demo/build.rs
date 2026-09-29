//! build.rs: the thin shim MDD 5.6 requires.
//!
//! It does three things and nothing else:
//!   1. runs the whole pipeline (preflight gates it, D5)
//!   2. emits one Rust module per sheet into $OUT_DIR, hash-gated (5.9)
//!   3. prints per-sheet `rerun-if-changed` hints and nothing global (5.9)

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // crates/terraria-demo -> repo root
    let root = manifest_dir.parent().and_then(|p| p.parent()).expect("repo root").to_path_buf();
    let sheets = root.join("sheets");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    // 5.9 item 3: emit nothing global. Every sheet this build reads gets its own
    // hint, so editing entities.tsv cannot dirty weapons.rs.
    for hint in sheetty::rerun_hints(&sheets) {
        println!("{hint}");
    }

    match sheetty::emit_run(&sheets, &out) {
        Ok(reports) => {
            let written = reports.iter().filter(|r| r.written).count();
            println!(
                "cargo:warning=sheetty: {} module(s), {written} written, {} unchanged",
                reports.len(),
                reports.len() - written
            );
            for r in &reports {
                println!("cargo:warning=  {} <- {} ({} rows)", r.module, r.sheet, r.rows);
            }
        }
        Err(e) => {
            // D5: a failed preflight fails the build, loudly, naming the sheet.
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
