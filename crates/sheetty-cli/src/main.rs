//! sheetty-cli: `sheetty preflight | overlap | report | view`
//!
//! Exit codes: 0 = clean, 1 = errors present (D5: preflight gates the build),
//! 2 = usage error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sheetty::*;

fn usage() -> &'static str {
    "sheetty <command>

commands:
  preflight [--sheets DIR] [--layer L0|L1|L2|L3] [--json] [--strict] [--pair A:B]
  overlap <SPEC> <IMPL> [--key id] [--json]
  report unimplemented [--spec A] [--impl B] [--rank blast-radius] [--json]
  view <SHEET> [--cols a,b,c] [--out FILE] [--sheets DIR]
  schema [--sheets DIR] [--json] [--emit PATH]
  rules   print which MDD preflight checks this engine implements
  order   print a topological order of the sheets (dependency order)
  emit    project the sheets marked `# emit: rust` into $OUT_DIR (--out DIR)
  pack    assemble a named view into a context pack and check its budget
"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprint!("{}", usage());
        return ExitCode::from(2);
    }
    match args[0].as_str() {
        "preflight" => cmd_preflight(&args[1..]),
        "overlap" => cmd_overlap(&args[1..]),
        "report" => cmd_report(&args[1..]),
        "view" => cmd_view(&args[1..]),
        "schema" => cmd_schema(&args[1..]),
        "rules" => {
            print!("{}", rules_report());
            ExitCode::SUCCESS
        }
        "order" => {
            let dir = sheets_dir(&args[1..]);
            let (sheets, _) = load_book(&dir);
            match topo_order(&sheets) {
                Ok(order) => {
                    println!("topological order ({} sheets; dependencies first)", order.len());
                    for (i, n) in order.iter().enumerate() {
                        println!("  {:>2}. {n}", i + 1);
                    }
                    ExitCode::SUCCESS
                }
                Err(stuck) => {
                    eprintln!("NO topological order: {} sheet(s) in cycles: {}", stuck.len(), stuck.join(", "));
                    ExitCode::from(1)
                }
            }
        }
        "pack" => {
            // args already excludes argv[0]; args[0] is the command itself.
            let rest: Vec<String> = args[1..].to_vec();
            let mut positional = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                if rest[i].starts_with("--") {
                    i += 2;
                } else {
                    positional.push(rest[i].clone());
                    i += 1;
                }
            }
            let view = match positional.first() {
                Some(v) => v.clone(),
                None => {
                    eprint!("{}", usage());
                    return ExitCode::from(2);
                }
            };
            let dir = sheets_dir(&rest);
            let (sheets, _) = load_book(&dir);
            let rows_override = flag_value(&rest, "--rows");
            match build_pack_opt(&sheets, &view, rows_override.as_deref()) {
                Ok((text, r)) => {
                    if has_flag(&rest, "--json") {
                        println!(
                            "{{\"view\":\"{}\",\"sheet\":\"{}\",\"rows\":{},\"total_rows\":{},\"window\":\"{}\",\"columns\":{},\"body_tokens_est\":{},\"prefix_tokens_est\":{},\"budget\":{},\"prefix_hash\":\"{}\",\"over_budget\":{}}}",
                            jesc(&r.view), jesc(&r.sheet), r.window_rows, r.total_rows, jesc(&r.window), r.columns.len(),
                            r.body_tokens, r.prefix_tokens, r.budget, r.prefix_hash, r.over_budget
                        );
                    } else {
                        println!("PACK  {} (sheet {})", r.view, r.sheet);
                        println!("  columns        {}", r.columns.len());
                        println!("  window         '{}' -> {} of {} rows", r.window, r.window_rows, r.total_rows);
                        println!("  body           ~{} tokens est ({} bytes)", r.body_tokens, r.body_bytes);
                        println!("  stable prefix  ~{} tokens est ({} bytes)", r.prefix_tokens, r.prefix_bytes);
                        println!("  prefix hash    {}   <- record this; if it changes the cache is cold (MDD 11.2)", r.prefix_hash);
                        println!("  budget         {}", if r.budget == 0 { "unbounded (v_full)".to_string() } else { r.budget.to_string() });
                        if r.budget > 0 {
                            println!("  status         {}", if r.over_budget { "OVER BUDGET" } else { "within budget" });
                        }
                    }
                    if let Some(p) = flag_value(&rest, "--out").map(PathBuf::from) {
                        if let Err(e) = std::fs::write(&p, text) {
                            eprintln!("cannot write {}: {e}", p.display());
                            return ExitCode::from(1);
                        }
                        eprintln!("wrote {}", p.display());
                    }
                    if has_flag(&rest, "--check") && r.over_budget {
                        return ExitCode::from(1);
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("pack failed: {e}");
                    ExitCode::from(1)
                }
            }
        }
        "emit" => {
            let dir = sheets_dir(&args[1..]);
            let out = PathBuf::from(flag_value(&args[1..], "--out").unwrap_or_else(|| "target/generated".to_string()));
            match emit_run(&dir, &out) {
                Ok(reports) => {
                    let written = reports.iter().filter(|r| r.written).count();
                    println!("EMIT  {} file(s), {} written, {} unchanged (content-hash gated)", reports.len(), written, reports.len() - written);
                    for r in &reports {
                        println!(
                            "  {:<14} {:>7} rows {:>10} bytes  {}",
                            r.module,
                            r.rows,
                            r.bytes,
                            if r.written { "written" } else { "unchanged" }
                        );
                        println!("                 {}", r.path);
                    }
                    if has_flag(&args[1..], "--hints") {
                        for h in rerun_hints(&dir) {
                            println!("{h}");
                        }
                    }
                    if has_flag(&args[1..], "--verify-determinism") {
                        // MDD L7: re-running the emitter from a clean OUT_DIR must
                        // produce byte-identical output.
                        let tmp = out.join(".determinism-check");
                        let _ = std::fs::remove_dir_all(&tmp);
                        match emit_run(&dir, &tmp) {
                            Ok(_) => {
                                let diff = compare_trees(&out.join("sheets"), &tmp.join("sheets"));
                                if diff == 0 {
                                    println!("DETERMINISM  clean re-emit is byte-identical (0 differing files)");
                                } else {
                                    println!("DETERMINISM  FAILED: {diff} differing file(s)");
                                }
                                let _ = std::fs::remove_dir_all(&tmp);
                                return if diff == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) };
                            }
                            Err(e) => {
                                eprintln!("determinism re-emit failed: {e}");
                                return ExitCode::from(1);
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("EMIT REFUSED\n{e}");
                    ExitCode::from(1)
                }
            }
        }
        "-h" | "--help" | "help" => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command '{other}'\n");
            eprint!("{}", usage());
            ExitCode::from(2)
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}
fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}
fn sheets_dir(args: &[String]) -> PathBuf {
    PathBuf::from(flag_value(args, "--sheets").unwrap_or_else(|| "sheets".to_string()))
}

fn cmd_preflight(args: &[String]) -> ExitCode {
    let dir = sheets_dir(args);
    let layer = flag_value(args, "--layer").unwrap_or_default();
    let json = has_flag(args, "--json");
    let strict = has_flag(args, "--strict");
    let (do_l0, do_l1, do_l2, do_l3) = run_layers(&layer);

    let (sheets, mut findings) = load_book(&dir);
    if sheets.is_empty() {
        eprintln!("no sheets found under {}", dir.display());
        return ExitCode::from(1);
    }
    // D5: preflight gates everything, so this runs unconditionally.
    let mut pre = Vec::new();
    validate(&sheets, &mut pre);

    // L0 check 22: no generated file may sit in the working tree (D6). The
    // banner written by the emitter is the marker.
    if do_l0 {
        if let Some(root) = dir.parent() {
            pre.extend(check_no_generated_in_tree(root, &root.join("target")));
        }
    }
    // layer filtering
    pre.retain(|f| {
        let l = f.code.chars().nth(2);
        match l {
            Some('0') => do_l0,
            Some('1') => do_l1,
            Some('2') => do_l2,
            _ => true,
        }
    });
    findings.append(&mut pre);

    let mut overlaps = Vec::new();
    if do_l3 {
        let pair = flag_value(args, "--pair");
        let (a_name, b_name) = match pair.as_deref() {
            Some(p) => match p.split_once(':') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => ("02-plan".to_string(), "03-impl".to_string()),
            },
            None => ("02-plan".to_string(), "03-impl".to_string()),
        };
        if let (Some(a), Some(b)) = (find_sheet(&sheets, &a_name), find_sheet(&sheets, &b_name)) {
            overlaps.push(overlap(a, b));
        }
    }

    let s = summarize(&sheets, &findings);
    if json {
        println!("{}", json_report(&sheets, &findings, &overlaps));
    } else {
        print!("{}", human_report(&sheets, &findings, &overlaps));
    }

    let fail = s.errors > 0 || (strict && s.warnings > 0);
    if fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn find_sheet<'a>(sheets: &'a [Sheet], name: &str) -> Option<&'a Sheet> {
    sheets
        .iter()
        .find(|s| s.name == name || s.rel.trim_end_matches(".tsv") == name || s.rel.trim_end_matches(".tsv").replace('/', "-") == name)
}

fn cmd_overlap(args: &[String]) -> ExitCode {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    if positional.len() < 2 {
        eprint!("{}", usage());
        return ExitCode::from(2);
    }
    let dir = sheets_dir(args);
    let json = has_flag(args, "--json");
    let (sheets, _) = load_book(&dir);
    let (a, b) = match (find_sheet(&sheets, &positional[0]), find_sheet(&sheets, &positional[1])) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("sheet not found: {}", positional.join(" "));
            return ExitCode::from(1);
        }
    };
    let o = overlap(a, b);
    if json {
        println!("{{\"spec\":\"{}\",\"impl\":\"{}\",\"covered\":{},\"unimplemented\":{},\"orphan\":{},\"divergent\":{}}}",
            jesc(&o.spec), jesc(&o.imp), o.covered.len(), o.unimplemented.len(), o.orphan.len(), o.divergent.len());
    } else {
        let (c, u, or, d) = o.counts();
        println!("overlap {} x {}  (key: {})", o.spec, o.imp, flag_value(args, "--key").unwrap_or_else(|| "id".into()));
        println!("  covered        {c}");
        println!("  unimplemented  {u}");
        for id in o.unimplemented.iter().take(50) {
            println!("      - {id}");
        }
        println!("  orphan         {or}");
        for id in o.orphan.iter().take(50) {
            println!("      + {id}");
        }
        println!("  divergent      {d}");
        for (k, col, a, b) in o.divergent.iter().take(50) {
            println!("      ~ {k}.{col}: spec='{a}' impl='{b}'");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_report(args: &[String]) -> ExitCode {
    if args.first().map(|s| s.as_str()) != Some("unimplemented") {
        eprint!("{}", usage());
        return ExitCode::from(2);
    }
    let dir = sheets_dir(args);
    let json = has_flag(args, "--json");
    let spec = flag_value(args, "--spec").unwrap_or_else(|| "02-plan".to_string());
    let imp = flag_value(args, "--impl").unwrap_or_else(|| "03-impl".to_string());
    let (sheets, _) = load_book(&dir);
    let (a, b) = match (find_sheet(&sheets, &spec), find_sheet(&sheets, &imp)) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("sheet not found: {spec} or {imp}");
            return ExitCode::from(1);
        }
    };
    let o = overlap(a, b);
    let ranked = rank_blast_radius(&sheets, a, &o.unimplemented);
    if json {
        print!("{{\"spec\":\"{}\",\"impl\":\"{}\",\"count\":{},\"work_queue\":[", jesc(&o.spec), jesc(&o.imp), ranked.len());
        for (i, (id, inbound, prio)) in ranked.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!("{{\"id\":\"{}\",\"inbound_refs\":{},\"priority\":{}}}", jesc(id), inbound, prio);
        }
        println!("]}}");
    } else {
        println!("UNIMPLEMENTED  {} ({} rows), ranked by blast radius", o.spec, ranked.len());
        println!("  {:<24} {:>12} {:>9}", "id", "inbound_refs", "priority");
        for (id, inbound, prio) in &ranked {
            println!("  {:<24} {:>12} {:>9}", id, inbound, prio);
        }
        if ranked.is_empty() {
            println!("  (none)");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_view(args: &[String]) -> ExitCode {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    let name = match positional.first() {
        Some(n) => n.clone(),
        None => {
            eprint!("{}", usage());
            return ExitCode::from(2);
        }
    };
    let dir = sheets_dir(args);
    let (sheets, _) = load_book(&dir);
    let s = match find_sheet(&sheets, &name) {
        Some(s) => s,
        None => {
            eprintln!("sheet not found: {name}");
            return ExitCode::from(1);
        }
    };
    let cols: Vec<String> = match flag_value(args, "--cols") {
        Some(c) => c.split(',').map(|x| x.trim().to_string()).collect(),
        None => s.columns.iter().map(|c| c.name.clone()).collect(),
    };
    let mut out = String::new();
    out.push_str(&cols.join("\t"));
    out.push('\n');
    for r in &s.rows {
        let cells: Vec<String> = cols
            .iter()
            .map(|c| s.cell(r, c).unwrap_or("").to_string())
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    match flag_value(args, "--out").map(PathBuf::from) {
        Some(p) => {
            if let Err(e) = std::fs::write(&p, out) {
                eprintln!("cannot write {}: {e}", p.display());
                return ExitCode::from(1);
            }
            eprintln!("wrote {} ({} cols, {} rows)", p.display(), cols.len(), s.rows.len());
        }
        None => print!("{out}"),
    }
    ExitCode::SUCCESS
}

fn cmd_schema(args: &[String]) -> ExitCode {
    let dir = sheets_dir(args);
    let json = has_flag(args, "--json");
    let (sheets, _) = load_book(&dir);
    if let Some(p) = flag_value(args, "--emit").map(PathBuf::from) {
        let tsv = schema_tsv(&sheets);
        if let Err(e) = std::fs::write(&p, tsv) {
            eprintln!("cannot write {}: {e}", p.display());
            return ExitCode::from(1);
        }
        eprintln!("wrote {} (authoritative schema)", p.display());
        return ExitCode::SUCCESS;
    }
    if json {
        print!("{{\"sheets\":[");
        for (i, s) in sheets.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!("{{\"sheet\":\"{}\",\"columns\":[", jesc(&s.name));
            for (j, c) in s.columns.iter().enumerate() {
                if j > 0 {
                    print!(",");
                }
                print!(
                    "{{\"name\":\"{}\",\"type\":\"{}\",\"optional\":{},\"pk\":{},\"unit\":{},\"ref\":{}}}",
                    jesc(&c.name),
                    jesc(&c.ty),
                    c.optional,
                    c.pk,
                    c.unit.as_deref().map(|u| format!("\"{}\"", jesc(u))).unwrap_or_else(|| "null".into()),
                    c.ref_to.as_ref().map(|(a, b)| format!("\"{}.{}\"", jesc(a), jesc(b))).unwrap_or_else(|| "null".into())
                );
            }
            print!("]}}");
        }
        println!("]}}");
    } else {
        for s in sheets.iter() {
            println!("{}  ({} cols, {} rows)", s.name, s.columns.len(), s.rows.len());
            for c in s.columns.iter() {
                let refs = c.ref_to.as_ref().map(|(a, b)| format!(" -> {a}.{b}")).unwrap_or_default();
                let unit = c.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
                println!("    {:<24} :{}{}{}{}{}", c.name, c.ty, if c.optional { "?" } else { "" }, unit, refs, if c.pk { "  [pk]" } else { "" });
            }
        }
    }
    ExitCode::SUCCESS
}

#[allow(dead_code)]
fn _unused(_p: &Path) {}

/// Count files present in `a` or `b` whose bytes differ. Missing files count as
/// a difference, which is what makes this a real determinism check rather than a
/// comparison of two runs that both happened to write nothing.
fn compare_trees(a: &Path, b: &Path) -> usize {
    let mut names: Vec<String> = Vec::new();
    for d in [a, b] {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    let mut diff = 0;
    for n in names {
        if std::fs::read(a.join(&n)).ok() != std::fs::read(b.join(&n)).ok() {
            diff += 1;
        }
    }
    diff
}
