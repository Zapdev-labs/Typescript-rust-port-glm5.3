// Scanner-side shims for the binder wave (M4 wave A).
//
// `GetErrorRangeForNode` lives in tsc/internal/scanner/scanner.go but is
// explicitly deferred by the scanner wave (see the PORT-TODO in
// rust/crates/scanner/src/scanner.rs: it needs `node.Body()`,
// `ast.GetNameOfDeclaration`, ... — surface that only exists as of this
// binder wave). Per the M4 wave-A brief it is shimmed HERE rather than by
// editing tsc-scanner (dedup candidate: fold into the scanner crate).
//
// Everything else the binder needs from the scanner package already has a
// ported home and is re-exported below (Go import surface parity):
//   DeclarationNameToString               → tsc_scanner::declaration_name_to_string
//   GetIdentifierToken                    → tsc_scanner::get_identifier_token
//   GetSourceTextOfNodeFromSourceFile     → tsc_scanner::get_source_text_of_node_from_source_file
//   GetRangeOfTokenAtPosition             → tsc_scanner::get_range_of_token_at_position
//   TokenToString                         → tsc_scanner::token_to_text (same
//                                           tokenToText table)

use tsc_ast::{
    is_jsx_text, Kind, NodeFlags, NodeId, NodeStore, SourceFile,
};
use tsc_core::text::{TextPos, TextRange};
use tsc_scanner::{
    get_range_of_token_at_position, get_scanner_for_source_file, skip_trivia, token_to_text,
};

pub use tsc_scanner::{declaration_name_to_string, get_identifier_token, token_to_text as token_to_string_fn};

/// Go: `func TokenToString(token ast.Kind) string` (scanner.go) —
/// `tokenToText[token]`.
pub fn token_to_string(token: Kind) -> &'static str {
    token_to_text(token)
}

// ────────────────────────────────────────────────────────────────────────────
// GetErrorRangeForNode (scanner.go, verbatim body)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getErrorRangeForArrowFunction(sourceFile *ast.SourceFile, node *ast.Node) core.TextRange`.
fn get_error_range_for_arrow_function(source_file: &SourceFile, node: NodeId) -> TextRange {
    let n = source_file.node(node);
    let pos = skip_trivia(source_file.text(), n.pos());
    let body = crate::node_data::body_of(source_file, node);
    if let Some(body) = body {
        if source_file.node(body).kind == Kind::Block {
            let start_line = tsc_scanner::get_ecma_line_of_position(source_file, body.pos());
            let end_line = tsc_scanner::get_ecma_line_of_position(source_file, body.end());
            if start_line < end_line {
                // The arrow function spans multiple lines, make the error span be the first line, inclusive.
                return TextRange::new(
                    pos,
                    tsc_scanner::get_ecma_end_line_position(source_file, start_line) + 1,
                );
            }
        }
    }
    TextRange::new(pos, n.end())
}

/// Go: `func findOriginatingJSDocSatisfiesTag(sourceFile *ast.SourceFile, node *ast.Node) *Node`.
///
/// PORT: the walk only ever fires for `NodeFlags::REPARSED` nodes, which the
/// (deferred) reparser.go wave alone produces — nothing in the parsed tree
/// carries the flag yet, so the reparsed-only inner walk (JSDoc cache lookup
/// for the originating @satisfies tag) is unreachable and returns `None`
/// until reparser.go lands (mirror of the scanner wave's own PORT-TODO).
fn find_originating_jsdoc_satisfies_tag(source_file: &SourceFile, node: NodeId) -> Option<NodeId> {
    let target_type = match &source_file.node(node).data {
        tsc_ast::NodeData::SatisfiesExpression(d) => d.type_,
        _ => return None,
    };
    if !source_file.node(target_type).flags.intersects(NodeFlags::REPARSED) {
        return None;
    }
    None
}

/// Go: `func GetErrorRangeForNode(sourceFile *ast.SourceFile, node *ast.Node) core.TextRange`
/// — verbatim body (the KindSatisfiesExpression reparsed sub-path is the
/// findOriginatingJSDocSatisfiesTag PORT case above).
pub fn get_error_range_for_node(source_file: &SourceFile, node: NodeId) -> TextRange {
    let n = source_file.node(node);
    let mut error_node: Option<NodeId> = Some(node);
    match n.kind {
        Kind::SourceFile => {
            let pos = skip_trivia(source_file.text(), 0);
            if pos as usize == source_file.text().len() {
                return TextRange::new(0, 0);
            }
            return get_range_of_token_at_position(source_file, pos);
        }
        // This list is a work in progress. Add missing node kinds to improve their error spans
        Kind::FunctionDeclaration | Kind::MethodDeclaration
            if n.flags.intersects(NodeFlags::REPARSED) => {}
        Kind::VariableDeclaration
        | Kind::BindingElement
        | Kind::ClassDeclaration
        | Kind::InterfaceDeclaration
        | Kind::ModuleDeclaration
        | Kind::EnumDeclaration
        | Kind::EnumMember
        | Kind::FunctionExpression
        | Kind::GetAccessor
        | Kind::SetAccessor
        | Kind::TypeAliasDeclaration
        | Kind::JSTypeAliasDeclaration
        | Kind::PropertyDeclaration
        | Kind::PropertySignature
        | Kind::NamespaceImport
        | Kind::FunctionDeclaration
        | Kind::MethodDeclaration => {
            error_node = crate::ast_shims::get_name_of_declaration(source_file, node);
        }
        Kind::ClassExpression => {
            error_node = n.name();
        }
        Kind::ArrowFunction => {
            return get_error_range_for_arrow_function(source_file, node);
        }
        Kind::CaseClause | Kind::DefaultClause => {
            let start = skip_trivia(source_file.text(), n.pos());
            let mut end = n.end();
            let statements = crate::node_data::statements_of(source_file, node);
            if let Some(statements) = statements {
                if let Some(&first) = statements.nodes.first() {
                    end = source_file.node(first).pos();
                }
            }
            return TextRange::new(start, end);
        }
        Kind::ReturnStatement | Kind::YieldExpression => {
            let pos = skip_trivia(source_file.text(), n.pos());
            return get_range_of_token_at_position(source_file, pos);
        }
        Kind::SatisfiesExpression => {
            if let Some(jsdoc_satisfies_tag) =
                find_originating_jsdoc_satisfies_tag(source_file, node)
            {
                let tag_name_pos = source_file
                    .node(jsdoc_satisfies_tag)
                    .name()
                    .map(|name| source_file.node(name).pos())
                    .unwrap_or_default();
                let pos = skip_trivia(source_file.text(), tag_name_pos);
                return get_range_of_token_at_position(source_file, pos);
            }
            let expr_end = source_file
                .node(crate::node_data::expression_of(source_file, node).expect("SatisfiesExpression expression"))
                .end();
            let pos = skip_trivia(source_file.text(), expr_end);
            return get_range_of_token_at_position(source_file, pos);
        }
        Kind::Constructor if n.flags.intersects(NodeFlags::REPARSED) => {}
        Kind::Constructor => {
            let mut scanner = get_scanner_for_source_file(source_file, n.pos());
            let start = scanner.token_start() as TextPos;
            while scanner.token() != Kind::ConstructorKeyword
                && scanner.token() != Kind::StringLiteral
                && scanner.token() != Kind::EndOfFile
            {
                scanner.scan();
            }
            return TextRange::new(start, scanner.token_end() as TextPos);
        }
        _ => {}
    }
    let Some(error_node) = error_node else {
        // If we don't have a better node, then just set the error on the first token of
        // construct.
        return get_range_of_token_at_position(source_file, n.pos());
    };
    let error_n = source_file.node(error_node);
    let mut pos = error_n.pos();
    if !tsc_ast::node_is_missing(error_n) && !is_jsx_text(error_n) {
        pos = skip_trivia(source_file.text(), pos);
    }
    TextRange::new(pos, error_n.end())
}
