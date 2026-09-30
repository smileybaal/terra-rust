//! Integration tests for the `sheetty` CLI front end.
//!
//! Every test builds a throwaway sheet book under the system temp directory
//! (never under `sheets/`, which is the real, committed evidence book) and
//! drives the compiled binary through `CARGO_BIN_EXE_sheetty`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary book root with a `sheets/` subdirectory, removed on drop.
struct Book {
    root: PathBuf,
}

impl Book {
    fn new(tag: &str) -> Book {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("sheetty-cli-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("sheets")).expect("create temp sheets dir");
        Book { root }
    }

    fn sheets(&self) -> PathBuf {
        self.root.join("sheets")
    }

    fn sheet(&self, name: &str, body: &str) {
        std::fs::write(self.sheets().join(name), body).expect("write fixture sheet");
    }
}

impl Drop for Book {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sheetty(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sheetty"))
        .args(args)
        .output()
        .expect("run the sheetty binary")
}

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// One sheet that raises an L0 finding (`W-L0-SHORT`, the trailing `note` cell
/// is omitted) and an L1 finding (`E-L1-EMPTY`, `value` is a required u8 left
/// empty).
const LAYER_SHEET: &str = "\
# sheet: lay
# version: 1
# generator: fixture
# target: scratch
# index: by_id
# requires: -
# owned_by: verifier
# doctrine: D1
id:string*\tvalue:u8\tnote:string
x\t1\thi
y\t2
z\t\thello
";

/// `--layer L0` is documented in docs/PIPELINE.md. It must run only the L0
/// checks: before the fix the layer digit was read at the wrong offset, so the
/// filter matched nothing and every L1/L2 finding leaked into an L0 run.
#[test]
fn preflight_layer_l0_does_not_leak_l1_findings() {
    let book = Book::new("layer");
    book.sheet("lay.tsv", LAYER_SHEET);
    let dir = book.sheets();

    let o = sheetty(&["preflight", "--sheets", dir.to_str().unwrap(), "--layer", "L0", "--json"]);
    let out = stdout(&o);

    assert!(
        out.contains("W-L0-SHORT"),
        "the L0 finding must still be reported under --layer L0; stdout: {out}\nstderr: {}",
        stderr(&o)
    );
    assert!(
        !out.contains("E-L1-EMPTY"),
        "--layer L0 leaked an L1 finding into the report; stdout: {out}"
    );
    assert!(
        !out.contains("E-L1-VALUE"),
        "--layer L0 leaked an L1 finding into the report; stdout: {out}"
    );

    // The L0 run still contains an error (E-L0-NOSCHEMA), so preflight fails.
    assert_eq!(o.status.code(), Some(1), "stderr: {}", stderr(&o));
}

const THING_SHEET: &str = "\
# sheet: thing
# version: 1
# generator: fixture
# target: scratch
# index: by_id
# requires: -
# owned_by: verifier
# doctrine: D1
id:string*\tname:string
a\talpha
b\tbeta
";

const VIEWS_SHEET: &str = "\
# sheet: views
# version: 1
# generator: fixture
# target: scratch
# index: by_id
# requires: -
# owned_by: verifier
# doctrine: D1
id:string*\tsheet:string\tcolumns:string\trow_window:string\ttoken_budget:u32\tused_by:string\tstatus:string
v1\tthing\t*\t*\t0\t-\tactive
";

/// docs/PIPELINE.md documents `pack v_client_methods --rows 1..900 --out
/// re/ctx/pack.txt` as runnable from a clean checkout, but `re/ctx` is not
/// committed, so `--out` has to create missing parent directories.
#[test]
fn pack_out_creates_missing_parent_directories() {
    let book = Book::new("packout");
    book.sheet("thing.tsv", THING_SHEET);
    book.sheet("views.tsv", VIEWS_SHEET);
    let dir = book.sheets();
    let out_file = book.root.join("re").join("ctx").join("pack.txt");
    assert!(!out_file.exists());

    let o = sheetty(&[
        "pack",
        "v1",
        "--sheets",
        dir.to_str().unwrap(),
        "--rows",
        "1..900",
        "--out",
        out_file.to_str().unwrap(),
    ]);

    assert_eq!(
        o.status.code(),
        Some(0),
        "pack --out into a missing directory should succeed; stderr: {}",
        stderr(&o)
    );
    let text = std::fs::read_to_string(&out_file).expect("the --out file must exist");
    assert!(text.contains("alpha"), "the pack body should contain the view rows; got: {text}");
}

/// `view --out` uses the same write path and must create missing parents too.
#[test]
fn view_out_creates_missing_parent_directories() {
    let book = Book::new("viewout");
    book.sheet("thing.tsv", THING_SHEET);
    let dir = book.sheets();
    let out_file = book.root.join("nested").join("deeper").join("thing.tsv");

    let o = sheetty(&[
        "view",
        "thing",
        "--sheets",
        dir.to_str().unwrap(),
        "--cols",
        "id,name",
        "--out",
        out_file.to_str().unwrap(),
    ]);

    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    let text = std::fs::read_to_string(&out_file).expect("the --out file must exist");
    assert!(text.contains("alpha"), "got: {text}");
}
