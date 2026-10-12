// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// (the call-resolution region 8424–10258 + 30591–30705).
//
// M6 S6 Section C — the CallExpression resolution family: getResolvedSignature
// → resolveSignature → resolveCallExpression/NewExpression/TaggedTemplate/
// Decorator/Instanceof → resolveCall (CallState) → chooseOverload, the arity
// ladders, isSignatureApplicable/inferTypeArguments, the overload-failure
// reporting family, resolveUntypedCall/resolveErrorCall/isUntypedFunctionCall,
// the invocationError family, getEffectiveCallArguments, and the
// checkCallExpression/checkImportCallExpression wrappers.
//
// SEAM NOTES (the permissive surfaces this wave composes around):
//  * S8 (getSignaturesOfType): typed calls cannot see call/construct
//    signatures yet (the structured-type members resolution is S8's). The
//    resolve*Expression arms therefore carry an explicit guard: a
//    zero-signature result for a non-any/unknown type composes Go's
//    *untyped-call* path (anySignature) instead of Go's TS2349/TS2351
//    invocationError, which would fire falsely for every ordinary function
//    call until S8 lands. When S8 makes getSignaturesOfType real, the guard
//    is dead code and the error arms come alive unchanged.
//  * S7 (signature instantiation): getSignatureInstantiation is the identity
//    stub (relater.rs), so chooseOverload's generic candidates pass through
//    un-instantiated — the arity/inference ladder still runs (the inference
//    engine is real, M6 W2).
//  * S9 (contextual return inference): inferTypeArguments' contextual-return
//    half needs getContextualType; the seam answers None so the arm is dormant.
//  * decorator wave: getDecoratorCallSignature / the synthetic decorator
//    argument list are not landed; the decorator arm composes resolveErrorCall
//    permissively.

use tsc_ast::{
    is_identifier, is_property_access_expression, Kind, ModifierFlags, NodeId,
    NodeStore, SymbolFlags, SymbolId,
};
use tsc_diagnostics as diag;
use tsc_diagnostics::Message;

use tsc_binder::ast_shims as binder_shims;
use tsc_binder::node_data as binder_node_data;

use crate::checker::CheckMode;
use crate::relater::RelationKind;
use crate::types::{SignatureFlags, SignatureId, TypeFlags, TypeId};
use crate::utilities as u;
use crate::Checker;

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // getResolvedSignature + resolveSignature (Go checker.go:8659–8673)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getResolvedSignature(node, candidatesOutArray, checkMode) *Signature`
    /// (checker.go:8659).
    pub(crate) fn get_resolved_signature_ex(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        // If getResolvedSignature has already been called, we will have cached
        // the resolvedSignature. However, it is possible that either
        // candidatesOutArray was not passed in the first time, or that a
        // different candidatesOutArray was passed in. Therefore, we need to
        // redo the work to correctly fill the candidatesOutArray.
        let cached = self.signature_node_links.get(node).resolved_signature;
        if cached.is_some()
            && cached != self.resolving_signature
            && candidates_out_array.is_none()
        {
            return cached;
        }
        let save_resolution_start = self.resolution_start;
        if cached.is_none() {
            // Temporarily reset the resolution stack so API-client inversions
            // retry symbol type resolution with the resolvingSignature marker
            // in place (suppression of the contextual-type circularity).
            self.resolution_start = self.type_resolutions.len();
        }
        self.signature_node_links.get(node).resolved_signature = self.resolving_signature;
        let result = self.resolve_signature(node, candidates_out_array, check_mode);
        self.resolution_start = save_resolution_start;
        // When CheckMode.SkipGenericFunctions is set we use resolvingSignature
        // to indicate that call resolution should be deferred.
        if result != self.resolving_signature {
            // If signature resolution originated on a node that itself depends
            // on the contextual type then it's possible that the resolved
            // signature might not be the same as the one that would be
            // computed in source order — the inner resolution may have cached
            // the resolvedSignature first; reuse it.
            let links_cached = self.signature_node_links.get(node).resolved_signature;
            let result = if links_cached != self.resolving_signature {
                links_cached
            } else {
                result
            };
            // If signature resolution originated in control flow type analysis
            // we don't cache the result as it may be based on temporary types.
            if self.flow_loop_stack.is_empty() {
                self.signature_node_links.get(node).resolved_signature = result;
            } else {
                self.signature_node_links.get(node).resolved_signature = cached;
            }
            return result;
        }
        result
    }

    /// Go: `func (c *Checker) resolveSignature(node, candidatesOutArray, checkMode) *Signature`
    /// (checker.go:8666).
    fn resolve_signature(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        match self.store().node(node).kind {
            Kind::CallExpression => {
                self.resolve_call_expression(node, candidates_out_array, check_mode)
            }
            Kind::NewExpression => {
                self.resolve_new_expression(node, candidates_out_array, check_mode)
            }
            Kind::TaggedTemplateExpression => {
                self.resolve_tagged_template_expression(node, candidates_out_array, check_mode)
            }
            Kind::Decorator => self.resolve_decorator(node, candidates_out_array, check_mode),
            Kind::JsxOpeningFragment | Kind::JsxOpeningElement | Kind::JsxSelfClosingElement => {
                // TODO(port M6 jsx wave): resolveJsxOpeningLikeElement — the
                // JSX attribute/type-argument surface; the untyped-call
                // composition keeps JSX calls any-typed.
                self.resolve_untyped_call(node)
            }
            Kind::BinaryExpression => {
                self.resolve_instanceof_expression(node, candidates_out_array, check_mode)
            }
            _ => panic!(
                "Unhandled case in resolveSignature: {}",
                self.store().node(node).kind_string()
            ),
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // The per-kind resolvers (Go checker.go:8680–9039)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) resolveCallExpression(node, candidatesOutArray, checkMode) *Signature`
    /// (checker.go:8680).
    fn resolve_call_expression(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .unwrap_or(NodeId::NONE);
        if expression != NodeId::NONE && self.store().node(expression).kind == Kind::SuperKeyword {
            let super_type = self.check_super_expression(expression);
            if crate::utilities::is_type_any(Some(self.type_(super_type))) {
                for arg in binder_node_data::arguments_of(self.store(), node).to_vec() {
                    // Still visit arguments so they get marked for visibility, etc
                    let _ = self.check_expression(arg);
                }
                return self.any_signature;
            }
            if !self.is_error_type(super_type) {
                // In super call, the candidate signatures are the matching
                // arity signatures of the base constructor function
                // instantiated with the type arguments specified in the
                // extends clause.
                let containing_class = crate::utilities::get_containing_class(self.store(), node);
                let base_type_node = containing_class
                    .and_then(|c| crate::statements::get_class_extends_heritage_element(self.store(), c));
                if let Some(base_type_node) = base_type_node {
                    let type_arguments = crate::statements::type_arguments_of(
                        self.store(),
                        base_type_node,
                    );
                    // S8: getInstantiatedConstructorsForTypeArguments is the
                    // REAL members.rs body (checker.go:19717) — the
                    // constructor set instantiates with the extends-clause
                    // type arguments.
                    let base_constructors = self
                        .get_instantiated_constructors_for_type_arguments(
                            super_type,
                            &type_arguments,
                            base_type_node,
                        );
                    if !base_constructors.is_empty() {
                        return self.resolve_call(
                            node,
                            &base_constructors,
                            candidates_out_array,
                            check_mode,
                            SignatureFlags::NONE,
                            None,
                        );
                    }
                }
            }
            return self.resolve_untyped_call(node);
        }
        if is_import_call(self.store(), node) {
            return self.resolve_untyped_call(node);
        }
        let mut call_chain_flags = SignatureFlags::NONE;
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .unwrap_or(NodeId::NONE);
        let mut func_type = if expression != NodeId::NONE {
            self.check_expression(expression)
        } else {
            self.error_type()
        };
        if expression != NodeId::NONE && u::is_call_chain(self.store().node(node)) {
            let non_optional_type = self.get_optional_expression_type(func_type, expression);
            call_chain_flags = if non_optional_type == func_type {
                SignatureFlags::NONE
            } else if binder_shims::is_outermost_optional_chain(self.store(), node) {
                SignatureFlags::IS_OUTER_CALL_CHAIN
            } else {
                SignatureFlags::IS_INNER_CALL_CHAIN
            };
            func_type = non_optional_type;
        }
        func_type = self.check_non_null_type_with_reporter(
            func_type,
            expression,
            Self::report_cannot_invoke_possibly_null_or_undefined_error,
        );
        if func_type == self.silent_never_type() {
            return self.silent_never_signature;
        }
        let apparent_type = self.get_apparent_type(func_type);
        if self.is_error_type(apparent_type) {
            // Another error has already been reported
            return self.resolve_error_call(node);
        }
        // Technically, this signatures list may be incomplete (see Go note).
        let call_signatures = self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Call);
        let num_construct_signatures =
            self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Construct).len();
        // TS 1.0 Spec: 4.12 — the untyped function call.
        if self.is_untyped_function_call(
            func_type,
            apparent_type,
            call_signatures.len(),
            num_construct_signatures,
        ) {
            // The unknownType indicates that an error already occurred (and
            // was reported). No need to report another error in this case.
            if !self.is_error_type(func_type)
                && !crate::statements::type_arguments_of(self.store(), node).is_empty()
            {
                self.error(node, &diag::UNTYPED_FUNCTION_CALLS_MAY_NOT_ACCEPT_TYPE_ARGUMENTS, &[]);
            }
            return self.resolve_untyped_call(node);
        }
        // If FuncExpr's apparent type is a function type, the call is a typed
        // function call (overload resolution applies).
        if call_signatures.is_empty() {
            // S8: the members are REAL — Go's checker.go:8749 arm, verbatim
            // (the S8-seam-era untyped-call composition is retired). The
            // missing-semicolon related-information probe rides the scanner
            // surface (TODO(port M6 scanner wave)).
            if num_construct_signatures != 0 {
                let func_type_string = self.type_to_string(func_type);
                self.error(
                    node,
                    &diag::VALUE_OF_TYPE_0_IS_NOT_CALLABLE_DID_YOU_MEAN_TO_INCLUDE_NEW,
                    &[func_type_string],
                );
            } else {
                self.invocation_error(
                    node,
                    apparent_type,
                    crate::types::SignatureKind::Call,
                    None,
                );
            }
            return self.resolve_error_call(node);
        }
        // The deferred-generic-function heuristic (resolvingSignature).
        if check_mode.intersects(CheckMode::SKIP_GENERIC_FUNCTIONS)
            && crate::statements::type_arguments_of(self.store(), node).is_empty()
            && call_signatures
                .iter()
                .any(|&sig| self.is_generic_function_returning_function(sig))
        {
            self.skipped_generic_function(node, check_mode);
            return self.resolving_signature;
        }
        self.resolve_call(
            node,
            &call_signatures,
            candidates_out_array,
            check_mode,
            call_chain_flags,
            None,
        )
    }

    /// Go: `func (c *Checker) resolveNewExpression(node, candidatesOutArray, checkMode) *Signature`
    /// (checker.go:8775).
    fn resolve_new_expression(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .unwrap_or(NodeId::NONE);
        let mut expression_type = if expression != NodeId::NONE {
            self.check_non_null_expression(expression)
        } else {
            self.error_type()
        };
        if expression_type == self.silent_never_type() {
            return self.silent_never_signature;
        }
        // TS 1.0 spec: 4.11 — apparent type with construct signatures.
        expression_type = self.get_apparent_type(expression_type);
        if self.is_error_type(expression_type) {
            return self.resolve_error_call(node);
        }
        if crate::utilities::is_type_any(Some(self.type_(expression_type))) {
            if !crate::statements::type_arguments_of(self.store(), node).is_empty() {
                self.error(node, &diag::UNTYPED_FUNCTION_CALLS_MAY_NOT_ACCEPT_TYPE_ARGUMENTS, &[]);
            }
            return self.resolve_untyped_call(node);
        }
        let construct_signatures =
            self.get_signatures_of_type(expression_type, crate::types::SignatureKind::Construct);
        if !construct_signatures.is_empty() {
            let accessibility_error = self.get_constructor_accessibility_error(
                node,
                &construct_signatures,
                ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER,
            );
            if let Some((kind, declaring_class)) = accessibility_error {
                let declaring_class_string = self.type_to_string(declaring_class);
                if kind.intersects(ModifierFlags::PRIVATE) {
                    self.error(
                        node,
                        &diag::CONSTRUCTOR_OF_CLASS_0_IS_PRIVATE_AND_ONLY_ACCESSIBLE_WITHIN_THE_CLASS_DECLARATION,
                        std::slice::from_ref(&declaring_class_string),
                    );
                }
                if kind.intersects(ModifierFlags::PROTECTED) {
                    self.error(
                        node,
                        &diag::CONSTRUCTOR_OF_CLASS_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_WITHIN_THE_CLASS_DECLARATION,
                        std::slice::from_ref(&declaring_class_string),
                    );
                }
                return self.resolve_error_call(node);
            }
            // Abstract classes / abstract construct signatures cannot be
            // instantiated.
            if construct_signatures.iter().any(|&sig| {
                self.signatures[sig.0 as usize]
                    .flags
                    .intersects(SignatureFlags::ABSTRACT)
            }) {
                self.error(node, &diag::CANNOT_CREATE_AN_INSTANCE_OF_AN_ABSTRACT_CLASS, &[]);
                return self.resolve_error_call(node);
            }
            if let Some(symbol) = self.type_(expression_type).symbol {
                let value_decl =
                    crate::expressions::get_class_like_declaration_of_symbol_for(self, symbol);
                if value_decl
                    .filter(|&d| crate::ast_shims::has_modifier(
                        self.store().node(d),
                        ModifierFlags::ABSTRACT,
                    ))
                    .is_some()
                {
                    self.error(node, &diag::CANNOT_CREATE_AN_INSTANCE_OF_AN_ABSTRACT_CLASS, &[]);
                    return self.resolve_error_call(node);
                }
            }
            return self.resolve_call(
                node,
                &construct_signatures,
                candidates_out_array,
                check_mode,
                SignatureFlags::NONE,
                None,
            );
        }
        // Apparent type with call signatures but no construct signatures: the
        // expression is processed as a function call; a compile-time error
        // occurs if the result is not Void; the result is Any.
        let call_signatures =
            self.get_signatures_of_type(expression_type, crate::types::SignatureKind::Call);
        if !call_signatures.is_empty() {
            let signature = self.resolve_call(
                node,
                &call_signatures,
                candidates_out_array,
                check_mode,
                SignatureFlags::NONE,
                None,
            );
            if let Some(signature) = signature {
                if !self.no_implicit_any {
                    let declaration = self.signatures[signature.0 as usize].declaration;
                    if declaration.is_some()
                        && self.get_return_type_of_signature(signature) != self.void_type()
                    {
                        self.error(
                            node,
                            &diag::ONLY_A_VOID_FUNCTION_CAN_BE_CALLED_WITH_THE_NEW_KEYWORD,
                            &[],
                        );
                    }
                    if self.get_this_type_of_signature(signature) == Some(self.void_type()) {
                        self.error(
                            node,
                            &diag::A_FUNCTION_THAT_IS_CALLED_WITH_THE_NEW_KEYWORD_CANNOT_HAVE_A_THIS_TYPE_THAT_IS_VOID,
                            &[],
                        );
                    }
                }
            }
            return signature;
        }
        // No construct signatures and no call signatures (Go's tail,
        // checker.go:8856): invocationError directly — IsTypeAny was handled
        // above, so there is no untyped-call arm here (the S8-seam-era
        // `0, 0` guard is retired).
        self.invocation_error(
            expression,
            expression_type,
            crate::types::SignatureKind::Construct,
            None,
        );
        self.resolve_error_call(node)
    }

    /// Go: `func (c *Checker) resolveTaggedTemplateExpression(...) *Signature`
    /// (checker.go:8906).
    fn resolve_tagged_template_expression(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        let tag = match &self.store().node(node).data {
            tsc_ast::NodeData::TaggedTemplateExpression(d) => d.tag,
            _ => return self.resolve_error_call(node),
        };
        let tag_type = self.check_expression(tag);
        let apparent_type = self.get_apparent_type(tag_type);
        if self.is_error_type(apparent_type) {
            // Another error has already been reported
            return self.resolve_error_call(node);
        }
        let call_signatures =
            self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Call);
        let num_construct_signatures =
            self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Construct).len();
        if self.is_untyped_function_call(
            tag_type,
            apparent_type,
            call_signatures.len(),
            num_construct_signatures,
        ) {
            return self.resolve_untyped_call(node);
        }
        if call_signatures.is_empty() {
            let parent = self.store().node(node).parent.get();
            if parent != NodeId::NONE
                && self.store().node(parent).kind == Kind::ArrayLiteralExpression
            {
                self.error(
                    tag,
                    &diag::IT_IS_LIKELY_THAT_YOU_ARE_MISSING_A_COMMA_TO_SEPARATE_THESE_TWO_TEMPLATE_EXPRESSIONS_THEY_FORM_A_TAGGED_TEMPLATE_EXPRESSION_WHICH_CANNOT_BE_INVOKED,
                    &[],
                );
                return self.resolve_error_call(node);
            }
            // S8: members are real — Go's invocationError arm
            // (checker.go:8932), verbatim (the seam-era untyped-call
            // composition is retired).
            self.invocation_error(tag, apparent_type, crate::types::SignatureKind::Call, None);
            return self.resolve_error_call(node);
        }
        self.resolve_call(
            node,
            &call_signatures,
            candidates_out_array,
            check_mode,
            SignatureFlags::NONE,
            None,
        )
    }

    /// Go: `func (c *Checker) resolveDecorator(...) *Signature` (checker.go:8959).
    fn resolve_decorator(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        let parent = self.store().node(node).parent.get();
        if parent == NodeId::NONE || !can_have_decorators(self.store().node(parent)) {
            return self.resolve_error_call(node);
        }
        let expression = crate::ast_shims::expression_of(self.store(), node).unwrap_or(NodeId::NONE);
        let func_type = if expression != NodeId::NONE {
            self.check_expression(expression)
        } else {
            self.error_type()
        };
        let apparent_type = self.get_apparent_type(func_type);
        if self.is_error_type(apparent_type) {
            return self.resolve_error_call(node);
        }
        let call_signatures =
            self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Call);
        let num_construct_signatures =
            self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Construct).len();
        if self.is_untyped_function_call(
            func_type,
            apparent_type,
            call_signatures.len(),
            num_construct_signatures,
        ) {
            return self.resolve_untyped_call(node);
        }
        // Go's len(callSignatures) == 0 arm (checker.go:8985): the
        // invocationError chain — the S8-seam-era untyped-call composition
        // is retired (members are real). The decorator-wave surfaces
        // (isPotentiallyUncalledDecorator, getDecoratorCallSignature) stay
        // bannered until that wave.
        // TODO(port M6 decorator wave): isPotentiallyUncalledDecorator's
        // accepts-too-few-arguments error + getDecoratorCallSignature's
        // synthetic-argument applicability filter.
        let head_message = self.get_diagnostic_head_message_for_decorator_resolution(node);
        if call_signatures.is_empty() {
            let chain = self.invocation_error_details(node, apparent_type, crate::types::SignatureKind::Call);
            let diagnostic = match head_message {
                Some(head) => {
                    u::new_diagnostic_chain_for_node(Some(&chain), self.store(), node, head, vec![])
                }
                None => chain,
            };
            self.add_diagnostic(diagnostic);
            return self.resolve_error_call(node);
        }
        self.resolve_call(
            node,
            &call_signatures,
            candidates_out_array,
            check_mode,
            SignatureFlags::NONE,
            head_message,
        )
    }

    /// Go: `func (c *Checker) getDiagnosticHeadMessageForDecoratorResolution(node) *diagnostics.Message`
    /// (checker.go:8997).
    fn get_diagnostic_head_message_for_decorator_resolution(
        &self,
        node: NodeId,
    ) -> Option<&'static Message> {
        let parent = self.store().node(node).parent.get();
        if parent == NodeId::NONE {
            return None;
        }
        Some(match self.store().node(parent).kind {
            Kind::ClassDeclaration | Kind::ClassExpression => {
                &diag::UNABLE_TO_RESOLVE_SIGNATURE_OF_CLASS_DECORATOR_WHEN_CALLED_AS_AN_EXPRESSION
            }
            Kind::Parameter => {
                &diag::UNABLE_TO_RESOLVE_SIGNATURE_OF_PARAMETER_DECORATOR_WHEN_CALLED_AS_AN_EXPRESSION
            }
            Kind::PropertyDeclaration => {
                &diag::UNABLE_TO_RESOLVE_SIGNATURE_OF_PROPERTY_DECORATOR_WHEN_CALLED_AS_AN_EXPRESSION
            }
            Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor => {
                &diag::UNABLE_TO_RESOLVE_SIGNATURE_OF_METHOD_DECORATOR_WHEN_CALLED_AS_AN_EXPRESSION
            }
            _ => panic!(
                "Unhandled case in getDiagnosticHeadMessageForDecoratorResolution: {}",
                self.store().node(node).kind_string()
            ),
        })
    }

    /// Go: `func (c *Checker) resolveInstanceofExpression(...) *Signature`
    /// (checker.go:9013).
    fn resolve_instanceof_expression(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        // if rightType is an object type with a custom `[Symbol.hasInstance]`
        // method, then it is potentially valid on the right-hand side.
        let (left, right) = match &self.store().node(node).data {
            tsc_ast::NodeData::BinaryExpression(d) => (d.left, d.right),
            _ => return self.any_signature,
        };
        let right_type = self.check_expression(right);
        if !crate::utilities::is_type_any(Some(self.type_(right_type))) {
            // TODO(port M6 S8): getSymbolHasInstanceMethodOfObjectType rides
            // the structured-type members seam; None keeps the plain
            // Function-interface check below.
            let has_instance_method_type: Option<TypeId> = None;
            if let Some(has_instance_method_type) = has_instance_method_type {
                let apparent_type = self.get_apparent_type(has_instance_method_type);
                if self.is_error_type(apparent_type) {
                    return self.resolve_error_call(node);
                }
                let call_signatures =
                    self.get_signatures_of_type(apparent_type, crate::types::SignatureKind::Call);
                let construct_signatures = self
                    .get_signatures_of_type(apparent_type, crate::types::SignatureKind::Construct)
                    .len();
                if self.is_untyped_function_call(
                    has_instance_method_type,
                    apparent_type,
                    call_signatures.len(),
                    construct_signatures,
                ) {
                    return self.resolve_untyped_call(node);
                }
                if !call_signatures.is_empty() {
                    return self.resolve_call(
                        node,
                        &call_signatures,
                        candidates_out_array,
                        check_mode,
                        SignatureFlags::NONE,
                        None,
                    );
                }
            } else if !(self.type_has_call_or_construct_signatures(right_type)
                || match self.global_function_type {
                    Some(global_function_type) => {
                        self.is_type_subtype_of(right_type, global_function_type)
                    }
                    // TODO(port M6 S7): globalFunctionType seeds with the global
                    // Function interface; permissive false until then.
                    None => false,
                })
            {
                self.error(
                    right,
                    &diag::THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_BE_EITHER_OF_TYPE_ANY_A_CLASS_FUNCTION_OR_OTHER_TYPE_ASSIGNABLE_TO_THE_FUNCTION_INTERFACE_TYPE_OR_AN_OBJECT_TYPE_WITH_A_SYMBOL_HASINSTANCE_METHOD,
                    &[],
                );
                return self.resolve_error_call(node);
            }
        }
        let _ = left;
        // fall back to a default signature
        self.any_signature
    }

    // ────────────────────────────────────────────────────────────────────────
    // resolveCall + CallState (Go checker.go:9045–9183)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) resolveCall(node, signatures, candidatesOutArray, checkMode, callChainFlags, headMessage) *Signature`
    /// (checker.go:9055). PORT: CallState is a struct passed by reference in
    /// Go; the port keeps a `CallState` value and threads `&mut` through the
    /// chooseOverload ladder exactly like Go's captured closure state.
    #[allow(clippy::too_many_arguments)]
    fn resolve_call(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        mut candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
        call_chain_flags: SignatureFlags,
        head_message: Option<&'static Message>,
    ) -> Option<SignatureId> {
        let node_kind = self.store().node(node).kind;
        let is_tagged_template = node_kind == Kind::TaggedTemplateExpression;
        let is_decorator = node_kind == Kind::Decorator;
        let is_instanceof = node_kind == Kind::BinaryExpression;
        let is_jsx_opening = matches!(
            node_kind,
            Kind::JsxOpeningElement | Kind::JsxSelfClosingElement | Kind::JsxOpeningFragment
        );
        let report_errors =
            !self.is_inference_partially_blocked && candidates_out_array.is_none();
        let mut s = CallState {
            node,
            type_arguments: Vec::new(),
            args: Vec::new(),
            candidates: Vec::new(),
            arg_check_mode: CheckMode::CHECK_MODE_NORMAL,
            is_single_non_generic_candidate: false,
            signature_help_trailing_comma: false,
            recursive_resolution: false,
            candidates_for_argument_error: Vec::new(),
            candidate_for_argument_arity_error: None,
            candidate_for_type_argument_error: None,
        };
        if !is_decorator && !is_instanceof && !u::is_super_call(self.store(), node)
            && node_kind != Kind::JsxOpeningFragment
        {
            s.type_arguments = crate::statements::type_arguments_of(self.store(), node);
            // We already perform checking on the type arguments on the class
            // declaration itself.
            let expression = crate::ast_shims::expression_of(self.store(), node)
                .unwrap_or(NodeId::NONE);
            if is_tagged_template
                || is_jsx_opening
                || expression == NodeId::NONE
                || self.store().node(expression).kind != Kind::SuperKeyword
            {
                self.check_source_elements(&s.type_arguments);
            }
        }
        s.candidates = self.reorder_candidates(signatures, call_chain_flags);
        if let Some(out) = candidates_out_array.as_deref_mut() {
            *out = s.candidates.clone();
        }
        if s.candidates.is_empty() {
            // In Strada we would error here, but no known repro doesn't have
            // at least one other error in this codepath (#54442).
            return self.unknown_signature;
        }
        s.args = self.get_effective_call_arguments(node);
        // The excludeArgument concept: context-sensitive arguments are
        // excluded from the first assignability pass.
        s.is_single_non_generic_candidate = s.candidates.len() == 1
            && self.signatures[s.candidates[0].0 as usize].type_parameters.is_empty();
        if !is_decorator && !s.is_single_non_generic_candidate && s.args.iter().any(|&a| self.is_context_sensitive(a))
        {
            s.arg_check_mode = CheckMode::SKIP_CONTEXT_SENSITIVE;
        } else {
            s.arg_check_mode = CheckMode::CHECK_MODE_NORMAL;
        }
        // If we are in signature help, a trailing comma indicates that we
        // intend to provide another argument.
        s.signature_help_trailing_comma = check_mode.intersects(CheckMode::IS_FOR_SIGNATURE_HELP)
            && node_kind == Kind::CallExpression
            && self.call_argument_list_has_trailing_comma(node);
        // Section 4.12.1 — the subtype pass then the assignable pass.
        let mut result: Option<SignatureId> = None;
        s.recursive_resolution = self.call_resolution_stack.contains(&s.node);
        self.call_resolution_stack.push(s.node);
        if s.candidates.len() > 1 {
            result = self.choose_overload(&mut s, RelationKind::Subtype);
        }
        if result.is_none() {
            result = self.choose_overload(&mut s, RelationKind::Assignable);
        }
        self.call_resolution_stack.pop();
        if let Some(result) = result {
            return Some(result);
        }
        let result = self.get_candidate_for_overload_failure(
            s.node,
            &mut s.candidates,
            &s.args,
            candidates_out_array.is_some(),
            check_mode,
        );
        // Preemptively cache the result (see Go note about circularity).
        self.signature_node_links.get(node).resolved_signature = result;
        // No signatures were applicable. Now report errors based on the last
        // applicable signature with no arguments excluded from assignability
        // checks.
        if report_errors {
            let head_message = head_message.or({
                if is_instanceof {
                    Some(&diag::THE_LEFT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_BE_ASSIGNABLE_TO_THE_FIRST_ARGUMENT_OF_THE_RIGHT_HAND_SIDE_S_SYMBOL_HASINSTANCE_METHOD)
                } else {
                    None
                }
            });
            self.report_call_resolution_errors(node, &s, signatures, head_message);
        }
        result
    }

    /// Go: `func (c *Checker) reorderCandidates(signatures, callChainFlags) []*Signature`
    /// (checker.go:9185).
    fn reorder_candidates(
        &mut self,
        signatures: &[SignatureId],
        call_chain_flags: SignatureFlags,
    ) -> Vec<SignatureId> {
        let mut last_parent: Option<NodeId> = None;
        let mut last_symbol: Option<SymbolId> = None;
        let mut index: usize = 0;
        let mut cutoff_index: usize = 0;
        let mut splice_index: usize;
        let mut specialized_index: isize = -1;
        let mut result: Vec<SignatureId> = Vec::with_capacity(signatures.len());
        for &signature in signatures {
            let mut symbol: Option<SymbolId> = None;
            let mut parent: Option<NodeId> = None;
            let declaration = self.signatures[signature.0 as usize].declaration;
            if let Some(declaration) = declaration {
                symbol = crate::typeofsymbol::payload_symbol(self.store().node(declaration));
                parent = binder_shims::parent_of(self.store(), declaration);
            }
            if last_symbol.is_none() || symbol == last_symbol {
                if last_parent.is_some() && parent == last_parent {
                    index += 1;
                } else {
                    last_parent = parent;
                    index = cutoff_index;
                }
            } else {
                // current declaration belongs to a different symbol: set
                // cutoffIndex so re-orderings in the future won't change the
                // result set from 0 to cutoffIndex.
                index = result.len();
                cutoff_index = result.len();
                last_parent = parent;
            }
            last_symbol = symbol;
            // specialized signatures always need to be placed before
            // non-specialized signatures regardless of the cutoff position
            // (GH#1133).
            if signature_has_literal_types(self, signature) {
                specialized_index += 1;
                splice_index = specialized_index.max(0) as usize;
                // The cutoff index always needs to be greater than or equal to
                // the specialized signature index.
                cutoff_index += 1;
            } else {
                splice_index = index;
            }
            let signature = if !call_chain_flags.is_empty() {
                self.get_optional_call_signature(signature, call_chain_flags)
            } else {
                signature
            };
            result.insert(splice_index.min(result.len()), signature);
        }
        result
    }

    /// Go: `func (c *Checker) getOptionalCallSignature(signature, callChainFlags) *Signature`
    /// (checker.go:9234).
    fn get_optional_call_signature(
        &mut self,
        signature: SignatureId,
        call_chain_flags: SignatureFlags,
    ) -> SignatureId {
        let flags = self.signatures[signature.0 as usize].flags;
        if flags & SignatureFlags::CALL_CHAIN_FLAGS == call_chain_flags {
            return signature;
        }
        let key: u64 = if call_chain_flags.intersects(SignatureFlags::IS_INNER_CALL_CHAIN) {
            1
        } else {
            2
        };
        if let Some(&cached) = self.cached_signatures.get(&(signature, key)) {
            return cached;
        }
        // Go: cloneSignature — the S8 real body (members.rs) replaced this
        // local shallow copy (Go's clone masks flags with
        // SignatureFlagsPropagatingFlags; the call-chain flags ride on top).
        let result = self.clone_signature(signature);
        self.signatures[result.0 as usize].flags |= call_chain_flags;
        self.cached_signatures.insert((signature, key), result);
        result
    }

    /// Go: `func (c *Checker) chooseOverload(s *CallState, relation *Relation) *Signature`
    /// (checker.go:9250).
    fn choose_overload(&mut self, s: &mut CallState, relation: RelationKind) -> Option<SignatureId> {
        s.candidates_for_argument_error.clear();
        s.candidate_for_argument_arity_error = None;
        s.candidate_for_type_argument_error = None;
        if s.is_single_non_generic_candidate {
            let candidate = s.candidates[0];
            if !s.type_arguments.is_empty()
                || !self.has_correct_arity(s.node, &s.args, candidate, s.signature_help_trailing_comma)
            {
                return None;
            }
            if !self.is_signature_applicable(
                s.node,
                &s.args,
                candidate,
                relation,
                CheckMode::CHECK_MODE_NORMAL,
                false,
                None,
            ) {
                s.candidates_for_argument_error = vec![candidate];
                return None;
            }
            return Some(candidate);
        }
        for candidate_index in 0..s.candidates.len() {
            let candidate = s.candidates[candidate_index];
            if !self.has_correct_type_argument_arity(candidate, &s.type_arguments)
                || !self.has_correct_arity(s.node, &s.args, candidate, s.signature_help_trailing_comma)
            {
                continue;
            }
            let check_candidate;
            let mut inference_context = None;
            let candidate_type_parameters =
                self.signatures[candidate.0 as usize].type_parameters.clone();
            if !candidate_type_parameters.is_empty() {
                let type_argument_types: Option<Vec<TypeId>>;
                if !s.type_arguments.is_empty() {
                    let checked =
                        self.check_type_arguments(candidate, &s.type_arguments, false, None);
                    match checked {
                        None => {
                            s.candidate_for_type_argument_error = Some(candidate);
                            continue;
                        }
                        Some(types) => type_argument_types = Some(types),
                    }
                } else {
                    // When we are recursively resolving a call with a single
                    // candidate, we skip constraint checks during type
                    // inference to avoid circularity errors (#64192).
                    let mut inference_flags = crate::inference::InferenceFlags::NONE;
                    if s.recursive_resolution && s.candidates.len() == 1 {
                        inference_flags |= crate::inference::InferenceFlags::NO_CONSTRAINT_CHECKS;
                    }
                    // TODO(port M6 W1/js): ast.IsInJSFile(node) adds
                    // InferenceFlagsAnyDefault; the JS file probe rides the
                    // W1/js seam.
                    let context = self.new_inference_context(
                        &candidate_type_parameters,
                        Some(candidate),
                        inference_flags,
                        crate::inference::InferenceComparer::Assignable,
                    );
                    let types = self.infer_type_arguments(
                        s.node,
                        candidate,
                        &s.args,
                        s.arg_check_mode | CheckMode::SKIP_GENERIC_FUNCTIONS,
                        context,
                    );
                    type_argument_types = Some(types);
                    inference_context = Some(context);
                }
                let type_argument_types = type_argument_types.unwrap_or_default();
                let inferred_type_parameters = inference_context
                    .map(|context| {
                        let count = self.inference_contexts[context.0 as usize].inferences.len();
                        (0..count)
                            .map(|i| {
                                self.get_inferred_type(
                                    context,
                                    i,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                check_candidate = self.get_signature_instantiation(
                    candidate,
                    &type_argument_types,
                    false,
                    &inferred_type_parameters,
                );
                // If the original signature has a generic rest type,
                // instantiation may produce a signature with different arity
                // and we need to perform another arity check.
                if self.get_non_array_rest_type(candidate).is_some()
                    && !self.has_correct_arity(
                        s.node,
                        &s.args,
                        check_candidate,
                        s.signature_help_trailing_comma,
                    )
                {
                    s.candidate_for_argument_arity_error = Some(check_candidate);
                    continue;
                }
            } else {
                check_candidate = candidate;
            }
            if !self.is_signature_applicable(
                s.node,
                &s.args,
                check_candidate,
                relation,
                s.arg_check_mode,
                false,
                None,
            ) {
                // Give preference to error candidates that have no rest
                // parameters (as they are more specific).
                s.candidates_for_argument_error.push(check_candidate);
                continue;
            }
            if !s.arg_check_mode.is_empty() {
                // If one or more context sensitive arguments were excluded, we
                // start including them now and perform a second round of type
                // inference and applicability checking for this candidate.
                s.arg_check_mode = CheckMode::CHECK_MODE_NORMAL;
                let mut final_candidate = check_candidate;
                if let Some(context) = inference_context {
                    let type_argument_types =
                        self.infer_type_arguments(s.node, candidate, &s.args, s.arg_check_mode, context);
                    let count = self.inference_contexts[context.0 as usize].inferences.len();
                    let inferred: Vec<TypeId> = (0..count)
                        .map(|i| self.get_inferred_type(context, i))
                        .collect();
                    final_candidate =
                        self.get_signature_instantiation(candidate, &type_argument_types, false, &inferred);
                    // Generic-rest arity re-check.
                    if self.get_non_array_rest_type(candidate).is_some()
                        && !self.has_correct_arity(
                            s.node,
                            &s.args,
                            final_candidate,
                            s.signature_help_trailing_comma,
                        )
                    {
                        s.candidate_for_argument_arity_error = Some(final_candidate);
                        continue;
                    }
                }
                if !self.is_signature_applicable(
                    s.node,
                    &s.args,
                    final_candidate,
                    relation,
                    s.arg_check_mode,
                    false,
                    None,
                ) {
                    s.candidates_for_argument_error.push(final_candidate);
                    continue;
                }
                s.candidates[candidate_index] = final_candidate;
                return Some(final_candidate);
            }
            s.candidates[candidate_index] = check_candidate;
            return Some(check_candidate);
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // The arity ladders (Go checker.go:9324–9489)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) hasCorrectArity(node, args, signature, signatureHelpTrailingComma) bool`
    /// (checker.go:9324).
    fn has_correct_arity(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
        signature_help_trailing_comma: bool,
    ) -> bool {
        if self.store().node(node).kind == Kind::JsxOpeningFragment {
            return true;
        }
        let node_kind = self.store().node(node).kind;
        let arg_count: usize;
        let mut call_is_incomplete = false;
        let mut effective_parameter_count = self.get_parameter_count(signature);
        let mut effective_minimum_arguments = self.get_min_argument_count(signature);
        match node_kind {
            Kind::TaggedTemplateExpression => {
                arg_count = args.len();
                // TODO(port M6 S4): the unterminated-literal probes read the
                // scanner's literal flags; permissive false keeps the
                // incomplete-call relaxation dormant.
                call_is_incomplete = false;
            }
            Kind::Decorator => {
                arg_count = self.get_decorator_argument_count(node, signature) as usize;
            }
            Kind::BinaryExpression => {
                arg_count = 1;
            }
            Kind::JsxOpeningElement | Kind::JsxSelfClosingElement => {
                // TODO(port M6 jsx wave): the attributes-end probe; the jsx
                // arm rides the jsx wave.
                arg_count = if effective_minimum_arguments == 0 {
                    args.len()
                } else {
                    1
                };
                if !args.is_empty() {
                    effective_parameter_count = 1;
                }
                effective_minimum_arguments = effective_minimum_arguments.min(1);
            }
            Kind::NewExpression
                if self
                    .store()
                    .node(node)
                    .as_new_expression()
                    .is_none_or(|d| d.arguments.is_none()) =>
            {
                // This only happens when we have something of the form: 'new C'
                return self.get_min_argument_count(signature) == 0;
            }
            _ => {
                arg_count = if signature_help_trailing_comma {
                    args.len() + 1
                } else {
                    args.len()
                };
                // If we are missing the close parenthesis, the call is
                // incomplete.
                // TODO(port M6 S4): node.ArgumentList().End() == node.End()
                // reads the parser's missing-paren flags; permissive false.
                call_is_incomplete = false;
                // If a spread argument is present, check that it corresponds
                // to a rest parameter or at least that it's in the valid range.
                let spread_arg_index = get_spread_argument_index(self.store(), args);
                if spread_arg_index >= 0 {
                    return spread_arg_index >= self.get_min_argument_count(signature)
                        && (self.has_effective_rest_parameter(signature)
                            || spread_arg_index < self.get_parameter_count(signature));
                }
            }
        }
        // Too many arguments implies incorrect arity.
        if !self.has_effective_rest_parameter(signature)
            && arg_count > effective_parameter_count as usize
        {
            return false;
        }
        // If the call is incomplete, we should skip the lower bound check.
        if call_is_incomplete || arg_count >= effective_minimum_arguments as usize {
            return true;
        }
        for i in arg_count..effective_minimum_arguments as usize {
            let t = self.get_type_at_position(signature, i);
            // PORT: Go filters the parameter type for acceptsVoid and checks
            // the never flag; the void-parameter filter is the same any-typed
            // filter family (utilities.go everyType).
            if self.type_(t).flags.intersects(TypeFlags::VOID) {
                continue;
            }
            // Go: `c.filterType(t, acceptsVoid).flags&TypeFlagsNever != 0` —
            // a non-void parameter with a never-filtered remainder means the
            // arity check fails. TODO(port M6 S7): the filterType union walk;
            // the flags probe below composes the never answer for the
            // non-union case (Go's everyType(t, acceptsVoid) keeps a
            // constituent only when void — a never result).
            if self.type_(t).flags.intersects(TypeFlags::NEVER) {
                return false;
            }
        }
        true
    }

    /// Go: `func (c *Checker) getDecoratorArgumentCount(node, signature) int`
    /// (checker.go:9408).
    fn get_decorator_argument_count(&mut self, node: NodeId, signature: SignatureId) -> i32 {
        if self.legacy_decorators {
            return self.get_legacy_decorator_argument_count(node, signature);
        }
        self.get_parameter_count(signature).clamp(1, 2)
    }

    /// Go: `func (c *Checker) getLegacyDecoratorArgumentCount(node, signature) int`
    /// (checker.go:9418).
    fn get_legacy_decorator_argument_count(&mut self, node: NodeId, signature: SignatureId) -> i32 {
        let parent = self.store().node(node).parent.get();
        if parent == NodeId::NONE {
            panic!(
                "Unhandled case in getLegacyDecoratorArgumentCount: {}",
                self.store().node(node).kind_string()
            );
        }
        match self.store().node(parent).kind {
            Kind::ClassDeclaration | Kind::ClassExpression => 1,
            Kind::PropertyDeclaration => {
                if binder_shims::has_accessor_modifier(self.store().node(parent)) {
                    3
                } else {
                    2
                }
            }
            Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor => {
                // For decorators with only two parameters we supply only two
                // arguments.
                if self.get_parameter_count(signature) <= 2 {
                    2
                } else {
                    3
                }
            }
            Kind::Parameter => 3,
            _ => panic!(
                "Unhandled case in getLegacyDecoratorArgumentCount: {}",
                self.store().node(parent).kind_string()
            ),
        }
    }

    /// Go: `func (c *Checker) hasCorrectTypeArgumentArity(signature, typeArguments) bool`
    /// (checker.go:9445).
    fn has_correct_type_argument_arity(
        &mut self,
        signature: SignatureId,
        type_arguments: &[NodeId],
    ) -> bool {
        // If the user supplied type arguments, but the number of type
        // arguments does not match the declared number of type parameters,
        // the call has an incorrect arity.
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        let num_type_parameters = type_parameters.len();
        let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
        type_arguments.is_empty()
            || type_arguments.len() >= min_type_argument_count
                && type_arguments.len() <= num_type_parameters
    }

    /// Go: `func (c *Checker) checkTypeArguments(signature, typeArgumentNodes, reportErrors, headMessage) []*Type`
    /// (checker.go:9458).
    #[allow(clippy::too_many_arguments)]
    fn check_type_arguments(
        &mut self,
        signature: SignatureId,
        type_argument_nodes: &[NodeId],
        report_errors: bool,
        head_message: Option<&'static Message>,
    ) -> Option<Vec<TypeId>> {
        // TODO(port M6 W1/js): IsInJSFile(signature.declaration) adds the
        // JS fill behavior; the JS file probe rides the W1/js seam.
        let is_java_script = false;
        let type_parameters = self.signatures[signature.0 as usize].type_parameters.clone();
        let mut type_argument_types: Vec<TypeId> = type_argument_nodes
            .iter()
            .map(|&n| self.get_type_from_type_node(n))
            .collect();
        let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
        type_argument_types = self.fill_missing_type_arguments(
            &type_argument_types,
            &type_parameters,
            min_type_argument_count,
            is_java_script,
        );
        let mut mapper: Option<crate::todo_shapes::TypeMapperId> = None;
        for i in 0..type_argument_nodes.len() {
            debug_assert!(
                i < type_parameters.len(),
                "Should not call checkTypeArguments with too many type arguments"
            );
            if i >= type_parameters.len() {
                break;
            }
            let constraint = self.get_constraint_of_type_parameter(type_parameters[i]);
            if let Some(constraint) = constraint {
                if mapper.is_none() {
                    mapper = Some(self.new_type_mapper(&type_parameters, &type_argument_types));
                }
                let mapper = mapper.unwrap();
                let type_argument_head_message = head_message
                    .unwrap_or(&diag::TYPE_0_DOES_NOT_SATISFY_THE_CONSTRAINT_1);
                let type_argument = type_argument_types[i];
                let error_node = if report_errors {
                    Some(type_argument_nodes[i])
                } else {
                    None
                };
                let instantiated_constraint = self.instantiate_type(constraint, mapper);
                // TODO(port M6 S9): getTypeWithThisArgument (the this-side
                // substitution of the constraint check) is the contextual
                // family's; the plain instantiation composes the check.
                let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
                if !self.check_type_assignable_to_ex(
                    type_argument,
                    instantiated_constraint,
                    error_node,
                    Some(type_argument_head_message),
                    Some(&mut diags),
                ) {
                    if let Some(diagnostic) = diags.into_iter().next() {
                        self.add_diagnostic(diagnostic);
                    }
                    return None;
                }
            }
        }
        Some(type_argument_types)
    }

    // ────────────────────────────────────────────────────────────────────────
    // isSignatureApplicable + inferTypeArguments (Go checker.go:9490–9732)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) isSignatureApplicable(node, args, signature, relation, checkMode, reportErrors, diagnosticOutput) bool`
    /// (checker.go:9490).
    #[allow(clippy::too_many_arguments)]
    fn is_signature_applicable(
        &mut self,
        node: NodeId,
        args: &[NodeId],
        signature: SignatureId,
        relation: RelationKind,
        check_mode: CheckMode,
        report_errors: bool,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        // TODO(port M6 jsx wave): checkApplicableSignatureForJsxCallLikeElement.
        let this_type = self
            .get_this_type_of_signature(signature)
            .filter(|&t| t != self.void_type());
        if let Some(this_type) = this_type
            && !(self.store().node(node).kind == Kind::NewExpression
                || self.store().node(node).kind == Kind::CallExpression
                    && is_super_property(
                        self.store(),
                        crate::ast_shims::expression_of(self.store(), node)
                            .unwrap_or(NodeId::NONE),
                    ))
        {
            let this_argument_node = self.get_this_arg_of_call(node);
            let this_argument_type = self.get_this_arg_type(this_argument_node);
            let error_node = if report_errors {
                this_argument_node.or(Some(node))
            } else {
                None
            };
            let head_message = &diag::THE_THIS_CONTEXT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_METHOD_S_THIS_OF_TYPE_1;
            if !self.check_type_related_to_ex(
                this_argument_type,
                this_type,
                relation,
                error_node,
                Some(head_message),
                diagnostic_output.as_deref_mut(),
            ) {
                return false;
            }
        }
        let head_message = &diag::ARGUMENT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_PARAMETER_OF_TYPE_1;
        let rest_type = self.get_non_array_rest_type(signature);
        let arg_count = if rest_type.is_some() {
            (self.get_parameter_count(signature) - 1).max(0) as usize
        } else {
            args.len()
        };
        let arg_count = arg_count.min(args.len());
        for (i, &arg) in args.iter().enumerate().take(arg_count) {
            if self.store().node(arg).kind != Kind::OmittedExpression {
                let param_type = self.get_type_at_position(signature, i);
                let arg_type =
                    self.check_expression_with_contextual_type(arg, Some(param_type), None, check_mode);
                // If one or more arguments are still excluded (as indicated
                // by CheckMode.SkipContextSensitive), we obtain the regular
                // type of any object literal arguments.
                let check_arg_type = if check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE) {
                    crate::relater::get_regular_type_of_object_literal(self, arg_type)
                } else {
                    arg_type
                };
                let effective_check_argument_node = self.get_effective_check_node(arg);
                let error_node = if report_errors {
                    Some(effective_check_argument_node)
                } else {
                    None
                };
                // Go passes `diagnosticOutput` straight through here (no
                // local buffer): the failed assignability's 2345 chain-head
                // and related info land in the CALLER's output even though
                // this returns false — the W3T7 fix for the missing 2345 on
                // classAbstractFactoryFunction (the port's local `diags` was
                // dropped on the failure path).
                let applicable = self.check_type_related_to_and_optionally_elaborate(
                    check_arg_type,
                    param_type,
                    relation,
                    error_node,
                    Some(effective_check_argument_node),
                    Some(head_message),
                    diagnostic_output.as_deref_mut(),
                );
                if !applicable {
                    self.maybe_add_missing_await_info(
                        Some(effective_check_argument_node),
                        check_arg_type,
                        param_type,
                        relation,
                        report_errors,
                        diagnostic_output.as_deref_mut(),
                    );
                    return false;
                }
            }
        }
        if let Some(rest_type) = rest_type {
            if arg_count < args.len() {
                // TODO(port M6 S8): getSpreadArgumentType builds the
                // tuple/union rest argument type; the seam composes the rest
                // type itself, which the assignability check below compares
                // structurally (the tuple expansion lands with S8's
                // structured members).
                let spread_type = rest_type;
                let rest_arg_count = args.len() - arg_count;
                let error_node = if report_errors {
                    match rest_arg_count {
                        0 => Some(node),
                        1 => Some(self.get_effective_check_node(args[arg_count])),
                        _ => {
                            // Go synthesizes a spread node spanning the
                            // arguments; the first argument carries the error.
                            Some(args[arg_count])
                        }
                    }
                } else {
                    None
                };
                // Go: the rest-type spread check — diagnosticOutput passes
                // straight through (same W3T7 fix as the argument loop).
                // This is the final use, but as_deref_mut keeps the
                // call-shape uniform with the loop's reborrows.
                #[allow(clippy::needless_option_as_deref)]
                if !self.check_type_related_to_ex(
                    spread_type,
                    rest_type,
                    relation,
                    error_node,
                    Some(head_message),
                    diagnostic_output.as_deref_mut(),
                ) {
                    return false;
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) maybeAddMissingAwaitInfo(...)`.
    fn maybe_add_missing_await_info(
        &mut self,
        error_node: Option<NodeId>,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        report_errors: bool,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) {
        if error_node.is_none() || !report_errors || diagnostic_output.is_none() {
            return;
        }
        // Bail if target is Promise-like---something else is wrong
        // TODO(port M6 S7): getAwaitedTypeOfPromise is the S7 promise
        // machinery; permissive None keeps the await hint dormant.
        let _ = (source, target, relation);
    }

    /// Go: `func (c *Checker) getThisArgOfCall(node) *ast.Node` (checker.go:9572).
    fn get_this_arg_of_call(&mut self, node: NodeId) -> Option<NodeId> {
        if self.store().node(node).kind == Kind::BinaryExpression {
            return match &self.store().node(node).data {
                tsc_ast::NodeData::BinaryExpression(d) => Some(d.right),
                _ => None,
            };
        }
        let expression = match self.store().node(node).kind {
            Kind::CallExpression => crate::ast_shims::expression_of(self.store(), node),
            Kind::TaggedTemplateExpression => match &self.store().node(node).data {
                tsc_ast::NodeData::TaggedTemplateExpression(d) => Some(d.tag),
                _ => None,
            },
            Kind::Decorator if !self.legacy_decorators => {
                crate::ast_shims::expression_of(self.store(), node)
            }
            _ => None,
        };
        if let Some(expression) = expression {
            let callee = self.skip_outer_expressions_all(expression);
            if tsc_ast::utilities::is_access_expression(self.store().node(callee)) {
                return match &self.store().node(callee).data {
                    tsc_ast::NodeData::PropertyAccessExpression(p) => Some(p.expression),
                    tsc_ast::NodeData::ElementAccessExpression(e) => Some(e.expression),
                    _ => None,
                };
            }
        }
        None
    }

    /// Go: `func (c *Checker) getThisArgType(node) *Type` (checker.go:9591).
    fn get_this_arg_type(&mut self, node: Option<NodeId>) -> TypeId {
        let Some(node) = node else {
            return self.void_type();
        };
        let this_argument_type = self.check_expression(node);
        let parent = self.store().node(node).parent.get();
        if parent != NodeId::NONE {
            if binder_shims::is_optional_chain_root(self.store(), parent) {
                return self.get_non_nullable_type(this_argument_type);
            }
            if binder_shims::is_optional_chain(self.store().node(parent)) {
                return self.remove_optional_type_marker(this_argument_type);
            }
        }
        this_argument_type
    }

    /// Go: `func (c *Checker) inferTypeArguments(node, signature, args, checkMode, context) []*Type`
    /// (checker.go:9622).
    fn infer_type_arguments(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        args: &[NodeId],
        check_mode: CheckMode,
        context: crate::todo_shapes::InferenceContextId,
    ) -> Vec<TypeId> {
        // TODO(port M6 S9): the contextual-return inference half (getContextualType
        // → inferTypes against the signature's return type + the return
        // mapper) rides the S9 contextual-typing engine; the seam answers
        // None so the arm is dormant.
        let _ = (node, check_mode);
        let rest_type = self.get_non_array_rest_type(signature);
        let mut arg_count = args.len();
        if rest_type.is_some() {
            arg_count = ((self.get_parameter_count(signature) - 1).max(0) as usize).min(arg_count);
        }
        if let Some(rest_type) = rest_type.filter(|&t| {
            self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
        }) {
            // The impliedArity probe: if no spread argument follows, the rest
            // type parameter's arity is the remaining argument count.
            let has_spread = args[arg_count.min(args.len())..]
                .iter()
                .any(|&a| self.store().node(a).kind == Kind::SpreadElement);
            if !has_spread {
                self.record_implied_arity(context, rest_type, (args.len() - arg_count) as i32);
            }
        }
        let this_type = self.get_this_type_of_signature(signature);
        if let Some(this_type) = this_type.filter(|&t| self.could_contain_type_variables(t)) {
            let this_argument_node = self.get_this_arg_of_call(node);
            let this_argument_type = self.get_this_arg_type(this_argument_node);
            self.infer_types(
                context,
                this_argument_type,
                this_type,
                crate::inference::InferencePriority::NONE,
                false,
            );
        }
        for (i, arg) in args.iter().enumerate().take(arg_count.min(args.len())) {
            let arg = *arg;
            if self.store().node(arg).kind != Kind::OmittedExpression {
                let param_type = self.get_type_at_position(signature, i);
                if self.could_contain_type_variables(param_type) {
                    let arg_type = self.check_expression_with_contextual_type(
                        arg,
                        Some(param_type),
                        Some(context),
                        check_mode,
                    );
                    self.infer_types(
                        context,
                        arg_type,
                        param_type,
                        crate::inference::InferencePriority::NONE,
                        false,
                    );
                }
            }
        }
        if let Some(rest_type) = rest_type.filter(|&t| self.could_contain_type_variables(t)) {
            // TODO(port M6 S8): getSpreadArgumentType builds the rest
            // argument tuple; the seam composes the rest type itself.
            if arg_count < args.len() {
                self.infer_types(
                    context,
                    rest_type,
                    rest_type,
                    crate::inference::InferencePriority::NONE,
                    false,
                );
            }
        }
        self.get_inferred_types(context)
    }

    /// The impliedArity write inferTypeArguments performs on the rest type
    /// parameter's InferenceInfo (Go: `info.impliedArity = len(args)-argCount`).
    fn record_implied_arity(
        &mut self,
        context: crate::todo_shapes::InferenceContextId,
        type_parameter: TypeId,
        arity: i32,
    ) {
        let count = self.inference_contexts[context.0 as usize].inferences.len();
        for i in 0..count {
            let candidate = self.inference_contexts[context.0 as usize].inferences[i]
                .type_parameter;
            if candidate == type_parameter {
                self.inference_contexts[context.0 as usize].inferences[i].implied_arity = arity;
                return;
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Overload failure (Go checker.go:9734–9990)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getCandidateForOverloadFailure(node, candidates, args, hasCandidatesOutArray, checkMode) *Signature`
    /// (checker.go:9735).
    fn get_candidate_for_overload_failure(
        &mut self,
        node: NodeId,
        candidates: &mut [SignatureId],
        args: &[NodeId],
        has_candidates_out_array: bool,
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        // Else should not have called this.
        self.check_node_deferred(node);
        // Normally we will combine overloads. Skip this if they have type
        // parameters since that's hard to combine. Don't do this if there is
        // a candidatesOutArray (we want the chosen best candidate to be one
        // of the overloads, not a combination).
        // TODO(port M6 S7): createUnionOfSignaturesForOverloadFailure needs
        // the newSignature family (thisParameters/combined symbols + the
        // union/intersection construction); the seam picks the longest
        // candidate until then.
        let _ = has_candidates_out_array;
        self.pick_longest_candidate_signature(node, candidates, args, check_mode)
    }

    /// Go: `func (c *Checker) pickLongestCandidateSignature(node, candidates, args, checkMode) *Signature`
    /// (checker.go:9756).
    fn pick_longest_candidate_signature(
        &mut self,
        node: NodeId,
        candidates: &mut [SignatureId],
        args: &[NodeId],
        check_mode: CheckMode,
    ) -> Option<SignatureId> {
        // Pick the longest signature. This way we can get a contextual type
        // for cases like `f({ |`.
        let mut arg_count = args.len() as i32;
        if let Some(apparent) = self.apparent_argument_count {
            arg_count = apparent;
        }
        let best_index = self.get_longest_candidate_index(candidates, arg_count);
        let Some(best_index) = best_index else {
            return candidates.first().copied();
        };
        let candidate = candidates[best_index];
        let type_parameters = self.signatures[candidate.0 as usize].type_parameters.clone();
        if type_parameters.is_empty() {
            return Some(candidate);
        }
        let type_argument_nodes = if call_like_expression_may_have_type_arguments(self.store(), node) {
            crate::statements::type_arguments_of(self.store(), node)
        } else {
            Vec::new()
        };
        
        let instantiated = if !type_argument_nodes.is_empty() {
            let type_arguments =
                self.get_type_arguments_from_nodes(&type_argument_nodes, &type_parameters);
            self.create_signature_instantiation(candidate, &type_arguments)
        } else {
            self.infer_signature_instantiation_for_overload_failure(
                node,
                &type_parameters,
                candidate,
                args,
                check_mode,
            )
        };
        candidates[best_index] = instantiated;
        Some(instantiated)
    }

    /// Go: `func (c *Checker) getLongestCandidateIndex(candidates, argsCount) int`
    /// (checker.go:9790).
    fn get_longest_candidate_index(
        &mut self,
        candidates: &[SignatureId],
        args_count: i32,
    ) -> Option<usize> {
        let mut max_params_index: Option<usize> = None;
        let mut max_params: i32 = -1;
        for (i, &candidate) in candidates.iter().enumerate() {
            let param_count = self.get_parameter_count(candidate);
            if self.has_effective_rest_parameter(candidate) || param_count >= args_count {
                return Some(i);
            }
            if param_count > max_params {
                max_params = param_count;
                max_params_index = Some(i);
            }
        }
        max_params_index
    }

    /// Go: `func (c *Checker) getTypeArgumentsFromNodes(typeArgumentNodes, typeParameters) []*Type`
    /// (checker.go:9806).
    fn get_type_arguments_from_nodes(
        &mut self,
        type_argument_nodes: &[NodeId],
        type_parameters: &[TypeId],
    ) -> Vec<TypeId> {
        let mut nodes: Vec<NodeId> = type_argument_nodes
            .iter()
            .take(type_parameters.len())
            .copied()
            .collect();
        let mut type_arguments: Vec<TypeId> = nodes
            .iter()
            .map(|&n| self.get_type_from_type_node(n))
            .collect();
        while type_arguments.len() < type_parameters.len() {
            let mut t = self.get_default_from_type_parameter(type_parameters[type_arguments.len()]);
            if t.is_none() {
                t = self.get_constraint_of_type_parameter(type_parameters[type_arguments.len()]);
                if t.is_none() {
                    t = Some(self.unknown_type());
                }
            }
            type_arguments.push(t.unwrap_or_else(|| self.unknown_type()));
        }
        let _ = &mut nodes;
        nodes.clear();
        type_arguments
    }

    /// Go: `func (c *Checker) inferSignatureInstantiationForOverloadFailure(node, typeParameters, candidate, args, checkMode) *Signature`
    /// (checker.go:9832).
    fn infer_signature_instantiation_for_overload_failure(
        &mut self,
        node: NodeId,
        type_parameters: &[TypeId],
        candidate: SignatureId,
        args: &[NodeId],
        check_mode: CheckMode,
    ) -> SignatureId {
        // TODO(port M6 W1/js): InferenceFlagsAnyDefault for JS files.
        let inference_context = self.new_inference_context(
            type_parameters,
            Some(candidate),
            crate::inference::InferenceFlags::NONE,
            crate::inference::InferenceComparer::Assignable,
        );
        let type_argument_types = self.infer_type_arguments(
            node,
            candidate,
            args,
            check_mode
                | CheckMode::SKIP_CONTEXT_SENSITIVE
                | CheckMode::SKIP_GENERIC_FUNCTIONS,
            inference_context,
        );
        self.create_signature_instantiation(candidate, &type_argument_types)
    }

    /// Go: `func (c *Checker) reportCallResolutionErrors(node, s, signatures, headMessage)`
    /// (checker.go:9894). PORT: the diagnostic-chain/related-info
    /// elaborations ride the diagnostic-enrichment surface; the head
    /// messages fire through the plain error path.
    fn report_call_resolution_errors(
        &mut self,
        node: NodeId,
        s: &CallState,
        signatures: &[SignatureId],
        head_message: Option<&'static Message>,
    ) {
        let _ = head_message;
        if !s.candidates_for_argument_error.is_empty() {
            let last = s.candidates_for_argument_error[s.candidates_for_argument_error.len() - 1];
            let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
            self.is_signature_applicable(
                s.node,
                &s.args,
                last,
                RelationKind::Assignable,
                CheckMode::CHECK_MODE_NORMAL,
                true,
                Some(&mut diags),
            );
            for diagnostic in diags {
                self.add_diagnostic(diagnostic);
            }
        } else if let Some(candidate) = s.candidate_for_argument_arity_error {
            let diagnostic = self.get_argument_arity_error(node, &[candidate], &s.args);
            self.add_diagnostic(diagnostic);
        } else if let Some(candidate) = s.candidate_for_type_argument_error {
            let _ = self.check_type_arguments(
                candidate,
                &crate::statements::type_arguments_of(self.store(), node),
                true,
                head_message,
            );
        } else if self.store().node(node).kind != Kind::JsxOpeningFragment {
            let signatures_with_correct_type_argument_arity: Vec<SignatureId> = signatures
                .iter()
                .copied()
                .filter(|&sig| self.has_correct_type_argument_arity(sig, &s.type_arguments))
                .collect();
            if signatures_with_correct_type_argument_arity.is_empty() {
                let diagnostic =
                    self.get_type_argument_arity_error(node, signatures, &s.type_arguments);
                self.add_diagnostic(diagnostic);
            } else {
                let diagnostic =
                    self.get_argument_arity_error(node, &signatures_with_correct_type_argument_arity, &s.args);
                self.add_diagnostic(diagnostic);
            }
        }
    }

    /// Go: `func (c *Checker) getArgumentArityError(node, signatures, args, headMessage) *ast.Diagnostic`
    /// (checker.go:9935). PORT: the related-info and position refinement ride
    /// the diagnostic-enrichment surface; the parameter-range messages are
    /// the real Go ladder.
    #[allow(clippy::too_many_arguments)]
    fn get_argument_arity_error(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        args: &[NodeId],
    ) -> tsc_ast::Diagnostic {
        let spread_index = get_spread_argument_index(self.store(), args);
        if spread_index > -1 {
            return u::new_diagnostic_for_node(
                self.store(),
                args[spread_index as usize],
                &diag::A_SPREAD_ARGUMENT_MUST_EITHER_HAVE_A_TUPLE_TYPE_OR_BE_PASSED_TO_A_REST_PARAMETER,
                Vec::new(),
            );
        }
        let mut min_count = i32::MAX; // smallest parameter count
        let mut max_count = i32::MIN; // largest parameter count
        let mut max_below = i32::MIN; // largest parameter count that is smaller than the number of arguments
        let mut min_above = i32::MAX; // smallest parameter count that is larger than the number of arguments
        let mut closest_signature: Option<SignatureId> = None;
        for &sig in signatures {
            let min_parameter = self.get_min_argument_count(sig);
            let max_parameter = self.get_parameter_count(sig);
            if min_parameter < min_count {
                min_count = min_parameter;
                closest_signature = Some(sig);
            }
            max_count = max_count.max(max_parameter);
            if min_parameter < args.len() as i32 && min_parameter > max_below {
                max_below = min_parameter;
            }
            if (args.len() as i32) < max_parameter && max_parameter < min_above {
                min_above = max_parameter;
            }
        }
        let has_rest_parameter = signatures
            .iter()
            .any(|&sig| self.has_effective_rest_parameter(sig));
        let parameter_range = if has_rest_parameter {
            min_count.to_string()
        } else if min_count < max_count {
            format!("{}-{}", min_count, max_count)
        } else {
            min_count.to_string()
        };
        // TODO(port M6 S7): isPromiseResolveArityError (the void-Promise
        // hint) reads the global Promise constructor symbol; permissive skip.
        let error_node = get_error_node_for_call_node(self.store(), node);
        let message: &'static Message = if self.store().node(node).kind == Kind::Decorator {
            if has_rest_parameter {
                &diag::THE_RUNTIME_WILL_INVOKE_THE_DECORATOR_WITH_1_ARGUMENTS_BUT_THE_DECORATOR_EXPECTS_AT_LEAST_0
            } else {
                &diag::THE_RUNTIME_WILL_INVOKE_THE_DECORATOR_WITH_1_ARGUMENTS_BUT_THE_DECORATOR_EXPECTS_0
            }
        } else if has_rest_parameter {
            &diag::EXPECTED_AT_LEAST_0_ARGUMENTS_BUT_GOT_1
        } else {
            &diag::EXPECTED_0_ARGUMENTS_BUT_GOT_1
        };
        let _ = closest_signature;
        if min_count < args.len() as i32 && (args.len() as i32) < max_count {
            // between min and max, but with no matching overload
            return u::new_diagnostic_for_node(
                self.store(),
                error_node,
                &diag::NO_OVERLOAD_EXPECTS_0_ARGUMENTS_BUT_OVERLOADS_DO_EXIST_THAT_EXPECT_EITHER_1_OR_2_ARGUMENTS,
                vec![
                    args.len().to_string(),
                    max_below.to_string(),
                    min_above.to_string(),
                ],
            );
        }
        if (args.len() as i32) < min_count {
            // too short: put the error span on the call expression, not any
            // of the args
            return u::new_diagnostic_for_node(
                self.store(),
                error_node,
                message,
                vec![parameter_range, args.len().to_string()],
            );
        }
        // Guard against out-of-bounds access when maxCount >= len(args).
        if max_count >= args.len() as i32 {
            return u::new_diagnostic_for_node(
                self.store(),
                error_node,
                message,
                vec![parameter_range, args.len().to_string()],
            );
        }
        // Go's too-many case: the span is the FIRST EXCESS argument through
        // the last argument, SkipTrivia'd (ES5SymbolProperty5's 2554 lands
        // on the `(0)` argument, not the call node).
        let file = self.store().file(node.file_id());
        let text = file.text();
        let start = args[max_count as usize];
        let pos = self.store().node(start).pos() as usize;
        let mut end = self.store().node(args[args.len() - 1]).end() as usize;
        if end == pos {
            end += 1;
        }
        let pos = tsc_scanner::skip_trivia(text, pos as i32) as usize;
        if end < pos {
            end = pos;
        }
        tsc_ast::Diagnostic::new(
            Some(file.as_node_id()),
            tsc_ast::TextRange::new(pos as i32, end as i32),            message.code(),
            message.category(),
            "",
            Some(message),
            "",
            message.key(),
            vec![parameter_range, args.len().to_string()],
        )
    }

    /// Go: `func (c *Checker) getTypeArgumentArityError(node, signatures, typeArguments, headMessage) *ast.Diagnostic`
    /// (checker.go:10066). PORT: the position spans the call node (Go trims
    /// trivia around the type-argument list).
    fn get_type_argument_arity_error(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        type_arguments: &[NodeId],
    ) -> tsc_ast::Diagnostic {
        let arg_count = type_arguments.len();
        if signatures.len() == 1 {
            // No overloads exist
            let sig = signatures[0];
            let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
            let min_count = self.get_min_type_argument_count(&type_parameters) as i32;
            let max_count = type_parameters.len() as i32;
            let mut expected = min_count.to_string();
            if min_count < max_count {
                expected = format!("{}-{}", expected, max_count);
            }
            return u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1,
                vec![expected, arg_count.to_string()],
            );
        }
        // Overloads exist
        let mut below_arg_count = i32::MIN;
        let mut above_arg_count = i32::MAX;
        for &sig in signatures {
            let type_parameters = self.signatures[sig.0 as usize].type_parameters.clone();
            let min_count = self.get_min_type_argument_count(&type_parameters) as i32;
            let max_count = type_parameters.len() as i32;
            if min_count > arg_count as i32 {
                above_arg_count = above_arg_count.min(min_count);
            } else if max_count < arg_count as i32 {
                below_arg_count = below_arg_count.max(max_count);
            }
        }
        if below_arg_count != i32::MIN && above_arg_count != i32::MAX {
            u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::NO_OVERLOAD_EXPECTS_0_TYPE_ARGUMENTS_BUT_OVERLOADS_DO_EXIST_THAT_EXPECT_EITHER_1_OR_2_TYPE_ARGUMENTS,
                vec![
                    arg_count.to_string(),
                    below_arg_count.to_string(),
                    above_arg_count.to_string(),
                ],
            )
        } else {
            let expected = if below_arg_count == i32::MIN {
                above_arg_count
            } else {
                below_arg_count
            };
            u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1,
                vec![expected.to_string(), arg_count.to_string()],
            )
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Untyped / error calls + the invocationError family (Go 10116–10258)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) resolveUntypedCall(node) *Signature` (checker.go:10116).
    pub(crate) fn resolve_untyped_call(&mut self, node: NodeId) -> Option<SignatureId> {
        if call_like_expression_may_have_type_arguments(self.store(), node) {
            // Check type arguments even though we will give an error that
            // untyped calls may not accept type arguments. This gets us
            // diagnostics for the type arguments and marks them as referenced.
            let type_arguments = crate::statements::type_arguments_of(self.store(), node);
            self.check_source_elements(&type_arguments);
        }
        match self.store().node(node).kind {
            Kind::TaggedTemplateExpression => {
                let template = match &self.store().node(node).data {
                    tsc_ast::NodeData::TaggedTemplateExpression(d) => d.template,
                    _ => node,
                };
                let _ = self.check_expression(template);
            }
            Kind::JsxOpeningElement | Kind::JsxSelfClosingElement => {
                // TODO(port M6 jsx wave): checkExpression(node.Attributes()).
            }
            Kind::BinaryExpression => {
                let left = match &self.store().node(node).data {
                    tsc_ast::NodeData::BinaryExpression(d) => d.left,
                    _ => node,
                };
                let _ = self.check_expression(left);
            }
            Kind::CallExpression | Kind::NewExpression => {
                let arguments = binder_node_data::arguments_of(self.store(), node).to_vec();
                for argument in arguments {
                    let _ = self.check_expression(argument);
                }
            }
            _ => {}
        }
        self.any_signature
    }

    /// Go: `func (c *Checker) resolveErrorCall(node) *Signature` (checker.go:10141).
    fn resolve_error_call(&mut self, node: NodeId) -> Option<SignatureId> {
        self.resolve_untyped_call(node);
        self.unknown_signature
    }

    /// Go: `func (c *Checker) isUntypedFunctionCall(funcType, apparentFuncType, numCallSignatures, numConstructSignatures) bool`
    /// (checker.go:10146).
    fn is_untyped_function_call(
        &mut self,
        func_type: TypeId,
        apparent_func_type: TypeId,
        num_call_signatures: usize,
        num_construct_signatures: usize,
    ) -> bool {
        // We exclude union types because we may have a union of function
        // types that happen to have no common signatures.
        let is_any = crate::utilities::is_type_any(Some(self.type_(func_type)));
        let apparent_any_type_parameter = crate::utilities::is_type_any(Some(
            self.type_(apparent_func_type),
        )) && self.type_(func_type).flags.intersects(TypeFlags::TYPE_PARAMETER);
        let empty_signatures_non_union = num_call_signatures == 0
            && num_construct_signatures == 0
            && !self.type_(apparent_func_type).flags.intersects(TypeFlags::UNION);
        if is_any || apparent_any_type_parameter || !empty_signatures_non_union {
            return is_any || apparent_any_type_parameter;
        }
        // Go: `c.getReducedType(apparentFuncType).flags&TypeFlagsNever == 0
        // && c.isTypeAssignableTo(funcType, c.globalFunctionType)`. The
        // reduced-type walk is identity until S9 (unions never reduce here),
        // and the never check composes the raw flags.
        if self.type_(apparent_func_type).flags.intersects(TypeFlags::NEVER) {
            return false;
        }
        match self.global_function_type {
            Some(global_function_type) => {
                self.check_type_assignable_to(func_type, global_function_type, None, None)
            }
            // TODO(port M6 S7): globalFunctionType seeds with the global
            // Function interface; permissive false until then.
            None => false,
        }
    }

    /// Go: `func (c *Checker) invocationErrorDetails(errorTarget, apparentType, kind) *ast.Diagnostic`
    /// (checker.go:10165). PORT: the union-constituent walk and the
    /// related-info elaborations ride the diagnostic-enrichment surface; the
    /// head message is the real Go form.
    fn invocation_error_details(
        &mut self,
        error_target: NodeId,
        apparent_type: TypeId,
        kind: crate::types::SignatureKind,
    ) -> tsc_ast::Diagnostic {
        let is_call = kind == crate::types::SignatureKind::Call;
        let target = if is_property_access_expression(self.store().node(error_target)) {
            match &self.store().node(error_target).data {
                tsc_ast::NodeData::PropertyAccessExpression(p) => p.name,
                _ => error_target,
            }
        } else {
            error_target
        };
        let (message, type_string) = if is_call {
            (
                &diag::TYPE_0_HAS_NO_CALL_SIGNATURES,
                self.type_to_string(apparent_type),
            )
        } else {
            (
                &diag::TYPE_0_HAS_NO_CONSTRUCT_SIGNATURES,
                self.type_to_string(apparent_type),
            )
        };
        // Diagnose get accessors incorrectly called as functions
        // (checker.go:10217) — the resolved symbol is a get accessor invoked
        // with no arguments: swap in the dedicated head message.
        let mut message = message;
        let mut message_args = vec![type_string];
        let parent = self.store().node(error_target).parent.get();
        if is_call
            && parent != NodeId::NONE
            && self.store().node(parent).kind == Kind::CallExpression
            && binder_node_data::arguments_of(self.store(), parent).is_empty()
        {
            if let Some(resolved_symbol) = self.get_resolved_symbol_or_nil(error_target) {
                if self
                    .program_symbol(resolved_symbol)
                    .flags
                    .intersects(SymbolFlags::GET_ACCESSOR)
                {
                    message = &diag::THIS_EXPRESSION_IS_NOT_CALLABLE_BECAUSE_IT_IS_A_GET_ACCESSOR_DID_YOU_MEAN_TO_USE_IT_WITHOUT;
                    message_args = Vec::new();
                }
            }
        }
        u::new_diagnostic_for_node(self.store(), target, message, message_args)
    }

    /// Go: `func (c *Checker) invocationError(errorTarget, apparentType, kind, relatedInformation)`
    /// (checker.go:10232).
    fn invocation_error(
        &mut self,
        error_target: NodeId,
        apparent_type: TypeId,
        kind: crate::types::SignatureKind,
        related_information: Option<tsc_ast::Diagnostic>,
    ) {
        let mut diagnostic = self.invocation_error_details(error_target, apparent_type, kind);
        // TODO(port M6 diagnostic-enrichment): AddRelatedInfo chains and the
        // originating-import recovery (invocationErrorRecovery reads
        // exportTypeLinks.originatingImport); the head diagnostic fires.
        let _ = related_information;
        let _ = &mut diagnostic;
        self.add_diagnostic(diagnostic);
    }

    /// Go: `func (c *Checker) isGenericFunctionReturningFunction(signature) bool`
    /// (checker.go:10249).
    fn is_generic_function_returning_function(&mut self, signature: SignatureId) -> bool {
        let has_type_parameters =
            !self.signatures[signature.0 as usize].type_parameters.is_empty();
        let return_type = self.get_return_type_of_signature(signature);
        has_type_parameters && self.is_function_type(return_type)
    }

    /// Go: `func (c *Checker) skippedGenericFunction(node, checkMode)` (checker.go:10256).
    fn skipped_generic_function(&mut self, node: NodeId, check_mode: CheckMode) {
        if check_mode.intersects(CheckMode::INFERENTIAL) {
            // We have skipped a generic function during inferential typing.
            // Obtain the inference context and indicate this has occurred
            // such that we know a second pass of inference is needed.
            // TODO(port M6 S9): the per-node inference-context links
            // (getInferenceContext) ride the S9 contextual engine.
            let _ = node;
            self.is_inference_partially_blocked = true;
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // checkCallExpression + checkImportCallExpression (Go 8476–8612)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) checkCallExpression(node, checkMode) *Type`
    /// (checker.go:8580).
    pub(crate) fn check_call_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        // Go: node.TypeArguments() — the list node for the grammar checker.
        let type_arguments = crate::grammarchecks::type_argument_list_of(self.store(), node).cloned();
        self.check_grammar_type_arguments(node, type_arguments.as_ref());
        let signature = self.get_resolved_signature_ex(node, None, check_mode);
        if signature == self.resolving_signature {
            // CheckMode.SkipGenericFunctions is enabled and this is a call to
            // a generic function that returns a function type. We defer
            // checking and return silentNeverType.
            return self.silent_never_type();
        }
        if let Some(signature) = signature {
            self.check_deprecated_signature(Some(signature), node);
        }
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .unwrap_or(NodeId::NONE);
        if expression != NodeId::NONE && self.store().node(expression).kind == Kind::SuperKeyword {
            return self.void_type();
        }
        if self.store().node(node).kind == Kind::NewExpression {
            let declaration = signature
                .and_then(|sig| self.signatures[sig.0 as usize].declaration);
            if let Some(declaration) = declaration {
                let declaration_kind = self.store().node(declaration).kind;
                if declaration_kind != Kind::Constructor
                    && declaration_kind != Kind::ConstructSignature
                    && declaration_kind != Kind::ConstructorType
                {
                    // When resolved signature is a call signature (and not a
                    // construct signature) the result type is any
                    if self.no_implicit_any {
                        self.error(
                            node,
                            &diag::X_NEW_EXPRESSION_WHOSE_TARGET_LACKS_A_CONSTRUCT_SIGNATURE_IMPLICITLY_HAS_AN_ANY_TYPE,
                            &[],
                        );
                    }
                    return self.any_type();
                }
            }
        }
        // TODO(port M6 W1/js): IsInJSFile + isCommonJSRequire redirection
        // (resolveExternalModuleTypeByLiteral) rides the W1/js seam.
        let Some(signature) = signature else {
            return self.any_type();
        };
        self.get_return_type_of_signature(signature)
    }

    /// Go: `func (c *Checker) checkDeprecatedSignature(sig, node)` (checker.go:8617).
    /// TODO(port M6 deprecation family): IsDeprecatedDeclaration + the
    /// deprecation-suggestion probes are permissive (never deprecated), so
    /// the body composes Go's no-declaration arm exactly.
    pub(crate) fn check_deprecated_signature_real(&mut self, sig: SignatureId, node: NodeId) {
        if self.signatures[sig.0 as usize]
            .flags
            .intersects(SignatureFlags::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE)
        {
            return;
        }
        let _ = node;
    }

    /// Go: `func (c *Checker) checkImportCallExpression(node) *Type`
    /// (checker.go:8476).
    pub(crate) fn check_import_call_expression(&mut self, node: NodeId) -> TypeId {
        // Check grammar of dynamic import
        self.check_grammar_import_call_expression(node);
        let args = binder_node_data::arguments_of(self.store(), node).to_vec();
        if args.is_empty() {
            // No call arguments exist, so there are no child expressions to
            // check.
            // TODO(port M6 S7): createPromiseReturnType (the Promise<>
            // wrapper) rides the S7 type-construction family; anyType is
            // the composed answer until then.
            return self.any_type();
        }
        let specifier = args[0];
        let specifier_type = self.check_expression_cached(specifier);
        let mut options_type: Option<TypeId> = None;
        if args.len() > 1 {
            options_type = Some(self.check_expression_cached(args[1]));
        }
        // Even though multiple arguments is grammatically incorrect,
        // type-check extra arguments for completion
        for &arg in args.iter().skip(2) {
            let _ = self.check_expression_cached(arg);
        }
        let type_string = self.type_to_string(specifier_type);
        let nullable = self.type_(specifier_type).flags.intersects(TypeFlags::NULLABLE);
        if nullable
            || !self.check_type_assignable_to(
                specifier_type,
                self.string_type(),
                Some(specifier),
                Some(&diag::DYNAMIC_IMPORT_S_SPECIFIER_MUST_BE_OF_TYPE_STRING_BUT_HERE_HAS_TYPE_0),
            )
        {
            // Go reports with the type string; the assignability reporter
            // already fired the message with the elaborated form when it
            // fails, so the explicit error below only fires for the
            // non-assignable-without-report path.
            self.error(
                specifier,
                &diag::DYNAMIC_IMPORT_S_SPECIFIER_MUST_BE_OF_TYPE_STRING_BUT_HERE_HAS_TYPE_0,
                &[type_string],
            );
        }
        let import_attributes_type: Option<TypeId> = None;
        if let Some(options_type) = options_type {
            // TODO(port M6 S7): getGlobalImportCallOptionsTypeChecked +
            // getTypeOfPropertyOfType ride the global-classes surface;
            // permissive skip of the options checks.
            let _ = options_type;
        }
        // resolveExternalModuleName will return undefined if the
        // moduleReferenceExpression is not a string literal
        let module_symbol = self.resolve_external_module_name_ex(
            node,
            Some(specifier),
            false,
            import_attributes_type,
        );
        if module_symbol.is_some() {
            // TODO(port M6 S7): the synthetic-default Promise<> wrapper
            // (getTypeWithSyntheticDefaultOnly/...ImportType) rides the S7
            // type-construction family; anyType composes until then.
            return self.any_type();
        }
        self.any_type()
    }

    // ────────────────────────────────────────────────────────────────────────
    // Effective call arguments (Go checker.go:30591–30705)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getEffectiveCallArguments(node) []*ast.Node`
    /// (checker.go:30591). PORT: the synthetic-expression arms (jsx fragment,
    /// tagged-template TemplateStringsArray, decorator parameters) need the
    /// checker factory's synthetic nodes; they compose permissive empty
    // argument lists until the factory surface lands.
    pub(crate) fn get_effective_call_arguments(&mut self, node: NodeId) -> Vec<NodeId> {
        match self.store().node(node).kind {
            Kind::JsxOpeningFragment => Vec::new(),
            Kind::TaggedTemplateExpression => {
                // TODO(port M6 factory): the TemplateStringsArray synthetic
                // first argument + the span expressions; empty until then.
                Vec::new()
            }
            Kind::Decorator => {
                // TODO(port M6 decorator wave): getEffectiveDecoratorArguments.
                Vec::new()
            }
            Kind::BinaryExpression => {
                // Handles instanceof operator
                match &self.store().node(node).data {
                    tsc_ast::NodeData::BinaryExpression(d) => vec![d.left],
                    _ => Vec::new(),
                }
            }
            Kind::JsxOpeningElement | Kind::JsxSelfClosingElement => {
                // TODO(port M6 jsx wave): the attributes argument.
                Vec::new()
            }
            _ => {
                let args = binder_node_data::arguments_of(self.store(), node).to_vec();
                let spread_index = get_spread_argument_index(self.store(), &args);
                if spread_index >= 0 {
                    // TODO(port M6 S8): Create synthetic arguments from
                    // spreads of tuple types; the tuple element walk rides
                    // the S8 structured-type members. The spread argument
                    // passes through unexpanded until then.
                    args
                } else {
                    args
                }
            }
        }
    }

    // Go: `func (c *Checker) getInstantiatedConstructorsForTypeArguments(type, typeArguments, node) []*Signature`
    // (checker.go:8733) — S8's REAL body (members.rs) replaced this seam;
    // the method resolves there now (the local duplicate is deleted).

    // Go: `func (c *Checker) getConstructorAccessibilityError(node, signatures, modifierFlags) (ModifierFlags, *Type)`
    // (checker.go:8810) — S8's REAL body (members.rs) replaced this seam.

    /// Go: `func (c *Checker) getSignatureInstantiation(signature, typeArguments, isJavaScript, inferredTypeParameters) *Signature`
    /// (checker.go:19745) — S8's REAL body (members.rs
    /// get_signature_instantiation_for_members) replaced the S7 seam. PORT:
    /// the old local stub's `eraseTypeParameters: bool` third parameter was
    /// the S7-era shape; Go's real parameter is `isJavaScript`, and the
    /// call sites already pass Go's `false` literal.
    fn get_signature_instantiation(
        &mut self,
        signature: SignatureId,
        type_arguments: &[TypeId],
        is_javascript: bool,
        inferred_type_parameters: &[TypeId],
    ) -> SignatureId {
        self.get_signature_instantiation_for_members(
            signature,
            type_arguments,
            is_javascript,
            inferred_type_parameters,
        )
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) getSignatureInstantiationWithoutFillingInTypeArguments(sig, typeArguments) *Signature`
    /// (checker.go — the cachedSignatures interning) — S8's REAL body
    /// (members.rs) replaced the S7 twin (the local duplicate is deleted).
    fn get_signature_instantiation_without_filling_in_type_arguments(
        &mut self,
        signature: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        self.get_signature_instantiation_without_filling_in_type_arguments_for_members(signature, type_arguments)
    }

    /// Go: `func (c *Checker) createSignatureInstantiation(signature, typeArguments) *Signature`
    /// (checker.go — the overload-failure instantiator) — S8's REAL body
    /// (members.rs) replaced the S7 seam (the local duplicate is deleted).
    fn create_signature_instantiation(
        &mut self,
        signature: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        self.create_signature_instantiation_for_members(signature, type_arguments)
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) createSignatureTypeMapper(sig, typeArguments) *TypeMapper`
    /// → newTypeMapper(getTypeParametersForMapper(sig), typeArguments) —
    /// S8's REAL body (members.rs) replaced the S7 twin.
    fn create_signature_type_mapper(&mut self, signature: SignatureId, type_arguments: &[TypeId]) -> crate::todo_shapes::TypeMapperId {
        self.create_signature_type_mapper_for_members(signature, type_arguments)
    }

    #[allow(dead_code)] // S8 surface — the later-wave consumers land with M6
    /// Go: `func (c *Checker) getTypeParametersForMapper(sig) []*Type` —
    /// S8's REAL body (members.rs) replaced the S7 twin.
    fn get_type_parameters_for_mapper(&mut self, signature: SignatureId) -> Vec<TypeId> {
        self.get_type_parameters_for_mapper_for_members(signature)
    }

    /// Go: the hasTrailingComma probe resolveCall's signature-help path needs
    /// (checker.go:9077). PORT: the trailing-comma trivia read rides the
    /// scanner surface; signature help never sees it in this wave, so the
    /// permissive false keeps the extra-argument relaxation dormant.
    fn call_argument_list_has_trailing_comma(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) reportCannotInvokePossiblyNullOrUndefinedError(node, facts)`
    /// (checker.go:7588) — the call-target arm of the non-null reporter
    /// family (spine's reportObjectPossiblyNullOrUndefinedError is the
    /// reference arm).
    pub(crate) fn report_cannot_invoke_possibly_null_or_undefined_error(
        &mut self,
        node: NodeId,
        facts: crate::relater::TypeFacts,
    ) {
        if facts.intersects(crate::relater::TypeFacts::IS_UNDEFINED)
            && facts.intersects(crate::relater::TypeFacts::IS_NULL)
        {
            self.error(node, &diag::OBJECT_IS_POSSIBLY_NULL_OR_UNDEFINED, &[]);
        } else if facts.intersects(crate::relater::TypeFacts::IS_UNDEFINED) {
            self.error(node, &diag::OBJECT_IS_POSSIBLY_UNDEFINED, &[]);
        } else {
            self.error(node, &diag::OBJECT_IS_POSSIBLY_NULL, &[]);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// CallState + free helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type CallState struct` (checker.go:9045).
struct CallState {
    node: NodeId,
    type_arguments: Vec<NodeId>,
    args: Vec<NodeId>,
    candidates: Vec<SignatureId>,
    arg_check_mode: CheckMode,
    is_single_non_generic_candidate: bool,
    signature_help_trailing_comma: bool,
    recursive_resolution: bool,
    candidates_for_argument_error: Vec<SignatureId>,
    candidate_for_argument_arity_error: Option<SignatureId>,
    candidate_for_type_argument_error: Option<SignatureId>,
}

/// Go: `func signatureHasLiteralTypes(s) bool` (checker.go:9231).
fn signature_has_literal_types(c: &Checker, signature: SignatureId) -> bool {
    c.signatures[signature.0 as usize]
        .flags
        .intersects(SignatureFlags::HAS_LITERAL_TYPES)
}

/// Go: `func getSpreadArgumentIndex(args) int` (checker.go:30649).
fn get_spread_argument_index(store: &dyn NodeStore, args: &[NodeId]) -> i32 {
    args.iter()
        .position(|&arg| c_is_spread_argument(store, arg))
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// Go: `func isSpreadArgument(arg) bool` (checker.go:30654) — the synthetic
/// spread markers need the factory surface; the syntactic SpreadElement is
/// the real-world case.
fn c_is_spread_argument(store: &dyn NodeStore, arg: NodeId) -> bool {
    store.node(arg).kind == Kind::SpreadElement
}

/// Go: `func getErrorNodeForCallNode(node) *ast.Node` (checker.go:10062).
fn get_error_node_for_call_node(store: &dyn NodeStore, node: NodeId) -> NodeId {
    // Go (checker.go:10062): an import call's arity error points at the
    // specifier argument.
    if is_import_call(store, node) {
        let args = binder_node_data::arguments_of(store, node);
        if !args.is_empty() {
            return args[0];
        }
    }
    node
}

/// Go: `func (c *Checker) callLikeExpressionMayHaveTypeArguments(node) bool`.
fn call_like_expression_may_have_type_arguments(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::CallExpression
            | Kind::NewExpression
            | Kind::TaggedTemplateExpression
            | Kind::JsxOpeningElement
            | Kind::JsxSelfClosingElement
            | Kind::JsxOpeningFragment
    )
}

/// Go: `func isImportCall(node) bool` — a CallExpression whose expression is
/// the `import` keyword identifier.
pub(crate) fn is_import_call(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::CallExpression {
        return false;
    }
    match crate::ast_shims::expression_of(store, node) {
        Some(expression) => {
            let n = store.node(expression);
            n.kind == Kind::ImportKeyword || is_identifier(n) && tsc_ast::node_text(store, expression) == "import"
        }
        None => false,
    }
}

/// Go: `ast.CanHaveDecorators` (the parser's copy is the dedup candidate).
fn can_have_decorators(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::Parameter
            | Kind::PropertyDeclaration
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassExpression
            | Kind::ClassDeclaration
    )
}

/// Go: `ast.IsSuperProperty` — a property/element access whose expression is
/// `super`.
fn is_super_property(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            let expression = match &n.data {
                tsc_ast::NodeData::PropertyAccessExpression(d) => d.expression,
                tsc_ast::NodeData::ElementAccessExpression(d) => d.expression,
                _ => return false,
            };
            store.node(expression).kind == Kind::SuperKeyword
        }
        _ => false,
    }
}
