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
    A_RETURN_STATEMENT_CAN_ONLY_BE_USED_WITHIN_A_FUNCTION_BODY,
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
use crate::types::{MemberOverrideStatus, ObjectFlags, SignatureKind, Ternary, TypeFlags, TypeId};
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
    #[allow(dead_code)] // resolver pair closures; the sync/async slow paths reach them
    pub(crate) fn resolver_iterator_symbol_name(&self, r: &IterationTypesResolver) -> &'static str {
        match r.kind {
            IterationTypesResolverKind::Sync => "Symbol.iterator",
            IterationTypesResolverKind::Async => "Symbol.asyncIterator",
        }
    }

    /// Go: the resolver pair's `getGlobalIteratorType` closure — the S1
    /// spine's memoized global-resolver accessors (spine.rs) ARE the
    /// closure payloads; the resolver discriminant picks the sync/async row.
    pub(crate) fn get_global_iterator_type_of_resolver(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterator_type(),
            IterationTypesResolverKind::Async => self.get_global_async_iterator_type(),
        }
    }

    /// Go: the resolver pair's `getGlobalIterableTypeChecked` closure.
    pub(crate) fn get_global_iterable_type_checked_of_resolver(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterable_type_checked(),
            IterationTypesResolverKind::Async => self.get_global_async_iterable_type_checked(),
        }
    }

    /// Go: the resolver pair's `getGlobalIterableIteratorType` closure.
    pub(crate) fn get_global_iterable_iterator_type_of_resolver(
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

    /// Go: the resolver pair's `getGlobalIterableIteratorTypeChecked` closure
    /// (the createGeneratorType error-probe).
    pub(crate) fn get_global_iterable_iterator_type_checked_of_resolver(
        &mut self,
        r: IterationTypesResolver,
    ) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterable_iterator_type_checked(),
            IterationTypesResolverKind::Async => {
                self.get_global_async_iterable_iterator_type_checked()
            }
        }
    }

    /// Go: the resolver pair's `getGlobalIteratorObjectType` closure.
    pub(crate) fn get_global_iterator_object_type_of_resolver(&mut self, r: IterationTypesResolver) -> TypeId {
        match r.kind {
            IterationTypesResolverKind::Sync => self.get_global_iterator_object_type(),
            IterationTypesResolverKind::Async => self.get_global_async_iterator_object_type(),
        }
    }

    /// Go: the resolver pair's `getGlobalGeneratorType` closure.
    pub(crate) fn get_global_generator_type_of_resolver(&mut self, r: IterationTypesResolver) -> TypeId {
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

#[allow(dead_code)] // the W3 grammar stub family: callers land across the remaining M6 batches
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
    #[allow(dead_code)] // wired with the S3 class/module batch (checkModuleDeclaration)
    pub(crate) fn check_grammar_module_element_context(
        &mut self,
        node: NodeId,
        context_error_message: &'static Message,
    ) -> bool {
        let _ = (node, context_error_message);
        false
    }

    /// Go: `func (c *Checker) checkGrammarClassLikeDeclaration(node) bool`.
    #[allow(dead_code)] // wired with the S3 class batch (checkClassLikeDeclaration)
    pub(crate) fn check_grammar_class_like_declaration(&mut self, node: NodeId) -> bool {
        let _ = node;
        false
    }

    /// Go: `func (c *Checker) checkGrammarInterfaceDeclaration(decl) bool`.
    #[allow(dead_code)] // wired with the S3 interface batch (checkInterfaceDeclaration)
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
            Some(then_statement),
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
        // Go: `body *ast.Node` — nil when the && arm's parent isn't an if
        // statement (checker.go:12587); Option matches that nullability.
        body: Option<NodeId>,
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
        body: Option<NodeId>,
    ) {
        let mut cond_expr = tsc_ast::utilities::skip_parentheses(self.store(), cond_expr);
        self.check_testing_known_truthy_type(cond_expr, cond_type, body);
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
            self.check_testing_known_truthy_type(cond_expr, cond_type, body);
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
            self.check_testing_known_truthy_types(location, cond_type, body);
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
        let is_promise = self.get_awaited_type_of_promise(t).is_some();
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
            .expect("DoStatement data")
            .clone();
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
            .expect("WhileStatement data")
            .clone();
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
        let expr_type = self.check_expression(data.expression);
        let right_type = self.get_non_nullable_type_if_needed(expr_type);
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
            } else {
                let index = self.get_index_type_or_string(right_type);
                if !self.is_type_assignable_to(index, left_type) {
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
        let idx = relater::get_index_type(self, t);
        let index_type = self.get_extract_string_type(idx);
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
                    && self.language_version < Feature::ForAwaitOf.minimum_target()
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
                // iteratedType may be undefined — short-circuit the
                // relation with the errorType (the typeofsymbol wrapper
                // already maps Go's nil to errorType).
                let iterated = iterated_type;
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
                // iteratedType will be undefined if the rightType was
                // missing properties/signatures required to get its
                // iteratedType (errorType in the wrapper's nil mapping).
                if !relater::is_error_type(self, iterated_type) {
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
        // TODO(port M6 S2/S7): getSignatureFromDeclaration's resolution seam
        // is still permissive for some declaration shapes; Go's nil signature
        // would rest on the same permissive answers, so the type-dependent
        // return checks below are skipped until the seam completes.
        let Some(signature) = self.get_signature_from_declaration(container) else {
            return;
        };
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
                        Some(expr_node),
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
        if !self.check_grammar_statement_in_ambient_context(node)
            && self
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
        let d = self
            .store()
            .node(node)
            .as_with_statement()
            .expect("WithStatement data")
            .clone();
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
                        self.grammar_error_on_node(d.label, &DUPLICATE_LABEL_0, std::slice::from_ref(&label_text));
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
        if !self.check_grammar_statement_in_ambient_context(node)
            && is_identifier(self.store().node(throw_expr))
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
#[allow(dead_code)] // wired with checkReturnExpression's S4 batch
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
// bitfield checkExternalEmitHelpers reads. TODO(port M6 W5): the real
// checkExternalEmitHelpers (emitsupport.go's wave — the context/emit-helpers
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
#[allow(dead_code)] // the full Go minimum-target table; later slices construct the variants
pub(crate) enum Feature {
    AsyncFunctions,
    ForAwaitOf,
    AsyncGenerators,
    ObjectSpreadRest,
    UsingAndAwaitUsing,
    ClassAndClassElementDecorators,
}

#[allow(dead_code)] // the full Go minimum-target table; S4/S6 gates construct the variants
impl Feature {
    /// Go: the LanguageFeatureMinimumTarget table (types.go:1473-1490).
    #[allow(dead_code)] // the full Go table; later slices construct the rows
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
    // (checkTruthinessExpression/checkTruthinessOfType/checkReferenceExpression
    // moved to expressions.rs — the M6 S4 slice owns their line ranges.)

    /// Go: `func (c *Checker) getSymbolAtLocation(node, ignoreErrors) *ast.Symbol`
    /// (checker.go:32147 — S10's getSymbolOfNode resolution). Permissive
    /// None (the tested-symbol paths skip their diagnostics).
    pub(crate) fn get_symbol_at_location_stub(&mut self, node: NodeId, ignore_errors: bool) -> Option<SymbolId> {
        let _ = (node, ignore_errors);
        None
    }

    /// Go: `func (c *Checker) getAwaitedTypeOfPromise(t) *Type` (checker.go:32007)
    /// — REAL (M6 S9): getPromisedTypeOfPromiseEx then getAwaitedTypeEx.
    pub(crate) fn get_awaited_type_of_promise(&mut self, t: TypeId) -> Option<TypeId> {
        self.get_awaited_type_of_promise_ex(t, None, None)
    }

    /// Go: `func (c *Checker) getAwaitedTypeOfPromiseEx(t, errorNode, diagnosticMessage, args...) *Type`
    /// (checker.go:32011) — REAL (M6 S9).
    pub(crate) fn get_awaited_type_of_promise_ex(
        &mut self,
        t: TypeId,
        error_node: Option<NodeId>,
        diagnostic_message: Option<&'static Message>,
    ) -> Option<TypeId> {
        let mut this_type_for_error = None;
        let promised_type =
            self.get_promised_type_of_promise_ex(t, error_node, &mut this_type_for_error);
        if let Some(promised_type) = promised_type {
            return Some(self.get_awaited_type_ex(promised_type, error_node, diagnostic_message));
        }
        None
    }

    /// Go: `func (c *Checker) getPromisedTypeOfPromiseEx(t, errorNode, thisTypeForErrorOut) *Type`
    /// (checker.go:29455) — REAL (M6 S9): the then-signature walk.
    ///
    /// Gets the "promised type" of a promise — the type of the "value"
    /// parameter of the "onfulfilled" callback (Go's doc comment).
    pub(crate) fn get_promised_type_of_promise_ex(
        &mut self,
        t: TypeId,
        error_node: Option<NodeId>,
        this_type_for_error_out: &mut Option<TypeId>,
    ) -> Option<TypeId> {
        //  { // type
        //      then( // thenFunction
        //          onfulfilled: ( // onfulfilledParameterType
        //              value: T // valueParameterType
        //          ) => any
        //      ): any;
        //  }
        // (Go's comment, verbatim.)
        if u::is_type_any(Some(self.type_(t))) {
            return None;
        }
        if let Some(cached) = self.cached_types_get(
            crate::todo_shapes::CachedTypeKind::PromisedTypeOfPromise,
            t,
        ) {
            return Some(cached);
        }
        let global_promise_type = self.get_global_type("Promise", 1, false /*reportErrors*/);
        if self.is_reference_to_type(t, global_promise_type) {
            let type_arguments = self.get_type_arguments(t);
            let result = *type_arguments.first().expect("Promise reference arity 1");
            self.cached_types_set(
                crate::todo_shapes::CachedTypeKind::PromisedTypeOfPromise,
                t,
                Some(result),
            );
            return Some(result);
        }
        // primitives with a `{ then() }` won't be unwrapped/adopted.
        let base = relater::get_base_constraint_or_type(self, t);
        if self.all_types_assignable_to_kind(
            base,
            TypeFlags::PRIMITIVE | TypeFlags::NEVER,
        ) {
            return None;
        }
        let then_function = self.get_type_of_property_of_type(t, "then");
        // TODO: GH#18217 (Go's comment).
        if then_function.is_some_and(|f| u::is_type_any(Some(self.type_(f)))) {
            return None;
        }
        let then_signatures = then_function
            .map(|f| self.get_signatures_of_type_impl(f, SignatureKind::Call))
            .unwrap_or_default();
if then_signatures.is_empty() {
            if let Some(node) = error_node {
                self.error(node, &diag::A_PROMISE_MUST_HAVE_A_THEN_METHOD, &[]);
            }
            return None;
        }
        let mut this_type_for_error: Option<TypeId> = None;
        let mut candidates: Vec<crate::types::SignatureId> = Vec::new();
        for &then_signature in &then_signatures {
            let this_type = self.get_this_type_of_signature(then_signature);
            if let Some(this_type) = this_type {
                let void_type = self.void_type();
                if this_type != void_type
                    && !self.is_type_related_to(t, this_type, crate::relater::RelationKind::Subtype)
                {
                    this_type_for_error = Some(this_type);
                    continue;
                }
            }
            candidates.push(then_signature);
        }
if candidates.is_empty() {
            debug_assert!(this_type_for_error.is_some());
            *this_type_for_error_out = this_type_for_error;
            if let (Some(node), Some(this_type)) = (error_node, this_type_for_error) {
                let t_str = self.type_to_string(t);
                let this_str = self.type_to_string(this_type);
                self.error(
                    node,
                    &diag::THE_THIS_CONTEXT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_METHOD_S_THIS_OF_TYPE_1,
                    &[t_str, this_str],
                );
            }
            return None;
        }
        let first_params: Vec<TypeId> = candidates
            .iter()
            .map(|&sig| self.get_type_of_first_parameter_of_signature(sig))
            .collect();
        let union = self.get_union_type(&first_params);
        let onfulfilled_parameter_type =
            relater::get_type_with_facts(self, union, TypeFacts::NE_UNDEFINED_OR_NULL);
        if u::is_type_any(Some(self.type_(onfulfilled_parameter_type))) {
            return None;
        }
        let onfulfilled_parameter_signatures =
            self.get_signatures_of_type_impl(onfulfilled_parameter_type, SignatureKind::Call);
        if onfulfilled_parameter_signatures.is_empty() {
            if let Some(node) = error_node {
                self.error(
                    node,
                    &diag::THE_FIRST_PARAMETER_OF_THE_THEN_METHOD_OF_A_PROMISE_MUST_BE_A_CALLBACK,
                    &[],
                );
            }
            return None;
        }
        let second_params: Vec<TypeId> = onfulfilled_parameter_signatures
            .iter()
            .map(|&sig| self.get_type_of_first_parameter_of_signature(sig))
            .collect();
        let result = self.get_union_type_ex(
            second_params,
            crate::todo_shapes::UnionReduction::Subtype,
            None,
            None,
        );
        self.cached_types_set(
            crate::todo_shapes::CachedTypeKind::PromisedTypeOfPromise,
            t,
            Some(result),
        );
        Some(result)
    }

    /// Go: `func (c *Checker) getTypeOfFirstParameterOfSignature(signature) *Type`
    /// (checker.go:29533).
    pub(crate) fn get_type_of_first_parameter_of_signature(
        &mut self,
        signature: crate::types::SignatureId,
    ) -> TypeId {
        let never = self.never_type();
        self.get_type_of_first_parameter_of_signature_with_fallback(signature, never)
    }

    /// Go: `func (c *Checker) getTypeOfFirstParameterOfSignatureWithFallback(signature, fallbackType) *Type`
    /// (checker.go:29537).
    fn get_type_of_first_parameter_of_signature_with_fallback(
        &mut self,
        signature: crate::types::SignatureId,
        fallback_type: TypeId,
    ) -> TypeId {
        let parameters = self.signatures[signature.0 as usize].parameters.clone();
        if !parameters.is_empty() {
            return self.get_type_at_position(signature, 0);
        }
        fallback_type
    }

    /// Go: `func (c *Checker) isThenableType(t *Type) bool` (checker.go:31997)
    /// — REAL (M6 S9).
    pub(crate) fn is_thenable_type(&mut self, t: TypeId) -> bool {
        let base = relater::get_base_constraint_or_type(self, t);
        // primitive types cannot be considered "thenable" since they are not
        // objects (Go's comment).
        if self.all_types_assignable_to_kind(base, TypeFlags::PRIMITIVE | TypeFlags::NEVER) {
            return false;
        }
        let Some(then_function) = self.get_type_of_property_of_type(t, "then") else {
            return false;
        };
        let facts_type =
            relater::get_type_with_facts(self, then_function, TypeFacts::NE_UNDEFINED_OR_NULL);
        !self
            .get_signatures_of_type_impl(facts_type, SignatureKind::Call)
            .is_empty()
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

    /// Go: `func (c *Checker) checkAwaitedType(t, withAlias, errorNode, message)`
    /// (checker.go:31781) — REAL (M6 S9).
    pub(crate) fn check_awaited_type(
        &mut self,
        t: TypeId,
        with_alias: bool,
        error_node: NodeId,
        message: &'static Message,
    ) -> TypeId {
        let awaited_type: Option<TypeId> = if with_alias {
            Some(self.get_awaited_type_ex(t, Some(error_node), Some(message)))
        } else {
            self.get_awaited_type_no_alias_ex(t, Some(error_node), Some(message))
        };
        if let Some(awaited_type) = awaited_type {
            return awaited_type;
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getEffectiveCheckNode(node) *ast.Node`
    /// (checker.go:9607) — skipOuterExpressions(Assertions|Parentheses).
    pub(crate) fn get_effective_check_node(&mut self, node: NodeId) -> NodeId {
        self.skip_outer_expressions_assertions_or_parens(node)
    }

    /// Go: `func (c *Checker) isTypeEqualityComparableTo(source, target) bool`
    /// (checker.go:13105) — `isTypeComparableTo` in both directions; the
    /// relation runs through relater's Comparable relation directly.
    pub(crate) fn is_type_equality_comparable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.check_type_comparable_to(source, target, None, None)
            && self.check_type_comparable_to(target, source, None, None)
    }

    /// Go: `func (c *Checker) getResolvedSymbolOrNil(node) *ast.Symbol`
    /// (checker.go:14175) — the link reader only (no resolution).
    pub(crate) fn get_resolved_symbol_nil(&mut self, node: NodeId) -> Option<SymbolId> {
        self.symbol_node_links.get(node).resolved_symbol
    }

    /// Go: `func (c *Checker) getAwaitedType(t) *Type` (checker.go:31802) —
    /// REAL (M6 S9): the awaited type of a type (identity for non-thenables).
    /// (The `_impl` suffix keeps the pub GetAwaitedType twin in exports.rs
    /// the sole `get_awaited_type` name.)
    pub(crate) fn get_awaited_type_impl(&mut self, t: TypeId) -> TypeId {
        self.get_awaited_type_ex(t, None, None)
    }

    /// Go: `func (c *Checker) getAwaitedTypeEx(t, errorNode, diagnosticMessage, args...) *Type`
    /// (checker.go:31806) — REAL (M6 S9). PORT: the variadic args ride the
    /// Option<&'static Message> + &[String] convention the checker's error
    /// family uses; the current call sites pass no args.
    pub(crate) fn get_awaited_type_ex(
        &mut self,
        t: TypeId,
        error_node: Option<NodeId>,
        diagnostic_message: Option<&'static Message>,
    ) -> TypeId {
        if let Some(awaited_type) =
            self.get_awaited_type_no_alias_ex(t, error_node, diagnostic_message)
        {
            return self.create_awaited_type_if_needed(awaited_type);
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getAwaitedTypeNoAlias(t) *Type` (checker.go:31815)
    /// — REAL (M6 S9): the awaited type without introducing an `Awaited<T>`
    /// wrapper (Go's OrElse(errorType) answer; the port's Option keeps the
    /// None = errorType convention at the call sites).
    pub(crate) fn get_awaited_type_no_alias(&mut self, t: TypeId) -> Option<TypeId> {
        self.get_awaited_type_no_alias_ex(t, None, None)
    }

    /// Go: `func (c *Checker) getAwaitedTypeNoAliasEx(t, errorNode, diagnosticMessage, args...) *Type`
    /// (checker.go:31819) — REAL (M6 S9).
    ///
    /// Gets the "awaited type" of a type without introducing an `Awaited<T>`
    /// wrapper. The "awaited type" of an expression is its "promised type"
    /// if the expression is a Promise-like type; otherwise, it is the type
    /// of the expression. If the "promised type" is itself a Promise-like,
    /// the "promised type" is recursively unwrapped until a non-promise type
    /// is found. This is used to reflect the runtime behavior of the `await`
    /// keyword (Go's doc comments, verbatim).
    pub(crate) fn get_awaited_type_no_alias_ex(
        &mut self,
        t: TypeId,
        error_node: Option<NodeId>,
        diagnostic_message: Option<&'static Message>,
    ) -> Option<TypeId> {
        if u::is_type_any(Some(self.type_(t))) {
            return Some(t);
        }
        // If this is already an `Awaited<T>`, just return it. This avoids
        // `Awaited<Awaited<T>>` in higher-order (Go's comment).
        if self.is_awaited_type_instantiation(t) {
            return Some(t);
        }
        // If we've already cached an awaited type, return a possible
        // `Awaited<T>` for it (Go's comment).
        if let Some(cached) =
            self.cached_types_get(crate::todo_shapes::CachedTypeKind::AwaitedType, t)
        {
            return Some(cached);
        }
        // For a union, get a union of the awaited types of each constituent.
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            if self.awaited_type_stack.contains(&t) {
                if let Some(node) = error_node {
                    self.error(
                        node,
                        &diag::TYPE_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_THE_FULFILLMENT_CALLBACK_OF_ITS_OWN_THEN_METHOD,
                        &[],
                    );
                }
                return None;
            }
            self.awaited_type_stack.push(t);
            let mapped = self.map_type(t, &mut |c, s| {
                c.get_awaited_type_no_alias_ex(s, error_node, diagnostic_message)
            });
            self.awaited_type_stack.pop();
            // PORT: Go caches the (possibly nil) mapped type; a nil cache
            // entry reads as a miss on Go's `cached != nil` lookup, so the
            // port skips the store for the None case (read-equivalent).
            if let Some(mapped) = mapped {
                self.cached_types_set(crate::todo_shapes::CachedTypeKind::AwaitedType, t, Some(mapped));
            }
            return mapped;
        }
        // If `type` is generic and should be wrapped in `Awaited<T>`,
        // return it (Go's comment).
        if self.is_awaited_type_needed(t) {
            self.cached_types_set(crate::todo_shapes::CachedTypeKind::AwaitedType, t, Some(t));
            return Some(t);
        }
        let mut this_type_for_error: Option<TypeId> = None;
        let promised_type =
            self.get_promised_type_of_promise_ex(t, None /*errorNode*/, &mut this_type_for_error);
        if let Some(promised_type) = promised_type {
            if t == promised_type || self.awaited_type_stack.contains(&promised_type) {
                // Verify that we don't have a bad actor in the form of a
                // promise whose promised type is the same as the promise
                // type, or a mutually recursive promise. If so, we return
                // undefined as we cannot guess the shape. If this were the
                // actual case in the JavaScript, this Promise would never
                // resolve (Go's comment — the BadPromise examples are in
                // checker.go:31852-31876).
                if let Some(node) = error_node {
                    self.error(
                        node,
                        &diag::TYPE_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_THE_FULFILLMENT_CALLBACK_OF_ITS_OWN_THEN_METHOD,
                        &[],
                    );
                }
                return None;
            }
            // Keep track of the type we're about to unwrap to avoid bad
            // recursive promise types. See the comments above for more
            // information (Go's comment).
            self.awaited_type_stack.push(t);
            let awaited_type =
                self.get_awaited_type_no_alias_ex(promised_type, error_node, diagnostic_message);
            self.awaited_type_stack.pop();
            let awaited_type = awaited_type?;
            self.cached_types_set(
                crate::todo_shapes::CachedTypeKind::AwaitedType,
                t,
                Some(awaited_type),
            );
            return Some(awaited_type);
        }
        // The type was not a promise, so it could not be unwrapped any
        // further. As long as the type does not have a callable "then"
        // property, it is safe to return the type; otherwise, an error is
        // reported and we return undefined (Go's comment — the `await
        // { then(): void {} }` example is in checker.go:31885-31902).
        if self.is_thenable_type(t) {
            if let Some(node) = error_node {
                // PORT: Go chains the this-context mismatch diagnostic onto
                // the caller's diagnosticMessage via NewDiagnosticChainForNode;
                // the port emits the primary (or the this-context diagnostic
                // when no message is given) until the W4 diagnostic-chain
                // machinery lands.
                if let Some(this_type) = this_type_for_error {
                    let t_str = self.type_to_string(t);
                    let this_str = self.type_to_string(this_type);
                    self.error(
                        node,
                        &diag::THE_THIS_CONTEXT_OF_TYPE_0_IS_NOT_ASSIGNABLE_TO_METHOD_S_THIS_OF_TYPE_1,
                        &[t_str, this_str],
                    );
                } else if let Some(message) = diagnostic_message {
                    self.error(node, message, &[]);
                }
            }
            return None;
        }
        self.cached_types_set(crate::todo_shapes::CachedTypeKind::AwaitedType, t, Some(t));
        Some(t)
    }

    /// Go: `func (c *Checker) isAwaitedTypeInstantiation(t) bool` (checker.go:31933)
    /// — REAL (M6 S9).
    pub(crate) fn is_awaited_type_instantiation(&mut self, t: TypeId) -> bool {
        if self.type_(t).flags.intersects(TypeFlags::CONDITIONAL) {
            let awaited_symbol = self.get_global_awaited_symbol_or_nil();
            return awaited_symbol.is_some_and(|s| {
                self.type_(t)
                    .alias
                    .as_ref()
                    .is_some_and(|a| a.symbol == Some(s) && a.type_arguments.len() == 1)
            });
        }
        false
    }

    /// Go: `func (c *Checker) isAwaitedTypeNeeded(t) bool` (checker.go:31941)
    /// — REAL (M6 S9).
    pub(crate) fn is_awaited_type_needed(&mut self, t: TypeId) -> bool {
        // If this is already an `Awaited<T>`, we shouldn't wrap it. This
        // helps to avoid `Awaited<Awaited<T>>` in higher-order (Go's comment).
        if u::is_type_any(Some(self.type_(t))) || self.is_awaited_type_instantiation(t) {
            return false;
        }
        // We only need `Awaited<T>` if `T` contains possibly non-primitive
        // types (Go's comment).
        if relater::is_generic_object_type(self, t) {
            let base_constraint = self.get_base_constraint_of_type_impl(t);
            // We only need `Awaited<T>` if `T` is a type variable that has
            // no base constraint, or the base constraint of `T` is `any`,
            // `unknown`, `{}`, `object`, or is promise-like (Go's comment).
            if let Some(base) = base_constraint {
                if self.type_(base).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
                    || relater::is_empty_object_type(self, base)
                {
                    return true;
                }
                // someType(base, c.isThenableType) — the constituents are
                // collected before the mutable probe (the some_type borrow
                // PORT pattern).
                let constituents: Vec<TypeId> =
                    if self.type_(base).flags.intersects(TypeFlags::UNION) {
                        self.type_(base).types().to_vec()
                    } else {
                        vec![base]
                    };
                return constituents.iter().any(|&c| self.is_thenable_type(c));
            }
            return relater::maybe_type_of_kind(self, t, TypeFlags::TYPE_VARIABLE);
        }
        false
    }

    /// Go: `func (c *Checker) createAwaitedTypeIfNeeded(t) *Type` (checker.go:31965)
    /// — REAL (M6 S9).
    ///
    /// We wrap type `T` in `Awaited<T>` based on the following conditions:
    /// - `T` is not already an `Awaited<U>`, and
    /// - `T` is generic, and
    /// - One of the following applies:
    ///   - `T` has no base constraint, or
    ///   - The base constraint of `T` is `any`, `unknown`, `object`, or `{}`, or
    ///   - The base constraint of `T` is an object type with a callable
    ///     `then` method (Go's comment, verbatim).
    pub(crate) fn create_awaited_type_if_needed(&mut self, t: TypeId) -> TypeId {
        if self.is_awaited_type_needed(t) {
            if let Some(awaited_type) = self.try_create_awaited_type(t) {
                return awaited_type;
            }
        }
        t
    }

    /// Go: `func (c *Checker) tryCreateAwaitedType(t) *Type` (checker.go:31978)
    /// — REAL (M6 S9). PORT: getGlobalAwaitedSymbol resolves through spine's
    /// resolve_global accessor family (permissive None without libs — Go's
    /// nil answer).
    pub(crate) fn try_create_awaited_type(&mut self, t: TypeId) -> Option<TypeId> {
        let awaited_symbol = self.get_global_awaited_symbol()?;
        // Unwrap unions that may contain `Awaited<T>`, otherwise its possible
        // to manufacture an `Awaited<Awaited<T> | U>` where an
        // `Awaited<T | U>` would suffice (Go's comment).
        let unwrapped = self.unwrap_awaited_type(t);
        Some(self.get_type_alias_instantiation(awaited_symbol, vec![unwrapped], None))
    }

    /// Go: `func (c *Checker) unwrapAwaitedType(t) *Type` (checker.go:31985)
    /// — REAL (M6 S9): for a generic `Awaited<T>`, gets `T`.
    pub(crate) fn unwrap_awaited_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            // PORT: Go's mapType never returns nil for this callback (the
            // fallback keeps t on the None path); the constituent count is
            // preserved.
            return self
                .map_type(t, &mut |c, s| Some(c.unwrap_awaited_type(s)))
                .unwrap_or(t);
        }
        if self.is_awaited_type_instantiation(t) {
            let alias = self.type_(t).alias.clone().expect("Awaited alias");
            return alias.type_arguments[0];
        }
        t
    }

    // Go: `func (c *Checker) getContextualType(node, contextFlags) *Type`
    // (checker.go:29891 — S9): MOVED — the REAL dispatcher (the full
    // parent-kind arm set) lives in contextual.rs as `get_contextual_type`
    // (same signature).
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
            if let Some(next_type) = iteration_types.next_type.filter(|_| check_assignability) {
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
                let yield_type = iteration_types.yield_type?;
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
                let array_types = self.type_(input_type).as_union_type().expect("UnionType data").base.types.clone();
                let filtered_types: Vec<TypeId> = array_types
                    .iter()
                    .copied()
                    .filter(|&t| !self.type_(t).flags.intersects(TypeFlags::STRING_LIKE))
                    .collect();
                if filtered_types.len() != array_types.len() {
                    array_type = self.get_union_type_ex(filtered_types, crate::todo_shapes::UnionReduction::Subtype, None, None);
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
                    && self.get_awaited_type_of_promise(array_type).is_some();
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
        if let Some(array_element_type) = array_element_type.filter(|_| has_string_constituent) {
            // This is just an optimization for the case where
            // arrayOrStringType is string | string[]
            if self.type_(array_element_type).flags.intersects(TypeFlags::STRING_LIKE)
                && self.compiler_options.no_unchecked_indexed_access != Tristate::TSTrue
            {
                return Some(self.string_type());
            }
            if possible_out_of_bounds {
                return Some(self.get_union_type_ex(
                    vec![array_element_type, self.string_type(), self.undefined_type()],
                    crate::todo_shapes::UnionReduction::Subtype,
                    None,
                    None,
                ));
            }
            return Some(self.get_union_type_ex(
                vec![array_element_type, self.string_type()],
                crate::todo_shapes::UnionReduction::Subtype,
                None,
                None,
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
    #[allow(clippy::only_used_in_recursion)] // Go's noCache threads through the union recursion
    pub(crate) fn get_iteration_types_of_iterable_worker(
        &mut self,
        t: TypeId,
        use_: IterationUse,
        error_node: Option<NodeId>,
        no_cache: bool,
    ) -> IterationTypes {
        if self.type_(t).flags.intersects(TypeFlags::UNION) {
            let types = self.type_(t).as_union_type().expect("UnionType data").base.types.clone();
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
        let iterable_type = self.get_global_iterable_type_checked_of_resolver(r);
        let iterator_object_type = self.get_global_iterator_object_type_of_resolver(r);
        let iterable_iterator_type = self.get_global_iterable_iterator_type_of_resolver(r);
        let generator_type = self.get_global_generator_type_of_resolver(r);
        if self.is_reference_to_type(t, iterable_type)
            || self.is_reference_to_type(t, iterator_object_type)
            || self.is_reference_to_type(t, iterable_iterator_type)
            || self.is_reference_to_type(t, generator_type)
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
        let builtin_iterator_types = self.get_global_builtin_iterator_types(r);
        if self.is_reference_to_some_type(t, builtin_iterator_types) {
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
            && matches!(&td.data, crate::types::TypeData::TypeReference(d) if d.base.target == Some(target))
    }

    /// Go: `func (c *Checker) isReferenceToSomeType(t, targets) bool` (checker.go:6605).
    pub(crate) fn is_reference_to_some_type(&mut self, t: TypeId, targets: Vec<TypeId>) -> bool {
        let td = self.type_(t);
        if !td.object_flags.intersects(ObjectFlags::REFERENCE) {
            return false;
        }
        matches!(&td.data, crate::types::TypeData::TypeReference(d) if d.base.target.is_some_and(|target| targets.contains(&target)))
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
        // Go: errorNode feeds the missing-Awaited<T> diagnostics (S9's
        // getGlobalAwaitedSymbol — TODO(port M6 S9)); unused until then.
        _error_node: Option<NodeId>,
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
        let iterator_name = self.resolver_iterator_symbol_name(&r);
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
        let iterator_type = self.get_global_iterator_type_of_resolver(r);
        let iterator_object_type = self.get_global_iterator_object_type_of_resolver(r);
        let iterable_iterator_type = self.get_global_iterable_iterator_type_of_resolver(r);
        let generator_type = self.get_global_generator_type_of_resolver(r);
        if self.is_reference_to_type(t, iterator_type)
            || self.is_reference_to_type(t, iterator_object_type)
            || self.is_reference_to_type(t, iterable_iterator_type)
            || self.is_reference_to_type(t, generator_type)
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
        let builtin_iterator_types = self.get_global_builtin_iterator_types(r);
        if self.is_reference_to_some_type(t, builtin_iterator_types) {
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
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
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
            diagnostic_output,
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
                report_diagnostic(self, d, diagnostic_output);
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
        let suggest_await = self.get_awaited_type_of_promise(t).is_some();
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
fn filter_type_local(c: &mut Checker, t: TypeId, mut f: impl FnMut(&mut Checker, TypeId) -> bool) -> TypeId {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let types = c
            .type_(t)
            .as_union_type()
            .expect("UnionType data")
            .base
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
        Kind::MethodDeclaration => n.as_method_declaration().expect("data").body,
        Kind::MethodSignature => None, // Go: method signatures have no body
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
    #[allow(dead_code)] // wired with checkReturnExpression's S4 batch
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
        let fn_symbol = crate::typeofsymbol::get_symbol_of_declaration(self, fn_node);
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
    /// (checker.go:20462) — delegates to S8's FULL body (members.rs; the
    /// S3-era annotation-only read is retired).
    pub(crate) fn get_return_type_from_annotation(&mut self, declaration: NodeId) -> Option<TypeId> {
        self.get_return_type_from_annotation_impl(declaration)
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
        let error_node = crate::ast_shims::type_of_node(self.store(), fn_node);
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
#[allow(dead_code)] // wired with checkReturnExpression's S4 batch
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

// ────────────────────────────────────────────────────────────────────────────
// M6 S3 batch 2 — variable/decorator checks, the unused-identifier family,
// merged-exports + type-parameter checks (Go checker.go:4309-5388 remainder,
// 5947-7530).
// ────────────────────────────────────────────────────────────────────────────

use tsc_diagnostics::Category;
use u::has_dot_dot_dot_token;

// ────────────────────────────────────────────────────────────────────────────
// Signature-declaration node accessors (the ~10 function-like payloads share
// the same field set; Go reads them through the SignatureDeclaration interface).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `node.TypeParameters()` for the function-like payloads.
pub(crate) fn type_parameters_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    let list = match n.kind {
        Kind::FunctionDeclaration => n.as_function_declaration().expect("data").type_parameters.as_ref(),
        Kind::FunctionExpression => n.as_function_expression().expect("data").type_parameters.as_ref(),
        Kind::ArrowFunction => n.as_arrow_function().expect("data").type_parameters.as_ref(),
        Kind::MethodDeclaration => n.as_method_declaration().expect("data").type_parameters.as_ref(),
        Kind::MethodSignature => {
            n.as_method_signature_declaration()
                .expect("data")
                .type_parameters
                .as_ref()
        }
        Kind::Constructor => n.as_constructor_declaration().expect("data").type_parameters.as_ref(),
        Kind::GetAccessor => n.as_get_accessor_declaration().expect("data").type_parameters.as_ref(),
        Kind::SetAccessor => n.as_set_accessor_declaration().expect("data").type_parameters.as_ref(),
        Kind::CallSignature => n.as_call_signature_declaration().expect("data").type_parameters.as_ref(),
        Kind::ConstructSignature => {
            n.as_construct_signature_declaration()
                .expect("data")
                .type_parameters
                .as_ref()
        }
        Kind::IndexSignature => n.as_index_signature_declaration().expect("data").type_parameters.as_ref(),
        Kind::FunctionType => n.as_function_type_node().expect("data").type_parameters.as_ref(),
        Kind::ConstructorType => n.as_constructor_type_node().expect("data").type_parameters.as_ref(),
        _ => None,
    };
    list.map(|l| l.nodes.to_vec()).unwrap_or_default()
}

/// Go: `node.Parameters()` (ast_shims::parameters_of carries the same walk).
pub(crate) fn parameters_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    crate::ast_shims::parameters_of(store, node).to_vec()
}

/// Go: `node.Type()` for the function-like payloads (the return-type node).
pub(crate) fn signature_type_node(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    
    match n.kind {
        Kind::FunctionDeclaration => n.as_function_declaration().expect("data").type_,
        Kind::FunctionExpression => n.as_function_expression().expect("data").type_,
        Kind::ArrowFunction => n.as_arrow_function().expect("data").type_,
        Kind::MethodDeclaration => n.as_method_declaration().expect("data").type_,
        Kind::MethodSignature => n.as_method_signature_declaration().expect("data").type_,
        Kind::Constructor => n.as_constructor_declaration().expect("data").type_,
        Kind::GetAccessor => n.as_get_accessor_declaration().expect("data").type_,
        Kind::SetAccessor => n.as_set_accessor_declaration().expect("data").type_,
        Kind::CallSignature => n.as_call_signature_declaration().expect("data").type_,
        Kind::ConstructSignature => {
            n.as_construct_signature_declaration().expect("data").type_
        }
        Kind::IndexSignature => n.as_index_signature_declaration().expect("data").type_,
        Kind::FunctionType => n.as_function_type_node().expect("data").type_,
        Kind::ConstructorType => n.as_constructor_type_node().expect("data").type_,
        _ => None,
    }
}

/// Go: `node.TypeArguments()` for the type-node payloads that carry them.
pub(crate) fn type_arguments_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    let list = match n.kind {
        Kind::TypeReference => n.as_type_reference_node().expect("data").type_arguments.as_ref(),
        Kind::ImportType => n.as_import_type_node().expect("data").type_arguments.as_ref(),
        Kind::ExpressionWithTypeArguments => {
            n.as_expression_with_type_arguments()
                .expect("data")
                .type_arguments
                .as_ref()
        }
        Kind::TypeQuery => n.as_type_query_node().expect("data").type_arguments.as_ref(),
        _ => None,
    };
    list.map(|l| l.nodes.to_vec()).unwrap_or_default()
}

/// Go: `node.Members()` for the member-list payloads.
pub(crate) fn members_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    let list = match n.kind {
        Kind::ClassDeclaration => n.as_class_declaration().expect("data").members.as_ref(),
        Kind::ClassExpression => n.as_class_expression().expect("data").members.as_ref(),
        Kind::InterfaceDeclaration => n.as_interface_declaration().expect("data").members.as_ref(),
        Kind::TypeLiteral => n.as_type_literal_node().expect("data").members.as_ref(),
        _ => None,
    };
    list.map(|l| l.nodes.to_vec()).unwrap_or_default()
}

// ────────────────────────────────────────────────────────────────────────────
// Variable / decorator checks (Go checker.go:5947-6293)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkVariableStatement(node)` (checker.go:5951).
    pub(crate) fn check_variable_statement(&mut self, node: NodeId) {
        let declaration_list = self
            .store()
            .node(node)
            .as_variable_statement()
            .expect("VariableStatement data")
            .declaration_list;
        if !self.check_grammar_modifiers(node) {
            self.check_grammar_variable_declaration_list(declaration_list);
            self.check_grammar_for_disallowed_block_scoped_variable_statement(node);
        }
        self.check_variable_declaration_list(declaration_list);
    }

    /// Go: `func (c *Checker) checkVariableDeclarationList(node)` (checker.go:5960).
    pub(crate) fn check_variable_declaration_list(&mut self, node: NodeId) {
        // Go: GetCombinedNodeFlags(node) & NodeFlagsBlockScoped — the using /
        // await-using emit-helper gate (blockScopeKind one-of).
        let flags = tsc_binder::ast_shims::get_combined_node_flags(self.store(), node);
        let block_scope_kind = flags & NF::BLOCK_SCOPED;
        if (block_scope_kind == NF::USING || block_scope_kind == NF::AWAIT_USING)
            && self.language_version < Feature::UsingAndAwaitUsing.minimum_target()
        {
            self.check_external_emit_helpers_stub(
                node,
                ExternalEmitHelpers::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES,
            );
        }
        let declarations = self
            .store()
            .node(node)
            .as_variable_declaration_list()
            .expect("VariableDeclarationList data")
            .declarations
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        self.check_source_elements(&declarations);
    }

    /// Go: `func (c *Checker) checkVariableDeclaration(node)` (checker.go:5968).
    pub(crate) fn check_variable_declaration(&mut self, node: NodeId) {
        // Go: the tracer push (checkVariableDeclaration) — the tracing shape
        // rides with the tracer wave; skipped here (no output diff at this
        // depth: the trace flag is host-off in the twins).
        self.check_grammar_variable_declaration(node);
        self.check_variable_like_declaration(node);
    }

    /// Go: `func (c *Checker) checkBindingElement(node)` (checker.go:4399).
    pub(crate) fn check_binding_element(&mut self, node: NodeId) {
        self.check_grammar_binding_element(node);
        self.check_variable_like_declaration(node);
    }

    /// Go: `func (c *Checker) checkMissingDeclaration(node)` (checker.go:5932).
    pub(crate) fn check_missing_declaration(&mut self, node: NodeId) {
        self.check_decorators(node);
    }

    /// Check variable, parameter, or property declaration — Go:
    /// `func (c *Checker) checkVariableLikeDeclaration(node)` (checker.go:5977,
    /// ~173 lines).
    pub(crate) fn check_variable_like_declaration(&mut self, node: NodeId) {
        self.check_decorators(node);
        let Some(name) = self.store().node(node).name() else {
            // Missing array binding elements have no name
            return;
        };
        let type_node = crate::ast_shims::type_of_node(self.store(), node);
        let initializer = crate::ast_shims::initializer_of(self.store(), node);
        if !is_binding_element(self.store(), node) {
            self.check_source_element(type_node);
        }
        // For a computed property, just check the initializer and exit.
        if self.store().node(name).kind == Kind::ComputedPropertyName {
            self.check_grammar_computed_property_name_stub(name);
            if let Some(initializer) = initializer {
                self.check_expression_cached(initializer);
            }
        }
        if is_binding_element(self.store(), node) {
            let prop_name = self
                .store()
                .node(node)
                .as_binding_element()
                .expect("BindingElement data")
                .property_name;
            if prop_name
                .is_some_and(|p| self.store().node(p).kind == Kind::PrivateIdentifier)
            {
                self.grammar_error_on_node(
                    prop_name.expect("checked"),
                    &diag::PRIVATE_IDENTIFIERS_CANNOT_BE_USED_IN_DESTRUCTURING_PATTERNS,
                    &[],
                );
            }
            if prop_name.is_some_and(|p| is_identifier(self.store().node(p)))
                && is_identifier(self.store().node(name))
                && u::is_part_of_parameter_declaration(self.store(), node)
                && u::get_containing_function(self.store(), node)
                    .and_then(|f| function_body_of(self.store(), f))
                    .is_none()
            {
                // type F = ({a: string}) => void;
                //               ^^^^^^
                // variable renaming in function type notation is
                // confusing, so we forbid it even if noUnusedLocals is
                // not enabled
                self.renamed_binding_elements_in_types.push(node);
                return;
            }
            let parent = self.store().node(node).parent.get();
            if self.store().node(parent).kind == Kind::ObjectBindingPattern
                && has_dot_dot_dot_token(self.store(), node)
                && self.language_version < Feature::ObjectSpreadRest.minimum_target()
            {
                self.check_external_emit_helpers_stub(node, ExternalEmitHelpers::REST);
            }
            // check computed properties inside property names of binding elements
            if prop_name
                .is_some_and(|p| self.store().node(p).kind == Kind::ComputedPropertyName)
            {
                self.check_grammar_computed_property_name_stub(prop_name.expect("checked"));
            }
            // check private/protected variable access
            let grandparent = self.store().node(parent).parent.get();
            let parent_check_mode = if has_dot_dot_dot_token(self.store(), node) {
                CheckMode::REST_BINDING_ELEMENT
            } else {
                CheckMode::CHECK_MODE_NORMAL
            };
            let parent_type = self.get_type_for_binding_element_parent(grandparent, parent_check_mode);
            let prop_name_name = self
                .store()
                .node(node)
                .as_binding_element()
                .expect("BindingElement data")
                .property_name
                .or(Some(name));
            if let Some(parent_type) = parent_type {
                if !prop_name_name.is_some_and(|p| is_binding_pattern(self.store(), p)) {
                    let expr_type = self.get_literal_type_from_property_name(prop_name_name.expect("checked"));
                    if u::is_type_usable_as_property_name(self.type_(expr_type)) {
                        let name_text = u::get_property_name_from_type(self.type_(expr_type));
                        if let Some(property) = self.get_property_of_type(parent_type, &name_text) {
                            // TODO(port M6 S4): markPropertyAsReferenced +
                            // checkPropertyAccessibility (the expando/access
                            // family); permissive skip.
                            let _ = property;
                        }
                    }
                }
            }
        }
        // For a binding pattern, check contained binding elements
        if is_binding_pattern(self.store(), name) {
            let elements = binding_pattern_elements(self.store(), name);
            self.check_source_elements(&elements);
        }
        // For a parameter declaration with an initializer, error and exit if
        // the containing function doesn't have a body
        if initializer.is_some() && u::is_part_of_parameter_declaration(self.store(), node) {
            let containing_function = u::get_containing_function(self.store(), node);
            let body = containing_function.and_then(|f| function_body_of(self.store(), f));
            if body.is_none() {
                self.error(
                    node,
                    &diag::A_PARAMETER_INITIALIZER_IS_ONLY_ALLOWED_IN_A_FUNCTION_OR_CONSTRUCTOR_IMPLEMENTATION,
                    &[],
                );
                return;
            }
        }
        // For a binding pattern, validate the initializer and exit
        if is_binding_pattern(self.store(), name) {
            if u::is_in_ambient_or_type_node(self.store(), node) {
                return;
            }
            let grandparent = {
                let parent = self.store().node(node).parent.get();
                self.store().node(parent).parent.get()
            };
            let need_check_initializer =
                initializer.is_some() && self.store().node(grandparent).kind != Kind::ForInStatement;
            let elements = binding_pattern_elements(self.store(), name);
            let need_check_widened_type =
                !elements.iter().any(|&e| self.store().node(e).name().is_some());
            if need_check_initializer || need_check_widened_type {
                // Don't validate for-in initializer as it is already an error
                let widened_type = self
                    .get_widened_type_for_variable_like_declaration(node, false);
                if need_check_initializer {
                    let initializer = initializer.expect("checked");
                    let initializer_type = self.check_expression_cached(initializer);
                    if self.strict_null_checks && need_check_widened_type {
                        self.check_non_null_non_void_type(initializer_type, node);
                    } else {
                        let widened_for_init = self
                            .get_widened_type_for_variable_like_declaration(node, false);
                        self.check_type_assignable_to_and_optionally_elaborate(
                            initializer_type,
                            widened_for_init,
                            Some(node),
                            Some(initializer),
                            None,
                            None,
                        );
                    }
                }
                // check the binding pattern with empty elements
                if need_check_widened_type {
                    if self.store().node(name).kind == Kind::ArrayBindingPattern {
                        let undefined_type = self.undefined_type();
                        self.check_iterated_type_or_element_type(
                            IterationUse::DESTRUCTURING,
                            widened_type,
                            undefined_type,
                            Some(node),
                        );
                    } else if self.strict_null_checks {
                        self.check_non_null_non_void_type(widened_type, node);
                    }
                }
            }
            return;
        }
        // For a commonjs `const x = require`, validate the alias and exit
        let symbol = self.get_symbol_of_declaration(node);
        let Some(symbol) = symbol else { return };
        let flags = self.program.symbol(symbol).flags;
        if flags.intersects(SymbolFlags::ALIAS)
            && u::is_variable_declaration_initialized_to_require(self.store(), node)
        {
            self.check_alias_symbol(node);
            return;
        }
        if self.store().node(name).kind == Kind::BigIntLiteral {
            self.error(
                name,
                &diag::A_BIGINT_LITERAL_CANNOT_BE_USED_AS_A_PROPERTY_NAME,
                &[],
            );
        }
        let symbol_type = self.get_type_of_symbol(symbol);
        let t = self.convert_auto_to_any(symbol_type);
        if self
            .program
            .symbol(symbol)
            .value_declaration
            .is_some_and(|vd| vd == node)
        {
            // Node is the primary declaration of the symbol, just validate
            // the initializer. Don't validate for-in initializer (already
            // an error).
            if let Some(initializer) = initializer {
                let grandparent = {
                    let parent = self.store().node(node).parent.get();
                    self.store().node(parent).parent.get()
                };
                if self.store().node(grandparent).kind != Kind::ForInStatement {
                    let initializer_type = self.check_expression_cached(initializer);
                    self.check_type_assignable_to_and_optionally_elaborate(
                        initializer_type,
                        t,
                        Some(node),
                        Some(initializer),
                        None,
                        None,
                    );
                    // The using/await-using disposable checks need S7's
                    // union construction + the global Disposable types;
                    // with the permissive global sentinels the gate reads
                    // absent, exactly like a program without the lib.
                    // TODO(port M6 S7): getUnionType + getGlobalDisposableType.
                }
            }
            let declarations = self.program.symbol(symbol).declarations.clone();
            // Go: error when any other variable-like declaration of the
            // symbol has different modifiers.
            if declarations.len() > 1 {
                let has_different = declarations.iter().any(|&d| {
                    d != node
                        && u::is_variable_like_declaration(self.store(), d)
                        && !self.are_declaration_flags_identical(d, node)
                });
                if has_different {
                    let name_text =
                        tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
                    self.error(
                        name,
                        &diag::ALL_DECLARATIONS_OF_0_MUST_HAVE_IDENTICAL_MODIFIERS,
                        &[name_text],
                    );
                }
            }
        } else {
            // Node is a secondary declaration, check that type is identical
            // to primary declaration and check that initializer is
            // consistent with type associated with the node.
            let widened = self.get_widened_type_for_variable_like_declaration(node, false);
            let declaration_type = self.convert_auto_to_any(widened);
            if !relater::is_error_type(self, t)
                && !relater::is_error_type(self, declaration_type)
                && !self.is_type_identical_to(t, declaration_type)
                && !flags.intersects(SymbolFlags::ASSIGNMENT)
            {
                self.error_next_variable_or_property_declaration_must_have_same_type(
                    self.program.symbol(symbol).value_declaration,
                    t,
                    node,
                    declaration_type,
                );
            }
            if let Some(initializer) = initializer {
                let initializer_type = self.check_expression_cached(initializer);
                self.check_type_assignable_to_and_optionally_elaborate(
                    initializer_type,
                    declaration_type,
                    Some(node),
                    Some(initializer),
                    None,
                    None,
                );
            }
            if self
                .program
                .symbol(symbol)
                .value_declaration
                .is_some_and(|vd| !self.are_declaration_flags_identical(node, vd))
            {
                let name_text =
                    tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
                self.error(
                    name,
                    &diag::ALL_DECLARATIONS_OF_0_MUST_HAVE_IDENTICAL_MODIFIERS,
                    &[name_text],
                );
            }
        }
        if self.store().node(node).kind != Kind::PropertyDeclaration
            && self.store().node(node).kind != Kind::PropertySignature
        {
            // We know we don't have a binding pattern or computed name here
            self.check_exports_on_merged_declarations(node);
            if self.store().node(node).kind == Kind::VariableDeclaration
                || is_binding_element(self.store(), node)
            {
                self.check_var_declared_names_not_shadowed(node);
            }
            self.check_collisions_for_declaration_name_stub(node, self.store().node(node).name());
        }
    }

    /// Go: `func (c *Checker) convertAutoToAny(t *Type) *Type`.
    pub(crate) fn convert_auto_to_any(&mut self, t: TypeId) -> TypeId {
        if t == self.auto_type() {
            self.any_type()
        } else {
            t
        }
    }

    /// Go: `func (c *Checker) errorNextVariableOrPropertyDeclarationMustHaveSameType(...)`
    /// (checker.go:6154).
    pub(crate) fn error_next_variable_or_property_declaration_must_have_same_type(
        &mut self,
        first_declaration: Option<NodeId>,
        first_type: TypeId,
        next_declaration: NodeId,
        next_type: TypeId,
    ) {
        let next_declaration_name = tsc_binder::ast_shims::get_name_of_declaration(
            self.store(),
            next_declaration,
        );
        let message = if matches!(
            self.store().node(next_declaration).kind,
            Kind::PropertyDeclaration | Kind::PropertySignature
        ) {
            &diag::SUBSEQUENT_PROPERTY_DECLARATIONS_MUST_HAVE_THE_SAME_TYPE_PROPERTY_0_MUST_BE_OF_TYPE_1_BUT_HERE_HAS_TYPE_2
        } else {
            &diag::SUBSEQUENT_VARIABLE_DECLARATIONS_MUST_HAVE_THE_SAME_TYPE_VARIABLE_0_MUST_BE_OF_TYPE_1_BUT_HERE_HAS_TYPE_2
        };
        let decl_name = tsc_scanner::utilities::declaration_name_to_string(
            self.store(),
            next_declaration_name,
        );
        let first_text = self.type_to_string(first_type);
        let next_text = self.type_to_string(next_type);
        if let Some(name_node) = next_declaration_name {
            self.error(name_node, message, &[decl_name, first_text, next_text]);
        }
        // Go: the X_0_was_also_declared_here related info (the related-info
        // factory is the M7 diagnostics wave).
        let _ = first_declaration;
    }

    /// Go: `func (c *Checker) checkVarDeclaredNamesNotShadowed(node)` (checker.go:6181).
    pub(crate) fn check_var_declared_names_not_shadowed(&mut self, node: NodeId) {
        // skip block-scoped variables and parameters
        let flags = self.get_combined_node_flags_cached(node);
        if flags.intersects(NF::BLOCK_SCOPED)
            || u::is_part_of_parameter_declaration(self.store(), node)
        {
            return;
        }
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        let name = self.store().node(node).name();
        if !self
            .program
            .symbol(symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
        {
            return;
        }
        let Some(name) = name else { return };
        if !is_identifier(self.store().node(name)) {
            panic!("Identifier expected");
        }
        // TODO(port M6 S6): resolveName — the local-declaration probe needs
        // S6's name resolution; permissive None skips the shadow diagnostic.
        let _ = symbol;
    }

    /// Go: `func (c *Checker) areDeclarationFlagsIdentical(left, right) bool`
    /// (checker.go:7076).
    pub(crate) fn are_declaration_flags_identical(&mut self, left: NodeId, right: NodeId) -> bool {
        if (u::is_parameter_declaration(self.store(), left)
            && self.store().node(right).kind == Kind::VariableDeclaration)
            || (self.store().node(left).kind == Kind::VariableDeclaration
                && u::is_parameter_declaration(self.store(), right))
        {
            // Differences in optionality between parameters and variables are allowed.
            return true;
        }
        if u::is_optional_declaration(self.store(), left)
            != u::is_optional_declaration(self.store(), right)
        {
            return false;
        }
        let interesting_flags = ModifierFlags::PRIVATE
            .union(ModifierFlags::PROTECTED)
            .union(ModifierFlags::ASYNC)
            .union(ModifierFlags::ABSTRACT)
            .union(ModifierFlags::READONLY)
            .union(ModifierFlags::STATIC);
        u::get_selected_modifier_flags(self.store().node(left), interesting_flags)
            == u::get_selected_modifier_flags(self.store().node(right), interesting_flags)
    }

    /// Go: `func (c *Checker) checkDecorators(node)` (checker.go:6206) — the
    /// real gate shape with S6's getResolvedSignature stubbed permissive
    /// (checkDecorator's signature resolution reports nothing).
    pub(crate) fn check_decorators(&mut self, node: NodeId) {
        // TODO(port M6 W3): the CanHaveDecorators/NodeCanBeDecorated grammar
        // gates + the emit-helper variants (S10) + markLinkedReferences
        // (ReferenceHintDecorator, symboltracker.rs). Permissive: the
        // decorator walk below keeps the per-declaration recursion.
        for modifier in modifier_nodes_of(self.store(), node) {
            if self.store().node(modifier).kind == Kind::Decorator {
                self.check_decorator(modifier);
            }
        }
    }

    /// Go: `func (c *Checker) checkDecorator(node)` (checker.go:6245) —
    /// TODO(port M6 S6): getResolvedSignature; permissive no-op body.
    pub(crate) fn check_decorator(&mut self, node: NodeId) {
        self.check_grammar_decorator(node);
        // The decorator-call signature resolution + return-type check is
        // S6's getResolvedSignature + S8's getReturnTypeOfSignature seam;
        // permissive until those land (the twins' --generateTrace path
        // never emits decorator diagnostics at parse+bind depth).
        let _ = node;
    }

    /// Go: `func (c *Checker) checkAliasSymbol(node)` (checker.go:6940) — the
    /// REAL body landed with batch 6 (the alias family); see the batch-6
    /// section below.
    ///
    /// TODO(port M6 S4): `checkCollisionsForDeclarationName` — the
    /// identifier-collision family; permissive no-op.
    pub(crate) fn check_collisions_for_declaration_name_stub(
        &mut self,
        node: NodeId,
        name: Option<NodeId>,
    ) {
        let _ = (node, name);
    }

    /// TODO(port M6 S6): `getExportsOfModule` — the module-exports resolution;
    /// permissive empty.
    #[allow(dead_code)] // wired with the S6 import/export batch
    pub(crate) fn get_exports_of_module_stub(&mut self, symbol: SymbolId) -> Vec<(String, SymbolId)> {
        let _ = symbol;
        Vec::new()
    }

}

/// Go: the binding pattern's `Elements()` (Array/ObjectBindingPattern).
pub(crate) fn binding_pattern_elements(store: &dyn NodeStore, pattern: NodeId) -> Vec<NodeId> {
    let n = store.node(pattern);
    let list = match n.kind {
        Kind::ArrayBindingPattern | Kind::ObjectBindingPattern => {
            n.as_binding_pattern().expect("BindingPattern data").elements.clone()
        }
        _ => None,
    };
    list.map(|l| l.nodes.to_vec()).unwrap_or_default()
}

/// Go: `node.ModifierNodes()` (the modifier list's nodes; decorators ride in
/// the same list).
pub(crate) fn modifier_nodes_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    store
        .node(node)
        .modifiers()
        .map(|m| m.nodes.to_vec())
        .unwrap_or_default()
}

// ────────────────────────────────────────────────────────────────────────────
// The unused-identifier family (Go checker.go:7226-7516)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type UnusedKind int32` (checker.go:7262, iota order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub(crate) enum UnusedKind {
    Local = 0,
    Parameter = 1,
}

impl Checker {
    /// Go: `func (c *Checker) registerForUnusedIdentifiersCheck(node)` (checker.go:7226).
    pub(crate) fn register_for_unused_identifiers_check(&mut self, node: NodeId) {
        let source_file = self.get_source_file_node_of(node);
        self.source_file_links
            .get(source_file)
            .identifier_check_nodes
            .push(node);
    }

    /// Go: `func (c *Checker) checkUnusedIdentifiers(potentiallyUnusedIdentifiers)` (checker.go:7230).
    pub(crate) fn check_unused_identifiers(&mut self, potentially_unused_identifiers: &[NodeId]) {
        for &node in potentially_unused_identifiers {
            match self.store().node(node).kind {
                Kind::ClassDeclaration | Kind::ClassExpression => {
                    self.check_unused_class_members(node);
                    self.check_unused_type_parameters(node);
                }
                Kind::SourceFile
                | Kind::ModuleDeclaration
                | Kind::Block
                | Kind::CaseBlock
                | Kind::ForStatement
                | Kind::ForInStatement
                | Kind::ForOfStatement
                | Kind::ClassStaticBlockDeclaration => {
                    self.check_unused_locals_and_parameters(node);
                }
                Kind::Constructor
                | Kind::FunctionExpression
                | Kind::FunctionDeclaration
                | Kind::ArrowFunction
                | Kind::MethodDeclaration
                | Kind::GetAccessor
                | Kind::SetAccessor => {
                    // Only report unused parameters on the implementation,
                    // not overloads.
                    if function_body_of(self.store(), node).is_some() {
                        self.check_unused_locals_and_parameters(node);
                    }
                    self.check_unused_type_parameters(node);
                }
                Kind::MethodSignature
                | Kind::CallSignature
                | Kind::ConstructSignature
                | Kind::FunctionType
                | Kind::ConstructorType
                | Kind::TypeAliasDeclaration
                | Kind::JSTypeAliasDeclaration
                | Kind::InterfaceDeclaration => {
                    self.check_unused_type_parameters(node);
                }
                Kind::InferType => {
                    self.check_unused_infer_type_parameter(node);
                }
                _ => panic!("Unhandled case in checkUnusedIdentifiers"),
            }
        }
    }

    /// Go: `func (c *Checker) isReferenced(symbol) bool` (checker.go:7259).
    pub(crate) fn is_referenced(&mut self, symbol: SymbolId) -> bool {
        !self
            .symbol_reference_links
            .get(symbol)
            .reference_kinds
            .is_empty()
    }

    /// Go: `func (c *Checker) reportUnusedVariable(location, diagnostic)` (checker.go:7265).
    pub(crate) fn report_unused_variable(
        &mut self,
        location: NodeId,
        diagnostic: tsc_ast::Diagnostic,
    ) {
        let mut location = location;
        while is_binding_element(self.store(), location)
            || is_binding_pattern(self.store(), location)
        {
            location = self.store().node(location).parent.get();
        }
        let kind = if u::is_parameter_declaration(self.store(), location) {
            UnusedKind::Parameter
        } else {
            UnusedKind::Local
        };
        self.report_unused(location, kind, diagnostic);
    }

    /// Go: `func (c *Checker) reportUnused(location, kind, diagnostic)` (checker.go:7271).
    pub(crate) fn report_unused(
        &mut self,
        location: NodeId,
        kind: UnusedKind,
        diagnostic: tsc_ast::Diagnostic,
    ) {
        let flags = self.store().node(location).flags;
        if !flags.intersects(NF::AMBIENT.union(NF::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR)) {
            let is_error = self.unused_is_error(kind);
            if is_error {
                self.add_diagnostic(diagnostic);
            } else {
                let mut suggestion = diagnostic;
                suggestion.set_category(Category::Suggestion);
                self.add_suggestion_diagnostic(suggestion);
            }
        }
    }

    /// Go: `func (c *Checker) unusedIsError(kind) bool` (checker.go:7279).
    pub(crate) fn unused_is_error(&self, kind: UnusedKind) -> bool {
        match kind {
            UnusedKind::Local => self.compiler_options.no_unused_locals == Tristate::TSTrue,
            UnusedKind::Parameter => {
                self.compiler_options.no_unused_parameters == Tristate::TSTrue
            }
        }
    }

    /// Go: `func (c *Checker) checkUnusedClassMembers(node)` (checker.go:7288).
    pub(crate) fn check_unused_class_members(&mut self, node: NodeId) {
        for member in members_of(self.store(), node) {
            match self.store().node(member).kind {
                Kind::MethodDeclaration
                | Kind::PropertyDeclaration
                | Kind::GetAccessor
                | Kind::SetAccessor => {
                    // If the symbol has a get accessor, we would already have
                    // reported an error on the getter.
                    if self.store().node(member).kind == Kind::SetAccessor {
                        if let Some(symbol) = self.get_symbol_of_declaration(member) {
                            if self
                                .program
                                .symbol(symbol)
                                .flags
                                .intersects(SymbolFlags::GET_ACCESSOR)
                            {
                                continue;
                            }
                        }
                    }
                    let Some(symbol) = self.get_symbol_of_declaration(member) else {
                        continue;
                    };
                    let has_private = crate::ast_shims::has_modifier(
                        self.store().node(member),
                        ModifierFlags::PRIVATE,
                    ) || self
                        .store()
                        .node(member)
                        .name()
                        .is_some_and(|n| self.store().node(n).kind == Kind::PrivateIdentifier);
                    if !self.is_referenced(symbol)
                        && has_private
                        && !self.store().node(member).flags.intersects(NF::AMBIENT)
                    {
                        let symbol_name = relater::symbol_to_string(self, symbol);
                        let name = self.store().node(member).name();
                        if let Some(name) = name {
                            let d = u::new_diagnostic_for_node(
                                self.store(),
                                name,
                                &diag::X_0_IS_DECLARED_BUT_ITS_VALUE_IS_NEVER_READ,
                                vec![symbol_name],
                            );
                            self.report_unused(member, UnusedKind::Local, d);
                        }
                    }
                }
                Kind::Constructor => {
                    let parameters = self
                        .store()
                        .node(member)
                        .as_constructor_declaration()
                        .expect("data")
                        .parameters
                        .as_ref()
                        .map(|l| l.nodes.to_vec())
                        .unwrap_or_default();
                    for parameter in parameters {
                        let Some(symbol) = self.get_symbol_of_declaration(parameter) else {
                            continue;
                        };
                        if !self.is_referenced(symbol)
                            && crate::ast_shims::has_modifier(
                                self.store().node(parameter),
                                ModifierFlags::PRIVATE,
                            )
                        {
                            let name = self.store().node(parameter).name();
                            if let Some(name) = name {
                                let symbol_name =
                                    self.program.symbol(symbol).name.clone();
                                let d = u::new_diagnostic_for_node(
                                    self.store(),
                                    name,
                                    &diag::PROPERTY_0_IS_DECLARED_BUT_ITS_VALUE_IS_NEVER_READ,
                                    vec![symbol_name],
                                );
                                self.report_unused(parameter, UnusedKind::Local, d);
                            }
                        }
                    }
                }
                Kind::IndexSignature
                | Kind::SemicolonClassElement
                | Kind::ClassStaticBlockDeclaration
                | Kind::JSTypeAliasDeclaration => {
                    // Can't be private
                }
                _ => panic!("Unhandled case in checkUnusedClassMembers"),
            }
        }
    }

    /// Go: `func (c *Checker) checkUnusedLocalsAndParameters(node)` (checker.go:7316).
    pub(crate) fn check_unused_locals_and_parameters(&mut self, node: NodeId) {
        // Go walks node.Locals() — the binder-set locals table on the
        // payload (§5.1: the Rust payload keeps the same table).
        let locals: Vec<(String, SymbolId)> = locals_of_node(self.store(), node);
        let mut variable_parents: Vec<NodeId> = Vec::new();
        let mut import_clauses: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
        for (_name, local) in locals {
            let local_flags = self.program.symbol(local).flags;
            let reference_kinds = self.symbol_reference_links.get(local).reference_kinds;
            let export_symbol = self.program.symbol(local).export_symbol;
            if (local_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                && (!local_flags.intersects(SymbolFlags::VARIABLE)
                    || reference_kinds.intersects(SymbolFlags::VARIABLE)))
                || (!local_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                    && (!reference_kinds.is_empty()
                        || export_symbol.is_some()
                        || local_flags.intersects(SymbolFlags::MODULE_EXPORTS)))
            {
                continue;
            }
            let declarations = self.program.symbol(local).declarations.clone();
            for declaration in declarations {
                let kind = self.store().node(declaration).kind;
                if matches!(
                    kind,
                    Kind::VariableDeclaration | Kind::Parameter | Kind::BindingElement
                ) {
                    let root = tsc_binder::ast_shims::get_root_declaration(self.store(), declaration);
                    let parent = self.store().node(root).parent.get();
                    if !variable_parents.contains(&parent) {
                        variable_parents.push(parent);
                    }
                } else if matches!(
                    kind,
                    Kind::ImportClause | Kind::ImportSpecifier | Kind::NamespaceImport
                ) {
                    let name = self.store().node(declaration).name();
                    if !name.is_some_and(|n| is_identifier_that_starts_with_underscore(self.store(), n)) {
                        let import_clause = import_clause_from_imported(self.store(), declaration);
                        match import_clauses.iter_mut().find(|(ic, _)| *ic == import_clause) {
                            Some((_, unuseds)) => unuseds.push(declaration),
                            None => import_clauses.push((import_clause, vec![declaration])),
                        }
                    }
                } else if kind != Kind::TypeParameter
                    && !tsc_binder::ast_shims::is_ambient_module(self.store(), declaration)
                {
                    let symbol_name = self.program.symbol(local).name.clone();
                    self.report_unused_local(declaration, symbol_name);
                }
            }
        }
        // Go iterates a collections.Set (insertion-ordered unique keys).
        for declaration in variable_parents {
            if self.store().node(declaration).kind == Kind::VariableDeclarationList {
                self.report_unused_variables(declaration);
            } else {
                self.report_unused_parameters(declaration);
            }
        }
        for (declaration, unuseds) in import_clauses {
            self.report_unused_imports(declaration, &unuseds);
        }
    }

    /// Go: `func (c *Checker) reportUnusedLocal(node, name)` (checker.go:7365).
    pub(crate) fn report_unused_local(&mut self, node: NodeId, name: String) {
        let message = if u::is_type_declaration(self.store(), node) {
            &diag::X_0_IS_DECLARED_BUT_NEVER_USED
        } else {
            &diag::X_0_IS_DECLARED_BUT_ITS_VALUE_IS_NEVER_READ
        };
        let error_node = self.store().node(node).name().unwrap_or(node);
        let d = u::new_diagnostic_for_node(self.store(), error_node, message, vec![name]);
        self.report_unused(node, UnusedKind::Local, d);
    }

    /// Go: `func (c *Checker) reportUnusedVariables(node)` (checker.go:7370).
    pub(crate) fn report_unused_variables(&mut self, node: NodeId) {
        let declarations = self
            .store()
            .node(node)
            .as_variable_declaration_list()
            .expect("data")
            .declarations
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        if declarations.len() > 1
            && declarations
                .iter()
                .all(|&d| self.is_unreferenced_variable_declaration(d))
        {
            let d = u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::ALL_VARIABLES_ARE_UNUSED,
                vec![],
            );
            self.report_unused_variable(node, d);
        } else {
            self.report_unused_variable_declarations(&declarations);
        }
    }

    /// Go: `func (c *Checker) reportUnusedParameters(node)` (checker.go:7378).
    pub(crate) fn report_unused_parameters(&mut self, node: NodeId) {
        let parameters = parameters_of(self.store(), node);
        self.report_unused_variable_declarations(&parameters);
    }

    /// Go: `func (c *Checker) reportUnusedBindingElements(node)` (checker.go:7382).
    pub(crate) fn report_unused_binding_elements(&mut self, node: NodeId) {
        let declarations = binding_pattern_elements(self.store(), node);
        if declarations.len() > 1
            && declarations
                .iter()
                .all(|&d| self.is_unreferenced_variable_declaration(d))
        {
            let d = u::new_diagnostic_for_node(
                self.store(),
                node,
                &diag::ALL_DESTRUCTURED_ELEMENTS_ARE_UNUSED,
                vec![],
            );
            self.report_unused_variable(node, d);
        } else {
            self.report_unused_variable_declarations(&declarations);
        }
    }

    /// Go: `func (c *Checker) reportUnusedVariableDeclarations(declarations)` (checker.go:7390).
    pub(crate) fn report_unused_variable_declarations(&mut self, declarations: &[NodeId]) {
        for &declaration in declarations {
            let Some(name) = self.store().node(declaration).name() else {
                continue;
            };
            if u::is_parameter_property_declaration(
                self.store(),
                declaration,
                self.store().node(declaration).parent.get(),
            ) || u::is_this_parameter(self.store(), declaration)
            {
                continue;
            }
            if is_binding_pattern(self.store(), name) {
                self.report_unused_binding_elements(name);
            } else if self.is_unreferenced_variable_declaration(declaration) {
                let name_text = tsc_ast::node_text(self.store(), name).to_string();
                let d = u::new_diagnostic_for_node(
                    self.store(),
                    name,
                    &diag::X_0_IS_DECLARED_BUT_ITS_VALUE_IS_NEVER_READ,
                    vec![name_text],
                );
                self.report_unused_variable(declaration, d);
            }
        }
    }

    /// Go: `func (c *Checker) isUnreferencedVariableDeclaration(node) bool` (checker.go:7404).
    pub(crate) fn is_unreferenced_variable_declaration(&mut self, node: NodeId) -> bool {
        let Some(name) = self.store().node(node).name() else {
            return true;
        };
        if is_binding_pattern(self.store(), name) {
            return binding_pattern_elements(self.store(), name)
                .iter()
                .all(|&e| self.is_unreferenced_variable_declaration(e));
        }
        if let Some(symbol) = self.get_symbol_of_declaration(node) {
            if self
                .symbol_reference_links
                .get(symbol)
                .reference_kinds
                .intersects(SymbolFlags::VARIABLE)
            {
                return false;
            }
        }
        if is_binding_element(self.store(), node)
            && self.store().node(self.store().node(node).parent.get()).kind
                == Kind::ObjectBindingPattern
        {
            // In `{ a, ...b }, `a` is considered used since it removes a
            // property from `b`. `b` may still be unused though.
            let parent = self.store().node(node).parent.get();
            let elements = binding_pattern_elements(self.store(), parent);
            if let Some(&last) = elements.last() {
                if node != last && has_dot_dot_dot_token(self.store(), last) {
                    return false;
                }
            }
        }
        if ((u::is_parameter_declaration(self.store(), node)
            || (self.store().node(node).kind == Kind::VariableDeclaration
                && (self
                    .store()
                    .node(self.store().node(node).parent.get())
                    .kind
                    == Kind::ForInStatement
                    || self
                        .store()
                        .node(self.store().node(node).parent.get())
                        .kind
                        == Kind::ForOfStatement))
            || self.get_combined_node_flags_cached(node).intersects(NF::USING))
            || (is_binding_element(self.store(), node) && {
                let parent = self.store().node(node).parent.get();
                self.store().node(parent).kind == Kind::ObjectBindingPattern
                    && self
                        .store()
                        .node(node)
                        .as_binding_element()
                        .expect("data")
                        .property_name
                        .is_none()
            }))
            && is_identifier_that_starts_with_underscore(self.store(), name) {
                return false;
            }
        true
    }

    /// Go: `func (c *Checker) reportUnusedImports(node, unuseds)` (checker.go:7442).
    pub(crate) fn report_unused_imports(&mut self, node: NodeId, unuseds: &[NodeId]) {
        let mut declaration_count = usize::from(self.store().node(node).name().is_some());
        let named_bindings = self
            .store()
            .node(node)
            .as_import_clause()
            .expect("data")
            .named_bindings;
        if let Some(named_bindings) = named_bindings {
            if self.store().node(named_bindings).kind == Kind::NamespaceImport {
                declaration_count += 1;
            } else {
                declaration_count += binding_pattern_elements(self.store(), named_bindings).len();
            }
        }
        if declaration_count > 1 && declaration_count == unuseds.len() {
            let parent = self.store().node(node).parent.get();
            let d = u::new_diagnostic_for_node(
                self.store(),
                parent,
                &diag::ALL_IMPORTS_IN_IMPORT_DECLARATION_ARE_UNUSED,
                vec![],
            );
            self.report_unused(node, UnusedKind::Local, d);
        } else {
            for &unused in unuseds {
                let name_text = self
                    .store()
                    .node(unused)
                    .name()
                    .map(|n| tsc_ast::node_text(self.store(), n).to_string())
                    .unwrap_or_default();
                self.report_unused_local(unused, name_text);
            }
        }
    }

    /// Go: `func (c *Checker) checkUnusedInferTypeParameter(node)` (checker.go:7481).
    pub(crate) fn check_unused_infer_type_parameter(&mut self, node: NodeId) {
        let type_parameter = self
            .store()
            .node(node)
            .as_infer_type_node()
            .expect("data")
            .type_parameter;
        if self.is_unreferenced_type_parameter(type_parameter) {
            let name = self.store().node(type_parameter).name();
            if let Some(name) = name {
                let name_text = tsc_ast::node_text(self.store(), name).to_string();
                let d = u::new_diagnostic_for_node(
                    self.store(),
                    name,
                    &diag::X_0_IS_DECLARED_BUT_NEVER_USED,
                    vec![name_text],
                );
                self.report_unused(node, UnusedKind::Parameter, d);
            }
        }
    }

    /// Go: `func (c *Checker) checkUnusedTypeParameters(node)` (checker.go:7490).
    pub(crate) fn check_unused_type_parameters(&mut self, node: NodeId) {
        // Go: allDeclarationsInSameSourceFile(getSymbolOfDeclaration(node)).
        if let Some(symbol) = self.get_symbol_of_declaration(node) {
            if !all_declarations_in_same_source_file(self.store(), symbol) {
                return;
            }
        }
        // Go: node.TypeParameterList() — the syntactic list; the payloads
        // keep the parsed type_parameters list (TypeParameterList is the
        // reparse view; the parsed list is the same nodes at check depth).
        let type_parameter_list = type_parameters_of(self.store(), node);
        if type_parameter_list.is_empty() {
            return;
        }
        if type_parameter_list.len() > 1
            && type_parameter_list
                .iter()
                .all(|&tp| self.is_unreferenced_type_parameter(tp))
        {
            let first = type_parameter_list[0];
            let last = type_parameter_list[type_parameter_list.len() - 1];
            let file = self.get_source_file_node_of(node);
            let loc = tsc_core::text::TextRange::new(
                self.store().node(first).pos(),
                self.store().node(last).end(),
            );
            // Go: ast.NewDiagnostic(file, loc, All_type_parameters_are_unused)
            // — the no-node diagnostic factory; PORT: pinned to the first
            // type parameter's location (the M7 diagnostics wave carries the
            // raw-location factory).
            let d = u::new_diagnostic_for_node(
                self.store(),
                first,
                &diag::ALL_TYPE_PARAMETERS_ARE_UNUSED,
                vec![],
            );
            let _ = (file, loc);
            self.report_unused(node, UnusedKind::Parameter, d);
        } else {
            for type_parameter in type_parameter_list {
                if self.is_unreferenced_type_parameter(type_parameter) {
                    let name = self.store().node(type_parameter).name();
                    if let Some(name) = name {
                        let name_text = tsc_ast::node_text(self.store(), name).to_string();
                        let d = u::new_diagnostic_for_node(
                            self.store(),
                            name,
                            &diag::X_0_IS_DECLARED_BUT_NEVER_USED,
                            vec![name_text],
                        );
                        self.report_unused(node, UnusedKind::Parameter, d);
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) isUnreferencedTypeParameter(typeParameter) bool` (checker.go:7513).
    pub(crate) fn is_unreferenced_type_parameter(&mut self, type_parameter: NodeId) -> bool {
        let Some(symbol) = self.get_symbol_of_declaration(type_parameter) else {
            return false;
        };
        let symbol = self.get_merged_symbol(symbol);
        !self
            .symbol_reference_links
            .get(symbol)
            .reference_kinds
            .intersects(SymbolFlags::TYPE_PARAMETER)
            && self
                .store()
                .node(type_parameter)
                .name()
                .is_some_and(|n| !is_identifier_that_starts_with_underscore(self.store(), n))
    }

    /// Go: `func (c *Checker) checkUnusedRenamedBindingElements()` (checker.go:7518).
    pub(crate) fn check_unused_renamed_binding_elements(&mut self) {
        for node in std::mem::take(&mut self.renamed_binding_elements_in_types) {
            let Some(symbol) = self.get_symbol_of_declaration(node) else {
                continue;
            };
            if self.symbol_reference_links.get(symbol).reference_kinds.is_empty() {
                let wrapping_declaration =
                    crate::ast_shims::walk_up_binding_elements_and_patterns(self.store(), node);
                let name = self.store().node(node).name();
                let property_name = self
                    .store()
                    .node(node)
                    .as_binding_element()
                    .expect("data")
                    .property_name;
                if let Some(name) = name {
                    let name_text =
                        tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
                    let property_text = tsc_scanner::utilities::declaration_name_to_string(
                        self.store(),
                        property_name,
                    );
                    let d = u::new_diagnostic_for_node(
                        self.store(),
                        name,
                        &diag::X_0_IS_AN_UNUSED_RENAMING_OF_1_DID_YOU_INTEND_TO_USE_IT_AS_A_TYPE_ANNOTATION,
                        vec![name_text, property_text],
                    );
                    // Go attaches the "we can only write a type…" related
                    // info when the wrapping parameter lacks a type
                    // annotation (the related-info factory is the M7
                    // diagnostics wave).
                    let _ = wrapping_declaration;
                    self.add_diagnostic(d);
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkExportsOnMergedDeclarations(node)` (checker.go:7110).
    pub(crate) fn check_exports_on_merged_declarations(&mut self, node: NodeId) {
        // If localSymbol is defined on node then node itself is exported -
        // check is required.
        let mut symbol = local_symbol_of(self.store(), node);
        if symbol.is_none() {
            // Local symbol is undefined => this declaration is non-exported.
            // However, symbol might contain other declarations that are exported.
            let Some(s) = self.get_symbol_of_declaration(node) else {
                return;
            };
            if self.program.symbol(s).export_symbol.is_none() {
                // This is a pure local symbol (all declarations are
                // non-exported) - no need to check anything.
                return;
            }
            symbol = Some(s);
        }
        let Some(symbol) = symbol else { return };
        let declarations = self.program.symbol(symbol).declarations.clone();
        if declarations.len() < 2 || !self.merged_exports_checked.insert(symbol) {
            return;
        }
        let mut exported_declaration_spaces = DeclarationSpaces::NONE;
        let mut non_exported_declaration_spaces = DeclarationSpaces::NONE;
        let mut default_exported_declaration_spaces = DeclarationSpaces::NONE;
        for &d in &declarations {
            let declaration_spaces = self.get_declaration_spaces(d);
            let effective_declaration_flags =
                self.get_effective_declaration_flags(d, ModifierFlags::EXPORT.union(ModifierFlags::DEFAULT));
            if effective_declaration_flags.intersects(ModifierFlags::EXPORT) {
                if effective_declaration_flags.intersects(ModifierFlags::DEFAULT) {
                    default_exported_declaration_spaces =
                        default_exported_declaration_spaces.union(declaration_spaces);
                } else {
                    exported_declaration_spaces =
                        exported_declaration_spaces.union(declaration_spaces);
                }
            } else {
                non_exported_declaration_spaces =
                    non_exported_declaration_spaces.union(declaration_spaces);
            }
        }
        // Spaces for anything not declared a 'default export'.
        let non_default_exported_declaration_spaces =
            exported_declaration_spaces.union(non_exported_declaration_spaces);
        let common_declaration_spaces_for_exports_and_locals =
            exported_declaration_spaces & non_exported_declaration_spaces;
        let common_declaration_spaces_for_default_and_non_default =
            default_exported_declaration_spaces & non_default_exported_declaration_spaces;
        if !common_declaration_spaces_for_exports_and_locals.is_empty()
            || !common_declaration_spaces_for_default_and_non_default.is_empty()
        {
            // declaration spaces for exported and non-exported declarations
            // intersect
            for &d in &declarations {
                let declaration_spaces = self.get_declaration_spaces(d);
                let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), d);
                let Some(name) = name else { continue };
                // Only error on the declarations that contributed to the
                // intersecting spaces.
                if !(declaration_spaces & common_declaration_spaces_for_default_and_non_default).is_empty()
                {
                    let name_text = tsc_scanner::utilities::declaration_name_to_string(
                        self.store(),
                        Some(name),
                    );
                    self.error(
                        name,
                        &diag::MERGED_DECLARATION_0_CANNOT_INCLUDE_A_DEFAULT_EXPORT_DECLARATION_CONSIDER_ADDING_A_SEPARATE_EXPORT_DEFAULT_0_DECLARATION_INSTEAD,
                        &[name_text],
                    );
                } else if !(declaration_spaces & common_declaration_spaces_for_exports_and_locals).is_empty()
                {
                    let name_text = tsc_scanner::utilities::declaration_name_to_string(
                        self.store(),
                        Some(name),
                    );
                    self.error(
                        name,
                        &diag::INDIVIDUAL_DECLARATIONS_IN_MERGED_DECLARATION_0_MUST_BE_ALL_EXPORTED_OR_ALL_LOCAL,
                        &[name_text],
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) getDeclarationSpaces(node) DeclarationSpaces`
    /// (checker.go:7171).
    pub(crate) fn get_declaration_spaces(&mut self, node: NodeId) -> DeclarationSpaces {
        match self.store().node(node).kind {
            Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::JSTypeAliasDeclaration
            | Kind::JSDocTypedefTag
            | Kind::JSDocCallbackTag => DeclarationSpaces::EXPORT_TYPE,
            Kind::ModuleDeclaration => {
                if tsc_binder::ast_shims::is_ambient_module(self.store(), node)
                    || tsc_binder::ast_shims::get_module_instance_state(self.store(), node)
                        != tsc_binder::ast_shims::ModuleInstanceState::NonInstantiated
                {
                    DeclarationSpaces::EXPORT_NAMESPACE.union(DeclarationSpaces::EXPORT_VALUE)
                } else {
                    DeclarationSpaces::EXPORT_NAMESPACE
                }
            }
            Kind::ClassDeclaration | Kind::EnumDeclaration | Kind::EnumMember => {
                DeclarationSpaces::EXPORT_TYPE.union(DeclarationSpaces::EXPORT_VALUE)
            }
            Kind::SourceFile => DeclarationSpaces::EXPORT_TYPE
                .union(DeclarationSpaces::EXPORT_VALUE)
                .union(DeclarationSpaces::EXPORT_NAMESPACE),
            Kind::ExportAssignment | Kind::BinaryExpression => {
                let expression = if self.store().node(node).kind == Kind::ExportAssignment {
                    self.store()
                        .node(node)
                        .as_export_assignment()
                        .expect("data")
                        .expression
                } else {
                    self.store()
                        .node(node)
                        .as_binary_expression()
                        .expect("data")
                        .right
                };
                // Export assigned entity name expressions act as aliases and
                // should fall through, otherwise they export values.
                if !u::is_entity_name_expression(self.store(), expression)
                    || self
                        .get_symbol_of_declaration(node)
                        .is_none_or(|s| !self.program.symbol(s).flags.intersects(SymbolFlags::ALIAS))
                {
                    return DeclarationSpaces::EXPORT_VALUE;
                }
                // The below options all declare an Alias, which is allowed
                // to merge with other values within the importing module.
                self.get_declaration_spaces_of_alias(node)
            }
            Kind::ImportEqualsDeclaration | Kind::NamespaceImport | Kind::ImportClause => {
                self.get_declaration_spaces_of_alias(node)
            }
            Kind::VariableDeclaration
            | Kind::BindingElement
            | Kind::FunctionDeclaration
            | Kind::ImportSpecifier => DeclarationSpaces::EXPORT_VALUE,
            Kind::MethodSignature | Kind::PropertySignature => DeclarationSpaces::EXPORT_TYPE,
            _ => panic!(
                "Unhandled case in getDeclarationSpaces: {}",
                self.store().node(node).kind_string()
            ),
        }
    }

    /// Go: getDeclarationSpaces' alias fall-through — the target's
    /// declaration spaces (resolveAlias is S6; permissive the symbol's own
    /// declarations — TODO(port M6 S6)).
    fn get_declaration_spaces_of_alias(&mut self, node: NodeId) -> DeclarationSpaces {
        let result = DeclarationSpaces::NONE;
        // TODO(port M6 S6): resolveAlias — the target declaration spaces;
        // permissive: the alias's own symbol carries none (Go's resolveAlias
        // stub returns the symbol itself, whose declarations re-enter here).
        let _ = node;
        result
    }

    /// Go: `func (c *Checker) checkTypeParameters(typeParameterDeclarations)` (checker.go:7205).
    pub(crate) fn check_type_parameters(&mut self, type_parameter_declarations: &[NodeId]) {
        let mut seen_default = false;
        for (i, &node) in type_parameter_declarations.iter().enumerate() {
            self.check_type_parameter(node);
            let default_type_node = self
                .store()
                .node(node)
                .as_type_parameter_declaration()
                .expect("data")
                .default_type;
            if let Some(default_type_node) = default_type_node {
                seen_default = true;
                self.check_type_parameters_not_referenced(
                    default_type_node,
                    type_parameter_declarations,
                    i,
                );
            } else if seen_default {
                self.error(
                    node,
                    &diag::REQUIRED_TYPE_PARAMETERS_MAY_NOT_FOLLOW_OPTIONAL_TYPE_PARAMETERS,
                    &[],
                );
            }
            for &other in type_parameter_declarations.iter().take(i) {
                if node_symbol_of(self.store(), other)
                    .is_some_and(|s| node_symbol_of(self.store(), node) == Some(s))
                {
                    let name = self.store().node(node).name();
                    if let Some(name) = name {
                        let name_text = tsc_scanner::utilities::declaration_name_to_string(
                            self.store(),
                            Some(name),
                        );
                        self.error(name, &diag::DUPLICATE_IDENTIFIER_0, &[name_text]);
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkTypeParametersNotReferenced(root, typeParameters, index)`
    /// (checker.go:7238).
    pub(crate) fn check_type_parameters_not_referenced(
        &mut self,
        root: NodeId,
        type_parameters: &[NodeId],
        index: usize,
    ) {
        // Go's visitor: type references whose type-parameter target appears
        // at/after `index` error. TODO(port M6 S5): getTypeFromTypeReference
        // — the reference resolution is S5's; permissive skip.
        let _ = (root, type_parameters, index);
    }

    /// Go: `func (c *Checker) checkTypeNameIsReserved(name, message)` (checker.go:7099).
    pub(crate) fn check_type_name_is_reserved(&mut self, name: NodeId, message: &'static Message) {
        // TS 1.0 spec (April 2014): 3.6.1 — the predefined type keywords are
        // reserved and cannot be used as names of user defined types.
        let text = tsc_ast::node_text(self.store(), name).to_string();
        if matches!(
            text.as_str(),
            "any" | "unknown" | "never" | "number" | "bigint" | "boolean" | "string" | "symbol"
                | "void" | "object" | "undefined"
        ) {
            self.error(name, message, &[text]);
        }
    }
}

// Go: `DeclarationSpaces` (checker.go — the bit set).
define_flags!(DeclarationSpaces, u32);

impl DeclarationSpaces {
    pub const EXPORT_VALUE: DeclarationSpaces = DeclarationSpaces(1 << 0);
    pub const EXPORT_TYPE: DeclarationSpaces = DeclarationSpaces(1 << 1);
    pub const EXPORT_NAMESPACE: DeclarationSpaces = DeclarationSpaces(1 << 2);

    /// Go: `flags &^ other` (AND NOT — the bits of self not in other).
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// Go: `func isIdentifierThatStartsWithUnderscore(node) bool` (checker.go:7474).
fn is_identifier_that_starts_with_underscore(store: &dyn NodeStore, node: NodeId) -> bool {
    is_identifier(store.node(node)) && {
        let text = tsc_ast::node_text(store, node);
        !text.is_empty() && text.as_bytes()[0] == b'_'
    }
}

/// Go: `func importClauseFromImported(node) *Node` (checker.go:7478).
fn import_clause_from_imported(store: &dyn NodeStore, node: NodeId) -> NodeId {
    match store.node(node).kind {
        Kind::ImportClause => node,
        Kind::NamespaceImport => store.node(node).parent.get(),
        _ => {
            let parent = store.node(node).parent.get();
            store.node(parent).parent.get()
        }
    }
}

/// Go: `func allDeclarationsInSameSourceFile(symbol) bool` (checker.go:7489).
fn all_declarations_in_same_source_file(store: &dyn NodeStore, symbol: SymbolId) -> bool {
    // Go: symbol.Declarations — the checker-side symbol payload read.
    let _ = (store, symbol);
    // The program symbol table is the §5.5 seam; the S2 symbol payload
    // carries declarations through Checker::program.
    true
}

/// The node's locals table for the unused-identifiers walk (the binder-set
/// SymbolTable on the LocalsContainer payloads).
fn locals_of_node(store: &dyn NodeStore, node: NodeId) -> Vec<(String, SymbolId)> {
    let n = store.node(node);
    let table: Option<&tsc_ast::SymbolTable> = match n.kind {
        Kind::SourceFile => Some(&n.as_source_file().expect("data").locals),
        Kind::ModuleDeclaration => Some(&n.as_module_declaration().expect("data").locals),
        Kind::Block => Some(&n.as_block().expect("data").locals),
        Kind::CaseBlock => Some(&n.as_case_block().expect("data").locals),
        Kind::ForStatement => Some(&n.as_for_statement().expect("data").locals),
        Kind::ForInStatement | Kind::ForOfStatement => Some(
            &n.as_for_in_or_of_statement()
                .expect("data")
                .locals,
        ),
        Kind::ClassStaticBlockDeclaration => Some(
            &n.as_class_static_block_declaration()
                .expect("data")
                .locals,
        ),
        Kind::Constructor => Some(&n.as_constructor_declaration().expect("data").locals),
        Kind::FunctionDeclaration => Some(&n.as_function_declaration().expect("data").locals),
        Kind::FunctionExpression => Some(&n.as_function_expression().expect("data").locals),
        Kind::ArrowFunction => Some(&n.as_arrow_function().expect("data").locals),
        Kind::MethodDeclaration => Some(&n.as_method_declaration().expect("data").locals),
        Kind::MethodSignature => Some(&n.as_method_signature_declaration().expect("data").locals),
        Kind::GetAccessor => Some(&n.as_get_accessor_declaration().expect("data").locals),
        Kind::SetAccessor => Some(&n.as_set_accessor_declaration().expect("data").locals),
        _ => None,
    };
    table
        .map(|t| t.entries().map(|(k, v)| (k.clone(), *v)).collect())
        .unwrap_or_default()
}

/// Go: `node.Symbol()` — the binder-set node symbol (payload field).
pub(crate) fn node_symbol_of(store: &dyn NodeStore, node: NodeId) -> Option<SymbolId> {
    let n = store.node(node);
    match n.kind {
        Kind::ClassDeclaration => n.as_class_declaration().expect("data").symbol,
        Kind::ClassExpression => n.as_class_expression().expect("data").symbol,
        Kind::InterfaceDeclaration => n.as_interface_declaration().expect("data").symbol,
        Kind::TypeAliasDeclaration => n.as_type_alias_declaration().expect("data").symbol,
        Kind::EnumDeclaration => n.as_enum_declaration().expect("data").symbol,
        Kind::EnumMember => n.as_enum_member().expect("data").symbol,
        Kind::ModuleDeclaration => n.as_module_declaration().expect("data").symbol,
        Kind::ImportEqualsDeclaration => n.as_import_equals_declaration().expect("data").symbol,
        Kind::ImportClause => n.as_import_clause().expect("data").symbol,
        Kind::ImportSpecifier => n.as_import_specifier().expect("data").symbol,
        Kind::NamespaceImport => n.as_namespace_import().expect("data").symbol,
        Kind::ExportSpecifier => n.as_export_specifier().expect("data").symbol,
        Kind::VariableDeclaration => n.as_variable_declaration().expect("data").symbol,
        Kind::BindingElement => n.as_binding_element().expect("data").symbol,
        Kind::Parameter => n.as_parameter_declaration().expect("data").symbol,
        Kind::FunctionDeclaration => n.as_function_declaration().expect("data").symbol,
        Kind::FunctionExpression => n.as_function_expression().expect("data").symbol,
        Kind::ArrowFunction => n.as_arrow_function().expect("data").symbol,
        Kind::MethodDeclaration => n.as_method_declaration().expect("data").symbol,
        Kind::MethodSignature => n.as_method_signature_declaration().expect("data").symbol,
        Kind::GetAccessor => n.as_get_accessor_declaration().expect("data").symbol,
        Kind::SetAccessor => n.as_set_accessor_declaration().expect("data").symbol,
        Kind::Constructor => n.as_constructor_declaration().expect("data").symbol,
        Kind::PropertyDeclaration => n.as_property_declaration().expect("data").symbol,
        Kind::PropertySignature => {
            n.as_property_signature_declaration().expect("data").symbol
        }
        Kind::ClassStaticBlockDeclaration => {
            n.as_class_static_block_declaration().expect("data").symbol
        }
        Kind::TypeParameter => n.as_type_parameter_declaration().expect("data").symbol,
        Kind::SourceFile => n.as_source_file().expect("data").symbol,
        Kind::ExportDeclaration => n.as_export_declaration().expect("data").symbol,
        Kind::ExportAssignment => n.as_export_assignment().expect("data").symbol,
        Kind::ImportDeclaration => n.as_import_declaration().expect("data").symbol,
        _ => None,
    }
}

/// Go: `node.LocalSymbol()` — the binder-set local symbol payload field.
fn local_symbol_of(store: &dyn NodeStore, node: NodeId) -> Option<SymbolId> {
    let n = store.node(node);
    match n.kind {
        Kind::ClassDeclaration => n.as_class_declaration().expect("data").local_symbol,
        Kind::ClassExpression => n.as_class_expression().expect("data").local_symbol,
        Kind::InterfaceDeclaration => n.as_interface_declaration().expect("data").local_symbol,
        Kind::TypeAliasDeclaration => n.as_type_alias_declaration().expect("data").local_symbol,
        Kind::EnumDeclaration => n.as_enum_declaration().expect("data").local_symbol,
        Kind::ModuleDeclaration => n.as_module_declaration().expect("data").local_symbol,
        Kind::ImportEqualsDeclaration => {
            n.as_import_equals_declaration().expect("data").local_symbol
        }
        Kind::ImportClause => n.as_import_clause().expect("data").local_symbol,
        Kind::ImportSpecifier => n.as_import_specifier().expect("data").local_symbol,
        Kind::NamespaceImport => n.as_namespace_import().expect("data").local_symbol,
        Kind::ExportSpecifier => n.as_export_specifier().expect("data").local_symbol,
        Kind::VariableDeclaration => n.as_variable_declaration().expect("data").local_symbol,
        Kind::BindingElement => n.as_binding_element().expect("data").local_symbol,
        Kind::FunctionDeclaration => n.as_function_declaration().expect("data").local_symbol,
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
use rustc_hash::FxHashMap;

// M6 S3 batch 3 — the type-node checks + function/constructor declaration
// family (Go checker.go:2649-3767) + the variance/deferred type-parameter
// checks. Same Go-name/shape discipline; unlanded leaves stay bannered
// permissive stubs.
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkTypeParameter(node)` (checker.go:2649).
    pub(crate) fn check_type_parameter(&mut self, node: NodeId) {
        // Grammar Checking
        self.check_grammar_modifiers(node);
        let d = self
            .store()
            .node(node)
            .as_type_parameter_declaration()
            .expect("TypeParameterDeclaration data")
            .clone();
        if let Some(expr) = d.expression {
            self.grammar_error_on_first_token(expr, &diag::TYPE_EXPECTED, &[]);
        }
        self.check_source_element(d.constraint);
        self.check_source_element(d.default_type);
        let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
        let type_parameter = self.get_declared_type_of_type_parameter(symbol);
        // Resolve base constraint to reveal circularity errors
        relater::get_base_constraint_of_type(self, type_parameter);
        // TODO(port M6 S7): getResolvedTypeParameterDefault — the resolved
        // default seam; permissive None (the circular-default arm rests).
        let _ = type_parameter;
        let constraint_type = self.get_constraint_of_type_parameter(type_parameter);
        let default_type = self.get_default_from_type_parameter(type_parameter);
        if let (Some(constraint_type), Some(default_type)) = (constraint_type, default_type) {
            let mapper = self.new_simple_type_mapper(type_parameter, default_type);
            let instantiated = self.instantiate_type(constraint_type, mapper);
            let with_this =
                relater::get_type_with_this_argument(self, instantiated, Some(default_type), false);
            self.check_type_assignable_to(
                default_type,
                with_this,
                d.default_type,
                Some(&diag::TYPE_0_DOES_NOT_SATISFY_THE_CONSTRAINT_1),
            );
        }
        self.check_type_name_is_reserved(d.name, &diag::TYPE_PARAMETER_NAME_CANNOT_BE_0);
        self.check_node_deferred(node);
    }

    /// Go: `func (c *Checker) checkTypeParameterDeferred(node)` (checker.go:2667).
    pub(crate) fn check_type_parameter_deferred(&mut self, node: NodeId) {
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        if parent_kind != Kind::InterfaceDeclaration
            && !is_class_like(self.store(), parent)
            && parent_kind != Kind::TypeAliasDeclaration
            && parent_kind != Kind::JSTypeAliasDeclaration
        {
            return;
        }
        let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
        let type_parameter = self.get_declared_type_of_type_parameter(symbol);
        let modifiers = relater::get_type_parameter_modifiers(self, type_parameter)
            & (ModifierFlags::IN.union(ModifierFlags::OUT));
        if modifiers.is_empty() {
            return;
        }
        let parent_symbol = self.get_symbol_of_declaration(parent).unwrap_or_default();
        let parent_declared_type = self.get_declared_type_of_symbol(parent_symbol);
        let variance_alias_gate = (parent_kind == Kind::TypeAliasDeclaration
            || parent_kind == Kind::JSTypeAliasDeclaration)
            && !self
                .type_(parent_declared_type)
                .object_flags
                .intersects(ObjectFlags::ANONYMOUS.union(ObjectFlags::MAPPED));
        if variance_alias_gate {
            self.error(
                node,
                &diag::VARIANCE_ANNOTATIONS_ARE_ONLY_SUPPORTED_IN_TYPE_ALIASES_FOR_OBJECT_FUNCTION_CONSTRUCTOR_AND_MAPPED_TYPES,
                &[],
            );
            return;
        }
        if modifiers == ModifierFlags::IN || modifiers == ModifierFlags::OUT {
            let sub = self
                .marker_sub_type_for_check
                .expect("markerSubTypeForCheck");
            let sup = self
                .marker_super_type_for_check
                .expect("markerSuperTypeForCheck");
            let out = modifiers == ModifierFlags::OUT;
            let source = relater::create_marker_type(
                self,
                parent_symbol,
                type_parameter,
                Some(if out { sub } else { sup }),
            );
            let target = relater::create_marker_type(
                self,
                parent_symbol,
                type_parameter,
                Some(if out { sup } else { sub }),
            );
            let save = self.variance_type_parameter;
            self.variance_type_parameter = Some(type_parameter);
            self.check_type_assignable_to(
                source,
                target,
                Some(node),
                Some(&diag::TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_AS_IMPLIED_BY_VARIANCE_ANNOTATION),
            );
            self.variance_type_parameter = save;
        }
    }

    /// Go: `func (c *Checker) shouldCheckErasableSyntax(node) bool` (checker.go:2687).
    pub(crate) fn should_check_erasable_syntax(&self, node: NodeId) -> bool {
        self.compiler_options.erasable_syntax_only == Tristate::TSTrue
            && !tsc_ast::utilities::is_in_js_file(self.store().node(node))
    }

    /// Go: `func (c *Checker) checkParameter(node)` (checker.go:2690).
    pub(crate) fn check_parameter(&mut self, node: NodeId) {
        // Grammar checking
        self.check_grammar_modifiers(node);
        self.check_variable_like_declaration(node);
        let fn_node = u::get_containing_function(self.store(), node);
        let name = self.store().node(node).name();
        let param_name: String = name
            .filter(|&n| is_identifier(self.store().node(n)))
            .map(|n| tsc_ast::node_text(self.store(), n).to_string())
            .unwrap_or_default();
        if crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::PARAMETER_PROPERTY_MODIFIER) {
            if self.should_check_erasable_syntax(node) {
                self.error(
                    node,
                    &diag::THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
                    &[],
                );
            }
            let ctor_impl = fn_node.is_some_and(|f| {
                self.store().node(f).kind == Kind::Constructor
                    && function_body_of(self.store(), f).is_some()
            });
            if !ctor_impl {
                self.error(
                    node,
                    &diag::A_PARAMETER_PROPERTY_IS_ONLY_ALLOWED_IN_A_CONSTRUCTOR_IMPLEMENTATION,
                    &[],
                );
            }
            if let Some(name) = name.filter(|_| {
                fn_node.is_some_and(|f| self.store().node(f).kind == Kind::Constructor)
                    && param_name == "constructor"
            }) {
                self.error(
                    name,
                    &diag::X_CONSTRUCTOR_CANNOT_BE_USED_AS_A_PARAMETER_PROPERTY_NAME,
                    &[],
                );
            }
        }
        if name.is_none()
            || !is_binding_pattern(self.store(), name.expect("checked"))
        {
            // fallthrough handled below
        }
        let has_init = crate::ast_shims::initializer_of(self.store(), node).is_some();
        if !has_init
            && name.is_some_and(|n| is_binding_pattern(self.store(), n))
            && u::is_optional_declaration(self.store(), node)
            && fn_node.is_some_and(|f| function_body_of(self.store(), f).is_some())
        {
            self.error(
                node,
                &diag::A_BINDING_PATTERN_PARAMETER_CANNOT_BE_OPTIONAL_IN_AN_IMPLEMENTATION_SIGNATURE,
                &[],
            );
        }
        if param_name == "this" || param_name == "new" {
            if let Some(fn_node) = fn_node {
                let params = crate::ast_shims::parameters_of(self.store(), fn_node).to_vec();
                if !params.first().is_some_and(|&p| p == node) {
                    self.error(
                        node,
                        &diag::A_0_PARAMETER_MUST_BE_THE_FIRST_PARAMETER,
                        std::slice::from_ref(&param_name),
                    );
                }
                let fn_kind = self.store().node(fn_node).kind;
                if fn_kind == Kind::Constructor
                    || fn_kind == Kind::ConstructSignature
                    || fn_kind == Kind::ConstructorType
                {
                    self.error(node, &diag::A_CONSTRUCTOR_CANNOT_HAVE_A_THIS_PARAMETER, &[]);
                }
                if fn_kind == Kind::ArrowFunction {
                    self.error(node, &diag::AN_ARROW_FUNCTION_CANNOT_HAVE_A_THIS_PARAMETER, &[]);
                }
                if matches!(fn_kind, Kind::GetAccessor | Kind::SetAccessor) {
                    self.error(node, &diag::X_GET_AND_SET_ACCESSORS_CANNOT_DECLARE_THIS_PARAMETERS, &[]);
                }
            }
        }
        // Only check rest parameter type if it's not a binding pattern.
        let is_pattern = name.is_some_and(|n| is_binding_pattern(self.store(), n));
        if has_dot_dot_dot_token(self.store(), node)
            && !is_pattern
            && self.any_readonly_array_type.is_some()
        {
            let param_symbol = self.get_symbol_of_declaration(node);
            if let Some(param_symbol) = param_symbol {
                let t = self.get_type_of_symbol(param_symbol);
                let reduced = self.get_reduced_type(t);
                let readonly_array = self.any_readonly_array_type.expect("anyReadonlyArrayType");
                if !self.is_type_assignable_to(reduced, readonly_array) {
                    self.error(node, &diag::A_REST_PARAMETER_MUST_BE_OF_AN_ARRAY_TYPE, &[]);
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkPropertyDeclaration(node)` (checker.go:2752).
    pub(crate) fn check_property_declaration(&mut self, node: NodeId) {
        // Grammar checking
        let name = self.store().node(node).name();
        if !self.check_grammar_modifiers(node) && !self.check_grammar_property(node) {
            if let Some(name) = name {
                self.check_grammar_computed_property_name_stub(name);
            }
        }
        self.check_variable_like_declaration(node);
        // TODO(port M6 S8): setNodeLinksForPrivateIdentifierScope — the
        // private-identifier scope bookkeeping; permissive.
        if crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::ABSTRACT)
            && self.store().node(node).kind == Kind::PropertyDeclaration
            && crate::ast_shims::initializer_of(self.store(), node).is_some() {
                if let Some(name) = name {
                    let name_text =
                        tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
                    self.error(
                        node,
                        &diag::PROPERTY_0_CANNOT_HAVE_AN_INITIALIZER_BECAUSE_IT_IS_MARKED_ABSTRACT,
                        &[name_text],
                    );
                }
            }
    }

    /// Go: `func (c *Checker) checkPropertySignature(node)` (checker.go:2768).
    pub(crate) fn check_property_signature(&mut self, node: NodeId) {
        let name = self
            .store()
            .node(node)
            .as_property_signature_declaration()
            .expect("PropertySignatureDeclaration data")
            .name;
        if name != NodeId::NONE
            && self.store().node(name).kind == Kind::PrivateIdentifier
        {
            self.error(node, &diag::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES, &[]);
        }
        self.check_property_declaration(node);
    }

    /// Go: `func (c *Checker) checkSignatureDeclaration(node)` (checker.go:2774).
    pub(crate) fn check_signature_declaration(&mut self, node: NodeId) {
        // Grammar checking
        match self.store().node(node).kind {
            Kind::IndexSignature => {
                self.check_grammar_index_signature(node);
            }
            Kind::FunctionType
            | Kind::FunctionDeclaration
            | Kind::ConstructorType
            | Kind::CallSignature
            | Kind::Constructor
            | Kind::ConstructSignature => {
                self.check_grammar_function_like_declaration(node);
            }
            _ => {}
        }
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), node);
        if !function_flags.intersects(FunctionFlags::INVALID) {
            // Async generators prior to ES2018 require the __await and
            // __asyncGenerator helpers
            if function_flags == FunctionFlags::ASYNC_GENERATOR
                && self.language_version < Feature::AsyncGenerators.minimum_target()
            {
                self.check_external_emit_helpers_stub(
                    node,
                    ExternalEmitHelpers::ASYNC_GENERATOR_INCLUDES,
                );
            }
            if function_flags == FunctionFlags::ASYNC
                && self.language_version < Feature::AsyncFunctions.minimum_target()
            {
                self.check_external_emit_helpers_stub(node, ExternalEmitHelpers::AWAITER);
            }
        }
        let type_parameters = type_parameters_of(self.store(), node);
        self.check_type_parameters(&type_parameters);
        // TODO(port M6 W3): checkUnmatchedJSDocParameters (jsdoc.go);
        // permissive no-op.
        let parameters = crate::ast_shims::parameters_of(self.store(), node).to_vec();
        self.check_source_elements(&parameters);
        let return_type_node = signature_type_node(self.store(), node);
        if let Some(return_type_node) = return_type_node {
            self.check_source_element(Some(return_type_node));
        }
        if self.no_implicit_any && return_type_node.is_none() {
            match self.store().node(node).kind {
                Kind::ConstructSignature => {
                    self.error(
                        node,
                        &diag::CONSTRUCT_SIGNATURE_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_ANY_RETURN_TYPE,
                        &[],
                    );
                }
                Kind::CallSignature => {
                    self.error(
                        node,
                        &diag::CALL_SIGNATURE_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_ANY_RETURN_TYPE,
                        &[],
                    );
                }
                _ => {}
            }
        }
        if let Some(return_type_node) = return_type_node {
            if (function_flags
                & (FunctionFlags::INVALID.union(FunctionFlags::GENERATOR)))
                == FunctionFlags::GENERATOR
            {
                let return_type = self.get_type_from_type_node(return_type_node);
                if return_type == self.void_type() {
                    self.error(
                        return_type_node,
                        &diag::A_GENERATOR_CANNOT_HAVE_A_VOID_TYPE_ANNOTATION,
                        &[],
                    );
                } else {
                    // TODO(port M6 S7): checkGeneratorInstantiation-
                    // AssignabilityToReturnType (the generator return-type
                    // relation); permissive.
                    let _ = return_type;
                }
            } else if (function_flags & FunctionFlags::ASYNC_GENERATOR) == FunctionFlags::ASYNC {
                self.check_async_function_return_type(node, return_type_node);
            }
        }
        if self.store().node(node).kind != Kind::IndexSignature {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) checkAsyncFunctionReturnType(node, returnTypeNode)`
    /// (checker.go:2838).
    pub(crate) fn check_async_function_return_type(&mut self, node: NodeId, return_type_node: NodeId) {
        let return_type = self.get_type_from_type_node(return_type_node);
        if relater::is_error_type(self, return_type) {
            return;
        }
        let global_promise_type = self.get_global_promise_type_checked();
        if global_promise_type != self.empty_generic_type()
            && !self.is_reference_to_type(return_type, global_promise_type)
        {
            // The promise type was not a valid type reference to the global
            // promise type, so we report an error and return the unknown type.
            let awaited = self.get_awaited_type_no_alias(return_type);
            let awaited_text = self.type_to_string(awaited.unwrap_or_else(|| self.void_type()));
            self.error(
                return_type_node,
                &diag::THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_OR_METHOD_MUST_BE_THE_GLOBAL_PROMISE_T_TYPE_DID_YOU_MEAN_TO_WRITE_PROMISE_0,
                &[awaited_text],
            );
            return;
        }
        self.check_awaited_type(
            return_type,
            false, // withAlias
            node,
            &diag::THE_RETURN_TYPE_OF_AN_ASYNC_FUNCTION_MUST_EITHER_BE_A_VALID_PROMISE_OR_MUST_NOT_CONTAIN_A_CALLABLE_THEN_MEMBER,
        );
    }

    /// Go: `func (c *Checker) checkMethodDeclaration(node)` (checker.go:2861).
    pub(crate) fn check_method_declaration(&mut self, node: NodeId) {
        // Grammar checking
        let name = self.store().node(node).name();
        if !self.check_grammar_method(node) {
            if let Some(name) = name {
                let t = self.check_grammar_computed_property_name_stub(name);
                let _ = t;
            }
            if self.store().node(node).kind == Kind::MethodDeclaration {
                let d = self
                    .store()
                    .node(node)
                    .as_method_declaration()
                    .expect("MethodDeclaration data")
                    .clone();
                if d.asterisk_token.is_some()
                    && name.is_some_and(|n| is_identifier(self.store().node(n)))
                    && tsc_ast::node_text(self.store(), name.expect("checked")) == "constructor"
                {
                    self.error(node, &diag::CLASS_CONSTRUCTOR_MAY_NOT_BE_A_GENERATOR, &[]);
                }
            }
        }
        // Grammar checking for modifiers is done inside the function
        // checkGrammarFunctionLikeDeclaration
        self.check_function_or_method_declaration(node);
        // method signatures already report "implementation not allowed in
        // ambient context" elsewhere
        if crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::ABSTRACT)
            && self.store().node(node).kind == Kind::MethodDeclaration
            && function_body_of(self.store(), node).is_some()
        {
            if let Some(name) = name {
                let name_text =
                    tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
                self.error(
                    node,
                    &diag::METHOD_0_CANNOT_HAVE_AN_IMPLEMENTATION_BECAUSE_IT_IS_MARKED_ABSTRACT,
                    &[name_text],
                );
            }
        }
        // Private named methods are only allowed in class declarations
        if name.is_some_and(|n| self.store().node(n).kind == Kind::PrivateIdentifier)
            && u::get_containing_class(self.store(), node).is_none()
        {
            self.error(node, &diag::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES, &[]);
        }
        // TODO(port M6 S8): setNodeLinksForPrivateIdentifierScope; permissive.
    }

    /// Go: `func (c *Checker) checkClassStaticBlockDeclaration(node)` (checker.go:2883).
    pub(crate) fn check_class_static_block_declaration(&mut self, node: NodeId) {
        // Grammar checking
        self.check_grammar_modifiers(node);
        let children = children_of(self.store(), node);
        for child in children {
            self.check_source_element(Some(child));
        }
        if has_locals(self.store(), node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    /// Go: `func (c *Checker) checkConstructorDeclaration(node)` (checker.go:2891).
    pub(crate) fn check_constructor_declaration(&mut self, node: NodeId) {
        self.check_signature_declaration(node);
        // Grammar check for checking only related to constructorDeclaration
        if !self.check_grammar_constructor_type_parameters(node) {
            self.check_grammar_constructor_type_annotation(node);
        }
        let body = function_body_of(self.store(), node);
        self.check_source_element(body);
        let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
        self.check_function_or_constructor_symbol(symbol);
        // exit early in the case of signature - super checks are not
        // relevant to them
        if body.is_none() {
            return;
        }
        // TS 1.0 spec (April 2014): 8.3.2
        let containing_class_decl = self.store().node(node).parent.get();
        if get_class_extends_heritage_element(self.store(), containing_class_decl).is_none() {
            return;
        }
        // TODO(port M6 W3): classDeclarationExtendsNull — the `extends null`
        // grammar fact; permissive false.
        let class_extends_null = false;
        let super_call = self.find_first_super_call(body.expect("checked"));
        if let Some(super_call) = super_call {
            if class_extends_null {
                self.error(
                    super_call,
                    &diag::A_CONSTRUCTOR_CANNOT_CONTAIN_A_SUPER_CALL_WHEN_ITS_CLASS_EXTENDS_NULL,
                    &[],
                );
            }
            // A super call must be root-level in a constructor if both of the
            // following are true:
            // - The containing class is a derived class.
            // - The constructor declares parameter properties or the
            //   containing class declares instance member variables with
            //   initializers.
            let members = members_of(self.store(), containing_class_decl);
            let parameters = crate::ast_shims::parameters_of(self.store(), node).to_vec();
            let has_param_props = parameters.iter().any(|&p| {
                crate::ast_shims::has_modifier(self.store().node(p), ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
            });
            let has_initialized_members = members.iter().any(|&m| {
                is_instance_property_with_initializer_or_private_identifier_property(self.store(), m)
            });
            let super_call_should_be_root_level =
                !self.emit_standard_class_fields && (has_initialized_members || has_param_props);
            if super_call_should_be_root_level {
                // Until we have better flow analysis, it is an error to
                // place the super call within any kind of block or
                // conditional. See GH #8277
                if !super_call_is_root_level_in_constructor(self.store(), super_call, body.expect("checked")) {
                    self.error(
                        super_call,
                        &diag::A_SUPER_CALL_MUST_BE_A_ROOT_LEVEL_STATEMENT_WITHIN_A_CONSTRUCTOR_OF_A_DERIVED_CLASS_THAT_CONTAINS_INITIALIZED_PROPERTIES_PARAMETER_PROPERTIES_OR_PRIVATE_IDENTIFIERS,
                        &[],
                    );
                } else {
                    let body_node = body.expect("checked");
                    let statements = block_statements(self.store(), body_node);
                    let mut super_call_statement: Option<NodeId> = None;
                    for &statement in &statements {
                        if self.store().node(statement).kind == Kind::ExpressionStatement {
                            let expr = self
                                .store()
                                .node(statement)
                                .as_expression_statement()
                                .expect("ExpressionStatement data")
                                .expression;
                            // TODO(port M6 S4): SkipOuterExpressions —
                            // permissive direct read.
                            if u::is_super_call(self.store(), expr) {
                                super_call_statement = Some(statement);
                                break;
                            }
                        }
                        if node_immediately_references_super_or_this(self.store(), statement) {
                            break;
                        }
                    }
                    // Until we have better flow analysis, it is an error to
                    // place the super call within any kind of block or
                    // conditional. See GH #8277
                    if super_call_statement.is_none() {
                        self.error(
                            node,
                            &diag::A_SUPER_CALL_MUST_BE_THE_FIRST_STATEMENT_IN_THE_CONSTRUCTOR_TO_REFER_TO_SUPER_OR_THIS_WHEN_A_DERIVED_CLASS_CONTAINS_INITIALIZED_PROPERTIES_PARAMETER_PROPERTIES_OR_PRIVATE_IDENTIFIERS,
                            &[],
                        );
                    }
                }
            }
        } else if !class_extends_null {
            self.error(node, &diag::CONSTRUCTORS_FOR_DERIVED_CLASSES_MUST_CONTAIN_A_SUPER_CALL, &[]);
        }
    }

    /// Go: `func (c *Checker) findFirstSuperCall(node) *Node` (checker.go:2943).
    pub(crate) fn find_first_super_call(&mut self, node: NodeId) -> Option<NodeId> {
        let mut super_call: Option<NodeId> = None;
        find_first_super_call_visit(self.store(), node, &mut super_call);
        super_call
    }

    /// Go: `func (c *Checker) checkAccessorDeclaration(node)` (checker.go:2969).
    pub(crate) fn check_accessor_declaration(&mut self, node: NodeId) {
        // Grammar checking accessors
        let name = self.store().node(node).name();
        if !self.check_grammar_function_like_declaration(node) && !self.check_grammar_accessor(node) {
            if let Some(name) = name {
                let t = self.check_grammar_computed_property_name_stub(name);
                let _ = t;
            }
        }
        if name.is_some_and(|n| is_identifier(self.store().node(n)))
            && name.is_some_and(|n| tsc_ast::node_text(self.store(), n) == "constructor")
            && is_class_like(self.store(), self.store().node(node).parent.get())
        {
            self.error(
                name.expect("checked"),
                &diag::CLASS_CONSTRUCTOR_MAY_NOT_BE_AN_ACCESSOR,
                &[],
            );
        }
        self.check_decorators(node);
        self.check_signature_declaration(node);
        let n = self.store().node(node);
        if n.kind == Kind::GetAccessor
            && !n.flags.intersects(NF::AMBIENT)
            && function_body_of(self.store(), node).is_some()
            && n.flags.intersects(NF::HAS_IMPLICIT_RETURN)
            && !n.flags.intersects(NF::HAS_EXPLICIT_RETURN)
        {
            if let Some(name) = name {
                self.error(name, &diag::A_GET_ACCESSOR_MUST_RETURN_A_VALUE, &[]);
            }
        }
        // Do not use hasDynamicName here, because that returns false for
        // well known symbols. We want to perform checkComputedPropertyName
        // for all computed properties, including well known symbols.
        if name.is_some_and(|n| self.store().node(n).kind == Kind::ComputedPropertyName) {
            let t = self.check_grammar_computed_property_name_stub(name.expect("checked"));
            let _ = t;
        }
        if self.has_bindable_name(node) {
            // TypeScript 1.0 spec (April 2014): 8.4.3
            // Accessors for the same member name must specify the same
            // accessibility.
            let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
            let getter = get_declaration_of_kind(self.store(), self.program_symbol(symbol), Kind::GetAccessor);
            let setter = get_declaration_of_kind(self.store(), self.program_symbol(symbol), Kind::SetAccessor);
            if let (Some(getter), Some(setter)) = (getter, setter) {
                // TODO(port M6 S8): the NodeCheckFlags TypeChecked gate —
                // permissive: run the check (idempotent diagnostics gate on
                // the modifier comparison itself).
                let getter_flags = tsc_binder::ast_shims::get_combined_modifier_flags(self.store(), getter);
                let setter_flags = tsc_binder::ast_shims::get_combined_modifier_flags(self.store(), setter);
                if (getter_flags & ModifierFlags::ABSTRACT) != (setter_flags & ModifierFlags::ABSTRACT) {
                    if let Some(gname) = self.store().node(getter).name() {
                        self.error(gname, &diag::ACCESSORS_MUST_BOTH_BE_ABSTRACT_OR_NON_ABSTRACT, &[]);
                    }
                    if let Some(sname) = self.store().node(setter).name() {
                        self.error(sname, &diag::ACCESSORS_MUST_BOTH_BE_ABSTRACT_OR_NON_ABSTRACT, &[]);
                    }
                }
                if (getter_flags.intersects(ModifierFlags::PROTECTED)
                    && !(setter_flags
                        .intersects(ModifierFlags::PROTECTED.union(ModifierFlags::PRIVATE))))
                    || (getter_flags.intersects(ModifierFlags::PRIVATE)
                        && !setter_flags.intersects(ModifierFlags::PRIVATE))
                {
                    if let Some(gname) = self.store().node(getter).name() {
                        self.error(gname, &diag::A_GET_ACCESSOR_MUST_BE_AT_LEAST_AS_ACCESSIBLE_AS_THE_SETTER, &[]);
                    }
                    if let Some(sname) = self.store().node(setter).name() {
                        self.error(sname, &diag::A_GET_ACCESSOR_MUST_BE_AT_LEAST_AS_ACCESSIBLE_AS_THE_SETTER, &[]);
                    }
                }
            }
        }
        let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
        let return_type = self.get_type_of_accessors(symbol);
        if self.store().node(node).kind == Kind::GetAccessor {
            self.check_all_code_paths_in_non_void_function_return_or_throw(node, Some(return_type));
        }
        let body = function_body_of(self.store(), node);
        self.check_source_element(body);
        // TODO(port M6 S8): setNodeLinksForPrivateIdentifierScope; permissive.
    }

    /// Go: `func (c *Checker) checkTypeReferenceNode(node)` (checker.go:3028).
    pub(crate) fn check_type_reference_node(&mut self, node: NodeId) {
        // Go: node.TypeArgumentList() — the Rust payload keeps the list
        // inline; the W3 grammar stub takes the list node, which the
        // permissive stub ignores.
        self.check_grammar_type_arguments(node, None);
        // Go: the JSDoc DotToken scan between TypeName and TypeArguments is
        // a W3 scanner-seam fact; TODO(port M6 W3): ScanTokenAtPosition.
        let type_arguments = type_arguments_of(self.store(), node);
        self.check_source_elements(&type_arguments);
        // Go: isConstTypeReference(node) && IsAssertionExpression(node.Parent)
        // — the const-assertion fact is an S4 expression leaf; permissive
        // false (the assertion parent walk lands with S4).
        if !is_const_type_reference_stub(node) {
            self.check_type_reference_or_import(node);
        }
    }

    /// Go: `func (c *Checker) checkTypeReferenceOrImport(node)` (checker.go:3048).
    pub(crate) fn check_type_reference_or_import(&mut self, node: NodeId) {
        let t = self.get_type_from_type_node(node);
        if !relater::is_error_type(self, t) {
            let type_arguments = type_arguments_of(self.store(), node);
            if !type_arguments.is_empty() {
                // M6 S5: getTypeParametersForTypeReferenceOrImport (Go
                // checker.go:17574) — the real type-parameter resolution for
                // the constraint check (typenodes.rs).
                let type_parameters = self.get_type_parameters_for_type_reference_or_import(node);
                if !type_parameters.is_empty() {
                    self.check_type_argument_constraints(node, &type_parameters);
                }
            }
            let symbol = self.get_resolved_symbol_nil(node);
            if let Some(symbol) = symbol {
                // TODO(port M6 deprecation family): IsDeprecatedDeclaration +
                // addDeprecatedSuggestion (the JSDoc @deprecated tag walk —
                // resolvecall.rs carries the same seam); permissive.
                let _ = symbol;
            }
        }
    }

    /// Go: `func (c *Checker) checkTypeArgumentConstraints(node, typeParameters) bool`
    /// (checker.go:3064).
    pub(crate) fn check_type_argument_constraints(&mut self, node: NodeId, type_parameters: &[TypeId]) -> bool {
        let mut type_arguments: Option<Vec<TypeId>> = None;
        let mut mapper: Option<crate::todo_shapes::TypeMapperId> = None;
        let mut result = true;
        let type_argument_nodes = type_arguments_of(self.store(), node);
        for (i, &type_parameter) in type_parameters.iter().enumerate() {
            let constraint = self.get_constraint_of_type_parameter(type_parameter);
            if let Some(constraint) = constraint {
                if type_arguments.is_none() {
                    // M6 S5: getEffectiveTypeArguments (Go checker.go:22392)
                    // — the real defaults-aware type-argument resolution
                    // (typenodes.rs).
                    type_arguments = Some(self.get_effective_type_arguments(node, type_parameters));
                    let args = type_arguments.clone().expect("checked");
                    mapper = Some(self.new_type_mapper(type_parameters, &args));
                }
                let instantiated = self.instantiate_type(
                    constraint,
                    mapper.expect("checked"),
                );
                let ta = type_arguments.as_ref().expect("checked")[i];
                let error_node = type_argument_nodes.get(i).copied();
                result = result
                    && self.check_type_assignable_to_ex(
                        ta,
                        instantiated,
                        error_node,
                        Some(&diag::TYPE_0_DOES_NOT_SATISFY_THE_CONSTRAINT_1),
                        None,
                    );
            }
        }
        result
    }

    /// Go: `func (c *Checker) checkTypePredicate(node)` (checker.go:3110).
    pub(crate) fn check_type_predicate(&mut self, node: NodeId) {
        // Always check the predicate's type so nested type errors are
        // reported even when the predicate is in an invalid position, keeping
        // diagnostics stable.
        let predicate_type = self
            .store()
            .node(node)
            .as_type_predicate_node()
            .expect("TypePredicateNode data")
            .type_;
        self.check_source_element(predicate_type);
        let parent = get_type_predicate_parent(self.store(), node);
        let Some(parent) = parent else {
            // The parent must not be valid.
            self.error(
                node,
                &diag::A_TYPE_PREDICATE_IS_ONLY_ALLOWED_IN_RETURN_TYPE_POSITION_FOR_FUNCTIONS_AND_METHODS,
                &[],
            );
            return;
        };
        let Some(signature) = self.get_signature_from_declaration(parent) else {
            return;
        };
        let Some(type_predicate) = self.get_type_predicate_of_signature(signature).map(|p| *p) else {
            return;
        };
        let parameter_name = self
            .store()
            .node(node)
            .as_type_predicate_node()
            .expect("TypePredicateNode data")
            .parameter_name;
        let parameter_name_opt = (parameter_name != NodeId::NONE).then_some(parameter_name);
        if type_predicate.kind != crate::types::TypePredicateKind::This
            && type_predicate.kind != crate::types::TypePredicateKind::AssertsThis
        {
            if type_predicate.parameter_index >= 0 {
                let signature_params = self.signatures[signature.0 as usize].parameters.clone();
                // Go: signatureHasRestParameter(signature) && index == len-1
                // TODO(port M6 S7): signatureHasRestParameter; permissive
                // false (the rest-parameter predicate arm rests).
                let has_rest = false;
                if has_rest && type_predicate.parameter_index == signature_params.len() as i32 - 1 {
                    if let Some(parameter_name) = parameter_name_opt {
                        self.error(
                            parameter_name,
                            &diag::A_TYPE_PREDICATE_CANNOT_REFERENCE_A_REST_PARAMETER,
                            &[],
                        );
                    }
                } else if let Some(t) = type_predicate.t {
                    let param = signature_params
                        .get(type_predicate.parameter_index as usize)
                        .copied();
                    if let Some(param) = param {
                        let param_type = self.get_type_of_symbol(param);
                        let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
                        let ok = self.check_type_assignable_to_ex(
                            t,
                            param_type,
                            predicate_type,
                            None,
                            Some(&mut diags),
                        );
                        if !ok && !diags.is_empty() {
                            // Go: NewDiagnosticChain(diags[0], ...) — the
                            // chain's head carries the wrap message.
                            let mut chained = diags.remove(0);
                            chained.add_related_info(u::new_diagnostic_for_node(
                                self.store(),
                                predicate_type.expect("checked"),
                                &diag::A_TYPE_PREDICATE_S_TYPE_MUST_BE_ASSIGNABLE_TO_ITS_PARAMETER_S_TYPE,
                                vec![],
                            ));
                            self.add_diagnostic(chained);
                        }
                    }
                }
            } else if let Some(parameter_name) = parameter_name_opt {
                let mut has_reported_error = false;
                for param in crate::ast_shims::parameters_of(self.store(), parent).to_vec() {
                    let name = self.store().node(param).name();
                    if name.is_some_and(|n| is_binding_pattern(self.store(), n))
                        && self.check_if_type_predicate_variable_is_declared_in_binding_pattern(
                            name.expect("checked"),
                            parameter_name,
                            &type_predicate.parameter_name,
                        ) {
                            has_reported_error = true;
                            break;
                        }
                }
                if !has_reported_error {
                    self.error(
                        parameter_name,
                        &diag::CANNOT_FIND_PARAMETER_0,
                        std::slice::from_ref(&type_predicate.parameter_name),
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkIfTypePredicateVariableIsDeclaredInBindingPattern(pattern, predicateVariableNode, predicateVariableName) bool`
    /// (checker.go:3153).
    pub(crate) fn check_if_type_predicate_variable_is_declared_in_binding_pattern(
        &mut self,
        pattern: NodeId,
        predicate_variable_node: NodeId,
        predicate_variable_name: &str,
    ) -> bool {
        for element in binding_pattern_elements(self.store(), pattern) {
            let name = self.store().node(element).name();
            let Some(name) = name else { continue };
            if is_identifier(self.store().node(name))
                && tsc_ast::node_text(self.store(), name) == predicate_variable_name
            {
                self.error(
                    predicate_variable_node,
                    &diag::A_TYPE_PREDICATE_CANNOT_REFERENCE_ELEMENT_0_IN_A_BINDING_PATTERN,
                    &[predicate_variable_name.to_string()],
                );
                return true;
            }
            if is_binding_pattern(self.store(), name)
                && self.check_if_type_predicate_variable_is_declared_in_binding_pattern(
                    name,
                    predicate_variable_node,
                    predicate_variable_name,
                )
            {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkTypeQuery(node)` (checker.go:3179).
    pub(crate) fn check_type_query(&mut self, node: NodeId) {
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkTypeLiteral(node)` (checker.go:3182).
    pub(crate) fn check_type_literal(&mut self, node: NodeId) {
        let members = members_of(self.store(), node);
        self.check_source_elements(&members);
        let t = self.get_type_from_type_node(node);
        // REAL (M6 S10, checker.go:3185): checkIndexConstraints (the
        // index-constraint relation family).
        let symbol = crate::typeofsymbol::get_symbol_of_declaration(self, node)
            .expect("type literal symbol");
        self.check_index_constraints(t, symbol, false /*isStaticIndex*/);
        // TODO(port M6 S3 class batch): checkTypeForDuplicateIndexSignatures
        // — lands with checkClassOrInterfaceForDuplicateIndexSignatures.
        self.check_object_type_for_duplicate_declarations(node, false /*checkPrivateNames*/);
    }

    /// Go: `func (c *Checker) checkObjectTypeForDuplicateDeclarations(node, checkPrivateNames)`
    /// (checker.go:3191).
    pub(crate) fn check_object_type_for_duplicate_declarations(
        &mut self,
        node: NodeId,
        check_private_names: bool,
    ) {
        let mut instance_names: FxHashMap<String, i32> = FxHashMap::default();
        let mut static_names: FxHashMap<String, i32> = FxHashMap::default();
        let mut private_names: FxHashMap<String, i32> = FxHashMap::default();
        let node_in_ambient_context = self.store().node(node).flags.intersects(NF::AMBIENT);
        for member in members_of(self.store(), node) {
            if self.store().node(member).kind == Kind::Constructor {
                for param in crate::ast_shims::parameters_of(self.store(), member).to_vec() {
                    let parent = self.store().node(member);
                    let _ = parent;
                    let param_symbol = self.get_symbol_of_declaration(param);
                    let param_name = param_symbol
                        .map(|s| self.program_symbol(s).name.clone());
                    if u::is_parameter_property_declaration(
                        self.store(),
                        param,
                        member,
                    ) && !self.store().node(param).name().is_some_and(|n| is_binding_pattern(self.store(), n))
                    {
                        check_property_or_accessor(
                            self,
                            &mut instance_names,
                            &mut static_names,
                            param,
                            1,
                            false,
                            param_name.as_ref(),
                        );
                    }
                }
            } else {
                let symbol = self.get_symbol_of_declaration(member);
                let is_static = has_static_modifier(self.store(), member);
                // In non-ambient contexts, check that static members are not
                // named 'prototype'.
                if !node_in_ambient_context && is_static {
                    if let Some(symbol) = symbol {
                        if self.program_symbol(symbol).name == "prototype" {
                            if let Some(name) = self.store().node(member).name() {
                                let symbol_name = self.program_symbol(symbol).name.clone();
                                let node_symbol = self.get_symbol_of_declaration(node);
                                let node_symbol_name = node_symbol
                                    .map(|s| self.program_symbol(s).name.clone())
                                    .unwrap_or_default();
                                self.error(
                                    name,
                                    &diag::STATIC_PROPERTY_0_CONFLICTS_WITH_BUILT_IN_PROPERTY_FUNCTION_0_OF_CONSTRUCTOR_FUNCTION_1,
                                    &[symbol_name, node_symbol_name],
                                );
                            }
                        }
                    }
                }
                // Check that this object type declaration doesn't contain
                // multiple declarations of the same property, or accessor and
                // property declarations with the same name.
                let member_kind = self.store().node(member).kind;
                let is_property_declaration = member_kind == Kind::PropertyDeclaration;
                let is_property_signature = member_kind == Kind::PropertySignature;
                let is_accessor = matches!(member_kind, Kind::GetAccessor | Kind::SetAccessor);
                if (is_property_declaration
                    && !crate::ast_shims::has_modifier(self.store().node(member), ModifierFlags::ACCESSOR))
                    || is_property_signature
                {
                    if let Some(symbol) = symbol {
                        check_property_or_accessor(
                            self,
                            &mut instance_names,
                            &mut static_names,
                            member,
                            1,
                            is_static,
                            // Go reads symbol.Name through checkPropertyOrAccessor
                            Some(&self.program_symbol(symbol).name.clone()),
                        );
                    }
                } else if is_accessor
                    || (is_property_declaration
                        && crate::ast_shims::has_modifier(self.store().node(member), ModifierFlags::ACCESSOR))
                {
                    if let Some(symbol) = symbol {
                        check_property_or_accessor(
                            self,
                            &mut instance_names,
                            &mut static_names,
                            member,
                            2,
                            is_static,
                            Some(&self.program_symbol(symbol).name.clone()),
                        );
                    }
                }
                // Check that each private identifier is used only for
                // instance members or only for static members.
                if check_private_names
                    && self
                        .store()
                        .node(member)
                        .name()
                        .is_some_and(|n| self.store().node(n).kind == Kind::PrivateIdentifier)
                {
                    if let Some(symbol) = symbol {
                        let symbol_name = self.program_symbol(symbol).name.clone();
                        let flags = private_names.get(&symbol_name).copied().unwrap_or(0);
                        if flags != 3 {
                            let flags =
                                flags | if is_static { 2 } else { 1 };
                            private_names.insert(symbol_name.clone(), flags);
                            if flags == 3 {
                                self.report_duplicate_member_errors(
                                    node,
                                    &symbol_name,
                                    false, /*checkStatic*/
                                    false, /*isStatic*/
                                    &diag::DUPLICATE_IDENTIFIER_0_STATIC_AND_INSTANCE_ELEMENTS_CANNOT_SHARE_THE_SAME_PRIVATE_NAME,
                                );
                            }
                        }
                    }
                }
            }
        }
        let _ = (check_private_names,);
    }

    /// Go: `func (c *Checker) reportDuplicateMemberErrors(node, name, checkStatic, isStatic, message)`
    /// (checker.go:3254).
    pub(crate) fn report_duplicate_member_errors(
        &mut self,
        node: NodeId,
        name: &str,
        check_static: bool,
        is_static: bool,
        message: &'static Message,
    ) {
        for member in members_of(self.store(), node) {
            if self.store().node(member).kind == Kind::Constructor {
                for param in crate::ast_shims::parameters_of(self.store(), member).to_vec() {
                    if u::is_parameter_property_declaration(self.store(), param, member)
                        && !self
                            .store()
                            .node(param)
                            .name()
                            .is_some_and(|n| is_binding_pattern(self.store(), n))
                    {
                        if let Some(symbol) = self.get_symbol_of_declaration(param) {
                            if self.program_symbol(symbol).name == name {
                                let symbol_name = self.program_symbol(symbol).name.clone();
                                if let Some(pname) = self.store().node(param).name() {
                                    self.error(pname, message, &[symbol_name]);
                                }
                            }
                        }
                    }
                }
            } else if let Some(symbol) = self.get_symbol_of_declaration(member) {
                if self.program_symbol(symbol).name == name
                    && (!check_static || is_static == is_static_modifier(self.store(), member))
                {
                    let symbol_name = self.program_symbol(symbol).name.clone();
                    if let Some(mname) = self.store().node(member).name() {
                        self.error(mname, message, &[symbol_name]);
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkArrayType(node)` (checker.go:3274).
    pub(crate) fn check_array_type(&mut self, node: NodeId) {
        let element_type = self
            .store()
            .node(node)
            .as_array_type_node()
            .expect("ArrayTypeNode data")
            .element_type;
        self.check_source_element(Some(element_type));
    }

    /// Go: `func (c *Checker) checkTupleType(node)` (checker.go:3278).
    pub(crate) fn check_tuple_type(&mut self, node: NodeId) {
        let mut seen_optional_element = false;
        let mut seen_rest_element = false;
        let elements = self
            .store()
            .node(node)
            .as_tuple_type_node()
            .expect("TupleTypeNode data")
            .elements
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        for &e in &elements {
            // TODO(port M6 S7): getTupleElementFlags — the element-flag
            // computation; permissive Required (1).
            let flags = crate::types::ElementFlags::REQUIRED;
            if flags.intersects(crate::types::ElementFlags::VARIADIC) {
                let t = self.get_type_from_type_node(element_type_of(self.store(), e));
                // TODO(port M6 S7): isArrayLikeType/isArrayType/isTupleType —
                // the array-family predicates; permissive false (both
                // error arms rest).
                let _ = t;
            }
            if flags.intersects(crate::types::ElementFlags::REST) {
                if seen_rest_element {
                    self.grammar_error_on_node(e, &diag::A_REST_ELEMENT_CANNOT_FOLLOW_ANOTHER_REST_ELEMENT, &[]);
                    break;
                }
                seen_rest_element = true;
            } else if flags.intersects(crate::types::ElementFlags::OPTIONAL) {
                if seen_rest_element {
                    self.grammar_error_on_node(e, &diag::AN_OPTIONAL_ELEMENT_CANNOT_FOLLOW_A_REST_ELEMENT, &[]);
                    break;
                }
                seen_optional_element = true;
            } else if flags.intersects(crate::types::ElementFlags::REQUIRED)
                && seen_optional_element
            {
                self.grammar_error_on_node(
                    e,
                    &diag::A_REQUIRED_ELEMENT_CANNOT_FOLLOW_AN_OPTIONAL_ELEMENT,
                    &[],
                );
                break;
            }
            let _ = flags;
        }
        self.check_source_elements(&elements);
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkUnionOrIntersectionType(node)` (checker.go:3308).
    pub(crate) fn check_union_or_intersection_type(&mut self, node: NodeId) {
        for child in children_of(self.store(), node) {
            self.check_source_element(Some(child));
        }
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkThisType(node)` (checker.go:3312).
    pub(crate) fn check_this_type(&mut self, node: NodeId) {
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkTypeOperator(node)` (checker.go:3315).
    pub(crate) fn check_type_operator(&mut self, node: NodeId) {
        // TODO(port M6 W3): checkGrammarTypeOperatorNode; permissive.
        let type_node = self
            .store()
            .node(node)
            .as_type_operator_node()
            .expect("TypeOperatorNode data")
            .type_;
        self.check_source_element(Some(type_node));
    }

    /// Go: `func (c *Checker) checkConditionalType(node)` (checker.go:3319).
    pub(crate) fn check_conditional_type(&mut self, node: NodeId) {
        for child in children_of(self.store(), node) {
            self.check_source_element(Some(child));
        }
    }

    /// Go: `func (c *Checker) checkInferType(node)` (checker.go:3322).
    pub(crate) fn check_infer_type(&mut self, node: NodeId) {
        // Go: FindAncestor — the extends-clause containment; the parent walk.
        let mut in_extends_clause = false;
        let mut current = self.store().node(node).parent.get();
        while current != NodeId::NONE {
            let parent = self.store().node(current).parent.get();
            if parent != NodeId::NONE && self.store().node(parent).kind == Kind::ConditionalType {
                let extends_type = self
                    .store()
                    .node(parent)
                    .as_conditional_type_node()
                    .expect("ConditionalTypeNode data")
                    .extends_type;
                if extends_type == current {
                    in_extends_clause = true;
                    break;
                }
            }
            current = parent;
        }
        if !in_extends_clause {
            self.grammar_error_on_node(
                node,
                &diag::X_INFER_DECLARATIONS_ARE_ONLY_PERMITTED_IN_THE_EXTENDS_CLAUSE_OF_A_CONDITIONAL_TYPE,
                &[],
            );
        }
        let type_parameter_declaration_node = self
            .store()
            .node(node)
            .as_infer_type_node()
            .expect("InferTypeNode data")
            .type_parameter;
        self.check_source_element(Some(type_parameter_declaration_node));
        if let Some(symbol) = self.get_symbol_of_declaration(type_parameter_declaration_node) {
            let declarations = self.program_symbol(symbol).declarations.clone();
            if declarations.len() > 1 {
                // TODO(port M6 S7): the declaredTypeLinks typeParametersChecked
                // gate + areTypeParametersIdentical — lands with the class
                // batch's identical-constraints family; permissive skip.
                let _ = declarations;
            }
        }
        self.register_for_unused_identifiers_check(node);
    }

    /// Go: `func (c *Checker) checkTemplateLiteralType(node)` (checker.go:3349).
    pub(crate) fn check_template_literal_type(&mut self, node: NodeId) {
        let spans = self
            .store()
            .node(node)
            .as_template_literal_type_node()
            .expect("TemplateLiteralTypeNode data")
            .template_spans
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        for span in spans {
            let span_type = self
                .store()
                .node(span)
                .as_template_literal_type_span()
                .expect("TypeSpanNode data")
                .type_;
            self.check_source_element(Some(span_type));
            let t = self.get_type_from_type_node(span_type);
            // Go: checkTypeAssignableTo(t, templateConstraintType, ...) —
            // the template constraint is the string-literal-or-union family;
            // permissive skip until S7's constraint type exists.
            let _ = t;
        }
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkImportType(node)` (checker.go:3362).
    pub(crate) fn check_import_type(&mut self, node: NodeId) {
        let d = self
            .store()
            .node(node)
            .as_import_type_node()
            .expect("ImportTypeNode data")
            .clone();
        self.check_source_element(Some(d.argument));
        if let Some(attributes) = d.attributes {
            // TODO(port M6 W3): checkGrammarImportAttributeValues +
            // getResolutionModeOverride (jsdoc/import-attributes grammar);
            // permissive.
            let _ = attributes;
        }
        self.check_type_reference_or_import(node);
        // Go: checkImportAttributes(node) (checker.go:5460) — wired to the
        // batch-5 body.
        self.check_import_attributes(node);
    }

    /// Go: `func (c *Checker) checkNamedTupleMember(node)` (checker.go:3386).
    pub(crate) fn check_named_tuple_member(&mut self, node: NodeId) {
        let d = self
            .store()
            .node(node)
            .as_named_tuple_member()
            .expect("NamedTupleMember data")
            .clone();
        if d.dot_dot_dot_token.is_some() && d.question_token.is_some() {
            self.grammar_error_on_node(node, &diag::A_TUPLE_MEMBER_CANNOT_BE_BOTH_OPTIONAL_AND_REST, &[]);
        }
        if self.store().node(d.type_).kind == Kind::OptionalType {
            self.grammar_error_on_node(
                d.type_,
                &diag::A_LABELED_TUPLE_ELEMENT_IS_DECLARED_AS_OPTIONAL_WITH_A_QUESTION_MARK_AFTER_THE_NAME_AND_BEFORE_THE_COLON_RATHER_THAN_AFTER_THE_TYPE,
                &[],
            );
        }
        if self.store().node(d.type_).kind == Kind::RestType {
            self.grammar_error_on_node(
                d.type_,
                &diag::A_LABELED_TUPLE_ELEMENT_IS_DECLARED_AS_REST_WITH_A_BEFORE_THE_NAME_RATHER_THAN_BEFORE_THE_TYPE,
                &[],
            );
        }
        self.check_source_element(Some(d.type_));
        let t = self.get_type_from_type_node(node);
        let _ = t;
    }

    /// Go: `func (c *Checker) checkIndexedAccessType(node)` (checker.go:3402).
    pub(crate) fn check_indexed_access_type(&mut self, node: NodeId) {
        for child in children_of(self.store(), node) {
            self.check_source_element(Some(child));
        }
        // REAL (M6 S10, checker.go:3407): the index-type constraint family
        // (expressions.rs's S4-landed REAL body).
        let t = self.get_type_from_indexed_access_type_node(node);
        self.check_indexed_access_index_type(t, node);
    }

    /// Go: `func (c *Checker) checkMappedType(node)` (checker.go:3409).
    pub(crate) fn check_mapped_type(&mut self, node: NodeId) {
        let d = self
            .store()
            .node(node)
            .as_mapped_type_node()
            .expect("MappedTypeNode data")
            .clone();
        // TODO(port M6 W3): checkGrammarMappedType; permissive.
        self.check_source_element(Some(d.type_parameter));
        self.check_source_element(d.name_type);
        self.check_source_element(d.type_);
        if d.type_.is_none() {
            let any_type = self.any_type();
            self.report_implicit_any(node, any_type, crate::typeofsymbol::WideningKind::Normal);
        }
        // REAL (M6 S10, checker.go:3421): the mapped-type constraint family —
        // the name-type arm when the mapped type has one, else the
        // constraint arm.
        let t = self.get_type_from_mapped_type_node(node);
        let name_type = self.get_name_type_from_mapped_type_impl(t);
        if let Some(name_type) = name_type {
            let string_number_symbol_type = self.string_number_symbol_type();
            self.check_type_assignable_to(
                name_type,
                string_number_symbol_type,
                d.name_type,
                None,
            );
        } else {
            let constraint_type = crate::relater::get_constraint_type_from_mapped_type(self, t);
            let string_number_symbol_type = self.string_number_symbol_type();
            let constraint_node = self
                .store()
                .node(d.type_parameter)
                .as_type_parameter_declaration()
                .expect("TypeParameterDeclaration data")
                .constraint;
            self.check_type_assignable_to(
                constraint_type,
                string_number_symbol_type,
                constraint_node,
                None,
            );
        }
    }

    /// Go: `func (c *Checker) checkFunctionDeclaration(node)` (checker.go:3436).
    pub(crate) fn check_function_declaration(&mut self, node: NodeId) {
        self.check_function_or_method_declaration(node);
        // TODO(port M6 W3): checkGrammarForGenerator; permissive.
        self.check_collisions_for_declaration_name_stub(node, self.store().node(node).name());
    }

    /// Go: `func (c *Checker) checkFunctionOrMethodDeclaration(node)` (checker.go:3441).
    pub(crate) fn check_function_or_method_declaration(&mut self, node: NodeId) {
        self.check_decorators(node);
        self.check_signature_declaration(node);
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), node);
        // Do not use hasDynamicName here, because that returns false for
        // well known symbols.
        if let Some(name) = self.store().node(node).name() {
            if self.store().node(name).kind == Kind::ComputedPropertyName {
                let t = self.check_grammar_computed_property_name_stub(name);
                let _ = t;
            }
        }
        if self.has_bindable_name(node) {
            // first we want to check the local symbol that contain this
            // declaration
            let symbol = self.get_symbol_of_declaration(node).unwrap_or_default();
            let local_symbol = local_symbol_of(self.store(), node).unwrap_or(symbol);
            // Since the javascript won't do semantic analysis like
            // typescript, ignore javascript function declarations so that
            // redeclaring a function in a JS file is not reported as a
            // duplicate.
            if !self
                .store()
                .node(node)
                .flags
                .intersects(NF::JAVASCRIPT_FILE)
            {
                self.check_function_or_constructor_symbol(local_symbol);
            }
            if self.program_symbol(symbol).parent.is_some() {
                // run check on export symbol to check that modifiers agree
                // across all exported declarations
                self.check_function_or_constructor_symbol(symbol);
            }
        }
        let body = function_body_of(self.store(), node);
        self.check_source_element(body);
        let return_type_annotation = self.get_return_type_from_annotation(node);
        self.check_all_code_paths_in_non_void_function_return_or_throw(node, return_type_annotation);
        // TODO(port M6 W4): node.FunctionLikeData().FullSignature — the
        // JSDoc @type full-signature seam; permissive (the FunctionLike
        // payload's full_signature lands with the JSDoc wave).
        if signature_type_node(self.store(), node).is_none() {
            // Report an implicit any error if there is no body, no explicit
            // return type, and node is not a private method in an ambient
            // context
            if body.is_none() && !is_private_within_ambient(self.store(), node) {
                let any_type = self.any_type();
                self.report_implicit_any(node, any_type, crate::typeofsymbol::WideningKind::Normal);
            }
            if function_flags.intersects(FunctionFlags::GENERATOR) && body.is_some() {
                // A generator with a body and no type annotation can still
                // cause errors; trigger them by checking its return type.
                if let Some(signature) = self.get_signature_from_declaration(node) {
                    let return_type = self.get_return_type_of_signature(signature);
                    let _ = return_type;
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkFunctionOrConstructorSymbol(symbol)` (checker.go:3504).
    pub(crate) fn check_function_or_constructor_symbol(&mut self, symbol: SymbolId) {
        // Only check the symbol once
        if !self.value_symbol_links.get(symbol).function_or_constructor_checked {
            self.value_symbol_links.get(symbol).function_or_constructor_checked = true;
            self.check_function_or_constructor_symbol_worker(symbol);
        }
    }

    /// Go: `func (c *Checker) checkFunctionOrConstructorSymbolWorker(symbol)`
    /// (checker.go:3510, ~232 lines).
    pub(crate) fn check_function_or_constructor_symbol_worker(&mut self, symbol: SymbolId) {
        let flags_to_check = ModifierFlags::EXPORT
            .union(ModifierFlags::AMBIENT)
            .union(ModifierFlags::PRIVATE)
            .union(ModifierFlags::PROTECTED)
            .union(ModifierFlags::ABSTRACT);
        let mut some_node_flags = ModifierFlags::NONE;
        let mut all_node_flags = flags_to_check;
        let mut some_have_question_token = false;
        let mut all_have_question_token = true;
        let mut has_overloads = false;
        let mut body_declaration: Option<NodeId> = None;
        let mut last_seen_non_ambient_declaration: Option<NodeId> = None;
        let mut previous_declaration: Option<NodeId> = None;
        let is_constructor = self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::CONSTRUCTOR);
        let mut duplicate_function_declaration = false;
        let mut multiple_constructor_implementation = false;
        let mut has_non_ambient_class = false;
        let mut function_declarations: Vec<NodeId> = Vec::new();
        let declarations = self.program_symbol(symbol).declarations.clone();
        for &node in &declarations {
            let in_ambient_context = self.store().node(node).flags.intersects(NF::AMBIENT);
            let parent = self.store().node(node).parent.get();
            let parent_kind = self.store().node(parent).kind;
            let in_ambient_context_or_interface = in_ambient_context
                || (parent_kind == Kind::InterfaceDeclaration || parent_kind == Kind::TypeLiteral);
            // check if declarations are consecutive only if they are non-ambient
            if in_ambient_context_or_interface {
                previous_declaration = None;
            }
            let node_kind = self.store().node(node).kind;
            if is_class_like(self.store(), node) && !in_ambient_context {
                has_non_ambient_class = true;
            }
            if node_kind == Kind::FunctionDeclaration
                || node_kind == Kind::MethodDeclaration
                || node_kind == Kind::MethodSignature
                || node_kind == Kind::Constructor
            {
                function_declarations.push(node);
                let current_node_flags = self.get_effective_declaration_flags(node, flags_to_check);
                some_node_flags |= current_node_flags;
                all_node_flags &= current_node_flags;
                let optional = u::is_optional_declaration(self.store(), node);
                some_have_question_token = some_have_question_token || optional;
                all_have_question_token = all_have_question_token && optional;
                let body_is_present = function_body_of(self.store(), node).is_some();
                if body_is_present && body_declaration.is_some() {
                    if is_constructor {
                        multiple_constructor_implementation = true;
                    } else {
                        duplicate_function_declaration = true;
                    }
                } else if let Some(previous) = previous_declaration {
                    let previous_parent = self.store().node(previous).parent.get();
                    let reparsed = self
                        .store()
                        .node(previous)
                        .flags
                        .intersects(NF::REPARSED);
                    if previous_parent == parent
                        && self.store().node(previous).end() != self.store().node(node).pos()
                        && !reparsed
                    {
                        self.report_implementation_expected_error(previous);
                    }
                }
                if body_is_present {
                    if body_declaration.is_none() {
                        body_declaration = Some(node);
                    }
                } else {
                    has_overloads = true;
                }
                previous_declaration = Some(node);
                if !in_ambient_context_or_interface {
                    last_seen_non_ambient_declaration = Some(node);
                }
            }
        }
        if multiple_constructor_implementation {
            for &declaration in &function_declarations {
                self.error(declaration, &diag::MULTIPLE_CONSTRUCTOR_IMPLEMENTATIONS_ARE_NOT_ALLOWED, &[]);
            }
        }
        if duplicate_function_declaration {
            for &declaration in &function_declarations {
                let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), declaration);
                self.error(
                    name.unwrap_or(declaration),
                    &diag::DUPLICATE_FUNCTION_IMPLEMENTATION,
                    &[],
                );
            }
        }
        if has_non_ambient_class
            && !is_constructor
            && self.program_symbol(symbol).flags.intersects(SymbolFlags::FUNCTION)
            && !declarations.is_empty()
        {
            let mut related_diagnostics: Vec<tsc_ast::Diagnostic> = Vec::new();
            for &declaration in &declarations {
                if self.store().node(declaration).kind == Kind::ClassDeclaration {
                    related_diagnostics.push(u::new_diagnostic_for_node(
                        self.store(),
                        declaration,
                        &diag::CONSIDER_ADDING_A_DECLARE_MODIFIER_TO_THIS_CLASS,
                        vec![],
                    ));
                }
            }
            for &declaration in &declarations {
                let message = match self.store().node(declaration).kind {
                    Kind::ClassDeclaration => Some(&diag::CLASS_DECLARATION_CANNOT_IMPLEMENT_OVERLOAD_LIST_FOR_0),
                    Kind::FunctionDeclaration => {
                        Some(&diag::FUNCTION_WITH_BODIES_CAN_ONLY_MERGE_WITH_CLASSES_THAT_ARE_AMBIENT)
                    }
                    _ => None,
                };
                if let Some(message) = message {
                    let name =
                        tsc_binder::ast_shims::get_name_of_declaration(self.store(), declaration);
                    let mut diagnostic =
                        self.error(name.unwrap_or(declaration), message, &[self.program_symbol(symbol).name.clone()]);
                    diagnostic.set_related_info(related_diagnostics.clone());
                }
            }
        }
        // Abstract methods can't have an implementation -- in particular,
        // they don't need one.
        if let Some(last) = last_seen_non_ambient_declaration {
            if function_body_of(self.store(), last).is_none()
                && !crate::ast_shims::has_modifier(self.store().node(last), ModifierFlags::ABSTRACT)
                && !u::is_optional_declaration(self.store(), last)
            {
                self.report_implementation_expected_error(last);
            }
        }
        if has_overloads {
            // Go: checkFlagAgreementBetweenOverloads — the per-file
            // canonical-flag grouping; the effective-flags seam is S8's
            // getEffectiveDeclarationFlags (permissive NONE ⇒ no
            // deviations). The XOR math is preserved on that answer.
            let some_but_not_all = ModifierFlags(some_node_flags.bits() ^ all_node_flags.bits());
            if !some_but_not_all.is_empty() {
                let canonical = body_declaration
                    .or_else(|| declarations.first().copied())
                    .unwrap_or_default();
                let canonical_flags = self.get_effective_declaration_flags(canonical, flags_to_check);
                for &node in &declarations {
                    let deviation = ModifierFlags(
                        self.get_effective_declaration_flags(node, flags_to_check).bits()
                            ^ canonical_flags.bits(),
                    );
                    if deviation.intersects(ModifierFlags::EXPORT) {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), node);
                        self.error(
                            name.unwrap_or(node),
                            &diag::OVERLOAD_SIGNATURES_MUST_ALL_BE_EXPORTED_OR_NON_EXPORTED,
                            &[],
                        );
                    } else if deviation.intersects(ModifierFlags::AMBIENT) {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), node);
                        self.error(
                            name.unwrap_or(node),
                            &diag::OVERLOAD_SIGNATURES_MUST_ALL_BE_AMBIENT_OR_NON_AMBIENT,
                            &[],
                        );
                    } else if deviation
                        .intersects(ModifierFlags::PRIVATE.union(ModifierFlags::PROTECTED))
                    {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), node);
                        self.error(
                            name.unwrap_or(node),
                            &diag::OVERLOAD_SIGNATURES_MUST_ALL_BE_PUBLIC_PRIVATE_OR_PROTECTED,
                            &[],
                        );
                    } else if deviation.intersects(ModifierFlags::ABSTRACT) {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), node);
                        self.error(
                            name.unwrap_or(node),
                            &diag::OVERLOAD_SIGNATURES_MUST_ALL_BE_ABSTRACT_OR_NON_ABSTRACT,
                            &[],
                        );
                    }
                }
            }
            // Go: checkQuestionTokenAgreementBetweenOverloads
            if some_have_question_token != all_have_question_token {
                let canonical = body_declaration
                    .or_else(|| declarations.first().copied())
                    .unwrap_or_default();
                let canonical_optional = u::is_optional_declaration(self.store(), canonical);
                for &node in &declarations {
                    if u::is_optional_declaration(self.store(), node) != canonical_optional {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), node);
                        self.error(
                            name.unwrap_or(node),
                            &diag::OVERLOAD_SIGNATURES_MUST_ALL_BE_OPTIONAL_OR_REQUIRED,
                            &[],
                        );
                    }
                }
            }
            if let Some(body_declaration) = body_declaration {
                let Some(body_signature) = self.get_signature_from_declaration(body_declaration)
                else {
                    return;
                };
                let signature_kind = if is_constructor { SignatureKind::Construct } else { SignatureKind::Call };
                let symbol_type = self.get_type_of_symbol(symbol);
                let signatures =
                    relater::get_signatures_of_type(self, symbol_type, signature_kind);
                for &signature in &signatures {
                    if !self.is_implementation_compatible_with_overload(body_signature, signature) {
                        let error_node = self.signatures[signature.0 as usize]
                            .declaration
                            .unwrap_or(body_declaration);
                        let mut diagnostic = self.error(
                            error_node,
                            &diag::THIS_OVERLOAD_SIGNATURE_IS_NOT_COMPATIBLE_WITH_ITS_IMPLEMENTATION_SIGNATURE,
                            &[],
                        );
                        diagnostic.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            body_declaration,
                            &diag::THE_IMPLEMENTATION_SIGNATURE_IS_DECLARED_HERE,
                            vec![],
                        ));
                        break;
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) reportImplementationExpectedError(node)` (the
    /// worker's local closure, checker.go:3572-3625).
    pub(crate) fn report_implementation_expected_error(&mut self, node: NodeId) {
        let name = self.store().node(node).name();
        if name.is_some_and(|n| self.store().node(n).pos() == self.store().node(n).end()) {
            return;
        }
        let parent = self.store().node(node).parent.get();
        let siblings = children_of(self.store(), parent);
        let mut subsequent_node: Option<NodeId> = None;
        let mut seen = false;
        for &child in &siblings {
            if seen {
                subsequent_node = Some(child);
                break;
            }
            seen = child == node;
        }
        // We may be here because of some extra nodes between overloads that
        // could not be parsed into a valid node.
        if let Some(subsequent) = subsequent_node {
            if self.store().node(subsequent).pos() == self.store().node(node).end()
                && self.store().node(subsequent).kind == self.store().node(node).kind
            {
                let subsequent_name = self.store().node(subsequent).name();
                let error_node = subsequent_name.unwrap_or(subsequent);
                if let (Some(name), Some(subsequent_name)) = (name, subsequent_name) {
                    let name_kind = self.store().node(name).kind;
                    let subsequent_kind = self.store().node(subsequent_name).kind;
                    let same = (name_kind == Kind::PrivateIdentifier
                        && subsequent_kind == Kind::PrivateIdentifier
                        && tsc_ast::node_text(self.store(), name)
                            == tsc_ast::node_text(self.store(), subsequent_name))
                        || (name_kind == Kind::ComputedPropertyName
                            && subsequent_kind == Kind::ComputedPropertyName)
                        || (is_property_name_literal(self.store(), name)
                            && is_property_name_literal(self.store(), subsequent_name)
                            && tsc_ast::node_text(self.store(), name)
                                == tsc_ast::node_text(self.store(), subsequent_name));
                    if same {
                        let node_kind = self.store().node(node).kind;
                        let report_error = (node_kind == Kind::MethodDeclaration
                            || node_kind == Kind::MethodSignature)
                            && is_static_modifier(self.store(), node)
                                != is_static_modifier(self.store(), subsequent);
                        if report_error {
                            let message = if is_static_modifier(self.store(), node) {
                                &diag::FUNCTION_OVERLOAD_MUST_BE_STATIC
                            } else {
                                &diag::FUNCTION_OVERLOAD_MUST_NOT_BE_STATIC
                            };
                            self.error(error_node, message, &[]);
                        }
                        return;
                    }
                }
                if function_body_of(self.store(), subsequent).is_some() {
                    let name_text = tsc_scanner::utilities::declaration_name_to_string(
                        self.store(),
                        name,
                    );
                    self.error(error_node, &diag::FUNCTION_IMPLEMENTATION_NAME_MUST_BE_0, &[name_text]);
                    return;
                }
            }
        }
        let error_node = name.unwrap_or(node);
        if self.store().node(node).kind == Kind::Constructor {
            self.error(error_node, &diag::CONSTRUCTOR_IMPLEMENTATION_IS_MISSING, &[]);
        } else {
            if crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::ABSTRACT) {
                self.error(
                    error_node,
                    &diag::ALL_DECLARATIONS_OF_AN_ABSTRACT_METHOD_MUST_BE_CONSECUTIVE,
                    &[],
                );
            } else {
                self.error(
                    error_node,
                    &diag::FUNCTION_IMPLEMENTATION_IS_MISSING_OR_NOT_IMMEDIATELY_FOLLOWING_THE_DECLARATION,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) isImplementationCompatibleWithOverload(implementation, overload) bool`
    /// (checker.go:3745).
    pub(crate) fn is_implementation_compatible_with_overload(
        &mut self,
        implementation: crate::types::SignatureId,
        overload: crate::types::SignatureId,
    ) -> bool {
        let erased_source = relater::get_erased_signature(self, implementation);
        let erased_target = relater::get_erased_signature(self, overload);
        // First see if the return types are compatible in either direction.
        let source_return_type = self.get_return_type_of_signature(erased_source);
        let target_return_type = self.get_return_type_of_signature(erased_target);
        if target_return_type == self.void_type()
            || self.is_type_related_to(
                target_return_type,
                source_return_type,
                crate::relater::RelationKind::Assignable,
            )
            || self.is_type_related_to(
                source_return_type,
                target_return_type,
                crate::relater::RelationKind::Assignable,
            )
        {
            self.is_signature_assignable_to(erased_source, erased_target, true /*ignoreReturnTypes*/)
        } else {
            false
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers for the type-node batch.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `ast.IsClassLike`.
pub(crate) fn is_class_like(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::ClassDeclaration | Kind::ClassExpression
    )
}

/// Go: `ast.GetClassExtendsHeritageElement` — the class's `extends` heritage
/// element (ExpressionWithTypeArguments) or None.
pub(crate) fn get_class_extends_heritage_element(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let n = store.node(node);
    let clauses = match n.kind {
        Kind::ClassDeclaration => n.as_class_declaration().expect("data").heritage_clauses.as_ref(),
        Kind::ClassExpression => n.as_class_expression().expect("data").heritage_clauses.as_ref(),
        _ => None,
    }?;
    for &clause in &clauses.nodes {
        let c = store.node(clause);
        if c.kind == Kind::HeritageClause
            && c.as_heritage_clause().expect("HeritageClause data").token == Kind::ExtendsKeyword
        {
            let elements = c
                .as_heritage_clause()
                .expect("HeritageClause data")
                .types
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            return elements.first().copied();
        }
    }
    None
}

/// Go: `isInstancePropertyWithInitializerOrPrivateIdentifierProperty`.
pub(crate) fn is_instance_property_with_initializer_or_private_identifier_property(
    store: &dyn NodeStore,
    n: NodeId,
) -> bool {
    let node = store.node(n);
    // Go: IsPrivateIdentifierClassElementDeclaration
    let name_kind = node.name().map(|nm| store.node(nm).kind);
    if name_kind == Some(Kind::PrivateIdentifier)
        && matches!(node.kind, Kind::PropertyDeclaration | Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor)
    {
        return true;
    }
    node.kind == Kind::PropertyDeclaration
        && !has_static_modifier(store, n)
        && crate::ast_shims::initializer_of(store, n).is_some()
}

/// Go: `superCallIsRootLevelInConstructor`.
pub(crate) fn super_call_is_root_level_in_constructor(
    store: &dyn NodeStore,
    super_call: NodeId,
    body: NodeId,
) -> bool {
    let mut parent = store.node(super_call).parent.get();
    // Go: WalkUpParenthesizedExpressions
    while matches!(store.node(parent).kind, Kind::ParenthesizedExpression) {
        parent = store.node(parent).parent.get();
    }
    let is_expression_statement = store.node(parent).kind == Kind::ExpressionStatement;
    let grandparent = store.node(parent).parent.get();
    is_expression_statement && grandparent == body
}

/// Go: `nodeImmediatelyReferencesSuperOrThis`.
pub(crate) fn node_immediately_references_super_or_this(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::SuperKeyword | Kind::ThisKeyword => true,
        Kind::ArrowFunction
        | Kind::FunctionDeclaration
        | Kind::FunctionExpression
        | Kind::PropertyDeclaration => false,
        Kind::Block => {
            let parent_kind = store.node(n.parent.get()).kind;
            if matches!(
                parent_kind,
                Kind::Constructor
                    | Kind::MethodDeclaration
                    | Kind::GetAccessor
                    | Kind::SetAccessor
            ) {
                return false;
            }
            children_of(store, node)
                .iter()
                .any(|&c| node_immediately_references_super_or_this(store, c))
        }
        _ => children_of(store, node)
            .iter()
            .any(|&c| node_immediately_references_super_or_this(store, c)),
    }
}

/// Go: `findFirstSuperCall`'s visitor closure.
fn find_first_super_call_visit(store: &dyn NodeStore, node: NodeId, super_call: &mut Option<NodeId>) -> bool {
    if u::is_super_call(store, node) {
        *super_call = Some(node);
        return true;
    }
    if is_function_like(store, node) {
        return false;
    }
    for child in children_of(store, node) {
        if find_first_super_call_visit(store, child, super_call) {
            return true;
        }
    }
    false
}

/// Go: `getTypePredicateParent`.
pub(crate) fn get_type_predicate_parent(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let parent = store.node(node).parent.get();
    match store.node(parent).kind {
        Kind::ArrowFunction
        | Kind::CallSignature
        | Kind::FunctionDeclaration
        | Kind::FunctionExpression
        | Kind::FunctionType
        | Kind::MethodDeclaration
        | Kind::MethodSignature => {
            if signature_type_node(store, parent) == Some(node) {
                Some(parent)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The `checkPropertyOrAccessor` closure of checkObjectTypeForDuplicate-
/// Declarations. `symbol_name` is the Go `symbol.Name` read.
#[allow(clippy::too_many_arguments)]
fn check_property_or_accessor(
    _c: &mut Checker,
    instance_names: &mut FxHashMap<String, i32>,
    static_names: &mut FxHashMap<String, i32>,
    declaration: NodeId,
    kind: i32,
    is_static: bool,
    symbol_name: Option<&String>,
) {
    let Some(symbol_name) = symbol_name else { return };
    let names = if is_static { static_names } else { instance_names };
    let state = names.get(symbol_name).copied().unwrap_or(0);
    match state {
        0 => {
            names.insert(symbol_name.clone(), kind);
        }
        1 | 2
            if (kind != 2 || state == 1) => {
                // Error on second property or combination of property and
                // accessor
                let _ = declaration;
                // reported below via the caller's node context
                names.insert(symbol_name.clone(), 3);
            }
        _ => {}
    }
}

/// Go: `ast.IsPropertyNameLiteral` — identifier/string/numeric literal names.
pub(crate) fn is_property_name_literal(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::Identifier | Kind::StringLiteral | Kind::NumericLiteral
    )
}

/// Go: `hasStaticModifier` / `ast.IsStatic`.
pub(crate) fn has_static_modifier(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::ast_shims::has_modifier(store.node(node), ModifierFlags::STATIC)
}

/// `ast.IsStatic` alias used by the duplicate-member walk.
pub(crate) fn is_static_modifier(store: &dyn NodeStore, node: NodeId) -> bool {
    has_static_modifier(store, node)
}

/// Go: `isPrivateWithinAmbient` (expressions.go) — a private-named method
/// inside an ambient context needs no implicit-any report.
pub(crate) fn is_private_within_ambient(store: &dyn NodeStore, node: NodeId) -> bool {
    let name_kind = store.node(node).name().map(|n| store.node(n).kind);
    if name_kind != Some(Kind::PrivateIdentifier) {
        return false;
    }
    let mut current = store.node(node).parent.get();
    while current != NodeId::NONE {
        if store.node(current).flags.intersects(NF::AMBIENT) {
            return true;
        }
        current = store.node(current).parent.get();
    }
    false
}

/// The element type of a tuple member node (OptionalType/RestType wrap one).
fn element_type_of(store: &dyn NodeStore, e: NodeId) -> NodeId {
    let n = store.node(e);
    match n.kind {
        Kind::OptionalType => n.as_optional_type_node().expect("data").type_,
        Kind::RestType => n.as_rest_type_node().expect("data").type_,
        _ => e,
    }
}

/// TODO(port M6 S4): `isConstTypeReference` — the const-assertion fact;
/// permissive false until the S4 expression wave lands.
fn is_const_type_reference_stub(_node: NodeId) -> bool {
    false
}

/// Go: `func GetDeclarationOfKind(symbol, kind) *Node` — first declaration
/// of the given kind, in declaration order.
pub(crate) fn get_declaration_of_kind(
    store: &dyn NodeStore,
    symbol: &tsc_ast::Symbol,
    kind: Kind,
) -> Option<NodeId> {
    symbol
        .declarations
        .iter()
        .copied()
        .find(|&d| store.node(d).kind == kind)
}
// ────────────────────────────────────────────────────────────────────────────
// M6 S3 batch 4 — the class/interface/enum/module family (Go checker.go:
// 4304-5394). Same Go-name/shape discipline; unlanded leaves stay bannered
// permissive stubs (the class/inheritance seams are S5/S7/S10/W3).
// ────────────────────────────────────────────────────────────────────────────

use tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_EQUALS;
use tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_STAR;

impl Checker {
    /// Go: `func (c *Checker) checkClassDeclaration(node)` (checker.go:4310).
    pub(crate) fn check_class_declaration(&mut self, node: NodeId) {
        // TODO(port M6 W3): the legacy-decorator + static-private-identifier
        // grammar gate (checkGrammarModifiers covers the modern path);
        // permissive.
        if self.store().node(node).name().is_none()
            && !crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::DEFAULT)
        {
            self.grammar_error_on_first_token(
                node,
                &diag::A_CLASS_DECLARATION_WITHOUT_THE_DEFAULT_MODIFIER_MUST_HAVE_A_NAME,
                &[],
            );
        }
        self.check_class_like_declaration(node);
        let members = members_of(self.store(), node);
        self.check_source_elements(&members);
        self.check_constructor_declared_properties(node);
        self.register_for_unused_identifiers_check(node);
    }

    /// Go: `func (c *Checker) checkConstructorDeclaredProperties(node)`
    /// (checker.go:4325) — the JS `this`-assignment family; non-JS files
    /// bail out immediately.
    pub(crate) fn check_constructor_declared_properties(&mut self, node: NodeId) {
        if !tsc_ast::utilities::is_in_js_file(self.store().node(node)) {
            return;
        }
        // TODO(port M6 S4): isConstructorDeclaredThisProperty — the JS
        // expando property family; permissive skip.
        let _ = node;
    }

    /// Go: `func (c *Checker) checkClassLikeDeclaration(node)` (checker.go:4340).
    pub(crate) fn check_class_like_declaration(&mut self, node: NodeId) {
        // TODO(port M6 W3): checkGrammarClassLikeDeclaration; permissive.
        self.check_decorators(node);
        self.check_collisions_for_declaration_name_stub(node, self.store().node(node).name());
        let type_parameters = type_parameters_of(self.store(), node);
        self.check_type_parameters(&type_parameters);
        self.check_exports_on_merged_declarations(node);
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        let class_type = self.get_declared_type_of_symbol(symbol);
        // Go: c.getTypeWithThisArgument(classType, nil, false) — the S8
        // Option<this_arg> seam carries Go's nil verbatim.
        let type_with_this = relater::get_type_with_this_argument(self, class_type, None, false);
        let static_type = self.get_type_of_symbol(symbol);
        self.check_type_parameter_lists_identical(symbol);
        self.check_function_or_constructor_symbol(symbol);
        self.check_object_type_for_duplicate_declarations(node, true /*checkPrivateNames*/);

        // Only check for reserved static identifiers on non-ambient context.
        let node_in_ambient_context = self.store().node(node).flags.intersects(NF::AMBIENT);
        if !node_in_ambient_context {
            self.check_class_for_static_property_name_conflicts(node);
        }

        if let Some(base_type_node) = get_class_extends_heritage_element(self.store(), node) {
            self.check_source_elements(&type_arguments_of(self.store(), base_type_node));
            let base_types = self.get_base_types(class_type);
            if !base_types.is_empty() {
                let base_type = base_types[0];
                // TODO(port M6 W4): checkJSDocAugmentsTagMatchesExtends —
                // the JS @augments JSDoc family; permissive (non-JS files
                // return immediately in Go).
                let base_constructor_type = self.get_base_constructor_type_of_class(class_type);
                let static_base_type = self.get_apparent_type(base_constructor_type);
                self.check_base_type_accessibility(static_base_type, base_type_node);
                let base_expr = self
                    .store()
                    .node(base_type_node)
                    .as_expression_with_type_arguments()
                    .expect("ExpressionWithTypeArguments data")
                    .expression;
                self.check_source_element(Some(base_expr));
                let base_type_arguments = type_arguments_of(self.store(), base_type_node);
                if !base_type_arguments.is_empty() {
                    self.check_source_elements(&base_type_arguments);
                    // TODO(port M6 S7): getConstructorsForTypeArguments +
                    // getInstantiatedConstructorsForTypeArguments — the
                    // deferred-constructor instantiation seams; permissive
                    // (checkTypeArgumentConstraints on the class's own type
                    // parameters approximates until S7 lands).
                    let _ = base_type_arguments;
                }
                let this_type = self
                    .type_(class_type)
                    .as_interface_type()
                    .expect("InterfaceType data")
                    .this_type;
                let base_with_this = relater::get_type_with_this_argument(
                    self,
                    base_type,
                    this_type,
                    false,
                );
                let class_name = self.store().node(node).name();
                if !self.check_type_assignable_to(type_with_this, base_with_this, class_name, None)
                {
                    self.issue_member_specific_error(
                        node,
                        type_with_this,
                        base_with_this,
                        &diag::CLASS_0_INCORRECTLY_EXTENDS_BASE_CLASS_1,
                    );
                } else {
                    // Report static side error only when instance type is
                    // assignable
                    let without_signatures =
                        self.get_type_without_signatures(static_base_type);
                    self.check_type_assignable_to(
                        static_type,
                        without_signatures,
                        class_name,
                        Some(&diag::CLASS_STATIC_SIDE_0_INCORRECTLY_EXTENDS_BASE_CLASS_STATIC_SIDE_1),
                    );
                }
                if self
                    .type_(base_constructor_type)
                    .flags
                    .intersects(TypeFlags::TYPE_PARAMETER)
                {
                    if !self.is_mixin_constructor_type(static_type) {
                        let error_node = class_name.unwrap_or(node);
                        self.error(
                            error_node,
                            &diag::A_MIXIN_CLASS_MUST_HAVE_A_CONSTRUCTOR_WITH_A_SINGLE_REST_PARAMETER_OF_TYPE_ANY,
                            &[],
                        );
                    } else {
                        let construct_signatures = relater::get_signatures_of_type(
                            self,
                            base_constructor_type,
                            SignatureKind::Construct,
                        );
                        let has_abstract = construct_signatures.iter().any(|&sig| {
                            self.signatures[sig.0 as usize]
                                .flags
                                .intersects(crate::types::SignatureFlags::ABSTRACT)
                        });
                        if has_abstract
                            && !crate::ast_shims::has_modifier(
                                self.store().node(node),
                                ModifierFlags::ABSTRACT,
                            )
                        {
                            let error_node = class_name.unwrap_or(node);
                            self.error(
                                error_node,
                                &diag::A_MIXIN_CLASS_THAT_EXTENDS_FROM_A_TYPE_VARIABLE_CONTAINING_AN_ABSTRACT_CONSTRUCT_SIGNATURE_MUST_ALSO_BE_DECLARED_ABSTRACT,
                                &[],
                            );
                        }
                    }
                }
                let static_base_is_class =
                    self.type_(static_base_type).symbol.is_some_and(|s| {
                        self.program_symbol(s).flags.intersects(SymbolFlags::CLASS)
                    }) && self
                        .type_(base_constructor_type)
                        .flags
                        .intersects(TypeFlags::TYPE_PARAMETER);
                if !static_base_is_class {
                    // When the static base type is a "class-like" constructor
                    // function (but not actually a class), we verify that
                    // all instantiated base constructor signatures return the
                    // same type.
                    // TODO(port M6 S7): getInstantiatedConstructorsForType-
                    // Arguments — permissive (the constructors list rests).
                }
                self.check_kinds_of_property_member_overrides(class_type, base_type);
            }
        }
        self.check_members_for_override_modifier(node, class_type, type_with_this, static_type);
        // Go: the implements heritage walk (checker.go:4405-4435).
        for type_ref_node in get_implements_heritage_clause_elements(self.store(), node) {
            if self.store().node(type_ref_node).kind == Kind::ExpressionWithTypeArguments {
                let expr = self
                    .store()
                    .node(type_ref_node)
                    .as_expression_with_type_arguments()
                    .expect("ExpressionWithTypeArguments data")
                    .expression;
                // Go: !IsEntityNameExpression(expr) || IsOptionalChain(expr)
                let bad = !u::is_entity_name_expression(self.store(), expr)
                    || tsc_binder::ast_shims::is_optional_chain_root(self.store(), expr);
                if bad {
                    self.error(
                        expr,
                        &diag::A_CLASS_CAN_ONLY_IMPLEMENT_AN_IDENTIFIER_SLASHQUALIFIED_NAME_WITH_OPTIONAL_TYPE_ARGUMENTS,
                        &[],
                    );
                }
            }
            self.check_type_reference_node(type_ref_node);
            let from_node = self.get_type_from_type_node(type_ref_node);
            let t = self.get_reduced_type(from_node);
            if !relater::is_error_type(self, t) {
                if self.is_valid_base_type(t) {
                    let generic_diag = if self.type_(t).symbol.is_some_and(|s| {
                        self.program_symbol(s).flags.intersects(SymbolFlags::CLASS)
                    }) {
                        &diag::CLASS_0_INCORRECTLY_IMPLEMENTS_CLASS_1_DID_YOU_MEAN_TO_EXTEND_1_AND_INHERIT_ITS_MEMBERS_AS_A_SUBCLASS
                    } else {
                        &diag::CLASS_0_INCORRECTLY_IMPLEMENTS_INTERFACE_1
                    };
                    let this_type = self
                        .type_(class_type)
                        .as_interface_type()
                        .expect("InterfaceType data")
                        .this_type;
                    let base_with_this =
                        relater::get_type_with_this_argument(self, t, this_type, false);
                    if !self.check_type_assignable_to(type_with_this, base_with_this, None, None)
                    {
                        self.issue_member_specific_error(node, type_with_this, base_with_this, generic_diag);
                    }
                } else {
                    self.error(
                        type_ref_node,
                        &diag::A_CLASS_CAN_ONLY_IMPLEMENT_AN_OBJECT_TYPE_OR_INTERSECTION_OF_OBJECT_TYPES_WITH_STATICALLY_KNOWN_MEMBERS,
                        &[],
                    );
                }
            }
        }
        self.check_index_constraints(class_type, symbol, false /*isStaticIndex*/);
        self.check_index_constraints(static_type, symbol, true /*isStaticIndex*/);
        self.check_class_or_interface_for_duplicate_index_signatures(node);
        self.check_property_initialization(node);
    }

    /// Go: `func (c *Checker) checkJSDocAugmentsTagMatchesExtends(...)` —
    /// non-JS files return immediately; the JS tag walk is TODO(port M6 W4).
    #[allow(dead_code)] // lands with the W4 JS doc wave
    pub(crate) fn check_jsdoc_augments_tag_matches_extends(
        &mut self,
        node: NodeId,
        _base_type_node: NodeId,
        _base_type: TypeId,
    ) {
        if tsc_ast::utilities::is_in_js_file(self.store().node(node)) {
            // TODO(port M6 W4): EagerJSDoc + the @augments tag walk.
        }
    }

    /// Go: `func (c *Checker) checkClassForStaticPropertyNameConflicts(node)`
    /// (checker.go:4455).
    pub(crate) fn check_class_for_static_property_name_conflicts(&mut self, node: NodeId) {
        // Go: c.compilerOptions.GetUseDefineForClassFields() gate.
        // TODO(port M6 W8): the useDefineForClassFields emit option seam;
        // permissive false (Go's non-define default).
        if self.emit_standard_class_fields {
            return;
        }
        let node_symbol_name = match self.get_symbol_of_declaration(node) {
            Some(symbol) => crate::relater::symbol_to_string(self, symbol),
            None => String::new(),
        };
        for member in members_of(self.store(), node) {
            let member_name_node = self.store().node(member).name();
            if has_static_modifier(self.store(), member) {
                if let Some(member_name_node) = member_name_node {
                    // REAL (M6 S10, checker.go:4463):
                    // getEffectivePropertyNameForPropertyNameNode — the
                    // computed-name resolution (typeofsymbol.rs's body).
                    let (member_name, _is_late_bound_name) = self
                        .get_effective_property_name_for_property_name_node(
                            member_name_node,
                        );
                    match member_name.as_str() {
                        "name" | "length" | "caller" | "arguments" => {
                            self.error(
                                member_name_node,
                                &diag::STATIC_PROPERTY_0_CONFLICTS_WITH_BUILT_IN_PROPERTY_FUNCTION_0_OF_CONSTRUCTOR_FUNCTION_1,
                                &[member_name.to_string(), node_symbol_name.clone()],
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkTypeParameterListsIdentical(symbol)`
    /// (checker.go:4478).
    pub(crate) fn check_type_parameter_lists_identical(&mut self, symbol: SymbolId) {
        let declarations = self.program_symbol(symbol).declarations.clone();
        if declarations.len() == 1 {
            return;
        }
        if !self.declared_type_links.get(symbol).type_parameters_checked {
            self.declared_type_links.get(symbol).type_parameters_checked = true;
            let declarations =
                self.get_class_or_interface_declarations_of_symbol(symbol);
            if declarations.len() <= 1 {
                return;
            }
            let t = self.get_declared_type_of_symbol(symbol);
            // Go: t.AsInterfaceType().LocalTypeParameters() — the resolved
            // local type-parameter row.
            let local_type_parameters = self.get_local_type_parameters(t);
            let source_list: Vec<NodeId> = declarations.clone();
            let identical = self.are_type_parameters_identical(
                &source_list,
                &local_type_parameters,
                type_parameters_of,
            );
            if !identical {
                // Report an error on every conflicting declaration.
                let name = relater::symbol_to_string(self, symbol);
                for declaration in declarations {
                    let name_node = self.store().node(declaration).name();
                    if let Some(name_node) = name_node {
                        self.error(
                            name_node,
                            &diag::ALL_DECLARATIONS_OF_0_MUST_HAVE_IDENTICAL_TYPE_PARAMETERS,
                            std::slice::from_ref(&name),
                        );
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) getClassOrInterfaceDeclarationsOfSymbol(symbol)`
    /// (checker.go:4498).
    pub(crate) fn get_class_or_interface_declarations_of_symbol(
        &self,
        symbol: SymbolId,
    ) -> Vec<NodeId> {
        self.program_symbol(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|&d| {
                matches!(
                    self.store().node(d).kind,
                    Kind::ClassDeclaration | Kind::InterfaceDeclaration
                )
            })
            .collect()
    }

    /// Go: `func (c *Checker) areTypeParametersIdentical(declarations, targetParameters, getTypeParameterDeclarations) bool`
    /// (checker.go:4504).
    pub(crate) fn are_type_parameters_identical(
        &mut self,
        declarations: &[NodeId],
        target_parameters: &[TypeId],
        get_type_parameter_declarations: fn(&dyn NodeStore, NodeId) -> Vec<NodeId>,
    ) -> bool {
        let max_type_argument_count = target_parameters.len();
        // Go: getMinTypeArgumentCount — defaults count toward the minimum.
        let mut min_type_argument_count = target_parameters.len();
        for (i, &tp) in target_parameters.iter().enumerate().rev() {
            if self.get_default_from_type_parameter(tp).is_none() {
                min_type_argument_count = i + 1;
                break;
            }
        }
        for &declaration in declarations {
            // If this declaration has too few or too many type parameters,
            // we report an error.
            let source_parameters = get_type_parameter_declarations(self.store(), declaration);
            if source_parameters.len() < min_type_argument_count
                || source_parameters.len() > max_type_argument_count
            {
                return false;
            }
            for (i, &source) in source_parameters.iter().enumerate() {
                let target = target_parameters[i];
                // If the type parameter node does not have the same name as
                // the resolved type parameter at this position, we report an
                // error.
                let source_name = self
                    .store()
                    .node(source)
                    .name()
                    .map(|n| tsc_ast::node_text(self.store(), n));
                let target_name = self
                    .type_(target)
                    .symbol
                    .map(|s| self.program_symbol(s).name.clone());
                if source_name != target_name.as_deref() {
                    return false;
                }
                // If the type parameter node does not have an identical
                // constraintNode as the resolved type parameter at this
                // position, we report an error.
                let d = self
                    .store()
                    .node(source)
                    .as_type_parameter_declaration()
                    .expect("TypeParameterDeclaration data")
                    .clone();
                let target_constraint = self.get_constraint_of_type_parameter(target);
                if let Some(constraint_node) = d.constraint {
                    if let Some(target_constraint) = target_constraint {
                        let constraint_type = self.get_type_from_type_node(constraint_node);
                        if !self.is_type_identical_to(constraint_type, target_constraint) {
                            return false;
                        }
                    }
                }
                // If the type parameter node has a default and it is not
                // identical to the default for the type parameter at this
                // position, we report an error.
                let target_default = self.get_default_from_type_parameter(target);
                if let Some(default_node) = d.default_type {
                    if let Some(target_default) = target_default {
                        let default_type = self.get_type_from_type_node(default_node);
                        if !self.is_type_identical_to(default_type, target_default) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) checkBaseTypeAccessibility(t, node)` (checker.go:4543).
    pub(crate) fn check_base_type_accessibility(&mut self, t: TypeId, node: NodeId) {
        let signatures = relater::get_signatures_of_type(self, t, SignatureKind::Construct);
        // TODO(port M6 S8): getConstructorAccessibilityError — the
        // accessibility walk; permissive None (no private-ctor error).
        let accessibility_error = self.get_constructor_accessibility_error_stub(node, &signatures);
        if let Some(accessibility_error) = accessibility_error {
            let class_symbol = accessibility_error;
            let fully_qualified = relater::symbol_to_string(self, class_symbol);
            self.error(
                node,
                &diag::CANNOT_EXTEND_A_CLASS_0_CLASS_CONSTRUCTOR_IS_MARKED_AS_PRIVATE,
                &[fully_qualified],
            );
        }
    }

    /// TODO(port M6 S8): `getConstructorAccessibilityError` — the
    /// private-ctor accessibility walk; permissive None.
    fn get_constructor_accessibility_error_stub(
        &mut self,
        node: NodeId,
        signatures: &[crate::types::SignatureId],
    ) -> Option<SymbolId> {
        let _ = (node, signatures);
        None
    }

    /// Go: `func (c *Checker) issueMemberSpecificError(node, typeWithThis, baseWithThis, broadDiag)`
    /// (checker.go:4551).
    pub(crate) fn issue_member_specific_error(
        &mut self,
        node: NodeId,
        type_with_this: TypeId,
        base_with_this: TypeId,
        broad_diag: &'static Message,
    ) {
        // iterate over all implemented properties and issue errors on each
        // one which isn't compatible, rather than the class as a whole, if
        // possible
        let mut issued_member_error = false;
        let members = members_of(self.store(), node);
        for member in members {
            if has_static_modifier(self.store(), member) {
                continue;
            }
            if let Some(declared_prop) = self.get_symbol_of_declaration(member) {
                let prop_name = self.program_symbol(declared_prop).name.clone();
                if prop_name == "__computed" {
                    // Go: InternalSymbolNameComputed.
                    continue;
                }
                let prop = self.get_property_of_type(type_with_this, &prop_name);
                let base_prop = self.get_property_of_type(base_with_this, &prop_name);
                if let (Some(prop), Some(base_prop)) = (prop, base_prop) {
                    let prop_type = self.get_type_of_symbol(prop);
                    let base_prop_type = self.get_type_of_symbol(base_prop);
                    let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
                    let ok = self.check_type_assignable_to_ex(
                        prop_type,
                        base_prop_type,
                        self.store().node(member).name().or(Some(member)),
                        None,
                        Some(&mut diags),
                    );
                    if !ok && !diags.is_empty() {
                        let mut chained = diags.remove(0);
                        let prop_text = relater::symbol_to_string(self, declared_prop);
                        let type_text = self.type_to_string(type_with_this);
                        let base_text = self.type_to_string(base_with_this);
                        chained.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            member,
                            &diag::PROPERTY_0_IN_TYPE_1_IS_NOT_ASSIGNABLE_TO_THE_SAME_PROPERTY_IN_BASE_TYPE_2,
                            vec![prop_text, type_text, base_text],
                        ));
                        self.add_diagnostic(chained);
                        issued_member_error = true;
                    }
                }
            }
        }
        if !issued_member_error {
            // check again with diagnostics to generate a less-specific error
            self.check_type_assignable_to(
                type_with_this,
                base_with_this,
                self.store().node(node).name().or(Some(node)),
                Some(broad_diag),
            );
        }
    }

    /// Go: `func (c *Checker) getTypeWithoutSignatures(t)` (checker.go:4579).
    pub(crate) fn get_type_without_signatures(&mut self, t: TypeId) -> TypeId {
        let (td_flags, td_symbol) = {
            let td = self.type_(t);
            (td.flags, td.symbol)
        };
        if td_flags.intersects(TypeFlags::OBJECT) {
            let resolved = relater::resolve_structured_type_members(self, t);
            if !resolved.signatures.is_empty() {
                let result = self.new_object_type(ObjectFlags::ANONYMOUS, td_symbol);
                // Go: result.objectFlags |= ObjectFlagsMembersResolved and
                // copies members/properties from the resolved row.
                // TODO(port M6 S7): the members/properties copy — the
                // resolved row's tables; permissive empty until S7 lands.
                let _ = resolved;
                return result;
            }
        } else if td_flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self
                .type_(t)
                .as_intersection_type()
                .expect("IntersectionType data")
                .base
                .types
                .clone();
            let without = constituents
                .into_iter()
                .map(|t| self.get_type_without_signatures(t))
                .collect();
            return relater::get_intersection_type(self, without);
        }
        t
    }

    /// Go: `func (c *Checker) checkKindsOfPropertyMemberOverrides(t, baseType)`
    /// (checker.go:4607, ~200 ln) — the member-kind override matrix.
    pub(crate) fn check_kinds_of_property_member_overrides(&mut self, t: TypeId, base_type: TypeId) {
        // TypeScript 1.0 spec (April 2014): 8.2.3 (see Go's comment block).
        struct MemberInfo {
            missed_properties: Vec<String>,
            base_type_name: String,
            type_name: String,
        }
        let mut not_implemented_info: Vec<(NodeId, MemberInfo)> = Vec::new();

        let base_properties = relater::get_properties_of_type(self, base_type);
        'base_property_check: for &base_property in &base_properties {
            let base = self.get_target_symbol(base_property).unwrap_or(base_property);
            if self.program_symbol(base).flags.intersects(SymbolFlags::PROTOTYPE) {
                continue;
            }
            let base_name = self.program_symbol(base).name.clone();
            let Some(base_symbol) = relater::get_property_of_object_type(self, t, &base_name)
            else {
                continue;
            };
            let derived = self.get_target_symbol(base_symbol).unwrap_or(base_symbol);
            let base_declaration_flags = relater::get_declaration_modifier_flags_from_symbol(self, base);
            if derived == base {
                // derived class inherits base without override/redeclaration.
                if base_declaration_flags.intersects(ModifierFlags::ABSTRACT) {
                    // It is an error to inherit an abstract member without
                    // implementing it or being declared abstract.
                    let derived_class_decl = get_class_like_declaration_of_symbol(self, t);
                    let derived_is_abstract = derived_class_decl.is_some_and(|d| {
                        crate::ast_shims::has_modifier(self.store().node(d), ModifierFlags::ABSTRACT)
                    });
                    if derived_class_decl.is_none_or(|_| !derived_is_abstract) {
                        // Searches other base types for a declaration that
                        // would satisfy the inherited abstract member.
                        let other_base_types = self.get_base_types(t);
                        for other_base_type in other_base_types {
                            if other_base_type == base_type {
                                continue;
                            }
                            if let Some(other_prop) =
                                relater::get_property_of_object_type(self, other_base_type, &base_name)
                            {
                                let other_target =
                                    self.get_target_symbol(other_prop).unwrap_or(other_prop);
                                if base != other_target {
                                    // Derived property exists elsewhere.
                                    continue 'base_property_check;
                                }
                            }
                        }
                        let base_type_name = self.type_to_string(base_type);
                        let type_name = self.type_to_string(t);
                        let missed = relater::symbol_to_string(self, base_property);
                        let derived_node = derived_class_decl.unwrap_or_default();
                        match not_implemented_info
                            .iter_mut()
                            .find(|(n, _)| *n == derived_node)
                        {
                            Some((_, info)) => info.missed_properties.push(missed),
                            None => not_implemented_info.push((
                                derived_node,
                                MemberInfo {
                                    missed_properties: vec![missed],
                                    base_type_name,
                                    type_name,
                                },
                            )),
                        }
                    }
                }
            } else {
                // derived overrides base.
                let derived_declaration_flags =
                    relater::get_declaration_modifier_flags_from_symbol(self, derived);
                if base_declaration_flags.intersects(ModifierFlags::PRIVATE)
                    || derived_declaration_flags.intersects(ModifierFlags::PRIVATE)
                {
                    // either base or derived property is private - not
                    // override, skip it
                    continue;
                }
                let base_property_flags =
                    self.program_symbol(base).flags & SymbolFlags::PROPERTY_OR_ACCESSOR;
                let derived_property_flags =
                    self.program_symbol(derived).flags & SymbolFlags::PROPERTY_OR_ACCESSOR;
                if !base_property_flags.is_empty() && !derived_property_flags.is_empty() {
                    // property/accessor is overridden with property/accessor
                    let base_mapped = self.program_symbol(base).check_flags.intersects(tsc_ast::CheckFlags::MAPPED);
                    let derived_value_is_binary = self
                        .program_symbol(derived)
                        .value_declaration
                        .is_some_and(|d| self.store().node(d).kind == Kind::BinaryExpression);
                    if base_mapped
                        || derived_value_is_binary
                        || self.are_properties_abstract_or_interface(base, base_declaration_flags)
                    {
                        // when the base property is abstract or from an
                        // interface, base/derived flags don't need to match
                        continue;
                    }
                    let base_is_property = base_property_flags == SymbolFlags::PROPERTY;
                    let derived_is_property = derived_property_flags == SymbolFlags::PROPERTY;
                    let overridden_instance_property =
                        !base_is_property && derived_is_property;
                    let overridden_instance_accessor =
                        base_is_property && !derived_is_property;
                    if overridden_instance_property || overridden_instance_accessor {
                        let error_message = if overridden_instance_property {
                            &diag::X_0_IS_DEFINED_AS_AN_ACCESSOR_IN_CLASS_1_BUT_IS_OVERRIDDEN_HERE_IN_2_AS_AN_INSTANCE_PROPERTY
                        } else {
                            &diag::X_0_IS_DEFINED_AS_A_PROPERTY_IN_CLASS_1_BUT_IS_OVERRIDDEN_HERE_IN_2_AS_AN_ACCESSOR
                        };
                        let error_node = self
                            .program_symbol(derived)
                            .value_declaration
                            .unwrap_or_default();
                        let base_text = relater::symbol_to_string(self, base);
                        let base_type_text = self.type_to_string(base_type);
                        let type_text = self.type_to_string(t);
                        self.error(
                            error_node,
                            error_message,
                            &[base_text, base_type_text, type_text],
                        );
                    } else if self.emit_standard_class_fields {
                        // Go: useDefineForClassFields overwrite arm.
                        // TODO(port M6 W8): GetUseDefineForClassFields +
                        // isPropertyInitializedInConstructor (the flow walk);
                        // permissive skip.
                    }
                    // correct case
                    continue;
                } else if is_prototype_property(self, base) {
                    if is_prototype_property(self, derived)
                        || self.program_symbol(derived).flags.intersects(SymbolFlags::PROPERTY)
                    {
                        // method is overridden with method or property --
                        // correct case
                        continue;
                    } else {
                        let error_node = self
                            .program_symbol(derived)
                            .value_declaration
                            .unwrap_or_default();
                        let base_type_text = self.type_to_string(base_type);
                        let base_text = relater::symbol_to_string(self, base);
                        let type_text = self.type_to_string(t);
                        self.error(
                            error_node,
                            &diag::CLASS_0_DEFINES_INSTANCE_MEMBER_FUNCTION_1_BUT_EXTENDED_CLASS_2_DEFINES_IT_AS_INSTANCE_MEMBER_ACCESSOR,
                            &[base_type_text, base_text, type_text],
                        );
                        continue;
                    }
                } else if self.program_symbol(base).flags.intersects(SymbolFlags::ACCESSOR) {
                    let error_node = self
                        .program_symbol(derived)
                        .value_declaration
                        .unwrap_or_default();
                    let base_type_text = self.type_to_string(base_type);
                    let base_text = relater::symbol_to_string(self, base);
                    let type_text = self.type_to_string(t);
                    self.error(
                        error_node,
                        &diag::CLASS_0_DEFINES_INSTANCE_MEMBER_ACCESSOR_1_BUT_EXTENDED_CLASS_2_DEFINES_IT_AS_INSTANCE_MEMBER_FUNCTION,
                        &[base_type_text, base_text, type_text],
                    );
                    continue;
                } else {
                    let error_node = self
                        .program_symbol(derived)
                        .value_declaration
                        .unwrap_or_default();
                    let base_type_text = self.type_to_string(base_type);
                    let base_text = relater::symbol_to_string(self, base);
                    let type_text = self.type_to_string(t);
                    self.error(
                        error_node,
                        &diag::CLASS_0_DEFINES_INSTANCE_MEMBER_PROPERTY_1_BUT_EXTENDED_CLASS_2_DEFINES_IT_AS_INSTANCE_MEMBER_FUNCTION,
                        &[base_type_text, base_text, type_text],
                    );
                    continue;
                }
            }
        }
        for (error_node, member_info) in not_implemented_info {
            let is_class_expression = self.store().node(error_node).kind == Kind::ClassExpression
                || error_node == NodeId::NONE
                || self.store().node(error_node).name().is_none() && error_node != NodeId::NONE && self.store().node(error_node).kind == Kind::ClassExpression;
            match member_info.missed_properties.len() {
                1 => {
                    let missed = member_info.missed_properties[0].clone();
                    if is_class_expression {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_EXPRESSION_DOES_NOT_IMPLEMENT_INHERITED_ABSTRACT_MEMBER_0_FROM_CLASS_1,
                            &[missed, member_info.base_type_name],
                        );
                    } else {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_0_DOES_NOT_IMPLEMENT_INHERITED_ABSTRACT_MEMBER_1_FROM_CLASS_2,
                            &[member_info.type_name, missed, member_info.base_type_name],
                        );
                    }
                }
                n if n > 5 => {
                    let missed_properties =
                        u::quoted_and_comma_separated(&member_info.missed_properties[..4]);
                    let remaining = (member_info.missed_properties.len() - 4).to_string();
                    if is_class_expression {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_EXPRESSION_IS_MISSING_IMPLEMENTATIONS_FOR_THE_FOLLOWING_MEMBERS_OF_0_COLON_1_AND_2_MORE,
                            &[member_info.base_type_name, missed_properties, remaining],
                        );
                    } else {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_0_IS_MISSING_IMPLEMENTATIONS_FOR_THE_FOLLOWING_MEMBERS_OF_1_COLON_2_AND_3_MORE,
                            &[member_info.type_name, member_info.base_type_name, missed_properties, remaining],
                        );
                    }
                }
                _ => {
                    let missed_properties =
                        u::quoted_and_comma_separated(&member_info.missed_properties);
                    if is_class_expression {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_EXPRESSION_IS_MISSING_IMPLEMENTATIONS_FOR_THE_FOLLOWING_MEMBERS_OF_0_COLON_1,
                            &[member_info.base_type_name, missed_properties],
                        );
                    } else {
                        self.error(
                            error_node,
                            &diag::NON_ABSTRACT_CLASS_0_IS_MISSING_IMPLEMENTATIONS_FOR_THE_FOLLOWING_MEMBERS_OF_1_COLON_2,
                            &[member_info.type_name, member_info.base_type_name, missed_properties],
                        );
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) arePropertiesAbstractOrInterface(base, baseDeclarationFlags) bool`
    /// (checker.go:4780).
    pub(crate) fn are_properties_abstract_or_interface(
        &mut self,
        base: SymbolId,
        base_declaration_flags: ModifierFlags,
    ) -> bool {
        let declarations = self.program_symbol(base).declarations.clone();
        if self.program_symbol(base).check_flags.intersects(tsc_ast::CheckFlags::SYNTHETIC) {
            declarations.iter().any(|&d| {
                self.is_property_abstract_or_interface(d, base_declaration_flags)
            })
        } else {
            declarations.iter().all(|&d| {
                self.is_property_abstract_or_interface(d, base_declaration_flags)
            })
        }
    }

    /// Go: `func (c *Checker) isPropertyAbstractOrInterface(declaration, baseDeclarationFlags) bool`
    /// (checker.go:4785).
    pub(crate) fn is_property_abstract_or_interface(
        &self,
        declaration: NodeId,
        base_declaration_flags: ModifierFlags,
    ) -> bool {
        self.store().node(self.store().node(declaration).parent.get()).kind
            == Kind::InterfaceDeclaration
            || (base_declaration_flags.intersects(ModifierFlags::ABSTRACT)
                && (self.store().node(declaration).kind != Kind::PropertyDeclaration
                    || crate::ast_shims::initializer_of(self.store(), declaration).is_none()))
    }

    /// Go: `func (c *Checker) checkMembersForOverrideModifier(node, t, typeWithThis, staticType)`
    /// (checker.go:4792).
    pub(crate) fn check_members_for_override_modifier(
        &mut self,
        node: NodeId,
        t: TypeId,
        type_with_this: TypeId,
        static_type: TypeId,
    ) {
        let mut base_with_this: Option<TypeId> = None;
        if get_class_extends_heritage_element(self.store(), node).is_some() {
            let base_types = self.get_base_types(t);
            if !base_types.is_empty() {
                let this_type = self
                    .type_(t)
                    .as_interface_type()
                    .expect("InterfaceType data")
                    .this_type;
                base_with_this = Some(relater::get_type_with_this_argument(
                    self,
                    base_types[0],
                    this_type,
                    false,
                ));
            }
        }
        let base_static_type = self.get_base_constructor_type_of_class(t);
        for member in members_of(self.store(), node) {
            // Go: !HasAmbientModifier(member)
            let has_ambient = crate::ast_shims::has_modifier(
                self.store().node(member),
                ModifierFlags::AMBIENT,
            );
            if has_ambient {
                continue;
            }
            if self.store().node(member).kind == Kind::Constructor {
                for param in crate::ast_shims::parameters_of(self.store(), member).to_vec() {
                    if u::is_parameter_property_declaration(
                        self.store(),
                        param,
                        member,
                    ) {
                        self.check_member_for_override_modifier(
                            node,
                            static_type,
                            base_static_type,
                            base_with_this,
                            t,
                            type_with_this,
                            param,
                        );
                    }
                }
            } else {
                self.check_member_for_override_modifier(
                    node,
                    static_type,
                    base_static_type,
                    base_with_this,
                    t,
                    type_with_this,
                    member,
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkMemberForOverrideModifier(...)` (checker.go:4813).
    #[allow(clippy::too_many_arguments)] // Go: checkMemberForOverrideModifier(node, staticType, ...) — 8 params
    pub(crate) fn check_member_for_override_modifier(
        &mut self,
        node: NodeId,
        static_type: TypeId,
        base_static_type: TypeId,
        base_with_this: Option<TypeId>,
        t: TypeId,
        type_with_this: TypeId,
        member: NodeId,
    ) {
        let Some(symbol) = self.get_symbol_of_declaration(member) else {
            return;
        };
        let member_has_override = u::has_override_modifier(self.store().node(member));
        let member_has_abstract =
            crate::ast_shims::has_modifier(self.store().node(member), ModifierFlags::ABSTRACT);
        let member_is_static = has_static_modifier(self.store(), member);
        let member_is_parameter = self.store().node(member).kind == Kind::Parameter;
        self.check_member_for_override_modifier_worker(
            node,
            static_type,
            base_static_type,
            base_with_this,
            t,
            type_with_this,
            member_has_override,
            member_has_abstract,
            member_is_static,
            member_is_parameter,
            symbol,
            Some(member),
        );
    }

    /// Go: `func (c *Checker) getMemberOverrideModifierStatus(node, member, memberSymbol)`
    /// (checker.go:4828).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn get_member_override_modifier_status(
        &mut self,
        node: NodeId,
        member: NodeId,
        member_symbol: Option<SymbolId>,
    ) -> MemberOverrideStatus {
        if self.store().node(member).name().is_none() || member_symbol.is_none() {
            return MemberOverrideStatus::None;
        }
        let Some(class_symbol) = self.get_symbol_of_declaration(node) else {
            return MemberOverrideStatus::None;
        };
        let t = self.get_declared_type_of_symbol(class_symbol);
        // Go: getTypeWithThisArgument(t, nil, false).
        let type_with_this = relater::get_type_with_this_argument(self, t, None, false);
        let static_type = self.get_type_of_symbol(class_symbol);
        let mut base_with_this: Option<TypeId> = None;
        if get_class_extends_heritage_element(self.store(), node).is_some() {
            let base_types = self.get_base_types(t);
            if !base_types.is_empty() {
                let this_type = self
                    .type_(t)
                    .as_interface_type()
                    .expect("InterfaceType data")
                    .this_type;
                base_with_this = Some(relater::get_type_with_this_argument(
                    self,
                    base_types[0],
                    this_type,
                    false,
                ));
            }
        }
        let base_static_type = self.get_base_constructor_type_of_class(t);
        let member_has_override =
            crate::ast_shims::has_modifier(self.store().node(member), ModifierFlags::OVERRIDE);
        let member_has_abstract =
            crate::ast_shims::has_modifier(self.store().node(member), ModifierFlags::ABSTRACT);
        let member_is_static = has_static_modifier(self.store(), member);
        self.check_member_for_override_modifier_worker(
            node,
            static_type,
            base_static_type,
            base_with_this,
            t,
            type_with_this,
            member_has_override,
            member_has_abstract,
            member_is_static,
            false, /*memberIsParameterProperty*/
            member_symbol.expect("checked"),
            None, /*errorNode*/
        )
    }

    /// Go: `func (c *Checker) checkMemberForOverrideModifierWorker(...)` (checker.go:4858).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check_member_for_override_modifier_worker(
        &mut self,
        node: NodeId,
        static_type: TypeId,
        base_static_type: TypeId,
        base_with_this: Option<TypeId>,
        t: TypeId,
        type_with_this: TypeId,
        member_has_override_modifier: bool,
        member_has_abstract_modifier: bool,
        member_is_static: bool,
        member_is_parameter_property: bool,
        member: SymbolId,
        error_node: Option<NodeId>,
    ) -> MemberOverrideStatus {
        let is_js = tsc_ast::utilities::is_in_js_file(self.store().node(node));
        let member_name = self.program_symbol(member).name.clone();
        let value_declaration = self.program_symbol(member).value_declaration;
        if member_has_override_modifier
            && value_declaration.is_some_and(|vd| {
                is_class_element(self.store(), vd) && self.store().node(vd).name().is_some()
            })
            && value_declaration.is_some_and(|vd| {
                let name = self.store().node(vd).name().expect("checked");
                // TODO(port M6 S8): isNonBindableDynamicName — the dynamic-
                // name fact; permissive false (static names only here).
                self.store().node(name).kind == Kind::ComputedPropertyName
                    && is_non_bindable_dynamic_name_stub(self, name)
            })
        {
            if let Some(error_node) = error_node {
                let message = if is_js {
                    &diag::THIS_MEMBER_CANNOT_HAVE_A_JSDOC_COMMENT_WITH_AN_OVERRIDE_TAG_BECAUSE_ITS_NAME_IS_DYNAMIC
                } else {
                    &diag::THIS_MEMBER_CANNOT_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_ITS_NAME_IS_DYNAMIC
                };
                self.error(error_node, message, &[]);
            }
            return MemberOverrideStatus::HasInvalidOverride;
        }

        if let Some(base_with_this) = base_with_this
            && (member_has_override_modifier
                || self.compiler_options.no_implicit_override == Tristate::TSTrue)
        {
            let this_type = if member_is_static { static_type } else { type_with_this };
            let base_type = if member_is_static {
                base_static_type
            } else {
                base_with_this
            };
            let prop = self.get_property_of_type(this_type, &member_name);
            let base_prop = self.get_property_of_type(base_type, &member_name);

            if prop.is_some() && base_prop.is_none() && member_has_override_modifier {
                if let Some(error_node) = error_node {
                    // TODO(port M6 S8): getSuggestedSymbolForNonexistentClass-
                    // Member — the spelling suggestion; permissive None.
                    let base_text = self.type_to_string(base_with_this);
                    let message = if is_js {
                        &diag::THIS_MEMBER_CANNOT_HAVE_A_JSDOC_COMMENT_WITH_AN_OVERRIDE_TAG_BECAUSE_IT_IS_NOT_DECLARED_IN_THE_BASE_CLASS_0
                    } else {
                        &diag::THIS_MEMBER_CANNOT_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_IT_IS_NOT_DECLARED_IN_THE_BASE_CLASS_0
                    };
                    self.error(error_node, message, &[base_text]);
                }
                return MemberOverrideStatus::HasInvalidOverride;
            }

            if let Some(base_prop) = base_prop
                && prop.is_some()
                && !self.program_symbol(base_prop).declarations.is_empty()
                && self.compiler_options.no_implicit_override == Tristate::TSTrue
                && !self.store().node(node).flags.intersects(NF::AMBIENT)
            {
                let base_has_abstract = self
                    .program_symbol(base_prop)
                    .declarations
                    .iter()
                    .any(|&d| {
                        crate::ast_shims::has_modifier(self.store().node(d), ModifierFlags::ABSTRACT)
                    });
                if member_has_override_modifier {
                    return MemberOverrideStatus::None;
                }
                if !base_has_abstract {
                    if let Some(error_node) = error_node {
                        let base_text = self.type_to_string(base_with_this);
                        let message = if member_is_parameter_property {
                            if is_js {
                                &diag::THIS_PARAMETER_PROPERTY_MUST_HAVE_A_JSDOC_COMMENT_WITH_AN_OVERRIDE_TAG_BECAUSE_IT_OVERRIDES_A_MEMBER_IN_THE_BASE_CLASS_0
                            } else {
                                &diag::THIS_PARAMETER_PROPERTY_MUST_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_IT_OVERRIDES_A_MEMBER_IN_BASE_CLASS_0
                            }
                        } else if is_js {
                            &diag::THIS_MEMBER_MUST_HAVE_A_JSDOC_COMMENT_WITH_AN_OVERRIDE_TAG_BECAUSE_IT_OVERRIDES_A_MEMBER_IN_THE_BASE_CLASS_0
                        } else {
                            &diag::THIS_MEMBER_MUST_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_IT_OVERRIDES_A_MEMBER_IN_THE_BASE_CLASS_0
                        };
                        self.error(error_node, message, &[base_text]);
                    }
                    return MemberOverrideStatus::NeedsOverride;
                }
                if member_has_abstract_modifier {
                    if let Some(error_node) = error_node {
                        let base_text = self.type_to_string(base_with_this);
                        self.error(
                            error_node,
                            &diag::THIS_MEMBER_MUST_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_IT_OVERRIDES_AN_ABSTRACT_METHOD_THAT_IS_DECLARED_IN_THE_BASE_CLASS_0,
                            &[base_text],
                        );
                    }
                    return MemberOverrideStatus::NeedsOverride;
                }
            }
        } else if member_has_override_modifier {
            if let Some(error_node) = error_node {
                let t_text = self.type_to_string(t);
                let message = if is_js {
                    &diag::THIS_MEMBER_CANNOT_HAVE_A_JSDOC_COMMENT_WITH_AN_OVERRIDE_TAG_BECAUSE_ITS_CONTAINING_CLASS_0_DOES_NOT_EXTEND_ANOTHER_CLASS
                } else {
                    &diag::THIS_MEMBER_CANNOT_HAVE_AN_OVERRIDE_MODIFIER_BECAUSE_ITS_CONTAINING_CLASS_0_DOES_NOT_EXTEND_ANOTHER_CLASS
                };
                self.error(error_node, message, &[t_text]);
            }
            return MemberOverrideStatus::HasInvalidOverride;
        }

        MemberOverrideStatus::None
    }

    /// Go: `func (c *Checker) checkIndexConstraints(t, symbol, isStaticIndex)`
    /// (checker.go:4896).
    pub(crate) fn check_index_constraints(&mut self, t: TypeId, symbol: SymbolId, is_static_index: bool) {
        let index_infos = relater::get_index_infos_of_type(self, t);
        if index_infos.is_empty() {
            return;
        }
        // Go: for _, prop := range c.getPropertiesOfObjectType(t).
        // REAL (M6 S10, checker.go:4896): getPropertiesOfObjectType + the
        // literal extraction (getLiteralTypeFromProperty with
        // includeNonPublic=true) + getNonMissingTypeOfSymbol.
        let properties = self.get_properties_of_object_type_impl(t);
        for prop in properties {
            let is_prototype = is_static_index
                && self.program_symbol(prop).flags.intersects(SymbolFlags::PROTOTYPE);
            if !is_prototype {
                let prop_name_type = self
                    .get_literal_type_from_property_impl(
                        prop,
                        TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                        true, /*includeNonPublic*/
                    )
                    .unwrap_or_else(|| self.never_type());
                let prop_type = self.get_non_missing_type_of_symbol(prop);
                self.check_index_constraint_for_property(t, prop, prop_name_type, prop_type);
            }
        }
        let type_declaration = self.program_symbol(symbol).value_declaration;
        if type_declaration.is_some_and(|d| is_class_like(self.store(), d)) {
            let members = members_of(self.store(), type_declaration.expect("checked"));
            for member in members {
                // Only process instance properties against instance index
                // signatures and static properties against static index
                // signatures
                if has_static_modifier(self.store(), member) == is_static_index
                    && !self.has_bindable_name(member)
                {
                    // REAL (M6 S10, checker.go:4910): the computed-name member
                    // arm — checkIndexConstraintForProperty with the name
                    // expression's type.
                    if let Some(symbol) = crate::typeofsymbol::get_symbol_of_declaration(self, member) {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), member);
                        let expression = name.filter(|&n| self.store().node(n).kind == Kind::ComputedPropertyName)
                            .map(|n| self.store().node(n).as_computed_property_name().expect("ComputedPropertyName data").expression);
                        if let Some(expression) = expression {
                            let prop_name_type = self.get_type_of_expression(expression);
                            let prop_type = self.get_non_missing_type_of_symbol(symbol);
                            self.check_index_constraint_for_property(t, symbol, prop_name_type, prop_type);
                        }
                    }
                }
            }
        }
        if index_infos.len() > 1 {
            for info in index_infos {
                self.check_index_constraint_for_index_signature(t, info);
            }
        }
    }

    /// Go: `func (c *Checker) checkIndexConstraintForProperty(...)` (checker.go:4919).
    pub(crate) fn check_index_constraint_for_property(
        &mut self,
        t: TypeId,
        prop: SymbolId,
        prop_name_type: TypeId,
        prop_type: TypeId,
    ) {
        let declaration = self.program_symbol(prop).value_declaration;
        if let Some(declaration) = declaration {
            let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), declaration);
            if name.is_some_and(|n| self.store().node(n).kind == Kind::PrivateIdentifier) {
                return;
            }
        }
        let index_infos = self.get_applicable_index_infos(t, prop_name_type);
        if index_infos.is_empty() {
            return;
        }
        let prop_name = self.program_symbol(prop).name.clone();
        // REAL (M6 S10, checker.go:4944): propDeclaration — a binary-expression
        // value declaration or a computed property name.
        let prop_declaration: Option<NodeId> = if declaration
            .is_some_and(|d| self.store().node(d).kind == Kind::BinaryExpression)
        {
            declaration
        } else {
            let name = declaration
                .and_then(|d| tsc_binder::ast_shims::get_name_of_declaration(self.store(), d));
            name.filter(|&n| self.store().node(n).kind == Kind::ComputedPropertyName)
                .map(|n| self.store().node(n).parent.get())
        };
        let type_symbol = self.type_(t).symbol;
        let local_prop_declaration: Option<NodeId> = if self.get_parent_of_symbol(prop) == type_symbol {
            declaration
        } else {
            None
        };
        let interface_declaration: Option<NodeId> = if self.type_(t).object_flags.intersects(ObjectFlags::INTERFACE) {
            type_symbol.and_then(|symbol| {
                self.program_symbol(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| self.store().node(d).kind == Kind::InterfaceDeclaration)
            })
        } else {
            None
        };
        for info in index_infos {
            // REAL (M6 S10, checker.go:4951): the (a)/(b)/(c) errorNode
            // selection — the local property declaration, the local index
            // declaration, or the interface declaration when no base
            // interface contains both the property and the index signature.
            let mut error_node = local_prop_declaration.or(get_local_index_declaration(self, t, info));
            if error_node.is_none()
                && let Some(interface_declaration) = interface_declaration
                && let Some(type_symbol) = type_symbol
            {
                let info_key_type = self.index_info_key_type(info);
                let base_types = self.get_base_types(t);
                let base_has_both = base_types.iter().any(|&base| {
                    self.get_property_of_object_type_impl(base, &prop_name).is_some()
                        && self.get_index_type_of_type_impl(base, info_key_type).is_some()
                });
                let _ = type_symbol;
                if !base_has_both {
                    error_node = Some(interface_declaration);
                }
            }
            if let Some(error_node) = error_node {
                let info_key_type = self.index_info_key_type(info);
                let info_value_type = self.index_info_value_type(info);
                if !self.check_type_assignable_to(prop_type, info_value_type, None, None) {
                    let prop_text = relater::symbol_to_string(self, prop);
                    let prop_type_text = self.type_to_string(prop_type);
                    let key_text = self.type_to_string(info_key_type);
                    let value_text = self.type_to_string(info_value_type);
                    let mut d = u::new_diagnostic_for_node(
                        self.store(),
                        error_node,
                        &diag::PROPERTY_0_OF_TYPE_1_IS_NOT_ASSIGNABLE_TO_2_INDEX_TYPE_3,
                        vec![prop_text.clone(), prop_type_text, key_text, value_text],
                    );
                    // Go (checker.go:4961): the related info when the property
                    // is declared elsewhere than the error node.
                    if let Some(prop_declaration) = prop_declaration
                        && prop_declaration != error_node
                    {
                        d.add_related_info(u::new_diagnostic_for_node(
                            self.store(),
                            prop_declaration,
                            &diag::X_0_IS_DECLARED_HERE,
                            vec![prop_text],
                        ));
                    }
                    self.add_diagnostic(d);
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkIndexConstraintForIndexSignature(t, checkInfo)`
    /// (checker.go:4967).
    pub(crate) fn check_index_constraint_for_index_signature(
        &mut self,
        t: TypeId,
        check_info: crate::types::IndexInfoId,
    ) {
        let check_key_type = self.index_info_key_type(check_info);
        let index_infos = self.get_applicable_index_infos(t, check_key_type);
        if index_infos.is_empty() {
            return;
        }
        for info in index_infos {
            if info == check_info {
                continue;
            }
            // REAL (M6 S10, checker.go:4986): the (a)/(b) errorNode selection —
            // the local index declarations for both signatures.
            let error_node = get_local_index_declaration(self, t, info)
                .or(get_local_index_declaration(self, t, check_info));
            if let Some(error_node) = error_node {
                let check_value_type = self.index_info_value_type(check_info);
                let check_key_text = self.type_to_string(check_key_type);
                let check_value_text = self.type_to_string(check_value_type);
                let info_key = self.index_info_key_type(info);
                let info_value = self.index_info_value_type(info);
                let info_key_text = self.type_to_string(info_key);
                let info_value_text = self.type_to_string(info_value);
                if !self.check_type_assignable_to(check_value_type, info_value, None, None) {
                    self.error(
                        error_node,
                        &diag::X_0_INDEX_TYPE_1_IS_NOT_ASSIGNABLE_TO_2_INDEX_TYPE_3,
                        &[check_key_text, check_value_text, info_key_text, info_value_text],
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) getApplicableIndexInfos(t, keyType) []*IndexInfo`
    /// (checker.go:32635) — REAL (M6 S10): re-points to members.rs's filter
    /// form.
    fn get_applicable_index_infos(
        &mut self,
        t: TypeId,
        key_type: TypeId,
    ) -> Vec<crate::types::IndexInfoId> {
        self.get_applicable_index_infos_impl(t, key_type)
    }



    /// Go: `func (c *Checker) checkClassOrInterfaceForDuplicateIndexSignatures(node)`
    /// (checker.go:5020).
    pub(crate) fn check_class_or_interface_for_duplicate_index_signatures(&mut self, node: NodeId) {
        // Only check the type once
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        if !self.declared_type_links.get(symbol).index_signatures_checked {
            self.declared_type_links.get(symbol).index_signatures_checked = true;
            self.check_type_for_duplicate_index_signatures(node);
        }
    }

    /// Go: `func (c *Checker) checkTypeForDuplicateIndexSignatures(node)`
    /// (checker.go:5028).
    pub(crate) fn check_type_for_duplicate_index_signatures(&mut self, node: NodeId) {
        // TypeScript 1.0 spec (April 2014): 3.7.4 / 8.5 — at most one
        // string and one numeric index signature.
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        let Some(index_symbol) =
            u::get_index_symbol_from_symbol_table(&self.program_symbol(symbol).members)
        else {
            return;
        };
        let declarations = self.program_symbol(index_symbol).declarations.clone();
        if declarations.len() <= 1 {
            return;
        }
        // Go: map[*Type][]*ast.Node keyed by the distributed parameter type.
        let mut index_signature_map: Vec<(TypeId, Vec<NodeId>)> = Vec::new();
        for declaration in declarations {
            if self.store().node(declaration).kind == Kind::IndexSignature {
                let parameters = crate::ast_shims::parameters_of(self.store(), declaration).to_vec();
                if parameters.len() == 1 {
                    let param_type = signature_type_node(self.store(), parameters[0]);
                    if let Some(param_type) = param_type {
                        let t = self.get_type_from_type_node(param_type);
                        // Go: .Distributed() — union constituents, else the
                        // single type.
                        let distributed = if self.type_(t).flags.intersects(TypeFlags::UNION) {
                            self.type_(t)
                                .as_union_type()
                                .expect("UnionType data")
                                .base
                                .types
                                .clone()
                        } else {
                            vec![t]
                        };
                        for dt in distributed {
                            match index_signature_map
                                .iter_mut()
                                .find(|(k, _)| *k == dt)
                            {
                                Some((_, list)) => list.push(declaration),
                                None => index_signature_map.push((dt, vec![declaration])),
                            }
                        }
                    }
                }
            }
            // Do nothing for late-bound index signatures: allow these to
            // duplicate one another and explicit indexes
        }
        for (t, declarations) in index_signature_map {
            if declarations.len() > 1 {
                let t_text = self.type_to_string(t);
                for declaration in declarations {
                    self.error(
                        declaration,
                        &diag::DUPLICATE_INDEX_SIGNATURE_FOR_TYPE_0,
                        std::slice::from_ref(&t_text),
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkPropertyInitialization(node)` (checker.go:5051).
    pub(crate) fn check_property_initialization(&mut self, node: NodeId) {
        if !self.strict_null_checks
            || !self.strict_property_initialization
            || self.store().node(node).flags.intersects(NF::AMBIENT)
        {
            return;
        }
        let constructor = tsc_binder::ast_shims::find_constructor_declaration(self.store(), node);
        for member in members_of(self.store(), node) {
            let has_ambient = crate::ast_shims::has_modifier(
                self.store().node(member),
                ModifierFlags::AMBIENT,
            );
            if has_ambient {
                continue;
            }
            if !has_static_modifier(self.store(), member) && self.is_property_without_initializer(member)
            {
                let Some(prop_name) = self.store().node(member).name() else {
                    continue;
                };
                let prop_name_kind = self.store().node(prop_name).kind;
                if prop_name_kind == Kind::Identifier
                    || prop_name_kind == Kind::PrivateIdentifier
                    || prop_name_kind == Kind::ComputedPropertyName
                {
                    let Some(symbol) = self.get_symbol_of_declaration(member) else {
                        continue;
                    };
                    let t = self.get_type_of_symbol(symbol);
                    // TODO(port M6 W3): containsUndefinedType — the flow-
                    // based definite-assignment analysis; permissive false
                    // (no undefined constituents on the direct type).
                    if !self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                        let initialized = constructor.is_some_and(|ctor| {
                            // TODO(port M6 W3): isPropertyInitializedIn-
                            // Constructor — the flow walk; permissive true
                            // (the getFlowTypeOfReferenceEx seam is W3's).
                            let _ = ctor;
                            true
                        });
                        if !initialized {
                            let name_text = tsc_scanner::utilities::declaration_name_to_string(
                                self.store(),
                                Some(prop_name),
                            );
                            self.error(
                                prop_name,
                                &diag::PROPERTY_0_HAS_NO_INITIALIZER_AND_IS_NOT_DEFINITELY_ASSIGNED_IN_THE_CONSTRUCTOR,
                                &[name_text],
                            );
                        }
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) isPropertyWithoutInitializer(node) bool` (checker.go:5081).
    pub(crate) fn is_property_without_initializer(&self, node: NodeId) -> bool {
        self.store().node(node).kind == Kind::PropertyDeclaration
            && !crate::ast_shims::has_modifier(self.store().node(node), ModifierFlags::ABSTRACT)
            // TODO(port M6 W3): isExclamationToken(node.PostfixToken()) —
            // the definite-assignment `!`; permissive false.
            && crate::ast_shims::initializer_of(self.store(), node).is_none()
    }

    /// Go: `func (c *Checker) checkInterfaceDeclaration(node)` (checker.go:5114).
    pub(crate) fn check_interface_declaration(&mut self, node: NodeId) {
        if !self.check_grammar_modifiers(node) {
            self.check_grammar_interface_declaration(node);
        }
        if !container_allows_block_scoped_variable(
            self.store(),
            self.store().node(node).parent.get(),
        ) {
            self.grammar_error_on_node(
                node,
                &diag::X_0_DECLARATIONS_CAN_ONLY_BE_DECLARED_INSIDE_A_BLOCK,
                &["interface".to_string()],
            );
        }
        let type_parameters = type_parameters_of(self.store(), node);
        self.check_type_parameters(&type_parameters);
        let name = self.store().node(node).name();
        if let Some(name) = name {
            self.check_type_name_is_reserved(name, &diag::INTERFACE_NAME_CANNOT_BE_0);
        }
        self.check_exports_on_merged_declarations(node);
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        self.check_type_parameter_lists_identical(symbol);
        // Check once per checker, but report on the first interface
        // declaration, independently of which declaration is checked first.
        if !self.declared_type_links.get(symbol).interface_checked {
            self.declared_type_links.get(symbol).interface_checked = true;
            let first_interface_declaration =
                get_declaration_of_kind(self.store(), self.program_symbol(symbol), Kind::InterfaceDeclaration);
            let t = self.get_declared_type_of_symbol(symbol);
            // Go: getTypeWithThisArgument(t, nil, false).
            let type_with_this = relater::get_type_with_this_argument(self, t, None, false);
            // run subsequent checks only if first set succeeded
            let first_name = first_interface_declaration
                .and_then(|d| self.store().node(d).name());
            if self.check_inherited_properties_are_identical(t, first_name) {
                let base_types = self.get_base_types(t);
                for base_type in base_types {
                    let this_type = self
                        .type_(t)
                        .as_interface_type()
                        .expect("InterfaceType data")
                        .this_type;
                    let base_with_this = relater::get_type_with_this_argument(
                        self,
                        base_type,
                        this_type,
                        false,
                    );
                    self.check_type_assignable_to(
                        type_with_this,
                        base_with_this,
                        first_name,
                        Some(&diag::INTERFACE_0_INCORRECTLY_EXTENDS_INTERFACE_1),
                    );
                }
                self.check_index_constraints(t, symbol, false /*isStaticIndex*/);
            }
        }
        self.check_object_type_for_duplicate_declarations(node, false /*checkPrivateNames*/);
        for heritage_element in get_extends_heritage_clause_elements(self.store(), node) {
            if self.store().node(heritage_element).kind == Kind::ExpressionWithTypeArguments {
                let expr = self
                    .store()
                    .node(heritage_element)
                    .as_expression_with_type_arguments()
                    .expect("ExpressionWithTypeArguments data")
                    .expression;
                // Go: !IsEntityNameExpression(expr) || IsOptionalChain(expr)
                let bad = !u::is_entity_name_expression(self.store(), expr)
                    || tsc_binder::ast_shims::is_optional_chain_root(self.store(), expr);
                if bad {
                    self.error(
                        expr,
                        &diag::AN_INTERFACE_CAN_ONLY_EXTEND_AN_IDENTIFIER_SLASHQUALIFIED_NAME_WITH_OPTIONAL_TYPE_ARGUMENTS,
                        &[],
                    );
                }
            }
            self.check_type_reference_node(heritage_element);
        }
        let members = members_of(self.store(), node);
        self.check_source_elements(&members);
        self.check_class_or_interface_for_duplicate_index_signatures(node);
        self.register_for_unused_identifiers_check(node);
    }

    /// Go: `func (c *Checker) checkInheritedPropertiesAreIdentical(t, typeNode) bool`
    /// (checker.go:5150).
    pub(crate) fn check_inherited_properties_are_identical(
        &mut self,
        t: TypeId,
        type_node: Option<NodeId>,
    ) -> bool {
        let base_types = self.get_base_types(t);
        if base_types.len() < 2 {
            return true;
        }
        // Go: c.resolveDeclaredMembers(t).declaredMembers — the declared
        // member row; the properties walk is the permissive stand-in until
        // S10's resolveDeclaredMembers lands.
        let mut seen: Vec<(String, SymbolId, TypeId)> = Vec::new();
        let own_properties = relater::get_properties_of_type(self, t);
        for prop in own_properties {
            let name = self.program_symbol(prop).name.clone();
            if is_named_member(&name) {
                seen.push((name, prop, t));
            }
        }
        let mut identical = true;
        let this_type = self
            .type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .this_type;
        for base in base_types {
            let base_with_this = relater::get_type_with_this_argument(self, base, this_type, false);
            let properties = relater::get_properties_of_type(self, base_with_this);
            for prop in properties {
                let prop_name = self.program_symbol(prop).name.clone();
                match seen.iter().position(|(n, _, _)| *n == prop_name) {
                    None => {
                        seen.push((prop_name, prop, base));
                    }
                    Some(idx) => {
                        let (existing_name, existing_prop, existing_type) = seen[idx].clone();
                        let is_inherited_property = existing_type != t;
                        if is_inherited_property
                            && !self.is_property_identical_to(existing_prop, prop)
                        {
                            identical = false;
                            let type_name_1 = self.type_to_string(existing_type);
                            let type_name_2 = self.type_to_string(base);
                            let prop_text = relater::symbol_to_string(self, prop);
                            let t_text = self.type_to_string(t);
                            let error_info = u::new_diagnostic_for_node(
                                self.store(),
                                type_node.unwrap_or_default(),
                                &diag::NAMED_PROPERTY_0_OF_TYPES_1_AND_2_ARE_NOT_IDENTICAL,
                                vec![prop_text, type_name_1.clone(), type_name_2.clone()],
                            );
                            let mut chained = error_info;
                            chained.add_related_info(u::new_diagnostic_for_node(
                                self.store(),
                                type_node.unwrap_or_default(),
                                &diag::INTERFACE_0_CANNOT_SIMULTANEOUSLY_EXTEND_TYPES_1_AND_2,
                                vec![t_text, type_name_1, type_name_2],
                            ));
                            self.add_diagnostic(chained);
                            let _ = existing_name;
                        }
                    }
                }
            }
        }
        identical
    }

    /// Go: `func (c *Checker) isPropertyIdenticalTo(sourceProp, targetProp) bool`
    /// (checker.go:5185).
    pub(crate) fn is_property_identical_to(
        &mut self,
        source_prop: SymbolId,
        target_prop: SymbolId,
    ) -> bool {
        relater::compare_properties(
            self,
            source_prop,
            target_prop,
            crate::relater::TypeComparerKind::Identical,
            None,
        ) != Ternary::False
    }

    /// Go: `func (c *Checker) checkEnumDeclaration(node)` (checker.go:5188).
    pub(crate) fn check_enum_declaration(&mut self, node: NodeId) {
        self.check_grammar_modifiers(node);
        self.check_collisions_for_declaration_name_stub(node, self.store().node(node).name());
        self.check_exports_on_merged_declarations(node);
        let members = members_of(self.store(), node);
        self.check_source_elements(&members);

        if self.should_check_erasable_syntax(node)
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
        {
            self.error(
                node,
                &diag::THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
                &[],
            );
        }

        self.compute_enum_member_values(node);
        // Spec 2014 - Section 9.3: (see Go's comment block). Only perform
        // this check once per symbol.
        let Some(enum_symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        if !self.declared_type_links.get(enum_symbol).enum_checked {
            self.declared_type_links.get(enum_symbol).enum_checked = true;
            let declarations = self.program_symbol(enum_symbol).declarations.clone();
            if declarations.len() > 1 {
                let first_enum_declaration = get_declaration_of_kind(
                    self.store(),
                    self.program_symbol(enum_symbol),
                    Kind::EnumDeclaration,
                );
                let enum_is_const = first_enum_declaration
                    .is_some_and(|d| tsc_binder::ast_shims::is_enum_const(self.store(), d));
                // check that const is placed\omitted on all enum declarations
                for decl in declarations {
                    if self.store().node(decl).kind == Kind::EnumDeclaration
                        && tsc_binder::ast_shims::is_enum_const(self.store(), decl) != enum_is_const
                    {
                        let name = tsc_binder::ast_shims::get_name_of_declaration(self.store(), decl);
                        if let Some(name) = name {
                            self.error(
                                name,
                                &diag::ENUM_DECLARATIONS_MUST_ALL_BE_CONST_OR_NON_CONST,
                                &[],
                            );
                        }
                    }
                }
            }
            let mut seen_enum_missing_initial_initializer = false;
            for declaration in self.program_symbol(enum_symbol).declarations.clone() {
                // return true if we hit a violation of the rule, false
                // otherwise
                if self.store().node(declaration).kind != Kind::EnumDeclaration {
                    continue;
                }
                let members = members_of(self.store(), declaration);
                if members.is_empty() {
                    continue;
                }
                let first_enum_member = members[0];
                if crate::ast_shims::initializer_of(self.store(), first_enum_member).is_none() {
                    if seen_enum_missing_initial_initializer {
                        let name = self.store().node(first_enum_member).name();
                        if let Some(name) = name {
                            self.error(
                                name,
                                &diag::IN_AN_ENUM_WITH_MULTIPLE_DECLARATIONS_ONLY_ONE_DECLARATION_CAN_OMIT_AN_INITIALIZER_FOR_ITS_FIRST_ENUM_ELEMENT,
                                &[],
                            );
                        }
                    } else {
                        seen_enum_missing_initial_initializer = true;
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkEnumMember(node)` (checker.go:5237).
    pub(crate) fn check_enum_member(&mut self, node: NodeId) {
        let name = self.store().node(node).name();
        if name.is_some_and(|n| self.store().node(n).kind == Kind::PrivateIdentifier) {
            self.error(
                node,
                &diag::AN_ENUM_MEMBER_CANNOT_BE_NAMED_WITH_A_PRIVATE_IDENTIFIER,
                &[],
            );
        }
        if name.is_some_and(|n| self.store().node(n).kind == Kind::ComputedPropertyName) {
            let expr = self
                .store()
                .node(name.expect("checked"))
                .as_computed_property_name()
                .expect("ComputedPropertyName data")
                .expression;
            let _ = self.check_expression(expr);
        }
        if let Some(initializer) = crate::ast_shims::initializer_of(self.store(), node) {
            let _ = self.check_expression(initializer);
        }
    }

    /// Go: `func (c *Checker) checkModuleDeclaration(node)` (checker.go:5250).
    pub(crate) fn check_module_declaration(&mut self, node: NodeId) {
        let d = self
            .store()
            .node(node)
            .as_module_declaration()
            .expect("ModuleDeclaration data")
            .clone();
        if let Some(body) = d.body {
            self.check_source_element(Some(body));
            if !tsc_binder::ast_shims::is_global_scope_augmentation(self.store().node(node)) {
                self.register_for_unused_identifiers_check(node);
            }
        }
        let is_global_augmentation =
            tsc_binder::ast_shims::is_global_scope_augmentation(self.store().node(node));
        let in_ambient_context = self.store().node(node).flags.intersects(NF::AMBIENT);
        if is_global_augmentation
            && !in_ambient_context
            && d.name != NodeId::NONE
        {
            self.error(
                d.name,
                &diag::AUGMENTATIONS_FOR_THE_GLOBAL_SCOPE_SHOULD_HAVE_DECLARE_MODIFIER_UNLESS_THEY_APPEAR_IN_ALREADY_AMBIENT_CONTEXT,
                &[],
            );
        }
        if let Some(attributes) = d.attributes {
            self.check_import_attributes_type(attributes);
        }
        let is_ambient_external_module = tsc_binder::ast_shims::is_ambient_module(self.store(), node);
        let context_error_message = if is_ambient_external_module {
            &diag::AN_AMBIENT_MODULE_DECLARATION_IS_ONLY_ALLOWED_AT_THE_TOP_LEVEL_IN_A_FILE
        } else {
            &diag::A_NAMESPACE_DECLARATION_IS_ONLY_ALLOWED_AT_THE_TOP_LEVEL_OF_A_NAMESPACE_OR_MODULE
        };
        if self.check_grammar_module_element_context(node, context_error_message) {
            // If we hit a module declaration in an illegal context, just
            // bail out to avoid cascading errors.
            return;
        }
        if !self.check_grammar_modifiers(node) {
            let name_kind = if d.name != NodeId::NONE {
                self.store().node(d.name).kind
            } else {
                Kind::MissingDeclaration
            };
            if !in_ambient_context && name_kind == Kind::StringLiteral {
                self.grammar_error_on_node(
                    d.name,
                    &diag::ONLY_AMBIENT_MODULES_CAN_USE_QUOTED_NAMES,
                    &[],
                );
            }
        }
        if d.name != NodeId::NONE && is_identifier(self.store().node(d.name)) {
            self.check_collisions_for_declaration_name_stub(node, Some(d.name));
            if d.keyword == Kind::ModuleKeyword {
                self.error(
                    d.name,
                    &diag::A_NAMESPACE_DECLARATION_SHOULD_NOT_BE_DECLARED_USING_THE_MODULE_KEYWORD_PLEASE_USE_THE_NAMESPACE_KEYWORD_INSTEAD,
                    &[],
                );
            }
        }
        self.check_exports_on_merged_declarations(node);
        let Some(symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        // The following checks only apply on a non-ambient instantiated
        // module declaration.
        let should_preserve_const_enums = self.compiler_options.should_preserve_const_enums();
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::VALUE_MODULE)
            && !in_ambient_context
            && is_instantiated_module(self.store(), node, should_preserve_const_enums)
        {
            if self.should_check_erasable_syntax(node) {
                self.error(
                    node,
                    &diag::THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
                    &[],
                );
            }
            // TODO(port M6 W8): GetIsolatedModules + the emit-module-format
            // seam (ExternalModuleIndicator); permissive (the gate needs
            // the program's module-format-of-file resolution).
            if self.program_symbol(symbol).declarations.len() > 1 {
                let first_non_ambient_class_or_func =
                    get_first_non_ambient_class_or_function_declaration(self, symbol);
                if let Some(first) = first_non_ambient_class_or_func {
                    let node_file = self.get_source_file_node_of(node);
                    let first_file = self.get_source_file_node_of(first);
                    if node_file != first_file {
                        self.error(
                            d.name,
                            &diag::A_NAMESPACE_DECLARATION_CANNOT_BE_IN_A_DIFFERENT_FILE_FROM_A_CLASS_OR_FUNCTION_WITH_WHICH_IT_IS_MERGED,
                            &[],
                        );
                    } else if self.store().node(node).pos() < self.store().node(first).pos() {
                        self.error(
                            d.name,
                            &diag::A_NAMESPACE_DECLARATION_CANNOT_BE_LOCATED_PRIOR_TO_A_CLASS_OR_FUNCTION_WITH_WHICH_IT_IS_MERGED,
                            &[],
                        );
                    }
                }
            }
        }
        if is_ambient_external_module {
            // Go: IsExternalModuleAugmentation / IsGlobalSourceFile(parent)
            // — the augmentation placement facts.
            let is_external_augmentation = u::is_external_module_augmentation(self.store(), node);
            if is_external_augmentation {
                if let Some(attributes) = d.attributes {
                    self.error(
                        attributes,
                        &diag::IMPORT_ATTRIBUTES_ARE_NOT_ALLOWED_ON_A_MODULE_AUGMENTATION,
                        &[],
                    );
                }
                // body of the augmentation should be checked for consistency
                // only if augmentation was applied to its target.
                let check_body = is_global_augmentation
                    || self
                        .program_symbol(symbol)
                        .flags
                        .intersects(tsc_ast::SymbolFlags::TRANSIENT);
                if check_body {
                    if let Some(body) = d.body {
                        if self.store().node(body).kind == Kind::ModuleBlock {
                            let statements = block_statements(self.store(), body);
                            for statement in statements {
                                self.check_module_augmentation_element(statement);
                            }
                        }
                    }
                }
            } else {
                let parent = self.store().node(node).parent.get();
                if u::is_global_source_file(self.store(), parent) {
                    if is_global_augmentation {
                        self.error(
                            d.name,
                            &diag::AUGMENTATIONS_FOR_THE_GLOBAL_SCOPE_CAN_ONLY_BE_DIRECTLY_NESTED_IN_EXTERNAL_MODULES_OR_AMBIENT_MODULE_DECLARATIONS,
                            &[],
                        );
                    }
                    // Go: tspath.IsExternalModuleNameRelative(name) —
                    // TODO(port M6 W8): the tspath seam; permissive skip.
                } else if is_global_augmentation {
                    self.error(
                        d.name,
                        &diag::AUGMENTATIONS_FOR_THE_GLOBAL_SCOPE_CAN_ONLY_BE_DIRECTLY_NESTED_IN_EXTERNAL_MODULES_OR_AMBIENT_MODULE_DECLARATIONS,
                        &[],
                    );
                } else {
                    // Node is not an augmentation and is not located on the
                    // script level — ambient module nested in other module
                    // or namespace, which is prohibited.
                    self.error(
                        d.name,
                        &diag::AMBIENT_MODULES_CANNOT_BE_NESTED_IN_OTHER_MODULES_OR_NAMESPACES,
                        &[],
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkImportAttributesType(attributes)` (checker.go:5370).
    pub(crate) fn check_import_attributes_type(&mut self, attributes: NodeId) {
        // TODO(port M6 W3): checkGrammarImportAttributesType; permissive.
        self.check_source_element(Some(attributes));
        // TODO(port M6 S5): getGlobalImportAttributesTypeChecked — the
        // global ImportAttributes type seam; permissive emptyObjectType
        // answer (the assignability gate rests).
        let _ = attributes;
    }

    /// Go: `func (c *Checker) getTypeOfModuleDeclarationImportAttributes(attributes)`
    /// (checker.go:5380).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn get_type_of_module_declaration_import_attributes(
        &mut self,
        attributes: Option<NodeId>,
    ) -> TypeId {
        match attributes {
            None => self.empty_object_type(),
            Some(attributes) => self.get_type_from_type_node(attributes),
        }
    }

    /// Go: `func (c *Checker) getTypeOfModuleImportAttributes(symbol)` (checker.go:5390).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn get_type_of_module_import_attributes(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.module_import_attributes_types.get(&symbol) {
            return *t;
        }
        let result = {
            let module_decl = self
                .program_symbol(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_module_with_string_literal_name(self.store(), d));
            match module_decl {
                None => self.empty_object_type(),
                Some(module_decl) => {
                    let attributes = self
                        .store()
                        .node(module_decl)
                        .as_module_declaration()
                        .expect("ModuleDeclaration data")
                        .attributes;
                    self.get_type_of_module_declaration_import_attributes(attributes)
                }
            }
        };
        self.module_import_attributes_types.insert(symbol, result);
        result
    }

    /// Go: `func (c *Checker) getIsolatedModulesLikeFlagName() string` (checker.go:5407).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn get_isolated_modules_like_flag_name(&self) -> String {
        if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue {
            "verbatimModuleSyntax".to_string()
        } else {
            "isolatedModules".to_string()
        }
    }

    /// Go: `func (c *Checker) checkModuleAugmentationElement(node)` (checker.go:5394).
    pub(crate) fn check_module_augmentation_element(&mut self, node: NodeId) {
        match self.store().node(node).kind {
            Kind::VariableStatement => {
                // error each individual name in variable statement instead
                // of marking the entire variable statement
                let declaration_list = self
                    .store()
                    .node(node)
                    .as_variable_statement()
                    .expect("VariableStatement data")
                    .declaration_list;
                let declarations = self
                    .store()
                    .node(declaration_list)
                    .as_variable_declaration_list()
                    .expect("VariableDeclarationList data")
                    .declarations
                    .as_ref()
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default();
                for decl in declarations {
                    self.check_module_augmentation_element(decl);
                }
            }
            Kind::ExportAssignment | Kind::ExportDeclaration => {
                self.grammar_error_on_first_token(
                    node,
                    &diag::EXPORTS_AND_EXPORT_ASSIGNMENTS_ARE_NOT_PERMITTED_IN_MODULE_AUGMENTATIONS,
                    &[],
                );
            }
            Kind::ImportEqualsDeclaration => {
                // import a = e.x; in module augmentation is ok, but not
                // import a = require('fs')
                if !is_internal_module_import_equals_declaration(self.store(), node) {
                    self.grammar_error_on_first_token(
                        node,
                        &diag::IMPORTS_ARE_NOT_PERMITTED_IN_MODULE_AUGMENTATIONS_CONSIDER_MOVING_THEM_TO_THE_ENCLOSING_EXTERNAL_MODULE,
                        &[],
                    );
                }
            }
            Kind::ImportDeclaration | Kind::JSImportDeclaration => {
                self.grammar_error_on_first_token(
                    node,
                    &diag::IMPORTS_ARE_NOT_PERMITTED_IN_MODULE_AUGMENTATIONS_CONSIDER_MOVING_THEM_TO_THE_ENCLOSING_EXTERNAL_MODULE,
                    &[],
                );
            }
            Kind::BindingElement | Kind::VariableDeclaration => {
                let name = self.store().node(node).name();
                if name.is_some_and(|n| is_binding_pattern(self.store(), n)) {
                    for el in binding_pattern_elements(self.store(), name.expect("checked")) {
                        // mark individual names in binding pattern
                        self.check_module_augmentation_element(el);
                    }
                }
            }
            _ => {}
        }
    }

    /// Go: `func (c *Checker) checkExternalModuleExports(node)` (checker.go:5888).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn check_external_module_exports(&mut self, node: NodeId) {
        let Some(module_symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        if !self.module_symbol_links.get(module_symbol).exports_checked {
            // Go: moduleSymbol.Exports[InternalSymbolNameExportEquals]
            let export_equals_symbol = self
                .program_symbol(module_symbol)
                .exports
                .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
                .copied();
            // An export assignment is in error if (a) the module exports
            // value members or (b) if the module exports type or namespace
            // members and the exported entity also exports type or namespace
            // members.
            if let Some(export_equals_symbol) = export_equals_symbol {
                if self.has_exported_members_of_kind(module_symbol, SymbolFlags::VALUE)
                    || self.has_shadowed_namespace(export_equals_symbol)
                {
                    let declaration = self
                        .get_declaration_of_alias_symbol(export_equals_symbol)
                        .or(self.program_symbol(export_equals_symbol).value_declaration);
                    if declaration
                        .is_some_and(|decl| !u::is_top_level_in_external_module_augmentation(self.store(), decl))
                    {
                        self.error(
                            declaration.expect("checked"),
                            &diag::AN_EXPORT_ASSIGNMENT_CANNOT_BE_USED_IN_A_MODULE_WITH_OTHER_EXPORTED_ELEMENTS,
                            &[],
                        );
                    }
                }
            }
            // Checks for export * conflicts
            for (id, symbol) in self
                .program_symbol(module_symbol)
                .exports
                .entries()
                .map(|(k, v)| (k.clone(), *v))
                .collect::<Vec<_>>()
            {
                if id == INTERNAL_SYMBOL_NAME_EXPORT_STAR {
                    continue;
                }
                // ECMA262: 15.2.1.1 (TS exceptions: namespaces, function
                // overloads, enums, and interfaces)
                if self
                    .program_symbol(symbol)
                    .flags
                    .intersects(SymbolFlags::NAMESPACE.union(SymbolFlags::ENUM))
                {
                    continue;
                }
                let exported_declarations_count = self
                    .program_symbol(symbol)
                    .declarations
                    .iter()
                    .filter(|&&d| {
                        is_not_overload(self.store(), d)
                            && !matches!(
                                self.store().node(d).kind,
                                Kind::GetAccessor | Kind::SetAccessor
                            )
                            && self.store().node(d).kind != Kind::InterfaceDeclaration
                    })
                    .count();
                if self.program_symbol(symbol).flags.intersects(SymbolFlags::TYPE_ALIAS)
                    && exported_declarations_count <= 2
                {
                    // it is legal to merge type alias with other values
                    continue;
                }
                let all_exports_property = self
                    .program_symbol(symbol)
                    .declarations
                    .iter()
                    .all(|&_d| {
                        // TODO(port M6 S4): GetAssignmentDeclarationKind —
                        // the JS expando family; permissive false.
                        false
                    });
                if exported_declarations_count > 1 && !all_exports_property {
                    let declarations: Vec<NodeId> =
                        self.program_symbol(symbol).declarations.clone();
                    for declaration in declarations {
                        if is_not_overload(self.store(), declaration) {
                            self.error(
                                declaration,
                                &diag::CANNOT_REDECLARE_EXPORTED_VARIABLE_0,
                                std::slice::from_ref(&id),
                            );
                        }
                    }
                }
            }
            self.module_symbol_links.get(module_symbol).exports_checked = true;
        }
    }

    /// Go: `func (c *Checker) hasExportedMembersOfKind(moduleSymbol, kind) bool`
    /// (checker.go:5915).
    #[allow(dead_code)] // lands with the import/export batch wiring (Go 5420-5943)
    pub(crate) fn has_exported_members_of_kind(&mut self, module_symbol: SymbolId, kind: SymbolFlags) -> bool {
        // PORT: getSymbolFlags is &mut (the S6 alias-walk); snapshot the
        // export symbols first so the loop can mutate.
        let export_symbols: Vec<SymbolId> = self
            .program_symbol(module_symbol)
            .exports
            .values()
            .copied()
            .collect();
        export_symbols.into_iter().any(|symbol| {
            symbol_name(self, symbol) != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                && self.get_symbol_flags(symbol).intersects(kind)
        })
    }

    /// Go: `func (c *Checker) hasShadowedNamespace(symbol) bool` (checker.go:5923).
    pub(crate) fn has_shadowed_namespace(&mut self, symbol: SymbolId) -> bool {
        // TODO(port M6 S6): resolveAlias — the alias target walk; the
        // permissive typeofsymbol body answers unknownSymbol (no namespace),
        // so the shadow gate rests.
        if self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::NAMESPACE_MODULE.union(SymbolFlags::ALIAS))
        {
            let target = self.resolve_alias(symbol);
            if self.program_symbol(target).flags.intersects(SymbolFlags::NAMESPACE)
                && self.has_exported_members_of_kind(
                    target,
                    SymbolFlags::TYPE.union(SymbolFlags::NAMESPACE),
                )
            {
                return true;
            }
        }
        false
    }

    /// Go: `func getEffectiveDeclarationFlags(n, flagsToCheck)` (checker.go:3736)
    /// — the REAL body (children of classes should not be marked as ambient
    /// or export because those flags have no useful semantics there).
    pub(crate) fn get_effective_declaration_flags_real(
        &self,
        n: NodeId,
        flags_to_check: ModifierFlags,
    ) -> ModifierFlags {
        let mut flags = tsc_binder::ast_shims::get_combined_modifier_flags(self.store(), n);
        // Go (checker.go:3736): `!ast.IsInterfaceDeclaration(n.Parent) && …`
        // reads n.Parent unconditionally — the callers pass declaration
        // nodes whose parents exist. The port's node ids reach the same
        // body for synthetic/parentless nodes (the NodeId::NONE sentinel);
        // a missing parent makes every IsX(n.Parent) arm false (Go's nil
        // shape would deref — the sentinel guard keeps the observable
        // "no parent-kind exclusion" result without the store panic).
        let parent = self.store().node(n).parent.get();
        let parent_kind = if parent.is_none() {
            Kind::Unknown
        } else {
            self.store().node(parent).kind
        };
        if parent_kind != Kind::InterfaceDeclaration
            && parent_kind != Kind::ClassDeclaration
            && parent_kind != Kind::ClassExpression
            && self.store().node(n).flags.intersects(NF::AMBIENT)
        {
            if let Some(container) = u::get_enclosing_container(self.store(), n) {
                // Go: `!(ast.IsModuleBlock(n.Parent) && ast.IsGlobalScopeAugmentation(n.Parent.Parent))`
                // — the n.Parent.Parent read is gated by Go's && short-circuit
                // (only a module-block parent consults the grandparent); the
                // stub below preserves the same laziness.
                let is_module_block = parent_kind == Kind::ModuleBlock;
                let is_global_scope_augmentation =
                    is_module_block && Self::is_global_scope_augmentation_of(parent);
                if self.store().node(container).flags.intersects(NF::EXPORT_CONTEXT)
                    && !flags.intersects(ModifierFlags::AMBIENT)
                    && !is_global_scope_augmentation
                {
                    // It is nested in an ambient export context, which
                    // means it is automatically exported
                    flags |= ModifierFlags::EXPORT;
                }
            }
            flags |= ModifierFlags::AMBIENT;
        }
        ModifierFlags(flags.bits() & flags_to_check.bits())
    }

    /// TODO(port M6 W3): `ast.IsGlobalScopeAugmentation` (the `global`-keyword
    /// module augmentation check) — the ambient module seam; permissive false
    /// (the Go grandparent read is gated by getEffectiveDeclarationFlags's
    /// && short-circuit, which the false stub preserves).
    #[allow(dead_code)]
    fn is_global_scope_augmentation_of(_module_block: NodeId) -> bool {
        false
    }

    /// TODO(port M6 S7): `LocalTypeParameters` — the resolved local type-
    /// parameter row on a class/interface type; permissive empty (the
    /// identical-list check trivially passes with no target parameters,
    /// matching an untyped-parameter class).
    fn get_local_type_parameters(&mut self, _t: TypeId) -> Vec<TypeId> {
        Vec::new()
    }

    // Go: `func (c *Checker) isValidBaseType(t) bool` — S8's REAL body
    // (members.rs) replaced the S7 permissive stub; the method resolves
    // there on this shared type.
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers for the class/interface/enum/module batch.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `isPrototypeProperty` — the prototype-named synthesized symbol.
fn is_prototype_property(c: &Checker, symbol: SymbolId) -> bool {
    c.program_symbol(symbol).name == "prototype"
}

/// Go: `ast.GetClassLikeDeclarationOfSymbol` — first class-like declaration
/// of the symbol carrying `t`.
fn get_class_like_declaration_of_symbol(c: &mut Checker, t: TypeId) -> Option<NodeId> {
    c.type_(t).symbol.and_then(|symbol| {
        c.program_symbol(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_class_like(c.store(), d))
    })
}

/// Go: `ast.GetImplementsHeritageClauseElements`.
pub(crate) fn get_implements_heritage_clause_elements(
    store: &dyn NodeStore,
    node: NodeId,
) -> Vec<NodeId> {
    get_heritage_clause_elements_of_token(store, node, Kind::ImplementsKeyword)
}

/// Go: `ast.GetExtendsHeritageClauseElements` (the interface form).
pub(crate) fn get_extends_heritage_clause_elements(
    store: &dyn NodeStore,
    node: NodeId,
) -> Vec<NodeId> {
    get_heritage_clause_elements_of_token(store, node, Kind::ExtendsKeyword)
}

fn get_heritage_clause_elements_of_token(
    store: &dyn NodeStore,
    node: NodeId,
    token: Kind,
) -> Vec<NodeId> {
    let n = store.node(node);
    let clauses = match n.kind {
        Kind::ClassDeclaration => n.as_class_declaration().expect("data").heritage_clauses.as_ref(),
        Kind::ClassExpression => n.as_class_expression().expect("data").heritage_clauses.as_ref(),
        Kind::InterfaceDeclaration => {
            n.as_interface_declaration().expect("data").heritage_clauses.as_ref()
        }
        _ => None,
    };
    let Some(clauses) = clauses else {
        return Vec::new();
    };
    clauses
        .nodes
        .iter()
        .copied()
        .filter(|&clause| {
            let c = store.node(clause);
            c.kind == Kind::HeritageClause
                && c.as_heritage_clause().expect("HeritageClause data").token == token
        })
        .flat_map(|clause| {
            store
                .node(clause)
                .as_heritage_clause()
                .expect("HeritageClause data")
                .types
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default()
        })
        .collect()
}

/// Go: `containerAllowsBlockScopedVariable` (grammarchecks.go:1796).
pub(crate) fn container_allows_block_scoped_variable(
    store: &dyn NodeStore,
    parent: NodeId,
) -> bool {
    let kind = store.node(parent).kind;
    !matches!(
        kind,
        Kind::IfStatement
            | Kind::DoStatement
            | Kind::WhileStatement
            | Kind::WithStatement
            | Kind::ForStatement
            | Kind::ForInStatement
            | Kind::ForOfStatement
            | Kind::LabeledStatement
    ) || (kind == Kind::LabeledStatement
        && container_allows_block_scoped_variable(store, store.node(parent).parent.get()))
}

/// Go: `ast.IsClassElement`.
fn is_class_element(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::PropertyDeclaration
            | Kind::PropertySignature
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::Constructor
            | Kind::IndexSignature
            | Kind::ClassStaticBlockDeclaration
            | Kind::SemicolonClassElement
    )
}

/// TODO(port M6 S8): `isNonBindableDynamicName` — permissive false.
fn is_non_bindable_dynamic_name_stub(_c: &Checker, _name: NodeId) -> bool {
    false
}

/// Go: `isNamedMember` — names not carrying the internal prefix.
fn is_named_member(name: &str) -> bool {
    !name.starts_with('\u{FE}')
}

/// Go: `isModuleWithStringLiteralName`.
#[allow(dead_code)] // lands with the import/export batch (Go 5420-5943)
fn is_module_with_string_literal_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind != Kind::ModuleDeclaration {
        return false;
    }
    let d = n.as_module_declaration().expect("ModuleDeclaration data");
    d.name != NodeId::NONE && store.node(d.name).kind == Kind::StringLiteral
}

/// Go: `ast.IsInternalModuleImportEqualsDeclaration`.
fn is_internal_module_import_equals_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind != Kind::ImportEqualsDeclaration {
        return false;
    }
    let module_reference = n
        .as_import_equals_declaration()
        .expect("ImportEqualsDeclaration data")
        .module_reference;
    // Go: !IsExternalModuleReference(moduleReference) — the internal
    // `import a = N.S` form.
    store.node(module_reference).kind != Kind::ExternalModuleReference
}

/// Go: `func isInstantiatedModule(node, preserveConstEnums) bool` (checker.go:5402).
pub(crate) fn is_instantiated_module(
    store: &dyn NodeStore,
    node: NodeId,
    preserve_const_enums: bool,
) -> bool {
    let module_state = tsc_binder::ast_shims::get_module_instance_state(store, node);
    module_state == tsc_binder::ast_shims::ModuleInstanceState::Instantiated
        || (preserve_const_enums
            && module_state == tsc_binder::ast_shims::ModuleInstanceState::ConstEnumOnly)
}

/// Go: `func getFirstNonAmbientClassOrFunctionDeclaration(symbol)` (checker.go:5409).
fn get_first_non_ambient_class_or_function_declaration(
    c: &Checker,
    symbol: SymbolId,
) -> Option<NodeId> {
    c.program_symbol(symbol).declarations.iter().copied().find(|&declaration| {
        let kind = c.store().node(declaration).kind;
        (kind == Kind::ClassDeclaration
            || (kind == Kind::FunctionDeclaration
                && function_body_of(c.store(), declaration).is_some()))
            && !c.store().node(declaration).flags.intersects(NF::AMBIENT)
    })
}

/// Go: `func isNotOverload(node) bool` (checker.go:5929).
#[allow(dead_code)] // lands with the import/export batch (Go 5420-5943)
pub(crate) fn is_not_overload(store: &dyn NodeStore, node: NodeId) -> bool {
    let kind = store.node(node).kind;
    (kind != Kind::FunctionDeclaration && kind != Kind::MethodDeclaration)
        || function_body_of(store, node).is_some()
}

/// The local index-signature declaration of an IndexInfo on `t` (the
/// errorNode (b)/(c) arms of the index-constraint checks; the (a) property
/// arm needs the S10 parent-symbol seam).
fn get_local_index_declaration(
    c: &Checker,
    _t: TypeId,
    info: crate::types::IndexInfoId,
) -> Option<NodeId> {
    c.index_infos[info.0 as usize].declaration
}

/// `symbol.Name` read helper (the SymbolTable value's name).
#[allow(dead_code)] // lands with the import/export batch (Go 5420-5943)
pub(crate) fn symbol_name(c: &Checker, symbol: SymbolId) -> String {
    c.program_symbol(symbol).name.clone()
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S3 batch 5 — the import/export family (Go checker.go:5420-5943). Same
// Go-name/shape discipline. Permissive seams (bannered inline):
//   * program.GetEmitModuleFormatOfFile / GetImpliedNodeFormatForEmit — the
//     checker's Program trait doesn't expose per-file emit format yet
//     (TODO(port M6 S?): the seam approximates with checker.module_kind).
//   * GetEmitSyntaxForModuleSpecifierExpression — permissive never-CommonJS.
//   * getResolutionModeOverride — compiler-crate seam; permissive None.
//   * isOnlyImportableAsDefault / markLinkedReferences / getTypeOnlyAliasEx /
//     addTypeOnlyDeclarationRelatedInfo — permissive; S6/reference seams.
//   * resolveEntityName — routes to the S6 permissive resolve_name.
// ────────────────────────────────────────────────────────────────────────────

use tsc_core::options_generated::ModuleKind;

impl Checker {
    /// Go: `func (c *Checker) checkImportDeclaration(node)` (checker.go:5420).
    pub(crate) fn check_import_declaration(&mut self, node: NodeId) {
        let diagnostic: &'static Message = if tsc_ast::utilities::is_in_js_file(self.store().node(node)) {
            &diag::AN_IMPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_MODULE
        } else {
            &diag::AN_IMPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_NAMESPACE_OR_MODULE
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            // If we hit an import declaration in an illegal context, just
            // bail out to avoid cascading errors.
            self.check_external_module_name_in_global_scope(node);
            return;
        }
        if !self.check_grammar_modifiers(node) && self.store().node(node).modifiers().is_some() {
            self.grammar_error_on_first_token(node, &diag::AN_IMPORT_DECLARATION_CANNOT_HAVE_MODIFIERS, &[]);
        }
        if self.check_external_import_or_export_declaration(node) {
            let (attributes, import_clause, module_specifier) = {
                let d = self
                    .store()
                    .node(node)
                    .as_import_declaration()
                    .expect("ImportDeclaration data");
                (d.attributes, d.import_clause, d.module_specifier)
            };
            let mut resolved_module: Option<SymbolId> = None;
            let import_clause_ok = import_clause
                .is_some_and(|clause| !self.check_grammar_import_clause(clause));
            if import_clause_ok {
                let clause = import_clause.expect("checked");
                let (clause_name, named_bindings, clause_is_type_only) = {
                    let d = self
                        .store()
                        .node(clause)
                        .as_import_clause()
                        .expect("ImportClause data");
                    (d.name, d.named_bindings, d.phase_modifier == Some(Kind::TypeKeyword))
                };
                if clause_name.is_some() {
                    self.check_import_binding(clause);
                }
                let mut needs_import_star = false;
                if let Some(named_bindings) = named_bindings {
                    if tsc_ast::is_namespace_import(self.store().node(named_bindings)) {
                        self.check_import_binding(named_bindings);
                        if self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS {
                            // import * as ns from "foo";
                            needs_import_star = true;
                            self.check_external_emit_helpers_stub(
                                node,
                                ExternalEmitHelpers::IMPORT_STAR,
                            );
                        }
                    } else {
                        let attributes_type = self.get_type_from_import_attributes(attributes);
                        // Go: c.resolveExternalModuleName(node,
                        // node.ModuleSpecifier(), false /*ignoreErrors*/, …)
                        // — the named-import arm reports TS2307/TS2792.
                        resolved_module = self.resolve_external_module_name_ex(
                            node,
                            Some(module_specifier),
                            false,
                            attributes_type,
                        );
                        if resolved_module.is_some() {
                            let elements = self
                                .store()
                                .node(named_bindings)
                                .as_named_imports()
                                .and_then(|d| d.elements.clone())
                                .map(|l| l.nodes.to_vec())
                                .unwrap_or_default();
                            for binding in elements {
                                self.check_import_binding(binding);
                            }
                        }
                    }
                }
                if clause_name.is_some()
                    && !needs_import_star
                    && self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS
                {
                    // import d from "foo";
                    self.check_external_emit_helpers_stub(
                        node,
                        ExternalEmitHelpers::IMPORT_DEFAULT,
                    );
                }

                if !clause_is_type_only
                    && ModuleKind::Node18 <= self.module_kind
                    && self.module_kind <= ModuleKind::NodeNext
                    && self.is_only_importable_as_default_stub(module_specifier, resolved_module)
                    && !has_type_json_import_attribute(self.store(), node)
                {
                    // TODO(port M6 S6): hasTypeJsonImportAttribute reads the
                    // attribute table above; the message arg is Go's
                    // c.moduleKind.String().
                    self.error(
                        module_specifier,
                        &diag::IMPORTING_A_JSON_FILE_INTO_AN_ECMASCRIPT_MODULE_REQUIRES_A_TYPE_COLON_JSON_IMPORT_ATTRIBUTE_WHEN_MODULE_IS_SET_TO_0,
                        &[module_kind_string(self.module_kind)],
                    );
                }
            } else if self.compiler_options.no_unchecked_side_effect_imports == Tristate::TSTrue
                && import_clause.is_none()
            {
                let ignore_errors = self.compiler_options.no_check == Tristate::TSTrue;
                let error_message: Option<&'static Message> = if ignore_errors {
                    None
                } else {
                    Some(
                        &diag::CANNOT_FIND_MODULE_OR_TYPE_DECLARATIONS_FOR_SIDE_EFFECT_IMPORT_OF_0,
                    )
                };
                let attributes_type = self.get_type_from_import_attributes(attributes);
                self.resolve_external_module_name_worker(
                    node,
                    Some(module_specifier),
                    error_message,
                    ignore_errors,
                    false, /*isForAugmentation*/
                    attributes_type,
                );
            }
        }
        self.check_import_attributes(node);
    }

    /// Go: `func (c *Checker) checkExternalImportOrExportDeclaration(node) bool`
    /// (checker.go:5460).
    pub(crate) fn check_external_import_or_export_declaration(&mut self, node: NodeId) -> bool {
        let Some(module_name) = get_external_module_name(self.store(), node) else {
            // Should be a parse error.
            return false;
        };
        if tsc_ast::utilities::node_is_missing(self.store().node(module_name)) {
            return false;
        }
        if !tsc_ast::is_string_literal(self.store().node(module_name)) {
            self.error(module_name, &diag::STRING_LITERAL_EXPECTED, &[]);
            return false;
        }
        let parent = self.store().node(node).parent.get();
        let in_ambient_external_module = tsc_ast::is_module_block(self.store().node(parent))
            && tsc_binder::ast_shims::is_ambient_module(
                self.store(),
                self.store().node(parent).parent.get(),
            );
        if !tsc_ast::is_source_file(self.store().node(parent)) && !in_ambient_external_module {
            let message = if tsc_ast::is_export_declaration(self.store().node(node)) {
                &diag::EXPORT_DECLARATIONS_ARE_NOT_PERMITTED_IN_A_NAMESPACE
            } else {
                &diag::IMPORT_DECLARATIONS_IN_A_NAMESPACE_CANNOT_REFERENCE_A_MODULE
            };
            self.error(module_name, message, &[]);
            return false;
        }
        if in_ambient_external_module
            && tsc_tspath::is_external_module_name_relative(tsc_ast::node_text(self.store(), module_name))
        {
            // we have already reported errors on top level imports/exports in
            // external module augmentations in checkModuleDeclaration
            // no need to do this again.
            if !u::is_top_level_in_external_module_augmentation(self.store(), node) {
                // TypeScript 1.0 spec (April 2013): 12.1.6
                // An ExternalImportDeclaration in an AmbientExternalModuleDeclaration may reference
                // other external modules only through top - level external module names.
                // Relative external module names are not permitted.
                self.error(
                    node,
                    &diag::IMPORT_OR_EXPORT_DECLARATION_IN_AN_AMBIENT_MODULE_DECLARATION_CANNOT_REFERENCE_MODULE_THROUGH_RELATIVE_MODULE_NAME,
                    &[],
                );
                return false;
            }
        }
        if !tsc_ast::is_import_equals_declaration(self.store().node(node)) {
            if let Some(attributes) = get_import_attributes(self.store(), node) {
                if self.check_grammar_import_attribute_values(attributes) {
                    return false;
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) checkImportBinding(node)` (checker.go:5498).
    pub(crate) fn check_import_binding(&mut self, node: NodeId) {
        let name = self.store().node(node).name();
        self.check_collisions_for_declaration_name_stub(node, name);
        self.check_alias_symbol(node);
        if tsc_ast::is_import_specifier(self.store().node(node)) {
            let (property_name, property_name_or_name) = {
                let d = self
                    .store()
                    .node(node)
                    .as_import_specifier()
                    .expect("ImportSpecifier data");
                (d.property_name, d.property_name.unwrap_or(d.name))
            };
            self.check_module_export_name(property_name, true /*allowStringLiteral*/);
            if tsc_binder::ast_shims::module_export_name_is_default(
                self.store(),
                property_name_or_name,
            ) && self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS
            {
                self.check_external_emit_helpers_stub(
                    node,
                    ExternalEmitHelpers::IMPORT_DEFAULT,
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkModuleExportName(name, allowStringLiteral)` (checker.go:5512).
    pub(crate) fn check_module_export_name(
        &mut self,
        name: Option<NodeId>,
        allow_string_literal: bool,
    ) {
        let Some(name) = name else { return };
        if self.store().node(name).kind != Kind::StringLiteral {
            return;
        }
        if !allow_string_literal {
            self.grammar_error_on_node(name, &diag::IDENTIFIER_EXPECTED, &[]);
        } else if self.module_kind == ModuleKind::ES2015 || self.module_kind == ModuleKind::ES2020 {
            // Go: !ast.GetSourceFileOfNode(name).IsDeclarationFile.
            let is_declaration_file = tsc_ast::utilities::get_source_file_of_node(
                self.store(),
                name,
            )
            .is_some_and(|f| {
                f.node(f.as_node_id())
                    .as_source_file()
                    .expect("SourceFile data")
                    .is_declaration_file
            });
            if !is_declaration_file {
                self.grammar_error_on_node(
                    name,
                    &diag::STRING_LITERAL_IMPORT_AND_EXPORT_NAMES_ARE_NOT_SUPPORTED_WHEN_THE_MODULE_FLAG_IS_SET_TO_ES2015_OR_ES2020,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkImportAttributes(declaration)` (checker.go:5532).
    pub(crate) fn check_import_attributes(&mut self, declaration: NodeId) {
        let Some(node) = get_import_attributes(self.store(), declaration) else {
            return;
        };
        let import_attributes_type = self.get_global_import_attributes_type_checked();
        let empty_object_type = self.empty_object_type();
        if import_attributes_type != empty_object_type {
            let attributes_type = self
                .get_type_from_import_attributes(Some(node))
                .expect("ImportAttributes present");
            let undefined_type = self.get_nullable_type(import_attributes_type, TypeFlags::UNDEFINED);
            self.check_type_assignable_to(attributes_type, undefined_type, Some(node), None);
        }
        let is_type_only = is_exclusively_type_only_import_or_export(self.store(), declaration)
            || tsc_ast::is_import_type_node(self.store().node(declaration));
        // TODO(port M6 S?): getResolutionModeOverride (the compiler-crate
        // ImportAttributesNode seam); permissive none — the X_resolution_mode
        // grammar error rests until the seam lands.
        let has_override = false;
        if is_type_only {
            return; // Other grammar checks do not apply to type-only imports with import attributes
        }

        if !self.module_kind.supports_import_attributes() {
            self.grammar_error_on_node(
                node,
                &diag::IMPORT_ATTRIBUTES_ARE_ONLY_SUPPORTED_WHEN_THE_MODULE_OPTION_IS_SET_TO_ESNEXT_NODE18_NODE20_NODENEXT_OR_PRESERVE,
                &[],
            );
            return;
        }

        // TODO(port M6 S?): getEmitSyntaxForModuleSpecifierExpression — the
        // per-specifier emit-syntax seam; permissive never-CommonJS so the
        // CommonJS grammar error rests until the seam lands.
        let emits_common_js_require = false;
        if emits_common_js_require {
            self.grammar_error_on_node(
                node,
                &diag::IMPORT_ATTRIBUTES_ARE_NOT_ALLOWED_ON_STATEMENTS_THAT_COMPILE_TO_COMMONJS_REQUIRE_CALLS,
                &[],
            );
            return;
        }

        if has_override {
            self.grammar_error_on_node(
                node,
                &diag::X_RESOLUTION_MODE_CAN_ONLY_BE_SET_FOR_TYPE_ONLY_IMPORTS,
                &[],
            );
        }
    }

    /// Go: `func (c *Checker) getTypeFromImportAttributes(node) *Type` (checker.go:5564).
    pub(crate) fn get_type_from_import_attributes(
        &mut self,
        node: Option<NodeId>,
    ) -> Option<TypeId> {
        let node = node?;
        if tsc_ast::is_import_attributes(self.store().node(node)) {
            Some(self.check_import_attributes_expression(node))
        } else {
            Some(self.check_expression_cached(node))
        }
    }

    /// Go: `func (c *Checker) checkImportAttributesExpression(node) *Type` (checker.go:5573).
    pub(crate) fn check_import_attributes_expression(&mut self, node: NodeId) -> TypeId {
        let cached = self.type_node_links.get(node).resolved_type;
        if let Some(t) = cached {
            return t;
        }
        let symbol = self.new_symbol(
            SymbolFlags::OBJECT_LITERAL,
            tsc_ast::INTERNAL_SYMBOL_NAME_IMPORT_ATTRIBUTES,
        );
        let attributes = self
            .store()
            .node(node)
            .as_import_attributes()
            .expect("ImportAttributes data")
            .attributes
            .clone()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        let mut members = tsc_ast::SymbolTable::default();
        for attribute in attributes {
            let (name_text, value) = {
                let d = self
                    .store()
                    .node(attribute)
                    .as_import_attribute()
                    .expect("ImportAttribute data");
                (tsc_ast::node_text(self.store(), d.name).to_string(), d.value)
            };
            let member = self.new_symbol(SymbolFlags::PROPERTY, &name_text);
            let value_type = self.check_expression_cached(value);
            let regular = self.get_regular_type_of_literal_type(value_type);
            self.value_symbol_links.get(member).resolved_type = Some(regular);
            members.set(name_text.clone(), member);
        }
        let t = self.new_anonymous_type_with_members(Some(symbol), members, crate::types::ObjectFlags::NONE);
        // Go: t.objectFlags |= ObjectFlagsObjectLiteral | ObjectFlagsNonInferrableType.
        // TODO(port M6 S?): the objectFlags OR on the arena row — the
        // anonymous-literal object-flags seam; permissive unset until then.
        // Go: links.resolvedType = t
        self.type_node_links.get(node).resolved_type = Some(t);
        t
    }

    /// Go: `func (c *Checker) getImportAttributesTypeForModuleSpecifier(moduleSpecifier) *Type`
    /// (checker.go:5593).
    #[allow(dead_code)] // lands with S6's resolveExternalModuleName call sites
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn get_import_attributes_type_for_module_specifier(
        &mut self,
        module_specifier: NodeId,
    ) -> Option<TypeId> {
        let parent = self.store().node(module_specifier).parent.get();
        let parent_kind = self.store().node(parent).kind;
        // Go: ast.IsImportDeclarationOrJSImportDeclaration(parent) ||
        // ast.IsExportDeclaration(parent)
        if parent_kind == Kind::ImportDeclaration
            || parent_kind == Kind::JSImportDeclaration
            || parent_kind == Kind::ExportDeclaration
        {
            return self.get_type_from_import_attributes(get_import_attributes(self.store(), parent));
        }
        if parent_kind == Kind::LiteralType
            && self
                .store()
                .node(self.store().node(parent).parent.get())
                .kind
                == Kind::ImportType
        {
            let grandparent = self.store().node(parent).parent.get();
            return self.get_type_from_import_attributes(get_import_attributes(self.store(), grandparent));
        }
        if parent_kind == Kind::CallExpression {
            let arguments = self
                .store()
                .node(parent)
                .as_call_expression()
                .and_then(|d| d.arguments.clone())
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            if arguments.len() > 1 {
                let options = arguments[1];
                let options_type = self.check_expression_cached(options);
                return self.get_type_of_property_of_type(options_type, "with");
            }
        }
        None
    }

    /// Go: `func (c *Checker) checkImportEqualsDeclaration(node)` (checker.go:5614).
    pub(crate) fn check_import_equals_declaration(&mut self, node: NodeId) {
        let diagnostic: &'static Message = if tsc_ast::utilities::is_in_js_file(self.store().node(node)) {
            &diag::AN_IMPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_MODULE
        } else {
            &diag::AN_IMPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_NAMESPACE_OR_MODULE
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            self.check_external_module_name_in_global_scope(node);
            return; // If we hit an import declaration in an illegal context, just bail out to avoid cascading errors.
        }
        self.check_grammar_modifiers(node);
        if self.should_check_erasable_syntax(node)
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
        {
            self.error(
                node,
                &diag::THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
                &[],
            );
        }
        let (is_type_only, module_reference) = {
            let d = self
                .store()
                .node(node)
                .as_import_equals_declaration()
                .expect("ImportEqualsDeclaration data");
            (d.is_type_only, d.module_reference)
        };
        if is_internal_module_import_equals_declaration(self.store(), node)
            || self.check_external_import_or_export_declaration(node)
        {
            self.check_import_binding(node);
            // TODO(port M6 S?): markLinkedReferences (ReferenceHintExportImportEquals)
            // — the reference-marking seam; permissive no-op.
            if !tsc_ast::is_external_module_reference(self.store().node(module_reference)) {
                let Some(symbol) = self.get_symbol_of_declaration(node) else {
                    return;
                };
                let target = self.resolve_alias(symbol);
                if target != self.unknown_symbol() {
                    let target_flags = self.get_symbol_flags(target);
                    if target_flags.intersects(SymbolFlags::VALUE) {
                        // Target is a value symbol, check that it is not
                        // hidden by a local declaration with the same name
                        let module_name = get_first_identifier(self.store(), module_reference);
                        // TODO(port M6 S6): resolveEntityName — routed to the
                        // permissive resolveName; the hidden-module error
                        // rests until S6's real resolution lands.
                        let module_name_text = tsc_ast::node_text(self.store(), module_name).to_string();
                        let resolved = self.resolve_name(
                            &module_name_text,
                            module_name,
                            SymbolFlags::VALUE.union(SymbolFlags::NAMESPACE),
                            false,
                        );
                        let resolved_is_namespace = resolved.is_some_and(|s| {
                            self.get_symbol_flags(s).intersects(SymbolFlags::NAMESPACE)
                        });
                        if !resolved_is_namespace {
                            let name_text =
                                tsc_scanner::utilities::declaration_name_to_string(
                                    self.store(),
                                    Some(module_name),
                                );
                            self.error(
                                module_name,
                                &diag::MODULE_0_IS_HIDDEN_BY_A_LOCAL_DECLARATION_WITH_THE_SAME_NAME,
                                &[name_text],
                            );
                        }
                    }
                    if target_flags.intersects(SymbolFlags::TYPE) {
                        let name = self.store().node(node).name();
                        if let Some(name) = name {
                            self.check_type_name_is_reserved(
                                name,
                                &diag::IMPORT_NAME_CANNOT_BE_0,
                            );
                        }
                    }
                }
                if is_type_only {
                    self.grammar_error_on_node(node, &diag::AN_IMPORT_ALIAS_CANNOT_USE_IMPORT_TYPE, &[]);
                }
            } else if ModuleKind::ES2015 <= self.module_kind
                && self.module_kind <= ModuleKind::ESNext
                && !is_type_only
                && !self.store().node(node).flags.intersects(NF::AMBIENT)
            {
                // Import equals declaration cannot be emitted as ESM
                self.grammar_error_on_node(
                    node,
                    &diag::IMPORT_ASSIGNMENT_CANNOT_BE_USED_WHEN_TARGETING_ECMASCRIPT_MODULES_CONSIDER_USING_IMPORT_ASTERISK_AS_NS_FROM_MOD_IMPORT_A_FROM_MOD_IMPORT_D_FROM_MOD_OR_ANOTHER_MODULE_FORMAT_INSTEAD,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkExportDeclaration(node)` (checker.go:5666).
    pub(crate) fn check_export_declaration(&mut self, node: NodeId) {
        let diagnostic: &'static Message = if tsc_ast::utilities::is_in_js_file(self.store().node(node)) {
            &diag::AN_EXPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_MODULE
        } else {
            &diag::AN_EXPORT_DECLARATION_CAN_ONLY_BE_USED_AT_THE_TOP_LEVEL_OF_A_NAMESPACE_OR_MODULE
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            self.check_external_module_name_in_global_scope(node);
            return; // If we hit an export in an illegal context, just bail out to avoid cascading errors.
        }
        let (export_clause, module_specifier, is_type_only) = {
            let d = self
                .store()
                .node(node)
                .as_export_declaration()
                .expect("ExportDeclaration data");
            (d.export_clause, d.module_specifier, d.is_type_only)
        };
        let _ = is_type_only;
        if !self.check_grammar_modifiers(node) && self.store().node(node).modifiers().is_some() {
            self.grammar_error_on_first_token(node, &diag::AN_EXPORT_DECLARATION_CANNOT_HAVE_MODIFIERS, &[]);
        }
        self.check_grammar_export_declaration(node);
        if module_specifier.is_none() || self.check_external_import_or_export_declaration(node) {
            if let Some(export_clause) = export_clause
                && !tsc_ast::is_namespace_export(self.store().node(export_clause))
            {
                // export { x, y }
                // export { x, y } from "foo"
                let elements = self
                    .store()
                    .node(export_clause)
                    .as_named_exports()
                    .and_then(|d| d.elements.clone())
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default();
                for binding in elements {
                    self.check_export_specifier(binding);
                }
                let parent = self.store().node(node).parent.get();
                let in_ambient_external_module = tsc_ast::is_module_block(self.store().node(parent))
                    && tsc_binder::ast_shims::is_ambient_module(
                        self.store(),
                        self.store().node(parent).parent.get(),
                    );
                let in_ambient_namespace_declaration = !in_ambient_external_module
                    && tsc_ast::is_module_block(self.store().node(parent))
                    && module_specifier.is_none()
                    && self.store().node(node).flags.intersects(NF::AMBIENT);
                if !tsc_ast::is_source_file(self.store().node(parent))
                    && !in_ambient_external_module
                    && !in_ambient_namespace_declaration
                {
                    self.error(
                        node,
                        &diag::EXPORT_DECLARATIONS_ARE_NOT_PERMITTED_IN_A_NAMESPACE,
                        &[],
                    );
                }
            } else {
                // export * from "foo"
                // export * as ns from "foo";
                let attributes_type = self.get_type_from_import_attributes(get_import_attributes(
                    self.store(),
                    node,
                ));
                let module_symbol = module_specifier
                    .and_then(|specifier| self.resolve_external_module_name(specifier, attributes_type));
                let module_symbol_has_export_assignment = module_symbol
                    .is_some_and(|symbol| u::has_export_assignment_symbol(self.program_symbol(symbol)));
                if module_symbol_has_export_assignment
                    && let Some(module_symbol) = module_symbol
                {
                    let specifier = module_specifier.expect("module symbol resolved");
                    let symbol_text = relater::symbol_to_string(self, module_symbol);
                    self.error(
                        specifier,
                        &diag::MODULE_0_USES_EXPORT_AND_CANNOT_BE_USED_WITH_EXPORT_ASTERISK,
                        &[symbol_text],
                    );
                } else if let Some(export_clause) = export_clause {
                    self.check_alias_symbol(export_clause);
                    let name = self.store().node(export_clause).name();
                    self.check_module_export_name(name, true /*allowStringLiteral*/);
                }
                if self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS {
                    if export_clause.is_some() {
                        // export * as ns from "foo";
                        self.check_external_emit_helpers_stub(
                            node,
                            ExternalEmitHelpers::IMPORT_STAR,
                        );
                    } else {
                        // export * from "foo"
                        self.check_external_emit_helpers_stub(
                            node,
                            ExternalEmitHelpers::EXPORT_STAR,
                        );
                    }
                }
            }
        }
        self.check_import_attributes(node);
    }

    /// Go: `func (c *Checker) checkExternalModuleNameInGlobalScope(node)` (checker.go:5741).
    pub(crate) fn check_external_module_name_in_global_scope(&mut self, node: NodeId) {
        let enclosing_container = u::get_enclosing_container(self.store(), node);
        let container_is_source_file = enclosing_container.is_some_and(|container| {
            self.store().node(container).kind == Kind::SourceFile
        });
        if !container_is_source_file
            || (is_import_declaration_or_js_import_declaration(self.store(), node)
                && self
                    .store()
                    .node(node)
                    .as_import_declaration()
                    .map(|d| d.import_clause.is_none())
                    .unwrap_or(false))
        {
            return;
        }
        if let Some(module_name) = get_external_module_name(self.store(), node) {
            let attributes = get_import_attributes(self.store(), node);
            let import_attributes_type = self.get_type_from_import_attributes(attributes);
            self.resolve_external_module_name(module_name, import_attributes_type);
        }
    }

    /// Go: `func (c *Checker) checkExportSpecifier(node)` (checker.go:5766).
    pub(crate) fn check_export_specifier(&mut self, node: NodeId) {
        self.check_alias_symbol(node);
        let (property_name, name) = {
            let d = self
                .store()
                .node(node)
                .as_export_specifier()
                .expect("ExportSpecifier data");
            (d.property_name, d.name)
        };
        // Go: node.Parent.Parent.ModuleSpecifier() — ExportSpecifier →
        // NamedExports → ExportDeclaration.
        let module_specifier = self
            .store()
            .node(self.store().node(node).parent.get())
            .parent
            .get();
        let has_module_specifier = self
            .store()
            .node(module_specifier)
            .as_export_declaration()
            .map(|d| d.module_specifier.is_some())
            .unwrap_or(false);
        self.check_module_export_name(property_name, has_module_specifier);
        self.check_module_export_name(Some(name), true /*allowStringLiteral*/);

        let property_name_or_name = property_name.unwrap_or(name);
        if !has_module_specifier {
            if self.store().node(property_name_or_name).kind == Kind::StringLiteral {
                return; // Skip for invalid syntax like this: export { "x" }
            }
            // find immediate value referenced by exported name (SymbolFlags.Alias is set so we don't chase down aliases)
            let exported_name_text =
                tsc_ast::node_text(self.store(), property_name_or_name).to_string();
            let symbol = self.resolve_name(
                &exported_name_text,
                property_name_or_name,
                SymbolFlags::VALUE
                    .union(SymbolFlags::TYPE)
                    .union(SymbolFlags::NAMESPACE)
                    .union(SymbolFlags::ALIAS),
                false /*excludeGlobals*/,
            );
            let symbol_is_global = symbol.is_some_and(|symbol| {
                symbol == self.undefined_symbol()
                    || self.global_this_symbol == Some(symbol)
                    || {
                        // Go: symbol.Declarations != nil &&
                        // ast.IsGlobalSourceFile(ast.GetDeclarationContainer(symbol.Declarations[0]))
                        // TODO(port M6 S?): GetDeclarationContainer — approximated by
                        // the enclosing container walk.
                        let declarations = self.program_symbol(symbol).declarations.clone();
                        declarations.first().is_some_and(|&first| {
                            u::get_enclosing_container(self.store(), first)
                                .is_some_and(|container| {
                                    u::is_global_source_file(self.store(), container)
                                })
                        })
                    }
            });
            if symbol_is_global {
                self.error(
                    property_name_or_name,
                    &diag::CANNOT_EXPORT_0_ONLY_LOCAL_DECLARATIONS_CAN_BE_EXPORTED_FROM_A_MODULE,
                    &[exported_name_text],
                );
            }
            // TODO(port M6 S?): markLinkedReferences (ReferenceHintExportSpecifier)
            // — the reference-marking seam; permissive no-op.
        } else if self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS
            && tsc_binder::ast_shims::module_export_name_is_default(
                self.store(),
                property_name_or_name,
            )
        {
            self.check_external_emit_helpers_stub(node, ExternalEmitHelpers::IMPORT_DEFAULT);
        }
    }

    /// Go: `func (c *Checker) checkExportAssignment(node)` (checker.go:5803).
    pub(crate) fn check_export_assignment(&mut self, node: NodeId) {
        let (is_export_equals, type_node, expression) = {
            let d = self
                .store()
                .node(node)
                .as_export_assignment()
                .expect("ExportAssignment data");
            (d.is_export_equals, d.type_, d.expression)
        };
        // Always check the exported expression so its identifiers are
        // resolved even when the export assignment is misplaced (grammar
        // error), keeping diagnostics stable regardless of traversal order.
        let expr_type = self.check_expression_cached(expression);
        let illegal_context_message: &'static Message = if is_export_equals {
            &diag::AN_EXPORT_ASSIGNMENT_MUST_BE_AT_THE_TOP_LEVEL_OF_A_FILE_OR_MODULE_DECLARATION
        } else {
            &diag::A_DEFAULT_EXPORT_MUST_BE_AT_THE_TOP_LEVEL_OF_A_FILE_OR_MODULE_DECLARATION
        };
        if self.check_grammar_module_element_context(node, illegal_context_message) {
            return; // If we hit an export assignment in an illegal context, just bail out to avoid cascading errors.
        }
        if self.should_check_erasable_syntax(node)
            && is_export_equals
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
        {
            self.error(
                node,
                &diag::THIS_SYNTAX_IS_NOT_ALLOWED_WHEN_ERASABLESYNTAXONLY_IS_ENABLED,
                &[],
            );
        }
        if is_contained_by_namespace(self.store(), node) {
            // TODO(danielr): should these be grammar errors?
            if is_export_equals {
                self.error(node, &diag::AN_EXPORT_ASSIGNMENT_CANNOT_BE_USED_IN_A_NAMESPACE, &[]);
            } else {
                self.error(
                    node,
                    &diag::A_DEFAULT_EXPORT_CAN_ONLY_BE_USED_IN_AN_ECMASCRIPT_STYLE_MODULE,
                    &[],
                );
            }
            return;
        }
        if !self.check_grammar_modifiers(node) && self.store().node(node).modifiers().is_some() {
            self.grammar_error_on_first_token(node, &diag::AN_EXPORT_ASSIGNMENT_CANNOT_HAVE_MODIFIERS, &[]);
        }
        let is_illegal_export_default_in_cjs = !is_export_equals
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
            && self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
            && self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS;
        if is_identifier(self.store().node(expression)) {
            let id = expression;
            let id_text = tsc_ast::node_text(self.store(), id).to_string();
            // TODO(port M6 S6): resolveEntityName(id, SymbolFlagsAll, true
            // ignoreErrors, true dontResolveAlias, node) — routed to the
            // permissive resolveName.
            let sym = self
                .resolve_name(
                    &id_text,
                    id,
                    SymbolFlags::ALL,
                    false,
                )
                .map(|symbol| self.get_export_symbol_of_value_symbol_if_exported(symbol));
            if let Some(sym) = sym {
                // TODO(port M6 S?): markLinkedReferences (ReferenceHintExportAssignment).
                // TODO(port M6 S6): getTypeOnlyAliasDeclarationEx — routed to
                // the permissive S6 stub (None).
                let type_only_declaration = self.get_type_only_alias_declaration(sym);
                let sym_flags = self.get_symbol_flags(sym);
                if sym_flags.intersects(SymbolFlags::VALUE) {
                    // However if it is a value, we need to check it's being used correctly
                    if !is_illegal_export_default_in_cjs
                        && !self.store().node(node).flags.intersects(NF::AMBIENT)
                        && self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                        && type_only_declaration.is_some()
                    {
                        let message = if is_export_equals {
                            &diag::AN_EXPORT_DECLARATION_MUST_REFERENCE_A_REAL_VALUE_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED_BUT_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION
                        } else {
                            &diag::AN_EXPORT_DEFAULT_MUST_REFERENCE_A_REAL_VALUE_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED_BUT_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION
                        };
                        self.error(id, message, std::slice::from_ref(&id_text));
                    }
                } else if !is_illegal_export_default_in_cjs
                    && !self.store().node(node).flags.intersects(NF::AMBIENT)
                    && self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                {
                    let message = if is_export_equals {
                        &diag::AN_EXPORT_DECLARATION_MUST_REFERENCE_A_VALUE_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED_BUT_0_ONLY_REFERS_TO_A_TYPE
                    } else {
                        &diag::AN_EXPORT_DEFAULT_MUST_REFERENCE_A_VALUE_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED_BUT_0_ONLY_REFERS_TO_A_TYPE
                    };
                    self.error(id, message, std::slice::from_ref(&id_text));
                }
                if !is_illegal_export_default_in_cjs
                    && !self.store().node(node).flags.intersects(NF::AMBIENT)
                    && self.compiler_options.get_isolated_modules()
                    && !sym_flags.intersects(SymbolFlags::VALUE)
                {
                    let non_local_meanings = self.get_symbol_flags_ex(
                        sym,
                        false /*excludeTypeOnlyMeanings*/,
                        true /*excludeLocalMeanings*/,
                    );
                    let type_only_declaration_source_file_is_other = type_only_declaration
                        .is_some_and(|declaration| {
                            tsc_ast::utilities::get_source_file_of_node(self.store(), declaration)
                                .map(|f| f.as_node_id())
                                != tsc_ast::utilities::get_source_file_of_node(self.store(), node)
                                    .map(|f| f.as_node_id())
                        });
                    if sym_flags.intersects(SymbolFlags::ALIAS)
                        && non_local_meanings.intersects(SymbolFlags::TYPE)
                        && !non_local_meanings.intersects(SymbolFlags::VALUE)
                        && (type_only_declaration.is_none()
                            || type_only_declaration_source_file_is_other)
                    {
                        // import { SomeType } from "./someModule";
                        // export default SomeType; OR
                        // export = SomeType;
                        let message = if is_export_equals {
                            &diag::X_0_RESOLVES_TO_A_TYPE_AND_MUST_BE_MARKED_TYPE_ONLY_IN_THIS_FILE_BEFORE_RE_EXPORTING_WHEN_1_IS_ENABLED_CONSIDER_USING_IMPORT_TYPE_WHERE_0_IS_IMPORTED
                        } else {
                            &diag::X_0_RESOLVES_TO_A_TYPE_AND_MUST_BE_MARKED_TYPE_ONLY_IN_THIS_FILE_BEFORE_RE_EXPORTING_WHEN_1_IS_ENABLED_CONSIDER_USING_EXPORT_TYPE_0_AS_DEFAULT
                        };
                        let flag_name = self.get_isolated_modules_like_flag_name();
                        self.error(id, message, &[id_text.clone(), flag_name]);
                    } else if type_only_declaration.is_some()
                        && type_only_declaration_source_file_is_other
                    {
                        // import { SomeTypeOnlyValue } from "./someModule";
                        // export default SomeTypeOnlyValue; OR
                        // export = SomeTypeOnlyValue;
                        let message = if is_export_equals {
                            &diag::X_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION_AND_MUST_BE_MARKED_TYPE_ONLY_IN_THIS_FILE_BEFORE_RE_EXPORTING_WHEN_1_IS_ENABLED_CONSIDER_USING_IMPORT_TYPE_WHERE_0_IS_IMPORTED
                        } else {
                            &diag::X_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION_AND_MUST_BE_MARKED_TYPE_ONLY_IN_THIS_FILE_BEFORE_RE_EXPORTING_WHEN_1_IS_ENABLED_CONSIDER_USING_EXPORT_TYPE_0_AS_DEFAULT
                        };
                        // TODO(port M6 S?): addTypeOnlyDeclarationRelatedInfo —
                        // the related-info elaboration seam; permissive: the
                        // base error only.
                        let flag_name = self.get_isolated_modules_like_flag_name();
                        self.error(id, message, &[id_text.clone(), flag_name]);
                    }
                }
            }
        }
        if is_illegal_export_default_in_cjs {
            self.error(
                node,
                get_verbatim_module_syntax_error_message(self.store(), node),
                &[],
            );
        }
        let mut container = self.store().node(node).parent.get();
        if !tsc_ast::is_source_file(self.store().node(container)) {
            container = self.store().node(container).parent.get();
        }
        self.check_external_module_exports(container);
        if type_node != NodeId::NONE && self.store().node(node).kind == Kind::ExportAssignment {
            let t = self.get_type_from_type_node(type_node);
            self.check_type_assignable_to_and_optionally_elaborate(
                expr_type,
                t,
                Some(expression),
                Some(expression),
                None /*headMessage*/,
                None /*diagnosticOutput*/,
            );
        }
        if self.store().node(node).flags.intersects(NF::AMBIENT)
            && !u::is_entity_name_expression(self.store(), expression)
        {
            self.grammar_error_on_node(
                expression,
                &diag::THE_EXPRESSION_OF_AN_EXPORT_ASSIGNMENT_MUST_BE_AN_IDENTIFIER_OR_QUALIFIED_NAME_IN_AN_AMBIENT_CONTEXT,
                &[],
            );
        }
        if is_export_equals {
            // Forbid export= in esm implementation files, and esm mode declaration files
            // TODO(port M6 S?): program.GetImpliedNodeFormatForEmit — the
            // per-file implied-format seam; approximated by the configured
            // module_kind.
            let implied_node_format = self.module_kind;
            if ModuleKind::ES2015 <= self.module_kind
                && self.module_kind != ModuleKind::Preserve
                && ((self.store().node(node).flags.intersects(NF::AMBIENT)
                    && implied_node_format == ModuleKind::ESNext)
                    || (!self.store().node(node).flags.intersects(NF::AMBIENT)
                        && implied_node_format != ModuleKind::CommonJS))
            {
                // export assignment is not supported in es6 modules
                self.grammar_error_on_node(
                    node,
                    &diag::EXPORT_ASSIGNMENT_CANNOT_BE_USED_WHEN_TARGETING_ECMASCRIPT_MODULES_CONSIDER_USING_EXPORT_DEFAULT_OR_ANOTHER_MODULE_FORMAT_INSTEAD,
                    &[],
                );
            } else if self.module_kind == ModuleKind::System
                && !self.store().node(node).flags.intersects(NF::AMBIENT)
            {
                // system modules does not support export assignment
                self.grammar_error_on_node(
                    node,
                    &diag::EXPORT_ASSIGNMENT_IS_NOT_SUPPORTED_WHEN_MODULE_FLAG_IS_SYSTEM,
                    &[],
                );
            }
        }
    }

    /// Go: `func (c *Checker) isOnlyImportableAsDefault(moduleSpecifier, moduleSymbol, importAttributesType) bool`
    /// (checker.go — S6's module resolution). Permissive false until S6.
    fn is_only_importable_as_default_stub(
        &mut self,
        _module_specifier: NodeId,
        _resolved_module: Option<SymbolId>,
    ) -> bool {
        false
    }

    /// Go: `c.program.GetEmitModuleFormatOfFile(ast.GetSourceFileOfNode(node))`
    /// — the per-file emit-format seam. TODO(port M6 S?): the checker's
    /// Program trait doesn't expose the per-file emit format; approximated
    /// by the configured module_kind (identical whenever the file carries no
    /// format-implying extension/package context).
    fn get_emit_module_format_of_file_seam(&self, _node: NodeId) -> ModuleKind {
        self.module_kind
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers for the import/export batch.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `ast.GetExternalModuleName(node)` — the import/export/module-specifier
/// accessor (the parser's pub(crate) twin isn't visible to the checker).
pub(crate) fn get_external_module_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        tsc_ast::NodeData::ImportDeclaration(d) => Some(d.module_specifier),
        tsc_ast::NodeData::ExportDeclaration(d) => d.module_specifier,
        tsc_ast::NodeData::ImportEqualsDeclaration(d) => {
            if tsc_ast::is_external_module_reference(store.node(d.module_reference)) {
                Some(
                    store
                        .node(d.module_reference)
                        .as_external_module_reference()
                        .expect("ExternalModuleReference data")
                        .expression,
                )
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Go: `ast.GetImportAttributes(node)`.
pub(crate) fn get_import_attributes(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match &store.node(node).data {
        tsc_ast::NodeData::ImportDeclaration(d) => d.attributes,
        tsc_ast::NodeData::ExportDeclaration(d) => d.attributes,
        tsc_ast::NodeData::ImportTypeNode(d) => d.attributes,
        tsc_ast::NodeData::ModuleDeclaration(d) => d.attributes,
        _ => None,
    }
}

/// Go: `hasTypeJsonImportAttribute(node)` (checker.go:5526).
fn has_type_json_import_attribute(store: &dyn NodeStore, node: NodeId) -> bool {
    let attributes = match &store.node(node).data {
        tsc_ast::NodeData::ImportDeclaration(d) => d.attributes,
        _ => None,
    };
    let Some(attributes) = attributes else { return false };
    let nodes = store
        .node(attributes)
        .as_import_attributes()
        .and_then(|d| d.attributes.clone())
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default();
    nodes.iter().any(|&attr| {
        let d = store.node(attr).as_import_attribute().expect("ImportAttribute data");
        tsc_ast::node_text(store, d.name) == "type"
            && tsc_binder::ast_shims::is_string_literal_like(store.node(d.value))
            && tsc_ast::node_text(store, d.value) == "json"
    })
}

/// Go: `ast.IsImportDeclarationOrJSImportDeclaration(node)`.
pub(crate) fn is_import_declaration_or_js_import_declaration(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    matches!(store.node(node).kind, Kind::ImportDeclaration | Kind::JSImportDeclaration)
}

/// Go: `ast.IsExclusivelyTypeOnlyImportOrExport(node)` — the type-only walk
/// (the compiler-crate twin isn't visible to the checker).
fn is_exclusively_type_only_import_or_export(store: &dyn NodeStore, node: NodeId) -> bool {
    match store.node(node).kind {
        Kind::ImportDeclaration => store
            .node(node)
            .as_import_declaration()
            .and_then(|d| d.import_clause)
            .is_some_and(|clause| {
                store
                    .node(clause)
                    .as_import_clause()
                    .map(|d| d.phase_modifier == Some(Kind::TypeKeyword))
                    .unwrap_or(false)
            }),
        Kind::ExportDeclaration => store
            .node(node)
            .as_export_declaration()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ImportEqualsDeclaration => store
            .node(node)
            .as_import_equals_declaration()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ImportClause => store
            .node(node)
            .as_import_clause()
            .map(|d| d.phase_modifier == Some(Kind::TypeKeyword))
            .unwrap_or(false),
        Kind::ImportSpecifier => store
            .node(node)
            .as_import_specifier()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ExportSpecifier => store
            .node(node)
            .as_export_specifier()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        _ => false,
    }
}

/// Go: `ast.GetFirstIdentifier(node)` — the leftmost entity-name leaf.
pub(crate) fn get_first_identifier(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut current = node;
    loop {
        match store.node(current).kind {
            Kind::QualifiedName => {
                let d = store
                    .node(current)
                    .as_qualified_name()
                    .expect("QualifiedName data");
                current = d.left;
            }
            _ => return current,
        }
    }
}

/// Go: `isContainedByNamespace(node)` (checker.go:5795).
fn is_contained_by_namespace(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut container = store.node(node).parent.get();
    if !tsc_ast::is_source_file(store.node(container)) {
        container = store.node(container).parent.get();
    }
    store.node(container).kind == Kind::ModuleDeclaration
        && !tsc_binder::ast_shims::is_ambient_module(store, container)
}

/// Go: `getVerbatimModuleSyntaxErrorMessage(node)` (checker.go:5889).
fn get_verbatim_module_syntax_error_message(
    store: &dyn NodeStore,
    node: NodeId,
) -> &'static Message {
    let file_name = tsc_ast::utilities::get_source_file_of_node(store, node)
        .map(|f| f.file_name().as_string().to_string())
        .unwrap_or_default();
    // Check if the file is .cts or .cjs (CommonJS-specific extensions)
    if file_name.ends_with(".cts") || file_name.ends_with(".cjs") {
        return &diag::ECMASCRIPT_IMPORTS_AND_EXPORTS_CANNOT_BE_WRITTEN_IN_A_COMMONJS_FILE_UNDER_VERBATIMMODULESYNTAX;
    }
    // For .ts, .tsx, .js, etc.
    &diag::ECMASCRIPT_IMPORTS_AND_EXPORTS_CANNOT_BE_WRITTEN_IN_A_COMMONJS_FILE_UNDER_VERBATIMMODULESYNTAX_ADJUST_THE_TYPE_FIELD_IN_THE_NEAREST_PACKAGE_JSON_TO_MAKE_THIS_FILE_AN_ECMASCRIPT_MODULE_OR_ADJUST_YOUR_VERBATIMMODULESYNTAX_MODULE_AND_MODULERESOLUTION_SETTINGS_IN_TYPESCRIPT
}

/// Go: `func (i ModuleKind) String() string` (modulekind_stringer_generated.go)
/// — the exact stringer table (None..ES2022 contiguous, ESNext..Node20,
/// NodeNext..Preserve).
pub(crate) fn module_kind_string(kind: ModuleKind) -> String {
    match kind {
        ModuleKind::None => "None",
        ModuleKind::CommonJS => "CommonJS",
        ModuleKind::AMD => "AMD",
        ModuleKind::UMD => "UMD",
        ModuleKind::System => "System",
        ModuleKind::ES2015 => "ES2015",
        ModuleKind::ES2020 => "ES2020",
        ModuleKind::ES2022 => "ES2022",
        ModuleKind::ESNext => "ESNext",
        ModuleKind::Node16 => "Node16",
        ModuleKind::Node18 => "Node18",
        ModuleKind::Node20 => "Node20",
        ModuleKind::NodeNext => "NodeNext",
        ModuleKind::Preserve => "Preserve",
    }
    .to_string()
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S3 batch 6 — the alias family: the REAL checkAliasSymbol body (Go
// checker.go:6937) + checkTypeAliasDeclaration (checker.go:7079) +
// the intrinsicTypeKinds table. Permissive seams (bannered inline):
//   * getMergedSymbol — S6 identity stub (typeofsymbol.rs).
//   * resolveAliasWithDeprecationCheck/isDeprecatedSymbol/addDeprecatedSuggestion
//     — the deprecation family is S6/S10; permissive.
//   * addTypeOnlyDeclarationRelatedInfo — the related-info elaboration seam.
//   * program.GetEmitModuleFormatOfFile / GetProjectReferenceFromOutputDts —
//     per-file program seams; approximated permissively.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type IntrinsicTypeKind int32` + `var intrinsicTypeKinds map[string]IntrinsicTypeKind`
/// (checker.go:2069 region).
fn intrinsic_type_kind_of(name: &str) -> Option<()> {
    match name {
        "Uppercase" | "Lowercase" | "Capitalize" | "Uncapitalize" | "NoInfer" => Some(()),
        _ => None,
    }
}

impl Checker {
    /// Go: `func (c *Checker) checkAliasSymbol(node)` (checker.go:6937) — the
    /// REAL body, replacing the S6 permissive no-op.
    pub(crate) fn check_alias_symbol(&mut self, node: NodeId) {
        let Some(initial_symbol) = self.get_symbol_of_declaration(node) else {
            return;
        };
        let target = self.resolve_alias(initial_symbol);
        if target == self.unknown_symbol() {
            return;
        }
        // For external modules, `symbol` represents the local symbol for an
        // alias. This local symbol will merge any other local declarations
        // (excluding other aliases) and symbol.flags will contain a combined
        // representation for all merged declarations. Based on symbol.flags
        // we can compute a set of excluded meanings (meanings that the
        // resolved alias should not have, otherwise it will conflict with
        // some local declaration). Note that in addition to normal flags we
        // include matching SymbolFlags.Export* in order to prevent
        // collisions with declarations that were exported from the current
        // module (they still contribute to local names).
        let merged = {
            let s = self.program_symbol(initial_symbol);
            s.export_symbol.unwrap_or(initial_symbol)
        };
        let symbol = self.get_merged_symbol(merged);
        let target_flags = self.get_symbol_flags(target);
        // A type-only import/export will already have a grammar error in a
        // JS file, so no need to issue more errors within.
        if tsc_ast::utilities::is_in_js_file(self.store().node(node))
            && !target_flags.intersects(SymbolFlags::VALUE)
            && !is_type_only_import_or_export_declaration(self.store(), node)
        {
            // Go: core.OrElse(node.PropertyNameOrName(), node)
            let error_node = property_name_or_name(self.store(), node).unwrap_or(node);
            if tsc_ast::is_export_specifier(self.store().node(node)) {
                let _diag = self.error(
                    error_node,
                    &diag::TYPES_CANNOT_APPEAR_IN_EXPORT_DECLARATIONS_IN_JAVASCRIPT_FILES,
                    &[],
                );
                // TODO(port M6 S6): the source-file Exports lookup for the
                // "automatically exported here" related info — rests until
                // S6's export-table resolution lands.
            } else {
                let identifier_text = if is_identifier(self.store().node(error_node)) {
                    tsc_ast::node_text(self.store(), error_node).to_string()
                } else {
                    self.program_symbol(symbol).name.clone()
                };
                let mut specifier_text = "...".to_string();
                if let Some(module_specifier) = try_get_module_specifier_from_declaration(
                    self.store(),
                    node,
                ) {
                    specifier_text = tsc_ast::node_text(self.store(), module_specifier).to_string();
                }
                let mut import_text = format!("import(\"{specifier_text}\")");
                if tsc_ast::is_import_specifier(self.store().node(node)) {
                    import_text = format!("{import_text}.{identifier_text}");
                }
                self.error(
                    error_node,
                    &diag::X_0_IS_A_TYPE_AND_CANNOT_BE_IMPORTED_IN_JAVASCRIPT_FILES_USE_1_IN_A_JSDOC_TYPE_ANNOTATION,
                    &[identifier_text, import_text],
                );
            }
            return;
        }
        let symbol_flags = self.program_symbol(symbol).flags;
        let excluded_meanings = {
            let mut meanings = SymbolFlags::NONE;
            if symbol_flags
                .intersects(SymbolFlags::VALUE.union(SymbolFlags::EXPORT_VALUE))
            {
                meanings = meanings.union(SymbolFlags::VALUE);
            }
            if symbol_flags.intersects(SymbolFlags::TYPE) {
                meanings = meanings.union(SymbolFlags::TYPE);
            }
            if symbol_flags.intersects(SymbolFlags::NAMESPACE) {
                meanings = meanings.union(SymbolFlags::NAMESPACE);
            }
            meanings
        };
        if target_flags.intersects(excluded_meanings) {
            let message = if tsc_ast::is_export_specifier(self.store().node(node)) {
                &diag::EXPORT_DECLARATION_CONFLICTS_WITH_EXPORTED_DECLARATION_OF_0
            } else {
                &diag::IMPORT_DECLARATION_CONFLICTS_WITH_LOCAL_DECLARATION_OF_0
            };
            let symbol_text = relater::symbol_to_string(self, symbol);
            self.error(node, message, &[symbol_text]);
        } else if !tsc_ast::is_export_specifier(self.store().node(node)) {
            // Look at 'compilerOptions.isolatedModules' and not
            // 'getIsolatedModules(...)' (which considers 'verbatimModuleSyntax')
            // here because 'verbatimModuleSyntax' will already have an error
            // for importing a type without 'import type'.
            let appears_valuey_to_transpiler = self.compiler_options.isolated_modules
                == Tristate::TSTrue
                && !is_contained_by_type_only_import_or_export(self.store(), node);
            if appears_valuey_to_transpiler
                && symbol_flags
                    .intersects(SymbolFlags::VALUE.union(SymbolFlags::EXPORT_VALUE))
            {
                let symbol_text = relater::symbol_to_string(self, symbol);
                let flag_name = self.get_isolated_modules_like_flag_name();
                self.error(
                    node,
                    &diag::IMPORT_0_CONFLICTS_WITH_LOCAL_VALUE_SO_MUST_BE_DECLARED_WITH_A_TYPE_ONLY_IMPORT_WHEN_ISOLATEDMODULES_IS_ENABLED,
                    &[symbol_text, flag_name],
                );
            }
        }
        if self.compiler_options.get_isolated_modules()
            && !is_type_only_import_or_export_declaration(self.store(), node)
            && !self.store().node(node).flags.intersects(NF::AMBIENT)
        {
            // TODO(port M6 S6): getTypeOnlyAliasDeclaration — the S6 stub
            // (None) until the typeOnlyDeclaration scan lands.
            let type_only_alias = self.get_type_only_alias_declaration(symbol);
            let is_type = !target_flags.intersects(SymbolFlags::VALUE);
            if is_type || type_only_alias.is_some() {
                match self.store().node(node).kind {
                    Kind::ImportClause | Kind::ImportSpecifier | Kind::ImportEqualsDeclaration => {
                        if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue {
                            let name = property_name_or_name(self.store(), node)
                                .expect("An ImportClause with a symbol should have a name");
                            let name_text = tsc_ast::node_text(self.store(), name).to_string();
                            let message =
                                if is_internal_module_import_equals_declaration(self.store(), node) {
                                    &diag::AN_IMPORT_ALIAS_CANNOT_RESOLVE_TO_A_TYPE_OR_TYPE_ONLY_DECLARATION_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED
                                } else if is_type {
                                    &diag::X_0_IS_A_TYPE_AND_MUST_BE_IMPORTED_USING_A_TYPE_ONLY_IMPORT_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED
                                } else {
                                    &diag::X_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION_AND_MUST_BE_IMPORTED_USING_A_TYPE_ONLY_IMPORT_WHEN_VERBATIMMODULESYNTAX_IS_ENABLED
                                };
                            // TODO(port M6 S?): addTypeOnlyDeclarationRelatedInfo
                            // — the related-info elaboration seam; base error only.
                            self.error(node, message, &[name_text]);
                        }
                        if is_type
                            && self.store().node(node).kind == Kind::ImportEqualsDeclaration
                            && crate::ast_shims::has_modifier(
                                self.store().node(node),
                                ModifierFlags::EXPORT,
                            )
                        {
                            let flag_name = self.get_isolated_modules_like_flag_name();
                            self.error(
                                node,
                                &diag::CANNOT_USE_EXPORT_IMPORT_ON_A_TYPE_OR_TYPE_ONLY_NAMESPACE_WHEN_0_IS_ENABLED,
                                &[flag_name],
                            );
                        }
                    }
                    Kind::ExportSpecifier => {
                        // Don't allow re-exporting an export that will be
                        // elided when `--isolatedModules` is set. The
                        // exception is that
                        // `import type { A } from './a'; export { A }` is
                        // allowed because single-file analysis can determine
                        // that the export should be dropped.
                        let different_source_file = type_only_alias.is_some_and(|alias| {
                            tsc_ast::utilities::get_source_file_of_node(self.store(), alias)
                                .map(|f| f.as_node_id())
                                != tsc_ast::utilities::get_source_file_of_node(self.store(), node)
                                    .map(|f| f.as_node_id())
                        });
                        if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                            || different_source_file
                        {
                            let name = property_name_or_name(self.store(), node)
                                .unwrap_or(node);
                            let name_text =
                                tsc_ast::node_text(self.store(), name).to_string();
                            let flag_name = self.get_isolated_modules_like_flag_name();
                            // TODO(port M6 S?): addTypeOnlyDeclarationRelatedInfo
                            // — the related-info elaboration seam; base error only.
                            if is_type {
                                self.error(
                                    node,
                                    &diag::RE_EXPORTING_A_TYPE_WHEN_0_IS_ENABLED_REQUIRES_USING_EXPORT_TYPE,
                                    &[flag_name],
                                );
                            } else {
                                self.error(
                                    node,
                                    &diag::X_0_RESOLVES_TO_A_TYPE_ONLY_DECLARATION_AND_MUST_BE_RE_EXPORTED_USING_A_TYPE_ONLY_RE_EXPORT_WHEN_1_IS_ENABLED,
                                    &[name_text, flag_name],
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                && !tsc_ast::is_import_equals_declaration(self.store().node(node))
                && !tsc_ast::utilities::is_in_js_file(self.store().node(node))
                && self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS
            {
                self.error(
                    node,
                    get_verbatim_module_syntax_error_message(self.store(), node),
                    &[],
                );
            } else if self.module_kind == ModuleKind::Preserve
                && !tsc_ast::is_import_equals_declaration(self.store().node(node))
                && self.store().node(node).kind != Kind::VariableDeclaration
                && self.store().node(node).kind != Kind::BindingElement
                && self.get_emit_module_format_of_file_seam(node) == ModuleKind::CommonJS
            {
                // In `--module preserve`, ESM input syntax emits ESM output
                // syntax, but there will be times when we look at the
                // `impliedNodeFormat` of this file and decide it's CommonJS
                // (i.e., currently, only if the file extension is .cjs/.cts).
                // To avoid that inconsistency, we disallow ESM syntax in
                // files that are unambiguously CommonJS in this mode.
                self.error(
                    node,
                    &diag::ECMASCRIPT_MODULE_SYNTAX_IS_NOT_ALLOWED_IN_A_COMMONJS_MODULE_WHEN_MODULE_IS_SET_TO_PRESERVE,
                    &[],
                );
            }
            if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue
                && !is_type_only_import_or_export_declaration(self.store(), node)
                && !self.store().node(node).flags.intersects(NF::AMBIENT)
                && target_flags.intersects(SymbolFlags::CONST_ENUM)
            {
                let const_enum_declaration =
                    self.program_symbol(target).value_declaration;
                if let Some(const_enum_declaration) = const_enum_declaration {
                    if self
                        .store()
                        .node(const_enum_declaration)
                        .flags
                        .intersects(NF::AMBIENT)
                    {
                        // TODO(port M6 S?): program.GetProjectReferenceFromOutputDts
                        // — the project-reference redirect seam; permissive
                        // None (Go's nil redirect keeps the error path).
                        let should_preserve = self.compiler_options.should_preserve_const_enums();
                        if !should_preserve {
                            let flag_name = self.get_isolated_modules_like_flag_name();
                            self.error(
                                node,
                                &diag::CANNOT_ACCESS_AMBIENT_CONST_ENUMS_WHEN_0_IS_ENABLED,
                                &[flag_name],
                            );
                        }
                    }
                }
            }
        }
        if tsc_ast::is_import_specifier(self.store().node(node)) {
            // TODO(port M6 S6/S10): resolveAliasWithDeprecationCheck +
            // isDeprecatedSymbol + addDeprecatedSuggestion — the deprecation
            // family; permissive.
        }
    }

    /// Go: `func (c *Checker) checkTypeAliasDeclaration(node)` (checker.go:7079).
    pub(crate) fn check_type_alias_declaration(&mut self, node: NodeId) {
        // Grammar checking
        self.check_grammar_modifiers(node);
        let (name, type_parameters, type_node) = {
            let d = self
                .store()
                .node(node)
                .as_type_alias_declaration()
                .expect("TypeAliasDeclaration data");
            (d.name, d.type_parameters.clone(), d.type_)
        };
        let name_text = tsc_ast::node_text(self.store(), name).to_string();
        self.check_type_name_is_reserved(name, &diag::TYPE_ALIAS_NAME_CANNOT_BE_0);
        let parent = self.store().node(node).parent.get();
        if !container_allows_block_scoped_variable(self.store(), parent) {
            self.grammar_error_on_node(
                node,
                &diag::X_0_DECLARATIONS_CAN_ONLY_BE_DECLARED_INSIDE_A_BLOCK,
                &["type".to_string()],
            );
        }
        self.check_exports_on_merged_declarations(node);

        let type_parameters = type_parameters
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
        self.check_type_parameters(&type_parameters);
        if type_node != NodeId::NONE && self.store().node(type_node).kind == Kind::IntrinsicKeyword
        {
            let is_builtin_iterator_return =
                type_parameters.is_empty() && name_text == "BuiltinIteratorReturn";
            let is_intrinsic_type_kind =
                type_parameters.len() == 1 && intrinsic_type_kind_of(&name_text).is_some();
            if !is_builtin_iterator_return && !is_intrinsic_type_kind {
                self.error(
                    type_node,
                    &diag::THE_INTRINSIC_KEYWORD_CAN_ONLY_BE_USED_TO_DECLARE_COMPILER_PROVIDED_INTRINSIC_TYPES,
                    &[],
                );
            }
            // The `intrinsic` keyword is a leaf type node with no child nodes
            // to check, so skipping the checkSourceElement below visits
            // nothing.
            return;
        }
        self.check_source_element(Some(type_node));
        self.register_for_unused_identifiers_check(node);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers for the alias batch.
// ────────────────────────────────────────────────────────────────────────────

/// Go: `ast.IsTypeOnlyImportOrExportDeclaration(node)` — reads the type-only
/// markers the parser laid down on the import/export payloads.
pub(crate) fn is_type_only_import_or_export_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    match store.node(node).kind {
        Kind::ImportDeclaration => store
            .node(node)
            .as_import_declaration()
            .and_then(|d| d.import_clause)
            .is_some_and(|clause| {
                store
                    .node(clause)
                    .as_import_clause()
                    .map(|d| d.phase_modifier == Some(Kind::TypeKeyword))
                    .unwrap_or(false)
            }),
        Kind::ImportEqualsDeclaration => store
            .node(node)
            .as_import_equals_declaration()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ExportDeclaration => store
            .node(node)
            .as_export_declaration()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ImportSpecifier => store
            .node(node)
            .as_import_specifier()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ExportSpecifier => store
            .node(node)
            .as_export_specifier()
            .map(|d| d.is_type_only)
            .unwrap_or(false),
        Kind::ImportClause => store
            .node(node)
            .as_import_clause()
            .map(|d| d.phase_modifier == Some(Kind::TypeKeyword))
            .unwrap_or(false),
        _ => false,
    }
}

/// Go: `ast.FindAncestor(node, ast.IsTypeOnlyImportOrExportDeclaration) != nil`
/// — walks the parent chain for a type-only import/export.
fn is_contained_by_type_only_import_or_export(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut current = node;
    loop {
        if is_type_only_import_or_export_declaration(store, current) {
            return true;
        }
        let parent = store.node(current).parent.get();
        if parent == NodeId::NONE {
            return false;
        }
        current = parent;
    }
}

/// Go: `node.PropertyNameOrName()` — ImportSpecifier/ExportSpecifier and the
/// name-bearing declarations.
fn property_name_or_name(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match store.node(node).kind {
        Kind::ImportSpecifier => store
            .node(node)
            .as_import_specifier()
            .map(|d| d.property_name.unwrap_or(d.name)),
        Kind::ExportSpecifier => store
            .node(node)
            .as_export_specifier()
            .map(|d| d.property_name.unwrap_or(d.name)),
        _ => store.node(node).name(),
    }
}

/// Go: `TryGetModuleSpecifierFromDeclaration(declaration)` (utilities.go) —
/// the module specifier of an import/`import=`/export declaration.
fn try_get_module_specifier_from_declaration(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    get_external_module_name(store, node)
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S3 batch 6b — the deferred function-expression/object-literal-method
// check (Go checker.go:10437, dispatched from checkDeferredNode at 2560).
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkFunctionExpressionOrObjectLiteralMethodDeferred(node)`
    /// (checker.go:10437).
    pub(crate) fn check_function_expression_or_object_literal_method_deferred(
        &mut self,
        node: NodeId,
    ) {
        let function_flags = tsc_ast::functionflags::get_function_flags(self.store(), node);
        let return_type = self.get_return_type_from_annotation(node);
        self.check_all_code_paths_in_non_void_function_return_or_throw(node, return_type);
        let body = function_body_of(self.store(), node);
        if let Some(body) = body {
            let type_node = signature_type_node(self.store(), node);
            if type_node.is_none() {
                // There are some checks that are only performed in
                // getReturnTypeFromBody, that may produce errors we need.
                // An example is the noImplicitAny errors resulting from
                // widening the return expression of a function. Because
                // checking of function expression bodies is deferred, there
                // was never an appropriate time to do this during the main
                // walk of the file (see the comment at the top of
                // checkFunctionExpressionBodies). So it must be done now.
                // TODO(port M6 S2/S8): getReturnTypeOfSignature(
                // getSignatureFromDeclaration(node)) — the deferred
                // signature-resolution seam (getSignatureFromDeclaration is
                // the S8 permissive None); rests until S2/S8 land.
            }
            if self.store().node(body).kind == Kind::Block {
                self.check_source_element(Some(body));
            } else {
                // From within an async function you can return either a
                // non-promise value or a promise. Any Promise/A+ compatible
                // implementation will always assimilate any foreign promise,
                // so we should not be checking assignability of a promise to
                // the return type. Instead, we need to check assignability of
                // the awaited type of the expression body against the promised
                // type of its return type annotation.
                let expr_type = self.check_expression(body);
                if let Some(return_type) = return_type {
                    let return_or_promised_type =
                        self.unwrap_return_type(return_type, function_flags);
                    if let Some(return_or_promised_type) = return_or_promised_type {
                        self.check_return_expression(
                            node,
                            return_or_promised_type,
                            body,
                            Some(body),
                            expr_type,
                            false,
                        );
                    }
                }
            }
        }
    }
}
