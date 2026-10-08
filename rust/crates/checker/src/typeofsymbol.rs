// checker.go — the M6 S2 slice: the getTypeOfSymbol SPINE + declared types +
// the circularity machinery + binding-element typing + the widen* helpers.
// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763,
// lines 16749–19300 (the spine, the keyBuilder key family, the constraint
// family, binding patterns, widening, accessors/enums/aliases, the
// circularity stack's callers), 24194–24466 (the getDeclaredTypeOf* family),
// 24628–24700 (getDeclaredTypeOfAlias), and getTypeOfMappedSymbol (21388).
//
// PORT (file map): SPEC §4.2 maps checker.go → checker.rs; the spike carried
// only the literal-inference slice there and S1 landed in spine.rs — the S2
// slice lives here as `typeofsymbol.rs` (the one-.rs-per-Go-file convention
// keeps checker.go's named-module split: spine.rs for S1, this for S2).
// Every function keeps its Go name snake_cased; the S1-era bodies in
// checker.rs that this slice supersedes were removed there (see git history).
//
// Slice-table banner map (M6-DISPATCH §2) — every not-yet-ported callee this
// slice routes to, with the slice that owns the real body:
//   * getUnionType / getIntersectionType / createArrayType / createTupleTypeEx
//     / createTypeReference / mapType → S7's permissive stubs (relater.rs
//     carries the identity/error seam; the S2↔S7 cycle is deliberate, one
//     stub direction per landing order).
//   * getTypeFromTypeNode → S5 (checker.rs's error-type stub).
//   * getFlowTypeInConstructor / getFlowTypeInStaticBlocks / the synthetic
//     element-access factory → W3 (flow.go) — permissive None/identity.
//   * checkPropertyAssignment / checkShorthandPropertyAssignment /
//     checkObjectLiteralMethod / checkJsxAttribute / the assignment-declaration
//     JS paths → S4/W5 (permissive error-type arms, Go's structure kept).
//   * resolveAlias / resolveEntityName / getMergedSymbol /
//     getExportSymbolOfValueSymbolIfExported / getTargetOfAliasDeclaration →
//     S6 stubs (permissive: aliases don't resolve yet — see resolve_alias).
//   * getSignatureFromDeclaration / getSignaturesOfType / getReturnTypeOfSignature
//     / getPropertiesOfObjectType → S8 (relater/exports stubs).
//   * getResolvedBaseConstraint / getNextBaseConstraint → S9 (the permissive
//     stub treats every base constraint as non-circular; the resolution stack
//     this slice owns is what S9 re-enters through).
//   * computeEnumMemberValues' constant-expression evaluation → the
//     tsc-evaluator crate port (not yet started) — permissive: every member
//     value evaluates to "not a constant", so enums get computed enum types
//     until that wave lands (getDeclaredTypeOfEnum itself is REAL).
//
// ── The circularity machinery (M6-DISPATCH risk 1 — S2 owns it) ──
//
// pushTypeResolution/popTypeResolution/findResolutionCycleStartIndex/
// typeResolutionHasProperty live in checker.rs (they landed with the M5 spike
// and are LIVE — S1's expression spine pushes through them); this slice is
// their owning wave per the dispatch table and every new call site lands
// here: getDeclaredTypeOfTypeAlias (DeclaredType), getDeclaredTypeOf* via
// the class base-constructor resolution (ResolvedBaseConstructorType),
// getTypeOfAccessors/getWriteTypeOfAccessors (Type/WriteType),
// getTypeOfAlias (Type), getTypeOfMappedSymbol (Type). The cache keys this
// slice owns (keyBuilder — checker.go 17770–18016) live below; the builder
// itself is relater.rs's (landed W2 with the relation cache as its
// load-bearing consumer); the FNV-1a PORT note there covers this slice's
// keys too (never serialized, map identity only).
//
// ── getWidenedTypeWithContext (the S9 stub-direction call) ──
//
// Landed HERE (its body is in this slice's range, checker.go 18838–18943):
// the any/nullable arm is real (drives `let y = undefined` → any under
// !strictNullChecks), the union/intersection/array arms route through the
// S7 permissive constructors, and the object-literal arm runs against the
// S8 member-resolution stubs (unreachable at this depth — object literal
// types first appear with S4's checkObjectLiteral). The WideningContext
// tree is arena-shaped (see the struct's PORT note). S9's wave keeps the
// body and completes the callee surface.

use std::collections::hash_map::Entry;

use rustc_hash::{FxHashMap, FxHashSet};

use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    CheckFlags, Kind, ModifierFlags, Node, NodeData, NodeId, NodeStore, Symbol, SymbolFlags,
    SymbolId, SymbolTable,
};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::{
    AN_ENUM_MEMBER_CANNOT_HAVE_A_NUMERIC_NAME, BINDING_ELEMENT_0_IMPLICITLY_HAS_AN_1_TYPE,
    CIRCULAR_DEFINITION_OF_IMPORT_ALIAS_0, COMPUTED_PROPERTY_NAMES_ARE_NOT_ALLOWED_IN_ENUMS,
    DID_YOU_MEAN_FOR_0_TO_BE_CONSTRAINED_TO_TYPE_NEW_ARGS_COLON_ANY_1,
    ENUM_MEMBER_FOLLOWING_A_NON_LITERAL_NUMERIC_MEMBER_MUST_HAVE_AN_INITIALIZER_WHEN_ISOLATEDMODULES_IS_ENABLED,
    ENUM_MEMBER_MUST_HAVE_INITIALIZER,
    FUNCTION_EXPRESSION_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_0_RETURN_TYPE,
    GENERATOR_IMPLICITLY_HAS_YIELD_TYPE_0_CONSIDER_SUPPLYING_A_RETURN_TYPE_ANNOTATION,
    IN_AMBIENT_ENUM_DECLARATIONS_MEMBER_INITIALIZER_MUST_BE_CONSTANT_EXPRESSION,
    MAPPED_OBJECT_TYPE_IMPLICITLY_HAS_AN_ANY_TEMPLATE_TYPE,
    MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE,
    MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE,
    PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    PARAMETER_HAS_A_NAME_BUT_NO_TYPE_DID_YOU_MEAN_0_COLON_1,
    PROPERTY_0_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_ITS_GET_ACCESSOR_LACKS_A_RETURN_TYPE_ANNOTATION,
    PROPERTY_0_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_ITS_SET_ACCESSOR_LACKS_A_PARAMETER_TYPE_ANNOTATION,
    REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE,
    REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    REST_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES,
    THIS_OVERLOAD_IMPLICITLY_RETURNS_THE_TYPE_0_BECAUSE_IT_LACKS_A_RETURN_TYPE_ANNOTATION,
    TYPE_0_IS_NOT_A_CONSTRUCTOR_FUNCTION_TYPE,
    TYPE_ALIAS_0_CIRCULARLY_REFERENCES_ITSELF,
    TYPE_OF_PROPERTY_0_CIRCULARLY_REFERENCES_ITSELF_IN_MAPPED_TYPE_1,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    X_0_HAS_A_STRING_TYPE_BUT_MUST_HAVE_SYNTACTICALLY_RECOGNIZABLE_STRING_SYNTAX_WHEN_ISOLATEDMODULES_IS_ENABLED,
    X_0_IMPLICITLY_HAS_AN_1_RETURN_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    X_0_IMPLICITLY_HAS_RETURN_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_RETURN_TYPE_ANNOTATION_AND_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ONE_OF_ITS_RETURN_EXPRESSIONS,
    X_0_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_IT_DOES_NOT_HAVE_A_TYPE_ANNOTATION_AND_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_INITIALIZER,
    X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_BASE_EXPRESSION,
    X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_TYPE_ANNOTATION,
    X_0_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_1_RETURN_TYPE,
    X_0_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_1_YIELD_TYPE,
    X_CONST_ENUM_MEMBER_INITIALIZER_WAS_EVALUATED_TO_A_NON_FINITE_VALUE,
    X_CONST_ENUM_MEMBER_INITIALIZER_WAS_EVALUATED_TO_DISALLOWED_VALUE_NAN,
    X_CONST_ENUM_MEMBER_INITIALIZERS_MUST_BE_CONSTANT_EXPRESSIONS,
};

use tsc_binder::ast_shims as binder_shims;

use crate::ast_shims;
use crate::checker::{CheckMode, TypeSystemEntity, TypeSystemPropertyName};
use crate::todo_shapes::{CacheHashKey, EvaluatorResult, TypeMapperId, UnionReduction};
use crate::types::{
    self, AccessFlags, ElementFlags, IndexInfo, IntersectionFlags, IntersectionType,
    LiteralValue, MappedType, ObjectFlags, SignatureFlags, SignatureId, TupleElementInfo,
    TypeAlias, TypeData, TypeFlags, TypeId, TypeParameter, UnionType,
};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// keyBuilder's free key functions (Go checker.go 17899–18016 — S2 owns them;
// the builder struct is relater.rs's, landed W2 for the relation cache)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getTypeListKey(types []*Type) CacheHashKey`.
pub(crate) fn get_type_list_key(types: &[TypeId]) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(types);
    b.hash()
}

/// Go: `func getAliasKey(alias *TypeAlias) CacheHashKey`.
pub(crate) fn get_alias_key(alias: Option<&TypeAlias>) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_alias(alias);
    b.hash()
}

/// Go: `func getUnionKey(types []*Type, origin *Type, alias *TypeAlias) CacheHashKey`.
pub(crate) fn get_union_key(
    types: &[TypeId],
    origin: Option<TypeId>,
    alias: Option<&TypeAlias>,
    c: &Checker,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    match origin {
        None => b.write_types(types),
        Some(origin) => {
            let flags = c.type_(origin).flags;
            if flags.intersects(TypeFlags::UNION) {
                b.write_byte(b'|');
                b.write_types(c.type_(origin).types());
            } else if flags.intersects(TypeFlags::INTERSECTION) {
                b.write_byte(b'&');
                b.write_types(c.type_(origin).types());
            } else if flags.intersects(TypeFlags::INDEX) {
                // origin type id alone is insufficient, as `keyof x` may
                // resolve to multiple WIP values while `x` is still resolving
                b.write_byte(b'#');
                b.write_type(origin);
                b.write_byte(b'|');
                b.write_types(types);
            } else {
                panic!("Unhandled case in getUnionKey");
            }
        }
    }
    b.write_alias(alias);
    b.hash()
}

/// Go: `func getIntersectionKey(types []*Type, flags IntersectionFlags, alias *TypeAlias) CacheHashKey`.
pub(crate) fn get_intersection_key(
    types: &[TypeId],
    flags: IntersectionFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(types);
    if !flags.intersects(IntersectionFlags::NO_CONSTRAINT_REDUCTION) {
        b.write_alias(alias);
    } else {
        b.write_byte(b'*');
    }
    b.hash()
}

/// Go: `func getTupleKey(elementInfos []TupleElementInfo, readonly bool) CacheHashKey`.
pub(crate) fn get_tuple_key(element_infos: &[TupleElementInfo], readonly: bool) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    for e in element_infos {
        if e.flags.intersects(ElementFlags::REQUIRED) {
            b.write_byte(b'#');
        } else if e.flags.intersects(ElementFlags::OPTIONAL) {
            b.write_byte(b'?');
        } else if e.flags.intersects(ElementFlags::REST) {
            b.write_byte(b'.');
        } else {
            b.write_byte(b'*');
        }
        if let Some(labeled_declaration) = e.labeled_declaration {
            b.write_node_id(labeled_declaration);
        }
    }
    if readonly {
        b.write_byte(b'!');
    }
    b.hash()
}

/// Go: `func getTypeAliasInstantiationKey(typeArguments []*Type, alias *TypeAlias) CacheHashKey`
/// — `getTypeInstantiationKey(typeArguments, alias, false)`.
pub(crate) fn get_type_alias_instantiation_key(
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    get_type_instantiation_key(type_arguments, alias, false)
}

/// Go: `func getTypeInstantiationKey(typeArguments []*Type, alias *TypeAlias, singleSignature bool) CacheHashKey`.
pub(crate) fn get_type_instantiation_key(
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
    single_signature: bool,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(type_arguments);
    b.write_alias(alias);
    if single_signature {
        b.write_byte(b'!');
    }
    b.hash()
}

/// Go: `func getIndexedAccessKey(objectType *Type, indexType *Type, accessFlags AccessFlags, alias *TypeAlias) CacheHashKey`.
pub(crate) fn get_indexed_access_key(
    object_type: TypeId,
    index_type: TypeId,
    access_flags: AccessFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_type(object_type);
    b.write_type(index_type);
    b.write_uint32(access_flags.0);
    b.write_alias(alias);
    b.hash()
}

/// Go: `func getTemplateTypeKey(texts []string, types []*Type) CacheHashKey`.
pub(crate) fn get_template_type_key(texts: &[String], types: &[TypeId]) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(types);
    b.write_byte(b'|');
    for s in texts {
        b.write_int(s.len() as i64);
    }
    b.write_byte(b'|');
    for s in texts {
        b.write_string(s);
    }
    b.hash()
}

/// Go: `func getConditionalTypeKey(typeArguments []*Type, alias *TypeAlias, forConstraint bool) CacheHashKey`.
pub(crate) fn get_conditional_type_key(
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
    for_constraint: bool,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(type_arguments);
    b.write_alias(alias);
    if for_constraint {
        b.write_byte(b'!');
    }
    b.hash()
}

/// Go: `func getNodeListKey(nodes []*ast.Node) CacheHashKey`.
#[allow(dead_code)] // the S7/S10 instantiation waves consume it
pub(crate) fn get_node_list_key(nodes: &[NodeId]) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_int(nodes.len() as i64);
    for &n in nodes {
        b.write_node_id(n);
    }
    b.hash()
}

/// Go: `func isUnconstrainedTypeParameter(tp *Type) bool` — true when the type
/// parameter originates in an unconstrained declaration in a type parameter list.
pub(crate) fn is_unconstrained_type_parameter(c: &Checker, tp: TypeId) -> bool {
    let target = c.type_(tp).target().unwrap_or(tp);
    let Some(symbol) = c.type_(target).symbol else {
        return false;
    };
    let store = c.store();
    let declarations = c.program.symbol(symbol).declarations.clone();
    for d in declarations {
        let node = store.node(d);
        if node.kind == Kind::TypeParameter {
            let parent = node.parent.get();
            if ast_shims::type_of_node(store, d).is_some()
                || store.node(parent).kind == Kind::MappedType
                || store.node(parent).kind == Kind::InferType
            {
                return false;
            }
        }
    }
    true
}

// ────────────────────────────────────────────────────────────────────────────
// thisAssignmentDeclarationKind (Go checker.go 18450)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type thisAssignmentDeclarationKind int32` (iota order preserved).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum ThisAssignmentDeclarationKind {
    /// not (all) this.property assignments
    None = 0,
    /// typed; use the type annotation
    Typed = 1,
    /// at least one in the constructor; use control flow
    Constructor = 2,
    /// methods only; look in base first, and if not found, union all
    /// declaration types plus undefined
    Method = 3,
}

/// Go: `type EnumLiteralKey struct` (checker.go's enum-literal intern key —
/// `enumLiteralTypes`/`enumNaNLiteralTypes`' key; the value side mirrors Go's
/// `any` (jsnum.Number | string)).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EnumLiteralKey {
    pub enum_symbol: SymbolId,
    pub value: LiteralValue,
}

// ────────────────────────────────────────────────────────────────────────────
// WideningContext (Go checker.go 18885 — the context tree)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type WideningContext struct { parent *WideningContext; propertyName
/// string; siblings []*Type; resolvedProperties []*ast.Symbol; widenedTypes
/// map[*Type]*Type; childContexts map[string]*WideningContext }`.
///
/// PORT: Go's tree of heap-allocated contexts becomes an arena the caller
/// owns (`Vec<WideningContext>` + index); `parent` is the arena index. The
/// lazily-populated fields become `Option`/map exactly as Go's nil-checks
/// read them. The tree shape (one child per property name, memoized) and
/// the root-context "never widens a particular object literal more than
/// once" rule (widenedTypes is only cached in CHILD contexts) are Go's.
#[derive(Default)]
pub(crate) struct WideningContext {
    pub parent: Option<usize>,
    pub property_name: String,
    pub siblings: Option<Vec<TypeId>>,
    pub resolved_properties: Option<Vec<SymbolId>>,
    pub widened_types: Option<FxHashMap<TypeId, TypeId>>,
    pub child_contexts: FxHashMap<String, usize>,
}

impl Checker {
    /// Go: `func (w *WideningContext) getChildContext(propertyName string) *WideningContext`
    /// — the port appends to the arena and hands back the index.
    fn get_child_context(
        contexts: &mut Vec<WideningContext>,
        index: usize,
        property_name: &str,
    ) -> usize {
        if let Some(&cached) = contexts[index].child_contexts.get(property_name) {
            return cached;
        }
        let result = contexts.len();
        contexts.push(WideningContext {
            parent: Some(index),
            property_name: property_name.to_string(),
            ..WideningContext::default()
        });
        contexts[index]
            .child_contexts
            .insert(property_name.to_string(), result);
        result
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The getTypeOfSymbol spine (Go checker.go 16749–17005, 17033–17432)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getDeclarationOfAliasSymbol(symbol *ast.Symbol) *ast.Node`
        &mut self,
        mut symbol: SymbolId,
        exclude_type_only_meanings: bool,
        exclude_local_meanings: bool,
    ) -> SymbolFlags {
        let mut seen_symbols: FxHashSet<SymbolId> = FxHashSet::default();
        let mut flags = if exclude_local_meanings {
            SymbolFlags::NONE
        } else {
            self.program.symbol(symbol).flags
        };
        while self.program.symbol(symbol).flags.intersects(SymbolFlags::ALIAS) {
            if exclude_type_only_meanings && self.get_type_only_alias_declaration(symbol).is_some()
            {
                break;
            }
            let target =
                self.get_export_symbol_of_value_symbol_if_exported(self.resolve_alias(symbol));
            if target == self.unknown_symbol() {
                return SymbolFlags::ALL;
            }
            if self.program.symbol(target).flags.intersects(SymbolFlags::ALIAS) {
                // Optimization - try to avoid creating or adding to
                // `seenSymbols` if possible
                if target == symbol || seen_symbols.contains(&target) {
                    break;
                }
                if seen_symbols.is_empty() {
                    seen_symbols.insert(symbol);
                }
                seen_symbols.insert(target);
            }
            flags |= self.program.symbol(target).flags;
            symbol = target;
        }
        flags
    }

    /// Go: `func (c *Checker) getDeclarationOfAliasSymbol(symbol *ast.Symbol) *ast.Node`
    /// — `core.FindLast(symbol.Declarations, ast.IsAliasSymbolDeclaration)`.
    pub(crate) fn get_declaration_of_alias_symbol(&self, symbol: SymbolId) -> Option<NodeId> {
        self.program
            .symbol(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|&d| binder_shims::is_alias_symbol_declaration(self.store().node(d)))
            .next_back()
    }

    /// Go: `func (c *Checker) getTypeOfSymbolWithDeferredType(symbol *ast.Symbol) *Type`.
    fn get_type_of_symbol_with_deferred_type(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let (parent_flags, constituents) = {
                let deferred = self.deferred_symbol_links.get(symbol);
                (
                    deferred.parent.map(|p| self.type_(p).flags).unwrap_or(TypeFlags::NONE),
                    deferred.constituents.clone(),
                )
            };
            let resolved = if parent_flags.intersects(TypeFlags::UNION) {
                crate::relater::get_union_type(self, constituents)
            } else {
                crate::relater::get_intersection_type(self, constituents)
            };
            self.value_symbol_links.get(symbol).resolved_type = Some(resolved);
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) getWriteTypeOfSymbolWithDeferredType(symbol *ast.Symbol) *Type`.
    fn get_write_type_of_symbol_with_deferred_type(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).write_type.is_none() {
            let (parent_flags, write_constituents) = {
                let deferred = self.deferred_symbol_links.get(symbol);
                (
                    deferred.parent.map(|p| self.type_(p).flags).unwrap_or(TypeFlags::NONE),
                    deferred.write_constituents.clone(),
                )
            };
            let write_type = if !write_constituents.is_empty() {
                if parent_flags.intersects(TypeFlags::UNION) {
                    crate::relater::get_union_type(self, write_constituents)
                } else {
                    crate::relater::get_intersection_type(self, write_constituents)
                }
            } else {
                self.get_type_of_symbol_with_deferred_type(symbol)
            };
            self.value_symbol_links.get(symbol).write_type = Some(write_type);
        }
        self.value_symbol_links.get(symbol).write_type.expect("just set")
    }

    /// Distinct write types come only from set accessors, but synthetic union
    /// and intersection properties deriving from set accessors will either
    /// pre-compute or defer the union or intersection of the writeTypes of
    /// their constituents.
    ///
    /// Go: `func (c *Checker) getWriteTypeOfSymbol(symbol *ast.Symbol) *Type`.
    pub fn get_write_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let (check_flags, flags) = {
            let s = self.program.symbol(symbol);
            (s.check_flags, s.flags)
        };
        if check_flags.intersects(CheckFlags::SYNTHETIC_PROPERTY) {
            if check_flags.intersects(CheckFlags::DEFERRED_TYPE) {
                return self.get_write_type_of_symbol_with_deferred_type(symbol);
            }
            let (write_type, resolved_type) = {
                let links = self.value_symbol_links.get(symbol);
                (links.write_type, links.resolved_type)
            };
            return write_type.or(resolved_type).unwrap_or_else(|| self.error_type());
        }
        if flags.intersects(SymbolFlags::PROPERTY) {
            let optional = flags.intersects(SymbolFlags::OPTIONAL);
            let t = self.get_type_of_symbol(symbol);
            return self.remove_missing_type(t, optional);
        }
        if flags.intersects(SymbolFlags::ACCESSOR) {
            if check_flags.intersects(CheckFlags::INSTANTIATED) {
                return self.get_write_type_of_instantiated_symbol(symbol);
            }
            return self.get_write_type_of_accessors(symbol);
        }
        self.get_type_of_symbol(symbol)
    }

    /// Go: `func (c *Checker) GetTypeOfSymbolAtLocation(symbol *ast.Symbol, location *ast.Node) *Type`.
    pub fn get_type_of_symbol_at_location(
        &mut self,
        symbol: SymbolId,
        location: Option<NodeId>,
    ) -> TypeId {
        let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol);
        if let Some(mut location) = location {
            // If we have an identifier or a property access at the given
            // location, if the location is an dotted name expression, and if
            // the location is not an assignment target, obtain the type of
            // the expression (which will reflect control flow analysis). If
            // the expression indeed resolved to the given symbol, return the
            // narrowed type.
            let node = self.store().node(location);
            let parent = node.parent.get();
            let parent_kind = self.store().node(parent).kind;
            if (tsc_ast::is_identifier(node) || tsc_ast::is_private_identifier(node))
                && !(is_jsx_tag_name(self.store(), location)
                    || parent_kind == Kind::JsxAttribute
                    || parent_kind == Kind::JsxNamespacedName)
            {
                if is_right_side_of_qualified_name_or_property_access(self.store(), location) {
                    location = parent;
                }
                // TODO(port M6 W3): the write-access arms (Go's
                // `IsWriteAccess(location) && PropertyAccessExpression` →
                // checkPropertyAccessExpression writeOnly; the port treats
                // write accesses like reads until the flow wave lands).
                if !binder_shims::is_assignment_target(self.store(), location) {
                    let t = self.get_type_of_expression(location);
                    let resolved_symbol = self.symbol_node_links.get(location).resolved_symbol;
                    if resolved_symbol.is_some_and(|s| {
                        self.get_export_symbol_of_value_symbol_if_exported(s) == symbol
                    }) {
                        // TODO(port M6 S7): removeOptionalTypeMarker — the
                        // union-marker removal; identity until then.
                        return t;
                    }
                }
            }
            let location_node = self.store().node(location);
            let parent = location_node.parent.get();
            if is_declaration_name(self.store(), location)
                && self.store().node(parent).kind == Kind::SetAccessor
                && self.get_annotated_accessor_type_node(Some(parent)).is_some()
            {
                let accessor_symbol = payload_symbol(self.store().node(parent))
                    .expect("set accessor declaration symbol");
                return self.get_write_type_of_accessors(accessor_symbol);
            }
            // The location isn't a reference to the given symbol, meaning
            // we're being asked a hypothetical question of what type the
            // symbol would have if there was a reference to it at the given
            // location. Since we have no control flow information for the
            // hypothetical reference (control flow information is created and
            // attached by the binder), we simply return the declared type of
            // the symbol.
            if u::is_right_side_of_access_expression(self.store(), location) {
                // TODO(port M6 W3): Go's `ast.IsWriteAccess(location.Parent)`
                // gate → getWriteTypeOfSymbol; permissive: falls through to
                // the declared type (the read answer).
                let _ = parent;
            }
        }
        self.get_non_missing_type_of_symbol(symbol)
    }

    /// Go: `func (c *Checker) getTypeOfSymbol(symbol *ast.Symbol) *Type`.
    pub fn get_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let (check_flags, flags) = {
            let s = self.program.symbol(symbol);
            (s.check_flags, s.flags)
        };
        if check_flags.intersects(CheckFlags::DEFERRED_TYPE) {
            return self.get_type_of_symbol_with_deferred_type(symbol);
        }
        if check_flags.intersects(CheckFlags::INSTANTIATED) {
            return self.get_type_of_instantiated_symbol(symbol);
        }
        if check_flags.intersects(CheckFlags::MAPPED) {
            return self.get_type_of_mapped_symbol(symbol);
        }
        if check_flags.intersects(CheckFlags::REVERSE_MAPPED) {
            return self.get_type_of_reverse_mapped_symbol(symbol);
        }
        if flags.intersects(SymbolFlags::ACCESSOR) {
            return self.get_type_of_accessors(symbol);
        }
        if flags.intersects(SymbolFlags::VARIABLE.union(SymbolFlags::PROPERTY)) {
            return self.get_type_of_variable_or_parameter_or_property(symbol);
        }
        if flags.intersects(
            SymbolFlags::FUNCTION
                .union(SymbolFlags::METHOD)
                .union(SymbolFlags::CLASS)
                .union(SymbolFlags::ENUM)
                .union(SymbolFlags::VALUE_MODULE),
        ) {
            return self.get_type_of_func_class_enum_module(symbol);
        }
        if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            return self.get_type_of_enum_member(symbol);
        }
        if flags.intersects(SymbolFlags::ALIAS) {
            return self.get_type_of_alias(symbol);
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getNonMissingTypeOfSymbol(symbol *ast.Symbol) *Type`.
    pub fn get_non_missing_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let optional = self.program.symbol(symbol).flags.intersects(SymbolFlags::OPTIONAL);
        let t = self.get_type_of_symbol(symbol);
        self.remove_missing_type(t, optional)
    }

    /// Go: `func (c *Checker) getTypeOfInstantiatedSymbol(symbol *ast.Symbol) *Type` —
    /// routes through the instantiation seam (the instantiateType engine is
    /// S7; checker.rs carries the permissive identity stub the mapper's
    /// composite machinery itself drives).
    fn get_type_of_instantiated_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let (target, mapper) = {
                let links = self.value_symbol_links.get(symbol);
                (links.target, links.mapper)
            };
            let target_type = self.get_type_of_symbol(target.expect("instantiated target"));
            // TODO(port M6 S7): instantiateType's real engine (activeMappers
            // caches); the identity stub preserves Go's result for identity
            // mappers.
            let resolved = self.instantiate_type(target_type, mapper.expect("instantiated mapper"));
            self.value_symbol_links.get(symbol).resolved_type = Some(resolved);
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) getWriteTypeOfInstantiatedSymbol(symbol *ast.Symbol) *Type`.
    fn get_write_type_of_instantiated_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).write_type.is_none() {
            let (target, mapper) = {
                let links = self.value_symbol_links.get(symbol);
                (links.target, links.mapper)
            };
            let target_type = self.get_write_type_of_symbol(target.expect("instantiated target"));
            // TODO(port M6 S7): as get_type_of_instantiated_symbol above.
            let write_type = self.instantiate_type(target_type, mapper.expect("instantiated mapper"));
            self.value_symbol_links.get(symbol).write_type = Some(write_type);
        }
        self.value_symbol_links.get(symbol).write_type.expect("just set")
    }

    /// Go: `func (c *Checker) getTypeOfVariableOrParameterOrProperty(symbol *ast.Symbol) *Type`.
    fn get_type_of_variable_or_parameter_or_property(&mut self, symbol: SymbolId) -> TypeId {
        let cached = self.value_symbol_links.get(symbol).resolved_type;
        if cached.is_none() {
            let t = self.get_type_of_variable_or_parameter_or_property_worker(symbol)
                .expect("Unexpected nil type");
            // For a contextually typed parameter it is possible that a type
            // has already been assigned (in assignTypeToParameterAndFixTypeParameters),
            // and we want to preserve this type. In fact, we need to _prefer_
            // that type, but it won't be assigned until contextual typing is
            // complete, so we need to defer in cases where contextual typing
            // may take place.
            let existing = self.value_symbol_links.get(symbol).resolved_type;
            if existing.is_none() && !self.is_parameter_of_context_sensitive_signature(symbol) {
                self.value_symbol_links.get(symbol).resolved_type = Some(t);
            }
            return t;
        }
        cached.expect("cached")
    }

    /// Go: `func (c *Checker) isParameterOfContextSensitiveSignature(symbol) bool`.
    fn is_parameter_of_context_sensitive_signature(&mut self, symbol: SymbolId) -> bool {
        let Some(mut decl) = self.program.symbol(symbol).value_declaration else {
            return false;
        };
        if self.store().node(decl).kind == Kind::BindingElement {
            decl = ast_shims::walk_up_binding_elements_and_patterns(self.store(), decl);
        }
        if self.store().node(decl).kind == Kind::Parameter {
            let parent = self.store().node(decl).parent.get();
            return self.is_context_sensitive(parent);
        }
        false
    }

    /// Go: `func (c *Checker) getTypeOfVariableOrParameterOrPropertyWorker(symbol) *Type`.
    fn get_type_of_variable_or_parameter_or_property_worker(
        &mut self,
        symbol: SymbolId,
    ) -> Option<TypeId> {
        // Handle prototype property
        if self.program.symbol(symbol).flags.intersects(SymbolFlags::PROTOTYPE) {
            return Some(self.get_type_of_prototype_property(symbol));
        }
        // CommonsJS require and module both have type any.
        if self.require_symbol == Some(symbol) {
            return Some(self.any_type());
        }
        let declaration = self
            .program
            .symbol(symbol)
            .value_declaration
            .expect("symbol.ValueDeclaration != nil (Go debug.Assert)");
        // JSON source files: the widened literal type of the first statement
        // expression (empty file ⇒ emptyObjectType). TODO(port M6): the
        // IsJsonSourceFile gate rides the program/loader wave; the arm is
        // dormant at checker depth.
        // Handle variable, parameter or property
        if !self.push_type_resolution(
            TypeSystemEntity::Symbol(symbol),
            TypeSystemPropertyName::Type,
        ) {
            return Some(self.report_circularity_error(symbol));
        }
        let result = if self.program.symbol(symbol).flags.intersects(SymbolFlags::MODULE_EXPORTS) {
            // TODO(port M6 S6): the `exports` arm resolves through
            // resolveExternalModuleSymbol; the named arm builds an anonymous
            // type over symbol.Members — permissive: the anonymous type with
            // the declared member set.
            let members: Vec<SymbolId> = self
                .program
                .symbol(symbol)
                .members
                .entries()
                .map(|(_, &s)| s)
                .collect();
            Some(self.new_anonymous_type(Some(symbol), &members))
        } else {
            let declaration_kind = self.store().node(declaration).kind;
            match declaration_kind {
                Kind::Parameter
                | Kind::PropertyDeclaration
                | Kind::PropertySignature
                | Kind::VariableDeclaration
                | Kind::BindingElement => {
                    // only report diagnostics for context-insensitive
                    // parameters - context-sensitive ones may have their
                    // type fixed to something else
                    let report_errors = !self.is_parameter_of_context_sensitive_signature(symbol);
                    Some(self.get_widened_type_for_variable_like_declaration(
                        declaration,
                        report_errors,
                    ))
                }
                Kind::PropertyAssignment => {
                    // TODO(port M6 S4): checkPropertyAssignment(declaration, CheckModeNormal).
                    Some(self.error_type())
                }
                Kind::ShorthandPropertyAssignment => {
                    // TODO(port M6 S4): checkShorthandPropertyAssignment(declaration, true, CheckModeNormal).
                    Some(self.error_type())
                }
                Kind::MethodDeclaration => {
                    // TODO(port M6 S4): checkObjectLiteralMethod(declaration, CheckModeNormal).
                    Some(self.error_type())
                }
                Kind::ExportAssignment => {
                    if ast_shims::type_of_node(self.store(), declaration).is_some() {
                        // TODO(port M6 S5): getTypeFromTypeNode(declaration.Type()).
                        Some(self.error_type())
                    } else {
                        let t = self.check_expression_cached_ex(declaration, CheckMode::CHECK_MODE_NORMAL);
                        Some(self.widen_type_for_variable_like_declaration(Some(t), declaration, false))
                    }
                }
                Kind::BinaryExpression | Kind::CallExpression => {
                    Some(self.get_widened_type_for_assignment_declaration(symbol))
                }
                Kind::JsxAttribute => {
                    // TODO(port M6 W5 jsx): checkJsxAttribute(declaration, CheckModeNormal).
                    Some(self.error_type())
                }
                Kind::EnumMember => Some(self.get_type_of_enum_member(symbol)),
                _ => panic!(
                    "Unhandled case in getTypeOfVariableOrParameterOrPropertyWorker: {}",
                    self.store().node(declaration).kind_string()
                ),
            }
        };
        if !self.pop_type_resolution() {
            return Some(self.report_circularity_error(symbol));
        }
        result
    }

    /// Return the type associated with a variable, parameter, or property
    /// declaration. In the simple case this is the type specified in a type
    /// annotation or inferred from an initializer. However, in the case of a
    /// destructuring declaration it is a bit more involved. For example:
    ///
    ///   var [x, s = ""] = [1, "one"];
    ///
    /// Here, the array literal [1, "one"] is contextually typed by the type
    /// [any, string], which is the implied type of the binding pattern
    /// [x, s = ""]. Because the contextual type is a tuple type, the
    /// resulting type of [1, "one"] is the tuple type [number, string].
    /// Thus, the type inferred for 'x' is number and the type inferred for
    /// 's' is string.
    ///
    /// Go: `func (c *Checker) getWidenedTypeForVariableLikeDeclaration(declaration, reportErrors) *Type`.
    pub fn get_widened_type_for_variable_like_declaration(
        &mut self,
        declaration: NodeId,
        report_errors: bool,
    ) -> TypeId {
        let t = self.get_type_for_variable_like_declaration(declaration, true, CheckMode::CHECK_MODE_NORMAL);
        self.widen_type_for_variable_like_declaration(t, declaration, report_errors)
    }

    /// Go: `func (c *Checker) getTypeOfFuncClassEnumModule(symbol *ast.Symbol) *Type`.
    fn get_type_of_func_class_enum_module(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let t = self.get_type_of_func_class_enum_module_worker(symbol);
            self.value_symbol_links.get(symbol).resolved_type = Some(t);
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) getTypeOfFuncClassEnumModuleWorker(symbol) *Type`.
    fn get_type_of_func_class_enum_module_worker(&mut self, symbol: SymbolId) -> TypeId {
        let (flags, value_declaration) = {
            let s = self.program.symbol(symbol);
            (s.flags, s.value_declaration)
        };
        if flags.intersects(SymbolFlags::MODULE)
            && u::is_shorthand_ambient_module_symbol(&self.program.symbol(symbol))
        {
            return self.any_type();
        } else if flags.intersects(SymbolFlags::VALUE_MODULE)
            && value_declaration.is_some_and(|d| {
                let store = self.store();
                let node = store.node(d);
                node.kind == Kind::SourceFile
                    && store
                        .node(node)
                        .as_source_file()
                        .is_some_and(|f| f.common_js_module_indicator.is_some())
            })
        {
            // TODO(port M6 S6): resolveExternalModuleSymbol — the
            // CommonJS-module re-type arm; permissive: fall through to the
            // module object type (Go's `resolvedModule != symbol` check
            // fails until S6 lands the resolver).
            let resolved_module = symbol;
            if resolved_module != symbol {
                return self.get_type_of_symbol(resolved_module);
            }
        }
        let t = self.new_object_type(ObjectFlags::ANONYMOUS, Some(symbol));
        if flags.intersects(SymbolFlags::CLASS) {
            let base_type_variable = self.get_base_type_variable_of_class(symbol);
            if let Some(base_type_variable) = base_type_variable {
                // TODO(port M6 S7): getIntersectionType's real constructor —
                // the permissive stub keeps the intersection's identity.
                return crate::relater::get_intersection_type(self, vec![t, base_type_variable]);
            }
            return t;
        }
        if self.strict_null_checks && flags.intersects(SymbolFlags::OPTIONAL) {
            return self.get_optional_type(t, true /*isProperty*/);
        }
        t
    }

    /// Go: `func (c *Checker) getBaseTypeVariableOfClass(symbol *ast.Symbol) *Type`.
    fn get_base_type_variable_of_class(&mut self, symbol: SymbolId) -> Option<TypeId> {
        let declared = self.get_declared_type_of_class_or_interface(symbol);
        let base_constructor_type = self.get_base_constructor_type_of_class(declared);
        let flags = self.type_(base_constructor_type).flags;
        if flags.intersects(TypeFlags::TYPE_VARIABLE) {
            return Some(base_constructor_type);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            return self
                .type_(base_constructor_type)
                .types()
                .iter()
                .copied()
                .find(|&t| self.type_(t).flags.intersects(TypeFlags::TYPE_VARIABLE));
        }
        None
    }

    /// The base constructor of a class can resolve to
    /// * undefinedType if the class has no extends clause,
    /// * errorType if an error occurred during resolution of the extends
    ///   expression,
    /// * nullType if the extends expression is the null value,
    /// * anyType if the extends expression has type any, or
    /// * an object type with at least one construct signature.
    ///
    /// Go: `func (c *Checker) getBaseConstructorTypeOfClass(t *Type) *Type`.
    pub(crate) fn get_base_constructor_type_of_class(&mut self, t: TypeId) -> TypeId {
        let resolved = self
            .type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .resolved_base_constructor_type;
        if let Some(resolved) = resolved {
            return resolved;
        }
        let base_type_node = get_base_type_node_of_class(self, t);
        let Some(base_type_node) = base_type_node else {
            let undefined_type = self.undefined_type();
            self.type_mut(t)
                .as_interface_type_mut()
                .expect("InterfaceType data")
                .resolved_base_constructor_type = Some(undefined_type);
            return undefined_type;
        };
        if !self.push_type_resolution(
            TypeSystemEntity::Type(t),
            TypeSystemPropertyName::ResolvedBaseConstructorType,
        ) {
            return self.error_type();
        }
        let expression = ast_shims::expression_of(self.store(), base_type_node)
            .expect("class heritage expression");
        let base_constructor_type = self.check_expression(expression);
        if self.type_(base_constructor_type).flags
            .intersects(TypeFlags::OBJECT.union(TypeFlags::INTERSECTION))
        {
            // Resolving the members of a class requires us to resolve the base
            // class of that class. We force resolution here such that we catch
            // circularities now.
            // TODO(port M6 S8): resolveStructuredTypeMembers.
            self.resolve_structured_type_members_stub(base_constructor_type);
        }
        if !self.pop_type_resolution() {
            let symbol = self.type_(t).symbol.expect("class symbol");
            self.error(
                expression,
                &X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_BASE_EXPRESSION,
                &[self.symbol_to_string(symbol)],
            );
            if self.type_(t)
                .as_interface_type()
                .expect("InterfaceType data")
                .resolved_base_constructor_type
                .is_none()
            {
                let error_type = self.error_type();
                self.type_mut(t)
                    .as_interface_type_mut()
                    .expect("InterfaceType data")
                    .resolved_base_constructor_type = Some(error_type);
            }
            return self
                .type_(t)
                .as_interface_type()
                .expect("InterfaceType data")
                .resolved_base_constructor_type
                .expect("just set");
        }
        let ctor_flags = self.type_(base_constructor_type).flags;
        if !ctor_flags.intersects(TypeFlags::ANY)
            && Some(base_constructor_type) != self.null_widening_type
            && !self.is_constructor_type(base_constructor_type)
        {
            let symbol = self.type_(t).symbol.expect("class symbol");
            self.error(
                expression,
                &TYPE_0_IS_NOT_A_CONSTRUCTOR_FUNCTION_TYPE,
                &[self.type_to_string(base_constructor_type)],
            );
            // Go: the type-parameter constraint related-info + the
            // Did_you_mean_for_0_to_be_constrained… suggestion ride the
            // related-diagnostic factory (M7 diagnostics wave).
            if self.type_(t)
                .as_interface_type()
                .expect("InterfaceType data")
                .resolved_base_constructor_type
                .is_none()
            {
                let error_type = self.error_type();
                self.type_mut(t)
                    .as_interface_type_mut()
                    .expect("InterfaceType data")
                    .resolved_base_constructor_type = Some(error_type);
            }
            return self
                .type_(t)
                .as_interface_type()
                .expect("InterfaceType data")
                .resolved_base_constructor_type
                .expect("just set");
        }
        if self.type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .resolved_base_constructor_type
            .is_none()
        {
            self.type_mut(t)
                .as_interface_type_mut()
                .expect("InterfaceType data")
                .resolved_base_constructor_type = Some(base_constructor_type);
        }
        self.type_(t)
            .as_interface_type()
            .expect("InterfaceType data")
            .resolved_base_constructor_type
            .expect("just set")
    }

    /// Go: `func (c *Checker) isFunctionType(t *Type) bool`.
    pub(crate) fn is_function_type(&mut self, t: TypeId) -> bool {
        self.type_(t).flags.intersects(TypeFlags::OBJECT)
            && !self.get_signatures_of_type(t, types::SignatureKind::Call).is_empty()
    }

    /// Go: `func (c *Checker) isConstructorType(t *Type) bool`.
    pub(crate) fn is_constructor_type(&mut self, t: TypeId) -> bool {
        if !self
            .get_signatures_of_type(t, types::SignatureKind::Construct)
            .is_empty()
        {
            return true;
        }
        if self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let constraint = self.get_base_constraint_of_type(t);
            return constraint.is_some_and(|constraint| self.is_mixin_constructor_type(constraint));
        }
        false
    }

    /// A type is a mixin constructor if it has a single construct signature
    /// taking no type parameters and a single rest parameter of type any[].
    ///
    /// Go: `func (c *Checker) isMixinConstructorType(t *Type) bool`.
    fn is_mixin_constructor_type(&mut self, t: TypeId) -> bool {
        let signatures = self.get_signatures_of_type(t, types::SignatureKind::Construct);
        if signatures.len() == 1 {
            let s = &self.signatures[signatures[0].0 as usize];
            if s.type_parameters.is_empty()
                && s.parameters.len() == 1
                && signature_has_rest_parameter(s)
            {
                let param_type = self.get_type_of_parameter(s.parameters[0]);
                if u::is_type_any(Some(self.type_(param_type))) {
                    return true;
                }
                if let Some(element) = self.get_element_type_of_array_type(param_type) {
                    return element == self.any_type();
                }
                return false;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getTypeOfParameter(symbol *ast.Symbol) *Type`.
    pub fn get_type_of_parameter(&mut self, symbol: SymbolId) -> TypeId {
        let declaration = self.program.symbol(symbol).value_declaration;
        let t = self.get_type_of_symbol(symbol);
        let is_optional = declaration.is_some_and(|d| {
            ast_shims::initializer_of(self.store(), d).is_some() || u::is_optional_declaration(self.store(), d)
        });
        self.add_optionality_ex(t, false /*isProperty*/, is_optional)
    }

    // ────────────────────────────────────────────────────────────────────────
    // The constraint family (Go checker.go 17434–17704 — S2 owns it; the
    // resolved-base-constraint engine is S9's, per the deliberate S2↔S9 seam)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) getConstraintOfType(t *Type) *Type`.
    pub(crate) fn get_constraint_of_type(&mut self, t: TypeId) -> Option<TypeId> {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.get_constraint_of_type_parameter(t);
        }
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return self.get_constraint_of_indexed_access(t);
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            return self.get_constraint_of_conditional_type(t);
        }
        self.get_base_constraint_of_type(t)
    }

    /// Go: `func (c *Checker) getConstraintOfTypeParameter(typeParameter *Type) *Type`.
    pub(crate) fn get_constraint_of_type_parameter(&mut self, type_parameter: TypeId) -> Option<TypeId> {
        if self.has_non_circular_base_constraint(type_parameter) {
            return self.get_constraint_from_type_parameter(type_parameter);
        }
        None
    }

    /// Go: `func (c *Checker) hasNonCircularBaseConstraint(t *Type) bool`.
    fn has_non_circular_base_constraint(&mut self, t: TypeId) -> bool {
        // TODO(port M6 S9): getResolvedBaseConstraint — the permissive stub
        // resolves nothing, so every constraint reads as non-circular (the
        // circular-constraint poison this guard exists for arrives with S9).
        self.get_resolved_base_constraint_stub(t) != Some(self.circular_constraint_type())
    }

    /// TODO(port M6 S9): `getResolvedBaseConstraint(t, stack)` — the real
    /// recursion with the RecursionId stack; permissive None until then.
    fn get_resolved_base_constraint_stub(&mut self, t: TypeId) -> Option<TypeId> {
        let _ = t;
        None
    }

    /// This is a worker function. Use getConstraintOfTypeParameter which
    /// guards against circular constraints.
    ///
    /// Go: `func (c *Checker) getConstraintFromTypeParameter(t *Type) *Type`.
    pub(crate) fn get_constraint_from_type_parameter(&mut self, t: TypeId) -> Option<TypeId> {
        if !self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return None;
        }
        let existing = self
            .type_(t)
            .as_type_parameter()
            .expect("TypeParameter data")
            .constraint;
        if existing.is_none() {
            let constraint = {
                // The borrow discipline: read the structural facts, then run
                // the mutating resolution arms (§5.4 arena idiom).
                let (target, mapper) = {
                    let tp = self.type_(t).as_type_parameter().expect("TypeParameter data");
                    (tp.target, tp.mapper)
                };
                if let Some(target) = target {
                    let target_constraint = self.get_constraint_of_type_parameter(target);
                    // TODO(port M6 S7): instantiateType's real engine — the
                    // identity stub preserves the target's constraint for
                    // identity mappers.
                    match (target_constraint, mapper) {
                        (Some(c), Some(m)) => Some(self.instantiate_type(c, m)),
                        (c, _) => c,
                    }
                } else {
                    match self.get_constraint_declaration(t) {
                        Some(constraint_declaration) => {
                            // TODO(port M6 S5): getTypeFromTypeNode — the
                            // annotation arm; permissive errorType (an ANY
                            // constraint routes to unknownType/stringNumberSymbolType
                            // below, matching Go's "any means unknown" rule).
                            let mut constraint = self.get_type_from_type_node(constraint_declaration);
                            let declaration_parent = {
                                let store = self.store();
                                let n = store.node(constraint_declaration);
                                (n.parent.get(), store.node(n.parent.get()).kind)
                            };
                            if self.type_(constraint).flags.intersects(TypeFlags::ANY)
                                && !self.is_error_type(constraint)
                            {
                                // use stringNumberSymbolType as the base
                                // constraint for mapped type key constraints
                                // (unknown isn't assignable to that, but `any`
                                // was), use unknown otherwise
                                if declaration_parent.1 == Kind::MappedType {
                                    // TODO(port M6 S7): stringNumberSymbolType
                                    // = getUnionType([string, number, symbol])
                                    // — the permissive union stub's value
                                    // (errorType) is the documented stand-in.
                                    constraint = crate::relater::get_union_type(
                                        self,
                                        vec![
                                            self.string_type(),
                                            self.number_type(),
                                            self.es_symbol_type(),
                                        ],
                                    );
                                } else {
                                    constraint = self.unknown_type();
                                }
                            }
                            Some(constraint)
                        }
                        None => self.get_inferred_type_parameter_constraint(t, false),
                    }
                }
            };
            let constraint = constraint.unwrap_or_else(|| self.no_constraint_type());
            self.type_mut(t)
                .as_type_parameter_mut()
                .expect("TypeParameter data")
                .constraint = Some(constraint);
        }
        let constraint = self
            .type_(t)
            .as_type_parameter()
            .expect("TypeParameter data")
            .constraint
            .expect("just set");
        if constraint != self.no_constraint_type() {
            return Some(constraint);
        }
        None
    }

    /// Go: `func (c *Checker) getConstraintOrUnknownFromTypeParameter(t *Type) *Type`.
    pub(crate) fn get_constraint_or_unknown_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        self.get_constraint_from_type_parameter(t)
            .unwrap_or_else(|| self.unknown_type())
    }

    /// Go: `func (c *Checker) getInferredTypeParameterConstraint(t *Type, omitTypeReferences bool) *Type`.
    fn get_inferred_type_parameter_constraint(
        &mut self,
        t: TypeId,
        omit_type_references: bool,
    ) -> Option<TypeId> {
        let mut inferences: Vec<TypeId> = Vec::new();
        let symbol = self.type_(t).symbol;
        if let Some(symbol) = symbol {
            let declarations = self.program.symbol(symbol).declarations.clone();
            for declaration in declarations {
                let (is_infer_parent, parent) = {
                    let store = self.store();
                    let parent = store.node(declaration).parent.get();
                    (store.node(parent).kind == Kind::InferType, parent)
                };
                if is_infer_parent {
                    // When an 'infer T' declaration is immediately contained
                    // in a type reference node (such as 'Foo<infer T>'), T's
                    // constraint is inferred from the constraint of the
                    // corresponding type parameter in 'Foo'. When multiple
                    // 'infer T' declarations are present, we form an
                    // intersection of the inferred constraint types.
                    let (mut child, mut parent_node) = (declaration, parent);
                    // Skip parenthesized type nodes.
                    loop {
                        let parent_kind = self.store().node(parent_node).kind;
                        if parent_kind == Kind::ParenthesizedType {
                            child = parent_node;
                            parent_node = self.store().node(child).parent.get();
                        } else {
                            break;
                        }
                    }
                    let parent_kind = self.store().node(parent_node).kind;
                    if parent_kind == Kind::TypeReference && !omit_type_references {
                        // TODO(port M6 S5/S7): the type-reference inference arm
                        // (getTypeParametersForTypeReferenceOrImport +
                        // newDeferredTypeMapper + instantiateType) — lands with
                        // the type-node family; the other four arms below are
                        // Go's structure, routed through the same seams.
                        let _ = (child, parent_node);
                    } else if parent_kind == Kind::Parameter
                        && self.store().node(parent_node).as_parameter_declaration().is_some_and(|p| p.dot_dot_dot_token.is_some())
                        || parent_kind == Kind::RestType
                        || parent_kind == Kind::NamedTupleMember
                            && self.store().node(parent_node).as_named_tuple_member().is_some_and(|m| m.dot_dot_dot_token.is_some())
                    {
                        // TODO(port M6 S7): createArrayType(c.unknownType) —
                        // the permissive array-type identity stub keeps
                        // unknownType (the element type) as the inference.
                        inferences.push(self.unknown_type());
                    } else if parent_kind == Kind::TemplateLiteralTypeSpan {
                        inferences.push(self.string_type());
                    } else if parent_kind == Kind::TypeParameter
                        && self.store().node(self.store().node(parent_node).parent.get()).kind == Kind::MappedType
                    {
                        // TODO(port M6 S7): stringNumberSymbolType (as the
                        // mapped-type-key arm above).
                        inferences.push(crate::relater::get_union_type(
                            self,
                            vec![
                                self.string_type(),
                                self.number_type(),
                                self.es_symbol_type(),
                            ],
                        ));
                    }
                    // Go's fifth arm (the mapped-type-in-conditional-template
                    // inference) is S5/S7's instantiateType path — same seam
                    // as the type-reference arm above.
                }
            }
        }
        if !inferences.is_empty() {
            // TODO(port M6 S7): getIntersectionType's real constructor.
            return Some(crate::relater::get_intersection_type(self, inferences));
        }
        None
    }

    /// TODO(port M6 S5): `getConstraintDeclaration(t)` — the type-parameter
    /// declaration's constraint node (checker.go's checker.go 25046 region);
    /// permissive None (unconstrained) until the type-node wave.
    fn get_constraint_declaration(&mut self, t: TypeId) -> Option<NodeId> {
        let symbol = self.type_(t).symbol?;
        let store = self.store();
        self.program
            .symbol(symbol)
            .declarations
            .iter()
            .copied()
            .find_map(|d| {
                let node = store.node(d);
                if node.kind == Kind::TypeParameter {
                    node.as_type_parameter_declaration()
                        .expect("TypeParameterDeclaration data")
                        .constraint
                } else {
                    None
                }
            })
    }

    /// Go: `func (c *Checker) getConstraintOfIndexedAccess(t *Type) *Type`.
    fn get_constraint_of_indexed_access(&mut self, t: TypeId) -> Option<TypeId> {
        if self.has_non_circular_base_constraint(t) {
            return self.get_constraint_from_indexed_access(t);
        }
        None
    }

    /// Go: `func (c *Checker) getConstraintFromIndexedAccess(t *Type) *Type`.
    fn get_constraint_from_indexed_access(&mut self, t: TypeId) -> Option<TypeId> {
        // TODO(port M6 S10): the substituteIndexedMappedType / simplified
        // constraint arms — the indexed-access constructor family's wave.
        let (object_type, index_type, access_flags) = {
            let d = self.type_(t).as_indexed_access_type().expect("IndexedAccessType data");
            (
                d.object_type.expect("objectType"),
                d.index_type.expect("indexType"),
                d.access_flags,
            )
        };
        let _ = (object_type, index_type, access_flags);
        None
    }

    /// Go: `func (c *Checker) getConstraintOfConditionalType(t *Type) *Type`.
    fn get_constraint_of_conditional_type(&mut self, t: TypeId) -> Option<TypeId> {
        if self.has_non_circular_base_constraint(t) {
            return self.get_constraint_from_conditional_type(t);
        }
        None
    }

    /// Go: `func (c *Checker) getConstraintFromConditionalType(t *Type) *Type`.
    fn get_constraint_from_conditional_type(&mut self, t: TypeId) -> Option<TypeId> {
        let constraint = self.get_constraint_of_distributive_conditional_type(t);
        if constraint.is_some() {
            return constraint;
        }
        Some(self.get_default_constraint_of_conditional_type(t))
    }

    /// Go: `func (c *Checker) getDefaultConstraintOfConditionalType(t *Type) *Type`.
    fn get_default_constraint_of_conditional_type(&mut self, t: TypeId) -> TypeId {
        let resolved = self
            .type_(t)
            .as_conditional_type()
            .expect("ConditionalType data")
            .resolved_default_constraint;
        if let Some(resolved) = resolved {
            return resolved;
        }
        // An `any` branch of a conditional type would normally be viral -
        // specifically, without special handling here, a conditional type
        // with a single branch of type `any` would be assignable to anything,
        // since its constraint would simplify to just `any`. This result is
        // _usually_ unwanted - so instead here we elide an `any` branch from
        // the constraint type, in effect treating `any` like `never` rather
        // than `unknown` in this location.
        // TODO(port M6 S5/S7): getInferredTrueTypeFromConditionalType's
        // resolved branch reads + getUnionType — the permissive union stub
        // builds the (unreduced) union of the two branch reads.
        let true_constraint = self.get_inferred_true_type_from_conditional_type_stub(t);
        let false_constraint = self.get_false_type_of_conditional_type(t);
        let result = if u::is_type_any(Some(self.type_(true_constraint))) {
            false_constraint
        } else if u::is_type_any(Some(self.type_(false_constraint))) {
            true_constraint
        } else {
            crate::relater::get_union_type(self, vec![true_constraint, false_constraint])
        };
        self.type_mut(t)
            .as_conditional_type_mut()
            .expect("ConditionalType data")
            .resolved_default_constraint = Some(result);
        result
    }

    /// TODO(port M6 S5): `getInferredTrueTypeFromConditionalType(t)` — the
    /// combinedMapper-instantiated true branch; permissive: the resolved true
    /// branch (the un-instantiated read S10 owns the real worker for).
    fn get_inferred_true_type_from_conditional_type_stub(&mut self, t: TypeId) -> TypeId {
        self.type_(t)
            .as_conditional_type()
            .and_then(|d| d.resolved_true_type)
            .unwrap_or_else(|| self.error_type())
    }

    /// Go: `func (c *Checker) getFalseTypeFromConditionalType(t)` (relater's
    /// resolved-branch read, surfaced here as the constraint family's callee).
    fn get_false_type_of_conditional_type(&mut self, t: TypeId) -> TypeId {
        self.type_(t)
            .as_conditional_type()
            .and_then(|d| d.resolved_false_type)
            .unwrap_or_else(|| self.error_type())
    }

    /// Go: `func (c *Checker) getConstraintOfDistributiveConditionalType(t *Type) *Type`.
    fn get_constraint_of_distributive_conditional_type(&mut self, t: TypeId) -> Option<TypeId> {
        let resolved = self
            .type_(t)
            .as_conditional_type()
            .expect("ConditionalType data")
            .resolved_constraint_of_distributive;
        if resolved.is_none() {
            // Check if we have a conditional type of the form
            // 'T extends U ? X : Y', where T is a constrained type parameter.
            // If so, create an instantiation of the conditional type where T
            // is replaced with its constraint. We do this because if the
            // constraint is a union type it will be distributed over the
            // conditional type and possibly reduced. For example,
            // 'T extends undefined ? never : T' removes 'undefined' from T.
            // We skip returning a distributive constraint for a restrictive
            // instantiation of a conditional type as the constraint for all
            // type params (check type included) have been replace with
            // `unknown`, which is going to produce even more false
            // positive/negative results than the distribute constraint
            // already does.
            // Please note: the distributive constraint is a kludge for
            // emulating what a negated type could to do filter a union -
            // once negated types exist and are applied to the conditional
            // false branch, this "constraint" likely doesn't need to exist.
            let (is_distributive, check_type, mapper, restrictive_hit) = {
                let d = self.type_(t).as_conditional_type().expect("ConditionalType data");
                let root = d.root.as_deref();
                (
                    root.is_some_and(|r| r.is_distributive),
                    d.check_type,
                    d.mapper,
                    self.cached_types_get(CachedTypeKind::RestrictiveInstantiation, t) == Some(t),
                )
            };
            if is_distributive && !restrictive_hit {
                // TODO(port M6 S7/S10): getSimplifiedType + the constraint
                // instantiation (getConditionalTypeInstantiation with
                // prependTypeMapping) — the instantiation engine's wave; the
                // permissive arm falls to the noConstraintType answer below.
                let _ = (check_type, mapper);
            }
            let no_constraint = self.no_constraint_type();
            self.type_mut(t)
                .as_conditional_type_mut()
                .expect("ConditionalType data")
                .resolved_constraint_of_distributive = Some(no_constraint);
        }
        let resolved = self
            .type_(t)
            .as_conditional_type()
            .expect("ConditionalType data")
            .resolved_constraint_of_distributive
            .expect("just set");
        if resolved != self.no_constraint_type() {
            return Some(resolved);
        }
        None
    }

    /// The `cachedTypes` map read (Go: `c.cachedTypes[CachedTypeKey{kind, typeId}]`).
    pub(crate) fn cached_types_get(&self, kind: CachedTypeKind, type_id: TypeId) -> Option<TypeId> {
        self.cached_types
            .get(&CachedTypeKey { kind, type_id: type_id.0 })
            .copied()
            .flatten()
    }

    /// The `cachedTypes` map write.
    pub(crate) fn cached_types_set(&mut self, kind: CachedTypeKind, type_id: TypeId, result: Option<TypeId>) {
        self.cached_types.insert(CachedTypeKey { kind, type_id: type_id.0 }, result);
    }

    /// Go: `func (c *Checker) getTypeOfPrototypeProperty(prototype *ast.Symbol) *Type`.
    fn get_type_of_prototype_property(&mut self, prototype: SymbolId) -> TypeId {
        // TypeScript 1.0 spec (April 2014): 8.4
        // Every class automatically contains a static property member named
        // 'prototype', the type of which is an instantiation of the class type
        // with type Any supplied as a type argument for each type parameter.
        // It is an error to explicitly declare a static property member with
        // the name 'prototype'.
        let parent = self.get_parent_of_symbol(prototype);
        let class_type = parent.map(|p| self.get_declared_type_of_symbol(p));
        let Some(class_type) = class_type else {
            return self.error_type();
        };
        let type_parameters = self
            .type_(class_type)
            .as_interface_type()
            .expect("InterfaceType data")
            .type_parameters()
            .to_vec();
        if !type_parameters.is_empty() {
            // TODO(port M6 S7): createTypeReference — the permissive identity
            // stub returns the class type itself (any-filled reference).
            return self.create_type_reference(class_type, vec![self.any_type(); type_parameters.len()]);
        }
        class_type
    }
}

/// Go: `func signatureHasRestParameter(sig *Signature) bool`.
pub(crate) fn signature_has_rest_parameter(sig: &crate::types::Signature) -> bool {
    sig.flags.intersects(SignatureFlags::HAS_REST_PARAMETER)
}

/// Go: `func getBaseTypeNodeOfClass(t *Type) *ast.Node` (utilities.go — the
/// class heritage read the base-constructor resolution needs; arena-local).
pub(crate) fn get_base_type_node_of_class(c: &Checker, t: TypeId) -> Option<NodeId> {
    let symbol = c.type_(t).symbol?;
    let store = c.store();
    for &declaration in &c.program.symbol(symbol).declarations {
        let node = store.node(declaration);
        if node.kind == Kind::ClassDeclaration || node.kind == Kind::ClassExpression {
            let heritage = node
                .as_class_declaration()
                .or_else(|| node.as_class_expression())
                .expect("ClassDeclaration/ClassExpression data")
                .heritage_clauses
                .clone();
            for clause in heritage.iter().flat_map(|list| list.nodes.iter().copied()) {
                if store.node(clause).kind == Kind::HeritageClause
                    && store.node(clause).as_heritage_clause().expect("HeritageClause data").token == Kind::ExtendsKeyword
                {
                    let types = store.node(clause).as_heritage_clause().expect("HeritageClause data").types.clone();
                    return types.nodes.first().copied();
                }
            }
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────────────
// Node/symbol shims the S2 spine needs (the S6 wave replaces these with the
// real merged/late-bound resolution; the bodies below are Go's node.Symbol()
// reads — the binder-set payload symbols — which is exactly what Go's
// getSymbolOfDeclaration returns while the merged-symbol table is trivial)
// ────────────────────────────────────────────────────────────────────────────

/// The payload `symbol` field of a declaration-carrying node (Go: `node.Symbol()`).
pub(crate) fn payload_symbol(node: &Node) -> Option<SymbolId> {
    match &node.data {
        NodeData::VariableDeclaration(d) => d.symbol,
        NodeData::Parameter(d) => d.symbol,
        NodeData::BindingElement(d) => d.symbol,
        NodeData::PropertyDeclaration(d) => d.symbol,
        NodeData::PropertySignature(d) => d.symbol,
        NodeData::PropertyAssignment(d) => d.symbol,
        NodeData::ShorthandPropertyAssignment(d) => d.symbol,
        NodeData::MethodDeclaration(d) => d.symbol,
        NodeData::MethodSignature(d) => d.symbol,
        NodeData::GetAccessor(d) => d.symbol,
        NodeData::SetAccessor(d) => d.symbol,
        NodeData::FunctionDeclaration(d) => d.symbol,
        NodeData::FunctionExpression(d) => d.symbol,
        NodeData::ArrowFunction(d) => d.symbol,
        NodeData::ClassDeclaration(d) => d.symbol,
        NodeData::ClassExpression(d) => d.symbol,
        NodeData::InterfaceDeclaration(d) => d.symbol,
        NodeData::TypeAliasDeclaration(d) => d.symbol,
        NodeData::EnumDeclaration(d) => d.symbol,
        NodeData::EnumMember(d) => d.symbol,
        NodeData::ModuleDeclaration(d) => d.symbol,
        NodeData::TypeParameter(d) => d.symbol,
        NodeData::ConstructorDeclaration(d) => d.symbol,
        NodeData::ClassStaticBlockDeclaration(d) => d.symbol,
        NodeData::CallSignatureDeclaration(d) => d.symbol,
        NodeData::ConstructSignatureDeclaration(d) => d.symbol,
        NodeData::IndexSignatureDeclaration(d) => d.symbol,
        NodeData::SourceFile(d) => d.symbol,
        _ => None,
    }
}

/// Go: `func (c *Checker) getSymbolOfDeclaration(node *ast.Node) *ast.Symbol` —
/// `node.Symbol()` then `getMergedSymbol(getLateBoundSymbol(symbol))`. The
/// merged/late-bound machinery is S6 (permissive identity stubs in exports.rs);
/// the payload read below is the binder-set declaration symbol.
pub(crate) fn get_symbol_of_declaration(c: &Checker, node: NodeId) -> Option<SymbolId> {
    payload_symbol(c.store().node(node))
}

/// Go: `func isJsxTagName(node *ast.Node) bool` (ast/astnav — the local form
/// of the two-line parent-kind check).
fn is_jsx_tag_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent_kind = store.node(store.node(node).parent.get()).kind;
    parent_kind == Kind::JsxOpeningElement || parent_kind == Kind::JsxSelfClosingElement
        || parent_kind == Kind::JsxClosingElement
}

/// Go: `func IsRightSideOfQualifiedNameOrPropertyAccess(node *ast.Node) bool`.
fn is_right_side_of_qualified_name_or_property_access(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    let parent_kind = store.node(parent).kind;
    (parent_kind == Kind::QualifiedName || parent_kind == Kind::PropertyAccessExpression)
        && store.node(parent).name() != Some(node)
}

/// Go: `func IsDeclarationName(node *ast.Node) bool`.
fn is_declaration_name(store: &dyn NodeStore, node: NodeId) -> bool {
    let parent = store.node(node).parent.get();
    store.node(parent).name() == Some(node)
}

impl Checker {
    /// Go: `func (c *Checker) getAnnotatedAccessorTypeNode(accessor *ast.Node) *ast.Node`.
    pub(crate) fn get_annotated_accessor_type_node(&self, accessor: Option<NodeId>) -> Option<NodeId> {
        let accessor = accessor?;
        match self.store().node(accessor).kind {
            Kind::GetAccessor | Kind::PropertyDeclaration => {
                ast_shims::type_of_node(self.store(), accessor)
            }
            Kind::SetAccessor => u::get_set_accessor_value_parameter(self.store(), accessor)
                .and_then(|param| ast_shims::type_of_node(self.store(), param)),
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getAnnotatedAccessorType(accessor *ast.Node) *Type`.
    fn get_annotated_accessor_type(&mut self, accessor: Option<NodeId>) -> Option<TypeId> {
        let node = self.get_annotated_accessor_type_node(accessor)?;
        // TODO(port M6 S5): getTypeFromTypeNode — the type-node family; the
        // stub's errorType keeps the "annotation present" distinction Go's
        // nil check reads (an errorType annotation is still a type).
        Some(self.get_type_from_type_node(node))
    }

    /// Go: `func (c *Checker) resolveAlias(symbol *ast.Symbol) *ast.Symbol`
    /// (checker.go:15028 — S6). TODO(port M6 S6): the alias-resolution walk
    /// (getMergedSymbol + getTargetOfAliasDeclaration); Go *panics* on an
    /// unresolvable alias, so the permissive seam resolves to the
    /// unknownSymbol — getSymbolFlagsEx maps that to SymbolFlagsAll exactly
    /// like Go's `target == c.unknownSymbol` arm.
    fn resolve_alias(&mut self, symbol: SymbolId) -> SymbolId {
        let _ = symbol;
        self.unknown_symbol()
    }

    /// Go: `func (c *Checker) getExportSymbolOfValueSymbolIfExported(symbol) *ast.Symbol`
    /// (checker.go:14652) — real body over the S6 getMergedSymbol identity stub.
    pub(crate) fn get_export_symbol_of_value_symbol_if_exported(&mut self, symbol: SymbolId) -> SymbolId {
        let (flags, export_symbol) = {
            let s = self.program.symbol(symbol);
            (s.flags, s.export_symbol)
        };
        if flags.intersects(SymbolFlags::EXPORT_VALUE) {
            if let Some(export_symbol) = export_symbol {
                return self.get_merged_symbol(export_symbol);
            }
        }
        self.get_merged_symbol(symbol)
    }

    /// Go: `func (c *Checker) getTargetOfAliasDeclaration(node *ast.Node) *ast.Symbol`
    /// (S6). TODO(port M6 S6): the alias-declaration target resolution;
    /// permissive None (getTypeOfAlias falls back to the alias symbol like
    /// Go's `core.OrElse(exportSymbol, symbol)`).
    fn get_target_of_alias_declaration(&mut self, node: Option<NodeId>) -> Option<SymbolId> {
        let _ = node;
        None
    }

    /// Go: `func (c *Checker) removeMissingType(t *Type, isOptional bool) *Type`
    /// (checker.go:29621 — S7). TODO(port M6 S7): the missing-type removal
    /// over the exactOptionalPropertyTypes marker; identity until then.
    pub(crate) fn remove_missing_type(&mut self, t: TypeId, is_optional: bool) -> TypeId {
        let _ = is_optional;
        t
    }

    /// Go: `func (c *Checker) removeOptionalTypeMarker(t *Type) *Type` —
    /// TODO(port M6 S7); identity.
    fn remove_optional_type_marker(&mut self, t: TypeId) -> TypeId {
        t
    }

    /// Go: `func (c *Checker) getTypeOfReverseMappedSymbol(symbol) *Type`
    /// (checker.go — the reverse-mapped links read).
    fn get_type_of_reverse_mapped_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let property_type = self.reverse_mapped_symbol_links.get(symbol).property_type;
            let t = if let Some(property_type) = property_type {
                property_type
            } else {
                self.error_type()
            };
            self.value_symbol_links.get(symbol).resolved_type = Some(t);
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) newAnonymousType(symbol, members, callSignatures, constructSignatures, indexInfos) *Type`
    /// — the S2 form that carries real members (S1's spine.rs helper
    /// debug-asserts empty members; this slice's binding-pattern/rest/module
    /// arms construct populated member tables).
    ///
    /// PORT: Go's setStructuredTypeMembers fills the StructuredType members
    /// and marks MembersResolved; the port does the same on the arena slot
    /// (properties = the members' values in declaration order — Go's member
    /// resolution computes the same list for anonymous types).
    pub(crate) fn new_anonymous_type_with_members(
        &mut self,
        symbol: Option<SymbolId>,
        members: SymbolTable,
    ) -> TypeId {
        let properties: Vec<SymbolId> = members.entries().map(|(_, &s)| s).collect();
        let t = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
        {
            let data = self
                .type_mut(t)
                .as_object_type_mut()
                .expect("ObjectType data");
            data.base.members = members;
            data.base.properties = properties;
        }
        self.type_mut(t).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
        t
    }

    /// Go: `func (c *Checker) resolveStructuredTypeMembers(t *Type)` (S8) —
    /// the permissive no-op (S1's anonymous types are constructed resolved;
    /// the declared-type members resolution is S8's wave).
    fn resolve_structured_type_members_stub(&mut self, t: TypeId) {
        let _ = t;
    }

    /// Go: `func (c *Checker) getSignatureFromDeclaration(declaration) *Signature`
    /// (S8). TODO(port M6 S8): the declaration→signature resolution;
    /// permissive None.
    fn get_signature_from_declaration(&mut self, declaration: NodeId) -> Option<SignatureId> {
        let _ = declaration;
        None
    }

    /// Go: `func (c *Checker) getAccessorThisParameter(accessor *ast.Node) *ast.Node`.
    fn get_accessor_this_parameter(&mut self, accessor: NodeId) -> Option<NodeId> {
        let expected = if self.store().node(accessor).kind == Kind::GetAccessor {
            1
        } else {
            2
        };
        let parameters = ast_shims::parameters_of(self.store(), accessor);
        if parameters.len() == expected {
            // Go: ast.GetThisParameter — the first parameter when it is one.
            return parameters.first().copied().filter(|&p| {
                ast_shims::is_this_parameter(self.store(), p)
            });
        }
        None
    }

    /// Go: `func (c *Checker) hasBindableName(node *ast.Node) bool`.
    fn has_bindable_name(&mut self, node: NodeId) -> bool {
        // TODO(port M6 S4/S6): the hasLateBindableName half (the computed
        // name's literal-type check); the early-bound half is real.
        !binder_shims::has_dynamic_name(self.store().node(node))
    }

    /// TODO(port M6 S9): `getParameterTypeOfFullSignature(fn, parameter)` —
    /// the contextual-signature parameter typing; permissive None.
    fn get_parameter_type_of_full_signature(&mut self, fn_: NodeId, parameter: NodeId) -> Option<TypeId> {
        let _ = (fn_, parameter);
        None
    }

    /// TODO(port M6 S9): `getContextuallyTypedParameterType(parameter)` —
    /// permissive None.
    fn get_contextually_typed_parameter_type(&mut self, parameter: NodeId) -> Option<TypeId> {
        let _ = parameter;
        None
    }

    /// TODO(port M6 S9): `getContextualThisParameterType(fn)` — permissive None.
    fn get_contextual_this_parameter_type(&mut self, fn_: NodeId) -> Option<TypeId> {
        let _ = fn_;
        None
    }

    /// TODO(port M6 W3): `getFlowTypeInConstructor(symbol, constructor)` —
    /// permissive None (Go: no assignments found ⇒ nil).
    fn get_flow_type_in_constructor(&mut self, symbol: SymbolId, constructor: NodeId) -> Option<TypeId> {
        let _ = (symbol, constructor);
        None
    }

    /// TODO(port M6 W3): `getFlowTypeInStaticBlocks(symbol, staticBlocks)`.
    fn get_flow_type_in_static_blocks(&mut self, symbol: SymbolId, static_blocks: Vec<NodeId>) -> Option<TypeId> {
        let _ = (symbol, static_blocks);
        None
    }

    /// TODO(port M6 S3): `getTypeOfPropertyInBaseClass(symbol)` — permissive None.
    fn get_type_of_property_in_base_class(&mut self, symbol: SymbolId) -> Option<TypeId> {
        let _ = symbol;
        None
    }

    /// TODO(port M6 S3): `checkRightHandSideOfForOf` → the iterated-type
    /// machinery; permissive errorType.
    fn check_right_hand_side_of_for_of(&mut self, statement: NodeId) -> TypeId {
        let _ = statement;
        self.error_type()
    }

    /// Go: `func (c *Checker) getExtractStringType(t *Type) *Type` (S10) —
    /// TODO(port M6 S10): the mapped-type/string-literal extraction; the
    /// identity keeps the for-in arm's stringType answer dominant.
    fn get_extract_string_type(&mut self, t: TypeId) -> TypeId {
        let _ = t;
        self.string_type()
    }

    /// TODO(port M6 S10): `getIndexedAccessTypeEx(objectType, indexType,
    /// accessFlags, accessNode, errorNode)` — the indexed-access constructor;
    /// permissive errorType (relater.rs carries the same seam).
    fn get_indexed_access_type_ex(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        access_node: Option<NodeId>,
    ) -> TypeId {
        let _ = (access_flags, access_node);
        crate::relater::get_indexed_access_type(self, object_type, index_type)
    }

    /// Go: `func (c *Checker) getTypeOfInitializer(declaration *ast.Node) *Type`
    /// (checker.go:25235 region) — the initializer's expression type.
    fn get_type_of_initializer(&mut self, node: NodeId) -> TypeId {
        let initializer = ast_shims::initializer_of(self.store(), node)
            .or_else(|| {
                // Go: `node.Initializer()` on a BindingElement/PropertyAssignment payload.
                match &self.store().node(node).data {
                    NodeData::BindingElement(d) => d.initializer,
                    NodeData::PropertyAssignment(d) => d.initializer,
                    _ => None,
                }
            })
            .expect("initializer");
        self.get_type_of_expression(initializer)
    }

    /// Go: `func (c *Checker) getNonUndefinedType(t *Type) *Type` (S10) —
    /// the generic-undefined-constraint map + getTypeWithFacts; the S7/S10
    /// permissive seams compose to the identity-with-facts-stub answer.
    fn get_non_undefined_type(&mut self, t: TypeId) -> TypeId {
        // TODO(port M6 S10): isGenericTypeWithUndefinedConstraint + the
        // base-constraint mapping; the facts-stub identity is the permissive
        // value (undefined removal is a no-op until the facts wave).
        t
    }

    /// Go: `func (c *Checker) getTypeOfSymbol(symbol)`'s mapped-symbol arm's
    /// callee, `getMappedSymbolLinks.keyType` — surfaced for the module doc's
    /// mapper seam. (Real use in get_type_of_mapped_symbol below.)
    fn mapped_symbol_key_type(&mut self, symbol: SymbolId) -> Option<TypeId> {
        self.mapped_symbol_links.get(symbol).key_type
    }

    // ────────────────────────────────────────────────────────────────────────
    // getTypeForVariableLikeDeclaration (Go checker.go 17038–17268)
    // ────────────────────────────────────────────────────────────────────────

    /// Return the inferred type for a variable, parameter, or property declaration.
    ///
    /// Go: `func (c *Checker) getTypeForVariableLikeDeclaration(declaration, includeOptionality, checkMode) *Type`.
    pub(crate) fn get_type_for_variable_like_declaration(
        &mut self,
        declaration: NodeId,
        include_optionality: bool,
        check_mode: CheckMode,
    ) -> Option<TypeId> {
        // Capture the node facts up front (ids are Copy) so the store borrow
        // never straddles a `&mut self` call (§5.4 arena idiom).
        let (kind, node_flags, name, name_kind, initializer, grandparent_kind) = {
            let store = self.program.node_store();
            let node = store.node(declaration);
            let parent = node.parent.get();
            let grandparent_kind = if parent == NodeId::NONE {
                Kind::SourceFile
            } else {
                store.node(store.node(parent).parent.get()).kind
            };
            (
                node.kind,
                node.flags,
                node.name(),
                node.name().map(|n| store.node(n).kind),
                ast_shims::initializer_of(store, declaration),
                grandparent_kind,
            )
        };
        // A variable declared in a for..in statement is of type string, or of
        // type keyof T when the right hand expression is of a type parameter
        // type.
        if kind == Kind::VariableDeclaration {
            match grandparent_kind {
                Kind::ForInStatement => {
                    let grandparent = self.store().node(declaration).parent.get();
                    let grandparent = self.store().node(grandparent).parent.get();
                    let expression = ast_shims::expression_of(self.store(), grandparent)
                        .expect("for-in expression");
                    let t = self.check_expression_ex(expression, check_mode);
                    // TODO(port M6 S9/S10): getNonNullableTypeIfNeeded — the
                    // facts-stub identity is the permissive value.
                    let t = t;
                    let index_type = crate::relater::get_index_type(self, t);
                    let index_flags = self.type_(index_type).flags;
                    if index_flags
                        .intersects(TypeFlags::TYPE_PARAMETER.union(TypeFlags::INDEX))
                    {
                        return Some(self.get_extract_string_type(index_type));
                    }
                    return Some(self.string_type());
                }
                Kind::ForOfStatement => {
                    // checkRightHandSideOfForOf will return undefined if the
                    // for-of expression type was missing properties/signatures
                    // required to get its iteratedType (like [Symbol.iterator]
                    // or next). This may be because we accessed properties
                    // from anyType, or it may have led to an error inside
                    // getElementTypeOfIterable.
                    // TODO(port M6 S3): the real iterated-type resolution;
                    // the permissive stub carries errorType (Go's missing
                    // answer is nil → any via the widening default).
                    let grandparent = self.store().node(declaration).parent.get();
                    let grandparent = self.store().node(grandparent).parent.get();
                    return Some(self.check_right_hand_side_of_for_of(grandparent));
                }
                _ => {}
            }
        } else if kind == Kind::BindingElement {
            return self.get_type_for_binding_element(declaration);
        }
        let is_property = (kind == Kind::PropertyDeclaration
            && !binder_shims::has_accessor_modifier(self.store().node(declaration)))
            || kind == Kind::PropertySignature;
        let is_optional = include_optionality && u::is_optional_declaration(self.store(), declaration);
        // Use type from type annotation if one is present
        let declared_type = self.try_get_type_from_type_node(declaration);
        if is_catch_clause_variable_declaration_or_binding_element(self.store(), declaration) {
            if let Some(declared_type) = declared_type {
                // If the catch clause is explicitly annotated with any or
                // unknown, accept it, otherwise error.
                if self.type_(declared_type).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                    return Some(declared_type);
                }
                let _ = self.error(
                    declaration,
                    &VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE,
                    &[String::new(), String::new()],
                );
                return Some(self.error_type());
            }
            // If the catch clause is not explicitly annotated, treat it as
            // though it were explicitly annotated with unknown or any,
            // depending on useUnknownInCatchVariables.
            return Some(if self.use_unknown_in_catch_variables {
                self.unknown_type()
            } else {
                self.any_type()
            });
        }
        if let Some(declared_type) = declared_type {
            return Some(self.add_optionality_ex(declared_type, is_property, is_optional));
        }
        if self.no_implicit_any
            && kind == Kind::VariableDeclaration
            && name_kind.is_some_and(|k| k != Kind::ObjectBindingPattern)
            && name_kind.is_some_and(|k| k != Kind::ArrayBindingPattern)
            && !self.get_combined_modifier_flags_cached(declaration).intersects(ModifierFlags::EXPORT)
            && !node_flags.intersects(NF::AMBIENT)
        {
            // If --noImplicitAny is on or the declaration is in a Javascript
            // file, use control flow tracked 'any' type for non-ambient,
            // non-exported var or let variables with no initializer or a
            // 'null' or 'undefined' initializer.
            let not_constant =
                !self.get_combined_node_flags_cached(declaration).intersects(NF::CONSTANT);
            if not_constant
                && (initializer.is_none()
                    || initializer.is_some_and(|i| self.is_null_or_undefined(i)))
            {
                return Some(self.auto_type());
            }
            // Use control flow tracked 'any[]' type for non-ambient,
            // non-exported variables with an empty array literal initializer.
            if initializer.is_some_and(|i| u::is_empty_array_literal(self.store(), i)) {
                return self.auto_array_type;
            }
        }
        if kind == Kind::Parameter {
            // For a parameter of a set accessor, use the type of the get
            // accessor if one is present
            let symbol = payload_symbol(self.store().node(declaration));
            // parameters of function types defined in JSDoc in TS files don't
            // have symbols
            let Some(symbol) = symbol else {
                return None;
            };
            let fn_ = self.store().node(declaration).parent.get();
            if self.store().node(fn_).kind == Kind::SetAccessor && self.has_bindable_name(fn_) {
                let parent_symbol = get_symbol_of_declaration(self, fn_);
                let getter = parent_symbol.and_then(|s| {
                    binder_shims::get_declaration_of_kind(self.program.symbol(s).declarations.clone(), Kind::GetAccessor)
                });
                if let Some(getter) = getter {
                    let getter_signature = self.get_signature_from_declaration(getter);
                    let this_parameter = self.get_accessor_this_parameter(fn_);
                    if let (Some(getter_signature), Some(this_parameter)) =
                        (getter_signature, this_parameter)
                    {
                        if declaration == this_parameter {
                            // Use the type from the *getter*
                            debug_assert!(
                                ast_shims::type_of_node(self.store(), this_parameter).is_none()
                            );
                            let getter_this = self.signatures[getter_signature.0 as usize]
                                .this_parameter
                                .expect("getter this parameter");
                            return Some(self.get_type_of_symbol(getter_this));
                        }
                    }
                    if let Some(getter_signature) = getter_signature {
                        // TODO(port M6 S8): getReturnTypeOfSignature — the
                        // resolved-return read; permissive errorType.
                        let _ = getter_signature;
                        return Some(self.error_type());
                    }
                }
            }
            if let Some(t) = self.get_parameter_type_of_full_signature(fn_, declaration) {
                return Some(t);
            }
            // Use contextual parameter type if one is available
            if let Some(t) = if self.program.symbol(symbol).name == "this" {
                self.get_contextual_this_parameter_type(fn_)
            } else {
                self.get_contextually_typed_parameter_type(declaration)
            } {
                return Some(self.add_optionality_ex(t, false /*isProperty*/, is_optional));
            }
        }
        // Use the type of the initializer expression if one is present and
        // the declaration is not a parameter of a contextually typed function
        if initializer.is_some() {
            let t = self.check_declaration_initializer(declaration, check_mode, None /*contextualType*/);
            let t = self.widen_type_inferred_from_initializer(declaration, t);
            return Some(self.add_optionality_ex(t, is_property, is_optional));
        }
        if self.no_implicit_any && kind == Kind::PropertyDeclaration {
            // We have a property declaration with no type annotation or
            // initializer, in noImplicitAny mode or a .js file. Use control
            // flow analysis of this.xxx assignments in the constructor or
            // static block to determine the type of the property.
            let modifier_flags = {
                let store = self.store();
                u::get_selected_modifier_flags(store.node(declaration), ModifierFlags::STATIC)
            };
            if !modifier_flags.intersects(ModifierFlags::STATIC) {
                let parent = self.store().node(declaration).parent.get();
                let constructor = binder_shims::find_constructor_declaration(self.store(), parent);
                let t = if let Some(constructor) = constructor {
                    self.get_flow_type_in_constructor(symbol, constructor)
                } else if {
                    let store = self.store();
                    u::get_selected_modifier_flags(store.node(declaration), ModifierFlags::AMBIENT)
                        .intersects(ModifierFlags::AMBIENT)
                } {
                    self.get_type_of_property_in_base_class(symbol)
                } else {
                    None
                };
                let Some(t) = t else {
                    return None;
                };
                return Some(self.add_optionality_ex(t, true /*isProperty*/, is_optional));
            } else {
                let parent = self.store().node(declaration).parent.get();
                let members = self.store().node(parent).as_class_declaration().and_then(|d| d.members.clone());
                let static_blocks: Vec<NodeId> = members
                    .iter()
                    .flat_map(|list| list.nodes.iter().copied())
                    .filter(|&m| self.store().node(m).kind == Kind::ClassStaticBlockDeclaration)
                    .collect();
                let t = if !static_blocks.is_empty() {
                    self.get_flow_type_in_static_blocks(symbol, static_blocks)
                } else if {
                    let store = self.store();
                    u::get_selected_modifier_flags(store.node(declaration), ModifierFlags::AMBIENT)
                        .intersects(ModifierFlags::AMBIENT)
                } {
                    self.get_type_of_property_in_base_class(symbol)
                } else {
                    None
                };
                let Some(t) = t else {
                    return None;
                };
                return Some(self.add_optionality_ex(t, true /*isProperty*/, is_optional));
            }
        }
        if kind == Kind::JsxAttribute {
            // if JSX attribute doesn't have initializer, by default the
            // attribute will have boolean value of true.
            // I.e <Elem attr /> is sugar for <Elem attr={true} />
            return Some(self.true_type());
        }
        // If the declaration specifies a binding pattern and is not a
        // parameter of a contextually typed function, use the type implied by
        // the binding pattern
        if matches!(name_kind, Some(Kind::ObjectBindingPattern) | Some(Kind::ArrayBindingPattern)) {
            let pattern = name.expect("binding pattern");
            return Some(self.get_type_from_binding_pattern(pattern, false /*includePatternInType*/, true /*reportErrors*/));
        }
        let _ = (name, name_kind);
        // No type specified and nothing can be inferred
        None
    }

    /// Go: `func (c *Checker) isNullOrUndefined(node *ast.Node) bool`.
    fn is_null_or_undefined(&self, node: NodeId) -> bool {
        let expr = ast_shims::walk_up_parenthesized_expressions(self.store(), node)
            .unwrap_or(node);
        let n = self.store().node(expr);
        if ast_shims::is_null_literal(n) {
            return true;
        }
        if tsc_ast::is_identifier(n) {
            return self.get_resolved_symbol(expr) == Some(self.undefined_symbol());
        }
        false
    }

    /// Go: `func (c *Checker) tryGetTypeFromTypeNode(node *ast.Node) *Type`.
    fn try_get_type_from_type_node(&mut self, declaration: NodeId) -> Option<TypeId> {
        // TODO(port M6 S5): getTypeFromTypeNode — the type-node family; the
        // stub's errorType keeps the annotation-present distinction.
        ast_shims::type_of_node(self.store(), declaration)
            .map(|node| self.get_type_from_type_node(node))
    }

    /// Go: `func (c *Checker) checkDeclarationInitializer(declaration, checkMode, contextualType) *Type`.
    fn check_declaration_initializer(
        &mut self,
        declaration: NodeId,
        check_mode: CheckMode,
        contextual_type: Option<TypeId>,
    ) -> TypeId {
        let initializer = ast_shims::initializer_of(self.store(), declaration)
            .expect("caller checked initializer != nil");
        let t = self.get_quick_type_of_expression(initializer);
        let t = match t {
            Some(t) => t,
            None => match contextual_type {
                // TODO(port M6 S9): checkExpressionWithContextualType's real
                // contextual path; the cached-check seam is S1's real body.
                Some(contextual_type) => {
                    self.check_expression_with_contextual_type(initializer, contextual_type, None, check_mode)
                }
                None => self.check_expression_cached_ex(initializer, check_mode),
            },
        };
        if binder_shims::get_root_declaration(self.store(), declaration)
            .is_some_and(|root| self.store().node(root).kind == Kind::Parameter)
        {
            let name = self.store().node(declaration).name();
            let name_kind = name.map(|n| self.store().node(n).kind);
            match name_kind {
                Some(Kind::ObjectBindingPattern) => {
                    if u::is_object_literal_type(self.type_(t)) {
                        return self.pad_object_literal_type(t, name.expect("name"));
                    }
                }
                Some(Kind::ArrayBindingPattern) => {
                    if self.is_tuple_type(t) {
                        return self.pad_tuple_type(t, name.expect("name"));
                    }
                }
                _ => {}
            }
        }
        t
    }

    /// Go: `func (c *Checker) padObjectLiteralType(t *Type, pattern *ast.Node) *Type`.
    fn pad_object_literal_type(&mut self, t: TypeId, pattern: NodeId) -> TypeId {
        let elements = binding_pattern_elements(self.store(), pattern);
        let mut missing_elements: Vec<NodeId> = Vec::new();
        for e in elements {
            if u::has_dot_dot_dot_token(self.store(), e) {
                continue;
            }
            let name = self.get_property_name_from_binding_element(e);
            if name != "__missing" && self.get_property_of_type(t, &name).is_none() {
                missing_elements.push(e);
            }
        }
        if missing_elements.is_empty() {
            return t;
        }
        let mut members = SymbolTable::new();
        // TODO(port M6 S8): getPropertiesOfObjectType — the member-resolution
        // wave; the permissive empty list keeps only the missing elements'
        // synthesized properties.
        for e in missing_elements {
            let name = self.get_property_name_from_binding_element(e);
            let symbol = self.new_symbol(
                SymbolFlags::PROPERTY.union(SymbolFlags::OPTIONAL),
                &name,
            );
            let element_type = self.get_type_from_binding_element(e, false /*includePatternInType*/, true /*reportErrors*/);
            self.value_symbol_links.get(symbol).resolved_type = Some(element_type);
            members.set(self.program.symbol(symbol).name.clone(), symbol);
        }
        let symbol = self.type_(t).symbol;
        let result = self.new_anonymous_type_with_members(symbol, members);
        let object_flags = self.type_(t).object_flags;
        self.type_mut(result).object_flags = object_flags;
        result
    }

    /// Go: `func (c *Checker) getPropertyNameFromBindingElement(e *ast.Node) string`.
    fn get_property_name_from_binding_element(&mut self, e: NodeId) -> String {
        let name = self
            .store()
            .node(e)
            .as_binding_element()
            .expect("BindingElement data")
            .property_name
            .or(self.store().node(e).name())
            .expect("PropertyNameOrName");
        let expr_type = self.get_literal_type_from_property_name(name);
        if u::is_type_usable_as_property_name(self.type_(expr_type)) {
            return u::get_property_name_from_type(self.type_(expr_type));
        }
        "__missing".to_string()
    }

    /// Go: `func (c *Checker) padTupleType(t *Type, pattern *ast.Node) *Type`.
    fn pad_tuple_type(&mut self, t: TypeId, pattern: NodeId) -> TypeId {
        let pattern_elements = binding_pattern_elements(self.store(), pattern);
        let target_tuple = {
            let ty = self.type_(t);
            ty.target()
                .map(|target| self.type_(target).as_tuple_type().is_some())
                .unwrap_or(false)
        };
        // TODO(port M6 S7): the tuple-target arity/element reads; the
        // permissive no-padding arm returns t unchanged until the tuple
        // constructor wave lands.
        if !target_tuple || self.get_type_reference_arity(t) as usize >= pattern_elements.len() {
            return t;
        }
        t
    }

    /// Go: `func (c *Checker) getLiteralTypeFromPropertyName(name *ast.Node) *Type`
    /// (checker.go:27303 — S2 uses it through the binding patterns; the body
    /// is this slice's).
    pub(crate) fn get_literal_type_from_property_name(&mut self, name: NodeId) -> TypeId {
        let node = self.store().node(name);
        if tsc_ast::is_private_identifier(node) {
            return self.never_type();
        }
        if node.kind == Kind::NumericLiteral {
            let t = self.check_expression(name);
            return self.get_regular_type_of_literal_type(t);
        }
        if node.kind == Kind::ComputedPropertyName {
            // TODO(port M6 S4): checkComputedPropertyName — the permissive
            // neverType matches Go's missing-name answer.
            return self.never_type();
        }
        let property_name = ast_shims::get_property_name_for_property_name_node(self.store(), name);
        if property_name != "__missing" {
            return self.get_string_literal_type(&property_name);
        }
        // Go: IsExpression(name) → checkExpression — identifiers inside
        // computed names; the neverType arm below is the fallback.
        self.never_type()
    }
}

/// The `Elements()` read shared by the binding-pattern arms (Go:
/// `pattern.Elements()` over the ObjectBindingPattern/ArrayBindingPattern
/// payload).
fn binding_pattern_elements(store: &dyn NodeStore, pattern: NodeId) -> Vec<NodeId> {
    store
        .node(pattern)
        .as_binding_pattern()
        .expect("BindingPattern data")
        .elements
        .clone()
        .map(|list| list.nodes)
        .unwrap_or_default()
}

/// Go: `ast.IsCatchClauseVariableDeclarationOrBindingElement`.
fn is_catch_clause_variable_declaration_or_binding_element(
    store: &dyn NodeStore,
    declaration: NodeId,
) -> bool {
    let node = binder_shims::get_root_declaration(store, declaration);
    store.node(node).kind == Kind::VariableDeclaration
        && store.node(store.node(node).parent.get()).kind == Kind::CatchClause
}
