//! FCS24 pins (full-corpus check sweep, tranche 24): the
//! intraExpressionInferences missing-2339 family — the last two
//! single-instance divergences on the
//! types/typeRelationships/typeInference/intraExpressionInferences.ts
//! record (its 11-instance census) and the intraExpressionInferencesJsx.tsx
//! record (its 3-instance census).
//!
//! ROOT A (the .ts record — the `callItT([_a => 0, n => n.toFixed()])`
//! tuple case): Go's `checkArrayLiteral` (checker.go:8277-8281) adds an
//! intra-expression inference site for a context-sensitive element under
//! `inTupleContext && CheckModeInferential &&
//! !CheckModeSkipContextSensitive`. The port's element loop had the M6
//! S6/S9 `TODO: addIntraExpressionInferenceSite` stub — the context-sensitive
//! producer `_a => 0` is omitted from the first argument-inference pass, its
//! return type never reached `inferFromIntraExpressionSites`, `T` stayed
//! `unknown`, and the consumer reported TS18046 (`n` is unknown) where Go
//! reports TS2339 (`toFixed` on `number`).
//!
//! ROOT B (the .tsx record — the `<Foo {...{ a, b }} />` spread-attribute
//! case): Go's `createJsxAttributesTypeFromAttributesProperty`
//! (jsx.go:736-739) reads `member := attributeDecl.Symbol()` and branches
//! on `ast.IsJsxAttribute(attributeDecl)` FIRST — the symbol is only used
//! inside the attribute arm, and the SPREAD arm runs unconditionally. The
//! port hoisted the member lookup (name() → JsxAttribute payload symbol)
//! above the kind branch and `continue`d when it resolved to None — a
//! JsxSpreadAttribute has neither, so EVERY spread attribute was silently
//! dropped from the attributes type: the spread props never merged (the
//! attributes type collapsed to `{}`, TS2739 against `Props<unknown>`)
//! and the nested literal's intra-expression inference sites were never
//! collected.
//!
//! Expectations verified byte-same against the go-bench twin's check-stage
//! dumps (rig probes f24p1/f24p2/f24p3/f24p4, both drivers, DIFF 0) — the
//! pins run WITHOUT libs (the corpus twin seam: `toFixed` on `number`
//! reports 2339 there, which is what makes the inference observable).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23 idiom) ──

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
}

impl Program for TestProgram {
    fn options(&self) -> &CompilerOptions {
        &self.options
    }
    fn source_files(&self) -> &[SourceFile] {
        std::slice::from_ref(&self.source_file)
    }
    fn bind_source_files(&mut self) {}
    fn node_store(&self) -> &dyn NodeStore {
        &self.source_file
    }
    fn node_store_mut(&mut self) -> &mut dyn NodeStore {
        &mut self.source_file
    }
    fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
        (&self.bind.flow_nodes, &self.bind.flow_lists)
    }
    fn symbol(&self, id: SymbolId) -> &Symbol {
        if id.0 < self.binder_symbol_count {
            &self.bind.symbols[id.0 as usize]
        } else {
            &self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        if id.0 < self.binder_symbol_count {
            &mut self.bind.symbols[id.0 as usize]
        } else {
            &mut self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.checker_symbols.push(symbol);
        SymbolId(self.binder_symbol_count + self.checker_symbols.len() as u32 - 1)
    }
    fn source_file_name(&self, _file: tsc_ast::NodeId) -> String {
        self.name.as_string().to_string()
    }
    fn source_file_meta_data(&self, _file: tsc_ast::NodeId) -> tsc_ast::SourceFileMetaData {
        tsc_ast::SourceFileMetaData {
            package_json_type: String::new(),
            package_json_directory: Default::default(),
            implied_node_format: ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

/// Check `src` with `kind` (TS/TSX), no libs, noImplicitAny — the conformance
/// harness configuration the twin drivers run.
fn check_kind(src: &str, kind: ScriptKind) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, kind);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the tuple intra-expression site (ROOT A) — both callItT
// variants infer `T = number`: the non-context-sensitive `() => 0` rides
// inferFromAnnotatedParametersAndReturn, and the context-sensitive
// `_a => 0` rides the checkArrayLiteral site → inferFromIntra
// ExpressionSites. Both consumers report 2339 toFixed/number (no libs),
// never TS18046 ──

const TUPLE_INTRA_EXPRESSION: &str = "\
declare function callItT<T>(obj: [(n: number) => T, (x: T) => void]): void;\n\
callItT([() => 0, n => n.toFixed()]);\n\
callItT([_a => 0, n => n.toFixed()]);\n\
";

#[test]
fn tuple_context_sensitive_producer_infers_through_site_pin() {
    // Pre-fix: the second call's `n` stayed `unknown` (TS18046) because the
    // context-sensitive producer was omitted from the first inference pass
    // and the site list never carried its return type. Post-fix both
    // consumers read `number`. Verified byte-same against the go-bench twin
    // (rig probe f24p1: two 2339 toFixed/number, nothing else).
    let (mut c, file) = check_kind(TUPLE_INTRA_EXPRESSION, ScriptKind::TS);
    let ds = diags(&mut c, file);
    let to_fixed: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, args)| *code == 2339 && args.first() == Some(&"toFixed".to_string()))
        .map(|(_, args)| args)
        .collect();
    assert_eq!(to_fixed.len(), 2, "both tuple consumers: {ds:?}");
    assert!(
        to_fixed.iter().all(|args| args[1] == "number"),
        "T inferred number in both variants: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 18046),
        "no unknown-parameter leaks: {ds:?}"
    );
}

// ── Pin 2: the JSX spread-attribute intra-expression inference (ROOT B)
// — `<Foo {...{ a: (x) => 10, b: (arg) => { arg.toString(); } }} />`
// merges the spread into the attributes type AND collects the nested
// literal's inference sites: T infers number (2339 toString/number), no
// TS2739 missing-properties report ──

const JSX_SPREAD_INFERENCE: &str = "\
interface Props<T> {\n\
  a: (x: string) => T;\n\
  b: (arg: T) => void;\n\
}\n\
declare function Foo<T>(props: Props<T>): any;\n\
<Foo {...{ a: (x) => 10, b: (arg) => { arg.toString(); }, }} />;\n\
";

#[test]
fn jsx_spread_attributes_infer_and_merge_pin() {
    // Pre-fix: the spread attribute was dropped from the attributes loop
    // (its member lookup ran before the kind branch) — the attributes type
    // collapsed to `{}` (TS2739 'a, b' missing against Props<unknown>) and
    // no inference ever ran. Post-fix the spread merges and the intra-
    // expression sites fire. Verified byte-same against the go-bench twin
    // (rig probe f24p2: 17004 jsx-flag + one 2339 toString/number).
    let (mut c, file) = check_kind(JSX_SPREAD_INFERENCE, ScriptKind::TSX);
    let ds = diags(&mut c, file);
    let to_string: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, args)| *code == 2339 && args.first() == Some(&"toString".to_string()))
        .map(|(_, args)| args)
        .collect();
    assert_eq!(to_string.len(), 1, "one toString on the inferred T: {ds:?}");
    assert_eq!(to_string[0][1], "number", "T inferred number from a's return: {ds:?}");
    assert!(
        !ds.iter().any(|(code, _)| *code == 2739),
        "the spread merged into the attributes type (no missing a, b): {ds:?}"
    );
}

// ── Pin 3: mixed attribute-then-spread (no over-apply) — the attribute
// arm flushes its table before the spread arm, the spread still merges
// (no TS2739 for b), but the spread member's parameter stays `unknown`
// (TS18046) exactly like the Go twin — the fix must not invent inference
// where Go has none ──

const JSX_MIXED_ATTR_AND_SPREAD: &str = "\
interface Props<T> {\n\
  a: (x: string) => T;\n\
  b: (arg: T) => void;\n\
}\n\
declare function Foo<T>(props: Props<T>): any;\n\
<Foo a={(x) => 10} {...{ b: (arg) => { arg.toString(); } }} />;\n\
";

#[test]
fn jsx_mixed_attr_and_spread_matches_go_no_over_apply_pin() {
    // Verified byte-same against the go-bench twin (rig probe f24p3:
    // 17004 + exactly one 18046 'arg' unknown, no 2739, no 2339). The
    // pre-fix port would have DROPPED the spread — TS2739 'b' missing and
    // no 18046 — so this pins both the merge and the no-over-apply edge.
    let (mut c, file) = check_kind(JSX_MIXED_ATTR_AND_SPREAD, ScriptKind::TSX);
    let ds = diags(&mut c, file);
    let unknowns: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 18046)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(unknowns.len(), 1, "arg reads unknown like Go: {ds:?}");
    assert_eq!(unknowns[0][0], "arg", "{ds:?}");
    assert!(
        !ds.iter().any(|(code, _)| *code == 2739),
        "the spread's b merged into the attributes type: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 2339),
        "no invented inference: arg was never narrowed to number: {ds:?}"
    );
}

// ── Pin 4: a plain (non-generic) spread-only element — the spread merges
// into the attributes type silently; with the dropped-spread bug this shape
// reported TS2739 (c missing) ──

const JSX_PLAIN_SPREAD: &str = "\
interface Props2 { c: string; }\n\
declare function Bar(props: Props2): any;\n\
<Bar {...{ c: \"s\" }} />;\n\
";

#[test]
fn jsx_plain_spread_merges_into_attributes_type_pin() {
    // Verified byte-same against the go-bench twin (rig probe f24p4:
    // only the jsx-flag 17004 — the attributes type carries c). The
    // pre-fix port reported TS2739 ('c' missing from the collapsed `{}`
    // attributes type).
    let (mut c, file) = check_kind(JSX_PLAIN_SPREAD, ScriptKind::TSX);
    let ds = diags(&mut c, file);
    assert!(
        !ds
            .iter()
            .any(|(code, _)| *code == 2739 || *code == 2339 || *code == 18046 || *code == 7006),
        "the spread attributes merge silently: {ds:?}"
    );
}
