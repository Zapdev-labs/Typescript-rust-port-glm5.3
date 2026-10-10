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
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CONSTANT, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CLASS,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_ENUM, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_FUNCTION,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_IMPORT, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_NAMESPACE,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY,
    CANNOT_ASSIGN_TO_PRIVATE_METHOD_0_PRIVATE_METHODS_ARE_NOT_WRITABLE,
    CANNOT_EXTEND_AN_INTERFACE_0_DID_YOU_MEAN_IMPLEMENTS,
    CANNOT_USE_THIS_IN_A_STATIC_PROPERTY_INITIALIZER_OF_A_DECORATED_CLASS,
    CLASS_0_USED_BEFORE_ITS_DECLARATION,
    CLASS_FIELD_0_DEFINED_BY_THE_PARENT_CLASS_IS_NOT_ACCESSIBLE_IN_THE_CHILD_CLASS_VIA_SUPER,
    DID_YOU_MEAN_0, ELEMENT_IMPLICITLY_HAS_AN_ANY_TYPE_BECAUSE_TYPE_0_HAS_NO_INDEX_SIGNATURE,
    EXPONENTIATION_CANNOT_BE_PERFORMED_ON_BIGINT_VALUES_UNLESS_THE_TARGET_OPTION_IS_SET_TO_ES2016_OR_LATER,
    IMPLICIT_CONVERSION_OF_A_SYMBOL_TO_A_STRING_WILL_FAIL_AT_RUNTIME_CONSIDER_WRAPPING_THIS_EXPRESSION_IN_STRING,
    INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING,
    LEFT_SIDE_OF_COMMA_OPERATOR_IS_UNUSED_AND_HAS_NO_SIDE_EFFECTS,
    META_PROPERTY_0_IS_ONLY_ALLOWED_IN_THE_BODY_OF_A_FUNCTION_DECLARATION_FUNCTION_EXPRESSION_OR_CONSTRUCTOR,
    NO_VALUE_EXISTS_IN_SCOPE_FOR_THE_SHORTHAND_PROPERTY_0_EITHER_DECLARE_ONE_OR_PROVIDE_AN_INITIALIZER,
    OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPE_1, OPERATOR_0_CANNOT_BE_APPLIED_TO_TYPES_1_AND_2,
    PRIVATE_ACCESSOR_WAS_DEFINED_WITHOUT_A_GETTER,
    PRIVATE_FIELD_0_MUST_BE_DECLARED_IN_AN_ENCLOSING_CLASS,
    PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
    PRIVATE_IDENTIFIERS_CANNOT_BE_USED_IN_DESTRUCTURING_PATTERNS,
    PROPERTY_0_COMES_FROM_AN_INDEX_SIGNATURE_SO_IT_MUST_BE_ACCESSED_WITH_0,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1, PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_TO_ACCESS_THE_STATIC_MEMBER_2_INSTEAD,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_2_OR_LATER,
    PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_INCLUDE_DOM,
    PROPERTY_0_IS_PRIVATE_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1,
    PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_THROUGH_AN_INSTANCE_OF_CLASS_1_THIS_IS_AN_INSTANCE_OF_CLASS_2,
    PROPERTY_0_IS_PROTECTED_AND_ONLY_ACCESSIBLE_WITHIN_CLASS_1_AND_ITS_SUBCLASSES,
    PROPERTY_0_IS_USED_BEFORE_BEING_ASSIGNED, PROPERTY_0_IS_USED_BEFORE_ITS_INITIALIZATION,
    PROPERTY_0_MAY_NOT_EXIST_ON_TYPE_1_DID_YOU_MEAN_2,
    PROPERTY_ASSIGNMENT_EXPECTED,
    RIGHT_OPERAND_OF_IS_UNREACHABLE_BECAUSE_THE_LEFT_OPERAND_IS_NEVER_NULLISH,
    SPREAD_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES,
    SUPER_CALLS_ARE_NOT_PERMITTED_OUTSIDE_CONSTRUCTORS_OR_IN_NESTED_FUNCTIONS_INSIDE_CONSTRUCTORS,
    THE_0_OPERATOR_CANNOT_BE_APPLIED_TO_TYPE_SYMBOL,
    THE_0_OPERATOR_IS_NOT_ALLOWED_FOR_BOOLEAN_TYPES_CONSIDER_USING_1_INSTEAD,
    THE_CONTAINING_ARROW_FUNCTION_CAPTURES_THE_GLOBAL_VALUE_OF_THIS,
    THE_IMPORT_META_META_PROPERTY_IS_NOT_ALLOWED_IN_FILES_WHICH_WILL_BUILD_INTO_COMMONJS_OUTPUT,
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
    THE_RIGHT_HAND_SIDE_OF_AN_ARITHMETIC_OPERATION_MUST_BE_OF_TYPE_ANY_NUMBER_BIGINT_OR_AN_ENUM_TYPE,
    THE_RIGHT_HAND_SIDE_OF_AN_INSTANCEOF_EXPRESSION_MUST_NOT_BE_AN_INSTANTIATION_EXPRESSION,
    THE_SHADOWING_DECLARATION_OF_0_IS_DEFINED_HERE,
    THE_DECLARATION_OF_0_THAT_YOU_PROBABLY_INTENDED_TO_USE_IS_DEFINED_HERE,
    THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_TARGET_OF_AN_OBJECT_REST_ASSIGNMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THIS_CONDITION_WILL_ALWAYS_RETURN_0,
    THIS_CONDITION_WILL_ALWAYS_RETURN_0_SINCE_JAVASCRIPT_COMPARES_OBJECTS_BY_REFERENCE_NOT_VALUE,
    THIS_COMPARISON_APPEARS_TO_BE_UNINTENTIONAL_BECAUSE_THE_TYPES_0_AND_1_HAVE_NO_OVERLAP,
    THIS_EXPRESSION_IS_ALWAYS_NULLISH, THIS_KIND_OF_EXPRESSION_IS_ALWAYS_FALSY,
    THIS_KIND_OF_EXPRESSION_IS_ALWAYS_TRUTHY, THIS_OPERATION_CAN_BE_SIMPLIFIED_THIS_SHIFT_IS_IDENTICAL_TO_0_1_2,
    THIS_SPREAD_ALWAYS_OVERWRITES_THIS_PROPERTY,
    THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
    THIS_SYNTAX_IS_RESERVED_IN_FILES_WITH_THE_MTS_OR_CTS_EXTENSION_USE_AN_AS_EXPRESSION_INSTEAD,
    TYPE_0_CANNOT_BE_USED_TO_INDEX_TYPE_1,
    TYPE_0_DOES_NOT_SATISFY_THE_EXPECTED_TYPE_1,
    TYPE_0_HAS_NO_SIGNATURES_FOR_WHICH_THE_TYPE_ARGUMENT_LIST_IS_APPLICABLE,
    TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPE_OF_THE_TARGET,
    TYPE_0_MAY_REPRESENT_A_PRIMITIVE_VALUE_WHICH_IS_NOT_PERMITTED_AS_THE_RIGHT_OPERAND_OF_THE_IN_OPERATOR,
    UNUSED_LABEL,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE, VARIABLE_0_IMPLICITLY_HAS_TYPE_1_IN_SOME_LOCATIONS_WHERE_ITS_TYPE_CANNOT_BE_DETERMINED,
    VARIABLE_0_IS_USED_BEFORE_BEING_ASSIGNED,
    X_0_AND_1_OPERATIONS_CANNOT_BE_MIXED_WITHOUT_PARENTHESES,
    X_0_IS_DECLARED_HERE, X_0_IS_SPECIFIED_MORE_THAN_ONCE_SO_THIS_USAGE_WILL_BE_OVERWRITTEN,
    X_ARGUMENTS_CANNOT_BE_REFERENCED_IN_PROPERTY_INITIALIZERS_OR_CLASS_STATIC_INITIALIZATION_BLOCKS,
    X_SUPER_CAN_ONLY_BE_REFERENCED_IN_A_DERIVED_CLASS,
    X_SUPER_CAN_ONLY_BE_REFERENCED_IN_MEMBERS_OF_DERIVED_CLASSES_OR_OBJECT_LITERAL_EXPRESSIONS,
    X_SUPER_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME,
    X_SUPER_CANNOT_BE_REFERENCED_IN_CONSTRUCTOR_ARGUMENTS,
    X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_A_PROPERTY_OF_SUPER_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
    X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_THIS_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
    X_SUPER_PROPERTY_ACCESS_IS_PERMITTED_ONLY_IN_A_CONSTRUCTOR_MEMBER_FUNCTION_OR_MEMBER_ACCESSOR_OF_A_DERIVED_CLASS,
    X_THIS_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME,
    X_THIS_CANNOT_BE_REFERENCED_IN_CURRENT_LOCATION,
    X_THIS_CANNOT_BE_REFERENCED_IN_A_MODULE_OR_NAMESPACE_BODY,
    X_THIS_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_TYPE_ANNOTATION,
    X_YIELD_EXPRESSION_IMPLICITLY_RESULTS_IN_AN_ANY_TYPE_BECAUSE_ITS_CONTAINING_GENERATOR_LACKS_A_RETURN_TYPE_ANNOTATION,
};
use tsc_diagnostics as diag;

use crate::checker::CheckMode;
use crate::relater::{self, TypeFacts};
use crate::types::{ObjectFlags, SignatureKind, TypeFlags, TypeId};
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
        let store = self.store();
        let left_parent = store.node(left).parent.get();
        // We ignore assignments of undefined to CommonJS exports when there are multiple assignment declarations
        if is_declaration_node(store.node(left_parent))
            && tsc_ast::get_assignment_declaration_kind(store, left_parent)
                == tsc_ast::JSDeclarationKind::ExportsProperty
        {
            if let Some(symbol) = self.symbol_node_links.get(left).resolved_symbol
                && self.program_symbol(symbol).declarations.len() > 1
                && self.type_(right_type).flags.intersects(TypeFlags::UNDEFINED)
            {
                return;
            }
        }
        // getters can be a subtype of setters, so to check for assignability we use the setter's type instead
        if tsc_ast::is_compound_assignment_operator(operator)
            && store.node(left).kind == Kind::PropertyAccessExpression
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
            self.error_and_maybe_suggest_await(
                operand,
                awaited_type.is_some()
                    && awaited_type.is_some_and(|t| {
                        self.is_type_assignable_to(t, number_or_big_int_type)
                    }),
                diagnostic,
                &[],
            );
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
        if !crate::utilities::is_type_any(Some(&self.type_(left_type)))
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
            self.type_(x).flags.intersects(TypeFlags::INTERSECTION)
                && self
                    .is_empty_anonymous_object_type(relater::get_base_constraint_or_type(self, x))
        })
    }

    /// Go: `func (c *Checker) getExactOptionalUnassignableProperties(source, target) []*ast.Symbol`
    /// (checker.go:13361). TODO(port M6 S8): the structured-member property
    /// walk (getPropertyOfType + getTypeOfSymbol per property); permissive
    /// empty (no unassignable properties at this depth).
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
            let file_name = self
                .store()
                .node(self.store().node(node).parent.get())
                .file()
                .and_then(|f| self.source_file_of(f))
                .map(|f| f.file_name().as_string().to_string());
            // TODO(port M6 W3): the mts/cts extension-reserved grammar error
            // (grammarchecks.go's file-extension probe) — permissive skip.
            let _ = file_name;
            // TODO(port M6 W3): shouldCheckErasableSyntax (grammarchecks.go).
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
        if tsc_ast::is_logical_or_coalescing_binary_operator(operator) {
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
                let result_type;
                // If both are any or unknown, allow operation; assume it will resolve to number
                if (self.is_type_assignable_to_kind(left_type, TypeFlags::ANY_OR_UNKNOWN)
                    && self.is_type_assignable_to_kind(right_type, TypeFlags::ANY_OR_UNKNOWN))
                    || (!relater::maybe_type_of_kind(self, left_type, TypeFlags::BIGINT_LIKE)
                        && !relater::maybe_type_of_kind(self, right_type, TypeFlags::BIGINT_LIKE))
                {
                    result_type = self.number_type();
                } else if self.both_are_big_int_like(left_type, right_type) {
                    match operator {
                        Kind::GreaterThanGreaterThanGreaterThanToken
                        | Kind::GreaterThanGreaterThanGreaterThanEqualsToken => {
                            self.report_operator_error(left_type, operator, right_type, error_node, None);
                        }
                        Kind::AsteriskAsteriskToken | Kind::AsteriskAsteriskEqualsToken => {
                            if self.language_version < tsc_core::options_generated::ScriptTarget::ES2016 {
                                self.error(
                                    error_node.expect("non-nil errorNode on this path"),
                                    &EXPONENTIATION_CANNOT_BE_PERFORMED_ON_BIGINT_VALUES_UNLESS_THE_TARGET_OPTION_IS_SET_TO_ES2016_OR_LATER,
                                    &[],
                                );
                            }
                        }
                        _ => {}
                    }
                    result_type = self.bigint_type();
                } else {
                    self.report_operator_error(left_type, operator, right_type, error_node, Some(Checker::both_are_big_int_like));
                    result_type = self.error_type();
                }
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
                } else if u::is_type_any(Some(&self.type_(left_type)))
                    || u::is_type_any(Some(&self.type_(right_type)))
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
                    let close_enough_kind = TypeFlags::NUMBER_LIKE
                        .union(TypeFlags::BIGINT_LIKE)
                        .union(TypeFlags::STRING_LIKE)
                        .union(TypeFlags::ANY_OR_UNKNOWN);
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
                    left_type = self.get_base_type_of_literal_type_for_comparison(
                        self.check_non_null_type(left_type, left),
                    );
                    right_type = self.get_base_type_of_literal_type_for_comparison(
                        self.check_non_null_type(right_type, right),
                    );
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
                        && (!binder_shims_is_in_js_file(self.store(), left)
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
                        crate::todo_shapes::UnionReduction::SUBTYPE,
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
                    result_type = self.get_union_type_ex(
                        &[self.get_non_nullable_type(left_type), right_type],
                        crate::todo_shapes::UnionReduction::SUBTYPE,
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
                let allow_unreachable = self.options().allow_unreachable_code == Tristate::TSTrue;
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
                if self.strict_null_checks
                    && !self.has_type_facts(self.check_expression(initializer), TypeFacts::IS_UNDEFINED)
                {
                    source_type = relater::get_type_with_facts(self, source_type, TypeFacts::NE_UNDEFINED);
                }
                self.check_binary_like_expression(name, equals_token, initializer, check_mode, None);
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
            let name = object_literal_element_name(self.store(), property)
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
            let element_type = self.get_indexed_access_type_ex(
                object_literal_type,
                expr_type,
                crate::types::AccessFlags::EXPRESSION_POSITION
                    | if self.has_default_value(property) {
                        crate::types::AccessFlags::ALLOW_MISSING
                    } else {
                        crate::types::AccessFlags::NONE
                    },
                Some(name),
                None,
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
                            object_literal_element_name(self.store(), *other_property)
                                .expect("property has a name"),
                        );
                    }
                }
            }
            let symbol = self.type_(object_literal_type).symbol;
            let t = self.get_rest_type(object_literal_type, non_rest_names, symbol);
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
        let no_unchecked_indexed_access = self.options().no_unchecked_indexed_access == Tristate::TSTrue;
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
                        Some(&elements),
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

    /// Go: `func (c *Checker) is_tuple_type(t) bool` (checker.go — the
    /// spike's arena helper; S2 owns the body).
    pub(crate) fn is_tuple_type(&self, t: TypeId) -> bool {
        self.is_tuple_type(t)
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
    if u::is_type_any(Some(&c.type_(left))) || u::is_type_any(Some(&c.type_(right))) {
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

