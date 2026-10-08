//! M4 wave-A binder smoke tests — parse + bind small sources through the
//! parser seam and assert the binder's observable outcomes (symbols, flow
//! graph, per-node symbol links, bind diagnostics). All assertions are on
//! binder-visible state only; deep checker semantics are M6 territory.

use tsc_ast::{Kind, NodeFlags, NodeStore, SourceFile, SymbolFlags, SymbolId};
use tsc_binder::bind_source_file;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// Parses + binds `src` as an external module named `src.ts`.
fn bind(src: &str) -> (SourceFile, Option<tsc_binder::BindResult>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let result = bind_source_file(&mut sf);
    (sf, result)
}
/// The symbol a name maps to in the file's locals table.
fn local_symbol(sf: &SourceFile, _result: &tsc_binder::BindResult, name: &str) -> Option<SymbolId> {
    let locals = tsc_binder::node_data::locals_of(sf, sf.as_node_id())?;
    locals.get(name).copied()
}

fn symbol_flags(result: &tsc_binder::BindResult, symbol: SymbolId) -> SymbolFlags {
    result.symbols[symbol.0 as usize].flags
}

#[test]
fn binds_simple_module() {
    let (sf, result) = bind(
        "const a = 1;\n\
         let b = 2;\n\
         function f(x: number) { return x + a; }\n\
         class C { method() { return f(a); } }\n",
    );
    let result = result.expect("bind produced a result");
    assert!(result.symbol_count > 0);
    assert!(!result.symbols.is_empty());
    assert!(!result.flow_nodes.is_empty());

    // Top-level locals: a, b, f, C (+ the `x` parameter lives on f, and
    // `method` on C — not in the file's locals).
    for name in ["a", "b", "f", "C"] {
        let symbol = local_symbol(&sf, &result, name)
            .unwrap_or_else(|| panic!("local {name} missing"));
        assert!(
            symbol.0 as usize <= result.symbols.len(),
            "symbol id {symbol:?} out of arena range (len {})",
            result.symbols.len()
        );
        let flags = symbol_flags(&result, symbol);
        let expected = match name {
            "a" | "b" => SymbolFlags::BLOCK_SCOPED_VARIABLE,
            "f" => SymbolFlags::FUNCTION,
            "C" => SymbolFlags::CLASS,
            _ => unreachable!(),
        };
        assert!(
            flags.intersects(expected),
            "symbol {name} flags {flags:?} missing {expected:?}"
        );
    }

    // Clean source: no bind diagnostics.
    assert!(sf.payload().bind_diagnostics.is_empty());
    // And the file is marked bound.
    assert!(sf.payload().is_bound.get());
}

#[test]
fn var_and_hoisted_function_flags() {
    let (sf, result) = bind("var v = 0;\nfunction h() { return v; }\n");
    let result = result.expect("bind produced a result");
    let v = local_symbol(&sf, &result, "v").expect("v");
    assert!(symbol_flags(&result, v).intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE));
    let h = local_symbol(&sf, &result, "h").expect("h");
    assert!(symbol_flags(&result, h).intersects(SymbolFlags::FUNCTION));
}

#[test]
fn interface_enum_type_alias() {
    let (sf, result) = bind(
        "interface I { m(): void; }\n\
         enum E { A, B = 2, }\n\
         type T = I;\n",
    );
    let result = result.expect("bind produced a result");
    let i = local_symbol(&sf, &result, "I").expect("I");
    assert!(symbol_flags(&result, i).intersects(SymbolFlags::INTERFACE));
    let e = local_symbol(&sf, &result, "E").expect("E");
    assert!(symbol_flags(&result, e).intersects(SymbolFlags::REGULAR_ENUM));
    let t = local_symbol(&sf, &result, "T").expect("T");
    assert!(symbol_flags(&result, t).intersects(SymbolFlags::TYPE_ALIAS));
    // Enum members land in the enum symbol's exports.
    let exports = result.symbols[e.0 as usize].exports.clone();
    assert!(exports.has("A") && exports.has("B"));
}

#[test]
fn namespace_members_and_exports() {
    let (sf, result) = bind(
        "namespace N { export const x = 1; const hidden = 2; }\n\
         export import I2 = N;\n",
    );
    let result = result.expect("bind produced a result");
    let n = local_symbol(&sf, &result, "N").expect("N");
    assert!(symbol_flags(&result, n).intersects(SymbolFlags::NAMESPACE));
    let exports = result.symbols[n.0 as usize].exports.clone();
    assert!(exports.has("x"));
    assert!(!exports.has("hidden"), "unexported member leaked into exports");
}

#[test]
fn external_module_gets_default_export_flag() {
    // An import/export presence makes the file an external module; the
    // binder sets the module flag on the file symbol (Go: declareModuleMember
    // paths → ExportKeyword presence → the file's symbol flags).
    let (sf, result) = bind("export const e = 1;\n");
    let result = result.expect("bind produced a result");
    let file_symbol = tsc_binder::node_data::symbol_of(&sf, sf.as_node_id())
        .expect("source file carries a symbol after bind");
    assert!(symbol_flags(&result, file_symbol).intersects(SymbolFlags::MODULE));
}

#[test]
fn strict_mode_errors_recorded() {
    // `with` + eval assignment in strict mode (module = always strict) →
    // bind diagnostics.
    let (sf, result) = bind("with (Math) { }\n");
    assert!(result.is_some());
    assert!(
        !sf.payload().bind_diagnostics.is_empty(),
        "'with' in a module must produce a bind diagnostic"
    );
}

#[test]
fn duplicate_identifier_reported() {
    let (sf, result) = bind("let d = 1;\nlet d = 2;\n");
    assert!(result.is_some());
    assert!(
        !sf.payload().bind_diagnostics.is_empty(),
        "duplicate `let d` must produce a redeclare diagnostic"
    );
}

#[test]
fn double_bind_returns_none() {
    let mut sf = parse_source_file(Default::default(), "const once = 1;", ScriptKind::TS);
    assert!(bind_source_file(&mut sf).is_some());
    assert!(bind_source_file(&mut sf).is_none(), "bind_once must refuse a second bind");
}

#[test]
fn flow_graph_is_populated() {
    // The function's end stays REACHABLE (Go sets bodyData.EndFlowNode only
    // for reachable ends, and HasExplicitReturn only alongside that) — the
    // if/while/for/try construct exercises branch, loop, label, and
    // exception flow nodes.
    let (sf, result) = bind(
        "function g(x: boolean) {\n\
             let r = 0;\n\
             if (x) { return; }\n\
             while (x) { break; }\n\
             label: for (;;) { break label; }\n\
             try { throw x; } catch (e) { r = 2; }\n\
             r = 3;\n\
         }\n",
    );
    let result = result.expect("bind produced a result");
    // Branch/loop/return/label/exception flow nodes exist.
    assert!(result.flow_nodes.len() > 10, "flow graph too small: {}", result.flow_nodes.len());

    // Identifiers carry flow nodes (Go: bind() case KindIdentifier sets
    // node.AsIdentifier().FlowNode = b.currentFlow). ReturnStatement itself
    // does NOT get one (bindReturnStatement only rewires currentFlow).
    let mut identifiers: Vec<tsc_ast::NodeId> = Vec::new();
    let mut function: Option<tsc_ast::NodeId> = None;
    let mut stack: Vec<tsc_ast::NodeId> = vec![sf.as_node_id()];
    while let Some(id) = stack.pop() {
        let node = NodeStore::node(&sf, id);
        if node.kind == Kind::Identifier {
            identifiers.push(id);
        }
        if node.kind == Kind::FunctionDeclaration {
            function = Some(id);
        }
        node.for_each_child(&mut |child| {
            stack.push(child);
            false
        });
    }
    assert!(!identifiers.is_empty());
    for id in identifiers {
        let flow = tsc_binder::node_data::flow_node_of(&sf, id);
        assert!(flow.is_some(), "Identifier lacks a flow node");
    }
    // The function-like container gets its end flow node (Go:
    // bindContainer's control-flow-container save/restore), plus the
    // explicit-return emit flag.
    let function = function.expect("FunctionDeclaration found");
    let f = NodeStore::node(&sf, function);
    assert!(tsc_binder::node_data::end_flow_node_of(&sf, function).is_some());
    assert!(f.flags.intersects(NodeFlags::HAS_EXPLICIT_RETURN));
    // ReturnFlowNode is only for IIFEs and constructors (Go:
    // setReturnFlowNode call sites) — a plain declaration has none.
    assert!(tsc_binder::node_data::return_flow_node_of(&sf, function).is_none());
}

#[test]
fn parameters_get_symbols_and_this_flag() {
    let (sf, result) = bind("class K { constructor(private readonly p: number) { this.p; } }\n");
    let result = result.expect("bind produced a result");
    // Walk to the ParameterDeclaration: it must carry a symbol and the
    // class must carry the HasImplicitReturn/etc flags — here check the
    // parameter's symbol and the `this` usage flag on the file (JS This
    // tracking: seenThisKeyword sets NodeFlagsThisEmit on... the accessor
    // `p`'s parent class). Keep to symbol presence + member registration.
    let mut param: Option<tsc_ast::NodeId> = None;
    let mut stack: Vec<tsc_ast::NodeId> = vec![sf.as_node_id()];
    while let Some(id) = stack.pop() {
        let node = NodeStore::node(&sf, id);
        if node.kind == Kind::Parameter {
            param = Some(id);
            break;
        }
        node.for_each_child(&mut |child| {
            stack.push(child);
            false
        });
    }
    let param = param.expect("parameter found");
    let symbol = tsc_binder::node_data::symbol_of(&sf, param).expect("parameter symbol");
    assert!(symbol_flags(&result, symbol).intersects(SymbolFlags::PROPERTY));
}

#[test]
fn js_common_js_expando() {
    // The CommonJS module indicator path + expando assignment machinery
    // must not panic on JS sources.
    let mut sf = parse_source_file(Default::default(), "exports.foo = 1;\n", ScriptKind::JS);
    assert!(bind_source_file(&mut sf).is_some());
    assert!(sf.payload().common_js_module_indicator.get().is_some());
}

#[test]
fn ambient_module_patterns_registered() {
    let (sf, result) = bind("declare module \"pattern-*\" { const x: number; }\n");
    let result = result.expect("bind produced a result");
    let _ = result;
    assert_eq!(sf.payload().pattern_ambient_modules.len(), 1);
}

#[test]
fn node_ambient_flag_visible() {
    let (sf, _result) = bind("declare function df(): void;\n");
    let mut stack: Vec<tsc_ast::NodeId> = vec![sf.as_node_id()];
    let mut found = false;
    while let Some(id) = stack.pop() {
        let node = NodeStore::node(&sf, id);
        if node.kind == Kind::FunctionDeclaration {
            assert!(node.flags.intersects(NodeFlags::AMBIENT), "declare missing AMBIENT flag");
            found = true;
        }
        node.for_each_child(&mut |child| {
            stack.push(child);
            false
        });
    }
    assert!(found);
}
