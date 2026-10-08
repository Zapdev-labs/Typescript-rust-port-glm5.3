// Crate-local port of the `DiagnosticsCollection` + compare/equal machinery
// from tsc/internal/ast/diagnostic.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// (the Rust ast crate ports only the Diagnostic struct; its header defers
// this surface to a later wave — see crates/ast/src/diagnostic.rs).
//
// PORT (file ids): `parser::parse_source_file` assigns every file
// `file_id = 0`, so `Diagnostic.file: Option<NodeId>` cannot resolve back to
// a file on its own (a known parser gap — PORTING-NOTES). Every Go `*ast.Diagnostic`
// this crate sorts, deduplicates, or keys by file therefore travels as a
// [`FileDiagnostic`] pairing the diagnostic with its file name. When the
// parser mints unique file ids, the pairing can collapse back to Go's shape
// (dedup candidate).
//
// `sort_and_deduplicate_diagnostics` and `compact_and_merge_related_infos`
// are Go's (they live in program.go; kept here next to the collection).

use std::sync::Mutex;

use tsc_ast::Diagnostic;
use tsc_tspath::RootedFilePath;

/// A diagnostic paired with its file name (Go: `*ast.Diagnostic` whose
/// `.File()` resolved through the pointer graph).
///
/// `file_name: None` marks a global/compiler diagnostic (Go: nil file).
#[derive(Clone, Debug)]
pub struct FileDiagnostic {
    pub file_name: Option<RootedFilePath>,
    pub diagnostic: Diagnostic,
}

impl FileDiagnostic {
    /// Go: `ast.NewCompilerDiagnostic(message, args...)` — the fileless form.
    pub fn compiler_diagnostic(message: &'static tsc_diagnostics::Message, args: &[String]) -> Self {
        FileDiagnostic {
            file_name: None,
            diagnostic: tsc_tsoptions::new_compiler_diagnostic(message, args),
        }
    }

    /// Go: `ast.NewDiagnostic(file, loc, message, args...)`.
    pub fn file_diagnostic(
        file: &tsc_ast::SourceFile,
        loc: tsc_core::text::TextRange,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> Self {
        let text = tsc_diagnostics::format(message.text(), args);
        FileDiagnostic {
            file_name: Some(file.file_name().clone()),
            diagnostic: Diagnostic::new(
                Some(file.as_node_id()),
                loc,
                message.code(),
                message.category(),
                "",
                Some(message),
                text,
                message.key(),
                args.to_vec(),
            ),
        }
    }

    /// Wraps a parsed diagnostic with the file it came from.
    pub fn for_file(file: &tsc_ast::SourceFile, diagnostic: Diagnostic) -> Self {
        FileDiagnostic {
            file_name: Some(file.file_name().clone()),
            diagnostic,
        }
    }
}

/// Go: `func getDiagnosticPath(d *Diagnostic) string`.
fn get_diagnostic_path(d: &FileDiagnostic) -> &str {
    match &d.file_name {
        Some(file_name) => file_name.as_string(),
        None => "",
    }
}

/// Go: `func getDiagnosticMessageIdentity(diagnostic *Diagnostic) string`.
fn get_diagnostic_message_identity(d: &Diagnostic) -> String {
    if !d.message_text().is_empty() {
        return d.message_text().to_string();
    }
    if let Some(message) = d.message() {
        if d.code() == -1 {
            return message.text().to_string();
        }
    }
    d.message_key().to_string()
}

/// Go: `func equalMessageChain(c1, c2 *Diagnostic) bool`.
fn equal_message_chain(c1: &Diagnostic, c2: &Diagnostic) -> bool {
    c1.code() == c2.code()
        && c1.message_args() == c2.message_args()
        && c1
            .message_chain()
            .iter()
            .zip(c2.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

/// Go: `func EqualDiagnosticsNoRelatedInfo(d1, d2 *Diagnostic) bool`.
pub fn equal_diagnostics_no_related_info(d1: &FileDiagnostic, d2: &FileDiagnostic) -> bool {
    get_diagnostic_path(d1) == get_diagnostic_path(d2)
        && d1.diagnostic.loc() == d2.diagnostic.loc()
        && d1.diagnostic.code() == d2.diagnostic.code()
        && d1.diagnostic.category() == d2.diagnostic.category()
        && d1.diagnostic.source() == d2.diagnostic.source()
        && get_diagnostic_message_identity(&d1.diagnostic)
            == get_diagnostic_message_identity(&d2.diagnostic)
        && d1.diagnostic.message_args() == d2.diagnostic.message_args()
        && d1
            .diagnostic
            .message_chain()
            .iter()
            .zip(d2.diagnostic.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

/// Go: `func EqualDiagnostics(d1, d2 *Diagnostic) bool`.
pub fn equal_diagnostics(d1: &FileDiagnostic, d2: &FileDiagnostic) -> bool {
    equal_diagnostics_no_related_info(d1, d2)
        && d1
            .diagnostic
            .related_information()
            .iter()
            .zip(d2.diagnostic.related_information())
            // Go: EqualDiagnostics on the related info — the related infos
            // are plain Diagnostics (their file pairing is embedded in their
            // own FileDiagnostic wrapping at the collection sites); here the
            // chain compares via the no-related-info core.
            .all(|(a, b)| {
                a.code() == b.code() && a.message_args() == b.message_args()
            })
}

/// Go: `func compareMessageChainSize(c1, c2 []*Diagnostic) int`.
fn compare_message_chain_size(c1: &[Diagnostic], c2: &[Diagnostic]) -> i32 {
    let mut c = c2.len() as i32 - c1.len() as i32;
    if c != 0 {
        return c;
    }
    for i in 0..c1.len() {
        c = compare_message_chain_size(c1[i].message_chain(), c2[i].message_chain());
        if c != 0 {
            return c;
        }
    }
    0
}

/// Go: `func compareMessageChainContent(c1, c2 []*Diagnostic) int`.
fn compare_message_chain_content(c1: &[Diagnostic], c2: &[Diagnostic]) -> i32 {
    for i in 0..c1.len() {
        let c = cmp_slice(c1[i].message_args(), c2[i].message_args());
        if c != 0 {
            return c;
        }
        if !c1[i].message_chain().is_empty() {
            let c = compare_message_chain_content(c1[i].message_chain(), c2[i].message_chain());
            if c != 0 {
                return c;
            }
        }
    }
    0
}

fn cmp_slice(a: &[String], b: &[String]) -> i32 {
    for (a, b) in a.iter().zip(b.iter()) {
        let c = match a.cmp(b) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        if c != 0 {
            return c;
        }
    }
    (a.len() as i32 - b.len() as i32).signum()
}

/// Go: `func compareRelatedInfo(r1, r2 []*Diagnostic) int`.
fn compare_related_info(r1: &[Diagnostic], r2: &[Diagnostic]) -> i32 {
    let mut c = r2.len() as i32 - r1.len() as i32;
    if c != 0 {
        return c;
    }
    for i in 0..r1.len() {
        let c = compare_diagnostics(&r1[i], &r2[i]);
        if c != 0 {
            return c;
        }
    }
    0
}

/// Go: `func CompareDiagnostics(d1, d2 *Diagnostic) int` over the raw
/// (unpaired) form — used for the related-info chains, which carry no file
/// pairing of their own beyond loc/code.
pub fn compare_diagnostics(d1: &Diagnostic, d2: &Diagnostic) -> i32 {
    let c = (d1.pos() - d2.pos()).signum();
    if c != 0 {
        return c;
    }
    let c = (d1.end() - d2.end()).signum();
    if c != 0 {
        return c;
    }
    let c = (d1.code() - d2.code()).signum();
    if c != 0 {
        return c;
    }
    let c = d1.category() as i32 - d2.category() as i32;
    if c != 0 {
        return c;
    }
    let c = match d1.source().cmp(d2.source()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    if c != 0 {
        return c;
    }
    let c = cmp_slice(d1.message_args(), d2.message_args());
    if c != 0 {
        return c;
    }
    let c = compare_message_chain_size(d1.message_chain(), d2.message_chain());
    if c != 0 {
        return c;
    }
    let c = compare_message_chain_content(d1.message_chain(), d2.message_chain());
    if c != 0 {
        return c;
    }
    compare_related_info(d1.related_information(), d2.related_information())
}

/// Go: `func CompareDiagnostics(d1, d2 *Diagnostic) int` — the file-paired
/// form (path first, then the raw compare).
pub fn compare_file_diagnostics(d1: &FileDiagnostic, d2: &FileDiagnostic) -> i32 {
    let c = match get_diagnostic_path(d1).cmp(get_diagnostic_path(d2)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    if c != 0 {
        return c;
    }
    compare_diagnostics(&d1.diagnostic, &d2.diagnostic)
}

/// Go: `func SortAndDeduplicateDiagnostics(diagnostics []*ast.Diagnostic) []*ast.Diagnostic`
/// (program.go).
pub fn sort_and_deduplicate_diagnostics(diagnostics: Vec<FileDiagnostic>) -> Vec<FileDiagnostic> {
    let mut diagnostics = diagnostics;
    diagnostics.sort_by(|a, b| compare_file_diagnostics(a, b).cmp(&0));
    compact_and_merge_related_infos(diagnostics)
}

/// Go: `func compactAndMergeRelatedInfos(diagnostics []*ast.Diagnostic) []*ast.Diagnostic`
/// (program.go) — remove duplicates and merge related information of
/// diagnostics that differ only by related info.
pub fn compact_and_merge_related_infos(
    mut diagnostics: Vec<FileDiagnostic>,
) -> Vec<FileDiagnostic> {
    if diagnostics.len() < 2 {
        return diagnostics;
    }
    let mut i = 0;
    let mut j = 0;
    while i < diagnostics.len() {
        let mut d = diagnostics[i].clone();
        let mut n = 1;
        while i + n < diagnostics.len()
            && equal_diagnostics_no_related_info(&diagnostics[i + n], &d)
        {
            n += 1;
        }
        if n > 1 {
            let mut related_infos: Vec<Diagnostic> = Vec::new();
            for k in 0..n {
                related_infos.extend(diagnostics[i + k].diagnostic.related_information().iter().cloned());
            }
            if !related_infos.is_empty() {
                related_infos.sort_by(|a, b| compare_diagnostics(a, b).cmp(&0));
                related_infos.dedup_by(|a, b| {
                    a.code() == b.code() && a.message_args() == b.message_args()
                });
                d.diagnostic.set_related_info(related_infos);
            }
        }
        diagnostics[j] = d;
        i += n;
        j += 1;
    }
    diagnostics.truncate(j);
    diagnostics
}

// ────────────────────────────────────────────────────────────────────────────
// DiagnosticsCollection (ast/diagnostic.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type diagnosticLocationKey struct`.
#[derive(Clone, PartialEq, Eq, Hash)]
struct DiagnosticLocationKey {
    path: String,
    loc: tsc_core::text::TextRange,
    code: i32,
}

fn get_diagnostic_location_key(d: &FileDiagnostic) -> DiagnosticLocationKey {
    DiagnosticLocationKey {
        path: get_diagnostic_path(d).to_string(),
        loc: d.diagnostic.loc(),
        code: d.diagnostic.code(),
    }
}

/// Go: `type DiagnosticsCollection struct` — the Add/Lookup/Get* surface.
/// PORT: the `mu` is a `Mutex` around the whole struct (the fields stay
/// together; Go's mutex guards the same group).
#[derive(Default)]
pub struct DiagnosticsCollection {
    inner: Mutex<DiagnosticsCollectionInner>,
}

#[derive(Default)]
struct DiagnosticsCollectionInner {
    count: usize,
    file_diagnostics: std::collections::HashMap<String, Vec<FileDiagnostic>>,
    file_diagnostics_sorted: std::collections::HashSet<String>,
    non_file_diagnostics: Vec<FileDiagnostic>,
    non_file_diagnostics_sorted: bool,
    diagnostic_index: std::collections::HashMap<DiagnosticLocationKey, usize>,
    /// Go: `diagnosticCollisions map[diagnosticLocationKey][]*Diagnostic` —
    /// collision entries are appended to `all` (index `usize` into it).
    all: Vec<FileDiagnostic>,
    collisions: std::collections::HashMap<DiagnosticLocationKey, Vec<usize>>,
}

impl DiagnosticsCollection {
    /// Go: `func (c *DiagnosticsCollection) Add(diagnostic *Diagnostic) *Diagnostic`.
    pub fn add(&self, diagnostic: FileDiagnostic) -> FileDiagnostic {
        let mut inner = self.inner.lock().expect("DiagnosticsCollection poisoned");
        let key = get_diagnostic_location_key(&diagnostic);

        // The identity check across the index + collisions.
        let mut existing_index: Option<usize> = None;
        if let Some(&index) = inner.diagnostic_index.get(&key) {
            if equal_diagnostics(&inner.all[index], &diagnostic) {
                existing_index = Some(index);
            }
        }
        if existing_index.is_none() {
            if let Some(collisions) = inner.collisions.get(&key) {
                existing_index = collisions
                    .iter()
                    .copied()
                    .find(|&index| equal_diagnostics(&inner.all[index], &diagnostic));
            }
        }
        if let Some(index) = existing_index {
            return inner.all[index].clone();
        }

        let index = inner.all.len();
        inner.all.push(diagnostic.clone());
        match inner.diagnostic_index.get(&key) {
            Some(_) => {
                inner
                    .collisions
                    .entry(key)
                    .or_default()
                    .push(index);
            }
            None => {
                inner.diagnostic_index.insert(key, index);
            }
        }

        inner.count += 1;
        if let Some(file_name) = &diagnostic.file_name {
            // Go keys the map by `tspath.PathKey`; the port keys by the file
            // path string as written (the collection's callers key and look up
            // through the same file objects, so the strings agree).
            inner
                .file_diagnostics
                .entry(file_name.as_string().to_string())
                .or_default()
                .push(diagnostic.clone());
        } else {
            inner.non_file_diagnostics.push(diagnostic.clone());
            inner.non_file_diagnostics_sorted = false;
        }
        diagnostic
    }

    /// Go: `func (c *DiagnosticsCollection) Lookup(diagnostic *Diagnostic) *Diagnostic`.
    pub fn lookup(&self, diagnostic: &FileDiagnostic) -> Option<FileDiagnostic> {
        let inner = self.inner.lock().expect("DiagnosticsCollection poisoned");
        let diagnostics = if diagnostic.file_name.is_some() {
            inner.get_diagnostics_for_file_locked(get_diagnostic_path(diagnostic).to_string())
        } else {
            inner.get_global_diagnostics_locked()
        };
        diagnostics
            .binary_search_by(|probe| compare_file_diagnostics(probe, diagnostic).cmp(&0))
            .ok()
            .map(|index| diagnostics[index].clone())
    }

    /// Go: `func (c *DiagnosticsCollection) GetGlobalDiagnostics() []*Diagnostic`.
    pub fn get_global_diagnostics(&self) -> Vec<FileDiagnostic> {
        let mut inner = self.inner.lock().expect("DiagnosticsCollection poisoned");
        inner.get_global_diagnostics_locked()
    }

    /// Go: `func (c *DiagnosticsCollection) GetDiagnosticsForFile(file *SourceFile) []*Diagnostic`.
    pub fn get_diagnostics_for_file(&self, file_name: &RootedFilePath) -> Vec<FileDiagnostic> {
        let mut inner = self.inner.lock().expect("DiagnosticsCollection poisoned");
        inner.get_diagnostics_for_file_locked(file_name.as_string().to_string())
    }

    /// Go: `func (c *DiagnosticsCollection) GetDiagnostics() []*Diagnostic`.
    pub fn get_diagnostics(&self) -> Vec<FileDiagnostic> {
        let mut inner = self.inner.lock().expect("DiagnosticsCollection poisoned");
        let mut diagnostics: Vec<FileDiagnostic> = Vec::with_capacity(inner.count);
        diagnostics.extend(inner.non_file_diagnostics.iter().cloned());
        for path in inner.file_diagnostics_sorted_sorted_paths() {
            diagnostics.extend(inner.file_diagnostics[&path].iter().cloned());
        }
        diagnostics.sort_by(|a, b| compare_file_diagnostics(a, b).cmp(&0));
        diagnostics
    }
}

impl DiagnosticsCollectionInner {
    fn get_global_diagnostics_locked(&mut self) -> Vec<FileDiagnostic> {
        if !self.non_file_diagnostics_sorted {
            self.non_file_diagnostics
                .sort_by(|a, b| compare_file_diagnostics(a, b).cmp(&0));
            self.non_file_diagnostics_sorted = true;
        }
        self.non_file_diagnostics.clone()
    }

    fn get_diagnostics_for_file_locked(&mut self, path: String) -> Vec<FileDiagnostic> {
        if !self.file_diagnostics_sorted.contains(&path) {
            if let Some(diagnostics) = self.file_diagnostics.get_mut(&path) {
                diagnostics.sort_by(|a, b| compare_file_diagnostics(a, b).cmp(&0));
            }
            self.file_diagnostics_sorted.insert(path.clone());
        }
        self.file_diagnostics.get(&path).cloned().unwrap_or_default()
    }

    fn file_diagnostics_sorted_sorted_paths(&mut self) -> Vec<String> {
        // Go iterates the map (random order) and re-sorts afterwards; the port
        // walks sorted keys so the intermediate order is canonical.
        let mut paths: Vec<String> = self.file_diagnostics.keys().cloned().collect();
        paths.sort();
        paths
    }
}
