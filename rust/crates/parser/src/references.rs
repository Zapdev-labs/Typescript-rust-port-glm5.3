//! Port of `tsc/internal/parser/references.go` plus the
//! `ast.ForEachDynamicImportOrRequireCall` walk it depends on (that walk is a
//! local PORT here because the M2 `tsc-ast` utilities surface does not carry
//! it; the bodies mirror Go exactly).
//!
//! Go mutates the `*SourceFile` struct as it walks (`SetImportsOfSourceFile`,
//! `file.ModuleAugmentations = append(...)`); the arena keeps that state in
//! the slot-0 `SourceFileNodeData`, so the port batches the walk's appends
//! into a [`ReferenceCollector`] (read-only traversal) and applies them in one
//! mutable pass afterwards. The batching preserves Go's append order exactly
//! (statement walk first, then the dynamic-import/require text scan).

use tsc_ast::{
    has_syntactic_modifier, is_call_expression, is_identifier, is_in_js_file, is_meta_property,
    is_module_declaration, is_string_literal, node_text, Kind, ModifierFlags, NodeFlags, NodeId,
    NodeStore,
};
use tsc_core::nodemodules::{EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES, UNPREFIXED_NODE_CORE_MODULES};
use tsc_core::text::TextPos;
use tsc_core::tristate::Tristate;
use tsc_tspath::is_external_module_name_relative;

use crate::parser::{
    get_external_module_name, is_ambient_module, is_any_import_or_re_export,
    is_import_phase_meta_property, is_string_literal_like,
};

/// Go: `func collectExternalModuleReferences(file *ast.SourceFile)`.
///
/// PORT: takes the store + the slot-0 SourceFile node instead of `*SourceFile`
/// (the arena owns the file data).
pub(crate) fn collect_external_module_references(s: &mut dyn NodeStore, file: NodeId) {
    // Seed the collector state and snapshot the file-level walk parameters.
    let (
        statements,
        is_declaration_file,
        is_external_module,
        possibly_dynamic,
        is_js_file,
        uses_uri,
    ) = {
        let s: &dyn NodeStore = s;
        let d = s.node(file).as_source_file().expect("SourceFile data");
        (
            d.statements
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default(),
            d.is_declaration_file,
            d.external_module_indicator.get().is_some(),
            s.node(file)
                .flags
                .intersects(NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT),
            is_in_js_file(s.node(file)),
            d.uses_uri_style_node_core_modules,
        )
    };

    let mut c = ReferenceCollector {
        imports: Vec::new(),
        module_augmentations: Vec::new(),
        ambient_module_names: Vec::new(),
        uses_uri_style_node_core_modules: uses_uri,
    };
    {
        let s: &dyn NodeStore = s;
        for &statement in &statements {
            collect_module_references(
                s,
                is_external_module,
                is_declaration_file,
                statement,
                false, /*inAmbientModule*/
                &mut c,
            );
        }

        if possibly_dynamic || is_js_file {
            for_each_dynamic_import_or_require_call(
                s,
                file,
                true, /*includeTypeSpaceImports*/
                true, /*requireStringLiteralLikeArgument*/
                &mut |_node, module_specifier| {
                    c.imports.push(module_specifier);
                    false
                },
            );
        }
    }

    // Apply the batched appends (Go: SetImportsOfSourceFile / direct field
    // appends interleaved with the walk).
    let d = s
        .node_mut(file)
        .as_source_file_mut()
        .expect("SourceFile data");
    d.imports.extend(c.imports);
    d.module_augmentations.extend(c.module_augmentations);
    d.ambient_module_names.extend(c.ambient_module_names);
    d.uses_uri_style_node_core_modules = c.uses_uri_style_node_core_modules;
}

/// The mutable `SourceFile` state Go's walk accumulates, batched (PORT).
struct ReferenceCollector {
    imports: Vec<NodeId>,
    module_augmentations: Vec<NodeId>,
    ambient_module_names: Vec<String>,
    uses_uri_style_node_core_modules: Tristate,
}

/// Go: `func collectModuleReferences(file *ast.SourceFile, node *ast.Statement,
/// inAmbientModule bool)`.
///
/// PORT: `file.IsDeclarationFile` / `IsExternalModule(file)` are loop-invariant,
/// so they ride along as parameters instead of being re-read from the file
/// data on every recursion. Go's `file` parameter is likewise dropped here:
/// every read of it was hoisted into the entry snapshot, and the mutable
/// file state lives in the collector, batched back onto the arena at exit.
fn collect_module_references(
    s: &dyn NodeStore,
    is_external_module: bool,
    is_declaration_file: bool,
    node: NodeId,
    in_ambient_module: bool,
    c: &mut ReferenceCollector,
) {
    let n = s.node(node);
    if is_any_import_or_re_export(n) {
        let module_name_expr = get_external_module_name(s, node);
        // TypeScript 1.0 spec (April 2014): 12.1.6
        // An ExternalImportDeclaration in an AmbientExternalModuleDeclaration may reference other external modules
        // only through top - level external module names. Relative external module names are not permitted.
        if let Some(module_name_expr) = module_name_expr {
            if is_string_literal(s.node(module_name_expr)) {
                let module_name = node_text(s, module_name_expr);
                if !module_name.is_empty()
                    && (!in_ambient_module || !is_external_module_name_relative(module_name))
                {
                    c.imports.push(module_name_expr);
                    // !!! removed `&& p.currentNodeModulesDepth == 0`
                    if !c.uses_uri_style_node_core_modules.is_true() && !is_declaration_file {
                        if module_name.starts_with("node:")
                            && !EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES.contains(&module_name)
                        {
                            // Presence of `node:` prefix takes precedence over unprefixed node core modules
                            c.uses_uri_style_node_core_modules = Tristate::TSTrue;
                        } else if c.uses_uri_style_node_core_modules.is_unknown()
                            && UNPREFIXED_NODE_CORE_MODULES.contains(&module_name)
                        {
                            // Avoid `unprefixedNodeCoreModules.has` for every import
                            c.uses_uri_style_node_core_modules = Tristate::TSFalse;
                        }
                    }
                }
            }
        }
        return;
    }
    if is_module_declaration(n)
        && is_ambient_module(s, node)
        && (in_ambient_module
            || has_syntactic_modifier(n, ModifierFlags::AMBIENT)
            || is_declaration_file)
    {
        let d = n.as_module_declaration().expect("ModuleDeclaration data");
        let name_text = node_text(s, d.name).to_string();
        // Ambient module declarations can be interpreted as augmentations for some existing external modules.
        // This will happen in two cases:
        // - if current file is external module then module augmentation is a ambient module declaration defined in the top level scope
        // - if current file is not external module then module augmentation is an ambient module declaration with non-relative module name
        //   immediately nested in top level ambient module declaration .
        if is_external_module
            || (in_ambient_module && !is_external_module_name_relative(&name_text))
        {
            c.module_augmentations.push(d.name);
        } else if !in_ambient_module {
            c.ambient_module_names.push(name_text);
            // An AmbientExternalModuleDeclaration declares an external module.
            // This type of declaration is permitted only in the global module.
            // The StringLiteral must specify a top - level external module name.
            // Relative external module names are not permitted
            // NOTE: body of ambient module is always a module block, if it exists
            if let Some(body) = d.body {
                let body_node = s.node(body);
                if body_node.kind == Kind::ModuleBlock {
                    let block = body_node.as_module_block().expect("ModuleBlock data");
                    if let Some(list) = &block.statements {
                        for &statement in &list.nodes {
                            collect_module_references(
                                s,
                                is_external_module,
                                is_declaration_file,
                                statement,
                                true, /*inAmbientModule*/
                                c,
                            );
                        }
                    }
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ast.ForEachDynamicImportOrRequireCall and its dependencies (local PORT of
// tsc/internal/ast/utilities.go — the M2 ast-utilities surface does not carry
// the text-scan walk yet).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func findImportOrRequire(text string, start int) (index int, size int)`.
fn find_import_or_require(text: &str, start: i32) -> (i32, usize) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut index = start.max(0) as usize;
    while index < n {
        // strings.IndexAny(text[index:], "ir")
        let Some(next) = bytes[index..].iter().position(|&b| b == b'i' || b == b'r') else {
            break;
        };
        index += next;

        let (size, expected): (usize, &[u8]) = if bytes[index] == b'i' {
            (6, b"import")
        } else {
            (7, b"require")
        };
        if index + size <= n && bytes[index..index + size] == *expected {
            return (index as i32, size);
        }
        index += 1;
    }

    (-1, 0)
}

/// Go: `func nodeContainsPosition(node *ast.Node, position int) bool`.
fn node_contains_position(s: &dyn NodeStore, node: NodeId, position: TextPos) -> bool {
    let n = s.node(node);
    n.kind >= Kind::FirstNode
        && n.pos() <= position
        && (position < n.end() || (position == n.end() && n.kind == Kind::EndOfFile))
}

/// Go: `func GetNodeAtPosition(file *ast.SourceFile, position int, includeJSDoc bool) *ast.Node`.
fn get_node_at_position(
    s: &dyn NodeStore,
    file: NodeId,
    position: TextPos,
    include_jsdoc: bool,
) -> NodeId {
    let mut current = file;
    loop {
        let mut child: Option<NodeId> = None;
        if include_jsdoc {
            // Go: `current.JSDoc(file)` — the eager parser-filled jsdoc cache.
            let jsdocs = jsdoc_of_node(s, file, current);
            for &jsdoc in jsdocs {
                if node_contains_position(s, jsdoc, position) {
                    child = Some(jsdoc);
                    break;
                }
            }
        }
        if child.is_none() {
            s.node(current).for_each_child(&mut |node| {
                if node_contains_position(s, node, position) {
                    child = Some(node);
                    true
                } else {
                    false
                }
            });
        }
        match child {
            None => return current,
            Some(c) if is_meta_property(s.node(c)) => return current,
            Some(c) => current = c,
        }
    }
}

/// Go: `Node.JSDoc(file)` — `file.JSDocCache[node.ID()]` (empty slice when the
/// cache misses).
fn jsdoc_of_node(s: &dyn NodeStore, file: NodeId, node: NodeId) -> &[NodeId] {
    let d = s.node(file).as_source_file().expect("SourceFile data");
    d.jsdoc_cache
        .as_ref()
        .and_then(|cache| cache.get(&node))
        .map(|jsdocs| jsdocs.as_slice())
        .unwrap_or(&[])
}

/// Go: `func IsRequireCall(node *ast.Node, requireStringLiteralLikeArgument bool) bool`
/// — returns true if the node is a CallExpression to the identifier `require`
/// with exactly one argument.
fn is_require_call(
    s: &dyn NodeStore,
    node: NodeId,
    require_string_literal_like_argument: bool,
) -> bool {
    let n = s.node(node);
    if !is_call_expression(n) {
        return false;
    }
    let call = n.as_call_expression().expect("CallExpression data");
    if !is_identifier(s.node(call.expression)) || node_text(s, call.expression) != "require" {
        return false;
    }
    let args: &[NodeId] = match &call.arguments {
        Some(list) => &list.nodes,
        None => &[],
    };
    if args.len() != 1 {
        return false;
    }
    !require_string_literal_like_argument || is_string_literal_like(s.node(args[0]))
}

/// Go: `func IsImportCall(node *ast.Node) bool`.
fn is_import_call(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    if !is_call_expression(n) {
        return false;
    }
    let expression = n
        .as_call_expression()
        .expect("CallExpression data")
        .expression;
    s.node(expression).kind == Kind::ImportKeyword || is_import_phase_meta_property(s, expression)
}

/// Go: `func IsLiteralImportTypeNode(node *ast.Node) bool`.
fn is_literal_import_type_node(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    if n.kind != Kind::ImportType {
        return false;
    }
    let d = n.as_import_type_node().expect("ImportTypeNode data");
    let argument = s.node(d.argument);
    if argument.kind != Kind::LiteralType {
        return false;
    }
    let literal = argument
        .as_literal_type_node()
        .expect("LiteralType data")
        .literal;
    is_string_literal(s.node(literal))
}

/// Go: `func ForEachDynamicImportOrRequireCall(file *ast.SourceFile,
/// includeTypeSpaceImports bool, requireStringLiteralLikeArgument bool,
/// cb func(node *ast.Node, argument *ast.Expression) bool) bool`.
///
/// PORT: `file.Text()` is read through the store's owning `SourceFile` (the
/// parser keeps a full-text reference; Go rebinds none of it here).
fn for_each_dynamic_import_or_require_call(
    s: &dyn NodeStore,
    file: NodeId,
    include_type_space_imports: bool,
    require_string_literal_like_argument: bool,
    cb: &mut dyn FnMut(NodeId, NodeId) -> bool,
) -> bool {
    let text = s.file(file.file_id()).text();
    let is_javascript_file = is_in_js_file(s.node(file));
    let (mut last_index, mut size) = find_import_or_require(text, 0);
    while last_index >= 0 {
        let node = get_node_at_position(
            s,
            file,
            last_index,
            is_javascript_file && include_type_space_imports,
        );
        let arguments: Option<Vec<NodeId>> = {
            let n = s.node(node);
            if is_call_expression(n) {
                n.as_call_expression()
                    .expect("CallExpression data")
                    .arguments
                    .as_ref()
                    .map(|list| list.nodes.to_vec())
            } else {
                None
            }
        };
        if is_javascript_file && is_require_call(s, node, require_string_literal_like_argument) {
            let argument = arguments.as_ref().and_then(|a| a.first()).copied();
            if let Some(argument) = argument {
                if cb(node, argument) {
                    return true;
                }
            }
        } else if is_import_call(s, node)
            && arguments.as_ref().is_some_and(|a| !a.is_empty())
            && (!require_string_literal_like_argument
                || arguments
                    .as_ref()
                    .is_some_and(|a| is_string_literal_like(s.node(a[0]))))
        {
            let argument = arguments.as_ref().and_then(|a| a.first()).copied();
            if let Some(argument) = argument {
                if cb(node, argument) {
                    return true;
                }
            }
        } else if include_type_space_imports && is_literal_import_type_node(s, node) {
            let n = s.node(node);
            let d = n.as_import_type_node().expect("ImportTypeNode data");
            let literal = s
                .node(d.argument)
                .as_literal_type_node()
                .expect("LiteralType data")
                .literal;
            if cb(node, literal) {
                return true;
            }
        }
        // skip past import/require
        last_index += size as i32;
        let (next_index, next_size) = find_import_or_require(text, last_index);
        last_index = next_index;
        size = next_size;
    }
    false
}
