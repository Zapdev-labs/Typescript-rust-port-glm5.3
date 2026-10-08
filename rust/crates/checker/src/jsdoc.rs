// Ported from tsc/internal/checker/jsdoc.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The checker-side JSDoc helpers — M6 W1. (The PARSER-side JSDoc reparse is
// tsc/internal/parser/{jsdoc,reparser}.go, already ported in
// rust/crates/parser/src/{jsdoc.rs,reparser.rs}; this file is only the
// checker's type-side helpers, exactly like Go.)
//
// PORT: the two unported checker.go callees this file needs are permissive
// W1 stubs on `Checker` (checker.rs's "W1 callee stubs" section):
// `containsArgumentsReference` (false — the `arguments`-matching arm stays
// dormant) and, behind it, `getTypeFromTypeNode` (errorType) and `isArrayType`
// (false). The diagnostics flow through the checker's `error`/
// `errorOrSuggestion` collection seam (also stubbed — the M5 compiler wave).

use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    get_source_file_of_node, is_identifier, is_in_js_file, is_qualified_name, node_text, Kind,
    NodeId, NodeStore,
};
use tsc_collections::Set;
use tsc_diagnostics::{
    JSDOC_PARAM_TAG_HAS_NAME_0_BUT_THERE_IS_NO_PARAMETER_WITH_THAT_NAME,
    JSDOC_PARAM_TAG_HAS_NAME_0_BUT_THERE_IS_NO_PARAMETER_WITH_THAT_NAME_IT_WOULD_MATCH_ARGUMENTS_IF_IT_HAD_AN_ARRAY_TYPE,
    QUALIFIED_NAME_0_IS_NOT_ALLOWED_WITHOUT_A_LEADING_PARAM_OBJECT_1,
};

use crate::ast_shims::{get_next_jsdoc_comment_location, parameters_of};
use crate::utilities::entity_name_to_string;
use crate::Checker;

impl Checker {
    /// Go: `func (c *Checker) checkUnmatchedJSDocParameters(node *ast.Node)`.
    pub(crate) fn check_unmatched_jsdoc_parameters(&mut self, node: NodeId) {
        let mut jsdoc_parameters: Vec<NodeId> = Vec::new();
        for tag in get_all_jsdoc_tags(self.store(), node) {
            if self.store().node(tag).kind != Kind::JSDocParameterTag {
                continue;
            }
            let name = self
                .store()
                .node(tag)
                .as_jsdoc_parameter_or_property_tag()
                .expect("JSDocParameterOrPropertyTag data")
                .name;
            // Go: `if ast.IsIdentifier(name) && len(name.Text()) == 0 { continue }`
            // — a bracketed, nameless `@param {T}` tag matches positionally.
            if is_identifier(self.store().node(name)) && node_text(self.store(), name).is_empty() {
                continue;
            }
            jsdoc_parameters.push(tag);
        }

        if jsdoc_parameters.is_empty() {
            return;
        }

        let is_js = is_in_js_file(self.store().node(node));
        let mut parameters: Set<String> = Set::new();
        let mut excluded_parameters: Set<i32> = Set::new();

        for (i, param) in parameters_of(self.store(), node).iter().enumerate() {
            let name = self
                .store()
                .node(*param)
                .as_parameter_declaration()
                .expect("ParameterDeclaration data")
                .name;
            if is_identifier(self.store().node(name)) {
                parameters.add(node_text(self.store(), name));
            }
            if is_binding_pattern(self.store().node(name)) {
                excluded_parameters.add(i as i32);
            }
        }
        if self.contains_arguments_reference(node) {
            // TODO(port M6 S4): `containsArgumentsReference` is the permissive
            // false stub (checker.rs) — this whole arm is dormant until it
            // lands; it is ported faithfully below.
            if is_js {
                let last_jsdoc_param_index = jsdoc_parameters.len() - 1;
                let last_tag = jsdoc_parameters[last_jsdoc_param_index];
                // Go: `lastJSDocParam := jsdocParameters[lastJSDocParamIndex].AsJSDocParameterOrPropertyTag();
                //       if lastJSDocParam == nil || !ast.IsIdentifier(lastJSDocParam.Name()) { return }`
                // — the nil cast is Go's data-interface nil; the port's
                // expect covers it.
                let last_jsdoc_param = self
                    .store()
                    .node(last_tag)
                    .as_jsdoc_parameter_or_property_tag()
                    .expect("JSDocParameterOrPropertyTag data");
                let last_name = last_jsdoc_param.name;
                if !is_identifier(self.store().node(last_name)) {
                    return;
                }
                let last_name_text = node_text(self.store(), last_name);
                // Go: `if excludedParameters.Has(lastJSDocParamIndex) ||
                //       parameters.Has(lastJSDocParam.Name().Text()) { return }`.
                if excluded_parameters.has(&(last_jsdoc_param_index as i32))
                    || parameters.has(&last_name_text)
                {
                    return;
                }
                // Go: `if lastJSDocParam.TypeExpression == nil ||
                //       lastJSDocParam.TypeExpression.Type() == nil { return }`.
                let Some(type_expression) = last_jsdoc_param.type_expression else {
                    return;
                };
                if crate::ast_shims::type_of_node(self.store(), type_expression).is_none() {
                    return;
                }
                // Go: `if c.isArrayType(c.getTypeFromTypeNode(...)) { return }`
                // — both callees are W1 stubs (false / errorType).
                let type_node =
                    crate::ast_shims::type_of_node(self.store(), type_expression).expect("just checked");
                if self.is_array_type(self.get_type_from_type_node(type_node)) {
                    return;
                }
                self.error(
                    last_name,
                    &JSDOC_PARAM_TAG_HAS_NAME_0_BUT_THERE_IS_NO_PARAMETER_WITH_THAT_NAME_IT_WOULD_MATCH_ARGUMENTS_IF_IT_HAD_AN_ARRAY_TYPE,
                    &[last_name_text],
                );
            }
        } else {
            for (index, tag) in jsdoc_parameters.iter().enumerate() {
                let d = self
                    .store()
                    .node(*tag)
                    .as_jsdoc_parameter_or_property_tag()
                    .expect("JSDocParameterOrPropertyTag data");
                let name = d.name;
                let is_name_first = d.is_name_first;

                if excluded_parameters.has(&(index as i32))
                    || (is_identifier(self.store().node(name))
                        && parameters.has(node_text(self.store(), name)))
                {
                    continue;
                }

                if is_qualified_name(self.store().node(name)) {
                    if is_js {
                        // Go passes `entityNameToString(name)` and
                        // `entityNameToString(name.AsQualifiedName().Left)`.
                        let name_string = entity_name_to_string(self.store(), name);
                        let left = self
                            .store()
                            .node(name)
                            .as_qualified_name()
                            .expect("QualifiedName data")
                            .left;
                        let left_string = entity_name_to_string(self.store(), left);
                        self.error(
                            name,
                            &QUALIFIED_NAME_0_IS_NOT_ALLOWED_WITHOUT_A_LEADING_PARAM_OBJECT_1,
                            &[name_string, left_string],
                        );
                    }
                } else {
                    if !is_name_first {
                        self.error_or_suggestion(
                            is_js,
                            name,
                            &JSDOC_PARAM_TAG_HAS_NAME_0_BUT_THERE_IS_NO_PARAMETER_WITH_THAT_NAME,
                            &[node_text(self.store(), name).to_string()],
                        );
                    }
                }
            }
        }
    }
}

/// Go: `func getAllJSDocTags(node *ast.Node) []*ast.Node` — free function
/// (crate-internal like its Go package scope).
pub(crate) fn get_all_jsdoc_tags(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    // Go: `if node.Flags&ast.NodeFlagsJSDoc == 0 { ... }` — only nodes not
    // parsed inside JSDoc carry host-attached JSDoc.
    if !store.node(node).flags.intersects(NF::JSDOC) {
        let mut current = Some(node);
        while let Some(cur) = current {
            let jsdocs = node_jsdoc(store, cur);
            if !jsdocs.is_empty() {
                let last_jsdoc = jsdocs[jsdocs.len() - 1];
                // Go: `if lastJSDoc.Tags != nil { return lastJSDoc.Tags.Nodes }`
                // — a nil Tags list falls through to the next comment location.
                if let Some(tags) = store.node(last_jsdoc).as_jsdoc().expect("JSDoc data").tags.as_ref() {
                    return tags.nodes.clone();
                }
            }
            // Go's for-post: `current = ast.GetNextJSDocCommentLocation(current)`.
            current = get_next_jsdoc_comment_location(store, cur);
        }
    }
    Vec::new()
}

/// Go: `func (node *Node) JSDoc(file *SourceFile) []*ast.Node` (ast/ast.go:1563)
/// — the nil-file walk-to-root form the checker uses.
///
/// PORT: Go's lazy TS path (`file.resolveJSDoc(node)` when `hasLazyJSDoc`)
/// has no port seam yet (parser/src/jsdoc.rs's header note — lazy per-node
/// JSDoc awaits an equivalent of Go's `ast.SetParseJSDocForNode` hook). The
/// port reads the parser's `jsdocCache` directly, which is the exact Go
/// behavior for JS files (eagerly parsed — `hasLazyJSDoc` false) and for the
/// @see/@link TS comments the parser eagerly parses into the same cache.
fn node_jsdoc(store: &dyn NodeStore, node: NodeId) -> &[NodeId] {
    if !store.node(node).flags.intersects(NF::HAS_JSDOC) {
        return &[];
    }
    let Some(file) = get_source_file_of_node(store, node) else {
        return &[];
    };
    // Go: `if file.hasLazyJSDoc { return file.resolveJSDoc(node) };
    //      return file.jsdocCache[node]` — see the PORT note above: the lazy
    // branch is served from the cache until the parser-side seam lands.
    file.payload()
        .jsdoc_cache
        .as_ref()
        .and_then(|cache| cache.get(&node).map(|v| &v[..]))
        .unwrap_or(&[])
}

/// Go: `func IsBindingPattern(node *Node) bool` (ast, generated guard) —
/// reached through the binder crate's ast_shims (the checker already imports
/// it as `binder_shims`); local alias so jsdoc.go's body reads like Go.
fn is_binding_pattern(node: &tsc_ast::Node) -> bool {
    tsc_binder::ast_shims::is_binding_pattern(node)
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (Go has no jsdoc_test.go in the checker package; these pin
// getAllJSDocTags' host-walk against the parser crate's real JSDoc parse —
// the checker-side surface this wave owns — plus checkUnmatchedJSDocParameters'
// traversal on a JS function with unmatched @param tags).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tsc_parser::parse_source_file;
    use tsc_core::scriptkind::ScriptKind;

    /// Parses a JS source file and returns the parsed file. The parser's
    /// `parse_source_file` signature needs the default options.
    fn parse_js(src: &str) -> tsc_ast::SourceFile {
        parse_source_file(Default::default(), src, ScriptKind::JS)
    }

    #[test]
    fn get_all_jsdoc_tags_finds_the_last_jsdoc_on_the_node() {
        let file = parse_js("/** @param a {number} */\nfunction f(a) {}\n");
        // The function declaration carries the HAS_JSDoc flag + cache entry.
        let fun = file
            .payload()
            .statements
            .as_ref()
            .and_then(|s| s.nodes.first().copied());
        let fun = fun.expect("parsed a function statement");
        assert!(file.node(fun).flags.intersects(NF::HAS_JSDOC));
        let tags = get_all_jsdoc_tags(&file, fun);
        // One @param tag.
        assert_eq!(tags.len(), 1);
        assert_eq!(file.node(tags[0]).kind, Kind::JSDocParameterTag);
    }

    #[test]
    fn get_all_jsdoc_tags_walks_to_the_variable_statement_host() {
        // GetNextJSDocCommentLocation: the JSDoc attaches to the
        // VariableStatement (via the VariableDeclaration/List chain), so the
        // lookup starting at the declaration walks up to it.
        let file = parse_js("/** @param a {number} */\nvar f = function (a) {};\n");
        let var_statement = file
            .payload()
            .statements
            .as_ref()
            .and_then(|s| s.nodes.first().copied())
            .expect("parsed a variable statement");
        let decl_list = file
            .node(var_statement)
            .as_variable_statement()
            .expect("VariableStatement data")
            .declaration_list;
        let decl = file
            .node(decl_list)
            .as_variable_declaration_list()
            .expect("VariableDeclarationList data")
            .declarations
            .as_ref()
            .and_then(|d| d.nodes.first().copied())
            .expect("one declaration");
        // The declaration node itself carries HAS_JSDoc (the parser attaches
        // it to the node it parsed JSDoc for) — the walk may find it there,
        // or at the statement host via GetNextJSDocCommentLocation.
        assert!(file.node(decl).flags.intersects(NF::HAS_JSDOC));
        let tags = get_all_jsdoc_tags(&file, decl);
        assert_eq!(tags.len(), 1);
        assert_eq!(file.node(tags[0]).kind, Kind::JSDocParameterTag);
    }

    #[test]
    fn get_all_jsdoc_tags_returns_empty_without_jsdoc() {
        let file = parse_js("function f(a) {}\n");
        let fun = file
            .payload()
            .statements
            .as_ref()
            .and_then(|s| s.nodes.first().copied())
            .expect("parsed a function statement");
        assert!(get_all_jsdoc_tags(&file, fun).is_empty());
    }

    #[test]
    fn check_unmatched_jsdoc_parameters_traverses_matched_and_unmatched() {
        // Runs the full checker flow on a JS function whose second @param
        // name matches no parameter — the diagnostics are stubbed no-ops
        // (the collection seam), so the observable contract here is the
        // traversal completing exactly: matched params skipped, unmatched
        // ones reported through the seam without panic.
        let file = parse_js("/** @param a {number} @param b {string} */\nfunction f(a) {}\n");
        let fun = file
            .payload()
            .statements
            .as_ref()
            .and_then(|s| s.nodes.first().copied())
            .expect("parsed a function statement");
        let program = super::jsdoc_tests_support::TestProgram::new(file);
        let mut c = crate::new_checker(Box::new(program));
        c.check_unmatched_jsdoc_parameters(fun);
    }

    #[test]
    fn check_unmatched_jsdoc_parameters_skips_nameless_tags_and_qualified_names() {
        // A bracketed nameless `@param {T}` tag is positional (skipped as a
        // jsdocParameter? no — it IS collected but excluded from reporting by
        // the name match below); a qualified-name @param in JS reports the
        // leading-@param error. Both paths must complete through the seam.
        let file = parse_js("/** @param {number} @param ns.b {string} */\nfunction f(a) {}\n");
        let fun = file
            .payload()
            .statements
            .as_ref()
            .and_then(|s| s.nodes.first().copied())
            .expect("parsed a function statement");
        let program = super::jsdoc_tests_support::TestProgram::new(file);
        let mut c = crate::new_checker(Box::new(program));
        c.check_unmatched_jsdoc_parameters(fun);
    }
}

/// Test support for jsdoc.rs's unit tests — the single-file Program seam
/// (mirrors tests/spike.rs's TestProgram).
#[cfg(test)]
pub(crate) mod jsdoc_tests_support {
    use crate::checker::Program;
    use tsc_ast::{SourceFile, Symbol, SymbolId};
    use tsc_binder::{bind_source_file, BindResult};

    pub(crate) struct TestProgram {
        options: tsc_core::options_generated::CompilerOptions,
        source_file: SourceFile,
        bind: BindResult,
        binder_symbol_count: u32,
        checker_symbols: Vec<Symbol>,
    }

    impl TestProgram {
        pub(crate) fn new(source_file: SourceFile) -> Self {
            // Bind so the checker's symbol seam resolves; the binder's
            // arena is the base of the program SymbolId space (§5.5).
            let mut sf = source_file;
            let bind = bind_source_file(&mut sf).expect("bind produced a result");
            let binder_symbol_count = bind.symbol_count as u32;
            Self {
                options: Default::default(),
                source_file: sf,
                bind,
                binder_symbol_count,
                checker_symbols: Vec::new(),
            }
        }
    }

    impl Program for TestProgram {
        fn options(&self) -> &tsc_core::options_generated::CompilerOptions {
            &self.options
        }
        fn source_files(&self) -> &[SourceFile] {
            std::slice::from_ref(&self.source_file)
        }
        fn bind_source_files(&mut self) {}
        fn node_store(&self) -> &dyn tsc_ast::NodeStore {
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
}
