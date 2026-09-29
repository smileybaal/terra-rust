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
    pub key_alias: Vec<String>,
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
            "key_alias" => {
                m.key_alias = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "-").collect()
            }
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
    let root = match kernel.path.parent().and_then(|p| p.parent()) {
        Some(p) => p.to_path_buf(),
        None => return,
    };
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

pub fn overlap(a: &Sheet, b: &Sheet) -> Overlap {
    let akeys: BTreeSet<String> = a.rows.iter().map(|r| a.row_key(r)).collect();
    let bkeys: BTreeSet<String> = b.rows.iter().map(|r| b.row_key(r)).collect();
    let mut o = Overlap { spec: a.name.clone(), imp: b.name.clone(), ..Default::default() };

    let shared: Vec<&Column> = a.columns.iter().filter(|c| b.col_index(&c.name).is_some() && c.name != "id").collect();

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
    RuleCheck { n: "24", name: "Divergence vs evidence (L5)", status: "absent", emits: "",              note: "sheet-vs-producer reconciliation is not implemented" },
    RuleCheck { n: "25", name: "Context packs within budget (L6)", status: "implemented", emits: "E-L6-BUDGET", note: "every view in sheets/views.tsv is assembled and measured on every preflight" },
    RuleCheck { n: "26", name: "Dead columns (L6)",    status: "absent",      emits: "",               note: "the emitter is generic and consumes every column, so a dead-column rule cannot fire yet" },
    RuleCheck { n: "27", name: "Emitter determinism (L7)", status: "implemented", emits: "emit --verify-determinism", note: "re-emits into a clean directory and byte-compares; also the per-file content-hash gate" },
    RuleCheck { n: "28", name: "Row count sanity",     status: "absent",      emits: "",               note: "no last-commit comparison" },
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

/// `type` is a Rust keyword and `type` is a real column in re/methods, so raw
/// identifiers are not optional here.
fn field_ident(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// `02-plan` -> `plan`, `re/types` -> `re_types`. The numeric ordering prefix is
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
