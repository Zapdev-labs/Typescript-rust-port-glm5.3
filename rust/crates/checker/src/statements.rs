// checker.go — the M6 S3 slice: the STATEMENT / DECLARATION / CLASS / MODULE
// checks (M6-DISPATCH wave 5). Ported from
// tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763,
// lines 2649-6293 (type-node checks, statement checks, class/interface/enum/
// module checks, import/export checks, variable/decorator checks),
// 6296-6915 (the iteration family), 6937-7530 (the alias/unused-identifier
// family), 18081 (checkRightHandSideOfForOf) and 20663-20989 (the return /
// yield aggregation spine slice S3 owns).
//
// PORT (file map): SPEC §4.2 maps checker.go → checker.rs; the slice lives
// here as `statements.rs` following the crate's one-.rs-per-slice-family
// convention (spine.rs = S1, typeofsymbol.rs = S2). Every function keeps its
// Go name snake_cased, same control flow, same evaluation order, same
// diagnostics.
//
// Stub directions (the slice table in M6-DISPATCH §2 + the wave brief):
//   * checkGrammar* → W3 (grammarchecks.go) — permissive no-op stubs below
//     (grammar checks are diagnostic-only; a false return is Go's
//     "no error reported" answer, which keeps every caller's `!`-composed
//     condition on the no-error path).
//   * resolveExternalModule*/getExportsOfModule → S6 (permissive).
//   * getResolvedSignature/resolveCall → S6 (permissive).
//   * getPropertyOfType → S8 (permissive None).
//   * getSpreadType → S4 (permissive).
//   * relater → REAL (relater.rs — routed directly).
//   * getUnionType multi-constituent → S7 (typeofsymbol.rs's pinned seam —
//     reused, not duplicated).
//   * flow (getFlowTypeOfReferenceEx/isReachableFlowNode body walks) → W3
//     (permissive: reachable; no narrowing).
//   * checkTruthinessOfType/checkReferenceExpression/… expression leaves →
//     S4 (permissive pass-through stubs).
//
// The IterationTypes/IterationTypesResolver todo_shapes shapes are REPLACED
// here with the real structs (checker.go:489-533) — this slice is their owner
// per the dispatch doc; todo_shapes keeps only a deprecated re-export for
// the two S1/S2 call sites that referenced the shape names before the swap.

use tsc_ast::functionflags::FunctionFlags;
use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    is_binary_expression, is_identifier, Kind, ModifierFlags, NodeId, NodeStore, SymbolFlags,
    SymbolId,
};
use tsc_core::text::TextPos;
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_diagnostics::{
    A_DEFAULT_CLAUSE_CANNOT_APPEAR_MORE_THAN_ONCE_IN_A_SWITCH_STATEMENT,
    A_REST_PARAMETER_MUST_BE_OF_AN_ARRAY_TYPE, A_RETURN_STATEMENT_CAN_ONLY_BE_USED_WITHIN_A_FUNCTION_BODY,
    A_RETURN_STATEMENT_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK,
    CATCH_CLAUSE_VARIABLE_CANNOT_HAVE_AN_INITIALIZER,
    CATCH_CLAUSE_VARIABLE_TYPE_ANNOTATION_MUST_BE_ANY_OR_UNKNOWN_IF_SPECIFIED,
    DUPLICATE_LABEL_0, NOT_ALL_CODE_PATHS_RETURN_A_VALUE, SETTERS_CANNOT_RETURN_A_VALUE,
    THE_BODY_OF_AN_IF_STATEMENT_CANNOT_BE_THE_EMPTY_STATEMENT,
    THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_BE_A_DESTRUCTURING_PATTERN,
    THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_OF_TYPE_STRING_OR_ANY,
    THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
    THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
    THE_RIGHT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_OF_TYPE_ANY_AN_OBJECT_TYPE_OR_A_TYPE_PARAMETER_BUT_HERE_HAS_TYPE_0,
    THE_WITH_STATEMENT_IS_NOT_SUPPORTED_ALL_SYMBOLS_IN_A_WITH_BLOCK_WILL_HAVE_TYPE_ANY,
    THIS_CONDITION_WILL_ALWAYS_RETURN_0,
    THIS_CONDITION_WILL_ALWAYS_RETURN_TRUE_SINCE_THIS_0_IS_ALWAYS_DEFINED,
    THIS_CONDITION_WILL_ALWAYS_RETURN_TRUE_SINCE_THIS_FUNCTION_IS_ALWAYS_DEFINED_DID_YOU_MEAN_TO_CALL_IT_INSTEAD,
    UNUSED_LABEL, X_FOR_AWAIT_LOOPS_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK,
    X_WITH_STATEMENTS_ARE_NOT_ALLOWED_IN_AN_ASYNC_FUNCTION_BLOCK,
    RETURN_TYPE_OF_CONSTRUCTOR_SIGNATURE_MUST_BE_ASSIGNABLE_TO_THE_INSTANCE_TYPE_OF_THE_CLASS,
    THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
};
use tsc_diagnostics as diag;

use crate::checker::CheckMode;
use crate::relater::{self, TypeFacts};
use crate::types::{ObjectFlags, SignatureKind, TypeFlags, TypeId};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// IterationUse / IterationTypes / IterationTypeKind / IterationTypesResolver
// (Go checker.go:484-533) — the REAL structs replacing todo_shapes.
// ────────────────────────────────────────────────────────────────────────────

define_flags!(IterationUse, u32);

impl IterationUse {
    /// Go: `IterationUseAllowsSyncIterablesFlag IterationUse = 1 << 0`.
    pub const ALLOWS_SYNC_ITERABLES_FLAG: IterationUse = IterationUse(1 << 0);
    /// Go: `IterationUseAllowsAsyncIterablesFlag IterationUse = 1 << 1`.
    pub const ALLOWS_ASYNC_ITERABLES_FLAG: IterationUse = IterationUse(1 << 1);
    /// Go: `IterationUseAllowsStringInputFlag IterationUse = 1 << 2`.
    pub const ALLOWS_STRING_INPUT_FLAG: IterationUse = IterationUse(1 << 2);
    /// Go: `IterationUseForOfFlag IterationUse = 1 << 3`.
    pub const FOR_OF_FLAG: IterationUse = IterationUse(1 << 3);
    /// Go: `IterationUseYieldStarFlag IterationUse = 1 << 4`.
    pub const YIELD_STAR_FLAG: IterationUse = IterationUse(1 << 4);
    /// Go: `IterationUseSpreadFlag IterationUse = 1 << 5`.
    pub const SPREAD_FLAG: IterationUse = IterationUse(1 << 5);
    /// Go: `IterationUseDestructuringFlag IterationUse = 1 << 6`.
    pub const DESTRUCTURING_FLAG: IterationUse = IterationUse(1 << 6);
    /// Go: `IterationUsePossiblyOutOfBounds IterationUse = 1 << 7`.
    pub const POSSIBLY_OUT_OF_BOUNDS: IterationUse = IterationUse(1 << 7);
    /// Go: `IterationUseElement = IterationUseAllowsSyncIterablesFlag`.
    pub const ELEMENT: IterationUse = IterationUse(1 << 0);
    /// Go: `IterationUseSpread = AllowsSync | SpreadFlag`.
    pub const SPREAD: IterationUse = IterationUse((1 << 0) | (1 << 5));
    /// Go: `IterationUseDestructuring = AllowsSync | DestructuringFlag`.
    pub const DESTRUCTURING: IterationUse = IterationUse((1 << 0) | (1 << 6));
    /// Go: `IterationUseForOf = AllowsSync|AllowsString|ForOfFlag`.
    pub const FOR_OF: IterationUse = IterationUse((1 << 0) | (1 << 2) | (1 << 3));
    /// Go: `IterationUseForAwaitOf = AllowsSync|AllowsAsync|AllowsString|ForOfFlag`.
    pub const FOR_AWAIT_OF: IterationUse =
        IterationUse((1 << 0) | (1 << 1) | (1 << 2) | (1 << 3));
    /// Go: `IterationUseYieldStar = AllowsSync | YieldStarFlag`.
    pub const YIELD_STAR: IterationUse = IterationUse((1 << 0) | (1 << 4));
    /// Go: `IterationUseAsyncYieldStar = AllowsSync|AllowsAsync|YieldStarFlag`.
    pub const ASYNC_YIELD_STAR: IterationUse = IterationUse((1 << 0) | (1 << 1) | (1 << 4));
    /// Go: `IterationUseGeneratorReturnType = AllowsSyncIterablesFlag`.
    pub const GENERATOR_RETURN_TYPE: IterationUse = IterationUse(1 << 0);
    /// Go: `IterationUseAsyncGeneratorReturnType = AllowsAsyncIterablesFlag`.
    pub const ASYNC_GENERATOR_RETURN_TYPE: IterationUse = IterationUse(1 << 1);
    /// Go: `IterationUseCacheFlags = AllowsSync|AllowsAsync|ForOfFlag`.
    pub const CACHE_FLAGS: IterationUse = IterationUse((1 << 0) | (1 << 1) | (1 << 3));
}

/// Go: `type IterationTypes struct` (checker.go:489).
#[derive(Clone, Copy, Debug, Default)]
pub struct IterationTypes {
    pub(crate) yield_type: Option<TypeId>,
    pub(crate) return_type: Option<TypeId>,
    pub(crate) next_type: Option<TypeId>,
}

impl IterationTypes {
    /// Go: `func (iterationTypes *IterationTypes) hasTypes() bool`.
    pub fn has_types(&self) -> bool {
        self.yield_type.is_some() || self.return_type.is_some() || self.next_type.is_some()
    }

    /// Go: `func (iterationTypes *IterationTypes) getType(typeKind) *Type`.
    pub fn get_type(&self, type_kind: IterationTypeKind) -> Option<TypeId> {
        match type_kind {
            IterationTypeKind::Yield => self.yield_type,
            IterationTypeKind::Return => self.return_type,
            IterationTypeKind::Next => self.next_type,
        }
    }
}

/// Go: `type IterationTypeKind int32` (checker.go:514, iota order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum IterationTypeKind {
    Yield = 0,
    Return = 1,
    Next = 2,
}

/// Go: `type IterationTypesKey struct` (checker.go — the iterationTypesCache
/// key; `use` is masked to `IterationUseCacheFlags` at the call site).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct IterationTypesKey {
    pub(crate) type_id: TypeId,
    pub(crate) use_: IterationUse,
}

/// Go: `type IterationTypesResolver struct` (checker.go:519-533) — the
/// sync/async pair of resolvers. The `getGlobal*` closures become enum
/// discriminants resolving through the checker's global-type resolver table
/// (spine.rs's GlobalResolvers — the memoization Go's closures carried);
/// `resolveIterationType` is the awaited-type hook (S9 for the async side —
/// permissive identity until the awaited family lands).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IterationTypesResolverKind {
    Sync,
    Async,
}

/// Go: `c.syncIterationTypesResolver` / `c.asyncIterationTypesResolver`
/// (constructed in initializeChecker — the checker.go:1084-1111 pair).
#[derive(Clone, Copy, Debug)]
pub struct IterationTypesResolver {
    pub(crate) kind: IterationTypesResolverKind,
}

impl Checker {
    /// Go: the resolver pair's `iteratorSymbolName`.
    pub(crate) fn resolver_iterator_symbol_name(&self, r: &IterationTypesResolver) -> &'static str {
        match r.kind {
            IterationTypesResolverKind::Sync => "Symbol.iterator",
            IterationTypesResolverKind::Async => "Symbol.asyncIterator",
        }
    }

    /// Go: the resolver pair's `getGlobalIteratorType` closure — the S1
    /// spine's memoized global-resolver accessors (spine.rs) ARE the
    /// closure payloads; the resolver discriminant picks the sync/async row.
    pub(crate) fn get_global_iterator_type(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterator_type(),
            IterationTypesResolverKind::Async => self.get_global_async_iterator_type(),
        }
    }

    /// Go: the resolver pair's `getGlobalIterableTypeChecked` closure.
    pub(crate) fn get_global_iterable_type_checked(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterable_type_checked(),
            IterationTypesResolverKind::Async => self.get_global_async_iterable_type_checked(),
        }
    }

    /// Go: the resolver pair's `getGlobalIterableIteratorType` closure.
    pub(crate) fn get_global_iterable_iterator_type(
        &mut self,
        r: IterationTypesResolver,
    ) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterable_iterator_type(),
            IterationTypesResolverKind::Async => {
                self.get_global_async_iterable_iterator_type()
            }
        }
    }

    /// Go: the resolver pair's `getGlobalIteratorObjectType` closure.
    pub(crate) fn get_global_iterator_object_type(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterator_object_type(),
            IterationTypesResolverKind::Async => self.get_global_async_iterator_object_type(),
        }
    }

    /// Go: the resolver pair's `getGlobalGeneratorType` closure.
    pub(crate) fn get_global_generator_type(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_generator_type(),
            IterationTypesResolverKind::Async => self.get_global_async_generator_type(),
        }
    }

    /// Go: the resolver pair's `getGlobalBuiltinIteratorTypes` closure.
    pub(crate) fn get_global_builtin_iterator_types(
        &mut self,
        r: IterationTypesResolver,
    ) -> Vec<TypeId> {
        // The builtin iterator list (lib *.d.ts Iterator types). The
        // global-type resolution machinery is S2/S5's getGlobalType seam;
        // permissive empty until the globals table carries them.
        let _ = r;
        Vec::new()
    }

    /// Go: the resolver pair's `resolveIterationType` closure — awaited for
    /// the async resolver, identity for the sync one. TODO(port M6 S9):
    /// `getAwaitedTypeEx` on the async side; identity is the sync answer.
    pub(crate) fn resolve_iteration_type(
        &mut self,
        r: IterationTypesResolver,
        t: Option<TypeId>,
        error_node: Option<NodeId>,
    ) -> Option<TypeId> {
        let _ = error_node;
        match r.kind {
            IterationTypesResolverKind::Sync => t,
            IterationTypesResolverKind::Async => t.map(|t| self.get_awaited_type(t)),
        }
    }

    /// The resolver pair's must-have diagnostics (checker.go:1084-1111).
    pub(crate) fn resolver_must_have_a_next_method_diagnostic(
        &self,
        r: IterationTypesResolver,
    ) -> &'static Message {
        match r.kind {
            IterationTypesResolverKind::Sync => {
                &diag::AN_ITERATOR_MUST_HAVE_A_NEXT_METHOD
            }
            IterationTypesResolverKind::Async => {
                &diag::AN_ASYNC_ITERATOR_MUST_HAVE_A_NEXT_METHOD
            }
        }
    }

    pub(crate) fn resolver_must_be_a_method_diagnostic(
        &self,
        r: IterationTypesResolver,
    ) -> &'static Message {
        match r.kind {
            IterationTypesResolverKind::Sync => {
                &diag::THE_0_PROPERTY_OF_AN_ITERATOR_MUST_BE_A_METHOD
            }
            IterationTypesResolverKind::Async => {
                &diag::THE_0_PROPERTY_OF_AN_ASYNC_ITERATOR_MUST_BE_A_METHOD
            }
        }
    }

    pub(crate) fn resolver_must_have_a_value_diagnostic(
        &self,
        r: IterationTypesResolver,
    ) -> &'static Message {
        match r.kind {
            IterationTypesResolverKind::Sync => {
                &diag::THE_TYPE_RETURNED_BY_THE_0_METHOD_OF_AN_ITERATOR_MUST_HAVE_A_VALUE_PROPERTY
            }
            IterationTypesResolverKind::Async => {
                &diag::THE_TYPE_RETURNED_BY_THE_0_METHOD_OF_AN_ASYNC_ITERATOR_MUST_BE_A_PROMISE_FOR_A_TYPE_WITH_A_VALUE_PROPERTY
            }
        }
    }

    pub(crate) fn sync_iteration_types_resolver(&self) -> IterationTypesResolver {
        IterationTypesResolver {
            kind: IterationTypesResolverKind::Sync,
        }
    }

    pub(crate) fn async_iteration_types_resolver(&self) -> IterationTypesResolver {
        IterationTypesResolver {
            kind: IterationTypesResolverKind::Async,
        }
    }

    /// Go: `func (c *Checker) getBuiltinIteratorReturnType() *Type`
    /// (checker.go:6625).
    pub(crate) fn get_builtin_iterator_return_type(&self) -> TypeId {
        if self.strict_builtin_iterator_return {
            self.undefined_type()
        } else {
            self.any_type()
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The W3 grammar-check stub family (grammarchecks.go — wave 13).
//
// PORT: every stub returns Go's "no error reported" (`false` for the bool
// producers, `true` for the `checkGrammarXxxInAmbientContext`-style gates
// where Go returns true when it REPORTED). The names map 1:1; the W3 wave
// replaces the bodies and keeps the call sites.
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkGrammarModifiers(node) bool` — TODO(port M6 W3).
    pub(crate) fn check_grammar_modifiers(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `checkGrammarStatementInAmbientContext` — TODO(port M6 W3).
    /// Go returns true when it reported (callers use `!`).
    pub(crate) fn check_grammar_statement_in_ambient_context(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) grammarErrorOnNode(node, message, args...) bool`.
    /// TODO(port M6 W3): the grammar-diagnostic helpers. Permissive: emits
    /// nothing, returns false (no error reported).
    pub(crate) fn grammar_error_on_node(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let _ = (node, message, args);
        false
    }

    /// Go: `func (c *Checker) grammarErrorOnFirstToken(node, message, args...) bool`.
    pub(crate) fn grammar_error_on_first_token(
        &mut self,
        node: NodeId,
        message: &'static Message,
        args: &[String],
    ) -> bool {
        let _ = (node, message, args);
        false
    }

    /// Go: `func (c *Checker) grammarErrorAtPos(file, pos, length, message, args...)`.
    pub(crate) fn grammar_error_at_pos(
        &mut self,
        file: NodeId,
        pos: TextPos,
        length: i32,
        message: &'static Message,
        args: &[String],
    ) {
        let _ = (file, pos, length, message, args);
    }

    /// Go: `func (c *Checker) checkGrammarProperty(node) bool` — TODO(port M6 W3).
    pub(crate) fn check_grammar_property(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarComputedPropertyName(node) *Type` —
    /// checker.go:27336 (S10's computed-name family). Permissive: the
    /// errorType answer (Go returns the checked expression type; the
    /// callers that compare two of these for identity still match).
    pub(crate) fn check_grammar_computed_property_name_stub(&mut self, node: NodeId) -> TypeId {
        let _ = node;
        self.error_type()
    }

    /// Go: `func (c *Checker) checkGrammarFunctionLikeDeclaration(node) bool`.
    pub(crate) fn check_grammar_function_like_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarIndexSignature(node) bool`.
    pub(crate) fn check_grammar_index_signature(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarConstructorTypeParameters(ctor) bool`.
    pub(crate) fn check_grammar_constructor_type_parameters(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarConstructorTypeAnnotation(ctor) bool`.
    pub(crate) fn check_grammar_constructor_type_annotation(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarAccessor(node) bool`.
    pub(crate) fn check_grammar_accessor(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarMethod(node) bool`.
    pub(crate) fn check_grammar_method(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarTypeArguments(node, typeArgumentList) bool`.
    pub(crate) fn check_grammar_type_arguments(
        &mut self,
        node: NodeId,
        type_argument_list: Option<NodeId>,
    ) -> bool {
        let _ = (node, type_argument_list);
        false
    }

    /// Go: `func (c *Checker) checkGrammarVariableDeclarationList(list) bool`.
    pub(crate) fn check_grammar_variable_declaration_list(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarForDisallowedBlockScopedVariableStatement(stmt) bool`.
    pub(crate) fn check_grammar_for_disallowed_block_scoped_variable_statement(
        &mut self,
        node: NodeId,
    ) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarVariableDeclaration(decl) bool`.
    pub(crate) fn check_grammar_variable_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarBindingElement(decl) bool`.
    pub(crate) fn check_grammar_binding_element(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarForInOrForOfStatement(stmt) bool`.
    pub(crate) fn check_grammar_for_in_or_for_of_statement(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarBreakOrContinueStatement(node) bool`.
    pub(crate) fn check_grammar_break_or_continue_statement(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarModuleElementContext(node, contextErrorMessage) bool`.
    pub(crate) fn check_grammar_module_element_context(
        &mut self,
        node: NodeId,
        context_error_message: &'static Message,
    ) -> bool {
        let _ = (node, context_error_message);
        false
    }

    /// Go: `func (c *Checker) checkGrammarClassLikeDeclaration(node) bool`.
    pub(crate) fn check_grammar_class_like_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarInterfaceDeclaration(decl) bool`.
    pub(crate) fn check_grammar_interface_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarMappedType(node) bool`.
    pub(crate) fn check_grammar_mapped_type(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarTypeOperatorNode(node) bool`.
    pub(crate) fn check_grammar_type_operator_node(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarForGenerator(node) bool`.
    pub(crate) fn check_grammar_for_generator(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarYieldExpression(node) bool`.
    pub(crate) fn check_grammar_yield_expression(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarImportClause(clause) bool`.
    pub(crate) fn check_grammar_import_clause(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarImportAttributeValues(attrs) bool`.
    pub(crate) fn check_grammar_import_attribute_values(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarImportAttributesType(node) bool`.
    pub(crate) fn check_grammar_import_attributes_type(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarExportDeclaration(decl) bool`.
    pub(crate) fn check_grammar_export_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarDecorator(node) bool`.
    pub(crate) fn check_grammar_decorator(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) hasParseDiagnostics(sourceFile) bool` — the
    /// parser diagnostics read (permissive false).
    pub(crate) fn has_parse_diagnostics(&self, source_file: NodeId) -> bool {
        let _ = source_file;
        false
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Statement checks (Go checker.go:3759-4304)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkBlock(node *ast.Node)` (checker.go:3804).
    pub(crate) fn check_block(&mut self, node: NodeId) {
        // Grammar checking for SyntaxKind.Block
        if self.store().node(node).kind == Kind::Block {
            self.check_grammar_statement_in_ambient_context(node);
        }
        let statements = block_statements(self.store(), node);
        if is_function_or_module_block(self.store(), node) {
            // Go saves/restores flowAnalysisDisabled around the walk (the
            // flag is W3's flow engine; the save/restore structure stays).
            let save_flow_analysis_disabled = self.flow_analysis_disabled;
            self.check_source_elements(&statements);
            self.flow_analysis_disabled = save_flow_analysis_disabled;
        } else {
            self.check_source_elements(&statements);
        }
        if has_locals(self.store(), node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) checkIfStatement(node)` (checker.go:3813).
    pub(crate) fn check_if_statement(&mut self, node: NodeId) {
        self.check_grammar_statement_in_ambient_context(node);
        let (expression, then_statement, else_statement) = {
            let d = self
                .store()
                .node(node)
                .as_if_statement()
                .expect("IfStatement data");
            (d.expression, d.then_statement, d.else_statement)
        };
        let t = self.check_truthiness_expression(expression, CheckMode::CHECK_MODE_NORMAL);
        self.check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(
            expression,
            t,
            then_statement,
        );
        self.check_source_element(Some(then_statement));
        if self.store().node(then_statement).kind == Kind::EmptyStatement {
            self.error(
                then_statement,
                &THE_BODY_OF_AN_IF_STATEMENT_CANNOT_BE_THE_EMPTY_STATEMENT,
                &[],
            );
        }
        self.check_source_element(else_statement);
    }

    /// Go: `func (c *Checker) checkTestingKnownTruthyCallableOrAwaitableOrEnumMemberType`
    /// (checker.go:3821).
    pub(crate) fn check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(
        &mut self,
        cond_expr: NodeId,
        cond_type: TypeId,
        body: NodeId,
    ) {
        if !self.strict_null_checks {
            return;
        }
        self.check_testing_known_truthy_types(cond_expr, cond_type, body);
    }

    /// Go: `func (c *Checker) checkTestingKnownTruthyTypes` (checker.go:3826).
    pub(crate) fn check_testing_known_truthy_types(
        &mut self,
        cond_expr: NodeId,
        cond_type: TypeId,
        body: NodeId,
    ) {
        let mut cond_expr = tsc_ast::utilities::skip_parentheses(self.store(), cond_expr);
        self.check_testing_known_truthy_type(cond_expr, cond_type, Some(body));
        while is_binary_expression(self.store().node(cond_expr)) {
            let op = self
                .store()
                .node(cond_expr)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .operator_token;
            let op_kind = self.store().node(op).kind;
            if op_kind != Kind::BarBarToken && op_kind != Kind::QuestionQuestionToken {
                break;
            }
            cond_expr = tsc_ast::utilities::skip_parentheses(
                self.store(),
                self.store()
                    .node(cond_expr)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .left,
            );
            self.check_testing_known_truthy_type(cond_expr, cond_type, Some(body));
        }
    }

    /// Go: `func (c *Checker) checkTestingKnownTruthyType` (checker.go:3833).
    pub(crate) fn check_testing_known_truthy_type(
        &mut self,
        cond_expr: NodeId,
        cond_type: TypeId,
        body: Option<NodeId>,
    ) {
        let mut location = cond_expr;
        let is_logical_or_coalescing =
            is_logical_or_coalescing_binary_expression(self.store(), cond_expr);
        if is_logical_or_coalescing {
            location = tsc_ast::utilities::skip_parentheses(
                self.store(),
                self.store()
                    .node(cond_expr)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .right,
            );
        }
        if is_module_exports_access_expression(self.store(), location) {
            return;
        }
        if is_logical_or_coalescing_binary_expression(self.store(), location) {
            self.check_testing_known_truthy_types(location, cond_type, body.expect("body"));
            return;
        }
        let mut t = cond_type;
        if location != cond_expr {
            t = self.check_expression(location);
        }
        let is_enum_literal = self.type_(t).flags.intersects(TypeFlags::ENUM_LITERAL);
        if is_enum_literal
            && self.store().node(location).kind == Kind::PropertyAccessExpression
        {
            // Go: OrElse(getResolvedSymbolOrNil(location.Expression()), unknownSymbol).Flags&Enum != 0
            let expression = self
                .store()
                .node(location)
                .as_property_access_expression()
                .expect("PropertyAccess data")
                .expression;
            let flags = self
                .get_resolved_symbol(expression)
                .map(|s| self.program.symbol(s).flags)
                .unwrap_or(SymbolFlags::ENUM);
            if flags.intersects(SymbolFlags::ENUM) {
                // EnumLiteral type at condition with known value is always
                // truthy or always falsy, likely an error.
                let is_truthy = self
                    .type_(t)
                    .as_literal_type()
                    .and_then(|d| d.value.as_ref())
                    .is_some_and(evaluator_is_truthy);
                self.error(
                    location,
                    &THIS_CONDITION_WILL_ALWAYS_RETURN_0,
                    &[if is_truthy { "true" } else { "false" }.to_string()],
                );
                return;
            }
        }
        let is_property_expression_cast = self.store().node(location).kind == Kind::PropertyAccessExpression
            && is_type_assertion(
                self.store(),
                self.store()
                    .node(location)
                    .as_property_access_expression()
                    .expect("PropertyAccess data")
                    .expression,
            );
        if !self.has_type_facts(t, type_facts_truthy()) || is_property_expression_cast {
            return;
        }
        // While it technically should be invalid for any known-truthy value
        // to be tested, we de-scope to functions and Promises unreferenced in
        // the block as a heuristic to identify the most common bugs.
        let call_signatures = relater::get_signatures_of_type(self, t, SignatureKind::Call);
        let is_promise = self.get_awaited_type_of_promise_stub(t).is_some();
        if call_signatures.is_empty() && !is_promise {
            return;
        }
        let mut tested_node: Option<NodeId> = None;
        match self.store().node(location).kind {
            Kind::Identifier => tested_node = Some(location),
            Kind::PropertyAccessExpression => {
                tested_node = Some(
                    self.store()
                        .node(location)
                        .as_property_access_expression()
                        .expect("PropertyAccess data")
                        .name,
                );
            }
            _ => {}
        }
        let mut tested_symbol: Option<SymbolId> = None;
        if let Some(tested_node) = tested_node {
            tested_symbol = self.get_symbol_at_location_stub(tested_node, false);
        }
        if tested_symbol.is_none() && !is_promise {
            return;
        }
        let is_used = tested_symbol.is_some_and(|sym| {
            let parent = self.store().node(cond_expr).parent.get();
            (is_binary_expression(self.store().node(parent))
                && self.is_symbol_used_in_binary_expression_chain(parent, sym))
                || body
                    .is_some_and(|b| self.is_symbol_used_in_condition_body(cond_expr, b, tested_node, sym))
        });
        if !is_used {
            if is_promise {
                let type_name = self.get_type_name_for_error_display(t);
                self.error_and_maybe_suggest_await(
                    location,
                    true,
                    &THIS_CONDITION_WILL_ALWAYS_RETURN_TRUE_SINCE_THIS_0_IS_ALWAYS_DEFINED,
                    &[type_name],
                );
            } else {
                self.error(
                    location,
                    &THIS_CONDITION_WILL_ALWAYS_RETURN_TRUE_SINCE_THIS_FUNCTION_IS_ALWAYS_DEFINED_DID_YOU_MEAN_TO_CALL_IT_INSTEAD,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) isSymbolUsedInBinaryExpressionChain` (checker.go:3885).
    pub(crate) fn is_symbol_used_in_binary_expression_chain(
        &mut self,
        mut node: NodeId,
        tested_symbol: SymbolId,
    ) -> bool {
        while is_binary_expression(self.store().node(node)) {
            let d = self
                .store()
                .node(node)
                .as_binary_expression()
                .expect("BinaryExpression data");
            if self.store().node(d.operator_token).kind != Kind::AmpersandAmpersandToken {
                break;
            }
            // Go: node.Right.ForEachChild(visit) — the right operand's
            // children (the visitor never sees the root, matching Go's
            // ForEachChild-only walk).
            let is_used = for_each_child_identifier_used(
                self,
                d.right,
                &mut |c, child| c.get_symbol_at_location_stub(child, false) == Some(tested_symbol),
            );
            if is_used {
                return true;
            }
            node = self.store().node(node).parent.get();
        }
        false
    }

    /// Go: `func (c *Checker) isSymbolUsedInConditionBody` (checker.go:3942).
    pub(crate) fn is_symbol_used_in_condition_body(
        &mut self,
        expr: NodeId,
        body: NodeId,
        tested_node: Option<NodeId>,
        tested_symbol: SymbolId,
    ) -> bool {
        let expr = Some(expr);
        let tested_node = tested_node;
        self.is_symbol_used_in_condition_body_visit(body, expr, tested_node, tested_symbol)
    }

    /// The `visit` closure of isSymbolUsedInConditionBody, lifted to a
    /// method (Go's recursive closure captures expr/testedNode/testedSymbol).
    fn is_symbol_used_in_condition_body_visit(
        &mut self,
        child_node: NodeId,
        expr: Option<NodeId>,
        tested_node: Option<NodeId>,
        tested_symbol: SymbolId,
    ) -> bool {
        if is_identifier(self.store().node(child_node)) {
            if let Some(child_symbol) = self.get_symbol_at_location_stub(child_node, false) {
                if child_symbol == tested_symbol {
                    // If the test was a simple identifier, the above check is sufficient
                    let expr_is_identifier = expr.is_some_and(|e| is_identifier(self.store().node(e)));
                    let tested_is_binary_target = tested_node.is_some_and(|tn| {
                        let parent = self.store().node(tn).parent.get();
                        is_binary_expression(self.store().node(parent))
                    });
                    if expr_is_identifier
                        || (tested_node.is_some_and(|tn| is_identifier(self.store().node(tn)))
                            && tested_is_binary_target)
                    {
                        return true;
                    }
                    // Otherwise we need to ensure the symbol is called on the same target
                    let mut tested_expression = tested_node
                        .map(|tn| self.store().node(tn).parent.get());
                    let mut child_expression = Some(self.store().node(child_node).parent.get());
                    while let (Some(te), Some(ce)) = (tested_expression, child_expression) {
                        let te_kind = self.store().node(te).kind;
                        let ce_kind = self.store().node(ce).kind;
                        if (is_identifier(self.store().node(te)) && is_identifier(self.store().node(ce)))
                            || (te_kind == Kind::ThisKeyword && ce_kind == Kind::ThisKeyword)
                        {
                            return self.get_symbol_at_location_stub(te, false)
                                == self.get_symbol_at_location_stub(ce, false);
                        } else if te_kind == Kind::PropertyAccessExpression
                            && ce_kind == Kind::PropertyAccessExpression
                        {
                            let te_name = self
                                .store()
                                .node(te)
                                .as_property_access_expression()
                                .expect("PropertyAccess data")
                                .name;
                            let ce_name = self
                                .store()
                                .node(ce)
                                .as_property_access_expression()
                                .expect("PropertyAccess data")
                                .name;
                            if self.get_symbol_at_location_stub(te_name, false)
                                != self.get_symbol_at_location_stub(ce_name, false)
                            {
                                return false;
                            }
                            child_expression = Some(
                                self.store()
                                    .node(ce)
                                    .as_property_access_expression()
                                    .expect("PropertyAccess data")
                                    .expression,
                            );
                            tested_expression = Some(
                                self.store()
                                    .node(te)
                                    .as_property_access_expression()
                                    .expect("PropertyAccess data")
                                    .expression,
                            );
                        } else if te_kind == Kind::CallExpression
                            && ce_kind == Kind::CallExpression
                        {
                            child_expression = Some(
                                self.store()
                                    .node(ce)
                                    .as_call_expression()
                                    .expect("CallExpression data")
                                    .expression,
                            );
                            tested_expression = Some(
                                self.store()
                                    .node(te)
                                    .as_call_expression()
                                    .expect("CallExpression data")
                                    .expression,
                            );
                        } else {
                            return false;
                        }
                    }
                }
            }
        }
        // childNode.ForEachChild(visit)
        children_of(self.store(), child_node)
            .iter()
            .any(|&child| self.is_symbol_used_in_condition_body_visit(child, expr, tested_node, tested_symbol))
    }

    /// Go: `func (c *Checker) checkDoStatement(node)` (checker.go:4008).
    pub(crate) fn check_do_statement(&mut self, node: NodeId) {
        self.check_grammar_statement_in_ambient_context(node);
        let d = self
            .store()
            .node(node)
            .as_do_statement()
            .expect("DoStatement data");
        self.check_source_element(Some(d.statement));
        self.check_truthiness_expression(d.expression, CheckMode::CHECK_MODE_NORMAL);
    }

    /// Go: `func (c *Checker) checkWhileStatement(node)` (checker.go:4013).
    pub(crate) fn check_while_statement(&mut self, node: NodeId) {
        self.check_grammar_statement_in_ambient_context(node);
        let d = self
            .store()
            .node(node)
            .as_while_statement()
            .expect("WhileStatement data");
        self.check_truthiness_expression(d.expression, CheckMode::CHECK_MODE_NORMAL);
        self.check_source_element(Some(d.statement));
    }

    /// Go: `func (c *Checker) checkForStatement(node)` (checker.go:4018).
    pub(crate) fn check_for_statement(&mut self, node: NodeId) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            let initializer = self
                .store()
                .node(node)
                .as_for_statement()
                .expect("ForStatement data")
                .initializer;
            if let Some(init) = initializer {
                if self.store().node(init).kind == Kind::VariableDeclarationList {
                    self.check_grammar_variable_declaration_list(init);
                }
            }
        }
        let d = self
            .store()
            .node(node)
            .as_for_statement()
            .expect("ForStatement data")
            .clone();
        if let Some(initializer) = d.initializer {
            if self.store().node(initializer).kind == Kind::VariableDeclarationList {
                self.check_variable_declaration_list(initializer);
            } else {
                self.check_expression(initializer);
            }
        }
        if let Some(condition) = d.condition {
            self.check_truthiness_expression(condition, CheckMode::CHECK_MODE_NORMAL);
        }
        if let Some(incrementor) = d.incrementor {
            self.check_expression(incrementor);
        }
        self.check_source_element(Some(d.statement));
        if has_locals(self.store(), node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) checkForInStatement(node)` (checker.go:4037).
    pub(crate) fn check_for_in_statement(&mut self, node: NodeId) {
        let data = self
            .store()
            .node(node)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement data")
            .clone();
        self.check_grammar_for_in_or_for_of_statement(node);
        let right_type = self.get_non_nullable_type_if_needed(self.check_expression(data.expression));
        // TypeScript 1.0 spec (April 2014): 5.4
        if self.store().node(data.initializer).kind == Kind::VariableDeclarationList {
            let declarations = self
                .store()
                .node(data.initializer)
                .as_variable_declaration_list()
                .expect("VariableDeclarationList data")
                .declarations
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            if let Some(&first) = declarations.first() {
                let name = self.store().node(first).name();
                if name.is_some_and(|n| is_binding_pattern(self.store(), n)) {
                    self.error(
                        name.expect("checked"),
                        &THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_BE_A_DESTRUCTURING_PATTERN,
                        &[],
                    );
                }
            }
            self.check_variable_declaration_list(data.initializer);
        } else {
            // In a 'for-in' statement of the form for (Var in Expr) Statement
            let var_expr = data.initializer;
            let left_type = self.check_expression(var_expr);
            let var_kind = self.store().node(var_expr).kind;
            if var_kind == Kind::ArrayLiteralExpression || var_kind == Kind::ObjectLiteralExpression {
                self.error(
                    var_expr,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_CANNOT_BE_A_DESTRUCTURING_PATTERN,
                    &[],
                );
            } else if !self
                .is_type_assignable_to(self.get_index_type_or_string(right_type), left_type)
            {
                self.error(
                    var_expr,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_OF_TYPE_STRING_OR_ANY,
                    &[],
                );
            } else {
                // run check only former check succeeded to avoid cascading errors
                self.check_reference_expression(
                    var_expr,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
                );
            }
        }
        // unknownType is returned i.e. if node.expression is identifier whose
        // name cannot be resolved — the error is already reported.
        if right_type == self.never_type()
            || !self.is_type_assignable_to_kind_stub(
                right_type,
                TypeFlags::NON_PRIMITIVE.union(TypeFlags::INSTANTIABLE_NON_PRIMITIVE),
            )
        {
            let right_text = self.type_to_string(right_type);
            self.error(
                data.expression,
                &THE_RIGHT_HAND_SIDE_OF_A_FOR_IN_STATEMENT_MUST_BE_OF_TYPE_ANY_AN_OBJECT_TYPE_OR_A_TYPE_PARAMETER_BUT_HERE_HAS_TYPE_0,
                &[right_text],
            );
        }
        self.check_source_element(Some(data.statement));
        if has_locals(self.store(), node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) getIndexTypeOrString(t)` (checker.go:4081).
    pub(crate) fn get_index_type_or_string(&mut self, t: TypeId) -> TypeId {
        let index_type = self.get_extract_string_type(relater::get_index_type(self, t));
        if self.type_(index_type).flags.intersects(TypeFlags::NEVER) {
            self.string_type()
        } else {
            index_type
        }
    }

    /// Go: `func (c *Checker) checkForOfStatement(node)` (checker.go:4086).
    pub(crate) fn check_for_of_statement(&mut self, node: NodeId) {
        let data = self
            .store()
            .node(node)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement data")
            .clone();
        self.check_grammar_for_in_or_for_of_statement(node);
        let container = u::get_containing_function_or_class_static_block(self.store(), node);
        if let Some(await_modifier) = data.await_modifier {
            if container
                .is_some_and(|c| self.store().node(c).kind == Kind::ClassStaticBlockDeclaration)
            {
                self.grammar_error_on_node(
                    await_modifier,
                    &X_FOR_AWAIT_LOOPS_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK,
                    &[],
                );
            } else {
                // Go: ast.GetFunctionFlags(container) — container may be nil;
                // Go's nil-safe flag read yields 0.
                let function_flags = container
                    .map(|c| tsc_ast::functionflags::get_function_flags(self.store(), c))
                    .unwrap_or(FunctionFlags::NORMAL);
                if function_flags
                    .intersects(FunctionFlags::ASYNC)
                    && !function_flags.intersects(FunctionFlags::INVALID)
                    && !self
                        .language_feature_minimum_target_reached(Feature::ForAwaitOf)
                {
                    // for..await..of prior to ESNext requires __asyncValues
                    self.check_external_emit_helpers_stub(node, ExternalEmitHelpers::FOR_AWAIT_OF_INCLUDES);
                }
            }
        }
        // Check the LHS and RHS
        if self.store().node(data.initializer).kind == Kind::VariableDeclarationList {
            self.check_variable_declaration_list(data.initializer);
        } else {
            let var_expr = data.initializer;
            let iterated_type = self.check_right_hand_side_of_for_of(node);
            let var_kind = self.store().node(var_expr).kind;
            if var_kind == Kind::ArrayLiteralExpression || var_kind == Kind::ObjectLiteralExpression {
                // iteratedType may be undefined — short-circuit the relation
                // with the errorType.
                let error_type = self.error_type();
                let iterated = iterated_type.unwrap_or(error_type);
                self.check_destructuring_assignment_stub(
                    var_expr,
                    iterated,
                    CheckMode::CHECK_MODE_NORMAL,
                    false,
                );
            } else {
                let left_type = self.check_expression(var_expr);
                self.check_reference_expression(
                    var_expr,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_MUST_BE_A_VARIABLE_OR_A_PROPERTY_ACCESS,
                    &THE_LEFT_HAND_SIDE_OF_A_FOR_OF_STATEMENT_MAY_NOT_BE_AN_OPTIONAL_PROPERTY_ACCESS,
                );
                // iteratedType will be undefined if the rightType was missing
                // properties/signatures required to get its iteratedType.
                if let Some(iterated_type) = iterated_type {
                    self.check_type_assignable_to_and_optionally_elaborate(
                        iterated_type,
                        left_type,
                        Some(var_expr),
                        Some(data.expression),
                        None,
                        None,
                    );
                }
            }
        }
        self.check_source_element(Some(data.statement));
        if has_locals(self.store(), node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) checkBreakOrContinueStatement(node)` (checker.go:4175).
    pub(crate) fn check_break_or_continue_statement(&mut self, node: NodeId) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            self.check_grammar_break_or_continue_statement(node);
        }
    }

    /// Go: `func (c *Checker) checkReturnStatement(node)` (checker.go:4179).
    pub(crate) fn check_return_statement(&mut self, node: NodeId) {
        // Always check the return expression so its identifiers are resolved
        // even when the return statement is misplaced.
        let expr_node = self
            .store()
            .node(node)
            .as_return_statement()
            .expect("ReturnStatement data")
            .expression;
        let mut expr_type = self.undefined_type();
        if let Some(expr_node) = expr_node {
            expr_type = self.check_expression_cached(expr_node);
        }
        if self.check_grammar_statement_in_ambient_context(node) {
            return;
        }
        let container = u::get_containing_function_or_class_static_block(self.store(), node);
        if let Some(container) = container {
            if self.store().node(container).kind == Kind::ClassStaticBlockDeclaration {
                self.grammar_error_on_first_token(
                    node,
                    &A_RETURN_STATEMENT_CANNOT_BE_USED_INSIDE_A_CLASS_STATIC_BLOCK,
                    &[],
                );
                return;
            }
        } else {
            self.grammar_error_on_first_token(
                node,
                &A_RETURN_STATEMENT_CAN_ONLY_BE_USED_WITHIN_A_FUNCTION_BODY,
                &[],
            );
            return;
        }
        let container = container.expect("checked");
        let signature = self
            .get_signature_from_declaration(container)
            .expect("containing function signature");
        let return_type = self.get_return_type_of_signature(signature);
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), container);
        if self.strict_null_checks
            || expr_node.is_some()
            || self.type_(return_type).flags.intersects(TypeFlags::NEVER)
        {
            let container_kind = self.store().node(container).kind;
            if container_kind == Kind::SetAccessor {
                if expr_node.is_some() {
                    self.error(node, &SETTERS_CANNOT_RETURN_A_VALUE, &[]);
                }
            } else if container_kind == Kind::Constructor {
                if let Some(expr_node) = expr_node {
                    if !self.check_type_assignable_to_and_optionally_elaborate(
                        expr_type,
                        return_type,
                        Some(node),
                        expr_node,
                        None,
                        None,
                    ) {
                        self.error(
                            node,
                            &RETURN_TYPE_OF_CONSTRUCTOR_SIGNATURE_MUST_BE_ASSIGNABLE_TO_THE_INSTANCE_TYPE_OF_THE_CLASS,
                            &[],
                        );
                    }
                }
            } else if self.get_return_type_from_annotation(container).is_some() {
                let unwrapped_return_type = self
                    .unwrap_return_type(return_type, function_flags)
                    .unwrap_or(return_type);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    expr_node,
                    expr_type,
                    false,
                );
            }
        } else if self.store().node(container).kind != Kind::Constructor
            && self.compiler_options.no_implicit_returns == Tristate::TSTrue
            && !self.is_unwrapped_return_type_undefined_void_or_any(container, return_type)
        {
            // The function has a return type, but the return statement
            // doesn't have an expression.
            self.error(node, &NOT_ALL_CODE_PATHS_RETURN_A_VALUE, &[]);
        }
    }

    /// Go: `func (c *Checker) checkReturnExpression(...)` (checker.go:4242).
    pub(crate) fn check_return_expression(
        &mut self,
        container: NodeId,
        unwrapped_return_type: TypeId,
        node: NodeId,
        expr: Option<NodeId>,
        expr_type: TypeId,
        in_conditional_expression: bool,
    ) {
        let mut unwrapped_expr_type = expr_type;
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), container);
        if let Some(expr) = expr {
            let unwrapped_expr = tsc_ast::utilities::skip_parentheses(self.store(), expr);
            if self.store().node(unwrapped_expr).kind == Kind::ConditionalExpression {
                let d = self
                    .store()
                    .node(unwrapped_expr)
                    .as_conditional_expression()
                    .expect("ConditionalExpression data");
                let (when_true, when_false) = (d.when_true, d.when_false);
                let when_true_type = self.check_expression(when_true);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    Some(when_true),
                    when_true_type,
                    true,
                );
                let when_false_type = self.check_expression(when_false);
                self.check_return_expression(
                    container,
                    unwrapped_return_type,
                    node,
                    Some(when_false),
                    when_false_type,
                    true,
                );
                return;
            }
        }
        let in_return_statement = self.store().node(node).kind == Kind::ReturnStatement;
        if function_flags.intersects(FunctionFlags::ASYNC) {
            unwrapped_expr_type = self.check_awaited_type(
                expr_type,
                false,
                node,
                &THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
            );
        }
        // The effective expression for diagnostics purposes.
        let effective_expr = match expr {
            Some(expr) => self.get_effective_check_node(expr),
            None => node,
        };
        let error_node = if in_return_statement && !in_conditional_expression {
            node
        } else {
            effective_expr
        };
        self.check_type_assignable_to_and_optionally_elaborate(
            unwrapped_expr_type,
            unwrapped_return_type,
            Some(error_node),
            Some(effective_expr),
            None,
            None,
        );
    }

    /// Go: `func (c *Checker) checkWithStatement(node)` (checker.go:4276).
    pub(crate) fn check_with_statement(&mut self, node: NodeId) {
        if !self.check_grammar_statement_in_ambient_context(node) {
            if self
                .store()
                .node(node)
                .flags
                .intersects(NF::AWAIT_CONTEXT)
            {
                self.grammar_error_on_first_token(
                    node,
                    &X_WITH_STATEMENTS_ARE_NOT_ALLOWED_IN_AN_ASYNC_FUNCTION_BLOCK,
                    &[],
                );
            }
        }
        let d = self
            .store()
            .node(node)
            .as_with_statement()
            .expect("WithStatement data");
        self.check_expression(d.expression);
        let source_file = self.get_source_file_node_of(node);
        if !self.has_parse_diagnostics(source_file) {
            let start = scanner_skip_trivia(self, source_file, self.store().node(node).pos());
            let end = self.store().node(d.statement).pos();
            self.grammar_error_at_pos(
                source_file,
                start,
                (end - start) as i32,
                &THE_WITH_STATEMENT_IS_NOT_SUPPORTED_ALL_SYMBOLS_IN_A_WITH_BLOCK_WILL_HAVE_TYPE_ANY,
                &[],
            );
        }
    }

    /// Go: `func (c *Checker) checkSwitchStatement(node)` (checker.go:4292).
    pub(crate) fn check_switch_statement(&mut self, node: NodeId) {
        // Grammar checking
        self.check_grammar_statement_in_ambient_context(node);
        let mut first_default_clause: Option<NodeId> = None;
        let mut has_duplicate_default_clause = false;
        let expression = self
            .store()
            .node(node)
            .as_switch_statement()
            .expect("SwitchStatement data")
            .expression;
        let expression_type = self.check_expression(expression);
        let case_block = self
            .store()
            .node(node)
            .as_switch_statement()
            .expect("SwitchStatement data")
            .case_block;
        let clauses = self
            .store()
            .node(case_block)
            .as_case_block()
            .expect("CaseBlock data")
            .clauses
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        for &clause in &clauses {
            // Grammar check for duplicate default clauses
            let clause_kind = self.store().node(clause).kind;
            if clause_kind == Kind::DefaultClause && !has_duplicate_default_clause {
                if first_default_clause.is_none() {
                    first_default_clause = Some(clause);
                } else {
                    self.grammar_error_on_node(
                        clause,
                        &A_DEFAULT_CLAUSE_CANNOT_APPEAR_MORE_THAN_ONCE_IN_A_SWITCH_STATEMENT,
                        &[],
                    );
                    has_duplicate_default_clause = true;
                }
            }
            if clause_kind == Kind::CaseClause {
                let clause_expr = self
                    .store()
                    .node(clause)
                    .as_case_or_default_clause()
                    .expect("CaseOrDefaultClause data")
                    .expression;
                let case_type = self.check_expression(clause_expr);
                if !self.is_type_equality_comparable_to(expression_type, case_type) {
                    // expressionType is not comparable to caseType, try the
                    // reversed check and report errors if it fails.
                    self.check_type_comparable_to(case_type, expression_type, Some(clause_expr), None);
                }
            }
            let statements = self
                .store()
                .node(clause)
                .as_case_or_default_clause()
                .expect("CaseOrDefaultClause data")
                .statements
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            self.check_source_elements(&statements);
            if self.compiler_options.no_fallthrough_cases_in_switch == Tristate::TSTrue {
                // TODO(port M6 W3): isReachableFlowNode — the flow walk.
                // Permissive: reachable flow nodes report the fallthrough,
                // but the fallthrough_flow_node read requires the binder's
                // flow wiring; the permissive skip keeps diagnostics off.
                let _ = clause;
            }
        }
        if has_locals(self.store(), case_block) {
            self.register_for_unused_identifiers_check(case_block);
        }
    }

    /// Go: `func (c *Checker) checkLabeledStatement(node)` (checker.go:4344).
    pub(crate) fn check_labeled_statement(&mut self, node: NodeId) {
        let d = self
            .store()
            .node(node)
            .as_labeled_statement()
            .expect("LabeledStatement data")
            .clone();
        let label_text = tsc_ast::node_text(self.store(), d.label).to_string();
        if !self.check_grammar_statement_in_ambient_context(node) {
            let mut current = self.store().node(node).parent.get();
            while current != NodeId::NONE && !is_function_like(self.store(), current) {
                if self.store().node(current).kind == Kind::LabeledStatement {
                    let label = self
                        .store()
                        .node(current)
                        .as_labeled_statement()
                        .expect("LabeledStatement data")
                        .label;
                    if tsc_ast::node_text(self.store(), label) == label_text {
                        self.grammar_error_on_node(d.label, &DUPLICATE_LABEL_0, &[label_text.clone()]);
                        break;
                    }
                }
                current = self.store().node(current).parent.get();
            }
        }
        if self.store().node(d.label).flags.intersects(NF::UNREACHABLE)
            && self.compiler_options.allow_unused_labels != Tristate::TSTrue
        {
            self.error_or_suggestion(
                self.compiler_options.allow_unused_labels == Tristate::TSFalse,
                d.label,
                &UNUSED_LABEL,
                &[],
            );
        }
        self.check_source_element(Some(d.statement));
    }

    /// Go: `func (c *Checker) checkThrowStatement(node)` (checker.go:4361).
    pub(crate) fn check_throw_statement(&mut self, node: NodeId) {
        let throw_expr = self
            .store()
            .node(node)
            .as_throw_statement()
            .expect("ThrowStatement data")
            .expression;
        if !self.check_grammar_statement_in_ambient_context(node) {
            if is_identifier(self.store().node(throw_expr))
                && tsc_ast::node_text(self.store(), throw_expr).is_empty()
            {
                self.grammar_error_at_pos(
                    node,
                    self.store().node(throw_expr).pos(),
                    0,
                    &diag::LINE_BREAK_NOT_PERMITTED_HERE,
                    &[],
                );
            }
        }
        self.check_expression(throw_expr);
    }

    /// Go: `func (c *Checker) checkTryStatement(node)` (checker.go:4372).
    pub(crate) fn check_try_statement(&mut self, node: NodeId) {
        self.check_grammar_statement_in_ambient_context(node);
        let d = self
            .store()
            .node(node)
            .as_try_statement()
            .expect("TryStatement data")
            .clone();
        self.check_block(d.try_block);
        if let Some(catch_clause) = d.catch_clause {
            self.check_catch_clause(catch_clause);
        }
        if let Some(finally_block) = d.finally_block {
            self.check_block(finally_block);
        }
    }

    /// Go: `func (c *Checker) checkCatchClause(node)` (checker.go:4382).
    pub(crate) fn check_catch_clause(&mut self, node: NodeId) {
        let declaration = self
            .store()
            .node(node)
            .as_catch_clause()
            .expect("CatchClause data")
            .variable_declaration;
        if let Some(declaration) = declaration {
            self.check_variable_like_declaration(declaration);
            let type_node = crate::ast_shims::type_of_node(self.store(), declaration);
            if let Some(type_node) = type_node {
                let t = self.get_type_from_type_node(type_node);
                if !self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                    self.grammar_error_on_first_token(
                        type_node,
                        &CATCH_CLAUSE_VARIABLE_TYPE_ANNOTATION_MUST_BE_ANY_OR_UNKNOWN_IF_SPECIFIED,
                        &[],
                    );
                }
            } else if crate::ast_shims::initializer_of(self.store(), declaration).is_some() {
                let initializer = crate::ast_shims::initializer_of(self.store(), declaration)
                    .expect("checked");
                self.grammar_error_on_first_token(
                    initializer,
                    &CATCH_CLAUSE_VARIABLE_CANNOT_HAVE_AN_INITIALIZER,
                    &[],
                );
            } else {
                // Go walks node.Locals() against the catch block's locals for
                // redeclarations — binder-set locals; the symbol tables are
                // the binder crate's seam (permissive: no redeclaration
                // diagnostics until the locals surface lands).
                // TODO(port M6 W3): the locals-walk redeclaration check.
            }
        }
        let block = self
            .store()
            .node(node)
            .as_catch_clause()
            .expect("CatchClause data")
            .block;
        self.check_block(block);
    }

    /// Go: `func (c *Checker) checkExpressionStatement(node)` (checker.go:7531 —
    /// the S1 slice's arm; body lives here with its statement family).
    pub(crate) fn check_expression_statement(&mut self, node: NodeId) {
        // Grammar checking
        self.check_grammar_statement_in_ambient_context(node);
        let expression = self
            .store()
            .node(node)
            .as_expression_statement()
            .expect("ExpressionStatement data")
            .expression;
        self.check_expression(expression);
    }
}

/// Go: `scanner.SkipTrivia(text, pos)` — the scanner-crate helper through the
/// checker's store (the with-statement's grammar positions).
fn scanner_skip_trivia(c: &Checker, source_file: NodeId, pos: TextPos) -> TextPos {
    let _ = (c, source_file);
    pos
}

// ────────────────────────────────────────────────────────────────────────────
// Local AST helpers (Go ast-package predicates the statement family reads;
// the same one-.rs-per-family convention as ast_shims.rs — fold candidates
// when the ast/utilities.go wholesale wave lands).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsFunctionOrModuleBlock(node *Node) bool` (ast).
pub(crate) fn is_function_or_module_block(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind != Kind::Block && n.kind != Kind::ModuleBlock {
        return false;
    }
    let parent = store.node(n.parent.get());
    matches!(
        parent.kind,
        Kind::Constructor
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::FunctionKeyword
            | Kind::ModuleDeclaration
    )
}

/// Go: `node.Statements()` for Block/ModuleBlock (the two checkBlock inputs).
fn block_statements(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::Block => n
            .as_block()
            .expect("Block data")
            .statements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        Kind::ModuleBlock => n
            .as_module_block()
            .expect("ModuleBlock data")
            .statements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Go: `node.Locals() != nil/len != 0` — the locals-container kinds the
/// statement family registers for the unused-identifiers check.
fn has_locals(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    let empty = |l: &tsc_ast::SymbolTable| !l.is_empty();
    match n.kind {
        Kind::Block => empty(&n.as_block().expect("Block data").locals),
        // Go's ModuleBlock embeds LocalsContainerBase, but the Rust payload
        // keeps the locals on the ModuleDeclaration (PORT: §5.1 data shape).
        Kind::ModuleBlock => false,
        Kind::ForStatement => empty(&n.as_for_statement().expect("ForStatement data").locals),
        Kind::ForInStatement | Kind::ForOfStatement => empty(
            &n.as_for_in_or_of_statement()
                .expect("ForInOrOfStatement data")
                .locals,
        ),
        Kind::CaseBlock => empty(&n.as_case_block().expect("CaseBlock data").locals),
        Kind::CatchClause => empty(&n.as_catch_clause().expect("CatchClause data").locals),
        Kind::ClassStaticBlockDeclaration => empty(
            &n.as_class_static_block_declaration()
                .expect("ClassStaticBlockDeclaration data")
                .locals,
        ),
        Kind::SourceFile => empty(&n.as_source_file().expect("SourceFile data").locals),
        Kind::ModuleDeclaration => {
            empty(&n.as_module_declaration().expect("ModuleDeclaration data").locals)
        }
        _ => false,
    }
}

/// Go: `func IsLogicalOrCoalescingBinaryExpression(node *Node) bool` (ast).
pub(crate) fn is_logical_or_coalescing_binary_expression(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let n = store.node(node);
    if !is_binary_expression(n) {
        return false;
    }
    matches!(
        store
            .node(n.as_binary_expression().expect("BinaryExpression data").operator_token)
            .kind,
        Kind::AmpersandAmpersandToken | Kind::BarBarToken | Kind::QuestionQuestionToken
    )
}

/// Go: `func IsModuleExportsAccessExpression(node *Node) bool` (ast) —
/// `module.exports` / `module["exports"]` (the JS module-exports seam).
pub(crate) fn is_module_exports_access_expression(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::PropertyAccessExpression => {
            let d = n.as_property_access_expression().expect("PropertyAccess data");
            store.node(d.expression).kind == Kind::Identifier
                && tsc_ast::node_text(store, d.expression) == "module"
                && tsc_ast::node_text(store, d.name) == "exports"
        }
        Kind::ElementAccessExpression => {
            let d = n.as_element_access_expression().expect("ElementAccess data");
            store.node(d.expression).kind == Kind::Identifier
                && tsc_ast::node_text(store, d.expression) == "module"
                && store.node(d.argument_expression).kind == Kind::StringLiteral
                && tsc_ast::node_text(store, d.argument_expression) == "exports"
        }
        _ => false,
    }
}

/// Go: `func IsAssertionExpression(node *Node) bool` (ast).
pub(crate) fn is_type_assertion(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::TypeAssertionExpression | Kind::AsExpression | Kind::SatisfiesExpression
    )
}

/// Go: `func IsBindingPattern(node *Node) bool` (ast).
pub(crate) fn is_binding_pattern(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::ArrayBindingPattern | Kind::ObjectBindingPattern
    )
}

/// Go: `func IsBindingElement(node *Node) bool` (ast).
pub(crate) fn is_binding_element(store: &dyn NodeStore, node: NodeId) -> bool {
    store.node(node).kind == Kind::BindingElement
}

/// Go: evaluator.IsTruthy (internal/evaluator/evaluator.go:156).
pub(crate) fn evaluator_is_truthy(v: &crate::types::LiteralValue) -> bool {
    use crate::types::LiteralValue;
    match v {
        LiteralValue::String(s) => !s.is_empty(),
        LiteralValue::Number(n) => n.0 != 0.0 && !n.0.is_nan(),
        LiteralValue::Boolean(b) => *b,
        LiteralValue::BigInt(b) => *b != tsc_jsnum::PseudoBigInt::default(),
        LiteralValue::None => false,
    }
}

/// Go: `TypeFactsTruthy TypeFacts = 1 << 22` (checker.go:425) — relater's
/// TypeFacts carries only the bits its relation reads; the statement family
/// reads the truthiness bit through the same bitset.
pub(crate) fn type_facts_truthy() -> TypeFacts {
    TypeFacts::from_bits(1 << 22)
}

/// The node's children, collected (Go's ForEachChild walk needs the store
/// borrow alive across &mut self calls — collect-then-walk, same order).
pub(crate) fn children_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let mut children = Vec::new();
    let n = store.node(node);
    n.for_each_child(&mut |child| {
        children.push(child);
        false
    });
    children
}

/// Go's `visit` closure over `node.ForEachChild(visit)` in
/// isSymbolUsedInBinaryExpressionChain — the children of the `&&` chain's
/// right operand, matching Go's ForEachChild-only walk (never the root).
fn for_each_child_identifier_used<F>(c: &mut Checker, node: NodeId, visit: &mut F) -> bool
where
    F: FnMut(&mut Checker, NodeId) -> bool,
{
    children_of(c.store(), node).iter().any(|&child| visit(c, child))
}

/// Go: `ast.ForEachReturnStatement(node, cb)` (ast — the return-statement
/// walk; cb returns true to bail early). PORT: the walk collects via
/// for_each_child; cb runs after each find.
fn for_each_return_statement<F>(c: &mut Checker, body: Option<NodeId>, cb: &mut F) -> bool
where
    F: FnMut(&mut Checker, NodeId) -> bool,
{
    let Some(body) = body else { return false };
    fn visit<F: FnMut(&mut Checker, NodeId) -> bool>(c: &mut Checker, node: NodeId, cb: &mut F) -> bool {
        if c.store().node(node).kind == Kind::ReturnStatement && cb(c, node) {
            return true;
        }
        children_of(c.store(), node)
            .iter()
            .any(|&child| visit(c, child, cb))
    }
    visit(c, body, cb)
}

// ────────────────────────────────────────────────────────────────────────────
// ExternalEmitHelpers (Go checker/types.go:130-150) — the emit-helper
// bitfield checkExternalEmitHelpers reads. TODO(port M6 S10): the real
// checkExternalEmitHelpers (checker.go:29105 — the context/emit-helpers
// machinery); permissive no-op here.
// ────────────────────────────────────────────────────────────────────────────

define_flags!(ExternalEmitHelpers, u32);

impl ExternalEmitHelpers {
    pub const REST: ExternalEmitHelpers = ExternalEmitHelpers(1 << 0);
    pub const DECORATE: ExternalEmitHelpers = ExternalEmitHelpers(1 << 1);
    pub const PARAM: ExternalEmitHelpers = ExternalEmitHelpers(1 << 2);
    pub const AWAITER: ExternalEmitHelpers = ExternalEmitHelpers(1 << 3);
    pub const ASYNC_VALUES: ExternalEmitHelpers = ExternalEmitHelpers(1 << 4);
    pub const ASYNC_GENERATOR_INCLUDES: ExternalEmitHelpers = ExternalEmitHelpers(1 << 5);
    pub const EXPORT_STAR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 6);
    pub const IMPORT_STAR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 7);
    pub const IMPORT_DEFAULT: ExternalEmitHelpers = ExternalEmitHelpers(1 << 8);
    pub const ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES: ExternalEmitHelpers =
        ExternalEmitHelpers(1 << 9);
    pub const SET_FUNCTION_NAME: ExternalEmitHelpers = ExternalEmitHelpers(1 << 10);
    pub const PROP_KEY: ExternalEmitHelpers = ExternalEmitHelpers(1 << 11);
    /// Go: `ExternalEmitHelpersForAwaitOfIncludes = ExternalEmitHelpersAsyncValues`.
    pub const FOR_AWAIT_OF_INCLUDES: ExternalEmitHelpers = ExternalEmitHelpers(1 << 4);
    /// Go: `ExternalEmitHelpersESDecorateAndRunInitializers = Decorate | Param`.
    pub const ES_DECORATE_AND_RUN_INITIALIZERS: ExternalEmitHelpers =
        ExternalEmitHelpers((1 << 1) | (1 << 2));
}

/// Go: `LanguageFeatureMinimumTarget` (checker/types.go:1450-1490) — the
/// feature→target table the statement/variable checks read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Feature {
    AsyncFunctions,
    ForAwaitOf,
    AsyncGenerators,
    ObjectSpreadRest,
    UsingAndAwaitUsing,
    ClassAndClassElementDecorators,
}

impl Feature {
    /// Go: the LanguageFeatureMinimumTarget table (types.go:1473-1490).
    pub(crate) fn minimum_target(self) -> tsc_core::options_generated::ScriptTarget {
        use tsc_core::options_generated::ScriptTarget;
        match self {
            Feature::AsyncFunctions => ScriptTarget::ES2017,
            Feature::ForAwaitOf => ScriptTarget::ES2018,
            Feature::AsyncGenerators => ScriptTarget::ES2018,
            Feature::ObjectSpreadRest => ScriptTarget::ES2018,
            Feature::UsingAndAwaitUsing => ScriptTarget::ESNext,
            Feature::ClassAndClassElementDecorators => ScriptTarget::ESNext,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The permissive callee stubs (later slices own the real bodies; the banner
// names the owner per the slice table).
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkTruthinessExpression(node, checkMode) *Type`
    /// (checker.go:11179). The spine is S1's; the `checkTruthinessOfType`
    /// tail is S4 — TODO(port M6 S4): the real truthiness facts narrowing.
    pub(crate) fn check_truthiness_expression(&mut self, node: NodeId, check_mode: CheckMode) -> TypeId {
        let t = self.check_expression_ex(node, check_mode);
        self.check_truthiness_of_type(t, node)
    }

    /// Go: `func (c *Checker) checkTruthinessOfType(t, node) *Type`
    /// (checker.go:13109 — S4). Permissive: the type passes through
    /// unchanged (no truthiness narrowing until S4/W3).
    pub(crate) fn check_truthiness_of_type(&mut self, t: TypeId, node: NodeId) -> TypeId {
        let _ = node;
        t
    }

    /// Go: `func (c *Checker) getSymbolAtLocation(node, ignoreErrors) *ast.Symbol`
    /// (checker.go:32147 — S10's getSymbolOfNode resolution). Permissive
    /// None (the tested-symbol paths skip their diagnostics).
    pub(crate) fn get_symbol_at_location_stub(&mut self, node: NodeId, ignore_errors: bool) -> Option<SymbolId> {
        let _ = (node, ignore_errors);
        None
    }

    /// Go: `func (c *Checker) getAwaitedTypeOfPromise(t) *Type` (S9's
    /// awaited family). Permissive None.
    pub(crate) fn get_awaited_type_of_promise_stub(&mut self, t: TypeId) -> Option<TypeId> {
        let _ = t;
        None
    }

    /// Go: `func (c *Checker) isTypeAssignableToKind(source, kind) bool`
    /// (checker.go:28175 — S10). Permissive true (the for-in right-side
    /// diagnostic never fires).
    pub(crate) fn is_type_assignable_to_kind_stub(&mut self, source: TypeId, kind: TypeFlags) -> bool {
        let _ = (source, kind);
        true
    }

    /// Go: `func (c *Checker) checkDestructuringAssignment(...)` (S4's
    /// destructuring family). Permissive no-op.
    pub(crate) fn check_destructuring_assignment_stub(
        &mut self,
        expr: NodeId,
        type_: TypeId,
        check_mode: CheckMode,
        is_destructuring_assignment: bool,
    ) {
        let _ = (expr, type_, check_mode, is_destructuring_assignment);
    }

    /// Go: `func (c *Checker) checkExternalEmitHelpers(node, helpers)`
    /// (checker.go:29105 — S10). Permissive no-op (the emit-helpers
    /// context machinery is the emit wave's).
    pub(crate) fn check_external_emit_helpers_stub(&mut self, node: NodeId, helpers: ExternalEmitHelpers) {
        let _ = (node, helpers);
    }

    /// Go: `func (c *Checker) checkReferenceExpression(expr, invalidReferenceMessage, invalidOptionalChainMessage) bool`
    /// (checker.go:13374 — S4). Permissive true (valid reference).
    pub(crate) fn check_reference_expression(
        &mut self,
        expr: NodeId,
        invalid_reference_message: &'static Message,
        invalid_optional_chain_message: &'static Message,
    ) -> bool {
        let _ = (expr, invalid_reference_message, invalid_optional_chain_message);
        true
    }

    /// Go: `func (c *Checker) checkAwaitedType(t, withAlias, errorNode, message)`
    /// (checker.go:31781 — S9/S10's awaited family). Permissive pass-through
    /// (the awaited unwrap on async returns is identity until S9).
    pub(crate) fn check_awaited_type(
        &mut self,
        t: TypeId,
        with_alias: bool,
        error_node: NodeId,
        message: &'static Message,
    ) -> TypeId {
        let _ = (with_alias, error_node, message);
        t
    }

    /// Go: `func (c *Checker) getEffectiveCheckNode(node)` (S4). Permissive
    /// identity.
    pub(crate) fn get_effective_check_node(&mut self, node: NodeId) -> NodeId {
        node
    }

    /// Go: `func (c *Checker) isTypeEqualityComparableTo(source, target) bool`
    /// (checker.go:13105) — `isTypeComparableTo` in both directions; the
    /// relation runs through relater's Comparable relation directly.
    pub(crate) fn is_type_equality_comparable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.check_type_comparable_to(source, target, None, None)
            && self.check_type_comparable_to(target, source, None, None)
    }

    /// Go: `func (c *Checker) getResolvedSymbolOrNil(node)` — S6's
    /// resolveName seam; the exports.rs method is the permissive carrier.
    pub(crate) fn get_resolved_symbol_nil(&mut self, node: NodeId) -> Option<SymbolId> {
        self.get_resolved_symbol(node)
    }

    /// Go: `func (c *Checker) isReachableFlowNode(flowNode) bool` (flow.go —
    /// W3). Permissive reachable (matches spine's is_reachable_flow_node_stub).
    pub(crate) fn is_reachable_flow_node(&mut self, flow_node: tsc_ast::FlowNodeId) -> bool {
        let _ = flow_node;
        true
    }

    /// Go: `func (c *Checker) getAwaitedTypeNoAlias(t) *Type` (S9). Permissive
    /// Some(t) identity (Go's OrElse(callers) then keeps t).
    pub(crate) fn get_awaited_type_no_alias(&mut self, t: TypeId) -> Option<TypeId> {
        Some(t)
    }

    /// Go: `func (c *Checker) unwrapAwaitedType(t) *Type` (S9). Identity.
    pub(crate) fn unwrap_awaited_type(&mut self, t: TypeId) -> TypeId {
        t
    }

    /// Go: `func (c *Checker) getContextualType(node, contextFlags) *Type`
    /// (checker.go:29448 — S9). Permissive None.
    pub(crate) fn get_contextual_type_stub(
        &mut self,
        node: NodeId,
        context_flags: crate::types::ContextFlags,
    ) -> Option<TypeId> {
        let _ = (node, context_flags);
        None
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The iteration family (Go checker.go:6296-6915)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkIteratedTypeOrElementType(use, inputType, sentType, errorNode) *Type`
    /// (checker.go:6296).
    pub(crate) fn check_iterated_type_or_element_type(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        sent_type: TypeId,
        error_node: Option<NodeId>,
    ) -> TypeId {
        if is_type_any(self, input_type) {
            return input_type;
        }
        if let Some(t) =
            self.get_iterated_type_or_element_type(use_, input_type, sent_type, error_node, true)
        {
            return t;
        }
        self.any_type()
    }

    /// Go: `func (c *Checker) getIteratedTypeOrElementType(use, inputType, sentType, errorNode, checkAssignability) *Type`
    /// (checker.go:6306).
    pub(crate) fn get_iterated_type_or_element_type(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        sent_type: TypeId,
        error_node: Option<NodeId>,
        check_assignability: bool,
    ) -> Option<TypeId> {
        let allow_async_iterables = use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG);
        if input_type == self.never_type() {
            if let Some(error_node) = error_node {
                self.report_type_not_iterable_error(error_node, input_type, allow_async_iterables);
            }
            return None;
        }
        let iterable_exists = self.get_global_iterable_type() != self.empty_generic_type.expect("emptyGenericType");
        let possible_out_of_bounds = self.compiler_options.no_unchecked_indexed_access == Tristate::TSTrue
            && use_.intersects(IterationUse::POSSIBLY_OUT_OF_BOUNDS);
        if iterable_exists || allow_async_iterables {
            let iteration_types = self.get_iteration_types_of_iterable(
                input_type,
                use_,
                if iterable_exists { error_node } else { None },
            );
            if check_assignability && iteration_types.next_type.is_some() {
                let next_type = iteration_types.next_type.expect("checked");
                let diagnostic = if use_.intersects(IterationUse::FOR_OF_FLAG) {
                    Some(
                        &diag::CANNOT_ITERATE_VALUE_BECAUSE_THE_NEXT_METHOD_OF_ITS_ITERATOR_EXPECTS_TYPE_1_BUT_FOR_OF_WILL_ALWAYS_SEND_0,
                    )
                } else if use_.intersects(IterationUse::SPREAD_FLAG) {
                    Some(
                        &diag::CANNOT_ITERATE_VALUE_BECAUSE_THE_NEXT_METHOD_OF_ITS_ITERATOR_EXPECTS_TYPE_1_BUT_ARRAY_SPREAD_WILL_ALWAYS_SEND_0,
                    )
                } else if use_.intersects(IterationUse::DESTRUCTURING_FLAG) {
                    Some(
                        &diag::CANNOT_ITERATE_VALUE_BECAUSE_THE_NEXT_METHOD_OF_ITS_ITERATOR_EXPECTS_TYPE_1_BUT_ARRAY_DESTRUCTURING_WILL_ALWAYS_SEND_0,
                    )
                } else if use_.intersects(IterationUse::YIELD_STAR_FLAG) {
                    Some(
                        &diag::CANNOT_DELEGATE_ITERATION_TO_VALUE_BECAUSE_THE_NEXT_METHOD_OF_ITS_ITERATOR_EXPECTS_TYPE_1_BUT_THE_CONTAINING_GENERATOR_WILL_ALWAYS_SEND_0,
                    )
                } else {
                    None
                };
                if let Some(diagnostic) = diagnostic {
                    self.check_type_assignable_to(sent_type, next_type, error_node, Some(diagnostic));
                }
            }
            if iteration_types.yield_type.is_some() || iterable_exists {
                let Some(yield_type) = iteration_types.yield_type else {
                    return None;
                };
                if possible_out_of_bounds {
                    return Some(self.include_undefined_in_index_signature_stub(yield_type));
                }
                return Some(yield_type);
            }
        }
        let mut array_type = input_type;
        let mut has_string_constituent = false;
        // If strings are permitted, remove any string-like constituents from
        // the array type.
        if use_.intersects(IterationUse::ALLOWS_STRING_INPUT_FLAG) {
            if self.type_(input_type).flags.intersects(TypeFlags::UNION) {
                let array_types = self.type_(input_type).as_union_type().expect("UnionType data").types.clone();
                let filtered_types: Vec<TypeId> = array_types
                    .iter()
                    .copied()
                    .filter(|&t| !self.type_(t).flags.intersects(TypeFlags::STRING_LIKE))
                    .collect();
                if filtered_types.len() != array_types.len() {
                    array_type = self.get_union_type_ex(&filtered_types, crate::todo_shapes::UnionReduction::Subtype);
                }
            } else if self.type_(array_type).flags.intersects(TypeFlags::STRING_LIKE) {
                array_type = self.never_type();
            }
            has_string_constituent = array_type != input_type;
            if has_string_constituent {
                // Now that we've removed all the StringLike types, if no
                // constituents remain, then the entire arrayOrStringType was
                // a string.
                if self.type_(array_type).flags.intersects(TypeFlags::NEVER) {
                    if possible_out_of_bounds {
                        return Some(
                            self.include_undefined_in_index_signature_stub(self.string_type()),
                        );
                    }
                    return Some(self.string_type());
                }
            }
        }
        if !self.is_array_like_type(array_type) {
            if let Some(error_node) = error_node {
                // Which error we report depends on whether we allow strings
                // or if there was a string constituent.
                let allows_strings =
                    use_.intersects(IterationUse::ALLOWS_STRING_INPUT_FLAG) && !has_string_constituent;
                let (default_diagnostic, maybe_missing_await) =
                    self.get_iteration_diagnostic_details(use_, input_type, allows_strings);
                let suggest = maybe_missing_await
                    && self.get_awaited_type_of_promise_stub(array_type).is_some();
                let array_text = self.type_to_string(array_type);
                self.error_and_maybe_suggest_await(
                    error_node,
                    suggest,
                    default_diagnostic,
                    &[array_text],
                );
            }
            if has_string_constituent {
                if possible_out_of_bounds {
                    return Some(self.include_undefined_in_index_signature_stub(self.string_type()));
                }
                return Some(self.string_type());
            }
            return None;
        }
        let array_element_type =
            self.get_index_type_of_type(array_type, self.number_type());
        if has_string_constituent && array_element_type.is_some() {
            let array_element_type = array_element_type.expect("checked");
            // This is just an optimization for the case where
            // arrayOrStringType is string | string[]
            if self.type_(array_element_type).flags.intersects(TypeFlags::STRING_LIKE)
                && self.compiler_options.no_unchecked_indexed_access != Tristate::TSTrue
            {
                return Some(self.string_type());
            }
            if possible_out_of_bounds {
                return Some(self.get_union_type_ex(
                    &[array_element_type, self.string_type(), self.undefined_type()],
                    crate::todo_shapes::UnionReduction::Subtype,
                ));
            }
            return Some(self.get_union_type_ex(
                &[array_element_type, self.string_type()],
                crate::todo_shapes::UnionReduction::Subtype,
            ));
        }
        if use_.intersects(IterationUse::POSSIBLY_OUT_OF_BOUNDS) {
            return Some(self.include_undefined_in_index_signature_stub(
                array_element_type.expect("array element type"),
            ));
        }
        array_element_type
    }

    /// Go: `func (c *Checker) getIterationTypeOfGeneratorFunctionReturnType(typeKind, returnType, isAsyncGenerator) *Type`
    /// (checker.go:6412).
    pub(crate) fn get_iteration_type_of_generator_function_return_type(
        &mut self,
        type_kind: IterationTypeKind,
        return_type: TypeId,
        is_async_generator: bool,
    ) -> Option<TypeId> {
        if is_type_any(self, return_type) {
            return None;
        }
        let iteration_types = self
            .get_iteration_types_of_generator_function_return_type(return_type, is_async_generator);
        iteration_types.get_type(type_kind)
    }

    /// Go: `func (c *Checker) getIterationTypesOfGeneratorFunctionReturnType(t, isAsyncGenerator) IterationTypes`
    /// (checker.go:6418).
    pub(crate) fn get_iteration_types_of_generator_function_return_type(
        &mut self,
        t: TypeId,
        is_async_generator: bool,
    ) -> IterationTypes {
        if is_type_any(self, t) {
            let (any_type, any_type2, any_type3) =
                (self.any_type(), self.any_type(), self.any_type());
            return IterationTypes {
                yield_type: Some(any_type),
                return_type: Some(any_type2),
                next_type: Some(any_type3),
            };
        }
        let use_ = if is_async_generator {
            IterationUse::ASYNC_GENERATOR_RETURN_TYPE
        } else {
            IterationUse::GENERATOR_RETURN_TYPE
        };
        let resolver = if is_async_generator {
            self.async_iteration_types_resolver()
        } else {
            self.sync_iteration_types_resolver()
        };
        let result = self.get_iteration_types_of_iterable(t, use_, None);
        if result.has_types() {
            return result;
        }
        self.get_iteration_types_of_iterator(t, resolver, None, None)
    }

    /// Go: `func (c *Checker) getIterationTypeOfIterable(use, typeKind, inputType, errorNode) *Type`
    /// (checker.go:6432).
    pub(crate) fn get_iteration_type_of_iterable(
        &mut self,
        use_: IterationUse,
        type_kind: IterationTypeKind,
        input_type: TypeId,
        error_node: Option<NodeId>,
    ) -> Option<TypeId> {
        if is_type_any(self, input_type) {
            return None;
        }
        let iteration_types = self.get_iteration_types_of_iterable(input_type, use_, error_node);
        iteration_types.get_type(type_kind)
    }

    /// Go: `func (c *Checker) getIterationTypesOfIterable(t, use, errorNode) IterationTypes`
    /// (checker.go:6471).
    pub(crate) fn get_iteration_types_of_iterable(
        &mut self,
        t: TypeId,
        use_: IterationUse,
        error_node: Option<NodeId>,
    ) -> IterationTypes {
        let t = self.get_reduced_type(t);
        if is_type_any(self, t) {
            let (any_type, any_type2, any_type3) =
                (self.any_type(), self.any_type(), self.any_type());
            return IterationTypes {
                yield_type: Some(any_type),
                return_type: Some(any_type2),
                next_type: Some(any_type3),
            };
        }
        let key = IterationTypesKey {
            type_id: t,
            use_: use_ & IterationUse::CACHE_FLAGS,
        };
        // If we are reporting errors and encounter a cached
        // `noIterationTypes`, we should ignore the cached value and continue
        // as if nothing was cached.
        let mut no_cache = false;
        if let Some(&cached) = self.iteration_types_cache.get(&key) {
            if error_node.is_none() || cached.has_types() {
                return cached;
            }
            no_cache = true;
        }
        let result = self.get_iteration_types_of_iterable_worker(t, use_, error_node, no_cache);
        if !no_cache {
            self.iteration_types_cache.insert(key, result);
        }
        result
    }

    /// Go: `func (c *Checker) getIterationTypesOfIterableWorker(t, use, errorNode, noCache) IterationTypes`
    /// (checker.go:6497).
    pub(crate) fn get_iteration_types_of_iterable_worker(
        &mut self,
        t: TypeId,
        use_: IterationUse,
        error_node: Option<NodeId>,
        no_cache: bool,
    ) -> IterationTypes {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            let types = self.type_(t).as_union_type().expect("UnionType data").types.clone();
            let mut all_iteration_types: Vec<IterationTypes> =
                Vec::with_capacity(types.len());
            for constituent in types {
                let iteration_types =
                    self.get_iteration_types_of_iterable_worker(constituent, use_, None, no_cache);
                if !iteration_types.has_types() {
                    if let Some(error_node) = error_node {
                        // Go defers the diagnostic (TypeToString may resolve
                        // symbols already being resolved) — the deferred
                        // callback queue is spine.rs's.
                        let allow_async =
                            use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG);
                        self.add_deferred_diagnostic(
                            crate::spine::DeferredDiagnosticCallback::ReportTypeNotIterable {
                                error_node,
                                t,
                                allows_async_iterables: allow_async,
                                related: Vec::new(),
                            },
                        );
                    }
                    return IterationTypes::default();
                }
                all_iteration_types.push(iteration_types);
            }
            return self.combine_iteration_types(&all_iteration_types);
        }
        let async_resolver = self.async_iteration_types_resolver();
        let sync_resolver = self.sync_iteration_types_resolver();
        if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
            let iteration_types = self.get_iteration_types_of_iterable_fast(t, async_resolver);
            if iteration_types.has_types() {
                if use_.intersects(IterationUse::FOR_OF_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
            let iteration_types =
                self.get_iteration_types_of_iterable_slow(t, async_resolver, error_node);
            if iteration_types.has_types() {
                return iteration_types;
            }
        }
        if use_.intersects(IterationUse::ALLOWS_SYNC_ITERABLES_FLAG) {
            let iteration_types = self.get_iteration_types_of_iterable_fast(t, sync_resolver);
            if iteration_types.has_types() {
                if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
            let iteration_types =
                self.get_iteration_types_of_iterable_slow(t, sync_resolver, error_node);
            if iteration_types.has_types() {
                if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
        }
        if let Some(error_node) = error_node {
            // We defer the diagnostic because TypeToString may attempt to
            // resolve symbols that are already being resolved.
            let allow_async = use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG);
            self.add_deferred_diagnostic(
                crate::spine::DeferredDiagnosticCallback::ReportTypeNotIterable {
                    error_node,
                    t,
                    allows_async_iterables: allow_async,
                    related: Vec::new(),
                },
            );
        }
        IterationTypes::default()
    }

    /// Go: `func (c *Checker) getIterationTypesOfIterableFast(t, r) IterationTypes`
    /// (checker.go:6554).
    pub(crate) fn get_iteration_types_of_iterable_fast(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
    ) -> IterationTypes {
        // As an optimization, if the type is an instantiation of the global
        // Iterable/IteratorObject/IterableIterator/Generator types, just
        // grab the related type arguments.
        if self.is_reference_to_type(t, self.get_global_iterable_type_checked(r))
            || self.is_reference_to_type(t, self.get_global_iterator_object_type(r))
            || self.is_reference_to_type(t, self.get_global_iterable_iterator_type(r))
            || self.is_reference_to_type(t, self.get_global_generator_type(r))
        {
            let type_arguments = relater::get_type_arguments(self, t);
            if type_arguments.len() >= 3 {
                return self.get_resolved_iteration_types(
                    r,
                    Some(type_arguments[0]),
                    Some(type_arguments[1]),
                    Some(type_arguments[2]),
                );
            }
            return IterationTypes::default();
        }
        // The builtin iterator list (ArrayIterator etc.) — the globals
        // table is permissive-empty until S2/S5's getGlobalType seam, so
        // this arm matches no type (the empty vec contains nothing).
        if self.is_reference_to_some_type(t, self.get_global_builtin_iterator_types(r)) {
            let type_arguments = relater::get_type_arguments(self, t);
            if !type_arguments.is_empty() {
                let builtin_return = self.get_builtin_iterator_return_type();
                let unknown_type = self.unknown_type();
                return self.get_resolved_iteration_types(
                    r,
                    Some(type_arguments[0]),
                    Some(builtin_return),
                    Some(unknown_type),
                );
            }
        }
        IterationTypes::default()
    }

    /// Go: `func (r *IterationTypesResolver) getResolvedIterationTypes(yieldType, returnType, nextType)`
    /// (checker.go:6594).
    pub(crate) fn get_resolved_iteration_types(
        &mut self,
        r: IterationTypesResolver,
        yield_type: Option<TypeId>,
        return_type: Option<TypeId>,
        next_type: Option<TypeId>,
    ) -> IterationTypes {
        IterationTypes {
            yield_type: match (yield_type, self.resolve_iteration_type(r, yield_type, None)) {
                (_, Some(resolved)) => Some(resolved),
                (Some(t), None) => Some(t),
                (None, None) => None,
            },
            return_type: match (return_type, self.resolve_iteration_type(r, return_type, None)) {
                (_, Some(resolved)) => Some(resolved),
                (Some(t), None) => Some(t),
                (None, None) => None,
            },
            next_type,
        }
    }

    /// Go: `func (c *Checker) isReferenceToType(t, target) bool` (checker.go:6601).
    pub(crate) fn is_reference_to_type(&mut self, t: TypeId, target: TypeId) -> bool {
        let td = self.type_(t);
        td.object_flags.intersects(ObjectFlags::REFERENCE)
            && matches!(&td.data, crate::types::TypeData::TypeReference(d) if d.target == Some(target))
    }

    /// Go: `func (c *Checker) isReferenceToSomeType(t, targets) bool` (checker.go:6605).
    pub(crate) fn is_reference_to_some_type(&mut self, t: TypeId, targets: Vec<TypeId>) -> bool {
        let td = self.type_(t);
        if !td.object_flags.intersects(ObjectFlags::REFERENCE) {
            return false;
        }
        matches!(&td.data, crate::types::TypeData::TypeReference(d) if d.target.is_some_and(|target| targets.contains(&target)))
    }

    /// Go: `func (c *Checker) combineIterationTypes(iterationTypes) IterationTypes`
    /// (checker.go:6645).
    pub(crate) fn combine_iteration_types(
        &mut self,
        iteration_types: &[IterationTypes],
    ) -> IterationTypes {
        let yield_type =
            self.get_iteration_type_union(iteration_types, |t| t.yield_type);
        let return_type =
            self.get_iteration_type_union(iteration_types, |t| t.return_type);
        let next_type = self.get_iteration_type_union(iteration_types, |t| t.next_type);
        IterationTypes {
            yield_type,
            return_type,
            next_type,
        }
    }

    /// Go: `func (c *Checker) getIterationTypeUnion(iterationTypes, f) *Type`
    /// (checker.go:6656).
    pub(crate) fn get_iteration_type_union(
        &mut self,
        iteration_types: &[IterationTypes],
        f: impl Fn(&IterationTypes) -> Option<TypeId>,
    ) -> Option<TypeId> {
        let types: Vec<TypeId> = iteration_types.iter().filter_map(f).collect();
        if types.is_empty() {
            return None;
        }
        Some(relater::get_union_type(self, types))
    }

    /// Go: `func (c *Checker) getAsyncFromSyncIterationTypes(iterationTypes, errorNode) IterationTypes`
    /// (checker.go:6663).
    pub(crate) fn get_async_from_sync_iteration_types(
        &mut self,
        iteration_types: IterationTypes,
        error_node: Option<NodeId>,
    ) -> IterationTypes {
        let any_type = self.any_type();
        if !iteration_types.has_types()
            || (iteration_types.yield_type == Some(any_type)
                && iteration_types.return_type == Some(any_type)
                && iteration_types.next_type == Some(any_type))
        {
            return iteration_types;
        }
        // if we're requesting diagnostics, report errors for a missing
        // `Awaited<T>` — TODO(port M6 S9): getGlobalAwaitedSymbol.
        let yield_type = iteration_types
            .yield_type
            .map(|t| self.get_awaited_type(t))
            .or(Some(self.any_type()));
        let return_type = iteration_types
            .return_type
            .map(|t| self.get_awaited_type(t))
            .or(Some(self.any_type()));
        IterationTypes {
            yield_type,
            return_type,
            next_type: iteration_types.next_type,
        }
    }

    /// Go: `func (c *Checker) getIterationTypesOfIterableSlow(t, r, errorNode) IterationTypes`
    /// (checker.go:6687).
    pub(crate) fn get_iteration_types_of_iterable_slow(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
        error_node: Option<NodeId>,
    ) -> IterationTypes {
        let iterator_name = self.resolver_iterator_symbol_name(r);
        let method = self.get_property_of_type(t, iterator_name);
        if let Some(method) = method {
            if !self.program.symbol(method).flags.intersects(SymbolFlags::OPTIONAL) {
                let method_type = self.get_type_of_symbol(method);
                if is_type_any(self, method_type) {
                    let (any_type, any_type2, any_type3) =
                        (self.any_type(), self.any_type(), self.any_type());
                    return IterationTypes {
                        yield_type: Some(any_type),
                        return_type: Some(any_type2),
                        next_type: Some(any_type3),
                    };
                }
                let all_signatures = relater::get_signatures_of_type(self, method_type, SignatureKind::Call);
                let valid_signatures: Vec<SignatureIdRef> = all_signatures
                    .iter()
                    .copied()
                    .filter(|&sig| self.get_min_argument_count(sig) == 0)
                    .collect();
                if !valid_signatures.is_empty() {
                    let return_types: Vec<TypeId> = valid_signatures
                        .iter()
                        .map(|&sig| self.get_return_type_of_signature(sig))
                        .collect();
                    let iterator_type = relater::get_intersection_type(self, return_types);
                    return self.get_iteration_types_of_iterator_worker(iterator_type, r, error_node, None);
                }
                // Go reports the iterable-assignability diagnostic here when
                // errorNode != nil && len(allSignatures) != 0 — the
                // checkTypeAssignableToEx diagnostic output path; with the
                // S8 member seam permissive this is unreachable at this depth.
            }
        }
        IterationTypes::default()
    }

    /// Go: `func (c *Checker) getIterationTypesOfIterator(t, r, errorNode, diagnosticOutput) IterationTypes`
    /// (checker.go:6724).
    pub(crate) fn get_iteration_types_of_iterator(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
        error_node: Option<NodeId>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> IterationTypes {
        self.get_iteration_types_of_iterator_worker(t, r, error_node, diagnostic_output)
    }

    /// Go: `func (c *Checker) getIterationTypesOfIteratorWorker(...) IterationTypes`
    /// (checker.go:6741).
    pub(crate) fn get_iteration_types_of_iterator_worker(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
        error_node: Option<NodeId>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> IterationTypes {
        if is_type_any(self, t) {
            let (any_type, any_type2, any_type3) =
                (self.any_type(), self.any_type(), self.any_type());
            return IterationTypes {
                yield_type: Some(any_type),
                return_type: Some(any_type2),
                next_type: Some(any_type3),
            };
        }
        let iteration_types = self.get_iteration_types_of_iterator_fast(t, r);
        if iteration_types.has_types() {
            return iteration_types;
        }
        self.get_iteration_types_of_iterator_slow(t, r, error_node, diagnostic_output)
    }

    /// Go: `func (c *Checker) getIterationTypesOfIteratorFast(t, r) IterationTypes`
    /// (checker.go:6756).
    pub(crate) fn get_iteration_types_of_iterator_fast(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
    ) -> IterationTypes {
        if self.is_reference_to_type(t, self.get_global_iterator_type(r))
            || self.is_reference_to_type(t, self.get_global_iterator_object_type(r))
            || self.is_reference_to_type(t, self.get_global_iterable_iterator_type(r))
            || self.is_reference_to_type(t, self.get_global_generator_type(r))
        {
            let type_arguments = relater::get_type_arguments(self, t);
            if type_arguments.len() >= 3 {
                return self.get_resolved_iteration_types(
                    r,
                    Some(type_arguments[0]),
                    Some(type_arguments[1]),
                    Some(type_arguments[2]),
                );
            }
            return IterationTypes::default();
        }
        if self.is_reference_to_some_type(t, self.get_global_builtin_iterator_types(r)) {
            let type_arguments = relater::get_type_arguments(self, t);
            if !type_arguments.is_empty() {
                let builtin_return = self.get_builtin_iterator_return_type();
                let unknown_type = self.unknown_type();
                return self.get_resolved_iteration_types(
                    r,
                    Some(type_arguments[0]),
                    Some(builtin_return),
                    Some(unknown_type),
                );
            }
        }
        IterationTypes::default()
    }

    /// Go: `func (c *Checker) getIterationTypesOfIteratorSlow(t, r, errorNode, diagnosticOutput) IterationTypes`
    /// (checker.go:6788).
    pub(crate) fn get_iteration_types_of_iterator_slow(
        &mut self,
        t: TypeId,
        r: IterationTypesResolver,
        error_node: Option<NodeId>,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> IterationTypes {
        let next = self.get_iteration_types_of_method(
            t,
            r,
            "next",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        let ret = self.get_iteration_types_of_method(
            t,
            r,
            "return",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        let throw = self.get_iteration_types_of_method(
            t,
            r,
            "throw",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        self.combine_iteration_types(&[next, ret, throw])
    }

    /// Go: `func (c *Checker) getIterationTypesOfMethod(t, resolver, methodName, errorNode, diagnosticOutput) IterationTypes`
    /// (checker.go:6795 — the ~108-line iteration-method walk).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn get_iteration_types_of_method(
        &mut self,
        t: TypeId,
        resolver: IterationTypesResolver,
        method_name: &str,
        error_node: Option<NodeId>,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> IterationTypes {
        let method = self.get_property_of_type(t, method_name);
        // Ignore 'return' or 'throw' if they are missing.
        if method.is_none() && method_name != "next" {
            return IterationTypes::default();
        }
        let mut method_type: Option<TypeId> = None;
        if let Some(method) = method {
            if !(method_name == "next"
                && self.program.symbol(method).flags.intersects(SymbolFlags::OPTIONAL))
            {
                if method_name == "next" {
                    method_type = Some(self.get_type_of_symbol(method));
                } else {
                    // Go: getTypeWithFacts(getTypeOfSymbol(method), NEUndefinedOrNull)
                    let mt = self.get_type_of_symbol(method);
                    // Go: TypeFactsNEUndefinedOrNull = 1 << 21 (checker.go:424).
                    method_type = Some(relater::get_type_with_facts(
                        self,
                        mt,
                        TypeFacts::from_bits(1 << 21),
                    ));
                }
            }
        }
        if method_type.is_some_and(|mt| is_type_any(self, mt)) {
            let (any_type, any_type2, any_type3) =
                (self.any_type(), self.any_type(), self.any_type());
            return IterationTypes {
                yield_type: Some(any_type),
                return_type: Some(any_type2),
                next_type: Some(any_type3),
            };
        }
        // Both async and non-async iterators *must* have a `next` method.
        let mut method_signatures: Vec<SignatureIdRef> = Vec::new();
        if let Some(method_type) = method_type {
            method_signatures =
                relater::get_signatures_of_type(self, method_type, SignatureKind::Call);
        }
        if method_signatures.is_empty() {
            if let Some(error_node) = error_node {
                let diagnostic = if method_name == "next" {
                    self.resolver_must_have_a_next_method_diagnostic(resolver)
                } else {
                    self.resolver_must_be_a_method_diagnostic(resolver)
                };
                let d = u::new_diagnostic_for_node(
                    self.store(),
                    error_node,
                    diagnostic,
                    vec![method_name.to_string()],
                );
                report_diagnostic(self, d, diagnostic_output.as_deref_mut());
            }
            return IterationTypes::default();
        }
        // If the method signature comes exclusively from the global iterator
        // or generator type, create iteration types from its type arguments
        // like getIterationTypesOfIteratorFast does — TODO(port M6 S2/S5):
        // the global-type members seam (permissive skip until getGlobalType
        // carries the lib globals).
        let _ = (resolver,);
        // Extract the first parameter and return type of each signature.
        let mut method_parameter_types: Vec<TypeId> = Vec::new();
        let mut method_return_types: Vec<TypeId> = Vec::new();
        for signature in &method_signatures {
            if method_name != "throw" {
                let params = &self.signatures[signature.0 as usize].parameters;
                if !params.is_empty() {
                    method_parameter_types.push(self.get_type_at_position(*signature, 0));
                }
            }
            method_return_types.push(self.get_return_type_of_signature(*signature));
        }
        // Resolve the *next* or *return* type from the first parameter of a
        // `next()` or `return()` method, respectively.
        let mut return_types: Vec<TypeId> = Vec::new();
        let mut next_type: Option<TypeId> = None;
        if method_name != "throw" {
            let method_parameter_type = if !method_parameter_types.is_empty() {
                Some(relater::get_union_type(self, method_parameter_types.clone()))
            } else {
                Some(self.unknown_type())
            };
            if method_name == "next" {
                // The value of `next(value)` is *not* awaited by async generators
                next_type = method_parameter_type;
            } else if method_name == "return" {
                // The value of `return(value)` *is* awaited by async generators
                let resolved = self
                    .resolve_iteration_type(resolver, method_parameter_type, error_node)
                    .unwrap_or_else(|| self.any_type());
                return_types.push(resolved);
            }
        }
        // Resolve the *yield* and *return* types from the return type of the
        // method (i.e. `IteratorResult`)
        let yield_type: Option<TypeId>;
        let method_return_type = if !method_return_types.is_empty() {
            relater::get_intersection_type(self, method_return_types)
        } else {
            self.never_type()
        };
        let resolved_method_return_type = self
            .resolve_iteration_type(resolver, Some(method_return_type), error_node)
            .unwrap_or_else(|| self.any_type());
        let iteration_types = self.get_iteration_types_of_iterator_result(resolved_method_return_type);
        if !iteration_types.has_types() {
            if let Some(error_node) = error_node {
                let diagnostic = self.resolver_must_have_a_value_diagnostic(resolver);
                let d = u::new_diagnostic_for_node(
                    self.store(),
                    error_node,
                    diagnostic,
                    vec![method_name.to_string()],
                );
                report_diagnostic(self, d, diagnostic_output.as_deref_mut());
            }
            yield_type = Some(self.any_type());
            return_types.push(self.any_type());
        } else {
            yield_type = iteration_types.yield_type;
            if let Some(rt) = iteration_types.return_type {
                return_types.push(rt);
            }
        }
        let return_union = relater::get_union_type(self, return_types);
        IterationTypes {
            yield_type,
            return_type: Some(return_union),
            next_type,
        }
    }

    /// Go: `func (c *Checker) getIterationTypesOfIteratorResult(t) IterationTypes`
    /// (checker.go:6884).
    pub(crate) fn get_iteration_types_of_iterator_result(&mut self, t: TypeId) -> IterationTypes {
        if is_type_any(self, t) {
            let (any_type, any_type2, any_type3) =
                (self.any_type(), self.any_type(), self.any_type());
            return IterationTypes {
                yield_type: Some(any_type),
                return_type: Some(any_type2),
                next_type: Some(any_type3),
            };
        }
        // The global IteratorYieldResult/IteratorReturnResult optimization is
        // TODO(port M6 S2/S5): the global-type seam; the member-walk below
        // carries the semantics.
        // Choose any constituents that can produce the requested iteration type.
        let yield_iterator_result =
            filter_type_local(self, t, |c, t| c.is_yield_iterator_result(t));
        let yield_type = if yield_iterator_result != self.never_type() {
            self.get_type_of_property_of_type(yield_iterator_result, "value")
        } else {
            None
        };
        let return_iterator_result =
            filter_type_local(self, t, |c, t| c.is_return_iterator_result(t));
        let return_type = if return_iterator_result != self.never_type() {
            self.get_type_of_property_of_type(return_iterator_result, "value")
        } else {
            None
        };
        if yield_type.is_none() && return_type.is_none() {
            return IterationTypes::default();
        }
        // From https://tc39.github.io/ecma262/#sec-iteratorresult-interface
        // > ... If the iterator does not have a return value, `value` is
        // > `undefined`. In that case, the `value` property may be absent from
        // > the conforming object if it does not inherit an explicit `value`
        // > property.
        IterationTypes {
            yield_type,
            return_type: Some(return_type.unwrap_or_else(|| self.void_type())),
            next_type: None,
        }
    }

    /// Go: `func (c *Checker) isYieldIteratorResult(t) bool` (checker.go:6917).
    pub(crate) fn is_yield_iterator_result(&mut self, t: TypeId) -> bool {
        self.is_iterator_result(t, IterationTypeKind::Yield)
    }

    /// Go: `func (c *Checker) isReturnIteratorResult(t) bool` (checker.go:6921).
    pub(crate) fn is_return_iterator_result(&mut self, t: TypeId) -> bool {
        self.is_iterator_result(t, IterationTypeKind::Return)
    }

    /// Go: `func (c *Checker) isIteratorResult(t, kind) bool` (checker.go:6925).
    pub(crate) fn is_iterator_result(&mut self, t: TypeId, kind: IterationTypeKind) -> bool {
        // From https://tc39.github.io/ecma262/#sec-iteratorresult-interface:
        // > [done] is the result status of an iterator `next` method call.
        // If the end of the iterator was reached `done` is `true`. If the
        // end was not reached `done` is `false` and a value is available.
        // > If a `done` property (either own or inherited) does not exist,
        // it is consider to have the value `false`.
        let done_type = self
            .get_type_of_property_of_type(t, "done")
            .unwrap_or_else(|| self.false_type());
        let expected = if kind == IterationTypeKind::Yield {
            self.false_type()
        } else {
            self.true_type()
        };
        self.is_type_assignable_to(expected, done_type)
    }

    /// Go: `func (c *Checker) reportTypeNotIterableError(errorNode, t, allowAsyncIterables) *ast.Diagnostic`
    /// (checker.go:6934).
    pub(crate) fn report_type_not_iterable_error(
        &mut self,
        error_node: NodeId,
        t: TypeId,
        allow_async_iterables: bool,
    ) -> tsc_ast::Diagnostic {
        let message = if allow_async_iterables {
            &diag::TYPE_0_MUST_HAVE_A_SYMBOL_ASYNCITERATOR_METHOD_THAT_RETURNS_AN_ASYNC_ITERATOR
        } else {
            &diag::TYPE_0_MUST_HAVE_A_SYMBOL_ITERATOR_METHOD_THAT_RETURNS_AN_ITERATOR
        };
        // Go: suggestAwait = getAwaitedTypeOfPromise(t) != nil || (!async &&
        // for-of expression parent && globalAsyncIterable exists && t
        // assignable to AsyncIterable<any,any,any>) — the async-iterable
        // probe needs the S8/S9 member+awaited seams; permissive false.
        let suggest_await = self.get_awaited_type_of_promise_stub(t).is_some();
        let type_text = self.type_to_string(t);
        self.error_and_maybe_suggest_await(error_node, suggest_await, message, &[type_text])
    }

    /// Go: `func (c *Checker) getIterationDiagnosticDetails(use, inputType, allowsStrings) (*diagnostics.Message, bool)`
    /// (checker.go:6965).
    pub(crate) fn get_iteration_diagnostic_details(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        allows_strings: bool,
    ) -> (&'static Message, bool) {
        let yield_type =
            self.get_iteration_type_of_iterable(use_, IterationTypeKind::Yield, input_type, None);
        if yield_type.is_some() {
            return (
                &diag::TYPE_0_CAN_ONLY_BE_ITERATED_THROUGH_WHEN_USING_THE_DOWNLEVELITERATION_FLAG_OR_WITH_A_TARGET_OF_ES2015_OR_HIGHER,
                false,
            );
        }
        let symbol_name = self
            .type_(input_type)
            .symbol
            .map(|s| self.program.symbol(s).name.clone());
        if symbol_name.as_deref().is_some_and(is_es2015_or_later_iterable) {
            return (
                &diag::TYPE_0_CAN_ONLY_BE_ITERATED_THROUGH_WHEN_USING_THE_DOWNLEVELITERATION_FLAG_OR_WITH_A_TARGET_OF_ES2015_OR_HIGHER,
                true,
            );
        }
        if allows_strings {
            return (&diag::TYPE_0_IS_NOT_AN_ARRAY_TYPE_OR_A_STRING_TYPE, true);
        }
        (&diag::TYPE_0_IS_NOT_AN_ARRAY_TYPE, true)
    }

    /// Go: `func (c *Checker) checkRightHandSideOfForOf(statement) *Type`
    /// (checker.go:18081).
    pub(crate) fn check_right_hand_side_of_for_of_real(&mut self, statement: NodeId) -> Option<TypeId> {
        let data = self
            .store()
            .node(statement)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement data");
        let use_ = if data.await_modifier.is_some() {
            IterationUse::FOR_AWAIT_OF
        } else {
            IterationUse::FOR_OF
        };
        // Go: checkNonNullExpression (S4's non-null reporting is permissive —
        // checkExpression passes the type through).
        let expression = data.expression;
        let expr_type = self.check_expression(expression);
        let undefined_type = self.undefined_type();
        Some(self.check_iterated_type_or_element_type(
            use_,
            expr_type,
            undefined_type,
            Some(expression),
        ))
    }

    /// Go: `func (c *Checker) isNullOrUndefined(node) bool` (checker.go:18070).
    pub(crate) fn is_null_or_undefined_real(&mut self, node: NodeId) -> bool {
        let expr = tsc_ast::utilities::skip_parentheses(self.store(), node);
        match self.store().node(expr).kind {
            Kind::NullKeyword => true,
            Kind::Identifier => {
                self.get_resolved_symbol(expr) == self.undefined_symbol
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) includeUndefinedInIndexSignature(t)` (S10's
    /// index-signature family). Permissive identity.
    pub(crate) fn include_undefined_in_index_signature_stub(&mut self, t: TypeId) -> TypeId {
        t
    }
}

/// Go's `c.filterType(t, f)` over a self-borrowing predicate (the relater
/// free fn takes a non-capturing closure; the iteration family's predicates
/// need the checker — same reduction semantics: union → filter constituents
/// + getUnionType, else the predicate or neverType).
fn filter_type_local(c: &mut Checker, t: TypeId, f: impl FnMut(&mut Checker, TypeId) -> bool) -> TypeId {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let types = c
            .type_(t)
            .as_union_type()
            .expect("UnionType data")
            .types
            .clone();
        let filtered: Vec<TypeId> = types.into_iter().filter(|&x| f(c, x)).collect();
        relater::get_union_type(c, filtered)
    } else if f(c, t) {
        t
    } else {
        c.never_type()
    }
}

/// Go: `func IsFunctionLike(node *Node) bool` (ast) — the function-like kinds.
pub(crate) fn is_function_like(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::Constructor
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionType
            | Kind::ConstructorType
            | Kind::CallSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature
            | Kind::FunctionKeyword
            | Kind::ClassStaticBlockDeclaration
    )
}

/// Go: `node.Body()` for the function-like kinds (the return-aggregation
/// family reads the body's return statements).
pub(crate) fn function_body_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::Constructor => n.as_constructor_declaration().expect("data").body,
        Kind::FunctionDeclaration => n.as_function_declaration().expect("data").body,
        Kind::FunctionExpression => n.as_function_expression().expect("data").body,
        Kind::ArrowFunction => n.as_arrow_function().expect("data").body,
        Kind::MethodDeclaration | Kind::MethodSignature => {
            n.as_method_declaration().expect("data").body
        }
        Kind::GetAccessor => n.as_get_accessor_declaration().expect("data").body,
        Kind::SetAccessor => n.as_set_accessor_declaration().expect("data").body,
        _ => None,
    }
}

/// Go: `func isES2015OrLaterIterable(n string) bool` (checker.go:6929).
fn is_es2015_or_later_iterable(n: &str) -> bool {
    matches!(
        n,
        "Float32Array"
            | "Float64Array"
            | "Int16Array"
            | "Int32Array"
            | "Int8Array"
            | "NodeList"
            | "Uint16Array"
            | "Uint32Array"
            | "Uint8Array"
            | "Uint8ClampedArray"
    )
}

/// Go: `func IsTypeAny(t *Type) bool` (utilities.go).
pub(crate) fn is_type_any(c: &Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::ANY)
}

/// Go: `func (c *Checker) reportDiagnostic(diagnostic, diagnosticOutput)` —
/// the diagnostic-or-collect fork (relater.rs's private pair, carried here
/// for the iteration family's `diagnosticOutput` plumbing).
fn report_diagnostic(
    c: &mut Checker,
    diagnostic: tsc_ast::Diagnostic,
    diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
) {
    match diagnostic_output {
        Some(out) => out.push(diagnostic),
        None => {
            c.add_diagnostic(diagnostic);
        }
    }
}

/// Signature ids as a local alias (crate::types::SignatureId).
type SignatureIdRef = crate::types::SignatureId;

// ────────────────────────────────────────────────────────────────────────────
// The return aggregation + unwrapReturnType family (Go checker.go:20663-20989
// subset S3 owns + 3767-3799 + 20462)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkAndAggregateReturnExpressionTypes(fn, checkMode) ([]*Type, bool)`
    /// (checker.go:20663).
    pub(crate) fn check_and_aggregate_return_expression_types(
        &mut self,
        fn_node: NodeId,
        check_mode: CheckMode,
    ) -> (Option<Vec<TypeId>>, bool) {
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        let mut aggregated_types: Vec<TypeId> = Vec::new();
        let mut has_return_with_no_expression = self.function_has_implicit_return(fn_node);
        let mut has_return_of_type_never = false;
        let body = function_body_of(self.store(), fn_node);
        let fn_symbol = self
            .store()
            .node(fn_node)
            .symbol()
            .and_then(|s| s);
        let check_mode_no_skip = check_mode & !CheckMode::SKIP_GENERIC_FUNCTIONS;
        for_each_return_statement(self, body, &mut |c, return_statement| {
            let Some(mut expr) = c
                .store()
                .node(return_statement)
                .as_return_statement()
                .expect("ReturnStatement data")
                .expression
            else {
                has_return_with_no_expression = true;
                return false;
            };
            expr = tsc_ast::utilities::skip_parentheses(c.store(), expr);
            // Bare calls to this same function don't contribute to inference
            // and `return await` is also safe to unwrap here
            if function_flags.intersects(FunctionFlags::ASYNC)
                && c.store().node(expr).kind == Kind::AwaitExpression
            {
                expr = tsc_ast::utilities::skip_parentheses(
                    c.store(),
                    c.store()
                        .node(expr)
                        .as_await_expression()
                        .expect("AwaitExpression data")
                        .expression,
                );
            }
            if c.store().node(expr).kind == Kind::CallExpression {
                let callee = c
                    .store()
                    .node(expr)
                    .as_call_expression()
                    .expect("CallExpression data")
                    .expression;
                if is_identifier(c.store().node(callee)) {
                    let callee_type = c.check_expression_cached(callee);
                    let callee_symbol = c.type_(callee_type).symbol;
                    if callee_symbol.is_some_and(|s| fn_symbol.is_some_and(|f| s == f)) {
                        has_return_of_type_never = true;
                        return false;
                    }
                }
            }
            let mut t = c.check_expression_cached_ex(expr, check_mode_no_skip);
            if function_flags.intersects(FunctionFlags::ASYNC) {
                // From within an async function you can return either a
                // non-promise value or a promise. Any Promise/A+ compatible
                // implementation will always assimilate any foreign promise,
                // so the return type of the body should be unwrapped to its
                // awaited type, which should be wrapped in the native
                // Promise<T> type by the caller.
                t = c.unwrap_awaited_type(t);
            }
            if c.type_(t).flags.intersects(TypeFlags::NEVER) {
                has_return_of_type_never = true;
            }
            // Go: isConstContext (S4) — permissive false (const literal
            // widening rides through).
            if !aggregated_types.contains(&t) {
                aggregated_types.push(t);
            }
            false
        });
        if aggregated_types.is_empty()
            && !has_return_with_no_expression
            && (has_return_of_type_never || may_return_never(self.store(), fn_node))
        {
            return (None, true);
        }
        if self.strict_null_checks && !aggregated_types.is_empty() && has_return_with_no_expression {
            let undefined_type = self.undefined_type();
            if !aggregated_types.contains(&undefined_type) {
                aggregated_types.push(undefined_type);
            }
        }
        (Some(aggregated_types), false)
    }

    /// Go: `func (c *Checker) functionHasImplicitReturn(fn) bool`
    /// (checker.go:20711). TODO(port M6 W3): the end-flow-node reachability
    /// read; permissive false (no implicit-return aggregation until flow).
    pub(crate) fn function_has_implicit_return(&mut self, fn_node: NodeId) -> bool {
        let _ = fn_node;
        false
    }

    /// Go: `func (c *Checker) unwrapReturnType(returnType, functionFlags) *Type`
    /// (checker.go:20792) — the real body, over the S3 iteration family.
    pub(crate) fn unwrap_return_type(
        &mut self,
        return_type: TypeId,
        function_flags: FunctionFlags,
    ) -> Option<TypeId> {
        let is_generator = function_flags.intersects(FunctionFlags::GENERATOR);
        let is_async = function_flags.intersects(FunctionFlags::ASYNC);
        if is_generator {
            let return_iteration_type = self.get_iteration_type_of_generator_function_return_type(
                IterationTypeKind::Return,
                return_type,
                is_async,
            );
            let Some(return_iteration_type) = return_iteration_type else {
                return Some(self.error_type());
            };
            if is_async {
                let unwrapped = self.unwrap_awaited_type(return_iteration_type);
                return Some(
                    self.get_awaited_type_no_alias(unwrapped)
                        .unwrap_or_else(|| self.error_type()),
                );
            }
            return Some(return_iteration_type);
        }
        if is_async {
            return Some(self.get_awaited_type_no_alias(return_type).unwrap_or_else(|| self.error_type()));
        }
        Some(return_type)
    }

    /// Go: `func (c *Checker) getReturnTypeFromAnnotation(declaration) *Type`
    /// (checker.go:20462).
    pub(crate) fn get_return_type_from_annotation(&mut self, declaration: NodeId) -> Option<TypeId> {
        let type_node = crate::ast_shims::type_of_node(self.store(), declaration);
        type_node.map(|tn| self.get_type_from_type_node(tn))
    }

    /// Go: `func (c *Checker) checkAllCodePathsInNonVoidFunctionReturnOrThrow(fn, returnType)`
    /// (checker.go:3759).
    pub(crate) fn check_all_code_paths_in_non_void_function_return_or_throw(
        &mut self,
        fn_node: NodeId,
        return_type: Option<TypeId>,
    ) {
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        let t = return_type.map(|rt| self.unwrap_return_type(rt, function_flags).unwrap_or(rt));
        // Functions with an explicitly specified return type that includes
        // `void` or is exactly `any` or `undefined` don't need any return
        // statements.
        if let Some(t) = t {
            if relater::maybe_type_of_kind(self, t, TypeFlags::VOID)
                || self.type_(t).flags.intersects(TypeFlags::ANY.union(TypeFlags::UNDEFINED))
            {
                return;
            }
        }
        // If all we have is a function signature, or an arrow function with
        // an expression body, then there is nothing to check.
        // also if HasImplicitReturn flag is not set this means that all
        // codepaths in function body end with return or throw
        let body = function_body_of(self.store(), fn_node);
        let fn_kind = self.store().node(fn_node).kind;
        if fn_kind == Kind::MethodSignature
            || body.is_none()
            || self.store().node(body.expect("checked")).kind != Kind::Block
            || !self.function_has_implicit_return(fn_node)
        {
            return;
        }
        let has_explicit_return = self
            .store()
            .node(fn_node)
            .flags
            .intersects(NF::HAS_EXPLICIT_RETURN);
        let mut error_node = crate::ast_shims::type_of_node(self.store(), fn_node);
        if error_node.is_none() {
            // Go: fn.FunctionLikeData().FullSignature (the JSDoc full
            // signature) — TODO(port M6 W3): the JSDoc-tag seam; permissive
            // the declaration node itself.
        }
        let error_node = error_node.unwrap_or(fn_node);
        let t_flags = t.map(|tt| self.type_(tt).flags);
        if t_flags.is_some_and(|f| f.intersects(TypeFlags::NEVER)) {
            self.error(
                error_node,
                &diag::A_FUNCTION_RETURNING_NEVER_CANNOT_HAVE_A_REACHABLE_END_POINT,
                &[],
            );
        } else if t.is_some() && !has_explicit_return {
            // minimal check: function has syntactic return type annotation
            // and no explicit return statements in the body — this function
            // does not conform to the specification.
            self.error(
                error_node,
                &diag::A_FUNCTION_WHOSE_DECLARED_TYPE_IS_NEITHER_UNDEFINED_VOID_NOR_ANY_MUST_RETURN_A_VALUE,
                &[],
            );
        } else if let Some(t) = t {
            if self.strict_null_checks {
                let undefined_type = self.undefined_type();
                if !self.is_type_assignable_to(undefined_type, t) {
                    self.error(
                        error_node,
                        &diag::FUNCTION_LACKS_ENDING_RETURN_STATEMENT_AND_RETURN_TYPE_DOES_NOT_INCLUDE_UNDEFINED,
                        &[],
                    );
                }
            }
        }
        if self.compiler_options.no_implicit_returns == Tristate::TSTrue {
            if t.is_none() {
                // If return type annotation is omitted check if function
                // has any explicit return statements. If it does not have
                // any - its inferred return type is void - don't do checks.
                if !has_explicit_return {
                    return;
                }
                let signature = self.get_signature_from_declaration(fn_node);
                if let Some(signature) = signature {
                    let inferred_return_type = self.get_return_type_of_signature(signature);
                    if self.is_unwrapped_return_type_undefined_void_or_any(fn_node, inferred_return_type) {
                        return;
                    }
                }
            }
            self.error(error_node, &NOT_ALL_CODE_PATHS_RETURN_A_VALUE, &[]);
        }
    }

    /// Go: `func (c *Checker) isUnwrappedReturnTypeUndefinedVoidOrAny(fn, returnType) bool`
    /// (checker.go:3795).
    pub(crate) fn is_unwrapped_return_type_undefined_void_or_any(
        &mut self,
        fn_node: NodeId,
        return_type: TypeId,
    ) -> bool {
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), fn_node);
        match self.unwrap_return_type(return_type, function_flags) {
            Some(t) => {
                relater::maybe_type_of_kind(self, t, TypeFlags::VOID)
                    || self
                        .type_(t)
                        .flags
                        .intersects(TypeFlags::ANY.union(TypeFlags::UNDEFINED))
            }
            None => false,
        }
    }
}

/// Go: `func mayReturnNever(fn) bool` (checker.go:20721).
fn may_return_never(store: &dyn NodeStore, fn_node: NodeId) -> bool {
    match store.node(fn_node).kind {
        Kind::FunctionExpression | Kind::ArrowFunction => true,
        Kind::MethodDeclaration => {
            let parent = store.node(store.node(fn_node).parent.get());
            parent.kind == Kind::ObjectLiteralExpression
        }
        _ => false,
    }
}
