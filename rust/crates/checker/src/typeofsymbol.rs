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

use rustc_hash::{FxHashMap, FxHashSet};

use tsc_ast::nodeflags::NodeFlags as NF;
use tsc_ast::{
    CheckFlags, Kind, ModifierFlags, Node, NodeData, NodeId, NodeStore, SymbolFlags, SymbolId,
    SymbolTable,
};
use tsc_diagnostics::Message;
use tsc_diagnostics::{
    AN_ENUM_MEMBER_CANNOT_HAVE_A_NUMERIC_NAME, BINDING_ELEMENT_0_IMPLICITLY_HAS_AN_1_TYPE,
    COMPUTED_PROPERTY_NAMES_ARE_NOT_ALLOWED_IN_ENUMS,
    ENUM_MEMBER_MUST_HAVE_INITIALIZER,
    FUNCTION_EXPRESSION_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_0_RETURN_TYPE,
    GENERATOR_IMPLICITLY_HAS_YIELD_TYPE_0_CONSIDER_SUPPLYING_A_RETURN_TYPE_ANNOTATION,
    MAPPED_OBJECT_TYPE_IMPLICITLY_HAS_AN_ANY_TEMPLATE_TYPE,
    MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE,
    MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE,
    PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    PROPERTY_0_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_ITS_GET_ACCESSOR_LACKS_A_RETURN_TYPE_ANNOTATION,
    REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE,
    REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    REST_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES,
    TYPE_0_IS_NOT_A_CONSTRUCTOR_FUNCTION_TYPE,
    TYPE_ALIAS_0_CIRCULARLY_REFERENCES_ITSELF,
    TYPE_OF_PROPERTY_0_CIRCULARLY_REFERENCES_ITSELF_IN_MAPPED_TYPE_1,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE,
    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    X_0_HAS_A_STRING_TYPE_BUT_MUST_HAVE_SYNTACTICALLY_RECOGNIZABLE_STRING_SYNTAX_WHEN_ISOLATEDMODULES_IS_ENABLED,
    X_0_IMPLICITLY_HAS_AN_1_RETURN_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE,
    X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_BASE_EXPRESSION,
    X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_TYPE_ANNOTATION,
    X_0_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_1_RETURN_TYPE,
};

use tsc_binder::ast_shims as binder_shims;
use tsc_binder::node_data as binder_node_data;

use crate::ast_shims;
use crate::checker::{CheckMode, TypeSystemEntity, TypeSystemPropertyName};
use crate::todo_shapes::{CacheHashKey, CachedTypeKind, CachedTypeKey, EvaluatorResult, EvaluatorValue, UnionReduction};
use crate::types::{
    self, AccessFlags, ElementFlags,
    LiteralValue, ObjectFlags, SignatureFlags, SignatureId, TupleElementInfo,
    TypeAlias, TypeData, TypeFlags, TypeId,
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
#[allow(dead_code)] // TODO(port M6 S7: getAliasKey (union interning)) consumes it
pub(crate) fn get_alias_key(alias: Option<&TypeAlias>) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_alias(alias);
    b.hash()
}

/// Go: `func getUnionKey(types []*Type, origin *Type, alias *TypeAlias) CacheHashKey`.
#[allow(dead_code)] // TODO(port M6 S7: getUnionKey) consumes it
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
#[allow(dead_code)] // TODO(port M6 S7: getIntersectionKey) consumes it
pub(crate) fn get_intersection_key(
    types: &[TypeId],
    flags: crate::relater::IntersectionFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let mut b = crate::relater::KeyBuilder::new();
    b.write_types(types);
    if !flags.intersects(crate::relater::IntersectionFlags::NO_CONSTRAINT_REDUCTION) {
        b.write_alias(alias);
    } else {
        b.write_byte(b'*');
    }
    b.hash()
}

/// Go: `func getTupleKey(elementInfos []TupleElementInfo, readonly bool) CacheHashKey`.
#[allow(dead_code)] // TODO(port M6 S7: getTupleKey) consumes it
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
            b.write_node_id(Some(labeled_declaration));
        }
    }
    if readonly {
        b.write_byte(b'!');
    }
    b.hash()
}

/// Go: `func getTypeAliasInstantiationKey(typeArguments []*Type, alias *TypeAlias) CacheHashKey`
/// — `getTypeInstantiationKey(typeArguments, alias, false)`.
#[allow(dead_code)] // TODO(port M6 S7: alias instantiation cache) consumes it
pub(crate) fn get_type_alias_instantiation_key(
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    get_type_instantiation_key(type_arguments, alias, false)
}

/// Go: `func getTypeInstantiationKey(typeArguments []*Type, alias *TypeAlias, singleSignature bool) CacheHashKey`.
#[allow(dead_code)] // TODO(port M6 S7: instantiation cache) consumes it
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
#[allow(dead_code)] // TODO(port M6 S7/S10: indexed-access cache) consumes it
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
#[allow(dead_code)] // TODO(port M6 S7/S10: template-literal cache) consumes it
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
#[allow(dead_code)] // TODO(port M6 S7/S10: conditional instantiation cache) consumes it
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
    for n in nodes {
        b.write_node_id(Some(*n));
    }
    b.hash()
}

/// Go: `func isUnconstrainedTypeParameter(tp *Type) bool` — true when the type
/// parameter originates in an unconstrained declaration in a type parameter list.
#[allow(dead_code)] // TODO(port M6 S9: getBaseConstraintOfTypeParameter) consumes it
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
///
/// PORT: Go's map key is the interface value — IEEE `==` semantics (`-0.0 ==
/// 0.0` one key; NaN never a key — it caches separately by enum symbol).
/// [`LiteralValue`] can't derive `Eq`/`Hash` (`f64`), so the key stores the
/// normalized form: the number case canonicalizes to
/// [`LiteralValue::number_key`] (0.0 for -0.0), the string case the text.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EnumLiteralKey {
    pub enum_symbol: SymbolId,
    /// The value's map-key form (Go's IEEE-equality key).
    pub value: EnumLiteralKeyValue,
}

/// The map-key form of Go's `any` enum value.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum EnumLiteralKeyValue {
    Number(u64),
    String(String),
}

impl EnumLiteralKey {
    /// Builds the key from the raw literal value (`number_key` canonicalizes).
    pub(crate) fn new(enum_symbol: SymbolId, value: &LiteralValue) -> Self {
        let value = match value {
            LiteralValue::Number(_) => EnumLiteralKeyValue::Number(value.number_key()),
            LiteralValue::String(s) => EnumLiteralKeyValue::String(s.clone()),
            _ => panic!("Unhandled case in getEnumLiteralType key"),
        };
        EnumLiteralKey { enum_symbol, value }
    }
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
#[allow(dead_code)] // TODO(port M6 S9): the S4 object-literal + S9 sibling
// property waves read the tree fields (resolved_properties/child_contexts).
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

// TODO(port M6 S8/S9/S10): isFunctionType / getConstraintOfType /
// getConstraintOfIndexedAccess / getConstraintOfDistributiveConditionalType and
// the other forward-landed bodies in this impl are consumed by those waves
// (the relation probers, the deferred-constraint walkers); the current slice's
// call sites reach them only through the bannered stubs.
#[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) getSymbolFlagsEx(symbol, excludeTypeOnlyMeanings, excludeLocalMeanings) ast.SymbolFlags`.
    pub(crate) fn get_symbol_flags_ex(
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
            let resolved = self.resolve_alias(symbol);
            let target = self.get_export_symbol_of_value_symbol_if_exported(resolved);
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
            .rfind(|&d| is_alias_symbol_declaration(self.store(), d))
    }

    /// Go: `func (c *Checker) getTypeOfSymbolWithDeferredType(symbol *ast.Symbol) *Type`.
    fn get_type_of_symbol_with_deferred_type(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let (parent, constituents) = {
                let deferred = self.deferred_symbol_links.get(symbol);
                (deferred.parent, deferred.constituents.clone())
            };
            let parent_flags =
                parent.map(|p| self.type_(p).flags).unwrap_or(TypeFlags::NONE);
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
            let (parent, write_constituents) = {
                let deferred = self.deferred_symbol_links.get(symbol);
                (deferred.parent, deferred.write_constituents.clone())
            };
            let parent_flags =
                parent.map(|p| self.type_(p).flags).unwrap_or(TypeFlags::NONE);
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
            && u::is_shorthand_ambient_module_symbol(self, self.store(), symbol)
        {
            return self.any_type();
        } else if flags.intersects(SymbolFlags::VALUE_MODULE)
            && value_declaration.is_some_and(|d| {
                let store = self.store();
                let node = store.node(d);
                node.kind == Kind::SourceFile
                    && node
                        .as_source_file()
                        .is_some_and(|f| f.common_js_module_indicator.get().is_some())
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
    /// (The `_impl` suffix keeps the pub-API surface's Get* twin in
    /// exports.rs the sole `get_base_constructor_type_of_class` name.)
    pub(crate) fn get_base_constructor_type_of_class_impl(&mut self, t: TypeId) -> TypeId {
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
                let symbol_name1 = self.symbol_to_string(symbol);
            self.error(
                expression,
                &X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_BASE_EXPRESSION,
                &[symbol_name1],
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
            let type_string = self.type_to_string(base_constructor_type);
            self.error(
                expression,
                &TYPE_0_IS_NOT_A_CONSTRUCTOR_FUNCTION_TYPE,
                &[type_string],
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
    /// (The `_impl` suffix keeps the pub-API surface's Get* twin in exports.rs
    /// the sole `get_constraint_of_type_parameter` method name.)
    pub(crate) fn get_constraint_of_type_parameter_impl(&mut self, type_parameter: TypeId) -> Option<TypeId> {
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
    /// Go: `func (c *Checker) is_error_type(t *Type) bool` — `t == c.errorType`.
    fn is_error_type(&self, t: TypeId) -> bool {
        t == self.error_type()
    }

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
        let false_constraint = self.get_false_type_from_conditional_type_impl(t);
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
    /// resolved-branch read — the `_impl` name keeps the pub twin in
    /// exports.rs the sole public one).
    fn get_false_type_from_conditional_type_impl(&mut self, t: TypeId) -> TypeId {
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
            // Go: `node.AsClassLikeDeclaration().HeritageClauses()` — both
            // payloads carry the same heritage-clause field.
            let heritage = if node.kind == Kind::ClassDeclaration {
                node.as_class_declaration()
                    .expect("ClassDeclaration data")
                    .heritage_clauses
                    .clone()
            } else {
                node.as_class_expression()
                    .expect("ClassExpression data")
                    .heritage_clauses
                    .clone()
            };
            for clause in heritage.iter().flat_map(|list| list.nodes.iter().copied()) {
                if store.node(clause).kind == Kind::HeritageClause
                    && store.node(clause).as_heritage_clause().expect("HeritageClause data").token == Kind::ExtendsKeyword
                {
                    let types = store.node(clause).as_heritage_clause().expect("HeritageClause data").types.clone();
                    return types.as_ref().and_then(|list| list.nodes.first().copied());
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
        NodeData::ParameterDeclaration(d) => d.symbol,
        NodeData::BindingElement(d) => d.symbol,
        NodeData::PropertyDeclaration(d) => d.symbol,
        NodeData::PropertySignatureDeclaration(d) => d.symbol,
        NodeData::PropertyAssignment(d) => d.symbol,
        NodeData::ShorthandPropertyAssignment(d) => d.symbol,
        NodeData::MethodDeclaration(d) => d.symbol,
        NodeData::MethodSignatureDeclaration(d) => d.symbol,
        NodeData::GetAccessorDeclaration(d) => d.symbol,
        NodeData::SetAccessorDeclaration(d) => d.symbol,
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
        NodeData::TypeParameterDeclaration(d) => d.symbol,
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

    // TODO(port M6 S4/S5: the accessor + contextual waves): the bannered permissive/stub bodies below
    // are forward-landed; their consumers arrive with those waves.
    #[allow(dead_code)]
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

    // TODO(port M6 S7/S10: removeMissingType’s real body + getNonUndefinedType’s
    // callers): the bannered permissive/stub bodies below are forward-landed;
    // their consumers arrive with those waves.
    #[allow(dead_code)]
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
    pub(crate) fn get_signature_from_declaration(&mut self, declaration: NodeId) -> Option<SignatureId> {
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
    pub(crate) fn has_bindable_name(&mut self, node: NodeId) -> bool {
        // TODO(port M6 S4/S6): the hasLateBindableName half (the computed
        // name's literal-type check); the early-bound half is real.
        !binder_shims::has_dynamic_name(self.store(), node)
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

    /// `checkRightHandSideOfForOf` (checker.go:18081) — REPLACED by the
    /// M6 S3 real body (statements.rs); this wrapper keeps the S2 call
    /// sites' non-optional TypeId shape (Go's nil iterated type maps to the
    /// errorType answer the S2 spine used).
    pub(crate) fn check_right_hand_side_of_for_of(&mut self, statement: NodeId) -> TypeId {
        match self.check_right_hand_side_of_for_of_real(statement) {
            Some(t) => t,
            None => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) getExtractStringType(t *Type) *Type` (S10) —
    /// TODO(port M6 S10): the mapped-type/string-literal extraction; the
    /// identity keeps the for-in arm's stringType answer dominant.
    pub(crate) fn get_extract_string_type(&mut self, t: TypeId) -> TypeId {
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
                    NodeData::PropertyAssignment(d) => Some(d.initializer),
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
            let symbol = payload_symbol(self.store().node(declaration))?;
            // parameters of function types defined in JSDoc in TS files don't
            // have symbols
            let fn_ = self.store().node(declaration).parent.get();
            if self.store().node(fn_).kind == Kind::SetAccessor && self.has_bindable_name(fn_) {
                let parent_symbol = get_symbol_of_declaration(self, fn_);
                let getter = parent_symbol.and_then(|s| {
                    find_declaration_of_kind(self, &self.program.symbol(s).declarations, Kind::GetAccessor)
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
            let symbol = self
                .get_symbol_of_declaration(declaration)
                .expect("property symbol");
            let modifier_flags = {
                let store = self.store();
                u::get_selected_modifier_flags(store.node(declaration), ModifierFlags::STATIC)
            };
            let is_ambient = {
                let store = self.store();
                u::get_selected_modifier_flags(store.node(declaration), ModifierFlags::AMBIENT)
                    .intersects(ModifierFlags::AMBIENT)
            };
            if !modifier_flags.intersects(ModifierFlags::STATIC) {
                let parent = self.store().node(declaration).parent.get();
                let constructor = binder_shims::find_constructor_declaration(self.store(), parent);
                let t = if let Some(constructor) = constructor {
                    self.get_flow_type_in_constructor(symbol, constructor)
                } else if is_ambient {
                    self.get_type_of_property_in_base_class(symbol)
                } else {
                    None
                };
                let t = t?;
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
                } else if is_ambient {
                    self.get_type_of_property_in_base_class(symbol)
                } else {
                    None
                };
                let t = t?;
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

    /// `isNullOrUndefined` (checker.go:18070) — REPLACED by the M6 S3 real
    /// body (statements.rs); kept as a thin alias for the S2 call sites.
    /// PORT note: the S2 stub skipped only *parenthesized* wrappers
    /// (walk_up_parenthesized_expressions); Go's `SkipParentheses` and S3's
    /// real body use the full skip.
    fn is_null_or_undefined(&mut self, node: NodeId) -> bool {
        self.is_null_or_undefined_real(node)
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
                    self.check_expression_with_contextual_type(initializer, Some(contextual_type), None, check_mode)
                }
                None => self.check_expression_cached_ex(initializer, check_mode),
            },
        };
        let root = binder_shims::get_root_declaration(self.store(), declaration);
        if self.store().node(root).kind == Kind::Parameter {
            let name = self.store().node(declaration).name();
            let name_kind = name.map(|n| self.store().node(n).kind);
            // The arms' `if` guards need &mut self (the tuple/object checks),
            // so they cannot be match guards — collapsible_match is
            // structurally barred here.
            #[allow(clippy::collapsible_match)]
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
        // constructor wave lands (getTypeReferenceTypeArguments' arity read
        // is S7's — the permissive 0 keeps the arm's `>=` false).
        if !target_tuple {
            return t;
        }
        let _ = &pattern_elements;
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
        .map(|list| list.nodes.to_vec())
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

// ────────────────────────────────────────────────────────────────────────────
// Binding-element typing (Go checker.go 18087–18461)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) hasTypeFacts(t *Type, facts TypeFacts) bool` —
    /// `(c.getTypeFacts(t, facts) & facts) == facts` over relater's stub.
    pub(crate) fn has_type_facts(&mut self, t: TypeId, facts: crate::relater::TypeFacts) -> bool {
        let computed = self.get_type_facts(t, facts);
        computed.contains(facts)
    }

    /// Return the inferred type for a binding element.
    ///
    /// Go: `func (c *Checker) getTypeForBindingElement(declaration *ast.Node) *Type`.
    fn get_type_for_binding_element(&mut self, declaration: NodeId) -> Option<TypeId> {
        let check_mode = if u::has_dot_dot_dot_token(self.store(), declaration) {
            CheckMode::REST_BINDING_ELEMENT
        } else {
            CheckMode::CHECK_MODE_NORMAL
        };
        let parent = self
            .store()
            .node(self.store().node(declaration).parent.get())
            .parent
            .get();
        let parent_type = self.get_type_for_binding_element_parent(parent, check_mode);
        if let Some(parent_type) = parent_type {
            return Some(self.get_binding_element_type_from_parent_type(
                declaration,
                parent_type,
                false, /*noTupleBoundsCheck*/
            ));
        }
        None
    }

    /// Return the type of a binding element parent. We check SymbolLinks
    /// first to see if a type has been assigned by contextual typing.
    ///
    /// Go: `func (c *Checker) getTypeForBindingElementParent(node, checkMode) *Type`.
    pub(crate) fn get_type_for_binding_element_parent(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
    ) -> Option<TypeId> {
        if check_mode == CheckMode::CHECK_MODE_NORMAL {
            // We can use a cached resolved type if no optionality was
            // included in that type.
            if let Some(symbol) = get_symbol_of_declaration(self, node) {
                let resolved_type = self.value_symbol_links.get(symbol).resolved_type;
                if let Some(resolved_type) = resolved_type {
                    if !(self.strict_null_checks && u::is_optional_declaration(self.store(), node)) {
                        return Some(resolved_type);
                    }
                }
            }
        }
        self.get_type_for_variable_like_declaration(node, false /*includeOptionality*/, check_mode)
    }

    /// Go: `func (c *Checker) getBindingElementTypeFromParentType(declaration, parentType, noTupleBoundsCheck) *Type`.
    fn get_binding_element_type_from_parent_type(
        &mut self,
        declaration: NodeId,
        mut parent_type: TypeId,
        no_tuple_bounds_check: bool,
    ) -> TypeId {
        // If an any type was inferred for parent, infer that for the binding
        // element
        if u::is_type_any(Some(self.type_(parent_type))) {
            return parent_type;
        }
        let pattern = self.store().node(declaration).parent.get();
        let declaration_flags = self.store().node(declaration).flags;
        // Relax null check on ambient destructuring parameters, since the
        // parameters have no implementation and are just documentation
        // TODO(port M6 S9/S10): the non-nullable facts adjustment arms
        // (GetNonNullableType / getTypeWithFacts) — the facts stub makes
        // both arms no-ops, preserving parentType unchanged.
        if self.strict_null_checks
            && declaration_flags.intersects(NF::AMBIENT)
            && binder_shims::is_part_of_parameter_declaration(self.store(), declaration)
        {
            parent_type = self.get_non_nullable_type(parent_type);
        } else if self.strict_null_checks
            && ast_shims::initializer_of(
                self.store(),
                self.store().node(pattern).parent.get(),
            )
            .is_some()
        {
            let pattern_parent = self.store().node(pattern).parent.get();
            let initializer = ast_shims::initializer_of(self.store(), pattern_parent).expect("checked");
            let initializer_type = self.get_type_of_initializer(initializer);
            let _ = self.has_type_facts(
                initializer_type,
                crate::relater::TypeFacts::EQ_UNDEFINED,
            );
        }
        let access_flags = AccessFlags::EXPRESSION_POSITION
            | if no_tuple_bounds_check || self.has_default_value(declaration) {
                AccessFlags::ALLOW_MISSING
            } else {
                AccessFlags::NONE
            };
        let t;
        let pattern_kind = self.store().node(pattern).kind;
        match pattern_kind {
            Kind::ObjectBindingPattern => {
                if u::has_dot_dot_dot_token(self.store(), declaration) {
                    let reduced = crate::relater::get_reduced_type(self, parent_type);
                    parent_type = reduced;
                    if self.type_(parent_type).flags.intersects(TypeFlags::UNKNOWN)
                        || !self.is_valid_spread_type(parent_type)
                    {
                        self.error(declaration, &REST_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES, &[]);
                        return self.error_type();
                    }
                    let elements = binding_pattern_elements(self.store(), pattern);
                    let mut literal_members: Vec<NodeId> = Vec::with_capacity(elements.len());
                    for element in elements {
                        if !u::has_dot_dot_dot_token(self.store(), element) {
                            let name = self
                                .store()
                                .node(element)
                                .as_binding_element()
                                .expect("BindingElement data")
                                .property_name
                                .or(self.store().node(element).name())
                                .expect("PropertyNameOrName");
                            literal_members.push(name);
                        }
                    }
                    let symbol = payload_symbol(self.store().node(declaration));
                    t = self.get_rest_type(parent_type, &literal_members, symbol);
                } else {
                    // Use explicitly specified property name ({ p: xxx }
                    // form), or otherwise the implied name ({ p } form)
                    let name = self
                        .store()
                        .node(declaration)
                        .as_binding_element()
                        .expect("BindingElement data")
                        .property_name
                        .or(self.store().node(declaration).name())
                        .expect("PropertyNameOrName");
                    let index_type = self.get_literal_type_from_property_name(name);
                    let declared_type = self.get_indexed_access_type_ex(
                        parent_type,
                        index_type,
                        access_flags,
                        Some(name),
                    );
                    t = self.get_flow_type_of_destructuring(declaration, declared_type);
                }
            }
            Kind::ArrayBindingPattern => {
                // This elementType will be used if the specific property
                // corresponding to this index is not present (aka the tuple
                // element property). This call also checks that the parentType
                // is in fact an iterable or array (depending on target
                // language).
                // TODO(port M6 S3): checkIteratedTypeOrElementType — the
                // destructuring iterated-type machinery; permissive errorType.
                let element_type = self.error_type();
                let index = binding_pattern_elements(self.store(), pattern)
                    .iter()
                    .position(|&e| e == declaration)
                    .expect("declaration is an element of its parent pattern");
                if u::has_dot_dot_dot_token(self.store(), declaration) {
                    // If the parent is a tuple type, the rest element has a
                    // tuple type of the remaining tuple element types.
                    // Otherwise, the rest element has an array type with
                    // same element type as the parent type.
                    // TODO(port M6 S7): the mapType base-constraint walk +
                    // everyType(baseConstraint, isTupleType) + sliceTupleType
                    // — the permissive array-type identity stub keeps the
                    // element type.
                    t = self.create_array_type(element_type);
                } else if self.is_array_like_type(parent_type) {
                    let index_type = self.get_number_literal_type(tsc_jsnum::Number(index as f64));
                    let declared_type = crate::relater::get_indexed_access_type_or_undefined_writing(
                        self,
                        parent_type,
                        index_type,
                        access_flags.intersects(AccessFlags::NO_INDEX_SIGNATURES),
                    )
                    .unwrap_or_else(|| self.error_type());
                    t = self.get_flow_type_of_destructuring(declaration, declared_type);
                } else {
                    t = element_type;
                }
            }
            _ => panic!("Unhandled case in getBindingElementTypeFromParentType"),
        }
        if ast_shims::initializer_of(self.store(), declaration).is_none() {
            return t;
        }
        if ast_shims::type_of_node(
            self.store(),
            ast_shims::walk_up_binding_elements_and_patterns(self.store(), declaration),
        )
        .is_some()
        {
            // In strict null checking mode, if a default value of a
            // non-undefined type is specified, remove undefined from the
            // final type.
            // TODO(port M6 S10): the TypeFactsIsUndefined probe — the facts
            // stub keeps t unchanged (Go strips undefined when the default
            // is non-undefined).
            return t;
        }
        let initializer_type = self.check_declaration_initializer(
            declaration,
            CheckMode::CHECK_MODE_NORMAL,
            None, /*contextualType*/
        );
        // TODO(port M6 S7): getUnionTypeEx(…, UnionReductionSubtype, …) —
        // the permissive union stub.
        let _ = initializer_type;
        self.widen_type_inferred_from_initializer(declaration, t)
    }

    /// Go: `func (c *Checker) getRestType(source *Type, properties []*ast.Node, symbol *ast.Symbol) *Type`.
    fn get_rest_type(
        &mut self,
        source: TypeId,
        properties: &[NodeId],
        symbol: Option<SymbolId>,
    ) -> TypeId {
        // TODO(port M6 S4/S7/S8): the spread/omit-key construction
        // (filterType, getUnionType of the omitted literal types,
        // getSpreadSymbol, isSpreadableProperty, getGlobalOmitSymbol,
        // getTypeAliasInstantiation) — the permissive stub returns the
        // source's non-nullable form (Go's answer for a spread with no
        // omissible properties).
        let _ = (properties, symbol);
        crate::relater::get_non_nullable_type(self, source)
    }

    /// TODO(port M6 S4): `isValidSpreadType` — permissive true.
    fn is_valid_spread_type(&mut self, t: TypeId) -> bool {
        let _ = t;
        true
    }

    /// Determine the control flow type associated with a destructuring
    /// declaration or assignment.
    ///
    /// Go: `func (c *Checker) getFlowTypeOfDestructuring(node, declaredType) *Type`.
    fn get_flow_type_of_destructuring(&mut self, node: NodeId, declared_type: TypeId) -> TypeId {
        // Go: getSyntheticElementAccess builds a synthetic element access so
        // the flow analyzer doesn't have to handle the destructuring syntaxes,
        // then narrows through getFlowTypeOfReference.
        // TODO(port M6 W3): the synthetic node factory + flow narrowing —
        // permissive declaredType (Go's answer when the reference has no flow
        // node).
        let _ = node;
        declared_type
    }

    /// Return the type implied by a binding pattern. This is the type
    /// implied purely by the binding pattern itself and without regard to
    /// its context (i.e. without regard any type annotation or initializer
    /// associated with the declaration in which the binding pattern is
    /// contained). For example, the implied type of [x, y] is [any, any]
    /// and the implied type of { x, y: z = 1 } is { x: any; y: number; }.
    /// The type implied by a binding pattern is used as the contextual type
    /// of an initializer associated with the binding pattern. Also, for a
    /// destructuring parameter with no type annotation or initializer, the
    /// type implied by the binding pattern becomes the type of the parameter.
    ///
    /// Go: `func (c *Checker) getTypeFromBindingPattern(pattern, includePatternInType, reportErrors) *Type`.
    fn get_type_from_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        if include_pattern_in_type {
            self.contextual_binding_patterns.push(pattern);
        }
        let result = if self.store().node(pattern).kind == Kind::ObjectBindingPattern {
            self.get_type_from_object_binding_pattern(pattern, include_pattern_in_type, report_errors)
        } else {
            self.get_type_from_array_binding_pattern(pattern, include_pattern_in_type, report_errors)
        };
        if include_pattern_in_type {
            self.contextual_binding_patterns.pop();
        }
        result
    }

    /// Return the type implied by an object binding pattern.
    ///
    /// Go: `func (c *Checker) getTypeFromObjectBindingPattern(pattern, includePatternInType, reportErrors) *Type`.
    fn get_type_from_object_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        let mut members = SymbolTable::new();
        let mut string_index_info: Option<usize> = None;
        let mut object_flags =
            ObjectFlags::OBJECT_LITERAL.union(ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL);
        let elements = binding_pattern_elements(self.store(), pattern);
        for e in elements {
            let name = self
                .store()
                .node(e)
                .as_binding_element()
                .expect("BindingElement data")
                .property_name
                .or(self.store().node(e).name())
                .expect("PropertyNameOrName");
            if u::has_dot_dot_dot_token(self.store(), e) {
                let string_type = self.string_type();
                let any_type = self.any_type();
                string_index_info =
                    Some(self.new_index_info(string_type, any_type, false, None, Vec::new()));
                continue;
            }
            let expr_type = self.get_literal_type_from_property_name(name);
            if !u::is_type_usable_as_property_name(self.type_(expr_type)) {
                // do not include computed properties in the implied type
                object_flags |= ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES;
                continue;
            }
            let text = u::get_property_name_from_type(self.type_(expr_type));
            let initializer = ast_shims::initializer_of(self.store(), e);
            let flags = SymbolFlags::PROPERTY.union(if initializer.is_some() {
                SymbolFlags::OPTIONAL
            } else {
                SymbolFlags::NONE
            });
            let symbol = self.new_symbol(flags, &text);
            let resolved = self.get_type_from_binding_element(e, include_pattern_in_type, report_errors);
            self.value_symbol_links.get(symbol).resolved_type = Some(resolved);
            members.set(self.program.symbol(symbol).name.clone(), symbol);
        }
        // PORT: Go hands newAnonymousType the indexInfos list; the port's
        // member-carrying constructor fills the members table and the index
        // infos slot below (the S8 members-resolution seam). The info clone
        // happens before the `type_mut` borrow (§5.4 idiom).
        let string_index_info_data = string_index_info.map(|index| self.index_infos[index].clone());
        let result = self.new_anonymous_type_with_members(None, members);
        {
            let data = self
                .type_mut(result)
                .as_object_type_mut()
                .expect("ObjectType data");
            if let Some(info) = string_index_info_data {
                data.base.index_infos.push(Box::new(info));
            }
        }
        self.type_mut(result).object_flags |= object_flags;
        if include_pattern_in_type {
            self.pattern_for_type.insert(result, pattern);
            self.type_mut(result).object_flags |=
                ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        }
        result
    }

    /// Return the type implied by an array binding pattern.
    ///
    /// Go: `func (c *Checker) getTypeFromArrayBindingPattern(pattern, includePatternInType, reportErrors) *Type`.
    fn get_type_from_array_binding_pattern(
        &mut self,
        pattern: NodeId,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        let elements = binding_pattern_elements(self.store(), pattern);
        let last_element = elements.last().copied();
        let rest_element = last_element.filter(|&last| {
            self.store().node(last).kind == Kind::BindingElement
                && u::has_dot_dot_dot_token(self.store(), last)
        });
        if elements.is_empty() || (elements.len() == 1 && rest_element.is_some()) {
            // TODO: remove ScriptTargetES2015
            if self.language_version >= tsc_core::options_generated::ScriptTarget::ES2015 {
                // TODO(port M6 S7): createIterableType(c.anyType) — the
                // iterable construction; permissive anyArrayType (Go's
                // pre-ES2015 answer).
                return self.any_array_type;
            }
            return self.any_array_type;
        }
        let min_length = elements
            .iter()
            .rposition(|&e| {
                !(Some(e) == rest_element
                    || self.store().node(e).name().is_none()
                    || self.has_default_value(e))
            })
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut element_types: Vec<TypeId> = Vec::with_capacity(elements.len());
        let mut element_infos: Vec<TupleElementInfo> = Vec::with_capacity(elements.len());
        for (i, &e) in elements.iter().enumerate() {
            let t = if self.store().node(e).name().is_none() {
                self.any_type()
            } else {
                self.get_type_from_binding_element(e, include_pattern_in_type, report_errors)
            };
            let flags = if Some(e) == rest_element {
                ElementFlags::REST
            } else if i >= min_length {
                ElementFlags::OPTIONAL
            } else {
                ElementFlags::REQUIRED
            };
            element_types.push(t);
            element_infos.push(TupleElementInfo { flags, labeled_declaration: None });
        }
        // TODO(port M6 S7): createTupleTypeEx / cloneTypeReference — the
        // tuple constructor; the permissive stub returns the first element
        // type (documented S7 seam).
        let result = self.create_tuple_type_ex(&element_types, &element_infos, false);
        if include_pattern_in_type {
            // TODO(port M6 S7): cloneTypeReference(result) + the
            // patternForType registration.
            self.pattern_for_type.insert(result, pattern);
            self.type_mut(result).object_flags |=
                ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        }
        result
    }

    /// Return the type implied by a binding pattern element. This is the type
    /// of the initializer of the element if one is present. Otherwise, if the
    /// element is itself a binding pattern, it is the type implied by the
    /// binding pattern. Otherwise, it is the type any.
    ///
    /// Go: `func (c *Checker) getTypeFromBindingElement(element, includePatternInType, reportErrors) *Type`.
    fn get_type_from_binding_element(
        &mut self,
        element: NodeId,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        let name = self.store().node(element).name();
        let name_kind = name.map(|n| self.store().node(n).kind);
        if ast_shims::initializer_of(self.store(), element).is_some() {
            // The type implied by a binding pattern is independent of
            // context, so we check the initializer with no contextual type
            // or, if the element itself is a binding pattern, with the type
            // implied by that binding pattern.
            let contextual_type = if matches!(
                name_kind,
                Some(Kind::ObjectBindingPattern) | Some(Kind::ArrayBindingPattern)
            ) {
                Some(self.get_type_from_binding_pattern(
                    name.expect("binding pattern"),
                    true,  /*includePatternInType*/
                    false, /*reportErrors*/
                ))
            } else {
                Some(self.unknown_type())
            };
            let t = self.check_declaration_initializer(element, CheckMode::CHECK_MODE_NORMAL, contextual_type);
            let t = self.get_widened_literal_type_for_initializer(element, t);
            return self.add_optionality(t);
        }
        if matches!(
            name_kind,
            Some(Kind::ObjectBindingPattern) | Some(Kind::ArrayBindingPattern)
        ) {
            return self.get_type_from_binding_pattern(
                name.expect("binding pattern"),
                include_pattern_in_type,
                report_errors,
            );
        }
        if report_errors && !u::declaration_belongs_to_private_ambient_member(self.store(), element) {
            self.report_implicit_any(element, self.any_type(), WideningKind::Normal);
        }
        // When we're including the pattern in the type (an indication we're
        // obtaining a contextual type), we use a non-inferrable any type.
        // Inference will never directly infer this type, but it is possible
        // to infer a type that contains it, e.g. for a binding pattern like
        // [foo] or { foo }. In such cases, widening of the binding pattern
        // type substitutes a regular any for the non-inferrable any.
        if include_pattern_in_type {
            return self.non_inferrable_any_type();
        }
        self.any_type()
    }

    /// Go: `func (c *Checker) hasDefaultValue(node *ast.Node) bool` (checker.go:13862).
    fn has_default_value(&mut self, node: NodeId) -> bool {
        let store = self.store();
        let n = store.node(node);
        if n.kind == Kind::BindingElement {
            return ast_shims::initializer_of(store, node).is_some();
        }
        if n.kind == Kind::PropertyAssignment {
            return ast_shims::initializer_of(store, node)
                .is_some_and(|init| self.has_default_value(init));
        }
        if n.kind == Kind::ShorthandPropertyAssignment {
            return n
                .as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment data")
                .object_assignment_initializer
                .is_some();
        }
        if n.kind == Kind::BinaryExpression {
            let token = n
                .as_binary_expression()
                .expect("BinaryExpression data")
                .operator_token;
            return store.node(token).kind == Kind::EqualsToken;
        }
        false
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The this-assignment family (Go checker.go 18466–18644 — the JS expando path)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getWidenedTypeForAssignmentDeclaration(symbol) *Type`.
    fn get_widened_type_for_assignment_declaration(&mut self, symbol: SymbolId) -> TypeId {
        // TODO(port M6 S4): isConstructorDeclaredThisProperty's control-flow
        // and base-class arms + getAssignmentDeclarationInitializerType's JS
        // expando paths — the permissive anyType is Go's fallback answer.
        let kind = self.is_constructor_declared_this_property(symbol).0;
        let mut t: Option<TypeId> = None;
        if kind == ThisAssignmentDeclarationKind::Typed {
            // TODO(port M6 S5): getTypeFromTypeNode(location).
            t = None;
        }
        let _ = &mut t;
        let mut types: Vec<TypeId> = Vec::new();
        // We ignore initial assignments of undefined to CommonJS exports when
        // there are multiple assignment declarations
        let _ = &mut types;
        let t = t.unwrap_or_else(|| self.any_type());
        // TODO(port M6 S7): getUnionType(types) — the permissive stub keeps
        // anyType.
        self.get_widened_type(t)
    }

    /// Go: `func (c *Checker) isConstructorDeclaredThisProperty(symbol) (kind, location)`.
    fn is_constructor_declared_this_property(
        &mut self,
        symbol: SymbolId,
    ) -> (ThisAssignmentDeclarationKind, Option<NodeId>) {
        let value_declaration = self.program.symbol(symbol).value_declaration;
        if value_declaration.is_none()
            || self.store().node(value_declaration.expect("checked")).kind != Kind::BinaryExpression
        {
            return (ThisAssignmentDeclarationKind::None, None);
        }
        if let Some(kind) = self.this_expando_kinds.get(&symbol).copied() {
            let location = self
                .this_expando_locations
                .get(&symbol)
                .copied()
                .expect("location should be cached whenever this expando symbol is cached");
            return (kind, location);
        }
        // TODO(port M6 S4): the allThis/typeAnnotation scan + the expando
        // location resolution (getDeclaringConstructor) — permissive None/None
        // (the JS expando classification).
        let (kind, location): (ThisAssignmentDeclarationKind, Option<NodeId>) =
            (ThisAssignmentDeclarationKind::None, None);
        self.this_expando_kinds.insert(symbol, kind);
        self.this_expando_locations.insert(symbol, location);
        (kind, location)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// widen* helpers (Go checker.go 18645–18834, 17269–17288)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type WideningKind int32` — the implicit-any report flavor (iota order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum WideningKind {
    Normal = 0,
    GeneratorYield = 1,
}

    // TODO(port M6 S8/S9: getPropertiesOfObjectType’s widened walk): the bannered permissive/stub bodies below
    // are forward-landed; their consumers arrive with those waves.
    #[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) widenTypeForVariableLikeDeclaration(t, declaration, reportErrors) *Type`.
    pub(crate) fn widen_type_for_variable_like_declaration(
        &mut self,
        t: Option<TypeId>,
        declaration: NodeId,
        report_errors: bool,
    ) -> TypeId {
        let Some(mut t) = t else {
            // Rest parameters default to type any[], other parameters
            // default to type any
            let is_rest_parameter = self.store().node(declaration).kind == Kind::Parameter
                && self
                    .store()
                    .node(declaration)
                    .as_parameter_declaration()
                    .is_some_and(|p| p.dot_dot_dot_token.is_some());
            let t = if is_rest_parameter {
                self.any_array_type
            } else {
                self.any_type()
            };
            // Report implicit any errors unless this is a private property
            // within an ambient declaration
            if report_errors
                && !u::declaration_belongs_to_private_ambient_member(self.store(), declaration)
            {
                self.report_implicit_any(declaration, t, WideningKind::Normal);
            }
            return t;
        };
        // This special case is required for backwards compatibility with
        // libraries that merge a `symbol` property into `SymbolConstructor`.
        // See https://github.com/microsoft/TypeScript/tsc/issues/1212
        // TODO(port M6 S6): isGlobalSymbolConstructor(declaration.Parent)
        // — the global SymbolConstructor merge check; dormant until the
        // global resolution wave.
        let parent_kind = self.store().node(self.store().node(declaration).parent.get()).kind;
        let _ = parent_kind;

        if report_errors {
            self.report_errors_from_widening(declaration, t);
        }

        // always widen a 'unique symbol' type if the type was created for a
        // different declaration.
        // TODO(port M6 S4): the unique-symbol binding-element arm (needs
        // getSymbolOfDeclaration's late-bound machinery).
        let _ = &mut t;
        self.get_widened_type(t)
    }

    /// Go: `func (c *Checker) reportErrorsFromWidening(declaration, t, wideningKind)`
    /// (utilities.go — the widening kind classification: a widening error is
    /// reported when a type contains a widening literal in an initializer).
    fn report_errors_from_widening(&mut self, declaration: NodeId, t: TypeId) {
        // TODO(port M6 W3/S9): getWidenedLiteralKind — the recursive
        // widening-kind scan (needs the object-literal member walks); the
        // diagnostics seam is real (self.error below) but the classifier
        // lands with the widening completion.
        let _ = (declaration, t);
    }

    /// Go: `func (c *Checker) widenTypeInferredFromInitializer(declaration, t) *Type`.
    pub(crate) fn widen_type_inferred_from_initializer(
        &mut self,
        declaration: NodeId,
        t: TypeId,
    ) -> TypeId {
        let widened = self.get_widened_literal_type_for_initializer(declaration, t);
        // Go: the .js empty-literal implicit-any reporting
        // (reportImplicitAny on `{}` / `[]` types) — IsInJSFile is real but
        // the empty-literal type predicates are S4's; dormant in TS files.
        if tsc_ast::utilities::is_in_js_file(self.store().node(declaration)) {
            // TODO(port M6 S4): isEmptyLiteralType/isEmptyArrayLiteralType.
        }
        widened
    }

    /// Go: `func (c *Checker) getWidenedLiteralTypeForInitializer(declaration, t) *Type`.
    pub(crate) fn get_widened_literal_type_for_initializer(
        &mut self,
        declaration: NodeId,
        t: TypeId,
    ) -> TypeId {
        if self.get_combined_node_flags_cached(declaration).intersects(NF::CONSTANT)
            || u::is_declaration_readonly(self.store(), declaration)
        {
            return t;
        }
        self.get_widened_literal_type(t)
    }

    /// Go: `func (c *Checker) getWidenedLiteralType(t *Type) *Type`.
    pub(crate) fn get_widened_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.type_(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) && crate::checker::is_fresh_literal_type(self, t) {
            // TODO(port M6 S7): getBaseTypeOfEnumLikeType — the enum-literal
            // base read; the permissive exports.rs identity stub keeps the
            // literal.
            return self.get_base_type_of_enum_like_type_stub(t);
        }
        if flags.intersects(TypeFlags::STRING_LITERAL)
            && crate::checker::is_fresh_literal_type(self, t)
        {
            return self.string_type();
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL)
            && crate::checker::is_fresh_literal_type(self, t)
        {
            return self.number_type();
        }
        if flags.intersects(TypeFlags::BIGINT_LITERAL)
            && crate::checker::is_fresh_literal_type(self, t)
        {
            return self.bigint_type();
        }
        if flags.intersects(TypeFlags::BOOLEAN_LITERAL)
            && crate::checker::is_fresh_literal_type(self, t)
        {
            // Go: c.booleanType = getUnionType([regularFalseType,
            // regularTrueType]) — TODO(port M6 S7): the union constructor;
            // the permissive S7 stub returns errorType for the 2-constituent
            // union (the documented seam value).
            return self.boolean_type_stub();
        }
        if flags.intersects(TypeFlags::UNION) {
            // TODO(port M6 S7): mapType union arm — the permissive identity
            // keeps each literal constituent (Go widens each fresh literal
            // and rebuilds the union).
            return t;
        }
        t
    }

    /// The permissive booleanType stand-in (see get_widened_literal_type's
    /// boolean arm): S7's getUnionType stub answer for [false, true].
    fn boolean_type_stub(&mut self) -> TypeId {
        // TODO(port M6 S7): booleanType = getUnionType([regularFalseType, regularTrueType]).
        crate::relater::get_union_type(
            self,
            vec![self.regular_false_type(), self.regular_true_type()],
        )
    }

    /// TODO(port M6 S7): `getBaseTypeOfEnumLikeType` — exports.rs carries the
    /// permissive identity; surfaced here for the widened-literal enum arm.
    fn get_base_type_of_enum_like_type_stub(&mut self, t: TypeId) -> TypeId {
        self.get_base_type_of_literal_type(t)
    }

    // ────────────────────────────────────────────────────────────────────────
    // reportImplicitAny + the widening-context family (Go checker.go 18689–18943)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) reportImplicitAny(declaration, t, wideningKind)`.
    pub(crate) fn report_implicit_any(&mut self, declaration: NodeId, t: TypeId, widening_kind: WideningKind) {
        // TODO(port M6 S4): the IsCheckJSEnabledForFile gate — implicit any
        // errors are only reported in TS and ts-check JS files; the current
        // is_in_js_file read keeps JS files silent like Go.
        if tsc_ast::utilities::is_in_js_file(self.store().node(declaration)) {
            return;
        }
        // TODO(port M6 W4 nodebuilder): TypeToString — the real printer;
        // the stub prints empty (the diagnostic's typeAsString argument).
        let widened = self.get_widened_type(t);
        let type_as_string = self.type_to_string(widened);
        let kind = self.store().node(declaration).kind;
        let diagnostic: Option<&'static Message> = match kind {
            Kind::BinaryExpression | Kind::PropertyDeclaration | Kind::PropertySignature => {
                Some(if self.no_implicit_any {
                    &MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE
                } else {
                    &MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE
                })
            }
            Kind::Parameter => {
                let param = self
                    .store()
                    .node(declaration)
                    .as_parameter_declaration()
                    .expect("ParameterDeclaration data");
                if tsc_ast::is_identifier(self.store().node(param.name)) {
                    // TODO(port M6 S6): the keyword-name resolution check
                    // (resolveName for type meanings) — the named-parameter
                    // "arg0" suggestion rides the name resolver.
                }
                Some(if param.dot_dot_dot_token.is_some() {
                    if self.no_implicit_any {
                        &REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE
                    } else {
                        &REST_PARAMETER_0_IMPLICITLY_HAS_AN_ANY_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE
                    }
                } else if self.no_implicit_any {
                    &PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE
                } else {
                    &PARAMETER_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE
                })
            }
            Kind::BindingElement => {
                if !self.no_implicit_any {
                    // Don't issue a suggestion for binding elements since the
                    // codefix doesn't yet support them.
                    return;
                }
                Some(&BINDING_ELEMENT_0_IMPLICITLY_HAS_AN_1_TYPE)
            }
            Kind::FunctionDeclaration
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionExpression
            | Kind::ArrowFunction => {
                // TODO(port M6 S3/S4): the function-family implicit-return
                // reporting (the nameless + Reparsed + generator-yield
                // arms); the declared-name form below is the common one.
                if self.no_implicit_any && self.store().node(declaration).name().is_none() {
                    if widening_kind == WideningKind::GeneratorYield {
                        self.error(
                            declaration,
                            &GENERATOR_IMPLICITLY_HAS_YIELD_TYPE_0_CONSIDER_SUPPLYING_A_RETURN_TYPE_ANNOTATION,
                            &[type_as_string],
                        );
                    } else {
                        self.error(
                            declaration,
                            &FUNCTION_EXPRESSION_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_0_RETURN_TYPE,
                            &[type_as_string],
                        );
                    }
                    return;
                }
                Some(if self.no_implicit_any {
                    &X_0_WHICH_LACKS_RETURN_TYPE_ANNOTATION_IMPLICITLY_HAS_AN_1_RETURN_TYPE
                } else {
                    &X_0_IMPLICITLY_HAS_AN_1_RETURN_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE
                })
            }
            Kind::MappedType => {
                if self.no_implicit_any {
                    self.error(declaration, &MAPPED_OBJECT_TYPE_IMPLICITLY_HAS_AN_ANY_TEMPLATE_TYPE, &[]);
                }
                return;
            }
            _ => Some(if self.no_implicit_any {
                &VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE
            } else {
                &VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE_BUT_A_BETTER_TYPE_MAY_BE_INFERRED_FROM_USAGE
            }),
        };
        if let Some(diagnostic) = diagnostic {
            // Go: scanner.DeclarationNameToString(GetNameOfDeclaration(declaration)).
            // TODO(port M6 W4): the declaration-name printer; the node text
            // of the declaration's name is the common case.
            let name = self
                .store()
                .node(declaration)
                .name()
                .map(|n| ast_shims::get_text_of_node(self.store(), n))
                .unwrap_or_default();
            self.error_or_suggestion(
                self.no_implicit_any,
                declaration,
                diagnostic,
                &[name, type_as_string],
            );
        }
    }

    /// Go: `func (c *Checker) getWidenedType(t *Type) *Type` — the entry.
    pub(crate) fn get_widened_type(&mut self, t: TypeId) -> TypeId {
        self.get_widened_type_with_context(t, None)
    }

    /// Go: `func (c *Checker) getWidenedTypeWithContext(t *Type, context *WideningContext) *Type`.
    pub(crate) fn get_widened_type_with_context(
        &mut self,
        t: TypeId,
        context: Option<(&mut Vec<WideningContext>, usize)>,
    ) -> TypeId {
        if !self.type_(t).object_flags.intersects(ObjectFlags::REQUIRES_WIDENING) {
            return t;
        }
        let is_root = context.is_none();
        if is_root {
            if let Some(cached) = self.cached_types_get(CachedTypeKind::Widened, t) {
                return cached;
            }
        }
        let flags = self.type_(t).flags;
        let mut result: Option<TypeId> = None;
        if flags.intersects(TypeFlags::ANY.union(TypeFlags::NULLABLE)) {
            result = Some(self.any_type());
        } else if u::is_object_literal_type(self.type_(t)) {
            result = Some(self.get_widened_type_of_object_literal(t, context));
        } else if flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(t).types().to_vec();
            // The union arm recurses with a context whose siblings are the
            // union's constituents (Go constructs the context when nil).
            let mut arena_storage: Vec<WideningContext> = Vec::new();
            let union_context: Option<(&mut Vec<WideningContext>, usize)> = match context {
                Some((arena, index)) => Some((arena, index)),
                None => {
                    arena_storage.push(WideningContext {
                        siblings: Some(constituents.clone()),
                        ..WideningContext::default()
                    });
                    Some((&mut arena_storage, 0))
                }
            };
            // PORT: the &mut borrow of arena_storage lives only for the
            // recursion below (§5.4); the widened constituents collect
            // through a closure-free loop instead of Go's SameMap.
            let (arena, index) = union_context.expect("just constructed");
            let mut widened_types: Vec<TypeId> = Vec::with_capacity(constituents.len());
            for &member in &constituents {
                if self.type_(member).flags.intersects(TypeFlags::NULLABLE) {
                    widened_types.push(member);
                } else {
                    widened_types.push(self.get_widened_type_with_context(member, Some((arena, index))));
                }
            }
            // Widening an empty object literal transitions from a highly
            // restrictive type to a highly inclusive one. For that reason we
            // perform subtype reduction here if the union includes empty
            // object types (e.g. reducing {} | string to just {}).
            // TODO(port M6 S7): the isEmptyObjectType probe +
            // getUnionTypeEx(UnionReductionSubtype|Literal) — the union
            // constructor's wave; the permissive stub builds the union.
            result = Some(crate::relater::get_union_type(self, widened_types));
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            let mut widened: Vec<TypeId> = Vec::with_capacity(constituents.len());
            for member in constituents {
                widened.push(self.get_widened_type(member));
            }
            // TODO(port M6 S7): getIntersectionType — the permissive stub.
            result = Some(crate::relater::get_intersection_type(self, widened));
        } else if crate::relater::is_array_or_tuple_type(self, t) {
            let target = self.type_(t).target();
            let type_arguments = crate::relater::get_type_arguments(self, t);
            let mut widened: Vec<TypeId> = Vec::with_capacity(type_arguments.len());
            for member in type_arguments {
                widened.push(self.get_widened_type(member));
            }
            // TODO(port M6 S7): createTypeReference — the permissive
            // identity stub returns the target.
            result = target.map(|target| self.create_type_reference(target, widened));
        }
        if let Some(result) = result {
            if is_root {
                self.cached_types_set(CachedTypeKind::Widened, t, Some(result));
            }
            return result;
        }
        t
    }

    /// Go: `func (c *Checker) getWidenedTypeOfObjectLiteral(t, context) *Type`.
    fn get_widened_type_of_object_literal(
        &mut self,
        t: TypeId,
        context: Option<(&mut Vec<WideningContext>, usize)>,
    ) -> TypeId {
        if let Some((arena, index)) = &context {
            if let Some(cached) = arena[*index].widened_types.as_ref().and_then(|m| m.get(&t)) {
                return *cached;
            }
        }
        let mut members = SymbolTable::new();
        // TODO(port M6 S8): getPropertiesOfObjectType — the member-resolution
        // stub returns no properties, so the widened object literal keeps
        // only the context-undefined additions below until S8 lands.
        let _ = &mut members;
        if context.is_some() {
            // TODO(port M6 S9): getPropertiesOfContext — the sibling
            // undefined-property additions.
        }
        let symbol = self.type_(t).symbol;
        // TODO(port M6 S7/S8): getIndexInfosOfType widened remap — the
        // index-info construction; empty at this depth.
        let result = self.new_anonymous_type_with_members(symbol, members);
        // Retain js literal flag through widening
        let retained = self.type_(t).object_flags
            & (ObjectFlags::JS_LITERAL.union(ObjectFlags::NON_INFERRABLE_TYPE));
        self.type_mut(result).object_flags |= retained;
        // Only cache in child contexts since the root context never widens
        // a particular object literal type more than once. (PORT: the
        // arena reborrow consumes `context` — this is its last read.)
        if let Some((arena, index)) = context {
            if arena[index].parent.is_some() {
                arena[index]
                    .widened_types
                    .get_or_insert_with(FxHashMap::default)
                    .insert(t, result);
            }
        }
        result
    }

    /// Go: `func (c *Checker) getWidenedProperty(prop, context) *ast.Symbol`.
    fn get_widened_property(
        &mut self,
        prop: SymbolId,
        context: Option<(&mut Vec<WideningContext>, usize)>,
    ) -> SymbolId {
        if !self.program.symbol(prop).flags.intersects(SymbolFlags::PROPERTY) {
            // Since get accessors already widen their return value there is
            // no need to widen accessor based properties here.
            return prop;
        }
        let original = self.get_type_of_symbol(prop);
        // PORT: Go's getChildContext memoization lives on the arena (the
        // child is constructed per call until the arena carries it — the
        // observable result is identical; see the WideningContext PORT note).
        let widened = match context {
            Some((arena, index)) => {
                let name = self.program.symbol(prop).name.clone();
                let child = Checker::get_child_context(arena, index, &name);
                self.get_widened_type_with_context(original, Some((arena, child)))
            }
            None => self.get_widened_type_with_context(original, None),
        };
        if widened == original {
            return prop;
        }
        self.create_symbol_with_type(prop, widened)
    }

    /// Go: `func (c *Checker) getUndefinedProperty(prop *ast.Symbol) *ast.Symbol`.
    fn get_undefined_property(&mut self, prop: SymbolId) -> SymbolId {
        if let Some(&cached) = self.undefined_properties.get(&self.program.symbol(prop).name) {
            return cached;
        }
        let undefined_or_missing = self.undefined_or_missing_type();
        let result = self.create_symbol_with_type(prop, undefined_or_missing);
        self.program.symbol_mut(result).flags |= SymbolFlags::OPTIONAL;
        let name = self.program.symbol(result).name.clone();
        self.undefined_properties.insert(name, result);
        result
    }

    /// Go: `func (c *Checker) createSymbolWithType(source, t) *ast.Symbol`
    /// (checker.go:22128).
    pub(crate) fn create_symbol_with_type(&mut self, source: SymbolId, t: TypeId) -> SymbolId {
        let (flags, name, check_flags, declarations, parent, value_declaration) = {
            let s = self.program.symbol(source);
            (
                s.flags,
                s.name.clone(),
                s.check_flags,
                s.declarations.clone(),
                s.parent,
                s.value_declaration,
            )
        };
        let symbol = self.new_symbol_ex(
            flags,
            &name,
            CheckFlags::READONLY & check_flags,
        );
        {
            let s = self.program.symbol_mut(symbol);
            s.declarations = declarations;
            s.parent = parent;
            s.value_declaration = value_declaration;
        }
        let name_type = self.value_symbol_links.get(source).name_type;
        {
            let links = self.value_symbol_links.get(symbol);
            links.resolved_type = Some(t);
            links.target = Some(source);
            links.name_type = name_type;
        }
        symbol
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Accessors, enums, aliases + the optionality family
// (Go checker.go 18910–19077)
// ────────────────────────────────────────────────────────────────────────────

    // TODO(port M6 S7/S9/S10: the facts/union consumers): the bannered permissive/stub bodies below
    // are forward-landed; their consumers arrive with those waves.
    #[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) getTypeOfEnumMember(symbol *ast.Symbol) *Type`.
    fn get_type_of_enum_member(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let t = self.get_declared_type_of_enum_member(symbol);
            self.value_symbol_links.get(symbol).resolved_type = Some(t);
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) getTypeOfAccessors(symbol *ast.Symbol) *Type`.
    pub(crate) fn get_type_of_accessors(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::Type,
            ) {
                return self.error_type();
            }
            let declarations = self.program.symbol(symbol).declarations.clone();
            let getter = find_declaration_of_kind(self, &declarations, Kind::GetAccessor);
            let setter = find_declaration_of_kind(self, &declarations, Kind::SetAccessor);
            let accessor = declarations
                .iter()
                .copied()
                .find(|&d| binder_shims::is_auto_accessor_property_declaration(self.store().node(d)));
            // We try to resolve a getter type annotation, a setter type
            // annotation, or a getter function body return type inference, in
            // that order.
            let mut t = self.get_annotated_accessor_type(getter);
            if t.is_none() {
                t = self.get_annotated_accessor_type(setter);
            }
            if t.is_none() {
                t = self.get_annotated_accessor_type(accessor);
            }
            if t.is_none() {
                if let Some(g) = getter {
                    let body = self
                        .store()
                        .node(g)
                        .as_get_accessor_declaration()
                        .map(|d| d.body);
                    if body.is_some() {
                        // TODO(port M6 S8/S9): getReturnTypeFromBody — the
                        // flow-aware body return inference; permissive nil
                        // (falls to the implicit-any report below, Go's
                        // answer for a bodyless getter).
                    }
                }
            }
            if t.is_none() {
                if let Some(accessor) = accessor {
                    t = Some(self.get_widened_type_for_variable_like_declaration(accessor, true /*reportErrors*/));
                }
            }
            if t.is_none() {
                if setter.is_some() {
                    // TODO(port M6 S3): the private-within-ambient gate +
                    // the noImplicitAny suggestion forms.
                } else if getter.is_some() {
                        let symbol_name2 = self.symbol_to_string(symbol);
                    self.error(
                        getter.expect("checked"),
                        &PROPERTY_0_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_ITS_GET_ACCESSOR_LACKS_A_RETURN_TYPE_ANNOTATION,
                        &[symbol_name2],
                    );
                }
                t = Some(self.any_type());
            }
            let mut t = t.expect("defaulted above");
            if !self.pop_type_resolution() {
                if self.get_annotated_accessor_type_node(getter).is_some() {
                        let symbol_name3 = self.symbol_to_string(symbol);
                    self.error(
                        getter.expect("checked"),
                        &X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_TYPE_ANNOTATION,
                        &[symbol_name3],
                    );
                } else if self.get_annotated_accessor_type_node(setter).is_some() {
                        let symbol_name4 = self.symbol_to_string(symbol);
                    self.error(
                        setter.expect("checked"),
                        &X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_TYPE_ANNOTATION,
                        &[symbol_name4],
                    );
                }
                t = self.any_type();
            }
            if self.value_symbol_links.get(symbol).resolved_type.is_none() {
                self.value_symbol_links.get(symbol).resolved_type = Some(t);
            }
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) getWriteTypeOfAccessors(symbol *ast.Symbol) *Type`.
    fn get_write_type_of_accessors(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).write_type.is_none() {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::WriteType,
            ) {
                return self.error_type();
            }
            let declarations = self.program.symbol(symbol).declarations.clone();
            let mut setter = find_declaration_of_kind(self, &declarations, Kind::SetAccessor);
            if setter.is_none() {
                let prop_declaration = find_declaration_of_kind(self, &declarations, Kind::PropertyDeclaration);
                if prop_declaration.is_some_and(|p| {
                    binder_shims::is_auto_accessor_property_declaration(self.store().node(p))
                }) {
                    setter = prop_declaration;
                }
            }
            let mut write_type = self.get_annotated_accessor_type(setter);
            if !self.pop_type_resolution() {
                if self.get_annotated_accessor_type_node(setter).is_some() {
                        let symbol_name5 = self.symbol_to_string(symbol);
                    self.error(
                        setter.expect("checked"),
                        &X_0_IS_REFERENCED_DIRECTLY_OR_INDIRECTLY_IN_ITS_OWN_TYPE_ANNOTATION,
                        &[symbol_name5],
                    );
                }
                write_type = Some(self.any_type());
            }
            // Absent an explicit setter type annotation we use the read
            // type of the accessor.
            if self.value_symbol_links.get(symbol).write_type.is_none() {
                let write_type = match write_type {
                    Some(write_type) => write_type,
                    None => self.get_type_of_accessors(symbol),
                };
                self.value_symbol_links.get(symbol).write_type = Some(write_type);
            }
        }
        self.value_symbol_links.get(symbol).write_type.expect("just set")
    }

    /// Go: `func (c *Checker) getTypeOfAlias(symbol *ast.Symbol) *Type`.
    fn get_type_of_alias(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::Type,
            ) {
                return self.error_type();
            }
            let target_symbol = self.resolve_alias(symbol);
            let export_symbol = self.get_target_of_alias_declaration(self.get_declaration_of_alias_symbol(symbol));
            // It only makes sense to get the type of a value symbol. If the
            // result of resolving the alias is not a value, then it has no
            // type. To get the type associated with a type symbol, call
            // getDeclaredTypeOfSymbol.
            // This check is important because without it, a call to
            // getTypeOfSymbol could end up recursively calling
            // getTypeOfAlias, causing a stack overflow.
            if self.value_symbol_links.get(symbol).resolved_type.is_none() {
                let resolved = if self
                    .get_symbol_flags_ex(target_symbol, false /*excludeTypeOnlyMeanings*/, false /*excludeLocalMeanings*/)
                    .intersects(SymbolFlags::VALUE)
                {
                    self.get_type_of_symbol(target_symbol)
                } else {
                    self.error_type()
                };
                self.value_symbol_links.get(symbol).resolved_type = Some(resolved);
            }
            if !self.pop_type_resolution() {
                self.report_circularity_error(export_symbol.unwrap_or(symbol));
                if self.value_symbol_links.get(symbol).resolved_type.is_none() {
                    let error_type = self.error_type();
                    self.value_symbol_links.get(symbol).resolved_type = Some(error_type);
                }
                return self.value_symbol_links.get(symbol).resolved_type.expect("just set");
            }
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }

    /// Go: `func (c *Checker) addOptionality(t *Type) *Type`.
    pub(crate) fn add_optionality(&mut self, t: TypeId) -> TypeId {
        self.add_optionality_ex(t, false /*isProperty*/, true /*isOptional*/)
    }

    /// Go: `func (c *Checker) addOptionalityEx(t, isProperty, isOptional) *Type`.
    pub(crate) fn add_optionality_ex(&mut self, t: TypeId, is_property: bool, is_optional: bool) -> TypeId {
        if self.strict_null_checks && is_optional {
            return self.get_optional_type(t, is_property);
        }
        t
    }

    /// Go: `func (c *Checker) getOptionalType(t *Type, isProperty bool) *Type`.
    pub(crate) fn get_optional_type(&mut self, t: TypeId, is_property: bool) -> TypeId {
        debug_assert!(self.strict_null_checks);
        let missing_or_undefined = if is_property {
            self.undefined_or_missing_type()
        } else {
            self.undefined_type()
        };
        if t == missing_or_undefined
            || self.type_(t).flags.intersects(TypeFlags::UNION)
                && self.type_(t).types().first() == Some(&missing_or_undefined)
        {
            return t;
        }
        // TODO(port M6 S7): the interned union construction; the permissive
        // stub carries errorType for the two-constituent union.
        crate::relater::get_union_type(self, vec![t, missing_or_undefined])
    }

    /// Add undefined or null or both to a type if they are missing.
    ///
    /// Go: `func (c *Checker) getNullableType(t *Type, flags TypeFlags) *Type`.
    pub(crate) fn get_nullable_type(&mut self, t: TypeId, flags: TypeFlags) -> TypeId {
        // Go: `missing := (flags &^ t.flags) & (Undefined | Null)`.
        let missing = TypeFlags(
            flags.bits() & !self.type_(t).flags.bits()
                & (TypeFlags::UNDEFINED | TypeFlags::NULL).bits(),
        );
        if missing.is_empty() {
            return t;
        }
        // TODO(port M6 S7): the interned union constructions below.
        if missing == TypeFlags::UNDEFINED {
            return crate::relater::get_union_type(self, vec![t, self.undefined_type()]);
        }
        if missing == TypeFlags::NULL {
            return crate::relater::get_union_type(self, vec![t, self.null_type()]);
        }
        crate::relater::get_union_type(self, vec![t, self.undefined_type(), self.null_type()])
    }

    /// Go: `func (c *Checker) IsNullableType(t *Type) bool`.
    pub fn is_nullable_type(&mut self, t: TypeId) -> bool {
        self.has_type_facts(t, crate::relater::TypeFacts::IS_UNDEFINED_OR_NULL)
    }

    /// Go: `func (c *Checker) getNonNullableTypeIfNeeded(t *Type) *Type`.
    pub(crate) fn get_non_nullable_type_if_needed(&mut self, t: TypeId) -> TypeId {
        if self.is_nullable_type(t) {
            return self.get_non_nullable_type(t);
        }
        t
    }

    /// Go: `scanner.DeclarationNameToString`-adjacent: relater's
    /// `symbolToString` bridge (the W2 nodebuilder stub).
    fn symbol_to_string(&mut self, symbol: SymbolId) -> String {
        crate::relater::symbol_to_string(self, symbol)
    }

    /// Go: `func (c *Checker) isVarConstLike(node *ast.Node) bool`.
    pub(crate) fn is_var_const_like(&mut self, node: NodeId) -> bool {
        let block_scope_kind = self.get_combined_node_flags_cached(node) & NF::BLOCK_SCOPED;
        block_scope_kind == NF::CONSTANT
            || block_scope_kind == NF::USING
            || block_scope_kind == NF::AWAIT_USING
    }

    /// Go: `func (c *Checker) getEffectivePropertyNameForPropertyNameNode(node) (string, bool)`.
    pub(crate) fn get_effective_property_name_for_property_name_node(
        &mut self,
        node: NodeId,
    ) -> (String, bool) {
        let name = ast_shims::get_property_name_for_property_name_node(self.store(), node);
        if name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING {
            return (name, true);
        }
        if self.store().node(node).kind == Kind::ComputedPropertyName {
            // This is cached so `getTypeOfExpression` isn't constantly
            // reinvoked for every property name lookup
            if let (Some(has_name), Some(name)) = {
                let links = self.computed_name_links.get(node);
                (links.has_name, if links.has_name == Some(true) { Some(links.name.clone()) } else { None })
            } {
                return (name, has_name);
            }
            // TODO(port M6 S4): tryGetNameFromType(getTypeOfExpression(...))
            // — the computed-name type read; permissive (missing, false).
            return (String::new(), false);
        }
        (String::new(), false)
    }

    /// Go: `func (c *Checker) tryGetNameFromType(t *Type) (string, bool)`.
    pub(crate) fn try_get_name_from_type(&self, t: TypeId) -> (String, bool) {
        let ty = self.type_(t);
        if ty.flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
            return (
                ty.as_unique_essymbol_type()
                    .expect("UniqueESSymbolType data")
                    .name
                    .clone(),
                true,
            );
        }
        if ty.flags.intersects(TypeFlags::STRING_LITERAL) {
            if let Some(LiteralValue::String(s)) = ty.as_literal_type().and_then(|d| d.value.as_ref()) {
                return (s.clone(), true);
            }
        }
        if ty.flags.intersects(TypeFlags::NUMBER_LITERAL) {
            if let Some(LiteralValue::Number(n)) = ty.as_literal_type().and_then(|d| d.value.as_ref()) {
                return (n.to_string(), true);
            }
        }
        (String::new(), false)
    }
}

/// Go: `ast.GetDeclarationOfKind(symbol, kind)` — the local form over the
/// program's symbol declarations.
fn find_declaration_of_kind(c: &Checker, declarations: &[NodeId], kind: Kind) -> Option<NodeId> {
    let store = c.store();
    declarations
        .iter()
        .copied()
        .find(|&d| store.node(d).kind == kind)
}

/// Go: `ast.IsAliasSymbolDeclaration(node)` (utilities.go) — the alias
/// declaration kinds (the binder doesn't carry it; S2's
/// getDeclarationOfAliasSymbol is the sole checker-side consumer).
fn is_alias_symbol_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::ImportEqualsDeclaration
        | Kind::NamespaceExportDeclaration
        | Kind::NamespaceImport
        | Kind::NamespaceExport
        | Kind::ImportSpecifier
        | Kind::ExportSpecifier => true,
        Kind::ImportClause => n.as_import_clause().map(|d| d.name).is_some(),
        Kind::ExportAssignment => {
            let expr = n
                .as_export_assignment()
                .expect("ExportAssignment data")
                .expression;
            // Go: ExpressionIsAlias — an Identifier or QualifiedName (the
            // entity-name forms; everything else is a value expression).
            let e = store.node(expr);
            matches!(e.kind, Kind::Identifier | Kind::QualifiedName)
        }
        _ => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The declared-type family (Go checker.go 17706–17759, 24194–24700)
// — the "second half" of the circularity/cache-key triangle: these read the
// SAME `pushTypeResolution` machinery the getTypeOfSymbol spine drives, on
// the `DeclaredType` property axis.
// ────────────────────────────────────────────────────────────────────────────

    // TODO(port M6 S8: canGetTypeParametersOfClassOrInterface’s callers): the bannered permissive/stub bodies below
    // are forward-landed; their consumers arrive with those waves.
    #[allow(dead_code)]
impl Checker {
    /// Go: `func (c *Checker) tryGetDeclaredTypeOfSymbol(symbol) *Type` —
    /// pub(crate) for the exports.rs Get* twin (the sole
    /// `get_declared_type_of_symbol` method name stays on the pub surface;
    /// internal call sites resolve to it).
    pub(crate) fn try_get_declared_type_of_symbol(&mut self, symbol: SymbolId) -> Option<TypeId> {
        let flags = self.program.symbol(symbol).flags;
        if flags.intersects(SymbolFlags::CLASS.union(SymbolFlags::INTERFACE)) {
            Some(self.get_declared_type_of_class_or_interface(symbol))
        } else if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
            Some(self.get_declared_type_of_type_parameter(symbol))
        } else if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            Some(self.get_declared_type_of_type_alias(symbol))
        } else if flags.intersects(SymbolFlags::ENUM) {
            Some(self.get_declared_type_of_enum(symbol))
        } else if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            Some(self.get_declared_type_of_enum_member(symbol))
        } else if flags.intersects(SymbolFlags::ALIAS) {
            Some(self.get_declared_type_of_alias(symbol))
        } else {
            None
        }
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfClassOrInterface(symbol) *Type`
    /// (checker.go:17706) — the class/interface declared type with the
    /// generic-reference promotion (thisType + instantiations table).
    pub(crate) fn get_declared_type_of_class_or_interface(&mut self, symbol: SymbolId) -> TypeId {
        let declared_type = self.declared_type_links.get(symbol).declared_type;
        if let Some(t) = declared_type {
            return t;
        }
        let is_class = self.program.symbol(symbol).flags.intersects(SymbolFlags::CLASS);
        let kind = if is_class {
            ObjectFlags::CLASS
        } else {
            ObjectFlags::INTERFACE
        };
        let t = self.new_object_type(kind, Some(symbol));
        self.declared_type_links.get(symbol).declared_type = Some(t);
        let outer_type_parameters = self.get_outer_type_parameters_of_class_or_interface(symbol);
        let type_parameters = self.append_local_type_parameters_of_class_or_interface_or_type_alias(
            outer_type_parameters.clone(),
            symbol,
        );
        // A class or interface is generic if it has type parameters or a
        // "this" type. We always give classes a "this" type because it is
        // not feasible to analyze all members to determine if the "this"
        // type escapes the class (in particular, property types inferred
        // from initializers and method return types inferred from return
        // statements are very hard to exhaustively analyze). We give
        // interfaces a "this" type if we can't definitely determine that
        // they are free of "this" references.
        if !type_parameters.is_empty() || is_class || !self.is_thisless_interface(symbol) {
            self.type_mut(t).object_flags |= ObjectFlags::REFERENCE;
            let this_type = self.new_type_parameter(Some(symbol));
            {
                let tp = self.type_mut(this_type);
                let tp = tp.as_type_parameter_mut().expect("TypeParameter data");
                tp.is_this_type = true;
                tp.constraint = Some(t);
            }
            let mut all_type_parameters = type_parameters;
            all_type_parameters.push(this_type);
            let resolved_type_arguments = all_type_parameters.clone();
            let instantiations = self.type_mut(t)
                .as_interface_type_mut()
                .expect("InterfaceType data");
            instantiations.this_type = Some(this_type);
            instantiations.all_type_parameters = all_type_parameters;
            instantiations.outer_type_parameter_count = outer_type_parameters.len() as i32;
            instantiations.base.resolved_type_arguments = resolved_type_arguments.clone();
            let mut table = FxHashMap::default();
            table.insert(get_type_list_key(&resolved_type_arguments), t);
            instantiations.base.base.instantiations = Some(table);
            instantiations.base.base.target = Some(t);
        }
        self.declared_type_links.get(symbol).declared_type.expect("just set")
    }

    /// Returns true if the interface given by the symbol is free of "this"
    /// references.
    ///
    /// Specifically, the result is true if the interface itself contains no
    /// references to "this" in its body, if all base types are interfaces,
    /// and if none of the base interfaces have a "this" type.
    ///
    /// Go: `func (c *Checker) isThislessInterface(symbol) bool` (17743).
    fn is_thisless_interface(&mut self, symbol: SymbolId) -> bool {
        let declarations = self.program.symbol(symbol).declarations.clone();
        for declaration in declarations {
            if self.store().node(declaration).kind == Kind::InterfaceDeclaration {
                if self.store().node(declaration).flags.intersects(NF::CONTAINS_THIS) {
                    return false;
                }
                let interface = self
                    .store()
                    .node(declaration)
                    .as_interface_declaration()
                    .expect("InterfaceDeclaration data");
                // Go: ast.GetExtendsHeritageClauseElements(declaration).
                let heritage: Vec<NodeId> = interface
                    .heritage_clauses
                    .iter()
                    .flat_map(|list| list.nodes.iter().copied())
                    .filter(|&clause| {
                        self.store().node(clause).as_heritage_clause().map(|h| h.token) == Some(Kind::ExtendsKeyword)
                    })
                    .flat_map(|clause| {
                        self.store()
                            .node(clause)
                            .as_heritage_clause()
                            .expect("HeritageClause data")
                            .types
                            .iter()
                            .flat_map(|list| list.nodes.iter().copied())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                for node in heritage {
                    // Go: ast.GetHeritageClauseElementName(node).
                    let name = self
                        .store()
                        .node(node)
                        .as_expression_with_type_arguments()
                        .map(|e| e.expression);
                    if let Some(name) = name {
                        if matches!(self.store().node(name).kind, Kind::Identifier | Kind::QualifiedName)
                            || tsc_ast::utilities::is_entity_name_expression(self.store(), name)
                        {
                            // TODO(port M6 S6): resolveEntityName — the name
                            // resolver; until it lands, this base type cannot
                            // be confirmed interface-and-thisless, so the
                            // interface conservatively gets a this type
                            // (Go's answer whenever resolution fails the
                            // SymbolFlagsInterface test).
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Go: `func (c *Checker) getOuterTypeParametersOfClassOrInterface(symbol)` (24261).
    fn get_outer_type_parameters_of_class_or_interface(&mut self, symbol: SymbolId) -> Vec<TypeId> {
        let declaration = self.get_class_or_interface_like_declaration(symbol);
        debug_assert!(
            declaration.is_some(),
            "Class was missing valueDeclaration -OR- non-class had no interface declarations"
        );
        match declaration {
            Some(declaration) => self.get_outer_type_parameters(declaration, false /*includeThisTypes*/),
            None => Vec::new(),
        }
    }

    /// Returns the declaration used to obtain a class, interface, or
    /// function symbol's outer type parameters.
    ///
    /// Go: `func (c *Checker) getClassOrInterfaceLikeDeclaration(symbol)` (24268).
    fn get_class_or_interface_like_declaration(&mut self, symbol: SymbolId) -> Option<NodeId> {
        let s = self.program.symbol(symbol);
        if s.flags.intersects(SymbolFlags::CLASS.union(SymbolFlags::FUNCTION)) {
            return s.value_declaration;
        }
        s.declarations.iter().copied().find(|&d| {
            let node = self.store().node(d);
            if node.kind == Kind::InterfaceDeclaration {
                return true;
            }
            if node.kind != Kind::VariableDeclaration {
                return false;
            }
            let initializer = ast_shims::initializer_of(self.store(), d);
            initializer.is_some_and(|i| {
                matches!(
                    self.store().node(i).kind,
                    Kind::FunctionExpression | Kind::ArrowFunction
                )
            })
        })
    }

    /// Go: `func (c *Checker) canGetTypeParametersOfClassOrInterface(symbol) bool`.
    pub(crate) fn can_get_type_parameters_of_class_or_interface(&mut self, symbol: SymbolId) -> bool {
        self.get_class_or_interface_like_declaration(symbol).is_some()
    }

    /// Return the outer type parameters of a node or undefined if the node
    /// has no outer type parameters.
    ///
    /// Go: `func (c *Checker) getOuterTypeParameters(node, includeThisTypes)` (24289).
    pub(crate) fn get_outer_type_parameters(&mut self, node: NodeId, include_this_types: bool) -> Vec<TypeId> {
        let mut node = node;
        loop {
            let parent = self.store().node(node).parent.get();
            if parent == NodeId::NONE {
                return Vec::new();
            }
            node = parent;
            let kind = self.store().node(node).kind;
            match kind {
                Kind::ClassDeclaration
                | Kind::ClassExpression
                | Kind::InterfaceDeclaration
                | Kind::CallSignature
                | Kind::ConstructSignature
                | Kind::MethodSignature
                | Kind::FunctionType
                | Kind::ConstructorType
                | Kind::FunctionDeclaration
                | Kind::MethodDeclaration
                | Kind::FunctionExpression
                | Kind::ArrowFunction
                | Kind::TypeAliasDeclaration
                | Kind::JSTypeAliasDeclaration
                | Kind::MappedType
                | Kind::ConditionalType => {
                    let mut outer_type_parameters = self.get_outer_type_parameters(node, include_this_types);
                    if matches!(kind, Kind::FunctionExpression | Kind::ArrowFunction)
                        && binder_shims::is_object_literal_method(self.store(), node)
                        && self.is_context_sensitive(node)
                    {
                        // TODO(port M6 S6): resolveSignature — the
                        // context-sensitive signature read; the permissive
                        // getSignaturesOfType returns no signatures, so no
                        // signature type parameters join (Go's nil arm).
                    }
                    if kind == Kind::MappedType {
                        let type_parameter = self
                            .store()
                            .node(node)
                            .as_mapped_type_node()
                            .expect("MappedTypeNode data")
                            .type_parameter;
                        let symbol = self.get_symbol_of_declaration(type_parameter)
                            .expect("MappedTypeNode type-parameter symbol");
                        outer_type_parameters
                            .push(self.get_declared_type_of_type_parameter(symbol));
                        return outer_type_parameters;
                    }
                    if kind == Kind::ConditionalType {
                        // TODO(port M6 S10): getInferTypeParameters — the
                        // conditional/infer wave; the locals read is its
                        // seam. Go appends the node's TypeParameter-flagged
                        // locals' declared types here.
                        return outer_type_parameters;
                    }
                    let declarations = binder_node_data::type_parameters_of(self.store(), node)
                        .map(|list| list.nodes.clone())
                        .unwrap_or_default();
                    let outer_and_own_type_parameters =
                        self.append_type_parameters(outer_type_parameters, &declarations);
                    if include_this_types
                        && matches!(kind, Kind::ClassDeclaration | Kind::ClassExpression | Kind::InterfaceDeclaration)
                    {
                        let symbol = self
                            .get_symbol_of_declaration(node)
                            .expect("class/interface declaration symbol");
                        let declared =
                            self.get_declared_type_of_class_or_interface(symbol);
                        let this_type = self
                            .type_(declared)
                            .as_interface_type()
                            .expect("InterfaceType data")
                            .this_type;
                        if let Some(this_type) = this_type {
                            let mut result = outer_and_own_type_parameters;
                            result.push(this_type);
                            return result;
                        }
                    }
                    return outer_and_own_type_parameters;
                }
                _ => {}
            }
        }
    }

    /// The local type parameters are the combined set of type parameters
    /// from all declarations of the class, interface, or type alias.
    ///
    /// Go: `func (c *Checker) getLocalTypeParametersOfClassOrInterfaceOrTypeAlias(symbol)` (24340).
    /// (The `_impl` suffix keeps the pub-API surface's Get* twin in exports.rs
    /// the sole public name.)
    pub(crate) fn get_local_type_parameters_of_class_or_interface_or_type_alias_impl(
        &mut self,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        self.append_local_type_parameters_of_class_or_interface_or_type_alias(Vec::new(), symbol)
    }

    /// Go: `func (c *Checker) appendLocalTypeParametersOfClassOrInterfaceOrTypeAlias(types, symbol)` (24344).
    fn append_local_type_parameters_of_class_or_interface_or_type_alias(
        &mut self,
        types: Vec<TypeId>,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        let declarations = self.program.symbol(symbol).declarations.clone();
        let mut types = types;
        for node in declarations {
            let store = self.store();
            let is_candidate = matches!(
                store.node(node).kind,
                Kind::InterfaceDeclaration | Kind::ClassDeclaration | Kind::ClassExpression
            ) || u::is_type_alias(store.node(node));
            if is_candidate {
                let declarations = binder_node_data::type_parameters_of(self.store(), node)
                    .map(|list| list.nodes.clone())
                    .unwrap_or_default();
                types = self.append_type_parameters(types, &declarations);
            }
        }
        types
    }

    /// Appends the type parameters given by a list of declarations to a set
    /// of type parameters and returns the resulting set.
    ///
    /// Go: `func (c *Checker) appendTypeParameters(typeParameters, declarations)` (24356).
    fn append_type_parameters(&mut self, type_parameters: Vec<TypeId>, declarations: &[NodeId]) -> Vec<TypeId> {
        let mut type_parameters = type_parameters;
        for declaration in declarations {
            let symbol = self
                .get_symbol_of_declaration(*declaration)
                .expect("type-parameter declaration symbol");
            let t = self.get_declared_type_of_type_parameter(symbol);
            if !type_parameters.contains(&t) {
                type_parameters.push(t);
            }
        }
        type_parameters
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfTypeParameter(symbol)` (24363).
    pub(crate) fn get_declared_type_of_type_parameter(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.declared_type_links.get(symbol).declared_type {
            return t;
        }
        let t = self.new_type_parameter(Some(symbol));
        self.declared_type_links.get(symbol).declared_type = Some(t);
        t
    }

    /// Go: `func (c *Checker) newTypeParameter(symbol)` (checker.go:25702).
    fn new_type_parameter(&mut self, symbol: Option<SymbolId>) -> TypeId {
        let t = self.new_type(
            TypeFlags::TYPE_PARAMETER,
            ObjectFlags::NONE,
            TypeData::TypeParameter(Box::default()),
        );
        self.type_mut(t).symbol = symbol;
        t
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfTypeAlias(symbol)` (24371).
    fn get_declared_type_of_type_alias(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.type_alias_links.get(symbol).declared_type {
            return t;
        }
        // Note that we use the links object as the target here because the
        // symbol object is used as the unique identity for resolution of the
        // 'type' property in SymbolLinks.
        if !self.push_type_resolution(
            TypeSystemEntity::Symbol(symbol),
            TypeSystemPropertyName::DeclaredType,
        ) {
            return self.error_type();
        }
        let declaration = self
            .program
            .symbol(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| u::is_type_alias(self.store().node(d)));
        let declaration = declaration.expect("type alias has an alias declaration");
        let type_node = self
            .store()
            .node(declaration)
            .as_type_alias_declaration()
            .expect("TypeAliasDeclaration data")
            .type_;
        // TODO(port M6 S5): getTypeFromTypeNode — the type-node resolution
        // wave; the permissive read resolves to errorType, so a type alias's
        // declared type is errorType until S5 lands (the shape — the
        // resolution push/pop and the instantiation-cache seeding below — is
        // Go's).
        let mut t = self.get_type_from_type_node(type_node);
        if self.pop_type_resolution() {
            let type_parameters = self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
            if !type_parameters.is_empty() {
                // Initialize the instantiation cache for generic type
                // aliases. The declared type corresponds to an instantiation
                // of the type alias with the type parameters supplied as
                // type arguments.
                let mut table = FxHashMap::default();
                table.insert(get_type_list_key(&type_parameters), t);
                {
                    let links = self.type_alias_links.get(symbol);
                    links.type_parameters = type_parameters.clone();
                    links.instantiations = Some(table);
                }
            }
            // Go: the intrinsicMarkerType BuiltinIteratorReturn swap — the
            // intrinsic machinery is S7's; the name test cannot match a
            // non-intrinsic type.
        } else {
            let mut error_node = self.store().node(declaration).name();
            if error_node.is_none() {
                error_node = Some(declaration);
            }
            let symbol_name = crate::relater::symbol_to_string(self, symbol);
            self.error(
                error_node.expect("defaulted to declaration"),
                &TYPE_ALIAS_0_CIRCULARLY_REFERENCES_ITSELF,
                &[symbol_name],
            );
            t = self.error_type();
        }
        if self.type_alias_links.get(symbol).declared_type.is_none() {
            self.type_alias_links.get(symbol).declared_type = Some(t);
        }
        self.type_alias_links.get(symbol).declared_type.expect("just set")
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfEnum(symbol)` (24409).
    fn get_declared_type_of_enum(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.declared_type_links.get(symbol).declared_type {
            return t;
        }
        let mut member_type_list: Vec<TypeId> = Vec::new();
        let declarations = self.program.symbol(symbol).declarations.clone();
        for declaration in declarations {
            if self.store().node(declaration).kind == Kind::EnumDeclaration {
                // Go reads declaration.Members() directly (an enum
                // declaration's member list) — getMembersOfDeclaration's
                // class/interface subset does not cover enums.
                for member in tsc_binder::node_data::members_of(self.store(), declaration).to_vec() {
                    if !binder_shims::has_dynamic_name(self.store(), member) {
                        let member_symbol = self
                            .get_symbol_of_declaration(member)
                            .expect("enum member symbol");
                        let value = self.get_enum_member_value(member);
                        let member_type = match value.value {
                            Some(EvaluatorValue::Number(n)) => {
                                Some(self.get_enum_literal_type(LiteralValue::Number(n), symbol, member_symbol))
                            }
                            Some(EvaluatorValue::String(s)) => {
                                Some(self.get_enum_literal_type(LiteralValue::String(s), symbol, member_symbol))
                            }
                            None => Some(self.create_computed_enum_type(member_symbol)),
                        };
                        let member_type = member_type.expect("handled");
                        let fresh = self.get_fresh_type_of_literal_type(member_type);
                        self.declared_type_links.get(member_symbol).declared_type = Some(fresh);
                        member_type_list.push(member_type);
                    }
                }
            }
        }
        let enum_type = if !member_type_list.is_empty() {
            // TODO(port M6 S7): getUnionTypeEx(UnionReductionLiteral, alias)
            // — the union constructor wave; the permissive stub reduces the
            // multi-constituent list to errorType.
            let alias = TypeAlias {
                symbol: Some(symbol),
                type_arguments: Vec::new(),
            };
            crate::relater::get_union_type_ex(self, member_type_list, UnionReduction::Literal, Some(Box::new(alias)), None)
        } else {
            self.create_computed_enum_type(symbol)
        };
        if self.type_(enum_type).flags.intersects(TypeFlags::UNION) {
            self.type_mut(enum_type).flags |= TypeFlags::ENUM_LITERAL;
            self.type_mut(enum_type).symbol = Some(symbol);
        }
        self.declared_type_links.get(symbol).declared_type = Some(enum_type);
        self.declared_type_links.get(symbol).declared_type.expect("just set")
    }

    /// Go: `func (c *Checker) getEnumMemberValue(node) evaluator.Result` (24446).
    fn get_enum_member_value(&mut self, node: NodeId) -> EvaluatorResult {
        let parent = self.store().node(node).parent.get();
        self.compute_enum_member_values(parent);
        self.enum_member_links.get(node).value.clone()
    }

    /// Go: `func (c *Checker) createComputedEnumType(symbol) *Type` (24451).
    fn create_computed_enum_type(&mut self, symbol: SymbolId) -> TypeId {
        let regular_type = self.new_literal_type(TypeFlags::ENUM, LiteralValue::None, None);
        self.type_mut(regular_type).symbol = Some(symbol);
        let fresh_type = self.new_literal_type(TypeFlags::ENUM, LiteralValue::None, Some(regular_type));
        self.type_mut(fresh_type).symbol = Some(symbol);
        self.type_mut(regular_type)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(fresh_type);
        self.type_mut(fresh_type)
            .as_literal_type_mut()
            .expect("LiteralType data")
            .fresh_type = Some(fresh_type);
        regular_type
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfEnumMember(symbol)` (24461).
    fn get_declared_type_of_enum_member(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.declared_type_links.get(symbol).declared_type {
            return t;
        }
        let parent = self.get_parent_of_symbol(symbol);
        if let Some(parent) = parent {
            let enum_type = self.get_declared_type_of_enum(parent);
            if self.declared_type_links.get(symbol).declared_type.is_none() {
                self.declared_type_links.get(symbol).declared_type = Some(enum_type);
            }
        }
        self.declared_type_links
            .get(symbol)
            .declared_type
            .unwrap_or_else(|| self.error_type())
    }

    /// Go: `func (c *Checker) computeEnumMemberValues(node)` (24472).
    pub(crate) fn compute_enum_member_values(&mut self, node: NodeId) {
        let computed = self
            .node_links
            .get(node)
            .flags
            .intersects(crate::types::NodeCheckFlags::ENUM_VALUES_COMPUTED);
        if !computed {
            self.node_links.get(node).flags |= crate::types::NodeCheckFlags::ENUM_VALUES_COMPUTED;
            let mut auto_value: Option<tsc_jsnum::Number> = Some(tsc_jsnum::Number(0.0));
            let mut previous: Option<NodeId> = None;
            // Go reads node.Members() directly (the enum declaration's
            // member list — getMembersOfDeclaration's class/interface subset
            // does not cover enums).
            let members = tsc_binder::node_data::members_of(self.store(), node).to_vec();
            for member in members {
                let result = self.compute_enum_member_value(member, auto_value, previous);
                self.enum_member_links.get(member).value = result.clone();
                if let Some(EvaluatorValue::Number(value)) = result.value.as_ref() {
                    auto_value = Some(tsc_jsnum::Number(value.0 + 1.0));
                } else {
                    auto_value = None;
                }
                previous = Some(member);
            }
        }
    }

    /// Go: `func (c *Checker) computeEnumMemberValue(member, autoValue, previous)` (24492).
    fn compute_enum_member_value(
        &mut self,
        member: NodeId,
        auto_value: Option<tsc_jsnum::Number>,
        previous: Option<NodeId>,
    ) -> EvaluatorResult {
        let member_name = self.store().node(member).name().expect("EnumMember name");
        let name_kind = self.store().node(member_name).kind;
        // Go: ast.IsComputedNonLiteralName(member.Name()).
        let is_computed_non_literal_name = name_kind == Kind::ComputedPropertyName && {
            let expr = self
                .store()
                .node(member_name)
                .as_computed_property_name()
                .expect("ComputedPropertyName data")
                .expression;
            !matches!(
                self.store().node(expr).kind,
                Kind::StringLiteral | Kind::NumericLiteral | Kind::NoSubstitutionTemplateLiteral
            )
        };
        if is_computed_non_literal_name {
            self.error(member_name, &COMPUTED_PROPERTY_NAMES_ARE_NOT_ALLOWED_IN_ENUMS, &[]);
        } else if name_kind == Kind::BigIntLiteral {
            self.error(member_name, &AN_ENUM_MEMBER_CANNOT_HAVE_A_NUMERIC_NAME, &[]);
        } else {
            let text = try_get_text_of_property_name(self.store(), member_name);
            if is_numeric_literal_name_str(&text) && !u::is_infinity_or_nan_string(&text) {
                self.error(member_name, &AN_ENUM_MEMBER_CANNOT_HAVE_A_NUMERIC_NAME, &[]);
            }
        }
        if ast_shims::initializer_of(self.store(), member).is_some() {
            return self.compute_constant_enum_member_value(member);
        }
        // In ambient non-const numeric enum declarations, enum members
        // without initializers are considered computed members (as opposed
        // to having auto-incremented values).
        let parent = self.store().node(member).parent.get();
        if self.store().node(parent).flags.intersects(NF::AMBIENT)
            && !binder_shims::is_enum_const(self.store(), parent)
        {
            return EvaluatorResult::new_none();
        }
        // If the member declaration specifies no value, the member is
        // considered a constant enum member. If the member is the first
        // member in the enum declaration, it is assigned the value zero.
        // Otherwise, it is assigned the value of the immediately preceding
        // member plus one, and an error occurs if the immediately preceding
        // member is not a constant enum member.
        if auto_value.is_none() {
            self.error(member_name, &ENUM_MEMBER_MUST_HAVE_INITIALIZER, &[]);
            return EvaluatorResult::new_none();
        }
        if self.compiler_options.get_isolated_modules()
            && previous.is_some()
            && previous
                .and_then(|p| ast_shims::initializer_of(self.store(), p))
                .is_some()
        {
            if let Some(previous) = previous {
                let prev_value = self.get_enum_member_value(previous);
                match prev_value.value {
                    Some(EvaluatorValue::Number(_)) if !prev_value.resolved_other_files => {}
                    // TODO(port M6 evaluator wave): the previous initializer is
                    // not literal-evaluable by the permissive stub — Go's real
                    // evaluator decides prevIsNum here; the isolatedModules
                    // diagnostic waits for it rather than over-reporting.
                    _ => {
                        let _ = member_name;
                    }
                }
            }
        }
        EvaluatorResult {
            value: auto_value.map(EvaluatorValue::Number),
            is_syntactically_string: false,
            resolved_other_files: false,
            has_external_references: false,
        }
    }

    /// Go: `func (c *Checker) computeConstantEnumMemberValue(member) evaluator.Result` (24517).
    fn compute_constant_enum_member_value(&mut self, member: NodeId) -> EvaluatorResult {
        let parent = self.store().node(member).parent.get();
        let is_const_enum = binder_shims::is_enum_const(self.store(), parent);
        let initializer = ast_shims::initializer_of(self.store(), member).expect("checked by caller");
        let result = self.evaluate_stub(initializer, member);
        match result.value {
            Some(_) => {
                if is_const_enum {
                    // TODO(port M6 evaluator wave): the const-enum NaN/Inf
                    // initializer diagnostics — the stub only produces
                    // finite literal values, so this arm is dormant.
                }
                if self.compiler_options.get_isolated_modules()
                    && matches!(result.value, Some(EvaluatorValue::String(_)))
                    && !result.is_syntactically_string
                {
                    let member_name = self.store().node(member).name().expect("EnumMember name");
                    let enum_name = self
                        .store()
                        .node(parent)
                        .name()
                        .map(|n| ast_shims::get_text_of_node(self.store(), n))
                        .unwrap_or_default();
                    let name = format!(
                        "{}.{}",
                        enum_name,
                        ast_shims::get_text_of_node(self.store(), member_name)
                    );
                    self.error(
                        initializer,
                        &X_0_HAS_A_STRING_TYPE_BUT_MUST_HAVE_SYNTACTICALLY_RECOGNIZABLE_STRING_SYNTAX_WHEN_ISOLATEDMODULES_IS_ENABLED,
                        &[name],
                    );
                }
            }
            None => {
                // TODO(port M6 evaluator wave): the three Go arms here
                // (const-enum initializer must be constant, ambient-enum
                // initializer must be constant, and the assignability check
                // against numberType) require the real evaluator to
                // distinguish a failed constant computation from a
                // non-constant initializer; the permissive stub cannot, so
                // the diagnostics wait for the evaluator wave rather than
                // over-reporting on non-literal initializers.
            }
        }
        result
    }

    /// TODO(port M6 evaluator wave): `func (c *Checker) evaluate(expr, location)`
    /// — the const-evaluator. The permissive stub computes exactly the
    /// syntactic literals (plus unary +/- on a numeric literal) so the enum
    /// member auto-increment chain and the literal interning behave as Go
    /// for literal-valued enums; every other expression form resolves to
    /// nil value (a computed member), which is Go's answer for genuinely
    /// non-constant initializers.
    fn evaluate_stub(&mut self, expr: NodeId, _location: NodeId) -> EvaluatorResult {
        let node = self.store().node(expr);
        match node.kind {
            Kind::NumericLiteral => {
                let text = ast_shims::get_text_of_node(self.store(), expr);
                let n = tsc_jsnum::from_string(&text);
                if n.0.is_nan() && !text.contains("NaN") {
                    EvaluatorResult::new_none()
                } else {
                    EvaluatorResult {
                        value: Some(EvaluatorValue::Number(n)),
                        is_syntactically_string: false,
                        resolved_other_files: false,
                        has_external_references: false,
                    }
                }
            }
            Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral => {
                // Go: `expr.Text()` — the parser stores the scanner's
                // UNQUOTED token value in the literal payload's `text`
                // field (the raw source slice would keep the quotes).
                let text = match &self.store().node(expr).data {
                    NodeData::StringLiteral(d) => d.text.to_string(),
                    NodeData::NoSubstitutionTemplateLiteral(d) => d.text.to_string(),
                    _ => unreachable!("kind-checked above"),
                };
                EvaluatorResult {
                    value: Some(EvaluatorValue::String(text)),
                    is_syntactically_string: true,
                    resolved_other_files: false,
                    has_external_references: false,
                }
            }
            Kind::PrefixUnaryExpression => {
                let data = node.as_prefix_unary_expression().expect("PrefixUnaryExpression data");
                let operand = data.operand;
                let operator = data.operator;
                let operand_node = self.store().node(operand);
                if operand_node.kind == Kind::NumericLiteral {
                    let text = ast_shims::get_text_of_node(self.store(), operand);
                    let n = tsc_jsnum::from_string(&text);
                    if n.0.is_nan() && !text.contains("NaN") {
                        return EvaluatorResult::new_none();
                    }
                    let n = match operator {
                        Kind::MinusToken => tsc_jsnum::Number(-n.0),
                        Kind::PlusToken => n,
                        _ => return EvaluatorResult::new_none(),
                    };
                    EvaluatorResult {
                        value: Some(EvaluatorValue::Number(n)),
                        is_syntactically_string: false,
                        resolved_other_files: false,
                        has_external_references: false,
                    }
                } else {
                    EvaluatorResult::new_none()
                }
            }
            _ => EvaluatorResult::new_none(),
        }
    }

    /// Go: `func (c *Checker) getEnumLiteralType(value, enumSymbol, symbol) *Type` (25896).
    fn get_enum_literal_type(&mut self, value: LiteralValue, enum_symbol: SymbolId, symbol: SymbolId) -> TypeId {
        let flags = match &value {
            LiteralValue::String(_) => TypeFlags::ENUM_LITERAL | TypeFlags::STRING_LITERAL,
            LiteralValue::Number(n) => {
                // NaN cannot be used as a Go map key because NaN != NaN in
                // IEEE 754, so Go map lookups for NaN always miss. Cache NaN
                // enum types separately by enum symbol.
                if n.0.is_nan() {
                    if let Some(&t) = self.enum_nan_literal_types.get(&enum_symbol) {
                        return t;
                    }
                    let t = self.new_literal_type(
                        TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL,
                        value.clone(),
                        None,
                    );
                    self.type_mut(t).symbol = Some(symbol);
                    self.enum_nan_literal_types.insert(enum_symbol, t);
                    return t;
                }
                TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL
            }
            _ => panic!("Unhandled case in getEnumLiteralType"),
        };
        let key = EnumLiteralKey::new(enum_symbol, &value);
        if let Some(&t) = self.enum_literal_types.get(&key) {
            return t;
        }
        let t = self.new_literal_type(flags, value, None);
        self.type_mut(t).symbol = Some(symbol);
        self.enum_literal_types.insert(key, t);
        t
    }

    /// Go: `func (c *Checker) getDeclaredTypeOfAlias(symbol)` (24628).
    fn get_declared_type_of_alias(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(t) = self.declared_type_links.get(symbol).declared_type {
            return t;
        }
        let target = self.resolve_alias(symbol);
        let t = self.get_declared_type_of_symbol(target);
        if self.declared_type_links.get(symbol).declared_type.is_none() {
            self.declared_type_links.get(symbol).declared_type = Some(t);
        }
        self.declared_type_links.get(symbol).declared_type.expect("just set")
    }

    /// Go: `func (c *Checker) getTypeOfMappedSymbol(symbol) *Type` (21388).
    fn get_type_of_mapped_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self.value_symbol_links.get(symbol).resolved_type.is_none() {
            let mapped_type = self.value_symbol_links.get(symbol).containing_type;
            let mapped_type = match mapped_type {
                Some(t) => t,
                None => return self.error_type(),
            };
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::Type,
            ) {
                // TODO(port M6 S7): the mappedType.containsError write —
                // the permissive stub cannot reach a mapped type yet.
                return self.error_type();
            }
            // TODO(port M6 S7): getTemplateTypeFromMappedType /
            // getTypeParameterFromMappedType — the mapped-type component
            // resolvers are S7's; the permissive reads resolve to never,
            // so the property type instantiates to never until they land.
            // (The payload's target/mapper fields ride the mapped-type wave;
            // the reads are Option/None until then.)
            let target = self
                .type_(mapped_type)
                .as_mapped_type()
                .and_then(|m| m.base.target);
            let template_type = crate::relater::get_template_type_from_mapped_type(
                self,
                target.unwrap_or(mapped_type),
            );
            let key_type = self
                .mapped_symbol_links
                .get(symbol)
                .key_type
                .unwrap_or_else(|| self.never_type());
            let mapper_source = self
                .type_(mapped_type)
                .as_mapped_type()
                .and_then(|m| m.base.mapper);
            let mapped_type_parameter =
                crate::relater::get_type_parameter_from_mapped_type(self, mapped_type);
            let mapper = self.append_type_mapping(mapper_source, mapped_type_parameter, key_type);
            let mut prop_type = self.instantiate_type(template_type, mapper);
            // When creating an optional property in strictNullChecks mode,
            // if 'undefined' isn't assignable to the type, we include
            // 'undefined' in the type. Similarly, when creating a
            // non-optional property in strictNullChecks mode, if the
            // underlying property is optional we remove 'undefined' from
            // the type.
            if self.strict_null_checks
                && self.program.symbol(symbol).flags.intersects(SymbolFlags::OPTIONAL)
                && !crate::relater::maybe_type_of_kind(
                    self,
                    prop_type,
                    TypeFlags::UNDEFINED | TypeFlags::VOID,
                )
            {
                prop_type = self.get_optional_type(prop_type, true /*isProperty*/);
            } else if self
                .program
                .symbol(symbol)
                .check_flags
                .intersects(CheckFlags::STRIP_OPTIONAL)
            {
                prop_type = self.remove_missing_or_undefined_type(prop_type);
            }
            if self.pop_type_resolution() {
                if self.value_symbol_links.get(symbol).resolved_type.is_none() {
                    self.value_symbol_links.get(symbol).resolved_type = Some(prop_type);
                }
            } else {
                if self.value_symbol_links.get(symbol).resolved_type.is_none() {
                    let error_type = self.error_type();
                    self.value_symbol_links.get(symbol).resolved_type = Some(error_type);
                }
                let symbol_name = crate::relater::symbol_to_string(self, symbol);
                let mapped_type_string = self.type_to_string(mapped_type);
                self.error(
                    self.current_node.expect("c.currentNode"),
                    &TYPE_OF_PROPERTY_0_CIRCULARLY_REFERENCES_ITSELF_IN_MAPPED_TYPE_1,
                    &[symbol_name, mapped_type_string],
                );
            }
        }
        self.value_symbol_links.get(symbol).resolved_type.expect("just set")
    }
}

/// Go: `ast.TryGetTextOfPropertyName` (utilities.go:2192) — the local form.
fn try_get_text_of_property_name(store: &dyn NodeStore, name: NodeId) -> String {
    let node = store.node(name);
    match node.kind {
        Kind::Identifier
        | Kind::PrivateIdentifier
        | Kind::StringLiteral
        | Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::NoSubstitutionTemplateLiteral => ast_shims::get_text_of_node(store, name),
        Kind::ComputedPropertyName => {
            let expr = node
                .as_computed_property_name()
                .expect("ComputedPropertyName data")
                .expression;
            if matches!(
                store.node(expr).kind,
                Kind::StringLiteral | Kind::NumericLiteral
            ) {
                ast_shims::get_text_of_node(store, expr)
            } else {
                String::new()
            }
        }
        Kind::JsxNamespacedName => String::new(),
        _ => String::new(),
    }
}

/// Go: `isNumericLiteralName` — utilities' form (leading-numeric test).
fn is_numeric_literal_name_str(name: &str) -> bool {
    u::is_numeric_literal_name(name)
}

// ────────────────────────────────────────────────────────────────────────────
// In-module unit tests (the S2 slice's internal units that the pub surface
// does not reach: flags-ex, the enum-literal interning tables, and the
// symbol-with-type / undefined-property caches).
// ────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn new_checker() -> Checker {
        crate::mapper::tests_support::new_empty_checker()
    }

    /// Go: getSymbolFlagsEx — the non-alias walk returns the symbol's own
    /// flags; the exclude arms drop local meanings (Go's excludeFlags mask).
    #[test]
    fn get_symbol_flags_ex_reads_own_flags_and_honors_the_exclude_arms() {
        let mut c = new_checker();
        let sym = c.get_unknown_symbol();
        let own = c.program_symbol(sym).flags;
        assert_eq!(c.get_symbol_flags_ex(sym, false, false), own);
        assert_eq!(c.get_symbol_flags_ex(sym, true, false), own);
        // excludeLocalMeanings: Go seeds flags = NONE for a non-alias symbol.
        assert_eq!(c.get_symbol_flags_ex(sym, false, true), SymbolFlags::NONE);
    }

    /// Go: getEnumLiteralType — the (enum, value) pair interns in
    /// enumLiteralTypes; string and number members get their literal flags;
    /// NaN cannot be a map key, so NaN members intern separately per enum.
    #[test]
    fn get_enum_literal_type_interns_by_value_and_caches_nan_separately() {
        let mut c = new_checker();
        let enum_symbol = c.new_symbol(SymbolFlags::REGULAR_ENUM, "E");
        let a = c.new_symbol(SymbolFlags::ENUM_MEMBER, "A");
        let b = c.new_symbol(SymbolFlags::ENUM_MEMBER, "B");
        let zero = c.get_enum_literal_type(LiteralValue::Number(tsc_jsnum::Number(0.0)), enum_symbol, a);
        let zero_again = c.get_enum_literal_type(LiteralValue::Number(tsc_jsnum::Number(0.0)), enum_symbol, b);
        assert_eq!(zero, zero_again, "the (enum, value) pair interns");
        assert!(c.type_(zero).flags.intersects(TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL));
        assert_eq!(c.type_(zero).symbol, Some(a), "the first symbol that created the type is attached");
        let one = c.get_enum_literal_type(LiteralValue::Number(tsc_jsnum::Number(1.0)), enum_symbol, b);
        assert_ne!(zero, one, "distinct values intern distinctly");
        let s = c.get_enum_literal_type(LiteralValue::String("x".to_string()), enum_symbol, b);
        assert!(c.type_(s).flags.intersects(TypeFlags::ENUM_LITERAL | TypeFlags::STRING_LITERAL));
        // NaN: the separate per-enum cache (IEEE 754 NaN != NaN means the
        // main table's key lookups always miss for it).
        let nan = tsc_jsnum::Number(f64::NAN);
        let nan_a = c.get_enum_literal_type(LiteralValue::Number(nan), enum_symbol, a);
        let nan_b = c.get_enum_literal_type(LiteralValue::Number(nan), enum_symbol, b);
        assert_eq!(nan_a, nan_b, "NaN enum literals cache by enum symbol");
        assert_ne!(nan_a, zero);
        assert!(c.type_(nan_a).flags.intersects(TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL));
    }

    /// Go: createSymbolWithType — the new symbol copies the source's flags,
    /// name, and identity, carries the given resolved type, and leaves the
    /// source untouched.
    #[test]
    fn create_symbol_with_type_copies_identity_and_seeds_the_resolved_type() {
        let mut c = new_checker();
        let source = c.get_unknown_symbol();
        let source_type_before = c.value_symbol_links.get(source).resolved_type;
        let t = c.string_type();
        let created = c.create_symbol_with_type(source, t);
        assert_ne!(created, source);
        assert_eq!(c.program_symbol(created).name, c.program_symbol(source).name);
        assert_eq!(c.program_symbol(created).flags, c.program_symbol(source).flags);
        assert_eq!(c.value_symbol_links.get(created).resolved_type, Some(t));
        // The source's link is untouched (the unknown symbol's own seeded
        // resolved type, whatever it was, survives).
        assert_eq!(
            c.value_symbol_links.get(source).resolved_type,
            source_type_before
        );
    }

    /// Go: getUndefinedProperty — the widened undefined-property symbol caches
    /// by name, carries the OPTIONAL flag, and resolves to
    /// undefinedOrMissingType.
    #[test]
    fn get_undefined_property_caches_by_name_with_the_optional_flag() {
        let mut c = new_checker();
        let prop = c.get_unknown_symbol(); // name "unknown"
        let undefined_prop = c.get_undefined_property(prop);
        assert!(c.program_symbol(undefined_prop).flags.intersects(SymbolFlags::OPTIONAL));
        let expected = c.undefined_or_missing_type();
        assert_eq!(c.value_symbol_links.get(undefined_prop).resolved_type, Some(expected));
        // Same name → the cached symbol; a different name → a fresh one.
        assert_eq!(c.get_undefined_property(c.get_unknown_symbol()), undefined_prop);
        let other = c.new_symbol(SymbolFlags::PROPERTY, "other");
        let other_prop = c.get_undefined_property(other);
        assert_ne!(other_prop, undefined_prop);
    }
}
