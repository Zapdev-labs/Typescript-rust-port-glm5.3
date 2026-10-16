//! FCS30 pins (full-corpus check sweep, tranche 30): the rust-missing
//! TS2818 "Duplicate identifier '{0}'. Compiler reserves name '{1}' when
//! emitting 'super' references in static initializers" family — 114
//! instances (38 per pre-ES2022 config) across the
//! superInStaticMembers1.ts record before the tranche, 0 after.
//!
//! ROOTS fixed this tranche ("FCS tranche 30" PORT notes in-tree):
//!
//! 1. The GetEnclosingBlockScopeContainer walk at checker.go:8155 —
//!    `super.x`/`super[x]` in a static initializer (target <= ES2021)
//!    marks every enclosing block-scope container with
//!    NodeCheckFlagsContainsSuperPropertyInStaticInitializer. The walk
//!    was a TODO stub in expressions.rs's check_super_property; the
//!    checkReflectCollision consumer (statements.rs) was already ported
//!    but never fired.
//!
//! 2. checkCollisionsForDeclarationName for FunctionExpression names
//!    (checker.go:10346) — a TODO stub in expressions.rs's
//!    check_function_expression_or_object_literal_method (the
//!    inContainingFuncExprStaticField/Block 6 instances).
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23+ idiom) ──

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
}

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
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

/// strict options at the given target.
fn options(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        strict: Tristate::TSTrue,
        target,
        ..CompilerOptions::default()
    }
}

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// corpus seam reads `semantic_diagnostics()`).
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    let _ = file;
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn codes(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, args)| args.clone())
        .collect()
}

// ── Pin 1: the keystone — a static block's `Reflect` binding collides ──
//
// `super.w()` inside a class static block marks every enclosing block
// scope container (checker.go:8155); the static block itself is one, so
// the block-scoped `var Reflect` declared in it reports TS2818 at
// <= ES2021 (superInStaticMembers1.ts's static-block shape).

const STATIC_BLOCK_COLLISION: &str = "declare class B { static w(): number; }\nclass C extends B {\n    static {\n        var Reflect;\n        super.w();\n    }\n}\n";

#[test]
fn static_block_reflect_collision_2818_pin() {
    let (mut c, f) = check_with(STATIC_BLOCK_COLLISION, options(ScriptTarget::ES2021));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2818),
        vec![vec!["Reflect".to_string(), "Reflect".to_string()]],
        "the static-block var Reflect reports exactly one 2818: {ds:?}"
    );
}

// ── Pin 2: the ES2022 gate — no 2818 at ES2022+ ──
//
// recordPotentialCollisionWithReflectInGeneratedCode only records the
// deferred diagnostic when languageVersion <= ES2021 (checker.go:10806).

#[test]
fn es2022_no_reflect_collision_pin() {
    let (mut c, f) = check_with(STATIC_BLOCK_COLLISION, options(ScriptTarget::ES2022));
    let ds = diags(&mut c, f);
    assert!(
        codes(&ds, 2818).is_empty(),
        "ES2022 emits super directly; no Reflect reservation: {ds:?}"
    );
}

// ── Pin 3: the static property initializer shape (arrow-wrapped) ──
//
// The super call sits in an IIFE inside a static property initializer;
// the container walk flags the PropertyDeclaration member, the enclosing
// Block scopes, and (via the ClassExpression arm) class members — the
// binding-pattern `var { Reflect }` in the arrow body collides.

const PROPERTY_INITIALIZER_COLLISION: &str = "declare class B { static w(): number; }\nclass C extends B {\n    static _ = (() => {\n        var { Reflect } = { Reflect: null };\n        super.w();\n    })();\n}\n";

#[test]
fn property_initializer_reflect_collision_2818_pin() {
    let (mut c, f) = check_with(PROPERTY_INITIALIZER_COLLISION, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2818),
        vec![vec!["Reflect".to_string(), "Reflect".to_string()]],
        "the arrow-body binding-pattern Reflect reports one 2818: {ds:?}"
    );
}

// ── Pin 4: the ClassExpression member arm ──
//
// `(class Reflect { ... })` never reports (its name does not contribute
// to containers), but a class EXPRESSION whose member contains a
// super-in-static-initializer marks the member itself (IsBlockScope
// lists PropertyDeclaration) — the ClassExpression arm of
// checkReflectCollision reads member flags (checker.go:10816).

const CLASS_EXPR_MEMBER_COLLISION: &str = "declare class B { static w(): number; }\n(class Reflect {\n    static {\n        class C extends B {\n            static _ = super.w();\n        }\n    }\n});\n";

#[test]
fn class_expression_member_flag_walk_pin() {
    // The corpus twin shape (inContainingClassExprStaticField.ts):
    // a class expression NAMED Reflect whose ClassStaticBlockDeclaration
    // member contains a super-in-static-initializer — the walk flags the
    // static-block member (IsBlockScope lists ClassStaticBlockDeclaration),
    // and checkReflectCollision's ClassExpression arm reads member flags
    // (checker.go:10816) — exactly one 2818 for the class expression name.
    let (mut c, f) = check_with(CLASS_EXPR_MEMBER_COLLISION, options(ScriptTarget::ES2021));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2818),
        vec![vec!["Reflect".to_string(), "Reflect".to_string()]],
        "the class expression name collides via its flagged member: {ds:?}"
    );
}

// ── Pin 6: the FunctionExpression arm (the root-2 corpus shape) ──
//
// The corpus twin shape (inContainingFuncExprStaticField.ts):
// checkCollisionsForDeclarationName for the function expression NAME
// (checker.go:10346, the tranche's second root) +
// checkReflectCollision's FunctionExpression arm reading the node's own
// flag (checker.go:10824) — the walk skips the function body Block and
// flags the FunctionExpression directly (IsBlockScope excludes
// function-body Blocks).

const FUNC_EXPR_COLLISION: &str = "declare class B { static w(): number; }\n(function Reflect() {\n    class C extends B {\n        static _ = super.w();\n    }\n});\n";

#[test]
fn function_expression_reflect_collision_2818_pin() {
    let (mut c, f) = check_with(FUNC_EXPR_COLLISION, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2818),
        vec![vec!["Reflect".to_string(), "Reflect".to_string()]],
        "the function expression name collides via its own container flag: {ds:?}"
    );
}

// ── Pin 5: the member-name escape hatch — a PROPERTY named Reflect ──
//
// needCollisionCheckForIdentifier (checker.go:10730) returns false for
// PropertyDeclaration — member access is always qualified, so a static
// member named `Reflect` never reports 2818.

const PROPERTY_NAMED_REFLECT: &str = "declare class B { static w(): number; }\nclass C extends B {\n    static Reflect = 1;\n    static {\n        super.w();\n    }\n}\n";

#[test]
fn property_named_reflect_no_2818_pin() {
    let (mut c, f) = check_with(PROPERTY_NAMED_REFLECT, options(ScriptTarget::ES2021));
    let ds = diags(&mut c, f);
    assert!(
        codes(&ds, 2818).is_empty(),
        "member names are always qualified; no Reflect collision: {ds:?}"
    );
}
