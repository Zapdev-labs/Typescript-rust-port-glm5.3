//! M6 S9 contextual-typing slice gate: pins that the REAL contextual engine
//! (contextual.rs, checker.go 29891-31523) drives the dispatcher's
//! initializer/argument/object-literal arms, the isContextSensitive scan
//! (checker.go 31405), and the contextual property lookup (checker.go
//! 31100) end-to-end through the check spine.

use tsc_ast::{Kind, NodeId, NodeStore, SourceFile, Symbol, SymbolId};
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
}

/// Depth-first collection of all nodes of `kind` (the binder's arena is
/// stable: NodeIds collected before the program move stay valid).
fn collect_kinds(sf: &SourceFile, node: NodeId, kind: Kind, out: &mut Vec<NodeId>) {
    if sf.node(node).kind == kind {
        out.push(node);
        return;
    }
    sf.node(node).for_each_child(&mut |child| {
        collect_kinds(sf, child, kind, out);
        false
    });
}

/// Parses + binds + checkers `src`; returns (checker, binder name→symbol
/// pairs, the requested node snapshots) so the pub-API pins can drive the
/// contextual family.
fn check_file_with(
    src: &str,
    wanted: &[Kind],
) -> (Checker, Vec<(String, SymbolId)>, Vec<Vec<NodeId>>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let mut snapshots = Vec::new();
    for kind in wanted {
        let mut out = Vec::new();
        collect_kinds(&sf, file, *kind, &mut out);
        snapshots.push(out);
    }
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
    (checker, symbols, snapshots)
}

fn declared_type(c: &mut Checker, symbols: &[(String, SymbolId)], name: &str) -> tsc_checker::TypeId {
    let symbol = symbols
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("symbol {name} not found"));
    c.get_type_of_symbol(symbol)
}

/// A function expression with an un-annotated parameter is context
/// sensitive (HasContextSensitiveParameters' first probe,
/// ast/utilities.go:4269); one with a fully annotated parameter list is not
/// (checker.go:31405).
#[test]
fn s9_is_context_sensitive_probes_parameter_annotations() {
    let (mut c, _symbols, fns) = check_file_with(
        "const a = function (x) { return x; };\nconst b = (y: number) => y;\n",
        &[Kind::FunctionExpression, Kind::ArrowFunction],
    );
    let (function_expression, arrow) = (fns[0][0], fns[1][0]);
    assert!(
        c.is_context_sensitive(function_expression),
        "function (x) with un-annotated parameter is context sensitive"
    );
    assert!(
        !c.is_context_sensitive(arrow),
        "(y: number) with annotated parameter is not context sensitive"
    );
}

/// The object-literal contextual arm (checker.go:30469): a property
/// assignment inside an object-literal initializer is contextually typed by
/// the variable's annotation property — end-to-end through
/// getApparentTypeOfContextualType (checker.go:31235) +
/// getTypeOfPropertyOfContextualType (checker.go:31100).
#[test]
fn s9_object_literal_element_contextual_type_is_the_annotation_property() {
    let (mut c, _symbols, props) = check_file_with(
        "const x: { a: number } = { a: 1 };\n",
        &[Kind::PropertyAssignment],
    );
    let property_assignment = props[0][0];
    let contextual = c
        .get_contextual_type_for_object_literal_element(
            property_assignment,
            tsc_checker::ContextFlags::NONE,
        )
        .expect("the contextual type of { a: 1 }'s `a` property");
    assert_eq!(contextual, c.number_type(), "contextually typed as number");
}

/// The call-argument contextual arm (checker.go:30321): the contextual type
/// of a call's first argument is the resolved signature's first parameter
/// type.
#[test]
fn s9_call_argument_contextual_type_is_the_parameter_type() {
    let (mut c, _symbols, calls) =
        check_file_with("function f(a: string): void {}\nf(\"x\");\n", &[Kind::CallExpression]);
    let call = calls[0][0];
    let contextual = c
        .get_contextual_type_for_argument_at_index(call, 0)
        .expect("the contextual type of f(\"x\")'s argument");
    assert_eq!(contextual, c.string_type());
}

/// The contextual property lookup over a union (checker.go:31100's
/// mapTypeEx arm): present on both constituents → union of both; present
/// on one → that constituent's property; missing everywhere → nil.
#[test]
fn s9_contextual_property_lookup_maps_union_constituents() {
    let (mut c, symbols, _snapshots) = check_file_with(
        "declare const u: { a: number } | { a: string; b: boolean };\n",
        &[],
    );
    let u = declared_type(&mut c, &symbols, "u");
    let a = c
        .get_type_of_property_of_contextual_type(u, "a")
        .expect("a present on both constituents");
    assert!(
        c.type_(a).flags.intersects(tsc_checker::TypeFlags::UNION),
        "union of both constituent property types"
    );
    assert_eq!(c.type_(a).types().len(), 2);
    let b = c
        .get_type_of_property_of_contextual_type(u, "b")
        .expect("b present on one constituent");
    assert_eq!(b, c.boolean_type());
    assert!(
        c.get_type_of_property_of_contextual_type(u, "missing").is_none(),
        "missing property is nil"
    );
}
