//! `tsc-bench symbols` — the M4 `.symbols` parity twin (parse + bind →
//! per-file symbol-table dump).
//!
//! Go reference: `runSymbols` in rust/bench/go-driver/main.go. Both drivers
//! walk the corpus, parse + bind each file, then walk the binder's symbol
//! graph and emit one NDJSON line per file; the outputs must be
//! byte-identical — plain `diff` is the verdict.
//!
//!   tsc-bench symbols --corpus <dir> [--fuzz <dir>]
//!
//! PORT notes (decisions shared with the Go driver — mirror there):
//!
//!   - CANONICAL WALK. Go's symbol tables are `map[string]*Symbol` (random
//!     iteration) and the Rust port's are insertion-ordered `OrderedMap`s,
//!     so table iteration order is NOT a shared concept; the canonical
//!     order is defined by the walk instead, and BOTH drivers implement it
//!     identically. The walk is an AST pre-order (ForEachChild — proven
//!     identical by the `dump` twin) visiting each node's Locals table,
//!     then the node's Symbol, then its LocalSymbol, then recursing into
//!     children; from each discovered symbol, its Members table, then its
//!     Exports table (both in canonical-key order, see below); the file's
//!     GlobalExports table closes the walk (PatternAmbientModules symbols
//!     are table-declared upstream in `bindModuleDeclaration` and
//!     therefore already reachable). This mirrors Go binder.go's own
//!     reachability contract ("symbol tables, node .Symbol links,
//!     container locals — not all symbols end up in tables",
//!     binder.go bindWorker header): anonymous symbols (module symbol of
//!     an external module, type-literal symbols, duplicate-identifier
//!     scratch symbols, the CJS `exports` property) are node-attached
//!     only, so a tables-only walk would miss them on BOTH sides — the
//!     union walk covers everything. Canonical ids are assigned in
//!     encounter order (0,1,2,...); records are emitted in id order;
//!     `parent` resolves to the parent's canonical id (null when the
//!     parent is nil or outside the emitted set).
//!
//!   - CANONICAL KEYS. Go's InternalSymbolNamePrefix is the single byte
//!     0xFE; the Rust port models it as U+00FE ("þ", UTF-8 bytes C3 BE).
//!     Raw names therefore differ BYTES-wise between the ports, so the
//!     twin canonicalizes every name/key through Go's own baseline
//!     sanitization rule (`ast.EscapeInternalSymbolName`, the same escape
//!     the .symbols/.types baselines rely on): a leading internal prefix
//!     becomes "__", everything else is untouched. Table keys are sorted
//!     by (canonical form, raw form) — identical byte ordering on both
//!     sides. (A latent port hazard, PORT-noted here: a user identifier
//!     literally starting with "þ" collides with the Rust internal prefix
//!     representation; no corpus file contains "þ".)
//!
//!   - RECORD FORMAT (per symbol; fields both sides emit identically):
//!     {"id":i,"name":"...","flags":"<%08x u32>","kind":<kind ordinal of
//!     first declaration, -1 if none>,"parent":<canonical id>|null,
//!     "declarations":["<kind>:<pos>",...],"exports":N,"members":M,
//!     "valueDeclaration":"<kind>:<pos>"|null}. Go-only/transient fields
//!     (CheckFlags, ExportSymbol, lazy symbol/node ids) are dropped from
//!     BOTH sides (the fallback rule). "alloc" carries the binder's own
//!     allocation count (Go `file.SymbolCount`) next to "symbols" (the
//!     emitted-record count) so unreachable-symbol divergence is visible.
//!
//!   - shape_hash: FNV-1a 64 over one line per record (see
//!     `hash_lines`; vectors in the tests below).
//!
//!   - "diag" is the BIND diagnostics array (key/code/pos/end/args, the
//!     dump line's shape); parse diagnostics are the `dump` twin's surface
//!     and are not repeated here. Related-info chains are dropped from
//!     BOTH sides.
//!
//!   - Panics (parse or bind) emit {"file":...,"ok":false,"error":...}
//!     instead of aborting the run — the binder gate proves the gate
//!     corpus is panic-free on both sides, so any ok:false line is itself
//!     a divergence finding. Unreadable files are skipped silently
//!     (indexCorpus rule, both sides).
//!
//!   - The fuzz-seed path (`--fuzz`, `go test fuzz v1` corpus files)
//!     mirrors the binder gate's sweep: corpus files first, then seeds.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId, SymbolTable};
use tsc_core::scriptkind::ScriptKind;
use tsc_tspath::{CaseSensitivity, rooted_file_path_from_absolute};

use crate::parse::{BenchFile, SKIP_DIRS, json_escape, script_kind_from_file_name};

// ──────────────────────────────────────────────────────────────────────
// CLI + corpus walks (Go: runSymbols / indexSymbolsCorpus / indexFuzzSeeds)
// ──────────────────────────────────────────────────────────────────────

/// Entry point for `tsc-bench symbols --corpus <dir> [--fuzz <dir>]`.
pub fn run_symbols(args: &[String]) {
    let flags = match parse_cli(args) {
        Ok(flags) => flags,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(2);
        }
    };
    let mut corpus = String::new();
    let mut fuzz = String::new();
    for (name, value) in flags {
        match name.as_str() {
            "corpus" => corpus = value,
            "fuzz" => fuzz = value,
            _ => unreachable!(),
        }
    }
    if corpus.is_empty() && fuzz.is_empty() {
        eprintln!("tsc-bench symbols: missing required --corpus <dir> or --fuzz <dir>");
        std::process::exit(2);
    }
    let mut files: Vec<BenchFile> = Vec::new();
    if !corpus.is_empty() {
        files.extend(index_symbols_corpus(&corpus).unwrap_or_else(|err| {
            eprintln!("tsc-bench: {err}");
            std::process::exit(1);
        }));
    }
    if !fuzz.is_empty() {
        files.extend(index_fuzz_seeds(&fuzz).unwrap_or_else(|err| {
            eprintln!("tsc-bench: {err}");
            std::process::exit(1);
        }));
    }
    let mut line = String::with_capacity(512);
    for f in &files {
        line.clear();
        emit_file_line(f, &mut line);
        println!("{line}");
    }
}

/// Parses the subcommand's flag grammar (Go `flag` semantics: `--x v`,
/// `--x=v`, `-x`).
fn parse_cli(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let (name, inline) = match args[i].split_once('=') {
            Some((n, v)) => (n.trim_start_matches('-').to_string(), Some(v.to_string())),
            None => (args[i].trim_start_matches('-').to_string(), None),
        };
        if name != "corpus" && name != "fuzz" {
            return Err(format!("symbols: flag provided but not defined: -{name}"));
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("symbols: flag needs an argument: -{name}"))?
            }
        };
        out.push((name, value));
        i += 1;
    }
    Ok(out)
}

/// Walks the corpus with dump's walk/skip rules but indexes EVERY file
/// whose scriptKind is known (the binder gate's rule — .js/.mjs/.json are
/// bound too, not just the bench extensions), globally sorted by path
/// (raw string sort, like `sort.Strings`).
pub(crate) fn index_symbols_corpus(corpus: &str) -> Result<Vec<BenchFile>, String> {
    let abs = std::path::absolute(corpus).map_err(|err| format!("resolving corpus dir: {err}"))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    walk_all(&abs, &abs, &mut paths)?;
    paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path.to_string_lossy().into_owned();
        if let Some(f) = index_one(|| std::fs::read(&path), name) {
            files.push(f);
        }
    }
    Ok(files)
}

/// Recursive walk: lexical per-directory order, SKIP_DIRS pruned, symlinked
/// directories NOT followed, every file collected regardless of extension
/// (the binder gate's walk rule).
fn walk_all(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            if dir == root {
                return Err(format!("walking corpus: {err}"));
            }
            return Ok(());
        }
    };
    let mut items: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((entry.file_name().to_string_lossy().into_owned(), entry.path(), is_dir));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path, is_dir) in items {
        if is_dir {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| SKIP_DIRS.contains(&name))
            {
                continue;
            }
            walk_all(root, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Indexes one file read through `read` with the file name `name` —
/// skips unreadable/non-UTF-8-path/unknown-scriptKind files (None).
fn index_one(read: impl FnOnce() -> std::io::Result<Vec<u8>>, name: String) -> Option<BenchFile> {
    let data = read().ok()?;
    let file_name = rooted_file_path_from_absolute(&name);
    let script_kind = script_kind_from_file_name(&file_name);
    if script_kind == ScriptKind::Unknown {
        return None;
    }
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    Some(BenchFile {
        bytes: data.len() as i64,
        text: String::from_utf8_lossy(&data).into_owned(),
        path_key,
        script_kind,
        file_name,
    })
}

/// Indexes a `go test fuzz v1` seed directory (the binder gate's fuzz
/// sweep): each seed contributes one synthetic `<path>.<ext>` file.
fn index_fuzz_seeds(dir: &str) -> Result<Vec<BenchFile>, String> {
    let abs = std::path::absolute(dir).map_err(|err| format!("resolving fuzz dir: {err}"))?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&abs).map_err(|err| format!("walking fuzz dir: {err}"))? {
        let Ok(entry) = entry else { continue };
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            paths.push(entry.path());
        }
    }
    paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    let mut files = Vec::new();
    for path in paths {
        let Some((ext, text)) = read_fuzz_seed(&path) else { continue };
        let name = format!("{}.{}", path.display(), ext.trim_start_matches('.'));
        let owned = text;
        if let Some(f) = index_one(move || Ok(owned.clone().into_bytes()), name) {
            files.push(f);
        }
    }
    Ok(files)
}

/// Parses one `string("...")` corpus value line (Go strconv.Quote syntax).
/// Verbatim contract from the parser/binder gates (`parse_corpus_string`);
/// the Go driver uses `strconv.Unquote` — identical for the corpus's
/// escapes (no `\xNN`/octal bytes appear in any current seed).
fn parse_corpus_string(line: &str) -> Option<String> {
    let rest = line.strip_prefix("string(")?.strip_suffix(')')?;
    let b = rest.as_bytes();
    if b.len() < 2 || b[0] != b'"' || b[b.len() - 1] != b'"' {
        return None;
    }
    let inner = &rest[1..rest.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\x0b'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'x' => {
                let mut h = String::new();
                for _ in 0..2 {
                    h.push(chars.next()?);
                }
                out.push(u8::from_str_radix(&h, 16).ok()? as char);
            }
            'u' => {
                let mut h = String::new();
                for _ in 0..4 {
                    h.push(chars.next()?);
                }
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            'U' => {
                let mut h = String::new();
                for _ in 0..8 {
                    h.push(chars.next()?);
                }
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            d @ '0'..='7' => {
                let mut v = d.to_digit(8).unwrap();
                for _ in 0..2 {
                    v = v * 8 + chars.next()?.to_digit(8)?;
                }
                out.push(v as u8 as char);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Reads one fuzz seed: the `go test fuzz v1` header, then the extension
/// `string(".ts")` line, then the text `string("...")` line (the binder
/// gate's `read_fuzz_seed`).
fn read_fuzz_seed(path: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut lines = raw.lines();
    if lines.next()?.trim() != "go test fuzz v1" {
        return None;
    }
    let ext = parse_corpus_string(lines.next()?)?;
    let text = parse_corpus_string(lines.next()?)?;
    Some((ext, text))
}

// ──────────────────────────────────────────────────────────────────────
// The canonical walk (Go: symWalker) — see the module PORT notes
// ──────────────────────────────────────────────────────────────────────

/// The internal-name sentinel for [`canonical_name`] (Go: the single byte
/// "\xFE"; the port models it as U+00FE — see the module PORT notes).
const INTERNAL_SYMBOL_NAME_PREFIX: &str = "\u{FE}";

/// The canonical-symbol name ("__"-escaped leading internal prefix),
/// written into the record, the sort comparison and the shape_hash fold.
/// Go: `ast.EscapeInternalSymbolName` (the same sanitization the Go port's
/// baselines apply to symbol names; see the module PORT notes for why the
/// raw names are not byte-identical between the ports).
fn canonical_name_owned(name: &str) -> String {
    match name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        Some(rest) => format!("__{rest}"),
        None => name.to_string(),
    }
}

/// The canonical-key comparison order: (canonical form, raw form) —
/// identical byte ordering on both ports (see the module PORT notes).
fn sort_keys(table: &SymbolTable) -> Vec<&String> {
    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort_by(|a, b| {
        canonical_name_owned(a).cmp(&canonical_name_owned(b)).then_with(|| a.as_str().cmp(b.as_str()))
    });
    keys
}

/// State of one file's canonical walk.
struct Walker<'a> {
    file: &'a SourceFile,
    symbols: &'a [Symbol],
    seen: Vec<bool>,
    /// `SymbolId` → canonical id (or -1 while undiscovered).
    canonical: Vec<i64>,
    /// Canonical-id order (index == canonical id).
    order: Vec<SymbolId>,
    /// Discovery worklist (see the module PORT notes: members are walked
    /// before exports; both in canonical-key order).
    sym_stack: Vec<SymbolId>,
    /// Node-traversal worklist (children pushed in reverse so the pop
    /// order is the AST's ForEachChild order — true pre-order).
    node_stack: Vec<NodeId>,
    children: Vec<NodeId>,
}

impl<'a> Walker<'a> {
    fn new(file: &'a SourceFile, symbols: &'a [Symbol]) -> Walker<'a> {
        Walker {
            file,
            symbols,
            seen: vec![false; symbols.len()],
            canonical: vec![-1; symbols.len()],
            order: Vec::new(),
            sym_stack: Vec::new(),
            node_stack: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Discover every symbol reachable from `root` (idempotent).
    fn discover(&mut self, root: Option<SymbolId>) {
        let Some(root) = root else { return };
        self.sym_stack.push(root);
        while let Some(sym) = self.sym_stack.pop() {
            if self.seen[sym.0 as usize] {
                continue;
            }
            self.seen[sym.0 as usize] = true;
            self.canonical[sym.0 as usize] = self.order.len() as i64;
            self.order.push(sym);
            let s = &self.symbols[sym.0 as usize];
            // Push exports first (reversed) so members pop first: the pop
            // order is members, then exports, both in canonical-key order.
            let export_keys = sort_keys(&s.exports);
            for key in export_keys.into_iter().rev() {
                if let Some(id) = s.exports.get(key.as_str()) {
                    self.sym_stack.push(*id);
                }
            }
            let member_keys = sort_keys(&s.members);
            for key in member_keys.into_iter().rev() {
                if let Some(id) = s.members.get(key.as_str()) {
                    self.sym_stack.push(*id);
                }
            }
        }
    }

    /// The full canonical walk: AST pre-order, then the file's
    /// GlobalExports.
    fn walk(mut self) -> Vec<SymbolId> {
        self.node_stack.push(self.file.as_node_id());
        while let Some(node) = self.node_stack.pop() {
            // 1. The node's Locals table (canonical-key order).
            let locals: Vec<SymbolId> = match tsc_binder::node_data::locals_of(self.file, node) {
                Some(table) => sort_keys(table)
                    .into_iter()
                    .filter_map(|key| table.get(key.as_str()).copied())
                    .collect(),
                None => Vec::new(),
            };
            for sym in locals {
                self.discover(Some(sym));
            }
            // 2. The node's own symbol and local symbol.
            let own = tsc_binder::node_data::symbol_of(self.file, node);
            let local = tsc_binder::node_data::local_symbol_of(self.file, node);
            self.discover(own);
            self.discover(local);
            // 3. Children (reversed push = ForEachChild pop order).
            self.children.clear();
            let n = NodeStore::node(self.file, node);
            n.for_each_child(&mut |child| {
                self.children.push(child);
                false // keep going (true would stop the traversal)
            });
            for child in self.children.iter().rev() {
                self.node_stack.push(*child);
            }
        }
        // 4. The file's GlobalExports.
        let global: Vec<SymbolId> = sort_keys(&self.file.payload().global_exports)
            .into_iter()
            .filter_map(|key| self.file.payload().global_exports.get(key.as_str()).copied())
            .collect();
        for sym in global {
            self.discover(Some(sym));
        }
        self.order
    }
}

// ──────────────────────────────────────────────────────────────────────
// Record emission (Go: emitSymbolsLine — byte-identical lines)
// ──────────────────────────────────────────────────────────────────────

/// One canonical symbol record, in the fold-friendly pieces the hash
/// recipe also consumes (see `hash_lines`).
struct Record {
    name: String,
    flags: String,
    kind: i32,
    parent: Option<i64>,
    declarations: Vec<String>,
    value_declaration: Option<String>,
    exports: usize,
    members: usize,
}

/// `"kind:pos"` — a declaration's identity in the record (kind ordinal +
/// start position; the node ids themselves are lazy on Go and therefore
/// not a shared field).
fn decl_id(file: &SourceFile, node: NodeId) -> String {
    let n = NodeStore::node(file, node);
    format!("{}:{}", n.kind_value(), n.pos())
}

/// Collects the records for one bound file (canonical order).
fn records(file: &SourceFile, symbols: &[Symbol], order: &[SymbolId]) -> Vec<Record> {
    let mut canonical_ids: Vec<i64> = vec![-1; symbols.len()];
    for (i, sym) in order.iter().enumerate() {
        canonical_ids[sym.0 as usize] = i as i64;
    }
    order
        .iter()
        .map(|&sym| {
            let s = &symbols[sym.0 as usize];
            let kind = s
                .declarations
                .first()
                .map(|&d| NodeStore::node(file, d).kind_value() as i32)
                .unwrap_or(-1);
            Record {
                name: canonical_name_owned(&s.name),
                flags: format!("{:08x}", s.flags.0),
                kind,
                parent: s.parent.map(|p| canonical_ids[p.0 as usize]).filter(|p| *p >= 0),
                declarations: s.declarations.iter().map(|&d| decl_id(file, d)).collect(),
                value_declaration: s.value_declaration.map(|d| decl_id(file, d)),
                exports: s.exports.size(),
                members: s.members.size(),
            }
        })
        .collect()
}

/// The exact byte stream the shape_hash folds (exposed for tests):
/// `alloc`/`symbols` header lines, one line per record, `diag` footer.
fn hash_lines(alloc: i32, records: &[Record], diag_count: usize) -> Vec<String> {
    let mut lines = vec![
        format!("alloc={alloc}\n"),
        format!("symbols={}\n", records.len()),
    ];
    for (id, r) in records.iter().enumerate() {
        lines.push(format!(
            "{id}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            r.name,
            r.flags,
            r.kind,
            r.parent.map_or_else(|| "-".to_string(), |p| p.to_string()),
            if r.declarations.is_empty() {
                "-".to_string()
            } else {
                r.declarations.join(",")
            },
            r.value_declaration.clone().unwrap_or_else(|| "-".to_string()),
            r.exports,
            r.members,
        ));
    }
    lines.push(format!("diag={diag_count}\n"));
    lines
}

/// FNV-1a 64 fold of one line: `h ^= b; h *= 0x100000001b3` (wrapping).
fn fold(h: &mut u64, s: &str) {
    for b in s.bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The shape_hash recipe — must match the Go driver byte-for-byte.
fn shape_hash(alloc: i32, records: &[Record], diag_count: usize) -> String {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a 64 offset basis
    for line in hash_lines(alloc, records, diag_count) {
        fold(&mut h, &line);
    }
    format!("{h:016x}") // %016x, lowercase
}

/// Emits one file's NDJSON line into `line` (Go: `emitSymbolsLine`).
fn emit_file_line(f: &BenchFile, line: &mut String) {
    // Parse under catch_unwind — a panic is an ok:false line, not an abort.
    let parsed = std::panic::catch_unwind(AssertUnwindSafe(|| crate::parser_seam::bench_parse(f)));
    let mut sf = match parsed {
        Ok(sf) => sf,
        Err(payload) => {
            emit_failure_line(f, line, "parse", &panic_message(payload));
            return;
        }
    };
    let bound = std::panic::catch_unwind(AssertUnwindSafe(|| tsc_binder::bind_source_file(&mut sf)));
    let result = match bound {
        Ok(Some(result)) => result,
        Ok(None) => {
            emit_failure_line(f, line, "bind", "bind_source_file returned None on a fresh file");
            return;
        }
        Err(payload) => {
            emit_failure_line(f, line, "bind", &panic_message(payload));
            return;
        }
    };

    let order = Walker::new(&sf, &result.symbols).walk();
    let recs = records(&sf, &result.symbols, &order);
    let diags = sf.bind_diagnostics();
    let alloc = result.symbol_count;

    line.push_str("{\"file\":\"");
    json_escape(f.file_name.as_string(), line);
    line.push_str("\",\"ok\":true,\"alloc\":");
    line.push_str(&alloc.to_string());
    line.push_str(",\"symbols\":");
    line.push_str(&recs.len().to_string());
    line.push_str(",\"sym\":[");
    for (i, r) in recs.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        line.push_str("{\"id\":");
        line.push_str(&i.to_string());
        line.push_str(",\"name\":\"");
        json_escape(&r.name, line);
        line.push_str("\",\"flags\":\"");
        line.push_str(&r.flags);
        line.push_str("\",\"kind\":");
        line.push_str(&r.kind.to_string());
        line.push_str(",\"parent\":");
        match r.parent {
            Some(p) => line.push_str(&p.to_string()),
            None => line.push_str("null"),
        }
        line.push_str(",\"declarations\":[");
        for (di, d) in r.declarations.iter().enumerate() {
            if di > 0 {
                line.push(',');
            }
            line.push('"');
            json_escape(d, line);
            line.push('"');
        }
        line.push_str("],\"exports\":");
        line.push_str(&r.exports.to_string());
        line.push_str(",\"members\":");
        line.push_str(&r.members.to_string());
        line.push_str(",\"valueDeclaration\":");
        match &r.value_declaration {
            Some(v) => {
                line.push('"');
                json_escape(v, line);
                line.push('"');
            }
            None => line.push_str("null"),
        }
        line.push('}');
    }
    line.push_str("],\"diag\":[");
    for (di, d) in diags.iter().enumerate() {
        if di > 0 {
            line.push(',');
        }
        line.push_str("{\"key\":\"");
        json_escape(d.message_key(), line);
        line.push_str("\",\"code\":");
        line.push_str(&d.code().to_string());
        line.push_str(",\"pos\":");
        line.push_str(&d.pos().to_string());
        line.push_str(",\"end\":");
        line.push_str(&d.end().to_string());
        line.push_str(",\"args\":[");
        for (ai, a) in d.message_args().iter().enumerate() {
            if ai > 0 {
                line.push(',');
            }
            line.push('"');
            json_escape(a, line);
            line.push('"');
        }
        line.push_str("]}");
    }
    line.push_str("],\"shape_hash\":\"");
    line.push_str(&shape_hash(alloc, &recs, diags.len()));
    line.push_str("\"}");
}

/// The ok:false NDJSON line (parse/bind panic; `stage` is "parse"/"bind").
fn emit_failure_line(f: &BenchFile, line: &mut String, stage: &str, message: &str) {
    line.push_str("{\"file\":\"");
    json_escape(f.file_name.as_string(), line);
    line.push_str("\",\"ok\":false,\"error\":\"");
    json_escape(&format!("{stage}: {message}"), line);
    line.push_str("\"}");
}

/// Extracts the message from a Rust panic payload (truncated to 200 chars,
/// the gate's rule — a full panic payload is not a shared format).
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    msg.chars().take(200).collect()
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tsc_ast::{SourceFileParseOptions, SymbolFlags};

    fn bind(src: &str) -> (SourceFile, tsc_binder::BindResult) {
        let mut sf = crate::parser_seam::parse_source_file(
            SourceFileParseOptions::default(),
            src,
            ScriptKind::TS,
        );
        let result = tsc_binder::bind_source_file(&mut sf).expect("bind");
        (sf, result)
    }

    // ── (a) shape_hash recipe against hand-computed FNV-1a vectors ──
    //
    // FNV-1a 64: h = 0xcbf29ce484222325; per byte: h ^= b; h *= 0x100000001b3
    // (mod 2^64). Step-by-step for fold("alloc=1\n") from the offset basis
    // (XOR then multiply, byte per byte):
    //
    //   h0 = 0xcbf29ce484222325
    //   'a' → 0xaf63dc4c8601ec8c
    //   'l' → 0x089c3607b54580a0
    //   'l' → 0xe6f09b190519daa4
    //   'o' → 0x84b25683aaeec6f1
    //   'c' → 0x69cf93bb73bb6a16
    //   '=' → 0x87223285a7756711
    //   '1' → 0x1482f91b8c7e3b60
    //   '\n'→ 0x58c8b3cfba7ef51e
    //
    // Full-recipe vectors (continuing the fold over the remaining lines,
    // verified with an independent FNV-1a implementation over the same
    // line stream):
    //   A: alloc=0, no records, diag=0 → lines "alloc=0\n" "symbols=0\n"
    //      "diag=0\n" → 9e6ff26b3131cec3
    //   B: alloc=1, one record "0|a|00000001|256|-|256:0|-|0|0", diag=0
    //      → 0adb8f9a380a3bf3
    #[test]
    fn fnv1a_fold_intermediate_vector() {
        let mut h: u64 = 0xcbf29ce484222325;
        fold(&mut h, "alloc=1\n");
        assert_eq!(h, 0x58c8b3cfba7ef51e);
    }

    #[test]
    fn shape_hash_recipe_vectors() {
        // One record: name "a", flags 00000001, kind 256 (SourceFile), no
        // parent, one declaration "256:0", no valueDeclaration, 0/0.
        let r = Record {
            name: "a".to_string(),
            flags: "00000001".to_string(),
            kind: 256,
            parent: None,
            declarations: vec!["256:0".to_string()],
            value_declaration: None,
            exports: 0,
            members: 0,
        };
        // lines: "alloc=1\n" "symbols=1\n" "0|a|00000001|256|-|256:0|-|0|0\n"
        //        "diag=0\n"
        assert_eq!(
            hash_lines(1, std::slice::from_ref(&r), 0),
            vec![
                "alloc=1\n".to_string(),
                "symbols=1\n".to_string(),
                "0|a|00000001|256|-|256:0|-|0|0\n".to_string(),
                "diag=0\n".to_string(),
            ]
        );
        assert_eq!(shape_hash(1, std::slice::from_ref(&r), 0), "0adb8f9a380a3bf3");

        // Empty file: just the header/footer lines.
        assert_eq!(shape_hash(0, &[], 0), "9e6ff26b3131cec3");
    }

    // ── (b) canonical-name escaping (Go EscapeInternalSymbolName) ──
    #[test]
    fn canonical_names_mirror_go_escape() {
        assert_eq!(canonical_name_owned("\u{FE}call"), "__call");
        assert_eq!(canonical_name_owned("\u{FE}#3@x"), "__#3@x");
        assert_eq!(canonical_name_owned("plain"), "plain");
        assert_eq!(canonical_name_owned("__foo"), "__foo");
        assert_eq!(canonical_name_owned("_foo"), "_foo");
        // The raw \xFE byte never reaches the record: it is always the
        // port's U+00FE prefix, which canonicalizes to "__".
    }

    // ── (c) canonical-key sort order (canonical form first, raw second) ──
    #[test]
    fn table_keys_sort_by_canonical_form() {
        let mut table = tsc_ast::SymbolTable::new();
        table.set("zzz".into(), SymbolId(0));
        table.set("\u{FE}default".into(), SymbolId(1));
        table.set("aaa".into(), SymbolId(2));
        table.set("__default".into(), SymbolId(3));
        let keys = sort_keys(&table);
        // "__" (0x5F 0x5F) sorts before "aaa" (0x61): byte order, the same
        // order the Go driver's (canonical, raw) sort produces.
        assert_eq!(
            keys.iter().map(|k| canonical_name_owned(k)).collect::<Vec<_>>(),
            vec!["__default", "__default", "aaa", "zzz"]
        );
        // Tie between the internal "\xFEdefault" and the user "__default":
        // the RAW forms break the tie identically on both ports ('_'
        // 0x5F sorts before both the Go "\xFE" byte and the port's C3 BE).
        assert_eq!(keys[0].as_str(), "__default");
        assert_eq!(keys[1].as_str(), "\u{FE}default");
    }

    // ── (d) end-to-end: bind a fixture and check the emitted line's shape ──
    #[test]
    fn emits_records_for_a_bound_file() {
        let (sf, result) = bind("const a = 1;\n");
        let order = Walker::new(&sf, &result.symbols).walk();
        let recs = records(&sf, &result.symbols, &order);
        assert!(!recs.is_empty());
        // `a` is the file's first local: canonical id 0.
        assert_eq!(recs[0].name, "a");
        assert_eq!(recs[0].flags, format!("{:08x}", SymbolFlags::BLOCK_SCOPED_VARIABLE.0));
        assert_eq!(recs[0].parent, None);
        assert_eq!(recs[0].members, 0);
        assert_eq!(recs[0].exports, 0);
        // alloc == emitted count for this file (every symbol reachable).
        assert_eq!(result.symbol_count as usize, recs.len());
        // The NDJSON line parses and round-trips the key fields.
        let f = BenchFile {
            text: "const a = 1;\n".to_string(),
            bytes: 13,
            file_name: rooted_file_path_from_absolute("/x/a.ts"),
            path_key: CaseSensitivity::CaseSensitive
                .path_key(&rooted_file_path_from_absolute("/x/a.ts").as_path()),
            script_kind: ScriptKind::TS,
        };
        let mut line = String::new();
        emit_file_line(&f, &mut line);
        assert!(line.starts_with("{\"file\":\"/x/a.ts\",\"ok\":true,\"alloc\":"));
        assert!(line.contains("\"sym\":[{\"id\":0,\"name\":\"a\""));
        assert!(line.ends_with("\"}"));
    }

    // ── (e) class members + prototype follow the canonical order ──
    #[test]
    fn class_members_and_prototype_are_reachable() {
        let (sf, result) = bind("class C { m() {} }\n");
        let order = Walker::new(&sf, &result.symbols).walk();
        let recs = records(&sf, &result.symbols, &order);
        let names: Vec<&str> = recs.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"C"));
        assert!(names.contains(&"m"));
        assert!(names.contains(&"prototype"));
        // `prototype` is reachable only through C's members table: its
        // parent must be C's canonical id.
        let c_id = names.iter().position(|n| *n == "C").unwrap();
        let proto = recs.iter().find(|r| r.name == "prototype").unwrap();
        assert_eq!(proto.parent, Some(c_id as i64));
        assert_eq!(proto.kind, -1); // no declarations
    }

    // ── (f) the internal module symbol is node-attached, not tabled ──
    #[test]
    fn external_module_symbol_is_walked_via_node_link() {
        let (sf, result) = bind("export const x = 1;\n");
        let order = Walker::new(&sf, &result.symbols).walk();
        let recs = records(&sf, &result.symbols, &order);
        let names: Vec<&str> = recs.iter().map(|r| r.name.as_str()).collect();
        // The module symbol's name is "\"<file>.ts\"" — it is attached to
        // the SourceFile node only (bindAnonymousDeclaration).
        assert!(names.iter().any(|n| n.starts_with("\"")), "module symbol missing: {names:?}");
    }
}
