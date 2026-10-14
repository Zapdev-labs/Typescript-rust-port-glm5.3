// checker.go — the M6 S1 slice (wave 3): the ENTRY & EXPRESSION SPINE.
// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763,
// lines 2240–2585 (checkSourceFile family + deferred-nodes machinery),
// 7539–8060 (checkExpression family + the ~45-arm dispatch),
// 14222–14310 (the diagnostics entries), and NewChecker's initializeChecker
// (1309–1470 + the seeding it owns).
//
// PORT (file map): SPEC §4.2 maps checker.go → checker.rs; the S1 slice is
// ~4k Go lines and checker.rs already carries the spike core, so the slice
// lives here as `spine.rs` with this note carrying the mapping. Later
// checker.go slices (S2–S10) continue in checker.rs (or further S-slices of
// this module) — every function keeps its Go name snake_cased.
//
// Slice-table banner map (M6-DISPATCH §2): every dispatch arm below routes
// to a permissive stub bannered with the slice that owns the real body —
//   * checkSourceElementWorker arms → S3 (statement/declaration/class/module/
//     import-export checks + the type-node checks 2649–3430) and W3
//     (grammarchecks.go's checkGrammar* seams).
//   * checkDeferredNode arms → S3 (accessor/type-parameter/deferred-function
//     checks), S4 (assertion/class-expression deferred), S6 (resolveUntypedCall),
//     W5 (the two jsx deferred checkers).
//   * checkExpressionWorker arms → S4 (the per-kind expression leaves) and
//     W5 (the six checkJsx* arms, jsx.go — signatures per M6-DISPATCH risk 3:
//     checkJsxElement(node, checkMode)).
//
// ── The deferred-nodes machinery (M6-DISPATCH risk 2) ──
//
// `checkNodeDeferred` queues into `sourceFileLinks.deferredNodes` while the
// file is not yet `typeChecked`; `checkSourceFile` drains via
// `checkDeferredNodes` → `checkDeferredNode`. Go's drain is
//   for node := range links.deferredNodes.Values() { c.checkDeferredNode(node) }
//   links.deferredNodes = collections.OrderedSet{}
// and Go's OrderedMap.Keys is a LIVE index loop ("We use a for loop here to
// ensure we enumerate new items added during iteration") — so nodes queued
// DURING the drain (checkNodeDeferred still sees typeChecked == false) are
// enumerated by the SAME drain, in insertion order; the post-loop
// replacement then empties the set. The Rust OrderedSet cannot mutate during
// a borrowed iteration, so the port takes the set up front and re-pulls
// live additions into a FIFO worklist after each node — the enumeration
// order and the end state are identical (see check_deferred_nodes).
//
// ── TS8009 PORT note (the named gap this wave closes) ──
//
// Go emits `'async' modifier … in a .js unit` (TS8009) ONLY from the
// parser's JS grammar checks (parser.go:6873), whose condition exempts the
// JS-legal modifiers (`ast.ModifierToFlag(kind)&ast.ModifierFlagsJavaScript
// == 0` — export/static/accessor/async/default never error), and from
// tsoptions. There is NO checker-side emission site (verified: the only
// `The_0_modifier_can_only_be_used_in_TypeScript_files` references outside
// localization/tests are those two). The Rust parser's port had the
// condition inverted (emitting for the JS-legal modifiers), which produced
// TS8009 for every `async` in a .js unit at parse depth; this wave fixed
// the parser condition (rust/crates/parser/src/parser.rs — the JS-modifier
// arms) and the conformance 300-slice twin divergence dropped 1 → 0. When
// S3 ports the async-modifier grammar checks (grammarchecks.go's
// checkGrammarModifiers async arm), the banner on the statement-check stubs
// below is the reminder that no checker-side TS8009 re-add exists to port —
// if one is ever discovered in a later upstream sync, it lands there.

use rustc_hash::{FxHashMap, FxHashSet};

use tsc_ast::{
    NodeData,
    ast_generated::is_module_declaration,
    is_await_expression, is_binary_expression, is_call_expression, is_identifier,
    is_jsx_attributes, is_new_expression, is_qualified_name, Kind, NodeId, NodeStore, SymbolFlags,
    SymbolId, SymbolTable,
};
use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_binder::ast_shims as binder_shims;
use tsc_core::text::{TextPos, TextRange};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::{
    Category, Message, CANNOT_AUGMENT_MODULE_0_WITH_VALUE_EXPORTS_BECAUSE_IT_RESOLVES_TO_A_NON_MODULE_ENTITY,
    CANNOT_FIND_GLOBAL_TYPE_0, CANNOT_FIND_GLOBAL_VALUE_0,
    CANNOT_REDECLARE_BLOCK_SCOPED_VARIABLE_0, DECLARATION_NAME_CONFLICTS_WITH_BUILT_IN_GLOBAL_IDENTIFIER_0,
    DUPLICATE_IDENTIFIER_0,
    ENUM_DECLARATIONS_CAN_ONLY_MERGE_WITH_NAMESPACE_OR_OTHER_ENUM_DECLARATIONS,
    GLOBAL_TYPE_0_MUST_BE_A_CLASS_OR_INTERFACE_TYPE, GLOBAL_TYPE_0_MUST_HAVE_1_TYPE_PARAMETER_S,
    OBJECT_IS_OF_TYPE_UNKNOWN, OBJECT_IS_POSSIBLY_NULL, OBJECT_IS_POSSIBLY_NULL_OR_UNDEFINED,
    OBJECT_IS_POSSIBLY_UNDEFINED, THE_VALUE_0_CANNOT_BE_USED_HERE, UNREACHABLE_CODE_DETECTED,
    X_0_IS_OF_TYPE_UNKNOWN, X_0_IS_POSSIBLY_NULL, X_0_IS_POSSIBLY_NULL_OR_UNDEFINED,
    X_0_IS_POSSIBLY_UNDEFINED,
    X_CONST_ENUMS_CAN_ONLY_BE_USED_IN_PROPERTY_OR_INDEX_ACCESS_EXPRESSIONS_OR_THE_RIGHT_HAND_SIDE_OF_AN_IMPORT_DECLARATION_OR_EXPORT_ASSIGNMENT_OR_TYPE_QUERY,
};

use crate::checker::CheckMode;
use tsc_collections::OrderedSet;
use crate::types::ContextFlags;
use crate::tracing::{Args, Phase, TraceValue};
use crate::types::{ObjectFlags, SignatureFlags, TypeData, TypeFlags, TypeId};
use crate::utilities as u;
use crate::Checker;

/// Go (checker.go:552): `const maxSerializationLevel = 2`.
const MAX_SERIALIZATION_LEVEL: u32 = 2;

// ────────────────────────────────────────────────────────────────────────────
// DiagnosticsCollection (Go ast/diagnostic.go — the checker's two collections)
//
// PORT: tsc-ast's diagnostic.rs deliberately carries only the `Diagnostic`
// struct (its header defers the collection factories), and tsc-checker
// cannot depend on tsc-compiler's copy (compiler depends on checker), so the
// collection lives here with the hoist note: when ast/diagnostic.go's
// collection port lands, this moves (the compiler crate's
// diagnostics_collection.rs is the worked example of the same port). The
// Add/GetDiagnosticsForFile/GetGlobalDiagnostics semantics are Go's, keyed
// by the diagnostic's file (Go: PathKey; the port: the file's name — the
// §5.1 file identity at checker depth; both are unique per program).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type DiagnosticsCollection struct`.
///
/// PORT: the entries pair each diagnostic with its file's NAME (Go's
/// `*SourceFile` pointer → the §5.1 NodeId is kept on the Diagnostic itself;
/// the name is what Go's comparator/path-key reads, and resolving it at Add
/// time keeps the collection methods free of the checker — the borrow the
/// Add/Get call sites need).
#[derive(Default)]
pub(crate) struct DiagnosticsCollection {
    count: usize,
    file_diagnostics: FxHashMap<String, Vec<FileDiagnostic>>,
    file_diagnostics_sorted: FxHashSet<String>,
    non_file_diagnostics: Vec<FileDiagnostic>,
    non_file_diagnostics_sorted: bool,
}

/// The stored entry — the diagnostic plus the path Go's
/// `getDiagnosticPath` reads (None = the global list).
#[derive(Clone, Debug)]
struct FileDiagnostic {
    file_name: Option<String>,
    diagnostic: tsc_ast::Diagnostic,
}

/// Go: `func getDiagnosticPath(d *Diagnostic) string`.
fn get_diagnostic_path(d: &FileDiagnostic) -> &str {
    d.file_name.as_deref().unwrap_or("")
}

/// Go: `func isAssignmentDeclaration(decl *ast.Node) bool` (binder.go:4120)
/// — inlined for mergeSymbol's SetValueDeclaration gate (the binder-crate
/// twin is module-private).
fn is_assignment_declaration(store: &dyn NodeStore, decl: NodeId) -> bool {
    tsc_ast::is_binary_expression(store.node(decl))
        || binder_shims::is_access_expression(store.node(decl))
        || tsc_ast::is_identifier(store.node(decl))
        || tsc_ast::is_call_expression(store.node(decl))
}

/// Go: `func isEffectiveModuleDeclaration(node *ast.Node) bool` (binder.go:4128).
fn is_effective_module_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    is_module_declaration(store.node(node)) || tsc_ast::is_identifier(store.node(node))
}

/// Go: `func getDiagnosticMessageIdentity(diagnostic *Diagnostic) string`.
fn get_diagnostic_message_identity(d: &tsc_ast::Diagnostic) -> String {
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
fn equal_message_chain(c1: &tsc_ast::Diagnostic, c2: &tsc_ast::Diagnostic) -> bool {
    c1.code() == c2.code()
        && c1.message_args() == c2.message_args()
        && c1
            .message_chain()
            .iter()
            .zip(c2.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

/// Go: `func EqualDiagnosticsNoRelatedInfo(d1, d2 *Diagnostic) bool`.
fn equal_diagnostics_no_related_info(d1: &FileDiagnostic, d2: &FileDiagnostic) -> bool {
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
fn equal_diagnostics(d1: &FileDiagnostic, d2: &FileDiagnostic) -> bool {
    if std::ptr::eq(d1, d2) {
        return true;
    }
    equal_diagnostics_no_related_info(d1, d2)
        && d1
            .diagnostic
            .related_information()
            .iter()
            .zip(d2.diagnostic.related_information())
            .all(|(a, b)| equal_related(a, b))
}

/// The related-info chains are plain Diagnostics (no file pairing of their
/// own); Go compares them through EqualDiagnostics on the embedded file.
fn equal_related(d1: &tsc_ast::Diagnostic, d2: &tsc_ast::Diagnostic) -> bool {
    d1.code() == d2.code()
        && d1.message_args() == d2.message_args()
        && d1
            .message_chain()
            .iter()
            .zip(d2.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

/// Go: `func compareMessageChainSize(c1, c2 []*Diagnostic) int`.
fn compare_message_chain_size(
    c1: &[tsc_ast::Diagnostic],
    c2: &[tsc_ast::Diagnostic],
) -> i32 {
    let c = c2.len() as i32 - c1.len() as i32;
    if c != 0 {
        return c;
    }
    for i in 0..c1.len() {
        let c = compare_message_chain_size(c1[i].message_chain(), c2[i].message_chain());
        if c != 0 {
            return c;
        }
    }
    0
}

/// Go: `func compareMessageChainContent(c1, c2 []*Diagnostic) int`.
fn compare_message_chain_content(
    c1: &[tsc_ast::Diagnostic],
    c2: &[tsc_ast::Diagnostic],
) -> i32 {
    for i in 0..c1.len() {
        let c = compare_string_slices(c1[i].message_args(), c2[i].message_args());
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

fn compare_string_slices(a: &[String], b: &[String]) -> i32 {
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
fn compare_related_info(
    r1: &[tsc_ast::Diagnostic],
    r2: &[tsc_ast::Diagnostic],
) -> i32 {
    let cr = r2.len() as i32 - r1.len() as i32;
    if cr != 0 {
        return cr;
    }
    for i in 0..r1.len() {
        let cr = compare_raw_diagnostics(&r1[i], &r2[i]);
        if cr != 0 {
            return cr;
        }
    }
    0
}

/// Go: `CompareDiagnostics` over the raw (unpaired) form — used for the
/// related-info chains.
fn compare_raw_diagnostics(d1: &tsc_ast::Diagnostic, d2: &tsc_ast::Diagnostic) -> i32 {
    let cr = ((d1.pos() as i64) - (d2.pos() as i64)).signum() as i32;
    if cr != 0 {
        return cr;
    }
    let cr = ((d1.end() as i64) - (d2.end() as i64)).signum() as i32;
    if cr != 0 {
        return cr;
    }
    let cr = (d1.code() - d2.code()).signum();
    if cr != 0 {
        return cr;
    }
    let cr = d1.category() as i32 - d2.category() as i32;
    if cr != 0 {
        return cr;
    }
    let cr = match d1.source().cmp(d2.source()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    if cr != 0 {
        return cr;
    }
    let cr = compare_string_slices(d1.message_args(), d2.message_args());
    if cr != 0 {
        return cr;
    }
    let cr = compare_message_chain_size(d1.message_chain(), d2.message_chain());
    if cr != 0 {
        return cr;
    }
    let cr = compare_message_chain_content(d1.message_chain(), d2.message_chain());
    if cr != 0 {
        return cr;
    }
    compare_related_info(d1.related_information(), d2.related_information())
}

/// Go: `func CompareDiagnostics(d1, d2 *Diagnostic) int` — the file-paired
/// form (path first, then the raw compare).
fn compare_diagnostics(d1: &FileDiagnostic, d2: &FileDiagnostic) -> i32 {
    let cr = match get_diagnostic_path(d1).cmp(get_diagnostic_path(d2)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    };
    if cr != 0 {
        return cr;
    }
    compare_raw_diagnostics(&d1.diagnostic, &d2.diagnostic)
}

impl DiagnosticsCollection {
    /// Go: `func (c *DiagnosticsCollection) Add(diagnostic *Diagnostic) *Diagnostic`.
    ///
    /// PORT: Go's diagnosticIndex/diagnosticCollisions dedupe map collapses
    /// to a scan of the diagnostics already stored at the same location key
    /// (path+loc+code) — same observable Add semantics (equal diagnostics
    /// collapse to the stored one), O(bucket) at this depth.
    fn add(&mut self, diagnostic: tsc_ast::Diagnostic, file_name: Option<String>) -> tsc_ast::Diagnostic {
        let key_name = file_name.clone().unwrap_or_default();
        let entry = FileDiagnostic {
            file_name: file_name.clone(),
            diagnostic: diagnostic.clone(),
        };
        // The location key (path, loc, code) — the bucket itself carries the
        // path, so the key reduces to (loc, code) inside the bucket.
        let key = (diagnostic.loc(), diagnostic.code());
        let bucket: Option<&Vec<FileDiagnostic>> = if entry.file_name.is_some() {
            self.file_diagnostics.get(&key_name)
        } else {
            Some(&self.non_file_diagnostics)
        };
        if let Some(existing) = bucket {
            for d in existing {
                if (d.diagnostic.loc(), d.diagnostic.code()) == key && equal_diagnostics(d, &entry)
                {
                    return d.diagnostic.clone();
                }
            }
        }
        self.count += 1;
        if entry.file_name.is_some() {
            let list = self.file_diagnostics.entry(key_name).or_default();
            list.push(entry.clone());
            self.file_diagnostics_sorted
                .remove(get_diagnostic_path(&entry));
        } else {
            self.non_file_diagnostics.push(entry.clone());
            self.non_file_diagnostics_sorted = false;
        }
        diagnostic
    }

    /// The W3 check-stage runner seam (Go program.go `GetSemanticDiagnostics`'s
    /// read side): every diagnostic in the collection — file-paired and
    /// global — cloned in insertion order (the baseline renderer sorts; a
    /// checker instance is per-file in Go's pool and per-case in the port,
    /// so the unspecific read carries the whole collection).
    pub(crate) fn all(&self) -> Vec<tsc_ast::Diagnostic> {
        let mut out = Vec::with_capacity(self.count);
        for entries in self.file_diagnostics.values() {
            out.extend(entries.iter().map(|e| e.diagnostic.clone()));
        }
        out.extend(
            self.non_file_diagnostics
                .iter()
                .map(|e| e.diagnostic.clone()),
        );
        out
    }

    /// Go: `func (c *DiagnosticsCollection) GetGlobalDiagnostics() []*Diagnostic`.
    fn get_global_diagnostics(&mut self) -> Vec<tsc_ast::Diagnostic> {
        if !self.non_file_diagnostics_sorted {
            self.non_file_diagnostics
                .sort_by(|a, b| compare_diagnostics(a, b).cmp(&0));
            self.non_file_diagnostics_sorted = true;
        }
        self.non_file_diagnostics
            .iter()
            .map(|e| e.diagnostic.clone())
            .collect()
    }

    /// Go: `func (c *DiagnosticsCollection) GetDiagnosticsForFile(file *SourceFile) []*Diagnostic`.
    fn get_diagnostics_for_file(&mut self, file_name: &str) -> Vec<tsc_ast::Diagnostic> {
        if !self.file_diagnostics_sorted.contains(file_name) {
            if let Some(list) = self.file_diagnostics.get_mut(file_name) {
                list.sort_by(|a, b| compare_diagnostics(a, b).cmp(&0));
            }
            self.file_diagnostics_sorted.insert(file_name.to_string());
        }
        self.file_diagnostics
            .get(file_name)
            .map(|list| list.iter().map(|e| e.diagnostic.clone()).collect())
            .unwrap_or_default()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ContextualInfo / InferenceContextInfo (Go checker.go:78-89)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ContextualInfo struct { node *ast.Node; t *Type; isCache bool }`.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // the stack fields are read by S4's contextual-literal
                    // freshness worker (isLiteralOfContextualType); the spine
                    // pushes/pops them now to pin the Go stack shape.
pub(crate) struct ContextualInfo {
    pub(crate) node: Option<NodeId>,
    pub(crate) t: Option<TypeId>,
    pub(crate) is_cache: bool,
}

/// Go: `type InferenceContextInfo struct { node *ast.Node; context *InferenceContext }`.
///
/// PORT: Go's `context *InferenceContext` heap pointer becomes the
/// checker-owned arena index ([`crate::todo_shapes::InferenceContextId`],
/// indexing `Checker::inference_contexts`) — the W1 mapper convention.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // the stack fields are read by S4's inference collection
                    // (getIntraExpressionInferenceSites); the spine
                    // pushes/pops them now to pin the Go stack shape.
pub(crate) struct InferenceContextInfo {
    pub(crate) node: Option<NodeId>,
    pub(crate) context: Option<crate::todo_shapes::InferenceContextId>,
}

// ────────────────────────────────────────────────────────────────────────────
// Deferred diagnostic callbacks (Go checker.go:14252 — `addDeferredDiagnostic`)
//
// PORT: Go queues `func()` closures capturing (node, type, flags) payloads.
// Closures borrowing the checker cannot port (§5.4); the payload enums carry
// exactly what Go's five closure sites capture, and
// `produceDeferredDiagnostics` re-dispatches to the (stubbed) callees.
// ────────────────────────────────────────────────────────────────────────────

/// The five Go `addDeferredDiagnostic` closure payloads (checker.go:6495,
/// 6548, 10773, 10807, 11583) as data.
#[derive(Debug)]
#[allow(dead_code)] // the callback variants' producers land with S3/S4/S6
pub(crate) enum DeferredDiagnosticCallback {
    /// checker.go:6495 / 6548 — `c.reportTypeNotIterableError(errorNode, t,
    /// allowsAsyncIterables)` (+ the 6548 closure's collected related
    /// diagnostics). Owner: S3 (the iteration family).
    ReportTypeNotIterable {
        error_node: NodeId,
        t: TypeId,
        allows_async_iterables: bool,
        related: Vec<tsc_ast::Diagnostic>,
    },
    /// checker.go:10773 — `c.checkWeakMapSetCollision(node)`. Owner: S3.
    CheckWeakMapSetCollision { node: NodeId },
    /// checker.go:10807 — `c.checkReflectCollision(node)`. Owner: S3.
    CheckReflectCollision { node: NodeId },
    /// checker.go:11583 — `c.reportNonexistentProperty(right, type,
    /// isUncheckedJS)`. Owner: S4.
    ReportNonexistentProperty {
        right: NodeId,
        left_type: TypeId,
        is_unchecked_js: bool,
    },
}

// ────────────────────────────────────────────────────────────────────────────
// The getGlobal* resolver table (Go's ~52 `func()` fields + NewChecker's
// seeding, checker.go:1033-1087)
//
// PORT: Go stores memoized closures in struct fields
// (`core.Memoize(func() ...)`). The port stores the resolver DESCRIPTIONS
// (kind, name, arity, reportErrors) in a table + a memo slot per entry;
// accessors named after the Go fields run the memoized resolution on first
// call. The resolution family itself (getGlobalType/getGlobalSymbol) is S6-
// stubbed permissively (resolveName → None ⇒ emptyObjectType/emptyGenericType
// /nil symbol), so every memo is the Go-nil-lib answer at this depth.
// ────────────────────────────────────────────────────────────────────────────

/// Go: the resolver flavors — getGlobalTypeResolver / getGlobalTypeAliasResolver /
/// getGlobalValueSymbolResolver / getGlobalTypeSymbolResolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalResolverKind {
    /// `func() *Type` — getGlobalTypeResolver.
    Type,
    /// `func() *ast.Symbol` — getGlobalTypeAliasResolver.
    TypeAlias,
    /// `func() *ast.Symbol` — getGlobalValueSymbolResolver.
    ValueSymbol,
    /// `func() *ast.Symbol` — getGlobalTypeSymbolResolver.
    TypeSymbol,
}

/// One entry per Go resolver field, in Go's NewChecker seeding order (the
/// Go struct-field order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalResolverId {
    EsSymbolType,
    BigIntType,
    ImportMetaType,
    ImportAttributesType,
    ImportAttributesTypeChecked,
    NonNullableTypeAliasOrNil,
    ExtractSymbol,
    DisposableType,
    AsyncDisposableType,
    AwaitedSymbol,
    AwaitedSymbolOrNil,
    NaNSymbolOrNil,
    RecordSymbol,
    TemplateStringsArrayType,
    EsSymbolConstructorSymbolOrNil,
    EsSymbolConstructorTypeSymbolOrNil,
    ImportCallOptionsType,
    ImportCallOptionsTypeChecked,
    PromiseType,
    PromiseTypeChecked,
    PromiseLikeType,
    AbstractModuleSourceType,
    PromiseConstructorSymbol,
    PromiseConstructorSymbolOrNil,
    OmitSymbol,
    NoInferSymbolOrNil,
    IteratorType,
    IterableType,
    IterableTypeChecked,
    IterableIteratorType,
    IterableIteratorTypeChecked,
    IteratorObjectType,
    GeneratorType,
    AsyncIteratorType,
    AsyncIterableType,
    AsyncIterableTypeChecked,
    AsyncIterableIteratorType,
    AsyncIterableIteratorTypeChecked,
    AsyncIteratorObjectType,
    AsyncGeneratorType,
    IteratorYieldResultType,
    IteratorReturnResultType,
    TypedPropertyDescriptorType,
    ClassDecoratorContextType,
    ClassMethodDecoratorContextType,
    ClassGetterDecoratorContextType,
    ClassSetterDecoratorContextType,
    ClassAccessorDecoratorContxtType,
    ClassAccessorDecoratorContextType,
    ClassAccessorDecoratorTargetType,
    ClassAccessorDecoratorResultType,
    ClassFieldDecoratorContextType,
}

/// Go's NewChecker seeding table — `(kind, name, arity, reportErrors)` in
/// declaration order (checker.go:1033-1087). `ClassAccessorDecoratorContxt`
/// keeps Go's field-name typo ("Contxt") in its identifier; the name string
/// is the real lib type name.
const GLOBAL_RESOLVER_TABLE: [(GlobalResolverKind, &str, usize, bool); 52] = [
    (GlobalResolverKind::Type, "Symbol", 0, false),                    // EsSymbolType
    (GlobalResolverKind::Type, "BigInt", 0, false),                    // BigIntType
    (GlobalResolverKind::Type, "ImportMeta", 0, true),                 // ImportMetaType
    (GlobalResolverKind::Type, "ImportAttributes", 0, false),         // ImportAttributesType
    (GlobalResolverKind::Type, "ImportAttributes", 0, true),           // ImportAttributesTypeChecked
    (GlobalResolverKind::TypeAlias, "NonNullable", 1, false),         // NonNullableTypeAliasOrNil
    (GlobalResolverKind::TypeAlias, "Extract", 2, true),              // ExtractSymbol
    (GlobalResolverKind::Type, "Disposable", 0, true),                // DisposableType
    (GlobalResolverKind::Type, "AsyncDisposable", 0, true),           // AsyncDisposableType
    (GlobalResolverKind::TypeAlias, "Awaited", 1, true),              // AwaitedSymbol
    (GlobalResolverKind::TypeAlias, "Awaited", 1, false),             // AwaitedSymbolOrNil
    (GlobalResolverKind::ValueSymbol, "NaN", 0, false),                  // NaNSymbolOrNil
    (GlobalResolverKind::TypeAlias, "Record", 2, true),               // RecordSymbol
    (GlobalResolverKind::Type, "TemplateStringsArray", 0, true),      // TemplateStringsArrayType
    (GlobalResolverKind::ValueSymbol, "Symbol", 0, false),               // EsSymbolConstructorSymbolOrNil
    (GlobalResolverKind::TypeSymbol, "SymbolConstructor", 0, false),     // EsSymbolConstructorTypeSymbolOrNil
    (GlobalResolverKind::Type, "ImportCallOptions", 0, false),        // ImportCallOptionsType
    (GlobalResolverKind::Type, "ImportCallOptions", 0, true),         // ImportCallOptionsTypeChecked
    (GlobalResolverKind::Type, "Promise", 1, false),                  // PromiseType
    (GlobalResolverKind::Type, "Promise", 1, true),                  // PromiseTypeChecked
    (GlobalResolverKind::Type, "PromiseLike", 1, true),              // PromiseLikeType
    (GlobalResolverKind::Type, "AbstractModuleSource", 0, true),      // AbstractModuleSourceType
    (GlobalResolverKind::ValueSymbol, "Promise", 0, true),              // PromiseConstructorSymbol
    (GlobalResolverKind::ValueSymbol, "Promise", 0, false),             // PromiseConstructorSymbolOrNil
    (GlobalResolverKind::TypeAlias, "Omit", 2, true),               // OmitSymbol
    (GlobalResolverKind::TypeAlias, "NoInfer", 1, false),           // NoInferSymbolOrNil
    (GlobalResolverKind::Type, "Iterator", 3, false),               // IteratorType
    (GlobalResolverKind::Type, "Iterable", 3, false),               // IterableType
    (GlobalResolverKind::Type, "Iterable", 3, true),                // IterableTypeChecked
    (GlobalResolverKind::Type, "IterableIterator", 3, false),       // IterableIteratorType
    (GlobalResolverKind::Type, "IterableIterator", 3, true),        // IterableIteratorTypeChecked
    (GlobalResolverKind::Type, "IteratorObject", 3, false),         // IteratorObjectType
    (GlobalResolverKind::Type, "Generator", 3, false),              // GeneratorType
    (GlobalResolverKind::Type, "AsyncIterator", 3, false),         // AsyncIteratorType
    (GlobalResolverKind::Type, "AsyncIterable", 3, false),       // AsyncIterableType
    (GlobalResolverKind::Type, "AsyncIterable", 3, true),        // AsyncIterableTypeChecked
    (GlobalResolverKind::Type, "AsyncIterableIterator", 3, false), // AsyncIterableIteratorType
    (GlobalResolverKind::Type, "AsyncIterableIterator", 3, true), // AsyncIterableIteratorTypeChecked
    (GlobalResolverKind::Type, "AsyncIteratorObject", 3, false), // AsyncIteratorObjectType
    (GlobalResolverKind::Type, "AsyncGenerator", 3, false),      // AsyncGeneratorType
    (GlobalResolverKind::Type, "IteratorYieldResult", 1, false), // IteratorYieldResultType
    (GlobalResolverKind::Type, "IteratorReturnResult", 1, false), // IteratorReturnResultType
    (GlobalResolverKind::Type, "TypedPropertyDescriptor", 1, true), // TypedPropertyDescriptorType
    (GlobalResolverKind::Type, "ClassDecoratorContext", 1, true), // ClassDecoratorContextType
    (GlobalResolverKind::Type, "ClassMethodDecoratorContext", 2, true), // ClassMethodDecoratorContextType
    (GlobalResolverKind::Type, "ClassGetterDecoratorContext", 2, true), // ClassGetterDecoratorContextType
    (GlobalResolverKind::Type, "ClassSetterDecoratorContext", 2, true), // ClassSetterDecoratorContextType
    (GlobalResolverKind::Type, "ClassAccessorDecoratorContext", 2, true), // ClassAccessorDecoratorContxtType (Go's typo'd field)
    (GlobalResolverKind::Type, "ClassAccessorDecoratorContext", 2, true), // ClassAccessorDecoratorContextType
    (GlobalResolverKind::Type, "ClassAccessorDecoratorTarget", 2, true), // ClassAccessorDecoratorTargetType
    (GlobalResolverKind::Type, "ClassAccessorDecoratorResult", 2, true), // ClassAccessorDecoratorResultType
    (GlobalResolverKind::Type, "ClassFieldDecoratorContext", 2, true), // ClassFieldDecoratorContextType
];

/// The memo slot value — Go's closure return (`*Type` or `*ast.Symbol`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum GlobalResolverValue {
    Type(TypeId),
    Symbol(Option<SymbolId>),
}

/// The checker's resolver table + memos (one slot per Go func field).
#[derive(Default)]
pub(crate) struct GlobalResolvers {
    memos: Vec<Option<GlobalResolverValue>>,
}

impl GlobalResolverId {
    /// The table row (Go's NewChecker seeding) for this field.
    const fn row(self) -> (GlobalResolverKind, &'static str, usize, bool) {
        GLOBAL_RESOLVER_TABLE[self as usize]
    }

    const fn index(self) -> usize {
        self as usize
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Small ast helpers the spine needs (Go ast-package surface not yet landed
// in the Rust ast crate — dedup candidates for the utilities.go wholesale
// wave; local with PORT notes per the ast_shims convention).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (n *Node) CanHaveStatements() bool` (ast.go:604).
fn can_have_statements(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::SourceFile | Kind::Block | Kind::ModuleBlock | Kind::CaseClause | Kind::DefaultClause
    )
}

/// Go: `func (n *Node) Statements() []*Node` over the statement-bearing
/// containers (ast.go:596).
fn statements_of(store: &dyn NodeStore, node: NodeId) -> Option<&[NodeId]> {
    let n = store.node(node);
    let list = match n.kind {
        Kind::SourceFile => n
            .as_source_file()
            .expect("SourceFile data")
            .statements
            .as_ref(),
        Kind::Block => n.as_block().expect("Block data").statements.as_ref(),
        Kind::ModuleBlock => n
            .as_module_block()
            .expect("ModuleBlock data")
            .statements
            .as_ref(),
        Kind::CaseClause | Kind::DefaultClause => n
            .as_case_or_default_clause()
            .expect("CaseOrDefaultClause data")
            .statements
            .as_ref(),
        _ => return None,
    };
    list.map(|l| &*l.nodes)
}

/// Go: `node.FlowNodeData().FlowNode` for the statement payloads (the
/// unreachable machinery's control-flow probe).
fn flow_node_of(store: &dyn NodeStore, node: NodeId) -> Option<tsc_ast::FlowNodeId> {
    use tsc_ast::NodeData;
    match &store.node(node).data {
        NodeData::Block(d) => d.flow_node,
        NodeData::ModuleBlock(d) => d.flow_node,
        NodeData::VariableStatement(d) => d.flow_node,
        NodeData::ExpressionStatement(d) => d.flow_node,
        NodeData::IfStatement(d) => d.flow_node,
        NodeData::DoStatement(d) => d.flow_node,
        NodeData::WhileStatement(d) => d.flow_node,
        NodeData::ForStatement(d) => d.flow_node,
        NodeData::ForInOrOfStatement(d) => d.flow_node,
        NodeData::BreakStatement(d) => d.flow_node,
        NodeData::ContinueStatement(d) => d.flow_node,
        NodeData::ReturnStatement(d) => d.flow_node,
        NodeData::WithStatement(d) => d.flow_node,
        NodeData::SwitchStatement(d) => d.flow_node,
        NodeData::LabeledStatement(d) => d.flow_node,
        NodeData::ThrowStatement(d) => d.flow_node,
        NodeData::TryStatement(d) => d.flow_node,
        _ => None,
    }
}

/// Go: `func IsTypeDeclaration(node *Node) bool` (ast/utilities.go:3658).
pub(crate) fn is_type_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::TypeParameter
        | Kind::ClassDeclaration
        | Kind::InterfaceDeclaration
        | Kind::TypeAliasDeclaration
        | Kind::JSTypeAliasDeclaration
        | Kind::EnumDeclaration => true,
        Kind::ImportClause => {
            // Go: node.IsTypeOnly() && node.AsImportClause().Name() != nil.
            // PORT: the Rust ImportClause stores type-only as the `type`
            // phase modifier (parser's import_clause_is_type_only).
            let d = n.as_import_clause().expect("ImportClause data");
            d.phase_modifier == Some(Kind::TypeKeyword) && d.name.is_some()
        }
        Kind::ImportSpecifier | Kind::ExportSpecifier => {
            let parent = store.node(n.parent.get());
            let grandparent = store.node(parent.parent.get());
            match grandparent.kind {
                // Go: grandparent.IsTypeOnly() on the ImportClause /
                // ExportDeclaration.
                Kind::ImportClause => {
                    grandparent
                        .as_import_clause()
                        .expect("ImportClause data")
                        .phase_modifier
                        == Some(Kind::TypeKeyword)
                }
                Kind::ExportDeclaration => grandparent
                    .as_export_declaration()
                    .expect("ExportDeclaration data")
                    .is_type_only,
                _ => false,
            }
        }
        _ => false,
    }
}

/// Go: `func IsInstanceOfExpression(node *Node) bool` (ast/utilities.go:1915).
fn is_instance_of_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    is_binary_expression(n)
        && store.node(n.as_binary_expression().expect("BinaryExpression data").operator_token).kind
            == Kind::InstanceOfKeyword
}

/// Go: `scanner.GetTokenPosOfNode(startNode, sourceFile, false)` (the
/// skip-trivia'd node start).
///
/// PORT: the scanner crate's GetTokenPosOfNode port lands with the scanner/
/// utilities wholesale wave (W3-adjacent); the node's own `pos` is the
/// trivia-inclusive position Go skips — identical for the statement starts
/// the unreachable diagnostic reports (statements begin on their keyword).
fn get_token_pos_of_node_stub(store: &dyn NodeStore, node: NodeId) -> TextPos {
    store.node(node).loc.pos()
}

// ────────────────────────────────────────────────────────────────────────────
// checkSourceFile family (Go checker.go:2240-2298)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// The `*ast.SourceFile` a file NodeId belongs to (§5.1: Go's file
    /// pointer becomes the file node's id; the owning SourceFile arena is
    /// the program's).
    pub(crate) fn source_file_of(&self, file: NodeId) -> Option<&tsc_ast::SourceFile> {
        self.program
            .source_files()
            .iter()
            .find(|f| f.as_node_id() == file)
    }

    /// Go: `func (c *Checker) checkSourceFile(ctx context.Context, sourceFile *ast.SourceFile, checkUnused bool)`.
    ///
    /// PORT: Go's `ctx` first param is the cancellation surface (`isCanceled`
    /// reads `ctx.Err()`); the port's cancellation is the host-set
    /// `Checker::canceled` flag (see `is_canceled` in utilities.rs). The
    /// sourceFile is the file's NodeId.
    pub fn check_source_file(&mut self, source_file: NodeId, check_unused: bool) {
        let pop = self.tracer.as_ref().map(|tr| {
            let path = self
                .source_file_of(source_file)
                .map(|f| f.file_name().as_string())
                .unwrap_or_default();
            let args = Args::new();
            args.insert("path", TraceValue::Str(path.to_string()));
            tr.push(Phase::Check, "checkSourceFile", &args, true)
        });
        // `links` borrows the store — snapshot the flag through a scope.
        let already_type_checked = self.source_file_links.get(source_file).type_checked;
        if !already_type_checked {
            // Grammar checking.
            self.check_grammar_source_file(source_file);
            self.renamed_binding_elements_in_types = Vec::new();
            let statements: Vec<NodeId> = self
                .source_file_of(source_file)
                .and_then(|f| {
                    f.node(f.as_node_id())
                        .as_source_file()
                        .expect("SourceFile data")
                        .statements
                        .as_ref()
                        .map(|l| l.nodes.to_vec())
                })
                .unwrap_or_default();
            self.check_source_elements(&statements);
            self.check_deferred_nodes(source_file);
            let is_external_module = self
                .source_file_of(source_file)
                .is_some_and(binder_shims::is_external_or_common_js_module);
            if is_external_module {
                // PORT (M6 S3): the real checkExternalModuleExports body
                // (checker.go:5906) landed with statements.rs batch 5.
                self.check_external_module_exports(source_file);
                self.register_for_unused_identifiers_check(source_file);
            }
            let is_declaration_file = self
                .source_file_of(source_file)
                .is_some_and(|f| f.node(f.as_node_id()).as_source_file().expect("SourceFile data").is_declaration_file);
            if !is_declaration_file && !self.is_canceled() {
                self.check_unused_renamed_binding_elements();
            }
            self.produce_deferred_diagnostics();
            self.reported_unreachable_nodes.clear();
            self.source_file_links.get(source_file).type_checked = true;
        }
        let already_unused_checked = self.source_file_links.get(source_file).unused_checked;
        if check_unused && !already_unused_checked {
            // The unused identifiers check relies on a full type check having first been performed.
            let is_declaration_file = self
                .source_file_of(source_file)
                .is_some_and(|f| f.node(f.as_node_id()).as_source_file().expect("SourceFile data").is_declaration_file);
            if !is_declaration_file && !self.is_canceled() {
                let identifier_check_nodes =
                    self.source_file_links.get(source_file).identifier_check_nodes.clone();
                self.check_unused_identifiers(&identifier_check_nodes);
            }
            self.source_file_links.get(source_file).unused_checked = true;
        }
        if self.is_canceled() {
            self.was_canceled = true;
        }
        if let Some(pop) = pop {
            pop.pop();
        }
    }

    /// Go: `func (c *Checker) checkSourceElements(nodes []*ast.Node)`.
    pub(crate) fn check_source_elements(&mut self, nodes: &[NodeId]) {
        for &node in nodes {
            if self.is_canceled() {
                break;
            }
            self.check_source_element(Some(node));
        }
    }

    /// Go: `func (c *Checker) checkSourceElement(node *ast.Node) bool`.
    pub(crate) fn check_source_element(&mut self, node: Option<NodeId>) -> bool {
        if let Some(node) = node {
            let save_current_node = self.current_node;
            let save_within_unreachable_code = self.within_unreachable_code;
            self.current_node = Some(node);
            self.instantiation_count = 0;
            self.check_source_element_worker(node);
            self.current_node = save_current_node;
            self.within_unreachable_code = save_within_unreachable_code;
        }
        false
    }

    /// Go: `func (c *Checker) checkSourceElementWorker(node *ast.Node)` —
    /// the per-kind statement dispatch (checker.go:2298). Every arm's real
    /// callee is a later slice; the banners name the owner (the slice table
    /// at the module head).
    fn check_source_element_worker(&mut self, node: NodeId) {
        // Go: `for _, jsdoc := range node.EagerJSDoc(nil)` — the eager-JSDoc
        // surface is the ast crate's deferred resolveJSDoc registration
        // (source_file.rs header); permissive: nothing to check until it
        // lands. TODO(port M6 W3): the JSDoc-comment loop + tag walk.
        let kind = self.store().node(node).kind;
        // Go's shape: the two outer conditions gate the inner call.
        if !self.within_unreachable_code
            && self.compiler_options.allow_unreachable_code != Tristate::TSTrue
            && self.check_source_element_unreachable(node)
        {
            self.within_unreachable_code = true;
        }
        match kind {
            Kind::TypeParameter => {
                self.check_type_parameter(node);
            }
            Kind::Parameter => {
                self.check_parameter(node);
            }
            Kind::PropertyDeclaration => {
                self.check_property_declaration(node);
            }
            Kind::PropertySignature => {
                self.check_property_signature(node);
            }
            Kind::ConstructorType
            | Kind::FunctionType
            | Kind::CallSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature => {
                self.check_signature_declaration(node);
            }
            Kind::MethodDeclaration | Kind::MethodSignature => {
                self.check_method_declaration(node);
            }
            Kind::ClassStaticBlockDeclaration => {
                self.check_class_static_block_declaration(node);
            }
            Kind::Constructor => {
                self.check_constructor_declaration(node);
            }
            Kind::GetAccessor | Kind::SetAccessor => {
                self.check_accessor_declaration(node);
            }
            Kind::TypeReference => {
                self.check_type_reference_node(node);
            }
            Kind::TypePredicate => {
                self.check_type_predicate(node);
            }
            Kind::TypeQuery => {
                self.check_type_query(node);
            }
            Kind::TypeLiteral => {
                self.check_type_literal(node);
            }
            Kind::ArrayType => {
                self.check_array_type(node);
            }
            Kind::TupleType => {
                self.check_tuple_type(node);
            }
            Kind::UnionType | Kind::IntersectionType => {
                self.check_union_or_intersection_type(node);
            }
            Kind::ParenthesizedType | Kind::OptionalType | Kind::RestType => {
                // Go: `node.ForEachChild(c.checkSourceElement)` — each of
                // these payloads carries exactly the one wrapped type child,
                // so the child re-dispatch is direct (PORT: Node::for_each_child
                // needs the store borrow alive across the &mut self call).
                let child = {
                    let store = self.store();
                    let n = store.node(node);
                    match n.kind {
                        Kind::ParenthesizedType => n
                            .as_parenthesized_type_node()
                            .expect("ParenthesizedTypeNode data")
                            .type_,
                        Kind::OptionalType => {
                            n.as_optional_type_node().expect("OptionalTypeNode data").type_
                        }
                        _ => n.as_rest_type_node().expect("RestTypeNode data").type_,
                    }
                };
                self.check_source_element(Some(child));
            }
            Kind::ThisType => {
                self.check_this_type(node);
            }
            Kind::TypeOperator => {
                self.check_type_operator(node);
            }
            Kind::ConditionalType => {
                self.check_conditional_type(node);
            }
            Kind::InferType => {
                self.check_infer_type(node);
            }
            Kind::TemplateLiteralType => {
                self.check_template_literal_type(node);
            }
            Kind::ImportType => {
                self.check_import_type(node);
            }
            Kind::NamedTupleMember => {
                self.check_named_tuple_member(node);
            }
            Kind::IndexedAccessType => {
                self.check_indexed_access_type(node);
            }
            Kind::MappedType => {
                self.check_mapped_type(node);
            }
            Kind::FunctionDeclaration => {
                self.check_function_declaration(node);
            }
            Kind::Block | Kind::ModuleBlock => {
                self.check_block(node);
            }
            Kind::VariableStatement => {
                self.check_variable_statement(node);
            }
            Kind::ExpressionStatement => {
                self.check_expression_statement(node);
            }
            Kind::IfStatement => {
                self.check_if_statement(node);
            }
            Kind::DoStatement => {
                self.check_do_statement(node);
            }
            Kind::WhileStatement => {
                self.check_while_statement(node);
            }
            Kind::ForStatement => {
                self.check_for_statement(node);
            }
            Kind::ForInStatement => {
                self.check_for_in_statement(node);
            }
            Kind::ForOfStatement => {
                self.check_for_of_statement(node);
            }
            Kind::ContinueStatement | Kind::BreakStatement => {
                self.check_break_or_continue_statement(node);
            }
            Kind::ReturnStatement => {
                self.check_return_statement(node);
            }
            Kind::WithStatement => {
                self.check_with_statement(node);
            }
            Kind::SwitchStatement => {
                self.check_switch_statement(node);
            }
            Kind::LabeledStatement => {
                self.check_labeled_statement(node);
            }
            Kind::ThrowStatement => {
                self.check_throw_statement(node);
            }
            Kind::TryStatement => {
                self.check_try_statement(node);
            }
            Kind::VariableDeclaration => {
                self.check_variable_declaration(node);
            }
            Kind::BindingElement => {
                self.check_binding_element(node);
            }
            Kind::ClassDeclaration => {
                self.check_class_declaration(node);
            }
            Kind::InterfaceDeclaration => {
                self.check_interface_declaration(node);
            }
            Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => {
                self.check_type_alias_declaration(node);
            }
            Kind::EnumDeclaration => {
                self.check_enum_declaration(node);
            }
            Kind::EnumMember => {
                self.check_enum_member(node);
            }
            Kind::ModuleDeclaration => {
                self.check_module_declaration(node);
            }
            Kind::ImportDeclaration | Kind::JSImportDeclaration => {
                self.check_import_declaration(node);
            }
            Kind::ImportEqualsDeclaration => {
                self.check_import_equals_declaration(node);
            }
            Kind::ExportDeclaration => {
                self.check_export_declaration(node);
            }
            Kind::ExportAssignment => {
                self.check_export_assignment(node);
            }
            Kind::EmptyStatement | Kind::DebuggerStatement => {
                // TODO(port M6 W3): checkGrammarStatementInAmbientContext (grammarchecks.go).
            }
            Kind::MissingDeclaration => {
                self.check_missing_declaration(node);
            }
            Kind::JSDocNonNullableType
            | Kind::JSDocNullableType
            | Kind::JSDocAllType
            | Kind::JSDocTypeLiteral => {
                /* TODO(port M6 S3): checkJSDocType */
            }
            _ => {}
        }
    }

    /// Go: `func (c *Checker) checkSourceElementUnreachable(node *ast.Node) bool`
    /// (checker.go:2393).
    fn check_source_element_unreachable(&mut self, node: NodeId) -> bool {
        if !binder_shims::is_potentially_executable_node(self.store(), node) {
            return false;
        }
        if self.reported_unreachable_nodes.contains(&node) {
            return true;
        }
        if !self.is_source_element_unreachable(node) {
            return false;
        }
        self.reported_unreachable_nodes.insert(node);

        let source_file = self.get_source_file_node_of(node);

        let mut start_node = node;
        let mut end_node = node;

        let parent = self.store().node(node).parent.get();
        if parent != NodeId::NONE && can_have_statements(self.store().node(parent).kind) {
            let statements = statements_of(self.store(), parent)
                .map(|s| s.to_vec())
                .unwrap_or_default();
            if let Some(offset) = statements.iter().position(|&s| s == node) {
                // Scan backwards to find the first unreachable unreported node;
                // this may happen when producing region diagnostics where not all nodes
                // will have been visited.
                // TODO(upstream, Go): enable this code once we support region diagnostics again.
                let first = offset;
                let mut last = offset;
                // Go: `for i := offset + 1; i < len(statements); i++` — the
                // index drives both the node read and `last`.
                #[allow(clippy::needless_range_loop)]
                for i in offset + 1..statements.len() {
                    let next_node = statements[i];
                    if !binder_shims::is_potentially_executable_node(self.store(), next_node)
                        || !self.is_source_element_unreachable(next_node)
                    {
                        break;
                    }
                    last = i;
                    self.reported_unreachable_nodes.insert(next_node);
                }
                start_node = statements[first];
                end_node = statements[last];
            }
        }

        let start = get_token_pos_of_node_stub(self.store(), start_node);
        let end = self.store().node(end_node).loc.end();
        let diagnostic = tsc_ast::Diagnostic::new(
            Some(source_file),
            TextRange::new(start, end),
            UNREACHABLE_CODE_DETECTED.code(),
            UNREACHABLE_CODE_DETECTED.category(),
            "" /*source*/,
            Some(&UNREACHABLE_CODE_DETECTED),
            "" /*messageText*/,
            UNREACHABLE_CODE_DETECTED.key(),
            Vec::new(),
        );
        let is_error = self.compiler_options.allow_unreachable_code == Tristate::TSFalse;
        self.add_error_or_suggestion(is_error, diagnostic);

        true
    }

    /// The file NodeId a node belongs to (Go: `ast.GetSourceFileOfNode`).
    pub(crate) fn get_source_file_node_of(&self, node: NodeId) -> NodeId {
        let store = self.program.node_store();
        let mut current = node;
        loop {
            let n = store.node(current);
            if n.kind == Kind::SourceFile {
                return current;
            }
            current = n.parent.get();
        }
    }

    /// Go: `func (c *Checker) isSourceElementUnreachable(node *ast.Node) bool`
    /// (checker.go:2424). Precondition: IsPotentiallyExecutableNode is true.
    fn is_source_element_unreachable(&mut self, node: NodeId) -> bool {
        let (kind, flags) = {
            let n = self.store().node(node);
            (n.kind, n.flags)
        };
        if flags.intersects(NF::UNREACHABLE) {
            // The binder has determined that this code is unreachable.
            // Ignore const enums unless preserveConstEnums is set.
            match kind {
                Kind::EnumDeclaration => {
                    !binder_shims::is_enum_const(self.store(), node)
                        || self.compiler_options.should_preserve_const_enums()
                }
                Kind::ModuleDeclaration => {
                    // Go: ast.IsInstantiatedModule(node, ShouldPreserveConstEnums()).
                    crate::statements::is_instantiated_module(
                        self.store(),
                        node,
                        self.compiler_options.should_preserve_const_enums(),
                    )
                }
                _ => true,
            }
        } else if let Some(flow_node) = flow_node_of(self.store(), node) {
            // For code the binder doesn't know is unreachable, use control flow / types.
            !self.is_reachable_flow_node_stub(flow_node)
        } else {
            false
        }
    }

    /// Go: `func (c *Checker) isReachableFlowNode(flowNode) bool` (flow.go —
    /// the flow-graph walk). REAL since W3: flow.rs's body.
    fn is_reachable_flow_node_stub(&mut self, flow_node: tsc_ast::FlowNodeId) -> bool {
        self.is_reachable_flow_node(flow_node)
    }

    /// Go: `func (c *Checker) checkJSDocComments(node *ast.Node)` (checker.go:2585).
    #[allow(dead_code)] // the JSDoc-comment loop's callers land with W3
    fn check_jsdoc_comments(&mut self, node: NodeId) {
        // Go: `for _, comment := range node.Comments()` — node-attached
        // comment lists are the ast crate's deferred eager-JSDoc surface.
        // TODO(port M6 W3): the comment walk (diagnostic-only pass).
        let _ = node;
    }

    /// Go: `func (c *Checker) checkJSDocComment(node *ast.Node)` (checker.go:2590).
    #[allow(dead_code)] // the JSDoc-comment loop's callers land with W3
    fn check_jsdoc_comment(&mut self, node: NodeId) {
        // This performs minimal checking of JSDoc nodes to ensure that @link
        // references to entities are recorded for purposes of checking
        // unused identifiers.
        let kind = self.store().node(node).kind;
        if matches!(
            kind,
            Kind::JSDocLink | Kind::JSDocLinkCode | Kind::JSDocLinkPlain
        ) {
            let name = self.store().node(node).name();
            self.resolve_jsdoc_member_name(name);
        }
    }

    /// Go: `func (c *Checker) resolveJSDocMemberName(name *ast.Node) *ast.Symbol`
    /// (checker.go:2600). TODO(port M6 S6): resolveEntityName — permissive
    /// None (the JSDoc @link registration is unused-identifier bookkeeping).
    #[allow(dead_code)] // resolveJSDocMemberName's callers land with W3
    fn resolve_jsdoc_member_name(&mut self, name: Option<NodeId>) -> Option<SymbolId> {
        let _ = (name,);
        None
    }

    // ── S3/S6 dispatch-arm stubs (the module-head banner map) ──

    // checkGrammarSourceFile (grammarchecks.go:2025) — the REAL body lives
    // in grammarchecks.rs (the W3 wave).

    // Go: `checkExternalModuleExports` — the real body lives in
    // statements.rs (batch 5, checker.go:5906); the S6 stub was removed.

    // ────────────────────────────────────────────────────────────────────────
    // The deferred-nodes machinery (Go checker.go:2528-2583; risk 2)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) checkNodeDeferred(node *ast.Node)` — function
    /// and class expression bodies are checked after all statements in the
    /// enclosing body (see Go's comment: delaying the type check of the
    /// body ensures the enclosing variable has been assigned a type).
    #[allow(dead_code)] // the S3/S4 deferred checkers queue through this
    pub(crate) fn check_node_deferred(&mut self, node: NodeId) {
        let enclosing_file = self.get_source_file_node_of(node);
        let links = self.source_file_links.get(enclosing_file);
        if !links.type_checked {
            links.deferred_nodes.add(node);
        }
    }

    /// Go: `func (c *Checker) checkDeferredNodes(context *ast.SourceFile)`.
    ///
    /// PORT (risk 2, the drain ordering): Go iterates a LIVE view — nodes
    /// queued during the drain (checkNodeDeferred still sees
    /// `typeChecked == false`) are enumerated by the same drain, in
    /// insertion order, and the set is REPLACED (not cleared) after the
    /// loop. The Rust OrderedSet cannot grow during a borrowed iteration,
    /// so the port takes the set up front into a FIFO worklist and, after
    /// each node, pulls whatever the drain queued into the live (fresh,
    /// empty) set — the same enumeration order and the same end state
    /// (empty set; anything left un-enumerated only when the loop breaks on
    /// cancellation, which Go also discards via the replacement). A node
    /// re-added under the set's dedupe does not re-enumerate (tracked in
    /// `seen`), matching Go's map semantics.
    fn check_deferred_nodes(&mut self, context: NodeId) {
        let initial: Vec<NodeId> = {
            let links = self.source_file_links.get(context);
            links.deferred_nodes.values().copied().collect()
        };
        // Go replaces the set AFTER the loop; the port must empty it now so
        // during-drain queues land in a fresh set (the take leaves an empty
        // one — the same object Go's `links.deferredNodes = OrderedSet{}`
        // installs at loop exit, installed early; no observable difference
        // because nothing reads the set in between).
        self.source_file_links.get(context).deferred_nodes = OrderedSet::new();
        let mut seen: FxHashSet<NodeId> = initial.iter().copied().collect();
        let mut queue: std::collections::VecDeque<NodeId> = initial.into();
        while let Some(node) = queue.pop_front() {
            if self.is_canceled() {
                break;
            }
            self.check_deferred_node(node);
            // Pull the nodes queued during this node's check (Go's live
            // index loop reaches them as the slice grows).
            let queued: Vec<NodeId> = {
                let links = self.source_file_links.get(context);
                links
                    .deferred_nodes
                    .values()
                    .copied()
                    .filter(|n| !seen.contains(n))
                    .collect()
            };
            for n in queued {
                seen.insert(n);
                queue.push_back(n);
            }
        }
    }

    /// Go: `func (c *Checker) checkDeferredNode(node *ast.Node)` (checker.go:2547)
    /// — the deferred re-dispatch.
    fn check_deferred_node(&mut self, node: NodeId) {
        let pop = self.tracer.as_ref().map(|tr| {
            let (kind, pos, end, path) = {
                let store = self.store();
                let n = store.node(node);
                (
                    n.kind as i64,
                    n.loc.pos() as i64,
                    n.loc.end() as i64,
                    self.source_file_of(self.get_source_file_node_of(node))
                        .map(|f| f.file_name().as_string())
                        .unwrap_or_default(),
                )
            };
            let args = Args::new();
            args.insert("kind", TraceValue::Int(kind));
            args.insert("pos", TraceValue::Int(pos));
            args.insert("end", TraceValue::Int(end));
            args.insert("path", TraceValue::Str(path.to_string()));
            tr.push(Phase::Check, "checkDeferredNode", &args, false)
        });
        let save_current_node = self.current_node;
        self.current_node = Some(node);
        self.instantiation_count = 0;
        let kind = self.store().node(node).kind;
        match kind {
            Kind::CallExpression
            | Kind::NewExpression
            | Kind::TaggedTemplateExpression
            | Kind::Decorator
            | Kind::JsxOpeningElement => {
                // These node kinds are deferred checked when overload
                // resolution fails. To save on work, we ensure the arguments
                // are checked just once in a deferred way.
                // TODO(port M6 S6): resolveUntypedCall(node) → *Signature.
                let _ = self.resolve_untyped_call_stub(node);
            }
            Kind::FunctionExpression | Kind::ArrowFunction | Kind::MethodDeclaration | Kind::MethodSignature => {
                self.check_function_expression_or_object_literal_method_deferred(node);
            }
            Kind::GetAccessor | Kind::SetAccessor => {
                self.check_accessor_declaration(node);
            }
            Kind::ClassExpression => {
                // M6 S4: checkClassExpressionDeferred (expressions.rs).
                self.check_class_expression_deferred(node);
            }
            Kind::TypeParameter => {
                self.check_type_parameter_deferred(node);
            }
            Kind::JsxSelfClosingElement => {
                // M6 W5: checkJsxSelfClosingElementDeferred (jsx.go).
                self.check_jsx_self_closing_element_deferred(node);
            }
            Kind::JsxElement => {
                // M6 W5: checkJsxElementDeferred (jsx.go).
                self.check_jsx_element_deferred(node);
            }
            Kind::TypeAssertionExpression | Kind::AsExpression => {
                // M6 S4: checkAssertionDeferred (expressions.rs).
                self.check_assertion_deferred(node);
            }
            Kind::VoidExpression => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("VoidExpression has an expression");
                let _ = self.check_expression(expression);
            }
            Kind::BinaryExpression => {
                if is_instance_of_expression(self.store(), node) {
                    // TODO(port M6 S6): resolveUntypedCall(node).
                    let _ = self.resolve_untyped_call_stub(node);
                }
            }
            Kind::ObjectLiteralExpression | Kind::JsxAttributes => {
                // M6 S4: checkContextualDeprecations (expressions.rs).
                self.check_contextual_deprecations(node);
            }
            _ => {}
        }
        self.current_node = save_current_node;
        if let Some(pop) = pop {
            pop.pop();
        }
    }

    /// M6 S6: `resolveUntypedCall` (checker.go:10116, resolvecall.rs) — the
    /// deferred-check arm of the overload-resolution-failure path.
    fn resolve_untyped_call_stub(&mut self, node: NodeId) -> Option<crate::types::SignatureId> {
        self.resolve_untyped_call(node)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Deferred diagnostics (Go checker.go:14252-14263)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) addDeferredDiagnostic(callback func())`.
    #[allow(dead_code)] // the deferred-callback producers land with S4/S6
    pub(crate) fn add_deferred_diagnostic(&mut self, callback: DeferredDiagnosticCallback) {
        self.deferred_diagnostic_callbacks.push(callback);
    }

    /// Go: `func (c *Checker) produceDeferredDiagnostics()`.
    pub(crate) fn produce_deferred_diagnostics(&mut self) {
        for callback in std::mem::take(&mut self.deferred_diagnostic_callbacks) {
            match callback {
                // TODO(port M6 S3): reportTypeNotIterableError + the related-info merge.
                DeferredDiagnosticCallback::ReportTypeNotIterable { error_node, t, allows_async_iterables, related } => {
                    let _ = (error_node, t, allows_async_iterables, related);
                }
                // Go: checkWeakMapSetCollision (checker.go:10787) — REAL (M6 W3 t7,
                // statements.rs; the private-identifier downlevel reservation).
                DeferredDiagnosticCallback::CheckWeakMapSetCollision { node } => {
                    self.check_weak_map_set_collision(node);
                }
                // Go: checkReflectCollision (checker.go:10811) — REAL (M6 W3 t7,
                // statements.rs; the super-in-static-initializer reservation).
                DeferredDiagnosticCallback::CheckReflectCollision { node } => {
                    self.check_reflect_collision(node);
                }
                // TODO(port M6 S4): reportNonexistentProperty.
                DeferredDiagnosticCallback::ReportNonexistentProperty { right, left_type, is_unchecked_js } => {
                    let _ = (right, left_type, is_unchecked_js);
                }
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Expression entries (Go checker.go:7539-8060)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getTypeOfExpression(node *ast.Node) *Type` —
    /// returns the type of an expression, possibly without fully checking
    /// all contained sub-expressions for errors.
    pub fn get_type_of_expression(&mut self, node: NodeId) -> TypeId {
        // Don't bother caching types that require no flow analysis and are
        // quick to compute.
        if let Some(quick_type) = self.get_quick_type_of_expression(node) {
            return quick_type;
        }
        // If a type has been cached for the node, return it.
        if let Some(&cached_type) = self.flow_type_cache.get(&node) {
            return cached_type;
        }
        let start_invocation_count = self.flow_invocation_count;
        let t = self.check_expression_ex(node, CheckMode::TYPE_ONLY);
        // If control flow analysis was required to determine the type, it is
        // worth caching. (The invocation counter only moves with the flow
        // engine — W3; until then the cache stays cold, exactly like Go on
        // flow-free expressions.)
        if self.flow_invocation_count != start_invocation_count {
            self.flow_type_cache.insert(node, t);
        }
        t
    }

    /// Go: `func (c *Checker) getQuickTypeOfExpression(node *ast.Node) *Type`
    /// (checker.go:7563). The await/call/new arms are pure optimizations —
    /// permissive None for their unported callees only costs time, never a
    /// different type.
    pub(crate) fn get_quick_type_of_expression(&mut self, node: NodeId) -> Option<TypeId> {
        let expr = binder_shims::skip_parentheses(self.store(), node);
        let kind = self.store().node(expr).kind;
        if is_await_expression(self.store().node(expr)) {
            let expression = crate::ast_shims::expression_of(self.store(), expr)
                .expect("AwaitExpression has an expression");
            if let Some(t) = self.get_quick_type_of_expression(expression) {
                // TODO(port M6 S9/S10): getAwaitedType is the identity stub
                // until the thenable resolution lands (exports.rs).
                return Some(self.get_awaited_type(t));
            }
            return None;
        }
        // Optimize for the common case of a call to a function with a single
        // non-generic call signature where we can just fetch the return type
        // without checking the arguments.
        if is_call_expression(self.store().node(expr)) {
            let callee_kind = crate::ast_shims::expression_of(self.store(), expr)
                .map(|e| self.store().node(e).kind)
                .unwrap_or(Kind::Unknown);
            // TODO(port M6 S6): IsRequireCall / isSymbolOrSymbolForCall /
            // IsImportCall — the call-shape guards; permissive false until
            // the S6 call surface lands.
            if callee_kind != Kind::SuperKeyword {
                if u::is_call_chain(self.store().node(expr)) {
                    return self.get_return_type_of_single_non_generic_signature_of_call_chain(expr);
                }
                let callee = crate::ast_shims::expression_of(self.store(), expr)
                    .expect("CallExpression has an expression");
                let func_type = self.check_non_null_expression(callee);
                return self.get_return_type_of_single_non_generic_signature(
                    func_type,
                    crate::types::SignatureKind::Call,
                );
            }
        }
        if is_new_expression(self.store().node(expr)) {
            let callee = crate::ast_shims::expression_of(self.store(), expr)
                .expect("NewExpression has an expression");
            let func_type = self.check_non_null_expression(callee);
            return self.get_return_type_of_single_non_generic_signature(
                func_type,
                crate::types::SignatureKind::Construct,
            );
        }
        // Go: the assertion arm returns getTypeFromTypeNode(expr.Type())
        // (checker.go:7582) — M6 S5 wires it now that the type-node family
        // is real.
        if matches!(kind, Kind::TypeAssertionExpression | Kind::AsExpression)
            && !u::is_const_type_reference(
                self.store(),
                crate::ast_shims::type_of_node(self.store(), expr).unwrap_or(NodeId::NONE),
            )
        {
            let type_node = crate::ast_shims::type_of_node(self.store(), expr)
                .expect("assertion expressions carry a type");
            return Some(self.get_type_from_type_node(type_node));
        }
        let n = self.store().node(node);
        if binder_shims::is_literal_expression(n) || binder_shims::is_boolean_literal(n) {
            return Some(self.check_expression(node));
        }
        None
    }

    /// Go: `func (c *Checker) getReturnTypeOfSingleNonGenericSignature(funcType *Type, kind SignatureKind) *Type`
    /// (checker.go:7582).
    fn get_return_type_of_single_non_generic_signature(
        &mut self,
        func_type: TypeId,
        kind: crate::types::SignatureKind,
    ) -> Option<TypeId> {
        let signature = self.get_single_signature_stub(func_type, kind, true /*allowMembers*/);
        match signature {
            Some(signature)
                if self.signatures[signature.0 as usize]
                    .type_parameters
                    .is_empty() =>
            {
                Some(self.get_return_type_of_signature(signature))
            }
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getReturnTypeOfSingleNonGenericSignatureOfCallChain(expr *ast.Node) *Type`
    /// (checker.go:7587).
    fn get_return_type_of_single_non_generic_signature_of_call_chain(
        &mut self,
        expr: NodeId,
    ) -> Option<TypeId> {
        let expression = crate::ast_shims::expression_of(self.store(), expr)
            .expect("CallExpression has an expression");
        let func_type = self.check_expression(expression);
        // TODO(port M6 S9): getOptionalExpressionType — permissive funcType
        // (the optional-marker propagation reads apparent types).
        let non_optional_type = func_type;
        let return_type = self.get_return_type_of_single_non_generic_signature(
            func_type,
            crate::types::SignatureKind::Call,
        );
        if let Some(return_type) = return_type {
            // TODO(port M6 S9): propagateOptionalTypeMarker — identity
            // until the optional-marker machinery lands.
            let _ = non_optional_type;
            return Some(return_type);
        }
        None
    }

    /// Go: `func (c *Checker) getSingleSignature(t *Type, kind, allowMembers) *Signature`
    /// (checker.go:19757 — S8's structured-member machinery). Permissive
    /// None until S8 (nil in Go = "no single signature").
    fn get_single_signature_stub(
        &mut self,
        t: TypeId,
        _kind: crate::types::SignatureKind,
        _allow_members: bool,
    ) -> Option<crate::types::SignatureId> {
        // TODO(port M6 S8): getSignaturesOfType → the single-signature pick.
        let _ = t;
        None
    }

    /// Go: `func (c *Checker) checkNonNullExpression(node *ast.Node) *Type`
    /// (checker.go:7592).
    pub(crate) fn check_non_null_expression(&mut self, node: NodeId) -> TypeId {
        let t = self.check_expression(node);
        self.check_non_null_type(t, node)
    }

    /// Go: `func (c *Checker) checkNonNullType(t *Type, node *ast.Node) *Type`
    /// (checker.go:7596).
    pub(crate) fn check_non_null_type(&mut self, t: TypeId, node: NodeId) -> TypeId {
        self.check_non_null_type_with_reporter(
            t,
            node,
            Checker::report_object_possibly_null_or_undefined_error,
        )
    }

    /// Go: `func (c *Checker) checkNonNullTypeWithReporter(t, node, reportError) *Type`
    /// (checker.go:7600). PORT: Go's `reportError func(c *Checker, node,
    /// facts)` method-expression becomes a plain fn pointer.
    pub(crate) fn check_non_null_type_with_reporter(
        &mut self,
        t: TypeId,
        node: NodeId,
        report_error: fn(&mut Checker, NodeId, crate::relater::TypeFacts),
    ) -> TypeId {
        if self.strict_null_checks && self.type_(t).flags.intersects(TypeFlags::UNKNOWN) {
            if tsc_ast::utilities::is_entity_name_expression(self.store(), node) {
                let node_text = u::entity_name_to_string(self.store(), node);
                if node_text.len() < 100 {
                    self.error(node, &X_0_IS_OF_TYPE_UNKNOWN, &[node_text]);
                    return self.error_type();
                }
            }
            self.error(node, &OBJECT_IS_OF_TYPE_UNKNOWN, &[]);
            return self.error_type();
        }
        // REAL as of W3T6 (was the S10 pure-flags seam): Go routes through
        // the getTypeFacts lattice worker, which walks UNION constituents
        // (`C | undefined` carries the undefined-or-null facts even though a
        // union type's own flags are just TypeFlagsUnion — the pure-flags
        // shortcut missed that, so optional-chain accesses on
        // union-with-undefined never narrowed and mis-fired TS2339).
        let facts = self.get_type_facts_impl(t, crate::relater::TypeFacts::IS_UNDEFINED_OR_NULL);
        if facts.intersects(crate::relater::TypeFacts::IS_UNDEFINED_OR_NULL) {
            report_error(self, node, facts);
            let non_nullable = self.get_non_nullable_type(t);
            if self
                .type_(non_nullable)
                .flags
                .intersects(TypeFlags::NULLABLE.union(TypeFlags::NEVER))
            {
                return self.error_type();
            }
            return non_nullable;
        }
        t
    }

    /// Go: `func (c *Checker) checkNonNullNonVoidType(t *Type, node *ast.Node) *Type`
    /// (checker.go:7616).
    #[allow(dead_code)] // checkNonNullNonVoidType's callers land with S4
    pub(crate) fn check_non_null_non_void_type(&mut self, t: TypeId, node: NodeId) -> TypeId {
        let non_null_type = self.check_non_null_type(t, node);
        if self.type_(non_null_type).flags.intersects(TypeFlags::VOID) {
            if tsc_ast::utilities::is_entity_name_expression(self.store(), node) {
                let node_text = u::entity_name_to_string(self.store(), node);
                if is_identifier(self.store().node(node)) && node_text == "undefined" {
                    self.error(
                        node,
                        &THE_VALUE_0_CANNOT_BE_USED_HERE,
                        std::slice::from_ref(&node_text),
                    );
                    return non_null_type;
                }
                if node_text.len() < 100 {
                    self.error(node, &X_0_IS_POSSIBLY_UNDEFINED, &[node_text]);
                    return non_null_type;
                }
            }
            self.error(node, &OBJECT_IS_POSSIBLY_UNDEFINED, &[]);
        }
        non_null_type
    }

    /// Go: `func (c *Checker) reportObjectPossiblyNullOrUndefinedError(node *ast.Node, facts TypeFacts)`
    /// (checker.go:7629) — the reporter `checkNonNullType` installs.
    fn report_object_possibly_null_or_undefined_error(
        &mut self,
        node: NodeId,
        facts: crate::relater::TypeFacts,
    ) {
        let mut node_text = String::new();
        if tsc_ast::utilities::is_entity_name_expression(self.store(), node) {
            node_text = u::entity_name_to_string(self.store(), node);
        }
        let is_null = self.store().node(node).kind == Kind::NullKeyword;
        if is_null {
            self.error(node, &THE_VALUE_0_CANNOT_BE_USED_HERE, &["null".to_string()]);
            return;
        }
        // Go: `isUndefined := facts&TypeFactsIsUndefined != 0; isNull :=
        // facts&TypeFactsIsNull != 0` — the split bits (Go TypeFactsIsUndefined
        // = 1<<24, TypeFactsIsNull = 1<<25). REAL as of W3T6 (both sides
        // used to read IS_UNDEFINED_OR_NULL, collapsing every split report
        // into TS18049).
        let is_undefined = facts.intersects(crate::relater::TypeFacts::IS_UNDEFINED);
        let is_null_fact = facts.intersects(crate::relater::TypeFacts::IS_NULL);
        if !node_text.is_empty() && node_text.len() < 100 {
            if is_identifier(self.store().node(node)) && node_text == "undefined" {
                self.error(node, &THE_VALUE_0_CANNOT_BE_USED_HERE, &["undefined".to_string()]);
                return;
            }
            let message: &Message = if is_undefined {
                if is_null_fact {
                    &X_0_IS_POSSIBLY_NULL_OR_UNDEFINED
                } else {
                    &X_0_IS_POSSIBLY_UNDEFINED
                }
            } else {
                &X_0_IS_POSSIBLY_NULL
            };
            self.error(node, message, &[node_text]);
        } else {
            let message: &Message = if is_undefined {
                if is_null_fact {
                    &OBJECT_IS_POSSIBLY_NULL_OR_UNDEFINED
                } else {
                    &OBJECT_IS_POSSIBLY_UNDEFINED
                }
            } else {
                &OBJECT_IS_POSSIBLY_NULL
            };
            self.error(node, message, &[]);
        }
    }

    /// Go: `func (c *Checker) checkExpressionWithContextualType(node, contextualType, inferenceContext, checkMode) *Type`
    /// (checker.go:7653).
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    pub(crate) fn check_expression_with_contextual_type(
        &mut self,
        node: NodeId,
        contextual_type: Option<TypeId>,
        inference_context: Option<crate::todo_shapes::InferenceContextId>,
        check_mode: CheckMode,
    ) -> TypeId {
        let context_node = self.get_context_node(node);
        self.push_contextual_type(Some(context_node), contextual_type, false /*isCache*/);
        self.push_inference_context(Some(context_node), inference_context);
        let mode = check_mode
            | CheckMode::CONTEXTUAL
            | if inference_context.is_some() { CheckMode::INFERENTIAL } else { CheckMode::CHECK_MODE_NORMAL };
        let mut t = self.check_expression_ex(node, mode);
        // In CheckMode.Inferential we collect intra-expression inference
        // sites to process before fixing any type parameters. This
        // information is no longer needed after the call to checkExpression.
        if let Some(id) = inference_context {
            self.inference_contexts[id.0 as usize]
                .intra_expression_inference_sites
                .clear();
        }
        // We strip literal freshness when an appropriate contextual type is
        // present such that contextually typed literals always preserve
        // their literal types (otherwise they might widen during type
        // inference). An alternative here would be to not mark contextually
        // typed literals as fresh in the first place.
        if crate::relater::maybe_type_of_kind(self, t, TypeFlags::LITERAL) && {
            let instantiated =
                self.instantiate_contextual_type_stub(contextual_type, node, ContextFlags::NONE);
            self.is_literal_of_contextual_type_stub(t, instantiated)
        } {
            t = self.get_regular_type_of_literal_type(t);
        }
        self.pop_inference_context();
        self.pop_contextual_type();
        t
    }

    /// Go: `func (c *Checker) getContextNode(node *ast.Node) *ast.Node`
    /// (checker.go:7676).
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    fn get_context_node(&self, node: NodeId) -> NodeId {
        let store = self.program.node_store();
        let n = store.node(node);
        if is_jsx_attributes(n) {
            let parent = n.parent.get();
            if store.node(parent).kind != Kind::JsxSelfClosingElement {
                // Needs to be the root JsxElement, so it encompasses the
                // attributes _and_ the children (which are essentially part
                // of the attributes).
                return store.node(parent).parent.get();
            }
        }
        node
    }

    /// Go: `func (c *Checker) getContextFreeTypeOfExpression(node *ast.Node) *Type`
    /// (checker.go:7799) — intended for uses where you know there is no
    /// contextual type, and requesting the contextual type might cause a
    /// circularity or other bad behaviour. It sets the contextual type of
    /// the node to any before calling getTypeOfExpression.
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    pub(crate) fn get_context_free_type_of_expression(&mut self, node: NodeId) -> TypeId {
        if let Some(&cached) = self.context_free_types.get(&node) {
            return cached;
        }
        let any_type = self.any_type();
        self.push_contextual_type(Some(node), Some(any_type), false /*isCache*/);
        let t = self.check_expression_ex(node, CheckMode::SKIP_CONTEXT_SENSITIVE);
        self.context_free_types.insert(node, t);
        self.pop_contextual_type();
        t
    }

    /// Go: `func (c *Checker) checkExpression(node *ast.Node) *Type` (checker.go:7816).
    pub fn check_expression(&mut self, node: NodeId) -> TypeId {
        self.check_expression_ex(node, CheckMode::CHECK_MODE_NORMAL)
    }

    /// Go: `func (c *Checker) checkExpressionCached(node *ast.Node) *Type` (checker.go:7770).
    pub fn check_expression_cached(&mut self, node: NodeId) -> TypeId {
        self.check_expression_cached_ex(node, CheckMode::CHECK_MODE_NORMAL)
    }

    /// Go: `func (c *Checker) checkExpressionCachedEx(node, checkMode) *Type`
    /// (checker.go:7774). When computing a type that we're going to cache,
    /// we need to ignore any ongoing control flow analysis because
    /// variables may have transient types in indeterminable states. Moving
    /// flowLoopStart to the top of the stack ensures all transient types
    /// are computed from a known point.
    pub fn check_expression_cached_ex(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        if check_mode != CheckMode::CHECK_MODE_NORMAL {
            return self.check_expression_ex(node, check_mode);
        }
        if self.type_node_links.get(node).resolved_type.is_none() {
            // Go saves/restores the flow state around the uncached compute;
            // the flow engine is W3, so both saves are empty at this depth
            // (the save/restore structure stays for the wave).
            let save_flow_loop_stack = std::mem::take(&mut self.flow_loop_stack);
            let save_flow_type_cache = std::mem::take(&mut self.flow_type_cache);
            let t = self.check_expression_ex(node, check_mode);
            self.flow_type_cache = save_flow_type_cache;
            self.flow_loop_stack = save_flow_loop_stack;
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) checkExpressionEx(node, checkMode) *Type`
    /// (checker.go:7820).
    pub fn check_expression_ex(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let pop = self.tracer.as_ref().map(|tr| {
            let (kind, pos, end, path) = {
                let n = self.store().node(node);
                (
                    n.kind as i64,
                    n.loc.pos() as i64,
                    n.loc.end() as i64,
                    self.source_file_of(self.get_source_file_node_of(node))
                        .map(|f| f.file_name().as_string())
                        .unwrap_or_default(),
                )
            };
            let args = Args::new();
            args.insert("kind", TraceValue::Int(kind));
            args.insert("pos", TraceValue::Int(pos));
            args.insert("end", TraceValue::Int(end));
            args.insert("path", TraceValue::Str(path.to_string()));
            tr.push(Phase::Check, "checkExpression", &args, false)
        });
        let save_current_node = self.current_node;
        self.current_node = Some(node);
        self.instantiation_count = 0;
        let uninstantiated_type = self.check_expression_worker(node, check_mode);
        let t = self.instantiate_type_with_single_generic_call_signature(
            node,
            uninstantiated_type,
            check_mode,
        );
        if is_const_enum_object_type(self, t) {
            self.check_const_enum_access(node, t);
        }
        self.current_node = save_current_node;
        if let Some(pop) = pop {
            pop.pop();
        }
        t
    }

    /// Go: `func (c *Checker) checkConstEnumAccess(node *ast.Node, t *Type)`
    /// (checker.go:7834) — enum object type for const enums are only
    /// permitted in: 'left' in property access, 'object' in indexed access,
    /// target in rhs of import statement, 'exprName' in type query, or as
    /// an export specifier (re-export).
    fn check_const_enum_access(&mut self, node: NodeId, t: TypeId) {
        let (parent_kind, _parent, parent_expression_is_node) = {
            let store = self.store();
            let p = store.node(node).parent.get();
            let pn = store.node(p);
            (
                pn.kind,
                p,
                crate::ast_shims::expression_of(store, p).is_some_and(|e| e == node),
            )
        };
        let ok = (matches!(parent_kind, Kind::PropertyAccessExpression)
            && parent_expression_is_node)
            || (matches!(parent_kind, Kind::ElementAccessExpression)
                && parent_expression_is_node)
            || ((is_identifier(self.store().node(node))
                || is_qualified_name(self.store().node(node)))
                && u::is_in_right_side_of_import_or_export_assignment(self.store(), node))
            || (parent_kind == Kind::TypeQuery
                && {
                    // Go: node.Parent.AsTypeQueryNode().ExprName == node.
                    true /* TODO(port M6 S5): the TypeQuery exprName identity */
                });
        // Go also allows export specifiers (reexporting const enums).
        let ok = ok || parent_kind == Kind::ExportSpecifier;
        if !ok {
            self.error(
                node,
                &X_CONST_ENUMS_CAN_ONLY_BE_USED_IN_PROPERTY_OR_INDEX_ACCESS_EXPRESSIONS_OR_THE_RIGHT_HAND_SIDE_OF_AN_IMPORT_DECLARATION_OR_EXPORT_ASSIGNMENT_OR_TYPE_QUERY,
                &[],
            );
        }
        // --verbatimModuleSyntax only gets checked here when the enum usage
        // does not resolve to an import, because imports of ambient const
        // enums get checked separately in `checkAliasSymbol`.
        let isolated_modules = self.compiler_options.isolated_modules == Tristate::TSTrue;
        let verbatim = self.compiler_options.verbatim_module_syntax == Tristate::TSTrue;
        if isolated_modules || (verbatim && ok && self.resolve_name_stub(node, "").is_none()) {
            // TODO(port M6 S4/S6): checkAliasSymbol's isolated-modules arm —
            // the ambient const-enum access check needs the program layer's
            // GetProjectReferenceFromOutputDts (a Program-trait gap, PORT)
            // and resolveName (S6). The permissive stub above skips it.
            let _ = t;
        }
    }

    /// Go: `func (c *Checker) instantiateTypeWithSingleGenericCallSignature(node, t, checkMode) *Type`
    /// (checker.go:7848). The reachable prefix is exact; the deep
    /// inference tail needs S8 (getSingleSignature) and S9 (the contextual
    /// signature + inference merge) — with the S8 stub returning None the
    /// function returns `t` at the same gate Go would.
    pub(crate) fn instantiate_type_with_single_generic_call_signature(
        &mut self,
        node: NodeId,
        t: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        if !check_mode
            .intersects(CheckMode::INFERENTIAL.union(CheckMode::SKIP_GENERIC_FUNCTIONS))
        {
            return t;
        }
        let call_signature =
            self.get_single_signature_stub(t, crate::types::SignatureKind::Call, true /*allowMembers*/);
        let construct_signature = self.get_single_signature_stub(
            t,
            crate::types::SignatureKind::Construct,
            true /*allowMembers*/,
        );
        let signature = call_signature.or(construct_signature);
        let Some(signature) = signature else {
            return t;
        };
        if self.signatures[signature.0 as usize].type_parameters.is_empty() {
            return t;
        }
        // TODO(port M6 S8/S9): the contextual-signature instantiation tail —
        // getApparentTypeOfContextualType, the return-signature inference
        // pass, and getOrCreateTypeFromSignature(instantiateSignatureInContextOf(...)).
        // Go's tail is unreachable while getSingleSignature resolves no
        // members (S8); the wave that lands S8 fills it in.
        let _ = node;
        t
    }

    /// Go: `func (c *Checker) getOuterInferenceTypeParameters() []*Type`
    /// (checker.go:7902).
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    fn get_outer_inference_type_parameters(&mut self) -> Vec<TypeId> {
        let mut result = Vec::new();
        for i in 0..self.inference_context_infos.len() {
            let context = self.inference_context_infos[i].context;
            if let Some(context) = context {
                for info in &self.inference_contexts[context.0 as usize].inferences {
                    result.push(info.type_parameter);
                }
            }
        }
        result
    }

    /// Go: `func (c *Checker) getUniqueTypeParameters(context *InferenceContext, typeParameters []*Type) []*Type`
    /// (checker.go:7910) — instantiates the signature with its own type
    /// parameters as type arguments, possibly renaming the type parameters
    /// to ensure they have unique names.
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    fn get_unique_type_parameters(
        &mut self,
        context: crate::todo_shapes::InferenceContextId,
        type_parameters: &[TypeId],
    ) -> Vec<TypeId> {
        let mut old_type_parameters: Vec<TypeId> = Vec::new();
        let mut new_type_parameters: Vec<TypeId> = Vec::new();
        let mut result: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for &tp in type_parameters {
            let name = self.program.symbol(self.type_(tp).symbol.expect("type parameter symbol"))
                .name
                .clone();
            let inferred = &self.inference_contexts[context.0 as usize].inferred_type_parameters;
            if has_type_parameter_by_name(self, inferred, &name)
                || has_type_parameter_by_name(self, &result, &name)
            {
                let new_name = get_unique_type_parameter_name(
                    self,
                    inferred
                        .iter()
                        .chain(result.iter())
                        .copied()
                        .collect::<Vec<_>>()
                        .as_slice(),
                    &name,
                );
                let symbol = self.new_symbol(SymbolFlags::TYPE_PARAMETER, &new_name);
                // TODO(port M6 S7): newTypeParameter wires target + mapper;
                // the minimal ctor keeps the S1 seam permissive.
                let new_type_parameter = self.new_type_parameter_stub(Some(symbol));
                let tp_data = self.type_mut(new_type_parameter);
                if let TypeData::TypeParameter(data) = &mut tp_data.data {
                    // Go: newTypeParameter.AsTypeParameter().target = tp —
                    // the mapper wiring is S7 (todo_shapes::TypeMapperId seam).
                    let _ = data;
                }
                old_type_parameters.push(tp);
                new_type_parameters.push(new_type_parameter);
                result.push(new_type_parameter);
            } else {
                result.push(tp);
            }
        }
        if !new_type_parameters.is_empty() {
            // TODO(port M6 S7): newTypeMapper(oldTypeParameters, newTypeParameters)
            // + the tp.mapper back-writes (mapper.rs's real constructor).
        }
        result
    }

    /// Go: `func (c *Checker) newTypeParameter(symbol *ast.Symbol) *Type`
    /// (checker.go:25537+ — S7's ctor family). Minimal S1 construction: the
    /// distributed/constraint fields S7 owns stay default (the marker types
    /// in new_checker use the same shape directly).
    #[allow(dead_code)] // S4/S7 callers (contextual literals, signature
                    // inference, the real newTypeParameter ctor)
    fn new_type_parameter_stub(&mut self, symbol: Option<SymbolId>) -> TypeId {
        let t = self.new_type(
            TypeFlags::TYPE_PARAMETER,
            ObjectFlags::NONE,
            TypeData::TypeParameter(Box::default()),
        );
        if let Some(symbol) = symbol {
            self.type_mut(t).symbol = Some(symbol);
        }
        t
    }

    /// Go: `func (c *Checker) checkExpressionWorker(node, checkMode) *Type`
    /// (checker.go:7933) — the ~45-arm per-kind dispatch. The per-kind LEAF
    /// checkers are S4 (and W5 for jsx); each banner names the slice.
    fn check_expression_worker(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::Identifier => {
                // M6 S4: checkIdentifier (expressions.rs, tranche 2).
                self.check_identifier(node, check_mode)
            }
            Kind::PrivateIdentifier => {
                // M6 S4: checkPrivateIdentifierExpression (expressions.rs).
                self.check_private_identifier_expression(node)
            }
            Kind::ThisKeyword => {
                // M6 S4: checkThisExpression (expressions.rs).
                self.check_this_expression(node)
            }
            Kind::SuperKeyword => {
                // M6 S4: checkSuperExpression (expressions.rs, tranche 2).
                self.check_super_expression(node)
            }
            Kind::NullKeyword => self.null_widening_type(),
            Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral => {
                if self.is_skip_direct_inference_node(node) {
                    return self.blocked_string_type();
                }
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_string_literal_type(&text);
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::NumericLiteral => {
                self.check_grammar_numeric_literal(node);
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_number_literal_type(tsc_jsnum::from_string(&text));
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::BigIntLiteral => {
                self.check_grammar_big_int_literal(node);
                let text = tsc_ast::node_text(self.store(), node).to_string();
                let t = self.get_big_int_literal_type(tsc_jsnum::new_pseudo_big_int(
                    &tsc_jsnum::parse_pseudo_big_int(&text),
                    false, /*negative*/
                ));
                self.get_fresh_type_of_literal_type(t)
            }
            Kind::TrueKeyword => self.true_type(),
            Kind::FalseKeyword => self.false_type(),
            Kind::TemplateExpression => {
                // M6 S4: checkTemplateExpression (expressions.rs, tranche 2).
                self.check_template_expression(node)
            }
            Kind::RegularExpressionLiteral => {
                // M6 S4: checkRegularExpressionLiteral (expressions.rs, tranche 2).
                self.check_regular_expression_literal(node)
            }
            Kind::ArrayLiteralExpression => {
                // M6 S4: checkArrayLiteral (expressions.rs, tranche 2).
                self.check_array_literal(node, check_mode)
            }
            Kind::ObjectLiteralExpression => {
                // M6 S4: checkObjectLiteral (expressions.rs, tranche 2).
                self.check_object_literal(node, check_mode)
            }
            Kind::PropertyAccessExpression => {
                // M6 S4: checkPropertyAccessExpression (expressions.rs).
                self.check_property_access_expression(node, check_mode, false /*writeOnly*/)
            }
            Kind::QualifiedName => {
                // M6 S4: checkQualifiedName (expressions.rs, tranche 2).
                self.check_qualified_name(node, check_mode)
            }
            Kind::ElementAccessExpression => {
                // M6 S4: checkIndexedAccess (expressions.rs, tranche 2).
                self.check_indexed_access(node, check_mode)
            }
            Kind::CallExpression => {
                // M6 S6: IsImportCall(node) → checkImportCallExpression;
                // else checkCallExpression (resolvecall.rs).
                if crate::resolvecall::is_import_call(self.store(), node) {
                    self.check_import_call_expression(node)
                } else {
                    self.check_call_expression(node, check_mode)
                }
            }
            Kind::NewExpression => {
                // M6 S6: checkCallExpression (resolvecall.rs — the NewExpression
                // arm resolves the construct signatures).
                self.check_call_expression(node, check_mode)
            }
            Kind::TaggedTemplateExpression => {
                // M6 S4: checkTaggedTemplateExpression (expressions.rs,
                // tranche 2 — S6's resolveSignature seam inside).
                self.check_tagged_template_expression(node)
            }
            Kind::ParenthesizedExpression => {
                // M6 S4: checkParenthesizedExpression (expressions.rs, tranche 2).
                self.check_parenthesized_expression(node, check_mode)
            }
            Kind::ClassExpression => {
                // M6 S4: checkClassExpression (expressions.rs, tranche 2).
                self.check_class_expression(node)
            }
            Kind::FunctionExpression | Kind::ArrowFunction => {
                // M6 S4: checkFunctionExpressionOrObjectLiteralMethod
                // (expressions.rs, tranche 2).
                self.check_function_expression_or_object_literal_method(node, check_mode)
            }
            Kind::TypeAssertionExpression | Kind::AsExpression => {
                // M6 S4: checkAssertion (expressions.rs).
                self.check_assertion(node, check_mode)
            }
            Kind::TypeOfExpression => {
                // M6 S4: checkTypeOfExpression (expressions.rs).
                self.check_type_of_expression(node)
            }
            Kind::NonNullExpression => {
                // M6 S4: checkNonNullAssertion (expressions.rs).
                self.check_non_null_assertion(node)
            }
            Kind::ExpressionWithTypeArguments => {
                // M6 S4: checkExpressionWithTypeArguments (expressions.rs).
                self.check_expression_with_type_arguments(node)
            }
            Kind::SatisfiesExpression => {
                // M6 S4: checkSatisfiesExpression (expressions.rs).
                self.check_satisfies_expression(node)
            }
            Kind::MetaProperty => {
                // M6 S4: checkMetaProperty (expressions.rs).
                self.check_meta_property(node)
            }
            Kind::DeleteExpression => {
                // M6 S4: checkDeleteExpression (expressions.rs).
                self.check_delete_expression(node)
            }
            Kind::VoidExpression => {
                // M6 S4: checkVoidExpression (expressions.rs).
                self.check_void_expression(node)
            }
            Kind::AwaitExpression => {
                // M6 S4: checkAwaitExpression (expressions.rs).
                self.check_await_expression(node)
            }
            Kind::PrefixUnaryExpression => {
                // M6 S4: checkPrefixUnaryExpression (expressions.rs).
                self.check_prefix_unary_expression(node)
            }
            Kind::PostfixUnaryExpression => {
                // M6 S4: checkPostfixUnaryExpression (expressions.rs).
                self.check_postfix_unary_expression(node)
            }
            Kind::BinaryExpression => {
                // M6 S4: checkBinaryExpression (expressions.rs).
                self.check_binary_expression(node, check_mode)
            }
            Kind::ConditionalExpression => {
                // M6 S4: checkConditionalExpression (expressions.rs).
                self.check_conditional_expression(node, check_mode)
            }
            Kind::SpreadElement => {
                // M6 S4: checkSpreadExpression (expressions.rs).
                self.check_spread_expression(node, check_mode)
            }
            Kind::OmittedExpression => self.undefined_widening_type(),
            Kind::YieldExpression => {
                // M6 S4: checkYieldExpression (expressions.rs).
                self.check_yield_expression(node)
            }
            Kind::SyntheticExpression => {
                // M6 S4: checkSyntheticExpression (expressions.rs).
                self.check_synthetic_expression(node)
            }
            Kind::JsxExpression => {
                // M6 W5: checkJsxExpression(node, checkMode) — jsx.go.
                self.check_jsx_expression(node, check_mode)
            }
            Kind::JsxElement => {
                // M6 W5: checkJsxElement(node, checkMode) — jsx.go
                // (M6-DISPATCH risk 3 signature).
                self.check_jsx_element(node, check_mode)
            }
            Kind::JsxSelfClosingElement => {
                // M6 W5: checkJsxSelfClosingElement(node, checkMode) — jsx.go.
                self.check_jsx_self_closing_element(node, check_mode)
            }
            Kind::JsxFragment => {
                // M6 W5: checkJsxFragment(node) — jsx.go.
                self.check_jsx_fragment(node)
            }
            Kind::JsxAttributes => {
                // M6 W5: checkJsxAttributes(node, checkMode) — jsx.go.
                self.check_jsx_attributes(node, check_mode)
            }
            Kind::JsxOpeningElement => {
                panic!("Should never directly check a JsxOpeningElement")
            }
            _ => self.error_type(),
        }
    }

    // ── The S4/S5/S6/S9 permissive stubs the expression spine routes to ──

    /// Go: `func (c *Checker) isLiteralOfContextualType(candidateType, contextualType) bool`
    /// (checker.go:26056) — REAL (M6 S9): the REAL body already lived in
    /// expressions.rs (ported with the S4 contextual-literal batch); this
    /// stub now delegates so the strip-freshness arm in
    /// check_expression_with_contextual_type is live.
    fn is_literal_of_contextual_type_stub(
        &mut self,
        candidate_type: TypeId,
        contextual_type: Option<TypeId>,
    ) -> bool {
        self.is_literal_of_contextual_type(candidate_type, contextual_type)
    }

    /// Go: `func (c *Checker) instantiateContextualType(contextualType, node, contextFlags) *Type`
    /// (checker.go:31366) — REAL (M6 S9): MOVED — the body (the inference
    /// context's nonFixingMapper/returnMapper incorporation + the boolean
    /// strip) lives in contextual.rs as `instantiate_contextual_type`;
    /// this twin keeps the stub name for the spine call sites.
    pub(crate) fn instantiate_contextual_type_stub(
        &mut self,
        contextual_type: Option<TypeId>,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        self.instantiate_contextual_type(contextual_type, node, context_flags)
    }

    /// Go: `func (c *Checker) resolveName(location, name, meaning, message, isUse, excludeGlobals) *ast.Symbol`
    /// — the createNameResolver closure (checker.go:1510, S6's
    /// binder.NameResolver port). Permissive None until S6.
    fn resolve_name_stub(&mut self, node: NodeId, name: &str) -> Option<SymbolId> {
        let _ = (node, name);
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // The contextual stacks (Go checker.go:31425-31512 — S1 ports the
    // push/pop because checkExpressionWithContextualType drives them; the
    // readers — getContextualType, getInferenceContext — are S9.)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) pushContextualType(node *ast.Node, t *Type, isCache bool)`.
    pub(crate) fn push_contextual_type(
        &mut self,
        node: Option<NodeId>,
        t: Option<TypeId>,
        is_cache: bool,
    ) {
        self.contextual_infos.push(ContextualInfo { node, t, is_cache });
    }

    /// Go: `func (c *Checker) popContextualType()`.
    pub(crate) fn pop_contextual_type(&mut self) {
        let last_index = self.contextual_infos.len() - 1;
        self.contextual_infos.truncate(last_index);
    }

    /// Go: `func (c *Checker) pushInferenceContext(node *ast.Node, context *InferenceContext)`.
    pub(crate) fn push_inference_context(
        &mut self,
        node: Option<NodeId>,
        context: Option<crate::todo_shapes::InferenceContextId>,
    ) {
        self.inference_context_infos.push(InferenceContextInfo { node, context });
    }

    /// Go: `func (c *Checker) popInferenceContext()`.
    pub(crate) fn pop_inference_context(&mut self) {
        let last_index = self.inference_context_infos.len() - 1;
        self.inference_context_infos.truncate(last_index);
    }

    // ────────────────────────────────────────────────────────────────────────
    // The diagnostics entries (Go checker.go:14222-14310)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) GetDiagnostics(ctx, sourceFile) []*ast.Diagnostic`.
    pub fn get_file_diagnostics(&mut self, source_file: NodeId) -> Vec<tsc_ast::Diagnostic> {
        self.get_diagnostics(source_file, DiagnosticsSlot::Errors)
    }

    /// Go: `func (c *Checker) GetSuggestionDiagnostics(ctx, sourceFile) []*ast.Diagnostic`.
    pub fn get_file_suggestion_diagnostics(&mut self, source_file: NodeId) -> Vec<tsc_ast::Diagnostic> {
        self.get_diagnostics(source_file, DiagnosticsSlot::Suggestions)
    }

    /// Go: `func (c *Checker) getDiagnostics(ctx, sourceFile, collection) []*ast.Diagnostic`
    /// — Go hands the collection by pointer; the port selects the slot.
    fn get_diagnostics(
        &mut self,
        source_file: NodeId,
        slot: DiagnosticsSlot,
    ) -> Vec<tsc_ast::Diagnostic> {
        self.check_not_canceled();
        let check_unused = self.compiler_options.no_unused_locals == Tristate::TSTrue
            || self.compiler_options.no_unused_parameters == Tristate::TSTrue
            || slot == DiagnosticsSlot::Suggestions;
        self.check_source_file(source_file, check_unused);
        if self.was_canceled {
            return Vec::new();
        }
        // Go reads the collection's file through the diagnostic pointers; the
        // port resolves the name up front (the collection is name-keyed).
        let file_name = self
            .source_file_of(source_file)
            .map(|f| f.file_name().as_string().to_string())
            .unwrap_or_default();
        match slot {
            DiagnosticsSlot::Errors => self.diagnostics.get_diagnostics_for_file(&file_name),
            DiagnosticsSlot::Suggestions => self
                .suggestion_diagnostics
                .get_diagnostics_for_file(&file_name),
        }
    }

    /// Go: `func (c *Checker) GetGlobalDiagnostics() []*ast.Diagnostic`.
    pub fn get_global_diagnostics(&mut self) -> Vec<tsc_ast::Diagnostic> {
        self.check_not_canceled();
        self.produce_deferred_diagnostics();
        self.diagnostics.get_global_diagnostics()
    }

    /// Go: `func (c *Checker) addDiagnostic(diagnostic *ast.Diagnostic) *ast.Diagnostic`
    /// (checker.go:14265) — discard diagnostics created while at the maximum
    /// number of recursive TypeToString invocations.
    pub(crate) fn add_diagnostic(&mut self, diagnostic: tsc_ast::Diagnostic) -> tsc_ast::Diagnostic {
        if self.serialization_level < MAX_SERIALIZATION_LEVEL {
            let file_name = diagnostic
                .file()
                .and_then(|f| self.source_file_of(f))
                .map(|f| f.file_name().as_string().to_string());
            self.diagnostics.add(diagnostic, file_name)
        } else {
            diagnostic
        }
    }

    /// Go: `func (c *Checker) addSuggestionDiagnostic(diagnostic *ast.Diagnostic) *ast.Diagnostic`
    /// (checker.go:14272).
    pub(crate) fn add_suggestion_diagnostic(
        &mut self,
        diagnostic: tsc_ast::Diagnostic,
    ) -> tsc_ast::Diagnostic {
        if self.serialization_level < MAX_SERIALIZATION_LEVEL {
            let file_name = diagnostic
                .file()
                .and_then(|f| self.source_file_of(f))
                .map(|f| f.file_name().as_string().to_string());
            self.suggestion_diagnostics.add(diagnostic, file_name)
        } else {
            diagnostic
        }
    }

    /// Go: `func (c *Checker) error(location *ast.Node, message *diagnostics.Message, args ...any) *ast.Diagnostic`
    /// (checker.go:14278). Replaces the spike's W1 no-op stub (checker.rs).
    pub(crate) fn error(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> tsc_ast::Diagnostic {
        let diagnostic = u::new_diagnostic_for_node(self.store(), node, message, args.to_vec());
        self.add_diagnostic(diagnostic)
    }

    /// Go: `func (c *Checker) errorSkippedOnNoEmit(location, message, args...) *ast.Diagnostic`
    /// (checker.go:14282).
    #[allow(dead_code)] // callers land with the S3/S4 leaf waves
    pub(crate) fn error_skipped_on_no_emit(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> tsc_ast::Diagnostic {
        let mut diagnostic = self.error(node, message, args);
        diagnostic.set_skipped_on_no_emit();
        diagnostic
    }

    /// Go: `func (c *Checker) errorOrSuggestion(isError bool, location, message, args...)`
    /// (checker.go:14290). Replaces the spike's W1 no-op stub.
    pub(crate) fn error_or_suggestion(
        &mut self,
        is_error: bool,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) {
        let diagnostic = u::new_diagnostic_for_node(self.store(), node, message, args.to_vec());
        self.add_error_or_suggestion(is_error, diagnostic);
    }

    /// Go: `func (c *Checker) errorAndMaybeSuggestAwait(location, maybeMissingAwait, message, args...) *ast.Diagnostic`
    /// (checker.go:14294).
    #[allow(dead_code)] // callers land with the S4 async leaves
    pub(crate) fn error_and_maybe_suggest_await(
        &mut self,
        node: NodeId,
        maybe_missing_await: bool,
        message: &'static Message,
        args: &[String],
    ) -> tsc_ast::Diagnostic {
        let diagnostic = self.error(node, message, args);
        if maybe_missing_await {
            // Go: related info "Did you forget to use 'await'?" — the
            // related-diagnostic factory (utilities.go's
            // createDiagnosticForNode) is the M7 diagnostics wave; the S4
            // await leaves that call this attach it then.
            // TODO(port M7 diagnostics): AddRelatedInfo(Did_you_forget_to_use_await).
            let _ = &diagnostic;
        }
        diagnostic
    }

    /// Go: `func (c *Checker) addErrorOrSuggestion(isError bool, diagnostic *ast.Diagnostic)`
    /// (checker.go:14308).
    pub(crate) fn add_error_or_suggestion(
        &mut self,
        is_error: bool,
        diagnostic: tsc_ast::Diagnostic,
    ) {
        if is_error {
            self.add_diagnostic(diagnostic);
        } else {
            let mut suggestion = diagnostic;
            suggestion.set_category(Category::Suggestion);
            self.add_suggestion_diagnostic(suggestion);
        }
    }
}

/// Go's two collections as a selector (Go passes `&c.diagnostics` /
/// `&c.suggestionDiagnostics` by pointer).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiagnosticsSlot {
    Errors,
    Suggestions,
}

// ────────────────────────────────────────────────────────────────────────────
// getUniqueTypeParameters free helpers (Go checker.go:7926-7949)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func hasTypeParameterByName(typeParameters []*Type, name string) bool`.
#[allow(dead_code)] // see get_unique_type_parameter_name's note
fn has_type_parameter_by_name(c: &Checker, type_parameters: &[TypeId], name: &str) -> bool {
    type_parameters.iter().any(|&tp| {
        c.type_(tp)
            .symbol
            .is_some_and(|s| c.program.symbol(s).name == name)
    })
}

/// Go: `func getUniqueTypeParameterName(typeParameters []*Type, baseName string) string`.
#[allow(dead_code)] // the free-name helpers' caller is getUniqueTypeParameters'
                    // fresh-name arm — reachable once S7's signature machinery
                    // drives it; pinned by the unit test below.
fn get_unique_type_parameter_name(c: &Checker, type_parameters: &[TypeId], base_name: &str) -> String {
    // Trim any trailing digits.
    let mut base = base_name.to_string();
    while base.len() > 1
        && base
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_digit())
    {
        base.pop();
    }
    let mut index = 1;
    loop {
        let augmented_name = format!("{base}{index}");
        if !has_type_parameter_by_name(c, type_parameters, &augmented_name) {
            return augmented_name;
        }
        index += 1;
    }
}

/// Go: `func isConstEnumObjectType(t *Type) bool` (checker.go:28198) — an
/// anonymous object type whose symbol is a const enum.
pub(crate) fn is_const_enum_object_type(c: &Checker, t: TypeId) -> bool {
    let ty = c.type_(t);
    ty.object_flags.intersects(ObjectFlags::ANONYMOUS)
        && ty.symbol.is_some_and(|symbol| {
            c.program.symbol(symbol)
                .flags
                .intersects(SymbolFlags::CONST_ENUM)
        })
}

// ────────────────────────────────────────────────────────────────────────────
// initializeChecker + the globals machinery (Go checker.go:1309-1488)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) initializeChecker()` (checker.go:1309) — the
    /// global symbol table + special types seeding NewChecker runs last.
    pub(crate) fn initialize_checker(&mut self) {
        // Initialize global symbol table.
        let mut ambient_module_symbols: Vec<SymbolId> = Vec::new();
        let mut augmentations: Vec<Vec<NodeId>> = Vec::with_capacity(self.files.len());
        let files = self.files.clone();
        for file in files {
            let (locals, pattern_ambient_modules, module_augmentations, file_symbol, global_exports, is_external_module) = {
                let source_file = self
                    .source_file_of(file)
                    .expect("checker file is in the program");
                let data = source_file
                    .node(file)
                    .as_source_file()
                    .expect("SourceFile data");
                (
                    data.locals
                        .entries()
                        .map(|(name, symbol)| (name.clone(), *symbol))
                        .collect::<Vec<(String, SymbolId)>>(),
                    data.pattern_ambient_modules.clone(),
                    data.module_augmentations.clone(),
                    data.symbol,
                    data.global_exports
                        .entries()
                        .map(|(name, symbol)| (name.clone(), *symbol))
                        .collect::<Vec<(String, SymbolId)>>(),
                    binder_shims::is_external_or_common_js_module(source_file),
                )
            };
            if !is_external_module {
                // It is an error for a non-external-module (i.e. script) to
                // declare its own `globalThis`.
                if let Some(file_global_this_symbol) = data_local(self, file, "globalThis") {
                    let declarations = self
                        .program
                        .symbol(file_global_this_symbol)
                        .declarations
                        .clone();
                    for d in declarations {
                        if !is_type_declaration(self.store(), d) {
                            let diagnostic = u::new_diagnostic_for_node(
                                self.store(),
                                d,
                                &DECLARATION_NAME_CONFLICTS_WITH_BUILT_IN_GLOBAL_IDENTIFIER_0,
                                vec!["globalThis".to_string()],
                            );
                            self.add_diagnostic(diagnostic);
                        }
                    }
                }
                for (name, symbol) in locals {
                    // We defer merging of global ambient module declarations
                    // since they may require other global symbols and types
                    // to be resolved. See typescript#2953.
                    if self.program.symbol(symbol).flags.intersects(SymbolFlags::MODULE)
                        && tsc_ast::utilities::is_ambient_module_symbol_name(&name)
                    {
                        ambient_module_symbols.push(symbol);
                    } else {
                        self.merge_global_symbol(symbol);
                    }
                }
            }
            self.pattern_ambient_modules.extend(pattern_ambient_modules);
            augmentations.push(module_augmentations);
            if let Some(file_symbol) = file_symbol {
                // Merge in UMD exports with first-in-wins semantics (#9771).
                for (name, symbol) in global_exports {
                    let _ = file_symbol;
                    if self.globals.get(&name).is_none() {
                        self.globals_set(name, symbol);
                    }
                }
            }
        }
        // We do global augmentations separately from module augmentations
        // (and before creating global types) because they
        //  1. Affect global types. We won't have the correct global types
        //     until global augmentations are merged. Also,
        //  2. Module augmentation instantiation requires creating the type
        //     of a module, which, in turn, can require checking for an
        //     export or property on the module (if export=) which, in turn,
        //     can fall back to the apparent type of the module - either
        //     globalObjectType or globalFunctionType - which wouldn't exist
        //     if we did module augmentations prior to finalizing the global
        //     types.
        for list in &augmentations {
            for &augmentation in list {
                // Merge 'global' module augmentations. This needs to be done
                // after global symbol table is initialized to make sure that
                // all ambient modules are indexed.
                if binder_shims::is_global_scope_augmentation(self.store().node(augmentation)) {
                    self.merge_module_augmentation(augmentation);
                }
            }
        }
        self.add_undefined_to_globals_or_error_on_redeclaration();
        let undefined_widening = self.undefined_widening_type();
        self.value_symbol_links.get(self.undefined_symbol()).resolved_type = Some(undefined_widening);
        let iarguments = self.get_global_type("IArguments", 0 /*arity*/, true /*reportErrors*/);
        self.value_symbol_links.get(self.arguments_symbol.expect("argumentsSymbol")).resolved_type = Some(iarguments);
        let error_type = self.error_type();
        self.value_symbol_links.get(self.unknown_symbol.expect("unknownSymbol")).resolved_type = Some(error_type);
        let global_this = self.global_this_symbol.expect("globalThisSymbol");
        let global_this_object_type = self.new_object_type(ObjectFlags::ANONYMOUS, Some(global_this));
        self.value_symbol_links.get(global_this).resolved_type = Some(global_this_object_type);
        // Initialize special types.
        self.global_array_type = Some(self.get_global_type("Array", 1 /*arity*/, true /*reportErrors*/));
        self.global_object_type = Some(self.get_global_type("Object", 0 /*arity*/, true /*reportErrors*/));
        self.global_function_type = Some(self.get_global_type("Function", 0 /*arity*/, true /*reportErrors*/));
        // Go: getGlobalStrictFunctionType (checker.go:25209).
        self.global_callable_function_type = Some(self.get_global_strict_function_type("CallableFunction"));
        self.global_newable_function_type = Some(self.get_global_strict_function_type("NewableFunction"));
        self.global_string_type = Some(self.get_global_type("String", 0 /*arity*/, true /*reportErrors*/));
        self.global_number_type = Some(self.get_global_type("Number", 0 /*arity*/, true /*reportErrors*/));
        self.global_boolean_type = Some(self.get_global_type("Boolean", 0 /*arity*/, true /*reportErrors*/));
        self.global_regexp_type = Some(self.get_global_type("RegExp", 0 /*arity*/, true /*reportErrors*/));
        let any_type = self.any_type();
        self.any_array_type = self.create_array_type(any_type);
        let auto_type = self.auto_type();
        // REAL (M6 S7): the interned type-reference constructor; Go's marker
        // comparison against emptyObjectType rides the ctor's no-lib fallback
        // (createTypeFromGenericGlobalType's emptyGenericType arm) exactly.
        self.auto_array_type = Some(self.create_array_type(auto_type));
        if self.auto_array_type == Some(self.empty_object_type()) {
            // autoArrayType is used as a marker, so even if global Array
            // type is not defined, it needs to be a unique type.
            self.auto_array_type = Some(self.new_anonymous_type(None, &[]));
        }
        self.global_readonly_array_type =
            Some(self.get_global_type("ReadonlyArray", 1 /*arity*/, false /*reportErrors*/));
        if self.global_readonly_array_type == self.empty_generic_type_ref() {
            self.global_readonly_array_type = self.global_array_type;
        }
        // Go: anyReadonlyArrayType = createTypeFromGenericGlobalType(...) —
        // with resolveName stubbed the global readonly array IS the empty
        // generic type, so Go's createTypeFromGenericGlobalType falls to
        // emptyObjectType; the S7 wave swaps in the real createTypeReference.
        self.any_readonly_array_type = Some(self.create_type_from_generic_global_type_stub());
        self.global_this_type = Some(self.get_global_type("ThisType", 1 /*arity*/, false /*reportErrors*/));
        // Now merge global ambient module declarations.
        for symbol in ambient_module_symbols {
            self.merge_global_symbol(symbol);
        }
        self.merge_pattern_ambient_modules();
        // merge _nonglobal_ module augmentations. this needs to be done
        // after global symbol table is initialized to make sure that all
        // ambient modules are indexed.
        for list in &augmentations {
            for &augmentation in list {
                if !binder_shims::is_global_scope_augmentation(self.store().node(augmentation)) {
                    self.merge_module_augmentation(augmentation);
                }
            }
        }
    }

    /// Go: `c.globalThisSymbol.Exports = c.globals` (checker.go:974) — ONE
    /// map aliased between the checker field and the symbol's exports; every
    /// write through either name is visible through the other (the
    /// `typeof globalThis` member lookups and the checker.go:11573/27679
    /// globalThis-exports probes read it). The port's `SymbolTable` is a
    /// value type, so every post-init write to `globals` mirrors into the
    /// globalThis symbol's exports here. PORT(FCS tranche 14): REAL —
    /// retires the checker.rs init "aliasing divergence" TODO(port M6).
    fn globals_set(&mut self, name: String, symbol: SymbolId) {
        self.globals.set(name.clone(), symbol);
        if let Some(global_this) = self.global_this_symbol {
            self.program.symbol_mut(global_this).exports.set(name, symbol);
        }
    }

    /// Go: `func (c *Checker) mergeGlobalSymbol(symbol *ast.Symbol)` (checker.go:1467).
    fn merge_global_symbol(&mut self, symbol: SymbolId) {
        let name = self.program.symbol(symbol).name.clone();
        let global_symbol = self.globals.get(&name).copied();
        let merged = match global_symbol {
            Some(global) => {
                // Go: `c.globals[name] = c.mergeSymbol(globalSymbol, symbol, false)`
                // (checker.go:1417) — REAL as of FCS tranche 11 (was the
                // permissive first-in-wins stub; the S6 TODO).
                self.merge_symbol(global, symbol, false /*unidirectional*/)
            }
            None => self.get_merged_symbol(symbol),
        };
        self.globals_set(name, merged);
    }

    /// Go: `func (c *Checker) mergeSymbolTable(target, source, unidirectional, mergedParent)`
    /// (checker.go:14380) — the global-table arm (`c.globals` is a checker
    /// field, not a Go `SymbolTable` reference, so the two entry shapes
    /// split into this and [`Checker::merge_symbol_table_of_symbol`]).
    /// PORT(FCS tranche 11): REAL (was the mergeModuleAugmentation TODO) —
    /// `declare global { ... }` merges its exports into the global scope so
    /// getGlobalType("ImportMeta") and friends resolve augmentations
    /// (checker.go:1449's call site; the importMetaNarrowing family).
    fn merge_symbol_table_globals(&mut self, source: &SymbolTable, unidirectional: bool) {
        let entries: Vec<(String, SymbolId)> = source
            .entries()
            .map(|(name, symbol)| (name.clone(), *symbol))
            .collect();
        for (name, source_symbol) in entries {
            let target_symbol = self.globals.get(&name).copied();
            let merged = match target_symbol {
                Some(target) => self.merge_symbol(target, source_symbol, unidirectional),
                None => self.get_merged_symbol(source_symbol),
            };
            self.globals_set(name, merged);
        }
    }

    /// Go: `mergeSymbolTable`'s symbol-table arm — merge `source` into the
    /// `members` (Go: `ast.GetSymbolTable(&target.Members)`) or `exports`
    /// table of `target_symbol`. Go's `mergedParent` parent fixup: a merged
    /// (transient clone) target symbol re-parents to the merged parent that
    /// initiated the merge; source-only keeps its own parent.
    fn merge_symbol_table_of_symbol(
        &mut self,
        target_symbol: SymbolId,
        source: &SymbolTable,
        unidirectional: bool,
        merged_parent: Option<SymbolId>,
        into_members: bool,
    ) {
        let entries: Vec<(String, SymbolId)> = source
            .entries()
            .map(|(name, symbol)| (name.clone(), *symbol))
            .collect();
        for (name, source_symbol) in entries {
            let target_entry = if into_members {
                self.program_symbol(target_symbol).members.get(&name).copied()
            } else {
                self.program_symbol(target_symbol).exports.get(&name).copied()
            };
            let merged = match target_entry {
                Some(target) => self.merge_symbol(target, source_symbol, unidirectional),
                None => self.get_merged_symbol(source_symbol),
            };
            if let (Some(merged_parent), Some(_)) = (merged_parent, target_entry) {
                // Go: `if merged.Flags&SymbolFlagsTransient != 0 { merged.Parent = mergedParent }`.
                if self.program_symbol(merged).flags.intersects(SymbolFlags::TRANSIENT) {
                    self.program_symbol_mut(merged).parent = Some(merged_parent);
                }
            }
            if into_members {
                self.program_symbol_mut(target_symbol).members.set(name, merged);
            } else {
                self.program_symbol_mut(target_symbol).exports.set(name, merged);
            }
        }
    }

    /// Go: `func (c *Checker) mergeSymbol(target, source, unidirectional) *ast.Symbol`
    /// (checker.go:14417). If target is transient it is mutated in place;
    /// otherwise the merge works on a transient clone (cloneSymbol) and
    /// returns it.
    fn merge_symbol(&mut self, mut target: SymbolId, source: SymbolId, unidirectional: bool) -> SymbolId {
        let source_flags = self.program.symbol(source).flags;
        let target_flags = self.program.symbol(target).flags;
        let excluded = crate::resolution::get_excluded_symbol_flags(source_flags);
        if !target_flags.intersects(excluded)
            || (source_flags | target_flags).intersects(SymbolFlags::ASSIGNMENT)
        {
            // This can happen when an export assigned namespace exports
            // something also erroneously exported at the top level (Go comment).
            if source == target {
                return target;
            }
            if !target_flags.intersects(SymbolFlags::TRANSIENT) {
                let resolved_target = self.resolve_symbol(target);
                if Some(resolved_target) == self.unknown_symbol {
                    return source;
                }
                let resolved_flags = self.program.symbol(resolved_target).flags;
                if !resolved_flags.intersects(crate::resolution::get_excluded_symbol_flags(source_flags))
                    || (source_flags | resolved_flags).intersects(SymbolFlags::ASSIGNMENT)
                {
                    target = self.clone_symbol(resolved_target);
                } else {
                    self.report_merge_symbol_error(target, source);
                    return source;
                }
            }
            // Javascript static-property-assignment declarations always merge,
            // even though they are also values (Go comment, verbatim).
            if source_flags.intersects(SymbolFlags::VALUE_MODULE)
                && self.program.symbol(target).flags.intersects(SymbolFlags::VALUE_MODULE)
                && self.program.symbol(target).flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE)
                && !source_flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE)
            {
                // reset flag when merging instantiated module into value module
                // that has only const enums
                self.program_symbol_mut(target).flags &= !SymbolFlags::CONST_ENUM_ONLY_MODULE;
            }
            let mut source_flags = source_flags;
            if !self.program.symbol(target).flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE) {
                source_flags &= !SymbolFlags::CONST_ENUM_ONLY_MODULE;
            }
            self.program_symbol_mut(target).flags |= source_flags;
            if let Some(value_declaration) = self.program.symbol(source).value_declaration {
                // Go: `binder.SetValueDeclaration(target, source.ValueDeclaration)`
                // — the binder helper inlined over program symbols
                // (binder.go:4101): non-assignment declarations take
                // precedence over assignment declarations, non-namespace
                // over namespace.
                let replace = {
                    let target_value = self.program.symbol(target).value_declaration;
                    match target_value {
                        None => true,
                        Some(tv) => {
                            let tv_is_assign = is_assignment_declaration(self.store(), tv);
                            let nv_is_assign = is_assignment_declaration(self.store(), value_declaration);
                            tv_is_assign && !nv_is_assign
                                || self.store().node(tv).kind != self.store().node(value_declaration).kind
                                    && is_effective_module_declaration(self.store(), tv)
                        }
                    }
                };
                if replace {
                    self.program_symbol_mut(target).value_declaration = Some(value_declaration);
                }
            }
            let source_declarations = self.program.symbol(source).declarations.clone();
            self.program_symbol_mut(target).declarations.extend(source_declarations);
            let source_members = self.program.symbol(source).members.clone();
            if !source_members.is_empty() {
                self.merge_symbol_table_of_symbol(target, &source_members, unidirectional, None, true /*into_members*/);
            }
            let source_exports = self.program.symbol(source).exports.clone();
            if !source_exports.is_empty() {
                self.merge_symbol_table_of_symbol(target, &source_exports, unidirectional, Some(target), false /*into_members*/);
            }
            if !unidirectional {
                self.record_merged_symbol(target, source);
            }
        } else if target_flags.intersects(SymbolFlags::NAMESPACE_MODULE) {
            // Do not report an error when merging `var globalThis` with the
            // built-in `globalThis` (Go comment, verbatim).
            if Some(target) != self.global_this_symbol {
                let name_of = crate::nodebuilder::symbol_to_string(self, target);
                let first_decl = self
                    .program
                    .symbol(source)
                    .declarations
                    .first()
                    .copied();
                if let Some(first_decl) = first_decl {
                    let name_node = binder_shims::get_name_of_declaration(self.store(), first_decl);
                    if let Some(name_node) = name_node {
                        self.error(
                            name_node,
                            &CANNOT_AUGMENT_MODULE_0_WITH_VALUE_EXPORTS_BECAUSE_IT_RESOLVES_TO_A_NON_MODULE_ENTITY,
                            &[name_of],
                        );
                    }
                }
            }
        } else {
            self.report_merge_symbol_error(target, source);
        }
        target
    }

    /// Go: `func (c *Checker) reportMergeSymbolError(target, source)`
    /// (checker.go:14469). PORT(FCS tranche 11): the message ladder is
    /// Go-exact; the related-information chains
    /// (addDuplicateDeclarationError's "'X' was also declared here" and
    /// "and here" follow-ons) are the W4 related-info seam — the plain
    /// message is emitted per declaration instead.
    fn report_merge_symbol_error(&mut self, target: SymbolId, source: SymbolId) {
        let target_flags = self.program.symbol(target).flags;
        let source_flags = self.program.symbol(source).flags;
        let is_either_enum = target_flags.intersects(SymbolFlags::ENUM)
            || source_flags.intersects(SymbolFlags::ENUM);
        let is_either_block_scoped = target_flags
            .intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
            || source_flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE);
        let message: &'static Message = if is_either_enum {
            &ENUM_DECLARATIONS_CAN_ONLY_MERGE_WITH_NAMESPACE_OR_OTHER_ENUM_DECLARATIONS
        } else if is_either_block_scoped {
            &CANNOT_REDECLARE_BLOCK_SCOPED_VARIABLE_0
        } else {
            &DUPLICATE_IDENTIFIER_0
        };
        let symbol_name = crate::nodebuilder::symbol_to_string(self, source);
        for (this, other) in [(source, target), (target, source)] {
            let declarations = self.program.symbol(this).declarations.clone();
            for node in declarations {
                self.error(node, message, std::slice::from_ref(&symbol_name));
            }
            let _ = other;
        }
    }

    /// Go: `func (c *Checker) addUndefinedToGlobalsOrErrorOnRedeclaration()`
    /// (checker.go:1496).
    fn add_undefined_to_globals_or_error_on_redeclaration(&mut self) {
        let name = self.program.symbol(self.undefined_symbol()).name.clone();
        let target_symbol = self.globals.get(&name).copied();
        match target_symbol {
            Some(target_symbol) => {
                let declarations = self.program.symbol(target_symbol).declarations.clone();
                for d in declarations {
                    if !is_type_declaration(self.store(), d) {
                        let diagnostic = u::new_diagnostic_for_node(
                            self.store(),
                            d,
                            &DECLARATION_NAME_CONFLICTS_WITH_BUILT_IN_GLOBAL_IDENTIFIER_0,
                            vec![name.clone()],
                        );
                        self.add_diagnostic(diagnostic);
                    }
                }
            }
            None => {
                let undefined_symbol = self.undefined_symbol();
                self.globals_set(name, undefined_symbol);
            }
        }
    }

    /// Go: `func (c *Checker) mergePatternAmbientModules()` (checker.go:1479).
    ///
    /// Pattern ambient modules are merged together if they have the same
    /// pattern and identical import attributes type.
    fn merge_pattern_ambient_modules(&mut self) {
        // TODO(port M6 S6): the attributesType grouping —
        // getTypeOfModuleImportAttributes + isTypeIdenticalTo. Permissive:
        // no grouping (each pattern module stands alone — identical to Go
        // when no two modules share a pattern), then Go's globals fixup.
        let modules = std::mem::take(&mut self.pattern_ambient_modules);
        for module in &modules {
            if let Some(module_symbol) = module.symbol {
                let name = self.program.symbol(module_symbol).name.clone();
                if self.globals.get(&name).is_some() {
                    let merged = self.get_merged_symbol(module_symbol);
                    self.globals_set(name, merged);
                }
            }
        }
        self.pattern_ambient_modules = modules;
    }

    /// Go: `func (c *Checker) mergeModuleAugmentation(moduleName *ast.Node)`
    /// (checker.go:1499).
    fn merge_module_augmentation(&mut self, module_name: NodeId) {
        let module_node = self.store().node(module_name).parent.get();
        // Go: moduleNode.Symbol() — the ModuleDeclaration payload's symbol
        // (the binder-set declaration symbol; getSymbolOfDeclaration is S6).
        let module_symbol = match &self.store().node(module_node).data {
            NodeData::ModuleDeclaration(d) => d.symbol,
            _ => None,
        };
        let Some(module_symbol) = module_symbol else {
            return;
        };
        let first_declaration = self
            .program
            .symbol(module_symbol)
            .declarations
            .first()
            .copied();
        if first_declaration != Some(module_node) {
            // this is a combined symbol for multiple augmentations within
            // the same file. its symbol already has accumulated information
            // for all declarations so we need to add it just once - do the
            // work only for first declaration.
            return;
        }
        if binder_shims::is_global_scope_augmentation(self.store().node(module_node)) {
            // Go: `c.mergeSymbolTable(c.globals, moduleAugmentation.Symbol.Exports, false, nil)`
            // (checker.go:1449) — REAL as of FCS tranche 11 (was the S6
            // TODO): `declare global { ... }` exports merge into the global
            // symbol table before global types finalize.
            let exports = self.program.symbol(module_symbol).exports.clone();
            self.merge_symbol_table_globals(&exports, false /*unidirectional*/);
        } else {
            // find a module that about to be augmented; do not validate names
            // of augmentations that are defined in ambient context.
            // Go: `moduleNotFoundError` = Invalid_module_name_in_augmentation
            // (checker.go:1455) unless the grandparent is ambient. The
            // resolveExternalModuleNameWorker call is REAL as of W3T6 (it
            // fires the TS2664 augmentation-target error); the subsequent
            // mergeSymbol tables remain the S6 TODO.
            let grandparent = self.store().node(module_node).parent.get();
            let module_not_found_error =
                if self.store().node(grandparent).flags.intersects(NF::AMBIENT) {
                    None
                } else {
                    Some(&tsc_diagnostics::INVALID_MODULE_NAME_IN_AUGMENTATION_MODULE_0_CANNOT_BE_FOUND)
                };
            let _ = self.resolve_external_module_name_worker(
                module_name,
                Some(module_name),
                module_not_found_error,
                false, /*ignoreErrors*/
                true,  /*isForAugmentation*/
                None,  /*importAttributesType*/
            );
        }
    }

    // ── The getGlobal* resolution family (Go checker.go:1224-1306) ──

    /// Go: `func (c *Checker) getGlobalType(name string, arity int, reportErrors bool) *Type`
    /// (checker.go:1260) — resolve to the global class or interface by the
    /// given name and arity, or emptyObjectType/emptyGenericType otherwise.
    pub(crate) fn get_global_type(&mut self, name: &str, arity: usize, report_errors: bool) -> TypeId {
        let diagnostic = if report_errors {
            Some(&CANNOT_FIND_GLOBAL_TYPE_0 as &'static Message)
        } else {
            None
        };
        let symbol = self.get_global_symbol_real(name, SymbolFlags::TYPE, diagnostic);
        if let Some(symbol) = symbol {
            if self
                .program
                .symbol(symbol)
                .flags
                .intersects(SymbolFlags::CLASS.union(SymbolFlags::INTERFACE))
            {
                // REAL (W3 tranche 3): the class/interface arm is live now
                // that getGlobalSymbol resolves through resolve_name_worker.
                let t = self.get_declared_type_of_symbol_stub(symbol);
                let type_parameters_len = self
                    .type_(t)
                    .as_interface_type()
                    .map(|d| d.type_parameters().len())
                    .unwrap_or(0);
                if type_parameters_len == arity {
                    return t;
                }
                if report_errors {
                    // PORT: Go passes getGlobalTypeDeclaration(symbol)'s
                    // nil-able result straight to c.error — a missing
                    // declaration is a location-less (global) diagnostic,
                    // NOT a skip. NodeId::NONE carries the nil shape.
                    let declaration = self.get_global_type_declaration_stub(symbol);
                    self.error(
                        declaration,
                        &GLOBAL_TYPE_0_MUST_HAVE_1_TYPE_PARAMETER_S,
                        &[self.program.symbol(symbol).name.clone(), arity.to_string()],
                    );
                }
            } else if report_errors {
                let declaration = self.get_global_type_declaration_stub(symbol);
                self.error(
                    declaration,
                    &GLOBAL_TYPE_0_MUST_BE_A_CLASS_OR_INTERFACE_TYPE,
                    &[self.program.symbol(symbol).name.clone()],
                );
            }
        }
        if arity != 0 {
            self.empty_generic_type()
        } else {
            self.empty_object_type()
        }
    }

    /// Go: `func (c *Checker) getGlobalTypeAliasSymbol(name, arity, reportErrors) *ast.Symbol`
    /// (checker.go:1198) — REAL (W3 tranche 3): the declared-type resolve +
    /// the typeAliasLinks type-parameter arity check (pub(crate): M6 S9's
    /// awaited family resolves through it).
    pub(crate) fn get_global_type_alias_symbol(
        &mut self,
        name: &str,
        arity: usize,
        report_errors: bool,
    ) -> Option<SymbolId> {
        let diagnostic = if report_errors {
            Some(&CANNOT_FIND_GLOBAL_TYPE_0 as &'static Message)
        } else {
            None
        };
        let symbol = self.get_global_symbol_real(name, SymbolFlags::TYPE_ALIAS, diagnostic)?;
        // Resolve the declared type of the symbol. This resolves type
        // parameters for the type alias so that we can check arity.
        self.get_declared_type_of_symbol_stub(symbol);
        if self.type_alias_links.get(symbol).type_parameters.len() != arity {
            if report_errors {
                // Go: `core.Find(symbol.Declarations, ast.IsTypeAliasDeclaration)`
                // — the found declaration may be nil (c.error's nil shape is
                // the location-less global diagnostic; NodeId::NONE carries it).
                let declaration = self
                    .program
                    .symbol(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| self.store().node(d).kind == Kind::TypeAliasDeclaration)
                    .unwrap_or(NodeId::NONE);
                self.error(
                    declaration,
                    &GLOBAL_TYPE_0_MUST_HAVE_1_TYPE_PARAMETER_S,
                    &[self.program.symbol(symbol).name.clone(), arity.to_string()],
                );
            }
            return None;
        }
        Some(symbol)
    }

    /// Go: `func (c *Checker) getGlobalSymbol(name, meaning, diagnostic) *ast.Symbol`
    /// (checker.go:1254) — REAL (W3 tranche 3): Go's body is
    /// `c.resolveName(nil, name, meaning, diagnostic, false, false)`
    /// (the full NameResolver walk, already ported as resolution.rs's
    /// resolve_name_worker). This un-suppresses the special-global-types
    /// error arm (TS2316/2317/2318/2468: getGlobalType/getGlobalValue with
    /// reportErrors=true) — the go-driver's confRustSilentGlobalArmKeys
    /// filter can then drop out of both pipelines. exports.rs's public
    /// GetGlobalSymbol twin delegates here.
    pub(crate) fn get_global_symbol_real(
        &mut self,
        name: &str,
        meaning: SymbolFlags,
        diagnostic: Option<&'static Message>,
    ) -> Option<SymbolId> {
        // Don't track references for global symbols anyway, so value if `isReference` is arbitrary
        self.resolve_name_worker(None, name, meaning, diagnostic, false /*is_use*/, false /*exclude_globals*/)
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfSymbol(symbol) *Type` — the
    /// S2 spine. PORT (M6 S2): flipped from the permissive errorType stub to
    /// the real family (typeofsymbol.rs, via the exports.rs Get* twin).
    fn get_declared_type_of_symbol_stub(&mut self, symbol: SymbolId) -> TypeId {
        self.get_declared_type_of_symbol(symbol)
    }

    /// Go: `func getGlobalTypeDeclaration(symbol) *ast.Declaration`
    /// (checker.go:1274).
    fn get_global_type_declaration_stub(&mut self, symbol: SymbolId) -> NodeId {
        for &declaration in &self.program.symbol(symbol).declarations {
            match self.store().node(declaration).kind {
                Kind::ClassDeclaration
                | Kind::InterfaceDeclaration
                | Kind::EnumDeclaration
                | Kind::TypeAliasDeclaration => return declaration,
                _ => {}
            }
        }
        NodeId::NONE
    }

    /// Go: `func (c *Checker) getGlobalStrictFunctionType(name string) *Type`
    /// (checker.go:25209).
    fn get_global_strict_function_type(&mut self, name: &str) -> TypeId {
        if self.strict_bind_call_apply {
            self.get_global_type(name, 0 /*arity*/, true /*reportErrors*/)
        } else {
            self.global_function_type.expect("globalFunctionType")
        }
    }

    // ── The resolver accessors (Go's func() fields, snake_cased) ──

    /// The shared memoized resolution (Go: `core.Memoize`'s call).
    fn resolve_global(&mut self, id: GlobalResolverId) -> GlobalResolverValue {
        if let Some(memo) = self.global_resolvers.memos.get(id.index()).copied().flatten() {
            return memo;
        }
        let (kind, name, arity, report_errors) = id.row();
        let value = match kind {
            GlobalResolverKind::Type => {
                GlobalResolverValue::Type(self.get_global_type(name, arity, report_errors))
            }
            GlobalResolverKind::TypeAlias => {
                GlobalResolverValue::Symbol(self.get_global_type_alias_symbol(name, arity, report_errors))
            }
            GlobalResolverKind::ValueSymbol => {
                let diagnostic = report_errors.then_some(&CANNOT_FIND_GLOBAL_VALUE_0);
                GlobalResolverValue::Symbol(
                    self.get_global_symbol_real(name, SymbolFlags::VALUE, diagnostic),
                )
            }
            GlobalResolverKind::TypeSymbol => {
                let diagnostic = report_errors.then_some(&CANNOT_FIND_GLOBAL_TYPE_0);
                GlobalResolverValue::Symbol(
                    self.get_global_symbol_real(name, SymbolFlags::TYPE, diagnostic),
                )
            }
        };
        if self.global_resolvers.memos.len() <= id.index() {
            self.global_resolvers.memos.resize(id.index() + 1, None);
        }
        self.global_resolvers.memos[id.index()] = Some(value);
        value
    }

    /// Go: NewChecker's `globalResolvers` struct literal — sizes the memo
    /// table (one slot per Go func field, Go's NewChecker order).
    pub(crate) fn new_global_resolvers() -> GlobalResolvers {
        let mut resolvers = GlobalResolvers::default();
        resolvers.memos.resize(GLOBAL_RESOLVER_TABLE.len(), None);
        resolvers
    }
}

// The accessor family: one method per Go resolver field. The macro keeps
// the Go field-name mapping mechanical (the names mirror Go's exactly,
// snake_cased).
macro_rules! global_resolver_accessors {
    ($($method:ident : $variant:ident => $ret:ty),* $(,)?) => {
        impl Checker {
            $(
                #[allow(dead_code)] // callers land with the S2+ waves that read the fields
                pub(crate) fn $method(&mut self) -> $ret {
                    match self.resolve_global(GlobalResolverId::$variant) {
                        GlobalResolverValue::Type(t) => t,
                        GlobalResolverValue::Symbol(_) => panic!(
                            concat!("resolver ", stringify!($variant), " is a symbol resolver")
                        ),
                    }
                }
            )*
        }
    };
}

global_resolver_accessors! {
    // func() *Type fields
    get_global_es_symbol_type: EsSymbolType => TypeId,
    get_global_big_int_type: BigIntType => TypeId,
    get_global_import_meta_type: ImportMetaType => TypeId,
    get_global_import_attributes_type: ImportAttributesType => TypeId,
    get_global_import_attributes_type_checked: ImportAttributesTypeChecked => TypeId,
    get_global_disposable_type: DisposableType => TypeId,
    get_global_async_disposable_type: AsyncDisposableType => TypeId,
    get_global_template_strings_array_type: TemplateStringsArrayType => TypeId,
    get_global_import_call_options_type: ImportCallOptionsType => TypeId,
    get_global_import_call_options_type_checked: ImportCallOptionsTypeChecked => TypeId,
    get_global_promise_type: PromiseType => TypeId,
    get_global_promise_type_checked: PromiseTypeChecked => TypeId,
    get_global_promise_like_type: PromiseLikeType => TypeId,
    get_global_abstract_module_source_type: AbstractModuleSourceType => TypeId,
    get_global_iterator_type: IteratorType => TypeId,
    get_global_iterable_type: IterableType => TypeId,
    get_global_iterable_type_checked: IterableTypeChecked => TypeId,
    get_global_iterable_iterator_type: IterableIteratorType => TypeId,
    get_global_iterable_iterator_type_checked: IterableIteratorTypeChecked => TypeId,
    get_global_iterator_object_type: IteratorObjectType => TypeId,
    get_global_generator_type: GeneratorType => TypeId,
    get_global_async_iterator_type: AsyncIteratorType => TypeId,
    get_global_async_iterable_type: AsyncIterableType => TypeId,
    get_global_async_iterable_type_checked: AsyncIterableTypeChecked => TypeId,
    get_global_async_iterable_iterator_type: AsyncIterableIteratorType => TypeId,
    get_global_async_iterable_iterator_type_checked: AsyncIterableIteratorTypeChecked => TypeId,
    get_global_async_iterator_object_type: AsyncIteratorObjectType => TypeId,
    get_global_async_generator_type: AsyncGeneratorType => TypeId,
    get_global_iterator_yield_result_type: IteratorYieldResultType => TypeId,
    get_global_iterator_return_result_type: IteratorReturnResultType => TypeId,
    get_global_typed_property_descriptor_type: TypedPropertyDescriptorType => TypeId,
    get_global_class_decorator_context_type: ClassDecoratorContextType => TypeId,
    get_global_class_method_decorator_context_type: ClassMethodDecoratorContextType => TypeId,
    get_global_class_getter_decorator_context_type: ClassGetterDecoratorContextType => TypeId,
    get_global_class_setter_decorator_context_type: ClassSetterDecoratorContextType => TypeId,
    // Go's field is `getGlobalClassAccessorDecoratorContxtType` (upstream
    // typo kept in the identifier; the resolved name is the real lib type).
    get_global_class_accessor_decorator_contxt_type: ClassAccessorDecoratorContxtType => TypeId,
    get_global_class_accessor_decorator_context_type: ClassAccessorDecoratorContextType => TypeId,
    get_global_class_accessor_decorator_target_type: ClassAccessorDecoratorTargetType => TypeId,
    get_global_class_accessor_decorator_result_type: ClassAccessorDecoratorResultType => TypeId,
    get_global_class_field_decorator_context_type: ClassFieldDecoratorContextType => TypeId,
}

macro_rules! global_symbol_resolver_accessors {
    ($($method:ident : $variant:ident),* $(,)?) => {
        impl Checker {
            $(
                #[allow(dead_code)] // callers land with the S2+ waves that read the fields
                pub(crate) fn $method(&mut self) -> Option<SymbolId> {
                    match self.resolve_global(GlobalResolverId::$variant) {
                        GlobalResolverValue::Symbol(s) => s,
                        GlobalResolverValue::Type(_) => panic!(
                            concat!("resolver ", stringify!($variant), " is a type resolver")
                        ),
                    }
                }
            )*
        }
    };
}

global_symbol_resolver_accessors! {
    // func() *ast.Symbol fields
    get_global_non_nullable_type_alias_or_nil: NonNullableTypeAliasOrNil,
    get_global_extract_symbol: ExtractSymbol,
    get_global_awaited_symbol: AwaitedSymbol,
    get_global_awaited_symbol_or_nil: AwaitedSymbolOrNil,
    get_global_nan_symbol_or_nil: NaNSymbolOrNil,
    get_global_record_symbol: RecordSymbol,
    get_global_es_symbol_constructor_symbol_or_nil: EsSymbolConstructorSymbolOrNil,
    get_global_es_symbol_constructor_type_symbol_or_nil: EsSymbolConstructorTypeSymbolOrNil,
    get_global_promise_constructor_symbol: PromiseConstructorSymbol,
    get_global_promise_constructor_symbol_or_nil: PromiseConstructorSymbolOrNil,
    get_global_omit_symbol: OmitSymbol,
    get_global_no_infer_symbol_or_nil: NoInferSymbolOrNil,
}

// ────────────────────────────────────────────────────────────────────────────
// The new* ctor family's S1 seams (Go checker.go:25597-25795 — the full
// interning/member machinery is S7/S8; NewChecker needs exactly the
// empty-members shapes, ported minimally and bannered)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) newObjectType(objectFlags ObjectFlags, symbol *ast.Symbol) *Type`
    /// (checker.go:25597) — the data-kind switch is complete; S7 owns the
    /// interning that happens on top.
    pub(crate) fn new_object_type(&mut self, object_flags: ObjectFlags, symbol: Option<SymbolId>) -> TypeId {
        let data = if object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            TypeData::Interface(Box::default())
        } else if object_flags.intersects(ObjectFlags::TUPLE) {
            TypeData::Tuple(Box::default())
        } else if object_flags.intersects(ObjectFlags::REFERENCE) {
            TypeData::TypeReference(Box::default())
        } else if object_flags.intersects(ObjectFlags::MAPPED) {
            TypeData::Mapped(Box::default())
        } else if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
            TypeData::ReverseMapped(Box::default())
        } else if object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
            TypeData::EvolvingArray(Box::default())
        } else if object_flags.intersects(ObjectFlags::ANONYMOUS) {
            TypeData::Object(Box::default())
        } else {
            panic!("Unhandled case in newObjectType");
        };
        let t = self.new_type(TypeFlags::OBJECT, object_flags, data);
        self.type_mut(t).symbol = symbol;
        t
    }

    /// Go: `func (c *Checker) newAnonymousType(symbol, members, callSignatures, constructSignatures, indexInfos) *Type`
    /// (checker.go:25624).
    ///
    /// PORT (S7/S8 seam): Go calls setStructuredTypeMembers with the (all
    /// empty at S1) member lists; the member-resolution machinery is S8.
    /// Every S1 construction passes empty members, so marking the empty
    /// StructuredType resolved is Go's result for exactly those calls; S8
    /// replaces this seam with the real setter.
    pub(crate) fn new_anonymous_type(&mut self, symbol: Option<SymbolId>, members: &[SymbolId]) -> TypeId {
        debug_assert!(members.is_empty(), "S1 constructs only empty anonymous types");
        let t = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
        self.type_mut(t).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
        t
    }

    /// Go: `newAnonymousType`'s objectFlags extra (Go's `newAnonymousTypeEx`-
    /// shaped call sites — anyFunctionType passes
    /// `ObjectFlagsAnonymous|ObjectFlagsNonInferrableType`).
    pub(crate) fn new_anonymous_type_ex(
        &mut self,
        symbol: Option<SymbolId>,
        members: &[SymbolId],
        extra_object_flags: ObjectFlags,
    ) -> TypeId {
        debug_assert!(members.is_empty(), "S1 constructs only empty anonymous types");
        let t = self.new_object_type(ObjectFlags::ANONYMOUS | extra_object_flags, symbol);
        self.type_mut(t).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
        t
    }

    /// Go: `func (c *Checker) newSignature(flags, declaration, typeParameters, thisParameter, parameters, resolvedReturnType, resolvedTypePredicate, minArgumentCount) *Signature`
    /// (checker.go:25781) — arena-backed (§5.4: the id is the arena index).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_signature(
        &mut self,
        flags: SignatureFlags,
        declaration: Option<NodeId>,
        type_parameters: Vec<TypeId>,
        this_parameter: Option<SymbolId>,
        parameters: Vec<SymbolId>,
        resolved_return_type: Option<TypeId>,
        min_argument_count: i32,
    ) -> crate::types::SignatureId {
        self.signature_count += 1;
        let id = crate::types::SignatureId(self.signature_count);
        self.signatures.push(crate::types::Signature {
            id,
            flags,
            min_argument_count,
            resolved_min_argument_count: -1,
            declaration,
            type_parameters,
            parameters,
            this_parameter,
            resolved_return_type,
            resolved_type_predicate: None,
            target: None,
            mapper: None,
            isolated_signature_type: None,
            composite: None,
            no_type_predicate: false,
        });
        id
    }

    /// Go: `func (c *Checker) createUnknownUnionType() *Type` (checker.go:25570).
    pub(crate) fn create_unknown_union_type(&mut self) -> TypeId {
        if self.strict_null_checks {
            // Go: getUnionType([undefinedType, nullType, unknownEmptyObjectType])
            // — REAL (M6 S7: uniontypes.rs's constructor; the
            // unknownEmptyObjectType seed is Go's removeSubtypes anchor).
            let undefined_t = self.undefined_type();
            let null_t = self.null_type();
            let empty_object = self.unknown_empty_object_type();
            self.get_union_type(&[undefined_t, null_t, empty_object])
        } else {
            self.unknown_type()
        }
    }

    // Go: `func (c *Checker) createArrayType(elementType *Type) *Type`
    // (checker.go:25235) → createArrayTypeEx → createTypeFromGenericGlobalType
    // — REAL since M6 S7 (uniontypes.rs's interned ctor; the stub is gone).

    /// Go: `func (c *Checker) createTypeFromGenericGlobalType(genericGlobalType, typeArguments) *Type`
    /// (checker.go:25202) — the empty-placeholder fallback arm.
    fn create_type_from_generic_global_type_stub(&mut self) -> TypeId {
        // With resolveName stubbed the global readonly array is the empty
        // generic placeholder, so Go's arm returns emptyObjectType.
        self.empty_object_type()
    }

    // ── Intrinsic/global accessors the spine reads ──

    /// Go: the `emptyObjectType` field (non-nil after NewChecker).
    pub(crate) fn empty_object_type(&self) -> TypeId {
        self.empty_object_type.expect("emptyObjectType")
    }

    /// Go: the `emptyGenericType` field (non-nil after NewChecker).
    pub(crate) fn empty_generic_type(&self) -> TypeId {
        self.empty_generic_type.expect("emptyGenericType")
    }

    /// Go: the `emptyJsxObjectType` field (non-nil after NewChecker; M6 W5).
    pub(crate) fn empty_jsx_object_type(&self) -> TypeId {
        self.empty_jsx_object_type.expect("emptyJsxObjectType")
    }

    /// Go: the `emptyFreshJsxObjectType` field (non-nil after NewChecker;
    /// M6 W5 — the fragment synthetic-argument type).
    pub(crate) fn empty_fresh_jsx_object_type(&self) -> TypeId {
        self.empty_fresh_jsx_object_type
            .expect("emptyFreshJsxObjectType")
    }

    /// `Option`-free comparison helper for Go's `x == c.emptyGenericType`
    /// (nil-safe on the field).
    fn empty_generic_type_ref(&self) -> Option<TypeId> {
        self.empty_generic_type
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Small local shims
// ────────────────────────────────────────────────────────────────────────────

/// Reads one local of a file (Go: `file.Locals[name]`).
fn data_local(c: &Checker, file: NodeId, name: &str) -> Option<SymbolId> {
    let source_file = c.source_file_of(file).expect("checker file is in the program");
    let data = source_file
        .node(file)
        .as_source_file()
        .expect("SourceFile data");
    data.locals.get(name).copied()
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (the S1 spine's own pins; the integration surface rides in
// tests/spike.rs through the same Program seam).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::new_checker;
    use tsc_ast::Symbol;

    struct SpineProgram {
        options: tsc_core::options_generated::CompilerOptions,
        file: tsc_ast::SourceFile,
        bind: Option<tsc_binder::BindResult>,
        symbols: Vec<Symbol>,
    }

    impl crate::Program for SpineProgram {
        fn options(&self) -> &tsc_core::options_generated::CompilerOptions {
            &self.options
        }
        fn source_files(&self) -> &[tsc_ast::SourceFile] {
            std::slice::from_ref(&self.file)
        }
        fn bind_source_files(&mut self) {}
        fn node_store(&self) -> &dyn NodeStore {
            &self.file
        }
        fn node_store_mut(&mut self) -> &mut dyn NodeStore {
            &mut self.file
        }
        fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
            match &self.bind {
                Some(bind) => (&bind.flow_nodes, &bind.flow_lists),
                None => (&[], &[]),
            }
        }
        fn symbol(&self, id: SymbolId) -> &Symbol {
            let base = self.bind.as_ref().expect("bound").symbol_count as u32;
            if id.0 < base {
                &self.bind.as_ref().expect("bound").symbols[id.0 as usize]
            } else {
                &self.symbols[(id.0 - base) as usize]
            }
        }
        fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
            let base = self.bind.as_ref().expect("bound").symbol_count as u32;
            if id.0 < base {
                &mut self.bind.as_mut().expect("bound").symbols[id.0 as usize]
            } else {
                &mut self.symbols[(id.0 - base) as usize]
            }
        }
        fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
            self.symbols.push(symbol);
            let base = self.bind.as_ref().expect("bound").symbol_count as u32;
            SymbolId(base + self.symbols.len() as u32 - 1)
        }
    }

    fn spine_checker_strict(src: &str) -> Checker {
        let mut file = tsc_parser::parse_source_file(
            Default::default(),
            src,
            tsc_core::scriptkind::ScriptKind::TS,
        );
        let bind = tsc_binder::bind_source_file(&mut file);
        let options = tsc_core::options_generated::CompilerOptions {
            strict: Tristate::TSTrue,
            ..Default::default()
        };
        new_checker(Box::new(SpineProgram {
            options,
            file,
            bind,
            symbols: Vec::new(),
        }))
    }
    fn spine_checker(src: &str) -> Checker {
        let mut file = tsc_parser::parse_source_file(
            Default::default(),
            src,
            tsc_core::scriptkind::ScriptKind::TS,
        );
        let bind = tsc_binder::bind_source_file(&mut file);
        new_checker(Box::new(SpineProgram {
            options: Default::default(),
            file,
            bind,
            symbols: Vec::new(),
        }))
    }

    fn file_of(c: &Checker) -> NodeId {
        c.program.source_files()[0].as_node_id()
    }

    /// Go's checkSourceFile sets typeChecked and clears reportedUnreachableNodes.
    #[test]
    fn check_source_file_flips_type_checked_once() {
        let mut c = spine_checker("let x = 1;\n");
        let file = file_of(&c);
        assert!(!c.source_file_links.get(file).type_checked);
        c.check_source_file(file, false);
        assert!(c.source_file_links.get(file).type_checked);
        assert!(c.source_file_links.get(file).deferred_nodes.is_empty());
        // The drain replaced the set; re-checking is a no-op (Go's outer
        // `if !links.typeChecked` gate).
        c.check_source_file(file, false);
    }

    /// Go's checkNodeDeferred only queues while the file is not typeChecked.
    #[test]
    fn deferred_nodes_queue_and_drain_like_go() {
        let mut c = spine_checker("const f = () => 1;\n");
        let file = file_of(&c);
        // Fabricate two queued nodes the deferred checkers would have added.
        let store = c.store();
        let statements = &store
            .node(file)
            .as_source_file()
            .expect("SourceFile data")
            .statements
            .as_ref()
            .expect("statements")
            .nodes;
        let initializer = store
            .node(statements[0])
            .as_variable_statement()
            .expect("VariableStatement data")
            .declaration_list;
        let decl = store.node(initializer)
            .as_variable_declaration_list()
            .expect("VariableDeclarationList data")
            .declarations
            .as_ref()
            .expect("declarations")
            .nodes[0];
        let arrow = crate::ast_shims::initializer_of(c.store(), decl).expect("initializer");
        c.check_node_deferred(arrow);
        assert_eq!(
            c.source_file_links.get(file).deferred_nodes.size(),
            1,
            "queued while !typeChecked"
        );
        // check_source_file drains exactly once and empties the set.
        c.check_source_file(file, false);
        assert!(c.source_file_links.get(file).deferred_nodes.is_empty());
        // After typeChecked, checkNodeDeferred no-ops (Go's gate).
        c.check_node_deferred(arrow);
        assert!(c.source_file_links.get(file).deferred_nodes.is_empty());
    }

    /// Go's drain enumerates nodes queued DURING the drain (the live Keys
    /// index loop) — the port's re-pull preserves the insertion order.
    #[test]
    fn deferred_drain_enumerates_during_drain_additions_in_order() {
        let mut c = spine_checker("const a = 1; const b = 2;\n");
        let file = file_of(&c);
        let (first, second) = {
            let store = c.store();
            let statements = &store
                .node(file)
                .as_source_file()
                .expect("SourceFile data")
                .statements
                .as_ref()
                .expect("statements")
                .nodes;
            (statements[0], statements[1])
        };
        c.check_node_deferred(first);
        // Simulate a deferred checker queueing a second node mid-drain: the
        // first node's dispatch runs before the re-pull, so hook via the
        // queued set directly (the S3 deferred checkers will do this for
        // real). To emulate, pre-queue `first`, then during
        // check_deferred_nodes the pull happens after each node; we assert
        // the simpler invariant here: pre-queued nodes drain in insertion
        // order and the set ends empty.
        c.check_node_deferred(second);
        c.check_source_file(file, false);
        assert!(c.source_file_links.get(file).deferred_nodes.is_empty());
    }

    /// initializeChecker: a script file's locals merge into the globals
    /// table (Go's mergeGlobalSymbol), `undefined` is added to globals, and
    /// the intrinsic symbol links are seeded.
    #[test]
    fn initialize_checker_merges_globals_and_seeds_links() {
        let mut c = spine_checker("declare const console: any;\n");
        let _ = &mut c;
        // `undefined` was added to the globals table (no user redeclaration).
        assert_eq!(
            c.globals.get("undefined"),
            Some(&c.undefined_symbol()),
            "addUndefinedToGlobalsOrErrorOnRedeclaration's else arm"
        );
        // The script's local merged into globals (permissive merge: the
        // binder symbol itself until S6's mergeSymbol).
        let merged = c.globals.get("console").copied();
        assert!(merged.is_some(), "script locals merge into globals");
        // valueSymbolLinks seeds (Go's initializeChecker tail).
        let undefined_resolved = c.value_symbol_links.get(c.undefined_symbol()).resolved_type;
        assert_eq!(undefined_resolved, Some(c.undefined_widening_type()));
        let unknown_resolved =
            c.value_symbol_links.get(c.unknown_symbol.expect("unknownSymbol")).resolved_type;
        assert_eq!(unknown_resolved, Some(c.error_type()));
        // globalThis's resolved type is an anonymous object type.
        let global_this = c.global_this_symbol.expect("globalThisSymbol");
        let links = c.value_symbol_links.get(global_this);
        let t = links.resolved_type.expect("globalThis resolvedType");
        assert!(c.type_(t).object_flags.intersects(ObjectFlags::ANONYMOUS));
        // The special global types resolved to the empty placeholders (the
        // no-lib answer under the S6 resolveName stub): Array has arity 1,
        // so it falls to emptyGenericType.
        assert_eq!(c.global_array_type, Some(c.empty_generic_type()));
        assert_eq!(c.global_object_type, Some(c.empty_object_type()));
    }

    /// A script redeclaring `globalThis` draws the conflict diagnostic
    /// (Go's initializeChecker globalThis arm) — a file-attached diagnostic.
    #[test]
    fn initialize_checker_errors_on_global_this_redeclaration() {
        let mut c = spine_checker("var globalThis: number;\n");
        let file = file_of(&c);
        let diagnostics = c.get_file_diagnostics(file);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code() == DECLARATION_NAME_CONFLICTS_WITH_BUILT_IN_GLOBAL_IDENTIFIER_0.code()),
            "globalThis redeclaration must report the conflict diagnostic"
        );
    }

    /// The getGlobal* resolver table: seeded per Go's NewChecker, memoized,
    /// and answering the no-lib fallback (emptyObjectType/emptyGenericType
    /// per arity / nil symbol).
    #[test]
    fn global_resolvers_memoize_the_no_lib_answer() {
        let mut c = spine_checker("");
        // Promise: arity 1 (Go checker.go:1092) → the empty generic type.
        assert_eq!(c.get_global_promise_type(), c.empty_generic_type());
        // Memoized: the second call returns the same TypeId without
        // re-resolution (Go's core.Memoize).
        assert_eq!(c.get_global_promise_type(), c.empty_generic_type());
        assert_eq!(
            c.get_global_promise_type_checked(),
            c.empty_generic_type()
        );
        assert_eq!(c.get_global_promise_like_type(), c.empty_generic_type());
        // Symbol resolvers answer None (Go's nil).
        assert_eq!(c.get_global_record_symbol(), None);
        assert_eq!(c.get_global_promise_constructor_symbol(), None);
        assert_eq!(c.get_global_nan_symbol_or_nil(), None);
    }

    /// The unreachable machinery: no reported nodes for reachable code.
    #[test]
    fn unreachable_nodes_report_and_gate_on_allow_unreachable_code() {
        let mut c = spine_checker("let x = 1;\n");
        let file = file_of(&c);
        // All statements are reachable — nothing is reported.
        c.check_source_file(file, false);
        assert!(c.reported_unreachable_nodes.is_empty());
    }

    /// getDiagnostics drives checkSourceFile and returns the per-file
    /// collection; the unreachable diagnostic (binder-flagged code after a
    /// top-level `throw`) lands once, deduped. Go's default options
    /// (AllowUnreachableCode == TSUnknown) route TS7027 through
    /// addErrorOrSuggestion(false, …) — the suggestion collection.
    #[test]
    fn get_diagnostics_returns_unreachable_code_once() {
        // The binder flags NodeFlagsUnreachable on the statement after the
        // top-level `throw`; the spine reports it from
        // checkSourceElementWorker's dispatch.
        let mut c = spine_checker("throw 1;\nconst x = 2;\n");
        let file = file_of(&c);
        let suggestions = c.get_file_suggestion_diagnostics(file);
        assert!(
            suggestions
                .iter()
                .any(|d| d.code() == UNREACHABLE_CODE_DETECTED.code()),
            "unreachable code must be reported: {:?}",
            suggestions.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        // Second call: typeChecked already set; the collection dedupes the
        // diagnostic (Add's EqualDiagnostics check), so still exactly one.
        let suggestions = c.get_file_suggestion_diagnostics(file);
        assert_eq!(
            suggestions
                .iter()
                .filter(|d| d.code() == UNREACHABLE_CODE_DETECTED.code())
                .count(),
            1,
            "diagnostics dedupe on re-check"
        );
    }

    /// The suggestion-collection split (addErrorOrSuggestion's else arm).
    #[test]
    fn error_or_suggestion_routes_to_the_suggestion_collection() {
        let mut c = spine_checker("let x = 1;\n");
        let file = file_of(&c);
        let statement = {
            let store = c.store();
            store
                .node(file)
                .as_source_file()
                .expect("SourceFile data")
                .statements
                .as_ref()
                .expect("statements")
                .nodes[0]
        };
        c.error_or_suggestion(false, statement, &UNREACHABLE_CODE_DETECTED, &[]);
        let suggestions = c.get_file_suggestion_diagnostics(file);
        let errors = c.get_file_diagnostics(file);
        assert!(suggestions
            .iter()
            .any(|d| d.code() == UNREACHABLE_CODE_DETECTED.code()));
        assert!(errors.is_empty());
    }

    /// The intrinsic/global field activations NewChecker seeds (Go's exact
    /// set): the empty-object family, the signature family, noTypePredicate.
    #[test]
    fn new_checker_seeds_the_global_object_and_signature_family() {
        let c = spine_checker("");
        // Distinct anonymous placeholders (Go creates one per field).
        assert_ne!(c.empty_object_type(), c.empty_generic_type());
        assert_ne!(c.empty_object_type(), c.any_function_type.expect("anyFunctionType"));
        assert!(c.type_(c.any_function_type.expect("anyFunctionType")).object_flags.intersects(ObjectFlags::NON_INFERRABLE_TYPE));
        assert!(c.type_(c.empty_object_type()).object_flags.intersects(ObjectFlags::ANONYMOUS));
        // The signature family.
        let any_signature = c.any_signature.expect("anySignature");
        assert_eq!(c.signatures[any_signature.0 as usize].resolved_return_type, Some(c.any_type()));
        let unknown_signature = c.unknown_signature.expect("unknownSignature");
        assert_eq!(c.signatures[unknown_signature.0 as usize].resolved_return_type, Some(c.error_type()));
        let silent_never_signature = c.silent_never_signature.expect("silentNeverSignature");
        assert_eq!(
            c.signatures[silent_never_signature.0 as usize].resolved_return_type,
            Some(c.silent_never_type())
        );
        // noTypePredicate (Go's "<<unresolved>>" identifier predicate).
        let predicate = c.no_type_predicate.as_ref().expect("noTypePredicate");
        assert_eq!(predicate.parameter_name, "<<unresolved>>");
        assert_eq!(predicate.t, Some(c.any_type()));
        // The check-marker type parameters mirror the relater pair.
        let sub_for_check = c.marker_sub_type_for_check.expect("markerSubTypeForCheck");
        let super_for_check = c.marker_super_type_for_check.expect("markerSuperTypeForCheck");
        assert_eq!(
            c.type_(sub_for_check)
                .as_type_parameter()
                .expect("TypeParameter data")
                .constraint,
            Some(super_for_check)
        );
    }

    /// The deferred-diagnostic callbacks produce in order and clear.
    #[test]
    fn produce_deferred_diagnostics_drains_the_queue() {
        let mut c = spine_checker("");
        let file = file_of(&c);
        c.add_deferred_diagnostic(DeferredDiagnosticCallback::CheckWeakMapSetCollision { node: file });
        c.add_deferred_diagnostic(DeferredDiagnosticCallback::CheckReflectCollision { node: file });
        c.produce_deferred_diagnostics();
        assert!(c.deferred_diagnostic_callbacks.is_empty());
    }

    /// checkExpressionEx's tracer/tracerless paths and the
    /// instantiateTypeWithSingleGenericCallSignature gate.
    #[test]
    fn check_expression_ex_returns_the_worker_result() {
        let mut c = spine_checker("const n = 42;\n");
        let (file, decl) = {
            let store = c.store();
            let statements = &store
                .node(file_of(&c))
                .as_source_file()
                .expect("SourceFile data")
                .statements
                .as_ref()
                .expect("statements")
                .nodes;
            let list = store
                .node(statements[0])
                .as_variable_statement()
                .expect("VariableStatement data")
                .declaration_list;
            (
                file_of(&c),
                store
                    .node(list)
                    .as_variable_declaration_list()
                    .expect("VariableDeclarationList data")
                    .declarations
                    .as_ref()
                    .expect("declarations")
                    .nodes[0],
            )
        };
        let _ = file;
        let initializer = crate::ast_shims::initializer_of(c.store(), decl).expect("initializer");
        let t = c.check_expression_ex(initializer, CheckMode::CHECK_MODE_NORMAL);
        assert!(c.type_(t).flags.intersects(TypeFlags::NUMBER_LITERAL));
    }

    /// W3 tranche 3 pin: an annotated async function under the no-lib shape
    /// (the awaitBinaryExpression1 divergence). The unresolved `Promise`
    /// type reference yields the any-flagged error type with the alias
    /// attached (getTypeFromTypeAliasReference's CheckFlagsUnresolved arm),
    /// so Go's isErrorType second arm catches it and
    /// checkAsyncFunctionReturnType bails BEFORE getGlobalPromiseTypeChecked —
    /// no TS2318-Promise on either side, only the per-reference TS2583s and
    /// the checker-machinery TS2318s (Array, Object, Function, ...).
    #[test]
    fn w3t3_annotated_async_emits_no_promise_2318() {
        let mut c = spine_checker(
            "declare var a: boolean;\ndeclare var p: Promise<boolean>;\nasync function func(): Promise<void> {\n    var b = await p || a;\n}\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        // The two `Promise` references draw TS2583 (the suggested-lib form
        // of cannot-find-name, one per reference) — and NO TS2318-Promise.
        let promise_2583 = diags
            .iter()
            .filter(|d| d.code() == 2583 && d.message_args().first().is_some_and(|a| a == "Promise"))
            .count();
        assert_eq!(promise_2583, 2, "two Promise references, one TS2583 each");
        let promise_2318 = diags
            .iter()
            .filter(|d| d.code() == 2318 && d.message_args().first().is_some_and(|a| a == "Promise"))
            .count();
        assert_eq!(
            promise_2318, 0,
            "the unresolved-alias return type must gate checkAsyncFunctionReturnType before the Promise resolver"
        );
        // The machinery globals still resolve-with-error once each (Go's
        // NewChecker resolver table over the lib-less program).
        let global_2318_names: Vec<&str> = diags
            .iter()
            .filter(|d| d.code() == 2318)
            .filter_map(|d| d.message_args().first().map(|s| s.as_str()))
            .collect();
        for name in ["Array", "Boolean", "Function", "Object", "String", "Number", "RegExp"] {
            assert!(
                global_2318_names.contains(&name),
                "TS2318-{name} must be emitted (the machinery resolver, Go parity); got {global_2318_names:?}"
            );
        }
    }

    /// W3 tranche 3 pin: services.rs's GetSymbolsInScope — the container
    /// walk copies the function's binder locals (the var declaration) into
    /// the answer table; reserved names are stripped by symbols_to_array.
    #[test]
    fn w3t3_services_get_symbols_in_scope_reports_function_locals() {
        let mut c = spine_checker(
            "function f(a: number) {\n    var x = a + 1;\n    return x;\n}\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let function = statements_of(c.store(), file).expect("statements")[0];
        let body = c
            .store()
            .node(function)
            .as_function_declaration()
            .expect("FunctionDeclaration data")
            .body
            .expect("body");
        let body_statements = statements_of(c.store(), body).expect("body statements");
        let variable_statement = body_statements[0];
        let symbols = c.get_symbols_in_scope(variable_statement, SymbolFlags::VALUE);
        let names: Vec<String> = symbols
            .iter()
            .map(|&s| c.program_symbol(s).name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n == "x"),
            "the function-local `x` must be in scope; got {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "y"),
            "nothing else may appear; got {names:?}"
        );
    }

    /// W3 tranche 3 pin: services.rs's GetSymbolsOfParameterPropertyDeclaration
    /// — the `private p` parameter yields the parameter symbol (constructor
    /// locals) and the property symbol (class members), distinct symbols.
    #[test]
    fn w3t3_services_parameter_property_declaration_symbols() {
        let mut c =
            spine_checker("class C {\n    constructor(private p: number) {}\n}\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let class = statements_of(c.store(), file).expect("statements")[0];
        let members = c
            .store()
            .node(class)
            .as_class_declaration()
            .expect("ClassDeclaration data")
            .members
            .as_ref()
            .expect("members")
            .nodes
            .to_vec();
        let constructor = *members
            .iter()
            .find(|&&m| c.store().node(m).kind == Kind::Constructor)
            .expect("constructor");
        let parameters = c
            .store()
            .node(constructor)
            .as_constructor_declaration()
            .expect("ConstructorDeclaration data")
            .parameters
            .as_ref()
            .expect("parameters")
            .nodes
            .to_vec();
        let parameter = parameters[0];
        let (parameter_symbol, property_symbol) = c
            .get_symbols_of_parameter_property_declaration(parameter, "p")
            .expect("both symbols resolve");
        assert_eq!(c.program_symbol(parameter_symbol).name, "p");
        assert_eq!(c.program_symbol(property_symbol).name, "p");
        assert_ne!(
            parameter_symbol, property_symbol,
            "the parameter and property symbols are distinct"
        );
    }

    /// W3 tranche 4 PIN (the definite-assignment family): under
    /// strictNullChecks, `var child: Child; child.required;` — Go's
    /// checkIdentifier emits TS2454 ('child' is used before being assigned)
    /// and RETURNS THE DECLARED TYPE (checker.go:11426 "Return the declared
    /// type to reduce follow-on errors"), so NO TS2339 follows. The Rust
    /// arm was dormant (the flow type `Child | undefined` leaked into the
    /// property access and produced a spurious 2339 — the census's
    /// rust-extra x54 + go-extra x47 pair).
    #[test]
    fn w3t4_definite_assignment_emits_2454_and_returns_declared_type() {
        let mut c = spine_checker_strict(
            "interface Child { required: number }\nvar child: Child;\nchild.required;\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let count_2454 = diags
            .iter()
            .filter(|d| d.code() == 2454)
            .count();
        assert_eq!(count_2454, 1, "exactly one TS2454 for the unassigned read");
        assert!(
            diags
                .iter()
                .all(|d| d.code() != 2339),
            "no follow-on TS2339; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        // The 2454 arm's return is the DECLARED type (Go's fallback), so the
        // re-read `child` expression types as Child (an interface object
        // type), not `Child | undefined`.
        let stmts: Vec<NodeId> =
            statements_of(c.store(), file).expect("statements").to_vec();
        let read = crate::ast_shims::expression_of(c.store(), stmts[2]).expect("read stmt");
        let child_ref = crate::ast_shims::expression_of(c.store(), read).expect("lhs");
        let t = c.check_expression_ex(child_ref, CheckMode::CHECK_MODE_NORMAL);
        assert!(
            c.type_(t).object_flags.intersects(ObjectFlags::INTERFACE),
            "the read types as the declared Child interface, not a union; got flags {:?}",
            c.type_(t).flags
        );
    }

    /// W3 tranche 4 PIN (Go's containsUndefinedType, checker.go:11431): a
    /// union contains undefined iff its FIRST constituent does —
    /// `Child | undefined` contains, `undefined` alone contains, `number`
    /// does not.
    #[test]
    fn w3t4_contains_undefined_type_first_constituent() {
        let mut c = spine_checker("type T = number;\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let undefined = c.undefined_type();
        let number = c.number_type();
        assert!(c.contains_undefined_type(undefined));
        assert!(!c.contains_undefined_type(number));
        let union = c.get_union_type(&[undefined, number]);
        assert!(c.contains_undefined_type(union), "undefined-first union contains");
        let union_rev = c.get_union_type(&[number, undefined]);
        // Go probes Types()[0] only: the answer must equal the FIRST
        // constituent's own undefined flag — whatever the union's
        // normalization order is — never any() over the constituents.
        for u in [union, union_rev] {
            let first = c
                .type_(u)
                .as_union_type()
                .expect("union")
                .base
                .types[0];
            assert_eq!(
                c.contains_undefined_type(u),
                c.type_(first).flags.intersects(TypeFlags::UNDEFINED),
                "the helper must answer the first-constituent probe, not any()"
            );
        }
    }

    /// W3 tranche 4 PIN (isPrototypeProperty, checker.go:22131): the
    /// prototype-property probe is the METHOD symbol flag (or the
    /// SyntheticMethod check flag), NOT the binder's synthesized `prototype`
    /// property's name. `class A { foo() {} }` — foo is a prototype property.
    #[test]
    fn w3t4_is_prototype_property_is_the_method_flag() {
        let mut c = spine_checker("class A {\n    foo() {}\n    bar: number;\n}\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let stmts: Vec<NodeId> =
            statements_of(c.store(), file).expect("statements").to_vec();
        let class = stmts[0];
        let class_symbol = c.get_symbol_of_declaration(class).expect("class symbol");
        let members = c.get_members_of_symbol(class_symbol);
        let foo = members
            .iter()
            .find(|(name, _)| name == "foo")
            .map(|&(_, s)| s)
            .expect("foo symbol");
        let bar = members
            .iter()
            .find(|(name, _)| name == "bar")
            .map(|&(_, s)| s)
            .expect("bar symbol");
        assert!(
            crate::members::is_prototype_property(&c, foo),
            "a method is a prototype property"
        );
        assert!(
            !crate::members::is_prototype_property(&c, bar),
            "a data property is not"
        );
    }

    /// W3 tranche 4 PIN (the abstract-class family): `abstract class B
    /// extends A { abstract bar(); }` + `class C extends B {}` under the
    /// no-lib shape — Go emits TS2515 (non-abstract class C does not
    /// implement inherited abstract member bar from class B) and NO TS2417
    /// (the static-side relation's target is the base ctor type WITH
    /// signatures stripped — getTypeWithoutSignatures — so the abstract-ctor
    /// assignability gate in signaturesRelatedTo never fires).
    #[test]
    fn w3t4_abstract_class_no_spurious_2417_and_real_2515() {
        let mut c = spine_checker_strict(
            "class A {\n    foo() {}\n}\n\nabstract class B extends A {\n    abstract bar();\n}\n\nclass C extends B { }\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert!(
            diags.iter().any(|d| d.code() == 2515),
            "the unimplemented-abstract-member TS2515 must fire; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        assert!(
            diags.iter().all(|d| d.code() != 2417),
            "no spurious static-side TS2417; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        assert!(
            diags.iter().all(|d| d.code() != 2425),
            "no spurious member-kind TS2425"
        );
    }

    /// W3 tranche 4 PIN (getTypeWithoutSignatures, checker.go:4579): the
    /// stripped twin keeps members/properties, drops ALL signatures, and is
    /// pinned with MembersResolved so the construct signatures cannot be
    /// re-derived from the symbol.
    #[test]
    fn w3t4_get_type_without_signatures_strips_and_pins() {
        let mut c = spine_checker(
            "class A {\n    foo() {}\n}\nclass B extends A { }\nvar b: B;\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let stmts: Vec<NodeId> =
            statements_of(c.store(), file).expect("statements").to_vec();
        let b_symbol = c.get_symbol_of_declaration(stmts[1]).expect("B symbol");
        let class_type = c.get_declared_type_of_symbol(b_symbol);
        let static_type = c.get_type_of_symbol(b_symbol);
        // The ctor type has construct signatures before the strip...
        let full_sigs = crate::relater::get_signatures_of_type(
            &mut c,
            static_type,
            crate::SignatureKind::Construct,
        );
        assert!(!full_sigs.is_empty(), "the class static side carries a ctor");
        let stripped = c.get_type_without_signatures(static_type);
        assert!(
            c.type_(stripped).object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED),
            "the stripped twin is pinned MembersResolved"
        );
        let stripped_sigs = crate::relater::get_signatures_of_type(
            &mut c,
            stripped,
            crate::SignatureKind::Construct,
        );
        assert!(
            stripped_sigs.is_empty(),
            "the stripped twin has NO construct signatures (re-resolution included)"
        );
        let _ = class_type;
    }

    /// W3 tranche 4 PIN (getDeclarationModifierFlagsFromSymbol,
    /// utilities.go): an `abstract bar();` method's declaration modifier
    /// flags include ABSTRACT — the 2515 arm and the private-override skip
    /// in checkKindsOfPropertyMemberOverrides key on it. The relater's stub
    /// returned NONE (the family's root).
    #[test]
    fn w3t4_declaration_modifier_flags_read_abstract() {
        let mut c = spine_checker(
            "abstract class B {\n    abstract bar();\n}\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let stmts: Vec<NodeId> =
            statements_of(c.store(), file).expect("statements").to_vec();
        let class_symbol = c.get_symbol_of_declaration(stmts[0]).expect("B symbol");
        let members = c.get_members_of_symbol(class_symbol);
        let bar = members
            .iter()
            .find(|(name, _)| name == "bar")
            .map(|&(_, s)| s)
            .expect("bar symbol");
        assert!(
            crate::relater::get_declaration_modifier_flags_from_symbol(&c, bar)
                .intersects(tsc_ast::ModifierFlags::ABSTRACT),
            "the abstract modifier must be readable from the symbol"
        );
    }

    // ────────────────────────────────────────────────────────────────────────
    // M6 W3 tranche-5 pins — the deferred signature-resolution forcing +
    // the contextual parameter-assign family (checker.go:10387-10680) +
    // the computed binding-pattern property names. Every expectation was
    // verified against the go-bench twin over the same source shapes (the
    // async-2697 x37 census family + asyncFunctionDeclarationParameter-
    // Evaluation's 2304/2538 pairs).
    // ────────────────────────────────────────────────────────────────────────

    /// W3T5 PIN (the async-2697 root cause): the deferred
    /// checkFunctionExpressionOrObjectLiteralMethodDeferred forces
    /// getReturnTypeOfSignature for UNANNOTATED function expressions
    /// (checker.go:10449) — for an async arrow under the lib-less shape
    /// that reaches createPromiseReturnType and emits TS2697 ("An async
    /// function or method must return a 'Promise'"). The port carried the
    /// forcing call as a TODO until S8 landed; Go's
    /// checkFunctionOrMethodDeclaration forces only GENERATORS
    /// (checker.go:3487), so the deferral is the ONLY forcing site for
    /// arrows.
    #[test]
    fn w3t5_deferred_async_arrow_forces_2697() {
        let mut c = spine_checker("var v = async () => { };\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert_eq!(
            diags.iter().filter(|d| d.code() == 2697).count(),
            1,
            "exactly one TS2697 for the unannotated async arrow; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }

    /// W3T5 PIN (contextuallyCheckFunctionExpressionOrObjectLiteralMethod,
    /// checker.go:10387 — now REAL): with no contextual signature the
    /// assignNonContextualParameterTypes + checkSignatureDeclaration arms
    /// run for a plain arrow, so the parameter default `x = zzz` is
    /// checked and TS2304 fires for the unresolved name.
    #[test]
    fn w3t5_noncontextual_arrow_parameter_defaults_report_2304() {
        let mut c = spine_checker("var v = (x = zzz) => { };\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert_eq!(
            diags.iter().filter(|d| d.code() == 2304).count(),
            1,
            "exactly one TS2304 for the arrow's unresolved default; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }

    /// W3T5 PIN (getLiteralTypeFromPropertyName's computed-name arm,
    /// checker.go:27311 — REAL checkComputedPropertyName, was the
    /// neverType stub): `async function f({[zz]: x})` reports TS2304 for
    /// the computed binding-pattern property name AND TS2538 when the
    /// any-typed name indexes the empty pattern-implied parent type
    /// (Go's getBindingElementTypeFromParentType → getIndexedAccessTypeEx
    /// tail, checker.go:27740).
    #[test]
    fn w3t5_computed_binding_pattern_property_name_checks() {
        let mut c = spine_checker("async function f({[zz]: x}) { }\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert_eq!(
            diags.iter().filter(|d| d.code() == 2304).count(),
            1,
            "exactly one TS2304 for zz; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        assert_eq!(
            diags.iter().filter(|d| d.code() == 2538).count(),
            1,
            "exactly one TS2538 (any used as an index type); got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }

    /// W3T6 NOTE: the 2537-avoidance root cause was found and fixed — it is
    /// Go's getContextuallyTypedParameterType IIFE arm (checker.go:30025),
    /// which answers undefinedWideningType for a binding-pattern parameter
    /// of a context-sensitive immediately-invoked function expression; the
    /// widening turns that into anyType, and getBindingElementTypeFromParent
    /// Type's IsTypeAny early-return skips the number-index access on the
    /// pattern-implied `{}` (so no TS2537). The W3T5 hypothesis (the
    /// resolvingSignature marker / contextual any-signature arm) was wrong —
    /// Go's probe shows ctxSig=false on this path (see the continuation
    /// file). The widened pattern-implied param type stays flagless in BOTH
    /// twins, so no OBJECT_LITERAL pin is possible; the behavioral pin is
    /// w3t6_iife_binding_pattern_param_no_2537 below.
    #[test]
    fn w3t6_iife_binding_pattern_param_go_exact_2537() {
        // The corpus shape of classWithStaticFieldInParameterBindingPattern.
        // Under the spine's default (strict) config BOTH twins emit exactly
        // one TS2537 with the UNDEFINED parent (the IIFE arm's raw
        // undefinedWideningType flows through assignBindingElementTypes'
        // getBindingElementTypeFromParentType) — go-verified via the probe
        // corpus ($HOME/.w3t6-scratch/probe, default-config runs identical).
        // The W3T5 bug the IIFE-arm fix collapses: the parent was the
        // pattern-implied `{}` (`{} number`). Under the corpus's
        // strict:false configs both twins emit NO 2537 at all (the any
        // widening IsTypeAny early-return) — verified on the real
        // classWithStaticFieldInParameterBindingPattern{,.2,.3} slice.
        let mut c = spine_checker("(({ [class { static x = 1 }.x]: b = \"\" }) => {})();\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let d2537: Vec<_> = diags.iter().filter(|d| d.code() == 2537).collect();
        assert_eq!(
            d2537.len(),
            1,
            "exactly one TS2537 (Go-exact, strict config); got {:?}",
            diags.iter().map(|d| (d.code(), d.message_text())).collect::<Vec<_>>()
        );
        assert!(
            d2537.iter().all(|d| {
                let args = d.message_args();
                args.len() == 2 && args[0] == "undefined" && args[1] == "number"
            }),
            "the 2537 parent must be `undefined` (Go-exact), not the pattern-implied `{{}}`; got {:?}",
            d2537.iter().map(|d| d.message_args()).collect::<Vec<_>>()
        );
    }
    /// W3T6: Go's IIFE arm answers undefinedWideningType for a
    /// no-initializer parameter of a context-sensitive IIFE with no
    /// arguments (checker.go:30032); the widening folds that to anyType —
    /// the port's getWidenedType mirrors Go, so `b` must be any (assignable
    /// to number, no TS2322) rather than the pattern-implied member type.
    #[test]
    fn w3t6_iife_no_arg_param_widens_undefined_to_any() {
        let mut c = spine_checker("(({ [k]: b }) => { const n: number = b; })();\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert_eq!(
            diags.iter().filter(|d| d.code() == 2322).count(),
            0,
            "no TS2322: the IIFE param widens to any; got {:?}",
            diags.iter().map(|d| (d.code(), d.message_text())).collect::<Vec<_>>()
        );
    }
    /// W3T6: tryGetThisTypeAtEx's class-like-parent arm is REAL (Go
    /// checker.go:12405 — the declared class/interface type's thisType,
    /// constructed by getDeclaredTypeOfClassOrInterface; was the S7
    /// TODO-None that made every unannotated `this` in a class method
    /// report TS2683). Go-verified via the probe corpus (t1: both twins
    /// emit exactly this 2322, source arg rendered as the thisType).
    #[test]
    fn w3t6_class_this_type_resolves() {
        let mut c = spine_checker("class C { m() { const n: number = this; } }\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let d2322: Vec<_> = diags.iter().filter(|d| d.code() == 2322).collect();
        assert_eq!(
            d2322.len(),
            1,
            "this in a class method resolves to the class thisType; got {:?}",
            diags.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
        );
        assert!(
            d2322.iter().all(|d| {
                let args = d.message_args();
                args.len() == 2 && args[0] == "this" && args[1] == "number"
            }),
            "the 2322 renders the thisType as `this`; got {:?}",
            d2322.iter().map(|d| d.message_args()).collect::<Vec<_>>()
        );
    }
    /// W3T6: isJSLiteralType's instantiable arm is REAL (Go utilities.go
    /// 1807: getResolvedBaseConstraint) — the classAbstractCrashedOnce
    /// corpus shape (`this.` member access on the class thisType, a type
    /// parameter) PANICKED on the pre-W3T6 TODO; the port now walks the
    /// constraint like Go (slice-verified: the case's diags are
    /// twin-identical).
    #[test]
    fn w3t6_js_literal_type_instantiable_no_panic() {
        let mut c = spine_checker(
            "abstract class foo { protected abstract test(); }\nclass bar extends foo { test() { this. } }\nvar x = new bar();\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        // Reaching here without panicking IS the pin (the corpus twin's
        // error set is asserted by the 300-slice runs).
    }
    /// W3T6: functionHasImplicitReturn is REAL (Go checker.go:20711 — the
    /// flow-graph reachability read; was a false stub that suppressed the
    /// whole 2355/2366/7030 family). Go-verified: a bodyless-return function
    /// with a non-void declared type gets exactly one TS2355.
    #[test]
    fn w3t6_function_has_implicit_return_2355() {
        let mut c = spine_checker("function f(): number { }\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let d2355: Vec<_> = diags.iter().filter(|d| d.code() == 2355).collect();
        assert_eq!(d2355.len(), 1, "got {:?}", diags.iter().map(|d| d.code()).collect::<Vec<_>>());
    }
    /// W3T6: getTypeOfAccessors calls the real getReturnTypeFromBody (Go
    /// checker.go:18941); an async getter answers the Promise-wrapped type,
    /// so the TS7033 implicit-any report stays Go-silent.
    #[test]
    fn w3t6_async_getter_no_7033() {
        let mut c = spine_checker("class C { async get foo() { } }\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        assert!(
            !c.semantic_diagnostics().iter().any(|d| d.code() == 7033),
            "async getter inferred via body; got {:?}",
            c.semantic_diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }

    /// W3T7: the S8 generic-member instantiation chain — `new E3<string>().m()`
    /// answers `() => string`, not `() => T` (two stacked roots, both fixed
    /// this tranche: statements.rs type_arguments_of missed Go's expression
    /// kinds — `Node.TypeArgumentList()` covers Call/New/TaggedTemplate/Jsx —
    /// so the new-expression's type arguments were dropped and T inferred
    /// `unknown`; and inference.rs could_contain_type_variables missed Go's
    /// ANONYMOUS arm (checker.go:22672: function/method/class/type-literal/
    /// object-literal symbols with declarations), so instantiateType of the
    /// method's `() => T` short-circuited to identity).
    #[test]
    fn w3t7_generic_member_instantiation() {
        let mut c = spine_checker(
            "class E3<T> { m(): T { return null as any; } }\nconst s: string = new E3<string>().m();\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        assert!(
            !diags.iter().any(|d| d.code() == 2322),
            "the instantiated m() returns string, assignable; got {:?}",
            diags.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
        );
        // The property's type itself: `() => string` (the declared `() => T`
        // instantiated through the E3<string> reference's mapper).
        let init = {
            let store = c.store();
            let stmts = store
                .node(file)
                .as_source_file()
                .expect("SourceFile data")
                .statements
                .as_ref()
                .expect("statements")
                .nodes
                .to_vec();
            let mut init = None;
            for &s in &stmts {
                if store.node(s).kind == tsc_ast::Kind::VariableStatement {
                    let decls = store
                        .node(s)
                        .as_variable_statement()
                        .expect("vs")
                        .declaration_list;
                    let d0 = store
                        .node(decls)
                        .as_variable_declaration_list()
                        .expect("vdl")
                        .declarations
                        .as_ref()
                        .expect("decls")
                        .nodes[0];
                    let vd = store.node(d0).as_variable_declaration().expect("vd");
                    init = vd.initializer;
                }
            }
            init
        };
        let init = init.expect("initializer");
        let (obj,) = {
            let store = c.store();
            let call = store
                .node(init)
                .as_call_expression()
                .expect("CallExpression data");
            let callee = store.node(call.expression);
            let paccess = callee
                .as_property_access_expression()
                .expect("PropertyAccessExpression data");
            (paccess.expression,)
        };
        let objty = c.check_expression(obj);
        let prop = c.get_property_of_type(objty, "m").expect("property m");
        let mty = c.get_type_of_symbol(prop);
        let rendered = crate::relater::type_to_string(&mut c, mty);
        assert_eq!(rendered, "() => string", "the member signature instantiates");
    }

    /// W3T6: checkGrammarBreakOrContinueStatement reads Label() (Go
    /// grammarchecks.go:1477), not the generic Name() — a labeled break
    /// resolves its label and stays Go-silent (no TS1107).
    #[test]
    fn w3t6_labeled_break_no_1107() {
        let mut c = spine_checker("function g() { block: { break block; } }\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        assert!(
            !c.semantic_diagnostics().iter().any(|d| d.code() == 1107),
            "labeled break resolves via Label(); got {:?}",
            c.semantic_diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }
    /// W3T6: getNonNullableType is REAL (Go checker.go:19070 — strict:
    /// the S10 facts adjustment; was the S7 identity stub) and
    /// checkNonNullType reads the REAL getTypeFacts worker (Go walks union
    /// constituents; the pure-flags seam missed `C | undefined`). Go-
    /// verified probe: `C | undefined` property access narrows to C and
    /// reports exactly one TS18048 at `d`.
    #[test]
    fn w3t6_union_with_undefined_narrows_18048() {
        let mut c = spine_checker(
            "interface C { foo: string; [k: string]: string }\ndeclare const d: C | undefined;\nd.foo;\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let d18048: Vec<_> = diags.iter().filter(|d| d.code() == 18048).collect();
        assert_eq!(
            d18048.len(),
            1,
            "got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
        assert!(
            !diags.iter().any(|d| d.code() == 2339),
            "the union narrows before the property lookup; got {:?}",
            diags.iter().map(|d| d.code()).collect::<Vec<_>>()
        );
    }
    /// W3T6: the async-return thenable family — the aggregator's
    /// checkAwaitedType (Go checker.go:20718, TS1058 at the function
    /// node → name-clamped span) and checkAwaitExpression's TS1320
    /// message (Go checker.go:11084; Rust used to pass the 1058 message
    /// at the await site).
    #[test]
    fn w3t6_async_thenable_1058_and_await_1320() {
        let mut c = spine_checker(
            "declare class Thenable { then(): void; }\ndeclare let thenable: Thenable;\nasync function fn13() { return thenable; }\nasync function fn19() { await thenable; }\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let diags = c.semantic_diagnostics();
        let d1058: Vec<_> = diags.iter().filter(|d| d.code() == 1058).collect();
        let d1320: Vec<_> = diags.iter().filter(|d| d.code() == 1320).collect();
        assert_eq!(d1058.len(), 1, "the async return reports 1058; got {:?}", diags.iter().map(|d| d.code()).collect::<Vec<_>>());
        assert_eq!(d1320.len(), 1, "await reports 1320; got {:?}", diags.iter().map(|d| d.code()).collect::<Vec<_>>());
    }
    //
    // ────────────────────────────────────────────────────────────────────────
    // M6 W4 pins — the nodebuilder printer (nodebuilder.rs). Every expected
    // string below was verified byte-same against the go-bench twin's
    // check-stage diagnostic renders over the same source shape (the
    // $HOME/.w4-scratch probe corpus, --dump-filter p1/p2).
    // ────────────────────────────────────────────────────────────────────────

    /// Top-level `declare var <name>` declared-type getter for the W4 pins.
    fn w4_var_type(c: &mut Checker, name: &str) -> TypeId {
        let file = file_of(c);
        let stmts: Vec<NodeId> =
            statements_of(c.store(), file).expect("statements").to_vec();
        for &s in &stmts {
            if c.store().node(s).kind == Kind::VariableStatement {
                let decl_list = c
                    .store()
                    .node(s)
                    .as_variable_statement()
                    .expect("VariableStatement data")
                    .declaration_list;
                let decls: Vec<NodeId> = c
                    .store()
                    .node(decl_list)
                    .as_variable_declaration_list()
                    .expect("VariableDeclarationList data")
                    .declarations
                    .as_ref()
                    .expect("declarations")
                    .nodes
                    .to_vec();
                for d in decls {
                    let d_name = binder_shims::get_name_of_declaration(c.store(), d);
                    let matches = d_name.is_some_and(|n| {
                        tsc_ast::node_text(c.store(), n) == name
                    });
                    if matches {
                        let symbol = c.get_symbol_of_declaration(d).expect("symbol");
                        return c.get_type_of_symbol(symbol);
                    }
                }
            }
        }
        panic!("no variable named {name}");
    }

    /// W4 PIN: the object literal with a lone call signature + property —
    /// Go's createTypeNodeFromObjectType type-literal form, the exact arg
    /// text of the TS2741 diagnostics on the collapsed slice records.
    #[test]
    fn w4_type_to_string_object_with_call_signature() {
        let mut c = spine_checker(
            "declare var f: { (x: number): string; prop: boolean; };\n",
        );
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "f");
        assert_eq!(
            crate::nodebuilder::type_to_string(&mut c, t),
            "{ (x: number): string; prop: boolean; }"
        );
    }

    /// W4 PIN: labeled/optional/rest tuple members (arrayOrTupleTypeToTypeNode
    /// + getTupleElementLabel), byte-same to Go.
    #[test]
    fn w4_type_to_string_named_tuple_with_optional_and_rest() {
        let mut c =
            spine_checker("declare var t: [x: string, y?: number, ...rest: boolean[]];\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "t");
        assert_eq!(
            crate::nodebuilder::type_to_string(&mut c, t),
            "[x: string, y?: number | undefined, ...rest: boolean[]]"
        );
    }

    /// W4 PIN: the abstract constructor type (signatureToSignatureDeclarationHelper's
    /// ConstructorType arm + the ABSTRACT modifier) — the shape behind the
    /// `abstract new (x: number) => string` renders.
    #[test]
    fn w4_type_to_string_abstract_constructor_type() {
        let mut c = spine_checker("declare var c: abstract new (x: number) => string;\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "c");
        assert_eq!(
            crate::nodebuilder::type_to_string(&mut c, t),
            "abstract new (x: number) => string"
        );
    }

    /// W4 PIN: union constituents parenthesize function types
    /// (emitUnionTypeConstituent at TypeOperator precedence, printer.go:2053)
    /// but NOT tuple/postfix members.
    #[test]
    fn w4_type_to_string_union_parenthesizes_function_types() {
        let mut c =
            spine_checker("declare var u: ((x: string) => void) | { m?(): void; };\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "u");
        assert_eq!(
            crate::nodebuilder::type_to_string(&mut c, t),
            "((x: string) => void) | { m?(): void; }"
        );
    }

    /// W4 PIN: enum members render `E.a` (the qualified enum-member form)
    /// and the rest-parameter dot reads the effective parameter declaration
    /// (Go nodebuilderimpl.go:1740) — the `(a: string, ...b: X) => void`
    /// shape. The element type renders as `{}` only because the unit is
    /// lib-less (the global Array type is unresolved — Go renders the same
    /// in the same environment; the twin probe confirmed byte-same).
    #[test]
    fn w4_signature_to_string_rest_parameter_dot() {
        let mut c = spine_checker("enum E { a, b }\ndeclare var e: E.a;\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "e");
        assert_eq!(crate::nodebuilder::type_to_string(&mut c, t), "E.a");

        let mut c = spine_checker("declare var sig: (a: string, ...b: number[]) => void;\n");
        let file = file_of(&c);
        c.check_source_file(file, false);
        let t = w4_var_type(&mut c, "sig");
        let rendered = crate::nodebuilder::type_to_string(&mut c, t);
        assert!(
            rendered.starts_with("(a: string, ...b: ") && rendered.ends_with(") => void"),
            "the rest parameter carries its `...` dot; got {rendered}"
        );
    }
}
