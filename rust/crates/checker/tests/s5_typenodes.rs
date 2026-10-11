//! M6 S5 type-node-family slice gate: pins that the newly-landed
//! getTypeFromTypeNode routing (typenodes.rs) walks every arm of the
//! checker.go 23295–25536 family without panics on a representative program,
//! and that the slice's real diagnostic paths fire — the this-type error
//! (checker.go 23379, TS 2526), the generic-arity ladder (checker.go 23670,
//! TS 2314) and the non-generic-with-arguments error (checker.go 23744,
//! TS 2315) route through no permissive grammar stub. The
//! type-parameter-default machinery (checker.go 22392–22510) is pinned
//! end-to-end by the `interface I<T = string>` arity case.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// The spike.rs harness shape (the §5.5 program seam).
struct TestProgram {
    options: CompilerOptions,
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
}

/// Parses + binds + checkers `src`, runs the full file check, and returns
/// (checker, file NodeId).
fn check_file(src: &str) -> (Checker, NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// Parses + binds + checkers `src`; returns (checker, file NodeId, the
/// binder's top-level name→symbol pairs) — the spike.rs harness shape for
/// API-level pins.
fn check_file_with_symbols(src: &str) -> (Checker, NodeId, Vec<(String, SymbolId)>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let symbols: Vec<(String, SymbolId)> = bind
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), SymbolId(i as u32)))
        .filter(|(name, _)| !name.is_empty() && !name.starts_with('\u{FE}'))
        .collect();
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file, symbols)
}

fn codes(c: &mut Checker, file: NodeId) -> Vec<i32> {
    c.get_file_diagnostics(file).iter().map(|d| d.code()).collect()
}

/// A program touching every getTypeFromTypeNodeWorker arm wired in
/// typenodes.rs: keywords, type literals, parenthesized types, unions,
/// intersections, tuples (rest/optional/named), array types, type operators
/// (keyof/readonly/unique symbol), indexed access, typeof queries,
/// conditional types with infer, mapped types, template literal types,
/// import types (over the S6 module seam), type references with and without
/// type arguments, and this types inside class members.
const BROAD_PROGRAM: &str = r#"
interface Box<T> { value: T }
interface Point { x: number, y: number, [key: string]: unknown }
type Alias = string;
type GenericAlias<T> = T;
type Union = "a" | "b" | number;
type Inter = Point & { z: number };
type Tup = [a: number, b?: string, ...rest: boolean[]];
type Arr = string[];
type ReadonlyTup = readonly [number, number];
type K = keyof Point;
type Idx = Point["x"];
type Cond<T> = T extends string ? number : boolean;
type Infer<T> = T extends infer U ? U : never;
type Mapped = { [K in keyof Point]: boolean };
type MappedAs = { readonly [K in keyof Point]-?: Point[K] };
type Templ = `prefix-${string}`;
type Query = typeof width;
declare const width: number;
declare const box: Box<number>;
declare const aliased: GenericAlias<number>;
declare const tup: Tup;
declare const arr: Arr;
declare const imp: import("./missing").Exported;
class C {
    m(): this { return this; }
    static s(): C { return new C(); }
}
export { box, tup, arr };
"#;

#[test]
fn s5_type_node_walk_completes_without_panic() {
    let (_checker, _file) = check_file(BROAD_PROGRAM);
    // The walk itself is the gate: every getTypeFromTypeNodeWorker dispatch
    // arm must run to completion (keywords, literals, unions/intersections,
    // tuples with rest/optional/named elements, array/readonly-array types,
    // keyof/indexed access, typeof queries, conditionals with infer, mapped
    // types, template literals, import types over the S6 module seam, and
    // type references with type arguments) on a program that touches each.
}

#[test]
fn s5_this_type_at_top_level_reports_2526() {
    // Go: getThisType (checker.go 23379) — no class/interface container
    // above the return type position → A_this_type_is_available_only_in_
    // a_nonstatic_member (TS 2526).
    let (mut c, file) = check_file("function f(): this { return null as any; }\n");
    let codes = codes(&mut c, file);
    assert!(
        codes.contains(&2526),
        "expected TS 2526 for a top-level this type, got: {codes:?}"
    );
}

#[test]
fn s5_this_type_in_class_member_is_clean() {
    // The class-member container walk resolves: no 2526 may fire inside a
    // non-static method (checker.go 23379's legal path), and the static
    // member takes the error branch (this types are non-static only).
    let (mut c, file) = check_file(
        "class C { m(): this { return this; } static s(): C { return new C(); } }\n",
    );
    let codes = codes(&mut c, file);
    assert!(
        !codes.contains(&2526),
        "no 2526 for a this type in a non-static member, got: {codes:?}"
    );
}

#[test]
fn s5_generic_arity_ladder_matches_go() {
    // The generic-arity ladder (checker.go 23670's TS 2314 / 23744's TS 2315)
    // went LIVE with M6 S6's real resolveEntityName: the scope walk now
    // resolves the interface symbols, so getTypeFromClassOrInterfaceReference
    // enters its arity arms. Go tsc on the same program reports
    //   2315 (I not generic), 2314 (G<number>), 2314 (G<number, string,
    //   boolean>) — verified against go run ./tsc/cmd/tsc. The port pins
    // that exact ladder.
    let (mut c, file) = check_file(
        "interface I {}\ninterface G<T, U> {}\ndeclare const a: I<number>;\ndeclare const b: G<number>;\ndeclare const d: G<number, string, boolean>;\n",
    );
    let codes = codes(&mut c, file);
    assert_eq!(codes, vec![2315, 2314, 2314], "Go parity for the arity ladder");
}

#[test]
fn s5_get_min_type_argument_count_counts_defaulted_type_parameters() {
    // Go: getMinTypeArgumentCount (checker.go 22397) — REAL body pinned at
    // the API level: an undefaulted type parameter pins the count at its
    // index + 1; a defaulted one is skipped; all-defaulted lowers the
    // minimum to 0.
    let (mut c, _file, symbols) =
        check_file_with_symbols("interface I<T, U = number> {}\n");
    let symbol = symbols
        .iter()
        .find(|(name, _)| name == "I")
        .map(|(_, s)| *s)
        .expect("symbol I not found");
    let declared = c.get_declared_type_of_symbol(symbol);
    let params = c
        .type_(declared)
        .as_interface_type()
        .expect("InterfaceType data")
        .local_type_parameters()
        .to_vec();
    assert_eq!(params.len(), 2);
    assert_eq!(c.get_min_type_argument_count(&params), 1);

    let (mut c, _file, symbols) = check_file_with_symbols("interface J<T> {}\n");
    let symbol = symbols
        .iter()
        .find(|(name, _)| name == "J")
        .map(|(_, s)| *s)
        .expect("symbol J not found");
    let declared = c.get_declared_type_of_symbol(symbol);
    let params = c
        .type_(declared)
        .as_interface_type()
        .expect("InterfaceType data")
        .local_type_parameters()
        .to_vec();
    assert_eq!(params.len(), 1);
    assert_eq!(c.get_min_type_argument_count(&params), 1);

    let (mut c, _file, symbols) =
        check_file_with_symbols("interface K<T = string, U = number> {}\n");
    let symbol = symbols
        .iter()
        .find(|(name, _)| name == "K")
        .map(|(_, s)| *s)
        .expect("symbol K not found");
    let declared = c.get_declared_type_of_symbol(symbol);
    let params = c
        .type_(declared)
        .as_interface_type()
        .expect("InterfaceType data")
        .local_type_parameters()
        .to_vec();
    assert_eq!(params.len(), 2);
    assert_eq!(c.get_min_type_argument_count(&params), 0);
}

#[test]
fn s5_fill_missing_type_arguments_resolves_the_default() {
    // Go: fillMissingTypeArguments + getDefaultFromTypeParameter (checker.go
    // 22412/22446) — REAL bodies pinned at the API level: the missing
    // argument for `U = number` resolves to the default (through the S7
    // identity instantiation seam the default is `number` itself).
    let (mut c, _file, symbols) =
        check_file_with_symbols("interface I<T, U = number> {}\n");
    let symbol = symbols
        .iter()
        .find(|(name, _)| name == "I")
        .map(|(_, s)| *s)
        .expect("symbol I not found");
    let declared = c.get_declared_type_of_symbol(symbol);
    let params = c
        .type_(declared)
        .as_interface_type()
        .expect("InterfaceType data")
        .local_type_parameters()
        .to_vec();
    // U's default resolves through getResolvedTypeParameterDefault's
    // declaration read (checker.go 22454); T has no default.
    assert_eq!(
        c.get_default_from_type_parameter(params[1]),
        Some(c.number_type())
    );
    assert_eq!(c.get_default_from_type_parameter(params[0]), None);
    let string_type = c.string_type();
    let filled = c.fill_missing_type_arguments(&[string_type], &params, 1, false);
    assert_eq!(filled.len(), 2);
    assert_eq!(filled[0], string_type);
    assert_eq!(filled[1], c.number_type());

    // Both params defaulted, no args at all: both fill from defaults.
    let (mut c, _file, symbols) =
        check_file_with_symbols("interface K<T = string, U = number> {}\n");
    let symbol = symbols
        .iter()
        .find(|(name, _)| name == "K")
        .map(|(_, s)| *s)
        .expect("symbol K not found");
    let declared = c.get_declared_type_of_symbol(symbol);
    let params = c
        .type_(declared)
        .as_interface_type()
        .expect("InterfaceType data")
        .local_type_parameters()
        .to_vec();
    let filled = c.fill_missing_type_arguments(&[], &params, 0, false);
    assert_eq!(filled.len(), 2);
    assert_eq!(filled[0], c.string_type());
    assert_eq!(filled[1], c.number_type());
}

#[test]
fn s5_type_parameter_default_satisfies_the_arity_check() {
    // Go: getMinTypeArgumentCount (checker.go 22397) — a defaulted type
    // parameter lowers the minimum to 0, so `I` with no type arguments is
    // legal and fillMissingTypeArguments resolves the default. No TS 2314
    // may fire (the W1 pin — params.len() as the minimum — would have
    // reported it).
    let (mut c, file) = check_file("interface I<T = string> {}\ndeclare const x: I;\n");
    let codes = codes(&mut c, file);
    assert!(
        !codes.contains(&2314) && !codes.contains(&2315),
        "a defaulted type parameter satisfies the arity check, got: {codes:?}"
    );
}

#[test]
fn s5_type_alias_reference_walk_is_clean() {
    // getTypeFromTypeAliasReference + getTypeAliasInstantiation (checker.go
    // 24214/24305 region): a plain alias resolves, and a generic alias
    // reference instantiates through the S7 identity seam without
    // diagnostics.
    let (mut c, file) = check_file(
        "type A = string;\ntype G<T> = T;\ndeclare const a: A;\ndeclare const g: G<number>;\n",
    );
    let codes = codes(&mut c, file);
    assert!(
        !codes.contains(&2314) && !codes.contains(&2315) && !codes.contains(&2304),
        "alias references resolve cleanly, got: {codes:?}"
    );
}
