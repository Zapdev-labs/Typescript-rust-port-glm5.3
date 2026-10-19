// Ported from tsc/internal/checker/grammarchecks.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The checkGrammar* family (M6 W3): the grammar-error helpers + every
// grammar check the Go checker runs from its check* leaves. Every function
// mirrors Go's name snake_cased with the same control flow and evaluation
// order; PORT notes where the §5.1 node-arena seam or a program-layer
// service changes the shape.
//
// Go runs these from the check stage's leaves (statements/expressions/
// typenodes); the Rust call sites were wired in the S1-S10 slices against
// the permissive stub family this wave replaces (the stubs lived in
// statements.rs / expressions.rs / spine.rs and are deleted with this file).

use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    has_syntactic_modifier, is_case_clause, is_default_clause, is_identifier,
    is_js_type_alias_declaration, is_parenthesized_expression,
    is_private_identifier, is_source_file, Kind, ModifierFlags, Node, NodeId, NodeList, NodeStore,
};
use tsc_core::options_generated::{ModuleKind, ScriptTarget};
use tsc_core::text::{TextPos, TextRange};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_diagnostics as diag;
use tsc_binder::scanner_shims::token_to_string;

use crate::ast_shims::{
    get_text_of_node, initializer_of, parameters_of, postfix_token_of,
};
use crate::checker::Checker;
use crate::utilities as u;

// ────────────────────────────────────────────────────────────────────────────
// Small Go-ast predicate ports the family reads (tsc/internal/ast/utilities.go —
// the checker crate cannot reach the parser-private shims; mirrored locally
// and PORT-noted per body)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsModifierKind(kind Kind) bool` (ast_generated.go:9838).
pub(crate) fn is_modifier_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::AbstractKeyword
            | Kind::AccessorKeyword
            | Kind::AsyncKeyword
            | Kind::ConstKeyword
            | Kind::DeclareKeyword
            | Kind::DefaultKeyword
            | Kind::ExportKeyword
            | Kind::InKeyword
            | Kind::PrivateKeyword
            | Kind::ProtectedKeyword
            | Kind::PublicKeyword
            | Kind::ReadonlyKeyword
            | Kind::OutKeyword
            | Kind::OverrideKeyword
            | Kind::StaticKeyword
    )
}

/// Go: `func IsModifier(node *Node) bool` (utilities.go:104).
pub(crate) fn is_modifier(node: &Node) -> bool {
    is_modifier_kind(node.kind)
}

/// Go: `func IsDecorator(node *Node) bool` (ast_generated.go:11454).
fn is_decorator(node: &Node) -> bool {
    tsc_ast::is_decorator(node)
}

/// Go: `func CanHaveIllegalDecorators(node *Node) bool` (utilities.go:1072).
pub(crate) fn can_have_illegal_decorators(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::FunctionDeclaration
            | Kind::Constructor
            | Kind::IndexSignature
            | Kind::ClassStaticBlockDeclaration
            | Kind::MissingDeclaration
            | Kind::VariableStatement
            | Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::EnumDeclaration
            | Kind::ModuleDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::NamespaceExportDeclaration
            | Kind::ExportDeclaration
            | Kind::ExportAssignment
    )
}

/// Go: `func CanHaveIllegalModifiers(node *Node) bool` (utilities.go:1088).
pub(crate) fn can_have_illegal_modifiers(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::ClassStaticBlockDeclaration
            | Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::MissingDeclaration
            | Kind::NamespaceExportDeclaration
    )
}

/// Go: `func CanHaveModifiers(node *Node) bool` (utilities.go:1100).
pub(crate) fn can_have_modifiers(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::TypeParameter
            | Kind::Parameter
            | Kind::PropertySignature
            | Kind::PropertyDeclaration
            | Kind::MethodSignature
            | Kind::MethodDeclaration
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::IndexSignature
            | Kind::ConstructorType
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::ClassExpression
            | Kind::VariableStatement
            | Kind::FunctionDeclaration
            | Kind::ClassDeclaration
            | Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::EnumDeclaration
            | Kind::ModuleDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::ExportAssignment
            | Kind::ExportDeclaration
    )
}

/// Go: `func HasDecorators(node *Node) bool` — the modifiers list carries
/// the decorators (utilities.go's `HasSyntacticModifier(node,
/// ModifierFlagsDecorator)` shape; mirrored over the list because the
/// parser computes the flags list-agnostically).
pub(crate) fn has_decorators(store: &dyn NodeStore, node: NodeId) -> bool {
    store
        .node(node)
        .modifiers()
        .is_some_and(|m| m.nodes.iter().any(|&mo| is_decorator(store.node(mo))))
}

/// Go: `func GetThisParameter(signature *Node) *Node` (utilities.go).
pub(crate) fn get_this_parameter(store: &dyn NodeStore, signature: NodeId) -> Option<NodeId> {
    // callback tags do not currently support this parameters
    let parameters = parameters_of(store, signature);
    if let Some(&first) = parameters.first() {
        if crate::ast_shims::is_this_parameter(store, first) {
            return Some(first);
        }
    }
    None
}

/// Go: `func HasAbstractModifier(node *Node) bool` / `HasAmbientModifier`
/// (utilities.go) — HasSyntacticModifier over the flag.
/// PORT (FCS tranche 29): pub(crate) — NodeCanBeDecorated (statements.rs)
/// reads both.
pub(crate) fn has_abstract_modifier(node: &Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
}

/// Go: `func HasAmbientModifier(node *Node) bool` (utilities.go).
pub(crate) fn has_ambient_modifier(node: &Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::AMBIENT)
}

/// Go: `func NodeCanBeDecorated(useLegacyDecorators, node, parent, grandparent) bool`
/// (utilities.go:4327) — ported locally (the parser crate's copy is
/// crate-private; PORT-noted body-identical).
pub(crate) fn node_can_be_decorated(
    store: &dyn NodeStore,
    use_legacy_decorators: bool,
    node: NodeId,
    parent: Option<NodeId>,
    grandparent: Option<NodeId>,
) -> bool {
    // private names cannot be used with decorators yet
    let name = store.node(node).name();
    if use_legacy_decorators
        && name.is_some()
        && is_private_identifier(store.node(name.expect("checked")))
    {
        return false;
    }
    let parent_is_class_declaration =
        parent.is_some_and(|p| store.node(p).kind == Kind::ClassDeclaration);
    let parent_is_class_like = parent.is_some_and(|p| is_class_like(store, p));
    match store.node(node).kind {
        Kind::ClassDeclaration => true,
        Kind::ClassExpression => !use_legacy_decorators,
        Kind::PropertyDeclaration => {
            // property declarations are valid if their parent is a class declaration.
            parent.is_some()
                && ((use_legacy_decorators && parent_is_class_declaration)
                    || (!use_legacy_decorators
                        && parent_is_class_like
                        && !has_abstract_modifier(store.node(node))
                        && !has_ambient_modifier(store.node(node))))
        }
        Kind::GetAccessor | Kind::SetAccessor | Kind::MethodDeclaration => {
            // if this method has a body and its parent is a class declaration, this is a valid target.
            parent.is_some()
                && body_of(store, node).is_some()
                && ((use_legacy_decorators && parent_is_class_declaration)
                    || (!use_legacy_decorators && parent_is_class_like))
        }
        Kind::Parameter => {
            // TODO(rbuckton): ParameterDeclaration decorator support for ES decorators
            // must wait until it is standardized
            if !use_legacy_decorators {
                return false;
            }
            // if the parameter's parent has a body and its grandparent is a
            // class declaration, this is a valid target.
            parent.is_some()
                && parent.is_some_and(|p| body_of(store, p).is_some())
                && parent.is_some_and(|p| {
                    matches!(
                        store.node(p).kind,
                        Kind::Constructor | Kind::MethodDeclaration | Kind::SetAccessor
                    )
                })
                && parent.is_some_and(|p| get_this_parameter(store, p) != Some(node))
                && grandparent.is_some_and(|g| store.node(g).kind == Kind::ClassDeclaration)
        }
        _ => false,
    }
}

/// Go: `node.Body()` — the function-like body accessor (ast.go's dispatch;
/// the kinds NodeCanBeDecorated reads all carry `body`).
fn body_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::FunctionDeclaration(d) => d.body,
        tsc_ast::NodeData::FunctionExpression(d) => d.body,
        tsc_ast::NodeData::MethodDeclaration(d) => d.body,
        tsc_ast::NodeData::GetAccessorDeclaration(d) => d.body,
        tsc_ast::NodeData::SetAccessorDeclaration(d) => d.body,
        tsc_ast::NodeData::ConstructorDeclaration(d) => d.body,
        tsc_ast::NodeData::ArrowFunction(d) => d.body,
        _ => None,
    }
}

/// Go: `func isClassLike(node *Node) bool` (utilities.go).
pub(crate) fn is_class_like(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::ClassDeclaration | Kind::ClassExpression
    )
}

/// Go: `func GetAllAccessorDeclarationsForDeclaration(accessor, declarationsOfSymbol) AllAccessorDeclarations`
/// (utilities.go:4436) — the shape the grammar modifier arm reads
/// (FirstAccessor/SecondAccessor).
struct AllAccessorDeclarations {
    first_accessor: Option<NodeId>,
    second_accessor: Option<NodeId>,
}

fn get_all_accessor_declarations_for_declaration(
    store: &dyn NodeStore,
    accessor: NodeId,
    declarations_of_symbol: &[NodeId],
) -> AllAccessorDeclarations {
    let kind = store.node(accessor).kind;
    let other_kind = if kind == Kind::SetAccessor {
        Kind::GetAccessor
    } else if kind == Kind::GetAccessor {
        Kind::SetAccessor
    } else {
        panic!("Unexpected node kind {:?}", kind)
    };
    let other_accessor = declarations_of_symbol
        .iter()
        .copied()
        .find(|&d| store.node(d).kind == other_kind);
    let (first_accessor, second_accessor) = if other_accessor
        .is_some_and(|o| store.node(o).loc.pos() < store.node(accessor).loc.pos())
    {
        (other_accessor, Some(accessor))
    } else {
        (Some(accessor), other_accessor)
    };
    AllAccessorDeclarations {
        first_accessor,
        second_accessor,
    }
}

/// Go: `func IsDynamicName(declaration *DeclarationName) bool` (utilities.go).
pub(crate) fn is_dynamic_name(store: &dyn NodeStore, name: NodeId) -> bool {
    let expr = match store.node(name).kind {
        Kind::ComputedPropertyName => {
            match store.node(name).as_computed_property_name() {
                Some(d) => d.expression,
                None => return false,
            }
        }
        Kind::ElementAccessExpression => {
            let argument = store
                .node(name)
                .as_element_access_expression()
                .map(|d| d.argument_expression);
            match argument {
                Some(arg) => tsc_ast::utilities::skip_parentheses(store, arg),
                None => return false,
            }
        }
        _ => return false,
    };
    !is_string_or_numeric_literal_like(store.node(expr)) && !is_signed_numeric_literal(store, expr)
}

fn is_string_or_numeric_literal_like(node: &Node) -> bool {
    tsc_ast::utilities::is_string_literal_like(node)
}

/// Go: `func IsSignedNumericLiteral(node *Node) bool` (utilities.go:335).
fn is_signed_numeric_literal(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind == Kind::PrefixUnaryExpression {
        let d = store
            .node(node)
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression data");
        return matches!(d.operator, Kind::PlusToken | Kind::MinusToken)
            && tsc_ast::is_numeric_literal(store.node(d.operand));
    }
    false
}

/// Go: `func IsEffectiveExternalModule(node *SourceFile, compilerOptions) bool`
/// (utilities.go:1695) — the checker's module-kind read over the file's
/// external-module indicator + CommonJS module indicator.
pub(crate) fn is_effective_external_module(
    file: &tsc_ast::SourceFile,
    emit_module_kind: ModuleKind,
) -> bool {
    if tsc_binder::ast_shims::is_external_module(file) {
        return true;
    }
    // Go: isCommonJSContainingModuleKind(GetEmitModuleKind()) && node.CommonJSModuleIndicator != nil
    let is_common_js_containing = emit_module_kind == ModuleKind::CommonJS
        || (ModuleKind::Node16 <= emit_module_kind && emit_module_kind <= ModuleKind::NodeNext);
    is_common_js_containing && file.payload().common_js_module_indicator.get().is_some()
}

/// Go: `func IsCommaSequence(node *Node) bool` / `IsCommaExpression`
/// (utilities.go:459/…).
pub(crate) fn is_comma_sequence(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    n.kind == Kind::BinaryExpression
        && n.as_binary_expression()
            .is_some_and(|d| store.node(d.operator_token).kind == Kind::CommaToken)
}

/// Go: `func IsIterationStatement(node, lookInLabeledStatements) bool`
/// (utilities.go:463).
pub(crate) fn is_iteration_statement(
    store: &dyn NodeStore,
    node: NodeId,
    look_in_labeled_statements: bool,
) -> bool {
    match store.node(node).kind {
        Kind::ForStatement | Kind::ForInStatement | Kind::ForOfStatement | Kind::DoStatement
        | Kind::WhileStatement => true,
        Kind::LabeledStatement => {
            look_in_labeled_statements
                && store
                    .node(node)
                    .as_labeled_statement()
                    .is_some_and(|d| is_iteration_statement(store, d.statement, look_in_labeled_statements))
        }
        _ => false,
    }
}

/// Go: `func IsAccessor(node *Node) bool` (utilities.go).
pub(crate) fn is_accessor(node: &Node) -> bool {
    matches!(node.kind, Kind::GetAccessor | Kind::SetAccessor)
}

/// Go: `func isImportMetaProperty(node, name) bool` + the four exported
/// predicates (utilities.go:2153-2177).
fn is_import_meta_property(store: &dyn NodeStore, node: NodeId, name: &str) -> bool {
    store.node(node).as_meta_property().is_some_and(|d| {
        d.keyword_token == Kind::ImportKeyword
            && tsc_ast::node_text(store, d.name) == name
    })
}

pub(crate) fn is_import_defer_meta_property(store: &dyn NodeStore, node: NodeId) -> bool {
    is_import_meta_property(store, node, "defer")
}

pub(crate) fn is_import_source_meta_property(store: &dyn NodeStore, node: NodeId) -> bool {
    is_import_meta_property(store, node, "source")
}

pub(crate) fn is_import_phase_meta_property(store: &dyn NodeStore, node: NodeId) -> bool {
    is_import_defer_meta_property(store, node) || is_import_source_meta_property(store, node)
}

/// Go: `func IsImportCall(node *Node) bool` (utilities.go).
fn is_import_call(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::CallExpression {
        return false;
    }
    match crate::ast_shims::expression_of(store, node) {
        Some(e) => {
            store.node(e).kind == Kind::ImportKeyword || is_import_phase_meta_property(store, e)
        }
        None => false,
    }
}

/// Go: `func IsJsxTagName(node *Node) bool` (utilities.go).
fn is_jsx_tag_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    if parent == NodeId::NONE {
        return false;
    }
    match store.node(parent).kind {
        Kind::JsxOpeningElement | Kind::JsxClosingElement | Kind::JsxSelfClosingElement => {
            store.node(parent).name() == Some(node)
        }
        _ => false,
    }
}

/// Go: `func IsTypeQueryNode(node *Node) bool` / `IsJSDocLinkLike` /
/// `IsJSDocNameReference` / `IsHeritageClause` (utilities.go).
fn is_type_query_node(node: &Node) -> bool {
    node.kind == Kind::TypeQuery
}

fn is_jsdoc_link_like(node: &Node) -> bool {
    matches!(
        node.kind,
        Kind::JSDocLink | Kind::JSDocLinkCode | Kind::JSDocLinkPlain
    )
}

fn is_jsdoc_name_reference(node: &Node) -> bool {
    node.kind == Kind::JSDocNameReference
}

fn is_heritage_clause(node: &Node) -> bool {
    node.kind == Kind::HeritageClause
}

/// Go: `func IsInExpressionContext(node *Node) bool` (utilities.go).
fn is_in_expression_context(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    if parent == NodeId::NONE {
        return false;
    }
    let parent_node = store.node(parent);
    match parent_node.kind {
        Kind::VariableDeclaration
        | Kind::Parameter
        | Kind::PropertyDeclaration
        | Kind::PropertySignature
        | Kind::EnumMember
        | Kind::PropertyAssignment
        | Kind::BindingElement => initializer_of(store, parent) == Some(node),
        Kind::ExpressionStatement
        | Kind::IfStatement
        | Kind::DoStatement
        | Kind::WhileStatement
        | Kind::ReturnStatement
        | Kind::WithStatement
        | Kind::SwitchStatement
        | Kind::CaseClause
        | Kind::DefaultClause
        | Kind::ThrowStatement
        | Kind::TypeAssertionExpression
        | Kind::AsExpression
        | Kind::TemplateSpan
        | Kind::ComputedPropertyName
        | Kind::SatisfiesExpression => crate::ast_shims::expression_of(store, parent) == Some(node),
        Kind::ForStatement => parent_node
            .as_for_statement()
            .is_some_and(|s| {
                (s.initializer == Some(node) && store.node(node).kind != Kind::VariableDeclarationList)
                    || s.condition == Some(node)
                    || s.incrementor == Some(node)
            }),
        Kind::ForInStatement | Kind::ForOfStatement => parent_node
            .as_for_in_or_of_statement()
            .is_some_and(|s| {
                (s.initializer == node && store.node(node).kind != Kind::VariableDeclarationList)
                    || s.expression == node
            }),
        Kind::Decorator | Kind::JsxExpression | Kind::JsxSpreadAttribute | Kind::SpreadAssignment => {
            true
        }
        Kind::ExpressionWithTypeArguments => {
            crate::ast_shims::expression_of(store, parent) == Some(node)
                && !is_part_of_type_node(store, parent)
        }
        Kind::ShorthandPropertyAssignment => parent_node
            .as_shorthand_property_assignment()
            .is_some_and(|s| s.object_assignment_initializer == Some(node)),
        Kind::FunctionExpression | Kind::ClassExpression => parent_node.name() != Some(node),
        _ => is_expression_node(store, parent),
    }
}

/// Go: `func IsPartOfTypeNode(node *Node) bool` — the minimal slice
/// IsInExpressionContext reads (an ExpressionWithTypeArguments parent that
/// is itself under a heritage clause is a type position).
fn is_part_of_type_node(store: &dyn NodeStore, node: NodeId) -> bool {
    is_heritage_clause(store.node(node))
}

/// Go: `func IsExpressionNode(node *Node) bool` (utilities.go:1997).
pub(crate) fn is_expression_node(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::SuperKeyword | Kind::NullKeyword | Kind::TrueKeyword | Kind::FalseKeyword
        | Kind::RegularExpressionLiteral | Kind::ArrayLiteralExpression | Kind::ObjectLiteralExpression
        | Kind::PropertyAccessExpression | Kind::ElementAccessExpression | Kind::CallExpression
        | Kind::NewExpression | Kind::TaggedTemplateExpression | Kind::AsExpression
        | Kind::TypeAssertionExpression | Kind::SatisfiesExpression | Kind::NonNullExpression
        | Kind::ParenthesizedExpression | Kind::FunctionExpression | Kind::ClassExpression
        | Kind::ArrowFunction | Kind::VoidExpression | Kind::DeleteExpression | Kind::TypeOfExpression
        | Kind::PrefixUnaryExpression | Kind::PostfixUnaryExpression | Kind::BinaryExpression
        | Kind::ConditionalExpression | Kind::SpreadElement | Kind::TemplateExpression
        | Kind::OmittedExpression | Kind::JsxElement | Kind::JsxSelfClosingElement
        | Kind::JsxFragment | Kind::YieldExpression | Kind::AwaitExpression => true,
        Kind::MetaProperty => {
            // `import.<phase>` in `import.<phase>(...)` is not an expression
            let parent = store.node(node).parent.get();
            !is_import_call(store, parent)
                || crate::ast_shims::expression_of(store, parent) != Some(node)
        }
        Kind::ExpressionWithTypeArguments => !is_heritage_clause(store.node(n.parent.get())),
        Kind::QualifiedName => {
            let mut node = node;
            while store.node(node).kind == Kind::QualifiedName {
                node = store.node(node).parent.get();
            }
            let parent = store.node(node).parent.get();
            is_type_query_node(store.node(parent))
                || is_jsdoc_link_like(store.node(parent))
                || is_jsdoc_name_reference(store.node(parent))
                || is_jsx_tag_name(store, node)
        }
        Kind::PrivateIdentifier => {
            let parent = store.node(node).parent.get();
            store.node(parent).kind == Kind::BinaryExpression
                && store
                    .node(parent)
                    .as_binary_expression()
                    .is_some_and(|d| d.left == node && store.node(d.operator_token).kind == Kind::InKeyword)
        }
        Kind::Identifier => {
            let parent = store.node(node).parent.get();
            if is_type_query_node(store.node(parent))
                || is_jsdoc_link_like(store.node(parent))
                || is_jsdoc_name_reference(store.node(parent))
                || is_jsx_tag_name(store, node)
            {
                return true;
            }
            // Go: fallthrough into the literal kinds' arm.
            is_in_expression_context(store, node)
        }
        Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::StringLiteral
        | Kind::NoSubstitutionTemplateLiteral
        | Kind::ThisKeyword => is_in_expression_context(store, node),
        _ => false,
    }
}

/// Go: `func IsInTopLevelContext(node *Node) bool` (utilities.go:1817) —
/// GetThisContainer(node, true, false) is a SourceFile.
pub(crate) fn is_in_top_level_context(store: &dyn NodeStore, node: NodeId) -> bool {
    // The name of a class or function declaration is a BindingIdentifier in
    // its surrounding scope.
    let mut node = node;
    if is_identifier(store.node(node)) {
        let parent = store.node(node).parent.get();
        let parent_is_decl_name = parent != tsc_ast::NodeId::NONE
            && matches!(store.node(parent).kind, Kind::ClassDeclaration | Kind::FunctionDeclaration)
            && store.node(parent).name() == Some(node);
        if parent_is_decl_name {
            node = parent;
        }
    }
    let container = tsc_binder::ast_shims::get_this_container(store, node, true, false);
    is_source_file(store.node(container))
}

// ────────────────────────────────────────────────────────────────────────────
// The grammar-error helpers (grammarchecks.go:19-57)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    fn new_diagnostic_for_range(
        &self,
        file: NodeId,
        span: TextRange,
        message: &'static Message,
        args: Vec<String>,
    ) -> tsc_ast::Diagnostic {
        tsc_ast::Diagnostic::new(
            Some(file),
            span,
            message.code(),
            message.category(),
            "",
            Some(message),
            "",
            message.key(),
            args,
        )
    }

    /// Go: `func (c *Checker) grammarErrorOnFirstToken(node, message, args...) bool`.
    pub(crate) fn grammar_error_on_first_token(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let source_file = self.get_source_file_node_of(node);
        if !self.has_parse_diagnostics(source_file) {
            let span = self
                .source_file_of(source_file)
                .map(|f| tsc_scanner::get_range_of_token_at_position(f, self.store().node(node).loc.pos()))
                .unwrap_or_default();
            let diagnostic = self.new_diagnostic_for_range(source_file, span, message, args.to_vec());
            self.add_diagnostic(diagnostic);
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) grammarErrorAtPos(nodeForSourceFile, start, length, message, args...) bool`.
    pub(crate) fn grammar_error_at_pos(
        &mut self,
        node_for_source_file: NodeId,
        start: TextPos,
        length: i32,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let source_file = self.get_source_file_node_of(node_for_source_file);
        if !self.has_parse_diagnostics(source_file) {
            let span = TextRange::new(start, start + length);
            let diagnostic = self.new_diagnostic_for_range(source_file, span, message, args.to_vec());
            self.add_diagnostic(diagnostic);
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) grammarErrorOnNode(node, message, args...) bool`.
    pub(crate) fn grammar_error_on_node(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let source_file = self.get_source_file_node_of(node);
        if !self.has_parse_diagnostics(source_file) {
            self.error(node, message, args);
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) grammarErrorOnNodeSkippedOnNoEmit(node, message, args...) bool`.
    #[allow(dead_code)] // the __esModule marker arm's caller chain
    pub(crate) fn grammar_error_on_node_skipped_on_no_emit(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let source_file = self.get_source_file_node_of(node);
        if !self.has_parse_diagnostics(source_file) {
            let mut d = u::new_diagnostic_for_node(self.store(), node, message, args.to_vec());
            d.set_skipped_on_no_emit();
            self.add_diagnostic(d);
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) hasParseDiagnostics(sourceFile) bool` — the
    /// source file's parse diagnostics (checker.go:hasParseDiagnostics; the
    /// family's gate). Replaces the W3 permissive stub (statements.rs).
    pub(crate) fn has_parse_diagnostics(&self, source_file: NodeId) -> bool {
        self.source_file_of(source_file)
            .is_some_and(|f| !f.diagnostics().is_empty())
    }

    // ────────────────────────────────────────────────────────────────────────
    // The check family (grammarchecks.go:58-2266, Go order)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) checkGrammarRegularExpressionLiteral(node) bool`.
    ///
    /// PORT: Go re-scans the literal with a fresh scanner and folds the
    /// scanner errors through a `lastError` closure (related-info spelling
    /// suggestions). The Rust scanner's on-error callback is `FnMut` over
    /// borrowed state, so the port records the callback events and replays
    /// Go's exact lastError loop after the scan — the same diagnostics and
    /// the same dedupe shape.
    pub(crate) fn check_grammar_regular_expression_literal(&mut self, node: NodeId) -> bool {
        let source_file = self.get_source_file_node_of(node);
        if !self.has_parse_diagnostics(source_file) {
            let Some(file) = self.source_file_of(source_file) else {
                return false;
            };
            let node_pos = self.store().node(node).loc.pos();
            let text = file.text().to_string();
            let language_variant = file.payload().language_variant;
            // Events: (message, start, length, args) in callback order.
            // (A named type — clippy's type_complexity gate.)
            type ScanEvents = std::cell::RefCell<Vec<(&'static Message, i32, i32, Vec<String>)>>;
            let events: ScanEvents = std::cell::RefCell::new(Vec::new());
            {
                let mut scanner = tsc_scanner::Scanner::new();
                scanner.set_script_target(self.language_version);
                scanner.set_language_variant(language_variant);
                {
                    let events = &events;
                    scanner.set_on_error(move |message, start, length, args| {
                        events.borrow_mut().push((message, start, length, args.to_vec()));
                    });
                }
                scanner.set_text(&text);
                scanner.reset_token_state(node_pos as usize);
                scanner.scan();
                let token_is_regular_expression_literal =
                    scanner.re_scan_slash_token(true) == Kind::RegularExpressionLiteral;
                debug_assert!(token_is_regular_expression_literal);
            }
            // Go's lastError loop.
            let mut last_error: Option<(i32, tsc_ast::Diagnostic)> = None;
            for (message, start, length, args) in events.into_inner() {
                let same_pos = last_error.as_ref().is_some_and(|(p, _)| *p == start);
                if message.category() == tsc_diagnostics::Category::Message && same_pos {
                    // For providing spelling suggestions.
                    let err = tsc_ast::Diagnostic::new(
                        None,
                        TextRange::new(start as TextPos, (start + length) as TextPos),
                        message.code(),
                        message.category(),
                        "",
                        Some(message),
                        "",
                        message.key(),
                        args,
                    );
                    if let Some((_, last)) = last_error.as_mut() {
                        last.add_related_info(err);
                    }
                } else if !same_pos {
                    let span = TextRange::new(start as TextPos, (start + length) as TextPos);
                    let d = self.new_diagnostic_for_range(source_file, span, message, args);
                    let d = self.add_diagnostic(d);
                    last_error = Some((start, d));
                }
            }
            return last_error.is_some();
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarPrivateIdentifierExpression(privId) bool`.
    pub(crate) fn check_grammar_private_identifier_expression(&mut self, node: NodeId) -> bool {
        let containing_class = find_ancestor_or_class(self.store(), node);
        if containing_class.is_none() {
            return self.grammar_error_on_node(
                node,
                &diag::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
                &[],
            );
        }
        let parent = self.store().node(node).parent.get();
        if self.store().node(parent).kind != Kind::ForInStatement {
            if !is_expression_node(self.store(), node) {
                return self.grammar_error_on_node(
                    node,
                    &diag::PRIVATE_IDENTIFIERS_ARE_ONLY_ALLOWED_IN_CLASS_BODIES_AND_MAY_ONLY_BE_USED_AS_PART_OF_A_CLASS_MEMBER_DECLARATION_PROPERTY_ACCESS_OR_ON_THE_LEFT_HAND_SIDE_OF_AN_IN_EXPRESSION,
                    &[],
                );
            }
            let is_in_operation = self.store().node(parent).kind == Kind::BinaryExpression
                && self
                    .store()
                    .node(parent)
                    .as_binary_expression()
                    .is_some_and(|d| self.store().node(d.operator_token).kind == Kind::InKeyword);
            if self.get_symbol_for_private_identifier_expression(node).is_none() && !is_in_operation
            {
                let text = get_text_of_node(self.store(), node);
                return self.grammar_error_on_node(node, &diag::CANNOT_FIND_NAME_0, &[text]);
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarMappedType(node *ast.MappedTypeNode) bool`.
    pub(crate) fn check_grammar_mapped_type(&mut self, node: NodeId) -> bool {
        if let Some(members) = self
            .store()
            .node(node)
            .as_mapped_type_node()
            .and_then(|d| d.members.as_ref().map(|m| m.nodes.to_vec()))
            && !members.is_empty()
        {
            return self.grammar_error_on_node(
                members[0],
                &diag::A_MAPPED_TYPE_MAY_NOT_DECLARE_PROPERTIES_OR_METHODS,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarDecorator(decorator *ast.Decorator) bool`.
    pub(crate) fn check_grammar_decorator(&mut self, decorator: NodeId) -> bool {
        let source_file = self.get_source_file_node_of(decorator);
        if !self.has_parse_diagnostics(source_file) {
            let mut node = self
                .store()
                .node(decorator)
                .as_decorator()
                .map(|d| d.expression)
                .expect("Decorator expression");

            // DecoratorParenthesizedExpression :
            //   `(` Expression `)`

            if is_parenthesized_expression(self.store().node(node)) {
                return false;
            }

            let mut can_have_call_expression = true;
            let mut error_node: Option<NodeId> = None;
            loop {
                // Allow TS syntax such as non-null assertions and instantiation expressions
                if matches!(
                    self.store().node(node).kind,
                    Kind::ExpressionWithTypeArguments | Kind::NonNullExpression
                ) {
                    node = crate::ast_shims::expression_of(self.store(), node)
                        .expect("ExpressionWithTypeArguments/NonNull expression");
                    continue;
                }

                // DecoratorCallExpression :
                //   DecoratorMemberExpression Arguments

                if self.store().node(node).kind == Kind::CallExpression {
                    let d = self
                        .store()
                        .node(node)
                        .as_call_expression()
                        .expect("CallExpression data")
                        .clone();
                    if !can_have_call_expression {
                        error_node = Some(node);
                    }
                    if let Some(question_dot) = d.question_dot_token {
                        // Even if we already have an error node, error at the `?.` token since it appears earlier.
                        error_node = Some(question_dot);
                    }
                    node = d.expression;
                    can_have_call_expression = false;
                    continue;
                }

                // DecoratorMemberExpression :
                //   IdentifierReference
                //   DecoratorMemberExpression `.` IdentifierName
                //   DecoratorMemberExpression `.` PrivateIdentifier

                if self.store().node(node).kind == Kind::PropertyAccessExpression {
                    let d = self
                        .store()
                        .node(node)
                        .as_property_access_expression()
                        .expect("PropertyAccessExpression data")
                        .clone();
                    if let Some(question_dot) = d.question_dot_token {
                        // Even if we already have an error node, error at the `?.` token since it appears earlier.
                        error_node = Some(question_dot);
                    }
                    node = d.expression;
                    can_have_call_expression = false;
                    continue;
                }

                if !is_identifier(self.store().node(node)) {
                    // Even if we already have an error node, error at this node since it appears earlier.
                    error_node = Some(node);
                }

                break;
            }

            if let Some(error_node) = error_node {
                let mut err = self.error(
                    self.decorator_expression_of(decorator),
                    &diag::EXPRESSION_MUST_BE_ENCLOSED_IN_PARENTHESES_TO_BE_USED_AS_A_DECORATOR,
                    &[],
                );
                err.add_related_info(u::new_diagnostic_for_node(
                    self.store(),
                    error_node,
                    &diag::INVALID_SYNTAX_IN_DECORATOR,
                    vec![],
                ));
                return true;
            }
        }
        false
    }

    /// The decorator's expression (Go: `decorator.Expression` re-read after
    /// the loop walked a copy of the variable).
    fn decorator_expression_of(&self, decorator: NodeId) -> NodeId {
        self.store()
            .node(decorator)
            .as_decorator()
            .map(|d| d.expression)
            .expect("Decorator expression")
    }

    /// Go: `func (c *Checker) checkGrammarExportDeclaration(node) bool`.
    pub(crate) fn check_grammar_export_declaration(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_export_declaration()
            .expect("ExportDeclaration data");
        if d.is_type_only
            && d.export_clause.is_some_and(|c| self.store().node(c).kind == Kind::NamedExports)
        {
            return self.check_grammar_type_only_named_imports_or_exports(d.export_clause.expect("checked"));
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarModuleElementContext(node, errorMessage) bool`.
    pub(crate) fn check_grammar_module_element_context(
        &mut self,
        node: NodeId,
        error_message: &'static Message,
    ) -> bool {
        let parent_kind = self.store().node(self.store().node(node).parent.get()).kind;
        let is_in_appropriate_context = matches!(
            parent_kind,
            Kind::SourceFile | Kind::ModuleBlock | Kind::ModuleDeclaration
        );
        if !is_in_appropriate_context {
            self.grammar_error_on_first_token(node, error_message, &[]);
        }
        !is_in_appropriate_context
    }
}

// find_ancestor_or_class — Go: ast.GetContainingClass (FindAncestor(node.Parent, IsClassLike)).
fn find_ancestor_or_class(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut current = store.node(node).parent.get();
    while current != NodeId::NONE {
        if is_class_like(store, current) {
            return Some(current);
        }
        current = store.node(current).parent.get();
    }
    None
}

/// Go: `type DeclarationMeaning uint32` + consts (checker.go:327-336).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationMeaning(u32);

impl DeclarationMeaning {
    pub(crate) const NONE: DeclarationMeaning = DeclarationMeaning(0);
    pub(crate) const GET_ACCESSOR: DeclarationMeaning = DeclarationMeaning(1 << 0);
    pub(crate) const SET_ACCESSOR: DeclarationMeaning = DeclarationMeaning(1 << 1);
    pub(crate) const PROPERTY_ASSIGNMENT: DeclarationMeaning = DeclarationMeaning(1 << 2);
    pub(crate) const METHOD: DeclarationMeaning = DeclarationMeaning(1 << 3);
    // Go defines PrivateStatic and PropertyAssignmentOrMethod alongside these
    // (checker.go:327-336); neither is read by any checker path (Go or the
    // port) — kept for enum fidelity, allowed dead.
    #[allow(dead_code)]
    pub(crate) const PRIVATE_STATIC: DeclarationMeaning = DeclarationMeaning(1 << 4);
    pub(crate) const GET_OR_SET_ACCESSOR: DeclarationMeaning =
        DeclarationMeaning((1 << 0) | (1 << 1));
    #[allow(dead_code)]
    pub(crate) const PROPERTY_ASSIGNMENT_OR_METHOD: DeclarationMeaning =
        DeclarationMeaning((1 << 2) | (1 << 3));

    fn intersects(self, other: DeclarationMeaning) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for DeclarationMeaning {
    type Output = DeclarationMeaning;
    fn bitor(self, rhs: DeclarationMeaning) -> DeclarationMeaning {
        DeclarationMeaning(self.0 | rhs.0)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Accessors → import-attribute grammar (grammarchecks.go:1289-2266)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkGrammarAccessor(accessor) bool`.
    pub(crate) fn check_grammar_accessor(&mut self, node: NodeId) -> bool {
        let body = body_of(self.store(), node);
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        if !self.store().node(node).flags.intersects(NF::AMBIENT)
            && parent_kind != Kind::TypeLiteral
            && parent_kind != Kind::InterfaceDeclaration
            && body.is_none()
            && !has_syntactic_modifier(self.store().node(node), ModifierFlags::ABSTRACT)
        {
            let node_end = self.store().node(node).loc.end();
            return self.grammar_error_at_pos(node, node_end - 1, 1, &diag::X_0_EXPECTED, &["{".to_string()]);
        }
        if let Some(body) = body {
            if has_syntactic_modifier(self.store().node(node), ModifierFlags::ABSTRACT) {
                return self.grammar_error_on_node(
                    node,
                    &diag::AN_ABSTRACT_ACCESSOR_CANNOT_HAVE_AN_IMPLEMENTATION,
                    &[],
                );
            }
            if parent_kind == Kind::TypeLiteral || parent_kind == Kind::InterfaceDeclaration {
                return self.grammar_error_on_node(
                    body,
                    &diag::AN_IMPLEMENTATION_CANNOT_BE_DECLARED_IN_AMBIENT_CONTEXTS,
                    &[],
                );
            }
        }

        let (type_parameters, kind, name, type_, parameters) = {
            let n = self.store().node(node);
            match n.kind {
                Kind::GetAccessor => {
                    let d = n.as_get_accessor_declaration().expect("GetAccessor data");
                    (d.type_parameters.clone(), n.kind, Some(d.name), d.type_, d.parameters.clone())
                }
                Kind::SetAccessor => {
                    let d = n.as_set_accessor_declaration().expect("SetAccessor data");
                    (d.type_parameters.clone(), n.kind, Some(d.name), d.type_, d.parameters.clone())
                }
                _ => (None, n.kind, None, None, None),
            }
        };

        if type_parameters.is_some() {
            return self.grammar_error_on_node(
                name.expect("accessor name"),
                &diag::AN_ACCESSOR_CANNOT_HAVE_TYPE_PARAMETERS,
                &[],
            );
        }
        if !self.does_accessor_have_correct_parameter_count(node) {
            let message = if kind == Kind::GetAccessor {
                &diag::A_GET_ACCESSOR_CANNOT_HAVE_PARAMETERS
            } else {
                &diag::A_SET_ACCESSOR_MUST_HAVE_EXACTLY_ONE_PARAMETER
            };
            return self.grammar_error_on_node(name.expect("accessor name"), message, &[]);
        }
        if kind == Kind::SetAccessor {
            if type_.is_some() {
                return self.grammar_error_on_node(
                    name.expect("accessor name"),
                    &diag::A_SET_ACCESSOR_CANNOT_HAVE_A_RETURN_TYPE_ANNOTATION,
                    &[],
                );
            }

            let parameter_node = u::get_set_accessor_value_parameter(self.store(), node);
            let Some(parameter_node) = parameter_node else {
                panic!("Return value does not match parameter count assertion.");
            };
            let parameter = self
                .store()
                .node(parameter_node)
                .as_parameter_declaration()
                .expect("ParameterDeclaration data")
                .clone();
            if let Some(dot_dot_dot_token) = parameter.dot_dot_dot_token {
                return self.grammar_error_on_node(
                    dot_dot_dot_token,
                    &diag::A_SET_ACCESSOR_CANNOT_HAVE_REST_PARAMETER,
                    &[],
                );
            }
            if let Some(question_token) = parameter.question_token {
                return self.grammar_error_on_node(
                    question_token,
                    &diag::A_SET_ACCESSOR_CANNOT_HAVE_AN_OPTIONAL_PARAMETER,
                    &[],
                );
            }
            if parameter.initializer.is_some() {
                return self.grammar_error_on_node(
                    name.expect("accessor name"),
                    &diag::A_SET_ACCESSOR_PARAMETER_CANNOT_HAVE_AN_INITIALIZER,
                    &[],
                );
            }
        }
        let _ = parameters;

        false
    }

    /// Go: `func (c *Checker) doesAccessorHaveCorrectParameterCount(accessor) bool`.
    ///
    /// A `get` accessor has no parameters or a single `this` parameter.
    /// A `set` accessor has one parameter or a `this` parameter and one more.
    fn does_accessor_have_correct_parameter_count(&mut self, accessor: NodeId) -> bool {
        // `getAccessorThisParameter` returns `nil` if the accessor's arity is
        // incorrect, even if there is a `this` parameter declared.
        let expected: usize = if self.store().node(accessor).kind == Kind::GetAccessor {
            0
        } else {
            1
        };
        self.get_accessor_this_parameter(accessor).is_some()
            || parameters_of(self.store(), accessor).len() == expected
    }

    /// Go: `func (c *Checker) checkGrammarTypeOperatorNode(node) bool`.
    pub(crate) fn check_grammar_type_operator_node(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_type_operator_node()
            .expect("TypeOperatorNode data")
            .clone();
        if d.operator == Kind::UniqueKeyword {
            let inner_type = d.type_;
            if self.store().node(inner_type).kind != Kind::SymbolKeyword {
                return self.grammar_error_on_node(
                    inner_type,
                    &diag::X_0_EXPECTED,
                    &[token_to_string(Kind::SymbolKeyword).to_string()],
                );
            }
            let parent = walk_up_parenthesized_types(self.store(), self.store().node(node).parent.get());
            match self.store().node(parent).kind {
                Kind::VariableDeclaration => {
                    let decl = self
                        .store()
                        .node(parent)
                        .as_variable_declaration()
                        .expect("VariableDeclaration data")
                        .clone();
                    if self.store().node(decl.name).kind != Kind::Identifier {
                        return self.grammar_error_on_node(
                            node,
                            &diag::X_UNIQUE_SYMBOL_TYPES_MAY_NOT_BE_USED_ON_A_VARIABLE_DECLARATION_WITH_A_BINDING_NAME,
                            &[],
                        );
                    }
                    if !u::is_variable_declaration_in_variable_statement(self.store(), parent) {
                        return self.grammar_error_on_node(
                            node,
                            &diag::X_UNIQUE_SYMBOL_TYPES_ARE_ONLY_ALLOWED_ON_VARIABLES_IN_A_VARIABLE_STATEMENT,
                            &[],
                        );
                    }
                    if !self
                        .store()
                        .node(self.store().node(parent).parent.get())
                        .flags
                        .intersects(NF::CONST)
                    {
                        return self.grammar_error_on_node(
                            decl.name,
                            &diag::A_VARIABLE_WHOSE_TYPE_IS_A_UNIQUE_SYMBOL_TYPE_MUST_BE_CONST,
                            &[],
                        );
                    }
                }
                Kind::PropertyDeclaration => {
                    if !u::is_static(self.store(), self.store().node(parent))
                        || !u::has_readonly_modifier(self.store().node(parent))
                    {
                        let name = self
                            .store()
                            .node(parent)
                            .as_property_declaration()
                            .map(|d| d.name)
                            .expect("PropertyDeclaration name");
                        return self.grammar_error_on_node(
                            name,
                            &diag::A_PROPERTY_OF_A_CLASS_WHOSE_TYPE_IS_A_UNIQUE_SYMBOL_TYPE_MUST_BE_BOTH_STATIC_AND_READONLY,
                            &[],
                        );
                    }
                }
                Kind::PropertySignature => {
                    if !has_syntactic_modifier(self.store().node(parent), ModifierFlags::READONLY) {
                        let name = self
                            .store()
                            .node(parent)
                            .as_property_signature_declaration()
                            .map(|d| d.name)
                            .expect("PropertySignature name");
                        return self.grammar_error_on_node(
                            name,
                            &diag::A_PROPERTY_OF_AN_INTERFACE_OR_TYPE_LITERAL_WHOSE_TYPE_IS_A_UNIQUE_SYMBOL_TYPE_MUST_BE_READONLY,
                            &[],
                        );
                    }
                }
                _ => {
                    return self.grammar_error_on_node(
                        node,
                        &diag::X_UNIQUE_SYMBOL_TYPES_ARE_NOT_ALLOWED_HERE,
                        &[],
                    );
                }
            }
        } else if d.operator == Kind::ReadonlyKeyword {
            let inner_type = d.type_;
            if !matches!(
                self.store().node(inner_type).kind,
                Kind::ArrayType | Kind::TupleType
            ) {
                return self.grammar_error_on_first_token(
                    node,
                    &diag::X_READONLY_TYPE_MODIFIER_IS_ONLY_PERMITTED_ON_ARRAY_AND_TUPLE_LITERAL_TYPES,
                    &[token_to_string(Kind::SymbolKeyword).to_string()],
                );
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarForInvalidDynamicName(node, message) bool`.
    fn check_grammar_for_invalid_dynamic_name(
        &mut self,
        node: Option<NodeId>,
        message: &'static Message,
    ) -> bool {
        let Some(node) = node else {
            return false;
        };
        if !self.is_non_bindable_dynamic_name(node) {
            return false;
        }
        let expression = if self.store().node(node).kind == Kind::ElementAccessExpression {
            let argument = self
                .store()
                .node(node)
                .as_element_access_expression()
                .map(|d| d.argument_expression);
            argument.map(|argument| tsc_ast::utilities::skip_parentheses(self.store(), argument))
        } else {
            crate::ast_shims::expression_of(self.store(), node)
        };

        if !expression.is_some_and(|e| tsc_ast::utilities::is_entity_name_expression(self.store(), e)) {
            return self.grammar_error_on_node(node, message, &[]);
        }

        false
    }

    /// Go: `func (c *Checker) isNonBindableDynamicName(node) bool` — a
    /// dynamic name that cannot be late-bound. pub(crate) for the
    /// override-modifier 4127/4122 gate (FCS tranche 47).
    pub(crate) fn is_non_bindable_dynamic_name(&mut self, node: NodeId) -> bool {
        is_dynamic_name(self.store(), node) && !self.is_late_bindable_name(node)
    }

    /// Go: `func (c *Checker) checkGrammarMethod(node) bool`.
    pub(crate) fn check_grammar_method(&mut self, node: NodeId) -> bool {
        if self.check_grammar_function_like_declaration(node) {
            return true;
        }

        if self.store().node(node).kind == Kind::MethodDeclaration {
            let parent_kind = self.store().node(self.store().node(node).parent.get()).kind;
            if parent_kind == Kind::ObjectLiteralExpression {
                // We only disallow modifier on a method declaration if it is a property of object-literal-expression
                let modifiers = self.store().node(node).modifiers();
                let single_async = modifiers.is_some_and(|m| {
                    m.nodes.len() == 1 && self.store().node(m.nodes[0]).kind == Kind::AsyncKeyword
                });
                if modifiers.is_some() && !single_async {
                    return self
                        .grammar_error_on_first_token(node, &diag::MODIFIERS_CANNOT_APPEAR_HERE, &[]);
                }

                let method_decl = self
                    .store()
                    .node(node)
                    .as_method_declaration()
                    .expect("MethodDeclaration data")
                    .clone();
                if self.check_grammar_for_invalid_question_mark(
                    method_decl.postfix_token,
                    &diag::AN_OBJECT_MEMBER_CANNOT_BE_DECLARED_OPTIONAL,
                ) {
                    return true;
                }
                if self.check_grammar_for_invalid_exclamation_token(
                    method_decl.postfix_token,
                    &diag::A_DEFINITE_ASSIGNMENT_ASSERTION_IS_NOT_PERMITTED_IN_THIS_CONTEXT,
                ) {
                    return true;
                }
                if method_decl.body.is_none() {
                    let node_end = self.store().node(node).loc.end();
                    return self.grammar_error_at_pos(
                        node,
                        node_end - 1,
                        1,
                        &diag::X_0_EXPECTED,
                        &["{".to_string()],
                    );
                }
            }
            if self.check_grammar_for_generator(node) {
                return true;
            }
        }

        let parent = self.store().node(node).parent.get();
        if is_class_like(self.store(), parent) {
            // Technically, computed properties in ambient contexts is disallowed
            // for property declarations and accessors too, not just methods.
            // However, property declarations disallow computed names in general,
            // and accessors are not allowed in ambient contexts in general,
            // so this error only really matters for methods.
            if self.store().node(node).flags.intersects(NF::AMBIENT) {
                return self.check_grammar_for_invalid_dynamic_name(
                    self.store().node(node).name(),
                    &diag::A_COMPUTED_PROPERTY_NAME_IN_AN_AMBIENT_CONTEXT_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
                );
            } else if self.store().node(node).kind == Kind::MethodDeclaration
                && body_of(self.store(), node).is_none()
            {
                return self.check_grammar_for_invalid_dynamic_name(
                    self.store().node(node).name(),
                    &diag::A_COMPUTED_PROPERTY_NAME_IN_A_METHOD_OVERLOAD_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
                );
            }
        } else if self.store().node(parent).kind == Kind::InterfaceDeclaration {
            return self.check_grammar_for_invalid_dynamic_name(
                self.store().node(node).name(),
                &diag::A_COMPUTED_PROPERTY_NAME_IN_AN_INTERFACE_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
            );
        } else if self.store().node(parent).kind == Kind::TypeLiteral {
            return self.check_grammar_for_invalid_dynamic_name(
                self.store().node(node).name(),
                &diag::A_COMPUTED_PROPERTY_NAME_IN_A_TYPE_LITERAL_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
            );
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarBreakOrContinueStatement(node) bool`.
    pub(crate) fn check_grammar_break_or_continue_statement(&mut self, node: NodeId) -> bool {
        // Go: `targetLabel := node.Label()` — Label() is the Break/Continue
        // (and Labeled) statement accessor; the generic Name() read returns
        // nil for break/continue, which made every labeled break/continue
        // climb past its label to the enclosing function and mis-fire
        // TS1107 (fixed in W3T6).
        let target_label = tsc_binder::node_data::label_of(self.store(), node);
        let mut current: Option<NodeId> = Some(node);
        while let Some(cur) = current {
            if tsc_binder::ast_shims::is_function_like_or_class_static_block_declaration(
                self.store().node(cur),
            ) {
                return self.grammar_error_on_node(
                    node,
                    &diag::JUMP_TARGET_CANNOT_CROSS_FUNCTION_BOUNDARY,
                    &[],
                );
            }

            let kind = self.store().node(cur).kind;
            if kind == Kind::LabeledStatement {
                let label = self
                    .store()
                    .node(cur)
                    .as_labeled_statement()
                    .map(|d| d.label)
                    .expect("LabeledStatement label");
                if target_label.is_some()
                    && tsc_ast::node_text(self.store(), label)
                        == tsc_ast::node_text(self.store(), target_label.expect("checked"))
                {
                    // found matching label - verify that label usage is correct
                    // continue can only target labels that are on iteration statements
                    let statement = self
                        .store()
                        .node(cur)
                        .as_labeled_statement()
                        .map(|d| d.statement)
                        .expect("LabeledStatement statement");
                    let is_misplaced_continue_label = self.store().node(node).kind
                        == Kind::ContinueStatement
                        && !is_iteration_statement(self.store(), statement, true);

                    if is_misplaced_continue_label {
                        return self.grammar_error_on_node(
                            node,
                            &diag::A_CONTINUE_STATEMENT_CAN_ONLY_JUMP_TO_A_LABEL_OF_AN_ENCLOSING_ITERATION_STATEMENT,
                            &[],
                        );
                    }

                    return false;
                }
            } else if kind == Kind::SwitchStatement {
                if self.store().node(node).kind == Kind::BreakStatement && target_label.is_none() {
                    // unlabeled break within switch statement - ok
                    return false;
                }
            } else if is_iteration_statement(self.store(), cur, false) && target_label.is_none() {
                // unlabeled break or continue within iteration statement - ok
                return false;
            }

            current = {
                let parent = self.store().node(cur).parent.get();
                if parent == NodeId::NONE {
                    None
                } else {
                    Some(parent)
                }
            };
        }

        if target_label.is_some() {
            let message = if self.store().node(node).kind == Kind::BreakStatement {
                &diag::A_BREAK_STATEMENT_CAN_ONLY_JUMP_TO_A_LABEL_OF_AN_ENCLOSING_STATEMENT
            } else {
                &diag::A_CONTINUE_STATEMENT_CAN_ONLY_JUMP_TO_A_LABEL_OF_AN_ENCLOSING_ITERATION_STATEMENT
            };
            self.grammar_error_on_node(node, message, &[])
        } else {
            let message = if self.store().node(node).kind == Kind::BreakStatement {
                &diag::A_BREAK_STATEMENT_CAN_ONLY_BE_USED_WITHIN_AN_ENCLOSING_ITERATION_OR_SWITCH_STATEMENT
            } else {
                &diag::A_CONTINUE_STATEMENT_CAN_ONLY_BE_USED_WITHIN_AN_ENCLOSING_ITERATION_STATEMENT
            };
            self.grammar_error_on_node(node, message, &[])
        }
    }

    /// Go: `func (c *Checker) checkGrammarBindingElement(node) bool`.
    pub(crate) fn check_grammar_binding_element(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_binding_element()
            .expect("BindingElement data")
            .clone();
        if d.dot_dot_dot_token.is_some() {
            let parent = self.store().node(node).parent.get();
            let elements = parent_element_list(self.store(), parent);
            if Some(node)
                != elements
                    .as_ref()
                    .and_then(|e| e.nodes.last().copied())
            {
                return self.grammar_error_on_node(
                    node,
                    &diag::A_REST_ELEMENT_MUST_BE_LAST_IN_A_DESTRUCTURING_PATTERN,
                    &[],
                );
            }
            self.check_grammar_for_disallowed_trailing_comma(
                elements.as_ref(),
                &diag::A_REST_PARAMETER_OR_BINDING_PATTERN_MAY_NOT_HAVE_A_TRAILING_COMMA,
            );

            if d.property_name.is_some() {
                return self.grammar_error_on_node(
                    d.name.expect("BindingElement name"),
                    &diag::A_REST_ELEMENT_CANNOT_HAVE_A_PROPERTY_NAME,
                    &[],
                );
            }
        }

        if d.dot_dot_dot_token.is_some()
            && let Some(initializer) = d.initializer
        {
            // Error on equals token which immediately precedes the initializer
            let initializer_pos = self.store().node(initializer).loc.pos();
            return self.grammar_error_at_pos(
                node,
                initializer_pos - 1,
                1,
                &diag::A_REST_ELEMENT_CANNOT_HAVE_AN_INITIALIZER,
                &[],
            );
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarVariableDeclaration(node) bool`.
    pub(crate) fn check_grammar_variable_declaration(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_variable_declaration()
            .expect("VariableDeclaration data")
            .clone();
        let node_flags = self.get_combined_node_flags_cached(node);
        let block_scope_kind = node_flags & NF::BLOCK_SCOPED;
        if crate::statements::is_binding_pattern(self.store(), d.name) {
            if block_scope_kind == NF::AWAIT_USING {
                return self.grammar_error_on_node(
                    node,
                    &diag::X_0_DECLARATIONS_MAY_NOT_HAVE_BINDING_PATTERNS,
                    &["await using".to_string()],
                );
            }
            if block_scope_kind == NF::USING {
                return self.grammar_error_on_node(
                    node,
                    &diag::X_0_DECLARATIONS_MAY_NOT_HAVE_BINDING_PATTERNS,
                    &["using".to_string()],
                );
            }
        }

        let parent = self.store().node(node).parent.get();
        let grandparent = self.store().node(parent).parent.get();
        if self.store().node(grandparent).kind != Kind::ForInStatement
            && self.store().node(grandparent).kind != Kind::ForOfStatement
        {
            if node_flags.intersects(NF::AMBIENT) {
                self.check_ambient_initializer(node);
            } else if d.initializer.is_none() {
                if crate::statements::is_binding_pattern(self.store(), d.name)
                    && !crate::statements::is_binding_pattern(self.store(), parent)
                {
                    return self.grammar_error_on_node(
                        node,
                        &diag::A_DESTRUCTURING_DECLARATION_MUST_HAVE_AN_INITIALIZER,
                        &[],
                    );
                }
                if block_scope_kind == NF::AWAIT_USING {
                    return self.grammar_error_on_node(
                        node,
                        &diag::X_0_DECLARATIONS_MUST_BE_INITIALIZED,
                        &["await using".to_string()],
                    );
                }
                if block_scope_kind == NF::USING {
                    return self.grammar_error_on_node(
                        node,
                        &diag::X_0_DECLARATIONS_MUST_BE_INITIALIZED,
                        &["using".to_string()],
                    );
                }
                if block_scope_kind == NF::CONST {
                    return self.grammar_error_on_node(
                        node,
                        &diag::X_0_DECLARATIONS_MUST_BE_INITIALIZED,
                        &["const".to_string()],
                    );
                }
            }
        }

        if let Some(exclamation_token) = d.exclamation_token
            && (self.store().node(grandparent).kind != Kind::VariableStatement
                || d.type_.is_none()
                || d.initializer.is_some()
                || node_flags.intersects(NF::AMBIENT))
        {
            let message = if d.initializer.is_some() {
                &diag::DECLARATIONS_WITH_INITIALIZERS_CANNOT_ALSO_HAVE_DEFINITE_ASSIGNMENT_ASSERTIONS
            } else if d.type_.is_none() {
                &diag::DECLARATIONS_WITH_DEFINITE_ASSIGNMENT_ASSERTIONS_MUST_ALSO_HAVE_TYPE_ANNOTATIONS
            } else {
                &diag::A_DEFINITE_ASSIGNMENT_ASSERTION_IS_NOT_PERMITTED_IN_THIS_CONTEXT
            };
            return self.grammar_error_on_node(exclamation_token, message, &[]);
        }

        let source_file = self.get_source_file_node_of(node);
        if self.program.get_emit_module_format_of_file(source_file) < ModuleKind::System
            && !self.store().node(grandparent).flags.intersects(NF::AMBIENT)
            && has_syntactic_modifier(self.store().node(grandparent), ModifierFlags::EXPORT)
        {
            self.check_grammar_for_es_module_marker_in_binding_name(Some(d.name));
        }

        // 1. LexicalDeclaration : LetOrConst BindingList ;
        // It is a Syntax Error if the BoundNames of BindingList contains "let".
        // 2. ForDeclaration: ForDeclaration : LetOrConst ForBinding
        // It is a Syntax Error if the BoundNames of ForDeclaration contains "let".
        block_scope_kind != NF::NONE
            && self.check_grammar_name_in_let_or_const_declarations(Some(d.name))
    }

    /// Go: `func (c *Checker) checkGrammarForEsModuleMarkerInBindingName(name) bool`.
    fn check_grammar_for_es_module_marker_in_binding_name(&mut self, name: Option<NodeId>) -> bool {
        let Some(name) = name else { return false };
        if is_identifier(self.store().node(name)) {
            if tsc_ast::node_text(self.store(), name) == "__esModule" {
                return self.grammar_error_on_node_skipped_on_no_emit(
                    name,
                    &diag::IDENTIFIER_EXPECTED_ESMODULE_IS_RESERVED_AS_AN_EXPORTED_MARKER_WHEN_TRANSFORMING_ECMASCRIPT_MODULES,
                    &[],
                );
            }
        } else {
            for element in binding_elements_of(self.store(), name) {
                if let Some(element_name) = self.store().node(element).name() {
                    return self.check_grammar_for_es_module_marker_in_binding_name(Some(element_name));
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarNameInLetOrConstDeclarations(name) bool`.
    fn check_grammar_name_in_let_or_const_declarations(&mut self, name: Option<NodeId>) -> bool {
        let Some(name) = name else { return false };
        if self.store().node(name).kind == Kind::Identifier {
            if tsc_ast::node_text(self.store(), name) == "let" {
                return self.grammar_error_on_node(
                    name,
                    &diag::X_LET_IS_NOT_ALLOWED_TO_BE_USED_AS_A_NAME_IN_LET_OR_CONST_DECLARATIONS,
                    &[],
                );
            }
        } else {
            for element in binding_elements_of(self.store(), name) {
                if let Some(element_name) = self
                    .store()
                    .node(element)
                    .as_binding_element()
                    .and_then(|d| d.name)
                {
                    self.check_grammar_name_in_let_or_const_declarations(Some(element_name));
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarVariableDeclarationList(declarationList) bool`.
    pub(crate) fn check_grammar_variable_declaration_list(&mut self, node: NodeId) -> bool {
        let declarations = self
            .store()
            .node(node)
            .as_variable_declaration_list()
            .and_then(|l| l.declarations.clone())
            .expect("VariableDeclarationList declarations");
        if self.check_grammar_for_disallowed_trailing_comma(
            Some(&declarations),
            &diag::TRAILING_COMMA_NOT_ALLOWED,
        ) {
            return true;
        }

        if declarations.nodes.is_empty() {
            let length = declarations.end() - declarations.pos();
            return self.grammar_error_at_pos(
                node,
                declarations.pos(),
                length,
                &diag::VARIABLE_DECLARATION_LIST_CANNOT_BE_EMPTY,
                &[],
            );
        }

        let list_flags = self.store().node(node).flags;
        let block_scope_flags = list_flags & NF::BLOCK_SCOPED;
        if block_scope_flags == NF::USING || block_scope_flags == NF::AWAIT_USING {
            let parent = self.store().node(node).parent.get();
            if self.store().node(parent).kind == Kind::ForInStatement {
                let message = if block_scope_flags == NF::USING {
                    &diag::THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_BE_A_USING_DECLARATION
                } else {
                    &diag::THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_BE_AN_AWAIT_USING_DECLARATION
                };
                return self.grammar_error_on_node(node, message, &[]);
            }
            if list_flags.intersects(NF::AMBIENT) {
                let message = if block_scope_flags == NF::USING {
                    &diag::X_USING_DECLARATIONS_ARE_NOT_ALLOWED_IN_AMBIENT_CONTEXTS
                } else {
                    &diag::X_AWAIT_USING_DECLARATIONS_ARE_NOT_ALLOWED_IN_AMBIENT_CONTEXTS
                };
                return self.grammar_error_on_node(node, message, &[]);
            }
            let parent = self.store().node(node).parent.get();
            let grandparent = self.store().node(parent).parent.get();
            if self.store().node(parent).kind == Kind::VariableStatement
                && (is_case_clause(self.store().node(grandparent))
                    || is_default_clause(self.store().node(grandparent)))
            {
                let message = if block_scope_flags == NF::USING {
                    &diag::X_USING_DECLARATIONS_ARE_NOT_ALLOWED_IN_CASE_OR_DEFAULT_CLAUSES_UNLESS_CONTAINED_WITHIN_A_BLOCK
                } else {
                    &diag::X_AWAIT_USING_DECLARATIONS_ARE_NOT_ALLOWED_IN_CASE_OR_DEFAULT_CLAUSES_UNLESS_CONTAINED_WITHIN_A_BLOCK
                };
                return self.grammar_error_on_node(node, message, &[]);
            }
        }

        if block_scope_flags == NF::AWAIT_USING {
            return self.check_grammar_await_or_await_using(node);
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarAwaitOrAwaitUsing(node) bool`
    /// (grammarchecks.go:1658, ~100 lines).
    pub(crate) fn check_grammar_await_or_await_using(&mut self, node: NodeId) -> bool {
        // Grammar checking
        let mut has_error = false;
        let container = u::get_containing_function_or_class_static_block(self.store(), node);
        if container.is_some_and(|c| self.store().node(c).kind == Kind::ClassStaticBlockDeclaration) {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            let message = if self.store().node(node).kind == Kind::AwaitExpression {
                &diag::X_AWAIT_EXPRESSION_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK
            } else {
                &diag::X_AWAIT_USING_STATEMENTS_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK
            };
            self.error(node, message, &[]);
            has_error = true;
        } else if !self.store().node(node).flags.intersects(NF::AWAIT_CONTEXT) {
            if is_in_top_level_context(self.store(), node) {
                let source_file = self.get_source_file_node_of(node);
                if !self.has_parse_diagnostics(source_file) {
                    let mut span_calculated = false;
                    let mut span = TextRange::default();
                    if !self
                        .source_file_of(source_file)
                        .is_some_and(|f| is_effective_external_module(f, self.module_kind))
                    {
                        span = self
                            .source_file_of(source_file)
                            .map(|f| tsc_scanner::get_range_of_token_at_position(f, self.store().node(node).loc.pos()))
                            .unwrap_or_default();
                        span_calculated = true;
                        let message = if self.store().node(node).kind == Kind::AwaitExpression {
                            &diag::X_AWAIT_EXPRESSIONS_ARE_ONLY_ALLOWED_AT_THE_TOP_LEVEL_OF_A_FILE_WHEN_THAT_FILE_IS_A_MODULE_BUT_THIS_FILE_HAS_NO_IMPORTS_OR_EXPORTS_CONSIDER_ADDING_AN_EMPTY_EXPORT_TO_MAKE_THIS_FILE_A_MODULE
                        } else {
                            &diag::X_AWAIT_USING_STATEMENTS_ARE_ONLY_ALLOWED_AT_THE_TOP_LEVEL_OF_A_FILE_WHEN_THAT_FILE_IS_A_MODULE_BUT_THIS_FILE_HAS_NO_IMPORTS_OR_EXPORTS_CONSIDER_ADDING_AN_EMPTY_EXPORT_TO_MAKE_THIS_FILE_A_MODULE
                        };
                        let diagnostic = self.new_diagnostic_for_range(source_file, span, message, vec![]);
                        self.add_diagnostic(diagnostic);
                        has_error = true;
                    }
                    // Go: the module-kind fallthrough (shared with the
                    // for-await arm).
                    let is_node_kind =
                        ModuleKind::Node16 <= self.module_kind && self.module_kind <= ModuleKind::NodeNext;
                    let is_es2022_like = matches!(
                        self.module_kind,
                        ModuleKind::ES2022
                            | ModuleKind::ESNext
                            | ModuleKind::Preserve
                            | ModuleKind::System
                    );
                    if is_node_kind
                        && self.program.source_file_implied_node_format(source_file) == ModuleKind::CommonJS
                    {
                        if !span_calculated {
                            span = self
                                .source_file_of(source_file)
                                .map(|f| tsc_scanner::get_range_of_token_at_position(f, self.store().node(node).loc.pos()))
                                .unwrap_or_default();
                        }
                        let diagnostic = self.new_diagnostic_for_range(
                            source_file,
                            span,
                            &diag::THE_CURRENT_FILE_IS_A_COMMONJS_MODULE_AND_CANNOT_USE_AWAIT_AT_THE_TOP_LEVEL,
                            vec![],
                        );
                        self.add_diagnostic(diagnostic);
                        has_error = true;
                    } else if !(is_node_kind || is_es2022_like && self.language_version >= ScriptTarget::ES2017) {
                        if !span_calculated {
                            span = self
                                .source_file_of(source_file)
                                .map(|f| tsc_scanner::get_range_of_token_at_position(f, self.store().node(node).loc.pos()))
                                .unwrap_or_default();
                        }
                        let message = if self.store().node(node).kind == Kind::AwaitExpression {
                            &diag::TOP_LEVEL_AWAIT_EXPRESSIONS_ARE_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE_AND_THE_TARGET_OPTION_IS_SET_TO_ES2017_OR_HIGHER
                        } else {
                            &diag::TOP_LEVEL_AWAIT_USING_STATEMENTS_ARE_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE_AND_THE_TARGET_OPTION_IS_SET_TO_ES2017_OR_HIGHER
                        };
                        let diagnostic = self.new_diagnostic_for_range(source_file, span, message, vec![]);
                        self.add_diagnostic(diagnostic);
                        has_error = true;
                    }
                }
            } else {
                // use of 'await' in non-async function
                let source_file = self.get_source_file_node_of(node);
                if !self.has_parse_diagnostics(source_file) {
                    let span = self
                        .source_file_of(source_file)
                        .map(|f| tsc_scanner::get_range_of_token_at_position(f, self.store().node(node).loc.pos()))
                        .unwrap_or_default();
                    let message = if self.store().node(node).kind == Kind::AwaitExpression {
                        &diag::X_AWAIT_EXPRESSIONS_ARE_ONLY_ALLOWED_WITHIN_ASYNC_FUNCTIONS_AND_AT_THE_TOP_LEVELS_OF_MODULES
                    } else {
                        &diag::X_AWAIT_USING_STATEMENTS_ARE_ONLY_ALLOWED_WITHIN_ASYNC_FUNCTIONS_AND_AT_THE_TOP_LEVELS_OF_MODULES
                    };
                    let mut diagnostic =
                        self.new_diagnostic_for_range(source_file, span, message, vec![]);
                    if let Some(container) = container
                        && self.store().node(container).kind != Kind::Constructor
                        && !u::has_async_modifier(self.store().node(container))
                    {
                        let related_info = u::new_diagnostic_for_node(
                            self.store(),
                            container,
                            &diag::DID_YOU_MEAN_TO_MARK_THIS_FUNCTION_AS_ASYNC,
                            vec![],
                        );
                        diagnostic.add_related_info(related_info);
                    }
                    self.add_diagnostic(diagnostic);
                    has_error = true;
                }
            }
        }

        if self.store().node(node).kind == Kind::AwaitExpression
            && self.is_in_parameter_initializer_before_containing_function(node)
        {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            self.error(node, &diag::X_AWAIT_EXPRESSIONS_CANNOT_BE_USED_IN_A_PARAMETER_INITIALIZER, &[]);
            has_error = true;
        }

        has_error
    }

    /// Go: `func (c *Checker) checkGrammarYieldExpression(node) bool`.
    pub(crate) fn check_grammar_yield_expression(&mut self, node: NodeId) -> bool {
        let mut has_error = false;
        if !self.store().node(node).flags.intersects(NF::YIELD_CONTEXT) {
            self.grammar_error_on_first_token(
                node,
                &diag::A_YIELD_EXPRESSION_IS_ONLY_ALLOWED_IN_A_GENERATOR_BODY,
                &[],
            );
            has_error = true;
        }
        if self.is_in_parameter_initializer_before_containing_function(node) {
            self.error(
                node,
                &diag::X_YIELD_EXPRESSIONS_CANNOT_BE_USED_IN_A_PARAMETER_INITIALIZER,
                &[],
            );
            has_error = true;
        }
        has_error
    }

    /// Go: `func (c *Checker) checkGrammarForDisallowedBlockScopedVariableStatement(node) bool`.
    pub(crate) fn check_grammar_for_disallowed_block_scoped_variable_statement(
        &mut self,
        node: NodeId,
    ) -> bool {
        let parent = self.store().node(node).parent.get();
        if !self.container_allows_block_scoped_variable(Some(parent)) {
            let declaration_list = self
                .store()
                .node(node)
                .as_variable_statement()
                .map(|d| d.declaration_list)
                .expect("VariableStatement data");
            let block_scope_kind = self.get_combined_node_flags_cached(declaration_list) & NF::BLOCK_SCOPED;
            if block_scope_kind != NF::NONE {
                let keyword = if block_scope_kind == NF::LET {
                    "let"
                } else if block_scope_kind == NF::CONST {
                    "const"
                } else if block_scope_kind == NF::USING {
                    "using"
                } else if block_scope_kind == NF::AWAIT_USING {
                    "await using"
                } else {
                    panic!("Unknown BlockScope flag")
                };
                self.error(
                    node,
                    &diag::X_0_DECLARATIONS_CAN_ONLY_BE_DECLARED_INSIDE_A_BLOCK,
                    &[keyword.to_string()],
                );
            }
        }

        false
    }

    /// Go: `func (c *Checker) containerAllowsBlockScopedVariable(parent) bool`.
    fn container_allows_block_scoped_variable(&self, parent: Option<NodeId>) -> bool {
        let Some(parent) = parent else {
            return true;
        };
        match self.store().node(parent).kind {
            Kind::IfStatement
            | Kind::DoStatement
            | Kind::WhileStatement
            | Kind::WithStatement
            | Kind::ForStatement
            | Kind::ForInStatement
            | Kind::ForOfStatement => false,
            Kind::LabeledStatement => {
                let grandparent = self.store().node(parent).parent.get();
                self.container_allows_block_scoped_variable(Some(grandparent))
            }
            _ => true,
        }
    }

    /// Go: `func (c *Checker) checkGrammarMetaProperty(node) bool`.
    pub(crate) fn check_grammar_meta_property(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_meta_property()
            .expect("MetaProperty data")
            .clone();
        let node_name = d.name;
        let name_text = tsc_ast::node_text(self.store(), node_name).to_string();

        match d.keyword_token {
            Kind::NewKeyword => {
                if name_text != "target" {
                    return self.grammar_error_on_node(
                        node_name,
                        &diag::X_0_IS_NOT_A_VALID_META_PROPERTY_FOR_KEYWORD_1_DID_YOU_MEAN_2,
                        &[name_text, token_to_string(d.keyword_token).to_string(), "target".to_string()],
                    );
                }
            }
            Kind::ImportKeyword if name_text != "meta" => {
                let parent = self.store().node(node).parent.get();
                let is_callee = parent != NodeId::NONE
                    && self.store().node(parent).kind == Kind::CallExpression
                    && crate::ast_shims::expression_of(self.store(), parent) == Some(node);
                if is_import_phase_meta_property(self.store(), node) {
                    if !is_callee {
                        let node_end = self.store().node(node).loc.end();
                        return self.grammar_error_at_pos(
                            node,
                            node_end,
                            0,
                            &diag::X_0_EXPECTED,
                            &["(".to_string()],
                        );
                    }
                } else {
                    if is_callee {
                        return self.grammar_error_on_node(
                            node_name,
                            &diag::X_0_IS_NOT_A_VALID_META_PROPERTY_FOR_KEYWORD_IMPORT_DID_YOU_MEAN_META_DEFER_OR_SOURCE,
                            &[name_text],
                        );
                    }
                    return self.grammar_error_on_node(
                        node_name,
                        &diag::X_0_IS_NOT_A_VALID_META_PROPERTY_FOR_KEYWORD_1_DID_YOU_MEAN_2,
                        &[name_text, token_to_string(d.keyword_token).to_string(), "meta".to_string()],
                    );
                }
            }
            _ => {}
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarConstructorTypeParameters(node) bool`.
    pub(crate) fn check_grammar_constructor_type_parameters(&mut self, node: NodeId) -> bool {
        let range_ = self
            .store()
            .node(node)
            .as_constructor_declaration()
            .and_then(|d| d.type_parameters.clone());
        if let Some(range_) = range_ {
            let pos = if range_.pos() == range_.end() {
                range_.pos()
            } else {
                let text = self
                    .source_file_of(self.get_source_file_node_of(node))
                    .map(|f| f.text().to_string())
                    .unwrap_or_default();
                tsc_scanner::skip_trivia(&text, range_.pos())
            };
            let length = range_.end() - pos;
            return self.grammar_error_at_pos(
                node,
                pos,
                length,
                &diag::TYPE_PARAMETERS_CANNOT_APPEAR_ON_A_CONSTRUCTOR_DECLARATION,
                &[],
            );
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarConstructorTypeAnnotation(node) bool`.
    pub(crate) fn check_grammar_constructor_type_annotation(&mut self, node: NodeId) -> bool {
        let t = self
            .store()
            .node(node)
            .as_constructor_declaration()
            .and_then(|d| d.type_);
        if let Some(t) = t {
            return self.grammar_error_on_node(
                t,
                &diag::TYPE_ANNOTATION_CANNOT_APPEAR_ON_A_CONSTRUCTOR_DECLARATION,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarProperty(node) bool` (PropertyDeclaration |
    /// PropertySignature).
    pub(crate) fn check_grammar_property(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        let property_name = self.store().node(node).name();
        if let Some(name) = property_name {
            // Go: IsComputedPropertyName(name) && IsBinaryExpression(name.Expression())
            // && operator is `in` — a mapped-type member.
            let is_mapped_member = self.store().node(name).kind == Kind::ComputedPropertyName
                && self
                    .store()
                    .node(name)
                    .as_computed_property_name()
                    .is_some_and(|cp| {
                        self.store().node(cp.expression).kind == Kind::BinaryExpression
                            && self
                                .store()
                                .node(cp.expression)
                                .as_binary_expression()
                                .is_some_and(|b| {
                                    self.store().node(b.operator_token).kind == Kind::InKeyword
                                })
                    });
            if is_mapped_member {
                let parent = self.store().node(node).parent.get();
                let first_member = crate::statements::members_of(self.store(), parent)
                    .first()
                    .copied();
                if let Some(first_member) = first_member {
                    return self.grammar_error_on_node(
                        first_member,
                        &diag::A_MAPPED_TYPE_MAY_NOT_DECLARE_PROPERTIES_OR_METHODS,
                        &[],
                    );
                }
                // Go: node.Parent.Members()[0] — the mapped-type container's
                // first member; when the container read fails the arm falls
                // through (the parser guarantees a member in practice).
            }
        }
        let parent = self.store().node(node).parent.get();
        if is_class_like(self.store(), parent) {
            if let Some(name) = property_name
                && tsc_ast::is_string_literal(self.store().node(name))
                && tsc_ast::node_text(self.store(), name) == "constructor"
            {
                return self.grammar_error_on_node(
                    name,
                    &diag::CLASSES_MAY_NOT_HAVE_A_FIELD_NAMED_CONSTRUCTOR,
                    &[],
                );
            }
            if self.check_grammar_for_invalid_dynamic_name(
                property_name,
                &diag::A_COMPUTED_PROPERTY_NAME_IN_A_CLASS_PROPERTY_DECLARATION_MUST_HAVE_A_SIMPLE_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
            ) {
                return true;
            }
            let is_auto_accessor = kind == Kind::PropertyDeclaration
                && has_syntactic_modifier(self.store().node(node), ModifierFlags::ACCESSOR);
            if is_auto_accessor
                && self.check_grammar_for_invalid_question_mark(
                    postfix_token_of(self.store(), node),
                    &diag::AN_ACCESSOR_PROPERTY_CANNOT_BE_DECLARED_OPTIONAL,
                )
            {
                return true;
            }
        } else if self.store().node(parent).kind == Kind::InterfaceDeclaration {
            if self.check_grammar_for_invalid_dynamic_name(
                self.store().node(node).name(),
                &diag::A_COMPUTED_PROPERTY_NAME_IN_AN_INTERFACE_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
            ) {
                return true;
            }
            if kind != Kind::PropertySignature {
                // Interfaces cannot contain property declarations
                panic!("Unexpected node kind {:?}", kind);
            }
            if let Some(initializer) = initializer_of(self.store(), node) {
                return self.grammar_error_on_node(
                    initializer,
                    &diag::AN_INTERFACE_PROPERTY_CANNOT_HAVE_AN_INITIALIZER,
                    &[],
                );
            }
        } else if self.store().node(parent).kind == Kind::TypeLiteral {
            if self.check_grammar_for_invalid_dynamic_name(
                self.store().node(node).name(),
                &diag::A_COMPUTED_PROPERTY_NAME_IN_A_TYPE_LITERAL_MUST_REFER_TO_AN_EXPRESSION_WHOSE_TYPE_IS_A_LITERAL_TYPE_OR_A_UNIQUE_SYMBOL_TYPE,
            ) {
                return true;
            }
            if kind != Kind::PropertySignature {
                // Type literals cannot contain property declarations
                panic!("Unexpected node kind {:?}", kind);
            }
            if let Some(initializer) = initializer_of(self.store(), node) {
                return self.grammar_error_on_node(
                    initializer,
                    &diag::A_TYPE_LITERAL_PROPERTY_CANNOT_HAVE_AN_INITIALIZER,
                    &[],
                );
            }
        }

        if self.store().node(node).flags.intersects(NF::AMBIENT) {
            self.check_ambient_initializer(node);
        }

        if self.store().node(node).kind == Kind::PropertyDeclaration {
            let prop_decl = self
                .store()
                .node(node)
                .as_property_declaration()
                .expect("PropertyDeclaration data")
                .clone();
            let postfix_token = prop_decl.postfix_token;
            if let Some(postfix_token) = postfix_token
                && self.store().node(postfix_token).kind == Kind::ExclamationToken
            {
                if prop_decl.initializer.is_some() {
                    return self.grammar_error_on_node(
                        postfix_token,
                        &diag::DECLARATIONS_WITH_INITIALIZERS_CANNOT_ALSO_HAVE_DEFINITE_ASSIGNMENT_ASSERTIONS,
                        &[],
                    );
                } else if prop_decl.type_.is_none() {
                    return self.grammar_error_on_node(
                        postfix_token,
                        &diag::DECLARATIONS_WITH_DEFINITE_ASSIGNMENT_ASSERTIONS_MUST_ALSO_HAVE_TYPE_ANNOTATIONS,
                        &[],
                    );
                } else if !is_class_like(self.store(), parent)
                    || self.store().node(node).flags.intersects(NF::AMBIENT)
                    || u::is_static(self.store(), self.store().node(node))
                    || has_abstract_modifier(self.store().node(node))
                {
                    return self.grammar_error_on_node(
                        postfix_token,
                        &diag::A_DEFINITE_ASSIGNMENT_ASSERTION_IS_NOT_PERMITTED_IN_THIS_CONTEXT,
                        &[],
                    );
                }
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkAmbientInitializer(node) bool`.
    pub(crate) fn check_ambient_initializer(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        let (initializer, type_node) = match kind {
            Kind::VariableDeclaration => {
                let d = self
                    .store()
                    .node(node)
                    .as_variable_declaration()
                    .expect("VariableDeclaration data");
                (d.initializer, d.type_)
            }
            Kind::PropertyDeclaration => {
                let d = self
                    .store()
                    .node(node)
                    .as_property_declaration()
                    .expect("PropertyDeclaration data");
                (d.initializer, d.type_)
            }
            Kind::PropertySignature => {
                let d = self
                    .store()
                    .node(node)
                    .as_property_signature_declaration()
                    .expect("PropertySignature data");
                (
                    (d.initializer != NodeId::NONE).then_some(d.initializer),
                    (d.type_ != NodeId::NONE).then_some(d.type_),
                )
            }
            _ => panic!("Unexpected node kind {:?}", kind),
        };

        if let Some(initializer) = initializer {
            let is_invalid_initializer = !(is_initializer_string_or_number_literal_expression(
                self.store(),
                initializer,
            ) || self.is_initializer_simple_literal_enum_reference(initializer)
                || self.store().node(initializer).kind == Kind::TrueKeyword
                || self.store().node(initializer).kind == Kind::FalseKeyword
                || is_initializer_big_int_literal_expression(self.store(), initializer));
            let is_const_or_readonly = u::is_declaration_readonly(self.store(), node)
                || (kind == Kind::VariableDeclaration && self.is_var_const_like(node));
            if is_const_or_readonly && type_node.is_none() {
                if is_invalid_initializer {
                    return self.grammar_error_on_node(
                        initializer,
                        &diag::A_CONST_INITIALIZER_IN_AN_AMBIENT_CONTEXT_MUST_BE_A_STRING_OR_NUMERIC_LITERAL_OR_LITERAL_ENUM_REFERENCE,
                        &[],
                    );
                }
            } else {
                return self.grammar_error_on_node(
                    initializer,
                    &diag::INITIALIZERS_ARE_NOT_ALLOWED_IN_AMBIENT_CONTEXTS,
                    &[],
                );
            }
        }

        false
    }

    /// Go: `func (c *Checker) isInitializerSimpleLiteralEnumReference(expr) bool`.
    fn is_initializer_simple_literal_enum_reference(&mut self, expr: NodeId) -> bool {
        if self.store().node(expr).kind == Kind::PropertyAccessExpression {
            let t = self.check_expression_cached(expr);
            return self.type_(t).flags.intersects(crate::types::TypeFlags::ENUM_LIKE);
        }

        if self.store().node(expr).kind == Kind::ElementAccessExpression {
            let element_access = self
                .store()
                .node(expr)
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .clone();
            let t = self.check_expression_cached(expr);
            return is_initializer_string_or_number_literal_expression(
                self.store(),
                element_access.argument_expression,
            ) && tsc_ast::utilities::is_entity_name_expression(
                self.store(),
                element_access.expression,
            ) && self
                .type_(t)
                .flags
                .intersects(crate::types::TypeFlags::ENUM_LIKE);
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarTopLevelElementForRequiredDeclareModifier(node) bool`.
    fn check_grammar_top_level_element_for_required_declare_modifier(&mut self, node: NodeId) -> bool {
        // A declare modifier is required for any top level .d.ts declaration except export=, export default, export as namespace
        // interfaces and imports categories (Go's comment).
        let kind = self.store().node(node).kind;
        if kind == Kind::InterfaceDeclaration
            || kind == Kind::TypeAliasDeclaration
            || kind == Kind::ImportDeclaration
            || kind == Kind::JSImportDeclaration
            || kind == Kind::ImportEqualsDeclaration
            || kind == Kind::ExportDeclaration
            || kind == Kind::ExportAssignment
            || kind == Kind::NamespaceExportDeclaration
            || has_syntactic_modifier(
                self.store().node(node),
                ModifierFlags::AMBIENT | ModifierFlags::EXPORT | ModifierFlags::DEFAULT,
            )
        {
            return false;
        }

        self.grammar_error_on_first_token(
            node,
            &diag::TOP_LEVEL_DECLARATIONS_IN_D_TS_FILES_MUST_START_WITH_EITHER_A_DECLARE_OR_EXPORT_MODIFIER,
            &[],
        )
    }

    /// Go: `func (c *Checker) checkGrammarTopLevelElementsForRequiredDeclareModifier(file) bool`.
    fn check_grammar_top_level_elements_for_required_declare_modifier(
        &mut self,
        file: NodeId,
    ) -> bool {
        let statements = self
            .source_file_of(file)
            .and_then(|f| {
                f.node(f.as_node_id())
                    .as_source_file()
                    .and_then(|d| d.statements.as_ref().map(|l| l.nodes.to_vec()))
            })
            .unwrap_or_default();
        for decl in statements {
            // Go: ast.IsDeclarationNode — DeclarationData() != nil; on
            // statements this is exactly the declaration-statement kind set
            // (the binder's port).
            let is_declaration_node =
                tsc_binder::ast_shims::is_declaration_statement_kind(self.store().node(decl).kind)
                    || self.store().node(decl).kind == Kind::VariableStatement;
            if is_declaration_node && self.check_grammar_top_level_element_for_required_declare_modifier(decl) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarSourceFile(node) bool`.
    pub(crate) fn check_grammar_source_file(&mut self, source_file: NodeId) -> bool {
        self.store().node(source_file).flags.intersects(NF::AMBIENT)
            && self.check_grammar_top_level_elements_for_required_declare_modifier(source_file)
    }

    /// Go: `func (c *Checker) checkGrammarStatementInAmbientContext(node) bool`.
    pub(crate) fn check_grammar_statement_in_ambient_context(&mut self, node: NodeId) -> bool {
        if self.store().node(node).flags.intersects(NF::AMBIENT) {
            // Find containing block which is either Block, ModuleBlock, SourceFile
            let parent = self.store().node(node).parent.get();
            let parent_kind = self.store().node(parent).kind;
            if !self.node_links.get(node).has_reported_statement_in_ambient_context
                && (tsc_ast::utilities::is_function_like(self.store().node(parent))
                    || is_accessor(self.store().node(parent)))
            {
                let reported = self.grammar_error_on_first_token(
                    node,
                    &diag::AN_IMPLEMENTATION_CANNOT_BE_DECLARED_IN_AMBIENT_CONTEXTS,
                    &[],
                );
                self.node_links.get(node).has_reported_statement_in_ambient_context = reported;
                return reported;
            }

            // We are either parented by another statement, or some sort of block.
            // If we're in a block, we only want to really report an error once
            // to prevent noisiness.  So use a bit on the block to indicate if
            // this has already been reported, and don't report if it has.
            if matches!(parent_kind, Kind::Block | Kind::ModuleBlock | Kind::SourceFile) {
                // Check if the containing block ever report this error
                if !self
                    .node_links
                    .get(parent)
                    .has_reported_statement_in_ambient_context
                {
                    let reported = self.grammar_error_on_first_token(
                        node,
                        &diag::STATEMENTS_ARE_NOT_ALLOWED_IN_AMBIENT_CONTEXTS,
                        &[],
                    );
                    self.node_links.get(parent).has_reported_statement_in_ambient_context = reported;
                    return reported;
                }
            } else {
                // We must be parented by a statement.  If so, there's no need
                // to report the error as our parent will have already done it.
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarNumericLiteral(node)` (no return).
    pub(crate) fn check_grammar_numeric_literal(&mut self, node: NodeId) {
        let node_text = get_text_of_node(self.store(), node);

        // Realism (size) checking (Go's comment preserved)
        let is_fractional = node_text.contains('.');
        let token_flags = self
            .store()
            .node(node)
            .as_numeric_literal()
            .map(|d| d.token_flags)
            .unwrap_or_default();
        let is_scientific = token_flags
            .intersects(tsc_ast::tokenflags::TokenFlags::SCIENTIFIC);

        // Scientific notation (e.g. 2e54 and 1e00000000010) can't be converted to bigint
        // Fractional numbers (e.g. 9000000000000000.001) are inherently imprecise anyway
        if is_fractional || is_scientific {
            return;
        }

        // Here `node` is guaranteed to be a numeric literal representing an integer.
        let text = tsc_ast::node_text(self.store(), node);
        let value = tsc_jsnum::from_string(text);
        if value <= tsc_jsnum::MAX_SAFE_INTEGER {
            return;
        }

        self.add_error_or_suggestion(
            false,
            u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::NUMERIC_LITERALS_WITH_ABSOLUTE_VALUES_EQUAL_TO_2_53_OR_GREATER_ARE_TOO_LARGE_TO_BE_REPRESENTED_ACCURATELY_AS_INTEGERS,
                vec![],
            ),
        );
    }

    /// Go: `func (c *Checker) checkGrammarBigIntLiteral(node) bool`.
    pub(crate) fn check_grammar_big_int_literal(&mut self, node: NodeId) -> bool {
        let parent = self.store().node(node).parent.get();
        let grandparent = self.store().node(parent).parent.get();
        let literal_type = tsc_ast::is_literal_type_node(self.store().node(parent))
            || (tsc_ast::is_prefix_unary_expression(self.store().node(parent))
                && tsc_ast::is_literal_type_node(self.store().node(grandparent)));
        if !literal_type
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
            && self.language_version < ScriptTarget::ES2020
        {
            // Don't error on BigInt literals in ambient contexts
            if self.grammar_error_on_node(
                node,
                &diag::BIGINT_LITERALS_ARE_NOT_AVAILABLE_WHEN_TARGETING_LOWER_THAN_ES2020,
                &[],
            ) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getEmitSyntaxForModuleSpecifierExpression(usage) ResolutionMode`
    /// (checker.go) — string-literal usages read the program layer's
    /// emit-syntax-for-usage-location service. PORT: the Rust Program seam
    /// `get_emit_module_format_of_file` stands in for
    /// `GetEmitSyntaxForUsageLocation` (the per-file format is the single
    /// determinant in the testrunner's single-file programs).
    // PORT (FCS tranche 40): pub(crate) — statements.rs
    // check_import_attributes reuses the helper (Go checker.go:5578's
    // getEmitSyntaxForModuleSpecifierExpression call for the TS2856 arm).
    pub(crate) fn get_emit_syntax_for_module_specifier_expression(&self, usage: NodeId) -> ModuleKind {
        if tsc_ast::utilities::is_string_literal_like(self.store().node(usage)) {
            let file = self.get_source_file_node_of(usage);
            return self.program.get_emit_module_format_of_file(file);
        }
        ModuleKind::None
    }

    /// Go: `func (c *Checker) checkGrammarImportClause(node) bool`.
    pub(crate) fn check_grammar_import_clause(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_import_clause()
            .expect("ImportClause data")
            .clone();
        match d.phase_modifier {
            Some(Kind::TypeKeyword) => {
                if !self.store().node(node).flags.intersects(NF::JSDOC)
                    && d.name.is_some()
                    && d.named_bindings.is_some()
                {
                    return self.grammar_error_on_node(
                        node,
                        &diag::A_TYPE_ONLY_IMPORT_CAN_SPECIFY_A_DEFAULT_IMPORT_OR_NAMED_BINDINGS_BUT_NOT_BOTH,
                        &[],
                    );
                }
                if let Some(named_bindings) = d.named_bindings
                    && self.store().node(named_bindings).kind == Kind::NamedImports
                {
                    return self.check_grammar_type_only_named_imports_or_exports(named_bindings);
                }
            }
            Some(Kind::DeferKeyword) => {
                if d.name.is_some() {
                    return self.grammar_error_on_node(
                        node,
                        &diag::DEFAULT_IMPORTS_ARE_NOT_ALLOWED_IN_A_DEFERRED_IMPORT,
                        &[],
                    );
                }
                if d.named_bindings.is_none() {
                    return self.grammar_error_on_node(
                        node,
                        &diag::A_DEFERRED_IMPORT_MUST_SPECIFY_A_NAMESPACE_BINDING,
                        &[],
                    );
                }
                if d.named_bindings.is_some_and(|nb| self.store().node(nb).kind == Kind::NamedImports) {
                    return self.grammar_error_on_node(
                        node,
                        &diag::NAMED_IMPORTS_ARE_NOT_ALLOWED_IN_A_DEFERRED_IMPORT,
                        &[],
                    );
                }
                if self.module_kind.supports_deferred_imports() {
                    return false;
                }
                return self.grammar_error_on_node(
                    node,
                    &diag::DEFERRED_IMPORTS_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_FLAG_IS_SET_TO_ESNEXT_OR_PRESERVE,
                    &[],
                );
            }
            Some(Kind::SourceKeyword) => {
                if d.named_bindings.is_some() {
                    return self.grammar_error_on_node(
                        node,
                        &diag::NAMED_AND_NAMESPACE_IMPORTS_ARE_NOT_ALLOWED_IN_A_SOURCE_PHASE_IMPORT,
                        &[],
                    );
                }
                if d.name.is_none() {
                    return self.grammar_error_on_node(
                        node,
                        &diag::A_SOURCE_PHASE_IMPORT_MUST_SPECIFY_A_LOCAL_BINDING,
                        &[],
                    );
                }
                if self.module_kind.supports_source_phase_imports() {
                    let parent = self.store().node(node).parent.get();
                    let module_specifier = crate::resolution::get_module_specifier_from_node(
                        self.store(),
                        parent,
                    );
                    if let Some(module_specifier) = module_specifier
                        && self.get_emit_syntax_for_module_specifier_expression(module_specifier)
                            == ModuleKind::CommonJS
                    {
                        return self.grammar_error_on_node(
                            node,
                            &diag::SOURCE_PHASE_IMPORTS_ARE_NOT_ALLOWED_ON_STATEMENTS_THAT_COMPILE_TO_COMMONJS_REQUIRE_CALLS,
                            &[],
                        );
                    }
                    return false;
                }
                return self.grammar_error_on_node(
                    node,
                    &diag::SOURCE_PHASE_IMPORTS_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ESNEXT_NODENEXT_OR_PRESERVE,
                    &[],
                );
            }
            _ => {}
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarImportAttributeValues(node) bool`.
    pub(crate) fn check_grammar_import_attribute_values(&mut self, node: NodeId) -> bool {
        let mut has_error = false;
        let attributes = self
            .store()
            .node(node)
            .as_import_attributes()
            .and_then(|d| d.attributes.as_ref().map(|l| l.nodes.to_vec()))
            .unwrap_or_default();
        for attribute in attributes {
            let value = self
                .store()
                .node(attribute)
                .as_import_attribute()
                .map(|d| d.value)
                .expect("ImportAttribute value");
            if tsc_ast::is_string_literal(self.store().node(value)) {
                continue;
            }
            has_error = true;
            self.error(value, &diag::IMPORT_ATTRIBUTE_VALUES_MUST_BE_STRING_LITERAL_EXPRESSIONS, &[]);
        }
        has_error
    }

    /// Go: `func (c *Checker) checkGrammarTypeOnlyNamedImportsOrExports(namedBindings) bool`.
    fn check_grammar_type_only_named_imports_or_exports(&mut self, named_bindings: NodeId) -> bool {
        let node_list = self
            .store()
            .node(named_bindings)
            .as_named_imports()
            .and_then(|d| d.elements.as_ref().map(|l| l.nodes.to_vec()))
            .or_else(|| {
                self.store()
                    .node(named_bindings)
                    .as_named_exports()
                    .and_then(|d| d.elements.as_ref().map(|l| l.nodes.to_vec()))
            })
            .unwrap_or_default();
        for specifier in node_list {
            let (specifier_is_type_only, message) = if self.store().node(specifier).kind == Kind::ImportSpecifier {
                (
                    self.store()
                        .node(specifier)
                        .as_import_specifier()
                        .is_some_and(|d| d.is_type_only),
                    &diag::THE_TYPE_MODIFIER_CANNOT_BE_USED_ON_A_NAMED_IMPORT_WHEN_IMPORT_TYPE_IS_USED_ON_ITS_IMPORT_STATEMENT,
                )
            } else {
                (
                    self.store()
                        .node(specifier)
                        .as_export_specifier()
                        .is_some_and(|d| d.is_type_only),
                    &diag::THE_TYPE_MODIFIER_CANNOT_BE_USED_ON_A_NAMED_EXPORT_WHEN_EXPORT_TYPE_IS_USED_ON_ITS_EXPORT_STATEMENT,
                )
            };

            if specifier_is_type_only {
                return self.grammar_error_on_first_token(specifier, message, &[]);
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarImportCallExpression(node) bool`.
    pub(crate) fn check_grammar_import_call_expression(&mut self, node: NodeId) -> bool {
        if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
            && self.module_kind == ModuleKind::CommonJS
        {
            return self.grammar_error_on_node(
                node,
                crate::statements::get_verbatim_module_syntax_error_message(self.store(), node),
                &[],
            );
        }

        let expression = crate::ast_shims::expression_of(self.store(), node);
        if expression.is_some_and(|e| is_import_source_meta_property(self.store(), e)) {
            if self.module_kind.supports_source_phase_imports() {
                // Go: break
            } else {
                return self.grammar_error_on_node(
                    node,
                    &diag::SOURCE_PHASE_IMPORTS_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ESNEXT_NODENEXT_OR_PRESERVE,
                    &[],
                );
            }
        } else if expression.is_some_and(|e| is_import_defer_meta_property(self.store(), e)) {
            if self.module_kind.supports_deferred_imports() {
                // Go: break
            } else {
                return self.grammar_error_on_node(
                    node,
                    &diag::DEFERRED_IMPORTS_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_FLAG_IS_SET_TO_ESNEXT_OR_PRESERVE,
                    &[],
                );
            }
        } else if self.module_kind == ModuleKind::ES2015 {
            return self.grammar_error_on_node(
                node,
                &diag::DYNAMIC_IMPORTS_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_FLAG_IS_SET_TO_ES2020_ES2022_ESNEXT_COMMONJS_AMD_SYSTEM_UMD_NODE16_NODE18_NODE20_OR_NODENEXT,
                &[],
            );
        }

        let node_as_call = self
            .store()
            .node(node)
            .as_call_expression()
            .expect("CallExpression data")
            .clone();
        let is_source_phase_import_call = is_import_call(self.store(), node)
            && expression.is_some_and(|e| is_import_source_meta_property(self.store(), e));
        if is_source_phase_import_call
            && let Some(question_dot_token) = node_as_call.question_dot_token
        {
            return self.grammar_error_on_node(
                question_dot_token,
                &diag::OPTIONAL_CHAINING_CANNOT_BE_USED_WITH_IMPORT_SOURCE,
                &[],
            );
        }
        if node_as_call.type_arguments.is_some() {
            return self.grammar_error_on_node(
                node,
                &diag::THIS_USE_OF_IMPORT_IS_INVALID_IMPORT_CALLS_CAN_BE_WRITTEN_BUT_THEY_MUST_HAVE_PARENTHESES_AND_CANNOT_HAVE_TYPE_ARGUMENTS,
                &[],
            );
        }

        let node_arguments = node_as_call.arguments.expect("CallExpression arguments");
        let argument_nodes = &node_arguments.nodes;
        let node16_to_next = ModuleKind::Node16 <= self.module_kind && self.module_kind <= ModuleKind::NodeNext;
        if !(node16_to_next
            || self.module_kind == ModuleKind::ESNext
            || self.module_kind == ModuleKind::Preserve)
        {
            // We are allowed trailing comma after proposal-import-assertions.
            self.check_grammar_for_disallowed_trailing_comma(
                Some(&node_arguments),
                &diag::TRAILING_COMMA_NOT_ALLOWED,
            );

            if argument_nodes.len() > 1 {
                let import_attributes_argument = argument_nodes[1];
                return self.grammar_error_on_node(
                    import_attributes_argument,
                    &diag::DYNAMIC_IMPORTS_ONLY_SUPPORT_A_SECOND_ARGUMENT_WHEN_THE_MODULE_OPTION_IS_SET_TO_ESNEXT_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE,
                    &[],
                );
            }
        }

        if argument_nodes.is_empty() || argument_nodes.len() > 2 {
            return self.grammar_error_on_node(
                node,
                &diag::DYNAMIC_IMPORTS_CAN_ONLY_ACCEPT_A_MODULE_SPECIFIER_AND_AN_OPTIONAL_SET_OF_ATTRIBUTES_AS_ARGUMENTS,
                &[],
            );
        }

        // see: parseArgumentOrArrayLiteralElement...we use this function which parse arguments of callExpression to parse specifier for dynamic import.
        // parseArgumentOrArrayLiteralElement allows spread element to be in an argument list which is not allowed as specifier in dynamic import.
        if let Some(&spread_element) = argument_nodes
            .iter()
            .find(|&&a| self.store().node(a).kind == Kind::SpreadElement)
        {
            return self.grammar_error_on_node(
                spread_element,
                &diag::ARGUMENT_OF_DYNAMIC_IMPORT_CANNOT_BE_SPREAD_ELEMENT,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarImportAttributesType(attributes) bool`.
    pub(crate) fn check_grammar_import_attributes_type(&mut self, node: NodeId) -> bool {
        let members = self
            .store()
            .node(node)
            .as_type_literal_node()
            .and_then(|d| d.members.clone());
        let Some(members) = members else {
            return false;
        };
        for member in members.nodes.iter().copied() {
            if self.store().node(member).kind != Kind::PropertySignature {
                return self.grammar_error_on_node(
                    member,
                    &diag::AN_IMPORT_ATTRIBUTES_TYPE_MAY_ONLY_CONTAIN_PROPERTY_SIGNATURES,
                    &[],
                );
            }
            let property_signature = self
                .store()
                .node(member)
                .as_property_signature_declaration()
                .expect("PropertySignature data");
            if let Some(modifiers) = property_signature.modifiers.as_ref() {
                for &modifier in modifiers.nodes.iter() {
                    if self.store().node(modifier).kind == Kind::ReadonlyKeyword {
                        return self.grammar_error_on_node(
                            modifier,
                            &diag::AN_IMPORT_ATTRIBUTES_PROPERTY_CANNOT_HAVE_A_READONLY_MODIFIER,
                            &[],
                        );
                    }
                }
            }
            if property_signature.type_.is_none() {
                return self.grammar_error_on_node(
                    member,
                    &diag::AN_IMPORT_ATTRIBUTES_PROPERTY_MUST_HAVE_A_TYPE_ANNOTATION,
                    &[],
                );
            }
            if postfix_token_of(self.store(), member)
                .is_some_and(|t| self.store().node(t).kind == Kind::QuestionToken)
            {
                return self.grammar_error_on_node(
                    member,
                    &diag::AN_IMPORT_ATTRIBUTES_PROPERTY_CANNOT_BE_OPTIONAL,
                    &[],
                );
            }
            let name = property_signature.name;
            if !(tsc_ast::utilities::is_string_literal_like(self.store().node(name))
                || is_identifier(self.store().node(name)))
            {
                return self.grammar_error_on_node(
                    name,
                    &diag::AN_IMPORT_ATTRIBUTES_PROPERTY_MUST_HAVE_A_STRING_LITERAL_OR_IDENTIFIER_NAME,
                    &[],
                );
            }
            if tsc_ast::node_text(self.store(), name) == "resolution-mode" {
                return self.grammar_error_on_node(
                    name,
                    &diag::X_0_IS_NOT_A_VALID_KEY_FOR_AN_IMPORT_ATTRIBUTES_TYPE,
                    &[tsc_ast::node_text(self.store(), name).to_string()],
                );
            }

            let type_node = property_signature.type_;
            if !tsc_ast::is_literal_type_node(self.store().node(type_node))
                || !self
                    .store()
                    .node(type_node)
                    .as_literal_type_node()
                    .map(|d| d.literal)
                    .is_some_and(|lit| tsc_ast::is_string_literal(self.store().node(lit)))
            {
                return self.grammar_error_on_node(
                    type_node,
                    &diag::AN_IMPORT_ATTRIBUTES_PROPERTY_MUST_HAVE_A_STRING_LITERAL_TYPE_ANNOTATION,
                    &[],
                );
            }
        }
        false
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go's file-local functions)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `isInitializerStringOrNumberLiteralExpression` (grammarchecks.go:1960).
pub(crate) fn is_initializer_string_or_number_literal_expression(
    store: &dyn NodeStore,
    expr: NodeId,
) -> bool {
    tsc_ast::utilities::is_string_or_numeric_literal_like(store.node(expr))
        || (store.node(expr).kind == Kind::PrefixUnaryExpression
            && store
                .node(expr)
                .as_prefix_unary_expression()
                .is_some_and(|d| {
                    d.operator == Kind::MinusToken
                        && store.node(d.operand).kind == Kind::NumericLiteral
                }))
}

/// Go: `isInitializerBigIntLiteralExpression` (grammarchecks.go:1965).
pub(crate) fn is_initializer_big_int_literal_expression(
    store: &dyn NodeStore,
    expr: NodeId,
) -> bool {
    if store.node(expr).kind == Kind::BigIntLiteral {
        return true;
    }

    if store.node(expr).kind == Kind::PrefixUnaryExpression {
        let unary_expr = store
            .node(expr)
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression data");
        return unary_expr.operator == Kind::MinusToken
            && store.node(unary_expr.operand).kind == Kind::BigIntLiteral;
    }

    false
}

/// Go: `getIdentifierFromEntityNameExpression` (grammarchecks.go:58) — its Go
/// callers are checker.go:4457/4458 (the heritage-clause compare), not the
/// JSX family; kept Go-mirrored, allowed dead.
#[allow(dead_code)] // the checker.go:4457 heritage-clause wave
pub(crate) fn get_identifier_from_entity_name_expression(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    match store.node(node).kind {
        Kind::Identifier => Some(node),
        Kind::PropertyAccessExpression => store.node(node).name(),
        _ => None,
    }
}

/// The parent binding pattern's element list (Go: `node.Parent.ElementList()`).
fn parent_element_list(store: &dyn NodeStore, parent: NodeId) -> Option<NodeList> {
    match &store.node(parent).data {
        // BindingPattern is the shared data struct for ObjectBindingPattern
        // and ArrayBindingPattern (Go's ElementList on both).
        tsc_ast::NodeData::BindingPattern(d) => d.elements.clone(),
        _ => None,
    }
}

/// Go: `name.Elements()` on a binding pattern (the binding-element walk the
/// let/const + __esModule arms read).
fn binding_elements_of(store: &dyn NodeStore, name: NodeId) -> Vec<NodeId> {
    match &store.node(name).data {
        tsc_ast::NodeData::BindingPattern(d) => {
            d.elements.as_ref().map(|l| l.nodes.to_vec()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Go: `ast.WalkUpParenthesizedTypes(node.Parent)` (utilities.go).
fn walk_up_parenthesized_types(store: &dyn NodeStore, mut node: NodeId) -> NodeId {
    while store.node(node).kind == Kind::ParenthesizedType {
        let parent = store.node(node).parent.get();
        if parent == NodeId::NONE {
            break;
        }
        node = parent;
    }
    node
}

// ────────────────────────────────────────────────────────────────────────────
// The modifier grammar (grammarchecks.go:214-670)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkGrammarModifiers(node) bool`
    /// (grammarchecks.go:214, ~355 lines).
    ///
    /// PORT: `node.ModifierNodes()` is the node's ModifierList nodes; the
    /// `blockScopeKind` read takes the VariableStatement's declaration-list
    /// node flags (Go reads `DeclarationList.Flags` — the Node's flags).
    pub(crate) fn check_grammar_modifiers(&mut self, node: NodeId) -> bool {
        let Some(modifier_list) = self.store().node(node).modifiers().cloned() else {
            return false;
        };
        if self.report_obvious_decorator_errors(node) || self.report_obvious_modifier_errors(node) {
            return true;
        }
        if crate::ast_shims::is_this_parameter(self.store(), node) {
            return self.grammar_error_on_first_token(
                node,
                &diag::NEITHER_DECORATORS_NOR_MODIFIERS_MAY_BE_APPLIED_TO_THIS_PARAMETERS,
                &[],
            );
        }
        let mut block_scope_kind = NF::NONE;
        if self.store().node(node).kind == Kind::VariableStatement {
            let declaration_list = self
                .store()
                .node(node)
                .as_variable_statement()
                .map(|d| d.declaration_list)
                .expect("VariableStatement data");
            block_scope_kind = self.store().node(declaration_list).flags & NF::BLOCK_SCOPED;
        }
        let mut last_static: Option<NodeId> = None;
        let mut last_declare: Option<NodeId> = None;
        let mut last_async: Option<NodeId> = None;
        let mut last_override: Option<NodeId> = None;
        let mut first_decorator: Option<NodeId> = None;
        let mut flags = ModifierFlags::NONE;
        let mut saw_export_before_decorators = false;
        // We parse decorators and modifiers in four contiguous chunks:
        // [...leadingDecorators, ...leadingModifiers, ...trailingDecorators, ...trailingModifiers]. It is an error to
        // have both leading and trailing decorators.
        let mut has_leading_decorators = false;
        for &modifier in modifier_list.nodes.iter() {
            let modifier_kind = self.store().node(modifier).kind;
            if modifier_kind == Kind::Decorator {
                let parent = self.store().node(node).parent.get();
                let grandparent = if parent == NodeId::NONE {
                    NodeId::NONE
                } else {
                    self.store().node(parent).parent.get()
                };
                if !node_can_be_decorated(
                    self.store(),
                    self.legacy_decorators,
                    node,
                    if parent == NodeId::NONE { None } else { Some(parent) },
                    if grandparent == NodeId::NONE { None } else { Some(grandparent) },
                ) {
                    if self.store().node(node).kind == Kind::MethodDeclaration
                        && body_of(self.store(), node).is_none()
                    {
                        return self.grammar_error_on_first_token(
                            node,
                            &diag::A_DECORATOR_CAN_ONLY_DECORATE_A_METHOD_IMPLEMENTATION_NOT_AN_OVERLOAD,
                            &[],
                        );
                    } else {
                        return self
                            .grammar_error_on_first_token(node, &diag::DECORATORS_ARE_NOT_VALID_HERE, &[]);
                    }
                } else if self.legacy_decorators
                    && matches!(self.store().node(node).kind, Kind::GetAccessor | Kind::SetAccessor)
                {
                    let symbol = self.get_symbol_of_declaration_merged(node);
                    let declarations = symbol
                        .map(|s| self.program_symbol(s).declarations.to_vec())
                        .unwrap_or_default();
                    let accessors =
                        get_all_accessor_declarations_for_declaration(self.store(), node, &declarations);
                    if accessors
                        .first_accessor
                        .is_some_and(|a| has_decorators(self.store(), a))
                        && accessors.second_accessor == Some(node)
                    {
                        return self.grammar_error_on_first_token(
                            node,
                            &diag::DECORATORS_CANNOT_BE_APPLIED_TO_MULTIPLE_GET_SLASHSET_ACCESSORS_OF_THE_SAME_NAME,
                            &[],
                        );
                    }
                }

                // if we've seen any modifiers aside from `export`, `default`, or another decorator, then this is an invalid position
                if !(flags & !(ModifierFlags::EXPORT_DEFAULT | ModifierFlags::DECORATOR)).is_empty() {
                    return self
                        .grammar_error_on_node(modifier, &diag::DECORATORS_ARE_NOT_VALID_HERE, &[]);
                }

                // if we've already seen leading decorators and leading modifiers, then trailing decorators are an invalid position
                if has_leading_decorators && flags.intersects(ModifierFlags::MODIFIER) {
                    let Some(first) = first_decorator else {
                        panic!("Expected firstDecorator to be set");
                    };
                    let source_file = self.get_source_file_node_of(modifier);
                    if !self.has_parse_diagnostics(source_file) {
                        let mut err = self.error(
                            modifier,
                            &diag::DECORATORS_MAY_NOT_APPEAR_AFTER_EXPORT_OR_EXPORT_DEFAULT_IF_THEY_ALSO_APPEAR_BEFORE_EXPORT,
                            &[],
                        );
                        err.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            first,
                            &diag::DECORATOR_USED_BEFORE_EXPORT_HERE,
                            vec![],
                        ));
                        return true;
                    }
                    return false;
                }

                flags |= ModifierFlags::DECORATOR;

                // if we have not yet seen a modifier, then these are leading decorators
                if !flags.intersects(ModifierFlags::MODIFIER) {
                    has_leading_decorators = true;
                } else if flags.intersects(ModifierFlags::EXPORT) {
                    saw_export_before_decorators = true;
                }

                if first_decorator.is_none() {
                    first_decorator = Some(modifier);
                }
            } else {
                if modifier_kind != Kind::ReadonlyKeyword {
                    if matches!(
                        self.store().node(node).kind,
                        Kind::PropertySignature | Kind::MethodSignature
                    ) {
                        return self.grammar_error_on_node(
                            modifier,
                            &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_TYPE_MEMBER,
                            &[token_to_string(modifier_kind).to_string()],
                        );
                    }
                    if self.store().node(node).kind == Kind::IndexSignature
                        && (modifier_kind != Kind::StaticKeyword
                            || !self
                                .store()
                                .node(self.store().node(node).parent.get())
                                .kind
                                .eq(&Kind::ClassDeclaration)
                                && !is_class_like(self.store(), self.store().node(node).parent.get()))
                    {
                        return self.grammar_error_on_node(
                            modifier,
                            &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_AN_INDEX_SIGNATURE,
                            &[token_to_string(modifier_kind).to_string()],
                        );
                    }
                }
                if !matches!(
                    modifier_kind,
                    Kind::InKeyword | Kind::OutKeyword | Kind::ConstKeyword
                ) && self.store().node(node).kind == Kind::TypeParameter
                {
                    return self.grammar_error_on_node(
                        modifier,
                        &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_TYPE_PARAMETER,
                        &[token_to_string(modifier_kind).to_string()],
                    );
                }
                let reparsed = self.store().node(modifier).flags.intersects(NF::REPARSED);
                match modifier_kind {
                    Kind::ConstKeyword => {
                        if self.store().node(node).kind != Kind::EnumDeclaration
                            && self.store().node(node).kind != Kind::TypeParameter
                        {
                            return self.grammar_error_on_node(
                                node,
                                &diag::A_CLASS_MEMBER_CANNOT_HAVE_THE_0_KEYWORD,
                                &[token_to_string(Kind::ConstKeyword).to_string()],
                            );
                        }
                        let parent = self.store().node(node).parent.get();
                        if self.store().node(node).kind == Kind::TypeParameter {
                            let parent_fn_like = parent != NodeId::NONE
                                && tsc_ast::utilities::is_function_like(self.store().node(parent));
                            let parent_class_like =
                                parent != NodeId::NONE && is_class_like(self.store(), parent);
                            let parent_type_node = parent != NodeId::NONE
                                && matches!(
                                    self.store().node(parent).kind,
                                    Kind::FunctionType | Kind::ConstructorType
                                );
                            let parent_sig_like = parent != NodeId::NONE
                                && matches!(
                                    self.store().node(parent).kind,
                                    Kind::CallSignature
                                        | Kind::ConstructSignature
                                        | Kind::MethodSignature
                                );
                            if !(parent_fn_like
                                || parent_class_like
                                || parent_type_node
                                || parent_sig_like)
                            {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_CAN_ONLY_APPEAR_ON_A_TYPE_PARAMETER_OF_A_FUNCTION_METHOD_OR_CLASS,
                                    &[token_to_string(modifier_kind).to_string()],
                                );
                            }
                        }
                    }
                    Kind::OverrideKeyword => {
                        // If node.kind === SyntaxKind.Parameter, checkParameter reports an error if it's not a parameter property.
                        if flags.intersects(ModifierFlags::OVERRIDE) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["override".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["override".to_string(), "declare".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["override".to_string(), "readonly".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["override".to_string(), "accessor".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["override".to_string(), "async".to_string()],
                            );
                        }
                        flags |= ModifierFlags::OVERRIDE;
                        last_override = Some(modifier);
                    }
                    Kind::PublicKeyword | Kind::ProtectedKeyword | Kind::PrivateKeyword => {
                        let text = crate::relater::visibility_to_string(
                            tsc_ast::visitor::modifier_to_flag(modifier_kind),
                        );
                        let parent_kind = self.store().node(self.store().node(node).parent.get()).kind;
                        if flags.intersects(ModifierFlags::ACCESSIBILITY_MODIFIER) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::ACCESSIBILITY_MODIFIER_ALREADY_SEEN,
                                &[],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &[text.to_string(), "override".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::STATIC) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &[text.to_string(), "static".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &[text.to_string(), "accessor".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &[text.to_string(), "readonly".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &[text.to_string(), "async".to_string()],
                            );
                        } else if matches!(parent_kind, Kind::ModuleBlock | Kind::SourceFile) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_MODULE_OR_NAMESPACE_ELEMENT,
                                &[text.to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) {
                            if modifier_kind == Kind::PrivateKeyword {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                    &[text.to_string(), "abstract".to_string()],
                                );
                            } else if !reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                    &[text.to_string(), "abstract".to_string()],
                                );
                            }
                        } else if tsc_ast::utilities::is_private_identifier_class_element_declaration(
                            self.store(),
                            node,
                        ) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::AN_ACCESSIBILITY_MODIFIER_CANNOT_BE_USED_WITH_A_PRIVATE_IDENTIFIER,
                                &[],
                            );
                        }
                        flags |= tsc_ast::visitor::modifier_to_flag(modifier_kind);
                    }
                    Kind::StaticKeyword => {
                        if flags.intersects(ModifierFlags::STATIC) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["static".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["static".to_string(), "readonly".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["static".to_string(), "async".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["static".to_string(), "accessor".to_string()],
                            );
                        } else if matches!(
                            self.store().node(self.store().node(node).parent.get()).kind,
                            Kind::ModuleBlock | Kind::SourceFile
                        ) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_MODULE_OR_NAMESPACE_ELEMENT,
                                &["static".to_string()],
                            );
                        } else if self.store().node(node).kind == Kind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_PARAMETER,
                                &["static".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["static".to_string(), "abstract".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["static".to_string(), "override".to_string()],
                            );
                        }
                        flags |= ModifierFlags::STATIC;
                        last_static = Some(modifier);
                    }
                    Kind::AccessorKeyword => {
                        if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["accessor".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["accessor".to_string(), "readonly".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["accessor".to_string(), "declare".to_string()],
                            );
                        } else if self.store().node(node).kind != Kind::PropertyDeclaration {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_ACCESSOR_MODIFIER_CAN_ONLY_APPEAR_ON_A_PROPERTY_DECLARATION,
                                &[],
                            );
                        }
                        flags |= ModifierFlags::ACCESSOR;
                    }
                    Kind::ReadonlyKeyword => {
                        if flags.intersects(ModifierFlags::READONLY) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["readonly".to_string()],
                            );
                        } else if !matches!(
                            self.store().node(node).kind,
                            Kind::PropertyDeclaration
                                | Kind::PropertySignature
                                | Kind::IndexSignature
                                | Kind::Parameter
                        ) {
                            // If node.kind === SyntaxKind.Parameter, checkParameter reports an error if it's not a parameter property.
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_READONLY_MODIFIER_CAN_ONLY_APPEAR_ON_A_PROPERTY_DECLARATION_OR_INDEX_SIGNATURE,
                                &[],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["readonly".to_string(), "accessor".to_string()],
                            );
                        }
                        flags |= ModifierFlags::READONLY;
                    }
                    Kind::ExportKeyword => {
                        let source_file_of_node = self.get_source_file_node_of(node);
                        if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                            && !self.store().node(node).flags.intersects(NF::AMBIENT)
                            && self.store().node(node).kind != Kind::TypeAliasDeclaration
                            && self.store().node(node).kind != Kind::InterfaceDeclaration
                            && self.store().node(node).kind != Kind::ModuleDeclaration
                            && self.store().node(node).parent.get() != NodeId::NONE
                            && is_source_file(
                                self.store().node(self.store().node(node).parent.get()),
                            )
                            && self.program.get_emit_module_format_of_file(source_file_of_node)
                                == ModuleKind::CommonJS
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::A_TOP_LEVEL_EXPORT_MODIFIER_CANNOT_BE_USED_ON_VALUE_DECLARATIONS_IN_A_COMMONJS_MODULE_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED,
                                &[],
                            );
                        }
                        if flags.intersects(ModifierFlags::EXPORT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["export".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["export".to_string(), "declare".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["export".to_string(), "abstract".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["export".to_string(), "async".to_string()],
                            );
                        } else if is_class_like(self.store(), self.store().node(node).parent.get())
                            && !is_js_type_alias_declaration(self.store().node(node))
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_CLASS_ELEMENTS_OF_THIS_KIND,
                                &["export".to_string()],
                            );
                        } else if self.store().node(node).kind == Kind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_PARAMETER,
                                &["export".to_string()],
                            );
                        } else if block_scope_kind == NF::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_USING_DECLARATION,
                                &["export".to_string()],
                            );
                        } else if block_scope_kind == NF::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_AN_AWAIT_USING_DECLARATION,
                                &["export".to_string()],
                            );
                        }
                        flags |= ModifierFlags::EXPORT;
                    }
                    Kind::DefaultKeyword => {
                        let parent = self.store().node(node).parent.get();
                        let container = if parent != NodeId::NONE
                            && is_source_file(self.store().node(parent))
                        {
                            Some(parent)
                        } else if parent != NodeId::NONE {
                            Some(self.store().node(parent).parent.get())
                        } else {
                            None
                        };
                        let is_ambient_module = container.is_some_and(|c| {
                            self.store().node(c).kind == Kind::ModuleDeclaration
                                && tsc_binder::ast_shims::is_ambient_module(self.store(), c)
                        });
                        if container.is_some_and(|c| self.store().node(c).kind == Kind::ModuleDeclaration)
                            && !is_ambient_module
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::A_DEFAULT_EXPORT_CAN_ONLY_BE_USED_IN_AN_ECMASCRIPT_STYLE_MODULE,
                                &[],
                            );
                        } else if block_scope_kind == NF::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_USING_DECLARATION,
                                &["default".to_string()],
                            );
                        } else if block_scope_kind == NF::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_AN_AWAIT_USING_DECLARATION,
                                &["default".to_string()],
                            );
                        } else if !flags.intersects(ModifierFlags::EXPORT) && !reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["export".to_string(), "default".to_string()],
                            );
                        } else if saw_export_before_decorators {
                            return self.grammar_error_on_node(
                                first_decorator.expect("sawExportBeforeDecorators implies firstDecorator"),
                                &diag::DECORATORS_ARE_NOT_VALID_HERE,
                                &[],
                            );
                        }
                        flags |= ModifierFlags::DEFAULT;
                    }
                    Kind::DeclareKeyword => {
                        if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["declare".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_IN_AN_AMBIENT_CONTEXT,
                                &["async".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_IN_AN_AMBIENT_CONTEXT,
                                &["override".to_string()],
                            );
                        } else if is_class_like(self.store(), self.store().node(node).parent.get())
                            && self.store().node(node).kind != Kind::PropertyDeclaration
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_CLASS_ELEMENTS_OF_THIS_KIND,
                                &["declare".to_string()],
                            );
                        } else if self.store().node(node).kind == Kind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_PARAMETER,
                                &["declare".to_string()],
                            );
                        } else if block_scope_kind == NF::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_USING_DECLARATION,
                                &["declare".to_string()],
                            );
                        } else if block_scope_kind == NF::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_AN_AWAIT_USING_DECLARATION,
                                &["declare".to_string()],
                            );
                        } else if self
                            .store()
                            .node(self.store().node(node).parent.get())
                            .flags
                            .intersects(NF::AMBIENT)
                            && self.store().node(self.store().node(node).parent.get()).kind
                                == Kind::ModuleBlock
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::A_DECLARE_MODIFIER_CANNOT_BE_USED_IN_AN_ALREADY_AMBIENT_CONTEXT,
                                &[],
                            );
                        } else if tsc_ast::utilities::is_private_identifier_class_element_declaration(
                            self.store(),
                            node,
                        ) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_A_PRIVATE_IDENTIFIER,
                                &["declare".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["declare".to_string(), "accessor".to_string()],
                            );
                        }
                        flags |= ModifierFlags::AMBIENT;
                        last_declare = Some(modifier);
                    }
                    Kind::AbstractKeyword => {
                        if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["abstract".to_string()],
                            );
                        }
                        let kind = self.store().node(node).kind;
                        if kind != Kind::ClassDeclaration && kind != Kind::ConstructorType {
                            if !matches!(
                                kind,
                                Kind::MethodDeclaration
                                    | Kind::PropertyDeclaration
                                    | Kind::GetAccessor
                                    | Kind::SetAccessor
                            ) {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_ABSTRACT_MODIFIER_CAN_ONLY_APPEAR_ON_A_CLASS_METHOD_OR_PROPERTY_DECLARATION,
                                    &[],
                                );
                            }
                            let parent = self.store().node(node).parent.get();
                            let parent_abstract = parent != NodeId::NONE
                                && self.store().node(parent).kind == Kind::ClassDeclaration
                                && has_syntactic_modifier(
                                    self.store().node(parent),
                                    ModifierFlags::ABSTRACT,
                                );
                            if !parent_abstract {
                                let message = if kind == Kind::PropertyDeclaration {
                                    &diag::ABSTRACT_PROPERTIES_CAN_ONLY_APPEAR_WITHIN_AN_ABSTRACT_CLASS
                                } else {
                                    &diag::ABSTRACT_METHODS_CAN_ONLY_APPEAR_WITHIN_AN_ABSTRACT_CLASS
                                };
                                return self.grammar_error_on_node(modifier, message, &[]);
                            }
                            if flags.intersects(ModifierFlags::STATIC) {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                    &["static".to_string(), "abstract".to_string()],
                                );
                            }
                            if flags.intersects(ModifierFlags::PRIVATE) {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                    &["private".to_string(), "abstract".to_string()],
                                );
                            }
                            if flags.intersects(ModifierFlags::ASYNC)
                                && let Some(last_async) = last_async
                            {
                                return self.grammar_error_on_node(
                                    last_async,
                                    &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                    &["async".to_string(), "abstract".to_string()],
                                );
                            }
                            if flags.intersects(ModifierFlags::OVERRIDE) && !reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                    &["abstract".to_string(), "override".to_string()],
                                );
                            }
                            if flags.intersects(ModifierFlags::ACCESSOR) && !reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                    &["abstract".to_string(), "accessor".to_string()],
                                );
                            }
                        }
                        if let Some(name) = self.store().node(node).name()
                            && self.store().node(name).kind == Kind::PrivateIdentifier
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_A_PRIVATE_IDENTIFIER,
                                &["abstract".to_string()],
                            );
                        }
                        flags |= ModifierFlags::ABSTRACT;
                    }
                    Kind::AsyncKeyword => {
                        if flags.intersects(ModifierFlags::ASYNC) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &["async".to_string()],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT)
                            || self
                                .store()
                                .node(self.store().node(node).parent.get())
                                .flags
                                .intersects(NF::AMBIENT)
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_IN_AN_AMBIENT_CONTEXT,
                                &["async".to_string()],
                            );
                        } else if self.store().node(node).kind == Kind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_PARAMETER,
                                &["async".to_string()],
                            );
                        }
                        if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_WITH_1_MODIFIER,
                                &["async".to_string(), "abstract".to_string()],
                            );
                        }
                        flags |= ModifierFlags::ASYNC;
                        last_async = Some(modifier);
                    }
                    Kind::InKeyword | Kind::OutKeyword => {
                        let (in_out_flag, in_out_text) = if modifier_kind == Kind::InKeyword {
                            (ModifierFlags::IN, "in")
                        } else {
                            (ModifierFlags::OUT, "out")
                        };
                        let parent = self.store().node(node).parent.get();
                        let parent_ok = parent != NodeId::NONE
                            && (matches!(
                                self.store().node(parent).kind,
                                Kind::InterfaceDeclaration
                            ) || is_class_like(self.store(), parent)
                                || matches!(self.store().node(parent).kind, Kind::TypeAliasDeclaration)
                                || is_js_type_alias_declaration(self.store().node(parent)));
                        if self.store().node(node).kind != Kind::TypeParameter || !parent_ok {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_CAN_ONLY_APPEAR_ON_A_TYPE_PARAMETER_OF_A_CLASS_INTERFACE_OR_TYPE_ALIAS,
                                &[in_out_text.to_string()],
                            );
                        }
                        if flags.intersects(in_out_flag) {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_ALREADY_SEEN,
                                &[in_out_text.to_string()],
                            );
                        }
                        if in_out_flag.intersects(ModifierFlags::IN)
                            && flags.intersects(ModifierFlags::OUT)
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                &diag::X_0_MODIFIER_MUST_PRECEDE_1_MODIFIER,
                                &["in".to_string(), "out".to_string()],
                            );
                        }
                        flags |= in_out_flag;
                    }
                    _ => {}
                }
            }
        }

        let kind = self.store().node(node).kind;
        if kind == Kind::Constructor {
            if flags.intersects(ModifierFlags::STATIC) {
                return self.grammar_error_on_node(
                    last_static.expect("flags&Static implies lastStatic"),
                    &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_CONSTRUCTOR_DECLARATION,
                    &["static".to_string()],
                );
            }
            if flags.intersects(ModifierFlags::OVERRIDE) {
                return self.grammar_error_on_node(
                    last_override.expect("flags&Override implies lastOverride"),
                    &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_CONSTRUCTOR_DECLARATION,
                    &["override".to_string()],
                );
            }
            if flags.intersects(ModifierFlags::ASYNC) {
                return self.grammar_error_on_node(
                    last_async.expect("flags&Async implies lastAsync"),
                    &diag::X_0_MODIFIER_CANNOT_APPEAR_ON_A_CONSTRUCTOR_DECLARATION,
                    &["async".to_string()],
                );
            }
            return false;
        } else if matches!(
            kind,
            Kind::ImportDeclaration | Kind::JSImportDeclaration | Kind::ImportEqualsDeclaration
        ) && flags.intersects(ModifierFlags::AMBIENT)
        {
            return self.grammar_error_on_node(
                last_declare.expect("flags&Ambient implies lastDeclare"),
                &diag::A_0_MODIFIER_CANNOT_BE_USED_WITH_AN_IMPORT_DECLARATION,
                &["declare".to_string()],
            );
        } else if kind == Kind::Parameter
            && flags.intersects(ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
        {
            let name = self.store().node(node).name();
            if name.is_some_and(|n| crate::statements::is_binding_pattern(self.store(), n)) {
                return self.grammar_error_on_node(
                    node,
                    &diag::A_PARAMETER_PROPERTY_MAY_NOT_BE_DECLARED_USING_A_BINDING_PATTERN,
                    &[],
                );
            }
            if name.is_some()
                && self
                    .store()
                    .node(node)
                    .as_parameter_declaration()
                    .is_some_and(|d| d.dot_dot_dot_token.is_some())
            {
                return self.grammar_error_on_node(
                    node,
                    &diag::A_PARAMETER_PROPERTY_CANNOT_BE_DECLARED_USING_A_REST_PARAMETER,
                    &[],
                );
            }
        }
        if flags.intersects(ModifierFlags::ASYNC) {
            return self.check_grammar_async_modifier(node, last_async);
        }
        false
    }

    /// Go: `func (c *Checker) reportObviousModifierErrors(node) bool`.
    fn report_obvious_modifier_errors(&mut self, node: NodeId) -> bool {
        let Some(modifier) = self.find_first_illegal_modifier(node) else {
            return false;
        };
        self.grammar_error_on_first_token(modifier, &diag::MODIFIERS_CANNOT_APPEAR_HERE, &[])
    }

    /// Go: `func (c *Checker) findFirstModifierExcept(node, allowedModifier) *ast.Node`.
    fn find_first_modifier_except(&self, node: NodeId, allowed_modifier: Kind) -> Option<NodeId> {
        let modifier = self
            .store()
            .node(node)
            .modifiers()
            .and_then(|m| m.nodes.iter().copied().find(|&mo| is_modifier(self.store().node(mo))));
        if let Some(modifier) = modifier
            && self.store().node(modifier).kind != allowed_modifier
        {
            return Some(modifier);
        }
        None
    }

    /// Go: `func (c *Checker) findFirstIllegalModifier(node) *ast.Node`.
    fn find_first_illegal_modifier(&self, node: NodeId) -> Option<NodeId> {
        let kind = self.store().node(node).kind;
        let parent_kind = self.store().node(self.store().node(node).parent.get()).kind;
        let first_modifier = self.find_first_modifier(node);
        match kind {
            Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::Constructor
            | Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::IndexSignature
            | Kind::ModuleDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ExportDeclaration
            | Kind::ExportAssignment
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::Parameter
            | Kind::TypeParameter
            | Kind::JSTypeAliasDeclaration => None,
            Kind::ClassStaticBlockDeclaration
            | Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::NamespaceExportDeclaration
            | Kind::MissingDeclaration => first_modifier,
            _ => {
                if matches!(parent_kind, Kind::ModuleBlock | Kind::SourceFile) {
                    return None;
                }
                match kind {
                    Kind::FunctionDeclaration => self.find_first_modifier_except(node, Kind::AsyncKeyword),
                    Kind::ClassDeclaration | Kind::ConstructorType => {
                        self.find_first_modifier_except(node, Kind::AbstractKeyword)
                    }
                    Kind::ClassExpression
                    | Kind::InterfaceDeclaration
                    | Kind::TypeAliasDeclaration => first_modifier,
                    Kind::VariableStatement => {
                        let declaration_list = self
                            .store()
                            .node(node)
                            .as_variable_statement()
                            .map(|d| d.declaration_list)
                            .expect("VariableStatement data");
                        if self
                            .store()
                            .node(declaration_list)
                            .flags
                            .intersects(NF::USING)
                        {
                            self.find_first_modifier_except(node, Kind::AwaitKeyword)
                        } else {
                            first_modifier
                        }
                    }
                    Kind::EnumDeclaration => self.find_first_modifier_except(node, Kind::ConstKeyword),
                    _ => panic!("Unhandled case in findFirstIllegalModifier."),
                }
            }
        }
    }

    /// Go: `core.Find(node.ModifierNodes(), ast.IsModifier)`.
    fn find_first_modifier(&self, node: NodeId) -> Option<NodeId> {
        self.store()
            .node(node)
            .modifiers()
            .and_then(|m| m.nodes.iter().copied().find(|&mo| is_modifier(self.store().node(mo))))
    }

    /// Go: `func (c *Checker) reportObviousDecoratorErrors(node) bool`.
    fn report_obvious_decorator_errors(&mut self, node: NodeId) -> bool {
        let Some(decorator) = self.find_first_illegal_decorator(node) else {
            return false;
        };
        self.grammar_error_on_first_token(decorator, &diag::DECORATORS_ARE_NOT_VALID_HERE, &[])
    }

    /// Go: `func (c *Checker) findFirstIllegalDecorator(node) *ast.Node`.
    fn find_first_illegal_decorator(&self, node: NodeId) -> Option<NodeId> {
        if can_have_illegal_decorators(self.store().node(node)) {
            self.store()
                .node(node)
                .modifiers()
                .and_then(|m| m.nodes.iter().copied().find(|&mo| is_decorator(self.store().node(mo))))
        } else {
            None
        }
    }

    /// Go: `func (c *Checker) checkGrammarAsyncModifier(node, asyncModifier) bool`.
    fn check_grammar_async_modifier(&mut self, node: NodeId, async_modifier: Option<NodeId>) -> bool {
        match self.store().node(node).kind {
            Kind::MethodDeclaration
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction => return false,
            _ => {}
        }
        self.grammar_error_on_node(
            async_modifier.expect("flags&Async implies lastAsync"),
            &diag::X_0_MODIFIER_CANNOT_BE_USED_HERE,
            &["async".to_string()],
        )
    }

    /// Go: `func (c *Checker) checkGrammarForDisallowedTrailingComma(list, diag) bool`.
    ///
    /// PORT: the list arrives as `Option<&NodeList>` (Go's *NodeList; nil
    /// for absent lists at the call sites).
    pub(crate) fn check_grammar_for_disallowed_trailing_comma(
        &mut self,
        list: Option<&NodeList>,
        diagnostic: &'static Message,
    ) -> bool {
        if let Some(list) = list
            && list.has_trailing_comma(self.store())
        {
            let first = list.nodes[0];
            return self.grammar_error_at_pos(
                first,
                list.end() - 1, // len(",")
                1,
                diagnostic,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarTypeParameterList(typeParameters, file) bool`.
    pub(crate) fn check_grammar_type_parameter_list(
        &mut self,
        type_parameters: Option<NodeList>,
        file: NodeId,
    ) -> bool {
        if let Some(type_parameters) = &type_parameters
            && type_parameters.nodes.is_empty()
        {
            let start = type_parameters.pos() - 1; // len("<")
            let text = self
                .source_file_of(file)
                .map(|f| f.text().to_string())
                .unwrap_or_default();
            let end = tsc_scanner::skip_trivia(&text, type_parameters.end()) + 1; // len(">")
            return self.grammar_error_at_pos(
                file,
                start,
                end - start,
                &diag::TYPE_PARAMETER_LIST_CANNOT_BE_EMPTY,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarParameterList(parameters) bool`.
    pub(crate) fn check_grammar_parameter_list(&mut self, parameters: &NodeList) -> bool {
        let mut seen_optional_parameter = false;
        let parameter_count = parameters.nodes.len();

        for i in 0..parameter_count {
            let parameter = parameters.nodes[i];
            let d = self
                .store()
                .node(parameter)
                .as_parameter_declaration()
                .expect("ParameterDeclaration data")
                .clone();
            if let Some(dot_dot_dot_token) = d.dot_dot_dot_token {
                if i != parameter_count - 1 {
                    return self.grammar_error_on_node(
                        dot_dot_dot_token,
                        &diag::A_REST_PARAMETER_MUST_BE_LAST_IN_A_PARAMETER_LIST,
                        &[],
                    );
                }
                if !self.store().node(parameter).flags.intersects(NF::AMBIENT) {
                    self.check_grammar_for_disallowed_trailing_comma(
                        Some(parameters),
                        &diag::A_REST_PARAMETER_OR_BINDING_PATTERN_MAY_NOT_HAVE_A_TRAILING_COMMA,
                    );
                }

                if let Some(question_token) = d.question_token {
                    return self.grammar_error_on_node(
                        question_token,
                        &diag::A_REST_PARAMETER_CANNOT_BE_OPTIONAL,
                        &[],
                    );
                }

                if d.initializer.is_some() {
                    return self.grammar_error_on_node(
                        d.name,
                        &diag::A_REST_PARAMETER_CANNOT_HAVE_AN_INITIALIZER,
                        &[],
                    );
                }
            } else if u::is_optional_declaration(self.store(), parameter) {
                seen_optional_parameter = true;
                // A reparsed '?' token indicates a bracketed name in @param tag
                if d.question_token.is_some()
                    && !self
                        .store()
                        .node(d.question_token.expect("checked"))
                        .flags
                        .intersects(NF::REPARSED)
                    && d.initializer.is_some()
                {
                    return self.grammar_error_on_node(
                        d.name,
                        &diag::PARAMETER_CANNOT_HAVE_QUESTION_MARK_AND_INITIALIZER,
                        &[],
                    );
                }
            } else if seen_optional_parameter && d.initializer.is_none() {
                return self.grammar_error_on_node(
                    d.name,
                    &diag::A_REQUIRED_PARAMETER_CANNOT_FOLLOW_AN_OPTIONAL_PARAMETER,
                    &[],
                );
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarForUseStrictSimpleParameterList(node) bool`.
    fn check_grammar_for_use_strict_simple_parameter_list(&mut self, node: NodeId) -> bool {
        if self.language_version >= ScriptTarget::ES2016 {
            let body = body_of(self.store(), node);
            let mut use_strict_directive = None;
            if let Some(body) = body
                && self.store().node(body).kind == Kind::Block
            {
                let statements = self
                    .store()
                    .node(body)
                    .as_block()
                    .and_then(|d| d.statements.as_ref().map(|l| l.nodes.to_vec()))
                    .unwrap_or_default();
                let source_file =
                    tsc_ast::utilities::get_source_file_of_node(self.store(), node);
                if let Some(source_file) = source_file {
                    use_strict_directive =
                        tsc_binder::binder::find_use_strict_prologue(source_file, &statements);
                }
            }
            if let Some(use_strict_directive) = use_strict_directive {
                let parameters = parameters_of(self.store(), node);
                let mut non_simple_parameters: Vec<NodeId> = Vec::new();
                for &parameter in parameters {
                    let d = self
                        .store()
                        .node(parameter)
                        .as_parameter_declaration()
                        .expect("ParameterDeclaration data");
                    if d.initializer.is_some()
                        || crate::statements::is_binding_pattern(self.store(), d.name)
                        || (d.dot_dot_dot_token.is_some())
                    {
                        non_simple_parameters.push(parameter);
                    }
                }
                if !non_simple_parameters.is_empty() {
                    for &parameter in &non_simple_parameters {
                        let mut err =
                            self.error(parameter, &diag::THIS_PARAMETER_IS_NOT_ALLOWED_WITH_USE_STRICT_DIRECTIVE, &[]);
                        err.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            use_strict_directive,
                            &diag::X_USE_STRICT_DIRECTIVE_USED_HERE,
                            vec![],
                        ));
                    }

                    let mut err = self.error(
                        use_strict_directive,
                        &diag::X_USE_STRICT_DIRECTIVE_CANNOT_BE_USED_WITH_NON_SIMPLE_PARAMETER_LIST,
                        &[],
                    );
                    for (index, &parameter) in non_simple_parameters.iter().enumerate() {
                        let related_message = if index == 0 {
                            &diag::NON_SIMPLE_PARAMETER_DECLARED_HERE
                        } else {
                            &diag::X_AND_HERE
                        };
                        err.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            parameter,
                            related_message,
                            vec![],
                        ));
                    }

                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarFunctionLikeDeclaration(node) bool`.
    pub(crate) fn check_grammar_function_like_declaration(&mut self, node: NodeId) -> bool {
        // Prevent cascading error by short-circuit
        let file = self.get_source_file_node_of(node);
        let (type_parameters, parameters) = self.function_like_data_of(node);
        self.check_grammar_modifiers(node)
            || self.check_grammar_type_parameter_list(type_parameters.clone(), file)
            || (parameters.is_some_and(|p| self.check_grammar_parameter_list(&p)))
            || self.check_grammar_arrow_function(node, file)
            || (tsc_ast::utilities::is_function_like(self.store().node(node))
                && self.check_grammar_for_use_strict_simple_parameter_list(node))
    }

    /// Go: `node.FunctionLikeData()` — the shared typeParameters/parameters
    /// pair the function-like kinds carry (the AST's flattened bases).
    fn function_like_data_of(&self, node: NodeId) -> (Option<NodeList>, Option<NodeList>) {
        let n = self.store().node(node);
        let (type_parameters, parameters) = match &n.data {
            tsc_ast::NodeData::FunctionDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::FunctionExpression(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::ArrowFunction(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::MethodDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::MethodSignatureDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::GetAccessorDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::SetAccessorDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::ConstructorDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::ConstructorTypeNode(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::FunctionTypeNode(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::CallSignatureDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::ConstructSignatureDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::IndexSignatureDeclaration(d) => (d.type_parameters.clone(), d.parameters.clone()),
            tsc_ast::NodeData::JSDocSignature(_) => {
                return (None, None)
            }
            _ => (None, None),
        };
        (type_parameters, parameters)
    }

    /// Go: `func (c *Checker) checkGrammarClassLikeDeclaration(node) bool`.
    pub(crate) fn check_grammar_class_like_declaration(&mut self, node: NodeId) -> bool {
        let file = self.get_source_file_node_of(node);
        self.check_grammar_class_declaration_heritage_clauses(node, file)
            || {
                let type_parameters = self.class_like_type_parameter_list(node);
                self.check_grammar_type_parameter_list(type_parameters, file)
            }
    }

    /// Go: `node.TypeParameterList()` on the class-like kinds.
    fn class_like_type_parameter_list(&self, node: NodeId) -> Option<NodeList> {
        let n = self.store().node(node);
        match &n.data {
            tsc_ast::NodeData::ClassDeclaration(d) => d.type_parameters.clone(),
            tsc_ast::NodeData::ClassExpression(d) => d.type_parameters.clone(),
            tsc_ast::NodeData::InterfaceDeclaration(d) => d.type_parameters.clone(),
            _ => None,
        }
    }

    /// Go: `func (c *Checker) checkGrammarArrowFunction(node, file) bool`.
    fn check_grammar_arrow_function(&mut self, node: NodeId, file: NodeId) -> bool {
        if self.store().node(node).kind != Kind::ArrowFunction {
            return false;
        }

        let arrow_func = self
            .store()
            .node(node)
            .as_arrow_function()
            .expect("ArrowFunction data")
            .clone();
        let type_parameters = arrow_func.type_parameters;
        if let Some(type_parameters) = &type_parameters {
            let type_param_nodes = &type_parameters.nodes;
            let has_constraint = !type_param_nodes.is_empty()
                && self
                    .store()
                    .node(type_param_nodes[0])
                    .as_type_parameter_declaration()
                    .is_some_and(|d| d.constraint.is_some());
            if !(type_param_nodes.len() > 1
                || type_parameters.has_trailing_comma(self.store())
                || has_constraint)
            {
                let file_name = self
                    .source_file_of(file)
                    .map(|f| f.file_name().as_string().to_string())
                    .unwrap_or_default();
                if file_name.ends_with(".mts") || file_name.ends_with(".cts") {
                    // TODO(danielr): should we return early here?
                    self.grammar_error_on_node(
                        type_parameters.nodes[0],
                        &diag::THIS_SYNTAX_IS_RESERVED_IN_FILES_WITH_THE_MTS_OR_CTS_EXTENSION_ADD_A_TRAILING_COMMA_OR_EXPLICIT_CONSTRAINT,
                        &[],
                    );
                }
            }
        }

        let equals_greater_than_token = arrow_func.equals_greater_than_token;
        let (arrow_full_start, arrow_full_end) = {
            let token = self.store().node(equals_greater_than_token);
            (token.loc.pos(), token.loc.end())
        };
        let text = self
            .source_file_of(file)
            .map(|f| f.text().to_string())
            .unwrap_or_default();
        let arrow_full_text = &text[arrow_full_start as usize..arrow_full_end as usize];
        arrow_full_text.chars().any(tsc_stringutil::is_line_break)
            && self.grammar_error_on_node(
                equals_greater_than_token,
                &diag::LINE_TERMINATOR_NOT_PERMITTED_BEFORE_ARROW,
                &[],
            )
    }

    /// Go: `func (c *Checker) checkGrammarIndexSignatureParameters(node) bool`.
    fn check_grammar_index_signature_parameters(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_index_signature_declaration()
            .expect("IndexSignatureDeclaration data")
            .clone();
        let parameters = d.parameters.expect("IndexSignature parameters");
        let param_nodes = &parameters.nodes;

        if param_nodes.is_empty() {
            return self.grammar_error_on_node(
                node,
                &diag::AN_INDEX_SIGNATURE_MUST_HAVE_EXACTLY_ONE_PARAMETER,
                &[],
            );
        }

        let parameter = self
            .store()
            .node(param_nodes[0])
            .as_parameter_declaration()
            .expect("ParameterDeclaration data")
            .clone();
        if param_nodes.len() != 1 {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_MUST_HAVE_EXACTLY_ONE_PARAMETER,
                &[],
            );
        }

        self.check_grammar_for_disallowed_trailing_comma(
            Some(&parameters),
            &diag::AN_INDEX_SIGNATURE_CANNOT_HAVE_A_TRAILING_COMMA,
        );
        if let Some(dot_dot_dot_token) = parameter.dot_dot_dot_token {
            return self.grammar_error_on_node(
                dot_dot_dot_token,
                &diag::AN_INDEX_SIGNATURE_CANNOT_HAVE_A_REST_PARAMETER,
                &[],
            );
        }
        if self.store().node(param_nodes[0]).modifiers().is_some() {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_CANNOT_HAVE_AN_ACCESSIBILITY_MODIFIER,
                &[],
            );
        }
        if let Some(question_token) = parameter.question_token {
            return self.grammar_error_on_node(
                question_token,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_CANNOT_HAVE_A_QUESTION_MARK,
                &[],
            );
        }
        if parameter.initializer.is_some() {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_CANNOT_HAVE_AN_INITIALIZER,
                &[],
            );
        }
        let type_node = parameter.type_;
        let Some(type_node) = type_node else {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_MUST_HAVE_A_TYPE_ANNOTATION,
                &[],
            );
        };
        let t = self.get_type_from_type_node(type_node);
        if crate::flow::some_type_c(self, t, |c, t| {
            c.type_(t)
                .flags
                .intersects(crate::types::TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
        }) || self.is_generic_type(t)
        {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_TYPE_CANNOT_BE_A_LITERAL_TYPE_OR_GENERIC_TYPE_CONSIDER_USING_A_MAPPED_OBJECT_TYPE_INSTEAD,
                &[],
            );
        }
        if !crate::flow::every_type_c(self, t, |c, t| c.is_valid_index_key_type(t)) {
            return self.grammar_error_on_node(
                parameter.name,
                &diag::AN_INDEX_SIGNATURE_PARAMETER_TYPE_MUST_BE_STRING_NUMBER_SYMBOL_OR_A_TEMPLATE_LITERAL_TYPE,
                &[],
            );
        }
        if d.type_.is_none() {
            return self.grammar_error_on_node(
                node,
                &diag::AN_INDEX_SIGNATURE_MUST_HAVE_A_TYPE_ANNOTATION,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarIndexSignature(node) bool`.
    pub(crate) fn check_grammar_index_signature(&mut self, node: NodeId) -> bool {
        // Prevent cascading error by short-circuit
        self.check_grammar_modifiers(node) || self.check_grammar_index_signature_parameters(node)
    }

    /// Go: `func (c *Checker) checkGrammarForAtLeastOneTypeArgument(node, typeArguments) bool`.
    fn check_grammar_for_at_least_one_type_argument(
        &mut self,
        node: NodeId,
        type_arguments: &NodeList,
    ) -> bool {
        if type_arguments.nodes.is_empty() {
            let source_file = self.get_source_file_node_of(node);
            let start = type_arguments.pos() - 1; // len("<")
            let text = self
                .source_file_of(source_file)
                .map(|f| f.text().to_string())
                .unwrap_or_default();
            let end = tsc_scanner::skip_trivia(&text, type_arguments.end()) + 1; // len(">")
            return self.grammar_error_at_pos(
                source_file,
                start,
                end - start,
                &diag::TYPE_ARGUMENT_LIST_CANNOT_BE_EMPTY,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarTypeArguments(node, typeArguments) bool`.
    ///
    /// PORT: the list arrives as the node's inline `Option<NodeList>` (Go
    /// reads `node.TypeArgumentList()`); the callers extract it with
    /// `type_argument_list_of`.
    pub(crate) fn check_grammar_type_arguments(
        &mut self,
        node: NodeId,
        type_arguments: Option<&NodeList>,
    ) -> bool {
        if let Some(list) = type_arguments {
            if self.check_grammar_for_disallowed_trailing_comma(
                Some(list),
                &diag::TRAILING_COMMA_NOT_ALLOWED,
            ) {
                return true;
            }
            return self.check_grammar_for_at_least_one_type_argument(node, list);
        }
        false
    }
}

/// Go: `node.TypeArgumentList()` — the type-arguments list of the kinds the
/// grammar checks read (CallExpression, NewExpression, TaggedTemplate,
/// TypeReferenceNode, ExpressionWithTypeArguments, TypeQuery).
pub(crate) fn type_argument_list_of(store: &dyn NodeStore, node: NodeId) -> Option<&NodeList> {
    let n = store.node(node);
    match &n.data {
        tsc_ast::NodeData::CallExpression(d) => d.type_arguments.as_ref(),
        tsc_ast::NodeData::NewExpression(d) => d.type_arguments.as_ref(),
        tsc_ast::NodeData::TaggedTemplateExpression(d) => d.type_arguments.as_ref(),
        tsc_ast::NodeData::TypeReferenceNode(d) => d.type_arguments.as_ref(),
        tsc_ast::NodeData::ExpressionWithTypeArguments(d) => d.type_arguments.as_ref(),
        tsc_ast::NodeData::TypeQueryNode(d) => d.type_arguments.as_ref(),
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The remaining grammar family (grammarchecks.go:859-2266)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkGrammarTaggedTemplateChain(node) bool`.
    pub(crate) fn check_grammar_tagged_template_chain(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_tagged_template_expression()
            .expect("TaggedTemplateExpression data");
        if d.question_dot_token != NodeId::NONE
            || self.store().node(node).flags.intersects(NF::OPTIONAL_CHAIN)
        {
            return self.grammar_error_on_node(
                d.template,
                &diag::TAGGED_TEMPLATE_EXPRESSIONS_ARE_NOT_PERMITTED_IN_AN_OPTIONAL_CHAIN,
                &[],
            );
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarHeritageClause(node) bool`.
    fn check_grammar_heritage_clause(&mut self, node: NodeId) -> bool {
        let d = self
            .store()
            .node(node)
            .as_heritage_clause()
            .expect("HeritageClause data")
            .clone();
        let types = d.types;
        if self.check_grammar_for_disallowed_trailing_comma(
            types.as_ref(),
            &diag::TRAILING_COMMA_NOT_ALLOWED,
        ) {
            return true;
        }
        if let Some(types) = &types
            && types.nodes.is_empty()
        {
            let list_type = token_to_string(d.token).to_string();
            // TODO(danielr): why not error on the token?
            return self.grammar_error_at_pos(
                node,
                types.pos(),
                0,
                &diag::X_0_LIST_CANNOT_BE_EMPTY,
                &[list_type],
            );
        }

        if let Some(types) = &types {
            for &heritage_type in types.nodes.iter() {
                if self.check_grammar_expression_with_type_arguments(heritage_type) {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarExpressionWithTypeArguments(node) bool`.
    pub(crate) fn check_grammar_expression_with_type_arguments(&mut self, node: NodeId) -> bool {
        if self.store().node(node).kind == Kind::ExpressionWithTypeArguments
            && crate::ast_shims::expression_of(self.store(), node)
                .is_some_and(|e| self.store().node(e).kind == Kind::ImportKeyword)
            && type_argument_list_of(self.store(), node).is_some()
        {
            return self.grammar_error_on_node(
                node,
                &diag::THIS_USE_OF_IMPORT_IS_INVALID_IMPORT_CALLS_CAN_BE_WRITTEN_BUT_THEY_MUST_HAVE_PARENTHESES_AND_CANNOT_HAVE_TYPE_ARGUMENTS,
                &[],
            );
        }
        let type_arguments = type_argument_list_of(self.store(), node).cloned();
        self.check_grammar_type_arguments(node, type_arguments.as_ref())
    }

    /// Go: `func (c *Checker) checkGrammarClassDeclarationHeritageClauses(node, file) bool`.
    fn check_grammar_class_declaration_heritage_clauses(&mut self, node: NodeId, file: NodeId) -> bool {
        let _ = file;
        let mut seen_extends_clause = false;
        let mut seen_implements_clause = false;

        let heritage_clauses = {
            let n = self.store().node(node);
            match &n.data {
                // ClassDeclaration | ClassExpression | InterfaceDeclaration —
                // the three class-like kinds with HeritageClauses.
                tsc_ast::NodeData::ClassDeclaration(d) => d.heritage_clauses.clone(),
                tsc_ast::NodeData::ClassExpression(d) => d.heritage_clauses.clone(),
                tsc_ast::NodeData::InterfaceDeclaration(d) => d.heritage_clauses.clone(),
                _ => None,
            }
        };

        if !self.check_grammar_modifiers(node)
            && let Some(heritage_clauses) = &heritage_clauses
        {
            for &heritage_clause_node in heritage_clauses.nodes.iter() {
                let heritage_clause = self
                    .store()
                    .node(heritage_clause_node)
                    .as_heritage_clause()
                    .expect("HeritageClause data")
                    .clone();
                if heritage_clause.token == Kind::ExtendsKeyword {
                    if seen_extends_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            &diag::X_EXTENDS_CLAUSE_ALREADY_SEEN,
                            &[],
                        );
                    }

                    if seen_implements_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            &diag::X_EXTENDS_CLAUSE_MUST_PRECEDE_IMPLEMENTS_CLAUSE,
                            &[],
                        );
                    }

                    let type_nodes = heritage_clause
                        .types
                        .as_ref()
                        .map(|t| t.nodes.to_vec())
                        .unwrap_or_default();
                    if type_nodes.len() > 1 {
                        return self.grammar_error_on_first_token(
                            type_nodes[1],
                            &diag::CLASSES_CAN_ONLY_EXTEND_A_SINGLE_CLASS,
                            &[],
                        );
                    }

                    seen_extends_clause = true;
                } else {
                    if heritage_clause.token != Kind::ImplementsKeyword {
                        panic!("Unexpected token {:?}", heritage_clause.token);
                    }
                    if seen_implements_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            &diag::X_IMPLEMENTS_CLAUSE_ALREADY_SEEN,
                            &[],
                        );
                    }

                    seen_implements_clause = true;
                }

                // Grammar checking heritageClause inside class declaration
                self.check_grammar_heritage_clause(heritage_clause_node);
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarInterfaceDeclaration(node) bool`.
    pub(crate) fn check_grammar_interface_declaration(&mut self, node: NodeId) -> bool {
        let heritage_clauses = self
            .store()
            .node(node)
            .as_interface_declaration()
            .and_then(|d| d.heritage_clauses.clone());
        if let Some(heritage_clauses) = &heritage_clauses {
            let mut seen_extends_clause = false;
            for &heritage_clause_node in heritage_clauses.nodes.iter() {
                let heritage_clause = self
                    .store()
                    .node(heritage_clause_node)
                    .as_heritage_clause()
                    .expect("HeritageClause data")
                    .clone();

                match heritage_clause.token {
                    Kind::ExtendsKeyword => {
                        if seen_extends_clause {
                            return self.grammar_error_on_first_token(
                                heritage_clause_node,
                                &diag::X_EXTENDS_CLAUSE_ALREADY_SEEN,
                                &[],
                            );
                        }
                        seen_extends_clause = true;
                    }
                    Kind::ImplementsKeyword => {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            &diag::INTERFACE_DECLARATION_CANNOT_HAVE_IMPLEMENTS_CLAUSE,
                            &[],
                        );
                    }
                    _ => panic!("Unexpected token {:?}", heritage_clause.token),
                }

                // Grammar checking heritageClause inside class declaration
                self.check_grammar_heritage_clause(heritage_clause_node);
            }
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarComputedPropertyName(node) bool`.
    ///
    /// PORT: replaces the S3-era `check_grammar_computed_property_name_stub`
    /// (which answered the errorType); the *Type spine stays with
    /// `check_computed_property_name` (expressions.rs).
    pub(crate) fn check_grammar_computed_property_name(&mut self, node: NodeId) -> bool {
        // If node is not a computedPropertyName, just skip the grammar checking
        if self.store().node(node).kind != Kind::ComputedPropertyName {
            return false;
        }

        let expression = self
            .store()
            .node(node)
            .as_computed_property_name()
            .map(|d| d.expression);
        if let Some(expression) = expression {
            let is_comma = self.store().node(expression).kind == Kind::BinaryExpression
                && self
                    .store()
                    .node(expression)
                    .as_binary_expression()
                    .is_some_and(|d| {
                        self.store().node(d.operator_token).kind == Kind::CommaToken
                    });
            if is_comma {
                return self.grammar_error_on_node(
                    expression,
                    &diag::A_COMMA_EXPRESSION_IS_NOT_ALLOWED_IN_A_COMPUTED_PROPERTY_NAME,
                    &[],
                );
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarForGenerator(node) bool`.
    pub(crate) fn check_grammar_for_generator(&mut self, node: NodeId) -> bool {
        let asterisk_token = self.asterisk_token_of(node);
        if let Some(asterisk_token) = asterisk_token {
            if !matches!(
                self.store().node(node).kind,
                Kind::FunctionDeclaration | Kind::FunctionExpression | Kind::MethodDeclaration
            ) {
                panic!("Unexpected node kind {:?}", self.store().node(node).kind);
            }
            if self.store().node(node).flags.intersects(NF::AMBIENT) {
                return self.grammar_error_on_node(
                    asterisk_token,
                    &diag::GENERATORS_ARE_NOT_ALLOWED_IN_AN_AMBIENT_CONTEXT,
                    &[],
                );
            }
            if body_of(self.store(), node).is_none() {
                return self.grammar_error_on_node(
                    asterisk_token,
                    &diag::AN_OVERLOAD_SIGNATURE_CANNOT_BE_DECLARED_AS_A_GENERATOR,
                    &[],
                );
            }
        }

        false
    }

    /// Go: `node.BodyData().AsteriskToken` — the function-like kinds carry
    /// the shared asterisk token (BodyData's read).
    fn asterisk_token_of(&self, node: NodeId) -> Option<NodeId> {
        let n = self.store().node(node);
        match &n.data {
            tsc_ast::NodeData::FunctionDeclaration(d) => d.asterisk_token,
            tsc_ast::NodeData::FunctionExpression(d) => d.asterisk_token,
            tsc_ast::NodeData::MethodDeclaration(d) => d.asterisk_token,
            tsc_ast::NodeData::GetAccessorDeclaration(d) => d.asterisk_token,
            tsc_ast::NodeData::SetAccessorDeclaration(d) => d.asterisk_token,
            _ => None,
        }
    }

    /// Go: `func (c *Checker) checkGrammarForInvalidQuestionMark(postfixToken, message) bool`.
    fn check_grammar_for_invalid_question_mark(
        &mut self,
        postfix_token: Option<NodeId>,
        message: &'static Message,
    ) -> bool {
        match postfix_token {
            Some(token) if self.store().node(token).kind == Kind::QuestionToken => {
                self.grammar_error_on_node(token, message, &[])
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) checkGrammarForInvalidExclamationToken(postfixToken, message) bool`.
    fn check_grammar_for_invalid_exclamation_token(
        &mut self,
        postfix_token: Option<NodeId>,
        message: &'static Message,
    ) -> bool {
        match postfix_token {
            Some(token) if self.store().node(token).kind == Kind::ExclamationToken => {
                self.grammar_error_on_node(token, message, &[])
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) checkGrammarObjectLiteralExpression(node, inDestructuring) bool`
    /// (grammarchecks.go:998, ~140 lines).
    ///
    /// PORT: `seen` is a plain map (Go's map iteration is only by key here;
    /// insertion order is irrelevant to the read).
    pub(crate) fn check_grammar_object_literal_expression(
        &mut self,
        node: NodeId,
        in_destructuring: bool,
    ) -> bool {
        let mut seen: rustc_hash::FxHashMap<String, DeclarationMeaning> =
            rustc_hash::FxHashMap::default();

        let properties = self
            .store()
            .node(node)
            .as_object_literal_expression()
            .and_then(|d| d.properties.as_ref().map(|l| l.nodes.to_vec()))
            .unwrap_or_default();
        for prop in properties {
            if self.store().node(prop).kind == Kind::SpreadAssignment {
                let spread_assignment = self
                    .store()
                    .node(prop)
                    .as_spread_assignment()
                    .map(|d| d.expression);
                if in_destructuring {
                    // a rest property cannot be destructured any further
                    if let Some(spread_assignment_original) = spread_assignment {
                        let expression = tsc_ast::utilities::skip_parentheses(self.store(), spread_assignment_original);
                        if matches!(
                            self.store().node(expression).kind,
                            Kind::ArrayLiteralExpression | Kind::ObjectLiteralExpression
                        ) {
                            // Go errors on the spread's original expression.
                            return self.grammar_error_on_node(
                                spread_assignment_original,
                                &diag::A_REST_ELEMENT_CANNOT_CONTAIN_A_BINDING_PATTERN,
                                &[],
                            );
                        }
                    }
                }
                continue;
            }
            // Go: prop.Name() — always present for object-literal properties.
            let name = self
                .store()
                .node(prop)
                .name()
                .expect("object literal property has a name");
            if self.store().node(name).kind == Kind::ComputedPropertyName {
                // If the name is not a ComputedPropertyName, the grammar checking will skip it
                self.check_grammar_computed_property_name(name);
            }

            if self.store().node(prop).kind == Kind::ShorthandPropertyAssignment
                && !in_destructuring
            {
                let shorthand = self
                    .store()
                    .node(prop)
                    .as_shorthand_property_assignment()
                    .expect("ShorthandPropertyAssignment data")
                    .clone();
                if let Some(object_assignment_initializer) = shorthand.object_assignment_initializer {
                    // having objectAssignmentInitializer is only valid in an ObjectAssignmentPattern.
                    // Outside of destructuring, it is a syntax error.

                    // Try to grab the last node prior to the initializer,
                    // then error on the first token following (which should be the `=` token).
                    let last_node_before_initializer = self
                        .last_child_before(prop, object_assignment_initializer);
                    self.grammar_error_on_first_token(
                        last_node_before_initializer,
                        &diag::DID_YOU_MEAN_TO_USE_A_COLON_AN_CAN_ONLY_FOLLOW_A_PROPERTY_NAME_WHEN_THE_CONTAINING_OBJECT_LITERAL_IS_PART_OF_A_DESTRUCTURING_PATTERN,
                        &[],
                    );
                }
            }

            if self.store().node(name).kind == Kind::PrivateIdentifier {
                self.grammar_error_on_node(
                    name,
                    &diag::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
                    &[],
                );
            }

            // Modifiers are never allowed on properties except for 'async' on a method declaration
            let modifiers = self
                .store()
                .node(prop)
                .modifiers()
                .map(|m| m.nodes.to_vec());
            if let Some(modifiers) = modifiers
                && !modifiers.is_empty()
            {
                if can_have_modifiers(self.store().node(prop)) {
                    for mod_ in modifiers {
                        if is_modifier(self.store().node(mod_))
                            && (self.store().node(mod_).kind != Kind::AsyncKeyword
                                || self.store().node(prop).kind != Kind::MethodDeclaration)
                        {
                            self.grammar_error_on_node(
                                mod_,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_HERE,
                                &[get_text_of_node(self.store(), mod_)],
                            );
                        }
                    }
                } else if can_have_illegal_modifiers(self.store().node(prop)) {
                    for mod_ in modifiers {
                        if is_modifier(self.store().node(mod_)) {
                            self.grammar_error_on_node(
                                mod_,
                                &diag::X_0_MODIFIER_CANNOT_BE_USED_HERE,
                                &[get_text_of_node(self.store(), mod_)],
                            );
                        }
                    }
                }
            }

            // ECMA-262 11.1.5 Object Initializer (Go's comment preserved)
            let current_kind = match self.store().node(prop).kind {
                Kind::ShorthandPropertyAssignment | Kind::PropertyAssignment => {
                    // Grammar checking for computedPropertyName and shorthandPropertyAssignment
                    let postfix_token = postfix_token_of(self.store(), prop);
                    self.check_grammar_for_invalid_exclamation_token(
                        postfix_token,
                        &diag::A_DEFINITE_ASSIGNMENT_ASSERTION_IS_NOT_PERMITTED_IN_THIS_CONTEXT,
                    );
                    self.check_grammar_for_invalid_question_mark(
                        postfix_token,
                        &diag::AN_OBJECT_MEMBER_CANNOT_BE_DECLARED_OPTIONAL,
                    );

                    if self.store().node(name).kind == Kind::NumericLiteral {
                        self.check_grammar_numeric_literal(name);
                    }

                    if self.store().node(name).kind == Kind::BigIntLiteral {
                        self.add_error_or_suggestion(
                            true,
                            u::new_diagnostic_for_node(
                                self.store(),
                                name,
                                &diag::A_BIGINT_LITERAL_CANNOT_BE_USED_AS_A_PROPERTY_NAME,
                                vec![],
                            ),
                        );
                    }

                    DeclarationMeaning::PROPERTY_ASSIGNMENT
                }
                Kind::MethodDeclaration => DeclarationMeaning::METHOD,
                Kind::GetAccessor => DeclarationMeaning::GET_ACCESSOR,
                Kind::SetAccessor => DeclarationMeaning::SET_ACCESSOR,
                kind => panic!("Unexpected node kind {:?}", kind),
            };

            if !in_destructuring {
                let (effective_name, ok) =
                    self.get_effective_property_name_for_property_name_node(name);
                if !ok {
                    continue;
                }

                let existing_kind = seen.get(&effective_name).copied().unwrap_or(DeclarationMeaning::NONE);
                if existing_kind == DeclarationMeaning::NONE {
                    seen.insert(effective_name, current_kind);
                } else {
                    if current_kind.intersects(DeclarationMeaning::METHOD)
                        && existing_kind.intersects(DeclarationMeaning::METHOD)
                    {
                        self.grammar_error_on_node(
                            name,
                            &diag::DUPLICATE_IDENTIFIER_0,
                            &[get_text_of_node(self.store(), name)],
                        );
                    } else if current_kind.intersects(DeclarationMeaning::PROPERTY_ASSIGNMENT)
                        && existing_kind.intersects(DeclarationMeaning::PROPERTY_ASSIGNMENT)
                    {
                        // PORT (FCS tranche 58): Go grammarchecks.go:1121
                        // does NOT return after this report — the loop
                        // continues so EVERY later duplicate property
                        // reports (e.g. `{ b: 1, ...o, b: 2, ...o2, b: 3 }`
                        // reports at both b:2 and b:3; the former early
                        // return dropped the b:3 TS1117 — the tranche-26
                        // "spread seam" was this, un-root-caused until now).
                        self.grammar_error_on_node(
                            name,
                            &diag::AN_OBJECT_LITERAL_CANNOT_HAVE_MULTIPLE_PROPERTIES_WITH_THE_SAME_NAME,
                            &[get_text_of_node(self.store(), name)],
                        );
                    } else if current_kind.intersects(DeclarationMeaning::GET_OR_SET_ACCESSOR)
                        && existing_kind.intersects(DeclarationMeaning::GET_OR_SET_ACCESSOR)
                    {
                        if existing_kind != DeclarationMeaning::GET_OR_SET_ACCESSOR
                            && current_kind != existing_kind
                        {
                            seen.insert(effective_name, current_kind | existing_kind);
                        } else {
                            return self.grammar_error_on_node(
                                name,
                                &diag::AN_OBJECT_LITERAL_CANNOT_HAVE_MULTIPLE_GET_SLASHSET_ACCESSORS_WITH_THE_SAME_NAME,
                                &[],
                            );
                        }
                    } else {
                        return self.grammar_error_on_node(
                            name,
                            &diag::AN_OBJECT_LITERAL_CANNOT_HAVE_PROPERTY_AND_ACCESSOR_WITH_THE_SAME_NAME,
                            &[],
                        );
                    }
                }
            }
        }

        false
    }

    /// Go: the ForEachChild walk in the shorthand-property arm — the last
    /// child before `stop_at` (the walk stops AT the initializer).
    fn last_child_before(&self, node: NodeId, stop_at: NodeId) -> NodeId {
        let mut last = NodeId::NONE;
        self.store().node(node).for_each_child(&mut |child| {
            if child == stop_at {
                return true;
            }
            last = child;
            false
        });
        last
    }

    /// Go: `func (c *Checker) checkGrammarJsxElement(node) bool` — the JSX
    /// family (jsx.go's wave owns the leaf callers; the bodies land here).
    /// The node is the JsxOpeningLikeElement (Go reads node.TagName(),
    /// node.Attributes().Properties(), node.TypeArgumentList()).
    pub(crate) fn check_grammar_jsx_element(&mut self, node: NodeId) -> bool {
        let (tag_name, attributes) = {
            let n = self.store().node(node);
            if let Some(d) = n.as_jsx_opening_element() {
                (Some(d.tag_name), Some(d.attributes))
            } else if let Some(d) = n.as_jsx_self_closing_element() {
                (Some(d.tag_name), Some(d.attributes))
            } else {
                (None, None)
            }
        };
        if let Some(tag_name) = tag_name {
            self.check_grammar_jsx_name(tag_name);
        }
        let type_arguments = type_argument_list_of(self.store(), node).cloned();
        self.check_grammar_type_arguments(node, type_arguments.as_ref());
        let mut seen: rustc_hash::FxHashSet<String> = rustc_hash::FxHashSet::default();
        let properties = attributes
            .and_then(|a| self.store().node(a).as_jsx_attributes())
            .and_then(|attrs| attrs.properties.as_ref())
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        for attr_node in properties {
            if self.store().node(attr_node).kind == Kind::JsxSpreadAttribute {
                continue;
            }
            let (name, initializer) = {
                let attr = self
                    .store()
                    .node(attr_node)
                    .as_jsx_attribute()
                    .expect("JsxAttribute data");
                (attr.name, attr.initializer)
            };
            // PORT(FCS, 2026-10-14): Go's name.Text() allocates for
            // KindJsxNamespacedName ("ns:name"); use the owned Go-exact form
            // (grammarchecks.go:1149).
            let text_of_name = tsc_ast::node_text_owned(self.store(), name);
            if seen.contains(&text_of_name) {
                return self.grammar_error_on_node(
                    name,
                    &diag::JSX_ELEMENTS_CANNOT_HAVE_MULTIPLE_ATTRIBUTES_WITH_THE_SAME_NAME,
                    &[],
                );
            }
            seen.insert(text_of_name);
            if let Some(initializer) = initializer
                && self.store().node(initializer).kind == Kind::JsxExpression
                && self
                    .store()
                    .node(initializer)
                    .as_jsx_expression()
                    .and_then(|d| d.expression)
                    .is_none()
            {
                return self.grammar_error_on_node(
                    initializer,
                    &diag::JSX_ATTRIBUTES_MUST_ONLY_BE_ASSIGNED_A_NON_EMPTY_EXPRESSION,
                    &[],
                );
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkGrammarJsxName(node) bool`.
    pub(crate) fn check_grammar_jsx_name(&mut self, node: NodeId) -> bool {
        if self.store().node(node).kind == Kind::PropertyAccessExpression
            && crate::ast_shims::expression_of(self.store(), node)
                .is_some_and(|e| self.store().node(e).kind == Kind::JsxNamespacedName)
        {
            let expression = crate::ast_shims::expression_of(self.store(), node).expect("checked");
            return self.grammar_error_on_node(
                expression,
                &diag::JSX_PROPERTY_ACCESS_EXPRESSIONS_CANNOT_INCLUDE_JSX_NAMESPACE_NAMES,
                &[],
            );
        }

        if self.store().node(node).kind == Kind::JsxNamespacedName
            && self.compiler_options.get_jsx_transform_enabled()
            && self
                .store()
                .node(node)
                .as_jsx_namespaced_name()
                .is_some_and(|d| {
                    !tsc_scanner::utilities::is_intrinsic_jsx_name(
                        tsc_ast::node_text(self.store(), d.namespace),
                    )
                })
        {
            return self.grammar_error_on_node(
                node,
                &diag::REACT_COMPONENTS_CANNOT_INCLUDE_JSX_NAMESPACE_NAMES,
                &[],
            );
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarJsxExpression(node) bool`.
    pub(crate) fn check_grammar_jsx_expression(&mut self, node: NodeId) -> bool {
        let expression = self
            .store()
            .node(node)
            .as_jsx_expression()
            .and_then(|d| d.expression);
        if let Some(expression) = expression
            && is_comma_sequence(self.store(), expression)
        {
            return self.grammar_error_on_node(
                expression,
                &diag::JSX_EXPRESSIONS_MAY_NOT_USE_THE_COMMA_OPERATOR_DID_YOU_MEAN_TO_WRITE_AN_ARRAY,
                &[],
            );
        }

        false
    }

    /// Go: `func (c *Checker) checkGrammarForInOrOfStatement(forInOrOfStatement) bool`
    /// (grammarchecks.go:1182, ~105 lines).
    ///
    /// PORT: the module-kind switches Go writes with fallthrough become the
    /// explicit case partitions (same predicates, same arms).
    pub(crate) fn check_grammar_for_in_or_for_of_statement(&mut self, node: NodeId) -> bool {
        if self.check_grammar_statement_in_ambient_context(node) {
            return true;
        }

        let d = self
            .store()
            .node(node)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement data")
            .clone();

        if self.store().node(node).kind == Kind::ForOfStatement
            && let Some(await_modifier) = d.await_modifier
            && !self.store().node(node).flags.intersects(NF::AWAIT_CONTEXT)
        {
            {
                let source_file = self.get_source_file_node_of(node);
                if is_in_top_level_context(self.store(), node) {
                    if !self.has_parse_diagnostics(source_file) {
                        let is_module = self
                            .source_file_of(source_file)
                            .is_some_and(|f| is_effective_external_module(f, self.module_kind));
                        if !is_module {
                            self.add_diagnostic(u::new_diagnostic_for_node(
                                self.store(),
                                await_modifier,
                                &diag::X_FOR_AWAIT_LOOPS_ARE_ONLY_ALLOWED_AT_THE_TOP_LEVEL_OF_A_FILE_WHEN_THAT_FILE_IS_A_MODULE_BUT_THIS_FILE_HAS_NO_IMPORTS_OR_EXPORTS_CONSIDER_ADDING_AN_EMPTY_EXPORT_TO_MAKE_THIS_FILE_A_MODULE,
                                vec![],
                            ));
                        }
                        // Go: the module-kind switch (Node16-Next → the
                        // implied-node-format CommonJS arm, then the ES2022/
                        // ESNext/Preserve/System target gate, else the
                        // top-level error).
                        self.report_await_modifier_module_diagnostics(
                            await_modifier,
                            source_file,
                        );
                    }
                } else {
                    // use of 'for-await-of' in non-async function
                    if !self.has_parse_diagnostics(source_file) {
                        let mut diagnostic = u::new_diagnostic_for_node(
                            self.store(),
                            await_modifier,
                            &diag::X_FOR_AWAIT_LOOPS_ARE_ONLY_ALLOWED_WITHIN_ASYNC_FUNCTIONS_AND_AT_THE_TOP_LEVELS_OF_MODULES,
                            vec![],
                        );
                        let containing_func =
                            u::get_containing_function(self.store(), node);
                        if let Some(containing_func) = containing_func
                            && self.store().node(containing_func).kind != Kind::Constructor
                        {
                            debug_assert!(
                                !tsc_ast::get_function_flags(self.store(), containing_func)
                                    .intersects(tsc_ast::functionflags::FunctionFlags::ASYNC),
                                "Enclosing function should never be an async function."
                            );
                            let related_info = u::new_diagnostic_for_node(
                                self.store(),
                                containing_func,
                                &diag::DID_YOU_MEAN_TO_MARK_THIS_FUNCTION_AS_ASYNC,
                                vec![],
                            );
                            diagnostic.add_related_info(related_info);
                        }
                        self.add_diagnostic(diagnostic);
                        return true;
                    }
                }
            }
        }

        if self.store().node(node).kind == Kind::ForOfStatement
            && !self.store().node(node).flags.intersects(NF::AWAIT_CONTEXT)
            && self.store().node(d.initializer).kind == Kind::Identifier
            && tsc_ast::node_text(self.store(), d.initializer) == "async"
        {
            self.grammar_error_on_node(
                d.initializer,
                &diag::THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_MAY_NOT_BE_ASYNC,
                &[],
            );
            return false;
        }

        if self.store().node(d.initializer).kind == Kind::VariableDeclarationList {
            let variable_list = d.initializer;
            if !self.check_grammar_variable_declaration_list(variable_list) {
                let declarations = self
                    .store()
                    .node(variable_list)
                    .as_variable_declaration_list()
                    .and_then(|l| l.declarations.clone())
                    .expect("VariableDeclarationList declarations");

                // declarations.length can be zero if there is an error in variable declaration in for-of or for-in
                // See http://www.ecma-international.org/ecma-262/6.0/#sec-for-in-and-for-of-statements for details
                if declarations.nodes.is_empty() {
                    return false;
                }

                if declarations.nodes.len() > 1 {
                    let diagnostic = if self.store().node(node).kind == Kind::ForInStatement {
                        &diag::ONLY_A_SINGLE_VARIABLE_DECLARATION_IS_ALLOWED_IN_A_FOR_IN_STATEMENT
                    } else {
                        &diag::ONLY_A_SINGLE_VARIABLE_DECLARATION_IS_ALLOWED_IN_A_FOR_OF_STATEMENT
                    };
                    return self.grammar_error_on_first_token(declarations.nodes[1], diagnostic, &[]);
                }

                let first_variable_declaration = self
                    .store()
                    .node(declarations.nodes[0])
                    .as_variable_declaration()
                    .expect("VariableDeclaration data")
                    .clone();
                if first_variable_declaration.initializer.is_some() {
                    let diagnostic = if self.store().node(node).kind == Kind::ForInStatement {
                        &diag::THE_VARIABLE_DECLARATION_OF_A_FOR_IN_STATEMENT_CANNOT_HAVE_AN_INITIALIZER
                    } else {
                        &diag::THE_VARIABLE_DECLARATION_OF_A_FOR_OF_STATEMENT_CANNOT_HAVE_AN_INITIALIZER
                    };
                    return self.grammar_error_on_node(
                        first_variable_declaration.name,
                        diagnostic,
                        &[],
                    );
                }
                if first_variable_declaration.type_.is_some() {
                    let diagnostic = if self.store().node(node).kind == Kind::ForInStatement {
                        &diag::THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_USE_A_TYPE_ANNOTATION
                    } else {
                        &diag::THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_CANNOT_USE_A_TYPE_ANNOTATION
                    };
                    return self.grammar_error_on_node(declarations.nodes[0], diagnostic, &[]);
                }
            }
        }

        false
    }

    /// The module-kind fallthrough switch both await arms share (Go:
    /// checkGrammarForInOrOfStatement's Node16-Next / ES2022-Next-Preserve-
    /// System / default partition).
    fn report_await_modifier_module_diagnostics(
        &mut self,
        await_modifier: NodeId,
        source_file: NodeId,
    ) {
        let is_node_kind = ModuleKind::Node16 <= self.module_kind && self.module_kind <= ModuleKind::NodeNext;
        let is_es2022_like = matches!(
            self.module_kind,
            ModuleKind::ES2022 | ModuleKind::ESNext | ModuleKind::Preserve | ModuleKind::System
        );
        if is_node_kind {
            if self.program.source_file_implied_node_format(source_file) == ModuleKind::CommonJS {
                self.add_diagnostic(u::new_diagnostic_for_node(
                    self.store(),
                    await_modifier,
                    &diag::THE_CURRENT_FILE_IS_A_COMMONJS_MODULE_AND_CANNOT_USE_AWAIT_AT_THE_TOP_LEVEL,
                    vec![],
                ));
                return;
            }
            // Go: fallthrough into the ES2022-like target gate.
            if self.language_version >= ScriptTarget::ES2017 {
                return;
            }
            self.add_diagnostic(u::new_diagnostic_for_node(
                self.store(),
                await_modifier,
                &diag::TOP_LEVEL_FOR_AWAIT_LOOPS_ARE_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE_AND_THE_TARGET_OPTION_IS_SET_TO_ES2017_OR_HIGHER,
                vec![],
            ));
            return;
        }
        if is_es2022_like {
            if self.language_version >= ScriptTarget::ES2017 {
                return;
            }
            self.add_diagnostic(u::new_diagnostic_for_node(
                self.store(),
                await_modifier,
                &diag::TOP_LEVEL_FOR_AWAIT_LOOPS_ARE_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE_AND_THE_TARGET_OPTION_IS_SET_TO_ES2017_OR_HIGHER,
                vec![],
            ));
            return;
        }
        self.add_diagnostic(u::new_diagnostic_for_node(
            self.store(),
            await_modifier,
            &diag::TOP_LEVEL_FOR_AWAIT_LOOPS_ARE_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_NODENEXT_OR_PRESERVE_AND_THE_TARGET_OPTION_IS_SET_TO_ES2017_OR_HIGHER,
            vec![],
        ));
    }
}
