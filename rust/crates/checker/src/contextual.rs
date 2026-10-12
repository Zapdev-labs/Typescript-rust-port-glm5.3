//! M6 S9 — the contextual-typing engine (Go checker.go:29891-31523 +
//! 10443-10590).
//!
//! Go's `getContextualType` dispatcher, the contextual-signature family, and
//! `getApparentTypeOfContextualType`/`instantiateContextualType`, composed
//! over the landed S9 conditional/awaited machinery, S4's contextual stacks
//! (`contextual_infos` / `inference_context_infos`), S8's signature
//! machinery, and the S3 iteration family. JSX contextual arms stay W5,
//! decorator arms W4, per the dispatch table.

use crate::checker::Checker;
use crate::types::{ContextFlags, TypeFlags, TypeId};
use tsc_ast::{Kind, NodeData, NodeId};
use tsc_ast::symbolflags::SymbolFlags;
use tsc_jsnum::Number;
use tsc_binder::ast_shims as binder_shims;

// ────────────────────────────────────────────────────────────────────────────
// Small local node accessors (Go's generated `nodeData.X()` family) — these
// mirror the Option-into-Option accessors Go's Node interface provides.
// ────────────────────────────────────────────────────────────────────────────

fn parent_of(c: &Checker, node: NodeId) -> Option<NodeId> {
    let p = c.store().node(node).parent.get();
    (p != NodeId::NONE).then_some(p)
}

fn initializer_of_node(c: &Checker, node: NodeId) -> Option<NodeId> {
    crate::ast_shims::initializer_of(c.store(), node)
}

fn type_of_node(c: &Checker, node: NodeId) -> Option<NodeId> {
    crate::ast_shims::type_of_node(c.store(), node)
}

fn parameters_of_node(c: &Checker, node: NodeId) -> &[NodeId] {
    crate::ast_shims::parameters_of(c.store(), node)
}

fn properties_of_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Vec<NodeId> {
    match &store.node(node).data {
        NodeData::ObjectLiteralExpression(d) => d
            .properties
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn elements_of_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Vec<NodeId> {
    match &store.node(node).data {
        NodeData::ArrayLiteralExpression(d) => d
            .elements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        NodeData::BindingPattern(d) => d
            .elements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn expression_of_node(c: &Checker, node: NodeId) -> Option<NodeId> {
    crate::ast_shims::expression_of(c.store(), node)
}

fn body_of_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.body,
        NodeData::FunctionExpression(d) => d.body,
        NodeData::ArrowFunction(d) => d.body,
        NodeData::MethodDeclaration(d) => d.body,
        NodeData::ConstructorDeclaration(d) => d.body,
        NodeData::GetAccessorDeclaration(d) => d.body,
        NodeData::SetAccessorDeclaration(d) => d.body,
        _ => None,
    }
}

fn name_of_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        NodeData::VariableDeclaration(d) => Some(d.name),
        NodeData::ParameterDeclaration(d) => Some(d.name),
        NodeData::PropertyDeclaration(d) => Some(d.name),
        NodeData::PropertySignatureDeclaration(d) => Some(d.name),
        NodeData::PropertyAssignment(d) => Some(d.name),
        NodeData::ShorthandPropertyAssignment(d) => Some(d.name),
        NodeData::BindingElement(d) => d.property_name.or(d.name),
        _ => None,
    }
}

fn operator_kind(store: &dyn tsc_ast::NodeStore, binary: NodeId) -> Kind {
    let NodeData::BinaryExpression(d) = &store.node(binary).data else {
        return Kind::Unknown;
    };
    store.node(d.operator_token).kind
}

fn left_right_of(store: &dyn tsc_ast::NodeStore, binary: NodeId) -> Option<(NodeId, NodeId)> {
    let NodeData::BinaryExpression(d) = &store.node(binary).data else {
        return None;
    };
    Some((d.left, d.right))
}

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // The contextual stacks' readers (Go checker.go:31449 + 31504)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getInferenceContext(node *ast.Node) *InferenceContext`
    /// (checker.go:31504).
    pub(crate) fn get_inference_context(
        &self,
        node: NodeId,
    ) -> Option<crate::todo_shapes::InferenceContextId> {
        for i in (0..self.inference_context_infos.len()).rev() {
            if self.inference_context_infos[i].node == Some(node) {
                return self.inference_context_infos[i].context;
            }
        }
        None
    }

    /// Go: `func (c *Checker) findContextualNode(node *ast.Node, includeCaches bool) int`
    /// (checker.go:31449).
    pub(crate) fn find_contextual_node(&self, node: NodeId, include_caches: bool) -> Option<usize> {
        for (i, info) in self.contextual_infos.iter().enumerate() {
            if info.node == Some(node) && (include_caches || !info.is_cache) {
                return Some(i);
            }
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // The isContextSensitive family (Go checker.go:31405-31499)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) isContextSensitive(node *ast.Node) bool`
    /// (checker.go:31405).
    pub(crate) fn is_context_sensitive_impl(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::MethodDeclaration
            | Kind::FunctionDeclaration => {
                self.is_context_sensitive_function_like_declaration(node)
            }
            Kind::ObjectLiteralExpression => {
                let props = properties_of_node(self.store(), node);
                props.into_iter().any(|p| self.is_context_sensitive_impl(p))
            }
            Kind::ArrayLiteralExpression => {
                let elements = elements_of_node(self.store(), node);
                elements
                    .into_iter()
                    .any(|e| self.is_context_sensitive_impl(e))
            }
            Kind::ConditionalExpression => {
                let (when_true, when_false) = {
                    let NodeData::ConditionalExpression(c) = &self.store().node(node).data else {
                        return false;
                    };
                    (c.when_true, c.when_false)
                };
                self.is_context_sensitive_impl(when_true)
                    || self.is_context_sensitive_impl(when_false)
            }
            Kind::BinaryExpression => {
                let op = operator_kind(self.store(), node);
                if op == Kind::BarBarToken || op == Kind::QuestionQuestionToken {
                    let Some((left, right)) = left_right_of(self.store(), node) else {
                        return false;
                    };
                    return self.is_context_sensitive_impl(left)
                        || self.is_context_sensitive_impl(right);
                }
                false
            }
            Kind::PropertyAssignment => {
                let Some(init) = initializer_of_node(self, node) else {
                    return false;
                };
                self.is_context_sensitive_impl(init)
            }
            Kind::ParenthesizedExpression => {
                let Some(e) = expression_of_node(self, node) else {
                    return false;
                };
                self.is_context_sensitive_impl(e)
            }
            // M6 W5: the JSX arms (Go checker.go:31463-31474).
            Kind::JsxAttributes => {
                // Go: some(Properties, isContextSensitive) || the opening
                // element's element children (the context-sensitivity
                // reaches through the children into nested lambdas).
                let props = crate::jsx::jsx_properties_of(self.store(), node);
                let props_sensitive =
                    props.into_iter().any(|p| self.is_context_sensitive_impl(p));
                if props_sensitive {
                    return true;
                }
                let parent = self.store().node(node).parent.get();
                if parent != NodeId::NONE
                    && self.store().node(parent).kind == Kind::JsxOpeningElement
                {
                    let element = self.store().node(parent).parent.get();
                    let children = crate::jsx::jsx_children_of(self.store(), element);
                    return children
                        .into_iter()
                        .any(|child| self.is_context_sensitive_impl(child));
                }
                false
            }
            Kind::JsxAttribute => {
                // If there is no initializer, JSX attribute has a boolean
                // value of true which is not context sensitive (Go comment).
                let initializer = self
                    .store()
                    .node(node)
                    .as_jsx_attribute()
                    .expect("data")
                    .initializer;
                initializer.is_some_and(|init| self.is_context_sensitive_impl(init))
            }
            Kind::JsxExpression => {
                // It is possible to that node.expression is undefined
                // (e.g <div x={} />) (Go comment).
                let expression = self
                    .store()
                    .node(node)
                    .as_jsx_expression()
                    .expect("data")
                    .expression;
                expression.is_some_and(|e| self.is_context_sensitive_impl(e))
            }
            // TODO(port M6 S4): KindYieldExpression rides the S4 leaves.
            _ => false,
        }
    }

    /// Go: `func HasContextSensitiveParameters(node *Node) bool`
    /// (ast/utilities.go:4269) — functions with type parameters are not
    /// context sensitive; functions with any parameters that lack type
    /// annotations are context sensitive; non-arrow functions without an
    /// explicit `this` parameter have an implicit `this` parameter that is
    /// subject to contextual typing.
    fn has_context_sensitive_parameters(&mut self, node: NodeId) -> bool {
        if type_parameters_of_node(self.store(), node).is_some() {
            return false;
        }
        let parameters = parameters_of_node(self, node);
        if parameters
            .iter()
            .any(|&p| type_of_node(self, p).is_none())
        {
            return true;
        }
        if self.store().node(node).kind != Kind::ArrowFunction {
            // If the first parameter is not an explicit 'this' parameter,
            // then the function has an implicit 'this' parameter which is
            // subject to contextual typing (Go's comment).
            let this_missing = match parameters.first() {
                None => true,
                Some(&first) => !crate::ast_shims::is_this_parameter(self.store(), first),
            };
            if this_missing {
                return self
                    .store()
                    .node(node)
                    .flags
                    .intersects(tsc_ast::NodeFlags::CONTAINS_THIS);
            }
        }
        false
    }

    /// Go: `func (c *Checker) isContextSensitiveFunctionLikeDeclaration(node *ast.Node) bool`
    /// (checker.go:31480).
    pub(crate) fn is_context_sensitive_function_like_declaration(
        &mut self,
        node: NodeId,
    ) -> bool {
        self.has_context_sensitive_parameters(node)
            || self.has_context_sensitive_return_expression(node)
            || self.has_context_sensitive_yield_expression(node)
    }

    /// Go: `func (c *Checker) hasContextSensitiveReturnExpression(node *ast.Node) bool`
    /// (checker.go:31484).
    fn has_context_sensitive_return_expression(&mut self, node: NodeId) -> bool {
        if type_parameters_of_node(self.store(), node).is_some() || type_of_node(self, node).is_some()
        {
            return false;
        }
        let Some(body) = body_of_node(self.store(), node) else {
            return false;
        };
        if self.store().node(body).kind != Kind::Block {
            return self.is_context_sensitive_impl(body);
        }
        // Go: ast.ForEachReturnStatement.
        let mut return_statements = Vec::new();
        collect_return_statements(self.store(), body, &mut return_statements);
        for stmt in return_statements {
            if let Some(expr) = expression_of_node(self, stmt) {
                if self.is_context_sensitive_impl(expr) {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) hasContextSensitiveYieldExpression(node *ast.Node) bool`
    /// (checker.go:31490 region).
    fn has_context_sensitive_yield_expression(&mut self, node: NodeId) -> bool {
        let Some(body) = body_of_node(self.store(), node) else {
            return false;
        };
        let mut yield_expressions = Vec::new();
        collect_yield_expressions(self.store(), body, &mut yield_expressions);
        for expr in yield_expressions {
            if let Some(e) = expression_of_node(self, expr) {
                if self.is_context_sensitive_impl(e) {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) isContextSensitiveFunctionOrObjectLiteralMethod(fn *ast.Node) bool`
    /// (checker.go:30047).
    pub(crate) fn is_context_sensitive_function_or_object_literal_method(
        &mut self,
        fn_node: NodeId,
    ) -> bool {
        let store = self.store();
        binder_shims::is_object_literal_method(store, fn_node)
            || binder_shims::is_function_expression_or_arrow_function(store.node(fn_node))
    }

    // ────────────────────────────────────────────────────────────────────────
    // The getContextualType dispatcher (Go checker.go:29891)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getContextualType(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:29891) — the dispatcher. JSX arms are W5 per the dispatch
    /// table; the decorator (W4) and import-attribute arms are bannered below.
    pub(crate) fn get_contextual_type(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        if self
            .store()
            .node(node)
            .flags
            .intersects(tsc_ast::NodeFlags::IN_WITH_STATEMENT)
        {
            // We cannot answer semantic questions within a with block, do not
            // proceed any further (Go's comment).
            return None;
        }
        // Cached contextual types are obtained with no ContextFlags, so we
        // can only consult them for requests with no ContextFlags.
        let include_caches = context_flags == ContextFlags::NONE;
        if let Some(index) = self.find_contextual_node(node, include_caches) {
            return self.contextual_infos[index].t;
        }
        let parent = self.store().node(node).parent.get();
        if parent == NodeId::NONE {
            return None;
        }
        match self.store().node(parent).kind {
            Kind::VariableDeclaration
            | Kind::Parameter
            | Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::BindingElement => {
                self.get_contextual_type_for_initializer_expression(node, context_flags)
            }
            Kind::ArrowFunction | Kind::ReturnStatement => {
                self.get_contextual_type_for_return_expression(node, context_flags)
            }
            Kind::YieldExpression => {
                self.get_contextual_type_for_yield_operand(parent, context_flags)
            }
            Kind::AwaitExpression => {
                self.get_contextual_type_for_await_operand(parent, context_flags)
            }
            Kind::CallExpression | Kind::NewExpression => {
                self.get_contextual_type_for_argument(parent, node)
            }
            Kind::TypeAssertionExpression | Kind::AsExpression => {
                if crate::expressions::is_const_assertion(self.store(), parent) {
                    return self.get_contextual_type(parent, context_flags);
                }
                let t = type_of_node(self, parent).expect("as/assert type node");
                Some(self.get_type_from_type_node(t))
            }
            Kind::BinaryExpression => {
                self.get_contextual_type_for_binary_operand(node, context_flags)
            }
            Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment => {
                self.get_contextual_type_for_object_literal_element(parent, context_flags)
            }
            Kind::SpreadAssignment => {
                let grandparent = parent_of(self, parent)?;
                self.get_contextual_type(grandparent, context_flags)
            }
            Kind::ArrayLiteralExpression => {
                let t = self.get_apparent_type_of_contextual_type(parent, context_flags);
                let elements = elements_of_node(self.store(), parent);
                let element_index = elements.iter().position(|&e| e == node)? as i32;
                let (first_spread_index, last_spread_index) =
                    self.get_spread_indices(&elements);
                self.get_contextual_type_for_element_expression(
                    t,
                    element_index as usize,
                    elements.len() as i32,
                    first_spread_index,
                    last_spread_index,
                )
            }
            Kind::ConditionalExpression => {
                self.get_contextual_type_for_conditional_operand(node, context_flags)
            }
            Kind::TemplateSpan => {
                let grandparent = parent_of(self, parent)?;
                self.get_contextual_type_for_substitution_expression(grandparent, node)
            }
            Kind::ParenthesizedExpression | Kind::NonNullExpression => {
                self.get_contextual_type(parent, context_flags)
            }
            Kind::SatisfiesExpression => {
                let t = type_of_node(self, parent).expect("satisfies type node");
                Some(self.get_type_from_type_node(t))
            }
            Kind::ExportAssignment => {
                let t = type_of_node(self, parent);
                t.and_then(|t| self.try_get_type_from_type_node(t))
            }
            // M6 W5: the JSX arms (Go checker.go:29949-29953) — the
            // jsx.go contextual family. KindDecorator needs W4's decorator
            // signature machinery; KindImportAttribute needs the global
            // ImportAttributes type resolver.
            Kind::JsxExpression => {
                self.get_contextual_type_for_jsx_expression(parent, context_flags)
            }
            Kind::JsxAttribute | Kind::JsxSpreadAttribute => {
                self.get_contextual_type_for_jsx_attribute(parent, context_flags)
            }
            Kind::JsxOpeningElement | Kind::JsxSelfClosingElement => {
                self.get_contextual_jsx_element_attributes_type(parent, context_flags)
            }
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getContextualTypeForInitializerExpression(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:29971).
    pub(crate) fn get_contextual_type_for_initializer_expression(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let declaration = self.store().node(node).parent.get();
        if initializer_of_node(self, declaration) != Some(node) {
            return None;
        }
        if let Some(result) =
            self.get_contextual_type_for_variable_like_declaration(declaration, context_flags)
        {
            return Some(result);
        }
        if !context_flags.intersects(ContextFlags::SKIP_BINDING_PATTERNS) {
            let name = name_of_node(self.store(), declaration)?;
            if binder_shims::is_binding_pattern(self.store().node(name))
                && !elements_of_node(self.store(), name).is_empty()
            {
                return Some(
                    self.get_type_from_binding_pattern(name, true, false), /*includePatternInType, reportErrors*/
                );
            }
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForVariableLikeDeclaration(declaration *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:29986).
    pub(crate) fn get_contextual_type_for_variable_like_declaration(
        &mut self,
        declaration: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        if let Some(type_node) = type_of_node(self, declaration) {
            return Some(self.get_type_from_type_node(type_node));
        }
        match self.store().node(declaration).kind {
            Kind::Parameter => self.get_contextually_typed_parameter_type(declaration),
            Kind::BindingElement => {
                self.get_contextual_type_for_binding_element(declaration, context_flags)
            }
            Kind::PropertyDeclaration => {
                if binder_shims::is_static(self.store().node(declaration)) {
                    return self.get_contextual_type_for_static_property_declaration(
                        declaration,
                        context_flags,
                    );
                }
                None
            }
            // By default, do nothing and return nil - only the above cases
            // have context implied by a parent (Go's comment).
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getContextualTypeForBindingElement(declaration *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30131).
    fn get_contextual_type_for_binding_element(
        &mut self,
        declaration: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let name = name_of_node(self.store(), declaration)?;
        if binder_shims::is_binding_pattern(self.store().node(name))
            || is_computed_non_literal_name(self.store(), name)
        {
            return None;
        }
        let parent = self.store().node(declaration).parent.get();
        let grandparent = self.store().node(parent).parent.get();
        let mut parent_type =
            self.get_contextual_type_for_variable_like_declaration(grandparent, context_flags);
        if parent_type.is_none()
            && self.store().node(grandparent).kind != Kind::BindingElement
            && initializer_of_node(self, grandparent).is_some()
        {
            let check_mode = if crate::utilities::has_dot_dot_dot_token(self.store(), declaration) {
                crate::checker::CheckMode::REST_BINDING_ELEMENT
            } else {
                crate::checker::CheckMode::CHECK_MODE_NORMAL
            };
            parent_type = Some(self.check_declaration_initializer(grandparent, check_mode, None));
        }
        let parent_type = parent_type?;
        let parent_name = name_of_node(self.store(), grandparent)?;
        if self.store().node(parent_name).kind == Kind::ArrayBindingPattern {
            let index = elements_of_node(self.store(), parent)
                .iter()
                .position(|&e| e == declaration)? as i32;
            return self.get_contextual_type_for_element_expression(
                Some(parent_type),
                index as usize,
                -1, /*length*/
                -1, /*firstSpreadIndex*/
                -1, /*lastSpreadIndex*/
            );
        }
        let name_type = self.get_literal_type_from_property_name(name);
        if crate::utilities::is_type_usable_as_property_name(self.type_(name_type)) {
            let property_name =
                crate::utilities::get_property_name_from_type(self.type_(name_type));
            return self.get_type_of_property_of_type_impl(parent_type, &property_name);
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForStaticPropertyDeclaration(declaration *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30152).
    fn get_contextual_type_for_static_property_declaration(
        &mut self,
        declaration: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let parent = self.store().node(declaration).parent.get();
        if self.store().node(parent).kind == Kind::ClassExpression {
            // Don't contextually type a static property by its own class, its
            // type might still be in-progress and that would cause spurious
            // circularities (Go's comment).
            if let Some(parent_type) = self.get_contextual_type(parent, context_flags) {
                if self.type_(parent_type).symbol() != self.get_symbol_of_declaration(parent) {
                    let name = name_of_node(self.store(), declaration)?;
                    let name_type = self.get_literal_type_from_property_name(name);
                    let property_name =
                        crate::utilities::get_property_name_from_type(self.type_(name_type));
                    return self.get_type_of_property_of_contextual_type_impl(
                        parent_type,
                        &property_name,
                    );
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) getContextuallyTypedParameterType(parameter *ast.Node) *Type`
    /// (checker.go:30006).
    pub(crate) fn get_contextually_typed_parameter_type(
        &mut self,
        parameter: NodeId,
    ) -> Option<TypeId> {
        let fn_node = self.store().node(parameter).parent.get();
        if !self.is_context_sensitive_function_or_object_literal_method(fn_node) {
            return None;
        }
        // Go: `iife := ast.GetImmediatelyInvokedFunctionExpression(fn)`
        // (checker.go:30025) — REAL (W3T6; was the S9 TODO that fell through
        // to the contextual-signature arm, so an IIFE's binding-pattern
        // parameter resolved from the pattern-implied type instead of the
        // IIFE argument shape and fired spurious TS2537s on
        // computed-name elements).
        if let Some(iife) = binder_shims::get_immediately_invoked_function_expression(
            self.store(),
            fn_node,
        ) {
            let args = self.get_effective_call_arguments(iife);
            let index_of_parameter =
                parameters_of_node(self, fn_node).iter().position(|&p| p == parameter)?;
            if crate::utilities::has_dot_dot_dot_token(self.store(), parameter) {
                return Some(self.get_spread_argument_type(
                    &args,
                    index_of_parameter,
                    args.len(),
                    self.any_type(),
                    None, /*context*/
                    crate::checker::CheckMode::CHECK_MODE_NORMAL,
                ));
            }
            let cached = self.signature_node_links.get(iife).resolved_signature;
            self.signature_node_links.get(iife).resolved_signature = self.any_signature;
            let t = if index_of_parameter < args.len() {
                let arg_type = self.check_expression(args[index_of_parameter]);
                Some(self.get_widened_literal_type(arg_type))
            } else if initializer_of_node(self, parameter).is_some() {
                // Go: `case parameter.Initializer() != nil: t = nil` — the
                // widening default (anyType, plus reportImplicitAny under
                // noImplicitAny) answers.
                None
            } else {
                Some(self.undefined_widening_type())
            };
            self.signature_node_links.get(iife).resolved_signature = cached;
            return t;
        }
        let contextual_signature = self.get_contextual_signature(fn_node)?;
        let parameters = parameters_of_node(self, fn_node);
        let mut index = parameters.iter().position(|&p| p == parameter)? as i32;
        if let Some(this_parameter) = get_this_parameter(self.store(), fn_node) {
            if this_parameter == parameter {
                return None;
            }
            if parameters.first() == Some(&this_parameter) {
                index -= 1;
            }
        }
        if crate::utilities::has_dot_dot_dot_token(self.store(), parameter)
            && parameters.last() == Some(&parameter)
        {
            return self.get_rest_type_at_position(
                contextual_signature,
                index as usize,
                false, /*readonly*/
            );
        }
        self.try_get_type_at_position(contextual_signature, index as usize)
    }

    /// Go: `func isSpreadArgument(arg *ast.Node) bool` (checker.go:30063).
    ///
    /// PORT: Go's second disjunct (`IsSyntheticExpression(arg) && arg.IsSpread`)
    /// rides the synthetic-expression factory (checker.go's
    /// createSyntheticExpression, the M6-factory TODO); the port's node store
    /// has no SyntheticExpression nodes yet, so only the SpreadElement arm
    /// is live (getEffectiveCallArguments never fabricates spread synthetics
    /// until that wave lands).
    fn is_spread_argument(&self, arg: NodeId) -> bool {
        tsc_ast::is_spread_element(self.store().node(arg))
    }

    /// Go: `func (c *Checker) getSpreadArgumentType(args, index, argCount, restType, context, checkMode) *Type`
    /// (checker.go:30065) — REAL (W3T6; the IIFE rest-parameter arm of
    /// getContextuallyTypedParameterType needs it).
    pub(crate) fn get_spread_argument_type(
        &mut self,
        args: &[NodeId],
        index: usize,
        arg_count: usize,
        rest_type: TypeId,
        context: Option<crate::todo_shapes::InferenceContextId>,
        check_mode: crate::checker::CheckMode,
    ) -> TypeId {
        let in_const_context = crate::inference::is_const_type_variable(self, rest_type, 0);
        if arg_count > 0 && index >= arg_count - 1 {
            let arg = args[arg_count - 1];
            if self.is_spread_argument(arg) {
                // We are inferring from a spread expression in the last
                // argument position, i.e. both the parameter and the
                // argument are ...x forms. (Go's comment.)
                let spread_type = self.check_expression_with_contextual_type(
                    expression_of_node(self, arg).expect("spread element expression"),
                    Some(rest_type),
                    context,
                    check_mode,
                );
                if crate::relater::is_array_like_type(self, spread_type) {
                    return self.get_mutable_array_or_tuple_type(spread_type);
                }
                let arg = if tsc_ast::is_spread_element(self.store().node(arg)) {
                    expression_of_node(self, arg).expect("spread element expression")
                } else {
                    arg
                };
                let element_type = self.check_iterated_type_or_element_type(
                    crate::statements::IterationUse::SPREAD,
                    spread_type,
                    self.undefined_type(),
                    Some(arg),
                );
                return self.create_array_type_ex(element_type, in_const_context);
            }
        }
        let mut types: Vec<TypeId> = Vec::new();
        let mut infos: Vec<crate::types::TupleElementInfo> = Vec::new();
        for (i, &arg) in args.iter().enumerate().take(arg_count).skip(index) {
            let t;
            let mut info = crate::types::TupleElementInfo {
                flags: crate::types::ElementFlags::NONE,
                labeled_declaration: None,
            };
            if self.is_spread_argument(arg) {
                let spread_type = self.check_expression(
                    expression_of_node(self, arg).expect("spread element expression"),
                );
                if crate::relater::is_array_like_type(self, spread_type) {
                    t = spread_type;
                    info.flags = crate::types::ElementFlags::VARIADIC;
                } else {
                    if tsc_ast::is_spread_element(self.store().node(arg)) {
                        t = self.check_iterated_type_or_element_type(
                            crate::statements::IterationUse::SPREAD,
                            spread_type,
                            self.undefined_type(),
                            expression_of_node(self, arg),
                        );
                    } else {
                        t = self.check_iterated_type_or_element_type(
                            crate::statements::IterationUse::SPREAD,
                            spread_type,
                            self.undefined_type(),
                            Some(arg),
                        );
                    }
                    info.flags = crate::types::ElementFlags::REST;
                }
            } else {
                // PORT: Go's tuple-name-source synthetic arm
                // (`IsSyntheticExpression(arg) && TupleNameSource != nil`)
                // rides the synthetic-expression factory (the M6-factory
                // TODO); unreachable until that wave lands.
                let contextual_type = if crate::relater::is_tuple_type(self, rest_type) {
                    self.get_contextual_type_for_element_expression(
                        Some(rest_type),
                        i - index,
                        arg_count as i32 - index as i32,
                        -1,
                        -1,
                    )
                    .unwrap_or_else(|| self.unknown_type())
                } else {
                    let number_literal = self.get_number_literal_type(Number::from(i as f64));
                    self.get_indexed_access_type_ex(
                        rest_type,
                        number_literal,
                        crate::types::AccessFlags::CONTEXTUAL,
                        None,
                        None,
                    )
                };
                let arg_type = self.check_expression_with_contextual_type(
                    arg,
                    Some(contextual_type),
                    context,
                    check_mode,
                );
                let has_primitive_contextual_type = in_const_context
                    || crate::relater::maybe_type_of_kind(
                        self,
                        contextual_type,
                        TypeFlags::PRIMITIVE
                            | TypeFlags::INDEX
                            | TypeFlags::TEMPLATE_LITERAL
                            | TypeFlags::STRING_MAPPING,
                    );
                t = if has_primitive_contextual_type {
                    self.get_regular_type_of_literal_type(arg_type)
                } else {
                    self.get_widened_literal_type(arg_type)
                };
                info.flags = crate::types::ElementFlags::REQUIRED;
            }
            types.push(t);
            infos.push(info);
        }
        // Go: `inConstContext && !someType(restType, c.isMutableArrayLikeType)`
        // — the someType fold inlined over the union constituents (the
        // closure would need `self` twice).
        let mut any_mutable = false;
        if self.type_(rest_type).flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(rest_type).types().to_vec();
            for constituent in constituents {
                if crate::inference::is_mutable_array_like_type(self, constituent) {
                    any_mutable = true;
                    break;
                }
            }
        } else {
            any_mutable = crate::inference::is_mutable_array_like_type(self, rest_type);
        }
        let readonly = in_const_context && !any_mutable;
        self.create_tuple_type_ex(&types, &infos, readonly)
    }

    /// Go: `func (c *Checker) getMutableArrayOrTupleType(t *Type) *Type`
    /// (checker.go:30134).
    pub(crate) fn get_mutable_array_or_tuple_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            return crate::typenodes::map_type(self, t, |c, t| {
                c.get_mutable_array_or_tuple_type(t)
            });
        }
        if self.type_(t).flags.intersects(TypeFlags::ANY) {
            return t;
        }
        let base_constraint = crate::relater::get_base_constraint_or_type(self, t);
        if crate::relater::is_mutable_array_or_tuple(self, base_constraint) {
            return t;
        }
        if crate::relater::is_tuple_type(self, t) {
            let element_types = self.get_element_types(t);
            let element_infos = self
                .type_(self.type_(t).target_tuple_type())
                .as_tuple_type()
                .expect("tuple target data")
                .element_infos
                .clone();
            return self.create_tuple_type_ex(&element_types, &element_infos, false /*readonly*/);
        }
        self.create_tuple_type_ex(
            &[t],
            &[crate::types::TupleElementInfo {
                flags: crate::types::ElementFlags::VARIADIC,
                labeled_declaration: None,
            }],
            false, /*readonly*/
        )
    }

    // ────────────────────────────────────────────────────────────────────────
    // Return / yield / await operands (Go checker.go:30170-30309)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getContextualTypeForReturnExpression(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30170).
    pub(crate) fn get_contextual_type_for_return_expression(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let fn_node = crate::utilities::get_containing_function(self.store(), node)?;
        let contextual_return_type = self.get_contextual_return_type(fn_node, context_flags)?;
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        let is_async = function_flags.intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
        let is_generator =
            function_flags.intersects(tsc_ast::functionflags::FunctionFlags::GENERATOR);
        let mut contextual_return_type = contextual_return_type;
        if is_generator {
            let is_async_generator = is_async;
            if self.type_(contextual_return_type).flags.intersects(TypeFlags::UNION) {
                let members = self.type_(contextual_return_type).types().to_vec();
                let mut filtered = Vec::new();
                for t in members {
                    if self
                        .get_iteration_type_of_generator_function_return_type(
                            crate::statements::IterationTypeKind::Return,
                            t,
                            is_async_generator,
                        )
                        .is_some()
                    {
                        filtered.push(t);
                    }
                }
                contextual_return_type = self.get_union_type_ex(
                    filtered,
                    crate::todo_shapes::UnionReduction::None,
                    None,
                    None,
                );
            }
            let iteration_return_type = self.get_iteration_type_of_generator_function_return_type(
                crate::statements::IterationTypeKind::Return,
                contextual_return_type,
                is_async,
            );
            // falls through to unwrap Promise for AsyncGenerators (Go's comment)
            contextual_return_type = iteration_return_type?;
        }
        if is_async {
            // Get the awaited type without the `Awaited<T>` alias (Go's comment).
            let contextual_awaited_type = self.get_awaited_type_no_alias(contextual_return_type)?;
            // PORT: hoisted — the union constructor takes the pair by value.
            let promise_like = self.create_promise_like_type(contextual_awaited_type);
            return Some(self.get_union_type_ex(
                vec![contextual_awaited_type, promise_like],
                crate::todo_shapes::UnionReduction::None,
                None,
                None,
            ));
        }
        // Regular function or Generator function
        Some(contextual_return_type)
    }

    /// Go: `func (c *Checker) getContextualReturnType(functionDecl *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30214).
    pub(crate) fn get_contextual_return_type(
        &mut self,
        function_decl: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        // If the containing function has a return type annotation, is a
        // constructor, or is a get accessor whose corresponding set accessor
        // has a type annotation, return statements in the function are
        // contextually typed (Go's comment).
        if let Some(return_type) = self.get_return_type_from_annotation(function_decl) {
            return Some(return_type);
        }
        // Otherwise, if the containing function is contextually typed by a
        // function type with exactly one call signature and that call
        // signature is non-generic, return statements are contextually typed
        // by the return type of the signature.
        let signature = self.get_contextual_signature_for_function_like_declaration(function_decl);
        if let Some(signature) = signature {
            if !self.is_resolving_return_type_of_signature(signature) {
                let return_type =
                    crate::relater::get_return_type_of_signature(self, signature);
                let function_flags =
                    tsc_ast::functionflags::get_function_flags(self.store(), function_decl);
                let is_generator = function_flags
                    .intersects(tsc_ast::functionflags::FunctionFlags::GENERATOR);
                let is_async =
                    function_flags.intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
                if is_generator || is_async {
                    let mut filtered = Vec::new();
                    for t in self.type_(return_type).types().to_vec() {
                        let flags = self.type_(t).flags;
                        if flags.intersects(
                            TypeFlags::ANY_OR_UNKNOWN
                                | TypeFlags::VOID
                                | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                        ) || (is_generator
                            && self.check_generator_instantiation_assignability_to_return_type(
                                t,
                                function_flags,
                                None, /*errorNode*/
                            ))
                            || (is_async && self.get_awaited_type_of_promise(t).is_some())
                        {
                            filtered.push(t);
                        }
                    }
                    return Some(self.get_union_type_ex(
                        filtered,
                        crate::todo_shapes::UnionReduction::None,
                        None,
                        None,
                    ));
                }
                return Some(return_type);
            }
            return None;
        }
        let iife =
            binder_shims::get_immediately_invoked_function_expression(self.store(), function_decl);
        if let Some(iife) = iife {
            return self.get_contextual_type(iife, context_flags);
        }
        None
    }

    /// Go: `func (c *Checker) getContextualSignatureForFunctionLikeDeclaration(node *ast.Node) *Signature`
    /// (checker.go:30260).
    pub(crate) fn get_contextual_signature_for_function_like_declaration(
        &mut self,
        node: NodeId,
    ) -> Option<crate::types::SignatureId> {
        // Only function expressions, arrow functions, and object literal
        // methods are contextually typed (Go's comment).
        if binder_shims::is_function_expression_or_arrow_function(self.store().node(node))
            || binder_shims::is_object_literal_method(self.store(), node)
        {
            return self.get_contextual_signature(node);
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // The contextual-signature family (Go checker.go:10443-10590)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getContextualSignature(node *ast.Node) *Signature`
    /// (checker.go:10472) — REAL (M6 S9). A contextual type provides a
    /// contextual signature if it has a single call signature and if that
    /// call signature is non-generic. If the contextual type is a union type,
    /// get the signature from each type possible and if they are all
    /// identical ignoring their return type, the result is same signature
    /// but with return type as union type of return types from these
    /// signatures (Go's comment).
    pub(crate) fn get_contextual_signature(
        &mut self,
        node: NodeId,
    ) -> Option<crate::types::SignatureId> {
        let t = self.get_apparent_type_of_contextual_type(node, ContextFlags::SIGNATURE)?;
        if !self.type_(t).flags.intersects(TypeFlags::UNION) {
            return self.get_contextual_call_signature(t, node);
        }
        let mut signature_list: Vec<crate::types::SignatureId> = Vec::new();
        for current in self.type_(t).types().to_vec() {
            if let Some(signature) = self.get_contextual_call_signature(current, node) {
                if !signature_list.is_empty()
                    && self.compare_signatures_identical(
                        signature_list[0],
                        signature,
                        false,  /*partialMatch*/
                        true,   /*ignoreThisTypes*/
                        true,   /*ignoreReturnTypes*/
                        crate::relater::TypeComparerKind::Identical,
                        None, /*relater — the checker-level method comparer*/
                    ) == crate::types::Ternary::False
                {
                    // Signatures aren't identical, do not use (Go's comment)
                    return None;
                }
                // Use this signature for contextual union signature
                signature_list.push(signature);
            }
        }
        match signature_list.len() {
            0 => None,
            1 => Some(signature_list[0]),
            // Result is union of signatures collected (return type is union
            // of return types of this signature set) (Go's comment)
            _ => Some(self.create_union_signature(signature_list[0], signature_list)),
        }
    }

    /// Go: `func (c *Checker) getContextualCallSignature(t *Type, node *ast.Node) *Signature`
    /// (checker.go:10537) — if the given type is an object or union type with
    /// a single signature, and if that signature has at least as many
    /// parameters as the given function, return the signature.
    pub(crate) fn get_contextual_call_signature(
        &mut self,
        t: TypeId,
        node: NodeId,
    ) -> Option<crate::types::SignatureId> {
        let signatures = self.get_signatures_of_type_impl(
            t,
            crate::types::SignatureKind::Call,
        );
        let mut applicable_by_arity = Vec::new();
        for s in signatures {
            if !self.is_arity_smaller(s, node) {
                applicable_by_arity.push(s);
            }
        }
        if applicable_by_arity.len() == 1 {
            return Some(applicable_by_arity[0]);
        }
        self.get_intersected_signatures_impl(&applicable_by_arity)
    }

    /// Go: `func (c *Checker) getIntersectedSignatures(signatures []*Signature) *Signature`
    /// (checker.go:10548).
    fn get_intersected_signatures_impl(
        &mut self,
        signatures: &[crate::types::SignatureId],
    ) -> Option<crate::types::SignatureId> {
        if !self.no_implicit_any {
            return None;
        }
        let mut combined: Option<crate::types::SignatureId> = None;
        for &sig in signatures {
            match combined {
                None => combined = Some(sig),
                Some(c) if c == sig => {}
                Some(c) => {
                    if self.compare_type_parameters_identical_of(c, sig) {
                        combined = Some(
                            self.combine_union_or_intersection_member_signatures(
                                c,
                                sig,
                                false, /*isUnion*/
                            ),
                        );
                    } else {
                        return None;
                    }
                }
            }
        }
        combined
    }

    /// If the contextual signature has fewer parameters than the function
    /// expression, do not use it (Go's comment).
    /// Go: `func (c *Checker) isAritySmaller(signature *Signature, target *ast.Node) bool`
    /// (checker.go:10567).
    fn is_arity_smaller(
        &mut self,
        signature: crate::types::SignatureId,
        target: NodeId,
    ) -> bool {
        let parameters = parameters_of_node(self, target);
        let mut target_parameter_count = 0usize;
        while target_parameter_count < parameters.len() {
            let param = parameters[target_parameter_count];
            let has_initializer = initializer_of_node(self, param).is_some();
            let has_question = crate::ast_shims::has_question_token(self.store(), param);
            let has_dotdotdot = crate::utilities::has_dot_dot_dot_token(self.store(), param);
            if has_initializer || has_question || has_dotdotdot {
                break;
            }
            target_parameter_count += 1;
        }
        if !parameters.is_empty() && crate::ast_shims::is_this_parameter(self.store(), parameters[0])
        {
            target_parameter_count -= 1;
        }
        !self.has_effective_rest_parameter(signature)
            && self.get_parameter_count(signature) < target_parameter_count as i32
    }

    /// Go: `func (c *Checker) compareTypeParametersIdentical(source, target []*Type) bool`
    /// — delegate (relater.rs's real body).
    fn compare_type_parameters_identical_of(
        &mut self,
        source: crate::types::SignatureId,
        target: crate::types::SignatureId,
    ) -> bool {
        let source_params = self.signatures[source.0 as usize].type_parameters.clone();
        let target_params = self.signatures[target.0 as usize].type_parameters.clone();
        self.compare_type_parameters_identical(&source_params, &target_params)
    }

    // ────────────────────────────────────────────────────────────────────────
    // getApparentTypeOfContextualType + instantiateContextualType
    // (Go checker.go:31235 + 31366)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getApparentTypeOfContextualType(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:31235).
    pub(crate) fn get_apparent_type_of_contextual_type(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let contextual_type = if binder_shims::is_object_literal_method(self.store(), node) {
            let element_parent = self.store().node(node).parent.get();
            self.get_contextual_type_for_object_literal_element(element_parent, context_flags)
        } else {
            self.get_contextual_type(node, context_flags)
        };
        let instantiated_type =
            self.instantiate_contextual_type(contextual_type, node, context_flags)?;
        if context_flags.intersects(ContextFlags::NO_CONSTRAINTS)
            && self.type_(instantiated_type).flags.intersects(TypeFlags::TYPE_VARIABLE)
        {
            return None;
        }
        // Go maps each constituent through getApparentType skipping mapped
        // types, with noReductions.
        let apparent_type = crate::typenodes::map_type(
            self,
            instantiated_type,
            |c, t| {
                if c.type_(t).as_mapped_type().is_some() {
                    t
                } else {
                    crate::relater::get_apparent_type(c, t)
                }
            },
        );
        // REAL (M6 S10, checker.go:31252): the union discriminate arms —
        // discriminateContextualTypeByObjectMembers for object literals;
        // discriminateContextualTypeByJSXAttributes for JsxAttributes
        // (REAL M6 W5 — jsx.go's arm).
        if self.type_(apparent_type).flags.intersects(TypeFlags::UNION) {
            let node_kind = self.store().node(node).kind;
            if node_kind == Kind::ObjectLiteralExpression {
                return self.discriminate_contextual_type_by_object_members(node, apparent_type);
            }
            if node_kind == Kind::JsxAttributes {
                return self.discriminate_contextual_type_by_jsx_attributes(node, apparent_type);
            }
        }
        Some(apparent_type)
    }

    /// Go: `func (c *Checker) discriminateContextualTypeByObjectMembers(node, contextualType) *Type`
    /// (checker.go:31298) — REAL (M6 S10): the cached
    /// getMatchingUnionConstituentForObjectLiteral probe, else the
    /// ObjectLiteralDiscriminator over the node's discriminant properties and
    /// the contextual type's optional members.
    pub(crate) fn discriminate_contextual_type_by_object_members(
        &mut self,
        node: NodeId,
        contextual_type: TypeId,
    ) -> Option<TypeId> {
        let key = (node, contextual_type);
        if let Some(discriminated) = self.discriminated_contextual_types.get(&key) {
            return *discriminated;
        }
        let mut discriminated =
            self.get_matching_union_constituent_for_object_literal(contextual_type, node);
        if discriminated.is_none() {
            // Go: discriminantProperties — the property-assignment/shorthand
            // filter over node.Properties().
            let properties: Vec<NodeId> = {
                let d = self
                    .store()
                    .node(node)
                    .as_object_literal_expression()
                    .expect("ObjectLiteralExpression data");
                d.properties
                    .as_ref()
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default()
            };
            let mut discriminant_properties: Vec<NodeId> = Vec::new();
            for p in properties {
                let Some(symbol) = self.get_symbol_of_node_real(p) else {
                    continue;
                };
                let name = self.program_symbol(symbol).name.clone();
                let kind = self.store().node(p).kind;
                if kind == Kind::PropertyAssignment {
                    let initializer = self
                        .store()
                        .node(p)
                        .as_property_assignment()
                        .expect("PropertyAssignment data")
                        .initializer;
                    if self.is_possibly_discriminant_value(initializer)
                        && crate::relater::is_discriminant_property(self, contextual_type, &name)
                    {
                        discriminant_properties.push(p);
                    }
                } else if kind == Kind::ShorthandPropertyAssignment
                    && crate::relater::is_discriminant_property(self, contextual_type, &name)
                {
                    discriminant_properties.push(p);
                }
            }
            // Go: discriminantMembers — the contextual type's optional
            // members absent from the node's symbol's member table.
            let node_symbol_members: Vec<(String, tsc_ast::SymbolId)> = self
                .get_symbol_of_node_real(node)
                .map(|symbol| {
                    self.program_symbol(symbol)
                        .members
                        .entries()
                        .map(|(k, &v)| (k.clone(), v))
                        .collect()
                })
                .unwrap_or_default();
            let discriminant_members: Vec<tsc_ast::SymbolId> =
                crate::relater::get_properties_of_type(self, contextual_type)
                    .into_iter()
                    .filter(|&s| {
                        let name = self.program_symbol(s).name.clone();
                        self.program_symbol(s).flags.intersects(SymbolFlags::OPTIONAL)
                            && !node_symbol_members.iter().any(|(k, _)| *k == name)
                            && crate::relater::is_discriminant_property(self, contextual_type, &name)
                    })
                    .collect();
            // Go's ObjectLiteralDiscriminator: len = props + members; name and
            // matches below (the closure triple rides relater.rs's
            // discriminateTypeByDiscriminableItems).
            let props = discriminant_properties;
            let members = discriminant_members;
            let (props_snapshot, members_snapshot) = (props.clone(), members.clone());
            let discriminated_type = crate::relater::discriminate_type_by_discriminable_items_with_closures(
                self,
                contextual_type,
                props_snapshot.len() + members_snapshot.len(),
                |c, index| {
                    if index < props_snapshot.len() {
                        let p = props_snapshot[index];
                        let symbol = c.get_symbol_of_node_real(p).expect("property symbol");
                        c.program_symbol(symbol).name.clone()
                    } else {
                        let s = members_snapshot[index - props_snapshot.len()];
                        c.program_symbol(s).name.clone()
                    }
                },
                |c, index, t| {
                    let prop_type = if index < props_snapshot.len() {
                        let p = props_snapshot[index];
                        let kind = c.store().node(p).kind;
                        if kind == Kind::PropertyAssignment || kind == Kind::JsxAttribute {
                            let initializer = if kind == Kind::PropertyAssignment {
                                Some(
                                    c.store()
                                        .node(p)
                                        .as_property_assignment()
                                        .expect("PropertyAssignment data")
                                        .initializer,
                                )
                            } else {
                                // Go: a JsxAttribute without initializer is
                                // always true.
                                None
                            };
                            match initializer {
                                Some(initializer) => c.get_context_free_type_of_expression(initializer),
                                None => c.true_type(),
                            }
                        } else {
                            let name = binder_shims::get_name_of_declaration(c.store(), p)
                                .expect("property name node");
                            c.get_context_free_type_of_expression(name)
                        }
                    } else {
                        c.undefined_type()
                    };
                    // Go: `for _, s := range propType.Distributed()`.
                    for s in c.type_(prop_type).distributed() {
                        if c.is_type_assignable_to(s, t) {
                            return true;
                        }
                    }
                    false
                },
            );
            discriminated = Some(discriminated_type);
        }
        self.discriminated_contextual_types.insert(key, discriminated);
        discriminated
    }

    /// Go: `func (c *Checker) getMatchingUnionConstituentForObjectLiteral(unionType, node) *Type`
    /// (checker.go:31333) — REAL (M6 S10).
    pub(crate) fn get_matching_union_constituent_for_object_literal(
        &mut self,
        union_type: TypeId,
        node: NodeId,
    ) -> Option<TypeId> {
        let key_property_name =
            crate::relater::get_key_property_name_public(self, union_type)?;
        let properties: Vec<NodeId> = {
            let d = self
                .store()
                .node(node)
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression data");
            d.properties
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default()
        };
        for p in properties {
            let Some(symbol) = self.get_symbol_of_node_real(p) else {
                continue;
            };
            if self.store().node(p).kind == Kind::PropertyAssignment
                && self.program_symbol(symbol).name == key_property_name
            {
                let initializer = self
                    .store()
                    .node(p)
                    .as_property_assignment()
                    .expect("PropertyAssignment data")
                    .initializer;
                if self.is_possibly_discriminant_value(initializer) {
                    let prop_type = self.get_context_free_type_of_expression(initializer);
                    return crate::relater::get_constituent_type_for_key_type_public(self, union_type, prop_type);
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) isPossiblyDiscriminantValue(node) bool` (checker.go:31346)
    /// — REAL (M6 S10). (Go comment, verbatim:) Return true if the given
    /// expression is possibly a discriminant value. We limit the kinds of
    /// expressions we check to those that don't depend on their contextual
    /// type in order not to cause recursive (and possibly infinite)
    /// invocations of getContextualType.
    pub(crate) fn is_possibly_discriminant_value(&mut self, node: NodeId) -> bool {
        match self.store().node(node).kind {
            Kind::StringLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::TemplateExpression
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::NullKeyword
            | Kind::Identifier
            | Kind::UndefinedKeyword => true,
            Kind::PropertyAccessExpression | Kind::ParenthesizedExpression => {
                let expression = {
                    let data = &self.store().node(node).data;
                    match data {
                        NodeData::PropertyAccessExpression(d) => Some(d.expression),
                        NodeData::ParenthesizedExpression(d) => Some(d.expression),
                        _ => None,
                    }
                };
                expression.is_some_and(|e| self.is_possibly_discriminant_value(e))
            }
            Kind::JsxExpression => {
                let expression = self
                    .store()
                    .node(node)
                    .as_jsx_expression()
                    .map(|d| d.expression);
                match expression {
                    None => true,
                    Some(Some(e)) => self.is_possibly_discriminant_value(e),
                    Some(None) => true,
                }
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) instantiateContextualType(contextualType *Type, node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:31366).
    pub(crate) fn instantiate_contextual_type(
        &mut self,
        contextual_type: Option<TypeId>,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let contextual_type = contextual_type?;
        if !crate::relater::maybe_type_of_kind(self, contextual_type, TypeFlags::INSTANTIABLE) {
            return Some(contextual_type);
        }
        // If no inferences have been made, and none of the type parameters
        // for which we are inferring specify default types, nothing is
        // gained from instantiating as type parameters would just be
        // replaced with their constraints similar to the apparent type
        // (Go's comment).
        let inference_context = self.get_inference_context(node);
        if let Some(context_id) = inference_context {
            // PORT: Go keeps the *InferenceContext pointer live; the port
            // snapshots the two mapper slots (the inferences scan is
            // immutable) so the instantiate calls can take &mut Checker.
            let non_fixing_mapper = self.inference_contexts[context_id.0 as usize].non_fixing_mapper;
            let return_mapper = self.inference_contexts[context_id.0 as usize].return_mapper;
            let has_candidates_or_default = self.inference_contexts[context_id.0 as usize]
                .inferences
                .iter()
                .any(crate::inference::has_inference_candidates_or_default);
            if context_flags.intersects(ContextFlags::SIGNATURE) && has_candidates_or_default
            {
                // For contextual signatures we incorporate all inferences
                // made so far, e.g. from return types as well as arguments
                // to the left in a function call (Go's comment).
                if let Some(non_fixing_mapper) = non_fixing_mapper {
                    let t =
                        self.instantiate_instantiable_types(contextual_type, non_fixing_mapper);
                    if !self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                        return Some(t);
                    }
                }
            }
            if let Some(return_mapper) = return_mapper {
                // For other purposes (e.g. determining whether to produce
                // literal types) we only incorporate inferences made from the
                // return type in a function call. We remove the 'boolean'
                // type from the contextual type such that contextually typed
                // boolean literals actually end up widening to 'boolean'
                // (see #48363).
                let t = self.instantiate_instantiable_types(contextual_type, return_mapper);
                if !self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                    if self.type_(t).flags.intersects(TypeFlags::UNION) {
                        let rf = self.regular_false_type();
                        let rt = self.regular_true_type();
                        let members = self.type_(t).types().to_vec();
                        if members.contains(&rf) && members.contains(&rt) {
                            let filtered: Vec<_> = members
                                .iter()
                                .copied()
                                .filter(|&m| m != rf && m != rt)
                                .collect();
                            return Some(self.get_union_type_ex(
                                filtered,
                                crate::todo_shapes::UnionReduction::None,
                                None,
                                None,
                            ));
                        }
                    }
                    return Some(t);
                }
            }
        }
        Some(contextual_type)
    }

    /// Go: `func (c *Checker) instantiateInstantiableTypes(t *Type, mapper *TypeMapper) *Type`
    /// (checker.go:31392).
    pub(crate) fn instantiate_instantiable_types(
        &mut self,
        t: TypeId,
        mapper: crate::todo_shapes::TypeMapperId,
    ) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            return self.instantiate_type(t, mapper);
        }
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            let mapped: Vec<_> = self
                .type_(t)
                .types()
                .to_vec()
                .into_iter()
                .map(|m| self.instantiate_instantiable_types(m, mapper))
                .collect();
            return self.get_union_type_ex(
                mapped,
                crate::todo_shapes::UnionReduction::None,
                None,
                None,
            );
        }
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let mapped: Vec<_> = self
                .type_(t)
                .types()
                .to_vec()
                .into_iter()
                .map(|m| self.instantiate_instantiable_types(m, mapper))
                .collect();
            return self.get_intersection_type(&mapped);
        }
        t
    }

    // ────────────────────────────────────────────────────────────────────────
    // Call-argument / binary / object-literal / element contextual arms
    // (Go checker.go:30311-30591)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getContextualTypeForArgument(callTarget *ast.Node, arg *ast.Node) *Type`
    /// (checker.go:30311).
    fn get_contextual_type_for_argument(
        &mut self,
        call_target: NodeId,
        arg: NodeId,
    ) -> Option<TypeId> {
        let args = self.get_effective_call_arguments(call_target);
        let arg_index = args.iter().position(|&a| a == arg)?;
        self.get_contextual_type_for_argument_at_index_impl(call_target, arg_index)
    }

    /// Go: `func (c *Checker) getContextualTypeForArgumentAtIndex(callTarget *ast.Node, argIndex int) *Type`
    /// (checker.go:30321).
    pub(crate) fn get_contextual_type_for_argument_at_index_impl(
        &mut self,
        call_target: NodeId,
        arg_index: usize,
    ) -> Option<TypeId> {
        // TODO(port M6 S9): Go's dynamic-import arm (stringType /
        // ImportCallOptions / anyType) rides the global resolvers wave.
        // If we're already in the process of resolving the given signature,
        // don't resolve again as that could cause infinite recursion.
        // Instead, return anySignature (Go's comment, checker.go:30330) —
        // the resolvingSignature marker read, NOT a second
        // getResolvedSignature (re-resolving mid-resolution recurses; the
        // marker signature's empty parameter list makes getTypeAtPosition
        // answer anyType — W3T5 fix, Go-exact).
        let signature = if self.signature_node_links.get(call_target).resolved_signature
            == self.resolving_signature
            && self.resolving_signature.is_some()
        {
            self.resolving_signature
        } else {
            self.get_resolved_signature(call_target)
        }?;
        // M6 W5 (Go checker.go:30341): the JsxOpeningLikeElement
        // first-argument arm — getEffectiveFirstArgumentForJsxSignature.
        if tsc_ast::utilities::is_jsx_opening_like_element(self.store().node(call_target))
            && arg_index == 0
        {
            return Some(
                self.get_effective_first_argument_for_jsx_signature(signature, call_target),
            );
        }
        let parameters = self.signatures[signature.0 as usize].parameters.clone();
        let rest_index = parameters.len().saturating_sub(1);
        if arg_index >= rest_index && self.has_effective_rest_parameter(signature) {
            let rest_type = crate::relater::get_type_of_symbol(self, parameters[rest_index]);
            let number_type = self.get_number_literal_type(Number((arg_index - rest_index) as f64));
            return Some(self.get_indexed_access_type_ex(
                rest_type,
                number_type,
                crate::types::AccessFlags::CONTEXTUAL,
                None, /*errorNode*/
                None, /*accessNode*/
            ));
        }
        Some(self.get_type_at_position(signature, arg_index))
    }

    /// Go: `func (c *Checker) getContextualTypeForBinaryOperand(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30358).
    fn get_contextual_type_for_binary_operand(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let parent = self.store().node(node).parent.get();
        let op = operator_kind(self.store(), parent);
        let (left, right) = left_right_of(self.store(), parent)?;
        if matches!(
            op,
            Kind::EqualsToken
                | Kind::AmpersandAmpersandEqualsToken
                | Kind::BarBarEqualsToken
                | Kind::QuestionQuestionEqualsToken
        ) {
            // In an assignment expression, the right operand is
            // contextually typed by the type of the left operand unless
            // it's an assignment declaration (Go's comment).
            if node == right {
                let leftmost = tsc_ast::precedence::get_leftmost_expression(
                    self.store(),
                    left,
                    false, /*stopAtCallExpressions*/
                );
                // TODO(port M6 W2/W4): the module-exports guard and JS
                // assignment-declaration special cases of
                // getContextualTypeForAssignmentExpression (checker.go:
                // 30392) ride the JS-support waves; the left-operand type
                // below is Go's tail for non-JS programs.
                let is_module_exports = self.store().node(leftmost).kind == Kind::Identifier
                    && self
                        .get_resolved_symbol(leftmost)
                        .is_some_and(|s| {
                            self.program_symbol(s).flags.intersects(SymbolFlags::MODULE_EXPORTS)
                        });
                if !is_module_exports {
                    return self.get_contextual_type_for_assignment_expression(parent);
                }
            }
        } else if matches!(op, Kind::BarBarToken | Kind::QuestionQuestionToken) {
            // When an || expression has a contextual type, the operands are
            // contextually typed by that type, except when that type
            // originates in a binding pattern, the right operand is
            // contextually typed by the type of the left operand (Go's
            // comment).
            let t = self.get_contextual_type(parent, context_flags);
            if node == right
                && t.is_none_or(|t| self.pattern_for_type.contains_key(&t))
            {
                return Some(self.get_type_of_expression(left));
            }
            return t;
        } else if matches!(op, Kind::AmpersandAmpersandToken | Kind::CommaToken)
            && node == right
        {
            return self.get_contextual_type(parent, context_flags);
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForAssignmentExpression(binary *ast.BinaryExpression) *Type`
    /// (checker.go:30392) — the non-JS tail: the type of the left operand.
    fn get_contextual_type_for_assignment_expression(
        &mut self,
        binary: NodeId,
    ) -> Option<TypeId> {
        let (left, _) = left_right_of(self.store(), binary)?;
        // TODO(port M6 W2/W4): the access-expression JS-declaration and
        // this-keyword special cases ride the JS-support waves.
        Some(self.get_type_of_expression(left))
    }

    /// Go: `func (c *Checker) getContextualTypeForObjectLiteralElement(element *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30469).
    pub(crate) fn get_contextual_type_for_object_literal_element_impl(
        &mut self,
        element: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        if let Some(t) = type_of_node(self, element) {
            if !binder_shims::is_object_literal_method(self.store(), element) {
                return Some(self.get_type_from_type_node(t));
            }
        }
        let object_literal = self.store().node(element).parent.get();
        let t = self.get_apparent_type_of_contextual_type(object_literal, context_flags)?;
        if self.has_bindable_name(element) {
            // For a (non-symbol) computed property, there is no reason to
            // look up the name in the type. It will just be "__computed",
            // which does not appear in any SymbolTable (Go's comment).
            let symbol = self.get_symbol_of_declaration(element)?;
            let name = crate::statements::symbol_name(self, symbol);
            // PORT: Go threads the declaration symbol's nameType through
            // getTypeOfPropertyOfContextualTypeEx; the nameType-based
            // refinement rides the S10 contextual-property family.
            return self.get_type_of_property_of_contextual_type_impl(t, &name);
        }
        if binder_shims::has_dynamic_name(self.store(), element) {
            // Go: for a dynamic name, look the computed property's
            // expression type up in the contextual type.
            if let Some(name) = name_of_node(self.store(), element) {
                if self.store().node(name).kind == Kind::ComputedPropertyName {
                    if let Some(expr) = expression_of_node(self, name) {
                        let expr_type = self.check_expression(expr);
                        if crate::utilities::is_type_usable_as_property_name(self.type_(expr_type))
                        {
                            let property_name = crate::utilities::get_property_name_from_type(
                                self.type_(expr_type),
                            );
                            if let Some(prop_type) =
                                self.get_type_of_property_of_contextual_type_impl(t, &property_name)
                            {
                                return Some(prop_type);
                            }
                        }
                    }
                }
            }
        }
        if let Some(name) = name_of_node(self.store(), element) {
            let name_type = self.get_literal_type_from_property_name(name);
            // We avoid calling getApplicableIndexInfo here because it
            // performs potentially expensive intersection reduction (Go's
            // comment).
            return crate::typenodes::map_type(
                self,
                t,
                |c, t| {
                    let index_infos = c.get_index_infos_of_structured_type_impl(t);
                    c.find_applicable_index_info(&index_infos, name_type)
                        .map(|info| c.index_info_value_type(info))
                        .unwrap_or_else(|| c.unknown_type())
                },
            )
            .into();
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForElementExpression(t *Type, index, length, firstSpreadIndex, lastSpreadIndex int) *Type`
    /// (checker.go:30521).
    pub(crate) fn get_contextual_type_for_element_expression(
        &mut self,
        t: Option<TypeId>,
        index: usize,
        _length: i32,
        first_spread_index: i32,
        _last_spread_index: i32,
    ) -> Option<TypeId> {
        let t = t?;
        Some(crate::typenodes::map_type(self, t, |c, t| {
            if c.is_tuple_type(t) {
                // TODO(port M6 S9): the tuple arm's spread/ending-fixed
                // geometry (getElementTypeOfSliceOfTupleType with
                // fixedEndLength/endSkipCount arithmetic) rides the tuple
                // arithmetic wave; the fixed-constituent answer below is the
                // common no-spread case.
                if first_spread_index < 0 || (index as i32) < first_spread_index {
                    if let Some(element) = crate::relater::get_type_arguments(c, t).get(index) {
                        return crate::relater::remove_missing_type(c, *element, false);
                    }
                }
                return c.unknown_type();
            }
            // If element index is known and a contextual property with that
            // name exists, return it. Otherwise return the iterated or
            // element type of the contextual type (Go's comment).
            if first_spread_index < 0 || (index as i32) < first_spread_index {
                if let Some(prop_type) =
                    c.get_type_of_property_of_contextual_type_impl(t, &index.to_string())
                {
                    return prop_type;
                }
            }
            c.get_iterated_type_or_element_type(
                crate::statements::IterationUse::ELEMENT,
                t,
                c.undefined_type(),
                None,     /*errorNode*/
                false,     /*checkAssignability*/
            )
            .unwrap_or_else(|| c.unknown_type())
        }))
    }

    /// Go: `func (c *Checker) getContextualTypeForConditionalOperand(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30571) — in a contextually typed conditional expression,
    /// the true/false expressions are contextually typed by the same type.
    fn get_contextual_type_for_conditional_operand(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let parent = self.store().node(node).parent.get();
        let NodeData::ConditionalExpression(c) = &self.store().node(parent).data else {
            return None;
        };
        if node == c.when_true || node == c.when_false {
            return self.get_contextual_type(parent, context_flags);
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForSubstitutionExpression(template *ast.Node, substitutionExpression *ast.Node) *Type`
    /// (checker.go:30579).
    fn get_contextual_type_for_substitution_expression(
        &mut self,
        template: NodeId,
        substitution_expression: NodeId,
    ) -> Option<TypeId> {
        let template_parent = self.store().node(template).parent.get();
        if self.store().node(template_parent).kind == Kind::TaggedTemplateExpression {
            return self.get_contextual_type_for_argument(
                template_parent,
                substitution_expression,
            );
        }
        None
    }

    /// Go: `func (c *Checker) getSpreadIndices(node *ast.Node) (int, int)`
    /// (checker.go:30672). PORT: computed per call rather than through Go's
    /// arrayLiteralLinks cache (the cache rides the links-arena wave).
    fn get_spread_indices(&self, elements: &[NodeId]) -> (i32, i32) {
        let mut first = -1;
        let mut last = -1;
        for (i, &element) in elements.iter().enumerate() {
            if self.store().node(element).kind == Kind::SpreadElement {
                if first < 0 {
                    first = i as i32;
                }
                last = i as i32;
            }
        }
        (first, last)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Yield / await operands (Go checker.go:30265-30309)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getContextualTypeForYieldOperand(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30265).
    pub(crate) fn get_contextual_type_for_yield_operand(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let fn_node = crate::utilities::get_containing_function(self.store(), node)?;
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        let is_async = function_flags.intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
        let contextual_return_type = self.get_contextual_return_type(fn_node, context_flags)?;
        let is_yield_star = match &self.store().node(node).data {
            NodeData::YieldExpression(y) => y.asterisk_token.is_some(),
            _ => false,
        };
        if !is_yield_star
            && self.type_(contextual_return_type).flags.intersects(TypeFlags::UNION)
        {
            let members = self.type_(contextual_return_type).types().to_vec();
            let mut filtered = Vec::new();
            for t in members {
                if self
                    .get_iteration_type_of_generator_function_return_type(
                        crate::statements::IterationTypeKind::Return,
                        t,
                        is_async,
                    )
                    .is_some()
                {
                    filtered.push(t);
                }
            }
            let filtered = self.get_union_type_ex(
                filtered,
                crate::todo_shapes::UnionReduction::None,
                None,
                None,
            );
            return self.get_iteration_type_of_generator_function_return_type(
                crate::statements::IterationTypeKind::Yield,
                filtered,
                is_async,
            );
        }
        // TODO(port M6 S9): the yield* arm's Generator-type construction
        // (createGeneratorType over iterationTypes + the getContextualType
        // fallback) rides the generator iteration wave; the yield iteration
        // type is the operand's core contextual answer for both arms.
        self.get_iteration_type_of_generator_function_return_type(
            crate::statements::IterationTypeKind::Yield,
            contextual_return_type,
            is_async,
        )
    }

    /// Go: `func (c *Checker) getContextualTypeForAwaitOperand(node *ast.Node, contextFlags ContextFlags) *Type`
    /// (checker.go:30297).
    pub(crate) fn get_contextual_type_for_await_operand(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<TypeId> {
        let contextual_type = self.get_contextual_type(node, context_flags)?;
        let contextual_awaited_type = self.get_awaited_type_no_alias(contextual_type)?;
        // PORT: hoisted — the union constructor takes the pair by value.
        let promise_like = self.create_promise_like_type(contextual_awaited_type);
        Some(self.get_union_type_ex(
            vec![contextual_awaited_type, promise_like],
            crate::todo_shapes::UnionReduction::None,
            None,
            None,
        ))
    }

    // ────────────────────────────────────────────────────────────────────────
    // The contextual property lookup (Go checker.go:31100 — the non-mapped,
    // non-intersection core; the mapped-type/index-info refinement is
    // S10-bannered inside)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getTypeOfPropertyOfContextualType(t *Type, name string) *Type`
    /// (checker.go:31100). PORT: the non-intersection arms are real; the
    /// intersection constituent walk's mapped-type substitution
    /// (getIndexedMappedTypeSubstitutedTypeOfContextualType) rides the S10
    /// mapped-type family.
    pub(crate) fn get_type_of_property_of_contextual_type_impl(
        &mut self,
        t: TypeId,
        name: &str,
    ) -> Option<TypeId> {
        let flags = self.type_(t).flags;
        if !flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            return self.get_type_of_property_of_type_impl(t, name);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            // Go walks the constituents' concrete properties and index
            // infos; the plain per-constituent lookup below is the common
            // case.
            let constituents = self.type_(t).types().to_vec();
            for constituent in constituents {
                if let Some(property_type) =
                    self.get_type_of_property_of_type_impl(constituent, name)
                {
                    return Some(property_type);
                }
            }
            return None;
        }
        // Union: map the property lookup over the constituents, keeping the
        // union of hits (Go's mapTypeEx shape).
        let constituents = self.type_(t).types().to_vec();
        let mut hits = Vec::new();
        for constituent in constituents {
            if let Some(property_type) =
                self.get_type_of_property_of_type_impl(constituent, name)
            {
                hits.push(property_type);
            }
        }
        if hits.is_empty() {
            return None;
        }
        Some(self.get_union_type_ex(
            hits,
            crate::todo_shapes::UnionReduction::None,
            None,
            None,
        ))
    }

    /// Go: `func (c *Checker) getTypeOfConcretePropertyOfContextualType(t *Type, name string) *Type`
    /// (checker.go — the concrete-object arm used by the intersection walk).
    #[allow(dead_code)] // the intersection refinement caller is S10's
    fn get_type_of_concrete_property_of_contextual_type(
        &mut self,
        t: TypeId,
        name: &str,
    ) -> Option<TypeId> {
        if !self.type_(t).flags.intersects(TypeFlags::OBJECT) {
            return None;
        }
        self.get_type_of_property_of_type_impl(t, name)
    }

    // ────────────────────────────────────────────────────────────────────────
    // The contextual function-expression check + the parameter-assign
    // family (Go checker.go:10387-10436 + 10470-10680 — the W3T5 tranche:
    // the async-2697 root cause; the S8/S9 seam this composed around is
    // now real, so the deferred signature resolution is forced exactly
    // where Go forces it).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) contextuallyCheckFunctionExpressionOrObjectLiteralMethod(node, checkMode)`
    /// (checker.go:10387) — REAL (W3T5; was the expressions.rs stub that
    /// bailed at the S8 `signature == nil` seam).
    pub(crate) fn contextually_check_function_expression_or_object_literal_method(
        &mut self,
        node: NodeId,
        check_mode: crate::checker::CheckMode,
    ) {
        // Check if function expression is contextually typed and assign
        // parameter types if so (Go's comment).
        if !self
            .node_links
            .get(node)
            .flags
            .intersects(crate::types::NodeCheckFlags::CONTEXT_CHECKED)
        {
            let contextual_signature = self.get_contextual_signature(node);
            // If a type check is started at a function expression that is an
            // argument of a function call, obtaining the contextual type may
            // recursively get back to here during overload resolution of the
            // call. If so, we will have already assigned contextual types
            // (Go's comment).
            if !self
                .node_links
                .get(node)
                .flags
                .intersects(crate::types::NodeCheckFlags::CONTEXT_CHECKED)
            {
                self.node_links.get(node).flags |=
                    crate::types::NodeCheckFlags::CONTEXT_CHECKED;
                let symbol = crate::typeofsymbol::get_symbol_of_declaration(self, node)
                    .expect("function expression has a symbol");
                let symbol_type = self.get_type_of_symbol(symbol);
                let signature = self
                    .get_signatures_of_type_impl(symbol_type, crate::types::SignatureKind::Call)
                    .first()
                    .copied();
                let Some(signature) = signature else {
                    return;
                };
                if self.is_context_sensitive(node) {
                    if let Some(contextual_signature) = contextual_signature {
                        let inference_context = self.get_inference_context(node);
                        let mut instantiated_contextual_signature: Option<crate::types::SignatureId> = None;
                        if check_mode.intersects(crate::checker::CheckMode::INFERENTIAL) {
                            self.infer_from_annotated_parameters_and_return(
                                signature,
                                contextual_signature,
                                inference_context,
                            );
                            let rest_type = self.get_effective_rest_type(contextual_signature);
                            if let Some(rest_type) =
                                rest_type.filter(|&t| self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER))
                            {
                                let _ = rest_type;
                                // Go: instantiateSignature(contextualSignature,
                                // inferenceContext.nonFixingMapper).
                                instantiated_contextual_signature = Some(
                                    self.instantiate_signature(
                                        contextual_signature,
                                        inference_context
                                            .and_then(|n| self.inference_contexts[n.0 as usize].non_fixing_mapper),
                                    ),
                                );
                            }
                        }
                        if instantiated_contextual_signature.is_none() {
                            // PORT: Go branches on `inferenceContext != nil`
                            // (its mapper is always non-nil once the context
                            // exists; the port's instantiate_signature(None)
                            // is the identity seam — the None-mapper case
                            // never carries the fixing mapper's inferences).
                            instantiated_contextual_signature = Some(match inference_context {
                                Some(n) => self.instantiate_signature(
                                    contextual_signature,
                                    self.inference_contexts[n.0 as usize].mapper,
                                ),
                                None => contextual_signature,
                            });
                        }
                        let instantiated = instantiated_contextual_signature
                            .expect("set in both arms above");
                        self.assign_contextual_parameter_types(signature, instantiated);
                    } else {
                        // Force resolution of all parameter types such that
                        // the absence of a contextual type is consistently
                        // reflected (Go's comment).
                        self.assign_non_contextual_parameter_types(signature);
                    }
                } else if contextual_signature.is_some()
                    && type_parameters_of_node(self.store(), node).is_none()
                    && self.signatures[contextual_signature.expect("checked some").0 as usize]
                        .parameters
                        .len()
                        > crate::ast_shims::parameters_of(self.store(), node).len()
                {
                    let inference_context = self.get_inference_context(node);
                    if check_mode.intersects(crate::checker::CheckMode::INFERENTIAL) {
                        self.infer_from_annotated_parameters_and_return(
                            signature,
                            contextual_signature.expect("checked some"),
                            inference_context,
                        );
                    }
                }
                if contextual_signature.is_some()
                    && self.get_return_type_from_annotation(node).is_none()
                    && self.signatures[signature.0 as usize].resolved_return_type.is_none()
                {
                    // resolvedReturnType is cached indefinitely, so the
                    // return type here has to be computed without
                    // CheckModeSkipContextSensitive; otherwise anyFunctionType
                    // could leak as part of the computed (and cached) return
                    // type (Go's comment).
                    let return_type = self.get_return_type_from_body(
                        node,
                        check_mode & !crate::checker::CheckMode::SKIP_CONTEXT_SENSITIVE,
                    );
                    if self.signatures[signature.0 as usize]
                        .resolved_return_type
                        .is_none()
                    {
                        self.signatures[signature.0 as usize].resolved_return_type = Some(return_type);
                    }
                }
                self.check_signature_declaration(node);
            }
        }
    }

    /// Go: `func (c *Checker) inferFromAnnotatedParametersAndReturn(sig, context, inferenceContext)`
    /// (checker.go:10470).
    ///
    /// PORT: Go passes `inferenceContext *InferenceContext` and dereferences
    /// `.inferences` unconditionally — at both call sites the context is
    /// non-nil whenever CheckModeInferential is set (createInferenceContext
    /// always seeds the mapper pair); the port carries the Option and
    /// no-ops on None (a nil dereference in Go, never reached on the
    /// corpus).
    pub(crate) fn infer_from_annotated_parameters_and_return(
        &mut self,
        sig: crate::types::SignatureId,
        context: crate::types::SignatureId,
        inference_context: Option<crate::todo_shapes::InferenceContextId>,
    ) {
        let Some(inference_context) = inference_context else {
            return;
        };
        let parameters = self.signatures[sig.0 as usize].parameters.clone();
        let length =
            parameters.len() - usize::from(self.signatures[sig.0 as usize].has_rest_parameter());
        for (i, &parameter) in parameters.iter().enumerate().take(length) {
            let Some(declaration) = self.program_symbol(parameter).value_declaration else {
                // PORT: Go dereferences the (never-nil-on-corpus)
                // ValueDeclaration; the transient-symbol case is skipped.
                continue;
            };
            if let Some(type_node) = crate::ast_shims::type_of_node(self.store(), declaration) {
                let from_node = self.get_type_from_type_node(type_node);
                let source = self.add_optionality_ex(
                    from_node,
                    false, /*isProperty*/
                    crate::utilities::is_optional_declaration(self.store(), declaration),
                );
                let target = self.get_type_at_position(context, i);
                self.infer_types(
                    inference_context,
                    source,
                    target,
                    crate::inference::InferencePriority::NONE,
                    false, /*contravariant*/
                );
            }
        }
        if let Some(declaration) = self.signatures[sig.0 as usize].declaration {
            if let Some(return_type_node) = crate::ast_shims::type_of_node(self.store(), declaration) {
                let source = self.get_type_from_type_node(return_type_node);
                let target = self.get_return_type_of_signature_impl(context);
                self.infer_types(
                    inference_context,
                    source,
                    target,
                    crate::inference::InferencePriority::NONE,
                    false, /*contravariant*/
                );
            }
        }
    }

    /// Go: `func (c *Checker) assignContextualParameterTypes(sig, context)`
    /// (checker.go:10580).
    pub(crate) fn assign_contextual_parameter_types(
        &mut self,
        sig: crate::types::SignatureId,
        context: crate::types::SignatureId,
    ) {
        let context_type_parameters =
            self.signatures[context.0 as usize].type_parameters.clone();
        if !context_type_parameters.is_empty() {
            if !self.signatures[sig.0 as usize].type_parameters.is_empty() {
                // This signature has already has a contextual inference
                // performed and cached on it (Go's comment).
                return;
            }
            self.signatures[sig.0 as usize].type_parameters = context_type_parameters;
        }
        let context_this_parameter = self.signatures[context.0 as usize].this_parameter;
        if let Some(context_this_parameter) = context_this_parameter {
            let parameter = self.signatures[sig.0 as usize].this_parameter;
            // PORT: Go's short-circuit `parameter == nil || (parameter.ValueDeclaration != nil && parameter.ValueDeclaration.Type() == nil)`.
            let needs_assign = match parameter {
                None => true,
                Some(parameter) => {
                    let value_declaration = self.program_symbol(parameter).value_declaration;
                    match value_declaration {
                        Some(declaration) => {
                            crate::ast_shims::type_of_node(self.store(), declaration).is_none()
                        }
                        None => false,
                    }
                }
            };
            if needs_assign {
                let this_parameter = match parameter {
                    None => {
                        let created = self.create_symbol_with_type(context_this_parameter, None);
                        self.signatures[sig.0 as usize].this_parameter = Some(created);
                        created
                    }
                    Some(parameter) => parameter,
                };
                let t = self.get_type_of_symbol(context_this_parameter);
                self.assign_parameter_type(this_parameter, Some(t));
            }
        }
        let parameters = self.signatures[sig.0 as usize].parameters.clone();
        let length = parameters.len()
            - usize::from(self.signatures[sig.0 as usize].has_rest_parameter());
        for (i, &parameter) in parameters.iter().enumerate().take(length) {
            let declaration = self.program_symbol(parameter).value_declaration;
            let has_annotation = declaration
                .and_then(|d| crate::ast_shims::type_of_node(self.store(), d))
                .is_some();
            if !has_annotation {
                let mut t = self.try_get_type_at_position(context, i);
                if let (Some(t_some), Some(declaration)) = (t, declaration) {
                    if crate::ast_shims::initializer_of(self.store(), declaration).is_some() {
                        let initializer_type = self.check_declaration_initializer(
                            declaration,
                            crate::checker::CheckMode::CHECK_MODE_NORMAL,
                            None, /*contextualType*/
                        );
                        if !self.is_type_assignable_to(initializer_type, t_some) {
                            let widened =
                                self.widen_type_inferred_from_initializer(declaration, initializer_type);
                            if self.is_type_assignable_to(t_some, widened) {
                                t = Some(widened);
                            }
                        }
                    }
                }
                self.assign_parameter_type(parameter, t);
            }
        }
        if self.signatures[sig.0 as usize].has_rest_parameter() {
            // parameter might be a transient symbol generated by use of
            // `arguments` in the function body (Go's comment).
            let Some(parameter) = parameters.last().copied() else {
                return;
            };
            let value_declaration = self.program_symbol(parameter).value_declaration;
            let needs_assign = match value_declaration {
                Some(declaration) => {
                    crate::ast_shims::type_of_node(self.store(), declaration).is_none()
                }
                None => self
                    .program_symbol(parameter)
                    .check_flags
                    .intersects(tsc_ast::CheckFlags::DEFERRED_TYPE),
            };
            if needs_assign {
                let contextual_parameter_type =
                    self.get_rest_type_at_position(context, length, false /*readonly*/);
                self.assign_parameter_type(parameter, contextual_parameter_type);
            }
        }
    }

    /// Go: `func (c *Checker) assignNonContextualParameterTypes(signature)`
    /// (checker.go:10626).
    pub(crate) fn assign_non_contextual_parameter_types(
        &mut self,
        signature: crate::types::SignatureId,
    ) {
        if let Some(this_parameter) = self.signatures[signature.0 as usize].this_parameter {
            self.assign_parameter_type(this_parameter, None);
        }
        let parameters = self.signatures[signature.0 as usize].parameters.clone();
        for parameter in parameters {
            self.assign_parameter_type(parameter, None);
        }
    }

    /// Go: `func (c *Checker) assignParameterType(parameter, contextualType)`
    /// (checker.go:10635).
    pub(crate) fn assign_parameter_type(
        &mut self,
        parameter: tsc_ast::SymbolId,
        contextual_type: Option<TypeId>,
    ) {
        if self.value_symbol_links.get(parameter).resolved_type.is_some() {
            return;
        }
        let declaration = self.program_symbol(parameter).value_declaration;
        let t = match contextual_type {
            Some(t) => Some(t),
            None => match declaration {
                Some(declaration) => Some(
                    self.get_widened_type_for_variable_like_declaration(
                        declaration,
                        true, /*reportErrors*/
                    ),
                ),
                None => Some(self.get_type_of_symbol(parameter)),
            },
        };
        let t = t.expect("either the contextual type or a resolved type");
        let is_optional = match declaration {
            Some(declaration) => {
                crate::ast_shims::initializer_of(self.store(), declaration).is_none()
                    && crate::utilities::is_optional_declaration(self.store(), declaration)
            }
            None => false,
        };
        let resolved = self.add_optionality_ex(t, false /*isProperty*/, is_optional);
        self.value_symbol_links.get(parameter).resolved_type = Some(resolved);
        if let Some(declaration) = declaration {
            let name = self.store().node(declaration).name();
            let is_identifier = name.is_some_and(|n| self.store().node(n).kind == Kind::Identifier);
            if !is_identifier {
                let name = name.expect("parameter declarations carry a name");
                // if inference didn't come up with anything but unknown,
                // fall back to the binding pattern if present (Go's comment).
                if resolved == self.unknown_type() {
                    let from_pattern =
                        self.get_type_from_binding_pattern(name, false, false);
                    self.value_symbol_links.get(parameter).resolved_type = Some(from_pattern);
                    self.assign_binding_element_types(name, from_pattern);
                } else {
                    self.assign_binding_element_types(name, resolved);
                }
            }
        }
    }

    /// Go: `func (c *Checker) assignBindingElementTypes(pattern, parentType)`
    /// (checker.go:10661) — when contextual typing assigns a type to a
    /// parameter that contains a binding pattern, we also need to push the
    /// destructured type into the contained binding elements (Go's comment).
    pub(crate) fn assign_binding_element_types(&mut self, pattern: NodeId, parent_type: TypeId) {
        let elements: Vec<NodeId> = self
            .store()
            .node(pattern)
            .as_binding_pattern()
            .expect("BindingPattern data")
            .elements
            .clone()
            .map(|list| list.nodes.to_vec())
            .unwrap_or_default();
        for element in elements {
            if let Some(name) = self.store().node(element).name() {
                let t = self.get_binding_element_type_from_parent_type(
                    element,
                    parent_type,
                    false, /*noTupleBoundsCheck*/
                );
                if self.store().node(name).kind == Kind::Identifier {
                    if let Some(symbol) =
                        crate::typeofsymbol::get_symbol_of_declaration(self, element)
                    {
                        self.value_symbol_links.get(symbol).resolved_type = Some(t);
                    }
                } else {
                    self.assign_binding_element_types(name, t);
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Small local walkers/predicates (Go ast.ForEachReturnStatement /
// ForEachYieldExpression / GetThisParameter / IsComputedNonLiteralName)
// ────────────────────────────────────────────────────────────────────────────

fn collect_return_statements(store: &dyn tsc_ast::NodeStore, node: NodeId, out: &mut Vec<NodeId>) {
    if store.node(node).kind == Kind::ReturnStatement {
        out.push(node);
        return;
    }
    store.node(node).for_each_child(&mut |child| {
        collect_return_statements(store, child, out);
        false
    });
}

fn collect_yield_expressions(store: &dyn tsc_ast::NodeStore, node: NodeId, out: &mut Vec<NodeId>) {
    if store.node(node).kind == Kind::YieldExpression {
        out.push(node);
        return;
    }
    store.node(node).for_each_child(&mut |child| {
        collect_yield_expressions(store, child, out);
        false
    });
}

/// Go: `func GetThisParameter(fn *Node) *Node` — the first parameter when it
/// is a `this` parameter.
fn get_this_parameter(store: &dyn tsc_ast::NodeStore, fn_node: NodeId) -> Option<NodeId> {
    let first = crate::ast_shims::parameters_of(store, fn_node).first().copied();
    first.filter(|&p| crate::ast_shims::is_this_parameter(store, p))
}

/// Go: `func IsComputedNonLiteralName(name *Node) bool` (ast/utilities.go:2181).
fn is_computed_non_literal_name(store: &dyn tsc_ast::NodeStore, name: NodeId) -> bool {
    store.node(name).kind == Kind::ComputedPropertyName
        && {
            let NodeData::ComputedPropertyName(d) = &store.node(name).data else {
                return false;
            };
            !tsc_ast::utilities::is_string_or_numeric_literal_like(store.node(d.expression))
        }
}

fn type_parameters_of_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Option<tsc_ast::NodeList> {
    match &store.node(node).data {
        NodeData::FunctionDeclaration(d) => d.type_parameters.clone(),
        NodeData::FunctionExpression(d) => d.type_parameters.clone(),
        NodeData::ArrowFunction(d) => d.type_parameters.clone(),
        NodeData::MethodDeclaration(d) => d.type_parameters.clone(),
        NodeData::MethodSignatureDeclaration(d) => d.type_parameters.clone(),
        NodeData::ConstructorDeclaration(d) => d.type_parameters.clone(),
        _ => None,
    }
}

