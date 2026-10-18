//! FCS46 pins (full-corpus check sweep, tranche 46): the top coherent
//! clusters of the fresh t46-final-dump census — TS7053 rust-MISSING
//! x23/2rec (indexSignatures1 x21 + keyofAndIndexedAccess x2) and TS2407
//! rust-MISSING x13/4rec (for-inStatementsInvalid x10 +
//! privateNameInInExpression x2 + for-inStatements x1).
//! ("FCS tranche 46" PORT notes in-tree.)
//!
//! TWO roots, both Go-exact:
//!
//! ROOT A — TS7053 + the inverted branded-index TS2322s: Go's `forEachType`
//! (checker.go:27061) distributes UNIONS only; the port's members.rs twin
//! distributed `UNION_OR_INTERSECTION`, destructuring a branded-intersection
//! index-signature key (`[key: string & Tag1]` in indexSignatures1.ts) into
//! its constituents — `string` silently created a plain string index info
//! and Tag1 was rejected, so every branded access lost its TS7053 and the
//! I1/I2/I4 index-signature assignability inverted (I3 was accidentally
//! Go-exact: its union key is distributed one level to the two branded
//! intersections, which the callback then kept whole).
//!
//! ROOT B — TS2407: Go's `checkForInStatement` right-side gate calls the REAL
//! `isTypeAssignableToKind(rightType, NonPrimitive | InstantiableNonPrimitive)`
//! (checker.go:4050); the port called a permissive S10 stub (always true),
//! so the for-in RHS diagnostic never fired (void/string/number/boolean
//! right-hand sides in for-inStatementsInvalid, boolean in
//! privateNameInInExpression).

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
}

fn check_unit(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
    );
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diags(c: &mut Checker) -> Vec<(i32, Vec<String>)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn args_of(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, a)| a.clone())
        .collect()
}

fn options() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — TS7053 + the branded-index TS2322s (forEachType) ──

/// The indexSignatures1.ts branded-intersection section: `[key: string &
/// Tag1]` index info must keep the WHOLE intersection as its key type, so
/// `i1[s0]` (string), `i1[s2]` (the other brand) and `i1[s3]` (the union)
/// each fire exactly one TS7053, the same-brand accesses stay silent, and
/// the interface assignments report exactly Go's set (I2→I1, I4→I1,
/// I1→I3) — never the pre-fix inverted I3→I1/I2/I4.
#[test]
fn fcs46_branded_index_7053_pin() {
    let (mut c, _) = check_unit(
        r#"
type Tag1 = { __tag1__: void };
type Tag2 = { __tag2__: void };
type TaggedString1 = string & Tag1;
type TaggedString2 = string & Tag2;
interface I1 { [key: TaggedString1]: string }
interface I2 { [key: TaggedString2]: string }
interface I3 { [key: TaggedString1 | TaggedString2]: string }
interface I4 { [key: TaggedString1 & TaggedString2]: string }
declare let s0: string;
declare let s1: TaggedString1;
declare let s2: TaggedString2;
declare let s3: TaggedString1 | TaggedString2;
declare let s4: TaggedString1 & TaggedString2;
declare let i1: I1;
declare let i2: I2;
declare let i3: I3;
declare let i4: I4;
i1[s0];
i1[s1];
i1[s2];
i1[s3];
i1[s4];
i1 = i2;
i1 = i3;
i1 = i4;
i3 = i1;
i4 = i1;
"#,
        "/x/brandedIndex.ts",
        options(),
    );
    let ds = diags(&mut c);
    let a7053 = args_of(&ds, 7053);
    assert_eq!(a7053.len(), 3, "expected 3 TS7053 (s0/s2/s3 on I1), got: {ds:?}");
    assert!(
        a7053.iter().any(|a| a == &["string".to_string(), "I1".to_string()]),
        "expected (string, I1), got: {ds:?}"
    );
    assert!(
        a7053.iter().any(|a| a.first().map(String::as_str) == Some("TaggedString2")),
        "expected (TaggedString2, I1), got: {ds:?}"
    );
    assert!(
        a7053
            .iter()
            .any(|a| a.first().map(String::as_str) == Some("TaggedString1 | TaggedString2")),
        "expected the branded union as index expression (Go's exact alias-name rendering, no parens), got: {ds:?}"
    );
    let a2322 = args_of(&ds, 2322);
    assert_eq!(a2322.len(), 3, "expected exactly I2→I1, I4→I1, I1→I3, got: {ds:?}");
    for pair in [("I2", "I1"), ("I4", "I1"), ("I1", "I3")] {
        assert!(
            a2322
                .iter()
                .any(|a| a.first().map(String::as_str) == Some(pair.0)
                    && a.get(1).map(String::as_str) == Some(pair.1)),
            "expected 2322 {pair:?}, got: {ds:?}"
        );
    }
    assert!(
        !a2322.iter().any(|a| a.first().map(String::as_str) == Some("I3")),
        "the pre-fix inverted I3→… 2322s must be gone, got: {ds:?}"
    );
}

/// keyof of the branded interface is the branded key itself (TaggedString1)
/// — the pre-fix port reduced the key to plain `string`, making keyof
/// "string | number" and this assignment silently pass.
#[test]
fn fcs46_branded_index_keyof_pin() {
    let (mut c, _) = check_unit(
        r#"
type Tag1 = { __tag1__: void };
type TaggedString1 = string & Tag1;
interface I1 { [key: TaggedString1]: string }
declare let k1: keyof I1;
declare let kk: string;
kk = k1;
k1 = kk;
"#,
        "/x/brandedKeyof.ts",
        options(),
    );
    let ds = diags(&mut c);
    let a2322 = args_of(&ds, 2322);
    assert_eq!(a2322.len(), 1, "expected only k1 = kk to error, got: {ds:?}");
    assert!(
        a2322
            .iter()
            .any(|a| a.first().map(String::as_str) == Some("string")
                && a.get(1).map(String::as_str) == Some("TaggedString1")),
        "expected (string, TaggedString1), got: {ds:?}"
    );
}

/// The object-literal twin of the same section (indexSignatures1's o1..o4):
/// a type-literal's branded index info must stay branded too.
#[test]
fn fcs46_branded_object_literal_7053_pin() {
    let (mut c, _) = check_unit(
        r#"
type Tag1 = { __tag1__: void };
type TaggedString1 = string & Tag1;
declare let o1: { [key: TaggedString1]: string };
declare let s0: string;
declare let s1: TaggedString1;
o1[s0];
o1[s1];
"#,
        "/x/brandedLiteral.ts",
        options(),
    );
    let ds = diags(&mut c);
    let a7053 = args_of(&ds, 7053);
    assert_eq!(a7053.len(), 1, "expected exactly one TS7053 (o1[s0]), got: {ds:?}");
    assert!(
        a7053.iter().any(|a| a.first().map(String::as_str) == Some("string")),
        "expected the string access to fire, got: {ds:?}"
    );
}

// ── ROOT B — TS2407 (for-in right-side gate) ──

/// for-in right-hand sides of void/string/number/boolean each report
/// exactly one TS2407 with the right-hand type as the argument;
/// `any` and object types stay silent (for-inStatementsInvalid's shape).
#[test]
fn fcs46_for_in_rhs_2407_pin() {
    let (mut c, _) = check_unit(
        r#"
declare let obj: any;
declare let st: string;
declare let nm: number;
declare let bl: boolean;
declare let vd: void;
declare let ob: { a: number };
for (var a in obj) {}
for (var b in st) {}
for (var c in nm) {}
for (var d in bl) {}
for (var e in vd) {}
for (var f in ob) {}
"#,
        "/x/forInRhs.ts",
        options(),
    );
    let ds = diags(&mut c);
    let a2407 = args_of(&ds, 2407);
    assert_eq!(a2407.len(), 4, "expected 4 TS2407, got: {ds:?}");
    for arg in ["string", "number", "boolean", "void"] {
        assert!(
            a2407.iter().any(|a| a.first().map(String::as_str) == Some(arg)),
            "expected a TS2407 for {arg:?}, got: {ds:?}"
        );
    }
}

/// The boolean-RHS shape (privateNameInInExpression fires this via
/// `for (let f in #field in v as any)` — whose right side is the
/// boolean-typed `in` binary). The probe-verified direct boolean call
/// form pins the check-stage gate; the corpus line's exact spelling
/// additionally needs a for-in-RHS parse fix that stays queued (the
/// binary itself already types as boolean here — `const r: boolean =
/// #field in v as any` passes silently).
#[test]
fn fcs46_for_in_boolean_rhs_2407_pin() {
    let (mut c, _) = check_unit(
        r#"
declare function f(): boolean;
for (const k in f()) {}
"#,
        "/x/forInBool.ts",
        options(),
    );
    let ds = diags(&mut c);
    let a2407 = args_of(&ds, 2407);
    assert_eq!(a2407.len(), 1, "expected one TS2407, got: {ds:?}");
    assert!(
        a2407.iter().any(|a| a.first().map(String::as_str) == Some("boolean")),
        "expected the boolean arg, got: {ds:?}"
    );
}
