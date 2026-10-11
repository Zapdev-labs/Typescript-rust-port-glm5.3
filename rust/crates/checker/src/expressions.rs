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
use tsc_binder::ast_shims as binder_shims;
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
    TYPE_OF_AWAIT_OPERAND_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
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
    // tranche 2
    A_CONST_ENUM_MEMBER_CAN_ONLY_BE_ACCESSED_USING_A_STRING_LITERAL,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CLASS, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CONSTANT,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_FUNCTION, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_NAMESPACE,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_ENUM, CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_IMPORT,
    CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_NOT_A_VARIABLE,
    IMPLICIT_CONVERSION_OF_A_SYMBOL_TO_A_STRING_WILL_FAIL_AT_RUNTIME_CONSIDER_WRAPPING_THIS_EXPRESSION_IN_STRING,
    INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING,
    NO_VALUE_EXISTS_IN_SCOPE_FOR_THE_SHORTHAND_PROPERTY_0_EITHER_DECLARE_ONE_OR_PROVIDE_AN_INITIALIZER,
    OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1,
    PRIVATE_OR_PROTECTED_MEMBER_0_CANNOT_BE_ACCESSED_ON_A_TYPE_PARAMETER,
    SPREAD_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES,
    SUPER_CALLS_ARE_NOT_PERMITTED_OUTSIDE_CONSTRUCTORS_OR_IN_NESTED_FUNCTIONS_INSIDE_CONSTRUCTORS,
    CANNOT_FIND_NAME_0, CANNOT_FIND_NAME_0_DID_YOU_MEAN_TO_WRITE_THIS_IN_AN_ASYNC_FUNCTION,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_1_OR_LATER,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_INCLUDE_DOM,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_A_TEST_RUNNER_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJEST_OR_NPM_I_SAVE_DEV_TYPES_SLASHMOCHA,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_A_TEST_RUNNER_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJEST_OR_NPM_I_SAVE_DEV_TYPES_SLASHMOCHA_AND_THEN_ADD_JEST_OR_MOCHA_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_BUN_TRY_NPM_I_SAVE_DEV_TYPES_SLASHBUN,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_BUN_TRY_NPM_I_SAVE_DEV_TYPES_SLASHBUN_AND_THEN_ADD_BUN_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_JQUERY_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJQUERY,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_JQUERY_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJQUERY_AND_THEN_ADD_JQUERY_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE,
    CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE_AND_THEN_ADD_NODE_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG,
    THIS_SPREAD_ALWAYS_OVERWRITES_THIS_PROPERTY,
    TYPE_0_CANNOT_BE_USED_TO_INDEX_TYPE_1,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE,
    VARIABLE_0_IMPLICITLY_HAS_TYPE_1_IN_SOME_LOCATIONS_WHERE_ITS_TYPE_CANNOT_BE_DETERMINED,
    VARIABLE_0_IS_USED_BEFORE_BEING_ASSIGNED,
    X_0_IS_SPECIFIED_MORE_THAN_ONCE_SO_THIS_USAGE_WILL_BE_OVERWRITTEN,
    X_ARGUMENTS_CANNOT_BE_REFERENCED_IN_PROPERTY_INITIALIZERS_OR_CLASS_STATIC_INITIALIZATION_BLOCKS,
    X_SUPER_CAN_ONLY_BE_REFERENCED_IN_A_DERIVED_CLASS,
    X_SUPER_CAN_ONLY_BE_REFERENCED_IN_MEMBERS_OF_DERIVED_CLASSES_OR_OBJECT_LITERAL_EXPRESSIONS,
    X_SUPER_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME,
    X_SUPER_CANNOT_BE_REFERENCED_IN_CONSTRUCTOR_ARGUMENTS,
    X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_A_PROPERTY_OF_SUPER_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
    X_SUPER_PROPERTY_ACCESS_IS_PERMITTED_ONLY_IN_A_CONSTRUCTOR_MEMBER_FUNCTION_OR_MEMBER_ACCESSOR_OF_A_DERIVED_CLASS,
};

use crate::checker::CheckMode;
use crate::relater::{self, MappedTypeModifiers, TypeFacts};
use crate::types::{ObjectFlags, TypeFlags, TypeId};
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
        // REAL since M6 S7: the accessor is infallible (the Option shape
        // stays for the caller contract); the assignability gate is live.
        let Some(number_or_big_int_type) = self.number_or_big_int_type() else {
            let _ = (operand, diagnostic);
            return true;
        };
        if !self.is_type_assignable_to(t, number_or_big_int_type) {
            let mut awaited_type = None;
            if is_await_valid {
                awaited_type = self.get_awaited_type_of_promise(t);
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
        // REAL (M6 S10, checker.go:31467): maybeTypeOfKindConsideringBaseConstraint —
        // the constraint-walking variant (index.rs's free fn over the facts-
        // lattice worker's base-constraint read).
        if self.maybe_type_of_kind_considering_base_constraint(left_type, TypeFlags::ESSYMBOL_LIKE) {
            offending_symbol_operand = Some(left);
        } else if self
            .maybe_type_of_kind_considering_base_constraint(right_type, TypeFlags::ESSYMBOL_LIKE)
        {
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
            // REAL (M6 S7): the eagerly-seeded string | number | symbol union
            // intrinsic carries the assignability gate (Go's checkInExpression:
            // checkTypeAssignableTo(checkNonNullType(leftType, left),
            // stringNumberSymbolType, left, nil)).
            let non_null_left = self.check_non_null_type(left_type, left);
            let string_number_symbol = self.string_number_symbol_type();
            self.check_type_assignable_to(non_null_left, string_number_symbol, Some(left), None);
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
    /// `getUnionType([numberType, bigintType])`). REAL since M6 S7: the
    /// seeded number | bigint union intrinsic (callers' assignability gates
    /// are live).
    pub(crate) fn number_or_big_int_type(&mut self) -> Option<TypeId> {
        // REAL (M6 S7): the eagerly-seeded number | bigint union intrinsic
        // (new_checker, mirroring checker.go's initializeChecker).
        Some(self.number_or_bigint_type())
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
        // resolvecall.rs's real getResolvedSignature (S6's call resolution).
        self.get_resolved_signature_ex(node, candidates_out_array, check_mode)
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
        let expr_type = {
            let base = self.get_base_type_of_literal_type(expr_type);
            relater::get_regular_type_of_object_literal(self, base)
        };
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
                    // REAL (M6 S10): extractDefinitelyFalsyTypes — the
                    // facts-lattice extract.
                    let extracted = self.extract_definitely_falsy_types(t);
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
                    // REAL (M6 S10): removeDefinitelyFalsyTypes — the
                    // facts-lattice filter (truthy-only constituents).
                    let removed = self.remove_definitely_falsy_types(left_type);
                    let non_falsy = self.get_non_nullable_type(removed);
                    result_type = self.get_union_type_ex(
                        vec![non_falsy, right_type],
                        crate::todo_shapes::UnionReduction::Subtype,
                        None,
                        None,
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
                        vec![left_non_nullable, right_type],
                        crate::todo_shapes::UnionReduction::Subtype,
                        None,
                        None,
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
                            self.store().node(*other_property).name()
                                .expect("property has a name"),
                        );
                    }
                }
            }
            let symbol = self.type_(object_literal_type).symbol;
            let t = self.get_rest_type(object_literal_type, &non_rest_names, symbol);
            // Go: checkGrammarForDisallowedTrailingComma(allProperties, …) —
            // allProperties is the object literal's own properties NodeList.
            let properties_list = self
                .store()
                .node(node)
                .as_object_literal_expression()
                .and_then(|d| d.properties.clone());
            self.check_grammar_for_disallowed_trailing_comma(
                properties_list.as_ref(),
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
                    // TODO(port M6 W4): createSyntheticExpression — the
                    // synthetic node factory (nodecopy.go's wave); the element
                    // node stands in for the access node (error positions
                    // only). The S10 indexed-access consumers are real.
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
                    // Go: checkGrammarForDisallowedTrailingComma(elements, …) —
                    // the array literal's own elements NodeList.
                    let elements_list = self
                        .store()
                        .node(node)
                        .as_array_literal_expression()
                        .and_then(|d| d.elements.clone());
                    self.check_grammar_for_disallowed_trailing_comma(
                        elements_list.as_ref(),
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
        // REAL (M6 S7): typeofType — the seeded literal-word union
        // (Go checker.go:1063 / checkTypeOfExpression checker.go:10850).
        self.typeof_type()
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
    /// Go: `func (c *Checker) getOptionalExpressionType(exprType, expression) *Type`
    /// (checker.go:29593) — REAL as of W3T6 (was the S9 passthrough stub;
    /// rides the now-real GetNonNullableType).
    pub(crate) fn get_optional_expression_type(&mut self, expr_type: TypeId, expression: NodeId) -> TypeId {
        if tsc_binder::ast_shims::is_expression_of_optional_chain_root(self.store(), expression) {
            self.get_non_nullable_type(expr_type)
        } else if tsc_ast::utilities::is_optional_chain(self.store().node(expression)) {
            self.remove_optional_type_marker(expr_type)
        } else {
            expr_type
        }
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
        // Go: `node.TypeArguments()` — both ExpressionWithTypeArguments and
        // TypeQuery (the instantiation-expression form `typeof f<number>`)
        // carry the shared optional typeArguments list.
        let n = self.store().node(node);
        let type_arguments = if n.kind == Kind::ExpressionWithTypeArguments {
            n.as_expression_with_type_arguments()
                .expect("ExpressionWithTypeArguments data")
                .type_arguments
                .clone()
        } else {
            n.as_type_query_node()
                .expect("TypeQueryNode data")
                .type_arguments
                .clone()
        };
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
        // Go: checkGrammarMetaProperty(node) runs before the dispatch.
        self.check_grammar_meta_property(node);
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
        // W3T7 PROBE (temporary): 2703 span divergence on await_unary cases.
        {
            let file = self.store().file(node.file_id());
            let text = file.text();
            let file_name = file.file_name().as_string().to_string();
            let pe = self.store().node(expr).pos() as usize;
            let ee = self.store().node(expr).end() as usize;
            // The diag span: get_error_range_for_node skips leading trivia.
            let mut lo = pe;
            while lo < ee && text.as_bytes()[lo].is_ascii_whitespace() {
                lo += 1;
            }
            eprintln!(
                "w3t7probe-rust file={} expr={}..{} diag={}..{} exprText={:?} diagText={:?}",
                file_name,
                pe,
                ee,
                lo,
                ee,
                &text.get(pe..ee.min(text.len())).unwrap_or("<oob>"),
                &text.get(lo..ee.min(text.len())).unwrap_or("<oob>")
            );
        }
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
            // Go checker.go:11084: the AWAIT message (TS1320), not the
            // async-return message (1058) — fixed in W3T6.
            &TYPE_OF_AWAIT_OPERAND_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
        );
        if awaited_type == operand_type
            && !relater::is_error_type(self, awaited_type)
            && !self.type_(operand_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
        {
            self.error_or_suggestion(false, node, &X_AWAIT_HAS_NO_EFFECT_ON_THE_TYPE_OF_THIS_EXPRESSION, &[]);
        }
        awaited_type
    }

    // checkGrammarAwaitOrAwaitUsing (grammarchecks.go:1658) — the REAL body
    // lives in grammarchecks.rs; the checkAwaitExpression leaf discards the
    // bool like Go.

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
                // REAL (M6 S10, checker.go:31467): the constraint walk
                // (maybeTypeOfKindConsideringBaseConstraint).
                if self
                    .maybe_type_of_kind_considering_base_constraint(operand_type, TypeFlags::ESSYMBOL_LIKE)
                {
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
                // Matches Go's checkPrefixUnaryExpression minus arm exactly
                // (checker.go: the any/unknown and numberLike shapes answer
                // numberType; the rest of the bigint-likes answer bigintType).
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
        self.get_union_type_ex(vec![type1, type2], crate::todo_shapes::UnionReduction::Subtype, None, None)
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
                    let contextual_type = self.get_contextual_type(node, crate::types::ContextFlags::NONE);
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
            Some(error_node),
            if is_yield_star {
                Some(&TYPE_OF_ITERATED_ELEMENTS_OF_A_YIELD_ASTERISK_OPERAND_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER)
            } else {
                Some(&TYPE_OF_YIELD_OPERAND_IN_AN_ASYNC_GENERATOR_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER)
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

    // Go: `func (c *Checker) getAwaitedTypeEx(t, errorNode, diagnosticMessage, args...) *Type`
    // (checker.go:31806) — REAL (M6 S9): the body lives in statements.rs with
    // the rest of the awaited family; this stub was removed.
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

    /// Go: `func (c *Checker) markPropertyAsReferenced(prop, nodeForCheckWriteOnly, isSelfTypeAccess)`
    /// (checker.go:28240) — REAL (M6 S10): the symbol-reference recording
    /// (the import-elision seam: only private class members record).
    pub(crate) fn mark_property_as_referenced(
        &mut self,
        prop: SymbolId,
        node_for_check_write_only: Option<NodeId>,
        is_self_type_access: bool,
    ) {
        if !self
            .program_symbol(prop)
            .flags
            .intersects(SymbolFlags::CLASS_MEMBER)
        {
            return;
        }
        let Some(value_declaration) = self.program_symbol(prop).value_declaration else {
            return;
        };
        let has_private_modifier =
            crate::ast_shims::has_modifier(self.store().node(value_declaration), ModifierFlags::PRIVATE);
        let has_private_identifier = self
            .store()
            .node(value_declaration)
            .name()
            .is_some_and(|name| tsc_ast::is_private_identifier(self.store().node(name)));
        if !has_private_modifier && !has_private_identifier {
            return;
        }
        if let Some(node) = node_for_check_write_only
            && is_write_only_access(self.store(), node)
            && !self
                .program_symbol(prop)
                .flags
                .intersects(SymbolFlags::SET_ACCESSOR)
        {
            return;
        }
        if is_self_type_access
            && let Some(node) = node_for_check_write_only
        {
            // Find any FunctionLikeDeclaration because those create a new
            // 'this' binding. But this should only matter for methods (or
            // getters/setters). (Go comment, verbatim.)
            let mut is_function_like = |n: &tsc_ast::Node| {
                matches!(
                    n.kind,
                    Kind::FunctionDeclaration
                        | Kind::FunctionExpression
                        | Kind::ArrowFunction
                        | Kind::MethodDeclaration
                        | Kind::MethodSignature
                        | Kind::GetAccessor
                        | Kind::SetAccessor
                        | Kind::Constructor
                )
            };
            let containing_method =
                crate::ast_shims::find_ancestor(self.store(), node, &mut is_function_like);
            if let Some(containing_method) = containing_method
                && crate::typeofsymbol::get_symbol_of_declaration(self, containing_method) == Some(prop)
            {
                return;
            }
        }
        // Go: instantiated symbols record on their target.
        let target = if self
            .program_symbol(prop)
            .check_flags
            .intersects(tsc_ast::CheckFlags::INSTANTIATED)
        {
            self.value_symbol_links.get(prop).target
        } else {
            Some(prop)
        };
        if let Some(target) = target {
            let kinds = self.symbol_reference_links.get(target).reference_kinds;
            self.symbol_reference_links.get(target).reference_kinds =
                kinds | SymbolFlags::ALL;
        }
    }

    /// Go: `func (c *Checker) markLinkedReferences(location, hint, propSymbol, parentType)`
    /// (checker.go:28723). Permissive no-op — the alias-reference recording
    /// engine (canCollectSymbolAliasAccessibilityData + the mark*AliasReferenced
    /// family) is symbolaccessibility.go's surface.
    pub(crate) fn mark_linked_references(
        &mut self,
        node: NodeId,
        reference_hint: ReferenceHint,
        prop_symbol: Option<SymbolId>,
        left_type: TypeId,
    ) {
        // TODO(port M6 W4): the deferred-reference recording
        // (symbolaccessibility.go's wave; the canCollectSymbolAliasAccessibilityData
        // gate is false until then, which is Go's no-op answer).
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
                // Go: the index-info arm (checker.go:11556) — REAL as of
                // W3T6 (was the "S8 permissive None" seam; getApplicable
                // IndexInfoForName landed with resolveStructuredTypeMembers).
                let right_text = tsc_ast::node_text(self.store(), right).to_string();
                let index_info = if !tsc_ast::is_private_identifier(self.store().node(right))
                    && (assignment_kind == u::AssignmentKind::None
                        || !relater::is_generic_object_type(self, left_type)
                        || u::is_this_type_parameter(self.type_(left_type)))
                {
                    relater::get_applicable_index_info_for_name(self, apparent_type, &right_text)
                } else {
                    None
                };
                if let Some(index_info) = index_info {
                    if self.index_info_is_readonly(index_info)
                        && (tsc_binder::ast_shims::is_assignment_target(self.store(), node)
                            || u::is_delete_target(self.store(), node))
                    {
                        let t_str = self.type_to_string(apparent_type);
                        self.error(
                            node,
                            &INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING,
                            &[t_str],
                        );
                    }
                    let value_type = self.index_info_value_type(index_info);
                    prop_type = if self.compiler_options.no_unchecked_indexed_access == Tristate::TSTrue
                        && u::get_assignment_target_kind(self.store(), node) != u::AssignmentKind::Definite
                    {
                        relater::get_union_type(self, vec![value_type, self.missing_type_id()])
                    } else {
                        value_type
                    };
                    // Go: `NoPropertyAccessFromIndexSignature` — the TS4111
                    // report (checker.go:11599).
                    if self.compiler_options.no_property_access_from_index_signature == Tristate::TSTrue
                        && self.store().node(node).kind == Kind::PropertyAccessExpression
                    {
                        self.error(
                            right,
                            &tsc_diagnostics::PROPERTY_0_COMES_FROM_AN_INDEX_SIGNATURE_SO_IT_MUST_BE_ACCESSED_WITH_0,
                            &[right_text],
                        );
                    }
                    // Go: the deprecated-suggestion arm (checker.go:11602)
                    // rides the deprecation stub (resolution.rs).
                } else {
                    let is_unchecked_js =
                        u::is_unchecked_js_suggestion(self, node, left_type_symbol(self, left_type), true /*excludeClasses*/);
                    if !is_unchecked_js && u::is_js_literal_type(self, left_type) {
                        return self.any_type();
                    }
                    // Go: the globalThis-exports arm — the S6 module seam keeps it
                    // dormant (the exports lookup answers None).
                    if !right_text.is_empty() && !self.check_and_report_error_for_extending_interface(node) {
                        let containing = if u::is_this_type_parameter(self.type_(left_type)) {
                            apparent_type
                        } else {
                            left_type
                        };
                        // M6 S8 landed resolveStructuredTypeMembers, so
                        // getPropertyOfType's None is now Go-faithful "genuinely
                        // missing" — the pre-S8 containing-has-resolved-properties
                        // gate is retired.
                        self.report_nonexistent_property(right, containing, is_unchecked_js);
                    }
                    return self.error_type();
                }
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
    /// (checker.go:11903) — REAL (M6 W3 t7): the entity-name climb +
    /// ignore-errors resolveEntityName that powers the onFailedToResolveSymbol
    /// ladder's `extends SomeInterface` rung.
    pub(crate) fn check_and_report_error_for_extending_interface(&mut self, error_location: NodeId) -> bool {
        if let Some(expression) = self.get_entity_name_for_extending_interface(error_location)
            && self
                .resolve_entity_name(
                    expression,
                    SymbolFlags::INTERFACE,
                    true, /*ignore_errors*/
                    false, /*dont_resolve_alias*/
                    NodeId::NONE,
                )
                .is_some()
        {
            let expression_text = tsc_ast::node_text(self.store(), expression).to_string();
            self.error(
                error_location,
                &tsc_diagnostics::CANNOT_EXTEND_AN_INTERFACE_0_DID_YOU_MEAN_IMPLEMENTS,
                &[expression_text],
            );
            return true;
        }
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
            // PORT (M6 W3 t7): Go's gate is `core.Some(prop.Declarations,
            // isClassInstanceProperty)` — accessors (get/set) are NOT class
            // instance properties, so `super.getter` in async methods stays
            // clean; the port's old symbol_has_non_method_declaration gate
            // wrongly flagged accessors (2855 x2 in asyncMethodWithSuper_es6).
            let declarations = self.program_symbol(prop).declarations.clone();
            if !flags.intersects(ModifierFlags::STATIC)
                && declarations
                    .iter()
                    .any(|&d| u::is_class_instance_property(self.store(), d))
            {
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
        self.get_narrowable_type_for_reference_real(t, reference, check_mode)
    }

    /// Go: `func (c *Checker) containsUndefinedType(t) bool` (checker.go:11431
    /// region) — a union contains undefined iff its FIRST constituent does
    /// (getOptionalType unions put undefinedType first by type id; Go's exact
    /// `t.Types()[0]` probe, PORT W3T4: the former `distributed().any()` was
    /// a misport that fired TS2454 on unions Go's probe rejects).
    pub(crate) fn contains_undefined_type(&mut self, t: TypeId) -> bool {
        let t = if self.type_(t).flags.intersects(TypeFlags::UNION) {
            self.type_(t)
                .as_union_type()
                .map(|u| u.base.types[0])
                .unwrap_or(t)
        } else {
            t
        };
        self.type_(t).flags.intersects(TypeFlags::UNDEFINED)
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
    IDENTIFIER,
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
            // Go: `t = c.getDeclaredTypeOfSymbol(symbol).AsInterfaceType().thisType`
            // (checker.go:12405) — REAL as of W3T6: the declared class/interface
            // type's thisType is constructed by getDeclaredTypeOfClassOrInterface
            // (every class always gets one). Go wraps with
            // getFlowTypeOfReference(node, t); the flow walk is the port's
            // permissive-declared-type seam (same PORT note as the
            // function-like arm above).
            let declared = self.get_declared_type_of_symbol(symbol);
            return self.type_(declared).as_interface_type().expect("InterfaceType data").this_type;
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
pub(crate) fn get_class_like_declaration_of_symbol_for(c: &Checker, symbol: SymbolId) -> Option<NodeId> {
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

// ────────────────────────────────────────────────────────────────────────────
// M6 S4 tranche 2 — super / template / regex / tagged-template /
// qualified-name / parenthesized / indexed-access / array-literal leaves
// (checker.go 8039-8460 + 10259-10292).
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkPrivateIdentifierExpression(node) *Type`
    /// (checker.go:8039).
    pub(crate) fn check_private_identifier_expression(&mut self, node: NodeId) -> TypeId {
        // Grammar checking
        self.check_grammar_private_identifier_expression(node);
        let symbol = self.get_symbol_for_private_identifier_expression(node);
        if let Some(symbol) = symbol {
            self.mark_property_as_referenced(symbol, None /*nodeForCheckWriteOnly*/, false /*isSelfTypeAccess*/);
        }
        self.any_type()
    }

    /// Go: `func (c *Checker) getSymbolForPrivateIdentifierExpression(node) *ast.Symbol`
    /// (checker.go:8045) — the cached private-identifier resolution.
    pub(crate) fn get_symbol_for_private_identifier_expression(&mut self, node: NodeId) -> Option<SymbolId> {
        if self.symbol_node_links.get(node).resolved_symbol.is_none() {
            let text = tsc_ast::node_text(self.store(), node).to_string();
            let symbol = self.lookup_symbol_for_private_identifier_declaration(&text, node);
            self.symbol_node_links.get(node).resolved_symbol = symbol;
            symbol
        } else {
            self.symbol_node_links.get(node).resolved_symbol
        }
    }

    /// Go: `func (c *Checker) checkSuperExpression(node) *Type` (checker.go:8056).
    pub(crate) fn check_super_expression(&mut self, node: NodeId) -> TypeId {
        let parent = self.store().node(node).parent.get();
        let is_call_expression = tsc_ast::is_call_expression(self.store().node(parent))
            && crate::ast_shims::expression_of(self.store(), parent) == Some(node);
        let immediate_container = u::get_super_container(self.store(), node, true /*stopOnFunctions*/);
        let mut container = immediate_container;

        // adjust the container reference in case if super is used inside arrow
        // functions with arbitrarily deep nesting
        if !is_call_expression {
            while let Some(c) = container
                && tsc_ast::is_arrow_function(self.store().node(c))
            {
                container = u::get_super_container(self.store(), c, true /*stopOnFunctions*/);
            }
        }

        let container_ok = container.is_some_and(|c| {
            is_legal_usage_of_super(self.store(), c, is_call_expression)
        });

        if !container_ok {
            // issue more specific error if super is used in computed property name
            // class A { foo() { return "1" }}
            // class B {
            //     [super.foo()]() {}
            // }
            // PORT: Go's FindAncestorOrQuit(node, quit-at-container,
            // true-at-computed-property-name) as an explicit walk (the
            // ancestor ids are compared, not the &Node payloads).
            let mut current: Option<NodeId> = None;
            let mut n = node;
            loop {
                if Some(n) == container {
                    break;
                }
                let nn = self.store().node(n);
                if nn.kind == Kind::ComputedPropertyName {
                    current = Some(n);
                    break;
                }
                let p = nn.parent.get();
                if p.is_none() {
                    break;
                }
                n = p;
            }
            let container_parent = container
                .map(|c| self.store().node(c).parent.get())
                .unwrap_or(NodeId::NONE);
            if current.is_some() {
                self.error(node, &X_SUPER_CANNOT_BE_REFERENCED_IN_A_COMPUTED_PROPERTY_NAME, &[]);
            } else if is_call_expression {
                self.error(
                    node,
                    &SUPER_CALLS_ARE_NOT_PERMITTED_OUTSIDE_CONSTRUCTORS_OR_IN_NESTED_FUNCTIONS_INSIDE_CONSTRUCTORS,
                    &[],
                );
            } else if container.is_none()
                || container_parent.is_none()
                || !(crate::statements::is_class_like(self.store(), container_parent)
                    || self.store().node(container_parent).kind == Kind::ObjectLiteralExpression)
            {
                self.error(
                    node,
                    &X_SUPER_CAN_ONLY_BE_REFERENCED_IN_MEMBERS_OF_DERIVED_CLASSES_OR_OBJECT_LITERAL_EXPRESSIONS,
                    &[],
                );
            } else {
                self.error(
                    node,
                    &X_SUPER_PROPERTY_ACCESS_IS_PERMITTED_ONLY_IN_A_CONSTRUCTOR_MEMBER_FUNCTION_OR_MEMBER_ACCESSOR_OF_A_DERIVED_CLASS,
                    &[],
                );
            }
            return self.error_type();
        }
        let container = container.expect("checked is_some above");
        if !is_call_expression && self.store().node(container).kind == Kind::Constructor {
            self.check_this_before_super(
                node,
                container,
                &X_SUPER_MUST_BE_CALLED_BEFORE_ACCESSING_A_PROPERTY_OF_SUPER_IN_THE_CONSTRUCTOR_OF_A_DERIVED_CLASS,
            );
        }
        let class_like_declaration = self.store().node(container).parent.get();
        if self.store().node(class_like_declaration).kind == Kind::ObjectLiteralExpression {
            // for object literal assume that type of 'super' is 'any'
            return self.any_type();
        }
        // at this point the only legal case for parent is ClassLikeDeclaration
        if crate::statements::get_class_extends_heritage_element(
            self.store(),
            class_like_declaration,
        )
        .is_none()
        {
            self.error(node, &X_SUPER_CAN_ONLY_BE_REFERENCED_IN_A_DERIVED_CLASS, &[]);
            return self.error_type();
        }
        if self.class_declaration_extends_null(class_like_declaration) {
            if is_call_expression {
                return self.error_type();
            }
            return self.null_widening_type();
        }
        let class_symbol = match self.get_symbol_of_declaration(class_like_declaration) {
            Some(s) => s,
            // PORT: Go asserts the class-like has a symbol; a binder-less
            // node answers Go's errorType shape.
            None => return self.error_type(),
        };
        let class_type = self.get_declared_type_of_symbol(class_symbol);
        let base_class_type = self.get_base_types(class_type).into_iter().next();
        let Some(base_class_type) = base_class_type else {
            return self.error_type();
        };
        if self.store().node(container).kind == Kind::Constructor
            && self.is_in_constructor_argument_initializer(node, container)
        {
            // issue custom error message for super property access in constructor arguments (to be aligned with old compiler)
            self.error(node, &X_SUPER_CANNOT_BE_REFERENCED_IN_CONSTRUCTOR_ARGUMENTS, &[]);
            return self.error_type();
        }
        if tsc_binder::ast_shims::is_static(self.store().node(container)) || is_call_expression {
            if !is_call_expression
                && self.language_version
                    <= tsc_core::options_generated::ScriptTarget::ES2021
                && matches!(
                    self.store().node(container).kind,
                    Kind::PropertyDeclaration | Kind::ClassStaticBlockDeclaration
                )
            {
                // for `super.x` or `super[x]` in a static initializer, mark all
                // enclosing block scope containers so that we can report potential
                // collisions with `Reflect`.
                // TODO(port M6 W3): GetEnclosingBlockScopeContainer walk +
                // the ContainsSuperPropertyInStaticInitializer flag (the
                // grammarchecks consumer is W3's); permissive skip.
            }
            return self.get_base_constructor_type_of_class(class_type);
        }
        let this_arg = self
            .type_(class_type)
            .as_interface_type()
            .and_then(|d| d.this_type);
        relater::get_type_with_this_argument(self, base_class_type, this_arg, false)
    }

    /// Go: `func (c *Checker) isInConstructorArgumentInitializer(node, constructorDecl) bool`
    /// (checker.go:8199).
    pub(crate) fn is_in_constructor_argument_initializer(
        &self,
        node: NodeId,
        constructor_decl: NodeId,
    ) -> bool {
        crate::ast_shims::find_ancestor_or_quit(self.store(), node, &mut |n| {
            if tsc_ast::is_function_like(n) {
                crate::ast_shims::FindAncestorResult::Quit
            } else if n.kind == Kind::Parameter && n.parent.get() == constructor_decl {
                crate::ast_shims::FindAncestorResult::True
            } else {
                crate::ast_shims::FindAncestorResult::False
            }
        })
        .is_some()
    }

    /// Go: `func (c *Checker) classDeclarationExtendsNull(classDecl) bool` (checker.go:12519).
    pub(crate) fn class_declaration_extends_null(&mut self, class_decl: NodeId) -> bool {
        let class_symbol = match self.get_symbol_of_declaration(class_decl) {
            Some(s) => s,
            None => return false,
        };
        let class_instance_type = self.get_declared_type_of_symbol(class_symbol);
        let base_constructor_type = self.get_base_constructor_type_of_class(class_instance_type);
        base_constructor_type == self.null_widening_type()
    }

    /// Go: `func (c *Checker) checkTemplateExpression(node) *Type` (checker.go:8145).
    pub(crate) fn check_template_expression(&mut self, node: NodeId) -> TypeId {
        let (head, spans) = {
            let d = self
                .store()
                .node(node)
                .as_template_expression()
                .expect("TemplateExpression data");
            (d.head, d.template_spans.clone())
        };
        let length = spans.as_ref().map(|l| l.nodes.len()).unwrap_or(0);
        let mut texts: Vec<String> = Vec::with_capacity(length + 1);
        let mut types: Vec<TypeId> = Vec::with_capacity(length);
        texts.push(tsc_ast::node_text(self.store(), head).to_string());
        for span in spans.iter().flat_map(|l| l.nodes.iter().copied()) {
            let (expression, literal) = {
                let d = self
                    .store()
                    .node(span)
                    .as_template_span()
                    .expect("TemplateSpan data");
                (d.expression, d.literal)
            };
            let t = self.check_expression(expression);
            if self.maybe_type_of_kind_considering_base_constraint(
                t,
                TypeFlags::ESSYMBOL_LIKE,
            ) {
                self.error(
                    expression,
                    &IMPLICIT_CONVERSION_OF_A_SYMBOL_TO_A_STRING_WILL_FAIL_AT_RUNTIME_CONSIDER_WRAPPING_THIS_EXPRESSION_IN_STRING,
                    &[],
                );
            }
            texts.push(tsc_ast::node_text(self.store(), literal).to_string());
            let constrained = {
                let constraint = self.template_constraint_type();
                self.is_type_assignable_to(t, constraint)
            };
            types.push(if constrained { t } else { self.string_type() });
        }
        // Go: the evaluate() constant-folding probe (the const-eval engine).
        let evaluated: Option<String> = {
            let parent = self.store().node(node).parent.get();
            if self.store().node(parent).kind != Kind::TaggedTemplateExpression {
                // TODO(port M6 S4 follow-up): the evaluate() fold (the
                // const-eval engine's own sub-port); permissive none (Go's
                // answer when the fold yields no value).
                None
            } else {
                None
            }
        };
        if let Some(value) = evaluated {
            let literal = self.get_string_literal_type(&value);
            return self.get_fresh_type_of_literal_type(literal);
        }
        let contextual = self
            .get_contextual_type(node, crate::types::ContextFlags::NONE)
            .unwrap_or_else(|| self.unknown_type());
        // Go: someType(contextualType, isTemplateLiteralContextualType) —
        // PORT: the constituent walk inlined (the closure captures &mut self).
        let is_contextual_template = if self.type_(contextual).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let constituents = self.type_(contextual).types().to_vec();
            constituents
                .iter()
                .any(|&t| self.is_template_literal_contextual_type(t))
        } else {
            self.is_template_literal_contextual_type(contextual)
        };
        if self.is_const_context(node)
            || self.is_template_literal_context(node)
            || is_contextual_template
        {
            return relater::get_template_literal_type(self, &texts, &types);
        }
        self.string_type()
    }

    /// Go: `func (c *Checker) isTemplateLiteralContext(node) bool` (checker.go:8169).
    pub(crate) fn is_template_literal_context(&self, node: NodeId) -> bool {
        let parent = self.store().node(node).parent.get();
        if self.store().node(parent).kind == Kind::ParenthesizedExpression {
            self.is_template_literal_context(parent)
        } else { self.store().node(parent).kind == Kind::ElementAccessExpression && self
                .store()
                .node(parent)
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .argument_expression
                == node }
    }

    /// Go: `func (c *Checker) isTemplateLiteralContextualType(t) bool` (checker.go:8173).
    pub(crate) fn is_template_literal_contextual_type(&mut self, t: TypeId) -> bool {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::STRING_LITERAL.union(TypeFlags::TEMPLATE_LITERAL)) {
            return true;
        }
        if flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            let constraint =
                relater::get_base_constraint_of_type(self, t).unwrap_or_else(|| self.unknown_type());
            return relater::maybe_type_of_kind(self, constraint, TypeFlags::STRING_LIKE);
        }
        false
    }

    // Go: the `templateConstraintType` field (checker.go's initializeChecker —
    // `getUnionType([stringType, numberType, booleanType, bigintType,
    // nullType, undefinedType])`) — REMOVED (M6 S7): the eagerly-constructed
    // union intrinsic landed with new_checker (checker.rs's intrinsic
    // accessor carries the field).


    /// Go: `func (c *Checker) maybeTypeOfKindConsideringBaseConstraint(t, kind) bool`
    /// (checker.go:26023 region). REAL body.
    pub(crate) fn maybe_type_of_kind_considering_base_constraint(
        &mut self,
        t: TypeId,
        kind: TypeFlags,
    ) -> bool {
        if relater::maybe_type_of_kind(self, t, kind) {
            return true;
        }
        let base_constraint = relater::get_base_constraint_or_type(self, t);
        base_constraint != self.never_type()
            && relater::maybe_type_of_kind(self, base_constraint, kind)
    }

    /// Go: `func (c *Checker) checkRegularExpressionLiteral(node) *Type` (checker.go:8185).
    pub(crate) fn check_regular_expression_literal(&mut self, node: NodeId) -> TypeId {
        if !self
            .node_links
            .get(node)
            .flags
            .intersects(crate::types::NodeCheckFlags::TYPE_CHECKED)
        {
            self.node_links.get(node).flags |= crate::types::NodeCheckFlags::TYPE_CHECKED;
            self.check_grammar_regular_expression_literal(node);
        }
        self.global_regexp_type.expect("globalRegExpType")
    }

    /// Go: `func (c *Checker) checkTaggedTemplateExpression(node) *Type` (checker.go:10259).
    pub(crate) fn check_tagged_template_expression(&mut self, node: NodeId) -> TypeId {
        // Go: if !checkGrammarTaggedTemplateChain(node) {
        //         c.checkGrammarTypeArguments(node, node.TypeArgumentList()) }
        if !self.check_grammar_tagged_template_chain(node) {
            let type_arguments =
                crate::grammarchecks::type_argument_list_of(self.store(), node).cloned();
            self.check_grammar_type_arguments(node, type_arguments.as_ref());
        }
        // TODO(port M6 S6): getResolvedSignature(node, nil, CheckModeNormal)
        // — resolveTaggedTemplate shares S6's overload-resolution engine;
        // the seam composes Go's errorSignature shape (silent-never return).
        let signature = self.get_resolved_signature_stub(node, None, CheckMode::CHECK_MODE_NORMAL);
        self.check_deprecated_signature(signature, node);
        match signature {
            Some(sig) => self.get_return_type_of_signature(sig),
            // Go: the error call's return type (errorType at this depth).
            None => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) checkDeprecatedSignature(sig, node)` (checker.go:8571).
    ///
    /// TODO(port M6 S6): the deprecation family (isDeprecatedSymbol /
    /// addDeprecatedSuggestionWithSignature — the @deprecated JSDoc walk);
    /// permissive no-op (diagnostics-neutral).
    pub(crate) fn check_deprecated_signature(
        &mut self,
        sig: Option<crate::types::SignatureId>,
        node: NodeId,
    ) {
        // resolvecall.rs's real body (the deprecation probes stay
        // permissive inside it until the deprecation wave).
        if let Some(sig) = sig {
            self.check_deprecated_signature_real(sig, node);
        }
    }

    /// Go: `func (c *Checker) checkParenthesizedExpression(node, checkMode) *Type`
    /// (checker.go:10269).
    pub(crate) fn check_parenthesized_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("ParenthesizedExpression has an expression");
        self.check_expression_ex(expression, check_mode)
    }

    /// Go: `func (c *Checker) checkQualifiedName(node, checkMode) *Type` (checker.go:8328).
    pub(crate) fn check_qualified_name(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let (left, right) = {
            let d = self
                .store()
                .node(node)
                .as_qualified_name()
                .expect("QualifiedName data");
            (d.left, d.right)
        };
        let left_type = if u::is_in_type_query(self.store(), node)
            && tsc_ast::is_this_identifier(self.store(), left)
        {
            let this_type = self.check_this_expression(left);
            self.check_non_null_type(this_type, left)
        } else {
            self.check_non_null_expression(left)
        };
        self.check_property_access_expression_or_qualified_name(
            node, left, left_type, right, check_mode, false /*writeOnly*/,
        )
    }

    /// Go: `func (c *Checker) checkIndexedAccess(node, checkMode) *Type` (checker.go:8336).
    pub(crate) fn check_indexed_access(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        if self.store().node(node).flags.intersects(NF::OPTIONAL_CHAIN) {
            return self.check_element_access_chain(node, check_mode);
        }
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("ElementAccessExpression has an expression");
        let expr_type = self.check_non_null_expression(expression);
        self.check_element_access_expression(node, expr_type, check_mode)
    }

    /// Go: `func (c *Checker) checkElementAccessChain(node, checkMode) *Type` (checker.go:8342).
    pub(crate) fn check_element_access_chain(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("ElementAccessExpression has an expression");
        let expr_type = self.check_expression(expression);
        let non_optional_type = self.get_optional_expression_type(expr_type, expression);
        let non_nullable = self.check_non_null_type(non_optional_type, expression);
        let t = self.check_element_access_expression(node, non_nullable, check_mode);
        self.propagate_optional_type_marker(t, node, non_optional_type != expr_type)
    }

    /// Go: `func (c *Checker) checkElementAccessExpression(node, exprType, checkMode) *Type`
    /// (checker.go:8349).
    pub(crate) fn check_element_access_expression(
        &mut self,
        node: NodeId,
        mut object_type: TypeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let assignment_kind = u::get_assignment_target_kind(self.store(), node);
        if assignment_kind != u::AssignmentKind::None || self.is_method_access_for_call(node) {
            object_type = self.get_widened_type(object_type);
        }
        let index_expression = self
            .store()
            .node(node)
            .as_element_access_expression()
            .expect("ElementAccessExpression data")
            .argument_expression;
        let index_type = self.check_expression(index_expression);
        if relater::is_error_type(self, object_type) || object_type == self.silent_never_type() {
            return object_type;
        }
        if crate::spine::is_const_enum_object_type(self, object_type)
            && !tsc_binder::ast_shims::is_string_literal_like(self.store().node(index_expression))
        {
            self.error(
                index_expression,
                &A_CONST_ENUM_MEMBER_CAN_ONLY_BE_ACCESSED_USING_A_STRING_LITERAL,
                &[],
            );
            return self.error_type();
        }
        let mut effective_index_type = index_type;
        if self.is_for_in_variable_for_numeric_property_names(index_expression) {
            effective_index_type = self.number_type();
        }
        let access_flags = if assignment_kind == u::AssignmentKind::None {
            crate::types::AccessFlags::EXPRESSION_POSITION
        } else {
            crate::types::AccessFlags::WRITING
                .union(if assignment_kind == u::AssignmentKind::Compound {
                    crate::types::AccessFlags::EXPRESSION_POSITION
                } else {
                    crate::types::AccessFlags::NONE
                })
                .union(
                    if relater::is_generic_object_type(self, object_type)
                        && !u::is_this_type_parameter(self.type_(object_type))
                    {
                        crate::types::AccessFlags::NO_INDEX_SIGNATURES
                    } else {
                        crate::types::AccessFlags::NONE
                    },
                )
        };
        let indexed_access_type = self
            .get_indexed_access_type_or_undefined(
                object_type,
                effective_index_type,
                access_flags,
                Some(index_expression),
                Some(node),
            )
            .unwrap_or_else(|| self.error_type());
        let resolved_symbol = self.get_resolved_symbol_nil(node);
        let flow_type = self.get_flow_type_of_access_expression(
            node,
            resolved_symbol,
            indexed_access_type,
            index_expression,
            check_mode,
        );
        self.check_indexed_access_index_type(flow_type, node)
    }

    /// Go: `func (c *Checker) isForInVariableForNumericPropertyNames(expr) bool`
    /// (checker.go:8383).
    pub(crate) fn is_for_in_variable_for_numeric_property_names(&mut self, expr: NodeId) -> bool {
        let e = tsc_ast::utilities::skip_parentheses(self.store(), expr);
        if is_identifier(self.store().node(e)) {
            let Some(symbol) = self.get_resolved_symbol(e) else {
                return false;
            };
            if self.program_symbol(symbol).flags.intersects(SymbolFlags::VARIABLE) {
                let mut child = expr;
                let mut node = self.store().node(expr).parent.get();
                while !node.is_none() {
                    if self.store().node(node).kind == Kind::ForInStatement {
                        let statement = self
                            .store()
                            .node(node)
                            .as_for_in_or_of_statement()
                            .expect("ForInStatement data")
                            .statement;
                        if child == statement
                            && self.get_for_in_variable_symbol(node) == Some(symbol)
                        {
                            let initializer_expr = crate::ast_shims::expression_of(
                                self.store(),
                                node,
                            )
                            .expect("ForInStatement has an expression");
                            let init_type = self.check_expression(initializer_expr);
                            if self.has_numeric_property_names(init_type) {
                                return true;
                            }
                        }
                    }
                    child = node;
                    node = self.store().node(node).parent.get();
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) getForInVariableSymbol(node) *ast.Symbol` (checker.go:8403).
    pub(crate) fn get_for_in_variable_symbol(&mut self, node: NodeId) -> Option<SymbolId> {
        let initializer = crate::ast_shims::initializer_of(self.store(), node);
        if let Some(initializer) = initializer
            && self.store().node(initializer).kind == Kind::VariableDeclarationList
        {
            let declarations = self
                .store()
                .node(initializer)
                .as_variable_declaration_list()
                .expect("VariableDeclarationList data")
                .declarations
                .clone();
            if let Some(variable) = declarations
                .as_ref()
                .and_then(|l| l.nodes.first().copied())
            {
                let name = self.store().node(variable).name();
                let is_binding_pattern = name
                    .is_some_and(|n| {
                        matches!(
                            self.store().node(n).kind,
                            Kind::ArrayBindingPattern | Kind::ObjectBindingPattern
                        )
                    });
                if !variable.is_none() && !is_binding_pattern {
                    return self.get_symbol_of_declaration(variable);
                }
            }
        } else if let Some(initializer) = initializer
            && is_identifier(self.store().node(initializer))
        {
            return self.get_resolved_symbol(initializer);
        }
        None
    }

    /// Go: `func (c *Checker) hasNumericPropertyNames(t) bool` (checker.go:8417).
    pub(crate) fn has_numeric_property_names(&mut self, t: TypeId) -> bool {
        // Go: len(getIndexInfosOfType(t)) == 1 && getIndexInfoOfType(t,
        // numberType) != nil. The index-info reader is the S8 seam (empty),
        // so the probe is permissively false — Go's answer when the type
        // carries no resolved index infos.
        // TODO(port M6 S8): resolveStructuredTypeMembers' indexInfos.
        let _ = t;
        false
    }

    /// Go: `func (c *Checker) checkIndexedAccessIndexType(t, accessNode) *Type`
    /// (checker.go:8421).
    pub(crate) fn check_indexed_access_index_type(&mut self, t: TypeId, access_node: NodeId) -> TypeId {
        if !self.type_(t).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return t;
        }
        let (object_type, index_type) = {
            let d = self.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
            (
                d.object_type.unwrap_or(t),
                d.index_type.unwrap_or(t),
            )
        };
        // skip index type deferral on remapping mapped types
        // TODO(port M6 S7): getMappedTypeNameTypeKind — the remap probe;
        // getIndexTypeForMappedType is the non-remap branch's S10 seam carrier.
        let object_index_type =
            relater::get_index_type_ex(self, object_type, crate::types::IndexFlags::NONE);
        let has_number_index_info = self
            .get_index_info_of_type(object_type, self.number_type())
            .is_some();
        let every_assignable = {
            // Go: everyType(indexType, f) — PORT: the constituent walk
            // inlined (the closure captures &mut self).
            if self.type_(index_type).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let constituents = self.type_(index_type).types().to_vec();
                constituents.iter().all(|&t| {
                    self.index_type_assignable_check(t, object_index_type, has_number_index_info)
                })
            } else {
                self.index_type_assignable_check(index_type, object_index_type, has_number_index_info)
            }
        };
        if every_assignable {
            if self.store().node(access_node).kind == Kind::ElementAccessExpression
                && tsc_binder::ast_shims::is_assignment_target(self.store(), access_node)
                && self.type_(object_type).object_flags.intersects(ObjectFlags::MAPPED)
                && relater::get_mapped_type_modifiers(self, object_type)
                    .intersects(MappedTypeModifiers::INCLUDE_READONLY)
            {
                let type_string = self.type_to_string(object_type);
                self.error(
                    access_node,
                    &INDEX_SIGNATURE_IN_TYPE_0_ONLY_PERMITS_READING,
                    &[type_string],
                );
            }
            return t;
        }
        if relater::is_generic_object_type(self, object_type) {
            // REAL (M6 S10, checker.go:27769): getPropertyNameFromIndex — the
            // literal-index name probe (index.rs's free fn).
            let property_name = self.get_property_name_from_index(index_type, None);
            if property_name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING {
                let property_symbol = self.get_constituent_property(object_type, &property_name);
                if let Some(property_symbol) = property_symbol
                    && relater::get_declaration_modifier_flags_from_symbol(self, property_symbol)
                        .intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER)
                {
                    self.error(
                        access_node,
                        &PRIVATE_OR_PROTECTED_MEMBER_0_CANNOT_BE_ACCESSED_ON_A_TYPE_PARAMETER,
                        &[property_name],
                    );
                    return self.error_type();
                }
            }
        }
        let index_string = self.type_to_string(index_type);
        let object_string = self.type_to_string(object_type);
        self.error(
            access_node,
            &TYPE_0_CANNOT_BE_USED_TO_INDEX_TYPE_1,
            &[index_string, object_string],
        );
        self.error_type()
    }

    /// Go: `everyType`'s per-constituent probe of checkIndexedAccessIndexType
    /// (checker.go:8436) — the inlined closure body.
    fn index_type_assignable_check(
        &mut self,
        t: TypeId,
        object_index_type: TypeId,
        has_number_index_info: bool,
    ) -> bool {
        self.is_type_assignable_to(t, object_index_type)
            || has_number_index_info
                && relater::is_applicable_index_type(self, Some(t), self.number_type())
    }

    /// Go: `func (c *Checker) getConstituentProperty(objectType, propertyName) *ast.Symbol`
    /// (checker.go:8460).
    pub(crate) fn get_constituent_property(
        &mut self,
        object_type: TypeId,
        property_name: &str,
    ) -> Option<SymbolId> {
        for t in self.get_apparent_type_distributed(object_type) {
            if let Some(prop) = relater::get_property_of_type(self, t, property_name) {
                return Some(prop);
            }
        }
        None
    }

    /// Go: `c.getApparentType(objectType).Distributed()` — the union
    /// constituents / the singleton itself.
    fn get_apparent_type_distributed(&mut self, t: TypeId) -> Vec<TypeId> {
        let apparent = relater::get_apparent_type(self, t);
        if self.type_(apparent).flags.intersects(TypeFlags::UNION) {
            self.type_(apparent).types().to_vec()
        } else {
            vec![apparent]
        }
    }

    /// Go: `func isSpreadIntoCallOrNew(node) bool` (checker.go:8302).
    fn is_spread_into_call_or_new(&self, node: NodeId) -> bool {
        let parent =
            crate::ast_shims::walk_up_parenthesized_expressions(self.store(), self.store().node(node).parent.get());
        match parent {
            Some(p) if self.store().node(p).kind == Kind::SpreadElement => {
                let grandparent = self.store().node(p).parent.get();
                matches!(
                    self.store().node(grandparent).kind,
                    Kind::CallExpression | Kind::NewExpression
                )
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) checkArrayLiteral(node, checkMode) *Type` (checker.go:8210).
    pub(crate) fn check_array_literal(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let elements: Vec<NodeId> = {
            let d = self
                .store()
                .node(node)
                .as_array_literal_expression()
                .expect("ArrayLiteralExpression data");
            d.elements.as_ref().map(|l| l.nodes.to_vec()).unwrap_or_default()
        };
        let length = elements.len();
        let mut element_types: Vec<TypeId> = vec![self.never_type(); length];
        let mut element_infos: Vec<crate::types::TupleElementInfo> =
            vec![Default::default(); length];
        self.push_cached_contextual_type(node);
        let in_destructuring_pattern = tsc_binder::ast_shims::is_assignment_target(self.store(), node);
        let in_const_context = self.is_const_context(node);
        let contextual_type = self.get_apparent_type_of_contextual_type(node, crate::types::ContextFlags::NONE);
        let in_tuple_context = self.is_spread_into_call_or_new(node)
            || contextual_type.is_some_and(|contextual_type| {
                // Go: someType(contextualType, isTupleLike || homomorphic
                // mapped) — PORT: the constituent walk inlined (the closure
                // captures &mut self).
                let constituents: Vec<TypeId> = if self
                    .type_(contextual_type)
                    .flags
                    .intersects(TypeFlags::UNION_OR_INTERSECTION)
                {
                    self.type_(contextual_type).types().to_vec()
                } else {
                    vec![contextual_type]
                };
                constituents.iter().any(|&t| {
                    relater::is_array_like_type(self, t)
                        || (relater::is_generic_mapped_type(self, t) && {
                            // Go: t.AsMappedType().nameType == nil &&
                            // getHomomorphicTypeVariable(...) != nil — the S7
                            // mapped-type probe; permissive false.
                            // TODO(port M6 S7): the homomorphic mapped-type walk.
                            false
                        })
                })
            });
        let mut has_omitted_expression = false;
        for (i, e) in elements.iter().copied().enumerate() {
            if self.store().node(e).kind == Kind::SpreadElement {
                let spread_expr = crate::ast_shims::expression_of(self.store(), e)
                    .expect("SpreadElement has an expression");
                let spread_type = self.check_expression_ex(spread_expr, check_mode);
                if relater::is_array_like_type(self, spread_type) {
                    element_types[i] = spread_type;
                    element_infos[i] = crate::types::TupleElementInfo {
                        flags: crate::types::ElementFlags::VARIADIC,
                        labeled_declaration: None,
                    };
                } else if in_destructuring_pattern {
                    // Given the following situation:
                    //    var c: {};
                    //    [...c] = ["", 0];
                    //
                    // c is represented in the tree as a spread element in an array literal.
                    // But c really functions as a rest element, and its purpose is to
                    // provide a contextual type for the right hand side of the assignment. Therefore,
                    // instead of calling checkExpression on "...c", which will give an error
                    // if c is not iterable/array-like, we need to act as if we are trying to
                    // get the contextual element type from it. So we do something similar to
                    // getContextualTypeForElementExpression, which will crucially not error
                    // if there is no index type / iterated type.
                    let rest_element_type = self
                        .get_index_type_of_type(spread_type, self.number_type())
                        .unwrap_or_else(|| {
                            self.get_iterated_type_or_element_type(
                                crate::statements::IterationUse::DESTRUCTURING,
                                spread_type,
                                self.undefined_type(),
                                None, /*errorNode*/
                                false, /*checkAssignability*/
                            )
                            .unwrap_or_else(|| self.unknown_type())
                        });
                    element_types[i] = rest_element_type;
                    element_infos[i] = crate::types::TupleElementInfo {
                        flags: crate::types::ElementFlags::REST,
                        labeled_declaration: None,
                    };
                } else {
                    let t = self.check_iterated_type_or_element_type(
                        crate::statements::IterationUse::SPREAD,
                        spread_type,
                        self.undefined_type(),
                        Some(spread_expr),
                    );
                    element_types[i] = t;
                    element_infos[i] = crate::types::TupleElementInfo {
                        flags: crate::types::ElementFlags::REST,
                        labeled_declaration: None,
                    };
                }
            } else if self.exact_optional_property_types
                && self.store().node(e).kind == Kind::OmittedExpression
            {
                has_omitted_expression = true;
                element_types[i] = self.undefined_or_missing_type();
                element_infos[i] = crate::types::TupleElementInfo {
                    flags: crate::types::ElementFlags::OPTIONAL,
                    labeled_declaration: None,
                };
            } else {
                let t = self.check_expression_for_mutable_location(e, check_mode);
                element_types[i] = self.add_optionality_ex(t, true /*isProperty*/, has_omitted_expression);
                element_infos[i] = crate::types::TupleElementInfo {
                    flags: if has_omitted_expression {
                        crate::types::ElementFlags::OPTIONAL
                    } else {
                        crate::types::ElementFlags::REQUIRED
                    },
                    labeled_declaration: None,
                };
                // Go: CheckModeInferential's intra-expression inference site
                // (the S6/S9 inference-context readers are permissive; the
                // isContextSensitive probe stays dormant under them).
                // TODO(port M6 S6/S9): addIntraExpressionInferenceSite.
                let _ = (in_tuple_context, check_mode);
            }
        }
        self.pop_contextual_type();
        if in_destructuring_pattern {
            return self.create_tuple_type_ex(&element_types, &element_infos, false);
        }
        if check_mode.intersects(CheckMode::FORCE_TUPLE) || in_const_context || in_tuple_context {
            let tuple = self.create_tuple_type_ex(&element_types, &element_infos, false);
            return self.create_array_literal_type(tuple);
        }
        let element_type = if !element_types.is_empty() {
            for (i, e) in element_types.iter_mut().enumerate() {
                if element_infos[i].flags.intersects(crate::types::ElementFlags::VARIADIC) {
                    *e = self
                        .get_indexed_access_type_or_undefined(
                            *e,
                            self.number_type(),
                            crate::types::AccessFlags::NONE,
                            None,
                            None,
                        )
                        .unwrap_or_else(|| self.any_type());
                }
            }
            relater::get_union_type_ex(
                self,
                element_types,
                crate::todo_shapes::UnionReduction::Subtype,
                None,
                None,
            )
        } else if self.strict_null_checks {
            self.implicit_never_type()
        } else {
            self.undefined_widening_type()
        };
        let array_type = self.create_array_type_ex(element_type, in_const_context);
        self.create_array_literal_type(array_type)
    }

    /// Go: `func (c *Checker) createArrayLiteralType(t) *Type` (checker.go:8290).
    pub(crate) fn create_array_literal_type(&mut self, t: TypeId) -> TypeId {
        if !self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return t;
        }
        // TODO(port M6 S7): cloneTypeReference + the ArrayLiteral flag pair
        // (the cachedTypes key); the identity is the non-reference answer.
        t
    }

    // Go: `func (c *Checker) createArrayTypeEx(elementType, isConstTuple)`
    // — REMOVED (M6 S7): superseded by uniontypes.rs's real constructor
    // (Go's second parameter is `readonly`; the const-context callers pass
    // true for the readonly array reference).


    /// Go: `func (c *Checker) checkExpressionForMutableLocation(node, checkMode) *Type`
    /// (checker.go:14131).
    pub(crate) fn check_expression_for_mutable_location(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let t = self.check_expression_ex(node, check_mode);
        if self.is_const_context(node) {
            return self.get_regular_type_of_literal_type(t);
        }
        if self.store().node(node).kind == Kind::TypeAssertionExpression {
            return t;
        }
        let contextual_type = {
            let raw = self.get_contextual_type(node, crate::types::ContextFlags::NONE);
            self.instantiate_contextual_type_stub(
                raw,
                node,
                crate::types::ContextFlags::NONE,
            )
        };
        self.get_widened_literal_like_type_for_contextual_type(t, contextual_type)
    }

    /// Go: `func (c *Checker) getWidenedLiteralLikeTypeForContextualType(t, contextualType) *Type`
    /// (checker.go:26049). REAL body (isLiteralOfContextualType's contextual
    /// probe is S9-bannered; with the getContextualType S9 seam the arms are
    /// Go's None answers).
    pub(crate) fn get_widened_literal_like_type_for_contextual_type(
        &mut self,
        t: TypeId,
        contextual_type: Option<TypeId>,
    ) -> TypeId {
        let mut t = t;
        if !self.is_literal_of_contextual_type(t, contextual_type) {
            let widened = self.get_widened_literal_type(t);
            t = self.get_widened_unique_es_symbol_type(widened);
        }
        self.get_regular_type_of_literal_type(t)
    }

    /// Go: `func (c *Checker) isLiteralOfContextualType(candidateType, contextualType) bool`
    /// (checker.go:26056).
    pub(crate) fn is_literal_of_contextual_type(
        &mut self,
        candidate_type: TypeId,
        contextual_type: Option<TypeId>,
    ) -> bool {
        let Some(contextual_type) = contextual_type else {
            return false;
        };
        if self.type_(contextual_type)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            let constituents = self.type_(contextual_type).types().to_vec();
            return constituents.iter().any(|&t| {
                self.is_literal_of_contextual_type(candidate_type, Some(t))
            });
        }
        if self
            .type_(contextual_type)
            .flags
            .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
        {
            // If the contextual type is a type variable constrained to a primitive type, consider
            // this a literal context for literals of that primitive type. For example, given a
            // type parameter 'T extends string', infer string literal types for T.
            let constraint = relater::get_base_constraint_of_type(self, contextual_type)
                .unwrap_or_else(|| self.unknown_type());
            return (relater::maybe_type_of_kind(self, constraint, TypeFlags::STRING)
                && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::STRING_LITERAL))
                || (relater::maybe_type_of_kind(self, constraint, TypeFlags::NUMBER)
                    && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::NUMBER_LITERAL))
                || (relater::maybe_type_of_kind(self, constraint, TypeFlags::BIGINT)
                    && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::BIGINT_LITERAL))
                || (relater::maybe_type_of_kind(self, constraint, TypeFlags::ESSYMBOL)
                    && relater::maybe_type_of_kind(
                        self,
                        candidate_type,
                        TypeFlags::UNIQUE_ESSYMBOL,
                    ))
                || self.is_literal_of_contextual_type(candidate_type, Some(constraint));
        }
        // If the contextual type is a literal of a particular primitive type, we consider this a
        // literal context for all literals of that primitive type.
        let cf = self.type_(contextual_type).flags;
        (cf.intersects(
            TypeFlags::STRING_LITERAL
                .union(TypeFlags::INDEX)
                .union(TypeFlags::TEMPLATE_LITERAL)
                .union(TypeFlags::STRING_MAPPING),
        ) && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::STRING_LITERAL))
            || (cf.intersects(TypeFlags::NUMBER_LITERAL)
                && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::NUMBER_LITERAL))
            || (cf.intersects(TypeFlags::BIGINT_LITERAL)
                && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::BIGINT_LITERAL))
            || (cf.intersects(TypeFlags::BOOLEAN_LITERAL)
                && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::BOOLEAN_LITERAL))
            || (cf.intersects(TypeFlags::UNIQUE_ESSYMBOL)
                && relater::maybe_type_of_kind(self, candidate_type, TypeFlags::UNIQUE_ESSYMBOL))
    }

    // Go: `func (c *Checker) getWidenedUniqueESSymbolType(t) *Type`
    // (checker.go:26031 region) — REMOVED (M6 S7): superseded by
    // uniontypes.rs's real body (same signature; the union arm now maps the
    // constituents).


    /// Go: `func (c *Checker) pushCachedContextualType(node)` (checker.go:31421).
    pub(crate) fn push_cached_contextual_type(&mut self, node: NodeId) {
        let t = self.get_contextual_type(node, crate::types::ContextFlags::NONE);
        self.push_contextual_type(Some(node), t, true /*isCache*/);
    }

    // Go: `func (c *Checker) getApparentTypeOfContextualType(node, contextFlags) *Type`
    // (checker.go:31235 — S9): MOVED — the REAL body (instantiateContextualType +
    // the apparent-type constituent map) lives in contextual.rs as
    // `get_apparent_type_of_contextual_type(node, contextFlags)` (same signature).

    /// Go: `func (c *Checker) isConstContext(node) bool` (checker.go:13935).
    pub(crate) fn is_const_context(&mut self, node: NodeId) -> bool {
        let parent = self.store().node(node).parent.get();
        if is_const_assertion(self.store(), parent)
            || self.is_inline_import_attributes(node)
        {
            return true;
        }
        if self.is_valid_const_assertion_argument(node) {
            let contextual_type =
                self.get_contextual_type(node, crate::types::ContextFlags::NONE);
            if self.is_const_type_variable_opt(contextual_type, 0) {
                return true;
            }
        }
        matches!(
            self.store().node(parent).kind,
            Kind::ParenthesizedExpression
                | Kind::ArrayLiteralExpression
                | Kind::SpreadElement
        ) && self.is_const_context(parent)
            || matches!(
                self.store().node(parent).kind,
                Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment | Kind::TemplateSpan
            ) && self.is_const_context(self.store().node(parent).parent.get())
    }

    /// Go: `func (c *Checker) isInlineImportAttributes(node) bool` (checker.go:13945).
    pub(crate) fn is_inline_import_attributes(&self, node: NodeId) -> bool {
        let parent = self.store().node(node).parent.get();
        if self.store().node(node).kind != Kind::ObjectLiteralExpression
            || self.store().node(parent).kind != Kind::PropertyAssignment
            || crate::ast_shims::initializer_of(self.store(), parent) != Some(node)
        {
            return false;
        }
        let property = parent;
        let property_name = self.store().node(property).name();
        let name_ok = property_name.is_some_and(|n| {
            (is_identifier(self.store().node(n))
                || tsc_binder::ast_shims::is_string_literal_like(self.store().node(n)))
                && tsc_ast::node_text(self.store(), n) == "with"
        });
        if !name_ok {
            return false;
        }
        let options = self.store().node(property).parent.get();
        if self.store().node(options).kind != Kind::ObjectLiteralExpression {
            return false;
        }
        let import_call = crate::ast_shims::find_ancestor(self.store(), options, &mut |n| {
            n.kind == Kind::CallExpression
                && {
                    // Go: IsImportCall — the import(...) callee probe.
                    let d = match &n.data {
                        tsc_ast::NodeData::CallExpression(d) => Some(d),
                        _ => None,
                    };
                    d.map(|d| {
                        let store = self.store();
                        let expr_kind = store.node(d.expression).kind;
                        expr_kind == Kind::ImportKeyword
                    })
                    .unwrap_or(false)
                }
        });
        match import_call {
            Some(import_call) => {
                let arguments: Vec<NodeId> = self
                    .store()
                    .node(import_call)
                    .as_call_expression()
                    .expect("CallExpression data")
                    .arguments
                    .as_ref()
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default();
                arguments.len() > 1
                    && tsc_ast::utilities::skip_parentheses(self.store(), arguments[1]) == options
            }
            None => false,
        }
    }

    /// Go: `func (c *Checker) isConstTypeVariable(t, depth) bool` (checker.go:13982)
    /// — the inference.rs free function (already real) is the carrier; this
    /// is its Option-context adapter (Go's `t == nil` arm).
    pub(crate) fn is_const_type_variable_opt(&mut self, t: Option<TypeId>, depth: usize) -> bool {
        let Some(t) = t else {
            return false;
        };
        crate::inference::is_const_type_variable(self, t, depth)
    }
}

/// Go: `func IsConstAssertion(node *Node) bool` (ast/utilities.go) —
/// `IsConstTypeReference(node.Type())` over the as/type-assertion pair.
pub(crate) fn is_const_assertion(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::AsExpression | Kind::TypeAssertionExpression => {
            let type_node = match (&n.data, n.kind) {
                (tsc_ast::NodeData::AsExpression(d), _) => d.type_,
                (tsc_ast::NodeData::TypeAssertion(d), _) => d.type_,
                _ => return false,
            };
            let tn = store.node(type_node);
            let reference = tn.as_type_reference_node();
            tn.kind == Kind::TypeReference
                && reference.is_some_and(|d| {
                    d.type_arguments.is_none()
                        || d.type_arguments.as_ref().is_some_and(|l| l.nodes.is_empty())
                })
                && reference
                    .is_some_and(|d| is_identifier(store.node(d.type_name)))
                && reference.is_some_and(|d| tsc_ast::node_text(store, d.type_name) == "const")
        }
        _ => false,
    }
}

/// Go: `checkSuperExpression`'s `isLegalUsageOfSuperExpression` closure —
/// free (store-only reads, no checker borrow).
fn is_legal_usage_of_super(store: &dyn NodeStore, container: NodeId, is_call_expression: bool) -> bool {
    if is_call_expression {
        // TS 1.0 SPEC (April 2014): 4.8.1
        // Super calls are only permitted in constructors of derived classes
        return store.node(container).kind == Kind::Constructor;
    }
    // TS 1.0 SPEC (April 2014)
    // 'super' property access is allowed
    // - In a constructor, instance member function, instance member accessor, or instance member variable initializer where this references a derived class instance
    // - In a static member function or static member accessor
    let parent = store.node(container).parent.get();
    if crate::statements::is_class_like(store, parent)
        || store.node(parent).kind == Kind::ObjectLiteralExpression
    {
        if tsc_binder::ast_shims::is_static(store.node(container)) {
            return matches!(
                store.node(container).kind,
                Kind::MethodDeclaration
                    | Kind::MethodSignature
                    | Kind::GetAccessor
                    | Kind::SetAccessor
                    | Kind::PropertyDeclaration
                    | Kind::ClassStaticBlockDeclaration
            );
        }
        return matches!(
            store.node(container).kind,
            Kind::MethodDeclaration
                | Kind::MethodSignature
                | Kind::GetAccessor
                | Kind::SetAccessor
                | Kind::PropertyDeclaration
                | Kind::PropertySignature
                | Kind::Constructor
        );
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S4 tranche 2 — the identifier family (checker.go:11278-11480,
// 14161-14215) + getNarrowedTypeOfSymbol (checker.go:13900-14131).
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkIdentifier(node, checkMode) *Type` (checker.go:11278).
    pub(crate) fn check_identifier(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        if is_this_in_type_query(self.store(), node) {
            return self.check_this_expression(node);
        }
        let Some(symbol) = self.get_resolved_symbol(node) else {
            // PORT: the Option convention — None is Go's unknownSymbol
            // (checker.go:11287's `symbol == c.unknownSymbol` arm).
            return self.error_type();
        };
        if Some(symbol) == self.arguments_symbol {
            if self.is_in_property_initializer_or_class_static_block(node, true /*ignoreArrowFunctions*/) {
                self.error(
                    node,
                    &X_ARGUMENTS_CANNOT_BE_REFERENCED_IN_PROPERTY_INITIALIZERS_OR_CLASS_STATIC_INITIALIZATION_BLOCKS,
                    &[],
                );
                return self.error_type();
            }
            return self.get_type_of_symbol(symbol);
        }
        if should_mark_identifier_alias_referenced(self.store(), node) {
            self.mark_linked_references(node, ReferenceHint::IDENTIFIER, None, self.never_type());
        }
        let local_or_export_symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol);
        let target_symbol = self.resolve_alias_with_deprecation_check(local_or_export_symbol, node);
        // TODO(port M6 S6): isDeprecatedSymbol + addDeprecatedSuggestion +
        // isUncalledFunctionReference — the deprecation family; permissive
        // (no suggestion diagnostics).
        let _ = (target_symbol,);
        let mut declaration = self.program_symbol(local_or_export_symbol).value_declaration;
        // PORT: Go tracks immediateDeclaration for the never-initialized probe;
        // the W3 flow seam keeps that probe dormant (see below).
        let _immediate_declaration = declaration;
        // If the identifier is declared in a binding pattern for which we're currently computing the implied type and the
        // reference occurs with the same binding pattern, return the non-inferrable any type. This for example occurs in
        // 'const [a, b = a + 1] = [2]' when we're computing the contextual type for the array literal '[2]'.
        if let Some(declaration) = declaration
            && self.store().node(declaration).kind == Kind::BindingElement
            && {
                let declaration_parent = self.store().node(declaration).parent.get();
                self.contextual_binding_patterns.contains(&declaration_parent)
                    && crate::ast_shims::find_ancestor(self.store(), node, &mut |n| {
                        n.parent.get() == declaration_parent
                    })
                    .is_some()
            }
        {
            return self.non_inferrable_any_type();
        }
        let mut t = self.get_narrowed_type_of_symbol(local_or_export_symbol, node);
        let assignment_kind = u::get_assignment_target_kind(self.store(), node);
        if assignment_kind != u::AssignmentKind::None {
            let local_flags = self.program_symbol(local_or_export_symbol).flags;
            let in_js = tsc_ast::utilities::is_in_js_file(self.store().node(node));
            if !local_flags.intersects(SymbolFlags::VARIABLE)
                && !(in_js && local_flags.intersects(SymbolFlags::VALUE_MODULE))
            {
                let assignment_error = if local_flags.intersects(SymbolFlags::ENUM) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_ENUM
                } else if local_flags.intersects(SymbolFlags::CLASS) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CLASS
                } else if local_flags.intersects(SymbolFlags::MODULE) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_NAMESPACE
                } else if local_flags.intersects(SymbolFlags::FUNCTION) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_FUNCTION
                } else if local_flags.intersects(SymbolFlags::ALIAS) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_AN_IMPORT
                } else {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_NOT_A_VARIABLE
                };
                let name = relater::symbol_to_string(self, symbol);
                self.error(node, assignment_error, &[name]);
                return self.error_type();
            }
            if self.is_readonly_symbol(local_or_export_symbol) {
                let message = if local_flags.intersects(SymbolFlags::VARIABLE) {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_CONSTANT
                } else {
                    &CANNOT_ASSIGN_TO_0_BECAUSE_IT_IS_A_READ_ONLY_PROPERTY
                };
                let name = relater::symbol_to_string(self, symbol);
                self.error(node, message, &[name]);
                return self.error_type();
            }
        }
        let is_alias = self
            .program_symbol(local_or_export_symbol)
            .flags
            .intersects(SymbolFlags::ALIAS);
        // We only narrow variables and parameters occurring in a non-assignment position. For all other
        // entities we simply return the declared type.
        if self
            .program_symbol(local_or_export_symbol)
            .flags
            .intersects(SymbolFlags::VARIABLE)
        {
            if assignment_kind == u::AssignmentKind::Definite {
                if u::is_in_compound_like_assignment(self.store(), node) {
                    return self.get_base_type_of_literal_type(t);
                }
                return t;
            }
        } else if is_alias {
            declaration = self.get_declaration_of_alias_symbol(symbol);
        } else {
            return t;
        }
        let Some(declaration) = declaration else {
            return t;
        };
        t = self.get_narrowable_type_for_reference(t, node, check_mode);
        // The declaration container is the innermost function that encloses the declaration of the variable
        // or parameter. The flow container is the innermost function starting with which we analyze the control
        // flow graph to determine the control flow based type.
        let is_parameter =
            self.store().node(tsc_binder::ast_shims::get_root_declaration(self.store(), declaration)).kind
                == Kind::Parameter;
        let declaration_container = self.get_control_flow_container(declaration);
        let mut flow_container = self.get_control_flow_container(node);
        let is_outer_variable = flow_container != declaration_container;
        let is_spread_destructuring_assignment_target = {
            let parent = self.store().node(node).parent.get();
            let grandparent = self.store().node(parent).parent.get();
            self.store().node(parent).kind == Kind::SpreadAssignment
                && self.is_destructuring_assignment_target(grandparent)
        };
        let is_module_exports = self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::MODULE_EXPORTS);
        let type_is_automatic = t == self.auto_type() || Some(t) == self.auto_array_type;
        let is_automatic_type_in_non_null = type_is_automatic && {
            let parent = self.store().node(node).parent.get();
            self.store().node(parent).kind == Kind::NonNullExpression
        };
        // When the control flow originates in a function expression, arrow function, method, or accessor, and
        // we are referencing a closed-over const variable or parameter or mutable local variable past its last
        // assignment, we extend the origin of the control flow analysis to include the immediately enclosing
        // control flow container.
        while flow_container != declaration_container
            && (matches!(
                self.store().node(flow_container).kind,
                Kind::FunctionExpression | Kind::ArrowFunction
            ) || tsc_binder::ast_shims::is_object_literal_or_class_expression_method_or_accessor(
                self.store(),
                flow_container,
            ))
            && ((u::is_constant_variable(self, local_or_export_symbol)
                && Some(t) != self.auto_array_type)
                || (u::is_parameter_or_mutable_local_variable(
                    self,
                    self.store(),
                    local_or_export_symbol,
                ) && self.is_past_last_assignment(local_or_export_symbol, node)))
        {
            flow_container = self.get_control_flow_container(flow_container);
        }
        // A variable is considered uninitialized when it is possible to analyze the entire control flow graph
        // from declaration to use, and when the variable's declared type doesn't include undefined but the
        // control flow based type does include undefined.
        // PORT (W3T4): the real Go arm — immediateDeclaration is the
        // pre-alias value declaration (checker.go:11304), the mutable-local
        // probe is utilities.go:isMutableLocalVariableDeclaration, and the
        // definite-assignment facts are flow.go's marked-assignment walk.
        let is_never_initialized = match _immediate_declaration {
            Some(immediate) if self.store().node(immediate).kind == Kind::VariableDeclaration => {
                let grandparent = self
                    .store()
                    .node(self.store().node(immediate).parent.get())
                    .parent
                    .get();
                !binder_shims::is_for_in_or_of_statement(self.store().node(grandparent))
                    && crate::ast_shims::initializer_of(self.store(), immediate).is_none()
                    && self
                        .store()
                        .node(immediate)
                        .as_variable_declaration()
                        .is_some_and(|d| d.exclamation_token.is_none())
                    && u::is_mutable_local_variable_declaration(self.store(), immediate)
                    && !self.is_symbol_assigned_definitely(symbol)
            }
            _ => false,
        };
        let assume_initialized = is_parameter
            || is_alias
            || (is_outer_variable && !is_never_initialized)
            || is_spread_destructuring_assignment_target
            || is_module_exports
            || self.is_same_scoped_binding_element(node, declaration)
            || (t != self.auto_type()
                && Some(t) != self.auto_array_type
                && (!self.strict_null_checks
                    || self.type_(t)
                        .flags
                        .intersects(TypeFlags::ANY_OR_UNKNOWN.union(TypeFlags::VOID))
                    || u::is_in_type_query(self.store(), node)
                    || self.is_in_ambient_or_type_node(node)
                    || self.store().node(self.store().node(node).parent.get()).kind
                        == Kind::ExportSpecifier))
            || self.store().node(self.store().node(node).parent.get()).kind
                == Kind::NonNullExpression
            || (self.store().node(declaration).kind == Kind::VariableDeclaration
                && self
                    .store()
                    .node(declaration)
                    .as_variable_declaration()
                    .is_some_and(|d| d.exclamation_token.is_some()))
            || self.store().node(declaration).flags.intersects(NF::AMBIENT);
        let initial_type = if is_automatic_type_in_non_null {
            self.undefined_type()
        } else if assume_initialized && is_parameter {
            self.remove_optionality_from_declared_type(t, declaration)
        } else if assume_initialized {
            t
        } else if type_is_automatic {
            self.undefined_type()
        } else {
            self.get_optional_type(t, false /*isProperty*/)
        };
        let flow_type = if is_automatic_type_in_non_null {
            let non_nullable =
                self.get_flow_type_of_reference_ex(node, t, initial_type, flow_container, None);
            relater::get_non_nullable_type(self, non_nullable)
        } else {
            self.get_flow_type_of_reference_ex(node, t, initial_type, flow_container, None)
        };
        // A variable is considered uninitialized when it is possible to analyze the entire control flow graph
        // from declaration to use, and when the variable's declared type doesn't include undefined but the
        // control flow based type does include undefined.
        // PORT (W3T4): the REAL arms — flow.go's isEvolvingArrayOperationTarget
        // and the live TS2454 emission with Go's declared-type fallback
        // (checker.go:11417-11429).
        let is_evolving_array_operation_target = self.is_evolving_array_operation_target(node);
        if !is_evolving_array_operation_target
            && (t == self.auto_type() || Some(t) == self.auto_array_type)
        {
            if flow_type == self.auto_type() || Some(flow_type) == self.auto_array_type {
                if self.no_implicit_any {
                    let name = relater::symbol_to_string(self, symbol);
                    let flow_str = relater::type_to_string(self, flow_type);
                    let name_of_decl = binder_shims::get_name_of_declaration(
                        self.store(),
                        declaration,
                    )
                    .unwrap_or(NodeId::NONE);
                    self.error(
                        name_of_decl,
                        &VARIABLE_0_IMPLICITLY_HAS_TYPE_1_IN_SOME_LOCATIONS_WHERE_ITS_TYPE_CANNOT_BE_DETERMINED,
                        &[name.clone(), flow_str.clone()],
                    );
                    self.error(
                        node,
                        &VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE,
                        &[name, flow_str],
                    );
                }
                return self.convert_auto_to_any(flow_type);
            }
        } else if !assume_initialized
            && !self.contains_undefined_type(t)
            && self.contains_undefined_type(flow_type)
        {
            // Go: "Return the declared type to reduce follow-on errors"
            // (checker.go:11426) — the TS2454 emission replaces the flow type
            // with the declared type.
            let name = relater::symbol_to_string(self, symbol);
            self.error(node, &VARIABLE_0_IS_USED_BEFORE_BEING_ASSIGNED, &[name]);
            return t;
        }
        if assignment_kind != u::AssignmentKind::None {
            // Identifier is target of a compound assignment
            return self.get_base_type_of_literal_type(flow_type);
        }
        flow_type
    }

    /// Go: `func (c *Checker) isSameScopedBindingElement(node, declaration) bool`
    /// (checker.go:11444).
    pub(crate) fn is_same_scoped_binding_element(
        &self,
        node: NodeId,
        declaration: NodeId,
    ) -> bool {
        if self.store().node(declaration).kind == Kind::BindingElement {
            let binding_element = crate::ast_shims::find_ancestor(self.store(), node, &mut |n| {
                n.kind == Kind::BindingElement
            });
            return binding_element.is_some_and(|be| {
                tsc_binder::ast_shims::get_root_declaration(self.store(), be)
                    == tsc_binder::ast_shims::get_root_declaration(self.store(), declaration)
            });
        }
        false
    }

    /// Go: `func (c *Checker) removeOptionalityFromDeclaredType(declaredType, declaration) *Type`
    /// (checker.go:11451).
    pub(crate) fn remove_optionality_from_declared_type(
        &mut self,
        declared_type: TypeId,
        declaration: NodeId,
    ) -> TypeId {
        let remove_undefined = self.strict_null_checks
            && self.store().node(declaration).kind == Kind::Parameter
            && crate::ast_shims::initializer_of(self.store(), declaration).is_some()
            && self.has_type_facts(declared_type, TypeFacts::IS_UNDEFINED)
            && !self.parameter_initializer_contains_undefined(declaration);
        if remove_undefined {
            return relater::get_type_with_facts(self, declared_type, TypeFacts::NE_UNDEFINED);
        }
        declared_type
    }

    /// Go: `func (c *Checker) parameterInitializerContainsUndefined(declaration) bool`
    /// (checker.go:11463).
    pub(crate) fn parameter_initializer_contains_undefined(&mut self, declaration: NodeId) -> bool {
        let links_flags = self.node_links.get(declaration).flags;
        if !links_flags
            .intersects(crate::types::NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED)
        {
            if !self.push_type_resolution(
                crate::checker::TypeSystemEntity::Node(declaration),
                crate::checker::TypeSystemPropertyName::InitializerIsUndefined,
            ) {
                // PORT: Go reportCircularityError(declaration.Symbol()) then
                // returns true; the port's circularity carrier answers true.
                return true;
            }
            let initializer_type = self.check_declaration_initializer(
                declaration,
                CheckMode::CHECK_MODE_NORMAL,
                None,
            );
            let contains_undefined =
                self.has_type_facts(initializer_type, TypeFacts::IS_UNDEFINED);
            if !self.pop_type_resolution() {
                return true;
            }
            let flags = crate::types::NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED
                | if contains_undefined {
                    crate::types::NodeCheckFlags::INITIALIZER_IS_UNDEFINED
                } else {
                    crate::types::NodeCheckFlags::NONE
                };
            self.node_links.get(declaration).flags |= flags;
        }
        self.node_links
            .get(declaration)
            .flags
            .intersects(crate::types::NodeCheckFlags::INITIALIZER_IS_UNDEFINED)
    }

    /// Go: `func (c *Checker) isInAmbientOrTypeNode(node) bool` (checker.go:11479).
    pub(crate) fn is_in_ambient_or_type_node(&self, node: NodeId) -> bool {
        if self.store().node(node).flags.intersects(NF::AMBIENT) {
            return true;
        }
        crate::ast_shims::find_ancestor(self.store(), node, &mut |n| {
            matches!(
                n.kind,
                Kind::InterfaceDeclaration
                    | Kind::TypeAliasDeclaration
                    | Kind::TypeLiteral
            )
        })
        .is_some()
    }

    /// Go: `func (c *Checker) isInPropertyInitializerOrClassStaticBlock(node, ignoreArrowFunctions) bool`
    /// (checker.go:13906).
    pub(crate) fn is_in_property_initializer_or_class_static_block(
        &self,
        node: NodeId,
        ignore_arrow_functions: bool,
    ) -> bool {
        let store = self.store();
        crate::ast_shims::find_ancestor_or_quit(store, node, &mut |n| match n.kind {
            Kind::PropertyDeclaration | Kind::ClassStaticBlockDeclaration => {
                crate::ast_shims::FindAncestorResult::True
            }
            Kind::TypeQuery | Kind::JsxClosingElement => {
                crate::ast_shims::FindAncestorResult::Quit
            }
            Kind::ArrowFunction => {
                if ignore_arrow_functions {
                    crate::ast_shims::FindAncestorResult::False
                } else {
                    crate::ast_shims::FindAncestorResult::Quit
                }
            }
            Kind::Block => {
                if tsc_ast::is_function_like(store.node(n.parent.get()))
                    && store.node(n.parent.get()).kind != Kind::ArrowFunction
                {
                    crate::ast_shims::FindAncestorResult::Quit
                } else {
                    crate::ast_shims::FindAncestorResult::False
                }
            }
            _ => crate::ast_shims::FindAncestorResult::False,
        })
        .is_some()
    }

    /// Go: `func (c *Checker) getNarrowedTypeOfSymbol(symbol, location) *Type`
    /// (checker.go:13900).
    pub(crate) fn get_narrowed_type_of_symbol(
        &mut self,
        symbol: SymbolId,
        location: NodeId,
    ) -> TypeId {
        let mut t = self.get_type_of_symbol(symbol);
        let declaration = self.program_symbol(symbol).value_declaration;
        if let Some(declaration) = declaration
            && self.store().node(declaration).kind == Kind::BindingElement
        {
            // If we have a non-rest binding element with no initializer declared as a const variable or a const-like
            // parameter (a parameter for which there are no assignments in the function body), and if the parent type
            // for the destructuring is a union type, one or more of the binding elements may represent discriminant
            // properties, and we want the effects of conditional checks on such discriminants to affect the types of
            // other binding elements from the same destructuring. (See checker.go's worked example.)
            let has_initializer = crate::ast_shims::initializer_of(self.store(), declaration).is_some();
            let has_dot_dot_dot = {
                let d = self
                    .store()
                    .node(declaration)
                    .as_binding_element()
                    .expect("BindingElement data");
                d.dot_dot_dot_token.is_some()
            };
            let elements_len = {
                let parent = self.store().node(declaration).parent.get();
                match &self.store().node(parent).data {
                    tsc_ast::NodeData::BindingPattern(d) => d
                        .elements
                        .as_ref()
                        .map(|l| l.nodes.len())
                        .unwrap_or(0),
                    _ => 0,
                }
            };
            if !has_initializer && !has_dot_dot_dot && elements_len >= 2 {
                let root_declaration =
                    tsc_binder::ast_shims::get_root_declaration(self.store(), declaration);
                let root_initializer =
                    crate::ast_shims::initializer_of(self.store(), root_declaration);
                // Avoid declaration circularity without blocking binding defaults or nested callbacks.
                let inside_root_initializer = root_initializer.is_some_and(|ri| {
                    u::is_node_descendant_of(self.store(), location, ri)
                        && self.get_control_flow_container(declaration)
                            == self.get_control_flow_container(location)
                });
                if !inside_root_initializer {
                    let pattern = self.store().node(declaration).parent.get();
                    let parent = self.store().node(pattern).parent.get();
                    let root_is_const_variable = self.store().node(root_declaration).kind
                        == Kind::VariableDeclaration
                        && self
                            .get_combined_node_flags_cached(root_declaration)
                            .intersects(NF::CONSTANT);
                    let root_is_parameter =
                        self.store().node(root_declaration).kind == Kind::Parameter;
                    if (root_is_const_variable || root_is_parameter)
                        && !self
                            .node_links
                            .get(parent)
                            .flags
                            .intersects(crate::types::NodeCheckFlags::IN_CHECK_IDENTIFIER)
                        {
                            self.node_links.get(parent).flags |=
                                crate::types::NodeCheckFlags::IN_CHECK_IDENTIFIER;
                            let parent_type = self
                                .get_type_for_binding_element_parent(parent, CheckMode::CHECK_MODE_NORMAL)
                                .unwrap_or_else(|| self.error_type());
                            // Guard parent-type resolution only; flow analysis should allow re-entrant narrowing
                            self.node_links.get(parent).flags &=
                                !crate::types::NodeCheckFlags::IN_CHECK_IDENTIFIER;
                            // PORT: Go maps parentType through getBaseConstraintOrType
                            // (mapType); the S7 mapType seam applies f to the
                            // type itself.
                            let parent_type_constraint =
                                relater::get_base_constraint_or_type(self, parent_type);
                            // TODO(port M6 W3): isSomeSymbolAssigned — the
                            // assignment flow probe; permissive false.
                            if self.type_(parent_type_constraint)
                                .flags
                                .intersects(TypeFlags::UNION)
                            {
                                // TODO(port M6 W3): getFlowTypeOfReferenceEx —
                                // the flow walk; the seam returns the initial
                                // (constraint) type.
                                let narrowed_type = self.get_flow_type_of_reference_ex(
                                    pattern,
                                    parent_type_constraint,
                                    parent_type_constraint,
                                    NodeId::NONE,
                                    None,
                                );
                                if self.type_(narrowed_type).flags.intersects(TypeFlags::NEVER) {
                                    t = self.never_type();
                                } else {
                                    // Destructurings are validated against the parent type elsewhere. Here we disable tuple bounds
                                    // checks because the narrowed type may have lower arity than the full parent type.
                                    t = self.get_binding_element_type_from_parent_type(
                                        declaration,
                                        narrowed_type,
                                        true, /*noTupleBoundsCheck*/
                                    );
                                }
                            }
                        }
                }
            }
        }
        t
    }

    /// Go: `func (c *Checker) isPastLastAssignment(symbol, location) bool`
    /// (flow.go — W3): the REAL flow-epoch probe (flow.rs).
    #[allow(dead_code)] // The checkIdentifier caller loop lands with the W3 check-stage runner wiring.
    pub(crate) fn is_past_last_assignment(&mut self, symbol: SymbolId, location: NodeId) -> bool {
        self.is_past_last_assignment_real(symbol, Some(location))
    }

    /// Go: `func (c *Checker) isSymbolAssignedDefinitely(symbol) bool` — the
    /// REAL definite-assignment flow walk (flow.rs).
    #[allow(dead_code)] // The checkIdentifier caller loop lands with the W3 check-stage runner wiring.
    pub(crate) fn is_symbol_assigned_definitely(&mut self, symbol: SymbolId) -> bool {
        self.is_symbol_assigned_definitely_real(symbol)
    }

    /// Go: `func (c *Checker) isDestructuringAssignmentTarget(parent) bool`
    /// (flow.go: checker.go:11371's call site). REAL body (store-only reads).
    pub(crate) fn is_destructuring_assignment_target(&self, parent: NodeId) -> bool {
        let parent_parent = self.store().node(parent).parent.get();
        if self.store().node(parent_parent).kind == Kind::BinaryExpression {
            let d = self
                .store()
                .node(parent_parent)
                .as_binary_expression()
                .expect("BinaryExpression data");
            return d.left == parent;
        }
        self.store().node(parent_parent).kind == Kind::ForOfStatement
            && crate::ast_shims::initializer_of(self.store(), parent_parent) == Some(parent)
    }

    /// Go: `func (c *Checker) resolveAliasWithDeprecationCheck(symbol, location) *ast.Symbol`
    /// (checker.go:16697). REAL body landed in resolution.rs
    /// (`resolve_alias_with_deprecation_check_real`); this carrier delegates.
    pub(crate) fn resolve_alias_with_deprecation_check(
        &mut self,
        symbol: SymbolId,
        location: NodeId,
    ) -> SymbolId {
        self.resolve_alias_with_deprecation_check_real(symbol, location)
    }

    /// Go: `func (c *Checker) getResolvedSymbol(node) *ast.Symbol` (checker.go:14161)
    /// — the exports.rs method is the carrier (real caching body over the S6
    /// resolveName seam). `get_cannot_find_name_diagnostic_for_name` is the
    /// diagnostic-message probe.
    pub(crate) fn get_cannot_find_name_diagnostic_for_name(
        &mut self,
        node: NodeId,
    ) -> &'static Message {
        let name = tsc_ast::node_text(self.store(), node);
        match name {
            "document" | "console" => {
                &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_INCLUDE_DOM
            }
            "$" => {
                if self.compiler_options_uses_wildcard_types() {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_JQUERY_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJQUERY
                } else {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_JQUERY_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJQUERY_AND_THEN_ADD_JQUERY_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG
                }
            }
            "beforeEach" | "describe" | "suite" | "it" | "test" => {
                if self.compiler_options_uses_wildcard_types() {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_A_TEST_RUNNER_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJEST_OR_NPM_I_SAVE_DEV_TYPES_SLASHMOCHA
                } else {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_A_TEST_RUNNER_TRY_NPM_I_SAVE_DEV_TYPES_SLASHJEST_OR_NPM_I_SAVE_DEV_TYPES_SLASHMOCHA_AND_THEN_ADD_JEST_OR_MOCHA_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG
                }
            }
            "process" | "require" | "Buffer" | "module" | "NodeJS" => {
                if self.compiler_options_uses_wildcard_types() {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE
                } else {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE_AND_THEN_ADD_NODE_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG
                }
            }
            "Bun" => {
                if self.compiler_options_uses_wildcard_types() {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_BUN_TRY_NPM_I_SAVE_DEV_TYPES_SLASHBUN
                } else {
                    &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_BUN_TRY_NPM_I_SAVE_DEV_TYPES_SLASHBUN_AND_THEN_ADD_BUN_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG
                }
            }
            "Map" | "Set" | "Promise" | "ast.Symbol" | "WeakMap" | "WeakSet" | "Iterator"
            | "AsyncIterator" | "SharedArrayBuffer" | "Atomics" | "AsyncIterable"
            | "AsyncIterableIterator" | "AsyncGenerator" | "AsyncGeneratorFunction" | "BigInt"
            | "Reflect" | "BigInt64Array" | "BigUint64Array" => {
                &CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_1_OR_LATER
            }
            "await" => {
                let parent = self.store().node(node).parent.get();
                if self.store().node(parent).kind == Kind::CallExpression {
                    &CANNOT_FIND_NAME_0_DID_YOU_MEAN_TO_WRITE_THIS_IN_AN_ASYNC_FUNCTION
                }
                // Go: fallthrough to default.
                else if self.store().node(parent).kind == Kind::ShorthandPropertyAssignment {
                    &NO_VALUE_EXISTS_IN_SCOPE_FOR_THE_SHORTHAND_PROPERTY_0_EITHER_DECLARE_ONE_OR_PROVIDE_AN_INITIALIZER
                } else {
                    &CANNOT_FIND_NAME_0
                }
            }
            _ => {
                let parent = self.store().node(node).parent.get();
                if self.store().node(parent).kind == Kind::ShorthandPropertyAssignment {
                    &NO_VALUE_EXISTS_IN_SCOPE_FOR_THE_SHORTHAND_PROPERTY_0_EITHER_DECLARE_ONE_OR_PROVIDE_AN_INITIALIZER
                } else {
                    &CANNOT_FIND_NAME_0
                }
            }
        }
    }

    /// Go: `core.CompilerOptions.UsesWildcardTypes()` — `TypeRoots != nil ||
    /// Types != nil` (the "config supplies @types" probe).
    fn compiler_options_uses_wildcard_types(&self) -> bool {
        self.compiler_options.uses_wildcard_types()
    }

    /// Go: `func (c *Checker) getReferencedValueOrAliasSymbol(reference) *ast.Symbol`
    /// (checker.go:14177). Dormant until the emitresolver wave (its only Go
    /// caller is emitresolver.go's reference walk).
    #[allow(dead_code)]
    pub(crate) fn get_referenced_value_or_alias_symbol(&mut self, reference: NodeId) -> Option<SymbolId> {
        let resolved_symbol = self.symbol_node_links.get(reference).resolved_symbol;
        if resolved_symbol.is_some() && resolved_symbol != Some(self.unknown_symbol()) {
            return resolved_symbol;
        }
        // TODO(port M6 S6): resolveName with the Alias meaning — the S6 seam
        // permissive None.
        None
    }
}

/// Go: `func shouldMarkIdentifierAliasReferenced(node *ast.IdentifierNode) bool`
/// (checker.go:28900). REAL body (store-only reads).
fn should_mark_identifier_alias_referenced(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    if !parent.is_none() {
        // A property access expression LHS? checkPropertyAccessExpression will handle that.
        if store.node(parent).kind == Kind::PropertyAccessExpression
            && crate::ast_shims::expression_of(store, parent) == Some(node)
        {
            return false;
        }
        // Next two check for an identifier inside a type only export.
        let is_type_only = |n: NodeId| match &store.node(n).data {
            tsc_ast::NodeData::ExportSpecifier(d) => d.is_type_only,
            tsc_ast::NodeData::ExportDeclaration(d) => d.is_type_only,
            _ => false,
        };
        if store.node(parent).kind == Kind::ExportSpecifier && is_type_only(parent) {
            return false;
        }
        let parent_parent = store.node(parent).parent.get();
        if !parent_parent.is_none() {
            let great_grandparent = store.node(parent_parent).parent.get();
            if !great_grandparent.is_none()
                && store.node(great_grandparent).kind == Kind::ExportDeclaration
                && is_type_only(great_grandparent)
            {
                return false;
            }
        }
    }
    true
}

/// Go: `func IsThisInTypeQuery(node *Node) bool` (ast/utilities.go) — REAL
/// body (the qualified-name left-chain walk to the TypeQuery parent).
pub(crate) fn is_this_in_type_query(store: &dyn NodeStore, node: NodeId) -> bool {
    if !tsc_ast::is_this_identifier(store, node) {
        return false;
    }
    let mut node = node;
    loop {
        let parent = store.node(node).parent.get();
        if store.node(parent).kind == Kind::QualifiedName
            && store
                .node(parent)
                .as_qualified_name()
                .is_some_and(|d| d.left == node)
        {
            node = parent;
            continue;
        }
        return store.node(parent).kind == Kind::TypeQuery;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S4 tranche 2 — the object-literal family (checker.go:13388-13737)
// + the spread/expando machinery (checker.go:13529-13903) + the class/
// function-expression leaves (checker.go:10271-10586).
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkObjectLiteral(node, checkMode) *Type` (checker.go:13388).
    pub(crate) fn check_object_literal(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let (properties, node_symbol) = {
            let d = self
                .store()
                .node(node)
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression data");
            (d.properties.clone(), d.symbol)
        };
        let in_js = tsc_ast::utilities::is_in_js_file(self.store().node(node));
        // Expando object literals have empty properties but filled exports
        let properties_empty = properties
            .as_ref()
            .map(|l| l.nodes.is_empty())
            .unwrap_or(true);
        if properties_empty
            && let Some(symbol) = node_symbol
            && !self.program_symbol(symbol).exports.is_empty()
        {
            let exports: Vec<(String, SymbolId)> = self
                .program_symbol(symbol)
                .exports
                .entries()
                .map(|(k, &v)| (k.clone(), v))
                .collect();
            let mut members = tsc_ast::SymbolTable::new();
            for (k, v) in exports {
                members.set(k, v);
            }
            let mut extra = ObjectFlags::NONE;
            if in_js {
                extra |= ObjectFlags::JS_LITERAL;
            }
            // An expando object literal has no property children (len == 0), so there
            // is nothing to check here.
            return self.new_anonymous_type_with_members(Some(symbol), members, extra);
        }
        self.check_node_deferred(node);
        let in_destructuring_pattern =
            tsc_binder::ast_shims::is_assignment_target(self.store(), node);
        // Grammar checking
        self.check_grammar_object_literal_expression(node, in_destructuring_pattern);
        let strict_null_checks = self.strict_null_checks;
        let mut all_properties_table: Option<tsc_ast::SymbolTable> =
            strict_null_checks.then(tsc_ast::SymbolTable::new);
        let mut properties_table = tsc_ast::SymbolTable::new();
        let mut properties_array: Vec<SymbolId> = Vec::new();
        let empty_object_type = self.empty_object_type();
        let mut spread = empty_object_type;
        self.push_cached_contextual_type(node);
        let contextual_type = self.get_apparent_type_of_contextual_type(node, crate::types::ContextFlags::NONE);
        let mut contextual_type_has_pattern = false;
        if let Some(contextual_type) = contextual_type
            && let Some(pattern) = self.pattern_for_type.get(&contextual_type).copied()
            && matches!(
                self.store().node(pattern).kind,
                Kind::ObjectBindingPattern | Kind::ObjectLiteralExpression
            )
        {
            contextual_type_has_pattern = true;
        }
        let in_const_context = self.is_const_context(node);
        let mut check_flags = tsc_ast::CheckFlags::NONE;
        if in_const_context {
            check_flags |= tsc_ast::CheckFlags::READONLY;
        }
        let mut object_flags = ObjectFlags::FRESH_LITERAL;
        let mut pattern_with_computed_properties = false;
        let mut has_computed_string_property = false;
        let mut has_computed_number_property = false;
        let mut has_computed_symbol_property = false;
        // Spreads may cause an early bail; ensure computed names are always checked (this is cached)
        // As otherwise they may not be checked until exports for the type at this position are retrieved,
        // which may never occur.
        let property_nodes: Vec<NodeId> = properties
            .iter()
            .flat_map(|l| l.nodes.iter().copied())
            .collect();
        for elem in property_nodes.iter().copied() {
            if let Some(name) = self.store().node(elem).name()
                && self.store().node(name).kind == Kind::ComputedPropertyName
            {
                let _ = self.check_computed_property_name(name);
            }
        }
        let mut offset = 0usize;
        // Go's createObjectLiteralType closure — the per-spread-segment type.
        let node_symbol_clone = node_symbol;
        macro_rules! create_object_literal_type {
            ($c:expr) => {{
                let _ = offset;
                // TODO(port M6 S7/S10): getObjectLiteralIndexInfo — the
                // computed-property index infos; the S7/S10 read seams are
                // empty at this depth, so the literal carries none.
                let _ = (
                    has_computed_string_property,
                    has_computed_number_property,
                    has_computed_symbol_property,
                );
                let mut members = tsc_ast::SymbolTable::new();
                for (k, &v) in properties_table.entries() {
                    members.set(k.clone(), v);
                }
                let result = $c.new_anonymous_type_with_members(
                    node_symbol_clone,
                    members,
                    object_flags | ObjectFlags::OBJECT_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL,
                );
                if contextual_type.is_none() && in_js {
                    $c.type_mut(result).object_flags |= ObjectFlags::JS_LITERAL;
                }
                if pattern_with_computed_properties {
                    $c.type_mut(result).object_flags |=
                        ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES;
                }
                if in_destructuring_pattern {
                    $c.pattern_for_type.insert(result, node);
                }
                result
            }};
        }
        for member_decl in property_nodes.iter().copied() {
            let mut member = self.get_symbol_of_declaration(member_decl);
            let mut computed_name_type: Option<TypeId> = None;
            let member_name_node = self.store().node(member_decl).name();
            if let Some(name) = member_name_node
                && self.store().node(name).kind == Kind::ComputedPropertyName
            {
                computed_name_type = Some(self.check_computed_property_name(name));
            }
            let member_decl_kind = self.store().node(member_decl).kind;
            let is_object_literal_element = matches!(
                member_decl_kind,
                Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment
            ) || member_decl_kind == Kind::MethodDeclaration && {
                let parent = self.store().node(member_decl).parent.get();
                self.store().node(parent).kind == Kind::ObjectLiteralExpression
            };
            if is_object_literal_element
            {
                let t = match self.store().node(member_decl).kind {
                    Kind::PropertyAssignment => self.check_property_assignment(member_decl, check_mode),
                    Kind::ShorthandPropertyAssignment => {
                        self.check_shorthand_property_assignment(member_decl, in_destructuring_pattern, check_mode)
                    }
                    _ => self.check_object_literal_method(member_decl, check_mode),
                };
                object_flags |= self.type_(t).object_flags & ObjectFlags::PROPAGATING_FLAGS;
                let mut name_type: Option<TypeId> = None;
                if let Some(c) = computed_name_type
                    && u::is_type_usable_as_property_name(self.type_(c))
                {
                    name_type = Some(c);
                }
                let member_flags = member
                    .map(|m| self.program_symbol(m).flags)
                    .unwrap_or(SymbolFlags::NONE);
                // Go: a computed member's name folds from its name type
                // (never `Text()` on the ComputedPropertyName node); the
                // literal names read through node_text.
                let member_name = match name_type {
                    Some(nt) => u::get_property_name_from_type(self.type_(nt)),
                    None => member_name_node
                        .filter(|n| self.store().node(*n).kind != Kind::ComputedPropertyName)
                        .map(|n| tsc_ast::node_text(self.store(), n).to_string())
                        .unwrap_or_default(),
                };
                let prop = match name_type {
                    Some(nt) => {
                        let name = u::get_property_name_from_type(self.type_(nt));
                        self.new_symbol_ex(
                            SymbolFlags::PROPERTY.union(member_flags),
                            &name,
                            check_flags | tsc_ast::CheckFlags::LATE,
                        )
                    }
                    None => self.new_symbol_ex(
                        SymbolFlags::PROPERTY.union(member_flags),
                        &member_name,
                        check_flags,
                    ),
                };
                if let Some(nt) = name_type {
                    self.value_symbol_links.get(prop).name_type = Some(nt);
                }
                if in_destructuring_pattern && self.has_default_value(member_decl) {
                    // If object literal is an assignment pattern and if the assignment pattern specifies a default value
                    // for the property, make the property optional.
                    self.program_symbol_mut(prop).flags |= SymbolFlags::OPTIONAL;
                } else if contextual_type_has_pattern
                    && !self
                        .type_(contextual_type.unwrap())
                        .object_flags
                        .intersects(ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES)
                {
                    // If object literal is contextually typed by the implied type of a binding pattern, and if the
                    // binding pattern specifies a default value for the property, make the property optional.
                    let implied_prop = contextual_type
                        .and_then(|ct| relater::get_property_of_type(self, ct, &member_name));
                    if let Some(implied_prop) = implied_prop {
                        let implied_flags = self.program_symbol(implied_prop).flags;
                        self.program_symbol_mut(prop).flags |=
                            implied_flags & SymbolFlags::OPTIONAL;
                    } else if contextual_type.is_some()
                        && self
                            .get_index_info_of_type(contextual_type.unwrap(), self.string_type())
                            .is_none()
                    {
                        let name_string = member
                            .map(|m| relater::symbol_to_string(self, m))
                            .unwrap_or_default();
                        let contextual_string = contextual_type
                            .map(|ct| self.type_to_string(ct))
                            .unwrap_or_default();
                        if let Some(name_node) = member_name_node {
                            self.error(
                                name_node,
                                &OBJECT_LITERAL_MAY_ONLY_SPECIFY_KNOWN_PROPERTIES_AND_0_DOES_NOT_EXIST_IN_TYPE_1,
                                &[name_string, contextual_string],
                            );
                        }
                    }
                }
                let (member_declarations, member_parent, member_value_declaration) = member
                    .map(|m| {
                        let s = self.program_symbol(m);
                        (s.declarations.clone(), s.parent, s.value_declaration)
                    })
                    .unwrap_or_default();
                {
                    let s = self.program_symbol_mut(prop);
                    s.declarations = member_declarations;
                    s.parent = member_parent;
                    s.value_declaration = member_value_declaration;
                }
                self.value_symbol_links.get(prop).resolved_type = Some(t);
                self.value_symbol_links.get(prop).target = member;
                member = Some(prop);
                if let Some(table) = all_properties_table.as_mut() {
                    let name = self.program_symbol(prop).name.clone();
                    table.set(name, prop);
                }
                // TODO(port M6 S6/S9): addIntraExpressionInferenceSite for
                // context-sensitive property/method initializers — the
                // inference-context readers are permissive at this depth.
            } else if self.store().node(member_decl).kind == Kind::SpreadAssignment {
                if !properties_array.is_empty() {
                    let segment = create_object_literal_type!(self);
                    spread = self.get_spread_type(
                        spread,
                        segment,
                        node_symbol,
                        object_flags,
                        in_const_context,
                    );
                    properties_array.clear();
                    properties_table = tsc_ast::SymbolTable::new();
                    has_computed_string_property = false;
                    has_computed_number_property = false;
                    has_computed_symbol_property = false;
                }
                let spread_expr = {
                    let d = self
                        .store()
                        .node(member_decl)
                        .as_spread_assignment()
                        .expect("SpreadAssignment data");
                    d.expression
                };
                let t = self.check_expression_ex(
                    spread_expr,
                    check_mode | CheckMode::INFERENTIAL,
                );
                let t = self.get_reduced_type(t);
                if self.is_valid_spread_type(t) {
                    let merged_type =
                        self.try_merge_union_of_object_type_and_empty_object(t, in_const_context);
                    if let Some(table) = all_properties_table.clone() {
                        self.check_spread_prop_overrides(merged_type, &table, member_decl);
                    }
                    offset = properties_array.len();
                    if relater::is_error_type(self, spread) {
                        continue;
                    }
                    spread = self.get_spread_type(
                        spread,
                        merged_type,
                        node_symbol,
                        object_flags,
                        in_const_context,
                    );
                } else {
                    self.error(member_decl, &SPREAD_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES, &[]);
                    spread = self.error_type();
                }
                continue;
            } else {
                // A get/set accessor pair — processed like an ordinary
                // function declaration (checker.go's TS 1.0 spec comment);
                // the accessor checks run deferred (S3's carriers).
                debug_assert!(matches!(
                    self.store().node(member_decl).kind,
                    Kind::GetAccessor | Kind::SetAccessor
                ));
                self.check_node_deferred(member_decl);
            }
            if let Some(c) = computed_name_type
                && !self.type_(c).flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
            {
                let string_number_symbol =
                    relater::get_string_number_symbol_type(self);
                if self.is_type_assignable_to(c, string_number_symbol) {
                    let number_type = self.number_type();
                    if self.is_type_assignable_to(c, number_type) {
                        has_computed_number_property = true;
                    } else if self.is_type_assignable_to(c, self.es_symbol_type()) {
                        has_computed_symbol_property = true;
                    } else {
                        has_computed_string_property = true;
                    }
                    if in_destructuring_pattern {
                        pattern_with_computed_properties = true;
                    }
                }
            } else if let Some(member) = member {
                let name = self.program_symbol(member).name.clone();
                properties_table.set(name, member);
            }
            if let Some(member) = member {
                properties_array.push(member);
            }
        }
        self.pop_contextual_type();
        if relater::is_error_type(self, spread) {
            return self.error_type();
        }
        if spread != empty_object_type {
            if !properties_array.is_empty() {
                let segment = create_object_literal_type!(self);
                spread = self.get_spread_type(
                    spread,
                    segment,
                    node_symbol,
                    object_flags,
                    in_const_context,
                );
                // Go resets propsArray/propsTable/hasComputed* here for the
                // (unreached) continued-accumulation tail; the port returns
                // immediately after the merge so the resets are dead code.
            }
            // remap the raw emptyObjectType fed in at the top into a fresh empty object literal type, unique to this use site
            // TODO(port M6 S7): mapType's empty-object re-mapping; the
            // identity is Go's answer when the spread carries no
            // empty-object constituent.
            return spread;
        }
        create_object_literal_type!(self)
    }

    /// Go: `func (c *Checker) checkComputedPropertyName(node) *Type`
    /// (checker.go:27336) — REAL (M6 S10): the typeNodeLinks resolved-type
    /// fold (circularity-guarded), the invalid-name probe, and the
    /// string|number|symbol|any assignability gate.
    pub(crate) fn check_computed_property_name(&mut self, node: NodeId) -> TypeId {
        if let Some(resolved) = self.type_node_links.get(node).resolved_type {
            return resolved;
        }
        let circular_constraint_type = self.circular_constraint_type();
        self.type_node_links.get(node).resolved_type = Some(circular_constraint_type);
        if Self::is_invalid_computed_property_name(self.store(), node) {
            let error_type = self.error_type();
            self.type_node_links.get(node).resolved_type = Some(error_type);
            return error_type;
        }
        let expression = self
            .store()
            .node(node)
            .as_computed_property_name()
            .expect("ComputedPropertyName data")
            .expression;
        let resolved = self.check_expression(expression);
        self.type_node_links.get(node).resolved_type = Some(resolved);
        // This will allow types number, string, symbol or any. It will also
        // allow enums, the unknown type, and any union of these types (like
        // string | number). (Go comment, verbatim.)
        let string_number_symbol_type = self.string_number_symbol_type();
        let allowed = !self.type_(resolved).flags.intersects(TypeFlags::NULLABLE)
            && (self.is_type_assignable_to_kind(
                resolved,
                TypeFlags::STRING_LIKE | TypeFlags::NUMBER_LIKE | TypeFlags::ESSYMBOL_LIKE,
            ) || self.is_type_assignable_to(resolved, string_number_symbol_type));
        if !allowed {
            self.error(
                node,
                &tsc_diagnostics::A_COMPUTED_PROPERTY_NAME_MUST_BE_OF_TYPE_STRING_NUMBER_SYMBOL_OR_ANY,
                &[],
            );
        }
        resolved
    }

    /// Go: `func isInvalidComputedPropertyName(node) bool` (checker.go:27330).
    fn is_invalid_computed_property_name(store: &dyn tsc_ast::NodeStore, node: tsc_ast::NodeId) -> bool {
        let parent = store.node(node).parent.get();
        let grandparent = store.node(parent).parent.get();
        let grandparent_kind = store.node(grandparent).kind;
        (grandparent_kind == Kind::TypeLiteral
            || crate::statements::is_class_like(store, grandparent)
            || grandparent_kind == Kind::InterfaceDeclaration)
            && store.node(node).kind == Kind::BinaryExpression
            && store
                .node(node)
                .as_binary_expression()
                .map(|d| store.node(d.operator_token).kind == Kind::InKeyword)
                .unwrap_or(false)
            && !matches!(
                store.node(parent).kind,
                Kind::GetAccessor | Kind::SetAccessor
            )
    }

    /// Go: `func (c *Checker) checkContextualDeprecations(node)` (checker.go:13605).
    pub(crate) fn check_contextual_deprecations(&mut self, node: NodeId) {
        let contextual_type = self.get_apparent_type_of_contextual_type(node, crate::types::ContextFlags::NONE);
        let property_nodes: Vec<NodeId> = {
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
        for property in property_nodes {
            if self.is_canceled() {
                return;
            }
            if let Some(name) = self.store().node(property).name()
                && self.store().node(name).kind != Kind::ComputedPropertyName
            {
                self.check_deprecated_property(name, contextual_type);
            }
        }
    }

    /// Go: `func (c *Checker) checkDeprecatedProperty(name, contextualType)` (checker.go:13616).
    pub(crate) fn check_deprecated_property(&mut self, name: NodeId, contextual_type: Option<TypeId>) {
        let Some(contextual_type) = contextual_type else { return };
        let text = tsc_ast::node_text(self.store(), name).to_string();
        let prop = relater::get_property_of_type(self, contextual_type, &text);
        let Some(prop) = prop else { return };
        if self.program_symbol(prop).declarations.is_empty() {
        }
        // TODO(port M6 S6): isDeprecatedSymbol + addDeprecatedSuggestion —
        // the @deprecated JSDoc family; permissive.
    }

    /// Go: `func (c *Checker) checkSpreadPropOverrides(t, props, spread)` (checker.go:13628).
    pub(crate) fn check_spread_prop_overrides(
        &mut self,
        t: TypeId,
        props: &tsc_ast::SymbolTable,
        spread: NodeId,
    ) {
        // (Go: `spread` backs the related-info attachment — S10 seam below.)
        let _ = spread;
        let right_props = relater::get_properties_of_type(self, t);
        for right in right_props {
            let right_flags = self.program_symbol(right).flags;
            let right_check_flags = self.program_symbol(right).check_flags;
            if !right_flags.intersects(SymbolFlags::OPTIONAL)
                && !right_check_flags.intersects(tsc_ast::CheckFlags::PARTIAL)
            {
                let name = self.program_symbol(right).name.clone();
                if let Some(&left) = props.get(&name) {
                    let left_value_declaration = self.program_symbol(left).value_declaration;
                    let left_name = self.program_symbol(left).name.clone();
                    if let Some(vd) = left_value_declaration {
                        self.error(
                            vd,
                            &X_0_IS_SPECIFIED_MORE_THAN_ONCE_SO_THIS_USAGE_WILL_BE_OVERWRITTEN,
                            &[left_name],
                        );
                        // Go: diagnostic.AddRelatedInfo(NewDiagnosticForNode(
                        //     spread, ThisSpreadAlwaysOverwritesThisProperty))
                        // TODO(port M6 W4): the related-info attachment
                        // (AddRelatedInfo's carrier wave); the main
                        // diagnostic carries the overwrite error (the
                        // related-info constant is referenced for the wave).
                        let _ = THIS_SPREAD_ALWAYS_OVERWRITES_THIS_PROPERTY;
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) getSpreadType(left, right, symbol, objectFlags, readonly) *Type`
    /// (checker.go:13641).
    pub(crate) fn get_spread_type(
        &mut self,
        left: TypeId,
        right: TypeId,
        symbol: Option<SymbolId>,
        object_flags: ObjectFlags,
        readonly: bool,
    ) -> TypeId {
        let left_flags = self.type_(left).flags;
        let right_flags = self.type_(right).flags;
        if left_flags.intersects(TypeFlags::ANY) || right_flags.intersects(TypeFlags::ANY) {
            return self.any_type();
        }
        if left_flags.intersects(TypeFlags::UNKNOWN) || right_flags.intersects(TypeFlags::UNKNOWN) {
            return self.unknown_type();
        }
        if left_flags.intersects(TypeFlags::NEVER) {
            return right;
        }
        if right_flags.intersects(TypeFlags::NEVER) {
            return left;
        }
        let left = self.try_merge_union_of_object_type_and_empty_object(left, readonly);
        if self.type_(left).flags.intersects(TypeFlags::UNION) {
            // TODO(port M6 S7): checkCrossProductUnion + mapType over the
            // union constituents; permissive errorType (Go's non-cross-
            // product answer).
            return self.error_type();
        }
        let right = self.try_merge_union_of_object_type_and_empty_object(right, readonly);
        if self.type_(right).flags.intersects(TypeFlags::UNION) {
            // TODO(port M6 S7): as above.
            return self.error_type();
        }
        if right_flags.intersects(
            TypeFlags::BOOLEAN_LIKE
                .union(TypeFlags::NUMBER_LIKE)
                .union(TypeFlags::BIGINT_LIKE)
                .union(TypeFlags::STRING_LIKE)
                .union(TypeFlags::ENUM_LIKE)
                .union(TypeFlags::NON_PRIMITIVE)
                .union(TypeFlags::INDEX),
        ) {
            return left;
        }
        if relater::is_generic_object_type(self, left) || relater::is_generic_object_type(self, right) {
            if relater::is_empty_object_type(self, left) {
                return right;
            }
            // When the left type is an intersection, we may need to merge the last constituent of the
            // intersection with the right type.
            if self.type_(left).flags.intersects(TypeFlags::INTERSECTION) {
                let types = self.type_(left).types().to_vec();
                let last_left = *types.last().expect("intersection has constituents");
                if self.is_non_generic_object_type(last_left)
                    && self.is_non_generic_object_type(right)
                {
                    let mut new_types = types;
                    let last = new_types.len() - 1;
                    new_types[last] =
                        self.get_spread_type(last_left, right, symbol, object_flags, readonly);
                    return relater::get_intersection_type(self, new_types);
                }
            }
            return relater::get_intersection_type(self, vec![left, right]);
        }
        let mut members = tsc_ast::SymbolTable::new();
        let mut skipped_private_members: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // REAL (M6 S10, checker.go:13691): indexInfos — the left==emptyObjectType
        // shortcut over the right's infos, else getUnionIndexInfos.
        let index_infos: Vec<crate::types::IndexInfoId> = if left == self.empty_object_type() {
            self.get_index_infos_of_type(right)
        } else {
            self.get_union_index_infos(&[left, right])
        };
        let right_props = relater::get_properties_of_type(self, right);
        for right_prop in right_props {
            let declaration_modifiers =
                relater::get_declaration_modifier_flags_from_symbol(self, right_prop);
            let name = self.program_symbol(right_prop).name.clone();
            if declaration_modifiers
                .intersects(ModifierFlags::PRIVATE.union(ModifierFlags::PROTECTED))
            {
                skipped_private_members.insert(name);
            } else if self.is_spreadable_property(right_prop) {
                let spread_symbol = self.get_spread_symbol(right_prop, readonly);
                members.set(name, spread_symbol);
            }
        }
        let left_props = relater::get_properties_of_type(self, left);
        for left_prop in left_props {
            let name = self.program_symbol(left_prop).name.clone();
            if skipped_private_members.contains(&name) || !self.is_spreadable_property(left_prop) {
                continue;
            }
            if let Some(&right_prop) = members.get(&name) {
                let right_type = self.get_type_of_symbol(right_prop);
                if self.program_symbol(right_prop).flags.intersects(SymbolFlags::OPTIONAL) {
                    let left_declarations = self.program_symbol(left_prop).declarations.clone();
                    let right_declarations = self.program_symbol(right_prop).declarations.clone();
                    let mut declarations = left_declarations;
                    declarations.extend(right_declarations);
                    let left_flags = self.program_symbol(left_prop).flags;
                    let flags =
                        SymbolFlags::PROPERTY.union(left_flags & SymbolFlags::OPTIONAL);
                    let result = self.new_symbol(flags, &name);
                    // Optimization: avoid calculating the union type if spreading into the exact same type.
                    let left_type = self.get_type_of_symbol(left_prop);
                    let left_without_undefined = self.remove_missing_or_undefined_type(left_type);
                    let right_without_undefined = self.remove_missing_or_undefined_type(right_type);
                    if left_without_undefined == right_without_undefined {
                        self.value_symbol_links.get(result).resolved_type = Some(left_type);
                    } else {
                        self.value_symbol_links.get(result).resolved_type = Some(
                            relater::get_union_type_ex(
                                self,
                                vec![left_type, right_without_undefined],
                                crate::todo_shapes::UnionReduction::Subtype,
                                None,
                                None,
                            ),
                        );
                    }
                    self.spread_links.get(result).left_spread = Some(left_prop);
                    self.spread_links.get(result).right_spread = Some(right_prop);
                    let s = self.program_symbol_mut(result);
                    s.declarations = declarations;
                    let name_type = self.value_symbol_links.get(left_prop).name_type;
                    self.value_symbol_links.get(result).name_type = name_type;
                    members.set(name, result);
                }
            } else {
                let spread_symbol = self.get_spread_symbol(left_prop, readonly);
                members.set(name, spread_symbol);
            }
        }
        // REAL (M6 S10, checker.go:13738): spreadIndexInfos :=
        // core.SameMap(indexInfos, func(info) { return c.getIndexInfoWithReadonly(info, readonly) }).
        let spread_index_infos: Vec<crate::types::IndexInfoId> = index_infos
            .into_iter()
            .map(|info| self.get_index_info_with_readonly(info, readonly))
            .collect();
        let spread = self.new_anonymous_type_with_members(
            symbol,
            members.clone(),
            object_flags | ObjectFlags::CONTAINS_SPREAD,
        );
        self.type_mut(spread).object_flags |=
            ObjectFlags::OBJECT_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        // Go's newAnonymousType(symbol, members, nil, nil, spreadIndexInfos):
        // the index-info slot rides the deferred-members writer.
        self.set_structured_type_members(spread, members, Vec::new(), Vec::new(), spread_index_infos);
        spread
    }

    /// Go: `func (c *Checker) getIndexInfoWithReadonly(info, readonly) *IndexInfo`
    /// (checker.go:13745).
    pub(crate) fn get_index_info_with_readonly(
        &mut self,
        info: crate::types::IndexInfoId,
        readonly: bool,
    ) -> crate::types::IndexInfoId {
        // TODO(port M6 S8): the IndexInfo arena's readonly-clone; identity
        // (the read seam is empty at this depth).
        // REAL (M6 S10, checker.go:13745): the arena's readonly-clone.
        let (key_type, value_type, is_readonly, declaration, components) = {
            let d = &self.index_infos[info.0 as usize];
            (
                d.key_type.expect("index info keyType"),
                d.value_type.expect("index info valueType"),
                d.is_readonly,
                d.declaration,
                d.components.clone(),
            )
        };
        if is_readonly != readonly {
            crate::types::IndexInfoId(self.new_index_info(key_type, value_type, readonly, declaration, components) as u32)
        } else {
            info
        }
    }

    /// Go: `func (c *Checker) getUnionIndexInfos(types) []*IndexInfo` (checker.go:13758) —
    /// REAL (M6 S10): re-points to members.rs's full port (the S8 read seam).
    pub(crate) fn get_union_index_infos(&mut self, types: &[TypeId]) -> Vec<crate::types::IndexInfoId> {
        self.get_union_index_infos_impl(types)
    }

    /// Go: `func (c *Checker) tryMergeUnionOfObjectTypeAndEmptyObject(t, readonly) *Type`
    /// (checker.go:13772).
    pub(crate) fn try_merge_union_of_object_type_and_empty_object(
        &mut self,
        t: TypeId,
        readonly: bool,
    ) -> TypeId {
        if !self.type_(t).flags.intersects(TypeFlags::UNION) {
            return t;
        }
        let constituents = self.type_(t).types().to_vec();
        // Go: Every(t.Types(), isEmptyObjectTypeOrSpreadsIntoEmptyObject) →
        // the empty / Find pair; then the spread-into-optional member table.
        let all_empty_like = constituents
            .iter()
            .all(|&c| self.is_empty_object_type_or_spreads_into_empty_object(c));
        if all_empty_like {
            if let Some(_empty) = constituents
                .iter()
                .copied()
                .find(|&c| relater::is_empty_object_type(self, c))
            {
                return _empty;
            }
            return self.empty_object_type();
        }
        let first_type = constituents
            .iter()
            .copied()
            .find(|&c| !self.is_empty_object_type_or_spreads_into_empty_object(c));
        let Some(first_type) = first_type else {
            return t;
        };
        let second_type = constituents
            .iter()
            .copied()
            .find(|&c| c != first_type && !self.is_empty_object_type_or_spreads_into_empty_object(c));
        if second_type.is_some() {
            return t;
        }
        // gets the type as if it had been spread, but where everything in the spread is made optional
        let _ = readonly;
        let mut members = tsc_ast::SymbolTable::new();
        let first_props = relater::get_properties_of_type(self, first_type);
        for prop in first_props {
            let declaration_modifiers =
                relater::get_declaration_modifier_flags_from_symbol(self, prop);
            if declaration_modifiers
                .intersects(ModifierFlags::PRIVATE.union(ModifierFlags::PROTECTED))
            {
                // do nothing, skip privates
            } else if self.is_spreadable_property(prop) {
                let is_setonly_accessor = {
                    let f = self.program_symbol(prop).flags;
                    f.intersects(SymbolFlags::SET_ACCESSOR) && !f.intersects(SymbolFlags::GET_ACCESSOR)
                };
                let flags = SymbolFlags::PROPERTY.union(SymbolFlags::OPTIONAL);
                let name = self.program_symbol(prop).name.clone();
                let prop_check_flags = self.program_symbol(prop).check_flags;
                let mut check_flags = prop_check_flags & tsc_ast::CheckFlags::LATE;
                if readonly {
                    check_flags |= tsc_ast::CheckFlags::READONLY;
                }
                let result = self.new_symbol_ex(flags, &name, check_flags);
                if is_setonly_accessor {
                    self.value_symbol_links.get(result).resolved_type =
                        Some(self.undefined_type());
                } else {
                    let prop_type = self.get_type_of_symbol(prop);
                    let optional = self.add_optionality_ex(prop_type, true /*isProperty*/, true /*isOptional*/);
                    self.value_symbol_links.get(result).resolved_type = Some(optional);
                }
                let declarations = self.program_symbol(prop).declarations.clone();
                self.program_symbol_mut(result).declarations = declarations;
                let name_type = self.value_symbol_links.get(prop).name_type;
                self.value_symbol_links.get(result).name_type = name_type;
                self.mapped_symbol_links.get(result).synthetic_origin = Some(prop);
                members.set(name, result);
            }
        }
        let symbol = self.type_(first_type).symbol;
        let spread = self.new_anonymous_type_with_members(
            symbol,
            members,
            ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL,
        );
        self.type_mut(spread).object_flags |= ObjectFlags::OBJECT_LITERAL;
        spread
    }

    /// Go: `func (c *Checker) isSpreadableProperty(prop) bool` (checker.go:13819).
    pub(crate) fn is_spreadable_property(&self, prop: SymbolId) -> bool {
        let s = self.program_symbol(prop);
        let flags = s.flags;
        let not_private_class_element = !s
            .declarations
            .iter()
            .any(|d| {
                tsc_ast::utilities::is_private_identifier_class_element_declaration(
                    self.store(), *d,
                )
            });
        (not_private_class_element
            && !flags.intersects(
                SymbolFlags::METHOD.union(SymbolFlags::GET_ACCESSOR).union(SymbolFlags::SET_ACCESSOR),
            ))
            || !s.declarations.iter().any(|d| {
                let parent = self.store().node(*d).parent.get();
                crate::statements::is_class_like(self.store(), parent)
            })
    }

    /// Go: `func (c *Checker) getSpreadSymbol(prop, readonly) *ast.Symbol` (checker.go:13826).
    pub(crate) fn get_spread_symbol(&mut self, prop: SymbolId, readonly: bool) -> SymbolId {
        let is_setonly_accessor = {
            let f = self.program_symbol(prop).flags;
            f.intersects(SymbolFlags::SET_ACCESSOR) && !f.intersects(SymbolFlags::GET_ACCESSOR)
        };
        if !is_setonly_accessor && readonly == relater::is_readonly_symbol(self, prop) {
            return prop;
        }
        let flags = SymbolFlags::PROPERTY
            .union(self.program_symbol(prop).flags & SymbolFlags::OPTIONAL);
        let name = self.program_symbol(prop).name.clone();
        let prop_check_flags = self.program_symbol(prop).check_flags;
        let mut check_flags = prop_check_flags & tsc_ast::CheckFlags::LATE;
        if readonly {
            check_flags |= tsc_ast::CheckFlags::READONLY;
        }
        let result = self.new_symbol_ex(flags, &name, check_flags);
        if is_setonly_accessor {
            self.value_symbol_links.get(result).resolved_type = Some(self.undefined_type());
        } else {
            let t = self.get_type_of_symbol(prop);
            self.value_symbol_links.get(result).resolved_type = Some(t);
        }
        let declarations = self.program_symbol(prop).declarations.clone();
        self.program_symbol_mut(result).declarations = declarations;
        let name_type = self.value_symbol_links.get(prop).name_type;
        self.value_symbol_links.get(result).name_type = name_type;
        self.mapped_symbol_links.get(result).synthetic_origin = Some(prop);
        result
    }

    /// Go: `func (c *Checker) isEmptyObjectTypeOrSpreadsIntoEmptyObject(t) bool`
    /// (checker.go:13842).
    pub(crate) fn is_empty_object_type_or_spreads_into_empty_object(&mut self, t: TypeId) -> bool {
        relater::is_empty_object_type(self, t)
            || self.type_(t).flags.intersects(
                TypeFlags::NULL
                    .union(TypeFlags::UNDEFINED)
                    .union(TypeFlags::BOOLEAN_LIKE)
                    .union(TypeFlags::NUMBER_LIKE)
                    .union(TypeFlags::BIGINT_LIKE)
                    .union(TypeFlags::STRING_LIKE)
                    .union(TypeFlags::ENUM_LIKE)
                    .union(TypeFlags::NON_PRIMITIVE)
                    .union(TypeFlags::INDEX),
            )
    }

    /// Go: `func (c *Checker) isReadonlyAssignmentDeclaration(node) bool` (checker.go:13911).
    pub(crate) fn is_readonly_assignment_declaration(&mut self, node: NodeId) -> bool {
        if self.store().node(node).kind != Kind::CallExpression {
            return false;
        }
        let arguments: Vec<NodeId> = self
            .store()
            .node(node)
            .as_call_expression()
            .expect("CallExpression data")
            .arguments
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        // PORT: Go indexes Arguments()[2] unguarded (the
        // Object.defineProperty call shape); the port answers false for the
        // short list instead of panicking.
        if arguments.len() <= 2 {
            return false;
        }
        let property_descriptor_type = self.check_expression_cached(arguments[2]);
        if let Some(_value_type) =
            self.get_type_of_property_of_type(property_descriptor_type, "value")
        {
            if let Some(writable_prop) =
                self.get_property_of_type(property_descriptor_type, "writable")
            {
                let writable_type = {
                    let value_declaration = self.program_symbol(writable_prop).value_declaration;
                    if let Some(vd) = value_declaration
                        && self.store().node(vd).kind == Kind::PropertyAssignment
                    {
                        let initializer = crate::ast_shims::initializer_of(self.store(), vd)
                            .unwrap_or(NodeId::NONE);
                        self.check_expression(initializer)
                    } else {
                        self.get_type_of_symbol(writable_prop)
                    }
                };
                return self.type_(writable_type).flags.intersects(TypeFlags::BOOLEAN_LITERAL)
                    && !crate::inference::get_boolean_literal_value(self, writable_type);
            }
            return true;
        }
        self.get_type_of_property_of_type(property_descriptor_type, "set")
            .is_none()
    }

    /// Go: `func (c *Checker) checkPropertyAssignment(node, checkMode) *Type` (checker.go:13879).
    pub(crate) fn check_property_assignment(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if let Some(name) = self.store().node(node).name()
            && self.store().node(name).kind == Kind::ComputedPropertyName
        {
            let _ = self.check_computed_property_name(name);
        }
        let initializer = crate::ast_shims::initializer_of(self.store(), node)
            .expect("PropertyAssignment has an initializer");
        let initializer_type = self.check_expression_for_mutable_location(initializer, check_mode);
        let type_node = {
            let d = self
                .store()
                .node(node)
                .as_property_assignment()
                .expect("PropertyAssignment data");
            if d.type_.is_none() { None } else { Some(d.type_) }
        };
        if let Some(type_node) = type_node {
            let t = self.get_type_from_type_node(type_node);
            self.check_type_assignable_to_and_optionally_elaborate(
                initializer_type,
                t,
                Some(node),
                Some(initializer),
                None, /*headMessage*/
                None,
            );
            return t;
        }
        initializer_type
    }

    /// Go: `func (c *Checker) checkShorthandPropertyAssignment(node, inDestructuringPattern, checkMode) *Type`
    /// (checker.go:13899).
    pub(crate) fn check_shorthand_property_assignment(
        &mut self,
        node: NodeId,
        in_destructuring_pattern: bool,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut expr = None;
        if !in_destructuring_pattern {
            expr = self
                .store()
                .node(node)
                .as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment data")
                .object_assignment_initializer;
        }
        let expr = expr.or_else(|| self.store().node(node).name());
        let Some(expr) = expr else {
            return self.error_type();
        };
        let expression_type = self.check_expression_for_mutable_location(expr, check_mode);
        let type_node = {
            let d = self
                .store()
                .node(node)
                .as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment data");
            if d.type_.is_none() { None } else { Some(d.type_) }
        };
        if let Some(type_node) = type_node {
            let t = self.get_type_from_type_node(type_node);
            self.check_type_assignable_to_and_optionally_elaborate(
                expression_type,
                t,
                Some(node),
                Some(expr),
                None, /*headMessage*/
                None,
            );
            return t;
        }
        expression_type
    }

    /// Go: `func (c *Checker) checkObjectLiteralMethod(node, checkMode) *Type` (checker.go:13938).
    pub(crate) fn check_object_literal_method(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        // Grammar checking
        // TODO(port M6 W3): checkGrammarMethod (the statements.rs carrier
        // is the permissive no-op).
        if let Some(name) = self.store().node(node).name()
            && self.store().node(name).kind == Kind::ComputedPropertyName
        {
            let _ = self.check_computed_property_name(name);
        }
        let uninstantiated_type =
            self.check_function_expression_or_object_literal_method(node, check_mode);
        self.instantiate_type_with_single_generic_call_signature(node, uninstantiated_type, check_mode)
    }

    /// Go: `func (c *Checker) checkFunctionExpressionOrObjectLiteralMethod(node, checkMode) *Type`
    /// (checker.go:10295).
    pub(crate) fn check_function_expression_or_object_literal_method(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        self.check_node_deferred(node);
        // TODO(port M6 S3): checkSourceElement(FullSignature) — the JSDoc
        // full-signature family; permissive.
        if self.store().node(node).kind == Kind::FunctionExpression {
            // TODO(port M6 S3): checkCollisionsForDeclarationName — the
            // statements.rs stub carrier.
        }
        if check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
            && self.is_context_sensitive(node)
        {
            // TODO(port M6 S9): the context-free return-only signature
            // (getContextualSignature + getReturnTypeFromBody + the
            // NonInferrableType construction); the seam answers
            // anyFunctionType.
            return self.any_function_type.expect("anyFunctionType");
        }
        // Grammar checking
        let _ = self.check_grammar_function_like_declaration(node);
        // TODO(port M6 S3): checkGrammarForGenerator (function expression arm).
        // contextuallyCheckFunctionExpressionOrObjectLiteralMethod — REAL
        // (W3T5, contextual.rs checker.go:10387): the contextual
        // parameter/return typing + checkSignatureDeclaration run exactly
        // where Go runs them (was the S9 seam's CONTEXT_CHECKED-only stub).
        self.contextually_check_function_expression_or_object_literal_method(node, check_mode);
        let symbol = self.get_symbol_of_declaration(node)
            .expect("function expression has a symbol");
        self.get_type_of_symbol(symbol)
    }

    /// Go: `func (c *Checker) checkClassExpression(node) *Type` (checker.go:10273).
    pub(crate) fn check_class_expression(&mut self, node: NodeId) -> TypeId {
        self.check_class_like_declaration(node);
        self.check_node_deferred(node);
        self.check_class_expression_external_helpers(node);
        let symbol = self.get_symbol_of_declaration(node)
            .expect("class expression has a symbol");
        self.get_type_of_symbol(symbol)
    }

    /// Go: `func (c *Checker) checkClassExpressionExternalHelpers(node)` (checker.go:10289)
    /// — the emit-helpers probe. Permissive (W5's checkExternalEmitHelpers
    /// seam is the no-op carrier).
    pub(crate) fn check_class_expression_external_helpers(&mut self, node: NodeId) {
        // TODO(port M6 W5): getFirstTransformableStaticClassElement +
        // checkExternalEmitHelpers (SetFunctionName/PropKey) — emitsupport.go's
        // wave.
        let _ = node;
    }

    /// Go: `func (c *Checker) checkClassExpressionDeferred(node)` (checker.go:10309).
    pub(crate) fn check_class_expression_deferred(&mut self, node: NodeId) {
        let members: Vec<NodeId> = self
            .store()
            .node(node)
            .as_class_expression()
            .expect("ClassExpression data")
            .members
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        self.check_source_elements(&members);
        self.check_constructor_declared_properties(node);
        self.register_for_unused_identifiers_check(node);
    }

    // Go: `func (c *Checker) getContextualSignature(node) *Signature`
    // (checker.go:10472 — S9): MOVED — the REAL body (the
    // contextual-signature family dispatcher) lives in contextual.rs as
    // `get_contextual_signature` (same signature).
}

/// Go: the property/shorthand/method membership probe of checkObjectLiteral's
/// dispatch (checker.go:13456's IsPropertyAssignment ||
/// IsShorthandPropertyAssignment || IsObjectLiteralMethod, with the
/// object-literal parent guard for methods).
#[allow(dead_code)]
fn is_object_literal_element_of_object_literal_expression_check() {}

