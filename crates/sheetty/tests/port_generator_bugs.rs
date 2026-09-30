//! Regression tests for real bugs in the sheet -> Rust port generator.
//!
//! Each fixture builds a tiny sheet book, runs the generator, and COMPILES the
//! result with rustc: a duplicate item, an illegal visibility qualifier or an
//! infinitely sized type is then a real, observed error rather than an assumption.
//! Several of these were latent (they do not fire on the current sheets), but the
//! generator accepted the input and emitted invalid Rust, so they are real bugs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let mut p = std::env::temp_dir();
    p.push(format!("sheetty-test-{}-{}-{}", std::process::id(), tag, n));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&p, body).unwrap();
}

const TYPES_HDR: &str =
    "# sheet: types\n# version: 1\n# emit_port: rust\n# port_from: fields,methods\nid:string*\tnamespace:string\tname:string\tkind:string\tbase:string\n";
const FIELDS_HDR: &str =
    "# sheet: fields\nid:string*\ttype:string\tname:string\tfield_type:string\tkind:string\tmodifiers:string\tvalue:string?\tord:u16\n";
const METHODS_HDR: &str =
    "# sheet: methods\nid:string*\ttype:string\tname:string\tret:string\tparams:string?\tmodifiers:string\tkind:string\n";

/// Emit the port for a fixture and return (source, rustc stderr).
fn emit(tag: &str, types_rows: &str, fields_rows: &str, methods_rows: &str) -> (String, String) {
    let d = tmpdir(tag);
    write(&d, "types.tsv", &format!("{TYPES_HDR}{types_rows}"));
    write(&d, "fields.tsv", &format!("{FIELDS_HDR}{fields_rows}"));
    write(&d, "methods.tsv", &format!("{METHODS_HDR}{methods_rows}"));
    let (sheets, _f) = sheetty::load_book(&d);
    let (src, _rep) = sheetty::emit_port_source(&sheets).expect("emit");

    std::fs::write(d.join("port.rs"), &src).unwrap();
    std::fs::write(
        d.join("wrapper.rs"),
        "pub mod port {\n#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]\n#![allow(dead_code, unused_variables, unused_mut)]\ninclude!(\"port.rs\");\n}\n",
    )
    .unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let res = std::process::Command::new(rustc)
        .arg("--edition")
        .arg("2021")
        .arg("--crate-type")
        .arg("lib")
        .arg("--emit")
        .arg("metadata")
        .arg("-o")
        .arg(d.join("meta"))
        .arg(d.join("wrapper.rs"))
        .output()
        .expect("run rustc");
    let stderr = String::from_utf8_lossy(&res.stderr).to_string();
    (src, stderr)
}

/// Emit and assert the generated port compiles.
fn assert_port_compiles(tag: &str, types_rows: &str, fields_rows: &str, methods_rows: &str) -> String {
    let (src, err) = emit(tag, types_rows, fields_rows, methods_rows);
    assert!(err.trim().is_empty(), "generated port did not compile ({tag}):\n{err}\n--- source ---\n{src}");
    src
}

#[test]
fn extern_arity_name_collision_is_disambiguated() {
    // A C# type `Foo` used at arity 1 and a C# type literally named `Foo_a1` both
    // want `externs::Foo_a1`; before the fix two items with that name were emitted.
    let types = "a\t-\tBar\tclass\t-\n";
    let fields = "f1\ta\tx\tFoo<int>\tfield\t-\t-\t0\nf2\ta\ty\tFoo_a1\tfield\t-\t-\t1\n";
    let src = assert_port_compiles("extern_arity", types, fields, "");
    assert_eq!(src.matches("pub struct Foo_a1<").count(), 1);
    assert_eq!(src.matches("pub struct Foo_a1_2;").count(), 1);
}

#[test]
fn namespace_sanitizing_to_externs_does_not_duplicate_the_module() {
    // A namespace `Externs` sanitizes to `externs`, the hard-coded externs module.
    let types = "a\tExterns\tBar\tclass\t-\n";
    let src = assert_port_compiles("externs_ns", types, "", "");
    assert_eq!(src.matches("pub mod externs {").count(), 1);
    assert_eq!(src.matches("pub mod externs_ {").count(), 1);
}

#[test]
fn interface_method_is_not_emitted_with_pub() {
    // `pub fn` inside a trait is E0449.
    let types = "a\t-\tIFoo\tinterface\t-\nb\t-\tBar\tclass\t-\n";
    let fields = "f1\tb\tiface\tIFoo\tfield\t-\t-\t0\n";
    let methods = "m1\ta\tDo\tvoid\t\t-\tmethod\n";
    let src = assert_port_compiles("iface_method", types, fields, methods);
    assert!(src.contains("fn Do(&self) { unimplemented!() }"));
    assert!(!src.contains("pub fn Do"));
}

#[test]
fn enum_variant_dedup_keeps_extending_until_free() {
    // Enum `A` with members `A_` and `A`: both fold to the variant `A_`.
    let types = "a\t-\tA\tenum\t-\n";
    let fields = "f1\ta\tA_\t-\tenum_member\t-\t0\t0\nf2\ta\tA\t-\tenum_member\t-\t-\t1\n";
    let src = assert_port_compiles("enum_dedup", types, fields, "");
    assert_eq!(src.matches("A_ = 0,").count(), 1);
    assert_eq!(src.matches("A__ = 1,").count(), 1);
}

#[test]
fn nullable_self_reference_is_boxed() {
    // `Foo?` maps to `Option<Foo>`; Option is a value wrapper, so a self cycle is
    // still infinitely sized unless the Box goes inside.
    let types = "a\t-\tFoo\tclass\t-\n";
    let fields = "f1\ta\tnext\tFoo?\tfield\t-\t-\t0\n";
    let src = assert_port_compiles("nullable_self", types, fields, "");
    assert!(src.contains("Option<Box<crate::port::Foo>>"), "src:\n{src}");
}

#[test]
fn nullable_mutual_reference_is_boxed() {
    let types = "a\t-\tFoo\tclass\t-\nb\t-\tBar\tclass\t-\n";
    let fields = "f1\ta\tb\tBar?\tfield\t-\t-\t0\nf2\tb\ta\tFoo?\tfield\t-\t-\t1\n";
    assert_port_compiles("nullable_mutual", types, fields, "");
}

#[test]
fn duplicate_const_names_are_disambiguated() {
    let types = "a\t-\tFoo\tclass\t-\n";
    let fields = "f1\ta\tX\tint\tconst\t-\t1\t0\nf2\ta\tX\tint\tconst\t-\t2\t1\n";
    let src = assert_port_compiles("const_dup", types, fields, "");
    assert!(src.contains("pub const X: i32 = 1;"));
    assert!(src.contains("pub const X_2: i32 = 2;"));
}

#[test]
fn hex_integer_constants_are_projected() {
    let types = "a\t-\tFoo\tclass\t-\n";
    let fields = "f1\ta\tMask\tint\tconst\t-\t0xFF\t0\n";
    let src = assert_port_compiles("const_hex", types, fields, "");
    assert!(src.contains("pub const Mask: i32 = 0xFF;"), "src:\n{src}");
}

#[test]
fn multi_char_integer_suffix_is_stripped() {
    // `1400159338497uL` is a plain `ulong` constant; the generator used to strip
    // only the final `L`, leaving `...u`, and dropped the constant entirely.
    let types = "a\t-\tFoo\tclass\t-\n";
    let fields = "f1\ta\tVersion\tulong\tconst\t-\t1400159338497uL\t0\n";
    let src = assert_port_compiles("int_suffix", types, fields, "");
    assert!(src.contains("pub const Version: u64 = 1400159338497;"), "src:\n{src}");
}
