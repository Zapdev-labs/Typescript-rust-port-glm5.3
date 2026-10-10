// checker.go — the M6 S4 slice: EXPRESSION LEAVES I (M6-DISPATCH wave 6).
// Ported from
// tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763,
// lines 8039-8460 (private-identifier/super/template/regex/array-literal/
// qualified-name/indexed-access checks), 10259-10580 (tagged-template/
// parenthesized/class-expression/function-expression + the contextual-
// signature helpers), 10848-11270 (typeof/non-null/instatiation-expression/
// satisfies/meta/delete/void/await/unary/conditional/spread/yield leaves),
// 11278-13109 (identifier/property-access/accessibility/this/assertion/
// binary/destructuring), and 13155-14220 (truthiness/nullish/object-literal/
// spread/expando/JS-assignment + the getResolvedSymbol family).
//
// PORT (file map): SPEC §4.2 maps checker.go → checker.rs; the slice lives
// here as `expressions.rs` following the crate's one-.rs-per-slice-family
// convention (spine.rs = S1, statements.rs = S3, typeofsymbol.rs = S2).
// Every function keeps its Go name snake_cased, same control flow, same
// evaluation order, same diagnostics.
//
// Stub directions (the slice table in M6-DISPATCH §2 + the wave brief):
//   * getResolvedSignature/resolveCall → S6 (permissive None → the OrElse
//     errorType arms, exactly Go's errorSignature shape at this depth).
//   * flow narrowing (getFlowTypeOfReference/Ex, getFlowTypeOfAccess
//     expression's flow call, isPastLastAssignment, isSymbolAssigned*)
//     → W3 (permissive: the declared type — Go's answer when the reference
//     carries no flow node).
//   * getUnionType multi-constituent → S7 (relater.rs's pinned seam —
//     reused, not duplicated); mapType → S7 (f applied to the type itself —
//     Go's answer for every non-compound type).
//   * getNarrowableTypeForReference/getTypeWithFacts lattice → S10
//     (relater.rs's equal-facts fallback — reused).
//   * getApparentTypeOfContextualType/getContextualSignature → S9 stubs
//     (permissive None; the contextual leaves stay dormant).
//   * getExactOptionalUnassignableProperties → S8 seam (permissive empty).
//   * markPropertyAsReferenced/markLinkedReferences → S10's reference-
//     tracking tail (permissive no-ops; diagnostics-neutral).
//   * checkGrammar* → W3 (statements.rs's permissive no-op family).
//   * the six checkJsx* arms → W5 (spine.rs keeps the jsx.go banners).
//
// S4-slice stubs REPLACED here (previously permissive in statements.rs):
// checkTruthinessOfType, checkTruthinessExpression, checkReferenceExpression,
// checkDestructuringAssignment (the *_stub carrier), isConstContext.

use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    is_identifier, Kind, ModifierFlags, NodeId, NodeStore, SymbolFlags, SymbolId,
};
use tsc_binder::scanner_shims::token_to_string;
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_diagnostics::{
    ABSTRACT_METHOD_0_IN_CLASS_1_CANNOT_BE_ACCESSED_VIA_SUPER_EXPRESSION,
    AN_ARITHMETIC_OPERAND_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
    AN_EXPRESSION_OF_TYPE_VOID_CANNOT_BE_TESTED_FOR_TRUTHINESS,
    AN_OBJECT_S_SYMBOL_HASINSTANCE_METHOD_MUST_RETURN_A_BOOLEAN_VALUE_FOR_IT_TO_BE_USED_ON_THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION,
    A_CONST_ASSERTION_CAN_ONLY_BE_APPLIED_TO_REFERENCES_TO_ENUM_MEMBERS_OR_STRING_NUMBER_BOOLEAN_ARRAY_OR_OBJECT_LITERALS,
    A_REST_ELEMENT_CANNOT_HAVE_AN_INITIALIZER, A_REST_ELEMENT_MUST_BE_LAST_IN_A_DESTRUCTURING_PATTERN,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY,
    CANNOT_ASSIGN_TO_PRIVATE_METHOD_0_PRIVATE_METHODS_ARE_NOT_WRITABLE,
    CANNOT_USE_THIS_IN_A_STATIC_PROPERTY_INITIALIZER_OF_A_DECORATED_CLASS,
    CLASS_FIELD_0_DEFINED_BY_THE_PARENT_CLASS_IS_NOT_ACCESSIBLE_IN_THE_CHILD_CLASS_VIA_SUPER,
    EXPONENTIATION_CANNOT_BE_PERFORMED_ON_BIGINT_VALUES_UNLESS_THE_TARGET_OPTION_IS_SET_TO_ES2016_OR_LATER,
    LEFT_SIDE_OF_COMMA_OPERATOR_IS_UNUSED_AND_HAS_NO_SIDE_EFFECTS,
    META_PROPERTY_0_IS_ONLY_ALLOWED_IN_THE_BODY_OF_A_FUNCTION_DECLARATION_FUNCTION_EXPRESSION_OR_CONSTRUCTOR,
    OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPE_1, OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPES_1_AND_2,
    PRIVATE_ACCESSOR_WAS_DEFINED_WITHOUT_A_GETTER,
    PRIVATE_FIELD_0_MUST_BE_DECLARED_IN_AN_ENCLOSING_CLASS,
    PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
    PRIVATE_IDENTIFIERS_CANNOT_BE_USED_IN_DESTRUCTURING_PATTERNS,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
    PROPERTY_0_IS_PRIVATE_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1,
    PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_THROUGH_AN_INSTANCE_OF_CLASS_1_THIS_IS_AN_INSTANCE_OF_CLASS_2,
    PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1_AND_ITS_SUBCLASSES,
    PROPERTY_0_IS_USED_BEFORE_BEING_ASSIGNED,
    PROPERTY_0_MAY_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
    PROPERTY_ASSIGNMENT_EXPECTED,
    RIGHT_OPERAND_OF_IS_UNREACHABLE_BECAUSE_THE_LEFT_OPERAND_IS_NEVER_NULLISH,
    THE_0_OPERATOR_CANNOT_BE_APPLIED_TO_TYPE_SYMBOL,
    THE_0_OPERATOR_IS_NOT_ALLOWED_FOR_BOOLEAN_TYPES_CONSIDER_USING_1_INSTEAD,
    THE_CONTAINING_ARROW_FUNCTION_CAPTURES_THE_GLOBAL_VALUE_OF_THIS,
    THE_IMPORT_META_META_PROPERTY_IS_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_ES2020_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_OR_NODENEXT,
    THE_LEFT_HAND_SIDE_OF_AN_ARITHMETIC_OPERATION_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
    THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THE_LEFT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_BE_OF_TYPE_ANY_AN_OBJECT_TYPE_OR_A_TYPE_PARAMETER,
    THE_OPERAND_OF_A_DELETE_OPERATOR_CANNOT_BE_A_PRIVATE_IDENTIFIER,
    THE_OPERAND_OF_A_DELETE_OPERATOR_CANNOT_BE_A_READ_ONLY_PROPERTY,
    THE_OPERAND_OF_A_DELETE_OPERATOR_MUST_BE_A_PROPERTY_REFERENCE,
    THE_OPERAND_OF_A_DELETE_OPERATOR_MUST_BE_OPTIONAL,
    THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
    THE_RIGHT_HAND_SIDE_OF_AN_ARITHMETIC_OPERATION_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
    THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_NOT_BE_AN_INSTANTIATION_EXPRESSION,
    A_REST_PARAMETER_OR_BINDING_PATTERN_MAY_NOT_HAVE_A_TRAILING_COMMA,
    CONVERSION_OF_TYPE_0_TO_TYPE_1_MAY_BE_A_MISTAKE_BECAUSE_NEITHER_TYPE_SUFFICIENTLY_OVERLAPS_WITH_THE_OTHER_IF_THIS_WAS_INTENTIONAL_CONVERT_THE_EXPRESSION_TO_UNKNOWN_FIRST,
    TYPE_OF_ITERATED_ELEMENTS_OF_A_YIELD_ASTERISK_OPERAND_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
    TYPE_OF_YIELD_OPERAND_IN_AN_ASYNC_GENERATOR_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
    X_AWAIT_HAS_NO_EFFECT_ON_THE_TYPE_OF_THIS_EXPRESSION,
    PROPERTY_0_IS_NOT_ACCESSIBLE_OUTSIDE_CLASS_1_BECAUSE_IT_HAS_A_PRIVATE_IDENTIFIER,
    THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THIS_CONDITION_WILL_ALWAYS_RETURN_0,
    THIS_CONDITION_WILL_ALWAYS_RETURN_0_SINCE_JAVASCRIPT_COMPARES_OBJECTS_BY_REFERENCE_NOT_VALUE,
    THIS_COMPARISON_APPEARS_TO_BE_UNINTENTIONAL_BECAUSE_THE_TYPES_0_AND_1_HAVE_NO_OVERLAP,
    THIS_EXPRESSION_IS_ALWAYS_NULLISH, THIS_KIND_OF_EXPRESSION_IS_ALWAYS_FALSY,
    THIS_KIND_OF_EXPRESSION_IS_ALWAYS_TRUTHY,
    TYPE_0_DOES_NOT_SATISFY_THE_EXPECTED_TYPE_1,
    TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPE_OF_THE_TARGET,
    TYPE_0_MAY_REPRESENT_A_PRIMITIVE_VALUE_WHICH_IS_NOT_PERMITTED_AS_THE_RIGHT_OPERAND_OF_THE_IN_OPERATOR,
    X_0_AND_1_OPERATIONS_CANNOT_BE_MIXED_WITHOUT_PARENTHESES,
    X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_THIS_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
    X_THIS_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME,
    X_THIS_CANNOT_BE_REFERENCED_IN_CURRENT_LOCATION,
    X_THIS_CANNOT_BE_REFERENCED_IN_A_MODULE_OR_NAMESPACE_BODY,
    X_THIS_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_TYPE_ANNOTATION,
    X_YIELD_EXPRESSION_IMPLICITLY_RESULTS_IN_AN_ANY_TYPE_BECAUSE_ITS_CONTAINING_GENERATOR_LACKS_A_RETURN_TYPE_ANNOTATION,
};

use crate::checker::CheckMode;
use crate::relater::{self, TypeFacts};
use crate::types::{TypeFlags, TypeId};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// PredicateSemantics (Go checker.go:13121-13128)
// ────────────────────────────────────────────────────────────────────────────

define_flags!(PredicateSemantics, u32);

impl PredicateSemantics {
    /// Go: `PredicateSemanticsAlways PredicateSemantics = 1 << 0`.
    pub const ALWAYS: PredicateSemantics = PredicateSemantics(1 << 0);
    /// Go: `PredicateSemanticsNever PredicateSemantics = 1 << 1`.
    pub const NEVER: PredicateSemantics = PredicateSemantics(1 << 1);
    /// Go: `PredicateSemanticsSometimes = PredicateSemanticsAlways | PredicateSemanticsNever`.
    pub const SOMETIMES: PredicateSemantics =
        PredicateSemantics(Self::ALWAYS.0 | Self::NEVER.0);
}

// ────────────────────────────────────────────────────────────────────────────
// Truthiness / nullish semantics (Go checker.go:13109-13245)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkTruthinessExpression(node, checkMode) *Type`
    /// (checker.go:11179). Moved from statements.rs's S4-bannered carrier —
    /// the slice line range (10848-11270) is S4's.
    pub(crate) fn check_truthiness_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let t = self.check_expression_ex(node, check_mode);
        self.check_truthiness_of_type(t, node)
    }

    /// Go: `func (c *Checker) checkTruthinessOfType(t, node) *Type`
    /// (checker.go:13109).
    pub(crate) fn check_truthiness_of_type(&mut self, t: TypeId, node: NodeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::VOID) {
            self.error(node, &AN_EXPRESSION_OF_TYPE_VOID_CANNOT_BE_TESTED_FOR_TRUTHINESS, &[]);
            return t;
        }
        let semantics = self.get_syntactic_truthy_semantics(node);
        if semantics != PredicateSemantics::SOMETIMES {
            let message = if semantics == PredicateSemantics::ALWAYS {
                &THIS_KIND_OF_EXPRESSION_IS_ALWAYS_TRUTHY
            } else {
                &THIS_KIND_OF_EXPRESSION_IS_ALWAYS_FALSY
            };
            self.error(node, message, &[]);
        }
        t
    }

    /// Go: `func (c *Checker) getSyntacticTruthySemantics(node) PredicateSemantics`
    /// (checker.go:13130).
    pub(crate) fn get_syntactic_truthy_semantics(&mut self, node: NodeId) -> PredicateSemantics {
        let node = self.skip_outer_expressions_all(node);
        let n = self.store().node(node);
        match n.kind {
            Kind::NumericLiteral => {
                // Allow `while(0)` or `while(1)`
                let text = tsc_ast::node_text(self.store(), node);
                if text == "0" || text == "1" {
                    return PredicateSemantics::SOMETIMES;
                }
                PredicateSemantics::ALWAYS
            }
            Kind::ArrayLiteralExpression
            | Kind::ArrowFunction
            | Kind::BigIntLiteral
            | Kind::ClassExpression
            | Kind::FunctionExpression
            | Kind::JsxElement
            | Kind::JsxSelfClosingElement
            | Kind::ObjectLiteralExpression
            | Kind::RegularExpressionLiteral => PredicateSemantics::ALWAYS,
            Kind::VoidExpression | Kind::NullKeyword => PredicateSemantics::NEVER,
            Kind::NoSubstitutionTemplateLiteral | Kind::StringLiteral => {
                if !tsc_ast::node_text(self.store(), node).is_empty() {
                    PredicateSemantics::ALWAYS
                } else {
                    PredicateSemantics::NEVER
                }
            }
            Kind::ConditionalExpression => {
                let (when_true, when_false) = {
                    let d = n.as_conditional_expression().expect("ConditionalExpression data");
                    (d.when_true, d.when_false)
                };
                self.get_syntactic_truthy_semantics(when_true)
                    .union(self.get_syntactic_truthy_semantics(when_false))
            }
            Kind::Identifier => {
                if self.get_resolved_symbol(node) == Some(self.undefined_symbol()) {
                    return PredicateSemantics::NEVER;
                }
                PredicateSemantics::SOMETIMES
            }
            _ => PredicateSemantics::SOMETIMES,
        }
    }

    /// Go: `func (c *Checker) checkNullishCoalesceOperands(left, right)` (checker.go:13159).
    pub(crate) fn check_nullish_coalesce_operands(&mut self, left: NodeId, right: NodeId) {
        let store = self.store();
        let left_parent = store.node(left).parent.get();
        let grandparent = store.node(left_parent).parent.get();
        if store.node(grandparent).kind == Kind::BinaryExpression {
            let d = store
                .node(grandparent)
                .as_binary_expression()
                .expect("BinaryExpression data");
            let grandparent_left = d.left;
            let grandparent_operator_kind = store.node(d.operator_token).kind;
            if store.node(grandparent_left).kind == Kind::BinaryExpression
                && grandparent_operator_kind == Kind::BarBarToken
            {
                self.grammar_error_on_node(
                    grandparent_left,
                    &X_0_AND_1_OPERATIONS_CANNOT_BE_MIXED_WITHOUT_PARENTHESES,
                    &[token_to_string(Kind::QuestionQuestionToken).to_string(), token_to_string(grandparent_operator_kind).to_string()],
                );
            }
        } else if store.node(left).kind == Kind::BinaryExpression {
            let operator_kind = store
                .node(left)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .operator_token;
            let operator_kind = store.node(operator_kind).kind;
            if operator_kind == Kind::BarBarToken || operator_kind == Kind::AmpersandAmpersandToken {
                self.grammar_error_on_node(
                    left,
                    &X_0_AND_1_OPERATIONS_CANNOT_BE_MIXED_WITHOUT_PARENTHESES,
                    &[token_to_string(operator_kind).to_string(), token_to_string(Kind::QuestionQuestionToken).to_string()],
                );
            }
        } else if store.node(right).kind == Kind::BinaryExpression {
            let operator_token = store
                .node(right)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .operator_token;
            let operator_kind = store.node(operator_token).kind;
            if operator_kind == Kind::AmpersandAmpersandToken {
                self.grammar_error_on_node(
                    right,
                    &X_0_AND_1_OPERATIONS_CANNOT_BE_MIXED_WITHOUT_PARENTHESES,
                    &[token_to_string(Kind::QuestionQuestionToken).to_string(), token_to_string(operator_kind).to_string()],
                );
            }
        }
        self.check_nullish_coalesce_operand_left(left);
    }

    /// Go: `func (c *Checker) checkNullishCoalesceOperandLeft(left)` (checker.go:13180).
    pub(crate) fn check_nullish_coalesce_operand_left(&mut self, left: NodeId) {
        let left_target = self.skip_outer_expressions_all(left);
        let nullish_semantics = self.get_syntactic_nullishness_semantics(left_target);
        if nullish_semantics != PredicateSemantics::SOMETIMES {
            if nullish_semantics == PredicateSemantics::ALWAYS {
                self.error(left_target, &THIS_EXPRESSION_IS_ALWAYS_NULLISH, &[]);
            } else {
                self.error(
                    left_target,
                    &RIGHT_OPERAND_OF_IS_UNREACHABLE_BECAUSE_THE_LEFT_OPERAND_IS_NEVER_NULLISH,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) getSyntacticNullishnessSemantics(node) PredicateSemantics`
    /// (checker.go:13192).
    pub(crate) fn get_syntactic_nullishness_semantics(&mut self, node: NodeId) -> PredicateSemantics {
        let node = self.skip_outer_expressions_all(node);
        let n = self.store().node(node);
        match n.kind {
            Kind::AwaitExpression
            | Kind::CallExpression
            | Kind::TaggedTemplateExpression
            | Kind::ElementAccessExpression
            | Kind::MetaProperty
            | Kind::NewExpression
            | Kind::PropertyAccessExpression
            | Kind::YieldExpression
            | Kind::ThisKeyword => PredicateSemantics::SOMETIMES,
            Kind::BinaryExpression => {
                // List of operators that can produce null/undefined:
                // || ||= && &&= ?? ??=
                let (op_kind, d_left, d_right) = {
                    let d = n.as_binary_expression().expect("BinaryExpression data");
                    (
                        self.store().node(d.operator_token).kind,
                        d.left,
                        d.right,
                    )
                };
                match op_kind {
                    Kind::BarBarToken
                    | Kind::BarBarEqualsToken
                    | Kind::AmpersandAmpersandToken
                    | Kind::AmpersandAmpersandEqualsToken => PredicateSemantics::SOMETIMES,
                    // For these operator kinds, the right operand is effectively controlling
                    Kind::CommaToken | Kind::EqualsToken => {
                        self.get_syntactic_nullishness_semantics(d_right)
                    }
                    // For nullish coalescing: result is the left operand when left is non-null,
                    // or the right operand when left is null/undefined. The nullishness of the
                    // result combines both paths: the left's non-null path contributes Never,
                    // and when left can be null, the right's semantics are also included.
                    Kind::QuestionQuestionToken | Kind::QuestionQuestionEqualsToken => {
                        let left_semantics = self.get_syntactic_nullishness_semantics(d_left);
                        // The non-null path (left is non-null): left branch taken, result has Never bit
                        let mut result = left_semantics & PredicateSemantics::NEVER;
                        // The null path (left is null/undefined): right branch taken, result inherits right's semantics
                        if left_semantics.intersects(PredicateSemantics::ALWAYS) {
                            result |= self.get_syntactic_nullishness_semantics(d_right);
                        }
                        result
                    }
                    _ => PredicateSemantics::NEVER,
                }
            }
            Kind::ConditionalExpression => {
                let (when_true, when_false) = {
                    let d = n.as_conditional_expression().expect("ConditionalExpression data");
                    (d.when_true, d.when_false)
                };
                self.get_syntactic_nullishness_semantics(when_true)
                    .union(self.get_syntactic_nullishness_semantics(when_false))
            }
            Kind::NullKeyword => PredicateSemantics::ALWAYS,
            Kind::Identifier => {
                if self.get_resolved_symbol(node) == Some(self.undefined_symbol()) {
                    PredicateSemantics::ALWAYS
                } else {
                    PredicateSemantics::SOMETIMES
                }
            }
            _ => PredicateSemantics::NEVER,
        }
    }

    /// Go: `func (c *Checker) isSideEffectFree(node) bool` (checker.go:13255) —
    /// the *shallow* side-effect smell test.
    pub(crate) fn is_side_effect_free(&mut self, node: NodeId) -> bool {
        let node = tsc_binder::ast_shims::skip_parentheses(self.store(), node);
        let n = self.store().node(node);
        match n.kind {
            Kind::Identifier
            | Kind::StringLiteral
            | Kind::RegularExpressionLiteral
            | Kind::TaggedTemplateExpression
            | Kind::TemplateExpression
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::NullKeyword
            | Kind::UndefinedKeyword
            | Kind::FunctionExpression
            | Kind::ClassExpression
            | Kind::ArrowFunction
            | Kind::ArrayLiteralExpression
            | Kind::ObjectLiteralExpression
            | Kind::TypeOfExpression
            | Kind::NonNullExpression
            | Kind::JsxSelfClosingElement
            | Kind::JsxElement => true,
            Kind::ConditionalExpression => {
                let (when_true, when_false) = {
                    let d = n.as_conditional_expression().expect("ConditionalExpression data");
                    (d.when_true, d.when_false)
                };
                self.is_side_effect_free(when_true) && self.is_side_effect_free(when_false)
            }
            Kind::BinaryExpression => {
                let (operator_kind, d_left, d_right) = {
                    let d = n.as_binary_expression().expect("BinaryExpression data");
                    (self.store().node(d.operator_token).kind, d.left, d.right)
                };
                if tsc_ast::is_assignment_operator(operator_kind) {
                    return false;
                }
                self.is_side_effect_free(d_left) && self.is_side_effect_free(d_right)
            }
            Kind::PrefixUnaryExpression => {
                // Unary operators ~, !, +, and - have no side effects.
                // The rest do.
                let d = n.as_prefix_unary_expression().expect("PrefixUnaryExpression data");
                matches!(
                    d.operator,
                    Kind::ExclamationToken | Kind::PlusToken | Kind::MinusToken | Kind::TildeToken
                )
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) isIndirectCall(node) bool` (checker.go:13283) —
    /// true for `(0, x.f)(...)` / `(0, eval)(...)`, which prevents passing `this`.
    pub(crate) fn is_indirect_call(&mut self, node: NodeId) -> bool {
        let store = self.store();
        let d = store
            .node(node)
            .as_binary_expression()
            .expect("BinaryExpression data");
        let left = d.left;
        let right = d.right;
        let parent = store.node(node).parent.get();
        let parent_is_paren = store.node(parent).kind == Kind::ParenthesizedExpression;
        let left_is_zero = store.node(left).kind == Kind::NumericLiteral
            && tsc_ast::node_text(store, left) == "0";
        if !(parent_is_paren && left_is_zero) {
            return false;
        }
        let grandparent = store.node(parent).parent.get();
        let callee_matches = (store.node(grandparent).kind == Kind::CallExpression
            && store
                .node(grandparent)
                .as_call_expression()
                .expect("CallExpression data")
                .expression
                == parent)
            || store.node(grandparent).kind == Kind::TaggedTemplateExpression;
        if !callee_matches {
            return false;
        }
        tsc_ast::is_access_expression(store.node(right))
            || is_identifier(store.node(right)) && tsc_ast::node_text(store, right) == "eval"
    }

    // ── The operator-error reporting family (Go checker.go:12962-13053) ──

    /// Go: `func (c *Checker) reportOperatorError(leftType, operator, rightType, errorNode, isRelated)`
    /// (checker.go:12962). PORT: Go's `isRelated func(left, right) bool`
    /// closure becomes an `Option<fn(&mut Checker, TypeId, TypeId) -> bool>`
    /// (every Go call site's closure is capture-free or a method reference).
    pub(crate) fn report_operator_error(
        &mut self,
        left_type: TypeId,
        operator: Kind,
        right_type: TypeId,
        error_node: Option<NodeId>,
        is_related: Option<fn(&mut Checker, TypeId, TypeId) -> bool>,
    ) {
        let mut would_work_with_await = false;
        if let Some(is_related) = is_related {
            let awaited_left_type = self.get_awaited_type_no_alias(left_type);
            let awaited_right_type = self.get_awaited_type_no_alias(right_type);
            would_work_with_await = !(awaited_left_type == Some(left_type)
                && awaited_right_type == Some(right_type))
                && awaited_left_type.is_some()
                && awaited_right_type.is_some()
                && is_related(self, awaited_left_type.expect("checked"), awaited_right_type.expect("checked"));
        }
        let mut effective_left = left_type;
        let mut effective_right = right_type;
        if !would_work_with_await && let Some(is_related) = is_related {
            let (l, r) = self.get_base_types_if_unrelated(left_type, right_type, is_related);
            effective_left = l;
            effective_right = r;
        }
        let (left_str, right_str) = self.get_type_names_for_error_display(effective_left, effective_right);
        let Some(error_node) = error_node else {
            // PORT: Go passes a nil errorNode only on paths that never
            // report (checkDestructuringAssignment's shorthand default);
            // the port skips the emit rather than panicking.
            return;
        };
        match operator {
            Kind::EqualsEqualsEqualsToken
            | Kind::EqualsEqualsToken
            | Kind::ExclamationEqualsEqualsToken
            | Kind::ExclamationEqualsToken => {
                self.error_and_maybe_suggest_await(
                    error_node,
                    would_work_with_await,
                    &THIS_COMPARISON_APPEARS_TO_BE_UNINTENTIONAL_BECAUSE_THE_TYPES_0_AND_1_HAVE_NO_OVERLAP,
                    &[left_str, right_str],
                );
            }
            _ => {
                self.error_and_maybe_suggest_await(
                    error_node,
                    would_work_with_await,
                    &OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPES_1_AND_2,
                    &[token_to_string(operator).to_string(), left_str, right_str],
                );
            }
        }
    }

    /// Go: `func (c *Checker) reportOperatorErrorUnless(leftType, operator, rightType, errorNode, typesAreCompatible)`
    /// (checker.go:12983).
    pub(crate) fn report_operator_error_unless(
        &mut self,
        left_type: TypeId,
        operator: Kind,
        right_type: TypeId,
        error_node: Option<NodeId>,
        types_are_compatible: fn(&mut Checker, TypeId, TypeId) -> bool,
    ) {
        if !types_are_compatible(self, left_type, right_type) {
            self.report_operator_error(left_type, operator, right_type, error_node, Some(types_are_compatible));
        }
    }

    /// Go: `func (c *Checker) getBaseTypesIfUnrelated(leftType, rightType, isRelated) (*Type, *Type)`
    /// (checker.go:12989).
    pub(crate) fn get_base_types_if_unrelated(
        &mut self,
        left_type: TypeId,
        right_type: TypeId,
        is_related: fn(&mut Checker, TypeId, TypeId) -> bool,
    ) -> (TypeId, TypeId) {
        let mut effective_left = left_type;
        let mut effective_right = right_type;
        let left_base = self.get_base_type_of_literal_type(left_type);
        let right_base = self.get_base_type_of_literal_type(right_type);
        if !is_related(self, left_base, right_base) {
            effective_left = left_base;
            effective_right = right_base;
        }
        (effective_left, effective_right)
    }

    /// Go: `func (c *Checker) checkAssignmentOperator(left, operator, right, leftType, rightType)`
    /// (checker.go:13001).
    pub(crate) fn check_assignment_operator(
        &mut self,
        left: NodeId,
        operator: Kind,
        right: NodeId,
        mut left_type: TypeId,
        right_type: TypeId,
    ) {
        if !tsc_ast::is_assignment_operator(operator) {
            return;
        }
        let is_exports_property_declaration = {
            let store = self.store();
            let left_parent = store.node(left).parent.get();
            // We ignore assignments of undefined to CommonJS exports when there are multiple assignment declarations
            is_declaration_node(store.node(left_parent))
                && tsc_ast::get_assignment_declaration_kind(store, left_parent)
                    == tsc_ast::JSDeclarationKind::ExportsProperty
        };
        if is_exports_property_declaration {
            let resolved = self.symbol_node_links.get(left).resolved_symbol;
            if let Some(symbol) = resolved
                && self.program_symbol(symbol).declarations.len() > 1
                && self.type_(right_type).flags.intersects(TypeFlags::UNDEFINED)
            {
                return;
            }
        }
        // getters can be a subtype of setters, so to check for assignability we use the setter's type instead
        if tsc_ast::is_compound_assignment_operator(operator)
            && self.store().node(left).kind == Kind::PropertyAccessExpression
        {
            left_type = self.check_property_access_expression(left, CheckMode::CHECK_MODE_NORMAL, true /*writeOnly*/);
        }
        if self.check_reference_expression(
            left,
            &THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
            &THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
        ) {
            let mut head_message: Option<&'static Message> = None;
            if self.exact_optional_property_types
                && self.store().node(left).kind == Kind::PropertyAccessExpression
                && relater::maybe_type_of_kind(self, right_type, TypeFlags::UNDEFINED)
            {
                let expression = crate::ast_shims::expression_of(self.store(), left)
                    .expect("PropertyAccessExpression has an expression");
                let owner_type = self.get_type_of_expression(expression);
                let prop_name = tsc_ast::node_text(self.store(), left_name_of(self, left)).to_string();
                let target = self.get_type_of_property_of_type(owner_type, &prop_name);
                if self.is_exact_optional_property_mismatch(Some(right_type), target) {
                    head_message = Some(&TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPE_OF_THE_TARGET);
                }
            }
            // to avoid cascading errors check assignability only if 'isReference' check succeeded and no errors were reported
            self.check_type_assignable_to_and_optionally_elaborate(
                right_type,
                left_type,
                Some(left),
                Some(right),
                head_message,
                None,
            );
        }
    }

    /// Go: `func (c *Checker) bothAreBigIntLike(left, right) bool` (checker.go:13027).
    pub(crate) fn both_are_big_int_like(&mut self, left: TypeId, right: TypeId) -> bool {
        self.is_type_assignable_to_kind(left, TypeFlags::BIGINT_LIKE)
            && self.is_type_assignable_to_kind(right, TypeFlags::BIGINT_LIKE)
    }

    /// Go: `func (c *Checker) getSuggestedBooleanOperator(operator) ast.Kind`
    /// (checker.go:13031).
    pub(crate) fn get_suggested_boolean_operator(&self, operator: Kind) -> Kind {
        match operator {
            Kind::BarToken | Kind::BarEqualsToken => Kind::BarBarToken,
            Kind::CaretToken | Kind::CaretEqualsToken => Kind::ExclamationEqualsEqualsToken,
            Kind::AmpersandToken | Kind::AmpersandEqualsToken => Kind::AmpersandAmpersandToken,
            _ => Kind::Unknown,
        }
    }

    /// Go: `func (c *Checker) checkArithmeticOperandType(operand, t, diagnostic, isAwaitValid) bool`
    /// (checker.go:13043). PORT: Go's `numberOrBigIntType` is a union
    /// (numberType | bigintType) the S7 wave constructs; until then the
    /// assignability gate is skipped (permissive no-error — a false error
    /// would be a wrong diagnostic).
    pub(crate) fn check_arithmetic_operand_type(
        &mut self,
        operand: NodeId,
        t: TypeId,
        diagnostic: &'static Message,
        is_await_valid: bool,
    ) -> bool {
        let Some(number_or_big_int_type) = self.number_or_big_int_type() else {
            // TODO(port M6 S7): numberOrBigIntType = getUnionType([number, bigint]).
            let _ = (operand, diagnostic);
            return true;
        };
        if !self.is_type_assignable_to(t, number_or_big_int_type) {
            let mut awaited_type = None;
            if is_await_valid {
                awaited_type = self.get_awaited_type_of_promise_stub(t);
            }
            let suggest_await = match awaited_type {
                Some(at) => self.is_type_assignable_to(at, number_or_big_int_type),
                None => false,
            };
            self.error_and_maybe_suggest_await(operand, suggest_await, diagnostic, &[]);
            return false;
        }
        true
    }

    /// Go: `func (c *Checker) checkForDisallowedESSymbolOperand(left, right, leftType, rightType, operator) bool`
    /// (checker.go:13056) — true when there was NO error.
    pub(crate) fn check_for_disallowed_es_symbol_operand(
        &mut self,
        left: NodeId,
        right: NodeId,
        left_type: TypeId,
        right_type: TypeId,
        operator: Kind,
    ) -> bool {
        let mut offending_symbol_operand = None;
        // TODO(port M6 S10): maybeTypeOfKindConsideringBaseConstraint — the
        // constraint-walking variant; the flag-only maybeTypeOfKind is Go's
        // answer for every non-generic type (the constraint walk only widens
        // the match, so this can only under-report, never over-report).
        if relater::maybe_type_of_kind(self, left_type, TypeFlags::ESSYMBOL_LIKE) {
            offending_symbol_operand = Some(left);
        } else if relater::maybe_type_of_kind(self, right_type, TypeFlags::ESSYMBOL_LIKE) {
            offending_symbol_operand = Some(right);
        }
        if let Some(offending) = offending_symbol_operand {
            self.error(
                offending,
                &THE_0_OPERATOR_CANNOT_BE_APPLIED_TO_TYPE_SYMBOL,
                &[token_to_string(operator).to_string()],
            );
            return false;
        }
        true
    }

    /// Go: `func (c *Checker) checkNaNEquality(errorNode, operator, left, right)`
    /// (checker.go:13071).
    pub(crate) fn check_nan_equality(
        &mut self,
        error_node: NodeId,
        operator: Kind,
        left: NodeId,
        right: NodeId,
    ) {
        let is_left_nan = self.is_global_nan(tsc_binder::ast_shims::skip_parentheses(self.store(), left));
        let is_right_nan = self.is_global_nan(tsc_binder::ast_shims::skip_parentheses(self.store(), right));
        if !(is_left_nan || is_right_nan) {
            return;
        }
        let result_token = if operator == Kind::EqualsEqualsEqualsToken || operator == Kind::EqualsEqualsToken {
            Kind::FalseKeyword
        } else {
            Kind::TrueKeyword
        };
        let err = self.error(error_node, &THIS_CONDITION_WILL_ALWAYS_RETURN_0, &[token_to_string(result_token).to_string()]);
        if is_left_nan && is_right_nan {
            return;
        }
        let mut operator_string = String::new();
        if operator == Kind::ExclamationEqualsEqualsToken || operator == Kind::ExclamationEqualsToken {
            operator_string = token_to_string(Kind::ExclamationToken).to_string();
        }
        let location = if is_left_nan { right } else { left };
        let expression = tsc_binder::ast_shims::skip_parentheses(self.store(), location);
        let mut entity_name = "...".to_string();
        if tsc_ast::utilities::is_entity_name_expression(self.store(), expression) {
            entity_name = u::entity_name_to_string(self.store(), expression);
        }
        let suggestion = format!("{operator_string}Number.isNaN({entity_name})");
        let _ = err;
        // Go: err.AddRelatedInfo(createDiagnosticForNode(location, Did_you_mean_0, suggestion)).
        // TODO(port M7 diagnostics): the related-info attach (the
        // related-diagnostic factory is the M7 diagnostics wave).
        let _ = (location, suggestion);
    }

    /// Go: `func (c *Checker) isGlobalNaN(expr) bool` (checker.go:13097).
    pub(crate) fn is_global_nan(&mut self, expr: NodeId) -> bool {
        if is_identifier(self.store().node(expr)) && tsc_ast::node_text(self.store(), expr) == "NaN" {
            let global_nan_symbol = self.get_global_nan_symbol_or_nil();
            return global_nan_symbol.is_some() && global_nan_symbol == self.get_resolved_symbol(expr);
        }
        false
    }

    // ── instanceof / in (Go checker.go:13291-13372) ──
    // (isTypeEqualityComparableTo lives in statements.rs — real relater routing.)

    /// Go: `func (c *Checker) checkInstanceOfExpression(left, right, leftType, rightType, checkMode) *Type`
    /// (checker.go:13291).
    pub(crate) fn check_instance_of_expression(
        &mut self,
        left: NodeId,
        right: NodeId,
        left_type: TypeId,
        right_type: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let _ = check_mode;
        if left_type == self.silent_never_type() || right_type == self.silent_never_type() {
            return self.silent_never_type();
        }
        // TypeScript 1.0 spec (April 2014): 4.15.4
        // The instanceof operator requires the left operand to be of type Any, an object type, or a type parameter type,
        // and the right operand to be of type Any, a subtype of the 'Function' interface type, or have a call or construct signature.
        // The result is always of the Boolean primitive type.
        // NOTE: do not raise error if leftType is unknown as related error was already reported
        if !crate::utilities::is_type_any(Some(self.type_(left_type)))
            && self.all_types_assignable_to_kind(left_type, TypeFlags::PRIMITIVE)
        {
            self.error(
                left,
                &THE_LEFT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_BE_OF_TYPE_ANY_AN_OBJECT_TYPE_OR_A_TYPE_PARAMETER,
                &[],
            );
        }
        // TODO(port M6 S6): getResolvedSignature(left.Parent, nil, checkMode) —
        // the hasInstance resolution; permissive error-signature shape (Go's
        // resolveSignature never returns nil — the error signature's return
        // type is errorType, which is what the stubbed seam yields here).
        let signature = self.get_resolved_signature_stub(left, None, check_mode);
        let return_type = match signature {
            Some(sig) => self.get_return_type_of_signature(sig),
            None => self.error_type(),
        };
        // We also verify that the return type of the `[Symbol.hasInstance]` method is assignable to
        // `boolean`. According to the spec, the runtime will actually perform `ToBoolean` on the result,
        // but this is more type-safe.
        let boolean_type = self.boolean_type();
        self.check_type_assignable_to(
            return_type,
            boolean_type,
            Some(right),
            Some(&AN_OBJECT_S_SYMBOL_HASINSTANCE_METHOD_MUST_RETURN_A_BOOLEAN_VALUE_FOR_IT_TO_BE_USED_ON_THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION),
        );
        boolean_type
    }

    /// Go: `func (c *Checker) checkInExpression(left, right, leftType, rightType) *Type`
    /// (checker.go:13321).
    pub(crate) fn check_in_expression(
        &mut self,
        left: NodeId,
        right: NodeId,
        left_type: TypeId,
        right_type: TypeId,
    ) -> TypeId {
        if left_type == self.silent_never_type() || right_type == self.silent_never_type() {
            return self.silent_never_type();
        }
        if self.store().node(left).kind == Kind::PrivateIdentifier {
            // TODO(port M6 W3/S10): the emit-helper gate on target/ decorators
            // (checkExternalEmitHelpers ClassPrivateFieldIn) — permissive
            // skip; the helper machinery is the emit wave's.
            // Unlike in 'checkPrivateIdentifierExpression' we now have access to the RHS type
            // which provides us with the opportunity to emit more detailed errors
            if self.symbol_node_links.get(left).resolved_symbol.is_none()
                && u::get_containing_class(self.store(), left).is_some()
            {
                let is_unchecked_js = u::is_unchecked_js_suggestion(
                    self,
                    left,
                    self.type_(right_type).symbol,
                    true, /*excludeClasses*/
                );
                self.report_nonexistent_property(left, right_type, is_unchecked_js);
            }
        } else {
            // The type of the left operand must be assignable to string, number, or symbol.
            // TODO(port M6 S7): stringNumberSymbolType = the string|number|symbol
            // union (checker.go's initializeChecker) — the assignability gate
            // is skipped until the union constructors land (a false positive
            // would error every legal `"k" in obj`).
            let non_null_left = self.check_non_null_type(left_type, left);
            let _ = non_null_left;
        }
        // The type of the right operand must be assignable to 'object'.
        let non_primitive_type = self.non_primitive_type();
        let non_null_right = self.check_non_null_type(right_type, right);
        if self.check_type_assignable_to(non_null_right, non_primitive_type, Some(right), None) {
            // The {} type is assignable to the object type, yet {} might represent a primitive type. Here we
            // detect and error on {} that results from narrowing the unknown type, as well as intersections
            // that include {} (we know that the other types in such intersections are assignable to object
            // since we already checked for that).
            if self.has_empty_object_intersection(right_type) {
                let right_str = self.type_to_string(right_type);
                self.error(
                    right,
                    &TYPE_0_MAY_REPRESENT_A_PRIMITIVE_VALUE_WHICH_IS_NOT_PERMITTED_AS_THE_RIGHT_OPERAND_OF_THE_IN_OPERATOR,
                    &[right_str],
                );
            }
        }
        // The result is always of the Boolean primitive type.
        self.boolean_type()
    }

    /// Go: `func (c *Checker) hasEmptyObjectIntersection(t) bool` (checker.go:13355).
    pub(crate) fn has_empty_object_intersection(&mut self, t: TypeId) -> bool {
        // TODO(port M6 S7): the `t == c.unknownEmptyObjectType` singleton
        // comparison (the field is the S7 initialization wave's); that arm
        // stays dormant. The intersection-with-{} arm is real below.
        let constituents = self.type_(t).distributed();
        constituents.iter().any(|&x| {
            let is_intersection = self.type_(x).flags.intersects(TypeFlags::INTERSECTION);
            if !is_intersection {
                return false;
            }
            let base = relater::get_base_constraint_or_type(self, x);
            self.is_empty_anonymous_object_type(base)
        })
    }

    /// Go: `func (c *Checker) getExactOptionalUnassignableProperties(source, target) []*ast.Symbol`
    /// (checker.go:13361). TODO(port M6 S8): the structured-member property
    /// walk (getPropertyOfType + getTypeOfSymbol per property); permissive
    /// empty (no unassignable properties at this depth).
    // (Called by S8's checkTypeAssignableTo exactOptionalPropertyTypes arm.)
    #[allow(dead_code)]
    pub(crate) fn get_exact_optional_unassignable_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Vec<SymbolId> {
        // TODO(port M6 S8): resolveStructuredTypeMembers plumbing.
        let _ = (source, target);
        Vec::new()
    }

    /// Go: `func (c *Checker) isExactOptionalPropertyMismatch(source, target) bool`
    /// (checker.go:13370).
    pub(crate) fn is_exact_optional_property_mismatch(
        &mut self,
        source: Option<TypeId>,
        target: Option<TypeId>,
    ) -> bool {
        match (source, target) {
            (Some(source), Some(target)) => {
                relater::maybe_type_of_kind(self, source, TypeFlags::UNDEFINED)
                    && self.contains_missing_type(target)
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) checkReferenceExpression(expr, invalidReferenceMessage, invalidOptionalChainMessage) bool`
    /// (checker.go:13374) — real body replacing statements.rs's permissive
    /// S4 carrier.
    pub(crate) fn check_reference_expression(
        &mut self,
        expr: NodeId,
        invalid_reference_message: &'static Message,
        invalid_optional_chain_message: &'static Message,
    ) -> bool {
        // References are combinations of identifiers, parentheses, and property accesses.
        let node = self.skip_outer_expressions_assertions_or_parens(expr);
        let is_access = tsc_ast::is_access_expression(self.store().node(node));
        if self.store().node(node).kind != Kind::Identifier && !is_access {
            self.error(expr, invalid_reference_message, &[]);
            return false;
        }
        if self.store().node(node).flags.intersects(NF::OPTIONAL_CHAIN) {
            self.error(expr, invalid_optional_chain_message, &[]);
            return false;
        }
        true
    }

    // ── S4's callee seams (bannered per the wave brief) ──

    /// Go: `func (c *Checker) allTypesAssignableToKind(source, kind) bool`
    /// (checker.go:28162).
    pub(crate) fn all_types_assignable_to_kind(&mut self, source: TypeId, kind: TypeFlags) -> bool {
        self.all_types_assignable_to_kind_ex(source, kind, false /*strict*/)
    }

    /// Go: `func (c *Checker) allTypesAssignableToKindEx(source, kind, strict) bool`
    /// (checker.go:28166). PORT: the checker-carrying everyType variant (the
    /// predicate needs `&mut Checker`; relater's free `every_type` takes a
    /// self-free closure).
    pub(crate) fn all_types_assignable_to_kind_ex(
        &mut self,
        source: TypeId,
        kind: TypeFlags,
        strict: bool,
    ) -> bool {
        let constituents = self.type_(source).distributed();
        constituents
            .iter()
            .all(|&t| self.is_type_assignable_to_kind_ex(t, kind, strict))
    }

    /// Go: `func (c *Checker) isTypeAssignableToKind(source, kind) bool`
    /// (checker.go:28175).
    pub(crate) fn is_type_assignable_to_kind(&mut self, source: TypeId, kind: TypeFlags) -> bool {
        self.is_type_assignable_to_kind_ex(source, kind, false /*strict*/)
    }

    /// Go: `func (c *Checker) isTypeAssignableToKindEx(source, kind, strict) bool`
    /// (checker.go:28179) — REAL body (intrinsic-only; the relater engine is
    /// real). PORT: the `c.booleanType` arm hits the crate's pinned S7
    /// booleanType panic for BooleanLike-kind queries — no current caller
    /// asks for BooleanLike (S4's plus arm is strict Number/BigInt/String).
    pub(crate) fn is_type_assignable_to_kind_ex(
        &mut self,
        source: TypeId,
        kind: TypeFlags,
        strict: bool,
    ) -> bool {
        if self.type_(source).flags.intersects(kind) {
            return true;
        }
        if strict
            && self
                .type_(source)
                .flags
                .intersects(
                    TypeFlags::ANY_OR_UNKNOWN
                        .union(TypeFlags::VOID)
                        .union(TypeFlags::UNDEFINED)
                        .union(TypeFlags::NULL),
                )
        {
            return false;
        }
        (kind.intersects(TypeFlags::NUMBER_LIKE) && {
            let number_type = self.number_type();
            self.is_type_assignable_to(source, number_type)
        })
            || (kind.intersects(TypeFlags::BIGINT_LIKE) && {
                let bigint_type = self.bigint_type();
                self.is_type_assignable_to(source, bigint_type)
            })
            || (kind.intersects(TypeFlags::STRING_LIKE) && {
                let string_type = self.string_type();
                self.is_type_assignable_to(source, string_type)
            })
            || (kind.intersects(TypeFlags::BOOLEAN_LIKE) && {
                let boolean_type = self.boolean_type();
                self.is_type_assignable_to(source, boolean_type)
            })
            || (kind.intersects(TypeFlags::VOID) && {
                let void_type = self.void_type();
                self.is_type_assignable_to(source, void_type)
            })
            || (kind.intersects(TypeFlags::NEVER) && {
                let never_type = self.never_type();
                self.is_type_assignable_to(source, never_type)
            })
            || (kind.intersects(TypeFlags::NULL) && {
                let null_type = self.null_type();
                self.is_type_assignable_to(source, null_type)
            })
            || (kind.intersects(TypeFlags::UNDEFINED) && {
                let undefined_type = self.undefined_type();
                self.is_type_assignable_to(source, undefined_type)
            })
            || (kind.intersects(TypeFlags::ESSYMBOL) && {
                let es_symbol_type = self.es_symbol_type();
                self.is_type_assignable_to(source, es_symbol_type)
            })
            || (kind.intersects(TypeFlags::NON_PRIMITIVE) && {
                let non_primitive_type = self.non_primitive_type();
                self.is_type_assignable_to(source, non_primitive_type)
            })
    }

    /// Go: `func (c *Checker) containsMissingType(t) bool` (checker.go:28388's
    /// pair — the missingType singleton / union-constituent probe). REAL
    /// body (the missing type is the intrinsic singleton).
    pub(crate) fn contains_missing_type(&mut self, t: TypeId) -> bool {
        let missing = self.missing_type_id();
        if t == missing {
            return true;
        }
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            return self.type_(t).types().contains(&missing);
        }
        false
    }

    /// Go: the `numberOrBigIntType` field (checker.go's initializeChecker —
    /// `getUnionType([numberType, bigintType])`). TODO(port M6 S7): the union
    /// constructor; None = the seam (callers skip their assignability gates,
    /// permissive no-error).
    pub(crate) fn number_or_big_int_type(&mut self) -> Option<TypeId> {
        // TODO(port M6 S7): numberOrBigIntType = getUnionType([number, bigint]).
        None
    }

    /// Go: `func (c *Checker) getResolvedSignature(node, candidatesOutArray, checkMode) *Signature`
    /// (checker.go:8660 — S6). Permissive None (the OrElse errorType arms
    /// compose Go's errorSignature shape at this depth).
    pub(crate) fn get_resolved_signature_stub(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<crate::types::SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<crate::types::SignatureId> {
        // TODO(port M6 S6): resolveSignature/resolveCall/chooseOverload.
        let _ = (node, candidates_out_array, check_mode);
        None
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Small expression-tree helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `ast.SkipOuterExpressions(node, ast.OEKAll)` — the checker.go call
/// sites' outer-expression skip.
impl Checker {
    pub(crate) fn skip_outer_expressions_all(&self, node: NodeId) -> NodeId {
        let mut current = node;
        while is_outer_expression_kind(self.store().node(current).kind) {
            current = crate::ast_shims::expression_of(self.store(), current)
                .expect("outer expression has an expression");
        }
        current
    }

    /// Go: `ast.SkipOuterExpressions(expr, ast.OEKAssertions|ast.OEKParentheses)`
    /// (checkReferenceExpression).
    pub(crate) fn skip_outer_expressions_assertions_or_parens(&self, node: NodeId) -> NodeId {
        let mut current = node;
        loop {
            let kind = self.store().node(current).kind;
            let is_assertion = matches!(
                kind,
                Kind::TypeAssertionExpression | Kind::AsExpression | Kind::SatisfiesExpression
            );
            if !is_assertion && kind != Kind::ParenthesizedExpression {
                return current;
            }
            current = crate::ast_shims::expression_of(self.store(), current)
                .expect("outer expression has an expression");
        }
    }
}

/// Go: `ast.IsOuterExpression(node)` for the OEKAll set used by
/// getSyntacticTruthySemantics/getSyntacticNullishnessSemantics
/// (parenthesized + assertion + satisfies + non-null outer expressions).
fn is_outer_expression_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::ParenthesizedExpression
            | Kind::TypeAssertionExpression
            | Kind::AsExpression
            | Kind::SatisfiesExpression
            | Kind::NonNullExpression
    )
}

/// Go: `func IsDeclarationNode(node *Node) bool` (ast.go:1515) —
/// `node.DeclarationData() != nil`. PORT: Go's data-pointer probe becomes a
/// Kind membership over the DeclarationBase-embedding node types
/// (ast_generated.go's embed list).
fn is_declaration_node(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::VariableDeclaration
            | Kind::Parameter
            | Kind::BindingElement
            | Kind::MissingDeclaration
            | Kind::FunctionDeclaration
            | Kind::ClassDeclaration
            | Kind::ClassExpression
            | Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::EnumMember
            | Kind::EnumDeclaration
            | Kind::NotEmittedTypeElement
            | Kind::ImportDeclaration
            | Kind::NamespaceImport
            | Kind::ExportAssignment
            | Kind::NamespaceExportDeclaration
            | Kind::NamespaceExport
            | Kind::ExportSpecifier
            | Kind::CallSignature
            | Kind::ConstructSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::IndexSignature
            | Kind::MethodSignature
            | Kind::MethodDeclaration
            | Kind::PropertySignature
            | Kind::PropertyDeclaration
            | Kind::SemicolonClassElement
            | Kind::ClassStaticBlockDeclaration
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::BinaryExpression
            | Kind::ArrowFunction
            | Kind::FunctionExpression
            | Kind::CallExpression
            | Kind::ObjectLiteralExpression
            | Kind::SpreadAssignment
            | Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::MappedType
            | Kind::TypeLiteral
            | Kind::NamedTupleMember
            | Kind::JsxAttributes
            | Kind::JsxAttribute
            | Kind::JsxSpreadAttribute
            | Kind::JSDocSignature
            | Kind::ModuleDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ExportDeclaration
            | Kind::ImportClause
            | Kind::ImportSpecifier
            | Kind::TypeParameter
            | Kind::JSDocTypeLiteral
    )
}

/// Go: `node.Name().Text()` on the PropertyAccessExpression left of the
/// exactOptionalPropertyTypes arm of checkAssignmentOperator.
fn left_name_of(c: &Checker, left: NodeId) -> NodeId {
    c.store()
        .node(left)
        .as_property_access_expression()
        .expect("PropertyAccessExpression data")
        .name
}

// ────────────────────────────────────────────────────────────────────────────
// The assertion / binary / destructuring family (Go checker.go:12526-12960)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkAssertion(node, checkMode) *Type` (checker.go:12526).
    pub(crate) fn check_assertion(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        if self.store().node(node).kind == Kind::TypeAssertionExpression {
            // TODO(port M6 W3): the mts/cts extension-reserved grammar error
            // (grammarchecks.go's file-extension probe) — permissive skip.
            let _ = node;
        }
        let type_node = crate::ast_shims::type_of_node(self.store(), node)
            .expect("assertion has a type node");
        let expr_type = {
            let expression = crate::ast_shims::expression_of(self.store(), node)
                .expect("assertion has an expression");
            self.check_expression_ex(expression, check_mode)
        };
        // Always check the type node so its identifiers are resolved. resolveName knows not
        // to resolve (or report an error for) the `const` in a `const` assertion, so this is
        // safe even for `x as const` and keeps diagnostics stable regardless of traversal order.
        self.check_source_element(Some(type_node));
        if u::is_const_type_reference(self.store(), type_node) {
            let expression = crate::ast_shims::expression_of(self.store(), node)
                .expect("assertion has an expression");
            if !self.is_valid_const_assertion_argument(expression) {
                self.error(
                    expression,
                    &A_CONST_ASSERTION_CAN_ONLY_BE_APPLIED_TO_REFERENCES_TO_ENUM_MEMBERS_OR_STRING_NUMBER_BOOLEAN_ARRAY_OR_OBJECT_LITERALS,
                    &[],
                );
            }
            return self.get_regular_type_of_literal_type(expr_type);
        }
        self.assertion_links.get(node).expr_type = Some(expr_type);
        self.check_node_deferred(node);
        self.get_type_from_type_node(type_node)
    }

    /// Go: `func (c *Checker) checkAssertionDeferred(node *ast.Node)` (checker.go:12554).
    // (Wired by checkDeferredNode's S4 arms.)
    #[allow(dead_code)]
    pub(crate) fn check_assertion_deferred(&mut self, node: NodeId) {
        let type_node = crate::ast_shims::type_of_node(self.store(), node)
            .expect("assertion has a type node");
        let expr_type = self
            .assertion_links
            .get(node)
            .expr_type
            .expect("checkAssertion ran first");
        let expr_type = relater::get_regular_type_of_object_literal(
            self,
            self.get_base_type_of_literal_type(expr_type),
        );
        let target_type = self.get_type_from_type_node(type_node);
        if !relater::is_error_type(self, target_type) {
            let widened_type = self.get_widened_type(expr_type);
            if !self.check_type_comparable_to(target_type, widened_type, None, None) {
                let err_node = node;
                // PORT: Go checks the Reparsed flag on the type node (the
                // JSDoc reparse seam — W3); the port checks the plain node.
                let _ = type_node;
                self.check_type_comparable_to(
                    expr_type,
                    target_type,
                    Some(err_node),
                    Some(&CONVERSION_OF_TYPE_0_TO_TYPE_1_MAY_BE_A_MISTAKE_BECAUSE_NEITHER_TYPE_SUFFICIENTLY_OVERLAPS_WITH_THE_OTHER_IF_THIS_WAS_INTENTIONAL_CONVERT_THE_EXPRESSION_TO_UNKNOWN_FIRST),
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkBinaryExpression(node, checkMode) *Type`
    /// (checker.go:12570).
    pub(crate) fn check_binary_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let (left, operator_token, right) = {
            let d = self
                .store()
                .node(node)
                .as_binary_expression()
                .expect("BinaryExpression data");
            (d.left, d.operator_token, d.right)
        };
        self.check_binary_like_expression(left, operator_token, right, check_mode, Some(node))
    }

    /// Go: `func (c *Checker) checkBinaryLikeExpression(left, operatorToken, right, checkMode, errorNode) *Type`
    /// (checker.go:12575).
    pub(crate) fn check_binary_like_expression(
        &mut self,
        left: NodeId,
        operator_token: NodeId,
        right: NodeId,
        check_mode: CheckMode,
        error_node: Option<NodeId>,
    ) -> TypeId {
        let operator = self.store().node(operator_token).kind;
        let left_kind = self.store().node(left).kind;
        let right_kind = self.store().node(right).kind;
        if operator == Kind::EqualsToken
            && (left_kind == Kind::ObjectLiteralExpression || left_kind == Kind::ArrayLiteralExpression)
        {
            let right_is_this = right_kind == Kind::ThisKeyword;
            let source_type = self.check_expression_ex(right, check_mode);
            return self.check_destructuring_assignment(left, source_type, check_mode, right_is_this);
        }
        let mut left_type = self.check_expression_ex(left, check_mode);
        let mut right_type = self.check_expression_ex(right, check_mode);
        if tsc_binder::ast_shims::is_logical_or_coalescing_binary_operator(operator) {
            let mut parent = self
                .store()
                .node(self.store().node(left).parent.get())
                .parent
                .get();
            while matches!(
                self.store().node(parent).kind,
                Kind::ParenthesizedExpression
            ) || tsc_binder::ast_shims::is_logical_or_coalescing_binary_expression(self.store(), parent)
            {
                parent = self.store().node(parent).parent.get();
            }
            if operator == Kind::AmpersandAmpersandToken || self.store().node(parent).kind == Kind::IfStatement
            {
                let mut body = None;
                if self.store().node(parent).kind == Kind::IfStatement {
                    body = Some(
                        self.store()
                            .node(parent)
                            .as_if_statement()
                            .expect("IfStatement data")
                            .then_statement,
                    );
                }
                self.check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(left, left_type, body);
            }
            if tsc_binder::ast_shims::is_logical_binary_operator(operator) {
                self.check_truthiness_of_type(left_type, left);
            }
        }
        match operator {
            Kind::AsteriskToken
            | Kind::AsteriskAsteriskToken
            | Kind::AsteriskEqualsToken
            | Kind::AsteriskAsteriskEqualsToken
            | Kind::SlashToken
            | Kind::SlashEqualsToken
            | Kind::PercentToken
            | Kind::PercentEqualsToken
            | Kind::MinusToken
            | Kind::MinusEqualsToken
            | Kind::LessThanLessThanToken
            | Kind::LessThanLessThanEqualsToken
            | Kind::GreaterThanGreaterThanToken
            | Kind::GreaterThanGreaterThanEqualsToken
            | Kind::GreaterThanGreaterThanGreaterThanToken
            | Kind::GreaterThanGreaterThanGreaterThanEqualsToken
            | Kind::BarToken
            | Kind::BarEqualsToken
            | Kind::CaretToken
            | Kind::CaretEqualsToken
            | Kind::AmpersandToken
            | Kind::AmpersandEqualsToken => {
                if left_type == self.silent_never_type() || right_type == self.silent_never_type() {
                    return self.silent_never_type();
                }
                left_type = self.check_non_null_type(left_type, left);
                right_type = self.check_non_null_type(right_type, right);
                // if a user tries to apply a bitwise operator to 2 boolean operands
                // try and return them a helpful suggestion
                if self.type_(left_type).flags.intersects(TypeFlags::BOOLEAN_LIKE)
                    && self.type_(right_type).flags.intersects(TypeFlags::BOOLEAN_LIKE)
                {
                    let suggested_operator = self.get_suggested_boolean_operator(operator);
                    if suggested_operator != Kind::Unknown {
                        self.error(
                            operator_token,
                            &THE_0_OPERATOR_IS_NOT_ALLOWED_FOR_BOOLEAN_TYPES_CONSIDER_USING_1_INSTEAD,
                            &[
                                token_to_string(operator).to_string(),
                                token_to_string(suggested_operator).to_string(),
                            ],
                        );
                        return self.number_type();
                    }
                }
                // otherwise just check each operand separately and report errors as normal
                let left_ok = self.check_arithmetic_operand_type(
                    left,
                    left_type,
                    &THE_LEFT_HAND_SIDE_OF_AN_ARITHMETIC_OPERATION_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
                    true, /*isAwaitValid*/
                );
                let right_ok = self.check_arithmetic_operand_type(
                    right,
                    right_type,
                    &THE_RIGHT_HAND_SIDE_OF_AN_ARITHMETIC_OPERATION_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
                    true, /*isAwaitValid*/
                );
                // If both are any or unknown, allow operation; assume it will resolve to number
                let result_type = if (self.is_type_assignable_to_kind(left_type, TypeFlags::ANY_OR_UNKNOWN)
                    && self.is_type_assignable_to_kind(right_type, TypeFlags::ANY_OR_UNKNOWN))
                    || (!relater::maybe_type_of_kind(self, left_type, TypeFlags::BIGINT_LIKE)
                        && !relater::maybe_type_of_kind(self, right_type, TypeFlags::BIGINT_LIKE))
                {
                    self.number_type()
                } else if self.both_are_big_int_like(left_type, right_type) {
                    match operator {
                        Kind::GreaterThanGreaterThanGreaterThanToken
                        | Kind::GreaterThanGreaterThanGreaterThanEqualsToken => {
                            self.report_operator_error(left_type, operator, right_type, error_node, None);
                        }
                        Kind::AsteriskAsteriskToken | Kind::AsteriskAsteriskEqualsToken
                            if self.language_version < tsc_core::options_generated::ScriptTarget::ES2016 =>
                        {
                            self.error(
                                error_node.expect("non-nil errorNode on this path"),
                                &EXPONENTIATION_CANNOT_BE_PERFORMED_ON_BIGINT_VALUES_UNLESS_THE_TARGET_OPTION_IS_SET_TO_ES2016_OR_LATER,
                                &[],
                            );
                        }
                        _ => {}
                    }
                    self.bigint_type()
                } else {
                    self.report_operator_error(left_type, operator, right_type, error_node, Some(Checker::both_are_big_int_like));
                    self.error_type()
                };
                if left_ok && right_ok {
                    self.check_assignment_operator(left, operator, right, left_type, result_type);
                    match operator {
                        Kind::LessThanLessThanToken
                        | Kind::LessThanLessThanEqualsToken
                        | Kind::GreaterThanGreaterThanToken
                        | Kind::GreaterThanGreaterThanEqualsToken
                        | Kind::GreaterThanGreaterThanGreaterThanToken
                        | Kind::GreaterThanGreaterThanGreaterThanEqualsToken => {
                            // TODO(port M6 W3): the evaluate() constant fold
                            // (the shift-simplification suggestion needs the
                            // numeric evaluator — grammarchecks-adjacent);
                            // permissive skip.
                            let _ = right;
                        }
                        _ => {}
                    }
                }
                result_type
            }
            Kind::PlusToken | Kind::PlusEqualsToken => {
                if left_type == self.silent_never_type() || right_type == self.silent_never_type() {
                    return self.silent_never_type();
                }
                if !self.is_type_assignable_to_kind(left_type, TypeFlags::STRING_LIKE)
                    && !self.is_type_assignable_to_kind(right_type, TypeFlags::STRING_LIKE)
                {
                    left_type = self.check_non_null_type(left_type, left);
                    right_type = self.check_non_null_type(right_type, right);
                }
                let mut result_type: Option<TypeId> = None;
                if self.is_type_assignable_to_kind_ex(left_type, TypeFlags::NUMBER_LIKE, true /*strict*/)
                    && self.is_type_assignable_to_kind_ex(right_type, TypeFlags::NUMBER_LIKE, true /*strict*/)
                {
                    // Operands of an enum type are treated as having the primitive type Number.
                    // If both operands are of the Number primitive type, the result is of the Number primitive type.
                    result_type = Some(self.number_type());
                } else if self.is_type_assignable_to_kind_ex(left_type, TypeFlags::BIGINT_LIKE, true /*strict*/)
                    && self.is_type_assignable_to_kind_ex(right_type, TypeFlags::BIGINT_LIKE, true /*strict*/)
                {
                    // If both operands are of the BigInt primitive type, the result is of the BigInt primitive type.
                    result_type = Some(self.bigint_type());
                } else if self.is_type_assignable_to_kind_ex(left_type, TypeFlags::STRING_LIKE, true /*strict*/)
                    || self.is_type_assignable_to_kind_ex(right_type, TypeFlags::STRING_LIKE, true /*strict*/)
                {
                    // If one or both operands are of the String primitive type, the result is of the String primitive type.
                    result_type = Some(self.string_type());
                } else if u::is_type_any(Some(self.type_(left_type)))
                    || u::is_type_any(Some(self.type_(right_type)))
                {
                    // Otherwise, the result is of type Any.
                    // NOTE: unknown type here denotes error type. Old compiler treated this case as any type so do we.
                    if relater::is_error_type(self, left_type) || relater::is_error_type(self, right_type) {
                        result_type = Some(self.error_type());
                    } else {
                        result_type = Some(self.any_type());
                    }
                }
                // Symbols are not allowed at all in arithmetic expressions
                if let Some(result) = result_type
                    && self.check_for_disallowed_es_symbol_operand(left, right, left_type, right_type, operator)
                {
                    return result;
                }
                if result_type.is_none() {
                    // Types that have a reasonably good chance of being a valid operand type.
                    // If both types have an awaited type of one of these, we'll assume the user
                    // might be missing an await without doing an exhaustive check that inserting
                    // await(s) will actually be a completely valid binary expression.
                    // (Go's closeEnoughKind lives inside the operand-kind closure —
                    // close_enough_kind_is_related recomputes it, the PORT note in
                    // report_operator_error.)
                    self.report_operator_error(
                        left_type,
                        operator,
                        right_type,
                        error_node,
                        Some(close_enough_kind_is_related),
                    );
                    return self.any_type();
                }
                if operator == Kind::PlusEqualsToken {
                    self.check_assignment_operator(left, operator, right, left_type, result_type.expect("checked"));
                }
                result_type.expect("checked")
            }
            Kind::LessThanToken
            | Kind::GreaterThanToken
            | Kind::LessThanEqualsToken
            | Kind::GreaterThanEqualsToken => {
                if self.check_for_disallowed_es_symbol_operand(left, right, left_type, right_type, operator) {
                    let left_non_null = self.check_non_null_type(left_type, left);
                    let right_non_null = self.check_non_null_type(right_type, right);
                    left_type = self.get_base_type_of_literal_type_for_comparison(left_non_null);
                    right_type = self.get_base_type_of_literal_type_for_comparison(right_non_null);
                    self.report_operator_error_unless(left_type, operator, right_type, error_node, relational_types_are_compatible);
                }
                self.boolean_type()
            }
            Kind::EqualsEqualsToken
            | Kind::ExclamationEqualsToken
            | Kind::EqualsEqualsEqualsToken
            | Kind::ExclamationEqualsEqualsToken => {
                // We suppress errors in CheckMode.TypeOnly (meaning the invocation came from getTypeOfExpression). During
                // control flow analysis it is possible for operands to temporarily have narrower types, and those narrower
                // types may cause the operands to not be comparable. We don't want such errors reported (see #46475).
                if !check_mode.intersects(CheckMode::TYPE_ONLY) {
                    if (u::is_literal_expression_of_object(self.store().node(left))
                        || u::is_literal_expression_of_object(self.store().node(right)))
                        // only report for === and !== in JS, not == or !=
                        && (!tsc_ast::utilities::is_in_js_file(self.store().node(left))
                            || operator == Kind::EqualsEqualsEqualsToken
                            || operator == Kind::ExclamationEqualsEqualsToken)
                    {
                        let eq_type = operator == Kind::EqualsEqualsToken || operator == Kind::EqualsEqualsEqualsToken;
                        let Some(error_node) = error_node else { unreachable!("equality arm with nil errorNode") };
                        self.error(
                            error_node,
                            &THIS_CONDITION_WILL_ALWAYS_RETURN_0_SINCE_JAVASCRIPT_COMPARES_OBJECTS_BY_REFERENCE_NOT_VALUE,
                            &[if eq_type { "false".to_string() } else { "true".to_string() }],
                        );
                    }
                    if let Some(error_node) = error_node {
                        self.check_nan_equality(error_node, operator, left, right);
                    }
                    self.report_operator_error_unless(left_type, operator, right_type, error_node, equality_types_are_compatible);
                }
                self.boolean_type()
            }
            Kind::InstanceOfKeyword => self.check_instance_of_expression(left, right, left_type, right_type, check_mode),
            Kind::InKeyword => self.check_in_expression(left, right, left_type, right_type),
            Kind::AmpersandAmpersandToken | Kind::AmpersandAmpersandEqualsToken => {
                let mut result_type = left_type;
                if self.has_type_facts(left_type, TypeFacts::TRUTHY) {
                    let t = if !self.strict_null_checks {
                        self.get_base_type_of_literal_type(right_type)
                    } else {
                        left_type
                    };
                    // TODO(port M6 S10): extractDefinitelyFalsyTypes — the
                    // facts-lattice extract; identity until then.
                    let extracted = t;
                    result_type = relater::get_union_type(self, vec![extracted, right_type]);
                }
                if operator == Kind::AmpersandAmpersandEqualsToken {
                    self.check_assignment_operator(left, operator, right, left_type, right_type);
                }
                result_type
            }
            Kind::BarBarToken | Kind::BarBarEqualsToken => {
                let mut result_type = left_type;
                if self.has_type_facts(left_type, TypeFacts::FALSY) {
                    // TODO(port M6 S10): removeDefinitelyFalsyTypes — identity
                    // until the facts lattice lands.
                    let non_falsy = self.get_non_nullable_type(left_type);
                    result_type = self.get_union_type_ex(
                        &[non_falsy, right_type],
                        crate::todo_shapes::UnionReduction::Subtype,
                    );
                }
                if operator == Kind::BarBarEqualsToken {
                    self.check_assignment_operator(left, operator, right, left_type, right_type);
                }
                result_type
            }
            Kind::QuestionQuestionToken | Kind::QuestionQuestionEqualsToken => {
                if operator == Kind::QuestionQuestionToken {
                    self.check_nullish_coalesce_operands(left, right);
                }
                let mut result_type = left_type;
                if self.has_type_facts(left_type, TypeFacts::EQ_UNDEFINED_OR_NULL) {
                    let left_non_nullable = self.get_non_nullable_type(left_type);
                    result_type = self.get_union_type_ex(
                        &[left_non_nullable, right_type],
                        crate::todo_shapes::UnionReduction::Subtype,
                    );
                }
                if operator == Kind::QuestionQuestionEqualsToken {
                    self.check_assignment_operator(left, operator, right, left_type, right_type);
                }
                result_type
            }
            Kind::EqualsToken => {
                self.check_assignment_operator(left, operator, right, left_type, right_type);
                right_type
            }
            Kind::CommaToken => {
                let allow_unreachable = self.compiler_options.allow_unreachable_code == Tristate::TSTrue;
                if !allow_unreachable
                    && self.is_side_effect_free(left)
                    && !self.is_indirect_call(self.store().node(left).parent.get())
                {
                    self.error(
                        left,
                        &LEFT_SIDE_OF_COMMA_OPERATOR_IS_UNUSED_AND_HAS_NO_SIDE_EFFECTS,
                        &[],
                    );
                }
                right_type
            }
            _ => panic!("Unhandled case in checkBinaryLikeExpression"),
        }
    }

    // ── Destructuring assignments (Go checker.go:12791-12960) ──

    /// Go: `func (c *Checker) checkDestructuringAssignment(node, sourceType, checkMode, rightIsThis) *Type`
    /// (checker.go:12791).
    pub(crate) fn check_destructuring_assignment(
        &mut self,
        node: NodeId,
        mut source_type: TypeId,
        check_mode: CheckMode,
        right_is_this: bool,
    ) -> TypeId {
        let node_kind = self.store().node(node).kind;
        let mut target: NodeId;
        if node_kind == Kind::ShorthandPropertyAssignment {
            let (name, equals_token, object_assignment_initializer) = {
                let d = self
                    .store()
                    .node(node)
                    .as_shorthand_property_assignment()
                    .expect("ShorthandPropertyAssignment data");
                (d.name, d.equals_token, d.object_assignment_initializer)
            };
            if let Some(initializer) = object_assignment_initializer {
                // In strict null checking mode, if a default value of a non-undefined type is specified, remove
                // undefined from the final type.
                if self.strict_null_checks {
                    let initializer_type = self.check_expression(initializer);
                    if !self.has_type_facts(initializer_type, TypeFacts::IS_UNDEFINED) {
                        source_type =
                            relater::get_type_with_facts(self, source_type, TypeFacts::NE_UNDEFINED);
                    }
                }
                self.check_binary_like_expression(
                    name,
                    equals_token.expect("ShorthandPropertyAssignment initializer carries '='"),
                    initializer,
                    check_mode,
                    None,
                );
            }
            target = name;
        } else {
            target = node;
        }
        if self.store().node(target).kind == Kind::BinaryExpression {
            self.check_binary_expression(target, check_mode);
            target = self
                .store()
                .node(target)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .left;
            // A default value is specified, so remove undefined from the final type.
            if self.strict_null_checks {
                source_type = relater::get_type_with_facts(self, source_type, TypeFacts::NE_UNDEFINED);
            }
        }
        let target_kind = self.store().node(target).kind;
        if target_kind == Kind::ObjectLiteralExpression {
            return self.check_object_literal_assignment(target, source_type, right_is_this);
        }
        if target_kind == Kind::ArrayLiteralExpression {
            return self.check_array_literal_assignment(target, source_type, check_mode);
        }
        self.check_reference_assignment(target, source_type, check_mode)
    }

    /// Go: `func (c *Checker) checkObjectLiteralAssignment(node, sourceType, rightIsThis) *Type`
    /// (checker.go:12824).
    pub(crate) fn check_object_literal_assignment(
        &mut self,
        node: NodeId,
        source_type: TypeId,
        right_is_this: bool,
    ) -> TypeId {
        let properties: Option<Vec<NodeId>> = self
            .store()
            .node(node)
            .as_object_literal_expression()
            .expect("ObjectLiteralExpression data")
            .properties
            .as_ref()
            .map(|l| l.nodes.to_vec());
        let property_count = properties.as_ref().map(|p| p.len()).unwrap_or(0);
        if self.strict_null_checks && property_count == 0 {
            return self.check_non_null_type(source_type, node);
        }
        if let Some(properties) = properties {
            for i in 0..property_count {
                self.check_object_literal_destructuring_property_assignment(
                    node,
                    source_type,
                    i,
                    Some(&properties),
                    right_is_this,
                );
            }
        }
        source_type
    }

    /// Go: `func (c *Checker) checkObjectLiteralDestructuringPropertyAssignment(node, objectLiteralType, propertyIndex, allProperties, rightIsThis) *Type`
    /// (checker.go:12836). Note: If property cannot be a SpreadAssignment, then allProperties does not need to be provided.
    pub(crate) fn check_object_literal_destructuring_property_assignment(
        &mut self,
        node: NodeId,
        object_literal_type: TypeId,
        property_index: usize,
        all_properties: Option<&[NodeId]>,
        right_is_this: bool,
    ) -> Option<TypeId> {
        let properties: Vec<NodeId> = self
            .store()
            .node(node)
            .as_object_literal_expression()
            .expect("ObjectLiteralExpression data")
            .properties
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        let property = properties[property_index];
        let property_kind = self.store().node(property).kind;
        if property_kind == Kind::PropertyAssignment || property_kind == Kind::ShorthandPropertyAssignment {
            let name = self
                .store()
                .node(property)
                .name()
                .expect("property has a name");
            if self.store().node(name).kind == Kind::PrivateIdentifier {
                self.grammar_error_on_node(
                    name,
                    &PRIVATE_IDENTIFIERS_CANNOT_BE_USED_IN_DESTRUCTURING_PATTERNS,
                    &[],
                );
            }
            let expr_type = self.get_literal_type_from_property_name(name);
            if u::is_type_usable_as_property_name(self.type_(expr_type)) {
                let text = u::get_property_name_from_type(self.type_(expr_type));
                if let Some(prop) = self.get_property_of_type(object_literal_type, &text) {
                    self.mark_property_as_referenced(prop, Some(property), right_is_this);
                    self.check_property_accessibility(
                        property,
                        false, /*isSuper*/
                        true, /*writing*/
                        object_literal_type,
                        prop,
                    );
                }
            }
            let allow_missing = self.has_default_value(property);
            let element_type = self.get_indexed_access_type_ex(
                object_literal_type,
                expr_type,
                crate::types::AccessFlags::EXPRESSION_POSITION
                    | if allow_missing {
                        crate::types::AccessFlags::ALLOW_MISSING
                    } else {
                        crate::types::AccessFlags::NONE
                    },
                Some(name),
            );
            let t = self.get_flow_type_of_destructuring(property, element_type);
            let expr = if property_kind == Kind::PropertyAssignment {
                crate::ast_shims::initializer_of(self.store(), property)
                    .expect("PropertyAssignment has an initializer")
            } else {
                property
            };
            return Some(self.check_destructuring_assignment(expr, t, CheckMode::CHECK_MODE_NORMAL, false));
        }
        if property_kind == Kind::SpreadAssignment {
            if property_index < properties.len() - 1 {
                self.error(
                    property,
                    &A_REST_ELEMENT_MUST_BE_LAST_IN_A_DESTRUCTURING_PATTERN,
                    &[],
                );
                return None;
            }
            // TODO(port M6 W3/S10): the ObjectSpreadRest emit-helper check
            // (checkExternalEmitHelpers) — permissive skip (the helper
            // machinery is the emit wave's).
            let mut non_rest_names = Vec::new();
            if let Some(all_properties) = all_properties {
                for other_property in all_properties {
                    if self.store().node(*other_property).kind != Kind::SpreadAssignment {
                        non_rest_names.push(
                            self.store().node(*other_property).name()
                                .expect("property has a name"),
                        );
                    }
                }
            }
            let symbol = self.type_(object_literal_type).symbol;
            let t = self.get_rest_type(object_literal_type, &non_rest_names, symbol);
            self.check_grammar_for_disallowed_trailing_comma(
                all_properties,
                &A_REST_PARAMETER_OR_BINDING_PATTERN_MAY_NOT_HAVE_A_TRAILING_COMMA,
            );
            let expression = crate::ast_shims::expression_of(self.store(), property)
                .expect("SpreadAssignment has an expression");
            return Some(self.check_destructuring_assignment(expression, t, CheckMode::CHECK_MODE_NORMAL, false));
        }
        self.error(property, &PROPERTY_ASSIGNMENT_EXPECTED, &[]);
        None
    }

    /// Go: `func (c *Checker) checkArrayLiteralAssignment(node, sourceType, checkMode) *Type`
    /// (checker.go:12887).
    pub(crate) fn check_array_literal_assignment(
        &mut self,
        node: NodeId,
        source_type: TypeId,
        _check_mode: CheckMode,
    ) -> TypeId {
        let elements: Vec<NodeId> = self
            .store()
            .node(node)
            .as_array_literal_expression()
            .expect("ArrayLiteralExpression data")
            .elements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        // This elementType will be used if the specific property corresponding to this index is not
        // present (aka the tuple element property). This call also checks that the parentType is in
        // fact an iterable or array (depending on target language).
        let possibly_out_of_bounds_type = self
            .get_iterated_type_or_element_type(
                crate::statements::IterationUse::DESTRUCTURING
                    .union(crate::statements::IterationUse::POSSIBLY_OUT_OF_BOUNDS),
                source_type,
                self.undefined_type(),
                Some(node),
                true, /*checkAssignability*/
            )
            .unwrap_or_else(|| self.error_type());
        let no_unchecked_indexed_access = self.compiler_options.no_unchecked_indexed_access == Tristate::TSTrue;
        let mut in_bounds_type: Option<TypeId> = if no_unchecked_indexed_access {
            None
        } else {
            Some(possibly_out_of_bounds_type)
        };
        for (i, &element) in elements.iter().enumerate() {
            let mut t = possibly_out_of_bounds_type;
            if self.store().node(element).kind == Kind::SpreadElement {
                if in_bounds_type.is_none() {
                    in_bounds_type = Some(
                        self.get_iterated_type_or_element_type(
                            crate::statements::IterationUse::DESTRUCTURING,
                            source_type,
                            self.undefined_type(),
                            Some(node),
                            true, /*checkAssignability*/
                        )
                        .unwrap_or_else(|| self.error_type()),
                    );
                }
                t = in_bounds_type.expect("just set");
            }
            self.check_array_literal_destructuring_element_assignment(node, source_type, i, t, _check_mode);
        }
        source_type
    }

    /// Go: `func (c *Checker) checkArrayLiteralDestructuringElementAssignment(node, sourceType, elementIndex, elementType, checkMode) *Type`
    /// (checker.go:12907).
    pub(crate) fn check_array_literal_destructuring_element_assignment(
        &mut self,
        node: NodeId,
        source_type: TypeId,
        element_index: usize,
        element_type: TypeId,
        check_mode: CheckMode,
    ) -> Option<TypeId> {
        let elements: Vec<NodeId> = self
            .store()
            .node(node)
            .as_array_literal_expression()
            .expect("ArrayLiteralExpression data")
            .elements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        let element = elements[element_index];
        if self.store().node(element).kind != Kind::OmittedExpression {
            if self.store().node(element).kind != Kind::SpreadElement {
                let index_type = self.get_number_literal_type(tsc_jsnum::Number::from(element_index as f64));
                if self.is_array_like_type(source_type) {
                    // We create a synthetic expression so that getIndexedAccessType doesn't get confused
                    // when the element is a SyntaxKind.ElementAccessExpression.
                    // TODO(port M6 S10): createSyntheticExpression — the
                    // synthetic node factory; the element node stands in for
                    // the access node (error positions only).
                    let access_flags = crate::types::AccessFlags::EXPRESSION_POSITION
                        | if self.has_default_value(element) {
                            crate::types::AccessFlags::ALLOW_MISSING
                        } else {
                            crate::types::AccessFlags::NONE
                        };
                    let element_type = self
                        .get_indexed_access_type_or_undefined(
                            source_type,
                            index_type,
                            access_flags,
                            Some(element),
                            None,
                        )
                        .unwrap_or_else(|| self.error_type());
                    let mut assigned_type = element_type;
                    if self.has_default_value(element) {
                        assigned_type = relater::get_type_with_facts(self, element_type, TypeFacts::NE_UNDEFINED);
                    }
                    let t = self.get_flow_type_of_destructuring(element, assigned_type);
                    return Some(self.check_destructuring_assignment(element, t, check_mode, false));
                }
                return Some(self.check_destructuring_assignment(element, element_type, check_mode, false));
            }
            if element_index < elements.len() - 1 {
                self.error(
                    element,
                    &A_REST_ELEMENT_MUST_BE_LAST_IN_A_DESTRUCTURING_PATTERN,
                    &[],
                );
            } else {
                let rest_expression = crate::ast_shims::expression_of(self.store(), element)
                    .expect("SpreadElement has an expression");
                let (is_binary_equals, operator_token) = {
                    let n = self.store().node(rest_expression);
                    if n.kind == Kind::BinaryExpression {
                        let d = n.as_binary_expression().expect("BinaryExpression data");
                        (self.store().node(d.operator_token).kind == Kind::EqualsToken, d.operator_token)
                    } else {
                        (false, rest_expression)
                    }
                };
                if is_binary_equals {
                    self.error(operator_token, &A_REST_ELEMENT_CANNOT_HAVE_AN_INITIALIZER, &[]);
                } else {
                    self.check_grammar_for_disallowed_trailing_comma(
                        Some(elements.as_slice()),
                        &A_REST_PARAMETER_OR_BINDING_PATTERN_MAY_NOT_HAVE_A_TRAILING_COMMA,
                    );
                    let all_tuples = self
                        .type_(source_type)
                        .distributed()
                        .iter()
                        .all(|&t| self.is_tuple_type(t));
                    let t = if all_tuples {
                        // TODO(port M6 S7): mapType over the tuple union +
                        // sliceTupleType — permissive elementType (the tuple
                        // slicing is S7's mapped-type machinery).
                        element_type
                    } else {
                        self.create_array_type(element_type)
                    };
                    return Some(self.check_destructuring_assignment(rest_expression, t, check_mode, false));
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) checkReferenceAssignment(target, sourceType, checkMode) *Type`
    /// (checker.go:12948).
    pub(crate) fn check_reference_assignment(
        &mut self,
        target: NodeId,
        source_type: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let target_type = self.check_expression_ex(target, check_mode);
        let parent_kind = self.store().node(self.store().node(target).parent.get()).kind;
        let is_spread = parent_kind == Kind::SpreadAssignment;
        let message = if is_spread {
            &THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS
        } else {
            &THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS
        };
        let optional_message = if is_spread {
            &THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS
        } else {
            &THE_LEFT_HAND_SIDE_OF_AN_ASSIGNMENT_EXPRESSION_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS
        };
        if self.check_reference_expression(target, message, optional_message) {
            self.check_type_assignable_to_and_optionally_elaborate(
                source_type,
                target_type,
                Some(target),
                Some(target),
                None,
                None,
            );
        }
        source_type
    }

    /// Go: `func (c *Checker) getBaseTypeOfLiteralTypeForComparison(t) *Type`
    /// (checker.go:25988 — S7's literal-widening family, called only by
    /// S4's relational arms). TODO(port M6 S7): the fresh-literal-aware
    /// comparison variant; the plain widening value below is Go's
    /// getBaseTypeOfLiteralType answer (identical for non-fresh literals).
    pub(crate) fn get_base_type_of_literal_type_for_comparison(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S7): the comparison-flavored widening.
        self.get_base_type_of_literal_type(t)
    }
}

/// Go: the `closeEnoughKind` isRelated closure of checkBinaryLikeExpression's
/// `+` arm (checker.go:12691) — capture-free, so a plain fn.
fn close_enough_kind_is_related(c: &mut Checker, left: TypeId, right: TypeId) -> bool {
    let close_enough_kind = TypeFlags::NUMBER_LIKE
        .union(TypeFlags::BIGINT_LIKE)
        .union(TypeFlags::STRING_LIKE)
        .union(TypeFlags::ANY_OR_UNKNOWN);
    c.is_type_assignable_to_kind(left, close_enough_kind)
        && c.is_type_assignable_to_kind(right, close_enough_kind)
}

/// Go: the `typesAreCompatible` closure of the relational arms
/// (checker.go:12704).
fn relational_types_are_compatible(c: &mut Checker, left: TypeId, right: TypeId) -> bool {
    if u::is_type_any(Some(c.type_(left))) || u::is_type_any(Some(c.type_(right))) {
        return true;
    }
    let number_or_big_int_type = c.number_or_big_int_type();
    let left_assignable_to_number = number_or_big_int_type
        .is_some_and(|nbt| c.is_type_assignable_to(left, nbt));
    let right_assignable_to_number = number_or_big_int_type
        .is_some_and(|nbt| c.is_type_assignable_to(right, nbt));
    left_assignable_to_number && right_assignable_to_number
        || !left_assignable_to_number && !right_assignable_to_number && c.are_types_comparable(left, right)
}

/// Go: the `typesAreCompatible` closure of the equality arms
/// (checker.go:12726).
fn equality_types_are_compatible(c: &mut Checker, left: TypeId, right: TypeId) -> bool {
    c.is_type_equality_comparable_to(left, right) || c.is_type_equality_comparable_to(right, left)
}


// ────────────────────────────────────────────────────────────────────────────
// The unary / conditional / spread / yield leaves (Go checker.go:10848-11277)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkTypeOfExpression(node) *Type` (checker.go:10848).
    pub(crate) fn check_type_of_expression(&mut self, node: NodeId) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("TypeOfExpression has an expression");
        self.check_expression(expression);
        // TODO(port M6 S7): typeofType = the
        // "string|number|bigint|boolean|symbol|undefined|object|function"
        // union (initializeChecker); stringType stands in until the union
        // constructors land.
        self.string_type()
    }

    /// Go: `func (c *Checker) checkNonNullAssertion(node) *Type` (checker.go:10853).
    pub(crate) fn check_non_null_assertion(&mut self, node: NodeId) -> TypeId {
        if self.store().node(node).flags.intersects(NF::OPTIONAL_CHAIN) {
            // checkNonNullChain checks the same operand expression (node.Expression()),
            // so the child is still visited on this branch.
            return self.check_non_null_chain(node);
        }
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("NonNullExpression has an expression");
        let expr_type = self.check_expression(expression);
        self.get_non_nullable_type(expr_type)
    }

    /// Go: `func (c *Checker) checkNonNullChain(node) *Type` (checker.go:10862).
    pub(crate) fn check_non_null_chain(&mut self, node: NodeId) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("NonNullExpression has an expression");
        let left_type = self.check_expression(expression);
        let non_optional_type = self.get_optional_expression_type(left_type, expression);
        let non_nullable = self.get_non_nullable_type(non_optional_type);
        self.propagate_optional_type_marker(
            non_nullable,
            node,
            non_optional_type != left_type,
        )
    }

    /// Go: `func (c *Checker) getOptionalExpressionType(exprType, expression) *Type`
    /// (checker.go:29593 — S9). Permissive identity (the optional-marker
    /// propagation reads apparent types; identity keeps chains exact for
    /// non-optional operands).
    pub(crate) fn get_optional_expression_type(&mut self, expr_type: TypeId, expression: NodeId) -> TypeId {
        // TODO(port M6 S9): getOptionalExpressionTypeWorker.
        let _ = expression;
        expr_type
    }

    /// Go: `func (c *Checker) propagateOptionalTypeMarker(t, node, wasOptional) *Type`
    /// (checker.go:29611 — S9). Permissive pass-through.
    pub(crate) fn propagate_optional_type_marker(&mut self, t: TypeId, node: NodeId, was_optional: bool) -> TypeId {
        // TODO(port M6 S9): the jsxElementLinks/optionalTypeMarker write.
        let _ = (node, was_optional);
        t
    }

    /// Go: `func (c *Checker) checkExpressionWithTypeArguments(node) *Type` (checker.go:10868).
    pub(crate) fn check_expression_with_type_arguments(&mut self, node: NodeId) -> TypeId {
        self.check_grammar_expression_with_type_arguments(node);
        let type_arguments = self.store().node(node).as_expression_with_type_arguments().expect("ExpressionWithTypeArguments data").type_arguments.clone();
        if let Some(type_arguments) = type_arguments.as_ref() {
            self.check_source_elements(&type_arguments.nodes);
        }
        if self.store().node(node).kind == Kind::ExpressionWithTypeArguments {
            let parent = crate::ast_shims::walk_up_parenthesized_expressions(
                self.store(),
                self.store().node(node).parent.get(),
            );
            if let Some(parent) = parent {
                let (is_binary_instanceof, bin_right) = {
                    let n = self.store().node(parent);
                    if n.kind == Kind::BinaryExpression {
                        let d = n.as_binary_expression().expect("BinaryExpression data");
                        (self.store().node(d.operator_token).kind == Kind::InstanceOfKeyword, d.right)
                    } else {
                        (false, node)
                    }
                };
                if is_binary_instance_of_store(self, node, bin_right) && is_binary_instanceof {
                    self.error(
                        node,
                        &THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_NOT_BE_AN_INSTANTIATION_EXPRESSION,
                        &[],
                    );
                }
            }
        }
        let expr_type;
        if self.store().node(node).kind == Kind::ExpressionWithTypeArguments {
            let expression = crate::ast_shims::expression_of(self.store(), node)
                .expect("ExpressionWithTypeArguments has an expression");
            expr_type = self.check_expression(expression);
        } else {
            let expr_name = self
                .store()
                .node(node)
                .as_type_query_node()
                .expect("TypeQueryNode data")
                .expr_name;
            if tsc_ast::utilities::is_this_identifier(self.store(), expr_name) {
                expr_type = self.check_this_expression(expr_name);
            } else {
                expr_type = self.check_expression(expr_name);
            }
        }
        self.get_instantiation_expression_type(expr_type, node)
    }

    /// Go: `func (c *Checker) getInstantiationExpressionType(exprType, node) *Type`
    /// (checker.go:10891). TODO(port M6 S8): the instantiated-signature
    /// construction (resolveStructuredTypeMembers + getSignatureInstantiation +
    /// the newInstantiationExpressionType object); permissive identity (with
    /// the seam, no signature data exists yet, so the TS2743-style
    /// "no signatures" diagnostic must not fire — it would be a false error
    /// for every legal `f<T>`).
    pub(crate) fn get_instantiation_expression_type(&mut self, expr_type: TypeId, node: NodeId) -> TypeId {
        // TODO(port M6 S8/S7): the signature-instantiation walk.
        let _ = node;
        expr_type
    }

    /// Go: `func (c *Checker) checkGrammarExpressionWithTypeArguments(node) bool`
    /// (grammarchecks.go). TODO(port M6 W3): permissive false (no error).
    pub(crate) fn check_grammar_expression_with_type_arguments(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkSatisfiesExpression(node) *Type` (checker.go:10975).
    pub(crate) fn check_satisfies_expression(&mut self, node: NodeId) -> TypeId {
        let type_node = crate::ast_shims::type_of_node(self.store(), node)
            .expect("SatisfiesExpression has a type");
        self.check_source_element(Some(type_node));
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("SatisfiesExpression has an expression");
        let expr_type = self.check_expression(expression);
        let target_type = self.get_type_from_type_node(type_node);
        if relater::is_error_type(self, target_type) {
            return target_type;
        }
        self.check_type_assignable_to_and_optionally_elaborate(
            expr_type,
            target_type,
            Some(node),
            Some(expression),
            Some(&TYPE_0_DOES_NOT_SATISFY_THE_EXPECTED_TYPE_1),
            None,
        );
        expr_type
    }

    /// Go: `func (c *Checker) checkMetaProperty(node) *Type` (checker.go:10987).
    pub(crate) fn check_meta_property(&mut self, node: NodeId) -> TypeId {
        let keyword_token = self
            .store()
            .node(node)
            .as_meta_property()
            .expect("MetaProperty data")
            .keyword_token;
        match keyword_token {
            Kind::NewKeyword => self.check_new_target_meta_property(node),
            Kind::ImportKeyword => {
                if is_import_phase_meta_property(self.store(), node) {
                    // (Go asserts the parent-call shape; the phase forms are
                    // S6's import-attribute resolution.)
                    return self.error_type();
                }
                self.check_import_meta_property(node)
            }
            _ => panic!("Unhandled case in checkMetaProperty"),
        }
    }

    /// Go: `func (c *Checker) checkNewTargetMetaProperty(node) *Type` (checker.go:11004).
    pub(crate) fn check_new_target_meta_property(&mut self, node: NodeId) -> TypeId {
        let container = get_new_target_container(self, node);
        let Some(container) = container else {
            self.error(
                node,
                &META_PROPERTY_0_IS_ONLY_ALLOWED_IN_THE_BODY_OF_A_FUNCTION_DECLARATION_FUNCTION_EXPRESSION_OR_CONSTRUCTOR,
                &["new.target".to_string()],
            );
            return self.error_type();
        };
        let symbol = if self.store().node(container).kind == Kind::Constructor {
            let parent = self.store().node(container).parent.get();
            self.get_symbol_of_declaration(parent)
        } else {
            self.get_symbol_of_declaration(container)
        };
        match symbol {
            Some(symbol) => self.get_type_of_symbol(symbol),
            None => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) checkImportMetaProperty(node) *Type` (checker.go:11018).
    pub(crate) fn check_import_meta_property(&mut self, node: NodeId) -> TypeId {
        let module_kind = self.module_kind;
        if tsc_core::options_generated::ModuleKind::Node16 <= module_kind
            && module_kind <= tsc_core::options_generated::ModuleKind::NodeNext
        {
            // TODO(port M6 S6): program.GetSourceFileMetaData — the
            // impliedNodeFormat read (the program seam); permissive skip of
            // the CommonJS-output diagnostic.
        } else if module_kind < tsc_core::options_generated::ModuleKind::ES2020
            && module_kind != tsc_core::options_generated::ModuleKind::System
        {
            self.error(
                node,
                &THE_IMPORT_META_META_PROPERTY_IS_ONLY_ALLOWED_WHEN_THE_MODULE_OPTION_IS_ES2020_ES2022_ESNEXT_SYSTEM_NODE16_NODE18_NODE20_OR_NODENEXT,
                &[],
            );
        }
        let name = self
            .store()
            .node(node)
            .as_meta_property()
            .expect("MetaProperty data")
            .name;
        if tsc_ast::node_text(self.store(), name) == "meta" {
            // TODO(port M6 S7/S6): getGlobalImportMetaType (the synthesized
            // `typeof import.meta` interface); errorType until the S7
            // anonymous-type construction + the S6 module seam land.
            return self.error_type();
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) checkDeleteExpression(node) *Type` (checker.go:11040).
    pub(crate) fn check_delete_expression(&mut self, node: NodeId) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("DeleteExpression has an expression");
        self.check_expression(expression);
        let expr = tsc_binder::ast_shims::skip_parentheses(self.store(), expression);
        let boolean_type = self.boolean_type();
        if !tsc_ast::is_access_expression(self.store().node(expr)) {
            self.error(
                expr,
                &THE_OPERAND_OF_A_DELETE_OPERATOR_MUST_BE_A_PROPERTY_REFERENCE,
                &[],
            );
            return boolean_type;
        }
        if self.store().node(expr).kind == Kind::PropertyAccessExpression
            && {
                let name = self
                    .store()
                    .node(expr)
                    .as_property_access_expression()
                    .expect("PropertyAccessExpression data")
                    .name;
                self.store().node(name).kind == Kind::PrivateIdentifier
            }
        {
            self.error(
                expr,
                &THE_OPERAND_OF_A_DELETE_OPERATOR_CANNOT_BE_A_PRIVATE_IDENTIFIER,
                &[],
            );
        }
        if let Some(resolved) = self.get_resolved_symbol_nil(expr) {
            let symbol = self.get_export_symbol_of_value_symbol_if_exported(resolved);
            if self.is_readonly_symbol(symbol) {
                self.error(
                    expr,
                    &THE_OPERAND_OF_A_DELETE_OPERATOR_CANNOT_BE_A_READ_ONLY_PROPERTY,
                    &[],
                );
            } else {
                self.check_delete_expression_must_be_optional(expr, symbol);
            }
        }
        boolean_type
    }

    /// Go: `func (c *Checker) checkDeleteExpressionMustBeOptional(expr, symbol)`
    /// (checker.go:11061).
    pub(crate) fn check_delete_expression_must_be_optional(&mut self, expr: NodeId, symbol: SymbolId) {
        let t = self.get_type_of_symbol(symbol);
        if self.strict_null_checks
            && !self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN.union(TypeFlags::NEVER))
        {
            let is_optional = if self.exact_optional_property_types {
                self.program_symbol(symbol).flags.intersects(SymbolFlags::OPTIONAL)
            } else {
                self.has_type_facts(t, TypeFacts::IS_UNDEFINED)
            };
            if !is_optional {
                self.error(expr, &THE_OPERAND_OF_A_DELETE_OPERATOR_MUST_BE_OPTIONAL, &[]);
            }
        }
    }

    /// Go: `func (c *Checker) checkVoidExpression(node) *Type` (checker.go:11076).
    pub(crate) fn check_void_expression(&mut self, node: NodeId) -> TypeId {
        self.check_node_deferred(node);
        self.undefined_widening_type()
    }

    /// Go: `func (c *Checker) checkAwaitExpression(node) *Type` (checker.go:11081).
    pub(crate) fn check_await_expression(&mut self, node: NodeId) -> TypeId {
        self.check_grammar_await_or_await_using(node);
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("AwaitExpression has an expression");
        let operand_type = self.check_expression(expression);
        let awaited_type = self.check_awaited_type(
            operand_type,
            true, /*withAlias*/
            node,
            &THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
        );
        if awaited_type == operand_type
            && !relater::is_error_type(self, awaited_type)
            && !self.type_(operand_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
        {
            self.error_or_suggestion(false, node, &X_AWAIT_HAS_NO_EFFECT_ON_THE_TYPE_OF_THIS_EXPRESSION, &[]);
        }
        awaited_type
    }

    /// Go: `func (c *Checker) checkGrammarAwaitOrAwaitUsing(node)` (grammarchecks.go).
    /// TODO(port M6 W3): permissive no-op.
    pub(crate) fn check_grammar_await_or_await_using(&mut self, node: NodeId) {
        let _ = node;
    }

    /// Go: `func (c *Checker) checkPrefixUnaryExpression(node) *Type` (checker.go:11091).
    pub(crate) fn check_prefix_unary_expression(&mut self, node: NodeId) -> TypeId {
        let (operator, operand) = {
            let d = self
                .store()
                .node(node)
                .as_prefix_unary_expression()
                .expect("PrefixUnaryExpression data");
            (d.operator, d.operand)
        };
        let operand_type = self.check_expression(operand);
        if operand_type == self.silent_never_type() {
            return self.silent_never_type();
        }
        let operand_kind = self.store().node(operand).kind;
        if operand_kind == Kind::NumericLiteral {
            let text = tsc_ast::node_text(self.store(), operand).to_string();
            match operator {
                Kind::MinusToken => {
                    let lit = self.get_number_literal_type(-tsc_jsnum::from_string(&text));
                    return self.get_fresh_type_of_literal_type(lit);
                }
                Kind::PlusToken => {
                    let lit = self.get_number_literal_type(tsc_jsnum::from_string(&text));
                    return self.get_fresh_type_of_literal_type(lit);
                }
                _ => {}
            }
        }
        if operand_kind == Kind::BigIntLiteral && operator == Kind::MinusToken {
            let text = tsc_ast::node_text(self.store(), operand).to_string();
            let big_int = self.get_big_int_literal_type(tsc_jsnum::new_pseudo_big_int(
                &tsc_jsnum::parse_pseudo_big_int(&text),
                true, /*negative*/
            ));
            return self.get_fresh_type_of_literal_type(big_int);
        }
        match operator {
            Kind::PlusToken | Kind::MinusToken | Kind::TildeToken => {
                self.check_non_null_type(operand_type, operand);
                // TODO(port M6 S10): maybeTypeOfKindConsideringBaseConstraint —
                // the constraint walk; the flag-only probe under-reports only.
                if relater::maybe_type_of_kind(self, operand_type, TypeFlags::ESSYMBOL_LIKE) {
                    self.error(
                        operand,
                        &THE_0_OPERATOR_CANNOT_BE_APPLIED_TO_TYPE_SYMBOL,
                        &[token_to_string(operator).to_string()],
                    );
                }
                if operator == Kind::PlusToken {
                    if relater::maybe_type_of_kind(self, operand_type, TypeFlags::BIGINT_LIKE) {
                        let base = self.get_base_type_of_literal_type(operand_type);
                        let base_str = self.type_to_string(base);
                        self.error(
                            operand,
                            &OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPE_1,
                            &[token_to_string(operator).to_string(), base_str],
                        );
                    }
                    return self.number_type();
                }
                self.get_unary_result_type(operand_type)
            }
            Kind::ExclamationToken => {
                self.check_truthiness_of_type(operand_type, operand);
                // PORT (S10 seam): getTypeFacts runs the facts lattice worker
                // — the equal-facts fallback (relater.rs) is the pinned seam;
                // the booleanType arm composes with the crate's documented
                // S7 booleanType panic.
                let facts = self.get_type_facts(
                    operand_type,
                    TypeFacts::TRUTHY.union(TypeFacts::FALSY),
                );
                if facts == TypeFacts::TRUTHY {
                    return self.false_type();
                }
                if facts == TypeFacts::FALSY {
                    return self.true_type();
                }
                self.boolean_type()
            }
            Kind::PlusPlusToken | Kind::MinusMinusToken => {
                let non_null = self.check_non_null_type(operand_type, operand);
                let ok = self.check_arithmetic_operand_type(
                    operand,
                    non_null,
                    &AN_ARITHMETIC_OPERAND_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
                    false, /*isAwaitValid*/
                );
                if ok {
                    // run check only if former checks succeeded to avoid reporting cascading errors
                    self.check_reference_expression(
                        operand,
                        &THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
                        &THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
                    );
                }
                self.get_unary_result_type(operand_type)
            }
            _ => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) checkPostfixUnaryExpression(node) *Type` (checker.go:11145).
    pub(crate) fn check_postfix_unary_expression(&mut self, node: NodeId) -> TypeId {
        let (operator, operand) = {
            let d = self
                .store()
                .node(node)
                .as_postfix_unary_expression()
                .expect("PostfixUnaryExpression data");
            (d.operator, d.operand)
        };
        let _ = operator;
        let operand_type = self.check_expression(operand);
        if operand_type == self.silent_never_type() {
            return self.silent_never_type();
        }
        let non_null = self.check_non_null_type(operand_type, operand);
        let ok = self.check_arithmetic_operand_type(
            operand,
            non_null,
            &AN_ARITHMETIC_OPERAND_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
            false, /*isAwaitValid*/
        );
        if ok {
            // run check only if former checks succeeded to avoid reporting cascading errors
            self.check_reference_expression(
                operand,
                &THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
                &THE_OPERAND_OF_AN_INCREMENT_OR_DECREMENT_OPERATOR_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
            );
        }
        self.get_unary_result_type(operand_type)
    }

    /// Go: `func (c *Checker) getUnaryResultType(operandType) *Type` (checker.go:11159).
    pub(crate) fn get_unary_result_type(&mut self, operand_type: TypeId) -> TypeId {
        if relater::maybe_type_of_kind(self, operand_type, TypeFlags::BIGINT_LIKE) {
            if self.is_type_assignable_to_kind(operand_type, TypeFlags::ANY_OR_UNKNOWN)
                || relater::maybe_type_of_kind(self, operand_type, TypeFlags::NUMBER_LIKE)
            {
                // TODO(port M6 S7): numberOrBigIntType union; the number arm
                // is Go's answer for every non-union shape at this depth.
                return self.number_type();
            }
            return self.bigint_type();
        }
        // If it's not a bigint type, implicit coercion will result in a number
        self.number_type()
    }

    /// Go: `func (c *Checker) checkConditionalExpression(node, checkMode) *Type` (checker.go:11170).
    pub(crate) fn check_conditional_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let (condition, when_true, when_false) = {
            let d = self
                .store()
                .node(node)
                .as_conditional_expression()
                .expect("ConditionalExpression data");
            (d.condition, d.when_true, d.when_false)
        };
        let t = self.check_truthiness_expression(condition, check_mode);
        self.check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(condition, t, Some(when_true));
        let type1 = self.check_expression_ex(when_true, check_mode);
        let type2 = self.check_expression_ex(when_false, check_mode);
        self.get_union_type_ex(&[type1, type2], crate::todo_shapes::UnionReduction::Subtype)
    }

    /// Go: `func (c *Checker) checkSpreadExpression(node, checkMode) *Type` (checker.go:11183).
    pub(crate) fn check_spread_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("SpreadElement has an expression");
        let array_or_iterable_type = self.check_expression_ex(expression, check_mode);
        self.check_iterated_type_or_element_type(
            crate::statements::IterationUse::SPREAD,
            array_or_iterable_type,
            self.undefined_type(),
            Some(expression),
        )
    }

    /// Go: `func (c *Checker) checkYieldExpression(node) *Type` (checker.go:11188).
    pub(crate) fn check_yield_expression(&mut self, node: NodeId) -> TypeId {
        self.check_grammar_yield_expression(node);
        // Always check the operand so its identifiers are resolved even when the yield is
        // outside a generator, keeping diagnostics stable regardless of traversal order.
        let (asterisk_token, expression) = {
            let d = self
                .store()
                .node(node)
                .as_yield_expression()
                .expect("YieldExpression data");
            (d.asterisk_token, d.expression)
        };
        let yield_expression_type = match expression {
            Some(expr) => self.check_expression(expr),
            None => self.undefined_widening_type(),
        };
        let Some(fn_node) = u::get_containing_function(self.store(), node) else {
            return self.any_type();
        };
        let function_flags = tsc_ast::get_function_flags(self.store(), fn_node);
        if !function_flags.intersects(tsc_ast::functionflags::FunctionFlags::GENERATOR) {
            // If the user's code is syntactically correct, the func should always have a star. After all, we are in a yield context.
            return self.any_type();
        }
        let is_async = function_flags.intersects(tsc_ast::functionflags::FunctionFlags::ASYNC);
        if asterisk_token.is_some() {
            // Async generator functions prior to ES2018 require the __await, __asyncDelegator,
            // and __asyncValues helpers
            // TODO(port M6 W3/S10): checkExternalEmitHelpers — permissive skip.
        }
        // There is no point in doing an assignability check if the function
        // has no explicit return type because the return type is directly computed
        // from the yield expressions.
        let mut return_type = self.get_return_type_from_annotation(fn_node);
        if let Some(rt) = return_type
            && self.type_(rt).flags.intersects(TypeFlags::UNION)
        {
            // Go: filterType (checker.go:5317). The closure borrows the
            // checker, so the union filter is distributed by hand.
            let constituents = self.type_(rt).distributed();
            let kept: Vec<TypeId> = constituents
                .iter()
                .copied()
                .filter(|t| {
                    self.check_generator_instantiation_assignability_to_return_type(*t, function_flags, None)
                })
                .collect();
            if kept.len() == constituents.len() {
                // filterType returns t unchanged when every constituent passes.
            } else {
                return_type = Some(relater::get_union_type(self, kept));
            }
        }
        let mut iteration_types = crate::statements::IterationTypes::default();
        if let Some(rt) = return_type {
            iteration_types = self.get_iteration_types_of_generator_function_return_type(rt, is_async);
        }
        let signature_yield_type = iteration_types.yield_type.unwrap_or_else(|| self.any_type());
        let signature_next_type = iteration_types.next_type.unwrap_or_else(|| self.any_type());
        let yielded_type =
            self.get_yielded_type_of_yield_expression(node, yield_expression_type, signature_next_type, is_async);
        if let Some(_rt) = return_type
            && let Some(yielded) = yielded_type
        {
            self.check_type_assignable_to_and_optionally_elaborate(
                yielded,
                signature_yield_type,
                expression.or(Some(node)),
                expression,
                None,
                None,
            );
        }
        if asterisk_token.is_some() {
            let use_ = if is_async {
                crate::statements::IterationUse::ASYNC_YIELD_STAR
            } else {
                crate::statements::IterationUse::YIELD_STAR
            };
            return self
                .get_iteration_type_of_iterable(
                    use_,
                    crate::statements::IterationTypeKind::Return,
                    yield_expression_type,
                    expression,
                )
                .unwrap_or_else(|| self.any_type());
        }
        if let Some(rt) = return_type {
            return self
                .get_iteration_type_of_generator_function_return_type(
                    crate::statements::IterationTypeKind::Next,
                    rt,
                    is_async,
                )
                .unwrap_or_else(|| self.any_type());
        }
        let t = self.get_contextual_iteration_type(crate::statements::IterationTypeKind::Next, fn_node);
        match t {
            Some(t) => t,
            None => {
                let t = self.any_type();
                if self.no_implicit_any && !u::expression_result_is_unused(self.store(), node) {
                    let contextual_type = self.get_contextual_type_stub(node, crate::types::ContextFlags::NONE);
                    if contextual_type.is_none() || u::is_type_any(contextual_type.as_ref().map(|t| self.type_(*t))) {
                        self.error(
                            node,
                            &X_YIELD_EXPRESSION_IMPLICITLY_RESULTS_IN_AN_ANY_TYPE_BECAUSE_ITS_CONTAINING_GENERATOR_LACKS_A_RETURN_TYPE_ANNOTATION,
                            &[],
                        );
                    }
                }
                t
            }
        }
    }

    /// Go: `func (c *Checker) getYieldedTypeOfYieldExpression(node, expressionType, sentType, isAsync) *Type`
    /// (checker.go:11254).
    pub(crate) fn get_yielded_type_of_yield_expression(
        &mut self,
        node: NodeId,
        expression_type: TypeId,
        sent_type: TypeId,
        is_async: bool,
    ) -> Option<TypeId> {
        let (asterisk_token, expression) = {
            let d = self
                .store()
                .node(node)
                .as_yield_expression()
                .expect("YieldExpression data");
            (d.asterisk_token, d.expression)
        };
        let error_node = expression.unwrap_or(node);
        let is_yield_star = asterisk_token.is_some();
        // A `yield*` expression effectively yields everything that its operand yields
        let mut yielded_type = expression_type;
        if is_yield_star {
            let use_ = if is_async {
                crate::statements::IterationUse::ASYNC_YIELD_STAR
            } else {
                crate::statements::IterationUse::YIELD_STAR
            };
            yielded_type =
                self.check_iterated_type_or_element_type(use_, expression_type, sent_type, Some(error_node));
        }
        if !is_async {
            return Some(yielded_type);
        }
        Some(self.get_awaited_type_ex(
            yielded_type,
            error_node,
            if is_yield_star {
                &TYPE_OF_ITERATED_ELEMENTS_OF_A_YIELD_ASTERISK_OPERAND_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER
            } else {
                &TYPE_OF_YIELD_OPERAND_IN_AN_ASYNC_GENERATOR_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER
            },
        ))
    }

    /// Go: `func (c *Checker) checkSyntheticExpression(node) *Type` (checker.go:11270).
    pub(crate) fn check_synthetic_expression(&mut self, node: NodeId) -> TypeId {
        let (t, is_spread) = {
            let d = self
                .store()
                .node(node)
                .as_synthetic_expression()
                .expect("SyntheticExpression data");
            // Go: node.AsSyntheticExpression().Type.(*Type) — the port's
            // synthetic expression carries the TypeId as an erased Arc
            // (SPEC §5.1); the downcast is Go's type assertion.
            (
                *d.type_.downcast_ref::<TypeId>().expect("SyntheticExpression carries a TypeId"),
                d.is_spread,
            )
        };
        if is_spread {
            let number_type = self.number_type();
            return self.get_indexed_access_type(t, number_type);
        }
        t
    }

    /// Go: `func (c *Checker) getContextualIterationType(kind, functionDecl) *Type`
    /// (checker.go:30205 — S9). Permissive None.
    pub(crate) fn get_contextual_iteration_type(
        &mut self,
        kind: crate::statements::IterationTypeKind,
        function_decl: NodeId,
    ) -> Option<TypeId> {
        // TODO(port M6 S9): the contextual iteration chain.
        let _ = (kind, function_decl);
        None
    }

    /// Go: `func (c *Checker) checkGeneratorInstantiationAssignabilityToReturnType(returnType, functionFlags, errorNode) bool`
    /// (checker.go:30246 — S9). Permissive true (the assignability gate
    /// stays open until the instantiation family lands).
    pub(crate) fn check_generator_instantiation_assignability_to_return_type(
        &mut self,
        return_type: TypeId,
        function_flags: tsc_ast::functionflags::FunctionFlags,
        error_node: Option<NodeId>,
    ) -> bool {
        // TODO(port M6 S9): getIterationTypesOfGeneratorFunctionReturnType +
        // the yield-type assignability probe.
        let _ = (return_type, function_flags, error_node);
        true
    }

    /// Go: `func (c *Checker) getAwaitedTypeEx(t, errorNode, diagnosticMessage, args...) *Type`
    /// (checker.go:31806 — S9). Permissive identity (Go's answer for every
    /// non-thenable type).
    pub(crate) fn get_awaited_type_ex(
        &mut self,
        t: TypeId,
        error_node: NodeId,
        diagnostic_message: &'static Message,
    ) -> TypeId {
        // TODO(port M6 S9): unwrapAwaitedType + the thenable resolution.
        let _ = (error_node, diagnostic_message);
        t
    }
}

/// Go: `isNodeDescendantOf(node, parent.AsBinaryExpression().Right)` on the
/// instanceof-right check of checkExpressionWithTypeArguments.
fn is_binary_instance_of_store(c: &Checker, node: NodeId, right: NodeId) -> bool {
    u::is_node_descendant_of(c.store(), node, right)
}

/// Go: `func getNewTargetContainer(node) *ast.Node` (checker.go:11199).
fn get_new_target_container(c: &Checker, node: NodeId) -> Option<NodeId> {
    // Go: getThisContainer(node, /*includeFunctionLikeInterface*/ false) — the
    // Rust shim's parameterization (includeArrowFunctions,
    // includeClassComputedPropertyName) ports the same walk; false/false
    // matches the single Go flag being clear.
    let container = tsc_binder::ast_shims::get_this_container(
        c.store(),
        node,
        false, /*includeArrowFunctions*/
        false, /*includeClassComputedPropertyName*/
    );
    let kind = c.store().node(container).kind;
    if kind == Kind::Constructor
        || kind == Kind::FunctionDeclaration
        || kind == Kind::FunctionExpression
    {
        return Some(container);
    }
    None
}

/// Go: `func IsImportPhaseMetaProperty(node) bool` (ast/utilities.go) =
/// `IsImportDeferMetaProperty(node) || IsImportSourceMetaProperty(node)`.
/// TODO(port M6 W5/ast): the Rust AST predates the phase forms, so the
/// predicate is ported locally (name text under the `import` keyword).
fn is_import_phase_meta_property(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::MetaProperty {
        return false;
    }
    let d = store
        .node(node)
        .as_meta_property()
        .expect("MetaProperty data");
    d.keyword_token == Kind::ImportKeyword
        && (tsc_ast::node_text(store, d.name) == "defer" || tsc_ast::node_text(store, d.name) == "source")
}

// ────────────────────────────────────────────────────────────────────────────
// Property-access / accessibility / nonexistent-property core
// (Go checker.go:11480-12180)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) isValidConstAssertionArgument(node) bool` (checker.go:13893).
    pub(crate) fn is_valid_const_assertion_argument(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::StringLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::ArrayLiteralExpression
            | Kind::ObjectLiteralExpression
            | Kind::TemplateExpression => true,
            Kind::ParenthesizedExpression => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("ParenthesizedExpression has an expression");
                self.is_valid_const_assertion_argument(expression)
            }
            Kind::PrefixUnaryExpression => {
                let (op, arg) = {
                    let d = self
                        .store()
                        .node(node)
                        .as_prefix_unary_expression()
                        .expect("PrefixUnaryExpression data");
                    (d.operator, d.operand)
                };
                let arg_kind = self.store().node(arg).kind;
                (op == Kind::MinusToken && (arg_kind == Kind::NumericLiteral || arg_kind == Kind::BigIntLiteral))
                    || (op == Kind::PlusToken && arg_kind == Kind::NumericLiteral)
            }
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                let expression = crate::ast_shims::expression_of(self.store(), node)
                    .expect("access expression has an expression");
                let expr = tsc_ast::utilities::skip_parentheses(self.store(), expression);
                // Go: IsEntityNameExpression(expr) gate on the resolved symbol.
                if u::is_entity_name_expression(self.store(), expr) {
                    // TODO(port M6 S5): resolveEntityName — the resolver seam;
                    // permissive: no enum-resolution, so the const-assertion
                    // enum arm answers false until the resolver lands.
                    return false;
                }
                false
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) checkGrammarDisallowedTrailingComma(node, diagnostic) bool`
    /// (grammarchecks.go). TODO(port M6 W3): permissive no-op (the
    /// trailing-comma grammar family).
    pub(crate) fn check_grammar_for_disallowed_trailing_comma(
        &mut self,
        list: Option<&[NodeId]>,
        diagnostic: &'static Message,
    ) -> bool {
        let _ = (list, diagnostic);
        false
    }

    /// Go: `func (c *Checker) getIndexedAccessTypeOrUndefined(t, key, keyNode, accessNode) *Type`
    /// (checker.go:29480 region). TODO(port M6 S7): the generic-index gate;
    /// the S7 relater indexed-access seam answers for the concrete shape.
    pub(crate) fn get_indexed_access_type_or_undefined(
        &mut self,
        t: TypeId,
        key: TypeId,
        _access_flags: crate::types::AccessFlags,
        _key_node: Option<NodeId>,
        _access_node: Option<NodeId>,
    ) -> Option<TypeId> {
        Some(self.get_indexed_access_type(t, key))
    }

    /// Go: `func (c *Checker) markPropertyAsReferenced(prop, node, isSelfReference)`
    /// (checker.go:12747 — S10 reference tracking). Permissive no-op.
    pub(crate) fn mark_property_as_referenced(
        &mut self,
        prop: SymbolId,
        node: Option<NodeId>,
        is_self_reference: bool,
    ) {
        // TODO(port M6 S10): the symbol-reference recording (import-elision).
        let _ = (prop, node, is_self_reference);
    }

    /// Go: `func (c *Checker) markLinkedReferences(node, referenceHint, propSymbol, leftType)`
    /// (checker.go:12754 — S10). Permissive no-op.
    pub(crate) fn mark_linked_references(
        &mut self,
        node: NodeId,
        reference_hint: ReferenceHint,
        prop_symbol: Option<SymbolId>,
        left_type: TypeId,
    ) {
        // TODO(port M6 S10): the deferred-reference recording.
        let _ = (node, reference_hint, prop_symbol, left_type);
    }

    /// Go: `func (c *Checker) checkPropertyAccessExpression(node, checkMode, writeOnly) *Type`
    /// (checker.go:11480).
    pub(crate) fn check_property_access_expression(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
        write_only: bool,
    ) -> TypeId {
        if self.store().node(node).flags.intersects(NF::OPTIONAL_CHAIN) {
            return self.check_property_access_chain(node, check_mode);
        }
        let (expr, name) = {
            let d = self
                .store()
                .node(node)
                .as_property_access_expression()
                .expect("PropertyAccessExpression data");
            (d.expression, d.name)
        };
        let left_type = self.check_non_null_expression(expr);
        self.check_property_access_expression_or_qualified_name(
            node, expr, left_type, name, check_mode, write_only,
        )
    }

    /// Go: `func (c *Checker) checkPropertyAccessChain(node, checkMode) *Type` (checker.go:11488).
    pub(crate) fn check_property_access_chain(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let expr = crate::ast_shims::expression_of(self.store(), node)
            .expect("PropertyAccessExpression has an expression");
        let name = self
            .store()
            .node(node)
            .as_property_access_expression()
            .expect("PropertyAccessExpression data")
            .name;
        let left_type = self.check_expression(expr);
        let non_optional_type = self.get_optional_expression_type(left_type, expr);
        let non_nullable = self.check_non_null_type(non_optional_type, expr);
        let t = self.check_property_access_expression_or_qualified_name(
            node, expr, non_nullable, name, check_mode, false /*writeOnly*/,
        );
        self.propagate_optional_type_marker(t, node, non_optional_type != left_type)
    }

    /// Go: `func (c *Checker) checkPropertyAccessExpressionOrQualifiedName(...) *Type`
    /// (checker.go:11494).
    pub(crate) fn check_property_access_expression_or_qualified_name(
        &mut self,
        node: NodeId,
        left: NodeId,
        left_type: TypeId,
        right: NodeId,
        check_mode: CheckMode,
        write_only: bool,
    ) -> TypeId {
        let parent_symbol = self.get_resolved_symbol_nil(left);
        let assignment_kind = u::get_assignment_target_kind(self.store(), node);
        let mut widened_type = left_type;
        if assignment_kind != u::AssignmentKind::None || self.is_method_access_for_call(node) {
            widened_type = self.get_widened_type(left_type);
        }
        let apparent_type = self.get_apparent_type(widened_type);
        let is_any_like =
            u::is_type_any(Some(self.type_(apparent_type))) || apparent_type == self.silent_never_type();
        let mut prop: Option<SymbolId> = None;
        if self.store().node(right).kind == Kind::PrivateIdentifier {
            // (Go's emit-helper arms for downlevel private names run under
            // checkExternalEmitHelpers — TODO(port M6 W3/S10).)
            let right_text = tsc_ast::node_text(self.store(), right).to_string();
            let lexically_scoped_symbol =
                self.lookup_symbol_for_private_identifier_declaration(&right_text, right);
            if assignment_kind != u::AssignmentKind::None
                && let Some(l) = lexically_scoped_symbol
            {
                let value_declaration = self.program_symbol(l).value_declaration;
                if let Some(vd) = value_declaration
                    && self.store().node(vd).kind == Kind::MethodDeclaration
                {
                    self.grammar_error_on_node(
                        right,
                        &CANNOT_ASSIGN_TO_PRIVATE_METHOD_0_PRIVATE_METHODS_ARE_NOT_WRITABLE,
                        &[tsc_ast::node_text(self.store(), right).to_string()],
                    );
                }
            }
            if is_any_like {
                if lexically_scoped_symbol.is_some() {
                    if relater::is_error_type(self, apparent_type) {
                        return self.error_type();
                    }
                    return apparent_type;
                }
                if u::get_containing_class_excluding_class_decorators(self.store(), right).is_none() {
                    self.grammar_error_on_node(
                        right,
                        &PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
                        &[],
                    );
                    return self.any_type();
                }
            }
            if let Some(l) = lexically_scoped_symbol {
                prop = self.get_private_identifier_property_of_type(left_type, l);
            }
            match prop {
                None => {
                    // Check for private-identifier-specific shadowing and lexical-scoping errors.
                    if self.check_private_identifier_property_access(left_type, right, lexically_scoped_symbol) {
                        return self.error_type();
                    }
                    let containing_class =
                        u::get_containing_class_excluding_class_decorators(self.store(), right);
                    if let Some(containing_class) = containing_class {
                        // Go: IsPlainJSFile(GetSourceFileOfNode(...), CheckJs) —
                        // the private-in-JS grammar error. TODO(port M6 W3):
                        // the checkJs gate is permissive open, so this reports
                        // only for actual JS files.
                        if tsc_ast::utilities::is_in_js_file(self.store().node(containing_class)) {
                            self.grammar_error_on_node(
                                right,
                                &PRIVATE_FIELD_0_MUST_BE_DECLARED_IN_AN_ENCLOSING_CLASS,
                                &[tsc_ast::node_text(self.store(), right).to_string()],
                            );
                        }
                    }
                }
                Some(prop) => {
                    let is_setonly_accessor = self
                        .program_symbol(prop)
                        .flags
                        .intersects(SymbolFlags::SET_ACCESSOR)
                        && !self.program_symbol(prop).flags.intersects(SymbolFlags::GET_ACCESSOR);
                    if is_setonly_accessor && assignment_kind != u::AssignmentKind::Definite {
                        self.error(node, &PRIVATE_ACCESSOR_WAS_DEFINED_WITHOUT_A_GETTER, &[]);
                    }
                }
            }
        } else {
            if is_any_like {
                if self.store().node(left).kind == Kind::Identifier && parent_symbol.is_some() {
                    self.mark_linked_references(
                        node,
                        ReferenceHint::PROPERTY,
                        None, /*propSymbol*/
                        left_type,
                    );
                }
                if relater::is_error_type(self, apparent_type) {
                    return self.error_type();
                }
                return apparent_type;
            }
            let right_text = tsc_ast::node_text(self.store(), right).to_string();
            prop = self.get_property_of_type_ex(
                apparent_type,
                &right_text,
                /*skipObjectFunctionPropertyAugment*/ crate::spine::is_const_enum_object_type(self, apparent_type),
                /*includeTypeOnlyMembers*/ self.store().node(node).kind == Kind::QualifiedName,
            );
        }
        self.mark_linked_references(node, ReferenceHint::PROPERTY, prop, left_type);
        let prop_type: TypeId;
        match prop {
            None => {
                // Go: the index-info arm — getApplicableIndexInfoForName is the
                // S8 seam (permissive None), so the union of index branches
                // resolves through reportNonexistentProperty, exactly Go's
                // no-indexInfo path.
                let is_unchecked_js =
                    u::is_unchecked_js_suggestion(self, node, left_type_symbol(self, left_type), true /*excludeClasses*/);
                if !is_unchecked_js && u::is_js_literal_type(self, left_type) {
                    return self.any_type();
                }
                // Go: the globalThis-exports arm — the S6 module seam keeps it
                // dormant (the exports lookup answers None).
                let right_text = tsc_ast::node_text(self.store(), right).to_string();
                if !right_text.is_empty() && !self.check_and_report_error_for_extending_interface(node) {
                    let containing = if u::is_this_type_parameter(self.type_(left_type)) {
                        apparent_type
                    } else {
                        left_type
                    };
                    // Go: addDeferredDiagnostic — must be deferred because
                    // reporting this error can cause us to materialize the
                    // containing type completely (to print it), leading to
                    // erroneous circularity errors.
                    self.report_nonexistent_property(right, containing, is_unchecked_js);
                }
                return self.error_type();
            }
            Some(prop) => {
                self.check_property_not_used_before_declaration(prop, node, right);
                let is_self_reference = self.is_self_type_access(left, parent_symbol);
                self.mark_property_as_referenced(prop, Some(node), is_self_reference);
                self.check_property_accessibility(
                    node,
                    self.store().node(left).kind == Kind::SuperKeyword,
                    is_write_access(self, node),
                    apparent_type,
                    prop,
                );
                if self.is_assignment_to_readonly_entity(node, prop, assignment_kind) {
                    self.error(
                        right,
                        &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY,
                        &[tsc_ast::node_text(self.store(), right).to_string()],
                    );
                    return self.error_type();
                }
                if self.is_this_property_access_in_constructor(node, prop) {
                    prop_type = self.auto_type.expect("autoType initialized (M6 S4: spine initialises checker types)");
                } else if write_only || is_write_only_access(self.store(), node) {
                    prop_type = self.get_write_type_of_symbol(prop);
                } else {
                    prop_type = self.get_type_of_symbol(prop);
                }
            }
        }
        self.get_flow_type_of_access_expression(node, prop, prop_type, right, check_mode)
    }

    /// Go: `func (c *Checker) getFlowTypeOfAccessExpression(node, prop, propType, errorNode, checkMode) *Type`
    /// (checker.go:11628).
    pub(crate) fn get_flow_type_of_access_expression(
        &mut self,
        node: NodeId,
        prop: Option<SymbolId>,
        prop_type: TypeId,
        error_node: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        // Only compute control flow type if this is a property access expression that isn't an
        // assignment target, and the referenced property was declared as a variable, property,
        // accessor, or optional method.
        let assignment_kind = u::get_assignment_target_kind(self.store(), node);
        if assignment_kind == u::AssignmentKind::Definite {
            let is_optional = prop
                .map(|p| self.program_symbol(p).flags.intersects(SymbolFlags::OPTIONAL))
                .unwrap_or(false);
            return relater::remove_missing_type(self, prop_type, is_optional);
        }
        if let Some(prop) = prop {
            let flags = self.program_symbol(prop).flags;
            if !flags.intersects(SymbolFlags::VARIABLE.union(SymbolFlags::PROPERTY).union(SymbolFlags::ACCESSOR))
                && !(flags.intersects(SymbolFlags::METHOD)
                    && self.type_(prop_type).flags.intersects(TypeFlags::UNION))
            {
                return prop_type;
            }
        }
        if Some(prop_type) == self.auto_type {
            return self.get_flow_type_of_property(node, prop);
        }
        let prop_type = self.get_narrowable_type_for_reference(prop_type, node, check_mode);
        // If strict null checks and strict property initialization checks are enabled, if we have
        // a this.xxx property access, if the property is an instance property without an initializer,
        // and if we are in a constructor of the same class as the property declaration, assume that
        // the property is uninitialized at the top of the control flow.
        let mut assume_uninitialized = false;
        if self.strict_null_checks
            && let Some(prop) = prop
        {
            let declaration = self.program_symbol(prop).value_declaration;
            if let Some(declaration) = declaration {
                if self.strict_property_initialization
                    && tsc_ast::is_access_expression(self.store().node(node))
                {
                    let expr = crate::ast_shims::expression_of(self.store(), node)
                        .expect("access expression has an expression");
                    if self.store().node(expr).kind == Kind::ThisKeyword
                        && self.is_property_without_initializer(declaration)
                        && !tsc_binder::ast_shims::is_static(self.store().node(declaration))
                    {
                        let flow_container = self.get_control_flow_container(node);
                        if self.store().node(flow_container).kind == Kind::Constructor
                            && self.store().node(flow_container).parent.get()
                                == self.store().node(declaration).parent.get()
                            && !self.store().node(declaration).flags.intersects(NF::AMBIENT)
                        {
                            assume_uninitialized = true;
                        }
                    }
                } else if self.store().node(declaration).kind == Kind::BinaryExpression
                    && {
                        let d = self
                            .store()
                            .node(declaration)
                            .as_binary_expression()
                            .expect("BinaryExpression data");
                        tsc_ast::is_access_expression(self.store().node(d.left))
                    }
                    && self.get_control_flow_container(node) == self.get_control_flow_container(declaration)
                {
                    assume_uninitialized = true;
                }
            }
        }
        // TODO(port M6 W3): getFlowTypeOfReferenceEx — the flow-node walk; the
        // permissive seam returns the declared (narrowable) type, Go's answer
        // when the reference carries no flow node.
        let flow_type = prop_type;
        if assume_uninitialized
            && !self.contains_undefined_type(prop_type)
            && self.contains_undefined_type(flow_type)
        {
            if let Some(prop) = prop {
                let name = relater::symbol_to_string(self, prop);
                self.error(error_node, &PROPERTY_0_IS_USED_BEFORE_BEING_ASSIGNED, &[name]);
            }
            // Return the declared type to reduce follow-on errors
            return prop_type;
        }
        if assignment_kind != u::AssignmentKind::None {
            return self.get_base_type_of_literal_type(flow_type);
        }
        flow_type
    }

    /// Go: `func (c *Checker) getControlFlowContainer(node) *ast.Node` (checker.go:11670).
    pub(crate) fn get_control_flow_container(&mut self, node: NodeId) -> NodeId {
        let mut parent = self.store().node(node).parent.get();
        loop {
            let n = self.store().node(parent);
            let is_match = (tsc_ast::is_function_like(n)
                && tsc_binder::ast_shims::get_immediately_invoked_function_expression(self.store(), parent).is_none())
                || n.kind == Kind::ModuleBlock
                || n.kind == Kind::SourceFile
                || n.kind == Kind::PropertyDeclaration;
            if is_match {
                return parent;
            }
            let next = n.parent.get();
            if next == parent {
                panic!("no control flow container");
            }
            parent = next;
        }
    }

    /// Go: `func (c *Checker) getFlowTypeOfProperty(reference, prop) *Type` (checker.go:11681).
    pub(crate) fn get_flow_type_of_property(&mut self, reference: NodeId, prop: Option<SymbolId>) -> TypeId {
        let mut initial_type = self.undefined_type();
        if let Some(prop) = prop
            && let Some(value_declaration) = self.program_symbol(prop).value_declaration
        {
            let is_ambient = self.store().node(value_declaration).modifier_flags().intersects(ModifierFlags::AMBIENT);
            if !self.is_auto_typed_property(prop) || is_ambient {
                if let Some(base_type) = self.get_type_of_property_in_base_class(prop) {
                    initial_type = base_type;
                }
            }
        }
        // TODO(port M6 W3): getFlowTypeOfReferenceEx(reference, autoType,
        // initialType) — the flow walk; permissive initial type (Go's answer
        // when the reference carries no flow node).
        let _ = reference;
        initial_type
    }


    /// Go: `func (c *Checker) isMethodAccessForCall(node) bool` (checker.go:11703).
    pub(crate) fn is_method_access_for_call(&mut self, node: NodeId) -> bool {
        let mut node = node;
        let mut parent = self.store().node(node).parent.get();
        while self.store().node(parent).kind == Kind::ParenthesizedExpression {
            node = parent;
            parent = self.store().node(node).parent.get();
        }
        let parent_kind = self.store().node(parent).kind;
        (parent_kind == Kind::CallExpression || parent_kind == Kind::NewExpression)
            && crate::ast_shims::expression_of(self.store(), parent) == Some(node)
    }

    /// Go: `func (c *Checker) lookupSymbolForPrivateIdentifierDeclaration(propName, location) *ast.Symbol`
    /// (checker.go:11711).
    pub(crate) fn lookup_symbol_for_private_identifier_declaration(
        &mut self,
        prop_name: &str,
        location: NodeId,
    ) -> Option<SymbolId> {
        // PORT: Go builds the "\xFE#<id>@<name>" member key via
        // binder.GetSymbolNameForPrivateIdentifier; the checker re-derives it
        // from the symbol's lazily assigned id (the binder-side helper is
        // binder-private, so the format is reimplemented here).
        let mut containing_class = u::get_containing_class_excluding_class_decorators(self.store(), location);
        while let Some(class) = containing_class {
            let symbol = self.get_symbol_of_declaration(class)?;
            let id = self.program_symbol(symbol).id.get();
            let name = format!("{}#{}@{}", tsc_ast::INTERNAL_SYMBOL_NAME_PREFIX, id, prop_name);
            let prop = self.program_symbol(symbol).members.get(&name).copied();
            if let Some(prop) = prop {
                return Some(prop);
            }
            let prop = self.program_symbol(symbol).exports.get(&name).copied();
            if let Some(prop) = prop {
                return Some(prop);
            }
            containing_class = u::get_containing_class(self.store(), class);
        }
        None
    }

    /// Go: `func (c *Checker) getPrivateIdentifierPropertyOfType(leftType, lexicallyScopedIdentifier) *ast.Symbol`
    /// (checker.go:11727).
    pub(crate) fn get_private_identifier_property_of_type(
        &mut self,
        left_type: TypeId,
        lexically_scoped_identifier: SymbolId,
    ) -> Option<SymbolId> {
        let name = self.program_symbol(lexically_scoped_identifier).name.clone();
        self.get_property_of_type(left_type, &name)
    }

    /// Go: `func (c *Checker) checkPrivateIdentifierPropertyAccess(leftType, right, lexicallyScopedIdentifier) bool`
    /// (checker.go:11731).
    pub(crate) fn check_private_identifier_property_access(
        &mut self,
        left_type: TypeId,
        right: NodeId,
        lexically_scoped_identifier: Option<SymbolId>,
    ) -> bool {
        // Either the identifier could not be looked up in the lexical scope OR the lexically scoped identifier did not exist on the type.
        // Find a private identifier with the same description on the type.
        let properties = self.get_properties_of_type(left_type);
        let mut property_on_type: Option<SymbolId> = None;
        for symbol in properties {
            let decl = self.program_symbol(symbol).value_declaration;
            if let Some(decl) = decl
                && let Some(name) = self.store().node(decl).name()
                && self.store().node(name).kind == Kind::PrivateIdentifier
                && tsc_ast::node_text(self.store(), name) == tsc_ast::node_text(self.store(), right)
            {
                property_on_type = Some(symbol);
                break;
            }
        }
        let diag_name = tsc_ast::node_text(self.store(), right).to_string();
        if let Some(property_on_type) = property_on_type {
            let type_value_decl = self
                .program_symbol(property_on_type)
                .value_declaration
                .expect("private identifier property has a declaration");
            let type_class = u::get_containing_class(self.store(), type_value_decl);
            // We found a private identifier property with the same description.
            // Either:
            // - There is a lexically scoped private identifier AND it shadows the one we found on the type.
            // - It is an attempt to access the private identifier outside of the class.
            if let Some(lex) = lexically_scoped_identifier
                && let Some(lexical_value_decl) = self.program_symbol(lex).value_declaration
            {
                let lexical_class = u::get_containing_class(self.store(), lexical_value_decl);
                // PORT: Go FindAncestor(lexicalClass, n == typeClass); the
                // port walks the chain directly.
                let mut cursor = lexical_class;
                let mut shadowed = false;
                while let Some(c) = cursor {
                    if Some(c) == type_class {
                        shadowed = true;
                        break;
                    }
                    cursor = u::get_containing_class(self.store(), c);
                }
                if shadowed {
                    self.error(
                        right,
                        &PROPERTY_0_IS_NOT_ACCESSIBLE_OUTSIDE_CLASS_1_BECAUSE_IT_HAS_A_PRIVATE_IDENTIFIER,
                        &[],
                    );
                    // (The related-info adds are the M7 diagnostics chain.)
                    return true;
                }
            }
            let class_name = type_class
                .and_then(|tc| self.get_symbol_of_declaration(tc))
                .map(|s| relater::symbol_to_string(self, s))
                .unwrap_or_default();
            self.error(
                right,
                &PROPERTY_0_IS_NOT_ACCESSIBLE_OUTSIDE_CLASS_1_BECAUSE_IT_HAS_A_PRIVATE_IDENTIFIER,
                &[diag_name.clone(), class_name],
            );
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForExtendingInterface(errorLocation) bool`
    /// (checker.go:11897). The resolveEntityName core is the S5 resolver
    /// seam, so the interface-extension error stays dormant (permissive false).
    pub(crate) fn check_and_report_error_for_extending_interface(&mut self, error_location: NodeId) -> bool {
        // TODO(port M6 S5): resolveEntityName(SymbolFlagsInterface, ignoreErrors).
        let _ = error_location;
        false
    }

    /// Go: `func (c *Checker) checkPropertyAccessibility(node, isSuper, writing, t, prop) bool`
    /// (checker.go:11991).
    pub(crate) fn check_property_accessibility(
        &mut self,
        node: NodeId,
        is_super: bool,
        writing: bool,
        t: TypeId,
        prop: SymbolId,
    ) -> bool {
        self.check_property_accessibility_ex(node, is_super, writing, t, prop, true /*reportError*/)
    }

    /// Go: `func (c *Checker) checkPropertyAccessibilityEx(node, isSuper, writing, t, prop, reportError) bool`
    /// (checker.go:11995).
    pub(crate) fn check_property_accessibility_ex(
        &mut self,
        node: NodeId,
        is_super: bool,
        writing: bool,
        t: TypeId,
        prop: SymbolId,
        report_error: bool,
    ) -> bool {
        let mut error_node: Option<NodeId> = None;
        if report_error {
            let node_kind = self.store().node(node).kind;
            error_node = match node_kind {
                Kind::PropertyAccessExpression => Some(
                    self.store()
                        .node(node)
                        .as_property_access_expression()
                        .expect("PropertyAccessExpression data")
                        .name,
                ),
                Kind::QualifiedName => Some(
                    self.store()
                        .node(node)
                        .as_qualified_name()
                        .expect("QualifiedName data")
                        .right,
                ),
                Kind::ImportType => Some(node),
                Kind::BindingElement => u::get_binding_element_property_name(self.store(), node),
                _ => self.store().node(node).name(),
            };
        }
        self.check_property_accessibility_at_location(node, is_super, writing, t, prop, error_node)
    }

    /// Go: `func (c *Checker) checkPropertyAccessibilityAtLocation(location, isSuper, writing, containingType, prop, errorNode) bool`
    /// (checker.go:12024).
    pub(crate) fn check_property_accessibility_at_location(
        &mut self,
        location: NodeId,
        is_super: bool,
        writing: bool,
        containing_type: TypeId,
        prop: SymbolId,
        error_node: Option<NodeId>,
    ) -> bool {
        let flags = u::get_declaration_modifier_flags_from_symbol_ex(self, self.store(), prop, writing);
        if is_super {
            // TS 1.0 spec (April 2014): 4.8.2
            // - In a constructor, instance member function, instance member accessor, or
            //   instance member variable initializer where this references a derived class instance,
            //   a super property access is permitted and must specify a public instance member function of the base class.
            // - In a static member function or static member accessor
            //   where this references the constructor function object of a derived class,
            //   a super property access is permitted and must specify a public static member function of the base class.
            if flags.intersects(ModifierFlags::ABSTRACT) {
                // A method cannot be accessed in a super property access if the method is abstract.
                // This error could mask a private property access error. But, a member
                // cannot simultaneously be private and abstract, so this will trigger an
                // additional error elsewhere.
                if let Some(error_node) = error_node {
                    let prop_name = relater::symbol_to_string(self, prop);
                    let declaring = relater::get_declaring_class(self, prop)
                        .map(|t| relater::type_to_string(self, t))
                        .unwrap_or_default();
                    self.error(error_node, &ABSTRACT_METHOD_0_IN_CLASS_1_CANNOT_BE_ACCESSED_VIA_SUPER_EXPRESSION, &[prop_name, declaring]);
                }
                return false;
            }
            // A class field cannot be accessed via super.* from a derived class.
            // This is true for both [[Set]] (old) and [[Define]] (ES spec) semantics.
            if !flags.intersects(ModifierFlags::STATIC) && self.symbol_has_non_method_declaration(prop) {
                if let Some(error_node) = error_node {
                    let prop_name = relater::symbol_to_string(self, prop);
                    self.error(error_node, &CLASS_FIELD_0_DEFINED_BY_THE_PARENT_CLASS_IS_NOT_ACCESSIBLE_IN_THE_CHILD_CLASS_VIA_SUPER, &[prop_name]);
                }
                return false;
            }
        }
        // Referencing abstract properties within their own constructors is not allowed
        // (the isThisProperty / this-initialized binding arms are S3's flow-adjacent
        // checks; TODO(port M6 W3/S9) — permissive skip of that arm.)
        let _ = location;
        // Public properties are otherwise accessible.
        if !flags.intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER) {
            return true;
        }
        // Property is known to be private or protected at this point
        // Private property is accessible if the property is within the declaring class
        if flags.intersects(ModifierFlags::PRIVATE) {
            // Go: GetClassLikeDeclarationOfSymbol(getParentOfSymbol(prop)) —
            // the symbol->declaration walk (S7 seam: class-likes only).
            let declaring_class_declaration = self
                .get_parent_of_symbol(prop)
                .and_then(|parent| get_class_like_declaration_of_symbol_for(self, parent));
            let inside = declaring_class_declaration
                .map(|d| self.is_node_within_class(location, d))
                .unwrap_or(false);
            if !inside {
                if let Some(error_node) = error_node {
                    let prop_name = relater::symbol_to_string(self, prop);
                    let class = relater::get_declaring_class(self, prop)
                        .unwrap_or(containing_type);
                    let class_str = relater::type_to_string(self, class);
                    self.error(error_node, &PROPERTY_0_IS_PRIVATE_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1, &[prop_name, class_str]);
                }
                return false;
            }
            return true;
        }
        // Property is known to be protected at this point
        // All protected properties of a supertype are accessible in a super access
        if is_super {
            return true;
        }
        // Find the first enclosing class that has the declaring classes of the protected constituents
        // of the property as base classes
        let mut enclosing_class: Option<TypeId> = None;
        let mut container = u::get_containing_class(self.store(), location);
        while let Some(current) = container {
            let symbol = self.get_symbol_of_declaration(current);
            if let Some(symbol) = symbol {
                let class = self.get_declared_type_of_symbol(symbol);
                if self.is_class_derived_from_declaring_classes(class, prop, writing) {
                    enclosing_class = Some(class);
                    break;
                }
            }
            container = u::get_containing_class(self.store(), current);
        }
        // A protected property is accessible if the property is within the declaring class or classes derived from it
        if enclosing_class.is_none() {
            // allow PropertyAccessibility if context is in function with this parameter
            // static member access is disallowed
            let class = self.get_enclosing_class_from_this_parameter(location);
            if let Some(class) = class
                && self.is_class_derived_from_declaring_classes(class, prop, writing)
            {
                enclosing_class = Some(class);
            }
            if flags.intersects(ModifierFlags::STATIC) || enclosing_class.is_none() {
                if let Some(error_node) = error_node {
                    let prop_name = relater::symbol_to_string(self, prop);
                    let class = relater::get_declaring_class(self, prop).unwrap_or(containing_type);
                    let class_str = relater::type_to_string(self, class);
                    self.error(error_node, &PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1_AND_ITS_SUBCLASSES, &[prop_name, class_str]);
                }
                return false;
            }
        }
        // No further restrictions for static properties
        if flags.intersects(ModifierFlags::STATIC) {
            return true;
        }
        let mut containing_type = containing_type;
        if self.type_(containing_type).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            // get the original type -- represented as the type constraint of
            // the 'this' type. (Go's isThisType vs base-constraint split
            // collapses here until S7's thisType carries its own constraint;
            // both arms resolve the constraint.)
            containing_type = relater::get_constraint_of_type_parameter(self, containing_type)
                .unwrap_or(containing_type);
        }
        let Some(enclosing) = enclosing_class else { return true };
        if !self.has_base_type(containing_type, enclosing) {
            if let Some(error_node) = error_node {
                let prop_name = relater::symbol_to_string(self, prop);
                let enclosing_str = relater::type_to_string(self, enclosing);
                let containing_str = relater::type_to_string(self, containing_type);
                self.error(error_node, &PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_THROUGH_AN_INSTANCE_OF_CLASS_1_THIS_IS_AN_INSTANCE_OF_CLASS_2, &[prop_name, enclosing_str, containing_str]);
            }
            return false;
        }
        true
    }

    /// Go: `func (c *Checker) symbolHasNonMethodDeclaration(symbol) bool` (checker.go:12150).
    pub(crate) fn symbol_has_non_method_declaration(&mut self, symbol: SymbolId) -> bool {
        self.for_each_property(symbol, |prop, c| {
            !c.program_symbol(prop).flags.intersects(SymbolFlags::METHOD)
        })
    }

    /// Go: `func (c *Checker) forEachProperty(prop, callback) bool` (checker.go:12155).
    /// The synthetic-property fan-out reads valueSymbolLinks'
    /// containingType (S7); the non-synthetic arm is real.
    pub(crate) fn for_each_property(
        &mut self,
        prop: SymbolId,
        mut callback: impl FnMut(SymbolId, &mut Checker) -> bool,
    ) -> bool {
        if !self.program_symbol(prop).check_flags.intersects(tsc_ast::CheckFlags::SYNTHETIC) {
            return callback(prop, self);
        }
        // TODO(port M6 S7): the synthetic fan-out over containingType.
        false
    }

    /// Go: `func (c *Checker) isClassDerivedFromDeclaringClasses(class, prop, writing) bool`
    /// (checker.go:12202).
    pub(crate) fn is_class_derived_from_declaring_classes(
        &mut self,
        class: TypeId,
        prop: SymbolId,
        writing: bool,
    ) -> bool {
        let _ = writing;
        // PORT: Go walks the property's synthetic constituents against the
        // class's base types; getBaseTypes/hasBaseType are the S8 relater
        // seams (permissive empty/false) until the inheritance engine lands.
        let _ = class;
        relater::get_declaring_class(self, prop).is_some()
    }

    /// Go: `func (c *Checker) getEnclosingClassFromThisParameter(node) *Type` (checker.go:12736).
    pub(crate) fn get_enclosing_class_from_this_parameter(&mut self, node: NodeId) -> Option<TypeId> {
        // Go: GetThisParameter(GetThisContainer(node,...)) — this-parameter
        // plumbing is S9; permissive None keeps the protected-access arm
        // dormant exactly where Go finds no this parameter.
        // TODO(port M6 S9).
        let _ = node;
        None
    }

    /// Go: `func (c *Checker) isNodeWithinClass(node, classDecl) bool` (checker.go:12209).
    pub(crate) fn is_node_within_class(&mut self, node: NodeId, class_decl: NodeId) -> bool {
        let mut cursor = Some(node);
        while let Some(current) = cursor {
            if current == class_decl {
                return true;
            }
            let parent = self.store().node(current).parent.get();
            if parent == current {
                break;
            }
            let parent = self.store().node(current).parent.get();
            cursor = (parent != NodeId::NONE).then_some(parent);
        }
        false
    }

    /// Go: `func (c *Checker) isSelfTypeAccess(expr, resolvedSymbol) bool` (checker.go:11461 region).
    pub(crate) fn is_self_type_access(&mut self, expr: NodeId, resolved_symbol: Option<SymbolId>) -> bool {
        if resolved_symbol.is_none() {
            let mut parent = self.store().node(expr).parent.get();
            while matches!(
                self.store().node(parent).kind,
                Kind::ParenthesizedExpression | Kind::NonNullExpression | Kind::TypeAssertionExpression | Kind::AsExpression | Kind::SatisfiesExpression
            ) {
                let next = self.store().node(parent).parent.get();
                if next == parent {
                    return false;
                }
                parent = next;
            }
            return self.store().node(parent).kind == Kind::PropertySignature
                || self.store().node(parent).kind == Kind::MethodSignature
                || self.store().node(parent).kind == Kind::MethodDeclaration
                || self.store().node(parent).kind == Kind::PropertyDeclaration
                || self.store().node(parent).kind == Kind::GetAccessor
                || self.store().node(parent).kind == Kind::SetAccessor;
        }
        false
    }

    /// Go: `func (c *Checker) isThisPropertyAccessInConstructor(node, prop) bool` (checker.go:11507 region).
    pub(crate) fn is_this_property_access_in_constructor(&mut self, node: NodeId, prop: SymbolId) -> bool {
        // Go: IsAccessExpression(node) && IsThisInTypeQuery(node.Expression()) &&
        // isNodeInsideCurrentClassishBodyOfConstructor(...)
        let _ = (node, prop);
        // TODO(port M6 W3): isNodeInsideCurrentClassishBodyOfConstructor.
        false
    }

    /// Go: `func (c *Checker) isAssignmentToReadonlyEntity(node, prop, assignmentKind) bool`
    /// (checker.go:11513 region).
    pub(crate) fn is_assignment_to_readonly_entity(
        &mut self,
        node: NodeId,
        prop: SymbolId,
        assignment_kind: u::AssignmentKind,
    ) -> bool {
        if assignment_kind != u::AssignmentKind::None {
            let is_write_access = is_write_access(self, node);
            if is_write_access || !self.is_readonly_symbol(prop) {
                return is_write_access && self.is_readonly_symbol(prop);
            }
        }
        false
    }

    /// Go: `func (c *Checker) getNarrowableTypeForReference(t, reference, checkMode) *Type`
    /// (checker.go:12360 — W3). Permissive identity (Go's answer for a
    /// reference with no flow node).
    pub(crate) fn get_narrowable_type_for_reference(
        &mut self,
        t: TypeId,
        reference: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        // TODO(port M6 W3): getNarrowableTypeForReferenceWorker.
        let _ = (reference, check_mode);
        t
    }

    /// Go: `func (c *Checker) containsUndefinedType(t) bool` (checker.go:27545 region).
    pub(crate) fn contains_undefined_type(&mut self, t: TypeId) -> bool {
        self.type_(t)
            .distributed()
            .iter()
            .any(|s| self.type_(*s).flags.intersects(TypeFlags::UNDEFINED))
    }

    /// Go: `func (c *Checker) isAutoTypedProperty(property) bool` (checker.go:11529 region — S7
    /// auto-type plumbing). Permissive false.
    pub(crate) fn is_auto_typed_property(&mut self, property: SymbolId) -> bool {
        // TODO(port M6 S7): the auto-literal property probe.
        let _ = property;
        false
    }

    /// Go: `func (c *Checker) checkPropertyNotUsedBeforeDeclaration(prop, node, right)` (checker.go:11947).
    pub(crate) fn check_property_not_used_before_declaration(
        &mut self,
        prop: SymbolId,
        node: NodeId,
        right: NodeId,
    ) {
        let Some(value_declaration) = self.program_symbol(prop).value_declaration else {
            return;
        };
        // (Go's GetSourceFileOfNode(node).IsDeclarationFile early-out is
        // covered by the permissive W3 skip below.)
        let declaration_name = tsc_ast::node_text(self.store(), right).to_string();
        // TODO(port M6 W3): isBlockScopedNameDeclaredBeforeUse (position
        // ordering) + isInPropertyInitializerOrClassStaticBlock — the
        // flow-position probes; permissive skip of the TS2729/2512 arms.
        let _ = (node, value_declaration, declaration_name);
    }

    /// Go: `func (c *Checker) getPropertyOfTypeEx(t, name, skipObjectFunctionPropertyAugment, includeTypeOnlyMembers) *ast.Symbol`
    /// (checker.go:31745 region). The augment skip + type-only-member walks
    /// are S7 seams; the base lookup is real.
    pub(crate) fn get_property_of_type_ex(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
        include_type_only_members: bool,
    ) -> Option<SymbolId> {
        // TODO(port M6 S7): the augmentation skip + type-only-member filter.
        let _ = (skip_object_function_property_augment, include_type_only_members);
        self.get_property_of_type(t, name)
    }

    /// Go: `func (c *Checker) reportNonexistentProperty(propNode, containingType, isUncheckedJS)` (checker.go:11767).
    pub(crate) fn report_nonexistent_property(
        &mut self,
        prop_node: NodeId,
        containing_type: TypeId,
        is_unchecked_js: bool,
    ) {
        // Go: the NonExistentPropertyKey memo table dedupes the same
        // propNode/type pair; the checker memo is an ID-keyed set.
        let key = (prop_node, containing_type, is_unchecked_js);
        if self.non_existent_properties.contains(&key) {
            return;
        }
        self.non_existent_properties.insert(key);
        if is_jsdoc_name_reference_context(self, prop_node) {
            return;
        }
        // Go: NewDiagnosticChainForNode builds the chained diagnostic and
        // addErrorOrSuggestion commits it; the related-info/chain plumbing is
        // M7, so the plain arm reports here.
        let missing_property = tsc_ast::node_text(self.store(), prop_node).to_string();
        let container = relater::type_to_string(self, containing_type);
        if !is_unchecked_js {
            self.error(
                prop_node,
                &PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                &[missing_property, container],
            );
        } else {
            self.error(
                prop_node,
                &PROPERTY_0_MAY_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
                &[missing_property, container],
            );
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// S4-local free helpers (predicates whose Go homes are outside the slice)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `const (ReferenceHintNone ReferenceHint = iota; ReferenceHintProperty; ...)`
/// (checker.go:12760 region). The S4 slice only uses ReferenceHintProperty.
// (Go keeps SCREAMING-SNAKE ReferenceHint constants; the port keeps the
// shape and silences the acronym-style lint rather than renaming away from
// the Go spelling.)
#[allow(clippy::upper_case_acronyms)]
pub(crate) enum ReferenceHint {
    #[allow(dead_code)]
    NONE,
    PROPERTY,
}

/// Go: `func IsWriteAccess(node) bool` (ast/utilities.go) —
/// `getAssignmentTargetKind(node) != AssignmentKindNone`. PORT: Go's
/// access-kind lattice is folded into the port's three-way
/// `AssignmentKind` (None/Definite/Compound), so write == any assignment
/// target.
pub(crate) fn is_write_access(c: &Checker, node: NodeId) -> bool {
    u::get_assignment_target_kind(c.store(), node) != u::AssignmentKind::None
}

/// Go: `func IsWriteOnlyAccess(node) bool` (ast/utilities.go) — the
/// write-exclusive probe. PORT: the reduced AssignmentKind lattice can't see
/// read-and-write targets, so the compound operators (which Go marks
/// read-write) answer false and `=` answers true, matching Go for the common
/// shapes.
pub(crate) fn is_write_only_access(store: &dyn NodeStore, node: NodeId) -> bool {
    u::get_assignment_target_kind(store, node) == u::AssignmentKind::Definite
}

/// Go: `func IsJSDocNameReferenceContext(node) bool` (checker.go:11777 region
/// via ast) — climbs Name/QualifiedName parents into a JSDocNameReference.
pub(crate) fn is_jsdoc_name_reference_context(c: &Checker, node: NodeId) -> bool {
    let mut parent = c.store().node(node).parent.get();
    loop {
        let kind = c.store().node(parent).kind;
        if kind == Kind::Identifier || kind == Kind::QualifiedName {
            let next = c.store().node(parent).parent.get();
            if next == parent {
                return false;
            }
            parent = next;
        } else {
            return kind == Kind::JSDocNameReference;
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// this / new.target family (Go checker.go:12232-12460)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkThisExpression(node) *Type` (checker.go:12318).
    pub(crate) fn check_this_expression(&mut self, node: NodeId) -> TypeId {
        // Stop at the first arrow function so that we can
        // tell whether 'this' needs to be captured.
        let mut container = tsc_binder::ast_shims::get_this_container(
            self.store(),
            node,
            true, /*includeArrowFunctions*/
            true, /*includeClassComputedPropertyName*/
        );
        let mut captured_by_arrow_function = false;
        let mut this_in_computed_property_name = false;
        if self.store().node(container).kind == Kind::Constructor {
            self.check_this_before_super(
                node,
                container,
                &X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_THIS_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
            );
        }
        loop {
            // Now skip arrow functions to get the "real" owner of 'this'.
            if self.store().node(container).kind == Kind::ArrowFunction {
                container = tsc_binder::ast_shims::get_this_container(
                    self.store(),
                    container,
                    false, /*includeArrowFunctions*/
                    !this_in_computed_property_name,
                );
                captured_by_arrow_function = true;
            }
            if self.store().node(container).kind == Kind::ComputedPropertyName {
                container = tsc_binder::ast_shims::get_this_container(
                    self.store(),
                    container,
                    !captured_by_arrow_function,
                    false, /*includeClassComputedPropertyName*/
                );
                this_in_computed_property_name = true;
                continue;
            }
            break;
        }
        self.check_this_in_static_class_field_initializer_in_decorated_class(node, container);
        if this_in_computed_property_name {
            self.error(node, &X_THIS_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME, &[]);
        } else {
            match self.store().node(container).kind {
                Kind::ModuleDeclaration => {
                    self.error(node, &X_THIS_CANNOT_BE_REFERENCED_IN_A_MODULE_OR_NAMESPACE_BODY, &[]);
                    // do not return here so in case if lexical this is captured - it will be reflected in flags on NodeLinks
                }
                Kind::EnumDeclaration => {
                    self.error(node, &X_THIS_CANNOT_BE_REFERENCED_IN_CURRENT_LOCATION, &[]);
                }
                _ => {}
            }
        }
        let t = self.try_get_this_type_at_ex(node, true /*includeGlobalThis*/, Some(container));
        if self.no_implicit_this {
            let global_this_type = self
                .global_this_symbol
                .map(|s| self.get_type_of_symbol(s));
            let is_global = match global_this_type {
                Some(g) => Some(g) == t,
                None => false,
            };
            if is_global && captured_by_arrow_function {
                self.error(node, &THE_CONTAINING_ARROW_FUNCTION_CAPTURES_THE_GLOBAL_VALUE_OF_THIS, &[]);
            } else if t.is_none() {
                // With noImplicitThis, functions may not reference 'this' if it has type 'any'
                self.error(node, &X_THIS_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_TYPE_ANNOTATION, &[]);
                // (Go's outside-this related info is the M7 diagnostics chain.)
            }
        }
        match t {
            Some(t) => t,
            None => self.any_type(),
        }
    }

    /// Go: `func (c *Checker) tryGetThisTypeAt(node) *Type` (checker.go:12382).
    // (Go's public TryGetThisTypeAt; used by the language-service slice.)
    #[allow(dead_code)]
    pub(crate) fn try_get_this_type_at(&mut self, node: NodeId) -> Option<TypeId> {
        self.try_get_this_type_at_ex(node, true /*includeGlobalThis*/, None /*container*/)
    }

    /// Go: `func (c *Checker) tryGetThisTypeAtEx(node, includeGlobalThis, container) *Type`
    /// (checker.go:12394).
    pub(crate) fn try_get_this_type_at_ex(
        &mut self,
        node: NodeId,
        include_global_this: bool,
        container: Option<NodeId>,
    ) -> Option<TypeId> {
        // TODO(port M6 W5/jsdoc): GetReparsedNodeForNode — the JSDoc
        // reparse guard; non-reparsed nodes don't reach the checker here.
        let container = match container {
            Some(c) => c,
            None => tsc_binder::ast_shims::get_this_container(
                self.store(),
                node,
                false, /*includeArrowFunctions*/
                false, /*includeClassComputedPropertyName*/
            ),
        };
        if tsc_ast::is_function_like(self.store().node(container))
            && (!self.is_in_parameter_initializer_before_containing_function(node)
                || has_this_parameter(self, container))
        {
            // Go: getSignatureOfFullSignatureType then
            // getSignatureFromDeclaration; the full-signature-type route is
            // the S6 seam, so the declaration route answers.
            let sig = self.get_signature_from_declaration(container);
            if let Some(sig) = sig {
                let this_type = self.get_this_type_of_signature(sig);
                // Note: a parameter initializer should refer to class-this unless function-this is explicitly annotated.
                // If this is a function in a JS file, it might be a class method.
                let this_type = match this_type {
                    Some(t) => Some(t),
                    None => self.get_contextual_this_parameter_type(container),
                };
                if let Some(this_type) = this_type {
                    // TODO(port M6 W3): getFlowTypeOfReference — the flow
                    // walk; permissive declared type.
                    return Some(this_type);
                }
            }
        }
        let parent = self.store().node(container).parent.get();
        if crate::statements::is_class_like(self.store(), parent) {
            let symbol = self.get_symbol_of_declaration(parent)?;
            if tsc_binder::ast_shims::is_static(self.store().node(container)) {
                return Some(self.get_type_of_symbol(symbol));
            }
            // Go: getDeclaredTypeOfSymbol(symbol).AsInterfaceType().thisType —
            // the thisType construction is the S7 anonymous-type seam;
            // permissive None (the noImplicitThis diagnostic fires, Go's
            // answer when the interface thisType is absent).
            // TODO(port M6 S7): the thisType construction.
            let _ = self.get_declared_type_of_symbol(symbol);
            return None;
        }
        if self.store().node(container).kind == Kind::SourceFile {
            // look up in the source file's locals or exports
            let external_module_indicator = self
                .store()
                .node(container)
                .as_source_file()
                .expect("SourceFile data")
                .external_module_indicator
                .get();
            if external_module_indicator.is_some() {
                // TODO: Maybe issue a better error than 'object is possibly undefined'
                return Some(self.undefined_type());
            }
            if include_global_this {
                let symbol = self.global_this_symbol?;
                return Some(self.get_type_of_symbol(symbol));
            }
        }
        None
    }

    /// Go: `func (c *Checker) isInParameterInitializerBeforeContainingFunction(node) bool`
    /// (checker.go:12473).
    pub(crate) fn is_in_parameter_initializer_before_containing_function(&mut self, node: NodeId) -> bool {
        let mut node = node;
        let mut in_binding_initializer = false;
        loop {
            let parent = self.store().node(node).parent.get();
            if parent == NodeId::NONE || tsc_ast::is_function_like(self.store().node(parent)) {
                return false;
            }
            let parent_kind = self.store().node(parent).kind;
            if parent_kind == Kind::Parameter
                && (in_binding_initializer
                    || crate::ast_shims::initializer_of(self.store(), parent) == Some(node))
            {
                return true;
            }
            if parent_kind == Kind::BindingElement
                && crate::ast_shims::initializer_of(self.store(), parent) == Some(node)
            {
                in_binding_initializer = true;
            }
            node = parent;
        }
    }


    /// Go: `func (c *Checker) checkThisInStaticClassFieldInitializerInDecoratedClass(thisExpression, container)`
    /// (checker.go:12502).
    pub(crate) fn check_this_in_static_class_field_initializer_in_decorated_class(
        &mut self,
        this_expression: NodeId,
        container: NodeId,
    ) {
        if self.store().node(container).kind == Kind::PropertyDeclaration
            && tsc_binder::ast_shims::is_static(self.store().node(container))
            && self.legacy_decorators
        {
            let initializer = crate::ast_shims::initializer_of(self.store(), container);
            if let Some(_initializer) = initializer {
                let pos_in_loc = {
                    let this_pos = self.store().node(this_expression).loc.pos();
                    let loc = &self.store().node(container).loc;
                    loc.contains_inclusive(this_pos)
                };
                let parent = self.store().node(container).parent.get();
                // PORT: Go HasDecorators(parent) — the modifier list carries
                // the decorators in the port (statements.rs modifier_nodes_of).
                let has_decorators = crate::statements::modifier_nodes_of(self.store(), parent)
                    .iter()
                    .any(|m| self.store().node(*m).kind == Kind::Decorator);
                if pos_in_loc && has_decorators {
                    self.error(
                        this_expression,
                        &CANNOT_USE_THIS_IN_A_STATIC_PROPERTY_INITIALIZER_OF_A_DECORATED_CLASS,
                        &[],
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkThisBeforeSuper(node, container, diagnosticMessage)` (checker.go:12513).
    pub(crate) fn check_this_before_super(
        &mut self,
        node: NodeId,
        container: NodeId,
        diagnostic_message: &'static Message,
    ) {
        // Go: the isPostSuperFlowNode flow probe (W3). Until the flow engine
        // lands, the probe answers "post-super" (no error), Go's answer when
        // the reference carries no flow node.
        // TODO(port M6 W3): isPostSuperFlowNode.
        let _ = (node, container, diagnostic_message);
    }
}

/// Go: `GetClassLikeDeclarationOfSymbol(parent)` (ast/utilities.go) — the
/// symbol's class-like declaration.
fn get_class_like_declaration_of_symbol_for(c: &Checker, symbol: SymbolId) -> Option<NodeId> {
    c.program_symbol(symbol)
        .declarations
        .iter()
        .copied()
        .find(|d| crate::statements::is_class_like(c.store(), *d))
}

/// Go: `leftType.symbol` of checkPropertyAccessExpressionOrQualifiedName —
/// the type's symbol slot for the JS-suggestion probe.
fn left_type_symbol(c: &Checker, left_type: TypeId) -> Option<SymbolId> {
    c.type_(left_type).symbol
}

/// Go: `func GetThisParameter(fn *Node) *Node` (ast/utilities.go) — the
/// first parameter named `this` of a function-like node.
fn has_this_parameter(c: &Checker, fn_node: NodeId) -> bool {
    let n = c.store().node(fn_node);
    if !tsc_ast::is_function_like(n) {
        return false;
    }
    let parameters = crate::ast_shims::parameters_of(c.store(), fn_node);
    parameters
        .first()
        .copied()
        .is_some_and(|p| {
            let name = c.store().node(p).name();
            matches!(name, Some(id) if tsc_ast::node_text(c.store(), id) == "this")
        })
}
