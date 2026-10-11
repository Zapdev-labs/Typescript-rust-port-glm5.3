//! M6 S10 index/facts slice gate: pins that the REAL keyof family
//! (index.rs, checker.go 27214-27459), the deferred indexed-access
//! constructor + resolver (checker.go 27461-27797), the facts lattice
//! (facts.rs, checker.go 31523-31765 + 29604-29636), and the public
//! GetIndexTypeOfType tail (checker.go 32712 region) drive the check spine
//! end-to-end.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program, TypeFlags, TypeId};
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

/// Parses + binds + checkers `src`; returns (checker, binder name→symbol
/// pairs) so the declared-type pins can drive the index family.
fn check_file(src: &str) -> (Checker, Vec<(String, SymbolId)>) {
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
    (checker, symbols)
}

fn declared_type(c: &mut Checker, symbols: &[(String, SymbolId)], name: &str) -> TypeId {
    let symbol = symbols
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("symbol {name} not found"));
    c.get_type_of_symbol(symbol)
}

/// The keyof constructor (checker.go:27214 → getLiteralTypeFromProperties,
/// checker.go:27249): `keyof { a: 1; b: 2 }` is the two-literal union
/// "a" | "b" (the propertiesTypes cache is keyed by the object type).
#[test]
fn s10_keyof_object_literal_is_the_property_name_union() {
    let (mut c, symbols) = check_file("type K = keyof { a: 1; b: 2 };\ndeclare const k: K;\n");
    let k = declared_type(&mut c, &symbols, "k");
    assert!(
        c.type_(k).flags.intersects(TypeFlags::UNION),
        "keyof of the two-property literal is a union type"
    );
    assert_eq!(c.type_(k).types().len(), 2, "the union is a | b");
    for &constituent in c.type_(k).types() {
        assert!(
            c.type_(constituent)
                .flags
                .intersects(TypeFlags::STRING_LITERAL),
            "each constituent is a string literal type"
        );
    }
}

/// The keyof constructor over an index-signature type (checker.go:27264's
/// index-info arm): `keyof { [k: string]: number }` is `string`.
#[test]
fn s10_keyof_string_index_signature_is_string() {
    let (mut c, symbols) = check_file("type K = keyof { [k: string]: number };\ndeclare const k: K;\n");
    let k = declared_type(&mut c, &symbols, "k");
    assert_eq!(
        k,
        c.string_type(),
        "keyof of the string index signature is string"
    );
}

/// The deferred indexed-access constructor + resolver
/// (getIndexedAccessTypeOrUndefined → getPropertyTypeForIndexType,
/// checker.go:27461+27535): `{ a: number; b: string }["a"]` is `number`.
#[test]
fn s10_indexed_access_on_concrete_object_is_the_property_type() {
    let (mut c, symbols) =
        check_file("declare const w: { a: number; b: string }[\"a\"];\n");
    let w = declared_type(&mut c, &symbols, "w");
    assert_eq!(
        w,
        c.number_type(),
        "the indexed access on the object literal resolves to number"
    );
}

/// The public tail (checker.go:32712 region): GetIndexTypeOfType over an
/// index-signature type + the string key type is the index value type.
#[test]
fn s10_get_index_type_of_type_reads_the_index_signature_value() {
    let (mut c, symbols) = check_file("declare const m: { [k: string]: number };\n");
    let m = declared_type(&mut c, &symbols, "m");
    let string = c.string_type();
    let value = c
        .get_index_type_of_type(m, string)
        .expect("the string index signature applies");
    assert_eq!(value, c.number_type(), "the value type is number");
}

/// The facts lattice (checker.go:31600 region + removeMissingOrUndefinedType,
/// checker.go:29616): RemoveMissingOrUndefinedType over `string | undefined`
/// strips the undefined constituent (the NEUndefined facts filter).
#[test]
fn s10_remove_missing_or_undefined_strips_undefined_constituent() {
    let (mut c, symbols) = check_file("declare const u: string | undefined;\n");
    let u = declared_type(&mut c, &symbols, "u");
    assert!(
        c.type_(u).flags.intersects(TypeFlags::UNION),
        "the declared type is the two-constituent union"
    );
    let removed = c.remove_missing_or_undefined_type(u);
    assert!(
        !c.type_(removed)
            .flags
            .intersects(TypeFlags::UNION | TypeFlags::UNDEFINED),
        "NEUndefined narrows to the non-undefined constituent alone"
    );
    assert_eq!(removed, c.string_type());
}

/// The definitely-falsy extractors (checker.go:29624-29636): the `&&`
/// operator's result composes extractDefinitelyFalsyTypes(leftType) with
/// the right type. `string` carries the Truthy fact (checker.go:436's
/// StringStrictFacts), so `s && 1` extracts string's definitely-falsy part
/// (the empty-string literal, checker.go:29634) and unions it with `1`; a
/// definitely-truthy object operand's falsy part is `never` (the default
/// arm), so `o && 1` reduces to the right operand's type alone.
#[test]
fn s10_boolean_and_composes_extracted_falsy_part() {
    let (mut c, symbols) = check_file("declare const s: string;\nconst r = s && 1;\n");
    let r = declared_type(&mut c, &symbols, "r");
    assert!(
        c.type_(r).flags.intersects(TypeFlags::UNION),
        "s && 1 is the union of the extracted falsy part of string and 1"
    );
    let constituents = c.type_(r).types().to_vec();
    assert_eq!(constituents.len(), 2, "two constituents");
    assert!(
        constituents.iter().any(|&t| c
            .type_(t)
            .flags
            .intersects(TypeFlags::STRING_LITERAL)),
        "the definitely-falsy part of string is the empty-string literal"
    );
    assert!(
        constituents.iter().any(|&t| c
            .type_(t)
            .flags
            .intersects(TypeFlags::NUMBER_LITERAL)),
        "the right operand is the number literal"
    );

    let (mut c, symbols) = check_file("declare const o: { a: number };\nconst r = o && 1;\n");
    let r = declared_type(&mut c, &symbols, "r");
    assert!(
        c.type_(r).flags.intersects(TypeFlags::NUMBER_LITERAL),
        "the definitely-falsy part of an object type is never, so o && 1 reduces to 1"
    );
}
