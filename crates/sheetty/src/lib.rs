//! sheetty: the parser, schema checker, overlap engine and preflight reporter
//! for THE SPREADSHEET METHOD.
//!
//! Doctrine implemented here:
//!   D1 sheets are the source of truth
//!   D3 columns are typed slots
//!   D5 preflight gates every build
//!   D7 canonical artifact is plain-text TSV (UTF-8 no BOM, LF, TAB)
//!   D10 fail loudly at build time
//!
//! Layers:
//!   L0 structural     encoding, line endings, delimiter, manifest, header/schema
//!                     agreement, id presence/uniqueness/charset, kernel allowlist
//!   L1 type & domain  every cell parses as its declared type, units, enum domain,
//!                     empty/NULL semantics, defaults, number form
//!   L2 reference      every foreign key resolves, no cycles, topological order
//!   L3 coverage       spec x impl overlap: covered / unimplemented / orphan / divergent
//!   L4 asset          referenced files exist (declared-but-not-yet checks report)
//!
//! Deliberate, recorded relaxations (see sheets/decisions.tsv):
//!   dec004 short rows are padded to header width
//!   dec005 ref_addr format is per-producer; L2 checks presence, not format

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};

pub const EMITTER_VERSION: &str = "0.1.0";

// ---------------------------------------------------------------------------
// model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub sheet: String,
    pub line: usize,
    pub row: String,
    pub col: String,
    pub message: String,
}

impl Finding {
    pub fn err(code: &str, sheet: &str, line: usize, row: &str, col: &str, msg: impl Into<String>) -> Self {
        Finding { code: code.into(), severity: Severity::Error, sheet: sheet.into(), line, row: row.into(), col: col.into(), message: msg.into() }
    }
    pub fn warn(code: &str, sheet: &str, line: usize, row: &str, col: &str, msg: impl Into<String>) -> Self {
        Finding { code: code.into(), severity: Severity::Warning, sheet: sheet.into(), line, row: row.into(), col: col.into(), message: msg.into() }
    }
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    pub raw: String,
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub pk: bool,
    pub unit: Option<String>,
    pub ref_to: Option<(String, String)>,
    pub default: Option<String>,
    pub flags: String,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub sheet: String,
    pub version: String,
    pub generator: String,
    pub target: String,
    pub index: String,
    pub requires: Vec<String>,
    pub owned_by: String,
    pub doctrine: String,
    pub evidence_required: bool,
    pub id_compound: bool,
    pub emit_rust: bool,
    /// Emit the whole Rust port from this sheet and the sheets it names in
    /// `port_from`. A port is a JOIN of relations, so it cannot be produced from
    /// any single sheet: the types sheet is the item, the fields sheet is the
    /// struct body, the methods sheet is the `impl`.
    pub emit_port: bool,
    pub port_from: Vec<String>,
    pub key_alias: Vec<String>,
    /// How many records the producer emitted, and how many were deliberately
    /// not kept. `source_rows == rows + dropped` must hold, or evidence was
    /// silently lost (L5, MDD 8.8 step 1 / risk R6).
    pub source_rows: Option<usize>,
    pub dropped: Option<usize>,
    pub dropped_reason: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub line: usize,
    pub cells: Vec<String>,
    pub short: bool,
    pub quoted: bool,
}

#[derive(Clone, Debug)]
pub struct Sheet {
    pub path: PathBuf,
    pub rel: String,
    pub name: String,
    pub manifest: Manifest,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl Sheet {
    pub fn col_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
    pub fn cell<'a>(&self, r: &'a Row, name: &str) -> Option<&'a str> {
        self.col_index(name).and_then(|i| r.cells.get(i)).map(|s| s.as_str())
    }
    pub fn pk_index(&self) -> Option<usize> {
        self.columns.iter().position(|c| c.pk).or(if self.columns.is_empty() { None } else { Some(0) })
    }
    /// The identity of a row in the report: its pk value if there is one.
    pub fn row_key(&self, r: &Row) -> String {
        self.pk_index().and_then(|i| r.cells.get(i)).cloned().unwrap_or_else(|| format!("line{}", r.line))
    }
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

/// Parse the column-header grammar (MDD 5.3):
///   <name> ':' <type> [ '?' ] [ ' ' <unit> ] [ '->' <sheet> '.' <col> ] [ '=' <default> ] [ '*' <flags> ]
pub fn parse_column(raw: &str) -> Result<Column, String> {
    let s = raw.trim();
    let (name, rest) = s.split_once(':').ok_or_else(|| format!("column '{raw}' has no ':' type separator"))?;
    if name.is_empty() {
        return Err(format!("column '{raw}' has an empty name"));
    }
    let mut rest = rest.trim().to_string();

    // flags: trailing run of * ! + .
    let mut flags = String::new();
    while let Some(c) = rest.chars().last() {
        if matches!(c, '*' | '!' | '+' | '.') {
            flags.insert(0, c);
            rest.pop();
        } else {
            break;
        }
    }
    let rest = rest.trim_end().to_string();

    // default: ' =' <value>
    let (rest, default) = match rest.find(" =") {
        Some(i) => (rest[..i].trim_end().to_string(), Some(rest[i + 2..].trim().to_string())),
        None => (rest, None),
    };

    // foreign key: ' -> ' <sheet>.<col>
    let (rest, ref_to) = match rest.find(" -> ") {
        Some(i) => {
            let target = rest[i + 4..].trim().to_string();
            let (tsheet, tcol) = target
                .split_once('.')
                .ok_or_else(|| format!("ref '{target}' must be <sheet>.<column>"))?;
            (rest[..i].trim_end().to_string(), Some((tsheet.to_string(), tcol.to_string())))
        }
        None => (rest, None),
    };

    // optional: trailing '?'
    let (rest, optional) = if let Some(stripped) = rest.strip_suffix('?') {
        (stripped.trim_end().to_string(), true)
    } else {
        (rest, false)
    };

    // unit: everything after the first whitespace run following the type token
    let mut it = rest.splitn(2, char::is_whitespace);
    let ty = it.next().unwrap_or("").to_string();
    let unit = it.next().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());

    if ty.is_empty() {
        return Err(format!("column '{raw}' has an empty type"));
    }

    Ok(Column {
        raw: raw.to_string(),
        name: name.trim().to_string(),
        ty,
        optional,
        pk: flags.contains('*'),
        unit,
        ref_to,
        default,
        flags,
    })
}

fn parse_manifest_line(line: &str, m: &mut Manifest) {
    // '# key: value'
    let body = line.trim_start_matches('#').trim();
    if let Some((k, v)) = body.split_once(':') {
        let k = k.trim().to_string();
        let v = v.trim().to_string();
        match k.as_str() {
            "sheet" => m.sheet = v.clone(),
            "version" => m.version = v.clone(),
            "generator" => m.generator = v.clone(),
            "target" => m.target = v.clone(),
            "index" => m.index = v.clone(),
            "requires" => {
                m.requires = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "-").collect()
            }
            "owned_by" => m.owned_by = v.clone(),
            "doctrine" => m.doctrine = v.clone(),
            "evidence" => m.evidence_required = v == "required",
            "id_form" => m.id_compound = v == "compound",
            "emit" => m.emit_rust = v == "rust",
            "emit_port" => m.emit_port = v == "rust" || v == "true",
            "port_from" => {
                m.port_from = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "-").collect()
            }
            "key_alias" => {
                m.key_alias = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "-").collect()
            }
            "source_rows" => m.source_rows = v.parse().ok(),
            "dropped" => m.dropped = v.parse().ok(),
            "dropped_reason" => m.dropped_reason = v.clone(),
            _ => {}
        }
        m.fields.insert(k, v);
    }
}

/// Load one sheet file. Returns the sheet plus any L0 findings raised while parsing.
pub fn load_sheet(path: &Path, sheets_dir: &Path) -> (Option<Sheet>, Vec<Finding>) {
    let rel = path
        .strip_prefix(sheets_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut findings = Vec::new();

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            findings.push(Finding::err("E-L0-IO", &rel, 0, "", "", format!("cannot read: {e}")));
            return (None, findings);
        }
    };

    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        findings.push(Finding::err("E-L0-BOM", &rel, 1, "", "", "file begins with a UTF-8 BOM (D7: no BOM)"));
    }
    let text = match String::from_utf8(bytes.clone()) {
        Ok(t) => t,
        Err(e) => {
            findings.push(Finding::err("E-L0-ENC", &rel, 0, "", "", format!("not valid UTF-8: {e}")));
            return (None, findings);
        }
    };
    if text.contains('\r') {
        let line = text.lines().position(|l| l.contains('\r')).map(|i| i + 1).unwrap_or(1);
        findings.push(Finding::err("E-L0-EOL", &rel, line, "", "", "CR found: canonical form is LF only (D7)"));
    }

    let mut manifest = Manifest::default();
    let mut columns: Option<Vec<Column>> = None;
    let mut rows = Vec::new();

    for (idx, raw_line) in text.split('\n').enumerate() {
        let lineno = idx + 1;
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if columns.is_none() {
                parse_manifest_line(trimmed, &mut manifest);
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if columns.is_none() {
            // header row
            if !line.contains('\t') {
                findings.push(Finding::err("E-L0-DELIM", &rel, lineno, "", "", "header row has no TAB: not a TSV sheet"));
            }
            let mut cols = Vec::new();
            for (ci, cell) in line.split('\t').enumerate() {
                match parse_column(cell) {
                    Ok(c) => cols.push(c),
                    Err(e) => {
                        findings.push(Finding::err("E-L0-HEADER", &rel, lineno, "", &format!("col{}", ci + 1), e));
                        cols.push(Column { raw: cell.to_string(), name: format!("col{}", ci + 1), ty: "string".into(), optional: true, pk: false, unit: None, ref_to: None, default: None, flags: String::new() });
                    }
                }
            }
            columns = Some(cols);
            continue;
        }
        let cols = columns.as_ref().unwrap();
        let mut cells: Vec<String> = line.split('\t').map(|s| s.to_string()).collect();
        let mut short = false;
        let mut quoted = false;
        if cells.len() > cols.len() {
            findings.push(Finding::err(
                "E-L0-WIDE",
                &rel,
                lineno,
                "",
                "",
                format!("row has {} cells but the header declares {}", cells.len(), cols.len()),
            ));
            cells.truncate(cols.len());
        } else if cells.len() < cols.len() {
            // dec004: trailing empty cells may be omitted
            short = true;
            cells.resize(cols.len(), String::new());
        }
        for c in &cells {
            if c.starts_with('"') {
                quoted = true;
            }
        }
        rows.push(Row { line: lineno, cells, short, quoted });
    }

    let columns = match columns {
        Some(c) if !c.is_empty() => c,
        _ => {
            findings.push(Finding::err("E-L0-NOHEADER", &rel, 0, "", "", "no column header row found"));
            return (None, findings);
        }
    };

    if manifest.sheet.is_empty() {
        findings.push(Finding::err("E-L0-MANIFEST", &rel, 1, "", "", "missing manifest: no '# sheet:' line"));
        manifest.sheet = rel.trim_end_matches(".tsv").replace('/', "-");
    }
    if manifest.requires.iter().any(|r| r == &manifest.sheet) {
        findings.push(Finding::warn("W-L0-SELFREQ", &manifest.sheet, 0, "", "", "manifest requires itself"));
    }

    let sheet = Sheet { path: path.to_path_buf(), rel, name: manifest.sheet.clone(), manifest, columns, rows };
    (Some(sheet), findings)
}

/// Walk `dir` recursively and load every `*.tsv`.
pub fn load_book(sheets_dir: &Path) -> (Vec<Sheet>, Vec<Finding>) {
    let mut files = Vec::new();
    collect_tsv(sheets_dir, &mut files);
    files.sort();
    let mut sheets = Vec::new();
    let mut findings = Vec::new();
    for f in files {
        let (s, mut fs_) = load_sheet(&f, sheets_dir);
        findings.append(&mut fs_);
        if let Some(s) = s {
            sheets.push(s);
        }
    }
    (sheets, findings)
}

fn collect_tsv(dir: &Path, out: &mut Vec<PathBuf>) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_tsv(&p, out);
        } else if p.extension().map(|x| x.eq_ignore_ascii_case("tsv")).unwrap_or(false) {
            out.push(p);
        }
    }
}

// ---------------------------------------------------------------------------
// L0 / L1 / L2 checks
// ---------------------------------------------------------------------------

fn is_valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Declaration-permitted exception: a sheet whose manifest says `id_form: compound`
/// uses '.' to join a multi-part key (see decisions.tsv dec009).
fn is_valid_compound_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

fn check_number_form(ty: &str, v: &str) -> Result<(), String> {
    match ty {
        "u8" | "u16" | "u32" | "u64" => {
            if !v.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("'{v}' is not an unsigned integer"));
            }
            let n: u64 = v.parse().map_err(|_| format!("'{v}' is not an unsigned integer"))?;
            let max = match ty {
                "u8" => u8::MAX as u64,
                "u16" => u16::MAX as u64,
                "u32" => u32::MAX as u64,
                _ => u64::MAX,
            };
            if n > max {
                return Err(format!("{n} overflows {ty}"));
            }
            Ok(())
        }
        "i8" | "i16" | "i32" | "i64" => {
            let body = v.strip_prefix('-').unwrap_or(v);
            if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("'{v}' is not an integer"));
            }
            let n: i64 = v.parse().map_err(|_| format!("'{v}' is not an integer"))?;
            let (min, max) = match ty {
                "i8" => (i8::MIN as i64, i8::MAX as i64),
                "i16" => (i16::MIN as i64, i16::MAX as i64),
                "i32" => (i32::MIN as i64, i32::MAX as i64),
                _ => (i64::MIN, i64::MAX),
            };
            if n < min || n > max {
                return Err(format!("{n} overflows {ty}"));
            }
            Ok(())
        }
        "f32" | "f64" => {
            if !v.contains('.') && !v.contains('e') && !v.contains('E') {
                return Err(format!("'{v}' is an integer written in a float column (D7 number form)"));
            }
            if v.parse::<f64>().is_err() {
                return Err(format!("'{v}' is not a float"));
            }
            Ok(())
        }
        "bool" => {
            if v == "0" || v == "1" {
                Ok(())
            } else {
                Err(format!("'{v}' is not 0 or 1"))
            }
        }
        _ => Ok(()),
    }
}

fn check_cell(ty: &str, v: &str) -> Result<(), String> {
    // NULL is a legal explicit-none marker in any column (MDD 5.2)
    if v == "NULL" {
        return Ok(());
    }
    if let Some(name) = ty.strip_prefix("enum:") {
        if name.is_empty() {
            return Err("enum type without a name".into());
        }
        let ok = {
            let mut ch = v.chars();
            match ch.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => ch.all(|c| c.is_ascii_alphanumeric() || c == '_'),
                _ => false,
            }
        };
        return if ok { Ok(()) } else { Err(format!("'{v}' is not a valid {ty} variant")) };
    }
    if let Some(inner) = ty.strip_prefix("vec2") {
        let _ = inner;
        let parts: Vec<&str> = v.split_whitespace().collect();
        return if parts.len() == 2 && parts.iter().all(|p| p.parse::<f64>().is_ok()) {
            Ok(())
        } else {
            Err(format!("'{v}' is not a vec2"))
        };
    }
    if ty == "vec3" || ty == "vec4" {
        let n = if ty == "vec3" { 3 } else { 4 };
        let parts: Vec<&str> = v.split_whitespace().collect();
        return if parts.len() == n && parts.iter().all(|p| p.parse::<f64>().is_ok()) {
            Ok(())
        } else {
            Err(format!("'{v}' is not a {ty}"))
        };
    }
    if let Some(inner) = ty.strip_prefix("range:") {
        let (a, b) = v.split_once("..").ok_or_else(|| format!("'{v}' is not a range (min..max)"))?;
        check_cell(inner, a.trim())?;
        check_cell(inner, b.trim())?;
        return Ok(());
    }
    if let Some(inner) = ty.strip_prefix("list:") {
        for p in v.split_whitespace() {
            check_cell(inner, p)?;
        }
        return Ok(());
    }
    check_number_form(ty, v)
}

fn known_type(ty: &str) -> bool {
    matches!(
        ty,
        "string" | "bool" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" | "fx16" | "fx32" | "vec2" | "vec3" | "vec4" | "quat" | "color"
    ) || ty.starts_with("enum:")
        || ty.starts_with("ref:")
        || ty.starts_with("asset:")
        || ty.starts_with("list:")
        || ty.starts_with("range:")
        || ty.starts_with("flags:")
}

/// L0: structure. L1: types. L2: references + cycles.
pub fn validate(sheets: &[Sheet], base: &mut Vec<Finding>) {
    let doctrine = sheets.iter().find(|s| s.name == "00-doctrine" || s.rel.ends_with("00-doctrine.tsv"));
    let emitter_version = doctrine
        .and_then(|d| d.rows.iter().find(|r| d.cell(r, "key") == Some("emitter_version")))
        .and_then(|r| doctrine.unwrap().cell(r, "value").map(|v| v.to_string()));

    // global keyspace: sheet name -> set of pk values
    let mut keyspace: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in sheets {
        let mut set = BTreeSet::new();
        if let Some(_pk) = s.pk_index() {
            for r in &s.rows {
                set.insert(s.row_key(r));
            }
        }
        keyspace.insert(s.name.clone(), set);
        // also index by file stem so refs can use either
        keyspace.insert(s.rel.trim_end_matches(".tsv").replace('/', "-"), s.rows.iter().map(|r| s.row_key(r)).collect());
    }

    for s in sheets {
        // manifest / emitter version agreement (L0)
        if !emitter_version.as_deref().map(|v| v == s.manifest.version).unwrap_or(true) {
            base.push(Finding::err(
                "E-L0-EMITTER",
                &s.name,
                0,
                "",
                "",
                format!("manifest version '{}' != doctrine emitter_version '{}'", s.manifest.version, emitter_version.clone().unwrap_or_default()),
            ));
        }

        // header vs schema drift (L0) -- against 01-schema when present
        // id presence, uniqueness, charset (L0)
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for r in &s.rows {
            if r.short {
                base.push(Finding::warn("W-L0-SHORT", &s.name, r.line, "", "", "short row padded to header width (dec004)"));
            }
            if r.quoted {
                base.push(Finding::warn("W-L1-QUOTED", &s.name, r.line, "", "", "quoted cell present (token and merge hazard)"));
            }
            let key = s.row_key(r);
            if key.is_empty() || key == "NULL" {
                base.push(Finding::err("E-L0-KEY", &s.name, r.line, "", "", "empty primary key"));
            } else if !(is_valid_id(&key) || (s.manifest.id_compound && is_valid_compound_id(&key))) {
                base.push(Finding::err(
                    "E-L0-CHARSET",
                    &s.name,
                    r.line,
                    &key,
                    "",
                    format!("id '{key}' is not [a-z0-9_]+ (D7)"),
                ));
            }
            if !seen.insert(key.clone()) {
                base.push(Finding::err("E-L0-DUPKEY", &s.name, r.line, &key, "", "duplicate primary key"));
            }
        }

        // L1
        for r in &s.rows {
            for (ci, c) in s.columns.iter().enumerate() {
                let v = r.cells.get(ci).map(|s| s.as_str()).unwrap_or("");
                if !known_type(&c.ty) {
                    base.push(Finding::err("E-L1-TYPE", &s.name, r.line, &s.row_key(r), &c.name, format!("unknown declared type '{}'", c.ty)));
                    continue;
                }
                if v.is_empty() {
                    // MDD 5.2: an empty cell inherits the column's schema default.
                    // Whether that default is itself valid is a property of the
                    // column, not of the row, so it is checked once per column
                    // above; reporting it per row would inflate the finding count.
                    if c.default.is_some() {
                        continue;
                    }
                    if c.optional {
                        continue;
                    }
                    base.push(Finding::err("E-L1-EMPTY", &s.name, r.line, &s.row_key(r), &c.name, "empty cell in a required column"));
                    continue;
                }
                if let Err(e) = check_cell(&c.ty, v) {
                    base.push(Finding::err("E-L1-VALUE", &s.name, r.line, &s.row_key(r), &c.name, e));
                }
            }
        }

        // L2 foreign keys
        for (ci, c) in s.columns.iter().enumerate() {
            let (target_sheet, _target_col) = match &c.ref_to {
                Some(t) => t.clone(),
                None => continue,
            };
            let target_keys = match keyspace.get(&target_sheet) {
                Some(k) => k,
                None => {
                    base.push(Finding::err("E-L2-REF", &s.name, 0, "", &c.name, format!("reference target sheet '{target_sheet}' is not in the book")));
                    continue;
                }
            };
            for r in &s.rows {
                let v = r.cells.get(ci).map(|s| s.as_str()).unwrap_or("");
                if v.is_empty() || v == "-" || v == "NULL" {
                    continue;
                }
                if !target_keys.contains(v) {
                    base.push(Finding::err(
                        "E-L2-REF",
                        &s.name,
                        r.line,
                        &s.row_key(r),
                        &c.name,
                        format!("'{v}' not found in {target_sheet}"),
                    ));
                }
            }
        }

        // L2 Mode B evidence rule (MDD 8.9 / O16)
        if s.manifest.evidence_required {
            let idx = |n: &str| s.col_index(n);
            if let (Some(ia), Some(ic), Some(ie)) = (idx("ref_addr"), idx("ref_conf"), idx("evidence")) {
                for r in &s.rows {
                    let addr = r.cells.get(ia).map(|s| s.as_str()).unwrap_or("");
                    let conf = r.cells.get(ic).map(|s| s.as_str()).unwrap_or("");
                    let ev = r.cells.get(ie).map(|s| s.as_str()).unwrap_or("");
                    if addr.is_empty() || addr == "-" {
                        base.push(Finding::err("E-L2-NOEVIDENCE", &s.name, r.line, &s.row_key(r), "ref_addr", "Mode B row has no ref_addr"));
                    }
                    if !matches!(conf, "certain" | "probable" | "speculative") {
                        base.push(Finding::err("E-L2-NOEVIDENCE", &s.name, r.line, &s.row_key(r), "ref_conf", format!("ref_conf '{conf}' not in {{certain,probable,speculative}}")));
                    }
                    if ev.is_empty() || ev == "-" {
                        base.push(Finding::err("E-L2-NOEVIDENCE", &s.name, r.line, &s.row_key(r), "evidence", "Mode B row has no evidence"));
                    }
                }
            }
        }
    }

    // L2 cycle detection over manifest `requires` edges + column refs
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in sheets {
        let e = adj.entry(s.name.clone()).or_default();
        for r in &s.manifest.requires {
            e.insert(r.clone());
        }
        for c in &s.columns {
            if let Some((t, _)) = &c.ref_to {
                e.insert(t.clone());
            }
        }
    }
    let mut color: BTreeMap<String, u8> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for node in adj.keys() {
        dfs_cycle(node, &adj, &mut color, &mut stack, base, &mut reported);
    }

    // kernel allowlist completeness (L0, check 21)
    check_kernel(sheets, base);

    // header vs authoritative schema (L0, check 5)
    drift_check(sheets, base);

    // unit consistency across sheets (L1, check 9)
    check_units(sheets, base);

    // context packs must fit their declared budgets (L6, check 25)
    check_budgets(sheets, base);

    // evidence must reconcile with the producer (L5, check 24)
    check_provenance(sheets, base);

    // declared defaults must themselves be valid values for their column (L1,
    // check 11). Once per column, not once per row.
    for s in sheets {
        for c in &s.columns {
            if let Some(d) = &c.default {
                if let Err(e) = check_cell(&c.ty, d) {
                    base.push(Finding::err(
                        "E-L1-DEFAULT",
                        &s.name,
                        0,
                        "",
                        &c.name,
                        format!("declared default '{d}' does not parse as '{}': {e}", c.ty),
                    ));
                }
            }
        }
    }

    // topological order must exist (L2, check 15)
    if let Err(stuck) = topo_order(sheets) {
        base.push(Finding::err(
            "E-L2-TOPO",
            "book",
            0,
            "",
            "",
            format!("no topological order exists; {} sheet(s) are in cycles: {}", stuck.len(), stuck.join(", ")),
        ));
    }

    // L0 check 13: the same key must not live in two different sheets by
    // accident. Some sheets deliberately share a key space (a 1:1 annotation
    // sheet keyed by its subject's id); those declare `key_alias:` and are
    // exempt, so the rule fires on mistakes rather than on intended design.
    let mut owner: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for s in sheets {
        let aliases: Vec<&str> = s.manifest.key_alias.iter().map(|a| a.as_str()).collect();
        for r in &s.rows {
            let k = s.row_key(r);
            match owner.get(&k) {
                Some((prev, _prevline)) => {
                    let exempt = prev == &s.name
                        || aliases.contains(&prev.as_str())
                        || sheets
                            .iter()
                            .find(|x| &x.name == prev)
                            .map(|x| x.manifest.key_alias.iter().any(|a| a == &s.name))
                            .unwrap_or(false);
                    if !exempt {
                        base.push(Finding::warn(
                            "W-L0-CROSSKEY",
                            &s.name,
                            r.line,
                            &k,
                            "",
                            format!("key '{k}' already appears in sheet '{prev}' (check 13); declare `key_alias: {prev}` if that is intended"),
                        ));
                    }
                }
                None => {
                    owner.insert(k, (s.name.clone(), r.line));
                }
            }
        }
    }
}

/// L1 check 9: the same column name must not carry two different units.
///
/// MDD 5.3 says a header unit must match the unit declared in 01-schema.tsv. In
/// this project the schema is bootstrapped from the headers, so that comparison
/// alone can never fire. The rule that *can* fire, and that actually catches the
/// bug the MDD is worried about, is cross-sheet disagreement: `speed: f32 m/s` in
/// one sheet and `speed: f32 kph` in another is the contradiction worth failing.
fn check_units(sheets: &[Sheet], out: &mut Vec<Finding>) {
    let mut map: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for s in sheets {
        for c in &s.columns {
            if let Some(u) = &c.unit {
                map.entry(c.name.clone()).or_default().push((s.name.clone(), u.clone()));
            }
        }
    }
    for (name, defs) in map {
        let units: BTreeSet<&str> = defs.iter().map(|d| d.1.as_str()).collect();
        if units.len() > 1 {
            let detail = defs
                .iter()
                .map(|(s, u)| format!("{s}={u}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Finding::err(
                "E-L1-UNIT",
                &defs[0].0,
                0,
                "",
                &name,
                format!("column '{name}' declares conflicting units across sheets: {detail}"),
            ));
        }
    }
}

/// L2 check 15: a topological order over the reference graph, via Kahn's
/// algorithm. An Err carries the sheets that could not be ordered.
pub fn topo_order(sheets: &[Sheet]) -> Result<Vec<String>, Vec<String>> {
    let names: BTreeSet<String> = sheets.iter().map(|s| s.name.clone()).collect();
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in sheets {
        let e = deps.entry(s.name.clone()).or_default();
        for r in &s.manifest.requires {
            if names.contains(r) {
                e.insert(r.clone());
            }
        }
        for c in &s.columns {
            if let Some((t, _)) = &c.ref_to {
                if names.contains(t) {
                    e.insert(t.clone());
                }
            }
        }
    }

    // if A depends on B, then B must come first: edge B -> A
    let mut succ: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indeg: BTreeMap<String, usize> = names.iter().map(|n| (n.clone(), 0)).collect();
    for (a, ds) in &deps {
        for b in ds {
            if b == a {
                continue;
            }
            if succ.entry(b.clone()).or_default().insert(a.clone()) {
                *indeg.get_mut(a).unwrap() += 1;
            }
        }
    }

    let mut ready: BTreeSet<String> = indeg.iter().filter(|(_, &d)| d == 0).map(|(n, _)| n.clone()).collect();
    let mut order: Vec<String> = Vec::new();
    while let Some(n) = ready.iter().next().cloned() {
        ready.remove(&n);
        order.push(n.clone());
        if let Some(ss) = succ.get(&n).cloned() {
            for s in ss {
                if let Some(d) = indeg.get_mut(&s) {
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(s);
                    }
                }
            }
        }
    }

    if order.len() == names.len() {
        Ok(order)
    } else {
        let done: BTreeSet<String> = order.into_iter().collect();
        Err(names.difference(&done).cloned().collect())
    }
}

/// The repository root implied by a sheets directory.
///
/// `Path::new("sheets").parent()` is `Some("")`, an empty path, not the current
/// directory. Rules that walk out of the sheet book (the kernel allowlist, the
/// generated-file scan, the git row-count baseline) therefore silently did
/// nothing under the default `--sheets sheets` - they passed by not running.
/// That is the exact failure mode this method exists to prevent, so the empty
/// parent is normalised explicitly.
pub fn book_root(sheets_dir: &Path) -> PathBuf {
    match sheets_dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// FNV-1a 64. Used for the prefix hash. Any stable hash works here; what matters
/// is that the identity of the cached prefix is checkable and recorded (MDD
/// 11.2), so a silent change to it is detectable rather than merely expensive.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Rough token estimate for a byte count. ~4 bytes/token is the usual heuristic
/// for tab-separated ASCII; it is an estimate and the report says so, because a
/// budget enforced by a fabricated precise number would be worse than no budget.
fn est_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

pub struct PackReport {
    pub view: String,
    pub sheet: String,
    pub columns: Vec<String>,
    pub window: String,
    pub window_rows: usize,
    pub total_rows: usize,
    pub body_bytes: usize,
    pub body_tokens: usize,
    pub prefix_bytes: usize,
    pub prefix_tokens: usize,
    pub budget: usize,
    pub prefix_hash: String,
    pub over_budget: bool,
}

/// Parse a row window: `*` for everything, or `A..B` inclusive and 1-based.
/// Returns a half-open range clamped to the sheet.
pub fn parse_window(spec: &str, total: usize) -> Result<(usize, usize), String> {
    let s = spec.trim();
    if s.is_empty() || s == "*" {
        return Ok((0, total));
    }
    let (a, b) = s
        .split_once("..")
        .ok_or_else(|| format!("row window '{s}' must be '*' or 'A..B'"))?;
    let lo: usize = a.trim().parse().map_err(|_| format!("row window '{s}': '{a}' is not a number"))?;
    let hi: usize = b.trim().parse().map_err(|_| format!("row window '{s}': '{b}' is not a number"))?;
    if lo == 0 || hi < lo {
        return Err(format!("row window '{s}' is not a valid 1-based inclusive range"));
    }
    let start = (lo - 1).min(total);
    let end = hi.min(total);
    Ok((start, end.max(start)))
}

pub fn build_pack(sheets: &[Sheet], view_name: &str) -> Result<(String, PackReport), String> {
    build_pack_opt(sheets, view_name, None)
}

/// `rows_override` lets a caller ask for a different window than the declared one
/// (`sheetty pack v_methods --rows 901..1800`), which is the practical way to walk
/// a sheet too large to load at once.
pub fn build_pack_opt(sheets: &[Sheet], view_name: &str, rows_override: Option<&str>) -> Result<(String, PackReport), String> {
    let views = sheets
        .iter()
        .find(|s| s.name == "views")
        .ok_or_else(|| "no views sheet: declare views in sheets/views.tsv (MDD 11.3)".to_string())?;
    let vrow = views
        .rows
        .iter()
        .find(|r| views.row_key(r) == view_name)
        .ok_or_else(|| format!("view '{view_name}' is not declared in sheets/views.tsv"))?;

    let sheet_name = views.cell(vrow, "sheet").unwrap_or("").to_string();
    let cols_spec = views.cell(vrow, "columns").unwrap_or("*").to_string();
    let budget: usize = views.cell(vrow, "token_budget").and_then(|v| v.parse().ok()).unwrap_or(0);

    let sheet = sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| format!("view '{view_name}' names sheet '{sheet_name}', which is not in the book"))?;

    let cols: Vec<String> = if cols_spec.trim() == "*" {
        sheet.columns.iter().map(|c| c.name.clone()).collect()
    } else {
        cols_spec.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
    };
    for c in &cols {
        if sheet.col_index(c).is_none() {
            return Err(format!("view '{view_name}' names column '{c}', which '{sheet_name}' does not declare"));
        }
    }

    // [1] + [2] the stable prefix
    let mut prefix = String::new();
    if let Some(d) = sheets.iter().find(|s| s.name == "00-doctrine") {
        prefix.push_str("# [1] DOCTRINE\n");
        for r in &d.rows {
            let _ = writeln!(prefix, "{}", r.cells.join("\t"));
        }
    }
    prefix.push_str("# [2] SCHEMA\n");
    let _ = writeln!(prefix, "{}", cols.join("\t"));
    for c in &cols {
        let ci = sheet.col_index(c).unwrap();
        let col = &sheet.columns[ci];
        let _ = writeln!(
            prefix,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            c,
            col.ty,
            col.unit.clone().unwrap_or_else(|| "-".into()),
            if col.optional { "yes" } else { "no" },
            col.ref_to.as_ref().map(|(a, b)| format!("{a}.{b}")).unwrap_or_else(|| "-".into()),
            col.default.clone().unwrap_or_else(|| "-".into()),
            col.flags
        );
    }

    // [3] the volatile part
    let window_spec = rows_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| views.cell(vrow, "row_window").unwrap_or("*").to_string());
    let (lo, hi) = parse_window(&window_spec, sheet.rows.len())?;

    let mut body = String::new();
    body.push_str("# [3] TARGET ROWS\n");
    let _ = writeln!(body, "{}", cols.join("\t"));
    for r in &sheet.rows[lo..hi] {
        let cells: Vec<String> = cols
            .iter()
            .map(|c| sheet.cell(r, c).unwrap_or("").to_string())
            .collect();
        let _ = writeln!(body, "{}", cells.join("\t"));
    }

    let prefix_bytes = prefix.len();
    let body_bytes = body.len();
    let body_tokens = est_tokens(body_bytes);
    let hash = fnv1a64(prefix.as_bytes());
    let report = PackReport {
        view: view_name.to_string(),
        sheet: sheet_name,
        columns: cols.clone(),
        window: window_spec,
        window_rows: hi - lo,
        total_rows: sheet.rows.len(),
        body_bytes,
        body_tokens,
        prefix_bytes,
        prefix_tokens: est_tokens(prefix_bytes),
        budget,
        prefix_hash: format!("{hash:016x}"),
        over_budget: budget > 0 && body_tokens > budget,
    };

    let full = format!("{prefix}{body}");
    Ok((full, report))
}

/// L6 check 25: every declared view must fit its token budget.
pub fn check_budgets(sheets: &[Sheet], out: &mut Vec<Finding>) {
    let views = match sheets.iter().find(|s| s.name == "views") {
        Some(v) => v,
        None => return,
    };
    for r in &views.rows {
        let name = views.row_key(r);
        match build_pack(sheets, &name) {
            Ok((_txt, rep)) => {
                if rep.over_budget {
                    out.push(Finding::err(
                        "E-L6-BUDGET",
                        "views",
                        r.line,
                        &name,
                        "token_budget",
                        format!(
                            "view '{name}' projects {} of {} rows (window '{}') to ~{} tokens, over its budget of {} (MDD 11.3: narrow the view, window the rows, or raise the budget deliberately)",
                            rep.window_rows, rep.total_rows, rep.window, rep.body_tokens, rep.budget
                        ),
                    ));
                }
            }
            Err(e) => out.push(Finding::warn("W-L6-VIEW", "views", r.line, &name, "", e)),
        }
    }
}
/// L5 check 24: the evidence must reconcile with what the producer emitted.
///
/// This is the cheapest possible guard for the most damaging Mode B failure:
/// MDD risk R6, "truncated extraction: a lost export silently halves the sheet".
/// A synthesizer records how many records it saw (`source_rows`) and how many it
/// deliberately did not keep (`dropped`, with a reason). The two must add up to
/// the row count. Row counts alone cannot catch this, because a synthesizer that
/// silently loses rows also reports the smaller count with a straight face; it
/// is the *sum* that has to balance against an independently recorded total.
pub fn check_provenance(sheets: &[Sheet], out: &mut Vec<Finding>) {
    for s in sheets {
        let src = match s.manifest.source_rows {
            Some(n) => n,
            None => continue,
        };
        let dropped = s.manifest.dropped.unwrap_or(0);
        let rows = s.rows.len();
        if rows + dropped != src {
            out.push(Finding::err(
                "E-L5-DIVERGE",
                &s.name,
                0,
                "",
                "",
                format!(
                    "provenance does not balance: source_rows={src}, rows={rows}, dropped={dropped} (rows + dropped = {}). {} Evidence was lost or double-counted; fix the synthesizer, do not edit the total.",
                    rows + dropped,
                    if s.manifest.dropped_reason.is_empty() { "" } else { "Recorded reason:" }
                ),
            ));
            if !s.manifest.dropped_reason.is_empty() {
                out.push(Finding::warn(
                    "W-L5-DROPPED",
                    &s.name,
                    0,
                    "",
                    "",
                    format!("{} record(s) dropped: {}", dropped, s.manifest.dropped_reason),
                ));
            }
        }
    }
}

/// L7 check 28: row count sanity against the last commit.
///
/// A sheet that halves between commits is the signature of a truncated export
/// (MDD R6) or an over-eager filter. This shells out to git for the baseline and
/// degrades silently when there is none - a missing baseline is not a defect.
pub fn check_row_counts(sheets: &[Sheet], root: &Path, out: &mut Vec<Finding>) {
    for s in sheets {
        let committed = match committed_row_count(root, &s.rel) {
            Some(n) if n >= 20 => n,
            _ => continue,
        };
        let current = s.rows.len();
        let ratio_pct = current * 100 / committed;
        if ratio_pct < 80 || ratio_pct > 200 {
            out.push(Finding::warn(
                "W-L7-ROWCOUNT",
                &s.name,
                0,
                "",
                "",
                format!(
                    "row count moved from {committed} (HEAD) to {current} ({ratio_pct}% of the committed count); confirm this is intended and not a truncated export (MDD check 28 / risk R6)"
                ),
            ));
        }
    }
}

/// Count data rows in the committed version of a sheet.
fn committed_row_count(root: &Path, rel: &str) -> Option<usize> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(format!("HEAD:sheets/{rel}"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut data = 0usize;
    let mut header_seen = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
            continue;
        }
        if !header_seen {
            header_seen = true;
            continue;
        }
        data += 1;
    }
    Some(data)
}

/// L0 check 5: the data-sheet header row is a courtesy; the authoritative
/// declaration is `01-schema.tsv`. Two copies that can drift is how projects
/// rot, so drift is a hard error rather than a warning (MDD 5.3).
pub fn drift_check(sheets: &[Sheet], out: &mut Vec<Finding>) {
    let schema_sheet = match sheets.iter().find(|s| s.name == "01-schema" || s.rel.ends_with("01-schema.tsv")) {
        Some(s) => s,
        None => {
            out.push(Finding::err("E-L0-NOSCHEMA", "01-schema", 0, "", "", "authoritative schema 01-schema.tsv is missing (D3)"));
            return;
        }
    };
    let mut declared: BTreeMap<(String, String), (String, usize)> = BTreeMap::new();
    for r in &schema_sheet.rows {
        let sh = schema_sheet.cell(r, "sheet").unwrap_or("").to_string();
        let col = schema_sheet.cell(r, "column").unwrap_or("").to_string();
        let ty = schema_sheet.cell(r, "type").unwrap_or("").to_string();
        if sh.is_empty() || col.is_empty() {
            continue;
        }
        declared.insert((sh, col), (ty, r.line));
    }
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for s in sheets {
        if s.name == schema_sheet.name {
            continue;
        }
        for c in &s.columns {
            let key = (s.name.clone(), c.name.clone());
            match declared.get(&key) {
                Some((ty, line)) => {
                    seen.insert(key.clone());
                    if ty != &c.ty {
                        out.push(Finding::err(
                            "E-SCHEMA-DRIFT",
                            &s.name,
                            0,
                            "",
                            &c.name,
                            format!("header type '{}' != schema type '{}' (01-schema.tsv:{line})", c.ty, ty),
                        ));
                    }
                }
                None => out.push(Finding::err(
                    "E-SCHEMA-MISSING",
                    &s.name,
                    0,
                    "",
                    &c.name,
                    format!("column '{}' of sheet '{}' is not declared in 01-schema.tsv", c.name, s.name),
                )),
            }
        }
    }
    for (k, _) in declared.iter() {
        if !seen.contains(k) {
            out.push(Finding::err(
                "E-SCHEMA-EXTRA",
                &k.0,
                0,
                "",
                &k.1,
                "declared in 01-schema.tsv but absent from the sheet header",
            ));
        }
    }
}

/// Emit the authoritative `01-schema.tsv`. The `view` and `notes` columns are
/// schema-only curation that does NOT live in the sheet headers, so an existing
/// file's values are preserved rather than reset. Without that, re-bootstrapping
/// would silently destroy every view assignment, which is exactly the kind of
/// quiet loss the method exists to prevent.
pub fn schema_tsv(sheets: &[Sheet]) -> String {
    let mut curated: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    if let Some(e) = sheets.iter().find(|s| s.name == "01-schema" || s.rel.ends_with("01-schema.tsv")) {
        for r in &e.rows {
            let sh = e.cell(r, "sheet").unwrap_or("").to_string();
            let col = e.cell(r, "column").unwrap_or("").to_string();
            if sh.is_empty() || col.is_empty() {
                continue;
            }
            let vw = e.cell(r, "view").unwrap_or("v_full").to_string();
            let nt = e.cell(r, "notes").unwrap_or("-").to_string();
            curated.insert((sh, col), (vw, nt));
        }
    }

    let mut out = String::new();
    out.push_str("# sheet: 01-schema\n# version: 1\n# generator: schema\n# target: sheets/01-schema.tsv\n# index: by_sheet_col\n# requires: -\n# owned_by: architect\n# doctrine: D1,D3\n# id_form: compound\n");
    out.push_str("# Authoritative column declarations. The header row of each data sheet is a\n# courtesy; preflight compares the two and fails on drift (E-SCHEMA-DRIFT).\n# The view and notes columns are curation and survive regeneration.\n");
    out.push_str("id:string*\tsheet:string\tcolumn:string\ttype:string\tunit:string\toptional:string\tref:string\tdefault:string\tview:string\tnotes:string?\n");
    for s in sheets {
        if s.name == "01-schema" {
            continue;
        }
        for c in &s.columns {
            let sid = format!("{}", s.name).replace(['-', '/'], "_");
            let id = format!("{sid}.{}", c.name);
            let unit = c.unit.clone().unwrap_or_else(|| "-".into());
            let opt = if c.optional { "yes" } else { "no" };
            let refv = c.ref_to.as_ref().map(|(a, b)| format!("{a}.{b}")).unwrap_or_else(|| "-".into());
            let def = c.default.clone().unwrap_or_else(|| "-".into());
            let (view, notes) = curated
                .get(&(s.name.clone(), c.name.clone()))
                .cloned()
                .unwrap_or_else(|| ("v_full".to_string(), "-".to_string()));
            out.push_str(&format!("{id}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{view}\t{notes}\n", s.name, c.name, c.ty, unit, opt, refv, def));
        }
    }
    out
}

fn dfs_cycle(
    node: &str,
    adj: &BTreeMap<String, BTreeSet<String>>,
    color: &mut BTreeMap<String, u8>,
    stack: &mut Vec<String>,
    out: &mut Vec<Finding>,
    reported: &mut BTreeSet<String>,
) {
    let c = color.get(node).copied().unwrap_or(0);
    if c == 2 {
        return;
    }
    if c == 1 {
        return;
    }
    color.insert(node.to_string(), 1);
    stack.push(node.to_string());
    if let Some(neigh) = adj.get(node) {
        for n in neigh {
            let nc = color.get(n).copied().unwrap_or(0);
            if nc == 1 {
                if reported.insert(n.clone()) {
                    let start = stack.iter().position(|x| x == n).unwrap_or(0);
                    let cyc: Vec<String> = stack[start..].to_vec();
                    out.push(Finding::err(
                        "E-L2-CYCLE",
                        node,
                        0,
                        "",
                        "",
                        format!("reference cycle: {} -> {}", cyc.join(" -> "), n),
                    ));
                }
            } else {
                dfs_cycle(n, adj, color, stack, out, reported);
            }
        }
    }
    stack.pop();
    color.insert(node.to_string(), 2);
}

fn check_kernel(sheets: &[Sheet], out: &mut Vec<Finding>) {
    let kernel = match sheets.iter().find(|s| s.name == "kernel") {
        Some(k) => k,
        None => return,
    };
    let root = book_root(&kernel.path.parent().unwrap_or(Path::new(".")));
    let kdir = root.join("kernel");
    let mut on_disk: BTreeSet<String> = BTreeSet::new();
    if kdir.is_dir() {
        collect_rs(&kdir, &mut on_disk);
    }
    // module id on disk: kernel/<relative path> with .rs and trailing /mod removed
    let mut disk_ids: BTreeSet<String> = BTreeSet::new();
    for f in &on_disk {
        if let Some(rest) = f.split("/kernel/").nth(1) {
            let id = rest.trim_end_matches(".rs").trim_end_matches("/mod");
            disk_ids.insert(id.replace('/', "::"));
        }
    }
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for r in &kernel.rows {
        let m = match kernel.cell(r, "rust_module") {
            Some(m) if m != "-" && !m.is_empty() => m,
            _ => continue,
        };
        listed.insert(m.trim_start_matches("kernel::").to_string());
        if let Some(t) = kernel.cell(r, "tests") {
            if t != "-" && !t.is_empty() && !root.join(t).exists() {
                out.push(Finding::err(
                    "E-L0-KERNEL",
                    &kernel.name,
                    r.line,
                    &kernel.row_key(r),
                    "tests",
                    format!("kernel test path '{t}' does not exist (the allowlist must not be a rubber stamp)"),
                ));
            }
        }
    }
    for id in &disk_ids {
        if !listed.contains(id) {
            out.push(Finding::err(
                "E-L0-KERNEL",
                &kernel.name,
                0,
                "",
                "rust_module",
                format!("kernel module '{id}' exists on disk but is not listed in kernel.tsv (D4)"),
            ));
        }
    }
    for id in &listed {
        if !disk_ids.is_empty() && !disk_ids.contains(id) {
            out.push(Finding::err(
                "E-L0-KERNEL",
                &kernel.name,
                0,
                "",
                "rust_module",
                format!("kernel module '{id}' is listed but not found under kernel/"),
            ));
        }
    }
}

fn collect_rs(dir: &Path, out: &mut BTreeSet<String>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rs(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.insert(p.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// L3 overlap
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Overlap {
    pub spec: String,
    pub imp: String,
    pub covered: Vec<String>,
    pub unimplemented: Vec<String>,
    pub orphan: Vec<String>,
    pub divergent: Vec<(String, String, String, String)>,
}

impl Overlap {
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (self.covered.len(), self.unimplemented.len(), self.orphan.len(), self.divergent.len())
    }
}

/// Columns that carry provenance or bookkeeping rather than domain data. They are
/// excluded from the divergence comparison, because two relations over the same
/// domain legitimately disagree about *where* a value came from while agreeing
/// about the value. Comparing them made every shared row of re/client/types and
/// re/server/types look divergent (artifact paths differ by platform prefix),
/// which reported "covered 0" for 1,546 identical types.
///
/// This is the MDD 5.4 reserved-column set minus `id`, which is the join key.
const RESERVED_COLS: &[&str] = &[
    "kind", "status", "artifact", "ref_src", "ref_addr", "ref_conf", "evidence", "notes", "tok",
];

pub fn overlap(a: &Sheet, b: &Sheet) -> Overlap {
    let akeys: BTreeSet<String> = a.rows.iter().map(|r| a.row_key(r)).collect();
    let bkeys: BTreeSet<String> = b.rows.iter().map(|r| b.row_key(r)).collect();
    let mut o = Overlap { spec: a.name.clone(), imp: b.name.clone(), ..Default::default() };

    let shared: Vec<&Column> = a
        .columns
        .iter()
        .filter(|c| {
            b.col_index(&c.name).is_some() && c.name != "id" && !RESERVED_COLS.contains(&c.name.as_str())
        })
        .collect();

    for k in &akeys {
        if bkeys.contains(k) {
            let ra = a.rows.iter().find(|r| &a.row_key(r) == k).unwrap();
            let rb = b.rows.iter().find(|r| &b.row_key(r) == k).unwrap();
            let mut diverged = false;
            for c in &shared {
                let va = a.cell(ra, &c.name).unwrap_or("");
                let vb = b.cell(rb, &c.name).unwrap_or("");
                if !va.is_empty() && !vb.is_empty() && va != vb {
                    o.divergent.push((k.clone(), c.name.clone(), va.to_string(), vb.to_string()));
                    diverged = true;
                }
            }
            if !diverged {
                o.covered.push(k.clone());
            }
        } else {
            o.unimplemented.push(k.clone());
        }
    }
    for k in &bkeys {
        if !akeys.contains(k) {
            o.orphan.push(k.clone());
        }
    }
    o
}

/// Rank unimplemented rows by blast radius: inbound foreign-key references from
/// every other sheet plus a weight derived from the spec row's priority.
pub fn rank_blast_radius(sheets: &[Sheet], target_sheet: &Sheet, missing: &[String]) -> Vec<(String, usize, u8)> {
    let mut inbound: BTreeMap<String, usize> = BTreeMap::new();
    for s in sheets {
        if s.name == target_sheet.name {
            continue;
        }
        for c in &s.columns {
            if let Some((t, _)) = &c.ref_to {
                if t == &target_sheet.name {
                    let ci = s.col_index(&c.name).unwrap();
                    for r in &s.rows {
                        if let Some(v) = r.cells.get(ci) {
                            if !v.is_empty() {
                                *inbound.entry(v.clone()).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let mut out: Vec<(String, usize, u8)> = missing
        .iter()
        .map(|id| {
            let prio = target_sheet
                .rows
                .iter()
                .find(|r| &target_sheet.row_key(r) == id)
                .and_then(|r| target_sheet.cell(r, "priority"))
                .and_then(|p| p.parse::<u8>().ok())
                .unwrap_or(9);
            (id.clone(), *inbound.get(id).unwrap_or(&0), prio)
        })
        .collect();
    out.sort_by(|x, y| y.1.cmp(&x.1).then(x.2.cmp(&y.2)).then(x.0.cmp(&y.0)));
    out
}

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------

pub struct Summary {
    pub sheets: usize,
    pub rows: usize,
    pub columns: usize,
    pub errors: usize,
    pub warnings: usize,
}

pub fn summarize(sheets: &[Sheet], findings: &[Finding]) -> Summary {
    Summary {
        sheets: sheets.len(),
        rows: sheets.iter().map(|s| s.rows.len()).sum(),
        columns: sheets.iter().map(|s| s.columns.len()).sum(),
        errors: findings.iter().filter(|f| f.is_error()).count(),
        warnings: findings.iter().filter(|f| !f.is_error()).count(),
    }
}

pub fn human_report(sheets: &[Sheet], findings: &[Finding], overlaps: &[Overlap]) -> String {
    let mut out = String::new();
    let s = summarize(sheets, findings);
    let _ = writeln!(out, "PREFLIGHT  sheets/  {} sheets, {} rows, {} columns", s.sheets, s.rows, s.columns);
    let _ = writeln!(out, "  emitter {EMITTER_VERSION}");
    let (ri, rp, ru, ra) = rules_summary();
    let total = RULES.len();
    let _ = writeln!(
        out,
        "  rule coverage  {}/{} MDD checks exist (implemented {ri}, partial {rp}, unexercised {ru}, absent {ra})",
        ri + rp + ru,
        total
    );
    let _ = writeln!(out, "  (run `sheetty rules` for the per-check status; an absent rule is not a passing one)");
    let _ = writeln!(out);

    for layer in ["L0", "L1", "L2", "L3"] {
        let mut shown = 0usize;
        let mut total = 0usize;
        for f in findings {
            if f.code.starts_with(&format!("E-{layer}")) || f.code.starts_with(&format!("W-{layer}")) {
                total += 1;
                if shown < 12 {
                    let sev = if f.is_error() { "E" } else { "W" };
                    let _ = writeln!(out, "      {sev} {}", f.code);
                    let _ = writeln!(
                        out,
                        "        {}:{}  {}{}{}  {}",
                        f.sheet,
                        f.line,
                        f.row,
                        if f.col.is_empty() { "" } else { " " },
                        f.col,
                        f.message
                    );
                    shown += 1;
                }
            }
        }
        let name = match layer {
            "L0" => "L0 structural",
            "L1" => "L1 types",
            "L2" => "L2 refs",
            _ => "L3 coverage",
        };
        if total == 0 && layer != "L3" {
            let _ = writeln!(out, "  {name} ................ ok");
        } else if layer != "L3" {
            let _ = writeln!(out, "  {name} .............. {total} finding(s)");
            if total > shown {
                let _ = writeln!(out, "      ... +{} more", total - shown);
            }
        }
    }

    // Findings outside L0-L3 (L4-L7 rules such as E-L6-BUDGET) must still be
    // visible. A finding that counts toward the total but appears in no group is
    // worse than useless: it makes the report fail without saying why.
    let mut others: Vec<&Finding> = findings
        .iter()
        .filter(|f| {
            let rest = f.code.get(2..).unwrap_or("");
            !["L0", "L1", "L2", "L3"].iter().any(|l| rest.starts_with(l))
        })
        .collect();
    if !others.is_empty() {
        others.sort_by(|a, b| a.code.cmp(&b.code));
        let _ = writeln!(out, "  L4-L7 other .............. {} finding(s)", others.len());
        for f in others.iter().take(12) {
            let sev = if f.is_error() { "E" } else { "W" };
            let _ = writeln!(out, "      {sev} {}", f.code);
            let _ = writeln!(out, "        {}:{}  {}{}{}", f.sheet, f.line, f.col, if f.col.is_empty() { "" } else { " " }, f.message);
        }
        if others.len() > 12 {
            let _ = writeln!(out, "      ... +{} more", others.len() - 12);
        }
    }

    // A compact inventory of every code present. The per-layer sections above cap
    // their detail at 12 findings each, so without this a run with 100 errors can
    // mention 12 and hide 88 - and a hidden finding is exactly the silent failure
    // this method exists to prevent. This summary is cheap and shows nothing
    // silently. (A test harness should still read --json.)
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    for f in findings {
        *by_code.entry(f.code.as_str()).or_insert(0) += 1;
    }
    if !by_code.is_empty() {
        let mut pairs: Vec<(&str, usize)> = by_code.into_iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let _ = writeln!(out);
        let _ = writeln!(out, "  findings by code ({} distinct):", pairs.len());
        for (code, n) in pairs {
            let _ = writeln!(out, "      {code:<22} {n}");
        }
    }

    for o in overlaps {
        let (c, u, or, d) = o.counts();
        let _ = writeln!(out);
        let _ = writeln!(out, "  L3 overlap {} x {}", o.spec, o.imp);
        let _ = writeln!(out, "      covered {c}  unimplemented {u}  orphan {or}  divergent {d}");
        if !o.unimplemented.is_empty() {
            let head: Vec<&str> = o.unimplemented.iter().take(10).map(|s| s.as_str()).collect();
            let _ = writeln!(out, "      unimplemented: {}{}", head.join(", "), if u > head.len() { format!(" ... (+{})", u - head.len()) } else { String::new() });
        }
        if !o.orphan.is_empty() {
            let head: Vec<&str> = o.orphan.iter().take(10).map(|s| s.as_str()).collect();
            let _ = writeln!(out, "      orphan: {}{}", head.join(", "), if or > head.len() { format!(" ... (+{})", or - head.len()) } else { String::new() });
        }
        for (k, c2, a, b) in o.divergent.iter().take(5) {
            let _ = writeln!(out, "      divergent {k}.{c2}: spec='{a}' impl='{b}'");
        }
    }

    let _ = writeln!(out);
    if s.errors == 0 {
        let _ = writeln!(out, "OK  {} warnings", s.warnings);
    } else {
        let _ = writeln!(out, "FAILED  {} errors, {} warnings", s.errors, s.warnings);
    }
    out
}

pub fn json_report(sheets: &[Sheet], findings: &[Finding], overlaps: &[Overlap]) -> String {
    let s = summarize(sheets, findings);
    let mut out = String::new();
    let (ri, rp, ru, ra) = rules_summary();
    let _ = write!(
        out,
        "{{\"emitter\":\"{EMITTER_VERSION}\",\"sheets\":{},\"rows\":{},\"columns\":{},\"errors\":{},\"warnings\":{},\"rule_coverage\":{{\"implemented\":{ri},\"partial\":{rp},\"unexercised\":{ru},\"absent\":{ra}}},\"findings\":[",
        s.sheets, s.rows, s.columns, s.errors, s.warnings
    );
    for (i, f) in findings.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"code\":\"{}\",\"severity\":\"{}\",\"sheet\":\"{}\",\"line\":{},\"row\":\"{}\",\"col\":\"{}\",\"message\":\"{}\"}}",
            jesc(&f.code),
            if f.is_error() { "E" } else { "W" },
            jesc(&f.sheet),
            f.line,
            jesc(&f.row),
            jesc(&f.col),
            jesc(&f.message)
        );
    }
    let _ = write!(out, "],\"overlaps\":[");
    for (i, o) in overlaps.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let (c, u, or, d) = o.counts();
        let _ = write!(out, "{{\"spec\":\"{}\",\"impl\":\"{}\",\"covered\":{c},\"unimplemented\":{u},\"orphan\":{or},\"divergent\":{d},", jesc(&o.spec), jesc(&o.imp));
        let _ = write!(out, "\"unimplemented_ids\":[{}],\"orphan_ids\":[{}]}}", arr(&o.unimplemented), arr(&o.orphan));
    }
    let _ = write!(out, "]}}");
    out
}

fn arr(v: &[String]) -> String {
    v.iter().map(|s| format!("\"{}\"", jesc(s))).collect::<Vec<_>>().join(",")
}

pub fn jesc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o
}

/// Determine which layers to run given `--layer` ("" = all).
pub fn run_layers(layer: &str) -> (bool, bool, bool, bool) {
    match layer {
        "L0" => (true, false, false, false),
        "L1" => (true, true, false, false),
        "L2" => (true, true, true, false),
        "L3" => (true, true, true, true),
        _ => (true, true, true, true),
    }
}

// ---------------------------------------------------------------------------
// rule coverage self-audit
// ---------------------------------------------------------------------------
//
// The MDD's own instrumentation rule: "never hand-maintain a number that can be
// derived. A hand-maintained metric is a metric that lies." The most dangerous
// lie this engine could tell is "preflight passed", when in fact only a subset
// of the method's checks exist. So the set of checks is enumerated here as data,
// with an honest status for each, and the preflight report states its own
// coverage. A rule that does not exist must never be implied by silence.
//
// status values:
//   implemented  the rule runs and has been seen to fire
//   partial      the rule runs but covers less than the MDD requires
//   unexercised  the rule runs but has never had anything to check
//   absent       the rule does not exist in this engine

pub struct RuleCheck {
    pub n: &'static str,
    pub name: &'static str,
    pub status: &'static str,
    pub emits: &'static str,
    pub note: &'static str,
}

pub const RULES: &[RuleCheck] = &[
    RuleCheck { n: "1", name: "UTF-8, no BOM",         status: "implemented", emits: "E-L0-BOM",       note: "" },
    RuleCheck { n: "2", name: "LF line endings only",  status: "implemented", emits: "E-L0-EOL",       note: "git core.autocrlf must stay input or this fires on every row" },
    RuleCheck { n: "3", name: "TAB delimited",         status: "implemented", emits: "E-L0-DELIM",     note: "" },
    RuleCheck { n: "4", name: "Manifest parses",       status: "implemented", emits: "E-L0-MANIFEST",  note: "" },
    RuleCheck { n: "5", name: "Header matches schema", status: "implemented", emits: "E-SCHEMA-DRIFT", note: "compares name and type only" },
    RuleCheck { n: "6", name: "Key present, unique",   status: "implemented", emits: "E-L0-KEY,E-L0-DUPKEY", note: "" },
    RuleCheck { n: "7", name: "id charset",            status: "implemented", emits: "E-L0-CHARSET",   note: "relaxed to allow '.' when a sheet declares id_form: compound (dec009)" },
    RuleCheck { n: "8", name: "Every cell type-checks", status: "implemented", emits: "E-L1-VALUE",    note: "" },
    RuleCheck { n: "9", name: "Units consistent",       status: "implemented", emits: "E-L1-UNIT",     note: "cross-sheet: the same column name must not carry two different units" },
    RuleCheck { n: "10", name: "Enum domain",          status: "partial",     emits: "E-L1-VALUE",    note: "checks identifier form only; the declared variant set is not consulted" },
    RuleCheck { n: "11", name: "Empty/NULL, defaults", status: "implemented", emits: "E-L1-EMPTY,E-L1-DEFAULT", note: "empty-in-required fails; NULL is explicit; a declared default must itself parse as the column type" },
    RuleCheck { n: "12", name: "Every FK resolves",    status: "implemented", emits: "E-L2-REF",       note: "" },
    RuleCheck { n: "13", name: "No duplicate rows across sheets", status: "implemented", emits: "W-L0-CROSSKEY", note: "sheets that intentionally share a key space declare `key_alias:` and are exempt" },
    RuleCheck { n: "14", name: "No cycles",            status: "implemented", emits: "E-L2-CYCLE",     note: "DFS over manifest requires plus column refs" },
    RuleCheck { n: "15", name: "Topological order exists", status: "implemented", emits: "E-L2-TOPO",   note: "Kahn over manifest requires plus column refs; `sheetty order` prints it" },
    RuleCheck { n: "16", name: "No unimplemented rows", status: "partial",    emits: "L3 overlap",     note: "reported, not gating; --strict gates warnings only" },
    RuleCheck { n: "17", name: "No orphan rows",       status: "partial",     emits: "L3 overlap",     note: "reported, never fails" },
    RuleCheck { n: "18", name: "No divergent rows",    status: "implemented", emits: "L3 overlap",     note: "attribute compare over shared columns" },
    RuleCheck { n: "19", name: "Every asset exists",   status: "partial",     emits: "re/assets status=found|missing", note: "done by the synthesizer at extraction time, not by sheetty" },
    RuleCheck { n: "20", name: "Asset hash matches baseline", status: "partial", emits: "re/assets sha256", note: "every present asset now carries a real sha256; drift against a previous run is not yet automated" },
    RuleCheck { n: "21", name: "Kernel allowlist complete", status: "unexercised", emits: "E-L0-KERNEL", note: "implemented both ways; kernel/ is empty so it has never had anything to check" },
    RuleCheck { n: "22", name: "No generated file in tree", status: "implemented", emits: "E-L0-GENERATED", note: "scans for the emitter's banner outside target/ and .git (D6)" },
    RuleCheck { n: "23", name: "No hand edits to generated files", status: "partial", emits: "E-L0-GENERATED", note: "covered indirectly: generated files cannot exist in the tree at all; there is no hash comparison inside OUT_DIR" },
    RuleCheck { n: "24", name: "Divergence vs evidence (L5)", status: "implemented", emits: "E-L5-DIVERGE", note: "source_rows must equal rows + dropped, so a silent export loss cannot balance" },
    RuleCheck { n: "25", name: "Context packs within budget (L6)", status: "implemented", emits: "E-L6-BUDGET", note: "every view in sheets/views.tsv is assembled and measured on every preflight" },
    RuleCheck { n: "26", name: "Dead columns (L6)",    status: "absent",      emits: "",               note: "the emitter is generic and consumes every column, so a dead-column rule cannot fire yet" },
    RuleCheck { n: "27", name: "Emitter determinism (L7)", status: "implemented", emits: "emit --verify-determinism", note: "re-emits into a clean directory and byte-compares; also the per-file content-hash gate" },
    RuleCheck { n: "28", name: "Row count sanity",     status: "implemented", emits: "W-L7-ROWCOUNT", note: "compares each sheet's row count to the committed one via git; degrades silently when there is no baseline" },
    RuleCheck { n: "+",  name: "Mode B evidence on every row (MDD 8.9)", status: "implemented", emits: "E-L2-NOEVIDENCE", note: "applies to sheets whose manifest says evidence: required" },
    RuleCheck { n: "+",  name: "Emitter version agreement", status: "implemented", emits: "E-L0-EMITTER", note: "manifest version vs doctrine emitter_version" },
];

/// (implemented, partial, unexercised, absent)
pub fn rules_summary() -> (usize, usize, usize, usize) {
    let mut a = (0, 0, 0, 0);
    for r in RULES {
        match r.status {
            "implemented" => a.0 += 1,
            "partial" => a.1 += 1,
            "unexercised" => a.2 += 1,
            _ => a.3 += 1,
        }
    }
    a
}

pub fn rules_report() -> String {
    let mut out = String::new();
    let (i, p, u, a) = rules_summary();
    let total = RULES.len();
    let enforced = i + p + u;
    let _ = writeln!(
        out,
        "PREFLIGHT RULE COVERAGE  {enforced} of {total} MDD checks exist in this engine\n  implemented {i}   partial {p}   unexercised {u}   absent {a}\n\n  {:<3} {:<38} {:<12} {:<24} {}",
        "#", "check", "status", "emits", "note"
    );
    for r in RULES {
        let _ = writeln!(out, "  {:<3} {:<38} {:<12} {:<24} {}", r.n, r.name, r.status, r.emits, r.note);
    }
    out
}

// ---------------------------------------------------------------------------
// the emit stage (MDD 5.1, 5.6, 5.9)
// ---------------------------------------------------------------------------
//
// "Code volume is decoupled from token cost. The agent authors rows, not code."
// This is where that claim is cashed: one strut per row, projected mechanically.
//
// Doctrine enforced here:
//   D2  one row, one strut
//   D5  emit is impossible to invoke when validate has failed
//   D6  every generated file carries a banner naming its source sheet
//   5.9 one file per sheet, content-hash gated, so a no-op write never touches
//       mtime and incremental compilation survives

pub struct EmitReport {
    pub sheet: String,
    pub module: String,
    pub path: String,
    pub rows: usize,
    pub bytes: usize,
    pub written: bool,
}

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "async", "await", "abstract", "become",
    "box", "do", "final", "macro", "override", "priv", "try", "typeof", "unsized",
    "virtual", "yield",
];

/// `type` is a Rust keyword and `type` is a real column in re/*/methods, so raw
/// identifiers are not optional here.
fn field_ident(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// `02-plan` -> `plan`, `re/client/types` -> `re_client_types`. The numeric prefix is
/// stripped because a Rust module cannot start with a digit.
pub fn module_name(sheet_name: &str) -> String {
    let mut s = sheet_name.replace(['/', '-'], "_");
    if let Some(i) = s.find('_') {
        if s[..i].chars().all(|c| c.is_ascii_digit()) {
            s = s[i + 1..].to_string();
        }
    } else if s.chars().all(|c| c.is_ascii_digit()) {
        s = format!("s{s}");
    }
    let s: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        format!("s_{s}")
    } else {
        s
    }
}

/// MDD Appendix B: the Rust type a declared sheet type projects to.
/// Types with no faithful projection yet (enums, refs, assets, lists) fall back
/// to their identifier as a string, and that fallback is visible rather than
/// silent because the field type changes in the generated output.
pub fn rust_type(ty: &str) -> String {
    match ty {
        "string" => "&'static str".to_string(),
        "bool" => "bool".to_string(),
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64" => ty.to_string(),
        "vec2" => "[f32; 2]".to_string(),
        "vec3" => "[f32; 3]".to_string(),
        "vec4" => "[f32; 4]".to_string(),
        _ => "&'static str".to_string(),
    }
}

fn rust_literal(ty: &str, v: &str) -> String {
    let t = rust_type(ty);
    if t == "&'static str" {
        return format!("{v:?}");
    }
    // an unset cell in a non-string column projects to the type's zero
    if v.is_empty() || v == "NULL" || v == "-" {
        return match t.as_str() {
            "bool" => "false".to_string(),
            "[f32; 2]" => "[0.0, 0.0]".to_string(),
            "[f32; 3]" => "[0.0, 0.0, 0.0]".to_string(),
            "[f32; 4]" => "[0.0, 0.0, 0.0, 0.0]".to_string(),
            _ => "0".to_string(),
        };
    }
    if v == "1" && t == "bool" {
        return "true".to_string();
    }
    if v == "0" && t == "bool" {
        return "false".to_string();
    }
    if t.starts_with('[') {
        let parts: Vec<String> = v
            .split_whitespace()
            .map(|p| if p.contains('.') { p.to_string() } else { format!("{p}.0") })
            .collect();
        return format!("[{}]", parts.join(", "));
    }
    if t == "f32" || t == "f64" {
        return if v.contains('.') || v.contains('e') || v.contains('E') { v.to_string() } else { format!("{v}.0") };
    }
    v.to_string()
}

pub fn emit_sheet_source(sheet: &Sheet) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// GENERATED BY sheetty v{EMITTER_VERSION} FROM {} - DO NOT EDIT",
        sheet.name
    );
    let _ = writeln!(out, "// source:   sheets/{}", sheet.rel);
    let _ = writeln!(out, "// rows:     {}", sheet.rows.len());
    let _ = writeln!(out, "// columns:  {}", sheet.columns.len());
    let _ = writeln!(out, "//");
    let _ = writeln!(out, "// Hand edits are lost on the next build (MDD D6). Change the sheet instead.");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "/// One strut per row of the '{}' sheet (MDD D2: one row, one strut).",
        sheet.name
    );
    let _ = writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq)]");
    let _ = writeln!(out, "pub struct Def {{");
    for c in &sheet.columns {
        let _ = writeln!(out, "    pub {}: {},", field_ident(&c.name), rust_type(&c.ty));
    }
    let _ = writeln!(out, "}}\n");

    // sorted by primary key so the lookup below is a binary search
    let pk = sheet.pk_index().unwrap_or(0);
    let pkname = sheet.columns.get(pk).map(|c| field_ident(&c.name)).unwrap_or_else(|| "id".to_string());
    let pk_is_str = sheet.columns.get(pk).map(|c| rust_type(&c.ty) == "&'static str").unwrap_or(true);
    let mut rows: Vec<&Row> = sheet.rows.iter().collect();
    rows.sort_by(|a, b| {
        let ka = a.cells.get(pk).map(|s| s.as_str()).unwrap_or("");
        let kb = b.cells.get(pk).map(|s| s.as_str()).unwrap_or("");
        ka.cmp(kb)
    });

    let _ = writeln!(out, "pub const ALL: &[Def] = &[");
    for r in &rows {
        let _ = writeln!(out, "    Def {{");
        for (i, c) in sheet.columns.iter().enumerate() {
            let v = r.cells.get(i).map(|x| x.as_str()).unwrap_or("");
            let _ = writeln!(out, "        {}: {},", field_ident(&c.name), rust_literal(&c.ty, v));
        }
        let _ = writeln!(out, "    }},");
    }
    let _ = writeln!(out, "];\n");
    let _ = writeln!(out, "pub const COUNT: usize = {};\n", rows.len());

    if pk_is_str {
        let _ = writeln!(out, "/// Rows are emitted sorted by `{pkname}`, so this is a binary search.");
        let _ = writeln!(out, "pub fn by_id(id: &str) -> Option<&'static Def> {{");
        let _ = writeln!(out, "    ALL.binary_search_by(|d| d.{pkname}.cmp(id)).ok().map(|i| &ALL[i])");
        let _ = writeln!(out, "}}");
    }
    out
}

pub fn emit_registry_source(sheets: &[Sheet]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "// GENERATED BY sheetty v{EMITTER_VERSION} FROM the sheet book - DO NOT EDIT");
    let _ = writeln!(out, "//");
    let _ = writeln!(out, "// MDD 5.9: this is the fan-out file, so it depends on every sheet and will");
    let _ = writeln!(out, "// always rebuild. It therefore carries identifiers and lengths only, never");
    let _ = writeln!(out, "// data bodies, to keep its recompilation cheap.");
    let _ = writeln!(out);
    let _ = writeln!(out, "pub struct SheetInfo {{");
    let _ = writeln!(out, "    pub name: &'static str,");
    let _ = writeln!(out, "    /// generated module name, or \"-\" when the sheet is not emitted");
    let _ = writeln!(out, "    pub module: &'static str,");
    let _ = writeln!(out, "    pub rows: usize,");
    let _ = writeln!(out, "    pub columns: usize,");
    let _ = writeln!(out, "}}\n");
    let _ = writeln!(out, "pub const SHEETS: &[SheetInfo] = &[");
    for s in sheets {
        let m = if s.manifest.emit_rust { module_name(&s.name) } else { "-".to_string() };
        let _ = writeln!(
            out,
            "    SheetInfo {{ name: {:?}, module: {:?}, rows: {}, columns: {} }},",
            s.name,
            m,
            s.rows.len(),
            s.columns.len()
        );
    }
    let _ = writeln!(out, "];\n");
    let _ = writeln!(out, "pub const SHEET_COUNT: usize = {};", sheets.len());
    let _ = writeln!(out, "pub const TOTAL_ROWS: usize = {};\n", sheets.iter().map(|s| s.rows.len()).sum::<usize>());
    let _ = writeln!(out, "pub fn rows_in(sheet: &str) -> Option<usize> {{");
    let _ = writeln!(out, "    SHEETS.iter().find(|s| s.name == sheet).map(|s| s.rows)");
    let _ = writeln!(out, "}}");
    out
}

/// Write only when the bytes differ. A no-op write still moves mtime, and cargo
/// and rustc key incremental work off mtime, so a no-op write is not free
/// (MDD 5.9 / optimization O2).
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// the port projection (MDD D2: one row, one strut)
//
// `re/<platform>/types`, `re/<platform>/fields` and `re/<platform>/methods` are
// three relations, and a Rust item is a JOIN of them: the type row is the item,
// its field rows are the struct body in DECLARATION order, and its method rows
// are the `impl`. Nothing is written back to the sheets, so the sheets remain the
// source of truth (D1).
//
// The projection is conservative in one direction on purpose: anything the
// binary REFERENCES but does not DECLARE - the BCL, XNA, `IntPtr`, a delegate's
// signature - resolves to a generated placeholder rather than being guessed at.
// So the module always compiles, and the unresolved surface is visible and
// countable instead of silently absent.
// ---------------------------------------------------------------------------

struct PortField {
    kind: String,
    name: String,
    ty: String,
    value: String,
    /// Kept because it is what distinguishes a static member from an instance one;
    /// the current projection puts both in an `impl`, so it is not read yet.
    #[allow(dead_code)]
    modifiers: String,
}

struct PortMethod {
    name: String,
    ret: String,
    params: String,
    modifiers: String,
    kind: String,
}

struct PortType {
    id: String,
    kind: String,
    /// The C# namespace, superseded by `mod_path` once the module tree is built.
    #[allow(dead_code)]
    ns: String,
    name: String,
    base_: String,
    fields: Vec<PortField>,
    methods: Vec<PortMethod>,
    mod_path: Vec<String>,
    item: String,
}

pub struct PortReport {
    pub module: String,
    pub path: String,
    pub types: usize,
    pub fields: usize,
    pub methods: usize,
    pub externs: usize,
    pub gaps: usize,
    pub bytes: usize,
    pub written: bool,
}

/// A Rust identifier for a namespace segment. Lowercased, because a namespace is
/// a module and a module that shares a name with a type in the same scope would
/// shadow it.
fn port_module(seg: &str) -> String {
    let mut s: String = seg
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() || s == "_" {
        s = "m_unset".to_string();
    }
    if s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        s = format!("m{s}");
    }
    // `crate`, `self`, `super` and `Self` cannot be raw identifiers at all, so a
    // namespace with one of those names needs a different name rather than `r#`.
    if matches!(s.as_str(), "crate" | "self" | "super" | "Self") {
        s = format!("{s}_");
    } else if RUST_KEYWORDS.contains(&s.as_str()) {
        s = format!("r#{s}");
    }
    s
}

/// A Rust identifier for an item or a member. Case is PRESERVED: a C# type name
/// is PascalCase and a field name may be `_serverIP`, and rewriting either would
/// make the port harder to read against the source it came from. The generated
/// module allows the resulting non-snake-case lint.
fn port_ident(s: &str) -> String {
    let mut t: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    // `_` is reserved and cannot name anything, and a name that sanitized away to
    // nothing has to become something.
    if t.is_empty() || t == "_" || t.trim_matches('_').is_empty() {
        t = format!("{t}unnamed");
    }
    if t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        t = format!("_{t}");
    }
    if matches!(t.as_str(), "crate" | "self" | "super" | "Self") {
        t = format!("{t}_");
    } else if RUST_KEYWORDS.contains(&t.as_str()) {
        t = format!("r#{t}");
    }
    t
}

/// Where a C# type name went. `Local` is a path into this module; `Extern` is a
/// placeholder for something not declared here.
struct Resolver {
    by_id: BTreeMap<String, (Vec<String>, String)>,
    by_name: BTreeMap<String, Vec<String>>,
    /// id -> kind, so a resolved INTERFACE can be boxed where it is used as a type
    kinds: BTreeMap<String, String>,
    /// The final Rust item name chosen for each (name, arity) extern, so the
    /// reference and the definition agree even when the naive arity-encoded name
    /// collides with another extern (`Foo` at arity 1 vs a C# type `Foo_a1`).
    extern_items: BTreeMap<(String, usize), String>,
    /// Extern item names already handed out; seeded with the hard-coded `Object`.
    extern_used: BTreeSet<String>,
    /// The name of the generated `externs` module. Normally `externs`, but a
    /// namespace or type at the port root can sanitize to the same name, so it is
    /// disambiguated against the real module tree before any path is built.
    externs_mod: String,
    ambiguous: BTreeSet<String>,
    gaps: BTreeSet<String>,
}

impl Resolver {
    fn path_of(&self, id: &str) -> Option<String> {
        self.by_id.get(id).map(|(segs, item)| {
            let mut p = String::from("crate::port");
            for s in segs {
                p.push_str("::");
                p.push_str(s);
            }
            p.push_str("::");
            p.push_str(item);
            p
        })
    }

    /// The Rust path for a C# type expression, recursing through generics, arrays
    /// and nullables. `ctx` is the id of the type that mentions it, which is what
    /// lets `Settings` inside `Terraria.Player` resolve to `Terraria.Player.Settings`
    /// rather than to some other `Settings`.
    fn map(&mut self, expr: &str, ctx: &str, depth: u32) -> String {
        if depth > 8 {
            return self.placeholder("RecursionGuard", 0, &[]);
        }
        let mut t = expr.trim();
        for pre in ["params ", "this ", "ref ", "out ", "in ", "scoped "] {
            if let Some(rest) = t.strip_prefix(pre) {
                t = rest.trim();
            }
        }
        if t.is_empty() {
            return "()".to_string();
        }
        if t == "void" || t == "Void" {
            return "()".to_string();
        }
        // `T[]` and `T[,]` both become a vector; the rank is lost, which is a
        // recorded gap rather than a silent lie
        if let Some(inner) = t.strip_suffix("[]") {
            return format!("Vec<{}>", self.map(inner, ctx, depth + 1));
        }
        if let Some(inner) = t.strip_suffix("[,]") {
            self.gaps.insert(format!("array rank 2: {t}"));
            return format!("Vec<{}>", self.map(inner, ctx, depth + 1));
        }
        if let Some(inner) = t.strip_suffix('*') {
            return format!("*mut {}", self.map(inner, ctx, depth + 1));
        }
        if let Some(inner) = t.strip_suffix('?') {
            return format!("Option<{}>", self.map(inner, ctx, depth + 1));
        }
        if let Some(rest) = t.strip_prefix("global::") {
            t = rest;
        }

        // `System.Object` is always present as the first extern; its path must use
        // the same (possibly disambiguated) module name as every other extern.
        if t == "object" || t == "Object" {
            return format!("crate::port::{}::Object", self.externs_mod);
        }

        let prim = match t {
            "bool" | "Boolean" => Some("bool"),
            "byte" | "Byte" => Some("u8"),
            "sbyte" | "SByte" => Some("i8"),
            "short" | "Int16" => Some("i16"),
            "ushort" | "UInt16" => Some("u16"),
            "int" | "Int32" => Some("i32"),
            "uint" | "UInt32" => Some("u32"),
            "long" | "Int64" => Some("i64"),
            "ulong" | "UInt64" => Some("u64"),
            "float" | "Single" => Some("f32"),
            "double" | "Double" => Some("f64"),
            "decimal" => Some("f64"),
            "char" | "Char" => Some("char"),
            "string" | "String" => Some("String"),
            "IntPtr" => Some("isize"),
            "UIntPtr" => Some("usize"),
            "nint" => Some("isize"),
            "nuint" => Some("usize"),
            "nfdresult_t" => Some("i32"),
            _ => None,
        };
        if let Some(p) = prim {
            return p.to_string();
        }

        // generic: split the head from the arguments at the OUTERMOST '<'
        if let Some(open) = t.find('<') {
            if t.ends_with('>') {
                let head = t[..open].trim();
                let inner = &t[open + 1..t.len() - 1];
                let args: Vec<String> = split_type_args(inner);
                let mapped: Vec<String> = args.iter().map(|a| self.map(a, ctx, depth + 1)).collect();
                let bare = head.rsplit('.').next().unwrap_or(head);
                return match bare {
                    "List" | "IList" | "IEnumerable" | "ICollection" | "IReadOnlyList"
                    | "IReadOnlyCollection" | "Queue" | "Stack" => format!("Vec<{}>", mapped.join(", ")),
                    "HashSet" | "ISet" => format!("std::collections::BTreeSet<{}>", mapped.join(", ")),
                    "Dictionary" | "IDictionary" | "IReadOnlyDictionary" | "SortedDictionary"
                    | "SortedList" => format!("std::collections::BTreeMap<{}>", mapped.join(", ")),
                    "Nullable" => format!("Option<{}>", mapped.join(", ")),
                    "KeyValuePair" => format!("({})", mapped.join(", ")),
                    "Action" => format!("Box<dyn Fn({})>", mapped.join(", ")),
                    "Func" if mapped.len() >= 2 => {
                        let r = mapped.last().cloned().unwrap_or_default();
                        format!("Box<dyn Fn({}) -> {}>", mapped[..mapped.len() - 1].join(", "), r)
                    }
                    "Func" => format!("Box<dyn Fn() -> {}>", mapped.join(", ")),
                    "Predicate" => format!("Box<dyn Fn({}) -> bool>", mapped.join(", ")),
                    "Comparison" => format!("Box<dyn Fn({}) -> i32>", mapped.join(", ")),
                    "ValueTuple" | "Tuple" => format!("({})", mapped.join(", ")),
                    _ => self.placeholder(&port_ident(bare), mapped.len(), &mapped),
                };
            }
        }

        if t == "Action" {
            return "Box<dyn Fn()>".to_string();
        }

        // a named type: prefer the one in the context's own namespace, then a
        // unique match, then the first match in sorted order (deterministic), and
        // record it when the choice was a guess
        let bare = t.rsplit('.').next().unwrap_or(t).to_string();
        let lower = bare.to_lowercase();
        let ctx_ns = ctx.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default();
        let own = format!("{ctx_ns}.{lower}");
        if let Some(p) = self.local_as_type(own.trim_start_matches('.')) {
            return p;
        }
        let candidates = self.by_name.get(&lower).cloned().unwrap_or_default();
        if candidates.len() == 1 {
            if let Some(p) = self.local_as_type(&candidates[0]) {
                return p;
            }
        }
        if candidates.len() > 1 {
            // an inner-most match wins; anything else is a recorded ambiguity
            let mut best: Option<String> = None;
            for c in &candidates {
                if c.starts_with(&ctx_ns) {
                    best = Some(c.clone());
                    break;
                }
            }
            if let Some(b) = best {
                if let Some(p) = self.local_as_type(&b) {
                    return p;
                }
            }
            self.ambiguous.insert(bare.clone());
            if let Some(p) = self.local_as_type(&candidates[0]) {
                return p;
            }
        }
        self.placeholder(&port_ident(&bare), 0, &[])
    }

    /// A local type used as a TYPE (a field, a parameter, a return). An interface
    /// is a trait in Rust and cannot be a bare type, so it is boxed; the alternative
    /// is 136 `expected a type, found a trait` errors.
    fn local_as_type(&self, id: &str) -> Option<String> {
        let p = self.path_of(id)?;
        if self.kinds.get(id).map(|k| k == "interface").unwrap_or(false) {
            Some(format!("Box<dyn {p}>"))
        } else {
            Some(p)
        }
    }

    /// Record a reference to something the binary does not declare, and return the
    /// path to its placeholder.
    ///
    /// Arity is part of the NAME (`Foo` is arity 0, `Foo_a2` is arity 2) rather
    /// than a suffix applied only when a name is used at several arities. That
    /// matters because the name has to be known at REFERENCE time, while the set
    /// of arities a name is used at is only known after every reference has been
    /// seen. The arity-encoded name is a HINT, not a guarantee: a C# type literally
    /// named `Foo_a1` collides with `Foo` at arity 1, so the first allocation of a
    /// name is recorded and every later reference reuses it, and a genuine clash is
    /// disambiguated deterministically rather than emitted as a duplicate item.
    fn placeholder(&mut self, name: &str, arity: usize, args: &[String]) -> String {
        let key = (name.to_string(), arity);
        let item = match self.extern_items.get(&key) {
            Some(it) => it.clone(),
            None => {
                let base = if arity == 0 {
                    name.to_string()
                } else {
                    format!("{name}_a{arity}")
                };
                let mut cand = base.clone();
                let mut n = 2;
                while self.extern_used.contains(&cand) {
                    cand = format!("{base}_{n}");
                    n += 1;
                }
                self.extern_used.insert(cand.clone());
                self.extern_items.insert(key, cand.clone());
                cand
            }
        };
        if arity == 0 {
            format!("crate::port::{}::{item}", self.externs_mod)
        } else {
            format!("crate::port::{}::{item}<{}>", self.externs_mod, args.join(", "))
        }
    }
}

/// The element type of a nullable: `T?` or `Nullable<T>` / `System.Nullable<T>`.
/// `Option<T>` is a value position, not a pointer, so this is used where a value
/// cycle has to be distinguished from heap indirection (`Vec<T>`, `*mut T`).
fn option_inner(ty: &str) -> Option<&str> {
    let t = ty.trim();
    if let Some(inner) = t.strip_suffix('?') {
        return Some(inner.trim());
    }
    for p in ["Nullable<", "System.Nullable<"] {
        if let Some(rest) = t.strip_prefix(p) {
            if let Some(inner) = rest.strip_suffix('>') {
                return Some(inner.trim());
            }
        }
    }
    None
}

/// Split `A, B<C, D>` into `["A", "B<C, D>"]`.
fn split_type_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            out.push(cur.trim().to_string());
            cur.clear();
            continue;
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// True when `v` is a SINGLE numeric literal and not an expression.
///
/// `1f / 60.0` is a real constant initializer in the source, and taking it verbatim
/// produced `pub const WAVE_FRAMERATE: f32 = 1f / 60.0;`, which rustc rejects for the
/// suffix. The expression is not evaluated: a constant we cannot state exactly is
/// left out rather than approximated.
fn is_single_number(v: &str) -> bool {
    let body = v.strip_prefix(['-', '+']).unwrap_or(v);
    if body.is_empty() {
        return false;
    }
    // A hex literal is validated as a whole. The old character scan only allowed a
    // hex digit immediately after the `x`, so `0x1F` was rejected and every hex
    // constant carrying a letter was silently dropped from the port.
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        return !hex.is_empty()
            && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_')
            && hex.chars().any(|c| c.is_ascii_hexdigit());
    }
    let mut prev = '\0';
    for c in body.chars() {
        let ok = c.is_ascii_digit()
            || c == '_'
            || c == '.'
            || ((c == '+' || c == '-') && (prev == 'e' || prev == 'E'))
            || c == 'e'
            || c == 'E';
        if !ok {
            return false;
        }
        prev = c;
    }
    body.chars().any(|c| c.is_ascii_digit())
}

/// A C# constant initializer as a Rust literal, or None when it is not something
/// we can state faithfully. A guessed constant is worse than an absent one: it
/// would put a plausible wrong number in an ID table.
fn cs_const_literal(rust_ty: &str, cs: &str) -> Option<String> {
    let v = cs.trim();
    if v.is_empty() || v == "-" {
        return None;
    }
    match rust_ty {
        "bool" => match v {
            "true" => Some("true".to_string()),
            "false" => Some("false".to_string()),
            _ => None,
        },
        "f32" | "f64" => {
            // strip ONE trailing C# suffix, if there is one
            let t = if v.ends_with(['f', 'F', 'd', 'D']) { &v[..v.len() - 1] } else { v };
            if !is_single_number(t) {
                return None;
            }
            if t.contains('.') || t.contains('e') || t.contains('E') {
                Some(t.to_string())
            } else {
                Some(format!("{t}.0"))
            }
        }
        "String" | "&str" => Some(format!("{v:?}")),
        "char" => {
            if v.starts_with('\'') && v.ends_with('\'') && v.len() >= 3 {
                Some(v.to_string())
            } else {
                None
            }
        }
        t if t.starts_with('i') || t.starts_with('u') => {
            // ONLY a plain integer or a hex integer, and only a single literal. An
            // earlier version accepted any run of hex digits, so `1f` - a float with a
            // suffix - passed as `1f` and rustc rejected it as an invalid suffix.
            // A C# integer suffix is a trailing run of `u`/`U`/`l`/`L`.
            let mut x = v;
            while x.len() > 1 && x.ends_with(['u', 'U', 'l', 'L']) {
                x = &x[..x.len() - 1];
            }
            let body = x.strip_prefix(['-', '+']).unwrap_or(x);
            // a hex literal is not a float with an `f` suffix: `0xFF` must survive
            let hex = body
                .strip_prefix("0x")
                .or_else(|| body.strip_prefix("0X"))
                .map(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit() || c == '_'))
                .unwrap_or(false);
            if hex {
                return Some(x.to_string());
            }
            if !is_single_number(x) || x.contains(['.', 'f', 'F']) {
                return None;
            }
            let dec = !body.is_empty() && body.chars().all(|c| c.is_ascii_digit() || c == '_');
            if dec {
                Some(x.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A nested module tree, built from the dotted namespace paths.
#[derive(Default)]
struct ModNode {
    children: BTreeMap<String, ModNode>,
    items: Vec<String>,
}

impl ModNode {
    fn insert(&mut self, path: &[String], item: String) {
        match path.split_first() {
            Some((head, rest)) => self
                .children
                .entry(head.clone())
                .or_default()
                .insert(rest, item),
            None => self.items.push(item),
        }
    }

    fn render(&self, indent: usize, out: &mut String) {
        let pad = "    ".repeat(indent);
        for (name, child) in &self.children {
            let _ = writeln!(out, "{pad}pub mod {name} {{");
            child.render(indent + 1, out);
            let _ = writeln!(out, "{pad}}}");
        }
        for it in &self.items {
            for line in it.lines() {
                if line.is_empty() {
                    let _ = writeln!(out);
                } else {
                    let _ = writeln!(out, "{pad}{line}");
                }
            }
        }
    }
}

/// Assemble the port from the sheets. Returns the source and a report.
pub fn emit_port_source(sheets: &[Sheet]) -> Result<(String, PortReport), String> {
    let anchor = sheets
        .iter()
        .find(|s| s.manifest.emit_port)
        .ok_or_else(|| "no sheet declares `emit_port`".to_string())?;
    let mut wanted: Vec<String> = vec![anchor.name.clone()];
    wanted.extend(anchor.manifest.port_from.iter().cloned());
    let find = |n: &str| sheets.iter().find(|s| s.name == n);

    let types = find(&anchor.name).ok_or("missing the types sheet")?;
    let fields = find(&wanted[1]).ok_or_else(|| format!("missing the fields sheet {}", wanted[1]))?;
    let methods = find(&wanted[2]).ok_or_else(|| format!("missing the methods sheet {}", wanted[2]))?;

    // --- join the three relations -----------------------------------------
    let mut recs: Vec<PortType> = Vec::with_capacity(types.rows.len());
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for r in &types.rows {
        let id = types.cell(r, "id").unwrap_or("").to_string();
        let ns = types.cell(r, "namespace").unwrap_or("-").to_string();
        let name = types.cell(r, "name").unwrap_or("").to_string();
        let mod_path: Vec<String> = if ns == "-" || ns.is_empty() {
            Vec::new()
        } else {
            ns.split('.').map(port_module).collect()
        };
        let item = port_ident(&name);
        index.insert(id.clone(), recs.len());
        recs.push(PortType {
            id,
            kind: types.cell(r, "kind").unwrap_or("class").to_string(),
            ns,
            name,
            base_: types.cell(r, "base").unwrap_or("-").to_string(),
            fields: Vec::new(),
            methods: Vec::new(),
            mod_path,
            item,
        });
    }

    let mut field_rows: Vec<&Row> = fields.rows.iter().collect();
    field_rows.sort_by_key(|r| {
        fields.cell(r, "ord").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0)
    });
    let mut n_fields = 0usize;
    for r in field_rows {
        let owner = fields.cell(r, "type").unwrap_or("");
        let Some(&i) = index.get(owner) else { continue };
        recs[i].fields.push(PortField {
            kind: fields.cell(r, "kind").unwrap_or("field").to_string(),
            name: fields.cell(r, "name").unwrap_or("").to_string(),
            ty: fields.cell(r, "field_type").unwrap_or("-").to_string(),
            value: fields.cell(r, "value").unwrap_or("-").to_string(),
            modifiers: fields.cell(r, "modifiers").unwrap_or("-").to_string(),
        });
        n_fields += 1;
    }
    let mut n_methods = 0usize;
    for r in &methods.rows {
        let owner = methods.cell(r, "type").unwrap_or("");
        let Some(&i) = index.get(owner) else { continue };
        recs[i].methods.push(PortMethod {
            name: methods.cell(r, "name").unwrap_or("").to_string(),
            ret: methods.cell(r, "ret").unwrap_or("void").to_string(),
            params: methods.cell(r, "params").unwrap_or("").to_string(),
            modifiers: methods.cell(r, "modifiers").unwrap_or("-").to_string(),
            kind: methods.cell(r, "kind").unwrap_or("method").to_string(),
        });
        n_methods += 1;
    }

    // --- the resolver ------------------------------------------------------
    let mut res = Resolver {
        by_id: BTreeMap::new(),
        by_name: BTreeMap::new(),
        kinds: BTreeMap::new(),
        extern_items: BTreeMap::new(),
        extern_used: ["Object".to_string()].into_iter().collect(),
        externs_mod: "externs".to_string(),
        ambiguous: BTreeSet::new(),
        gaps: BTreeSet::new(),
    };
    // two types can share a bare name inside one module (UIDynamicItemCollection
    // has a generic and a non-generic form), and Rust cannot; disambiguate
    // deterministically and record it
    let mut used: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    let mut n_collide = 0usize;
    for r in recs.iter_mut() {
        let taken = used.entry(r.mod_path.clone()).or_default();
        if taken.contains(&r.item) {
            n_collide += 1;
            let mut n = 2;
            while taken.contains(&format!("{}_{n}", r.item)) {
                n += 1;
            }
            r.item = format!("{}_{n}", r.item);
        }
        taken.insert(r.item.clone());
        res.by_id
            .insert(r.id.clone(), (r.mod_path.clone(), r.item.clone()));
        res.kinds.insert(r.id.clone(), r.kind.clone());
        res.by_name.entry(r.name.to_lowercase()).or_default().push(r.id.clone());
    }

    // A MODULE and an ITEM cannot share a name in one scope, and a root-namespace
    // type called `nativefiledialog` next to a namespace of the same name is a real
    // collision. Rename the module, before any path is built from it.
    let mut child_names: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    let mut item_names: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
    for r in &recs {
        item_names
            .entry(r.mod_path.clone())
            .or_default()
            .insert(r.item.clone());
        for i in 0..r.mod_path.len() {
            child_names
                .entry(r.mod_path[..i].to_vec())
                .or_default()
                .insert(r.mod_path[i].clone());
        }
    }
    // The generated `externs` module shares the ROOT scope with every top-level
    // item and module, so a namespace (`Externs`) or a root type whose item is
    // `externs` would define the name twice. Pick a free name before any path is
    // built from it; the resolver uses the same name for every extern reference.
    {
        let root_mods = child_names.get(&Vec::new());
        let root_items = item_names.get(&Vec::new());
        let mut name = "externs".to_string();
        while root_mods.map(|m| m.contains(&name)).unwrap_or(false)
            || root_items.map(|i| i.contains(&name)).unwrap_or(false)
        {
            name.push('_');
        }
        res.externs_mod = name;
    }
    let mut renamed: BTreeMap<(Vec<String>, String), String> = BTreeMap::new();
    for (parent, kids) in &child_names {
        let items = item_names.get(parent);
        for k in kids {
            if items.map(|i| i.contains(k)).unwrap_or(false) {
                renamed.insert((parent.clone(), k.clone()), format!("{k}_mod"));
                res.gaps.insert(format!("module renamed to avoid an item: {k}"));
            }
        }
    }
    if !renamed.is_empty() {
        for r in recs.iter_mut() {
            if r.mod_path.is_empty() {
                continue;
            }
            let clone = r.mod_path.clone();
            for i in 0..clone.len() {
                if let Some(n) = renamed.get(&(clone[..i].to_vec(), clone[i].clone())) {
                    r.mod_path[i] = n.clone();
                }
            }
        }
        for r in &recs {
            res.by_id
                .insert(r.id.clone(), (r.mod_path.clone(), r.item.clone()));
        }
    }

    // Break field cycles among local types.
    //
    // A C# CLASS field is a reference, so boxing it is what the original means; a
    // C# struct field is a value, and a cycle through value fields cannot have
    // finite size in either language - the decompiler flattens a reference into
    // what looks like a value. Either way, the edge that CLOSES a cycle is boxed,
    // and only that edge, so 100 types do not all grow a Box they do not need.
    let mut boxed_fields: BTreeSet<(String, String)> = BTreeSet::new();
    {
        fn dfs(
            u: usize,
            adj: &[Vec<usize>],
            color: &mut Vec<u8>,
            back: &mut BTreeSet<(usize, usize)>,
        ) {
            color[u] = 1;
            for &v in &adj[u] {
                if color[v] == 1 {
                    back.insert((u, v));
                } else if color[v] == 0 {
                    dfs(v, adj, color, back);
                }
            }
            color[u] = 2;
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); recs.len()];
        let mut edge_name: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
        for (i, r) in recs.iter().enumerate() {
            for f in &r.fields {
                if !matches!(f.kind.as_str(), "field" | "property" | "event") {
                    continue;
                }
                let ty = f.ty.trim();
                // `Option<T>` is a value position, not a pointer, so a cycle
                // through a nullable field is a real size cycle and must be seen.
                let base = if let Some(inner) = option_inner(ty) {
                    inner
                } else if ty.contains(['<', '[', '*']) {
                    continue;
                } else {
                    ty
                };
                let bare = base.rsplit('.').next().unwrap_or(base).to_lowercase();
                let ctx_ns = r.id.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default();
                let own = format!("{ctx_ns}.{bare}");
                let cand = if index.contains_key(own.trim_start_matches('.')) {
                    Some(own.trim_start_matches('.').to_string())
                } else {
                    match res.by_name.get(&bare) {
                        Some(c) if c.len() == 1 => Some(c[0].clone()),
                        _ => None,
                    }
                };
                if let Some(tid) = cand {
                    if let Some(&j) = index.get(&tid) {
                        if matches!(recs[j].kind.as_str(), "class" | "struct") {
                            adj[i].push(j);
                            edge_name.entry((i, j)).or_default().push(f.name.clone());
                        }
                    }
                }
            }
        }
        let mut color = vec![0u8; recs.len()];
        let mut back: BTreeSet<(usize, usize)> = BTreeSet::new();
        for i in 0..recs.len() {
            if color[i] == 0 {
                dfs(i, &adj, &mut color, &mut back);
            }
        }
        for (i, j) in &back {
            if let Some(names) = edge_name.get(&(*i, *j)) {
                for n in names {
                    boxed_fields.insert((recs[*i].id.clone(), n.clone()));
                }
            }
        }
        if !boxed_fields.is_empty() {
            res.gaps.insert(format!(
                "{} field(s) boxed to break a size cycle",
                boxed_fields.len()
            ));
        }
    }

    // An interface's own interface bases. `IPooledParticle: IParticle` means an
    // `impl IPooledParticle for X` does NOT satisfy the supertrait bound unless X
    // also has `impl IParticle`.
    let mut iface_parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in &recs {
        if r.kind != "interface" {
            continue;
        }
        let mut v = Vec::new();
        for b in r.base_.split(',') {
            let bl = b.trim().to_lowercase();
            if bl.is_empty() {
                continue;
            }
            if let Some(c) = res.by_name.get(&bl) {
                if c.len() == 1
                    && index
                        .get(&c[0])
                        .map(|&i| recs[i].kind == "interface")
                        .unwrap_or(false)
                {
                    v.push(c[0].clone());
                }
            }
        }
        iface_parents.insert(r.id.clone(), v);
    }

    // --- render ------------------------------------------------------------
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// GENERATED BY sheetty v{EMITTER_VERSION} FROM {} + {} + {} - DO NOT EDIT",
        types.name, fields.name, methods.name
    );
    let _ = writeln!(out, "// sources:  sheets/{}", types.rel);
    let _ = writeln!(out, "//           sheets/{}", fields.rel);
    let _ = writeln!(out, "//           sheets/{}", methods.rel);
    let _ = writeln!(out, "// rows:     {} types, {} fields, {} methods", recs.len(), n_fields, n_methods);
    let _ = writeln!(out, "//");
    let _ = writeln!(out, "// Hand edits are lost on the next build (MDD D6). Change the sheets instead.");
    let _ = writeln!(out);
    let _ = writeln!(out, "// A Rust item is a JOIN of three relations: the type row is the item, its");
    let _ = writeln!(out, "// field rows are the struct body in declaration order, its method rows are");
    let _ = writeln!(out, "// the impl. Every body is a stub: this is the shape of the server, not its");
    let _ = writeln!(out, "// behaviour. Names keep their C# case so the port reads against the source.");
    let _ = writeln!(out);
    // Inner attributes are NOT permitted inside `include!`, which is how this file
    // is used, so the lints are allowed by the including module instead (kernel/lib.rs).
    let _ = writeln!(out);

    // items, grouped into a real module TREE
    //
    // Keying this by full path and emitting a fresh `pub mod terraria { .. }` per
    // path defines `terraria` once per namespace under it, which rustc rejects as
    // 409 duplicate definitions. The tree has to be nested, not flat.
    let mut tree = ModNode::default();
    let mut trait_impls: Vec<String> = Vec::new();
    for r in &recs {
        let mut item = String::new();
        let doc = format!("/// `{}` ({}) from the {} sheet.", r.id, r.kind, types.name);
        let _ = writeln!(item, "{doc}");
        match r.kind.as_str() {
            "enum" => {
                let _ = writeln!(item, "pub enum {} {{", r.item);
                let mut next: i64 = 0;
                let mut seen: BTreeSet<String> = BTreeSet::new();
                let mut used_values: BTreeSet<i64> = BTreeSet::new();
                for f in &r.fields {
                    if f.kind != "enum_member" {
                        continue;
                    }
                    let mut v = port_ident(&f.name);
                    // A single `_` suffix is not enough: the variant `A` of enum `A`
                    // becomes `A_`, which then collides with a member already named
                    // `A_`. Keep extending until the name is free.
                    while v == r.item || seen.contains(&v) {
                        v.push('_');
                    }
                    seen.insert(v.clone());
                    let mut disc = match f.value.trim().parse::<i64>() {
                        Ok(n) => n,
                        Err(_) => {
                            // no explicit value in the source: C# numbers from the
                            // previous member, which `ord` preserves
                            next
                        }
                    };
                    // Rust rejects a repeated discriminant; a C# enum may legitimately
                    // have one (`A = 1, B = 1`), so nudge it rather than fail to build
                    while used_values.contains(&disc) {
                        disc += 1;
                    }
                    used_values.insert(disc);
                    next = disc + 1;
                    let _ = writeln!(item, "    {v} = {disc},");
                }
                let _ = writeln!(item, "}}\n");
            }
            "interface" => {
                // An interface that extends interfaces is a Rust SUPERTRAIT, not an
                // `impl`. Emitting `impl A for B` where B is also a trait is 18
                // `expected a type, found a trait` errors.
                let mut supers: Vec<String> = Vec::new();
                for b in r.base_.split(',') {
                    let b = b.trim();
                    if b.is_empty() {
                        continue;
                    }
                    if let Some(cands) = res.by_name.get(&b.to_lowercase()) {
                        if cands.len() == 1 {
                            if let Some(&bi) = index.get(&cands[0]) {
                                if recs[bi].kind == "interface" {
                                    if let Some(p) = res.path_of(&cands[0]) {
                                        supers.push(p);
                                    }
                                }
                            }
                        }
                    }
                }
                supers.sort();
                supers.dedup();
                if supers.is_empty() {
                    let _ = writeln!(item, "pub trait {} {{", r.item);
                } else {
                    let _ = writeln!(item, "pub trait {}: {} {{", r.item, supers.join(" + "));
                }
                let mut seen: BTreeSet<String> = BTreeSet::new();
                for m in &r.methods {
                    let sig = port_method_sig(&mut res, m, &r.id, &mut seen);
                    if sig.is_empty() {
                        continue;
                    }
                    // A trait item has no visibility of its own; `pub fn` inside a
                    // trait is E0449. `port_method_sig` emits `pub fn` for inherent
                    // methods, so strip it here rather than special-casing the sig.
                    let sig = sig.strip_prefix("pub ").unwrap_or(&sig);
                    let _ = writeln!(item, "    {sig} {{ unimplemented!() }}");
                }
                let _ = writeln!(item, "}}\n");
            }
            "delegate" => {
                // the delegate's signature is not in the book (a delegate is a
                // type declaration, not a member), so it is projected as an
                // opaque marker rather than invented
                res.gaps.insert(format!("delegate signature: {}", r.id));
                let _ = writeln!(item, "pub struct {};\n", r.item);
            }
            _ => {
                let _ = writeln!(item, "pub struct {} {{", r.item);
                let own_path = res.path_of(&r.id).unwrap_or_default();
                let mut seen: BTreeSet<String> = BTreeSet::new();
                for f in &r.fields {
                    if !matches!(f.kind.as_str(), "field" | "property" | "event") {
                        continue;
                    }
                    let mut n = port_ident(&f.name);
                    if seen.contains(&n) {
                        let mut k = 2;
                        while seen.contains(&format!("{n}_{k}")) {
                            k += 1;
                        }
                        n = format!("{n}_{k}");
                    }
                    seen.insert(n.clone());
                    let ty = res.map(&f.ty, &r.id, 0);
                    // a field of the struct's OWN type, or the edge that closes a
                    // cycle, has infinite size unless it is boxed
                    let ty = if ty == own_path
                        || boxed_fields.contains(&(r.id.clone(), f.name.clone()))
                    {
                        // `Option<T>` is a value wrapper: `Box<Option<T>>` still
                        // stores `T` inline, so the Box has to go inside, next to
                        // the recursive type.
                        if let Some(inner) = ty.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
                            format!("Option<Box<{inner}>>")
                        } else {
                            format!("Box<{ty}>")
                        }
                    } else {
                        ty
                    };
                    let _ = writeln!(item, "    pub {n}: {ty},");
                }
                let _ = writeln!(item, "}}\n");
            }
        }

        // the impl: constants, then methods
        if matches!(r.kind.as_str(), "class" | "struct") {
            let mut body = String::new();
            // Consts and methods share one associated-item namespace, so they are
            // deduped together: two rows whose names sanitize to the same Rust item
            // would otherwise emit the (invalid) duplicate.
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for f in &r.fields {
                if f.kind != "const" {
                    continue;
                }
                let ty = res.map(&f.ty, &r.id, 0);
                // a Rust `const` cannot hold a `String`
                let ty = if ty == "String" { "&'static str".to_string() } else { ty };
                let Some(lit) = cs_const_literal(&ty, &f.value) else {
                    res.gaps.insert(format!("const value: {}", f.name));
                    continue;
                };
                let mut name = port_ident(&f.name);
                if seen.contains(&name) {
                    let mut k = 2;
                    while seen.contains(&format!("{name}_{k}")) {
                        k += 1;
                    }
                    name = format!("{name}_{k}");
                }
                seen.insert(name.clone());
                let _ = writeln!(body, "    pub const {name}: {ty} = {lit};");
            }
            for m in &r.methods {
                let sig = port_method_sig(&mut res, m, &r.id, &mut seen);
                if sig.is_empty() {
                    continue;
                }
                let _ = writeln!(body, "    {sig} {{ unimplemented!() }}");
            }
            if !body.is_empty() {
                let _ = writeln!(item, "impl {} {{", r.item);
                item.push_str(&body);
                let _ = writeln!(item, "}}\n");
            }
        }

        // an interface base becomes a real impl, which the trait's default bodies
        // make legal
        if r.base_ != "-" {
            for b in r.base_.split(',') {
                let b = b.trim();
                if b.is_empty() {
                    continue;
                }
                let bl = b.to_lowercase();
                if let Some(cands) = res.by_name.get(&bl) {
                    if cands.len() == 1 {
                        if let Some(&bi) = index.get(&cands[0]) {
                            if recs[bi].kind == "interface" && r.kind != "interface" {
                                let sp = res.path_of(&r.id).unwrap_or_default();
                                // the trait, and every interface it extends, or the
                                // impl does not satisfy the supertrait bound
                                let mut stack = vec![cands[0].clone()];
                                let mut seen_t: BTreeSet<String> = BTreeSet::new();
                                while let Some(t) = stack.pop() {
                                    if !seen_t.insert(t.clone()) {
                                        continue;
                                    }
                                    if let Some(p) = res.path_of(&t) {
                                        trait_impls.push(format!("impl {p} for {sp} {{}}"));
                                    }
                                    if let Some(ps) = iface_parents.get(&t) {
                                        for p in ps {
                                            stack.push(p.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        tree.insert(&r.mod_path, item);
    }

    // --- externs -----------------------------------------------------------
    let mut ext_items: Vec<String> = Vec::new();
    for ((_name, a), item) in &res.extern_items {
        let a = *a;
        if a == 0 {
            ext_items.push(format!("/// Referenced by the server but not declared in it.\npub struct {item};"));
        } else if a == 1 {
            ext_items.push(format!(
                "/// Referenced by the server but not declared in it.\npub struct {item}<T0>(pub core::marker::PhantomData<T0>);"
            ));
        } else {
            let gens: Vec<String> = (0..a).map(|i| format!("T{i}")).collect();
            // a TUPLE inside PhantomData, because PhantomData takes exactly one
            // type parameter and `PhantomData<T0, T1>` does not compile
            ext_items.push(format!(
                "/// Referenced by the server but not declared in it.\npub struct {item}<{}>(pub core::marker::PhantomData<({})>);",
                gens.join(", "),
                gens.join(", ")
            ));
        }
    }

    // --- write it out ------------------------------------------------------
    let _ = writeln!(out, "/// Types the server references but does not declare: the BCL, XNA, and the");
    let _ = writeln!(out, "/// delegates whose signatures are not in the book. Generated so the port");
    let _ = writeln!(out, "/// compiles; replace them with real definitions as they are ported.");
    let _ = writeln!(out, "pub mod {} {{", res.externs_mod);
    let _ = writeln!(out, "    #![allow(non_camel_case_types)]");
    let _ = writeln!(out, "    /// A stand-in for `System.Object`.");
    let _ = writeln!(out, "    pub struct Object;");
    for it in &ext_items {
        for line in it.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
    let _ = writeln!(out, "}}\n");

    tree.render(0, &mut out);

    if !trait_impls.is_empty() {
        trait_impls.sort();
        trait_impls.dedup();
        let _ = writeln!(out, "\n/// Interface inheritance, projected as real trait impls.");
        for t in &trait_impls {
            let _ = writeln!(out, "{t}");
        }
    }

    let report = PortReport {
        module: "port".to_string(),
        path: String::new(),
        types: recs.len(),
        fields: n_fields,
        methods: n_methods,
        externs: ext_items.len(),
        gaps: res.gaps.len(),
        bytes: out.len(),
        written: false,
    };
    let _ = n_collide;
    let _ = res.ambiguous.len();
    Ok((out, report))
}

/// One method as a Rust signature, with overload and keyword handling. Returns an
/// empty string when the method cannot be named at all.
fn port_method_sig(
    res: &mut Resolver,
    m: &PortMethod,
    owner_id: &str,
    seen: &mut BTreeSet<String>,
) -> String {
    let base = if m.kind == "ctor" {
        "new".to_string()
    } else {
        let raw = m.name.trim();
        if raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !raw.is_empty() {
            port_ident(raw)
        } else {
            // `operator !=` and friends are not identifiers; the id already
            // carries a stable word for them
            port_ident(
                &raw
                    .replace("operator ", "op_")
                    .replace('!', "ne")
                    .replace('=', "eq"),
            )
        }
    };
    if base.is_empty() || base == "r#" {
        return String::new();
    }
    let mut name = base.clone();
    if seen.contains(&name) {
        let mut k = 2;
        while seen.contains(&format!("{base}_{k}")) {
            k += 1;
        }
        name = format!("{base}_{k}");
    }
    seen.insert(name.clone());

    let mut args: Vec<String> = Vec::new();
    let is_static = m.modifiers.split_whitespace().any(|w| w == "static");
    if m.kind != "ctor" && !is_static {
        args.push("&self".to_string());
    }
    for p in m.params.split(';') {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (pname, ptype) = match p.split_once(':') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => ("arg", p),
        };
        let pname = port_ident(pname);
        let pname = if pname == "self" { "this".to_string() } else { pname };
        args.push(format!("{pname}: {}", res.map(ptype, owner_id, 0)));
    }
    let ret = if m.kind == "ctor" {
        "Self".to_string()
    } else {
        res.map(&m.ret, owner_id, 0)
    };
    if ret == "()" {
        format!("pub fn {name}({})", args.join(", "))
    } else {
        format!("pub fn {name}({}) -> {ret}", args.join(", "))
    }
}

/// Run the whole emit stage. Refuses to emit when preflight has errors (D5).
pub fn emit_run(sheets_dir: &Path, out_dir: &Path) -> Result<Vec<EmitReport>, String> {
    let (sheets, mut findings) = load_book(sheets_dir);
    if sheets.is_empty() {
        return Err(format!("no sheets found under {}", sheets_dir.display()));
    }
    validate(&sheets, &mut findings);
    let errors: Vec<&Finding> = findings.iter().filter(|f| f.is_error()).collect();
    if !errors.is_empty() {
        // D5: "emit must be impossible to invoke when validate has failed".
        let mut msg = format!("preflight failed with {} error(s); emit is refused (D5)", errors.len());
        for f in errors.iter().take(5) {
            msg.push_str(&format!("\n  {} {}:{} {}", f.code, f.sheet, f.line, f.message));
        }
        return Err(msg);
    }

    let mut reports = Vec::new();
    for s in sheets.iter().filter(|s| s.manifest.emit_rust) {
        let src = emit_sheet_source(s);
        let module = module_name(&s.name);
        let path = out_dir.join("sheets").join(format!("{module}.rs"));
        let written = write_if_changed(&path, src.as_bytes())?;
        reports.push(EmitReport {
            sheet: s.name.clone(),
            module,
            path: path.to_string_lossy().replace('\\', "/"),
            rows: s.rows.len(),
            bytes: src.len(),
            written,
        });
    }

    let reg = emit_registry_source(&sheets);
    let rpath = out_dir.join("registry.rs");
    let written = write_if_changed(&rpath, reg.as_bytes())?;
    reports.push(EmitReport {
        sheet: "(registry)".to_string(),
        module: "registry".to_string(),
        path: rpath.to_string_lossy().replace('\\', "/"),
        rows: sheets.len(),
        bytes: reg.len(),
        written,
    });

    // The port is a projection of several sheets, not of one, so it is written
    // once rather than per sheet. `emit_port` is how a sheet asks for it.
    if sheets.iter().any(|s| s.manifest.emit_port) {
        let (src, _rep) = emit_port_source(&sheets)?;
        let ppath = out_dir.join("port.rs");
        let written = write_if_changed(&ppath, src.as_bytes())?;
        reports.push(EmitReport {
            sheet: "(port)".to_string(),
            module: "port".to_string(),
            path: ppath.to_string_lossy().replace('\\', "/"),
            rows: sheets.iter().filter(|s| s.manifest.emit_port).count(),
            bytes: src.len(),
            written,
        });
    }

    Ok(reports)
}

/// The per-sheet `rerun-if-changed` hints the emitter's own build.rs must print
/// (MDD 5.9 item 3: "emit nothing global").
pub fn rerun_hints(sheets_dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir(sheets_dir) {
        let mut stack = vec![sheets_dir.to_path_buf()];
        let _ = rd;
        while let Some(dir) = stack.pop() {
            if let Ok(entries) = fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().map(|x| x.eq_ignore_ascii_case("tsv")).unwrap_or(false) {
                        v.push(format!("cargo:rerun-if-changed={}", p.to_string_lossy().replace('\\', "/")));
                    }
                }
            }
        }
    }
    v.sort();
    v
}

/// L0 check 22: no generated file may be committed outside the build directory.
/// The banner is the marker.
pub fn check_no_generated_in_tree(root: &Path, out_dir: &Path) -> Vec<Finding> {
    let mut found = Vec::new();
    let banner = format!("GENERATED BY sheetty v{EMITTER_VERSION}");
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir.starts_with(out_dir) || dir.file_name().map(|n| n == "target" || n == ".git").unwrap_or(false) {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    if let Ok(t) = fs::read_to_string(&p) {
                        if t.contains(&banner) {
                            found.push(Finding::err(
                                "E-L0-GENERATED",
                                &p.to_string_lossy().replace('\\', "/"),
                                0,
                                "",
                                "",
                                "generated file found in the working tree; generated code is never committed (D6)",
                            ));
                        }
                    }
                }
            }
        }
    }
    found
}
