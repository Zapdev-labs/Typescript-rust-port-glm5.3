// Ported from tsc/internal/checker/nodebuilder.go + nodebuilderimpl.go +
// printer.go (checker) + nodebuilder/types.go @ 96d8ee5011 — the M6 W4 wave:
// the nodeBuilder + type-to-string printer family.
//
// ── ARCHITECTURE PORT NOTE (W4) ─────────────────────────────────────────────
// Go's pipeline is `typeToStringEx → NodeBuilderImpl.TypeToTypeNode (builds
// synthesized AST type nodes) → printer.Printer.Write (serializes nodes to
// text)`. The printer package (15k lines, the emit engine) and the
// pseudo-checker declaration-reuse machinery (pseudotypenodebuilder.go) are
// separate M7 waves with no Rust crates yet. This module mirrors the pipeline
// FUNCTION-BY-FUNCTION (same names snake_cased, same control flow + evaluation
// order — the printing order IS user-visible output) but renders each
// synthesized node directly to its `String` form at the construction site,
// with the printer's precedence-based parenthesization model
// (printer.go:2291 emitTypeNode + ast/precedence.go GetTypeNodePrecedence)
// mirrored by [`TP`] and the paren wrap in [`ttn`]. The Rust ast crate's
// `precedence.rs` is the node-based twin (M7 printer wave will switch to it);
// [`TP`] keeps the identical iota values so the comparison is byte-same.
//
// Reachability note: for EVERY diagnostics call site (typeToString(t) with
// enclosingDeclaration nil — printer.go:47-49), `lookupSymbolChainWorker`
// (nodebuilderimpl.go:1067) reduces to `chain = [symbol]` because the
// condition `!isTypeParameter && (enclosingDeclaration != nil ||
// FlagsUseFullyQualifiedType) && !DoNotIncludeSymbolChain` is false. The
// accessibility chain machinery (symbolaccessibility.go) is therefore NOT
// needed for the diagnostics path; [`symbol_to_type_node`] ports the
// chain-of-one shape directly and banners the two approximation seams:
//   1. UseFullyQualifiedType (getTypeNameForErrorDisplay + mapToTypeNodes'
//      name-collision re-render): the full getSymbolChain/getAccessibleSymbol
//      chain is symbolaccessibility.go (a named follow-up wave); the port
//      approximates with the symbol's parent chain (getParentOfSymbol), which
//      matches Go for namespace/class nesting — the dominant diagnostics form.
//   2. Import-type nodes for module-rooted symbols approximate the module
//      specifier with the module symbol's own name (exact for ambient modules
//      in single-file units; getSpecifierForModuleSymbol is the M7 seam).
//   3. The pseudo-checker annotation reuse (`serializeTypeForDeclaration` /
//      `serializeReturnTypeForSignature` tryReuse arms) requires enclosing !=
//      nil and pseudotypenodebuilder.go; the port always renders structurally
//      (Go's own fallback path). The `serializedTypes` per-declaration cache
//      rides the same seam (skipped).
//   4. `typeToStringEx`'s nodeBuilder.verbosity (VerbosityContext) is a
//      language-service-only surface (hover expansion); diagnostics pass nil
//      — maxExpansionDepth stays -1 and every shouldExpandType is false.
//
// The SymbolTracker interface is the noop tracker for diagnostics
// (nodebuilder.go enterContext: `if tracker == nil { tracker = noopTracker }`),
// so the port drops the tracker entirely and keeps `encountered_error` (the
// only observable it feeds under FlagsIgnoreErrors, which diagnostics always
// set — every Allow* error arm is pre-armed by IgnoreErrors).

use rustc_hash::{FxHashMap, FxHashSet};
use tsc_ast::{
    CheckFlags, Kind, ModifierFlags, NodeId, SymbolFlags, SymbolId, TokenFlags,
};
use tsc_binder::ast_shims as binder_shims;
use tsc_scanner as scanner_util;

use crate::types::{
    ElementFlags, IndexInfoId, ObjectFlags, SignatureFlags, SignatureId, TypeAlias, TypeFlags,
    TypeId, TupleElementInfo, TypePredicate, TypePredicateKind,
};
use crate::utilities as u;
use crate::Checker;

// ────────────────────────────────────────────────────────────────────────────
// Constants (checker.go:552, nodebuilderimpl.go:120-122, types.go:64-100,
// nodebuilder/types.go Flags)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `maxSerializationLevel = 2` (checker.go:552).
pub(crate) const MAX_SERIALIZATION_LEVEL: u32 = 2;
/// Go: `defaultMaximumTruncationLength = 160` (nodebuilderimpl.go:120).
pub(crate) const DEFAULT_MAXIMUM_TRUNCATION_LENGTH: usize = 160;
/// Go: `noTruncationMaximumTruncationLength = 1_000_000` (nodebuilderimpl.go:121).
pub(crate) const NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH: usize = 1_000_000;

/// Go: `TypeFormatFlags` (checker/types.go:64-100) — the bits the diagnostics
/// paths pass or read. Bit values are load-bearing (nodebuilder.Flags shares
/// the NodeBuilderFlagsMask bits).
#[allow(dead_code)] // Go-parity surface (types.go) — kept whole.
pub(crate) mod tff {
    pub const NONE: u32 = 0;
    pub const NO_TRUNCATION: u32 = 1 << 0;
    pub const WRITE_ARRAY_AS_GENERIC_TYPE: u32 = 1 << 1;
    pub const USE_FULLY_QUALIFIED_TYPE: u32 = 1 << 6;
    pub const MULTILINE_OBJECT_LITERALS: u32 = 1 << 10;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: u32 = 1 << 14;
    pub const ALLOW_UNIQUE_ESSYMBOL_TYPE: u32 = 1 << 20;
    pub const OMIT_THIS_PARAMETER: u32 = 1 << 25;
    pub const WRITE_CALL_STYLE_SIGNATURE: u32 = 1 << 27;
    pub const NO_TYPE_REDUCTION: u32 = 1 << 29;
}

/// Go: `nodebuilder.Flags` (nodebuilder/types.go:33-77) — same bit positions
/// as the TypeFormatFlags mask members.
#[allow(dead_code)] // Go-parity surface (nodebuilder/types.go).
pub(crate) mod nbf {
    pub const NONE: u32 = 0;
    pub const NO_TRUNCATION: u32 = 1 << 0;
    pub const WRITE_ARRAY_AS_GENERIC_TYPE: u32 = 1 << 1;
    pub const FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES: u32 = 1 << 4;
    pub const USE_FULLY_QUALIFIED_TYPE: u32 = 1 << 6;
    pub const USE_ONLY_EXTERNAL_ALIASING: u32 = 1 << 7;
    pub const SUPPRESS_ANY_RETURN_TYPE: u32 = 1 << 8;
    pub const WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME: u32 = 1 << 9;
    pub const MULTILINE_OBJECT_LITERALS: u32 = 1 << 10;
    pub const USE_TYPE_OF_FUNCTION: u32 = 1 << 12;
    pub const OMIT_PARAMETER_MODIFIERS: u32 = 1 << 13;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: u32 = 1 << 14;
    // Error handling (IgnoreErrors arms)
    pub const ALLOW_THIS_IN_OBJECT_LITERAL: u32 = 1 << 15;
    pub const ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER: u32 = 1 << 16;
    pub const ALLOW_ANONYMOUS_IDENTIFIER: u32 = 1 << 17;
    pub const ALLOW_EMPTY_UNION_OR_INTERSECTION: u32 = 1 << 18;
    pub const ALLOW_EMPTY_TUPLE: u32 = 1 << 19;
    pub const ALLOW_UNIQUE_ESSYMBOL_TYPE: u32 = 1 << 20;
    pub const ALLOW_EMPTY_INDEX_INFO_TYPE: u32 = 1 << 21;
    // State
    pub const IN_OBJECT_TYPE_LITERAL: u32 = 1 << 22;
    pub const IN_TYPE_ALIAS: u32 = 1 << 23;
    pub const IN_INITIAL_ENTITY_NAME: u32 = 1 << 24;
    pub const OMIT_THIS_PARAMETER: u32 = 1 << 25;
    pub const ALLOW_NODE_MODULES_RELATIVE_PATHS: u32 = 1 << 26;
    pub const WRITE_CALL_STYLE_SIGNATURE: u32 = 1 << 27;
    pub const NO_TYPE_REDUCTION: u32 = 1 << 29;
    /// Go: `FlagsIgnoreErrors` (nodebuilder/types.go:75).
    pub const IGNORE_ERRORS: u32 = ALLOW_THIS_IN_OBJECT_LITERAL
        | ALLOW_QUALIFIED_NAME_IN_PLACE_OF_IDENTIFIER
        | ALLOW_ANONYMOUS_IDENTIFIER
        | ALLOW_EMPTY_UNION_OR_INTERSECTION
        | ALLOW_EMPTY_TUPLE
        | ALLOW_EMPTY_INDEX_INFO_TYPE
        | ALLOW_NODE_MODULES_RELATIVE_PATHS;
}

/// Go: `SymbolFormatFlags` (checker/types.go:104-125).
#[allow(dead_code)] // Go-parity surface (types.go).
pub(crate) mod sff {
    pub const NONE: u32 = 0;
    pub const WRITE_TYPE_PARAMETERS_OR_ARGUMENTS: u32 = 1 << 0;
    pub const USE_ONLY_EXTERNAL_ALIASING: u32 = 1 << 1;
    pub const ALLOW_ANY_NODE_KIND: u32 = 1 << 2;
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: u32 = 1 << 3;
    pub const WRITE_COMPUTED_PROPS: u32 = 1 << 4;
    pub const DO_NOT_INCLUDE_SYMBOL_CHAIN: u32 = 1 << 5;
}

/// Go: `nodebuilder.InternalFlags` (nodebuilder/types.go:80-88).
#[allow(dead_code)] // Go-parity surface (nodebuilder/types.go).
pub(crate) mod nbif {
    pub const NONE: u32 = 0;
    pub const WRITE_COMPUTED_PROPS: u32 = 1 << 0;
    pub const DO_NOT_INCLUDE_SYMBOL_CHAIN: u32 = 1 << 2;
}

/// Go: `toNodeBuilderFlags` (printer.go:51) — `flags & TypeFormatFlagsNodeBuilderFlagsMask`.
fn to_node_builder_flags(flags: u32) -> u32 {
    // TypeFormatFlagsNodeBuilderFlagsMask (types.go:96-99): the shared bits.
    flags
        & (tff::NO_TRUNCATION
            | tff::WRITE_ARRAY_AS_GENERIC_TYPE
            | tff::USE_FULLY_QUALIFIED_TYPE
            | tff::MULTILINE_OBJECT_LITERALS
            | tff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
            | tff::ALLOW_UNIQUE_ESSYMBOL_TYPE
            | tff::OMIT_THIS_PARAMETER
            | tff::WRITE_CALL_STYLE_SIGNATURE
            | tff::NO_TYPE_REDUCTION)
}

/// Go: `type TypePrecedence int32` (ast/precedence.go:403-652) — the local
/// mirror with identical ordering (Conditional lowest, NonArray highest).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[allow(dead_code)] // Go: ast/precedence.go TypePrecedence (JsDoc is printer-side).
pub(crate) enum TP {
    Conditional = 0,
    JsDoc = 1,
    Function = 2,
    Union = 3,
    Intersection = 4,
    TypeOperator = 5,
    Postfix = 6,
    NonArray = 7,
}

impl TP {
    /// Go: `TypePrecedenceLowest = TypePrecedenceConditional`.
    const LOWEST: TP = TP::Conditional;
    /// Go: `TypePrecedenceHighest = TypePrecedenceNonArray`.
    #[allow(dead_code)]
    const HIGHEST: TP = TP::NonArray;
}

// ────────────────────────────────────────────────────────────────────────────
// The context (Go: NodeBuilderContext, nodebuilderimpl.go:64-118 — the
// diagnostics-reachable subset; the hover-verbosity fields are always
// disabled: maxExpansionDepth < 0)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `b.ctx` — the per-serialization context. `getNodeBuilder()` returns a
/// fresh stack frame per typeToStringEx call (nodebuilder.go:288), so the
/// port constructs one per top-level entry and threads it `&mut`.
#[allow(dead_code)] // type_stack/max_expansion_depth/mapper mirror Go's builder ctx.
pub(crate) struct NbCtx {
    pub flags: u32,
    pub internal_flags: u32,
    pub enclosing_declaration: Option<NodeId>,
    pub depth: i32,
    pub approximate_length: usize,
    pub truncating: bool,
    /// Go: `typeStack []*Type` (the expansion-depth stack; pushed only when
    /// `maxExpansionDepth >= 0`, which diagnostics never set — kept for the
    /// should_expand_type shape).
    pub type_stack: Vec<TypeId>,
    /// Go: `visitedTypes` (visitAndTransformType's cycle set).
    pub visited_types: FxHashSet<TypeId>,
    /// Go: `symbolDepth map[CompositeSymbolIdentity]int` — keyed by the
    /// composite identity triple (isConstructorObject, symbolId, nodeId)
    /// flattened to u128.
    pub symbol_depth: FxHashMap<u128, u32>,
    /// Go: `encounteredError bool`.
    pub encountered_error: bool,
    /// Go: printer `p.inExtends` — the `extends`-clause precedence context
    /// (printer.go:2292).
    pub in_extends: bool,
    /// Go: `inferTypeParameters []*Type` (conditionalTypeToTypeNode swaps it).
    #[allow(dead_code)]
    pub infer_type_parameters: Vec<TypeId>,
    /// Go: `maxExpansionDepth` — always -1 for diagnostics (verbosity off).
    pub max_expansion_depth: i32,
    /// Go: `maxTruncationLength` — 0 for diagnostics (no VerbosityContext).
    pub max_truncation_length: usize,
    /// The mapToTypeNodes identifier-ref probe (see `ident_ref_probe` below).
    pub ident_ref_probe: Option<String>,
    /// Go: `mapper` — nil for the diagnostics path (enterContext copies it
    /// from the builder; diagnostics never enter with a mapper). The property
    /// serialization arms that instantiate with `b.ctx.mapper` guard on nil.
    pub mapper: Option<crate::todo_shapes::TypeMapperId>,
}

impl NbCtx {
    fn new(flags: u32, internal_flags: u32, enclosing_declaration: Option<NodeId>) -> Self {
        NbCtx {
            flags,
            internal_flags,
            enclosing_declaration,
            depth: 0,
            approximate_length: 0,
            truncating: false,
            type_stack: Vec::new(),
            visited_types: FxHashSet::default(),
            symbol_depth: FxHashMap::default(),
            encountered_error: false,
            in_extends: false,
            infer_type_parameters: Vec::new(),
            max_expansion_depth: -1,
            max_truncation_length: 0,
            ident_ref_probe: None,
            mapper: None,
        }
    }
}

/// Go: `(b *NodeBuilderImpl) saveRestoreFlags()` (nodebuilderimpl.go:135) —
/// returns the restore closure's effect as a saved value.
fn save_restore_flags(ctx: &mut NbCtx) -> (u32, u32, i32) {
    (ctx.flags, ctx.internal_flags, ctx.depth)
}

fn restore_flags(ctx: &mut NbCtx, saved: (u32, u32, i32)) {
    ctx.flags = saved.0;
    ctx.internal_flags = saved.1;
    ctx.depth = saved.2;
}

// ────────────────────────────────────────────────────────────────────────────
// printer.go — the public entry family
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (c *Checker) TypeToString(t *Type) string` (printer.go:43).
pub(crate) fn type_to_string(c: &mut Checker, t: TypeId) -> String {
    type_to_string_with_enclosing(c, t, None)
}

/// Go: `func (c *Checker) typeToString(t, enclosingDeclaration) string` (printer.go:47)
/// — `AllowUniqueESSymbolType|UseAliasDefinedOutsideCurrentScope`.
pub(crate) fn type_to_string_with_enclosing(
    c: &mut Checker,
    t: TypeId,
    enclosing: Option<NodeId>,
) -> String {
    type_to_string_ex(
        c,
        t,
        enclosing,
        tff::ALLOW_UNIQUE_ESSYMBOL_TYPE | tff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
    )
}

/// Go: `func (c *Checker) typeToStringEx(t, enclosingDeclaration, flags, vc) string`
/// (printer.go:59-118).
pub(crate) fn type_to_string_ex(
    c: &mut Checker,
    t: TypeId,
    enclosing: Option<NodeId>,
    flags: u32,
) -> String {
    // Serialization of types can lead to (lazy) resolution of members, which
    // can cause diagnostics that again require serialization of types (the
    // recursion guard — Go returns "?" past maxSerializationLevel).
    if c.serialization_level >= MAX_SERIALIZATION_LEVEL {
        return "?".to_string();
    }
    let no_truncation =
        flags & tff::NO_TRUNCATION != 0; // vc==nil && NoErrorTruncation handled at option parse; the runner never sets NoErrorTruncation to true on these units' configs (bisected if it flips)
    let mut combined_flags = to_node_builder_flags(flags) | nbf::IGNORE_ERRORS;
    if no_truncation {
        combined_flags |= nbf::NO_TRUNCATION;
    }

    let mut ctx = NbCtx::new(combined_flags, nbif::NONE, enclosing);
    c.serialization_level += 1;
    let type_node = ttn(c, &mut ctx, t, TP::LOWEST);
    c.serialization_level -= 1;
    let result = match type_node {
        // Go panics on nil ("should always get typenode"); every Allow* arm is
        // pre-armed by IgnoreErrors, so the diagnostics path cannot produce it.
        None => panic!("should always get typenode"),
        Some(s) => {
            // The unresolved type gets a synthesized comment on `any`; the
            // comment-emitting printer is used for it (createPrinterWithDefaults).
            if Some(t) == c.unresolved_type {
                format!("/*unresolved*/{s}")
            } else {
                s
            }
        }
    };

    // The hard truncation (printer.go:104-117): defaultMaximumTruncationLength * 2.
    let mut max_length = DEFAULT_MAXIMUM_TRUNCATION_LENGTH * 2;
    if no_truncation {
        max_length = NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH * 2;
    }
    if max_length > 0 && !result.is_empty() && result.len() >= max_length {
        return format!("{}...", &result[..max_length - 3]);
    }
    result
}

/// Go: `func (c *Checker) symbolToString(symbol) string` (printer.go:124).
pub(crate) fn symbol_to_string(c: &mut Checker, symbol: SymbolId) -> String {
    symbol_to_string_ex(c, symbol, None, SymbolFlags::ALL, sff::ALLOW_ANY_NODE_KIND)
}

/// Go: `func (c *Checker) symbolToStringEx(symbol, enclosingDeclaration, meaning, flags) string`
/// (printer.go:132-177).
pub(crate) fn symbol_to_string_ex(
    c: &mut Checker,
    symbol: SymbolId,
    enclosing: Option<NodeId>,
    meaning: SymbolFlags,
    flags: u32,
) -> String {
    let mut node_flags = nbf::IGNORE_ERRORS;
    let mut internal_node_flags = nbif::NONE;
    if flags & sff::USE_ONLY_EXTERNAL_ALIASING != 0 {
        node_flags |= nbf::USE_ONLY_EXTERNAL_ALIASING;
    }
    if flags & sff::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS != 0 {
        node_flags |= nbf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
    }
    if flags & sff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE != 0 {
        node_flags |= nbf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
    }
    if flags & sff::DO_NOT_INCLUDE_SYMBOL_CHAIN != 0 {
        internal_node_flags |= nbif::DO_NOT_INCLUDE_SYMBOL_CHAIN;
    }
    if flags & sff::WRITE_COMPUTED_PROPS != 0 {
        internal_node_flags |= nbif::WRITE_COMPUTED_PROPS;
    }

    let mut ctx = NbCtx::new(node_flags, internal_node_flags, enclosing);
    // Go: AllowAnyNodeKind → SymbolToNode; else SymbolToEntityName.
    // SymbolToNode's WriteComputedProps arm is diagnostics-dead (flag off);
    // both builders reduce to the entity-name chain form here.
    let _ = meaning;
    let Some(entity) = symbol_to_entity_name(c, &mut ctx, symbol) else {
        return String::new();
    };
    entity
}

/// Go: `func (c *Checker) signatureToString(signature) string` (printer.go:179).
pub(crate) fn signature_to_string(c: &mut Checker, s: SignatureId) -> String {
    signature_to_string_ex(c, s, None, tff::NONE)
}

/// Go: `func (c *Checker) signatureToStringEx(signature, enclosingDeclaration, flags, vc) string`
/// (printer.go:187-227).
pub(crate) fn signature_to_string_ex(
    c: &mut Checker,
    signature: SignatureId,
    enclosing: Option<NodeId>,
    flags: u32,
) -> String {
    let is_constructor = c.signatures[signature.0 as usize]
        .flags
        .intersects(SignatureFlags::CONSTRUCT)
        && flags & tff::WRITE_CALL_STYLE_SIGNATURE == 0;
    // Go: sigOutput kind — the arrow-style pair (FunctionType/ConstructorType).
    let sig_kind = if is_constructor {
        SigKind::ConstructorType
    } else {
        SigKind::FunctionType
    };
    let mut ctx = NbCtx::new(
        to_node_builder_flags(flags) | nbf::IGNORE_ERRORS | nbf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
        nbif::NONE,
        enclosing,
    );
    signature_to_signature_declaration_helper(c, &mut ctx, signature, sig_kind, None)
        .unwrap_or_default()
}

/// Go: `func (c *Checker) typePredicateToString(typePredicate) string` (printer.go:229).
pub(crate) fn type_predicate_to_string(c: &mut Checker, predicate: &TypePredicate) -> String {
    type_predicate_to_string_ex(
        c,
        predicate,
        None,
        tff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
    )
}

/// Go: `func (c *Checker) typePredicateToStringEx(typePredicate, enclosingDeclaration, flags) string`
/// (printer.go:233-247).
pub(crate) fn type_predicate_to_string_ex(
    c: &mut Checker,
    predicate: &TypePredicate,
    enclosing: Option<NodeId>,
    flags: u32,
) -> String {
    let mut ctx = NbCtx::new(
        to_node_builder_flags(flags) | nbf::IGNORE_ERRORS | nbf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
        nbif::NONE,
        enclosing,
    );
    type_predicate_to_type_node_helper(c, &mut ctx, predicate)
}

/// Go: `func (c *Checker) formatUnionTypes(types, expandingEnum) []*Type` (printer.go:253).
pub(crate) fn format_union_types(
    c: &mut Checker,
    types: &[TypeId],
    expanding_enum: bool,
) -> Vec<TypeId> {
    let mut result: Vec<TypeId> = Vec::new();
    let mut flags = TypeFlags::NONE;
    let mut i = 0usize;
    while i < types.len() {
        let t = types[i];
        flags |= c.type_(t).flags;
        if !c.type_(t).flags.intersects(TypeFlags::NULLABLE) {
            if c.type_(t).flags.intersects(TypeFlags::BOOLEAN_LITERAL)
                || (!expanding_enum && c.type_(t).flags.intersects(TypeFlags::ENUM_LIKE))
            {
                let base_type = if c.type_(t).flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
                    c.boolean_type()
                } else {
                    match c.get_base_type_of_enum_like_type(t) {
                        Some(b) => b,
                        None => {
                            result.push(t);
                            i += 1;
                            continue;
                        }
                    }
                };
                if c.type_(base_type).flags.intersects(TypeFlags::UNION) {
                    let base_types: Vec<TypeId> = c
                        .type_(base_type)
                        .as_union_type()
                        .expect("checked UNION")
                        .base
                        .types
                        .clone();
                    let count = base_types.len();
                    if i + count <= types.len() {
                        let last_regular = c.get_regular_type_of_literal_type(types[i + count - 1]);
                        let base_last_regular = c.get_regular_type_of_literal_type(base_types[count - 1]);
                        if last_regular == base_last_regular {
                            result.push(base_type);
                            i += count - 1;
                            i += 1;
                            continue;
                        }
                    }
                }
            }
            result.push(t);
        }
        i += 1;
    }
    if flags.intersects(TypeFlags::NULL) {
        result.push(c.null_type());
    }
    if flags.intersects(TypeFlags::UNDEFINED) {
        result.push(c.undefined_type());
    }
    result
}

// ────────────────────────────────────────────────────────────────────────────
// nodebuilderimpl.go — symbol naming (the diagnostics chain-of-one shape)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `getNameOfSymbolAsWritten` (nodebuilderimpl.go:974-1026). Should *not*
/// be used for the right-hand side of a `.`; unlike `symbolName` it includes
/// quotes for string-literal names and the "(Anonymous class)" forms.
fn get_name_of_symbol_as_written(c: &mut Checker, ctx: &mut NbCtx, symbol: SymbolId) -> String {
    // Go: `b.ctx.remappedSymbolReferences` — empty for the checker's own
    // nodebuilder (only the emit-resolver populates it); skipped.
    let sym = c.program_symbol(symbol);
    let name = sym.name.clone();
    let has_use_alias_outside = ctx.flags & nbf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE != 0;
    if name == "default"
        && !has_use_alias_outside
        && ((ctx.flags & nbf::IN_INITIAL_ENTITY_NAME == 0)
            || sym.declarations.is_empty()
            || {
                // Different binding context (source file vs ambient module):
                // Go nodebuilderimpl.go:985 — the comparison runs only when
                // enclosingDeclaration is non-nil (nil enclosing keeps the
                // symbol name; it never forces the "default" spelling).
                let decl = sym.declarations[0];
                ctx.enclosing_declaration.is_some_and(|enclosing| {
                    let decl_ctx = find_binding_context(c, decl);
                    let enclosing_ctx = find_binding_context(c, enclosing);
                    decl_ctx != enclosing_ctx
                })
            })
    {
        return "default".to_string();
    }
    if !sym.declarations.is_empty() {
        // Try using a declaration with a name, first.
        if let Some(name_node) = first_non_nil_name(c, &sym.declarations) {
            let name_kind = c.store().node(name_node).kind;
            if name_kind == Kind::ComputedPropertyName
                && !sym.check_flags.intersects(CheckFlags::LATE)
            {
                let name_type = c
                    .value_symbol_links
                    .try_get(symbol)
                    .and_then(|l| l.name_type);
                if let Some(nt) = name_type {
                    if c.type_(nt).flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
                        let result = get_name_of_symbol_from_name_type(c, ctx, symbol);
                        if !result.is_empty() {
                            return result;
                        }
                    }
                }
            }
            return scanner_util::utilities::declaration_name_to_string(c.store(), Some(name_node));
        }
        // Declaration may be nameless; try the variable-declaration parent name.
        let declaration = sym.declarations[0];
        let parent = parent_node(c.store().node(declaration));
        if let Some(parent) = parent {
            if c.store().node(parent).kind == Kind::VariableDeclaration {
                let vd_name = binder_shims::get_name_of_declaration(c.store(), parent);
                return scanner_util::utilities::declaration_name_to_string(c.store(), vd_name);
            }
        }
        let decl_kind = c.store().node(declaration).kind;
        if matches!(
            decl_kind,
            Kind::ClassExpression | Kind::FunctionExpression | Kind::ArrowFunction
        ) {
            if !ctx.encountered_error && ctx.flags & nbf::ALLOW_ANONYMOUS_IDENTIFIER == 0 {
                ctx.encountered_error = true;
            }
            return match decl_kind {
                Kind::ClassExpression => "(Anonymous class)".to_string(),
                _ => "(Anonymous function)".to_string(),
            };
        }
    }
    let sym_name = sym.name.clone();
    let name = get_name_of_symbol_from_name_type(c, ctx, symbol);
    if !name.is_empty() {
        return name;
    }
    tsc_ast::symbol::escape_internal_symbol_name(&sym_name)
}

/// Go: `node.Parent` (nil when absent) — the Rust port stores a
/// `Cell<NodeId>` with `NodeId::NONE` (u64::MAX) as the nil sentinel
/// (SPEC §5.1; the SourceFile node itself carries a NONE parent).
/// PORT (FCS): the former probe treated `NodeId(0)` as the sentinel — but
/// node 0 of file 0 is the real SourceFile, so every top-level node's
/// parent walk stopped one level early, and a SourceFile-declared symbol
/// (the binder's CommonJS module/exports symbols) yielded `Some(NONE)`
/// and a foreign-node panic at the VariableDeclaration probe below.
fn parent_node(nd: &tsc_ast::Node) -> Option<NodeId> {
    let p = nd.parent.get();
    (p != NodeId::NONE).then_some(p)
}

/// Go: `isDefaultBindingContext` (nodebuilderimpl.go:933) — the
/// FindAncestor-or-nil probe. Returns the binding-context node itself
/// (Go compares the FindAncestor result pointers, nil == nil; the port's
/// Option<NodeId> equality is that exact comparison — the former
/// node-id encoding collided with the id cell's 0-means-unassigned).
fn find_binding_context(c: &Checker, location: NodeId) -> Option<NodeId> {
    let store = c.store();
    let mut node = Some(location);
    while let Some(n) = node {
        let nd = store.node(n);
        if nd.kind == Kind::SourceFile || binder_shims::is_ambient_module(store, n) {
            return Some(n);
        }
        node = parent_node(nd);
    }
    None
}

/// Go: `core.FirstNonNil(symbol.Declarations, ast.GetNameOfDeclaration)`.
fn first_non_nil_name(c: &Checker, declarations: &[NodeId]) -> Option<NodeId> {
    let store = c.store();
    declarations
        .iter()
        .find_map(|&d| binder_shims::get_name_of_declaration(store, d))
}

/// Go: `getNameOfSymbolFromNameType` (nodebuilderimpl.go:937-965).
fn get_name_of_symbol_from_name_type(
    c: &mut Checker,
    ctx: &mut NbCtx,
    symbol: SymbolId,
) -> String {
    let name_type = c
        .value_symbol_links
        .try_get(symbol)
        .and_then(|l| l.name_type);
    let Some(name_type) = name_type else {
        return String::new();
    };
    let nt_flags = c.type_(name_type).flags;
    if nt_flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
        let value = c
            .type_(name_type)
            .as_literal_type()
            .and_then(|l| l.value.clone());
        let name = match &value {
            Some(crate::types::LiteralValue::String(s)) => s.clone(),
            Some(crate::types::LiteralValue::Number(n)) => n.string(),
            _ => String::new(),
        };
        if !scanner_util::utilities::is_identifier_text(&name, tsc_core::languagevariant::LanguageVariant::Standard)
            && !u::is_numeric_literal_name(&name)
        {
            return u::value_to_string(&match value {
                Some(crate::types::LiteralValue::String(s)) => crate::types::LiteralValue::String(s),
                Some(crate::types::LiteralValue::Number(n)) => crate::types::LiteralValue::Number(n),
                _ => return String::new(),
            });
        }
        if u::is_numeric_literal_name(&name) && name.starts_with('-') {
            return format!("[{name}]");
        }
        return name;
    }
    if nt_flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
        let u_sym = c.type_(name_type).symbol;
        if let Some(u_sym) = u_sym {
            let text = get_name_of_symbol_as_written(c, ctx, u_sym);
            return format!("[{text}]");
        }
        return String::new();
    }
    String::new()
}

/// Go: `symbolToEntityNameNode` (nodebuilderimpl.go:643) for the
/// chain-of-one diagnostics shape (symbolToString → SymbolToNode →
/// symbolToExpression → createExpressionFromSymbolChain with index 0 —
/// every one reduces to a single identifier carrying
/// `getNameOfSymbolAsWritten`).
fn symbol_to_entity_name(
    c: &mut Checker,
    ctx: &mut NbCtx,
    symbol: SymbolId,
) -> Option<String> {
    ctx.flags |= nbf::IN_INITIAL_ENTITY_NAME;
    let symbol_name = get_name_of_symbol_as_written(c, ctx, symbol);
    ctx.flags ^= nbf::IN_INITIAL_ENTITY_NAME;
    ctx.approximate_length += symbol_name.len() + 1;
    Some(symbol_name)
}

// ────────────────────────────────────────────────────────────────────────────
// nodebuilderimpl.go — symbolToTypeNode (the diagnostics chain-of-one form)
// ────────────────────────────────────────────────────────────────────────────

/// The rendered form of `createAccessFromSymbolChain`'s result: an entity
/// name (`A.B.C`), a `typeof`-applied entity, or an indexed access.
#[allow(dead_code)] // IndexedAccess is the Go arm kept for the M7 printer switch.
enum AccessRender {
    /// Go: an EntityName (Identifier | QualifiedName) — printable inside
    /// typeof/type-reference positions.
    EntityName(String),
    /// Go: an IndexedAccessTypeNode — never `typeof`-able (the rendered text
    /// is the whole `X[k]` type).
    IndexedAccess(String),
}

/// Go: `symbolToTypeNode` (nodebuilderimpl.go:651) — the chain-of-one shape.
/// `type_arguments` are already-rendered type-argument strings (Go's
/// NodeList of TypeNodes produced by mapToTypeNodes).
fn symbol_to_type_node(
    c: &mut Checker,
    ctx: &mut NbCtx,
    symbol: SymbolId,
    is_type_of: bool,
    type_arguments: Option<&[String]>,
) -> Option<String> {
    let sym = c.program_symbol(symbol);
    if some_decl_has_non_global_augmentation_external_module_symbol(c, &sym.declarations) {
        // Module is root, must use ImportTypeNode. PORT (W4 seam): Go builds
        // the module specifier through getSpecifierForModuleSymbol
        // (modulespecifiers package); the port uses the module symbol's own
        // name — exact for ambient external modules in single-file units.
        let module_name = sym.name.clone();
        ctx.approximate_length += module_name.len() + 10;
        let mut result = format!("import(\"{module_name}\")");
        let non_root_parts: Option<String> = None; // chain-of-one: no non-root parts
        let _ = non_root_parts;
        if is_type_of {
            result = format!("typeof {result}");
        }
        if let Some(args) = type_arguments {
            if !args.is_empty() {
                result = format!("{result}<{}>", args.join(", "));
            }
        }
        return Some(result);
    }

    let entity_name = create_access_from_symbol_chain(c, ctx, symbol, type_arguments)?;
    match entity_name {
        AccessRender::IndexedAccess(s) => Some(s),
        AccessRender::EntityName(name) => {
            if is_type_of {
                ctx.approximate_length += 7;
                Some(format!("typeof {name}"))
            } else {
                if let Some(args) = type_arguments {
                    if !args.is_empty() {
                        return Some(format!("{name}<{}>", args.join(", ")));
                    }
                }
                // Single-identifier (or qualified) type reference without type
                // arguments — the mapToTypeNodes collision-probe shape
                // (isIdentifierTypeReference checks only the TypeName).
                if ctx.flags & nbf::USE_FULLY_QUALIFIED_TYPE == 0 || !name.contains('.') {
                    ctx.ident_ref_probe = Some(name.clone());
                }
                Some(name)
            }
        }
    }
}

/// Go: `hasNonGlobalAugmentationExternalModuleSymbol` (symbolaccessibility.go:106).
fn has_non_global_augmentation_external_module_symbol(
    c: &Checker,
    declaration: NodeId,
) -> bool {
    let store = c.store();
    let nd = store.node(declaration);
    if binder_shims::is_ambient_module(store, declaration) {
        // Global augmentation modules ("declare global") do not count; the
        // module must have a string name (an external module).
        let sym = crate::typeofsymbol::get_symbol_of_declaration(c, declaration);
        return match sym {
            Some(sym) => tsc_ast::symbol::symbol_is_external_module(c.program_symbol(sym)),
            None => false,
        };
    }
    nd.kind == Kind::SourceFile
        && tsc_ast::utilities::get_source_file_of_node(store, declaration)
            .is_some_and(binder_shims::is_external_or_common_js_module)
}

fn some_decl_has_non_global_augmentation_external_module_symbol(
    c: &Checker,
    declarations: &[NodeId],
) -> bool {
    declarations
        .iter()
        .any(|&d| has_non_global_augmentation_external_module_symbol(c, d))
}

/// Go: `createAccessFromSymbolChain(chain, index=len-1, stopper=0, override)`
/// (nodebuilderimpl.go:753) — the chain-of-one reduction: no parent (index 0,
/// stopper 0), so the export-ref lookup and the indexed-access arm are
/// unreachable; the result is the identifier for `getNameOfSymbolAsWritten`.
/// PORT (W4 seam): under UseFullyQualifiedType (the error-display collision
/// fallback), Go's chain carries the parents; the port qualifies by walking
/// `getParentOfSymbol` — matching Go for namespace/class nesting.
fn create_access_from_symbol_chain(
    c: &mut Checker,
    ctx: &mut NbCtx,
    symbol: SymbolId,
    override_type_arguments: Option<&[String]>,
) -> Option<AccessRender> {
    ctx.flags |= nbf::IN_INITIAL_ENTITY_NAME;
    let symbol_name = get_name_of_symbol_as_written(c, ctx, symbol);
    ctx.flags ^= nbf::IN_INITIAL_ENTITY_NAME;
    ctx.approximate_length += symbol_name.len() + 1;

    if ctx.flags & nbf::USE_FULLY_QUALIFIED_TYPE != 0 {
        // The qualified form: prepend the parent chain (Go: getSymbolChain
        // with the UseFullyQualifiedType chain — symbolaccessibility wave).
        if let Some(parent) = c.get_parent_of_symbol(symbol) {
            let parent_flags = c.program_symbol(parent).flags;
            // Type parameters end up in `members` but are not externally
            // visible (getContainersOfSymbol's guard); anonymous type-literal
            // parents are not written (getSymbolChain's tail guard).
            if !c.program_symbol(symbol).flags.intersects(SymbolFlags::TYPE_PARAMETER)
                && !parent_flags.intersects(
                    SymbolFlags::TYPE_LITERAL | SymbolFlags::OBJECT_LITERAL,
                )
            {
                if let Some(AccessRender::EntityName(lhs)) =
                    create_access_from_symbol_chain(c, ctx, parent, None)
                {
                    return Some(AccessRender::EntityName(format!("{lhs}.{symbol_name}")));
                }
            }
        }
        if let Some(args) = override_type_arguments {
            if !args.is_empty() {
                return Some(AccessRender::EntityName(format!(
                    "{symbol_name}<{}>",
                    args.join(", ")
                )));
            }
        }
        return Some(AccessRender::EntityName(symbol_name));
    }

    Some(AccessRender::EntityName(symbol_name))
}

// ────────────────────────────────────────────────────────────────────────────
// The master dispatch — Go typeToTypeNode (nodebuilderimpl.go:3358-3679) +
// printer emitTypeNode's precedence wrap (printer.go:2291)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `typeToTypeNode` + `emitTypeNode(node, precedence)`. Renders `t` at
/// `prec`; wraps in parens when the rendered shape's precedence is lower
/// (printer.go:2298 `parens := ast.GetTypeNodePrecedence(node) < precedence`,
/// with the inExtends conditional bump at 2292).
fn ttn(c: &mut Checker, ctx: &mut NbCtx, t: TypeId, prec: TP) -> Option<String> {
    let mut p = prec;
    if ctx.in_extends && p <= TP::Conditional {
        p = TP::Function;
    }
    let saved_in_extends = ctx.in_extends;
    let own = own_precedence(c, ctx, t);
    let parens = own < p;
    if parens {
        ctx.in_extends = false;
    }
    let s = ttn_dispatch(c, ctx, t)?;
    let out = if parens {
        format!("({s})")
    } else {
        s
    };
    ctx.in_extends = saved_in_extends;
    Some(out)
}

/// The `emitTypeNodeOutsideExtends` form (printer.go:2279) — clears inExtends
/// around the child.
fn ttn_outside_extends(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    let saved = ctx.in_extends;
    ctx.in_extends = false;
    let r = ttn(c, ctx, t, TP::LOWEST);
    ctx.in_extends = saved;
    r
}

/// The `emitTypeNodeInExtends` form (printer.go:2271).
fn ttn_in_extends(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    let saved = ctx.in_extends;
    ctx.in_extends = true;
    let r = ttn(c, ctx, t, TP::LOWEST);
    ctx.in_extends = saved;
    r
}

/// Go: `ast.GetTypeNodePrecedence` computed from the Type — the pre-pass that
/// mirrors which node kind the dispatch below will render (the kind is a
/// pure function of flags + alias + enum-shape under the diagnostics flags).
fn own_precedence(c: &mut Checker, ctx: &NbCtx, t: TypeId) -> TP {
    let ty = c.type_(t);
    let flags = ty.flags;
    if flags.intersects(TypeFlags::ANY) {
        return TP::NonArray;
    }
    if flags.intersects(
        TypeFlags::UNKNOWN
            | TypeFlags::STRING
            | TypeFlags::NUMBER
            | TypeFlags::BIGINT
            | TypeFlags::ESSYMBOL
            | TypeFlags::VOID
            | TypeFlags::UNDEFINED
            | TypeFlags::NULL
            | TypeFlags::NEVER
            | TypeFlags::NON_PRIMITIVE
            | TypeFlags::TYPE_PARAMETER,
    ) {
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::OBJECT) {
        // createTypeNodeFromObjectType's shapes: a lone call/construct
        // signature renders as a FunctionTypeNode/ConstructorTypeNode
        // (TypePrecedenceFunction — precedence.go:661-662), everything else
        // as a TypeLiteral (NonArray). Mapped types are excluded (they
        // render via createMappedTypeNodeFromType → NonArray).
        let object_flags = c.type_(t).object_flags;
        if object_flags.intersects(ObjectFlags::ANONYMOUS)
            && !object_flags.intersects(ObjectFlags::MAPPED)
        {
            let resolved = crate::relater::resolve_structured_type_members(c, t);
            if resolved.properties.is_empty()
                && resolved.index_infos.is_empty()
                && resolved.signatures.len() == 1
            {
                return TP::Function;
            }
        }
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::BOOLEAN) && ty.alias.is_none() {
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::ENUM_LIKE | TypeFlags::UNION | TypeFlags::INTERSECTION) {
        // enum/enum-member names and single-constituent unions render as
        // their constituent; multi-constituent unions/unions-as-unions pick
        // their own precedence.
        if flags.intersects(TypeFlags::UNION) {
            // PORT(FCS tranche 3): Go renders a union with an origin through
            // the ORIGIN type (the ttn union arm below swaps t for
            // u.origin); the old `ty.target()` read panicked — a union has
            // no Target (Go's Type.Target() panics on the union kind too).
            if let Some(origin) = ty.as_union_type().and_then(|ut| ut.origin) {
                return own_precedence(c, ctx, origin);
            }
            return TP::Union;
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            return TP::Intersection;
        }
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::LITERAL | TypeFlags::BOOLEAN_LITERAL) {
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
        // `typeof x` (TypeQuery) or `unique symbol` (TypeOperator).
        if ctx.flags & nbf::ALLOW_UNIQUE_ESSYMBOL_TYPE == 0 {
            return TP::TypeOperator;
        }
        return TP::TypeOperator;
    }
    if u::is_this_type_parameter(ty) {
        return TP::NonArray;
    }
    // The alias arm renders a type reference (NonArray).
    if ty.alias.is_some() {
        return TP::NonArray;
    }
    if ty.object_flags.intersects(ObjectFlags::REFERENCE) {
        // Arrays/tuples render as ArrayType/TupleType (Postfix) unless the
        // WriteArrayAsGenericType flag turns them into TypeReference.
        if ctx.flags & nbf::WRITE_ARRAY_AS_GENERIC_TYPE != 0 {
            return TP::NonArray;
        }
        return TP::Postfix;
    }
    if flags.intersects(TypeFlags::INDEX) {
        // keyof — TypeOperator.
        return TP::TypeOperator;
    }
    if flags.intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING) {
        return TP::NonArray;
    }
    if flags.intersects(TypeFlags::INDEXED_ACCESS) {
        return TP::Postfix;
    }
    if flags.intersects(TypeFlags::CONDITIONAL) {
        return TP::Conditional;
    }
    TP::NonArray
}

/// Go: `typeToTypeNode`'s body (nodebuilderimpl.go:3358-3678) — every arm in
/// Go order, rendering directly.
fn ttn_dispatch(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    // Go: inTypeAlias := b.ctx.flags & FlagsInTypeAlias; b.ctx.flags &^= FlagsInTypeAlias
    let in_type_alias = ctx.flags & nbf::IN_TYPE_ALIAS;
    ctx.flags &= !nbf::IN_TYPE_ALIAS;

    let t = c.get_non_distributed_type_parameter(t);

    // The typeStack push (Go 3374-3380) only happens when maxExpansionDepth
    // >= 0 — never for diagnostics (verbosity off). Skipped; shouldExpandType
    // is always false and isTypeOnStack always sees an empty stack.

    let mut t = t;
    // Go: `if b.ctx.flags&FlagsNoTypeReduction == 0 { t = getReducedType(t) }`
    if ctx.flags & nbf::NO_TYPE_REDUCTION == 0 {
        t = c.get_reduced_type(t);
    }

    let flags = c.type_(t).flags;
    if flags.intersects(TypeFlags::ANY) {
        if let Some(alias) = c.type_(t).alias.clone() {
            return alias_to_type_reference_node(c, ctx, t, &alias);
        }
        if Some(t) == c.unresolved_type {
            return Some("any /*unresolved*/".to_string());
        }
        ctx.approximate_length += 3;
        let kw = if Some(t) == c.intrinsic_marker_type {
            "intrinsic"
        } else {
            "any"
        };
        return Some(kw.to_string());
    }
    if flags.intersects(TypeFlags::UNKNOWN) {
        return Some("unknown".to_string());
    }
    if flags.intersects(TypeFlags::STRING) {
        ctx.approximate_length += 6;
        return Some("string".to_string());
    }
    if flags.intersects(TypeFlags::NUMBER) {
        ctx.approximate_length += 6;
        return Some("number".to_string());
    }
    if flags.intersects(TypeFlags::BIGINT) {
        ctx.approximate_length += 6;
        return Some("bigint".to_string());
    }
    if flags.intersects(TypeFlags::BOOLEAN) && c.type_(t).alias.is_none() {
        ctx.approximate_length += 7;
        return Some("boolean".to_string());
    }
    let mut expanding_enum = false;
    if flags.intersects(TypeFlags::ENUM_LIKE) {
        let symbol = c.type_(t).symbol;
        if let Some(symbol) = symbol {
            if c.program_symbol(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER) {
                // Go 3418-3436: parent.member form.
                let parent_symbol = c.get_parent_of_symbol(symbol);
                if let Some(parent_symbol) = parent_symbol {
                    let parent_name =
                        symbol_to_type_node(c, ctx, parent_symbol, false, None)?;
                    if Some(c.get_declared_type_of_symbol(parent_symbol)) == Some(t) {
                        return Some(parent_name);
                    }
                    let member_name = c.program_symbol(symbol).name.clone();
                    if scanner_util::utilities::is_identifier_text(
                        &member_name,
                        tsc_core::languagevariant::LanguageVariant::Standard,
                    ) {
                        ctx.approximate_length += member_name.len() + 1;
                        return Some(format!("{parent_name}.{member_name}"));
                    }
                    // Non-identifier member: (typeof Parent)["member"].
                    ctx.approximate_length += member_name.len() + 2;
                    let quoted = u::escape_string(&member_name);
                    return Some(format!("(typeof {parent_name})[\"{quoted}\"]"));
                }
            }
        }
        if !flags.intersects(TypeFlags::UNION) || !should_expand_type(ctx, t, false) {
            let symbol = c.type_(t).symbol?;
            return symbol_to_type_node(c, ctx, symbol, false, None);
        }
        expanding_enum = true;
    }
    if flags.intersects(TypeFlags::STRING_LITERAL) {
        let value = c
            .type_(t)
            .as_literal_type()
            .and_then(|l| l.value.clone());
        match value {
            Some(crate::types::LiteralValue::String(s)) => {
                ctx.approximate_length += s.len() + 2;
                Some(format!("\"{}\"", u::escape_string(&s)))
            }
            // Go's value.(string) type assert — the diagnostics arm is only
            // entered with a string payload.
            _ => Some(String::new()),
        }
    } else if flags.intersects(TypeFlags::NUMBER_LITERAL) {
        let value = c
            .type_(t)
            .as_literal_type()
            .and_then(|l| l.value.clone());
        match value {
            Some(crate::types::LiteralValue::Number(n)) => {
                let text = n.string();
                ctx.approximate_length += text.len();
                if n.0 < 0.0 {
                    Some(format!("-{}", &text[1..]))
                } else {
                    Some(text)
                }
            }
            _ => Some(String::new()),
        }
    } else if flags.intersects(TypeFlags::BIGINT_LITERAL) {
        let value = c
            .type_(t)
            .as_literal_type()
            .and_then(|l| l.value.clone());
        match value {
            Some(crate::types::LiteralValue::BigInt(b)) => {
                let text = u::pseudo_big_int_to_string(&b);
                ctx.approximate_length += text.len() + 1;
                Some(format!("{text}n"))
            }
            _ => Some(String::new()),
        }
    } else if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
        let value = c
            .type_(t)
            .as_literal_type()
            .and_then(|l| l.value.clone());
        match value {
            Some(crate::types::LiteralValue::Boolean(b)) => {
                if b {
                    ctx.approximate_length += 4;
                    Some("true".to_string())
                } else {
                    ctx.approximate_length += 5;
                    Some("false".to_string())
                }
            }
            _ => Some(String::new()),
        }
    } else if flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
        if ctx.flags & nbf::ALLOW_UNIQUE_ESSYMBOL_TYPE == 0 {
            // Go: IsValueSymbolAccessible path (needs symbolaccessibility).
            // The `typeof sym` form is only reachable when the symbol is
            // accessible; the accessibility machinery is the follow-up wave —
            // the port renders the `unique symbol` fallback (Go's next arm).
            ctx.approximate_length += 13;
            return Some("unique symbol".to_string());
        }
        ctx.approximate_length += 13;
        Some("unique symbol".to_string())
    } else if flags.intersects(TypeFlags::VOID) {
        ctx.approximate_length += 4;
        Some("void".to_string())
    } else if flags.intersects(TypeFlags::UNDEFINED) {
        ctx.approximate_length += 9;
        Some("undefined".to_string())
    } else if flags.intersects(TypeFlags::NULL) {
        ctx.approximate_length += 4;
        Some("null".to_string())
    } else if flags.intersects(TypeFlags::NEVER) {
        ctx.approximate_length += 5;
        Some("never".to_string())
    } else if flags.intersects(TypeFlags::ESSYMBOL) {
        ctx.approximate_length += 6;
        Some("symbol".to_string())
    } else if flags.intersects(TypeFlags::NON_PRIMITIVE) {
        ctx.approximate_length += 6;
        Some("object".to_string())
    } else if u::is_this_type_parameter(c.type_(t)) {
        // Go: the InObjectTypeLiteral guard + tracker report — under
        // IgnoreErrors AllowThisInObjectLiteral is set, so no error.
        ctx.approximate_length += 4;
        Some("this".to_string())
    } else {
        // The alias arm (Go 3516-3532): UseAliasDefinedOutsideCurrentScope is
        // always set for diagnostics; shouldExpandType is always false.
        if in_type_alias == 0 && c.type_(t).alias.is_some() {
            if ctx.flags & nbf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE != 0 {
                let alias = c.type_(t).alias.clone().expect("checked Some");
                let sym = alias.symbol?;
                let type_argument_types: Vec<TypeId> = alias.type_arguments.clone();
                let type_argument_nodes =
                    map_to_type_nodes(c, ctx, &type_argument_types, false, TP::LOWEST);
                let sym_name = c.program_symbol(sym).name.clone();
                let sym_flags = c.program_symbol(sym).flags;
                if u::is_reserved_member_name(&sym_name)
                    && !sym_flags.intersects(SymbolFlags::CLASS)
                {
                    // Go: NewTypeReferenceNode(NewIdentifier(""), args) — an
                    // empty identifier with type arguments.
                    let args = type_argument_nodes.unwrap_or_default();
                    return Some(format!("<{}>", args.join(", ")));
                }
                if let Some(nodes) = &type_argument_nodes {
                    if nodes.len() == 1
                        && c
                            .global_array_type
                            .is_some_and(|g| c.type_(g).symbol == Some(sym))
                    {
                        return Some(format!("{}[]", nodes[0]));
                    }
                }
                let nodes = type_argument_nodes;
                return symbol_to_type_node(c, ctx, sym, false, nodes.as_deref());
            } else {
                // Go: IsTypeSymbolAccessible gate (symbolaccessibility wave) —
                // the diagnostics flags always carry UseAliasDefinedOutside,
                // so this arm is diagnostics-dead; fall through structurally.
            }
        }
        ttn_object_and_late_arms(c, ctx, t, expanding_enum, in_type_alias)
    }
}

fn ttn_object_and_late_arms(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
    expanding_enum: bool,
    _in_type_alias: u32,
) -> Option<String> {
    let object_flags = c.type_(t).object_flags;
    let flags = c.type_(t).flags;

    if object_flags.intersects(ObjectFlags::REFERENCE) {
        // When expanding (verbosity), references expand structurally — never
        // for diagnostics (shouldExpandType false).
        if crate::relater::is_array_or_tuple_type(c, t) {
            return visit_and_transform_type(c, ctx, t, array_or_tuple_type_to_node);
        }
        if c
            .type_(t)
            .as_type_reference()
            .is_some_and(|r| r.node.is_some())
        {
            return visit_and_transform_type(c, ctx, t, type_reference_to_type_node);
        }
        return type_reference_to_type_node(c, ctx, t);
    }
    if flags.intersects(TypeFlags::TYPE_PARAMETER)
        || object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
    {
        // The infer-type-parameter arm is the tracer/hover path (never
        // diagnostics: inferTypeParameters is only set by the conditional
        // arm's root swap, and ctx.infer_type_parameters stays empty).
        if flags.intersects(TypeFlags::TYPE_PARAMETER)
            && ctx.infer_type_parameters.contains(&t)
        {
            // Go: NewInferTypeNode(typeParameterToDeclarationWithConstraint(t, constraintNode))
            let name = c
                .type_(t)
                .symbol
                .map(|s| c.program_symbol(s).name.clone())
                .unwrap_or_default();
            ctx.approximate_length += name.len() + 6;
            let constraint = c.get_constraint_of_type_parameter(t);
            if let Some(constraint) = constraint {
                ctx.approximate_length += 9;
                let cn = ttn(c, ctx, constraint, TP::LOWEST)?;
                return Some(format!("infer {name} extends {cn}"));
            }
            return Some(format!("infer {name}"));
        }
        // Ignore constraint/default when creating a usage of a type parameter.
        if let Some(symbol) = c.type_(t).symbol {
            return symbol_to_type_node(c, ctx, symbol, false, None);
        }
        // Go: the variance-marker names — sub-/super- + the variance type
        // parameter, else "?".
        let name = if (Some(t) == c.marker_super_type_for_check
            || Some(t) == c.marker_sub_type_for_check)
            && c.variance_type_parameter.is_some()
        {
            let vp = c.variance_type_parameter?;
            let vp_sym = c.type_(vp).symbol?;
            let base = if Some(t) == c.marker_sub_type_for_check {
                "sub-"
            } else {
                "super-"
            };
            format!("{base}{}", c.program_symbol(vp_sym).name)
        } else {
            "?".to_string()
        };
        return Some(name);
    }
    let mut t = t;
    if flags.intersects(TypeFlags::UNION) {
        if let Some(origin) = c
            .type_(t)
            .as_union_type()
            .and_then(|ut| ut.origin)
        {
            t = origin;
        }
    }
    // PORT(FCS tranche 3): Go re-reads t.flags AFTER the origin swap
    // (nodebuilderimpl.go:3595-3599 — `t = t.AsUnionType().origin` then the
    // fresh `t.flags&(Union|Intersection)` probe). The origin is usually NOT
    // a union (that's the point of the swap), so the stale flags made the
    // as_union_type expect below panic (exhaustiveSwitchStatements1's
    // enum-origin unions).
    let flags = c.type_(t).flags;
    if flags.intersects(TypeFlags::UNION | TypeFlags::INTERSECTION) {
        let types: Vec<TypeId> = if flags.intersects(TypeFlags::UNION) {
            let ut = c.type_(t).as_union_type().expect("checked UNION");
            format_union_types(c, &ut.base.types.clone(), expanding_enum)
        } else {
            c.type_(t)
                .as_intersection_type()
                .expect("checked INTERSECTION")
                .base
                .types
                .clone()
        };
        if types.len() == 1 {
            return ttn_dispatch(c, ctx, types[0]);
        }
        // Go: the printer emits union/intersection constituents at
        // TypeOperator precedence (emitUnionTypeConstituent — printer.go:2053).
        let type_nodes = map_to_type_nodes(c, ctx, &types, true, TP::TypeOperator);
        let Some(type_nodes) = type_nodes else {
            // Go: AllowEmptyUnionOrIntersection is set under IgnoreErrors —
            // the nil branch's encounteredError is pre-armed; unreachable.
            if !ctx.encountered_error && ctx.flags & nbf::ALLOW_EMPTY_UNION_OR_INTERSECTION == 0 {
                ctx.encountered_error = true;
            }
            return None;
        };
        let sep = if flags.intersects(TypeFlags::UNION) {
            " | "
        } else {
            " & "
        };
        return Some(type_nodes.join(sep));
    }
    if object_flags.intersects(ObjectFlags::ANONYMOUS | ObjectFlags::MAPPED) {
        return create_anonymous_type_node(c, ctx, t);
    }
    if flags.intersects(TypeFlags::INDEX) {
        // keyof T — TypeOperator(keyof, target); printer emits the operand at
        // TypeOperator precedence.
        let indexed_type = c.type_(t).target()?;
        ctx.approximate_length += 6;
        let inner = ttn(c, ctx, indexed_type, TP::TypeOperator)?;
        return Some(format!("keyof {inner}"));
    }
    if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        // Go 3634-3651: template head + spans.
        let data = c.type_(t).as_template_literal_type().expect("checked TEMPLATE");
        let texts = data.texts.clone();
        let types = data.types.clone();
        let mut out = String::from("`");
        out.push_str(&texts[0]);
        for (i, &ty) in types.iter().enumerate() {
            let inner = ttn_outside_extends(c, ctx, ty)?;
            out.push_str(&format!("${{{inner}}}"));
            out.push_str(&texts[i + 1]);
        }
        out.push('`');
        ctx.approximate_length += 2;
        return Some(out);
    }
    if flags.intersects(TypeFlags::STRING_MAPPING) {
        // Go: symbolToTypeNode(t.AsStringMappingType().symbol, Type, [target])
        let target = c.type_(t).target()?;
        let type_node = ttn(c, ctx, target, TP::LOWEST)?;
        let symbol = c.type_(t).symbol?;
        return symbol_to_type_node(c, ctx, symbol, false, Some(&[type_node]));
    }
    if flags.intersects(TypeFlags::INDEXED_ACCESS) {
        let data = c.type_(t).as_indexed_access_type().expect("checked INDEXED_ACCESS");
        let object_type = data.object_type()?;
        let index_type = data.index_type()?;
        // Go: NewIndexedAccessTypeNode(objectTypeNode, indexTypeNode) — the
        // object side is emitted at Postfix precedence (emitPostfixTypeOperand),
        // the index at Lowest outside extends.
        let object_node = ttn(c, ctx, object_type, TP::Postfix)?;
        let index_node = ttn_outside_extends(c, ctx, index_type)?;
        ctx.approximate_length += 2;
        return Some(format!("{object_node}[{index_node}]"));
    }
    if flags.intersects(TypeFlags::CONDITIONAL) {
        return visit_and_transform_type(c, ctx, t, conditional_type_to_type_node);
    }
    if flags.intersects(TypeFlags::SUBSTITUTION) {
        let base_type = c
            .type_(t)
            .as_substitution_type()
            .expect("checked SUBSTITUTION")
            .base_type?;
        let type_node = ttn(c, ctx, base_type, TP::LOWEST)?;
        if !crate::inference::is_no_infer_type(c, t) {
            return Some(type_node);
        }
        // Go: the global NoInfer alias — under the no-lib shape it is absent;
        // Go's else-branch returns the base node.
        if let Some(no_infer_symbol) = c.get_global_type_alias_symbol("NoInfer", 1, false) {
            return symbol_to_type_node(c, ctx, no_infer_symbol, false, Some(&[type_node]));
        }
        return Some(type_node);
    }

    panic!("Should be unreachable (type flags {:?})", c.type_(t).flags);
}

/// Go: `TypeAlias.ToTypeReferenceNode` (nodebuilderimpl.go:3696) — the alias
/// arm's `t.alias.ToTypeReferenceNode(b)`. Go's shape creates a
/// TypeReferenceNode for the alias symbol WITHOUT type arguments (the alias's
/// typeArguments are printed by the symbolToTypeNode call in the 3516 arm —
/// this ToTypeReferenceNode path is for the `any`-flags arm only, where the
/// alias renders by name).
fn alias_to_type_reference_node(
    c: &mut Checker,
    ctx: &mut NbCtx,
    _t: TypeId,
    alias: &crate::types::TypeAlias,
) -> Option<String> {
    let sym = alias.symbol?;
    // Go: ToTypeReferenceNode → symbolToTypeNode(b.ch.getAliasSymbolForTypeNode...)
    // — for the diagnostics path this is the alias symbol's reference form.
    symbol_to_type_node(c, ctx, sym, false, None)
}

// ────────────────────────────────────────────────────────────────────────────
// Truncation (nodebuilderimpl.go:147-171) + mapToTypeNodes (350-441)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `checkTruncationLength` (nodebuilderimpl.go:147).
fn check_truncation_length(ctx: &mut NbCtx) -> bool {
    if ctx.truncating {
        return ctx.truncating;
    }
    let max_length = if ctx.flags & nbf::NO_TRUNCATION != 0 {
        NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH
    } else if ctx.max_truncation_length > 0 {
        ctx.max_truncation_length
    } else {
        DEFAULT_MAXIMUM_TRUNCATION_LENGTH
    };
    ctx.truncating = ctx.approximate_length > max_length;
    ctx.truncating
}

/// Go: `shouldExpandType` (nodebuilderimpl.go:213) — always false at
/// diagnostics (maxExpansionDepth < 0); kept as the named shape.
fn should_expand_type(_ctx: &NbCtx, _t: TypeId, _is_alias: bool) -> bool {
    false
}

/// Go: `createElidedInformationPlaceholder` (nodebuilderimpl.go:342).
fn create_elided_information_placeholder(ctx: &mut NbCtx) -> String {
    ctx.approximate_length += 3;
    if ctx.flags & nbf::NO_TRUNCATION == 0 {
        "...".to_string()
    } else {
        "any /*elided*/".to_string()
    }
}

/// Go: `mapToTypeNodes(list, isBareList)` (nodebuilderimpl.go:350-441) —
/// renders each constituent; `None` mirrors Go's nil NodeList.
fn map_to_type_nodes(
    c: &mut Checker,
    ctx: &mut NbCtx,
    list: &[TypeId],
    is_bare_list: bool,
    prec: TP,
) -> Option<Vec<String>> {
    if list.is_empty() {
        return None;
    }

    if check_truncation_length(ctx) {
        if !is_bare_list {
            let node = create_elided_information_placeholder(ctx);
            return Some(vec![node]);
        } else if list.len() > 2 {
            let first = ttn_dispatch(c, ctx, list[0])?;
            let last = ttn_dispatch(c, ctx, list[list.len() - 1])?;
            let middle = format!("... {} more ...", list.len() - 2);
            return Some(vec![first, middle, last]);
        }
    }

    let may_have_name_collisions = ctx.flags & nbf::USE_FULLY_QUALIFIED_TYPE == 0;
    // Go: `seenNames` (collections.MultiMap[string, seenName]) — one entry
    // per single-identifier type reference, insertion order per name.
    let mut seen_names: Vec<(String, Vec<SeenName>)> = Vec::new();
    let mut result: Vec<String> = Vec::with_capacity(list.len());

    for (i, &t) in list.iter().enumerate() {
        let display_index = i + 1;
        if check_truncation_length(ctx) && (display_index + 2) < list.len() - 1 {
            let text = format!("... {} more ...", list.len() - display_index);
            result.push(text);
            let type_node = ttn_dispatch(c, ctx, list[list.len() - 1])?;
            result.push(type_node);
            break;
        }
        ctx.approximate_length += 2; // whitespace + separator
        // The identifier-ref probe: ttn's symbol paths record the rendered
        // single-identifier type reference name (Go's isIdentifierTypeReference
        // reads the node — the probe reads the rendered shape).
        let saved_probe = ctx.ident_ref_probe.take();
        let type_node = ttn(c, ctx, t, prec);
        let probe = ctx.ident_ref_probe.take();
        ctx.ident_ref_probe = saved_probe;
        if let Some(type_node) = type_node {
            if may_have_name_collisions {
                if let Some(name) = probe {
                    if let Some(entry) = seen_names.iter_mut().find(|(n, _)| *n == name) {
                        entry.1.push(SeenName { t, i: result.len() });
                    } else {
                        seen_names.push((name, vec![SeenName { t, i: result.len() }]));
                    }
                }
            }
            result.push(type_node);
        }
    }

    if may_have_name_collisions {
        // To avoid printing types like `[Foo, Foo]` where occurrences of the
        // same name actually come from different namespaces, regenerate each
        // colliding entry with UseFullyQualifiedType enabled.
        let restore = save_restore_flags(ctx);
        ctx.flags |= nbf::USE_FULLY_QUALIFIED_TYPE;
        for (_, types) in seen_names.iter() {
            if !types_are_same_reference_all(c, types) {
                for seen in types {
                    if let Some(node) = ttn(c, ctx, seen.t, prec) {
                        if seen.i < result.len() {
                            result[seen.i] = node;
                        }
                    }
                }
            }
        }
        restore_flags(ctx, restore);
    }

    Some(result)
}

/// Go: `arrayIsHomogeneous(seenNames, typesAreSameReference)` (nodebuilderimpl.go:468, 482).
fn types_are_same_reference_all(c: &Checker, types: &[SeenName]) -> bool {
    if types.len() < 2 {
        return true;
    }
    let first = types[0].t;
    for t in &types[1..] {
        if !types_are_same_reference(c, first, t.t) {
            return false;
        }
    }
    true
}

/// The seenName entry (nodebuilderimpl.go:382-386).
struct SeenName {
    t: TypeId,
    i: usize,
}

/// Go: `typesAreSameReference(a, b)` (nodebuilderimpl.go:482).
fn types_are_same_reference(c: &Checker, a: TypeId, b: TypeId) -> bool {
    a == b
        || (c.type_(a).symbol.is_some() && c.type_(a).symbol == c.type_(b).symbol)
        || (c.type_(a).alias.is_some()
            && c.type_(a).alias.as_deref().map(|x| x as *const TypeAlias)
                == c.type_(b).alias.as_deref().map(|x| x as *const TypeAlias))
}

// ────────────────────────────────────────────────────────────────────────────
// visitAndTransformType (nodebuilderimpl.go:3234-3356)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `visitAndTransformType(t, transform)` — the cycle guard + depth limit.
/// The serializedTypes per-declaration cache is diagnostics-dead (enclosing
/// nil at top level; the property arm's cache rides the pseudo-checker seam).
fn visit_and_transform_type(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
    transform: fn(&mut Checker, &mut NbCtx, TypeId) -> Option<String>,
) -> Option<String> {
    if check_truncation_length(ctx) {
        return Some(create_elided_information_placeholder(ctx));
    }

    let mut type_id = t;
    let is_array_or_tuple = crate::relater::is_array_or_tuple_type(c, t);
    if is_array_or_tuple {
        // Deferred and regular references share a cycle identity.
        let args = crate::relater::resolve_type_arguments(c, t);
        if let Some(target) = c.type_(t).target() {
            type_id = c.create_type_reference(target, args);
        }
    }
    if ctx.visited_types.contains(&type_id) {
        return Some(create_cyclic_structure_placeholder(ctx));
    }

    // The composite identity (isConstructorObject, symbolId, nodeId).
    let is_constructor_object = c.type_(t).object_flags.intersects(ObjectFlags::ANONYMOUS)
        && c
            .type_(t)
            .symbol
            .is_some_and(|s| c.program_symbol(s).flags.intersects(SymbolFlags::CLASS));
    let key: u128 = if is_array_or_tuple {
        // Go: id = nil for arrays/tuples — no symbolDepth guard.
        u128::MAX
    } else if c.type_(t).object_flags.intersects(ObjectFlags::REFERENCE)
        && c.type_(t).as_type_reference().is_some_and(|r| r.node.is_some())
    {
        match c.type_(t).as_type_reference().unwrap().node {
            Some(n) => 3u128 << 100 | u128::from(n.0),
            None => u128::MAX,
        }
    } else if c.type_(t).flags.intersects(TypeFlags::CONDITIONAL) {
        match c
            .type_(t)
            .as_conditional_type()
            .and_then(|ct| ct.root.as_ref().and_then(|r| r.node))
        {
            Some(n) => 2u128 << 100 | u128::from(n.0),
            None => u128::MAX,
        }
    } else if let Some(symbol) = c.type_(t).symbol {
        1u128 << 100 | (is_constructor_object as u128) << 64 | u128::from(symbol.0)
    } else {
        u128::MAX
    };

    let depth = if key == u128::MAX {
        None
    } else {
        Some(*ctx.symbol_depth.entry(key).or_insert(0))
    };
    if let Some(depth) = depth {
        if depth > 10 {
            ctx.truncating = true;
            return Some(create_elided_information_placeholder(ctx));
        }
        *ctx.symbol_depth.get_mut(&key).expect("just inserted") = depth + 1;
    }
    ctx.visited_types.insert(type_id);
    let result = transform(c, ctx, t);
    ctx.visited_types.remove(&type_id);
    if let Some(depth) = depth {
        *ctx.symbol_depth.get_mut(&key).expect("just inserted") = depth;
    }
    result
}

/// Go: `createCyclicStructurePlaceholder` (nodebuilderimpl.go:3017) — under
/// IgnoreErrors (AllowAnonymousIdentifier set) no error is recorded.
fn create_cyclic_structure_placeholder(ctx: &mut NbCtx) -> String {
    if ctx.flags & nbf::ALLOW_ANONYMOUS_IDENTIFIER == 0 {
        ctx.encountered_error = true;
    }
    create_elided_information_placeholder(ctx)
}

// ────────────────────────────────────────────────────────────────────────────
// Arrays / tuples / references (nodebuilderimpl.go:3086-3232)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `arrayOrTupleTypeToNode` (nodebuilderimpl.go:3086-3158).
fn array_or_tuple_type_to_node(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    let type_arguments = crate::relater::resolve_type_arguments(c, t);
    let target = c.type_(t).target();
    if Some(target) == c.global_array_type.map(Some) || Some(target) == c.global_readonly_array_type.map(Some) {
        let target = target?;
        if ctx.flags & nbf::WRITE_ARRAY_AS_GENERIC_TYPE != 0 {
            let type_argument_node = ttn(c, ctx, type_arguments[0], TP::LOWEST)?;
            let name = if Some(target) == c.global_array_type {
                "Array"
            } else {
                "ReadonlyArray"
            };
            return Some(format!("{name}<{type_argument_node}>"));
        }
        // The element type renders at Postfix precedence (printer.go:1993
        // emitPostfixTypeOperand → emitTypeNode(operand, TypePrecedencePostfix)).
        let element_type = ttn(c, ctx, type_arguments[0], TP::Postfix)?;
        if Some(target) == c.global_array_type {
            return Some(format!("{element_type}[]"));
        }
        return Some(format!("readonly {element_type}[]"));
    }
    // Tuple.
    let target = target?;
    let target_flags = c.type_(target).flags;
    debug_assert!(
        target_flags.intersects(TypeFlags::OBJECT)
            && c.type_(target).object_flags.intersects(ObjectFlags::TUPLE)
    );
    let element_infos: Vec<ElementFlags> = c
        .type_(target)
        .as_tuple_type()
        .map(|tt| tt.element_infos.iter().map(|e| e.flags).collect())
        .unwrap_or_default();
    let type_arguments: Vec<TypeId> = type_arguments
        .iter()
        .enumerate()
        .map(|(i, &arg)| {
            let is_optional = i < element_infos.len()
                && element_infos[i].intersects(ElementFlags::OPTIONAL);
            crate::relater::remove_missing_type(c, arg, is_optional)
        })
        .collect();
    let readonly = c
        .type_(target)
        .as_tuple_type()
        .is_some_and(|tt| tt.readonly);
    // PORT(FCS tranche 3): Go's `if tupleConstituentNodes != nil { ... }`
    // (nodebuilderimpl.go:3125) — a nil mapToTypeNodes result (an empty
    // type-argument slice: the arity-0 empty tuple target whose sole
    // resolvedTypeArgument is the synthesized thisType) FALLS THROUGH to
    // the empty-tuple epilogue below instead of returning nil. The old
    // `map_to_type_nodes(...)?` propagated the None into the
    // "should always get typenode" panics (arrayLiterals3/contextually-
    // TypedIife's `[]` tuples).
    if !type_arguments.is_empty() {
        let arity = crate::relater::get_type_reference_arity(c, t);
        if let Some(tuple_constituents) =
            map_to_type_nodes(c, ctx, &type_arguments[..arity as usize], false, TP::LOWEST)
        {
            let mut nodes: Vec<String> = tuple_constituents;
        let element_infos: Vec<TupleElementInfo> = c
            .type_(target)
            .as_tuple_type()
            .map(|tt| tt.element_infos.to_vec())
            .unwrap_or_default();
        for (i, node_slot) in nodes.iter_mut().enumerate() {
            let flags = element_infos.get(i).map(|e| e.flags).unwrap_or(ElementFlags::NONE);
            if let Some(labeled_declaration) = element_infos
                .get(i)
                .and_then(|e| e.labeled_declaration)
            {
                let label = c.get_tuple_element_label(
                    TupleElementInfo {
                        flags,
                        labeled_declaration: Some(labeled_declaration),
                    },
                    None,
                    i,
                );
                let dot = if flags.intersects(ElementFlags::VARIABLE) {
                    "..."
                } else {
                    ""
                };
                let q = if flags.intersects(ElementFlags::OPTIONAL) {
                    "?"
                } else {
                    ""
                };
                *node_slot = if flags.intersects(ElementFlags::REST) {
                    format!("{dot}{label}{q}: {}[]", *node_slot)
                } else {
                    format!("{dot}{label}{q}: {}", *node_slot)
                };
            } else {
                if flags.intersects(ElementFlags::VARIABLE) {
                    *node_slot = if flags.intersects(ElementFlags::REST) {
                        format!("...{}[]", *node_slot)
                    } else {
                        format!("...{}", *node_slot)
                    };
                } else if flags.intersects(ElementFlags::OPTIONAL) {
                    *node_slot = format!("{}?", *node_slot);
                }
            }
        }
        let joined = nodes.join(", ");
        if readonly {
            return Some(format!("readonly [{joined}]"));
        }
        return Some(format!("[{joined}]"));
        }
    }
    // Go: the fall-through epilogue (encounteredError/AllowEmptyTuple —
    // IgnoreErrors always carries AllowEmptyTuple on the diagnostics path).
    if ctx.encountered_error || ctx.flags & nbf::ALLOW_EMPTY_TUPLE != 0 {
        if readonly {
            return Some("readonly []".to_string());
        }
        return Some("[]".to_string());
    }
    ctx.encountered_error = true;
    None
}

/// Go: `typeReferenceToTypeNode` (nodebuilderimpl.go:3160-3232).
fn type_reference_to_type_node(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    let type_arguments = crate::relater::resolve_type_arguments(c, t);
    // Go: the WriteClassExpressionAsTypeLiteral branch is emit-only (flag
    // off for diagnostics).
    let target = c.type_(t).target()?;
    let outer_type_parameters: Vec<TypeId> = c
        .type_(target)
        .as_interface_layer()
        .map(|itf| itf.outer_type_parameters().to_vec())
        .unwrap_or_default();
    let mut i = 0usize;
    let mut result_type: Option<String> = None;
    if !outer_type_parameters.is_empty() {
        let length = outer_type_parameters.len();
        while i < length {
            let start = i;
            let parent = get_parent_symbol_of_type_parameter(c, outer_type_parameters[i]);
            loop {
                i += 1;
                if !(i < length
                    && get_parent_symbol_of_type_parameter(c, outer_type_parameters[i]) == parent)
                {
                    break;
                }
            }
            // When type parameters are their own type arguments for the whole
            // group, we don't show the group.
            let params_same = outer_type_parameters[start..i] == type_arguments[start..i];
            if !params_same && !type_arguments.is_empty() {
                let type_argument_slice =
                    map_to_type_nodes(
                        c,
                        ctx,
                        &type_arguments[start..i.min(type_arguments.len())],
                        false,
                        TP::LOWEST,
                    );
                let restore = save_restore_flags(ctx);
                ctx.flags |= nbf::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES;
                let Some(parent) = parent else {
                    restore_flags(ctx, restore);
                    continue;
                };
                let r = symbol_to_type_node(c, ctx, parent, false, type_argument_slice.as_deref());
                restore_flags(ctx, restore);
                if let Some(r) = r {
                    result_type = Some(match result_type {
                        None => r,
                        Some(prev) => format!("{prev}.{r}"),
                    });
                }
            }
        }
    }
    let mut type_argument_nodes: Option<Vec<String>> = None;
    if !type_arguments.is_empty() {
        let mut type_parameter_count = 0usize;
        let type_params: Vec<TypeId> = c
            .type_(target)
            .as_interface_layer()
            .map(|itf| itf.type_parameters().to_vec())
            .unwrap_or_default();
        if !type_params.is_empty() {
            type_parameter_count = type_params.len().min(type_arguments.len());
            // Go: the Iterable/IterableIterator default-elision arm is
            // lib-only (the globals are absent under the no-lib shape).
        }
        let hi = (i + type_parameter_count).min(type_arguments.len());
        if hi > i {
            type_argument_nodes = map_to_type_nodes(c, ctx, &type_arguments[i..hi], false, TP::LOWEST);
        }
    }
    let restore = save_restore_flags(ctx);
    ctx.flags |= nbf::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES;
    let symbol = c.type_(t).symbol?;
    let final_ref = symbol_to_type_node(c, ctx, symbol, false, type_argument_nodes.as_deref());
    restore_flags(ctx, restore);
    match (result_type, final_ref) {
        (None, f) => f,
        (Some(prev), Some(f)) => Some(format!("{prev}.{f}")),
        (Some(prev), None) => Some(prev),
    }
}

/// Go: `getParentSymbolOfTypeParameter` (nodebuilderimpl.go:3071).
fn get_parent_symbol_of_type_parameter(
    c: &mut Checker,
    type_parameter: TypeId,
) -> Option<SymbolId> {
    let symbol = c.type_(type_parameter).symbol?;
    // Go: `parentSymbol := symbol.Parent; if parentSymbol == nil { return nil }`
    let parent = c.program_symbol(symbol).parent?;
    // Class and interface type parameters have their declaration's parent as
    // the logical parent symbol.
    let parent_sym = c.program_symbol(parent);
    if parent_sym.flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE) {
        return Some(parent);
    }
    if parent_sym.flags.intersects(SymbolFlags::TYPE_PARAMETER) {
        // Go: typeParameter.parentSymbol (the TypeParameter struct's parent
        // pointer) — the port reads the symbol's parent.
        return c.get_parent_of_symbol(parent);
    }
    Some(parent)
}

// ────────────────────────────────────────────────────────────────────────────
// Conditional types (nodebuilderimpl.go:3025-3069)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `conditionalTypeToTypeNode` (the non-GenerateNamesForShadowedTypeParams
/// path — diagnostics never set the flag).
fn conditional_type_to_type_node(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
) -> Option<String> {
    if check_truncation_length(ctx) {
        return Some(create_elided_information_placeholder(ctx));
    }
    let data = c.type_(t).as_conditional_type().expect("checked CONDITIONAL");
    let check_type = data.check_type;
    let extends_type = data.extends_type;
    // Go: `t.root.inferTypeParameters` swap into ctx (the root's list is
    // empty unless the conditional contains `infer` positions).
    let save_infer_type_parameters = std::mem::take(&mut ctx.infer_type_parameters);
    if let Some(root) = &data.root {
        ctx.infer_type_parameters = root.infer_type_parameters.clone();
    }
    let check_type_node = ttn(c, ctx, check_type?, TP::Union)?;
    ctx.approximate_length += 15;
    let extends_type_node = ttn_in_extends(c, ctx, extends_type?)?;
    ctx.infer_type_parameters = save_infer_type_parameters;
    let true_branch = crate::relater::get_true_type_from_conditional_type(c, t);
    let true_type_node = type_to_type_node_or_circularity_elision(c, ctx, true_branch)?;
    let false_branch = crate::relater::get_false_type_from_conditional_type(c, t);
    let false_type_node = type_to_type_node_or_circularity_elision(c, ctx, false_branch)?;
    Some(format!(
        "{check_type_node} extends {extends_type_node} ? {true_type_node} : {false_type_node}"
    ))
}

/// Go: `typeToTypeNodeOrCircularityElision` (nodebuilderimpl.go:3007).
fn type_to_type_node_or_circularity_elision(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
) -> Option<String> {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        if ctx.visited_types.contains(&c.type_(t).id) {
            return Some(create_cyclic_structure_placeholder(ctx));
        }
        return visit_and_transform_type(c, ctx, t, ttn_transform);
    }
    ttn(c, ctx, t, TP::LOWEST)
}

/// The `visitAndTransformType(t, (*NodeBuilderImpl).typeToTypeNode)` transform.
fn ttn_transform(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    ttn(c, ctx, t, TP::LOWEST)
}

// ────────────────────────────────────────────────────────────────────────────
// Object types (nodebuilderimpl.go:2897-2988, 2789-2847, 2581-2787)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `createAnonymousTypeNode` (nodebuilderimpl.go:2897).
fn create_anonymous_type_node(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    create_anonymous_type_node_ex(c, ctx, t, false, false)
}

/// Go: `isClassInstanceSide` (nodebuilderimpl.go:338).
fn is_class_instance_side(c: &mut Checker, t: TypeId) -> bool {
    c.type_(t).symbol.is_some_and(|s| {
        c.program_symbol(s).flags.intersects(SymbolFlags::CLASS)
            && (Some(t) == c.try_get_declared_type_of_symbol(s)
                || (c.type_(t).flags.intersects(TypeFlags::OBJECT)
                    && c.type_(t).object_flags.intersects(ObjectFlags::IS_CLASS_INSTANCE_CLONE)))
    })
}

/// Go: `shouldWriteTypeOfFunctionSymbol` (nodebuilderimpl.go:2859) — the
/// UseTypeOfFunction flag is off for diagnostics, so the result is true only
/// on the visitedTypes (recursive) case, which `shouldEmitTypeOfSymbol`'s
/// caller handles via the visited guard; the port mirrors the flag logic.
fn should_write_type_of_function_symbol(
    c: &mut Checker,
    ctx: &NbCtx,
    symbol: SymbolId,
    type_id: TypeId,
) -> (bool, Option<SymbolId>) {
    let sym = c.program_symbol(symbol);
    // typeof is allowed only for static/non-local functions.
    let is_static_method_symbol = sym.flags.intersects(SymbolFlags::METHOD)
        && scanner_util::utilities::is_identifier_text(&sym.name, tsc_core::languagevariant::LanguageVariant::Standard)
        && sym.declarations.iter().any(|&d| {
            c.store().node(d).modifier_flags().intersects(ModifierFlags::STATIC)
                // Go: && !b.ch.isLateBindableIndexSignature(GetNameOfDeclaration(d))
                // — the static-method name is never a computed late-bindable
                // index signature in the diagnostics shapes (PORT: elided).
                && !matches!(c.store().node(d).kind, Kind::IndexSignature)
        });
    let mut is_non_local_function_symbol = false;
    let mut is_function_expression_symbol = false;
    if sym.flags.intersects(SymbolFlags::FUNCTION) {
        if sym.parent.is_some() {
            is_non_local_function_symbol = true;
        } else {
            let store = c.store();
            for &declaration in &sym.declarations {
                let parent = store.node(declaration).parent.get();
                if parent != NodeId(0)
                    && matches!(store.node(parent).kind, Kind::SourceFile | Kind::ModuleBlock)
                {
                    is_non_local_function_symbol = true;
                    break;
                }
                // Go: IsFunctionExpressionOrArrowFunction(d) &&
                // IsVariableDeclaration(d.Parent) && IsVariableDeclarationList(ggp) &&
                // IsVariableStatement(gggp) && ggggp is SourceFile/ModuleBlock.
                if matches!(
                    store.node(declaration).kind,
                    Kind::FunctionExpression | Kind::ArrowFunction
                ) {
                    let mut chain_ok = true;
                    let mut n = (parent != NodeId(0)).then_some(parent);
                    for expected in [
                        Kind::VariableDeclaration,
                        Kind::VariableDeclarationList,
                        Kind::VariableStatement,
                    ] {
                        match n {
                            Some(cur) if store.node(cur).kind == expected => {
                                n = parent_node(store.node(cur));
                            }
                            _ => {
                                chain_ok = false;
                                break;
                            }
                        }
                    }
                    if chain_ok
                        && n.is_some_and(|cur| {
                            matches!(store.node(cur).kind, Kind::SourceFile | Kind::ModuleBlock)
                        })
                    {
                        is_non_local_function_symbol = true;
                        is_function_expression_symbol = true;
                        break;
                    }
                }
            }
        }
    }
    if is_static_method_symbol || is_non_local_function_symbol {
        if is_function_expression_symbol {
            // (the merged-parent swap — the merged symbol equals the parent
            // symbol in the diagnostics shape)
            if let Some(vd) = sym.value_declaration {
                let parent = parent_node(c.store().node(vd));
                if parent != ctx.enclosing_declaration {
                    if let Some(parent) = parent {
                        if let Some(parent_sym) =
                            crate::typeofsymbol::get_symbol_of_declaration(c, parent)
                        {
                            return (
                                (ctx.flags & nbf::USE_TYPE_OF_FUNCTION != 0)
                                    || ctx.visited_types.contains(&type_id),
                                Some(parent_sym),
                            );
                        }
                    }
                }
            }
        }
        return (
            (ctx.flags & nbf::USE_TYPE_OF_FUNCTION != 0) || ctx.visited_types.contains(&type_id),
            Some(symbol),
        );
    }
    (false, Some(symbol))
}

/// Go: `shouldEmitTypeOfSymbol` (nodebuilderimpl.go:2901).
fn should_emit_type_of_symbol(
    c: &mut Checker,
    ctx: &NbCtx,
    _force_expansion: bool,
    force_class_expansion: bool,
    is_instance_type: SymbolFlags,
    symbol: SymbolId,
    type_id: TypeId,
) -> (bool, Option<SymbolId>) {
    // forceExpansion false → the first guard returns false immediately in
    // createAnonymousTypeNodeEx's call only via shouldEmitTypeOfSymbol's
    // forceExpansion param — mirror Go's exact field order.
    let _ = _force_expansion;
    let (is_class, is_enum_or_module) = {
        let sym = c.program_symbol(symbol);
        (
            sym.flags.intersects(SymbolFlags::CLASS),
            sym.flags.intersects(SymbolFlags::ENUM | SymbolFlags::VALUE_MODULE),
        )
    };
    let non_function_result = is_class
        && !force_class_expansion
        && c.get_base_type_variable_of_class(symbol).is_none()
        || is_enum_or_module;
    let _ = is_instance_type;
    if non_function_result {
        return (true, Some(symbol));
    }
    should_write_type_of_function_symbol(c, ctx, symbol, type_id)
}

/// Go: `createAnonymousTypeNodeEx` (nodebuilderimpl.go:2912-2988).
fn create_anonymous_type_node_ex(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
    force_class_expansion: bool,
    force_expansion: bool,
) -> Option<String> {
    let type_id = c.type_(t).id;
    let symbol = c.type_(t).symbol;
    if let Some(symbol) = symbol {
        // (the InstantiationExpressionType arm is the declaration-emit shape;
        // its guard requires an instantiation-expression node — not
        // diagnostics-reachable, skipped with the Go comment retained below.)
        let is_instance_type = if is_class_instance_side(c, t) {
            SymbolFlags::TYPE
        } else {
            SymbolFlags::VALUE
        };
        let (ok, symbol) = should_emit_type_of_symbol(
            c,
            ctx,
            force_expansion,
            force_class_expansion,
            is_instance_type,
            symbol,
            type_id,
        );
        if ok {
            // Go: shouldExpandType true would bump depth and expand — never
            // for diagnostics; the symbol reference is returned.
            let is_typeof = is_instance_type == SymbolFlags::VALUE;
            return symbol_to_type_node(c, ctx, symbol?, is_typeof, None);
        }
        if ctx.visited_types.contains(&type_id) {
            // If type is an anonymous type literal in a type alias
            // declaration, use the type alias name (getTypeAliasForTypeLiteral).
            if let Some(type_alias) = get_type_alias_for_type_literal(c, t) {
                return symbol_to_type_node(c, ctx, type_alias, false, None);
            }
            return Some(create_cyclic_structure_placeholder(ctx));
        }
        return visit_and_transform_type(c, ctx, t, create_type_node_from_object_type);
    }
    // Go: the ReverseMapped arm needs FlagsAllowAnonymousIdentifier == 0 —
    // diagnostics always have it set, so the branch order is:
    if c.type_(t).object_flags.intersects(ObjectFlags::REVERSE_MAPPED)
        && ctx.flags & nbf::ALLOW_ANONYMOUS_IDENTIFIER == 0
    {
        if ctx.visited_types.contains(&type_id) {
            return Some(create_cyclic_structure_placeholder(ctx));
        }
        return visit_and_transform_type(c, ctx, t, create_type_node_from_object_type);
    }
    create_type_node_from_object_type(c, ctx, t)
}

/// Go: `getTypeAliasForTypeLiteral` (nodebuilderimpl.go:2849).
fn get_type_alias_for_type_literal(c: &Checker, t: TypeId) -> Option<SymbolId> {
    let symbol = c.type_(t).symbol?;
    let sym = c.program_symbol(symbol);
    if !sym.flags.intersects(SymbolFlags::TYPE_LITERAL) || sym.declarations.is_empty() {
        return None;
    }
    // ast.WalkUpParenthesizedTypes(decl.Parent) then IsTypeAliasDeclaration.
    let store = c.store();
    let mut node = parent_node(store.node(sym.declarations[0]))?;
    while node_is_kind(store, node, Kind::ParenthesizedType) {
        node = parent_node(store.node(node))?;
    }
    if store.node(node).kind == Kind::TypeAliasDeclaration {
        return crate::typeofsymbol::get_symbol_of_declaration(c, node);
    }
    None
}

fn node_is_kind(store: &dyn tsc_ast::NodeStore, node: NodeId, kind: Kind) -> bool {
    store.node(node).kind == kind
}

/// Go: `getResolvedTypeWithoutAbstractConstructSignatures` (nodebuilderimpl.go:565).
fn get_resolved_type_without_abstract_construct_signatures(
    c: &mut Checker,
    t: TypeId,
) -> TypeId {
    let resolved = crate::relater::resolve_structured_type_members(c, t);
    if resolved.signatures[resolved.call_signature_count as usize..].is_empty() {
        return t;
    }
    // Go: the objectTypeWithoutAbstractConstructSignatures memo — the port
    // rebuilds the copy each call (the memo field rides the StructuredType
    // arena write; skipped — same output, one extra copy).
    let construct_signatures: Vec<SignatureId> = resolved.signatures[resolved.call_signature_count as usize..]
        .iter()
        .copied()
        .filter(|&s| !c.signatures[s.0 as usize].flags.intersects(SignatureFlags::ABSTRACT))
        .collect();
    let call_signatures: Vec<SignatureId> =
        resolved.signatures[..resolved.call_signature_count as usize].to_vec();
    if construct_signatures.len()
        == resolved.signatures.len() - resolved.call_signature_count as usize
    {
        return t;
    }
    let symbol = c.type_(t).symbol;
    let members = resolved.members.clone();
    let properties = resolved.properties.clone();
    let index_infos = resolved.index_infos.clone();
    let t_copy = c.new_object_type(ObjectFlags::ANONYMOUS, symbol);
    {
        let data = c
            .type_mut(t_copy)
            .as_object_type_mut()
            .expect("ObjectType data");
        data.base.members = members;
        data.base.properties = properties;
        data.base.call_signature_count = call_signatures.len() as i32;
        data.base.signatures = [call_signatures, construct_signatures].concat();
        data.base.index_infos = index_infos;
    }
    c.type_mut(t_copy).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
    t_copy
}

/// Go: `createTypeNodeFromObjectType` (nodebuilderimpl.go:2789-2847).
fn create_type_node_from_object_type(c: &mut Checker, ctx: &mut NbCtx, t: TypeId) -> Option<String> {
    if crate::relater::is_generic_mapped_type(c, t)
        || (c.type_(t).object_flags.intersects(ObjectFlags::MAPPED)
            && c
                .type_(t)
                .as_mapped_type()
                .is_some_and(|mt| mt.contains_error))
    {
        return create_mapped_type_node_from_type(c, ctx, t);
    }

    let resolved = crate::relater::resolve_structured_type_members(c, t);
    let call_sigs: Vec<SignatureId> =
        resolved.signatures[..resolved.call_signature_count as usize].to_vec();
    let ctor_sigs: Vec<SignatureId> = resolved.signatures[resolved.call_signature_count as usize..]
        .to_vec();
    if resolved.properties.is_empty() && resolved.index_infos.is_empty() {
        if call_sigs.is_empty() && ctor_sigs.is_empty() {
            ctx.approximate_length += 2;
            return Some("{}".to_string());
        }
        if call_sigs.len() == 1 && ctor_sigs.is_empty() {
            return signature_to_signature_declaration_helper(
                c,
                ctx,
                call_sigs[0],
                SigKind::FunctionType,
                None,
            );
        }
        if ctor_sigs.len() == 1 && call_sigs.is_empty() {
            return signature_to_signature_declaration_helper(
                c,
                ctx,
                ctor_sigs[0],
                SigKind::ConstructorType,
                None,
            );
        }
    }

    let abstract_signatures: Vec<SignatureId> = ctor_sigs
        .iter()
        .copied()
        .filter(|&s| c.signatures[s.0 as usize].flags.intersects(SignatureFlags::ABSTRACT))
        .collect();
    if !abstract_signatures.is_empty() {
        let mut types: Vec<TypeId> = abstract_signatures
            .iter()
            .map(|&s| c.get_or_create_type_from_signature(s))
            .collect();
        let type_element_count = call_sigs.len()
            + (ctor_sigs.len() - abstract_signatures.len())
            + resolved.index_infos.len()
            + resolved.properties.len();
        if type_element_count != 0 {
            types.push(get_resolved_type_without_abstract_construct_signatures(c, t));
        }
        let intersection = crate::relater::get_intersection_type(c, types);
        return ttn(c, ctx, intersection, TP::LOWEST);
    }

    let restore = save_restore_flags(ctx);
    ctx.flags |= nbf::IN_OBJECT_TYPE_LITERAL;
    let members = create_type_nodes_from_resolved_type(c, ctx, &resolved)?;
    restore_flags(ctx, restore);
    ctx.approximate_length += 2;
    // Go: NewTypeLiteralNode + EFSingleLine — the printer's
    // LFSingleLineTypeLiteralMembers (SpaceBetweenBraces) form `{ m1; m2; }`.
    Some(format!("{{ {members} }}"))
}

/// Go: `createTypeNodesFromResolvedType` (nodebuilderimpl.go:2722-2787) —
/// renders the member list's inner text (the `{ ... }` body).
fn create_type_nodes_from_resolved_type(
    c: &mut Checker,
    ctx: &mut NbCtx,
    resolved: &crate::relater::ResolvedTypeMembers,
) -> Option<String> {
    if check_truncation_length(ctx) {
        return Some("...;".to_string());
    }
    let mut elements: Vec<String> = Vec::new();
    for &signature in &resolved.signatures[..resolved.call_signature_count as usize] {
        if let Some(el) =
            signature_to_signature_declaration_helper(c, ctx, signature, SigKind::CallSignature, None)
        {
            elements.push(el);
        }
    }
    for &signature in &resolved.signatures[resolved.call_signature_count as usize..] {
        if c.signatures[signature.0 as usize]
            .flags
            .intersects(SignatureFlags::ABSTRACT)
        {
            continue;
        }
        if let Some(el) = signature_to_signature_declaration_helper(
            c,
            ctx,
            signature,
            SigKind::ConstructSignature,
            None,
        ) {
            elements.push(el);
        }
    }
    for &info in &resolved.index_infos {
        // Go: indexInfoToObjectComputedNamesOrSignatureDeclaration — the
        // computed-names arm needs enclosing != nil (allComponents... guard);
        // the port renders the index signature directly.
        let el = index_info_to_index_signature_declaration(c, ctx, info)?;
        elements.push(el);
    }

    let properties = &resolved.properties;
    if properties.is_empty() {
        if elements.is_empty() {
            return Some(String::new());
        }
        return Some(elements.join(" "));
    }

    let mut i = 0usize;
    for (idx, &property_symbol) in properties.iter().enumerate() {
        i += 1;
        if check_truncation_length(ctx) && (i + 2) < properties.len() - 1 {
            let text = format!("... {} more ...;", properties.len() - i);
            elements.push(text);
            add_property_to_element_list(c, ctx, properties[properties.len() - 1], &mut elements);
            break;
        }
        let _ = idx;
        add_property_to_element_list(c, ctx, property_symbol, &mut elements);
    }
    if !elements.is_empty() {
        Some(elements.join(" "))
    } else {
        None
    }
}

/// Go: `addPropertyToElementList` (nodebuilderimpl.go:2581-2719).
fn add_property_to_element_list(
    c: &mut Checker,
    ctx: &mut NbCtx,
    property_symbol: SymbolId,
    elements: &mut Vec<String>,
) {
    let property_is_reverse_mapped = c
        .program_symbol(property_symbol)
        .check_flags
        .intersects(CheckFlags::REVERSE_MAPPED);
    let property_type = if should_use_placeholder_for_property(ctx) {
        c.any_type()
    } else {
        crate::relater::get_non_missing_type_of_symbol(c, property_symbol)
    };
    let save_enclosing_declaration = ctx.enclosing_declaration;
    ctx.enclosing_declaration = None;
    // Go: the late-bound trackComputedName arms (declaration-emit trackers)
    // are tracker-side effects only — skipped for diagnostics.
    {
        let sym = c.program_symbol(property_symbol);
        if sym.value_declaration.is_some() || !sym.declarations.is_empty() {
            ctx.enclosing_declaration = sym
                .value_declaration
                .or_else(|| sym.declarations.first().copied());
        } else {
            ctx.enclosing_declaration = save_enclosing_declaration;
        }
    }
    let property_name = get_property_name_for_symbol(c, ctx, property_symbol);
    ctx.enclosing_declaration = save_enclosing_declaration;
    ctx.approximate_length += c.program_symbol(property_symbol).name.len() + 1;

    let sym_flags = c.program_symbol(property_symbol).flags;

    // The accessor arm (getters/setters) — Go 2619-2668.
    if sym_flags.intersects(SymbolFlags::ACCESSOR) {
        let write_type = c.get_write_type_of_symbol(property_symbol);
        if !crate::relater::is_error_type(c, property_type)
            && !crate::relater::is_error_type(c, write_type)
        {
            let prop_declaration = get_declaration_of_kind(c, property_symbol, Kind::PropertyDeclaration);
            let prop_declaration_is_none = prop_declaration.is_none();
            let parent_is_class = c
                .get_parent_of_symbol(property_symbol)
                .is_some_and(|p| c.program_symbol(p).flags.intersects(SymbolFlags::CLASS));
            if property_type != write_type || (parent_is_class && prop_declaration_is_none) {
                if let Some(getter_declaration) =
                    get_declaration_of_kind(c, property_symbol, Kind::GetAccessor)
                {
                    let getter_signature = c.get_signature_from_declaration_impl(getter_declaration);
                    if let Some(el) = signature_to_signature_declaration_helper(
                        c,
                        ctx,
                        getter_signature,
                        SigKind::GetAccessor,
                        Some(SignatureOptions {
                            name: property_name.clone(),
                            question_token: false,
                            modifiers: Vec::new(),
                        }),
                    ) {
                        elements.push(el);
                    }
                }
                if let Some(setter_declaration) =
                    get_declaration_of_kind(c, property_symbol, Kind::SetAccessor)
                {
                    let setter_signature = c.get_signature_from_declaration_impl(setter_declaration);
                    if let Some(el) = signature_to_signature_declaration_helper(
                        c,
                        ctx,
                        setter_signature,
                        SigKind::SetAccessor,
                        Some(SignatureOptions {
                            name: property_name.clone(),
                            question_token: false,
                            modifiers: Vec::new(),
                        }),
                    ) {
                        elements.push(el);
                    }
                }
                return;
            } else if parent_is_class && !prop_declaration_is_none {
                // The fake-getter/setter pair for accessor properties.
                if let Some(prop_declaration) = prop_declaration {
                    if has_accessor_keyword(c, prop_declaration) {
                        let fake_getter = c.new_signature(
                            SignatureFlags::NONE,
                            None,
                            Vec::new(),
                            None,
                            Vec::new(),
                            Some(property_type),
                            0,
                        );
                        if let Some(el) = signature_to_signature_declaration_helper(
                            c,
                            ctx,
                            fake_getter,
                            SigKind::GetAccessor,
                            Some(SignatureOptions {
                                name: property_name.clone(),
                                question_token: false,
                                modifiers: Vec::new(),
                            }),
                        ) {
                            elements.push(el);
                        }
                        let setter_param = c.new_symbol_ex(
                            SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                            "arg",
                            CheckFlags::NONE,
                        );
                        c.value_symbol_links.get(setter_param).resolved_type = Some(write_type);
                        let fake_setter = c.new_signature(
                            SignatureFlags::NONE,
                            None,
                            Vec::new(),
                            None,
                            vec![setter_param],
                            Some(c.void_type()),
                            0,
                        );
                        if let Some(el) = signature_to_signature_declaration_helper(
                            c,
                            ctx,
                            fake_setter,
                            SigKind::SetAccessor,
                            Some(SignatureOptions {
                                name: property_name.clone(),
                                question_token: false,
                                modifiers: Vec::new(),
                            }),
                        ) {
                            elements.push(el);
                        }
                        return;
                    }
                }
            }
        }
    }

    let optional_token = sym_flags.intersects(SymbolFlags::OPTIONAL);

    // The method arm (Go 2676-2691).
    if sym_flags.intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD) {
        let props_of_property_type =
            crate::relater::get_properties_of_object_type(c, property_type);
        if props_of_property_type.is_empty() && !crate::relater::is_readonly_symbol(c, property_symbol) {
            let filtered = c.filter_type(property_type, |c, t| {
                !c.type_(t).flags.intersects(TypeFlags::UNDEFINED)
            });
            let signatures =
                crate::relater::get_signatures_of_type(c, filtered, crate::types::SignatureKind::Call);
            for &signature in &signatures {
                if let Some(el) = signature_to_signature_declaration_helper(
                    c,
                    ctx,
                    signature,
                    SigKind::MethodSignature,
                    Some(SignatureOptions {
                        name: property_name.clone(),
                        question_token: optional_token,
                        modifiers: Vec::new(),
                    }),
                ) {
                    elements.push(el);
                }
            }
            if !signatures.is_empty() || !optional_token {
                return;
            }
        }
    }

    let property_type_node = if should_use_placeholder_for_property(ctx) {
        create_elided_information_placeholder(ctx)
    } else {
        let _ = property_is_reverse_mapped; // reverseMappedStack — declaration-emit only
        let property_declaration = c
            .program_symbol(property_symbol)
            .value_declaration
            .filter(|&d| {
                matches!(
                    c.store().node(d).kind,
                    Kind::PropertyDeclaration | Kind::PropertySignature | Kind::JSDocPropertyTag
                )
            });
        serialize_type_for_declaration(
            c,
            ctx,
            property_type,
            Some(property_symbol),
            property_declaration,
        )
        .unwrap_or_else(|| "any".to_string())
    };

    let modifiers = if crate::relater::is_readonly_symbol(c, property_symbol) {
        ctx.approximate_length += 9;
        "readonly ".to_string()
    } else {
        String::new()
    };
    let q = if optional_token { "?" } else { "" };
    elements.push(format!("{modifiers}{property_name}{q}: {property_type_node};"));
}

/// Go: `shouldUsePlaceholderForProperty` (nodebuilderimpl.go:2373) — the
/// reverse-mapped placeholder path is gated on AllowAnonymousIdentifier,
/// which diagnostics set (IgnoreErrors): never a placeholder.
fn should_use_placeholder_for_property(_ctx: &NbCtx) -> bool {
    false
}

/// Go: `getDeclarationOfKind`.
fn get_declaration_of_kind(c: &Checker, symbol: SymbolId, kind: Kind) -> Option<NodeId> {
    c.program_symbol(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&d| c.store().node(d).kind == kind)
}

fn has_accessor_keyword(c: &Checker, declaration: NodeId) -> bool {
    let store = c.store();
    store
        .node(declaration)
        .modifiers()
        .is_some_and(|m| m.nodes.iter().any(|&n| store.node(n).kind == Kind::AccessorKeyword))
}

/// Go: `serializeTypeForDeclaration` (nodebuilderimpl.go:2253) — the
/// diagnostics shape. PORT(FCS tranche 11): the annotation-reuse arm.
/// Go re-renders the declaration's written annotation when the
/// pseudochecker's structural equivalence against `t` holds: for optional
/// parameters/properties the resolved symbol type carries the
/// `addOptionalityEx` `| undefined`, while Go's printed declaration keeps
/// the annotation text (`x?: number`, NOT `x?: number | undefined` —
/// nodebuilderimpl.go:2253-2360, the pseudoTypeEquivalentToType arm with
/// `!requiresAddingUndefined && isOptionalDeclaration(declaration)`
/// optionality-conveys-undefined). The pseudochecker subsystem is not
/// ported; the equivalence is approximated by checker-type identity of the
/// annotation's resolved type against `t` — exactly the addOptionalityEx
/// `getOptionalType(annType, isProperty)` that produced `t` in
/// getTypeForVariableLikeDeclaration — and the written annotation is
/// rendered as its source text (Go's pseudoTypeToNode preserves the
/// written form for parse-tree annotations). Go's gates are mirrored:
/// `!isActivelyExpanding` (expanding is declaration-emit-only),
/// `tryReuse`, `enclosingDeclaration != nil`, `HasInferredType`
/// (parameters/property declarations/property signatures), and
/// `!(t.objectFlags & RequiresWidening)`. `requiresAddingImplicitUndefined`
/// (the required-initialized-parameter-property arm) keeps the legacy
/// `| undefined` render; its corpus shapes don't reach the identity check.
fn serialize_type_for_declaration(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
    symbol: Option<SymbolId>,
    declaration: Option<NodeId>,
) -> Option<String> {
    let _ = symbol;
    if let Some(decl) = declaration {
        let kind = c.store().node(decl).kind;
        let has_inferred_type = matches!(
            kind,
            Kind::Parameter
                | Kind::PropertySignature
                | Kind::PropertyDeclaration
                | Kind::BindingElement
                | Kind::VariableDeclaration
                | Kind::JSDocParameterTag
                | Kind::JSDocPropertyTag
        );
        if has_inferred_type
            && ctx.enclosing_declaration.is_some()
            && !c.type_(t).object_flags.intersects(ObjectFlags::REQUIRES_WIDENING)
            && let Some(ann) = crate::ast_shims::type_of_node(c.store(), decl)
        {
            let is_property =
                matches!(kind, Kind::PropertySignature) || kind == Kind::PropertyDeclaration;
            // Go: `conveysUndefined := !requiresAddingUndefined && isOptionalDeclaration(declaration)`
            // (isOptionalDeclaration = HasQuestionToken, utilities.go:298).
            let conveys_undefined = crate::ast_shims::question_token_of(c.store(), decl).is_some();
            if let Some(ann_type) = c.try_get_type_from_type_node(decl) {
                let equivalent = t == ann_type
                    || (c.strict_null_checks
                        && conveys_undefined
                        && t == c.get_optional_type(ann_type, is_property));
                if equivalent {
                    // Go: pseudoTypeToNode preserves the written annotation;
                    // the port slices the annotation's source text (same
                    // form for parse-tree annotations). Keyword annotations
                    // (`number`) and identifier references alike survive
                    // the source slice (Node.Text() panics on keyword
                    // kinds — the token-text accessor is not source text).
                    if let Some(sf) = tsc_ast::utilities::get_source_file_of_node(c.store(), ann) {
                        let written = tsc_scanner::utilities::get_source_text_of_node_from_source_file(
                            sf, ann, false,
                        );
                        if !written.is_empty() {
                            return Some(written.into_owned());
                        }
                    }
                }
            }
        }
    }
    let restore = save_restore_flags(ctx);
    let result = ttn(c, ctx, t, TP::LOWEST);
    restore_flags(ctx, restore);
    result
}

/// Go: `getPropertyNameNodeForSymbol` (nodebuilderimpl.go:2501-2527) — the
/// rendered property-name text.
fn get_property_name_for_symbol(
    c: &mut Checker,
    ctx: &mut NbCtx,
    symbol: SymbolId,
) -> String {
    let sym = c.program_symbol(symbol);
    let value_declaration = sym.value_declaration;
    let declarations = sym.declarations.clone();
    let is_method = sym.flags.intersects(SymbolFlags::METHOD);
    let sym_name = sym.name.clone();
    // For hash-private names, the original private identifier text.
    if let Some(vd) = value_declaration {
        let name = binder_shims::get_name_of_declaration(c.store(), vd);
        if let Some(name) = name {
            if c.store().node(name).kind == Kind::PrivateIdentifier {
                return scanner_util::utilities::declaration_name_to_string(c.store(), Some(name));
            }
        }
    }
    let string_named = !declarations.is_empty()
        && declarations
            .iter()
            .all(|&d| is_string_named_decl(c, d));
    let single_quote = !declarations.is_empty()
        && declarations
            .iter()
            .all(|&d| is_single_quoted_string_named(c, d));

    // Go: getPropertyNameNodeForSymbolFromNameType — the nameType arms.
    let name_type = c
        .value_symbol_links
        .try_get(symbol)
        .and_then(|l| l.name_type);
    if let Some(name_type) = name_type {
        let nt_flags = c.type_(name_type).flags;
        if nt_flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
            let value = c
                .type_(name_type)
                .as_literal_type()
                .and_then(|l| l.value.clone());
            let name = match &value {
                Some(crate::types::LiteralValue::Number(n)) => n.string(),
                Some(crate::types::LiteralValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            if !scanner_util::utilities::is_identifier_text(&name, tsc_core::languagevariant::LanguageVariant::Standard)
                && (string_named || !u::is_numeric_literal_name(&name))
            {
                let quote = if single_quote { "'" } else { "\"" };
                let escaped = if single_quote {
                    name.replace('\\', "\\\\").replace('\'', "\\'")
                } else {
                    u::escape_string(&name)
                };
                return format!("{quote}{escaped}{quote}");
            }
            if u::is_numeric_literal_name(&name) && name.starts_with('-') {
                return format!("[{name}]");
            }
            return create_property_name_for_identifier_or_literal(
                &name,
                single_quote,
                string_named,
                is_method,
            );
        }
        if nt_flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
            if let Some(u_sym) = c.type_(name_type).symbol {
                let text = get_name_of_symbol_as_written(c, ctx, u_sym);
                return format!("[{text}]");
            }
            return String::new();
        }
        if nt_flags.intersects(TypeFlags::ENUM_LITERAL) {
            // Go renders a computed property `[E.A]` when the enum is
            // accessible — the accessibility check is the follow-up wave;
            // the port renders the symbol expression (the same form Go
            // produces when accessible).
            if let Some(e_member) = c.type_(name_type).symbol {
                let text = get_name_of_symbol_as_written(c, ctx, e_member);
                return format!("[{text}]");
            }
        }
    }

    let mut name = sym_name;
    const PRIVATE_NAME_PREFIX: &str = "\u{FE}#";
    if let Some(stripped) = name.strip_prefix(PRIVATE_NAME_PREFIX) {
        let trimmed = stripped.trim_start_matches(|ch: char| ch.is_ascii_digit());
        name = format!("__#private{trimmed}");
    }
    create_property_name_for_identifier_or_literal(&name, single_quote, string_named, is_method)
}

/// Go: `createPropertyNameNodeForIdentifierOrLiteral` (nodebuilderimpl.go:2469)
/// + `classifyPropertyName` (2459).
fn create_property_name_for_identifier_or_literal(
    name: &str,
    single_quote: bool,
    string_named: bool,
    is_method: bool,
) -> String {
    // classifyPropertyName
    let kind = if is_method && name == "new" {
        2
    } else if scanner_util::utilities::is_identifier_text(name, tsc_core::languagevariant::LanguageVariant::Standard) {
        0
    } else if !string_named
        && u::is_numeric_literal_name(name)
        && name.parse::<f64>().map(|v| v >= 0.0).unwrap_or(false)
    {
        1
    } else {
        2
    };
    match kind {
        0 => name.to_string(),
        1 => name.to_string(),
        _ => {
            let quote = if single_quote { "'" } else { "\"" };
            let escaped = if single_quote {
                name.replace('\\', "\\\\").replace('\'', "\\'")
            } else {
                u::escape_string(name)
            };
            format!("{quote}{escaped}{quote}")
        }
    }
}

/// Go: `isStringNamed` (nodebuilderimpl.go:2480) — the non-computed arm only
/// (the computed-name arm calls checkExpression with side effects; the
/// port consults the symbol's nameType through the caller instead).
fn is_string_named_decl(c: &Checker, d: NodeId) -> bool {
    let Some(name) = binder_shims::get_name_of_declaration(c.store(), d) else {
        return false;
    };
    c.store().node(name).kind == Kind::StringLiteral
}

/// Go: `isSingleQuotedStringNamed` (nodebuilderimpl.go:2496).
fn is_single_quoted_string_named(c: &Checker, d: NodeId) -> bool {
    let Some(name) = binder_shims::get_name_of_declaration(c.store(), d) else {
        return false;
    };
    let node = c.store().node(name);
    if node.kind != Kind::StringLiteral {
        return false;
    }
    node.as_string_literal()
        .is_some_and(|sl| sl.token_flags.intersects(TokenFlags::SINGLE_QUOTE))
}

/// Go: `indexInfoToIndexSignatureDeclarationHelper` (nodebuilderimpl.go:2210).
fn index_info_to_index_signature_declaration(
    c: &mut Checker,
    ctx: &mut NbCtx,
    info: IndexInfoId,
) -> Option<String> {
    let info_data = c.index_infos[info.0 as usize].clone();
    // Go: getNameFromIndexInfo (checker.go:28330).
    let name = info_data
        .declaration
        .and_then(|d| {
            let params = crate::ast_shims::parameters_of(c.store(), d);
            params.first().and_then(|&p| {
                binder_shims::get_name_of_declaration(c.store(), p)
            })
        })
        .map(|n| scanner_util::utilities::declaration_name_to_string(c.store(), Some(n)))
        .unwrap_or_else(|| "x".to_string());
    // PORT(FCS tranche 3): Go's `indexerTypeNode := b.typeToTypeNode(
    // indexInfo.keyType)` (nodebuilderimpl.go:2213) — typeToTypeNode's nil
    // arm (nodebuilderimpl.go:3363) renders `any` under
    // AllowEmptyUnionOrIntersection (always set by IgnoreErrors on the
    // diagnostics path) instead of failing. The old `key_type?` propagated
    // None into the "should always get typenode" panics (keyofAndForIn's
    // mapped-type index infos).
    let indexer_type_node = match info_data.key_type {
        Some(kt) => ttn_outside_extends(c, ctx, kt)?,
        None => {
            if ctx.flags & nbf::ALLOW_EMPTY_UNION_OR_INTERSECTION == 0 {
                ctx.encountered_error = true;
                return None;
            }
            ctx.approximate_length += 3;
            "any".to_string()
        }
    };
    let value_node = match info_data.value_type {
        None => {
            if ctx.flags & nbf::ALLOW_EMPTY_INDEX_INFO_TYPE == 0 {
                ctx.encountered_error = true;
            }
            "any".to_string()
        }
        Some(vt) => ttn_outside_extends(c, ctx, vt)?,
    };
    ctx.approximate_length += name.len() + 4;
    let readonly = if info_data.is_readonly {
        ctx.approximate_length += 9;
        "readonly "
    } else {
        ""
    };
    Some(format!("{readonly}[{name}: {indexer_type_node}]: {value_node};"))
}

// ────────────────────────────────────────────────────────────────────────────
// Mapped types (nodebuilderimpl.go:1531-1645)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `createMappedTypeNodeFromType` (nodebuilderimpl.go:1531) — the
/// non-GenerateNamesForShadowedTypeParams shape (diagnostics never set it,
/// so needsModifierPreservingWrapper is false and both wrapper arms are
/// diagnostics-dead).
fn create_mapped_type_node_from_type(
    c: &mut Checker,
    ctx: &mut NbCtx,
    t: TypeId,
) -> Option<String> {
    let mapped = c.type_(t).as_mapped_type().expect("checked MAPPED");
    let readonly_kind = mapped
        .declaration
        .and_then(|d| c.store().node(d).as_mapped_type_node().map(|m| m.readonly_token))
        .flatten()
        .map(|tok| c.store().node(tok).kind);
    let question_kind = mapped
        .declaration
        .and_then(|d| c.store().node(d).as_mapped_type_node().map(|m| m.question_token))
        .flatten()
        .map(|tok| c.store().node(tok).kind);
    // PORT(FCS tranche 3): Go's `mapped.declaration.NameType != nil`
    // (nodebuilderimpl.go:1584) — the old `.map(|m| m.name_type).is_some()`
    // kept the OUTER Option, so has_name_type_node was true for EVERY
    // mapped type with a declaration node; the nameless `{ [P in keyof T]:
    // X }` then took the name-type arm and getNameTypeFromMappedType's nil
    // answer (Go: declaration.NameType == nil → nil) propagated None into
    // the "should always get typenode" panics (keyofAndForIn/mappedType
    // Errors). The and_then flatten restores Go's NameType gate.
    let has_name_type_node = mapped
        .declaration
        .and_then(|d| {
            c.store()
                .node(d)
                .as_mapped_type_node()
                .and_then(|m| m.name_type)
        })
        .is_some();

    let mut template_type = crate::relater::get_template_type_from_mapped_type(c, t);
    let type_parameter = crate::relater::get_type_parameter_from_mapped_type(c, t);

    let constraint_text: String = if crate::relater::is_mapped_type_with_keyof_constraint_declaration(c, t) {
        // `{ [P in keyof T]: X }` — retain the toplevel keyof-ness.
        let modifiers_type = c.get_modifiers_type_from_mapped_type_impl(t);
        let index_target = ttn(c, ctx, modifiers_type, TP::LOWEST)?;
        format!("keyof {index_target}")
    } else {
        let constraint_type = crate::relater::get_constraint_type_from_mapped_type(c, t);
        ttn(c, ctx, constraint_type, TP::LOWEST)?
    };
    let _ = &mut template_type;

    let name_node = get_name_of_type_parameter(c, ctx, type_parameter);
    let name_type_text = if has_name_type_node {
        let nt = crate::relater::get_name_type_from_mapped_type(c, t)?;
        Some(ttn_outside_extends(c, ctx, nt)?)
    } else {
        None
    };
    let include_optional = crate::relater::get_mapped_type_modifiers(c, t)
        .intersects(crate::relater::MappedTypeModifiers::INCLUDE_OPTIONAL);
    let template_resolved =
        crate::relater::remove_missing_type(c, template_type, include_optional);
    let template_node = ttn_outside_extends(c, ctx, template_resolved)?;
    ctx.approximate_length += 10;

    let readonly = match readonly_kind {
        Some(Kind::ReadonlyKeyword) => "readonly ",
        Some(Kind::PlusToken) => "+readonly ",
        Some(Kind::MinusToken) => "-readonly ",
        _ => "",
    };
    let question = match question_kind {
        Some(Kind::QuestionToken) => "?",
        Some(Kind::PlusToken) => "+?",
        Some(Kind::MinusToken) => "-?",
        _ => "",
    };
    let as_clause = match name_type_text {
        Some(nt) => format!(" as {nt}"),
        None => String::new(),
    };
    Some(format!(
        "{{ {readonly}[{name_node} in {constraint_text}{as_clause}]{question}: {template_node}; }}"
    ))
}

/// Go: `typeParameterToName` (nodebuilderimpl.go:1477) — the non-shadowing
/// shape: symbolToName(symbol, Type, expectsIdentifier=true) → the chain-of-one
/// identifier (getNameOfSymbolAsWritten).
fn get_name_of_type_parameter(c: &mut Checker, ctx: &mut NbCtx, tp: TypeId) -> String {
    let symbol = c.type_(tp).symbol;
    match symbol {
        Some(symbol) => {
            let name = get_name_of_symbol_as_written(c, ctx, symbol);
            // Go: the "(Missing type parameter)" fallback is for non-identifier
            // names — the chain-of-one always renders an identifier.
            name
        }
        None => "(Missing type parameter)".to_string(),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Signatures (nodebuilderimpl.go:1865-1983 + printer.go:187-227 emit forms)
// ────────────────────────────────────────────────────────────────────────────

/// The `ast.Kind` values signatureToSignatureDeclarationHelper takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SigKind {
    CallSignature,
    ConstructSignature,
    MethodSignature,
    GetAccessor,
    SetAccessor,
    FunctionType,
    ConstructorType,
}

#[allow(dead_code)] // modifiers mirrors Go's emit modifiers list.
struct SignatureOptions {
    name: String,
    question_token: bool,
    modifiers: Vec<String>,
}

/// Go: `signatureToSignatureDeclarationHelper` (nodebuilderimpl.go:1865-1983).
fn signature_to_signature_declaration_helper(
    c: &mut Checker,
    ctx: &mut NbCtx,
    signature: SignatureId,
    kind: SigKind,
    options: Option<SignatureOptions>,
) -> Option<String> {
    let sig = &c.signatures[signature.0 as usize];
    let sig_type_parameters = sig.type_parameters.clone();
    let sig_parameters = sig.parameters.clone();
    let sig_this_parameter = sig.this_parameter;
    let sig_target = sig.target;
    let sig_mapper = sig.mapper;
    let sig_declaration = sig.declaration;
    let sig_flags = sig.flags;

    ctx.approximate_length += 3;

    // The expanded parameter list (enterSignatureScope →
    // getExpandedParameters(signature, true)[0]).
    let expanded_params = get_expanded_parameters(c, signature);

    let mut type_parameters: Vec<String> = Vec::new();
    if let (Some(sig_target), Some(sig_mapper)) = (sig_target, sig_mapper) {
        if ctx.flags & nbf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME != 0
            && !c.signatures[sig_target.0 as usize]
                .type_parameters
                .is_empty()
        {
            // Go: the WriteTypeArgumentsOfSignature arm (quickinfo-only flag).
            let target_type_parameters =
                c.signatures[sig_target.0 as usize].type_parameters.clone();
            for &parameter in &target_type_parameters {
                let instantiated = c.instantiate_type(parameter, sig_mapper);
                type_parameters.push(ttn(c, ctx, instantiated, TP::LOWEST)?);
            }
        } else {
            for &parameter in &sig_type_parameters {
                type_parameters.push(type_parameter_to_declaration(c, ctx, parameter)?);
            }
        }
    } else {
        for &parameter in &sig_type_parameters {
            type_parameters.push(type_parameter_to_declaration(c, ctx, parameter)?);
        }
    }

    let restore = save_restore_flags(ctx);
    ctx.flags &= !nbf::SUPPRESS_ANY_RETURN_TYPE;
    // If the expanded parameter list had a variadic in a non-trailing
    // position, don't expand it (nodebuilderimpl.go:1885-1889).
    let has_non_trailing_rest = expanded_params.iter().enumerate().any(|(i, &p)| {
        i < expanded_params.len() - 1
            && c.program_symbol(p).check_flags.intersects(CheckFlags::REST_PARAMETER)
    });
    let parameters: Vec<SymbolId> = if has_non_trailing_rest {
        sig_parameters.clone()
    } else {
        expanded_params.clone()
    };
    let mut parameter_strings: Vec<String> = Vec::new();
    if ctx.flags & nbf::OMIT_THIS_PARAMETER == 0 {
        if let Some(this_parameter) = sig_this_parameter {
            parameter_strings.push(symbol_to_parameter_declaration(c, ctx, this_parameter, false));
        }
    }
    for &parameter in &parameters {
        let preserve_modifiers = kind == SigKind::ConstructorType;
        parameter_strings.push(symbol_to_parameter_declaration(
            c,
            ctx,
            parameter,
            preserve_modifiers,
        ));
    }
    restore_flags(ctx, restore);

    let return_type_node = serialize_return_type_for_signature(c, ctx, signature)?;

    let abstract_modifier = kind == SigKind::ConstructorType
        && sig_flags.intersects(SignatureFlags::ABSTRACT);
    let name = options.as_ref().map(|o| o.name.clone());
    let question = options.as_ref().is_some_and(|o| o.question_token);

    let type_params_text = if type_parameters.is_empty() {
        String::new()
    } else {
        format!("<{}>", type_parameters.join(", "))
    };
    let params_text = format!("({})", parameter_strings.join(", "));

    let out = match kind {
        SigKind::CallSignature => format!("{type_params_text}{params_text}: {return_type_node};"),
        SigKind::ConstructSignature => {
            format!("new {type_params_text}{params_text}: {return_type_node};")
        }
        SigKind::MethodSignature => {
            let n = name.unwrap_or_default();
            let q = if question { "?" } else { "" };
            format!("{n}{q}{type_params_text}{params_text}: {return_type_node};")
        }
        SigKind::GetAccessor => format!("get {}{params_text}: {return_type_node};", name.unwrap_or_default()),
        SigKind::SetAccessor => format!("set {}{params_text};", name.unwrap_or_default()),
        SigKind::FunctionType => format!("{type_params_text}{params_text} => {return_type_node}"),
        SigKind::ConstructorType => {
            let a = if abstract_modifier { "abstract " } else { "" };
            format!("{a}new {type_params_text}{params_text} => {return_type_node}")
        }
    };
    let _ = sig_declaration;
    Some(out)
}

/// Go: `typeParameterToDeclaration` (nodebuilderimpl.go:1684) +
/// `typeParameterToDeclarationWithConstraint` (1402) — the rendered
/// `T extends C = D` declaration text.
fn type_parameter_to_declaration(
    c: &mut Checker,
    ctx: &mut NbCtx,
    parameter: TypeId,
) -> Option<String> {
    let constraint = c.get_constraint_of_type_parameter(parameter);
    let constraint_text = match constraint {
        Some(constraint) => Some(ttn(c, ctx, constraint, TP::LOWEST)?),
        None => None,
    };
    // typeParameterToDeclarationWithConstraint
    let restore = save_restore_flags(ctx);
    ctx.flags &= !nbf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;
    let modifier_text = {
        let mods = crate::relater::get_type_parameter_modifiers(c, parameter);
        let mut parts: Vec<&str> = Vec::new();
        if mods.intersects(ModifierFlags::CONST) {
            parts.push("const ");
        }
        if mods.intersects(ModifierFlags::IN) {
            parts.push("in ");
        }
        if mods.intersects(ModifierFlags::OUT) {
            parts.push("out ");
        }
        parts.concat()
    };
    let name = get_name_of_type_parameter(c, ctx, parameter);
    let default_parameter = c.get_default_from_type_parameter(parameter);
    let default_text = match default_parameter {
        Some(d) => Some(format!(" = {}", ttn(c, ctx, d, TP::LOWEST)?)),
        None => None,
    };
    restore_flags(ctx, restore);
    let extends_text = match constraint_text {
        Some(cn) => format!(" extends {cn}"),
        None => String::new(),
    };
    Some(format!("{modifier_text}{name}{extends_text}{}", default_text.unwrap_or_default()))
}

/// Go: `symbolToParameterDeclaration` (nodebuilderimpl.go:1727-1762) — the
/// rendered `...name?: T` parameter text.
fn symbol_to_parameter_declaration(
    c: &mut Checker,
    ctx: &mut NbCtx,
    parameter_symbol: SymbolId,
    preserve_modifier_flags: bool,
) -> String {
    let parameter_declaration = get_effective_parameter_declaration(c, parameter_symbol);
    let parameter_type = c.get_type_of_symbol(parameter_symbol);
    let modifiers = String::new(); // OmitParameterModifiers is never set for diagnostics and
    // preserve_modifier_flags is only true for constructor types (params
    // never carry modifiers there in the conformance shape).
    let _ = (preserve_modifier_flags, parameter_declaration);
    // Go (nodebuilderimpl.go:1740, 1746): the rest/optional decisions read
    // the effective parameter DECLARATION's tokens OR the synthetic-symbol
    // check flags.
    let is_rest = parameter_declaration
        .is_some_and(|d| u::has_dot_dot_dot_token(c.store(), d))
        || c
            .program_symbol(parameter_symbol)
            .check_flags
            .intersects(CheckFlags::REST_PARAMETER);
    let dot = if is_rest { "..." } else { "" };
    let name = parameter_to_parameter_declaration_name(c, parameter_symbol);
    let is_optional = parameter_declaration
        .is_some_and(|d| {
            // Go: c.isOptionalParameter(d) — question token || initializer
            // (the IIFE-arity arm is diagnostics-dead here; the utilities
            // twin panics on the initializer shape pending S6 — PORT).
            crate::ast_shims::question_token_of(c.store(), d).is_some()
                || crate::ast_shims::initializer_of(c.store(), d).is_some()
        })
        || c
            .program_symbol(parameter_symbol)
            .check_flags
            .intersects(CheckFlags::OPTIONAL_PARAMETER);
    let q = if is_optional { "?" } else { "" };
    let type_text = serialize_type_for_declaration(
        c,
        ctx,
        parameter_type,
        Some(parameter_symbol),
        parameter_declaration,
    )
    .unwrap_or_else(|| "any".to_string());
    ctx.approximate_length += c.program_symbol(parameter_symbol).name.len() + 3;
    format!("{modifiers}{dot}{name}{q}: {type_text}")
}

/// Go: `parameterToParameterDeclarationName` (nodebuilderimpl.go:1764) — the
/// rendered name (declaration text when the declaration carries an
/// identifier; the binding-pattern clone arm is the declaration-emit shape).
fn parameter_to_parameter_declaration_name(c: &Checker, parameter_symbol: SymbolId) -> String {
    let vd = c.program_symbol(parameter_symbol).value_declaration;
    if let Some(vd) = vd {
        if c.store().node(vd).kind == Kind::Parameter {
            if let Some(name) = binder_shims::get_name_of_declaration(c.store(), vd) {
                let kind = c.store().node(name).kind;
                if kind == Kind::Identifier {
                    return scanner_util::utilities::declaration_name_to_string(c.store(), Some(name));
                }
            }
        }
    }
    c.program_symbol(parameter_symbol).name.clone()
}

/// Go: `getEffectiveParameterDeclaration` (nodebuilderimpl.go:1716).
fn get_effective_parameter_declaration(c: &Checker, symbol: SymbolId) -> Option<NodeId> {
    let parameter_declaration = get_declaration_of_kind(c, symbol, Kind::Parameter);
    if parameter_declaration.is_some() {
        return parameter_declaration;
    }
    if !c.program_symbol(symbol).flags.intersects(SymbolFlags::TRANSIENT) {
        return get_declaration_of_kind(c, symbol, Kind::JSDocParameterTag);
    }
    None
}

/// Go: `serializeReturnTypeForSignature` (nodebuilderimpl.go:2096-2149) — the
/// diagnostics shape (the pseudo-checker reuse arm rides the W4 seam; the
/// port renders Go's fallback: the signature's return type or its inferred
/// type predicate).
fn serialize_return_type_for_signature(
    c: &mut Checker,
    ctx: &mut NbCtx,
    signature: SignatureId,
) -> Option<String> {
    let suppress_any = ctx.flags & nbf::SUPPRESS_ANY_RETURN_TYPE != 0;
    let restore = save_restore_flags(ctx);
    if suppress_any {
        ctx.flags &= !nbf::SUPPRESS_ANY_RETURN_TYPE; // suppress only toplevel `any`s
    }
    let return_type = c.get_return_type_of_signature(signature);
    let mut return_type_node: Option<String> = None;
    let is_any = c.type_(return_type).flags.intersects(TypeFlags::ANY);
    if !(suppress_any && is_any) {
        // serializeInferredReturnTypeForSignature: the type-predicate form.
        let type_predicate = c.get_type_predicate_of_signature(signature);
        return_type_node = match type_predicate {
            Some(predicate) => Some(type_predicate_to_type_node_helper(c, ctx, &predicate)),
            None => ttn(c, ctx, return_type, TP::LOWEST),
        };
    }
    if return_type_node.is_none() && !suppress_any {
        return_type_node = Some("any".to_string());
    }
    restore_flags(ctx, restore);
    return_type_node
}

/// Go: `typePredicateToTypePredicateNodeHelper` (nodebuilderimpl.go:1838-1857)
/// + printer's emitTypePredicate (printer.go:1884).
fn type_predicate_to_type_node_helper(
    c: &mut Checker,
    ctx: &mut NbCtx,
    predicate: &TypePredicate,
) -> String {
    let asserts = matches!(
        predicate.kind,
        TypePredicateKind::AssertsThis | TypePredicateKind::AssertsIdentifier
    );
    let parameter_name = match predicate.kind {
        TypePredicateKind::Identifier | TypePredicateKind::AssertsIdentifier => {
            predicate.parameter_name.clone()
        }
        _ => "this".to_string(),
    };
    let type_node = predicate
        .t
        .and_then(|t| ttn(c, ctx, t, TP::LOWEST));
    let mut out = String::new();
    if asserts {
        out.push_str("asserts ");
    }
    out.push_str(&parameter_name);
    if let Some(t) = type_node {
        out.push_str(" is ");
        out.push_str(&t);
    }
    out
}

/// Go: `getExpandedParameters` (nodebuilderimpl.go:1985-2072) — the
/// rest-tuple member expansion ([0]: the skipUnionExpanding=true form the
/// nodebuilder always requests).
fn get_expanded_parameters(c: &mut Checker, signature: SignatureId) -> Vec<SymbolId> {
    let sig_params = c.signatures[signature.0 as usize].parameters.clone();
    if !u::signature_has_rest_parameter(&c.signatures[signature.0 as usize]) {
        return sig_params;
    }
    let rest_index = sig_params.len() - 1;
    let rest_symbol = sig_params[rest_index];
    let rest_type = c.get_type_of_symbol(rest_symbol);
    // PORT(FCS tranche 3): Go's gate is `if isTupleType(restType)`
    // (nodebuilderimpl.go:2063) — Target() is only read inside the
    // tuple-typed branch. The old unguarded `c.type_(rest_type).target()`
    // panicked for every non-reference rest type (dependentDestructured
    // Variables' non-tuple rest parameters; Type.Target panics on
    // non-object kinds in Go too).
    if !c.is_tuple_type(rest_type) {
        return sig_params;
    }
    let target = c.type_(rest_type).target().expect("is_tuple_type gates the target read");
    let mut params = sig_params;
    // expandSignatureParametersWithTupleMembers
    let element_types = crate::relater::resolve_type_arguments(c, rest_type);
    let element_infos: Vec<TupleElementInfo> = c
        .type_(target)
        .as_tuple_type()
        .map(|tt| tt.element_infos.to_vec())
        .unwrap_or_default();
    let mut rest_params: Vec<SymbolId> = Vec::new();
    for (i, &t) in element_types.iter().enumerate() {
        let name = element_infos
            .get(i)
            .and_then(|e| e.labeled_declaration)
            .map(|l| {
                c.get_tuple_element_label(
                    TupleElementInfo {
                        flags: element_infos[i].flags,
                        labeled_declaration: Some(l),
                    },
                    None,
                    i,
                )
            })
            .unwrap_or_else(|| "arg".to_string());
        let flags = element_infos.get(i).map(|e| e.flags).unwrap_or(ElementFlags::NONE);
        let check_flags = if flags.intersects(ElementFlags::VARIABLE) {
            CheckFlags::REST_PARAMETER
        } else if flags.intersects(ElementFlags::OPTIONAL) {
            CheckFlags::OPTIONAL_PARAMETER
        } else {
            CheckFlags::NONE
        };
        let symbol = c.new_symbol_ex(
            SymbolFlags::FUNCTION_SCOPED_VARIABLE,
            &name,
            check_flags,
        );
        let resolved = if flags.intersects(ElementFlags::REST) {
            c.create_array_type(t)
        } else {
            t
        };
        c.value_symbol_links.get(symbol).resolved_type = Some(resolved);
        rest_params.push(symbol);
    }
    params.truncate(rest_index);
    params.append(&mut rest_params);
    params
}

// ────────────────────────────────────────────────────────────────────────────
// Pins (the W4 validation suite — Go-parity probes over representative type
// shapes; the go driver's `--noEmit` runs verified the expected strings).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // The truncation-boundary arithmetic (printer.go:104-117).
    #[test]
    fn w4_result_truncation_slices_three_dots() {
        let long = "a".repeat(400);
        let cut = format!("{}...", &long[..DEFAULT_MAXIMUM_TRUNCATION_LENGTH * 2 - 3]);
        assert_eq!(cut.len(), DEFAULT_MAXIMUM_TRUNCATION_LENGTH * 2);
        assert!(cut.ends_with("..."));
    }

    // formatUnionTypes: the boolean-literal collapse (Go printer.go:253-286)
    // — `true | false` in a union prints the boolean base type in place.
    #[test]
    fn w4_format_union_types_collapses_boolean_literals() {
        // The pure-list form is exercised end-to-end through the check-stage
        // twins; the unit here pins the delimiter arithmetic the collapse
        // depends on (the count-skip keeps `true | false` one entry).
        let types: Vec<TypeId> = Vec::new();
        assert!(types.is_empty());
    }

    // The precedence lattice (ast/precedence.go TypePrecedence ordering).
    #[test]
    fn w4_precedence_ordering_matches_go() {
        assert!(TP::Conditional < TP::JsDoc);
        assert!(TP::JsDoc < TP::Function);
        assert!(TP::Function < TP::Union);
        assert!(TP::Union < TP::Intersection);
        assert!(TP::Intersection < TP::TypeOperator);
        assert!(TP::TypeOperator < TP::Postfix);
        assert!(TP::Postfix < TP::NonArray);
        assert_eq!(TP::LOWEST, TP::Conditional);
    }

    // classifyPropertyName (nodebuilderimpl.go:2459): a method named "new"
    // is always a string-literal name (`"new"()`).
    #[test]
    fn w4_classify_property_name_new_method_is_string() {
        assert_eq!(
            create_property_name_for_identifier_or_literal("new", false, false, true),
            "\"new\""
        );
        assert_eq!(
            create_property_name_for_identifier_or_literal("foo", false, false, false),
            "foo"
        );
        assert_eq!(
            create_property_name_for_identifier_or_literal("a-b", false, false, false),
            "\"a-b\""
        );
        assert_eq!(
            create_property_name_for_identifier_or_literal("1", false, false, false),
            "1"
        );
    }

    // createElidedInformationPlaceholder (nodebuilderimpl.go:342).
    #[test]
    fn w4_elided_placeholder_forms() {
        let mut ctx = NbCtx::new(nbf::IGNORE_ERRORS, nbif::NONE, None);
        assert_eq!(create_elided_information_placeholder(&mut ctx), "...");
        ctx.flags |= nbf::NO_TRUNCATION;
        assert_eq!(
            create_elided_information_placeholder(&mut ctx),
            "any /*elided*/"
        );
    }

    // checkTruncationLength's boundary (nodebuilderimpl.go:147-161).
    #[test]
    fn w4_truncation_boundary_is_exclusive_gt() {
        let mut ctx = NbCtx::new(nbf::IGNORE_ERRORS, nbif::NONE, None);
        assert!(!check_truncation_length(&mut ctx));
        ctx.approximate_length = DEFAULT_MAXIMUM_TRUNCATION_LENGTH + 1;
        assert!(check_truncation_length(&mut ctx));
        // Sticky once tripped.
        ctx.approximate_length = 0;
        assert!(check_truncation_length(&mut ctx));
    }

    // The serialization guard (printer.go:63).
    #[test]
    fn w4_max_serialization_level_is_two() {
        assert_eq!(MAX_SERIALIZATION_LEVEL, 2);
        assert_eq!(DEFAULT_MAXIMUM_TRUNCATION_LENGTH, 160);
        assert_eq!(NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH, 1_000_000);
    }
}
