//! build.rs: the thin shim MDD 5.6 requires.
//!
//! It runs the pipeline (preflight gates it, D5), emits the port and the per-sheet
//! modules into $OUT_DIR (5.9), and prints per-sheet `rerun-if-changed` hints and
//! nothing global (5.9).
//!
//! There is no checked-in `port.rs`. The port is a join of three sheets, so it is
//! generated on every build from `re/server/{types,fields,methods}` and the sheets
//! stay the only place the server's shape is written down (D1).

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // kernel/ -> repo root
    let root = manifest_dir.parent().expect("repo root").to_path_buf();
    let sheets = root.join("sheets");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

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
