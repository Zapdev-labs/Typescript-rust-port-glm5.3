// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// M6 S5 — the TYPE-NODE family (Go checker.go 23295–25536, minus the S7 new*
// ctor block 25537+ and the S2-landed getDeclaredTypeOf* / enum-value spans).
// One-.rs-per-region split per the M6-DISPATCH slice table:
//
//   - getTypeFromTypeNode(Worker) routing + the ~20 getTypeFrom*TypeNode arms
//   - type-reference resolution (getTypeReferenceType,
//     getTypeFromClassOrInterfaceReference, getTypeFromTypeAliasReference,
//     getTypeAliasInstantiation, getTypeArgumentsFromNode,
//     checkNoTypeArguments, the deferred/mayResolveTypeAlias predicates)
//   - tuple construction (getTupleTargetType/createTupleTargetType/
//     createNormalizedTupleTypeEx/TupleNormalizer + the element-flag
//     helpers) and the array/tuple is* predicates (Go 24005–24084 — their
//     W1 permissive stubs in relater.rs now delegate to the real bodies
//     here)
//   - the type-argument default machinery (getMinTypeArgumentCount/
//     fillMissingTypeArguments/getDefaultFromTypeParameter — Go 22392–22510)
//   - getTypeParametersForTypeReferenceOrImport (Go 17574 — the S3
//     checkTypeReferenceOrImport seam this wave was bannered for)
//   - the conditional-flow wrapper (getConditionalFlowTypeOfType) + the
//     implied-constraint walk
//
// Stub directions this wave leaves in place (per the dispatch table):
//   - S6: resolveEntityName (resolveTypeReferenceName resolves nothing —
//     type references become unresolved-alias symbols exactly like Go's
//     name-failure path), resolveExternalModuleName/Symbol, getExportsOfSymbol
//   - S7: getUnionType/getIntersectionType/getTemplateLiteralType/
//     getStringMappingType constructors, instantiateType (identity),
//     createTypeReference (identity), createArrayType/createTupleTypeEx
//     reference construction
//   - S8: resolveStructuredTypeMembers seam (getPropertyOfType /
//     getConstraintTypeFromMappedType read through the S8 stubs)
//   - S9: getConditionalType (the deferred-conditional resolution loop,
//     checker.go 24801–25137) + isTypeParameterPossiblyReferenced
//   - S10: getSubstitutionType/getOrCreateSubstitutionType (checker.go
//     27945), isPatternLiteralType/isGenericStringLikeType (27050/27057),
//     getIndexType (keyof)
//
// PORT notes where Rust forces a change are inline (Option vs nil, the
// LinkStore get/set split for resolvedType caching, and the arena SymbolId
// id reads for the unique-symbol name).

use tsc_ast::ast_generated::{
    is_interface_declaration, is_type_reference_node,
};
use tsc_ast::{CheckFlags, Kind, Node, NodeFlags as NF, NodeId, SymbolFlags, SymbolId};
use tsc_core::tristate::Tristate;
use tsc_binder::ast_shims as binder_shims;
use tsc_binder::node_data as binder_node_data;
use tsc_diagnostics as diag;

use crate::todo_shapes::TypeMapperId;
use crate::types::{
    self, AccessFlags, ElementFlags, ObjectFlags, TupleElementInfo, TypeAlias,
    TypeData, TypeFlags, TypeId,
};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// Local ast-package helpers (Go ast/utilities.go — the three the slice needs
// that no landed crate exports yet; fold into tsc-ast's utilities wave).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `node.Type()` for the type-node payloads this family recurses through.
fn type_node_type(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::ParenthesizedType => Some(n.as_parenthesized_type_node().expect("data").type_),
        Kind::OptionalType => Some(n.as_optional_type_node().expect("data").type_),
        Kind::RestType => Some(n.as_rest_type_node().expect("data").type_),
        Kind::TypeOperator => Some(n.as_type_operator_node().expect("data").type_),
        Kind::NamedTupleMember => Some(n.as_named_tuple_member().expect("data").type_),
        Kind::JSDocNonNullableType => Some(n.as_jsdoc_non_nullable_type().expect("data").type_),
        Kind::JSDocNullableType => Some(n.as_jsdoc_nullable_type().expect("data").type_),
        Kind::JSDocOptionalType => Some(n.as_jsdoc_optional_type().expect("data").type_),
        Kind::JSDocVariadicType => Some(n.as_jsdoc_variadic_type().expect("data").type_),
        _ => None,
    }
}

/// Go: `func (c *Checker) isSimpleTupleType(node) bool` (checker.go:24968) —
/// a plain-tuple check with no optional/rest/variadic-named elements.
/// PORT: a free function (Go's receiver only carries the node store).
fn is_simple_tuple_type(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    use tsc_ast::Kind;
    if store.node(node).kind != Kind::TupleType {
        return false;
    }
    let elements = elements_of(store, node);
    !elements.is_empty()
        && !elements.iter().any(|&e| {
            let n = store.node(e);
            n.kind == Kind::OptionalType
                || n.kind == Kind::RestType
                || (n.kind == Kind::NamedTupleMember
                    && n.as_named_tuple_member()
                        .is_some_and(|d| d.question_token.is_some() || d.dot_dot_dot_token.is_some()))
        })
}

/// Go: `someType(c.getPermissiveInstantiation(inferredExtendsType), t =>
/// c.isTypeAssignableTo(t, c.getPermissiveInstantiation(checkType)))`
/// (checker.go:24893) — the forConstraint reverse-assignability probe.
/// PORT: the constituents are collected before the mutable probe (the
/// closure can't capture `&mut Checker` under `some_type`'s borrow).
fn some_type_extends_assignable_to_check(c: &mut Checker, extends: TypeId, check: TypeId) -> bool {
    let constituents: Vec<TypeId> = if c.type_(extends).flags.intersects(TypeFlags::UNION) {
        c.type_(extends).types().to_vec()
    } else {
        vec![extends]
    };
    constituents.iter().any(|&t| c.is_type_assignable_to(t, check))
}

/// Go: `ast.SkipTypeParentheses(node)` (utilities.go:833).
#[allow(dead_code)] // the element-access/spread waves consume it
fn skip_type_parentheses(store: &dyn tsc_ast::NodeStore, mut node: NodeId) -> NodeId {
    while tsc_ast::ast_generated::is_parenthesized_type_node(store.node(node)) {
        node = store
            .node(node)
            .as_parenthesized_type_node()
            .expect("ParenthesizedTypeNode data")
            .type_;
    }
    node
}

/// Go: `ast.WalkUpParenthesizedTypes(node)` (utilities.go:853).
fn walk_up_parenthesized_types(store: &dyn tsc_ast::NodeStore, mut node: NodeId) -> NodeId {
    while store.node(node).kind == Kind::ParenthesizedType {
        node = store.node(node).parent.get();
    }
    node
}

/// Go: `ast.IsTypeReferenceType(node)` (utilities.go:3104).
fn is_type_reference_type(node: &Node) -> bool {
    node.kind == Kind::TypeReference || node.kind == Kind::ExpressionWithTypeArguments
}

/// Go: `isStatementKindButNotDeclarationKind(kind)` (utilities.go:657).
fn is_statement_kind_but_not_declaration_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::BreakStatement
            | Kind::ContinueStatement
            | Kind::DebuggerStatement
            | Kind::DoStatement
            | Kind::ExpressionStatement
            | Kind::EmptyStatement
            | Kind::ForInStatement
            | Kind::ForOfStatement
            | Kind::ForStatement
            | Kind::IfStatement
            | Kind::LabeledStatement
            | Kind::ReturnStatement
            | Kind::SwitchStatement
            | Kind::ThrowStatement
            | Kind::TryStatement
            | Kind::VariableStatement
            | Kind::WhileStatement
            | Kind::WithStatement
            | Kind::NotEmittedStatement
    )
}

/// Go: `isBlockStatement(node)` (utilities.go:701) — a Block that is not part
/// of a `try` statement, a `catch` clause, or the Block-like body of a
/// function.
fn is_block_statement(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::Block {
        return false;
    }
    let parent = store.node(node).parent.get();
    if parent != NodeId::NONE {
        let parent_kind = store.node(parent).kind;
        if parent_kind == Kind::TryStatement || parent_kind == Kind::CatchClause {
            return false;
        }
    }
    // Go: !IsFunctionBlock(node) — the parent is not function-like.
    !(parent != NodeId::NONE && binder_shims::is_function_like(store.node(parent)))
}

/// Go: `ast.IsStatement(node)` (utilities.go:695).
fn is_statement(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    let kind = store.node(node).kind;
    is_statement_kind_but_not_declaration_kind(kind)
        || binder_shims::is_declaration_statement_kind(kind)
        || is_block_statement(store, node)
}

/// Go: `node.Elements()` for TupleTypeNode (the local form the slice uses;
/// statements.rs's members_of owns the member-list payloads).
pub(crate) fn elements_of(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Vec<NodeId> {
    store
        .node(node)
        .as_tuple_type_node()
        .expect("TupleTypeNode data")
        .elements
        .as_ref()
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default()
}

/// Go: `node.AsUnionTypeNode().Types.Nodes`.
fn union_types_of(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Vec<NodeId> {
    store
        .node(node)
        .as_union_type_node()
        .expect("UnionTypeNode data")
        .types
        .as_ref()
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default()
}

/// Go: `node.AsIntersectionTypeNode().Types.Nodes`.
fn intersection_types_of(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Vec<NodeId> {
    store
        .node(node)
        .as_intersection_type_node()
        .expect("IntersectionTypeNode data")
        .types
        .as_ref()
        .map(|l| l.nodes.to_vec())
        .unwrap_or_default()
}

/// Go: `isValidESSymbolDeclaration(node)` (utilities.go:1004).
fn is_valid_essymbol_declaration(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if tsc_ast::ast_generated::is_variable_declaration(n) {
        return crate::ast_shims::is_var_const(store, node)
            && tsc_ast::ast_generated::is_identifier(
                store.node(n.as_variable_declaration().expect("data").name),
            )
            && u::is_variable_declaration_in_variable_statement(store, node);
    }
    if tsc_ast::ast_generated::is_property_declaration(n) {
        return u::has_readonly_modifier(n) && binder_shims::has_static_modifier(n);
    }
    tsc_ast::ast_generated::is_property_signature_declaration(n) && u::has_readonly_modifier(n)
}

/// Go: `getTypeReferenceName(node)` (checker.go:23600).
fn get_type_reference_name(store: &dyn tsc_ast::NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::TypeReference => Some(n.as_type_reference_node().expect("data").type_name),
        Kind::ExpressionWithTypeArguments => {
            // We only support expressions that are simple qualified names. For
            // other expressions this produces nil.
            let expr = crate::ast_shims::expression_of(store, node)?;
            if tsc_ast::utilities::is_entity_name_expression(store, expr) {
                Some(expr)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go package-level functions)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getSymbolPath(symbol *ast.Symbol) string` (checker.go:23640).
fn get_symbol_path(c: &Checker, symbol: SymbolId) -> String {
    let s = c.program_symbol(symbol);
    match s.parent {
        Some(parent) => format!("{}.{}", get_symbol_path(c, parent), s.name),
        None => s.name.clone(),
    }
}

/// Go: `func isSimpleIdentifierTypeReference(node) bool` (checker.go:23466).
fn is_simple_identifier_type_reference(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    is_type_reference_node(store.node(node))
        && tsc_ast::ast_generated::is_identifier(
            store.node(store.node(node).as_type_reference_node().expect("data").type_name),
        )
        && crate::statements::type_arguments_of(store, node).is_empty()
}

/// Go: `func isUnaryTupleTypeNode(node) bool` (checker.go:25534).
fn is_unary_tuple_type_node(store: &dyn tsc_ast::NodeStore, node: NodeId) -> bool {
    tsc_ast::ast_generated::is_tuple_type_node(store.node(node))
        && elements_of(store, node).len() == 1
}

/// Go: `func getStartElementCount(t *TupleType, flags ElementFlags) int` (checker.go:23894).
#[allow(dead_code)] // the tuple-assignment/spread waves consume it
fn get_start_element_count(t: &types::TupleType, flags: ElementFlags) -> usize {
    for (i, info) in t.element_infos.iter().enumerate() {
        if !info.flags.intersects(flags) {
            return i;
        }
    }
    t.element_infos.len()
}

/// Go: `func getEndElementCount(t *TupleType, flags ElementFlags) int` (checker.go:23901).
pub(crate) fn get_end_element_count(t: &types::TupleType, flags: ElementFlags) -> usize {
    for i in (1..=t.element_infos.len()).rev() {
        if !t.element_infos[i - 1].flags.intersects(flags) {
            return t.element_infos.len() - i;
        }
    }
    t.element_infos.len()
}

/// Go: `func getTotalFixedElementCount(t *TupleType) int` (checker.go:23910).
fn get_total_fixed_element_count(t: &types::TupleType) -> i32 {
    t.fixed_length + get_end_element_count(t, ElementFlags::FIXED) as i32
}

/// The pub(crate) carrier of [`get_total_fixed_element_count`] for index.rs's
/// shouldDeferIndexedAccessType / getSimplifiedIndexedAccessTypeWorker.
pub(crate) fn total_fixed_element_count_of(t: &types::TupleType) -> i32 {
    get_total_fixed_element_count(t)
}

/// Go: `type IntrinsicTypeKind int32` + `var intrinsicTypeKinds` (checker.go
/// checker.go's intrinsic-marker table). statements.rs's
/// `intrinsic_type_kind_of` keeps the S3 error-arm shape; this enum carries
/// the kind the alias-instantiation switch needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IntrinsicTypeKind {
    /// `NoInfer`
    NoInfer,
    /// Uppercase/Lowercase/Capitalize/Uncapitalize — the string-mapping kinds.
    StringMapping,
}

/// Go: `intrinsicTypeKinds[symbol.Name]` lookup.
fn intrinsic_type_kind(name: &str) -> Option<IntrinsicTypeKind> {
    match name {
        "NoInfer" => Some(IntrinsicTypeKind::NoInfer),
        "Uppercase" | "Lowercase" | "Capitalize" | "Uncapitalize" => {
            Some(IntrinsicTypeKind::StringMapping)
        }
        _ => None,
    }
}

/// Go: `func (c *Checker) isPatternLiteralType(t) bool` (checker.go:27050)
/// — REAL (M6 S10): a pattern literal type is a template literal or a string
/// mapping type that contains only non-generic pattern literal placeholders.
pub(crate) fn is_pattern_literal_type(c: &mut Checker, t: TypeId) -> bool {
    let flags = c.type_(t).flags;
    if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        let types = c.type_(t)
            .as_template_literal_type()
            .expect("TemplateLiteralType data")
            .types
            .clone();
        return types.iter().all(|&u| c.is_pattern_literal_placeholder_type(u));
    }
    if flags.intersects(TypeFlags::STRING_MAPPING) {
        let target = c
            .type_(t)
            .as_string_mapping_type()
            .expect("StringMappingType data")
            .target
            .expect("string-mapping target");
        return c.is_pattern_literal_placeholder_type(target);
    }
    false
}

/// Go: `func (c *Checker) isGenericStringLikeType(t) bool` (checker.go:27057)
/// — REAL (M6 S10).
pub(crate) fn is_generic_string_like_type(c: &mut Checker, t: TypeId) -> bool {
    c.type_(t)
        .flags
        .intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
        && !is_pattern_literal_type(c, t)
}

/// Go: `func (c *Checker) getGenericObjectFlags(t) ObjectFlags` (checker.go:25495)
/// — REAL body (the union/intersection memoization on objectFlags included;
/// the mapped-type arms compose over the S8 constraint/nameType seams).
pub(crate) fn get_generic_object_flags(c: &mut Checker, t: TypeId) -> ObjectFlags {
    let ty = c.type_(t);
    if ty
        .flags
        .intersects(TypeFlags::UNION_OR_INTERSECTION.union(TypeFlags::SUBSTITUTION))
    {
        let computed = ty
            .object_flags
            .intersects(ObjectFlags::IS_GENERIC_TYPE_COMPUTED);
        let is_union_or_intersection = ty.flags.intersects(TypeFlags::UNION_OR_INTERSECTION);
        let substitution = ty
            .as_substitution_type()
            .map(|d| (d.base_type(), d.subst_constraint()));
        if !computed {
            let mut combined_flags = ObjectFlags::NONE;
            if is_union_or_intersection {
                let types: Vec<TypeId> = c.type_(t).types().to_vec();
                for u in types {
                    combined_flags =
                        combined_flags.union(get_generic_object_flags(c, u));
                }
            } else if let Some((base, subst_constraint)) = substitution {
                // Go: d.base | getSubstitutionConstraint(d.subst).
                combined_flags = base
                    .map(|b| get_generic_object_flags(c, b))
                    .unwrap_or(ObjectFlags::NONE)
                    .union(
                        subst_constraint
                            .map(|s| get_generic_object_flags(c, s))
                            .unwrap_or_else(|| get_generic_object_flags(c, t)),
                    );
            }
            c.type_mut(t).object_flags |=
                ObjectFlags::IS_GENERIC_TYPE_COMPUTED.union(combined_flags);
        }
        return c.type_(t).object_flags & ObjectFlags::IS_GENERIC_TYPE;
    }
    let mut combined_flags = ObjectFlags::NONE;
    let is_instantiable_or_index = c
        .type_(t)
        .flags
        .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE.union(TypeFlags::INDEX))
        || is_generic_string_like_type(c, t);
    if c
        .type_(t)
        .flags
        .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
        || crate::relater::is_generic_mapped_type(c, t)
        || is_generic_tuple_type(c, t)
    {
        combined_flags |= ObjectFlags::IS_GENERIC_OBJECT_TYPE;
    }
    if is_instantiable_or_index {
        combined_flags |= ObjectFlags::IS_GENERIC_INDEX_TYPE;
    }
    combined_flags
}

/// Go: `func isTupleType(t) bool` — the free form delegates to the Checker
/// method (checker.rs:1080 — the arena target read; §5.4).
pub(crate) fn is_tuple_type(c: &Checker, t: TypeId) -> bool {
    c.is_tuple_type(t)
}

/// Go: `func isMutableTupleType(t) bool` (checker.go:24018).
pub(crate) fn is_mutable_tuple_type(c: &Checker, t: TypeId) -> bool {
    is_tuple_type(c, t)
        && !c.type_(t)
            .target()
            .and_then(|target| c.type_(target).as_tuple_type())
            .is_some_and(|d| d.readonly)
}

/// Go: `func isGenericTupleType(t) bool` (checker.go:24021).
pub(crate) fn is_generic_tuple_type(c: &Checker, t: TypeId) -> bool {
    is_tuple_type(c, t)
        && c.type_(t)
            .target()
            .and_then(|target| c.type_(target).as_tuple_type())
            .is_some_and(|d| d.combined_flags.intersects(ElementFlags::VARIADIC))
}

/// Go: `func isSingleElementGenericTupleType(t) bool` (checker.go:24024).
pub(crate) fn is_single_element_generic_tuple_type(c: &Checker, t: TypeId) -> bool {
    is_generic_tuple_type(c, t)
        && c.type_(t)
            .target()
            .and_then(|target| c.type_(target).as_tuple_type())
            .is_some_and(|d| d.element_infos.len() == 1)
}

/// Go: `func everyType(t, f) bool` (utilities.go — the union/
/// intersection-distributing predicate; the local form threads `&mut
/// Checker` through the closure where relater's free fn cannot share the
/// borrow).
pub(crate) fn every_type_ex(
    c: &mut Checker,
    t: TypeId,
    mut f: impl FnMut(&mut Checker, TypeId) -> bool,
) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        let types: Vec<TypeId> = c.type_(t).types().to_vec();
        types.into_iter().all(|s| f(c, s))
    } else {
        f(c, t)
    }
}

/// Go: `func (c *Checker) isGenericReducibleType(t) bool` (checker.go:25481).
#[allow(dead_code)] // the S7 reduce* engine consumes it
pub(crate) fn is_generic_reducible_type(c: &mut Checker, t: TypeId) -> bool {
    let ty = c.type_(t);
    let is_union = ty.flags.intersects(TypeFlags::UNION);
    let is_intersection = ty.flags.intersects(TypeFlags::INTERSECTION);
    let contains_intersections = ty.object_flags.intersects(ObjectFlags::CONTAINS_INTERSECTIONS);
    let constituents: Option<Vec<TypeId>> =
        is_union.then(|| c.type_(t).types().to_vec());
    (is_union
        && contains_intersections
        && constituents.is_some_and(|cs| cs.into_iter().any(|u| is_generic_reducible_type(c, u))))
        || (is_intersection && is_reducible_intersection(c, t))
}

/// Go: `func (c *Checker) isReducibleIntersection(t) bool` (checker.go:25486)
/// — composed over the S7 instantiateType/getReducedType seams (identity
/// ⇒ never reducible until the instantiation engine lands).
#[allow(dead_code)] // the S7 reduce* engine consumes it
pub(crate) fn is_reducible_intersection(c: &mut Checker, t: TypeId) -> bool {
    let unique_literal_filled_instantiation = {
        let existing = c
            .type_(t)
            .as_intersection_type()
            .and_then(|d| d.unique_literal_filled_instantiation);
        match existing {
            Some(i) => i,
            None => {
                // REAL (M6 S7): instantiateType over the seeded
                // uniqueLiteralMapper (uniontypes.rs's engine); the memo
                // field preserves Go's fill-once answer.
                let mapper = match c.unique_literal_mapper {
                    Some(m) => m,
                    None => return false,
                };
                let i = c.instantiate_type(t, mapper);
                if let Some(d) = c.type_mut(t).as_intersection_type_mut() {
                    d.unique_literal_filled_instantiation = Some(i);
                }
                i
            }
        }
    };
    crate::relater::get_reduced_type(c, unique_literal_filled_instantiation)
        != unique_literal_filled_instantiation
}

/// Go: `func (c *Checker) getUniqueLiteralTypeForTypeParameter(t) *Type` (checker.go:25494).
#[allow(dead_code)] // the instantiate* engine (S7) consumes it
pub(crate) fn get_unique_literal_type_for_type_parameter(c: &Checker, t: TypeId) -> TypeId {
    if c.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
        return c.unique_literal_type();
    }
    t
}

// ────────────────────────────────────────────────────────────────────────────
// The checker's array/tuple predicates (Go checker.go 24005–24084 — real
// bodies; the W1 permissive free-fn stubs in relater.rs delegate here).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (c *Checker) isArrayLikeType(t) bool` (checker.go:24049).
pub(crate) fn is_array_like_type(c: &mut Checker, t: TypeId) -> bool {
    // A type is array-like if it is a reference to the global Array or global
    // ReadonlyArray type, or if it is not the undefined or null type and if it
    // is assignable to ReadonlyArray<any>.
    c.is_array_type(t)
        || (!c.type_(t).flags.intersects(TypeFlags::NULLABLE)
            && c.is_type_assignable_to(t, c.any_readonly_array_type.expect("anyReadonlyArrayType")))
}

/// Go: `func (c *Checker) isTupleLikeType(t) bool` (utilities.go twin —
/// checker.go:24068).
pub(crate) fn is_tuple_like_type(c: &mut Checker, t: TypeId) -> bool {
    if is_tuple_type(c, t) || c.get_property_of_type(t, "0").is_some() {
        return true;
    }
    if is_array_like_type(c, t) {
        if let Some(length_type) = c.get_type_of_property_of_type(t, "length") {
            return every_type_ex(c, length_type, |c, t| {
                c.type_(t).flags.intersects(TypeFlags::NUMBER_LITERAL)
            });
        }
    }
    false
}

/// Go: `func (c *Checker) isArrayOrTupleLikeType(t) bool` (checker.go:24082).
#[allow(dead_code)] // callers land with the iteration/spread waves
pub(crate) fn is_array_or_tuple_like_type(c: &mut Checker, t: TypeId) -> bool {
    is_array_like_type(c, t) || is_tuple_like_type(c, t)
}

/// Go: `func (c *Checker) isArrayOrTupleOrIntersection(t) bool` (checker.go:24085).
#[allow(dead_code)] // callers land with the iteration/spread waves
pub(crate) fn is_array_or_tuple_or_intersection(c: &mut Checker, t: TypeId) -> bool {
    c.type_(t).flags.intersects(TypeFlags::INTERSECTION)
        && every_type_ex(c, t, crate::relater::is_array_or_tuple_type)
}

/// Go: `func (c *Checker) getTupleElementType(t, index) *Type` (checker.go:24088).
#[allow(dead_code)] // the contextual/element-access waves consume it
pub(crate) fn get_tuple_element_type(
    c: &mut Checker,
    t: TypeId,
    index: usize,
) -> Option<TypeId> {
    let prop_type = c.get_type_of_property_of_type(t, &index.to_string());
    if let Some(prop_type) = prop_type {
        return Some(prop_type);
    }
    if every_type_ex(c, t, |c, t| is_tuple_type(c, t)) {
        let undefined_like_type = if c.compiler_options.no_unchecked_indexed_access == Tristate::TSTrue
        {
            Some(c.undefined_type())
        } else {
            None
        };
        return Some(get_tuple_element_type_out_of_start_count(
            c,
            t,
            tsc_jsnum::Number::from(index as f64),
            undefined_like_type,
        ));
    }
    None
}

/// Go: `func (c *Checker) getTupleElementTypeOutOfStartCount(t, index, undefinedLikeType)` (checker.go:25380).
pub(crate) fn get_tuple_element_type_out_of_start_count(
    c: &mut Checker,
    t: TypeId,
    index: tsc_jsnum::Number,
    undefined_like_type: Option<TypeId>,
) -> TypeId {
    map_type(c, t, |c, t| {
        let rest_type = get_rest_type_of_tuple_type(c, t);
        let Some(rest_type) = rest_type else {
            return c.undefined_type();
        };
        let total_fixed = c
            .type_(t)
            .target()
            .and_then(|target| c.type_(target).as_tuple_type())
            .map(get_total_fixed_element_count)
            .unwrap_or(0);
        if let Some(undefined_like_type) = undefined_like_type
            && index >= tsc_jsnum::Number::from(total_fixed as f64)
        {
            return crate::relater::get_union_type(c, vec![rest_type, undefined_like_type]);
        }
        rest_type
    })
}

/// Go: `func (c *Checker) mapType(t, f)` (checker.go:27004 region — the union
/// distributing map; a checker method in Go, free-fn shape here to keep the
/// `&mut` reborrow out of the callers).
pub(crate) fn map_type(c: &mut Checker, t: TypeId, mut f: impl FnMut(&mut Checker, TypeId) -> TypeId) -> TypeId {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let constituents = c.type_(t).types().to_vec();
        let mapped: Vec<TypeId> = constituents.into_iter().map(|t| f(c, t)).collect();
        crate::relater::get_union_type(c, mapped)
    } else {
        f(c, t)
    }
}

/// Go: `func (c *Checker) getElementTypeOfSliceOfTupleType(t, index, endSkipCount, writing, noReductions)` (checker.go:25347)
/// — REAL body (replaces the W2 permissive helper in inference.rs).
#[allow(clippy::too_many_arguments)]
pub(crate) fn get_element_type_of_slice_of_tuple_type(
    c: &mut Checker,
    t: TypeId,
    index: usize,
    end_skip_count: usize,
    writing: bool,
    no_reductions: bool,
) -> Option<TypeId> {
    let arity = crate::relater::get_type_reference_arity(c, t) as usize;
    let length = arity.saturating_sub(end_skip_count);
    let element_infos: Vec<TupleElementInfo> = c
        .type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type().map(|d| d.element_infos.clone()))
        .unwrap_or_default();
    if index < length {
        // PORT(FCS tranche 3): Go's resolving getTypeArguments (relater.rs's
        // resolve_type_arguments) guarantees arity-matched arguments.
        let type_arguments = crate::relater::resolve_type_arguments(c, t);
        let mut element_types = Vec::new();
        for i in index..length {
            let mut e = type_arguments[i];
            if element_infos[i].flags.intersects(ElementFlags::VARIADIC) {
                e = c.get_indexed_access_type(e, c.number_type());
            }
            element_types.push(e);
        }
        if writing {
            return Some(crate::relater::get_intersection_type(c, element_types));
        }
        return Some(crate::relater::get_union_type_ex(
            c,
            element_types,
            if no_reductions {
                crate::todo_shapes::UnionReduction::None
            } else {
                crate::todo_shapes::UnionReduction::Literal
            },
            None,
            None,
        ));
    }
    None
}

/// Go: `func (c *Checker) getRestTypeOfTupleType(t) *Type` (checker.go:25379).
pub(crate) fn get_rest_type_of_tuple_type(c: &mut Checker, t: TypeId) -> Option<TypeId> {
    let fixed_length = c
        .type_(t)
        .target()
        .and_then(|target| c.type_(target).as_tuple_type())
        .map(|d| d.fixed_length)
        .unwrap_or(0) as usize;
    get_element_type_of_slice_of_tuple_type(c, t, fixed_length, 0, false, false)
}

// ────────────────────────────────────────────────────────────────────────────
// TupleNormalizer (Go checker.go:23820–23893)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type TupleNormalizer struct` (checker.go:23821).
struct TupleNormalizer {
    types: Vec<TypeId>,
    infos: Vec<TupleElementInfo>,
    last_required_index: isize,
    first_rest_index: isize,
    last_optional_or_rest_index: isize,
}

// PORT: Go holds `c *Checker` inside the normalizer and mutates it freely;
// the crate forbids `unsafe`, so `&mut Checker` threads through
// `normalize_worker`/`add` instead — the control flow stays Go-identical.
impl TupleNormalizer {
    /// Go: `func (n *TupleNormalizer) normalize(c, elementTypes, elementInfos) bool`.
    fn normalize(
        c: &mut Checker,
        element_types: &[TypeId],
        element_infos: &[TupleElementInfo],
    ) -> Option<(Vec<TypeId>, Vec<TupleElementInfo>)> {
        let mut n = TupleNormalizer {
            types: Vec::new(),
            infos: Vec::new(),
            last_required_index: -1,
            first_rest_index: -1,
            last_optional_or_rest_index: -1,
        };
        if !n.normalize_worker(c, element_types, element_infos) {
            return None;
        }
        Some((n.types, n.infos))
    }

    fn normalize_worker(
        &mut self,
        c: &mut Checker,
        element_types: &[TypeId],
        element_infos: &[TupleElementInfo],
    ) -> bool {
        for (i, &t) in element_types.iter().enumerate() {
            let info = element_infos[i].clone();
            if info.flags.intersects(ElementFlags::VARIADIC) {
                if c.type_(t).flags.intersects(TypeFlags::ANY) {
                    self.add(
                        c,
                        t,
                        TupleElementInfo {
                            flags: ElementFlags::REST,
                            labeled_declaration: info.labeled_declaration,
                        },
                    );
                } else if c.type_(t)
                    .flags
                    .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
                    || crate::relater::is_generic_mapped_type(c, t)
                {
                    // Generic variadic elements stay as they are.
                    self.add(c, t, info);
                } else if is_tuple_type(c, t) {
                    let spread_types = c.get_element_types(t);
                    if spread_types.len() + self.types.len() >= 10_000 {
                        let current_node = c.current_node.expect("currentNode");
                        let message = if u::is_part_of_type_node(c.store(), current_node) {
                            &diag::TYPE_PRODUCES_A_TUPLE_TYPE_THAT_IS_TOO_LARGE_TO_REPRESENT
                        } else {
                            &diag::EXPRESSION_PRODUCES_A_TUPLE_TYPE_THAT_IS_TOO_LARGE_TO_REPRESENT
                        };
                        c.error(current_node, message, &[]);
                        return false;
                    }
                    // Spread variadic elements with tuple types into the
                    // resulting tuple.
                    let spread_infos: Vec<TupleElementInfo> = c
                        .type_(t)
                        .target()
                        .and_then(|target| {
                            c.type_(target).as_tuple_type().map(|d| d.element_infos.clone())
                        })
                        .unwrap_or_default();
                    for (j, s) in spread_types.iter().enumerate() {
                        self.add(c, *s, spread_infos[j].clone());
                    }
                } else {
                    // Treat everything else as an array type and create a rest
                    // element.
                    let number_type = c.number_type();
                    let s = if is_array_like_type(c, t) {
                        c.get_index_type_of_type(t, number_type)
                    } else {
                        None
                    };
                    let s = s.unwrap_or_else(|| c.error_type());
                    self.add(
                        c,
                        s,
                        TupleElementInfo {
                            flags: ElementFlags::REST,
                            labeled_declaration: info.labeled_declaration,
                        },
                    );
                }
            } else {
                // Copy other element kinds with no change.
                self.add(c, t, info);
            }
        }
        // Turn optional elements preceding the last required element into
        // required elements
        for i in 0..self.last_required_index.max(0) as usize {
            if self.infos[i].flags.intersects(ElementFlags::OPTIONAL) {
                self.infos[i].flags = ElementFlags::REQUIRED;
            }
        }
        if self.first_rest_index >= 0 && self.first_rest_index < self.last_optional_or_rest_index {
            // Turn elements between first rest and last optional/rest into a
            // single rest element
            let number_type = c.number_type();
            let mut types = Vec::new();
            for i in self.first_rest_index as usize..=(self.last_optional_or_rest_index as usize) {
                let mut t = self.types[i];
                if self.infos[i].flags.intersects(ElementFlags::VARIADIC) {
                    t = c.get_indexed_access_type(t, number_type);
                }
                types.push(t);
            }
            self.types[self.first_rest_index as usize] =
                crate::relater::get_union_type(c, types);
            let first = self.first_rest_index as usize + 1;
            let last = self.last_optional_or_rest_index as usize + 1;
            self.types.drain(first..last);
            self.infos.drain(first..last);
        }
        true
    }

    /// Go: `func (n *TupleNormalizer) add(t, info)`.
    fn add(&mut self, c: &mut Checker, t: TypeId, info: TupleElementInfo) {
        if info.flags.intersects(ElementFlags::REQUIRED) {
            self.last_required_index = self.types.len() as isize;
        }
        if info.flags.intersects(ElementFlags::REST) && self.first_rest_index < 0 {
            self.first_rest_index = self.types.len() as isize;
        }
        if info.flags.intersects(ElementFlags::OPTIONAL.union(ElementFlags::REST)) {
            self.last_optional_or_rest_index = self.types.len() as isize;
        }
        let optional = info.flags.intersects(ElementFlags::OPTIONAL);
        self.types
            .push(crate::relater::add_optionality_ex(c, t, true /*isProperty*/, optional));
        self.infos.push(info);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Checker methods — the type-node family
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    // ── The routing spine ──

    /// Go: `func (c *Checker) getTypeFromTypeNode(node) *Type` (checker.go:23295).
    pub(crate) fn get_type_from_type_node(&mut self, node: NodeId) -> TypeId {
        let t = self.get_type_from_type_node_worker(node);
        self.get_conditional_flow_type_of_type(t, node)
    }

    /// Go: `func (c *Checker) getTypeFromTypeNodeWorker(node) *Type` (checker.go:23299).
    fn get_type_from_type_node_worker(&mut self, node: NodeId) -> TypeId {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::AnyKeyword | Kind::JSDocAllType => self.any_type(),
            Kind::JSDocNonNullableType => {
                let inner = type_node_type(self.store(), node).expect("JSDocNonNullableType type");
                self.get_type_from_type_node(inner)
            }
            Kind::JSDocNullableType => {
                let inner = type_node_type(self.store(), node).expect("JSDocNullableType type");
                let t = self.get_type_from_type_node(inner);
                if self.strict_null_checks {
                    self.get_nullable_type(t, TypeFlags::NULL)
                } else {
                    t
                }
            }
            Kind::JSDocVariadicType => {
                let inner = type_node_type(self.store(), node).expect("JSDocVariadicType type");
                let t = self.get_type_from_type_node(inner);
                self.create_array_type(t)
            }
            Kind::JSDocOptionalType => {
                let inner = type_node_type(self.store(), node).expect("JSDocOptionalType type");
                let t = self.get_type_from_type_node(inner);
                self.add_optionality(t)
            }
            Kind::UnknownKeyword => self.unknown_type(),
            Kind::StringKeyword => self.string_type(),
            Kind::NumberKeyword => self.number_type(),
            Kind::BigIntKeyword => self.bigint_type(),
            Kind::BooleanKeyword => self.boolean_type(),
            Kind::SymbolKeyword => self.es_symbol_type(),
            Kind::VoidKeyword => self.void_type(),
            Kind::UndefinedKeyword => self.undefined_type(),
            Kind::NullKeyword => self.null_type(),
            Kind::NeverKeyword => self.never_type(),
            Kind::ObjectKeyword => self.non_primitive_type(),
            Kind::IntrinsicKeyword => self.intrinsic_marker_type(),
            Kind::ThisType | Kind::ThisKeyword => self.get_type_from_this_type_node(node),
            Kind::LiteralType => self.get_type_from_literal_type_node(node),
            Kind::TypeReference | Kind::ExpressionWithTypeArguments => {
                self.get_type_from_type_reference(node)
            }
            Kind::TypePredicate => {
                // Go: an asserts type-predicate type node resolves to the
                // void type; a regular predicate to the boolean type.
                if self
                    .store()
                    .node(node)
                    .as_type_predicate_node()
                    .expect("TypePredicateNode data")
                    .asserts_modifier
                    .is_some()
                {
                    self.void_type()
                } else {
                    self.boolean_type()
                }
            }
            Kind::TypeQuery => self.get_type_from_type_query_node(node),
            Kind::ArrayType | Kind::TupleType => self.get_type_from_array_or_tuple_type_node(node),
            Kind::OptionalType => self.get_type_from_optional_type_node(node),
            Kind::UnionType => self.get_type_from_union_type_node(node),
            Kind::IntersectionType => self.get_type_from_intersection_type_node(node),
            Kind::NamedTupleMember => self.get_type_from_named_tuple_type_node(node),
            Kind::ParenthesizedType => {
                let inner = type_node_type(self.store(), node).expect("ParenthesizedType type");
                self.get_type_from_type_node(inner)
            }
            Kind::RestType => self.get_type_from_rest_type_node(node),
            Kind::FunctionType | Kind::ConstructorType | Kind::TypeLiteral => {
                self.get_type_from_type_literal_or_function_or_constructor_type_node(node)
            }
            Kind::TypeOperator => self.get_type_from_type_operator_node(node),
            Kind::IndexedAccessType => self.get_type_from_indexed_access_type_node(node),
            Kind::TemplateLiteralType => self.get_type_from_template_type_node(node),
            Kind::MappedType => self.get_type_from_mapped_type_node(node),
            Kind::ConditionalType => self.get_type_from_conditional_type_node(node),
            Kind::InferType => self.get_type_from_infer_type_node(node),
            Kind::ImportType => self.get_type_from_import_type_node(node),
            _ => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) getConditionalFlowTypeOfType(t, node) *Type` (checker.go:25502)
    /// — the wrapper every getTypeFromTypeNode call routes through.
    fn get_conditional_flow_type_of_type(&mut self, t: TypeId, mut node: NodeId) -> TypeId {
        let mut constraints: Vec<TypeId> = Vec::new();
        let mut covariant = true;
        loop {
            if node == NodeId::NONE
                || is_statement(self.store(), node)
                || self.store().node(node).kind == Kind::JSDoc
            {
                break;
            }
            let parent = self.store().node(node).parent.get();
            if parent != NodeId::NONE {
                // Only consider variance flipped by parameter locations —
                // `keyof` types would usually be considered variance
                // inverting, but often get used in indexed accesses where
                // they behave sort of invariantly.
                if tsc_ast::ast_generated::is_parameter_declaration(self.store().node(parent)) {
                    covariant = !covariant;
                }
                // Always substitute on type parameters, regardless of
                // variance, since even in contravariant positions, they may
                // rely on substituted constraints to be valid.
                let parent_is_conditional = self.store().node(parent).kind == Kind::ConditionalType;
                let parent_is_mapped = self.store().node(parent).kind == Kind::MappedType;
                if (covariant || self.type_(t).flags.intersects(TypeFlags::TYPE_VARIABLE))
                    && parent_is_conditional
                {
                    let (check, extends) = {
                        let d = self
                            .store()
                            .node(parent)
                            .as_conditional_type_node()
                            .expect("ConditionalTypeNode data");
                        (d.check_type, d.extends_type)
                    };
                    if node == self.store().node(parent).as_conditional_type_node().expect("data").true_type {
                        if let Some(constraint) =
                            self.get_implied_constraint(t, check, extends)
                        {
                            constraints.push(constraint);
                        }
                    }
                } else if self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
                    && parent_is_mapped
                    && self
                        .store()
                        .node(parent)
                        .as_mapped_type_node()
                        .expect("MappedTypeNode data")
                        .name_type
                        .is_none()
                    && self
                        .store()
                        .node(parent)
                        .as_mapped_type_node()
                        .expect("MappedTypeNode data")
                        .type_
                        == Some(node)
                {
                    let mapped_type = self.get_type_from_type_node(parent);
                    // Go: getTypeParameterFromMappedType(mappedType) ==
                    // getActualTypeVariable(t) — the S8/S7 mapped-type
                    // component reads are the seams; the comparison stays
                    // Go-shaped.
                    let mapped_type_parameter =
                        crate::relater::get_type_parameter_from_mapped_type(self, mapped_type);
                    if Some(crate::inference::get_actual_type_variable(self, t))
                        == Some(mapped_type_parameter)
                    {
                        // TODO(port M6 S9): getHomomorphicTypeVariable — the
                        // homomorphic mapped-type substitution; permissive
                        // None keeps the constraint uncollected.
                        let type_parameter: Option<TypeId> = None;
                        if let Some(type_parameter) = type_parameter {
                            let constraint = self.get_constraint_of_type_parameter(type_parameter);
                            if let Some(constraint) = constraint
                                && every_type_ex(self, constraint, |c, ct| {
                                    crate::relater::is_array_or_tuple_type(c, ct)
                                })
                            {
                                // Go: getUnionType([numberType, numericStringType]).
                                // TODO(port M6 S7): numericStringType (the
                                // S7-seeded string|number alias); the number
                                // half alone preserves the constraint's shape.
                                constraints.push(self.number_type());
                            }
                        }
                    }
                }
            }
            node = parent;
        }
        if !constraints.is_empty() {
            // REAL (M6 S10, checker.go:25376):
            // `c.getSubstitutionType(t, c.getIntersectionType(constraints))`
            // (uniontypes.rs's substitution-type ctor).
            let constraint = crate::relater::get_intersection_type(self, constraints);
            return self.get_substitution_type(t, constraint);
        }
        t
    }

    /// Go: `func (c *Checker) getImpliedConstraint(t, checkNode, extendsNode) *Type` (checker.go:25526).
    fn get_implied_constraint(
        &mut self,
        t: TypeId,
        check_node: NodeId,
        extends_node: NodeId,
    ) -> Option<TypeId> {
        if is_unary_tuple_type_node(self.store(), check_node)
            && is_unary_tuple_type_node(self.store(), extends_node)
        {
            let (check_el, extends_el) = {
                let store = self.store();
                let c = binder_node_data::elements_of(store, check_node);
                let e = binder_node_data::elements_of(store, extends_node);
                (c[0], e[0])
            };
            return self.get_implied_constraint(t, check_el, extends_el);
        }
        let check_type = self.get_type_from_type_node(check_node);
        if crate::inference::get_actual_type_variable(self, check_type)
            == crate::inference::get_actual_type_variable(self, t)
        {
            return Some(self.get_type_from_type_node(extends_node));
        }
        None
    }

    // ── this/literal/type-literal arms ──

    /// Go: `func (c *Checker) getTypeFromThisTypeNode(node) *Type` (checker.go:23373).
    fn get_type_from_this_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let t = self.get_this_type(node);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getThisType(node) *Type` (checker.go:23379).
    fn get_this_type(&mut self, node: NodeId) -> TypeId {
        let container =
            binder_shims::get_this_container(self.store(), node, false, false);
        if container != NodeId::NONE {
            let parent = self.store().node(container).parent.get();
            if parent != NodeId::NONE {
                let parent_node = self.store().node(parent);
                if binder_shims::is_class_like(parent_node)
                    || is_interface_declaration(parent_node)
                {
                    let is_static = binder_shims::has_static_modifier(self.store().node(container));
                    let body = binder_node_data::body_of(self.store(), container);
                    let in_constructor_body = self.store().node(container).kind
                        != Kind::Constructor
                        || body.is_some_and(|b| u::is_node_descendant_of(self.store(), node, b));
                    if !is_static && in_constructor_body {
                        let symbol = self
                            .get_symbol_of_declaration_merged(parent)
                            .expect("class/interface declaration symbol");
                        let declared = self.get_declared_type_of_class_or_interface(symbol);
                        let this_type = self
                            .type_(declared)
                            .as_interface_type()
                            .and_then(|d| d.this_type);
                        // Go: core.Coalesce(thisType, errorType).
                        return this_type.unwrap_or_else(|| self.error_type());
                    }
                }
            }
        }
        self.error(
            node,
            &diag::A_THIS_TYPE_IS_AVAILABLE_ONLY_IN_A_NON_STATIC_MEMBER_OF_A_CLASS_OR_INTERFACE,
            &[],
        );
        self.error_type()
    }

    /// Go: `func (c *Checker) getTypeFromLiteralTypeNode(node) *Type` (checker.go:23396).
    fn get_type_from_literal_type_node(&mut self, node: NodeId) -> TypeId {
        let literal = self
            .store()
            .node(node)
            .as_literal_type_node()
            .expect("LiteralTypeNode data")
            .literal;
        if self.store().node(literal).kind == Kind::NullKeyword {
            return self.null_type();
        }
        if self.type_node_links.get(node).resolved_type.is_none() {
            let t = self.check_expression(literal);
            let t = self.get_regular_type_of_literal_type(t);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getMembersOfSymbol(symbol) ast.SymbolTable` (checker.go:16510).
    ///
    /// PORT: Go hands back the live table; the port snapshots the entries
    /// (the only S5 reads are the emptiness check and getSymbol lookups).
    /// The LateBindingContainer arm routes through the S6 seam permissively
    /// (the raw binder members table).
    pub(crate) fn get_members_of_symbol(&mut self, symbol: SymbolId) -> Vec<(String, SymbolId)> {
        if self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::LATE_BINDING_CONTAINER)
        {
            // M6 S6: the real early+late-bound resolution (resolution.rs).
            return self
                .get_resolved_members_or_exports_of_symbol(
                    symbol,
                    crate::types::MembersOrExportsResolutionKind::ResolvedMembers,
                )
                .entries()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
        }
        self.program_symbol(symbol)
            .members
            .entries()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Go: `func (c *Checker) getTypeFromTypeLiteralOrFunctionOrConstructorTypeNode(node) *Type` (checker.go:23404).
    fn get_type_from_type_literal_or_function_or_constructor_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            // Deferred resolution of members is handled by resolveObjectTypeMembers.
            let alias = self.get_alias_for_type_node(node);
            let symbol = self.get_symbol_of_declaration(node);
            let members_empty = symbol
                .map(|s| self.get_members_of_symbol(s).is_empty())
                .unwrap_or(true);
            if symbol.is_none() || (members_empty && alias.is_none()) {
                let empty = self.empty_type_literal_type.expect("emptyTypeLiteralType");
                self.type_node_links.get(node).resolved_type = Some(empty);
            } else {
                // Go: node.Symbol() is the merged/late-bound symbol; the
                // binder-set declaration symbol is identical for type-literal
                // symbols (no late-bind containers here).
                let t = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
                self.type_mut(t).alias = alias;
                self.type_node_links.get(node).resolved_type = Some(t);
            }
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    // ── indexed-access / typeof-operator arms ──

    /// Go: `func (c *Checker) getTypeFromIndexedAccessTypeNode(node) *Type` (checker.go:23418).
    pub(crate) fn get_type_from_indexed_access_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let (object_type, index_type) = {
                let d = self
                    .store()
                    .node(node)
                    .as_indexed_access_type_node()
                    .expect("IndexedAccessTypeNode data");
                (d.object_type, d.index_type)
            };
            let object_type = self.get_type_from_type_node(object_type);
            let index_type = self.get_type_from_type_node(index_type);
            let potential_alias = self.get_alias_for_type_node(node);
            let t = self.get_indexed_access_type_ex(
                object_type,
                index_type,
                AccessFlags::NONE,
                Some(node),
                potential_alias,
            );
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromTypeOperatorNode(node) *Type` (checker.go:23432).
    fn get_type_from_type_operator_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let (operator, arg_type) = {
                let d = self
                    .store()
                    .node(node)
                    .as_type_operator_node()
                    .expect("TypeOperatorNode data");
                (d.operator, d.type_)
            };
            let resolved = match operator {
                Kind::KeyOfKeyword => {
                    let t = self.get_type_from_type_node(arg_type);
                    crate::relater::get_index_type(self, t)
                }
                Kind::UniqueKeyword => {
                    if self.store().node(arg_type).kind == Kind::SymbolKeyword {
                        let parent = self.store().node(node).parent.get();
                        let host = walk_up_parenthesized_types(self.store(), parent);
                        self.get_es_symbol_like_type_for_node(host)
                    } else {
                        self.error_type()
                    }
                }
                Kind::ReadonlyKeyword => self.get_type_from_type_node(arg_type),
                _ => panic!("Unhandled case in getTypeFromTypeOperatorNode"),
            };
            self.type_node_links.get(node).resolved_type = Some(resolved);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getESSymbolLikeTypeForNode(node) *Type` (checker.go:23457).
    fn get_es_symbol_like_type_for_node(&mut self, node: NodeId) -> TypeId {
        if is_valid_essymbol_declaration(self.store(), node) {
            // Go: getSymbolOfNode (merged + late-bound); the plain declaration
            // symbol is identical for the const/readonly declaration kinds
            // isValidESSymbolDeclaration admits.
            if let Some(symbol) = self.get_symbol_of_declaration(node) {
                if let Some(&unique_type) = self.unique_essymbol_types.get(&symbol) {
                    return unique_type;
                }
                let mut b = String::new();
                b.push_str(tsc_ast::INTERNAL_SYMBOL_NAME_PREFIX);
                b.push('@');
                b.push_str(&self.program_symbol(symbol).name);
                b.push('@');
                // Go: ast.GetSymbolId(symbol) — the lazily-assigned binder id.
                b.push_str(&self.program_symbol(symbol).id.get().to_string());
                let unique_type = self.new_unique_es_symbol_type(symbol, b);
                self.unique_essymbol_types.insert(symbol, unique_type);
                return unique_type;
            }
        }
        self.es_symbol_type()
    }

    /// Go: `func (c *Checker) newUniqueESSymbolType(symbol, name) *Type` (checker.go:25589)
    /// — REAL ctor (the new* block, but every dependency — newType + the
    /// UniqueESSymbolType payload — landed with S1/S2; landing it keeps the
    /// relater's unique-symbol arms exact instead of esSymbolType-permissive).
    pub(crate) fn new_unique_es_symbol_type(&mut self, symbol: SymbolId, name: String) -> TypeId {
        let data = TypeData::UniqueESSymbol(Box::new(types::UniqueESSymbolType { name }));
        let t = self.new_type(TypeFlags::UNIQUE_ESSYMBOL, ObjectFlags::NONE, data);
        self.type_mut(t).symbol = Some(symbol);
        t
    }

    // ── type references ──

    /// Go: `func (c *Checker) getTypeFromTypeReference(node) *Type` (checker.go:23468).
    fn get_type_from_type_reference(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let resolved = {
                let parent = self.store().node(node).parent.get();
                // Cache both the resolved symbol and the resolved type. The
                // resolved symbol is needed when we check the type reference
                // in checkTypeReferenceNode. Handle LS queries on the `const`
                // in `x as const` by resolving to the type of `x`.
                let is_const_assertion = u::is_const_type_reference(self.store(), node)
                    && matches!(
                        self.store().node(parent).kind,
                        Kind::TypeAssertionExpression | Kind::AsExpression
                    );
                if is_const_assertion {
                    let expr = crate::ast_shims::expression_of(self.store(), parent)
                        .expect("assertion expression");
                    self.check_expression_cached(expr)
                } else if let Some(t) = self.get_intended_type_from_jsdoc_type_reference(node) {
                    t
                } else {
                    let symbol = self.get_symbol_from_type_reference(node);
                    let t = self.get_type_reference_type(node, symbol);
                    self.get_distributed_type_parameter(node, t)
                }
            };
            self.type_node_links.get(node).resolved_type = Some(resolved);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getDistributedTypeParameter(node, t) *Type` (checker.go:23482).
    fn get_distributed_type_parameter(&mut self, node: NodeId, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
            && !self
                .type_(t)
                .as_type_parameter()
                .expect("TypeParameter data")
                .is_distributed
        {
            let mut n = self.store().node(node).parent.get();
            while n != NodeId::NONE && !is_statement(self.store(), n) {
                if self.store().node(n).kind == Kind::ConditionalType {
                    let check_type_node = self
                        .store()
                        .node(n)
                        .as_conditional_type_node()
                        .expect("ConditionalTypeNode data")
                        .check_type;
                    if is_simple_identifier_type_reference(self.store(), check_type_node) {
                        let symbol =
                            self.get_symbol_from_type_reference(check_type_node);
                        if self.type_(t).symbol == Some(symbol) {
                            // If node is contained in a distributive
                            // conditional type for the given type parameter,
                            // return the distributed form of the type
                            // parameter.
                            return self.get_distributed_type_from_type_parameter(t);
                        }
                    }
                }
                n = self.store().node(n).parent.get();
            }
        }
        t
    }

    /// Go: `func (c *Checker) getDistributedTypeFromTypeParameter(t) *Type` (checker.go:23499).
    fn get_distributed_type_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        let existing = self
            .type_(t)
            .as_type_parameter()
            .and_then(|d| d.distributed_type);
        if let Some(distributed) = existing {
            return distributed;
        }
        let symbol = self.type_(t).symbol;
        let distributed = self.new_type_parameter(symbol);
        if let Some(d) = self.type_mut(distributed).as_type_parameter_mut() {
            d.is_distributed = true;
            d.constraint = Some(t);
        }
        if let Some(d) = self.type_mut(t).as_type_parameter_mut() {
            d.distributed_type = Some(distributed);
        }
        distributed
    }

    /// Go: `func (c *Checker) getIntendedTypeFromJSDocTypeReference(node) *Type` (checker.go:23526).
    fn get_intended_type_from_jsdoc_type_reference(&mut self, node: NodeId) -> Option<TypeId> {
        let n = self.store().node(node);
        if n.flags.intersects(NF::JSDOC) && is_type_reference_node(n) {
            let type_name = n.as_type_reference_node().expect("data").type_name;
            if tsc_ast::ast_generated::is_identifier(self.store().node(type_name)) {
                let type_args = crate::statements::type_arguments_of(self.store(), node);
                match tsc_ast::node_text(self.store(), type_name) {
                    "String" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.string_type());
                    }
                    "Number" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.number_type());
                    }
                    "BigInt" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.bigint_type());
                    }
                    "Boolean" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.boolean_type());
                    }
                    "Void" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.void_type());
                    }
                    "Undefined" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.undefined_type());
                    }
                    "Null" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.null_type());
                    }
                    "Function" | "function" => {
                        self.check_no_type_arguments(node, None);
                        return Some(self.global_function_type.expect("globalFunctionType"));
                    }
                    "array" => {
                        if type_args.is_empty() && !self.no_implicit_any {
                            return Some(self.any_array_type);
                        }
                    }
                    "promise" => {
                        if type_args.is_empty() && !self.no_implicit_any {
                            return Some(self.create_promise_type(self.any_type()));
                        }
                    }
                    "Object" => {
                        if type_args.len() == 2 {
                            let record_symbol = self.get_global_record_symbol();
                            if let Some(record_symbol) = record_symbol {
                                let index_type = self.get_type_from_type_node(type_args[0]);
                                // TODO(port M6 S8): isValidIndexKeyType — the
                                // index-key validation; permissive false routes
                                // to Go's anyType fallback arm.
                                if false {
                                    let t1 = index_type;
                                    let t2 = self.get_type_from_type_node(type_args[1]);
                                    return Some(
                                        self.get_type_alias_instantiation(
                                            record_symbol,
                                            vec![t1, t2],
                                            None,
                                        ),
                                    );
                                }
                            }
                            return Some(self.any_type());
                        }
                        if !self.no_implicit_any {
                            self.check_no_type_arguments(node, None);
                            return Some(self.any_type());
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) createPromiseType(promisedType) *Type` (checker.go:20752)
    /// — the JSDoc `promise` arm's constructor, composed over the S7
    /// createTypeReference seam and the S9 awaited-type seam (REAL — M6 S9:
    /// the Awaited<T> unwrap composes the landed awaited family).
    pub(crate) fn create_promise_type(&mut self, promised_type: TypeId) -> TypeId {
        // Creates a `Promise<T>` type where T is the promisedType argument.
        let global_promise_type = self.get_global_promise_type_checked();
        if Some(global_promise_type) != self.empty_generic_type {
            // If the promised type is itself a promise, get the underlying
            // type; otherwise, fallback to the promised type.
            // Unwrap an `Awaited<T>` to `T` to improve inference (Go's comment).
            let unwrapped = self.unwrap_awaited_type(promised_type);
            let promised_type = self
                .get_awaited_type_no_alias(unwrapped)
                .unwrap_or_else(|| self.unknown_type());
            return self.create_type_reference(global_promise_type, vec![promised_type]);
        }
        self.unknown_type()
    }

    /// Go: `func (c *Checker) createPromiseLikeType(promisedType) *Type`
    /// (checker.go:20764) — REAL (M6 S9): the contextual await/yield arm's
    /// constructor over the global PromiseLike resolver.
    pub(crate) fn create_promise_like_type(&mut self, promised_type: TypeId) -> TypeId {
        // Creates a `PromiseLike<T>` type where T is the promisedType argument.
        let global_promise_like_type = self.get_global_promise_like_type();
        if Some(global_promise_like_type) != self.empty_generic_type {
            let unwrapped = self.unwrap_awaited_type(promised_type);
            let promised_type = self
                .get_awaited_type_no_alias(unwrapped)
                .unwrap_or_else(|| self.unknown_type());
            return self.create_type_reference(global_promise_like_type, vec![promised_type]);
        }
        self.unknown_type()
    }

    /// Go: `func (c *Checker) getSymbolFromTypeReference(node) *ast.Symbol` (checker.go:23609).
    pub(crate) fn get_symbol_from_type_reference(&mut self, node: NodeId) -> SymbolId {
        if self
            .symbol_node_links
            .get(node)
            .resolved_symbol
            .is_none()
        {
            // The `const` in a `const` assertion resolves to nothing;
            // resolveName knows not to report an error for it, so no
            // special-casing is needed here.
            let symbol = self.resolve_type_reference_name(node, SymbolFlags::TYPE, false);
            self.symbol_node_links.get(node).resolved_symbol = Some(symbol);
        }
        self.symbol_node_links
            .get(node)
            .resolved_symbol
            .expect("just cached")
    }

    /// Go: `func (c *Checker) resolveTypeReferenceName(typeReference, meaning, ignoreErrors) *ast.Symbol` (checker.go:23619).
    fn resolve_type_reference_name(
        &mut self,
        type_reference: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
    ) -> SymbolId {
        let name = get_type_reference_name(self.store(), type_reference);
        let Some(name) = name else {
            return self.unknown_symbol();
        };
        let symbol = self.resolve_entity_name(name, meaning, ignore_errors, false, NodeId::NONE);
        if let Some(symbol) = symbol
            && symbol != self.unknown_symbol()
        {
            return symbol;
        }
        if ignore_errors {
            return self.unknown_symbol();
        }
        self.get_unresolved_symbol_for_entity_name(name)
    }

    // Go: `func (c *Checker) resolveEntityName(name, meaning, ignoreErrors, dontResolveAlias, location) *ast.Symbol`
    // (checker.go:16158 — S6's name-resolution ROOT). REAL body landed in
    // resolution.rs (`resolve_entity_name`, same signature); this permissive
    // twin was removed.

    /// Go: `func (c *Checker) getUnresolvedSymbolForEntityName(name) *ast.Symbol` (checker.go:23632).
    fn get_unresolved_symbol_for_entity_name(&mut self, name: NodeId) -> SymbolId {
        let n = self.store().node(name);
        let identifier = match n.kind {
            Kind::QualifiedName => n.as_qualified_name().expect("data").right,
            Kind::PropertyAccessExpression => {
                n.as_property_access_expression()
                    .expect("PropertyAccessExpression data")
                    .name
            }
            _ => name,
        };
        let text = tsc_ast::node_text(self.store(), identifier).to_string();
        if !text.is_empty() {
            let parent_symbol = match n.kind {
                Kind::QualifiedName => {
                    let left = n.as_qualified_name().expect("data").left;
                    Some(self.get_unresolved_symbol_for_entity_name(left))
                }
                Kind::PropertyAccessExpression => {
                    let expr = crate::ast_shims::expression_of(self.store(), name)
                        .unwrap_or(NodeId::NONE);
                    if expr != NodeId::NONE {
                        Some(self.get_unresolved_symbol_for_entity_name(expr))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let path = match parent_symbol {
                Some(parent) => format!("{}.{}", get_symbol_path(self, parent), text),
                None => text.clone(),
            };
            if let Some(&result) = self.unresolved_symbols.get(&path) {
                return result;
            }
            let result = self.new_symbol_ex(SymbolFlags::TYPE_ALIAS, &text, CheckFlags::UNRESOLVED);
            self.program_symbol_mut(result).parent = parent_symbol;
            self.unresolved_symbols.insert(path, result);
            let unresolved = self.unresolved_type();
            self.type_alias_links.get(result).declared_type = Some(unresolved);
            return result;
        }
        self.unknown_symbol()
    }

    /// Go: `func (c *Checker) getTypeReferenceType(node, symbol) *Type` (checker.go:23653).
    fn get_type_reference_type(&mut self, node: NodeId, symbol: SymbolId) -> TypeId {
        if symbol == self.unknown_symbol() {
            return self.error_type();
        }
        let flags = self.get_symbol_flags(symbol);
        if flags.intersects(SymbolFlags::CLASS.union(SymbolFlags::INTERFACE)) {
            return self.get_type_from_class_or_interface_reference(node, symbol);
        }
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return self.get_type_from_type_alias_reference(node, symbol);
        }
        // Get type from reference to named type that cannot be generic (enum
        // or type parameter)
        if let Some(res) = self.try_get_declared_type_of_symbol(symbol)
            && self.check_no_type_arguments(node, Some(symbol))
        {
            return self.get_regular_type_of_literal_type(res);
        }
        // !!! Resolving values as types for JS
        self.error_type()
    }

    /// Go: `func (c *Checker) getTypeFromClassOrInterfaceReference(node, symbol) *Type` (checker.go:23670).
    pub(crate) fn get_type_from_class_or_interface_reference(&mut self, node: NodeId, symbol: SymbolId) -> TypeId {
        let t = self.get_declared_type_of_class_or_interface(self.get_merged_symbol(symbol));
        let (type_parameters, outer_type_parameters) = {
            let d = self.type_(t).as_interface_type().expect("InterfaceType data");
            (d.local_type_parameters().to_vec(), d.outer_type_parameters().to_vec())
        };
        if !type_parameters.is_empty() {
            let num_type_arguments = crate::statements::type_arguments_of(self.store(), node).len();
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            let is_js = tsc_ast::utilities::is_in_js_file(self.store().node(node));
            let is_js_implicit_any = !self.no_implicit_any && is_js;
            if !is_js_implicit_any
                && (num_type_arguments < min_type_argument_count
                    || num_type_arguments > type_parameters.len())
            {
                // Go: the JSDoc-augments tag fact needs the W3 scanner seam
                // (IsJSDocAugmentsTag); false keeps the standard messages.
                let missing_augments_tag = false;
                let message = if missing_augments_tag {
                    if min_type_argument_count < type_parameters.len() {
                        &diag::EXPECTED_0_1_TYPE_ARGUMENTS_PROVIDE_THESE_WITH_AN_EXTENDS_TAG
                    } else {
                        &diag::EXPECTED_0_TYPE_ARGUMENTS_PROVIDE_THESE_WITH_AN_EXTENDS_TAG
                    }
                } else if min_type_argument_count < type_parameters.len() {
                    &diag::GENERIC_TYPE_0_REQUIRES_BETWEEN_1_AND_2_TYPE_ARGUMENTS
                } else {
                    &diag::GENERIC_TYPE_0_REQUIRES_1_TYPE_ARGUMENT_S
                };
                let type_str = self.type_to_string_ex(t);
                self.error(
                    node,
                    message,
                    &[
                        type_str,
                        min_type_argument_count.to_string(),
                        type_parameters.len().to_string(),
                    ],
                );
                if !is_js {
                    // TODO: Adopt same permissive behavior in TS as in JS to
                    // reduce follow-on editing experience failures (requires
                    // editing fillMissingTypeArguments).
                    return self.error_type();
                }
            }
            if self.store().node(node).kind == Kind::TypeReference
                && self.is_deferred_type_reference_node(
                    node,
                    num_type_arguments != type_parameters.len(),
                )
            {
                return self.create_deferred_type_reference(t, node, None, None);
            }
            // In a type reference, the outer type parameters of the
            // referenced class or interface are automatically supplied as
            // type arguments and the type reference only specifies arguments
            // for the local type parameters of the class or interface.
            let args = self.get_type_arguments_from_node(node);
            let local_type_arguments = self.fill_missing_type_arguments(
                &args,
                &type_parameters,
                min_type_argument_count,
                is_js,
            );
            let mut type_arguments = outer_type_parameters;
            type_arguments.extend(local_type_arguments);
            // REAL (M6 S7): createTypeReferenceEx carries the FROM_TYPE_NODE
            // object flag to the interned reference ctor.
            self.create_type_reference_ex(t, type_arguments, ObjectFlags::FROM_TYPE_NODE)
        } else if self.check_no_type_arguments(node, Some(symbol)) {
            t
        } else {
            self.error_type()
        }
    }

    /// Go: `func (c *Checker) getTypeArgumentsFromNode(node) []*Type` (checker.go:23740).
    pub(crate) fn get_type_arguments_from_node(&mut self, node: NodeId) -> Vec<TypeId> {
        crate::statements::type_arguments_of(self.store(), node)
            .into_iter()
            .map(|n| self.get_type_from_type_node(n))
            .collect()
    }

    /// Go: `func (c *Checker) checkNoTypeArguments(node, symbol) bool` (checker.go:23744).
    fn check_no_type_arguments(&mut self, node: NodeId, symbol: Option<SymbolId>) -> bool {
        if !crate::statements::type_arguments_of(self.store(), node).is_empty() {
            let type_name = match symbol {
                Some(symbol) => crate::relater::symbol_to_string(self, symbol),
                None => {
                    let type_name = self
                        .store()
                        .node(node)
                        .as_type_reference_node()
                        .expect("TypeReferenceNode data")
                        .type_name;
                    tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(type_name))
                }
            };
            self.error(node, &diag::TYPE_0_IS_NOT_GENERIC, &[type_name]);
            return false;
        }
        true
    }

    /// Go: `func (c *Checker) isDeferredTypeReferenceNode(node, hasDefaultTypeArguments) bool` (checker.go:23752).
    fn is_deferred_type_reference_node(
        &mut self,
        node: NodeId,
        has_default_type_arguments: bool,
    ) -> bool {
        if self.get_alias_symbol_for_type_node(node).is_some() {
            return true;
        }
        if self.is_resolved_by_type_alias(node) {
            let kind = self.store().node(node).kind;
            match kind {
                Kind::ArrayType => {
                    let element_type = self
                        .store()
                        .node(node)
                        .as_array_type_node()
                        .expect("ArrayTypeNode data")
                        .element_type;
                    return self.may_resolve_type_alias(element_type);
                }
                Kind::TupleType => {
                    return elements_of(self.store(), node)
                        .iter()
                        .any(|&e| self.may_resolve_type_alias(e));
                }
                Kind::TypeReference => {
                    if has_default_type_arguments {
                        return true;
                    }
                    return crate::statements::type_arguments_of(self.store(), node)
                        .iter()
                        .any(|&ta| self.may_resolve_type_alias(ta));
                }
                _ => panic!("Unhandled case in isDeferredTypeReferenceNode"),
            }
        }
        false
    }

    /// Go: `func (c *Checker) isResolvedByTypeAlias(node) bool` (checker.go:23775).
    fn is_resolved_by_type_alias(&mut self, node: NodeId) -> bool {
        let mut parent = self.store().node(node).parent.get();
        loop {
            if parent == NodeId::NONE {
                return false;
            }
            let kind = self.store().node(parent).kind;
            match kind {
                Kind::ParenthesizedType
                | Kind::NamedTupleMember
                | Kind::TypeReference
                | Kind::UnionType
                | Kind::IntersectionType
                | Kind::IndexedAccessType
                | Kind::ConditionalType
                | Kind::TypeOperator
                | Kind::ArrayType
                | Kind::TupleType => {
                    parent = self.store().node(parent).parent.get();
                }
                Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => {
                    return true;
                }
                _ => return false,
            }
        }
    }

    /// Go: `func (c *Checker) mayResolveTypeAlias(node) bool` (checker.go:23789).
    fn may_resolve_type_alias(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::TypeReference => {
                let symbol = self.resolve_type_reference_name(node, SymbolFlags::TYPE, false);
                self.get_symbol_flags(symbol).intersects(SymbolFlags::TYPE_ALIAS)
            }
            Kind::TypeQuery => true,
            Kind::TypeOperator => {
                let operator = self
                    .store()
                    .node(node)
                    .as_type_operator_node()
                    .expect("TypeOperatorNode data")
                    .operator;
                let type_ = self
                    .store()
                    .node(node)
                    .as_type_operator_node()
                    .expect("TypeOperatorNode data")
                    .type_;
                operator != Kind::UniqueKeyword && self.may_resolve_type_alias(type_)
            }
            Kind::ParenthesizedType | Kind::OptionalType | Kind::NamedTupleMember => {
                let type_ = type_node_type(self.store(), node).expect("wrapped type");
                self.may_resolve_type_alias(type_)
            }
            Kind::RestType => {
                let type_ = type_node_type(self.store(), node).expect("RestType type");
                if self.store().node(type_).kind != Kind::ArrayType {
                    return self.may_resolve_type_alias(type_);
                }
                let element_type = self
                    .store()
                    .node(type_)
                    .as_array_type_node()
                    .expect("ArrayTypeNode data")
                    .element_type;
                self.may_resolve_type_alias(element_type)
            }
            Kind::UnionType => {
                let nodes = union_types_of(self.store(), node);
                nodes.iter().any(|&t| self.may_resolve_type_alias(t))
            }
            Kind::IntersectionType => {
                let nodes = intersection_types_of(self.store(), node);
                nodes.iter().any(|&t| self.may_resolve_type_alias(t))
            }
            Kind::IndexedAccessType => {
                let (object_type, index_type) = {
                    let d = self
                        .store()
                        .node(node)
                        .as_indexed_access_type_node()
                        .expect("IndexedAccessTypeNode data");
                    (d.object_type, d.index_type)
                };
                self.may_resolve_type_alias(object_type) || self.may_resolve_type_alias(index_type)
            }
            Kind::ConditionalType => {
                let (check_type, extends_type, true_type, false_type) = {
                    let d = self
                        .store()
                        .node(node)
                        .as_conditional_type_node()
                        .expect("ConditionalTypeNode data");
                    (d.check_type, d.extends_type, d.true_type, d.false_type)
                };
                self.may_resolve_type_alias(check_type)
                    || self.may_resolve_type_alias(extends_type)
                    || self.may_resolve_type_alias(true_type)
                    || self.may_resolve_type_alias(false_type)
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) createDeferredTypeReference(target, node, mapper, alias) *Type` (checker.go:25655)
    /// — REAL ctor (newObjectType + the TypeReference payload fields all
    /// landed; the deferred type-argument resolution is S7's instantiateType).
    pub(crate) fn create_deferred_type_reference(
        &mut self,
        target: TypeId,
        node: NodeId,
        mapper: Option<TypeMapperId>,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let mut alias = alias;
        if alias.is_none() {
            alias = self.get_alias_for_type_node(node);
            // TODO(port M6 S8): instantiateTypes(alias.typeArguments, mapper)
            // — rides S8's alias-mapper instantiation paths.
        }
        let symbol = self.type_(target).symbol;
        let t = self.new_object_type(ObjectFlags::REFERENCE, symbol);
        self.type_mut(t).alias = alias;
        match &mut self.type_mut(t).data {
            TypeData::TypeReference(d) => {
                d.base.target = Some(target);
                d.base.mapper = mapper;
                d.node = Some(node);
            }
            _ => unreachable!("newObjectType(Reference) yields TypeReference data"),
        }
        t
    }

    // ── normalized tuple references ──

    /// Go: `func (c *Checker) createNormalizedTypeReference(target, typeArguments) *Type` (checker.go:23816).
    pub(crate) fn create_normalized_type_reference(&mut self, target: TypeId, type_arguments: Vec<TypeId>) -> TypeId {
        if self.type_(target).object_flags.intersects(ObjectFlags::TUPLE) {
            return self.create_normalized_tuple_type(target, type_arguments);
        }
        self.create_type_reference(target, type_arguments)
    }

    /// Go: `func (c *Checker) createNormalizedTupleTypeEx(target, elementTypes, objectFlags) *Type` (checker.go:23825).
    fn create_normalized_tuple_type_ex(
        &mut self,
        target: TypeId,
        element_types: Vec<TypeId>,
        object_flags: ObjectFlags,
    ) -> TypeId {
        let (combined_flags, element_infos_len) = {
            let d = self.type_(target).as_tuple_type().expect("TupleType data");
            (d.combined_flags, d.element_infos.len())
        };
        if !combined_flags.intersects(ElementFlags::NON_REQUIRED) {
            // No need to normalize when we only have regular required elements
            return self.create_type_reference_ex(target, element_types, object_flags);
        }
        if combined_flags.intersects(ElementFlags::VARIADIC) {
            let element_infos: Vec<TupleElementInfo> = self
                .type_(target)
                .as_tuple_type()
                .map(|d| d.element_infos.clone())
                .unwrap_or_default();
            for (i, &e) in element_types.iter().enumerate() {
                if i < element_infos_len
                    && element_infos[i].flags.intersects(ElementFlags::VARIADIC)
                    && self.type_(e)
                        .flags
                        .intersects(TypeFlags::NEVER.union(TypeFlags::UNION))
                {
                    // Transform [A, ...(X | Y | Z)] into [A, ...X] | [A, ...Y] | [A, ...Z]
                    let check_types: Vec<TypeId> = element_types
                        .iter()
                        .enumerate()
                        .map(|(j, &t)| {
                            if j < element_infos_len
                                && element_infos[j].flags.intersects(ElementFlags::VARIADIC)
                            {
                                t
                            } else {
                                self.unknown_type()
                            }
                        })
                        .collect();
                    // REAL (M6 S10, checker.go:22183): checkCrossProductUnion
                    // gates the cross-product expansion (uniontypes.rs's
                    // S7-landed body).
                    if self.check_cross_product_union(&check_types) {
                        let element_types_snapshot = element_types.clone();
                        let object_flags_snapshot = object_flags;
                        return self
                            .map_type(e, &mut |c, u| {
                                let mut new_elements = element_types_snapshot.clone();
                                new_elements[i] = u;
                                Some(c.create_normalized_tuple_type_ex(
                                    target,
                                    new_elements,
                                    object_flags_snapshot,
                                ))
                            })
                            .expect("mapType result");
                    }
                }
            }
        }
        // We have optional, rest, or variadic elements that may need
        // normalizing. Normalization ensures that all variadic elements are
        // generic and that the tuple type has one of the following layouts,
        // disregarding variadic elements: (1) zero or more required elements,
        // followed by zero or more optional elements, followed by zero or one
        // rest element; (2) zero or more required elements, followed by a
        // rest element, followed by zero or more required elements. In either
        // layout, zero or more generic variadic elements may be present at any
        // location. Note that the element types may contain an extra 'this'
        // type argument that we want to ignore during normalization and then
        // just append to the normalized element types.
        let element_infos: Vec<TupleElementInfo> = self
            .type_(target)
            .as_tuple_type()
            .map(|d| d.element_infos.clone())
            .unwrap_or_default();
        let (mut types, mut infos) =
            match TupleNormalizer::normalize(self, &element_types[..element_infos_len.min(element_types.len())], &element_infos) {
                Some(result) => result,
                None => return self.error_type(),
            };
        if element_types.len() > element_infos_len {
            types.push(element_types[element_infos_len]);
        }
        let _ = &mut infos;
        let tuple_target = self.get_tuple_target_type(infos.clone(), self
            .type_(target)
            .as_tuple_type()
            .map(|d| d.readonly)
            .unwrap_or(false));
        if Some(tuple_target) == self.empty_generic_type {
            return self.empty_object_type.expect("emptyObjectType");
        }
        if !types.is_empty() {
            return self.create_type_reference_ex(tuple_target, types, object_flags);
        }
        tuple_target
    }

    /// Go: `func (c *Checker) createNormalizedTupleType(target, elementTypes) *Type` (checker.go:23870).
    #[allow(dead_code)] // the S7 instantiate* engine consumes it
    pub(crate) fn create_normalized_tuple_type(&mut self, target: TypeId, element_types: Vec<TypeId>) -> TypeId {
        self.create_normalized_tuple_type_ex(target, element_types, ObjectFlags::NONE)
    }

    // Go: `func (c *Checker) createTypeReferenceEx(target, typeArguments, objectFlags) *Type`
    // (checker.go — S7's ctor) — M6 S7: uniontypes.rs owns the REAL interned
    // constructor; this pinned seam is retired (S5 callers now resolve to
    // the real body, which carries the FROM_TYPE_NODE propagating flags and
    // resolvedTypeArguments).

    /// Go: `func (c *Checker) getElementTypes(t) []*Type` (checker.go:23990) —
    /// REAL body (replaces the W2 permissive reader in inference.rs).
    pub(crate) fn get_element_types(&mut self, t: TypeId) -> Vec<TypeId> {
        // PORT(FCS tranche 3): Go's getTypeArguments resolves deferred
        // references (relater.rs's resolve_type_arguments) — the resolving
        // form is what guarantees len(args) >= arity for the slice below.
        let type_arguments = crate::relater::resolve_type_arguments(self, t);
        let arity = crate::relater::get_type_reference_arity(self, t) as usize;
        if type_arguments.len() == arity {
            return type_arguments;
        }
        type_arguments[..arity].to_vec()
    }

    // ── type aliases ──

    /// Go: `func (c *Checker) getTypeFromTypeAliasReference(node, symbol) *Type` (checker.go:24120).
    fn get_type_from_type_alias_reference(&mut self, node: NodeId, symbol: SymbolId) -> TypeId {
        let type_arguments = crate::statements::type_arguments_of(self.store(), node);
        if self
            .program_symbol(symbol)
            .check_flags
            .intersects(CheckFlags::UNRESOLVED)
        {
            let type_arguments: Vec<TypeId> = type_arguments
                .iter()
                .map(|&n| self.get_type_from_type_node(n))
                .collect();
            let alias = TypeAlias {
                symbol: Some(symbol),
                type_arguments,
            };
            let key = crate::typeofsymbol::get_alias_key(Some(&alias));
            let error_type = match self.error_types.get(&key) {
                Some(&t) => t,
                None => {
                    let t = self.new_intrinsic_type(TypeFlags::ANY, "error");
                    self.type_mut(t).alias = Some(Box::new(alias));
                    self.error_types.insert(key, t);
                    t
                }
            };
            return error_type;
        }
        let t = self.get_declared_type_of_symbol(symbol);
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        if !type_parameters.is_empty() {
            let num_type_arguments = type_arguments.len();
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            if num_type_arguments < min_type_argument_count
                || num_type_arguments > type_parameters.len()
            {
                let message = if min_type_argument_count == type_parameters.len() {
                    &diag::GENERIC_TYPE_0_REQUIRES_1_TYPE_ARGUMENT_S
                } else {
                    &diag::GENERIC_TYPE_0_REQUIRES_BETWEEN_1_AND_2_TYPE_ARGUMENTS
                };
                let name = crate::relater::symbol_to_string(self, symbol);
                self.error(
                    node,
                    message,
                    &[
                        name,
                        min_type_argument_count.to_string(),
                        type_parameters.len().to_string(),
                    ],
                );
                return self.error_type();
            }
            // We refrain from associating a local type alias with an
            // instantiation of a top-level type alias because the local alias
            // may end up being referenced in an inferred return type where it
            // is not accessible--which in turn may lead to a large structural
            // expansion of the type when generating a .d.ts file. See #43622
            // for an example.
            let alias_symbol = self.get_alias_symbol_for_type_node(node);
            let mut new_alias_symbol: Option<SymbolId> = None;
            if alias_symbol.is_some()
                && (is_local_type_alias(self, symbol)
                    || alias_symbol.is_some_and(|a| !is_local_type_alias(self, a)))
            {
                new_alias_symbol = alias_symbol;
            }
            let mut alias_type_arguments: Vec<TypeId> = Vec::new();
            if new_alias_symbol.is_none() && is_type_reference_type(self.store().node(node)) {
                // Refers to an alias import/export/reexport — by making sure
                // we use the target as an aliasSymbol, we ensure the exported
                // symbol is used to refer to the type when it is reserialized
                // later.
                let alias_symbol = self.resolve_type_reference_name(node, SymbolFlags::ALIAS, true);
                if alias_symbol != self.unknown_symbol() {
                    let resolved = self.resolve_alias(alias_symbol);
                    if self
                        .get_symbol_flags(resolved)
                        .intersects(SymbolFlags::TYPE_ALIAS)
                    {
                        new_alias_symbol = Some(resolved);
                        alias_type_arguments = self.get_type_arguments_from_node(node);
                    }
                }
            }
            let new_alias = new_alias_symbol.map(|s| {
                Box::new(TypeAlias {
                    symbol: Some(s),
                    type_arguments: alias_type_arguments,
                })
            });
            let type_arguments = self.get_type_arguments_from_node(node);
            return self.get_type_alias_instantiation(
                symbol,
                type_arguments,
                new_alias,
            );
        }
        if self.check_no_type_arguments(node, Some(symbol)) {
            return t;
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getTypeAliasInstantiation(symbol, typeArguments, alias) *Type` (checker.go:24158)
    /// — REAL body (replaces relater.rs's S7/W4 placeholder; the instantiation
    /// itself rides the S7 identity seam).
    pub(crate) fn get_type_alias_instantiation(
        &mut self,
        symbol: SymbolId,
        type_arguments: Vec<TypeId>,
        alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let t = self.get_declared_type_of_symbol(symbol);
        if Some(t) == self.intrinsic_marker_type {
            let name = self.program_symbol(symbol).name.clone();
            if let Some(kind) = intrinsic_type_kind(&name)
                && type_arguments.len() == 1
            {
                return match kind {
                    IntrinsicTypeKind::NoInfer => {
                        // REAL (M6 S10, checker.go:24177): the
                        // substitution-wrapping no-infer type
                        // (uniontypes.rs's getNoInferType).
                        self.get_no_infer_type(type_arguments[0])
                    }
                    IntrinsicTypeKind::StringMapping => {
                        crate::relater::get_string_mapping_type(self, Some(symbol), type_arguments[0])
                    }
                };
            }
        }
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        let key = crate::typeofsymbol::get_type_alias_instantiation_key(
            &type_arguments,
            alias.as_deref(),
        );
        if let Some(&instantiation) = self
            .type_alias_links
            .get(symbol)
            .instantiations
            .as_ref()
            .and_then(|m| m.get(&key))
        {
            return instantiation;
        }
        let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
        let is_js = self
            .program_symbol(symbol)
            .value_declaration
            .is_some_and(|d| tsc_ast::utilities::is_in_js_file(self.store().node(d)));
        let filled =
            self.fill_missing_type_arguments(&type_arguments, &type_parameters, min_type_argument_count, is_js);
        let mapper = self.new_type_mapper(&type_parameters, &filled);
        let instantiation = self.instantiate_type_with_alias(t, Some(mapper), alias);
        let links = self.type_alias_links.get(symbol);
        if links.instantiations.is_none() {
            links.instantiations = Some(rustc_hash::FxHashMap::default());
        }
        links
            .instantiations
            .as_mut()
            .expect("just initialized")
            .insert(key, instantiation);
        instantiation
    }

    // Go: `func (c *Checker) instantiateTypeWithAlias(t, m, alias) *Type`
    // (checker.go:22563) — M6 S7: uniontypes.rs owns the REAL engine (alias
    // is carried onto created types and the instantiation cache keys include
    // it); this S5 identity stub is retired. Callers pass `Some(m)`.


    /// Go: `func (c *Checker) getAliasForTypeNode(node) *TypeAlias` (checker.go:24318).
    fn get_alias_for_type_node(&mut self, node: NodeId) -> Option<Box<TypeAlias>> {
        let symbol = self.get_alias_symbol_for_type_node(node);
        symbol.map(|symbol| {
            let type_arguments = self.get_type_arguments_for_alias_symbol(Some(symbol));
            Box::new(TypeAlias {
                symbol: Some(symbol),
                type_arguments,
            })
        })
    }

    /// Go: `func (c *Checker) getAliasSymbolForTypeNode(node) *ast.Symbol` (checker.go:24325).
    fn get_alias_symbol_for_type_node(&mut self, node: NodeId) -> Option<SymbolId> {
        let mut host = self.store().node(node).parent.get();
        while tsc_ast::ast_generated::is_parenthesized_type_node(self.store().node(host))
            || (tsc_ast::ast_generated::is_type_operator_node(self.store().node(host))
                && self
                    .store()
                    .node(host)
                    .as_type_operator_node()
                    .expect("TypeOperatorNode data")
                    .operator
                    == Kind::ReadonlyKeyword)
        {
            host = self.store().node(host).parent.get();
        }
        if u::is_type_alias(self.store().node(host)) {
            return Some(self.get_symbol_of_declaration_merged(host).expect("alias declaration symbol"));
        }
        None
    }

    /// Go: `func (c *Checker) getTypeArgumentsForAliasSymbol(symbol) []*Type` (checker.go:24337).
    fn get_type_arguments_for_alias_symbol(&mut self, symbol: Option<SymbolId>) -> Vec<TypeId> {
        if let Some(symbol) = symbol {
            return self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) getInferTypeParameters(node) []*Type` (checker.go:24319).
    pub(crate) fn get_infer_type_parameters(&mut self, node: NodeId) -> Vec<TypeId> {
        let locals = self
            .store()
            .node(node)
            .as_conditional_type_node()
            .expect("ConditionalTypeNode data")
            .locals
            .values()
            .copied()
            .collect::<Vec<_>>();
        let mut result = Vec::new();
        for symbol in locals {
            if self
                .program_symbol(symbol)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
            {
                result.push(self.get_declared_type_of_symbol(symbol));
            }
        }
        result.sort_by(|&a, &b| u::compare_types(self, a, b));
        result
    }

    // ── type queries / array / tuple / optional / rest arms ──

    /// Go: `func (c *Checker) getTypeFromTypeQueryNode(node) *Type` (checker.go:24705).
    fn get_type_from_type_query_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            // TypeScript 1.0 spec (April 2014): 3.6.3 — the expression is
            // processed as an identifier expression (section 4.3) or property
            // access expression (section 4.10), the widened type (section 3.9)
            // of which becomes the result.
            let t = self.check_expression_with_type_arguments(node);
            let t = self.get_widened_type(t);
            let t = self.get_regular_type_of_literal_type(t);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromArrayOrTupleTypeNode(node) *Type` (checker.go:24714).
    fn get_type_from_array_or_tuple_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let target = self.get_array_or_tuple_target_type(node);
            let resolved = if Some(target) == self.empty_generic_type {
                self.empty_object_type.expect("emptyObjectType")
            } else {
                let has_variadic = self.store().node(node).kind == Kind::TupleType
                    && elements_of(self.store(), node)
                        .iter()
                        .any(|&e| self.is_variadic_tuple_element(e));
                if !has_variadic && self.is_deferred_type_reference_node(node, false) {
                    if self.store().node(node).kind == Kind::TupleType
                        && elements_of(self.store(), node).is_empty()
                    {
                        target
                    } else {
                        // TODO(port M6 S7/S9): the deferred reference's
                        // arguments resolve at instantiation time; the eager
                        // construction below is the pinned-seam stand-in
                        // (createTypeReference is identity, so the target is
                        // the observable answer either way).
                        self.create_deferred_type_reference(target, node, None, None)
                    }
                } else {
                    let element_types: Vec<TypeId> = if self.store().node(node).kind == Kind::ArrayType {
                        let element_type = self
                            .store()
                            .node(node)
                            .as_array_type_node()
                            .expect("ArrayTypeNode data")
                            .element_type;
                        vec![self.get_type_from_type_node(element_type)]
                    } else {
                        elements_of(self.store(), node)
                            .into_iter()
                            .map(|e| self.get_type_from_type_node(e))
                            .collect()
                    };
                    if self.type_(target).object_flags.intersects(ObjectFlags::TUPLE) {
                        self.create_normalized_tuple_type_ex(
                            target,
                            element_types,
                            ObjectFlags::FROM_TYPE_NODE,
                        )
                    } else {
                        self.create_type_reference_ex(target, element_types, ObjectFlags::FROM_TYPE_NODE)
                    }
                }
            };
            self.type_node_links.get(node).resolved_type = Some(resolved);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) isVariadicTupleElement(node) bool` (checker.go:24745).
    fn is_variadic_tuple_element(&mut self, node: NodeId) -> bool {
        self.get_tuple_element_flags(node).intersects(ElementFlags::VARIADIC)
    }

    /// Go: `func (c *Checker) getArrayOrTupleTargetType(node) *Type` (checker.go:24750).
    fn get_array_or_tuple_target_type(&mut self, node: NodeId) -> TypeId {
        let parent = self.store().node(node).parent.get();
        let readonly = self.is_readonly_type_operator(parent);
        let element_type = self.get_array_element_type_node(node);
        if element_type.is_some() {
            if readonly {
                return self.global_readonly_array_type.expect("globalReadonlyArrayType");
            }
            return self.global_array_type.expect("globalArrayType");
        }
        let element_infos: Vec<TupleElementInfo> = elements_of(self.store(), node)
            .into_iter()
            .map(|e| self.get_tuple_element_info(e))
            .collect();
        self.get_tuple_target_type(element_infos, readonly)
    }

    /// Go: `func (c *Checker) isReadonlyTypeOperator(node) bool` (checker.go:24763).
    fn is_readonly_type_operator(&mut self, node: NodeId) -> bool {
        tsc_ast::ast_generated::is_type_operator_node(self.store().node(node))
            && self
                .store()
                .node(node)
                .as_type_operator_node()
                .expect("TypeOperatorNode data")
                .operator
                == Kind::ReadonlyKeyword
    }

    /// Go: `func (c *Checker) getTypeFromNamedTupleTypeNode(node) *Type` (checker.go:24768).
    fn get_type_from_named_tuple_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let d = self
                .store()
                .node(node)
                .as_named_tuple_member()
                .expect("NamedTupleMember data");
            let resolved = if d.dot_dot_dot_token.is_some() {
                self.get_type_from_rest_type_node(node)
            } else {
                let optional = d.question_token.is_some();
                let t = self.get_type_from_type_node(d.type_);
                crate::relater::add_optionality_ex(self, t, true /*isProperty*/, optional)
            };
            self.type_node_links.get(node).resolved_type = Some(resolved);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromRestTypeNode(node) *Type` (checker.go:24779).
    fn get_type_from_rest_type_node(&mut self, node: NodeId) -> TypeId {
        let type_node = type_node_type(self.store(), node).expect("RestType type");
        let element_type_node = self.get_array_element_type_node(type_node);
        let type_node = element_type_node.unwrap_or(type_node);
        self.get_type_from_type_node(type_node)
    }

    /// Go: `func (c *Checker) getArrayElementTypeNode(node) *ast.Node` (checker.go:24788).
    fn get_array_element_type_node(&mut self, node: NodeId) -> Option<NodeId> {
        match self.store().node(node).kind {
            Kind::ParenthesizedType => {
                let inner = type_node_type(self.store(), node).expect("ParenthesizedType type");
                self.get_array_element_type_node(inner)
            }
            Kind::TupleType => {
                let elements = elements_of(self.store(), node);
                if elements.len() == 1 {
                    let mut node = elements[0];
                    if self.store().node(node).kind == Kind::RestType {
                        let inner = type_node_type(self.store(), node).expect("RestType type");
                        return self.get_array_element_type_node(inner);
                    }
                    if self.store().node(node).kind == Kind::NamedTupleMember
                        && self
                            .store()
                            .node(node)
                            .as_named_tuple_member()
                            .expect("NamedTupleMember data")
                            .dot_dot_dot_token
                            .is_some()
                    {
                        node = type_node_type(self.store(), node).expect("NamedTupleMember type");
                        return self.get_array_element_type_node(node);
                    }
                }
                None
            }
            Kind::ArrayType => Some(
                self.store()
                    .node(node)
                    .as_array_type_node()
                    .expect("ArrayTypeNode data")
                    .element_type,
            ),
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getTypeFromOptionalTypeNode(node) *Type` (checker.go:24808).
    fn get_type_from_optional_type_node(&mut self, node: NodeId) -> TypeId {
        let type_ = type_node_type(self.store(), node).expect("OptionalType type");
        let t = self.get_type_from_type_node(type_);
        crate::relater::add_optionality_ex(self, t, true /*isProperty*/, true /*isOptional*/)
    }

    // ── union / intersection / template arms ──

    /// Go: `func (c *Checker) getTypeFromUnionTypeNode(node) *Type` (checker.go:24818).
    fn get_type_from_union_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let alias = self.get_alias_for_type_node(node);
            let types = union_types_of(self.store(), node);
            let types: Vec<TypeId> = types
                .iter()
                .map(|&n| self.get_type_from_type_node(n))
                .collect();
            // REAL (M6 S7): getUnionTypeEx via the relater delegate.
            let t = crate::relater::get_union_type_ex(
                self,
                types,
                crate::todo_shapes::UnionReduction::Literal,
                alias,
                None,
            );
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromIntersectionTypeNode(node) *Type` (checker.go:24830).
    fn get_type_from_intersection_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let alias = self.get_alias_for_type_node(node);
            let types = intersection_types_of(self.store(), node);
            let types: Vec<TypeId> = types
                .iter()
                .map(|&n| self.get_type_from_type_node(n))
                .collect();
            // We perform no supertype reduction for X & {} or {} & X, where X
            // is one of string, number, bigint, or a pattern literal template
            // type. This enables union types like "a" | "b" | string & {} or
            // "aa" | "ab" | `a${string}` which preserve the literal types for
            // purposes of statement completion.
            let mut no_supertype_reduction = false;
            if types.len() == 2 {
                let empty_index = {
                    let empty = self.empty_type_literal_type;
                    types.iter().position(|&t| Some(t) == empty)
                };
                if let Some(empty_index) = empty_index {
                    let t = types[1 - empty_index];
                    let flags = self.type_(t).flags;
                    no_supertype_reduction = flags
                        .intersects(
                            TypeFlags::STRING
                                .union(TypeFlags::NUMBER)
                                .union(TypeFlags::BIGINT),
                        )
                        || (flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                            && is_pattern_literal_type(self, t));
                }
            }
            // REAL (M6 S7): getIntersectionTypeEx via the relater delegate.
            let flags = if no_supertype_reduction {
                crate::relater::IntersectionFlags::NO_SUPERTYPE_REDUCTION
            } else {
                crate::relater::IntersectionFlags::NONE
            };
            let t = crate::relater::get_intersection_type_ex(self, types, flags, alias);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromTemplateTypeNode(node) *Type` (checker.go:24863).
    fn get_type_from_template_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let (head, spans) = {
                let d = self
                    .store()
                    .node(node)
                    .as_template_literal_type_node()
                    .expect("TemplateLiteralTypeNode data");
                (d.head, d.template_spans.clone())
            };
            let spans: Vec<NodeId> = spans.map(|l| l.nodes.to_vec()).unwrap_or_default();
            let mut texts: Vec<String> = Vec::with_capacity(spans.len() + 1);
            let mut types: Vec<TypeId> = Vec::with_capacity(spans.len());
            texts.push(self.store().node(head).as_template_head().expect("TemplateHead data").text.to_string());
            for span in spans {
                let (span_literal, span_type) = {
                    let d = self
                        .store()
                        .node(span)
                        .as_template_literal_type_span()
                        .expect("TemplateLiteralTypeSpan data");
                    (d.literal, d.type_)
                };
                let literal_text = {
                    let n = self.store().node(span_literal);
                    if let Some(d) = n.as_template_middle() {
                        d.text.to_string()
                    } else if let Some(d) = n.as_template_tail() {
                        d.text.to_string()
                    } else {
                        // Go: Literal.Text() — the raw text for the literal
                        // payloads a type span admits.
                        tsc_ast::node_text(self.store(), span_literal).to_string()
                    }
                };
                texts.push(literal_text);
                types.push(self.get_type_from_type_node(span_type));
            }
            // REAL (M6 S7): getTemplateLiteralType via the relater delegate.
            let t = crate::relater::get_template_literal_type(self, &texts, &types);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getTypeFromMappedTypeNode(node) *Type` (checker.go:24789).
    pub(crate) fn get_type_from_mapped_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let symbol = self.get_symbol_of_declaration(node);
            let t = self.new_object_type(ObjectFlags::MAPPED, symbol);
            match &mut self.type_mut(t).data {
                TypeData::Mapped(d) => {
                    d.declaration = Some(node);
                }
                _ => unreachable!("newObjectType(Mapped) yields Mapped data"),
            }
            let alias = self.get_alias_for_type_node(node);
            self.type_mut(t).alias = alias;
            self.type_node_links.get(node).resolved_type = Some(t);
            // Eagerly resolve the constraint type which forces an error if
            // the constraint type circularly references itself through one or
            // more type aliases.
            // TODO(port M6 S8): the mapped-type constraint resolution
            // (getConstraintTypeFromMappedType is the pinned seam).
            let _ = crate::relater::get_constraint_type_from_mapped_type(self, t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    // ── conditional / infer arms ──

    /// Go: `func (c *Checker) getTypeFromConditionalTypeNode(node) *Type` (checker.go:24801 region —
    /// the root constructor; checker.go:24790).
    fn get_type_from_conditional_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let (check_type, extends_type, true_type, false_type) = {
                let d = self
                    .store()
                    .node(node)
                    .as_conditional_type_node()
                    .expect("ConditionalTypeNode data");
                (d.check_type, d.extends_type, d.true_type, d.false_type)
            };
            let _ = (true_type, false_type);
            let check_type = self.get_type_from_type_node(check_type);
            let alias = self.get_alias_for_type_node(node);
            let all_outer_type_parameters = self.get_outer_type_parameters(node, true);
            let outer_type_parameters: Vec<TypeId> =
                if alias.as_deref().is_some_and(|a| !a.type_arguments.is_empty()) {
                    all_outer_type_parameters.clone()
                } else {
                    // REAL (M6 S9): isTypeParameterPossiblyReferenced
                    // (checker.go:22891 — uniontypes.rs's reference scan).
                    all_outer_type_parameters
                        .iter()
                        .filter(|&&tp| self.is_type_parameter_possibly_referenced(tp, node))
                        .copied()
                        .collect()
                };
            let extends_resolved = self.get_type_from_type_node(extends_type);
            let is_distributive = self.type_(check_type).flags.intersects(TypeFlags::TYPE_PARAMETER);
            let infer_type_parameters = self.get_infer_type_parameters(node);
            let root = Box::new(types::ConditionalRoot {
                node: Some(node),
                check_type: Some(check_type),
                extends_type: Some(extends_resolved),
                is_distributive,
                infer_type_parameters,
                outer_type_parameters: outer_type_parameters.clone(),
                instantiations: None,
                alias: alias.clone(),
            });
            // REAL (M6 S9): getConditionalType — the deferred-conditional
            // resolution loop (checker.go:24834). The root Box is MOVED into
            // get_conditional_type; a deferred result carries the same Box
            // (Go's shared pointer), so the identity-instantiation seed below
            // lands on the type's own root.
            let t = self.get_conditional_type(root, None, false, None);
            self.type_node_links.get(node).resolved_type = Some(t);
            if !outer_type_parameters.is_empty() {
                let key = crate::typeofsymbol::get_conditional_type_key(
                    &outer_type_parameters,
                    None,
                    false,
                );
                // PORT: Go seeds root.instantiations with the identity
                // instantiation (checker.go:24829) — the root is shared with
                // the deferred conditional type, so the seed is visible to
                // getConditionalTypeInstantiation. When the conditional
                // resolves eagerly (the result is a branch type, not a
                // conditional), Go's seed is only reachable from the
                // discarded standalone root object — dead — so the port
                // seeds only the deferred case.
                if let Some(d) = self.type_mut(t).as_conditional_type_mut()
                    && let Some(root) = d.root.as_deref_mut()
                {
                    let mut map = root.instantiations.take().unwrap_or_default();
                    map.insert(key, t);
                    root.instantiations = Some(map);
                }
            }
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getConditionalType(root *ConditionalRoot, mapper *TypeMapper, forConstraint bool, alias *TypeAlias) *Type`
    /// (checker.go:24834) — REAL (M6 S9): the deferred-conditional
    /// resolution loop (tail-recursion ladder over checkType/extendsType
    /// with the inference context, permissive/restrictive instantiations and
    /// the definite-true/false assignability probes).
    ///
    /// PORT: Go reassigns the local `root` pointer (the immediate-nested
    /// false-type and tail-recursion switches); the port moves the owned
    /// `Box<ConditionalRoot>` — a deferred result carries the CURRENT box,
    /// mirroring Go's shared-pointer identity for the
    /// identity-instantiation seed.
    pub(crate) fn get_conditional_type(
        &mut self,
        mut root: Box<types::ConditionalRoot>,
        mut mapper: Option<TypeMapperId>,
        for_constraint: bool,
        mut alias: Option<Box<TypeAlias>>,
    ) -> TypeId {
        let result: TypeId;
        let mut extra_types: Vec<TypeId> = Vec::new();
        let mut tail_count: i32 = 0;
        // We loop here for an immediately nested conditional type in the
        // false position, effectively treating types of the form
        // 'A extends B ? X : C extends D ? Y : E extends F ? Z : ...' as a
        // single construct for purposes of resolution. We also loop here when
        // resolution of a conditional type ends in resolution of another (or,
        // through recursion, possibly the same) conditional type. In the
        // potentially tail-recursive cases we increment the tail recursion
        // counter and stop after 1000 iterations (Go's comment, verbatim).
        loop {
            if tail_count == 1000 {
                let node = self.current_node.unwrap_or(NodeId::NONE);
                self.error(
                    node,
                    &diag::TYPE_INSTANTIATION_IS_EXCESSIVELY_DEEP_AND_POSSIBLY_INFINITE,
                    &[],
                );
                return self.error_type();
            }
            let root_check_type = root.check_type.expect("root.checkType");
            let root_extends_type = root.extends_type.expect("root.extendsType");
            let actual_check = crate::inference::get_actual_type_variable(self, root_check_type);
            let check_type = self.instantiate_type_with_alias(actual_check, mapper, None);
            let extends_type = self.instantiate_type_with_alias(root_extends_type, mapper, None);
            if check_type == self.error_type() || extends_type == self.error_type() {
                return self.error_type();
            }
            if check_type == self.wildcard_type() || extends_type == self.wildcard_type() {
                return self.wildcard_type();
            }
            let root_node = root.node.expect("root.node");
            let (check_type_node, extends_type_node, true_type_node, false_type_node) = {
                let d = self
                    .store()
                    .node(root_node)
                    .as_conditional_type_node()
                    .expect("ConditionalTypeNode data");
                (
                    skip_type_parentheses(self.store(), d.check_type),
                    skip_type_parentheses(self.store(), d.extends_type),
                    d.true_type,
                    d.false_type,
                )
            };
            // When the check and extends types are simple tuple types of the
            // same arity, we defer resolution of the conditional type when any
            // tuple elements are generic. This is such that non-distributable
            // conditional types can be written `[X] extends [Y] ? ...` and be
            // deferred similarly to `X extends Y ? ...` (Go's comment).
            let check_tuples = is_simple_tuple_type(self.store(), check_type_node)
                && is_simple_tuple_type(self.store(), extends_type_node)
                && elements_of(self.store(), check_type_node).len()
                    == elements_of(self.store(), extends_type_node).len();
            let check_type_deferred = self.is_deferred_type(check_type, check_tuples);
            let mut combined_mapper: Option<TypeMapperId> = None;
            if !root.infer_type_parameters.is_empty() {
                // When we're looking at making an inference for an infer type,
                // when we get its constraint, it'll automagically be
                // instantiated with the context, so it doesn't need the mapper
                // for the inference context (Go's comment, checker.go:24859 —
                // the full prose is preserved in checker.go).
                let context = self.new_inference_context(
                    &root.infer_type_parameters,
                    None, /*signature*/
                    crate::inference::InferenceFlags::NONE,
                    crate::inference::InferenceComparer::Assignable,
                );
                if let Some(mapper) = mapper {
                    let combined = self.combine_type_mappers(
                        self.inference_contexts[context.0 as usize].non_fixing_mapper,
                        Some(mapper),
                    );
                    self.inference_contexts[context.0 as usize].non_fixing_mapper = combined;
                }
                if !check_type_deferred {
                    // We don't want inferences from constraints as they may
                    // cause us to eagerly resolve the conditional type instead
                    // of deferring resolution. Also, we always want strict
                    // function types rules (i.e. proper contravariance) for
                    // inferences (Go's comment).
                    self.infer_types(
                        context,
                        check_type,
                        extends_type,
                        crate::inference::InferencePriority::NO_CONSTRAINTS
                            | crate::inference::InferencePriority::ALWAYS_STRICT,
                        false, /*contravariant*/
                    );
                }
                // It's possible for 'infer T' type parameters to be given
                // uninstantiated constraints when those type parameters are
                // used in type references (see getInferredTypeParameterConstraint).
                // For that reason we need context.mapper to be first in the
                // combined mapper. See #42636 for examples (Go's comment).
                let context_mapper = crate::inference::inference_context_mapper(self, context);
                combined_mapper = match (context_mapper, mapper) {
                    (Some(cm), Some(m)) => self.combine_type_mappers(Some(cm), Some(m)),
                    (cm, None) => cm,
                    (None, Some(m)) => Some(m),
                };
            }
            // Instantiate the extends type including inferences for
            // 'infer T' type parameters.
            let inferred_extends_type = match combined_mapper {
                Some(cm) => self.instantiate_type_with_alias(root_extends_type, Some(cm), None),
                None => extends_type,
            };
            // We attempt to resolve the conditional type only when the check
            // and extends types are non-generic.
            if !check_type_deferred && !self.is_deferred_type(inferred_extends_type, check_tuples)
            {
                // Return falseType for a definitely false extends check. We
                // check an instantiations of the two types with type
                // parameters mapped to the wildcard type, the most permissive
                // instantiations possible (the wildcard type is assignable to
                // and from all types). If those are not related, then no
                // instantiations will be and we can just return the false
                // branch type (Go's comment).
                let check_flags = self.type_(check_type).flags;
                // PORT: Go evaluates the permissive instantiations lazily
                // inside the `||`; the borrows force the same shape.
                let definitely_false = check_flags.intersects(TypeFlags::ANY) || {
                    let permissive_check = self.get_permissive_instantiation(check_type);
                    let permissive_extends =
                        self.get_permissive_instantiation(inferred_extends_type);
                    !self.is_type_assignable_to(permissive_check, permissive_extends)
                };
                if !self
                    .type_(inferred_extends_type)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                    && definitely_false
                {
                    // Return union of trueType and falseType for 'any' since it
                    // matches anything. Furthermore, for a distributive
                    // conditional type applied to the constraint of a type
                    // variable, include trueType if there are possible values
                    // of the check type that are also possible values of the
                    // extends type. We use a reverse assignability check as it
                    // is less expensive than the comparable relationship and
                    // avoids false positives of a non-empty intersection
                    // check (Go's comment).
                    let extends_never =
                        !self.type_(inferred_extends_type).flags.intersects(TypeFlags::NEVER);
                    let include_true = if check_flags.intersects(TypeFlags::ANY) {
                        true
                    } else if for_constraint && extends_never {
                        let permissive_extends =
                            self.get_permissive_instantiation(inferred_extends_type);
                        let permissive_check = self.get_permissive_instantiation(check_type);
                        some_type_extends_assignable_to_check(
                            self,
                            permissive_extends,
                            permissive_check,
                        )
                    } else {
                        false
                    };
                    if include_true {
                        let true_branch = self.get_type_from_type_node(true_type_node);
                        let true_mapper = combined_mapper.or(mapper);
                        let instantiated =
                            self.instantiate_type_with_alias(true_branch, true_mapper, None);
                        extra_types.push(instantiated);
                    }
                    // If falseType is an immediately nested conditional type
                    // that isn't distributive or has an identical checkType,
                    // switch to that type and loop (Go's comment).
                    let false_type = self.get_type_from_type_node(false_type_node);
                    if self.type_(false_type).flags.intersects(TypeFlags::CONDITIONAL) {
                        let new_root = self
                            .type_(false_type)
                            .as_conditional_type()
                            .and_then(|d| d.root.clone())
                            .expect("conditional root");
                        let new_root_node = new_root.node.expect("root.node");
                        let new_root_check = new_root.check_type;
                        let new_root_is_distributive = new_root.is_distributive;
                        if self.store().node(new_root_node).parent.get() == root_node
                            && (!new_root_is_distributive || new_root_check == root.check_type)
                        {
                            root = new_root;
                            continue;
                        }
                        if let Some((new_root, new_root_mapper)) =
                            self.get_tail_recursion_root(false_type, mapper)
                        {
                            let has_alias = new_root.alias.is_some();
                            root = new_root;
                            mapper = new_root_mapper;
                            alias = None;
                            if has_alias {
                                tail_count += 1;
                            }
                            continue;
                        }
                    }
                    result = self.instantiate_type_with_alias(false_type, mapper, None);
                    break;
                }
                // Return trueType for a definitely true extends check. We
                // check instantiations of the two types with type parameters
                // mapped to their restrictive form, i.e. a form of the type
                // parameter that has no constraint. This ensures that, for
                // example, the type
                //   type Foo<T extends { x: any }> = T extends { x: string } ? string : number
                // doesn't immediately resolve to 'string' instead of being
                // deferred (Go's comment).
                let definitely_true = self
                    .type_(inferred_extends_type)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                    || {
                        let restrictive_check = self.get_restrictive_instantiation(check_type);
                        let restrictive_extends =
                            self.get_restrictive_instantiation(inferred_extends_type);
                        self.is_type_assignable_to(restrictive_check, restrictive_extends)
                    };
                if definitely_true {
                    let true_type = self.get_type_from_type_node(true_type_node);
                    let true_mapper = combined_mapper.or(mapper);
                    if let Some((new_root, new_root_mapper)) =
                        self.get_tail_recursion_root(true_type, true_mapper)
                    {
                        let has_alias = new_root.alias.is_some();
                        root = new_root;
                        mapper = new_root_mapper;
                        alias = None;
                        if has_alias {
                            tail_count += 1;
                        }
                        continue;
                    }
                    result = self.instantiate_type_with_alias(true_type, true_mapper, None);
                    break;
                }
            }
            // Return a deferred type for a check that is neither definitely
            // true nor definitely false.
            let deferred = self.new_conditional_type(&root, mapper, combined_mapper);
            if let Some(alias) = alias {
                self.type_mut(deferred).alias = Some(alias);
            } else {
                let instantiated_alias = match root.alias.as_deref() {
                    None => None,
                    Some(a) => {
                        let type_arguments =
                            self.instantiate_types(&a.type_arguments, mapper);
                        Some(Box::new(TypeAlias {
                            symbol: a.symbol,
                            type_arguments,
                        }))
                    }
                };
                self.type_mut(deferred).alias = instantiated_alias;
            }
            // Move the CURRENT root into the deferred type (Go shares the
            // pointer; see the method doc-comment).
            let root = std::mem::take(&mut root);
            if let Some(d) = self.type_mut(deferred).as_conditional_type_mut() {
                d.root = Some(root);
            }
            result = deferred;
            break;
        }
        if !extra_types.is_empty() {
            let mut types = extra_types;
            types.push(result);
            return self.get_union_type(&types);
        }
        result
    }

    /// Go: `func (c *Checker) getTailRecursionRoot(newType *Type, newMapper *TypeMapper) (*ConditionalRoot, *TypeMapper)`
    /// (checker.go:24977).
    ///
    /// We tail-recurse for generic conditional types that (a) have not
    /// already been evaluated and cached, and (b) are non distributive, have
    /// a check type that is unaffected by instantiation, or have a non-union
    /// check type. Note that recursion is possible only through aliased
    /// conditional types, so we only increment the tail recursion counter for
    /// those (Go's comment).
    fn get_tail_recursion_root(
        &mut self,
        new_type: TypeId,
        new_mapper: Option<TypeMapperId>,
    ) -> Option<(Box<types::ConditionalRoot>, Option<TypeMapperId>)> {
        let new_mapper = new_mapper?;
        if !self.type_(new_type).flags.intersects(TypeFlags::CONDITIONAL) {
            return None;
        }
        let new_root = self
            .type_(new_type)
            .as_conditional_type()
            .and_then(|d| d.root.clone())?;
        if new_root.outer_type_parameters.is_empty() {
            return None;
        }
        let type_param_mapper = self
            .combine_type_mappers(
                self.type_(new_type).as_conditional_type().expect("data").mapper,
                Some(new_mapper),
            )
            .expect("combineTypeMappers with a non-nil mapper is non-nil");
        let type_arguments: Vec<TypeId> = new_root
            .outer_type_parameters
            .iter()
            .map(|&tp| self.mapper_map(type_param_mapper, tp))
            .collect();
        let new_root_mapper = self.new_type_mapper(&new_root.outer_type_parameters, &type_arguments);
        let mut new_check_type: Option<TypeId> = None;
        if new_root.is_distributive {
            let mapped =
                crate::mapper::get_mapped_type(self, new_root.check_type.expect("checkType"), new_root_mapper);
            new_check_type = Some(mapped);
        }
        if new_check_type.is_none()
            || new_check_type == new_root.check_type
            || !self
                .type_(new_check_type.expect("just checked"))
                .flags
                .intersects(TypeFlags::UNION | TypeFlags::NEVER)
        {
            return Some((new_root, Some(new_root_mapper)));
        }
        None
    }

    /// Go: `func (c *Checker) newConditionalType(root, mapper, combinedMapper) *Type`
    /// (checker.go:25764).
    pub(crate) fn new_conditional_type(
        &mut self,
        root: &types::ConditionalRoot,
        mapper: Option<TypeMapperId>,
        combined_mapper: Option<TypeMapperId>,
    ) -> TypeId {
        let check_type = self.instantiate_type_with_alias(
            root.check_type.expect("root.checkType"),
            mapper,
            None,
        );
        let extends_type = self.instantiate_type_with_alias(
            root.extends_type.expect("root.extendsType"),
            mapper,
            None,
        );
        let data = crate::types::ConditionalType {
            base: Default::default(),
            root: None, // the caller moves the current Box in (identity PORT)
            check_type: Some(check_type),
            extends_type: Some(extends_type),
            resolved_true_type: None,
            resolved_false_type: None,
            resolved_inferred_true_type: None,
            resolved_default_constraint: None,
            resolved_constraint_of_distributive: None,
            mapper,
            combined_mapper,
        };
        self.new_type(
            TypeFlags::CONDITIONAL,
            crate::types::ObjectFlags::NONE,
            crate::types::TypeData::Conditional(Box::new(data)),
        )
    }

    /// Go: `func (c *Checker) isDeferredType(t, checkTuples) bool` (checker.go:24973).
    pub(crate) fn is_deferred_type(&mut self, t: TypeId, check_tuples: bool) -> bool {
        self.is_generic_type(t)
            || (check_tuples
                && is_tuple_type(self, t)
                && self
                    .get_element_types(t)
                    .iter()
                    .any(|&e| self.is_generic_type(e)))
    }

    /// Go: `func (c *Checker) getTypeFromInferTypeNode(node) *Type` (checker.go:25113).
    fn get_type_from_infer_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let type_parameter = self
                .store()
                .node(node)
                .as_infer_type_node()
                .expect("InferTypeNode data")
                .type_parameter;
            let symbol = self
                .get_symbol_of_declaration_merged(type_parameter)
                .expect("infer type-parameter symbol");
            let t = self.get_declared_type_of_type_parameter(symbol);
            self.type_node_links.get(node).resolved_type = Some(t);
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    // ── import types ──

    /// Go: `func (c *Checker) getTypeFromImportTypeNode(node) *Type` (checker.go:25121).
    fn get_type_from_import_type_node(&mut self, node: NodeId) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_none() {
            let (argument, is_type_of, qualifier, attributes) = {
                let d = self
                    .store()
                    .node(node)
                    .as_import_type_node()
                    .expect("ImportTypeNode data");
                (d.argument, d.is_type_of, d.qualifier, d.attributes)
            };
            // Go: ast.IsLiteralImportTypeNode — the argument must be a
            // string-literal-wrapping LiteralTypeNode.
            let is_literal_import = {
                let arg = self.store().node(argument);
                arg.kind == Kind::LiteralType
                    && self
                        .store()
                        .node(arg.as_literal_type_node().expect("data").literal)
                        .kind
                        == Kind::StringLiteral
            };
            if !is_literal_import {
                self.error(argument, &diag::STRING_LITERAL_EXPECTED, &[]);
                self.symbol_node_links.get(node).resolved_symbol = Some(self.unknown_symbol());
                let t = self.error_type();
                self.type_node_links.get(node).resolved_type = Some(t);
                return t;
            }
            let target_meaning = if is_type_of {
                SymbolFlags::VALUE
            } else {
                SymbolFlags::TYPE
            };
            // TODO: Future work: support unions/generics/whatever via a
            // deferred import-type.
            let module_specifier = self
                .store()
                .node(argument)
                .as_literal_type_node()
                .expect("data")
                .literal;
            // PORT(FCS tranche 28): Go checker.go:25116 calls the INTERNAL
            // resolver — `c.resolveExternalModuleName(node, literal, false
            // /*ignoreErrors*/, c.getTypeFromImportAttributes(ast.
            // GetImportAttributes(node)))` — with errors ON and the
            // import-attributes type. The previous code used the public
            // ResolveExternalModuleName carrier (ignoreErrors hard-coded
            // true), which swallowed the module-not-found diagnostics of
            // EVERY import-type node (the TS2307 residual family).
            let import_attributes_type = self.get_type_from_import_attributes(attributes);
            let inner_module_symbol = self.resolve_external_module_name_ex(
                node,
                Some(module_specifier),
                false, /*ignoreErrors*/
                import_attributes_type,
            );
            let Some(inner_module_symbol) = inner_module_symbol else {
                self.symbol_node_links.get(node).resolved_symbol = Some(self.unknown_symbol());
                let t = self.error_type();
                self.type_node_links.get(node).resolved_type = Some(t);
                return t;
            };
            let Some(module_symbol) = self.resolve_external_module_symbol(inner_module_symbol)
            else {
                self.symbol_node_links.get(node).resolved_symbol = Some(self.unknown_symbol());
                let t = self.error_type();
                self.type_node_links.get(node).resolved_type = Some(t);
                return t;
            };
            if let Some(qualifier) = qualifier {
                let name_chain = self.get_identifier_chain(qualifier);
                let mut current_namespace = module_symbol;
                for (i, current) in name_chain.iter().enumerate() {
                    let mut meaning = SymbolFlags::NAMESPACE;
                    if i == name_chain.len() - 1 {
                        meaning = target_meaning;
                    }
                    // typeof a.b.c is normally resolved using checkExpression
                    // which in turn defers to checkQualifiedName — that, in
                    // turn, ultimately uses getPropertyOfType on the type of
                    // the symbol, which differs slightly from the exports
                    // lookup process that only looks up namespace members
                    // which is used for most type references.
                    let resolved_symbol = self.resolve_symbol(current_namespace);
                    let merged_resolved_symbol = self.get_merged_symbol(resolved_symbol);
                    let mut symbol_from_variable: Option<SymbolId> = None;
                    let mut symbol_from_module: Option<SymbolId> = None;
                    if is_type_of {
                        // TODO(port M6 S8): getPropertyOfTypeEx — the
                        // includeTypeOnlyMembers form; the plain property
                        // lookup is the seam read.
                        if let Some(type_of_symbol) = self.get_type_of_symbol_opt(merged_resolved_symbol) {
                            let name = tsc_ast::node_text(self.store(), *current).to_string();
                            symbol_from_variable = self.get_property_of_type(type_of_symbol, &name);
                        }
                    } else {
                        let name = tsc_ast::node_text(self.store(), *current).to_string();
                        let found: Option<SymbolId> = self
                            .get_members_of_symbol(merged_resolved_symbol)
                            .into_iter()
                            .find(|(member_name, _)| member_name == &name)
                            .map(|(_, symbol)| symbol);
                        // Go: c.getSymbol(exports, name, meaning)
                        symbol_from_module = found
                            .filter(|&sym| self.get_symbol_flags(sym).intersects(meaning));
                        if symbol_from_module.is_none() {
                            // A CommonJS module might have typedefs exported
                            // alongside an export= — the S6 immediate-module
                            // seam resolves permissively; the module.exports
                            // assignment scan is S6's.
                        }
                    }
                    let next = symbol_from_module.or(symbol_from_variable);
                    let Some(next) = next else {
                        let fqn = self.get_fully_qualified_name(current_namespace);
                        self.error(
                            *current,
                            &diag::NAMESPACE_0_HAS_NO_EXPORTED_MEMBER_1,
                            &[fqn, tsc_ast::node_text(self.store(), *current).to_string()],
                        );
                        let t = self.error_type();
                        self.type_node_links.get(node).resolved_type = Some(t);
                        return t;
                    };
                    self.symbol_node_links.get(*current).resolved_symbol = Some(next);
                    self.symbol_node_links
                        .get(self.store().node(*current).parent.get())
                        .resolved_symbol = Some(next);
                    current_namespace = next;
                }
                let t = self.resolve_import_symbol_type(node, current_namespace, target_meaning);
                self.type_node_links.get(node).resolved_type = Some(t);
            } else if self
                .get_symbol_flags(module_symbol)
                .intersects(target_meaning)
            {
                let t = self.resolve_import_symbol_type(node, module_symbol, target_meaning);
                self.type_node_links.get(node).resolved_type = Some(t);
            } else {
                let literal_text = {
                    let lit = self
                        .store()
                        .node(argument)
                        .as_literal_type_node()
                        .expect("data")
                        .literal;
                    tsc_ast::node_text(self.store(), lit).to_string()
                };
                let message = if target_meaning == SymbolFlags::VALUE {
                    &diag::MODULE_0_DOES_NOT_REFER_TO_A_VALUE_BUT_IS_USED_AS_A_VALUE_HERE
                } else {
                    &diag::MODULE_0_DOES_NOT_REFER_TO_A_TYPE_BUT_IS_USED_AS_A_TYPE_HERE_DID_YOU_MEAN_TYPEOF_IMPORT_0
                };
                self.error(node, message, &[literal_text]);
                self.symbol_node_links.get(node).resolved_symbol = Some(self.unknown_symbol());
                let t = self.error_type();
                self.type_node_links.get(node).resolved_type = Some(t);
            }
        }
        self.type_node_links
            .get(node)
            .resolved_type
            .expect("just cached")
    }

    /// Go: `func (c *Checker) getIdentifierChain(node) []*ast.Node` (checker.go:25190).
    fn get_identifier_chain(&mut self, node: NodeId) -> Vec<NodeId> {
        if tsc_ast::ast_generated::is_identifier(self.store().node(node)) {
            return vec![node];
        }
        let (left, right) = {
            let d = self.store().node(node).as_qualified_name().expect("QualifiedName data");
            (d.left, d.right)
        };
        let mut chain = self.get_identifier_chain(left);
        chain.push(right);
        chain
    }

    /// Go: `func (c *Checker) resolveImportSymbolType(node, symbol, meaning) *Type` (checker.go:25192).
    fn resolve_import_symbol_type(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> TypeId {
        let resolved_symbol = self.resolve_symbol(symbol);
        self.symbol_node_links.get(node).resolved_symbol = Some(resolved_symbol);
        if meaning == SymbolFlags::VALUE {
            // Intentionally doesn't use resolved symbol so type is cached as
            // expected on the alias.
            let t = self.get_type_of_symbol(symbol);
            return self.get_instantiation_expression_type(t, node);
        }
        // getTypeReferenceType doesn't handle aliases — it must get the
        // resolved symbol.
        self.get_type_reference_type(node, resolved_symbol)
    }

    // Go: `func (c *Checker) resolveSymbol(symbol) *ast.Symbol` (checker.go:14755)
    // — REAL body landed in resolution.rs (`resolve_symbol`, the S6
    // alias-aware merged resolution); this permissive twin was removed.

    /// `getTypeOfSymbol` through the Option surface (the import-type typeof
    /// arm; Go's non-nil symbol always has a type here).
    fn get_type_of_symbol_opt(&mut self, symbol: SymbolId) -> Option<TypeId> {
        let _ = symbol;
        // TODO(port M6 S2): getTypeOfSymbol's Option form for module symbols
        // — unreachable while resolveExternalModuleName is the S6 stub.
        None
    }

    // ── tuple target construction (Go checker.go:25231–25346) ──

    /// Go: `func (c *Checker) getTupleElementFlags(node) ElementFlags` (checker.go:25268).
    fn get_tuple_element_flags(&mut self, node: NodeId) -> ElementFlags {
        match self.store().node(node).kind {
            Kind::OptionalType => ElementFlags::OPTIONAL,
            Kind::RestType => {
                let type_ = type_node_type(self.store(), node).expect("RestType type");
                if self.get_array_element_type_node(type_).is_some() {
                    ElementFlags::REST
                } else {
                    ElementFlags::VARIADIC
                }
            }
            Kind::NamedTupleMember => {
                let d = self
                    .store()
                    .node(node)
                    .as_named_tuple_member()
                    .expect("NamedTupleMember data");
                if d.question_token.is_some() {
                    ElementFlags::OPTIONAL
                } else if d.dot_dot_dot_token.is_some() {
                    if self.get_array_element_type_node(d.type_).is_some() {
                        ElementFlags::REST
                    } else {
                        ElementFlags::VARIADIC
                    }
                } else {
                    ElementFlags::REQUIRED
                }
            }
            _ => ElementFlags::REQUIRED,
        }
    }

    /// Go: `func (c *Checker) getTupleElementInfo(node) TupleElementInfo` (checker.go:25283).
    fn get_tuple_element_info(&mut self, node: NodeId) -> TupleElementInfo {
        let labeled_declaration = if tsc_ast::ast_generated::is_named_tuple_member(self.store().node(node))
            || tsc_ast::ast_generated::is_parameter_declaration(self.store().node(node))
        {
            Some(node)
        } else {
            None
        };
        TupleElementInfo {
            flags: self.get_tuple_element_flags(node),
            labeled_declaration,
        }
    }

    /// Go: `func (c *Checker) getTupleTargetType(elementInfos, readonly) *Type` (checker.go:25312).
    pub(crate) fn get_tuple_target_type(&mut self, element_infos: Vec<TupleElementInfo>, readonly: bool) -> TypeId {
        if element_infos.len() == 1 && element_infos[0].flags.intersects(ElementFlags::REST) {
            // [...X[]] is equivalent to just X[]
            if readonly {
                return self.global_readonly_array_type.expect("globalReadonlyArrayType");
            }
            return self.global_array_type.expect("globalArrayType");
        }
        let key = crate::typeofsymbol::get_tuple_key(&element_infos, readonly);
        if let Some(&t) = self.tuple_types.get(&key) {
            return t;
        }
        let t = self.create_tuple_target_type(element_infos, readonly);
        self.tuple_types.insert(key, t);
        t
    }

    /// Go: `func (c *Checker) createTupleTargetType(elementInfos, readonly) *Type` (checker.go:25326)
    /// — REAL. We represent tuple types as type references to synthesized
    /// generic interface types created by this function; the types are of
    /// the form `interface Tuple<T0, T1, ...> extends Array<T0|T1|...> {
    /// 0: T0, 1: T1, ... }`. Note that the generic type created by this
    /// function has no symbol associated with it. The same is true for each
    /// of the synthesized type parameters.
    pub(crate) fn create_tuple_target_type(&mut self, element_infos: Vec<TupleElementInfo>, readonly: bool) -> TypeId {
        let arity = element_infos.len();
        let min_length = element_infos
            .iter()
            .filter(|e| e.flags.intersects(ElementFlags::REQUIRED.union(ElementFlags::VARIADIC)))
            .count();
        let mut type_parameters: Vec<TypeId> = Vec::with_capacity(arity);
        let mut members = tsc_ast::SymbolTable::new();
        let mut combined_flags = ElementFlags::NONE;
        if arity != 0 {
            for (i, info) in element_infos.iter().enumerate() {
                let type_parameter = self.new_type_parameter(None);
                type_parameters.push(type_parameter);
                let flags = info.flags;
                combined_flags |= flags;
                if !combined_flags.intersects(ElementFlags::VARIABLE) {
                    let mut symbol_flags =
                        SymbolFlags::PROPERTY.union(SymbolFlags::OPTIONAL);
                    if !flags.intersects(ElementFlags::OPTIONAL) {
                        symbol_flags = SymbolFlags::PROPERTY;
                    }
                    let check_flags = if readonly {
                        CheckFlags::READONLY
                    } else {
                        CheckFlags::NONE
                    };
                    let property = self.new_symbol_ex(symbol_flags, &i.to_string(), check_flags);
                    self.value_symbol_links.get(property).resolved_type = Some(type_parameter);
                    members.set(i.to_string(), property);
                }
            }
        }
        let fixed_length = members.size();
        let length_symbol = self.new_symbol_ex(
            SymbolFlags::PROPERTY,
            "length",
            if readonly {
                CheckFlags::READONLY
            } else {
                CheckFlags::NONE
            },
        );
        if combined_flags.intersects(ElementFlags::VARIABLE) {
            self.value_symbol_links.get(length_symbol).resolved_type = Some(self.number_type());
        } else {
            let mut literal_types = Vec::new();
            for i in min_length..=arity {
                literal_types.push(self.get_number_literal_type(tsc_jsnum::Number::from(i as f64)));
            }
            // TODO(port M6 S7): getUnionType — the length union rides the S7
            // constructor seam.
            let length_type = crate::relater::get_union_type(self, literal_types);
            self.value_symbol_links.get(length_symbol).resolved_type = Some(length_type);
        }
        members.set("length".to_string(), length_symbol);
        let t = self.new_object_type(ObjectFlags::TUPLE.union(ObjectFlags::REFERENCE), None);
        {
            let this_type = self.new_type_parameter(None);
            if let Some(d) = self.type_mut(this_type).as_type_parameter_mut() {
                d.is_this_type = true;
                d.constraint = Some(t);
            }
            let type_parameters_with_this = {
                let mut all = type_parameters.clone();
                all.push(this_type);
                all
            };
            let resolved_type_arguments = type_parameters_with_this.clone();
            // PORT: Go's TupleType embeds InterfaceType→TypeReference→
            // ObjectType; the flattened Rust paths are spelled out below
            // (thisType/allTypeParameters/declaredMembers live on the
            // embedded InterfaceType, the instantiations table on the
            // embedded ObjectType).
            if let TypeData::Tuple(d) = &mut self.type_mut(t).data {
                d.base.this_type = Some(this_type);
                d.base.all_type_parameters = type_parameters_with_this;
                d.base.base.base.instantiations = Some(rustc_hash::FxHashMap::default());
                d.base.base.base.target = Some(t);
                d.base.base.resolved_type_arguments = resolved_type_arguments;
                d.base.declared_members_resolved = true;
                d.base.declared_members = members;
                d.element_infos = element_infos;
                d.min_length = min_length as i32;
                d.fixed_length = fixed_length as i32;
                d.combined_flags = combined_flags;
                d.readonly = readonly;
            }
            // Seed the identity instantiation (Go: d.instantiations[getTypeListKey(d.TypeParameters())] = t).
            let key = crate::typeofsymbol::get_type_list_key(&type_parameters);
            if let TypeData::Tuple(d) = &mut self.type_mut(t).data
                && let Some(map) = d.base.base.base.instantiations.as_mut()
            {
                map.insert(key, t);
            }
        }
        t
    }

    // ── the type-argument default machinery (Go checker.go:22392–22510) ──

    /// Go: `func (c *Checker) getEffectiveTypeArguments(node, typeParameters) []*Type` (checker.go:22392).
    pub(crate) fn get_effective_type_arguments(&mut self, node: NodeId, type_parameters: &[TypeId]) -> Vec<TypeId> {
        let type_arguments: Vec<TypeId> =
            crate::statements::type_arguments_of(self.store(), node)
                .iter()
                .map(|&n| self.get_type_from_type_node(n))
                .collect();
        let min = self.get_min_type_argument_count(type_parameters);
        let is_js = tsc_ast::utilities::is_in_js_file(self.store().node(node));
        self.fill_missing_type_arguments(&type_arguments, type_parameters, min, is_js)
    }

    /// Go: `func (c *Checker) getMinTypeArgumentCount(typeParameters) int` (checker.go:22397)
    /// — REAL body (replaces the W1/W2 permissive count).
    pub fn get_min_type_argument_count(&mut self, type_parameters: &[TypeId]) -> usize {
        let mut min_type_argument_count = 0;
        for (i, &type_parameter) in type_parameters.iter().enumerate() {
            if !self.has_type_parameter_default(type_parameter) {
                min_type_argument_count = i + 1;
            }
        }
        min_type_argument_count
    }

    /// Go: `func (c *Checker) hasTypeParameterDefault(t) bool` (checker.go:22406).
    fn has_type_parameter_default(&mut self, t: TypeId) -> bool {
        let Some(symbol) = self.type_(t).symbol else {
            return false;
        };
        let declarations = self.program_symbol(symbol).declarations.clone();
        declarations.iter().any(|&d| {
            self.store().node(d).kind == Kind::TypeParameter
                && self
                    .store()
                    .node(d)
                    .as_type_parameter_declaration()
                    .expect("TypeParameterDeclaration data")
                    .default_type
                    .is_some()
        })
    }

    /// Go: `func (c *Checker) fillMissingTypeArguments(typeArguments, typeParameters, minTypeArgumentCount, isJavaScriptImplicitAny) []*Type` (checker.go:22412)
    /// — REAL body (replaces the permissive identity stub).
    pub fn fill_missing_type_arguments(
        &mut self,
        type_arguments: &[TypeId],
        type_parameters: &[TypeId],
        // PORT: Go's parameter is unused in the body (checker.go:22412) —
        // the `_` spelling is the faithful mirror.
        _min_type_argument_count: usize,
        is_java_script_implicit_any: bool,
    ) -> Vec<TypeId> {
        let num_type_parameters = type_parameters.len();
        if num_type_parameters == 0 {
            return Vec::new();
        }
        let num_type_arguments = type_arguments.len();
        if is_java_script_implicit_any || num_type_arguments < num_type_parameters {
            let mut result: Vec<TypeId> = type_arguments.to_vec();
            result.resize(num_type_parameters, self.error_type());
            let base_default_type = self.get_default_type_argument_type(is_java_script_implicit_any);
            for i in num_type_arguments..num_type_parameters {
                let default_type = self.get_default_from_type_parameter(type_parameters[i]);
                let default_type = if is_java_script_implicit_any {
                    default_type.filter(|&dt| {
                        !(self.is_type_identical_to(dt, self.unknown_type())
                            || self.is_type_identical_to(dt, self.empty_object_type()))
                    })
                } else {
                    default_type
                };
                if let Some(default_type) = default_type {
                    let mapper = self.new_type_mapper(type_parameters, &result);
                    result[i] = self.instantiate_type(default_type, mapper);
                } else {
                    result[i] = base_default_type;
                }
            }
            return result;
        }
        type_arguments.to_vec()
    }

    /// Go: `func (c *Checker) getDefaultTypeArgumentType(isInJavaScriptFile) *Type` (checker.go:22439).
    fn get_default_type_argument_type(&mut self, is_in_java_script_file: bool) -> TypeId {
        if is_in_java_script_file {
            return self.any_type();
        }
        self.unknown_type()
    }

    /// Go: `func (c *Checker) getDefaultFromTypeParameter(t) *Type` (checker.go:22446)
    /// — REAL body (the pub-API twin; the exports.rs stub was removed).
    pub fn get_default_from_type_parameter(&mut self, t: TypeId) -> Option<TypeId> {
        if !self.type_(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return None;
        }
        let default_type = self.get_resolved_type_parameter_default(t);
        if default_type != self.no_constraint_type() && default_type != self.circular_constraint_type() {
            return Some(default_type);
        }
        None
    }

    /// Go: `func (c *Checker) getResolvedTypeParameterDefault(t) *Type` (checker.go:22454).
    fn get_resolved_type_parameter_default(&mut self, t: TypeId) -> TypeId {
        let (resolved_default_type, target, mapper, symbol) = {
            let d = self.type_(t).as_type_parameter().expect("TypeParameter data");
            (
                d.resolved_default_type,
                d.target,
                d.mapper,
                self.type_(t).symbol,
            )
        };
        if resolved_default_type.is_none() {
            if let Some(target) = target {
                // Go: instantiate the target's resolved default through
                // the type-parameter's mapper; a nil default (the port's
                // noConstraintType sentinel) maps to noConstraintType.
                let target_default = self.get_resolved_type_parameter_default(target);
                let no_constraint = self.no_constraint_type();
                let resolved = if target_default != no_constraint {
                    match mapper {
                        Some(mapper) => self.instantiate_type(target_default, mapper),
                        // Go: instantiateType(t, nil) is the identity.
                        None => target_default,
                    }
                } else {
                    no_constraint
                };
                if let Some(d) = self.type_mut(t).as_type_parameter_mut() {
                    d.resolved_default_type = Some(resolved);
                }
            } else {
                // To block recursion, set the initial value to the
                // resolvingDefaultType.
                let resolving = self.resolving_default_type.expect("resolvingDefaultType");
                if let Some(d) = self.type_mut(t).as_type_parameter_mut() {
                    d.resolved_default_type = Some(resolving);
                }
                let mut default_type = self.no_constraint_type();
                if let Some(symbol) = symbol {
                    let declarations = self.program_symbol(symbol).declarations.clone();
                    if let Some(&declaration) = declarations.iter().find(|&&decl| {
                        self.store().node(decl).kind == Kind::TypeParameter
                    }) {
                        let default_declaration = self
                            .store()
                            .node(declaration)
                            .as_type_parameter_declaration()
                            .expect("TypeParameterDeclaration data")
                            .default_type;
                        if let Some(default_declaration) = default_declaration {
                            default_type = self.get_type_from_type_node(default_declaration);
                        }
                    }
                }
                let current = self
                    .type_(t)
                    .as_type_parameter()
                    .and_then(|d| d.resolved_default_type);
                if current == Some(resolving) {
                    // If we have not been called recursively, set the correct
                    // default type.
                    if let Some(d) = self.type_mut(t).as_type_parameter_mut() {
                        d.resolved_default_type = Some(default_type);
                    }
                }
            }
        } else if resolved_default_type == self.resolving_default_type {
            // If we are called recursively for this type parameter, mark the
            // default as circular.
            let circular = self.circular_constraint_type();
            if let Some(d) = self.type_mut(t).as_type_parameter_mut() {
                d.resolved_default_type = Some(circular);
            }
        }
        self.type_(t)
            .as_type_parameter()
            .and_then(|d| d.resolved_default_type)
            .unwrap_or_else(|| self.no_constraint_type())
    }

    // ── the S3 seam this wave was bannered for ──

    /// Go: `func (c *Checker) getTypeParametersForTypeReferenceOrImport(node) []*Type` (checker.go:17574).
    pub(crate) fn get_type_parameters_for_type_reference_or_import(&mut self, node: NodeId) -> Vec<TypeId> {
        let t = self.get_type_from_type_node(node);
        if !crate::relater::is_error_type(self, t) {
            // Go: getResolvedSymbolOrNil(node) — the S6-resolved symbol read;
            // the symbolNodeLinks cache the type-reference family populated.
            let symbol = self.symbol_node_links.get(node).resolved_symbol;
            if let Some(symbol) = symbol {
                return self.get_type_parameters_for_type_and_symbol(t, symbol);
            }
        }
        Vec::new()
    }

    /// Go: `func (c *Checker) getTypeParametersForTypeAndSymbol(t, symbol) []*Type` (checker.go:17584).
    fn get_type_parameters_for_type_and_symbol(&mut self, t: TypeId, symbol: SymbolId) -> Vec<TypeId> {
        if !crate::relater::is_error_type(self, t) {
            if self
                .get_symbol_flags(symbol)
                .intersects(SymbolFlags::TYPE_ALIAS)
            {
                let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
                if !type_parameters.is_empty() {
                    return type_parameters;
                }
            }
            if self.type_(t).object_flags.intersects(ObjectFlags::REFERENCE) {
                if let Some(target) = self.type_(t).target() {
                    return self
                        .type_(target)
                        .as_interface_type()
                        .map(|d| d.local_type_parameters().to_vec())
                        .unwrap_or_default();
                }
            }
        }
        Vec::new()
    }
}

/// Go: `func isLocalTypeAlias(symbol) bool` (checker.go:24181).
fn is_local_type_alias(c: &mut Checker, symbol: SymbolId) -> bool {
    let declarations = c.program_symbol(symbol).declarations.clone();
    let declaration = declarations
        .into_iter()
        .find(|&d| u::is_type_alias(c.store().node(d)));
    declaration.is_some_and(|d| u::get_containing_function(c.store(), d).is_some())
}

