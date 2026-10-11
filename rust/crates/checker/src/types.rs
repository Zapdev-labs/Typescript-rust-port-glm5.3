// Ported from tsc/internal/checker/types.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Complete port of the type-kind/flag tables, the TypeData shapes, the
// Signature/IndexInfo/TypePredicate carriers, and every link struct declared
// in types.go (the M5 spike's declared surface — SPEC §8 M5). Value shapes:
//   *Type / *ast.Symbol / *ast.Node / *Signature  → TypeId / SymbolId / NodeId /
//   SignatureId arena handles; map[...]*Type with observable laziness → Option<FxHashMap<...>>;
//   `any` literal values (LiteralType.value) → LiteralValue.
// The `checker *Checker` back-pointer becomes `checker_shard: u16` (SPEC §5.4,
// see lib.rs's PORT note); `TypeBase`'s Go self-reference (data.AsType() == t)
// disappears — the arena slot owns the payload.
//
// PORT (layout): Go's TypeData is an interface pointer — every concrete type
// struct is heap-allocated and embeds TypeBase (i.e. the Type itself). The port
// boxes every enum payload, which reproduces Go's allocation shape (one heap
// object per type payload) and keeps the arena slot at 48 bytes. Go's
// struct-embedding chains (ConstrainedType → StructuredType → ObjectType →
// TypeReference → InterfaceType → TupleType) port as nested base structs so the
// embedded-cast accessor family (AsConstrainedType/AsStructuredType/...)
// succeeds for the same variant families as in Go.

use std::fmt::Write as _;

use rustc_hash::FxHashMap;
use tsc_ast::{NodeId, SymbolFlags, SymbolId, SymbolTable};
use tsc_collections::OrderedSet;
use tsc_core::options_generated::ScriptTarget;
use tsc_core::tristate::Tristate;

use crate::todo_shapes::{CacheHashKey, EvaluatorResult, TypeMapperId};

define_flags!(ParseFlags, u32);
define_flags!(ContextFlags, u32);
define_flags!(TypeFormatFlags, u32);
define_flags!(SymbolFormatFlags, u32);
define_flags!(ExternalEmitHelpers, u32);
define_flags!(TypeFlags, u32);
define_flags!(ObjectFlags, u32);
define_flags!(VarianceFlags, u32);
define_flags!(AccessFlags, u32);
define_flags!(NodeCheckFlags, u32);
define_flags!(ElementFlags, u32);
define_flags!(IndexFlags, u32);
define_flags!(SignatureFlags, u32);

// ParseFlags

impl ParseFlags {
    pub const YIELD: ParseFlags = ParseFlags(1 << 0);
    pub const AWAIT: ParseFlags = ParseFlags(1 << 1);
    pub const TYPE: ParseFlags = ParseFlags(1 << 2);
    pub const IGNORE_MISSING_OPEN_BRACE: ParseFlags = ParseFlags(1 << 4);
    pub const JSDOC: ParseFlags = ParseFlags(1 << 5);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum SignatureKind {
    Call = 0,
    Construct = 1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum IndexKind {
    String = 0,
    Number = 1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum MemberOverrideStatus {
    None = 0,
    NeedsOverride = 1,
    HasInvalidOverride = 2,
}

impl ContextFlags {
    /// Obtaining contextual signature
    pub const SIGNATURE: ContextFlags = ContextFlags(1 << 0);
    /// Don't obtain type variable constraints
    pub const NO_CONSTRAINTS: ContextFlags = ContextFlags(1 << 1);
    /// Ignore inference to current node and parent nodes out to the containing call for, for example, completions
    pub const IGNORE_NODE_INFERENCES: ContextFlags = ContextFlags(1 << 2);
    /// Ignore contextual types applied by binding patterns
    pub const SKIP_BINDING_PATTERNS: ContextFlags = ContextFlags(1 << 3);
}

impl TypeFormatFlags {
    /// Don't truncate typeToString result
    pub const NO_TRUNCATION: TypeFormatFlags = TypeFormatFlags(1 << 0);
    /// Write Array<T> instead T[]
    pub const WRITE_ARRAY_AS_GENERIC_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 1);
    /// When a type parameter T is shadowing another T, generate a name for it so it can still be referenced
    pub const GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS: TypeFormatFlags = TypeFormatFlags(1 << 2);
    /// When an alias cannot be named by its symbol, rather than report an error, fallback to a structural printout if possible
    pub const USE_STRUCTURAL_FALLBACK: TypeFormatFlags = TypeFormatFlags(1 << 3);
    // hole because there's a hole in node builder flags
    /// Write the type arguments instead of type parameters of the signature
    pub const WRITE_TYPE_ARGUMENTS_OF_SIGNATURE: TypeFormatFlags = TypeFormatFlags(1 << 5);
    /// Write out the fully qualified type name (eg. Module.Type, instead of Type)
    pub const USE_FULLY_QUALIFIED_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 6);
    // hole because `UseOnlyExternalAliasing` is here in node builder flags, but functions which take old flags use `SymbolFormatFlags` instead
    /// If the return type is any-like, don't offer a return type.
    pub const SUPPRESS_ANY_RETURN_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 8);
    // hole because `WriteTypeParametersInQualifiedName` is here in node builder flags, but functions which take old flags use `SymbolFormatFlags` for this instead
    /// Always print object literals across multiple lines (only used to map into node builder flags)
    pub const MULTILINE_OBJECT_LITERALS: TypeFormatFlags = TypeFormatFlags(1 << 10);
    /// Write a type literal instead of (Anonymous class)
    pub const WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL: TypeFormatFlags = TypeFormatFlags(1 << 11);
    /// Write typeof instead of function type literal
    pub const USE_TYPE_OF_FUNCTION: TypeFormatFlags = TypeFormatFlags(1 << 12);
    /// Omit modifiers on parameters
    pub const OMIT_PARAMETER_MODIFIERS: TypeFormatFlags = TypeFormatFlags(1 << 13);
    /// For a `type T = ... ` defined in a different file, write `T` instead of its value, even though `T` can't be accessed in the current scope.
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: TypeFormatFlags = TypeFormatFlags(1 << 14);
    /// Use single quotes for string literal type
    pub const USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 28);
    /// Don't call getReducedType
    pub const NO_TYPE_REDUCTION: TypeFormatFlags = TypeFormatFlags(1 << 29);
    /// Use instantiation expressions for qualified instantiated names like Foo<string>.Bar
    pub const USE_INSTANTIATION_EXPRESSIONS: TypeFormatFlags = TypeFormatFlags(1 << 30);
    pub const OMIT_THIS_PARAMETER: TypeFormatFlags = TypeFormatFlags(1 << 25);
    /// Write construct signatures as call style signatures
    pub const WRITE_CALL_STYLE_SIGNATURE: TypeFormatFlags = TypeFormatFlags(1 << 27);
    // Error Handling
    /// This is bit 20 to align with the same bit in `NodeBuilderFlags`
    pub const ALLOW_UNIQUE_ESSYMBOL_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 20);
    // TypeFormatFlags exclusive
    /// Add undefined to types of initialized, non-optional parameters
    pub const ADD_UNDEFINED: TypeFormatFlags = TypeFormatFlags(1 << 17);
    /// Write arrow style signature
    pub const WRITE_ARROW_STYLE_SIGNATURE: TypeFormatFlags = TypeFormatFlags(1 << 18);
    // State
    /// Writing an array element type
    pub const IN_ARRAY_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 19);
    /// Writing an array or union element type
    pub const IN_ELEMENT_TYPE: TypeFormatFlags = TypeFormatFlags(1 << 21);
    /// Writing first type argument of the instantiated type
    pub const IN_FIRST_TYPE_ARGUMENT: TypeFormatFlags = TypeFormatFlags(1 << 22);
    /// Writing type in type alias declaration
    pub const IN_TYPE_ALIAS: TypeFormatFlags = TypeFormatFlags(1 << 23);

    pub const NODE_BUILDER_FLAGS_MASK: TypeFormatFlags = TypeFormatFlags(
        Self::NO_TRUNCATION.0
            | Self::WRITE_ARRAY_AS_GENERIC_TYPE.0
            | Self::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS.0
            | Self::USE_STRUCTURAL_FALLBACK.0
            | Self::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE.0
            | Self::USE_FULLY_QUALIFIED_TYPE.0
            | Self::SUPPRESS_ANY_RETURN_TYPE.0
            | Self::MULTILINE_OBJECT_LITERALS.0
            | Self::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL.0
            | Self::USE_TYPE_OF_FUNCTION.0
            | Self::OMIT_PARAMETER_MODIFIERS.0
            | Self::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE.0
            | Self::ALLOW_UNIQUE_ESSYMBOL_TYPE.0
            | Self::IN_TYPE_ALIAS.0
            | Self::USE_INSTANTIATION_EXPRESSIONS.0
            | Self::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE.0
            | Self::NO_TYPE_REDUCTION.0
            | Self::OMIT_THIS_PARAMETER.0,
    );
}

impl SymbolFormatFlags {
    // Write symbols's type argument if it is instantiated symbol
    // eg. class C<T> { p: T }   <-- Show p as C<T>.p here
    //     var a: C<number>;
    //     var p = a.p; <--- Here p is property of C<number> so show it as C<number>.p instead of just C.p
    pub const WRITE_TYPE_PARAMETERS_OR_ARGUMENTS: SymbolFormatFlags = SymbolFormatFlags(1 << 0);
    // Use only external alias information to get the symbol name in the given context
    // eg.  module m { export class c { } } import x = m.c;
    // When this flag is specified m.c will be used to refer to the class instead of alias symbol x
    pub const USE_ONLY_EXTERNAL_ALIASING: SymbolFormatFlags = SymbolFormatFlags(1 << 1);
    // Build symbol name using any nodes needed, instead of just components of an entity name
    pub const ALLOW_ANY_NODE_KIND: SymbolFormatFlags = SymbolFormatFlags(1 << 2);
    // Prefer aliases which are not directly visible
    pub const USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE: SymbolFormatFlags = SymbolFormatFlags(1 << 3);
    // { [E.A]: 1 }
    pub const WRITE_COMPUTED_PROPS: SymbolFormatFlags = SymbolFormatFlags(1 << 4);
    // Skip building an accessible symbol chain
    pub const DO_NOT_INCLUDE_SYMBOL_CHAIN: SymbolFormatFlags = SymbolFormatFlags(1 << 5);
}

impl ExternalEmitHelpers {
    // __rest (used by ESNext object rest transformation)
    pub const REST: ExternalEmitHelpers = ExternalEmitHelpers(1 << 0);
    // __decorate (used by TypeScript decorators transformation)
    pub const DECORATE: ExternalEmitHelpers = ExternalEmitHelpers(1 << 1);
    // __metadata (used by TypeScript decorators transformation)
    pub const METADATA: ExternalEmitHelpers = ExternalEmitHelpers(1 << 2);
    // __param (used by TypeScript decorators transformation)
    pub const PARAM: ExternalEmitHelpers = ExternalEmitHelpers(1 << 3);
    // __awaiter (used by ES2017 async functions transformation)
    pub const AWAITER: ExternalEmitHelpers = ExternalEmitHelpers(1 << 4);
    // __await (used by ES2017 async generator transformation)
    pub const AWAIT: ExternalEmitHelpers = ExternalEmitHelpers(1 << 5);
    // __asyncGenerator (used by ES2017 async generator transformation)
    pub const ASYNC_GENERATOR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 6);
    // __asyncDelegator (used by ES2017 async generator yield* transformation)
    pub const ASYNC_DELEGATOR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 7);
    // __asyncValues (used by ES2017 for..await..of transformation)
    pub const ASYNC_VALUES: ExternalEmitHelpers = ExternalEmitHelpers(1 << 8);
    // __exportStar (used by CommonJS/AMD/UMD module transformation)
    pub const EXPORT_STAR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 9);
    // __importStar (used by CommonJS/AMD/UMD module transformation)
    pub const IMPORT_STAR: ExternalEmitHelpers = ExternalEmitHelpers(1 << 10);
    // __importDefault (used by CommonJS/AMD/UMD module transformation)
    pub const IMPORT_DEFAULT: ExternalEmitHelpers = ExternalEmitHelpers(1 << 11);
    // __makeTemplateObject (used for constructing template string array objects)
    pub const MAKE_TEMPLATE_OBJECT: ExternalEmitHelpers = ExternalEmitHelpers(1 << 12);
    // __classPrivateFieldGet (used by the class private field transformation)
    pub const CLASS_PRIVATE_FIELD_GET: ExternalEmitHelpers = ExternalEmitHelpers(1 << 13);
    // __classPrivateFieldSet (used by the class private field transformation)
    pub const CLASS_PRIVATE_FIELD_SET: ExternalEmitHelpers = ExternalEmitHelpers(1 << 14);
    // __classPrivateFieldIn (used by the class private field transformation)
    pub const CLASS_PRIVATE_FIELD_IN: ExternalEmitHelpers = ExternalEmitHelpers(1 << 15);
    // __setFunctionName (used by class fields and ECMAScript decorators)
    pub const SET_FUNCTION_NAME: ExternalEmitHelpers = ExternalEmitHelpers(1 << 16);
    // __propKey (used by class fields and ECMAScript decorators)
    pub const PROP_KEY: ExternalEmitHelpers = ExternalEmitHelpers(1 << 17);
    // __addDisposableResource and __disposeResources (used by ESNext transformations)
    pub const ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES: ExternalEmitHelpers =
        ExternalEmitHelpers(1 << 18);
    // __rewriteRelativeImportExtension (used by --rewriteRelativeImportExtensions)
    pub const REWRITE_RELATIVE_IMPORT_EXTENSION: ExternalEmitHelpers = ExternalEmitHelpers(1 << 19);

    /// __esDecorate and __runInitializers (used by ECMAScript decorators transformation)
    pub const ES_DECORATE_AND_RUN_INITIALIZERS: ExternalEmitHelpers = Self::DECORATE;

    pub const FIRST_EMIT_HELPER: ExternalEmitHelpers = Self::REST;
    pub const LAST_EMIT_HELPER: ExternalEmitHelpers = Self::REWRITE_RELATIVE_IMPORT_EXTENSION;

    // Helpers included by ES2017 for..await..of
    pub const FOR_AWAIT_OF_INCLUDES: ExternalEmitHelpers = Self::ASYNC_VALUES;

    // Helpers included by ES2017 async generators
    pub const ASYNC_GENERATOR_INCLUDES: ExternalEmitHelpers =
        ExternalEmitHelpers(Self::AWAIT.0 | Self::ASYNC_GENERATOR.0);

    // Helpers included by yield* in ES2017 async generators
    pub const ASYNC_DELEGATOR_INCLUDES: ExternalEmitHelpers = ExternalEmitHelpers(
        Self::AWAIT.0 | Self::ASYNC_DELEGATOR.0 | Self::ASYNC_VALUES.0,
    );
}

/// Go: `const externalHelpersModuleNameText = "tslib"`.
pub const EXTERNAL_HELPERS_MODULE_NAME_TEXT: &str = "tslib";

// Ids

/// Go: `type TypeId uint32` — the numeric type ids ARE the port's ids: index of
/// the type in the owning checker's `Vec<Type>` arena (1-based; the port parks
/// a dummy at index 0 so `types[id.0]` resolves — see checker.rs's PORT note).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TypeId(pub u32);

/// Go: `type SignatureId uint32` — index into the checker's signature arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct SignatureId(pub u32);

/// Go's `*IndexInfo` — index into the checker's index-info arena (the same
/// §5.4 handle idiom as SignatureId; exports.go's GetIndexInfoOf* family
/// hands these to the services layer). TODO(port): the M6 waves that
/// populate `Checker::index_infos` (S8/S10) own the construction paths.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct IndexInfoId(pub u32);

// Links for referenced symbols

/// Go: `type SymbolReferenceLinks struct`.
#[derive(Debug, Default)]
pub struct SymbolReferenceLinks {
    /// Flags for the meanings of the symbol that were referenced
    pub reference_kinds: SymbolFlags,
}

// Links for value symbols

/// Go: `type ValueSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct ValueSymbolLinks {
    /// Type of value symbol
    pub resolved_type: Option<TypeId>,
    pub write_type: Option<TypeId>,
    pub target: Option<SymbolId>,
    pub mapper: Option<TypeMapperId>,
    pub name_type: Option<TypeId>,
    /// Mapped type for mapped type property, containing union or intersection type for synthetic property
    pub containing_type: Option<TypeId>,
    pub function_or_constructor_checked: bool,
}

// Additional links for mapped symbols

/// Go: `type MappedSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct MappedSymbolLinks {
    /// Key type for mapped type member
    pub key_type: Option<TypeId>,
    /// For a property on a mapped or spread type, points back to the original property
    pub synthetic_origin: Option<SymbolId>,
}

// Additional links for deferred type symbols

/// Go: `type DeferredSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct DeferredSymbolLinks {
    /// Source union/intersection of a deferred type
    pub parent: Option<TypeId>,
    /// Calculated list of constituents for a deferred type
    pub constituents: Vec<TypeId>,
    /// Constituents of a deferred `writeType`
    pub write_constituents: Vec<TypeId>,
}

// Links for alias symbols

/// Go: `type AliasSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct AliasSymbolLinks {
    /// Immediate target of an alias. May be another alias. Do not access directly, use `checker.getImmediateAliasedSymbol` instead.
    pub immediate_target: Option<SymbolId>,
    /// Resolved (non-alias) target of an alias
    pub alias_target: Option<SymbolId>,
    /// True if alias symbol has been referenced as a value that can be emitted
    pub referenced: bool,
    /// First resolved alias declaration that makes the symbol only usable in type constructs
    pub type_only_declaration: Option<NodeId>,
}

// Links for module symbols

/// Go: `type ModuleSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct ModuleSymbolLinks {
    /// Resolved exports of module or combined early- and late-bound static members of a class.
    pub resolved_exports: SymbolTable,
    /// Set on a module symbol when some of its exports were resolved through a 'export type * from "mod"' declaration
    pub type_only_export_star_map: Option<FxHashMap<String, NodeId>>,
    pub exports_checked: bool,
    /// PORT: Go distinguishes `resolvedExports == nil` from an empty table;
    /// the port's `SymbolTable` has no nil state, so this marker carries the
    /// "resolved by getExportsOfModule" bit (S6's getExportsOfModule).
    pub resolved_exports_computed: bool,
}

/// Go: `type ReverseMappedSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct ReverseMappedSymbolLinks {
    pub property_type: Option<TypeId>,
    /// References a mapped type
    pub mapped_type: Option<TypeId>,
    /// References an index type
    pub constraint_type: Option<TypeId>,
}

// Links for late-bound symbols

/// Go: `type LateBoundLinks struct`.
#[derive(Debug, Default)]
pub struct LateBoundLinks {
    pub late_symbol: Option<SymbolId>,
}

// Links for export type symbols

/// Go: `type ExportTypeLinks struct`.
#[derive(Debug, Default)]
pub struct ExportTypeLinks {
    /// Target symbol
    pub target: Option<SymbolId>,
    /// Import declaration which produced the symbol, present if the symbol is marked as uncallable but had call signatures in `resolveESModuleSymbol`
    pub originating_import: Option<NodeId>,
}

// Links for type aliases

/// Go: `type TypeAliasLinks struct`.
#[derive(Debug, Default)]
pub struct TypeAliasLinks {
    pub declared_type: Option<TypeId>,
    /// Type parameters of type alias (undefined if non-generic)
    pub type_parameters: Vec<TypeId>,
    /// Instantiations of generic type alias (undefined if non-generic)
    pub instantiations: Option<FxHashMap<CacheHashKey, TypeId>>,
    pub is_constructor_declared_property: bool,
}

// Links for declared types (type parameters, class types, interface types, enums)

/// Go: `type DeclaredTypeLinks struct`.
#[derive(Debug, Default)]
pub struct DeclaredTypeLinks {
    pub declared_type: Option<TypeId>,
    pub interface_checked: bool,
    pub index_signatures_checked: bool,
    pub type_parameters_checked: bool,
    pub enum_checked: bool,
}

// Links for switch clauses

/// Go: `type ExhaustiveState byte` + consts (iota order preserved).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum ExhaustiveState {
    /// Exhaustive state not computed
    #[default]
    Unknown = 0,
    /// Exhaustive state computation in progress
    Computing = 1,
    /// Switch statement is not exhaustive
    False = 2,
    /// Switch statement is exhaustive
    True = 3,
}

/// Go: `type SwitchStatementLinks struct`.
#[derive(Debug, Default)]
pub struct SwitchStatementLinks {
    /// Switch statement exhaustiveness
    pub exhaustive_state: ExhaustiveState,
    pub switch_types_computed: bool,
    pub witnesses_computed: bool,
    pub switch_types: Vec<TypeId>,
    pub witnesses: Vec<String>,
}

/// Go: `type ArrayLiteralLinks struct`.
#[derive(Debug, Default)]
pub struct ArrayLiteralLinks {
    pub indices_computed: bool,
    /// Index of first spread expression (or -1 if none)
    pub first_spread_index: i32,
    /// Index of last spread expression (or -1 if none)
    pub last_spread_index: i32,
}

// Links for late-binding containers

/// Go: `type MembersOrExportsResolutionKind int` + consts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum MembersOrExportsResolutionKind {
    ResolvedExports = 0,
    ResolvedMembers = 1,
}

/// Go: `type MembersAndExportsLinks [2]ast.SymbolTable` — indexed by
/// MembersOrExportsResolutionKind.
#[derive(Debug, Default)]
pub struct MembersAndExportsLinks(pub [SymbolTable; 2]);

// Links for synthetic spread properties

/// Go: `type SpreadLinks struct`.
#[derive(Debug, Default)]
pub struct SpreadLinks {
    /// Left source for synthetic spread property
    pub left_spread: Option<SymbolId>,
    /// Right source for synthetic spread property
    pub right_spread: Option<SymbolId>,
}

// Links for variances of type aliases and interface types

/// Go: `type VarianceLinks struct`.
#[derive(Debug, Default)]
pub struct VarianceLinks {
    pub variances: Vec<VarianceFlags>,
    /// Go: the nil-vs-empty `variances` slice distinction — nil means
    /// "circular variance measurement in progress" (getVariancesWorker's
    /// varianceStack hit returns nil). PORT: the nil slice sentinel doesn't
    /// port; an explicit flag carries the marker.
    pub circular_variances: bool,
}

impl VarianceFlags {
    /// Neither covariant nor contravariant
    pub const INVARIANT: VarianceFlags = VarianceFlags(0);
    /// Covariant
    pub const COVARIANT: VarianceFlags = VarianceFlags(1 << 0);
    /// Contravariant
    pub const CONTRAVARIANT: VarianceFlags = VarianceFlags(1 << 1);
    /// Both covariant and contravariant
    pub const BIVARIANT: VarianceFlags = VarianceFlags(Self::COVARIANT.0 | Self::CONTRAVARIANT.0);
    /// Unwitnessed type parameter
    pub const INDEPENDENT: VarianceFlags = VarianceFlags(1 << 2);
    /// Mask containing all measured variances without the unmeasurable flag
    pub const VARIANCE_MASK: VarianceFlags =
        VarianceFlags(Self::INVARIANT.0 | Self::COVARIANT.0 | Self::CONTRAVARIANT.0 | Self::INDEPENDENT.0);
    /// Variance result is unusable - relationship relies on structural comparisons which are not reflected in generic relationships
    pub const UNMEASURABLE: VarianceFlags = VarianceFlags(1 << 3);
    /// Variance result is unreliable - checking may produce false negatives, but not false positives
    pub const UNRELIABLE: VarianceFlags = VarianceFlags(1 << 4);
    pub const ALLOWS_STRUCTURAL_FALLBACK: VarianceFlags =
        VarianceFlags(Self::UNMEASURABLE.0 | Self::UNRELIABLE.0);
}

/// Go: `func (v VarianceFlags) String() string`.
impl std::fmt::Display for VarianceFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variance = *self & VarianceFlags::VARIANCE_MASK;
        let mut result = match variance {
            VarianceFlags::INVARIANT => "in out",
            VarianceFlags::BIVARIANT => "[bivariant]",
            VarianceFlags::CONTRAVARIANT => "in",
            VarianceFlags::COVARIANT => "out",
            VarianceFlags::INDEPENDENT => "[independent]",
            _ => "",
        }
        .to_string();
        if self.intersects(VarianceFlags::UNMEASURABLE) {
            result += " (unmeasurable)";
        } else if self.intersects(VarianceFlags::UNRELIABLE) {
            result += " (unreliable)";
        }
        f.write_str(&result)
    }
}

/// Go: `type MarkedAssignmentSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct MarkedAssignmentSymbolLinks {
    pub last_assignment_pos: i32,
    /// Symbol is definitely assigned somewhere
    pub has_definite_assignment: bool,
}

/// Go: `type accessibleChainCacheKey struct`.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct AccessibleChainCacheKey {
    pub use_only_external_aliasing: bool,
    pub location: Option<NodeId>,
    pub meaning: SymbolFlags,
}

/// Go: `type ContainingSymbolLinks struct`.
#[derive(Debug, Default)]
pub struct ContainingSymbolLinks {
    /// Symbols of nodes which which logically contain this one, cached by file the request is made within
    pub extended_containers_by_file: Option<FxHashMap<u32, Vec<SymbolId>>>,
    /// Containers (other than the parent) which this symbol is aliased in
    pub extended_containers: Option<Vec<SymbolId>>,
    pub accessible_chain_cache: FxHashMap<AccessibleChainCacheKey, Vec<SymbolId>>,
}

impl AccessFlags {
    pub const INCLUDE_UNDEFINED: AccessFlags = AccessFlags(1 << 0);
    pub const NO_INDEX_SIGNATURES: AccessFlags = AccessFlags(1 << 1);
    pub const WRITING: AccessFlags = AccessFlags(1 << 2);
    pub const CACHE_SYMBOL: AccessFlags = AccessFlags(1 << 3);
    pub const ALLOW_MISSING: AccessFlags = AccessFlags(1 << 4);
    pub const EXPRESSION_POSITION: AccessFlags = AccessFlags(1 << 5);
    pub const REPORT_DEPRECATED: AccessFlags = AccessFlags(1 << 6);
    pub const SUPPRESS_NO_IMPLICIT_ANY_ERROR: AccessFlags = AccessFlags(1 << 7);
    pub const CONTEXTUAL: AccessFlags = AccessFlags(1 << 8);
    pub const PERSISTENT: AccessFlags = Self::INCLUDE_UNDEFINED;
}

impl NodeCheckFlags {
    /// Node has been type checked
    pub const TYPE_CHECKED: NodeCheckFlags = NodeCheckFlags(1 << 0);
    /// Contextual types have been assigned
    pub const CONTEXT_CHECKED: NodeCheckFlags = NodeCheckFlags(1 << 6);
    /// Values for enum members have been computed, and any errors have been reported for them.
    pub const ENUM_VALUES_COMPUTED: NodeCheckFlags = NodeCheckFlags(1 << 10);
    /// Parameter assignments have been marked
    pub const ASSIGNMENTS_MARKED: NodeCheckFlags = NodeCheckFlags(1 << 17);
    /// Marked on all block-scoped containers containing a class with private identifiers.
    pub const CONTAINS_CLASS_WITH_PRIVATE_IDENTIFIERS: NodeCheckFlags = NodeCheckFlags(1 << 20);
    /// Marked on all block-scoped containers containing a static initializer with 'super.x' or 'super[x]'.
    pub const CONTAINS_SUPER_PROPERTY_IN_STATIC_INITIALIZER: NodeCheckFlags = NodeCheckFlags(1 << 21);
    pub const IN_CHECK_IDENTIFIER: NodeCheckFlags = NodeCheckFlags(1 << 22);
    pub const INITIALIZER_IS_UNDEFINED: NodeCheckFlags = NodeCheckFlags(1 << 24);
    pub const INITIALIZER_IS_UNDEFINED_COMPUTED: NodeCheckFlags = NodeCheckFlags(1 << 25);
}

// Common links

/// Go: `type NodeLinks struct`.
#[derive(Debug, Default)]
pub struct NodeLinks {
    /// Set of flags specific to Node
    pub flags: NodeCheckFlags,
    /// Set by `useOuterVariableScopeInParameter` in checker when downlevel emit would change the name resolution scope inside of a parameter.
    pub declaration_requires_scope_change: Tristate,
    /// Cache boolean if we report statements in ambient context
    pub has_reported_statement_in_ambient_context: bool,
}

/// Go: `type SymbolNodeLinks struct`.
#[derive(Debug, Default)]
pub struct SymbolNodeLinks {
    /// Resolved symbol associated with node
    pub resolved_symbol: Option<SymbolId>,
}

/// Go: `type TypeNodeLinks struct`.
#[derive(Debug, Default)]
pub struct TypeNodeLinks {
    /// Resolved type associated with node
    pub resolved_type: Option<TypeId>,
    /// Outer type parameters of anonymous object type
    pub outer_type_parameters: Vec<TypeId>,
}

/// Go: `type ComputedNameNodeLinks struct`.
#[derive(Debug, Default)]
pub struct ComputedNameNodeLinks {
    /// If the node has a computable name
    pub has_name: Option<bool>,
    /// Resolved name associated with the type of the node
    pub name: String,
}

// Links for enum members

/// Go: `type EnumMemberLinks struct` — `value evaluator.Result`.
/// TODO(port) M6: the `tsc-evaluator` crate port replaces
/// [`crate::todo_shapes::EvaluatorResult`].
#[derive(Debug, Default)]
pub struct EnumMemberLinks {
    /// Constant value of enum member
    pub value: EvaluatorResult,
}

// Links for assertion expressions

/// Go: `type AssertionLinks struct`.
#[derive(Debug, Default)]
pub struct AssertionLinks {
    /// Assertion expression type
    pub expr_type: Option<TypeId>,
}

// SourceFile links

/// Go: `type SourceFileLinks struct`.
#[derive(Debug, Default)]
pub struct SourceFileLinks {
    pub type_checked: bool,
    pub unused_checked: bool,
    pub external_helpers_module: Option<SymbolId>,
    pub requested_external_emit_helpers: ExternalEmitHelpers,
    pub deferred_nodes: OrderedSet<NodeId>,
    pub identifier_check_nodes: Vec<NodeId>,
    pub local_jsx_namespace: String,
    pub local_jsx_fragment_namespace: String,
    pub local_jsx_factory: Option<NodeId>,
    pub local_jsx_fragment_factory: Option<NodeId>,
    pub jsx_fragment_type: Option<TypeId>,
}

// Signature specific links

/// Go: `type SignatureLinks struct`.
#[derive(Debug, Default)]
pub struct SignatureLinks {
    /// Cached signature of signature node or call expression
    pub resolved_signature: Option<SignatureId>,
    /// Signature with possible control flow effects
    pub effects_signature: Option<SignatureId>,
    /// Signature for decorator as if invoked by the runtime
    pub decorator_signature: Option<SignatureId>,
}

impl TypeFlags {
    pub const ANY: TypeFlags = TypeFlags(1 << 0);
    pub const UNKNOWN: TypeFlags = TypeFlags(1 << 1);
    pub const UNDEFINED: TypeFlags = TypeFlags(1 << 2);
    pub const NULL: TypeFlags = TypeFlags(1 << 3);
    pub const VOID: TypeFlags = TypeFlags(1 << 4);
    pub const STRING: TypeFlags = TypeFlags(1 << 5);
    pub const NUMBER: TypeFlags = TypeFlags(1 << 6);
    pub const BIGINT: TypeFlags = TypeFlags(1 << 7);
    pub const BOOLEAN: TypeFlags = TypeFlags(1 << 8);
    /// Type of symbol primitive introduced in ES6
    pub const ESSYMBOL: TypeFlags = TypeFlags(1 << 9);
    pub const STRING_LITERAL: TypeFlags = TypeFlags(1 << 10);
    pub const NUMBER_LITERAL: TypeFlags = TypeFlags(1 << 11);
    pub const BIGINT_LITERAL: TypeFlags = TypeFlags(1 << 12);
    pub const BOOLEAN_LITERAL: TypeFlags = TypeFlags(1 << 13);
    /// unique symbol
    pub const UNIQUE_ESSYMBOL: TypeFlags = TypeFlags(1 << 14);
    /// Always combined with StringLiteral, NumberLiteral, or Union
    pub const ENUM_LITERAL: TypeFlags = TypeFlags(1 << 15);
    /// Numeric computed enum member value (must be right after EnumLiteral, see getSortOrderFlags)
    pub const ENUM: TypeFlags = TypeFlags(1 << 16);
    /// intrinsic object type
    pub const NON_PRIMITIVE: TypeFlags = TypeFlags(1 << 17);
    /// Never type
    pub const NEVER: TypeFlags = TypeFlags(1 << 18);
    /// Type parameter
    pub const TYPE_PARAMETER: TypeFlags = TypeFlags(1 << 19);
    /// Object type
    pub const OBJECT: TypeFlags = TypeFlags(1 << 20);
    /// keyof T
    pub const INDEX: TypeFlags = TypeFlags(1 << 21);
    /// Template literal type
    pub const TEMPLATE_LITERAL: TypeFlags = TypeFlags(1 << 22);
    /// Uppercase/Lowercase type
    pub const STRING_MAPPING: TypeFlags = TypeFlags(1 << 23);
    /// Type parameter substitution
    pub const SUBSTITUTION: TypeFlags = TypeFlags(1 << 24);
    /// T[K]
    pub const INDEXED_ACCESS: TypeFlags = TypeFlags(1 << 25);
    /// T extends U ? X : Y
    pub const CONDITIONAL: TypeFlags = TypeFlags(1 << 26);
    /// Union (T | U)
    pub const UNION: TypeFlags = TypeFlags(1 << 27);
    /// Intersection (T & U)
    pub const INTERSECTION: TypeFlags = TypeFlags(1 << 28);
    /// Used by union/intersection type construction
    pub const RESERVED1: TypeFlags = TypeFlags(1 << 29);
    /// Used by union/intersection type construction
    pub const RESERVED2: TypeFlags = TypeFlags(1 << 30);
    pub const RESERVED3: TypeFlags = TypeFlags(1 << 31);

    pub const ANY_OR_UNKNOWN: TypeFlags = TypeFlags(Self::ANY.0 | Self::UNKNOWN.0);
    pub const NULLABLE: TypeFlags = TypeFlags(Self::UNDEFINED.0 | Self::NULL.0);
    pub const LITERAL: TypeFlags =
        TypeFlags(Self::STRING_LITERAL.0 | Self::NUMBER_LITERAL.0 | Self::BIGINT_LITERAL.0 | Self::BOOLEAN_LITERAL.0);
    pub const UNIT: TypeFlags =
        TypeFlags(Self::ENUM.0 | Self::LITERAL.0 | Self::UNIQUE_ESSYMBOL.0 | Self::NULLABLE.0);
    pub const FRESHABLE: TypeFlags = TypeFlags(Self::ENUM.0 | Self::LITERAL.0);
    pub const STRING_OR_NUMBER_LITERAL: TypeFlags =
        TypeFlags(Self::STRING_LITERAL.0 | Self::NUMBER_LITERAL.0);
    pub const STRING_OR_NUMBER_LITERAL_OR_UNIQUE: TypeFlags =
        TypeFlags(Self::STRING_LITERAL.0 | Self::NUMBER_LITERAL.0 | Self::UNIQUE_ESSYMBOL.0);
    pub const DEFINITELY_FALSY: TypeFlags = TypeFlags(
        Self::STRING_LITERAL.0
            | Self::NUMBER_LITERAL.0
            | Self::BIGINT_LITERAL.0
            | Self::BOOLEAN_LITERAL.0
            | Self::VOID.0
            | Self::UNDEFINED.0
            | Self::NULL.0,
    );
    pub const POSSIBLY_FALSY: TypeFlags = TypeFlags(
        Self::DEFINITELY_FALSY.0 | Self::STRING.0 | Self::NUMBER.0 | Self::BIGINT.0 | Self::BOOLEAN.0,
    );
    pub const INTRINSIC: TypeFlags = TypeFlags(
        Self::ANY.0
            | Self::UNKNOWN.0
            | Self::STRING.0
            | Self::NUMBER.0
            | Self::BIGINT.0
            | Self::ESSYMBOL.0
            | Self::VOID.0
            | Self::UNDEFINED.0
            | Self::NULL.0
            | Self::NEVER.0
            | Self::NON_PRIMITIVE.0,
    );
    pub const STRING_LIKE: TypeFlags =
        TypeFlags(Self::STRING.0 | Self::STRING_LITERAL.0 | Self::TEMPLATE_LITERAL.0 | Self::STRING_MAPPING.0);
    pub const NUMBER_LIKE: TypeFlags = TypeFlags(Self::NUMBER.0 | Self::NUMBER_LITERAL.0 | Self::ENUM.0);
    pub const BIGINT_LIKE: TypeFlags = TypeFlags(Self::BIGINT.0 | Self::BIGINT_LITERAL.0);
    pub const BOOLEAN_LIKE: TypeFlags = TypeFlags(Self::BOOLEAN.0 | Self::BOOLEAN_LITERAL.0);
    pub const ENUM_LIKE: TypeFlags = TypeFlags(Self::ENUM.0 | Self::ENUM_LITERAL.0);
    pub const ESSYMBOL_LIKE: TypeFlags = TypeFlags(Self::ESSYMBOL.0 | Self::UNIQUE_ESSYMBOL.0);
    pub const VOID_LIKE: TypeFlags = TypeFlags(Self::VOID.0 | Self::UNDEFINED.0);
    pub const PRIMITIVE: TypeFlags = TypeFlags(
        Self::STRING_LIKE.0
            | Self::NUMBER_LIKE.0
            | Self::BIGINT_LIKE.0
            | Self::BOOLEAN_LIKE.0
            | Self::ENUM_LIKE.0
            | Self::ESSYMBOL_LIKE.0
            | Self::VOID_LIKE.0
            | Self::NULL.0,
    );
    pub const DEFINITELY_NON_NULLABLE: TypeFlags = TypeFlags(
        Self::STRING_LIKE.0
            | Self::NUMBER_LIKE.0
            | Self::BIGINT_LIKE.0
            | Self::BOOLEAN_LIKE.0
            | Self::ENUM_LIKE.0
            | Self::ESSYMBOL_LIKE.0
            | Self::OBJECT.0
            | Self::NON_PRIMITIVE.0,
    );
    pub const DISJOINT_DOMAINS: TypeFlags = TypeFlags(
        Self::NON_PRIMITIVE.0
            | Self::STRING_LIKE.0
            | Self::NUMBER_LIKE.0
            | Self::BIGINT_LIKE.0
            | Self::BOOLEAN_LIKE.0
            | Self::ESSYMBOL_LIKE.0
            | Self::VOID_LIKE.0
            | Self::NULL.0,
    );
    pub const UNION_OR_INTERSECTION: TypeFlags = TypeFlags(Self::UNION.0 | Self::INTERSECTION.0);
    pub const STRUCTURED_TYPE: TypeFlags = TypeFlags(Self::OBJECT.0 | Self::UNION.0 | Self::INTERSECTION.0);
    pub const TYPE_VARIABLE: TypeFlags = TypeFlags(Self::TYPE_PARAMETER.0 | Self::INDEXED_ACCESS.0);
    pub const INSTANTIABLE_NON_PRIMITIVE: TypeFlags =
        TypeFlags(Self::TYPE_VARIABLE.0 | Self::CONDITIONAL.0 | Self::SUBSTITUTION.0);
    pub const INSTANTIABLE_PRIMITIVE: TypeFlags =
        TypeFlags(Self::INDEX.0 | Self::TEMPLATE_LITERAL.0 | Self::STRING_MAPPING.0);
    pub const INSTANTIABLE: TypeFlags = TypeFlags(Self::INSTANTIABLE_NON_PRIMITIVE.0 | Self::INSTANTIABLE_PRIMITIVE.0);
    pub const STRUCTURED_OR_INSTANTIABLE: TypeFlags =
        TypeFlags(Self::STRUCTURED_TYPE.0 | Self::INSTANTIABLE.0);
    pub const OBJECT_FLAGS_TYPE: TypeFlags = TypeFlags(
        Self::ANY.0 | Self::NULLABLE.0 | Self::NEVER.0 | Self::OBJECT.0 | Self::UNION.0 | Self::INTERSECTION.0,
    );
    pub const SIMPLIFIABLE: TypeFlags = TypeFlags(Self::INDEXED_ACCESS.0 | Self::CONDITIONAL.0 | Self::INDEX.0);
    pub const SINGLETON: TypeFlags = TypeFlags(
        Self::ANY.0
            | Self::UNKNOWN.0
            | Self::STRING.0
            | Self::NUMBER.0
            | Self::BOOLEAN.0
            | Self::BIGINT.0
            | Self::ESSYMBOL.0
            | Self::VOID.0
            | Self::UNDEFINED.0
            | Self::NULL.0
            | Self::NEVER.0
            | Self::NON_PRIMITIVE.0,
    );
    /// 'TypeFlagsNarrowable' types are types where narrowing actually narrows.
    /// This *should* be every type other than null, undefined, void, and never
    pub const NARROWABLE: TypeFlags = TypeFlags(
        Self::ANY.0
            | Self::UNKNOWN.0
            | Self::STRUCTURED_OR_INSTANTIABLE.0
            | Self::STRING_LIKE.0
            | Self::NUMBER_LIKE.0
            | Self::BIGINT_LIKE.0
            | Self::BOOLEAN_LIKE.0
            | Self::ESSYMBOL.0
            | Self::UNIQUE_ESSYMBOL.0
            | Self::NON_PRIMITIVE.0,
    );
    // The following flags are aggregated during union and intersection type construction
    pub const INCLUDES_MASK: TypeFlags = TypeFlags(
        Self::ANY.0
            | Self::UNKNOWN.0
            | Self::PRIMITIVE.0
            | Self::NEVER.0
            | Self::OBJECT.0
            | Self::UNION.0
            | Self::INTERSECTION.0
            | Self::NON_PRIMITIVE.0
            | Self::TEMPLATE_LITERAL.0
            | Self::STRING_MAPPING.0,
    );
    // The following flags are used for different purposes during union and intersection type construction
    pub const INCLUDES_MISSING_TYPE: TypeFlags = Self::TYPE_PARAMETER;
    pub const INCLUDES_NON_WIDENING_TYPE: TypeFlags = Self::INDEX;
    pub const INCLUDES_WILDCARD: TypeFlags = Self::INDEXED_ACCESS;
    pub const INCLUDES_EMPTY_OBJECT: TypeFlags = Self::CONDITIONAL;
    pub const INCLUDES_INSTANTIABLE: TypeFlags = Self::SUBSTITUTION;
    pub const INCLUDES_CONSTRAINED_TYPE_VARIABLE: TypeFlags = Self::RESERVED1;
    pub const INCLUDES_ERROR: TypeFlags = Self::RESERVED2;
    pub const NOT_PRIMITIVE_UNION: TypeFlags = TypeFlags(
        Self::ANY.0
            | Self::UNKNOWN.0
            | Self::VOID.0
            | Self::NEVER.0
            | Self::OBJECT.0
            | Self::INTERSECTION.0
            | Self::INCLUDES_INSTANTIABLE.0,
    );
}

/// Go: `var typeFlagNames = [...]struct { flag TypeFlags; name string }`.
const TYPE_FLAG_NAMES: [(TypeFlags, &str); 29] = [
    (TypeFlags::ANY, "Any"),
    (TypeFlags::UNKNOWN, "Unknown"),
    (TypeFlags::UNDEFINED, "Undefined"),
    (TypeFlags::NULL, "Null"),
    (TypeFlags::VOID, "Void"),
    (TypeFlags::STRING, "String"),
    (TypeFlags::NUMBER, "Number"),
    (TypeFlags::BIGINT, "BigInt"),
    (TypeFlags::BOOLEAN, "Boolean"),
    (TypeFlags::ESSYMBOL, "ESSymbol"),
    (TypeFlags::STRING_LITERAL, "StringLiteral"),
    (TypeFlags::NUMBER_LITERAL, "NumberLiteral"),
    (TypeFlags::BIGINT_LITERAL, "BigIntLiteral"),
    (TypeFlags::BOOLEAN_LITERAL, "BooleanLiteral"),
    (TypeFlags::UNIQUE_ESSYMBOL, "UniqueESSymbol"),
    (TypeFlags::ENUM_LITERAL, "EnumLiteral"),
    (TypeFlags::ENUM, "Enum"),
    (TypeFlags::NON_PRIMITIVE, "NonPrimitive"),
    (TypeFlags::NEVER, "Never"),
    (TypeFlags::TYPE_PARAMETER, "TypeParameter"),
    (TypeFlags::OBJECT, "Object"),
    (TypeFlags::INDEX, "Index"),
    (TypeFlags::TEMPLATE_LITERAL, "TemplateLiteral"),
    (TypeFlags::STRING_MAPPING, "StringMapping"),
    (TypeFlags::SUBSTITUTION, "Substitution"),
    (TypeFlags::INDEXED_ACCESS, "IndexedAccess"),
    (TypeFlags::CONDITIONAL, "Conditional"),
    (TypeFlags::UNION, "Union"),
    (TypeFlags::INTERSECTION, "Intersection"),
];

/// Go: `func FormatTypeFlags(flags TypeFlags) []string` — returns the
/// individual flag names as a slice of strings.
pub fn format_type_flags(flags: TypeFlags) -> Vec<&'static str> {
    let mut result = Vec::with_capacity(flags.0.count_ones() as usize);
    for &(flag, name) in &TYPE_FLAG_NAMES {
        if flags.intersects(flag) {
            result.push(name);
        }
    }
    if result.is_empty() {
        result.push("None");
    }
    result
}

/// Go: `func (f TypeFlags) String() string` — a pipe-separated string of flag
/// names.
impl std::fmt::Display for TypeFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = format_type_flags(*self);
        let mut first = true;
        for name in names {
            if !first {
                f.write_char('|')?;
            }
            first = false;
            f.write_str(name)?;
        }
        Ok(())
    }
}

// Note that for types of different kinds, the numeric values of TypeFlags determine the order
// computed by the CompareTypes function and therefore the order of constituent types in union types.
// Since union type processing often bails out early when a result is known, it is important to order
// TypeFlags in increasing order of potential type complexity. In particular, indexed access and
// conditional types should sort last as those types are potentially recursive and possibly infinite.

impl ObjectFlags {
    /// Class
    pub const CLASS: ObjectFlags = ObjectFlags(1 << 0);
    /// Interface
    pub const INTERFACE: ObjectFlags = ObjectFlags(1 << 1);
    /// Generic type reference
    pub const REFERENCE: ObjectFlags = ObjectFlags(1 << 2);
    /// Synthesized generic tuple type
    pub const TUPLE: ObjectFlags = ObjectFlags(1 << 3);
    /// Anonymous
    pub const ANONYMOUS: ObjectFlags = ObjectFlags(1 << 4);
    /// Mapped
    pub const MAPPED: ObjectFlags = ObjectFlags(1 << 5);
    /// Instantiated anonymous or mapped type
    pub const INSTANTIATED: ObjectFlags = ObjectFlags(1 << 6);
    /// Originates in an object literal
    pub const OBJECT_LITERAL: ObjectFlags = ObjectFlags(1 << 7);
    /// Evolving array type
    pub const EVOLVING_ARRAY: ObjectFlags = ObjectFlags(1 << 8);
    /// Object literal pattern with computed properties
    pub const OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES: ObjectFlags = ObjectFlags(1 << 9);
    /// Object contains a property from a reverse-mapped type
    pub const REVERSE_MAPPED: ObjectFlags = ObjectFlags(1 << 10);
    /// Jsx attributes type
    pub const JSX_ATTRIBUTES: ObjectFlags = ObjectFlags(1 << 11);
    /// Object type declared in JS - disables errors on read/write of nonexisting members
    pub const JS_LITERAL: ObjectFlags = ObjectFlags(1 << 12);
    /// Fresh object literal
    pub const FRESH_LITERAL: ObjectFlags = ObjectFlags(1 << 13);
    /// Originates in an array literal
    pub const ARRAY_LITERAL: ObjectFlags = ObjectFlags(1 << 14);
    /// Union of only primitive types
    pub const PRIMITIVE_UNION: ObjectFlags = ObjectFlags(1 << 15);
    /// Type is or contains undefined or null widening type
    pub const CONTAINS_WIDENING_TYPE: ObjectFlags = ObjectFlags(1 << 16);
    /// Type is or contains object literal type
    pub const CONTAINS_OBJECT_OR_ARRAY_LITERAL: ObjectFlags = ObjectFlags(1 << 17);
    /// Type is or contains anyFunctionType or silentNeverType
    pub const NON_INFERRABLE_TYPE: ObjectFlags = ObjectFlags(1 << 18);
    /// CouldContainTypeVariables flag has been computed
    pub const COULD_CONTAIN_TYPE_VARIABLES_COMPUTED: ObjectFlags = ObjectFlags(1 << 19);
    /// Type could contain a type variable
    pub const COULD_CONTAIN_TYPE_VARIABLES: ObjectFlags = ObjectFlags(1 << 20);
    /// Members have been resolved
    pub const MEMBERS_RESOLVED: ObjectFlags = ObjectFlags(1 << 21);

    pub const CLASS_OR_INTERFACE: ObjectFlags = ObjectFlags(Self::CLASS.0 | Self::INTERFACE.0);
    pub const REQUIRES_WIDENING: ObjectFlags =
        ObjectFlags(Self::CONTAINS_WIDENING_TYPE.0 | Self::CONTAINS_OBJECT_OR_ARRAY_LITERAL.0);
    pub const PROPAGATING_FLAGS: ObjectFlags = ObjectFlags(
        Self::CONTAINS_WIDENING_TYPE.0 | Self::CONTAINS_OBJECT_OR_ARRAY_LITERAL.0 | Self::NON_INFERRABLE_TYPE.0,
    );
    pub const INSTANTIATED_MAPPED: ObjectFlags = ObjectFlags(Self::MAPPED.0 | Self::INSTANTIATED.0);
    // Object flags that uniquely identify the kind of ObjectType
    pub const OBJECT_TYPE_KIND_MASK: ObjectFlags = ObjectFlags(
        Self::CLASS_OR_INTERFACE.0
            | Self::REFERENCE.0
            | Self::TUPLE.0
            | Self::ANONYMOUS.0
            | Self::MAPPED.0
            | Self::REVERSE_MAPPED.0
            | Self::EVOLVING_ARRAY.0
            | Self::INSTANTIATION_EXPRESSION_TYPE.0
            | Self::SINGLE_SIGNATURE_TYPE.0,
    );
    // Flags that require TypeFlags.Object
    /// Object literal contains spread operation
    pub const CONTAINS_SPREAD: ObjectFlags = ObjectFlags(1 << 22);
    /// Originates in object rest declaration
    pub const OBJECT_REST_TYPE: ObjectFlags = ObjectFlags(1 << 23);
    /// Originates in instantiation expression
    pub const INSTANTIATION_EXPRESSION_TYPE: ObjectFlags = ObjectFlags(1 << 24);
    /// A single signature type extracted from a potentially broader type
    pub const SINGLE_SIGNATURE_TYPE: ObjectFlags = ObjectFlags(1 << 25);
    /// Type is a clone of a class instance type
    pub const IS_CLASS_INSTANCE_CLONE: ObjectFlags = ObjectFlags(1 << 26);
    // Flags that require TypeFlags.Object and ObjectFlags.Reference
    /// has had `getSingleBaseForNonAugmentingSubtype` invoked on it already
    pub const IDENTICAL_BASE_TYPE_CALCULATED: ObjectFlags = ObjectFlags(1 << 27);
    /// has a defined cachedEquivalentBaseType member
    pub const IDENTICAL_BASE_TYPE_EXISTS: ObjectFlags = ObjectFlags(1 << 28);
    /// Originates in resolution of AST type node
    pub const FROM_TYPE_NODE: ObjectFlags = ObjectFlags(1 << 29);
    // Flags that require TypeFlags.UnionOrIntersection or TypeFlags.Substitution
    /// IsGenericObjectType flag has been computed
    pub const IS_GENERIC_TYPE_COMPUTED: ObjectFlags = ObjectFlags(1 << 22);
    /// Union or intersection contains generic object type
    pub const IS_GENERIC_OBJECT_TYPE: ObjectFlags = ObjectFlags(1 << 23);
    /// Union or intersection contains generic index type
    pub const IS_GENERIC_INDEX_TYPE: ObjectFlags = ObjectFlags(1 << 24);
    pub const IS_GENERIC_TYPE: ObjectFlags = ObjectFlags(Self::IS_GENERIC_OBJECT_TYPE.0 | Self::IS_GENERIC_INDEX_TYPE.0);
    // Flags that require TypeFlags.Union
    /// Union contains intersections
    pub const CONTAINS_INTERSECTIONS: ObjectFlags = ObjectFlags(1 << 25);
    /// IsUnknownLikeUnion flag has been computed
    pub const IS_UNKNOWN_LIKE_UNION_COMPUTED: ObjectFlags = ObjectFlags(1 << 26);
    /// Union of null, undefined, and empty object type
    pub const IS_UNKNOWN_LIKE_UNION: ObjectFlags = ObjectFlags(1 << 27);
    /// IsUniformEnum flag has been computed
    pub const IS_UNIFORM_ENUM_COMPUTED: ObjectFlags = ObjectFlags(1 << 28);
    /// Union contains uniform literal types
    pub const IS_UNIFORM_ENUM: ObjectFlags = ObjectFlags(1 << 29);
    // Flags that require TypeFlags.Intersection
    /// IsNeverLike flag has been computed
    pub const IS_NEVER_INTERSECTION_COMPUTED: ObjectFlags = ObjectFlags(1 << 25);
    /// Intersection reduces to never
    pub const IS_NEVER_INTERSECTION: ObjectFlags = ObjectFlags(1 << 26);
    /// T & C, where T's constraint and C are primitives, object, or {}
    pub const IS_CONSTRAINED_TYPE_VARIABLE: ObjectFlags = ObjectFlags(1 << 27);
}

// TypeAlias

/// Go: `type TypeAlias struct` — Go heap-allocates alias records and stores the
/// pointer on Type; the port stores `Option<Box<TypeAlias>>` (nil → None; the
/// nil-tolerant accessors below mirror Go's nil-receiver methods).
#[derive(Debug, Default, Clone)]
pub struct TypeAlias {
    pub symbol: Option<SymbolId>,
    pub type_arguments: Vec<TypeId>,
}

impl TypeAlias {
    /// Go: `func (a *TypeAlias) Symbol() *ast.Symbol` — nil-tolerant.
    pub fn symbol(&self) -> Option<SymbolId> {
        self.symbol
    }

    /// Go: `func (a *TypeAlias) TypeArguments() []*Type` — nil-tolerant.
    pub fn type_arguments(&self) -> &[TypeId] {
        &self.type_arguments
    }
}

// Type

/// Go: `type Type struct { flags; objectFlags; id; symbol; alias; checker; data }`.
///
/// PORT (SPEC §5.4): Go's `checker *Checker` back-pointer is replaced by
/// `checker_shard: u16` — the id of the pool checker that created the type.
/// Code that read `t.checker` takes the checker as an explicit parameter (see
/// `compare_types` in utilities.rs); the shard id still distinguishes types
/// across checkers (Go: `t1.checker != t2.checker` panics in CompareTypes).
///
/// PORT (layout): `data TypeData` is an enum with boxed payloads — Go's
/// interface pointer becomes an owned box, so the arena slot stays 48 bytes and
/// every payload is separately allocated exactly like Go's heap-allocated
/// TypeData impls. `TypeBase` (the embedded `Type` + the `AsType` identity)
/// disappears: the slot IS the type; payloads carry only the per-kind fields.
#[derive(Debug)]
pub struct Type {
    pub flags: TypeFlags,
    pub object_flags: ObjectFlags,
    pub id: TypeId,
    pub symbol: Option<SymbolId>,
    pub alias: Option<Box<TypeAlias>>,
    /// PORT: replaces Go's `checker *Checker` (SPEC §5.4).
    pub checker_shard: u16,
    /// Type specific data
    pub data: TypeData,
}

impl Type {
    /// Go: `func (t *Type) Id() TypeId`.
    pub fn id(&self) -> TypeId {
        self.id
    }

    /// Go: `func (t *Type) Flags() TypeFlags`.
    pub fn flags(&self) -> TypeFlags {
        self.flags
    }

    /// Go: `func (t *Type) ObjectFlags() ObjectFlags`.
    pub fn object_flags(&self) -> ObjectFlags {
        self.object_flags
    }

    // Casts for concrete struct types
    //
    // Go: `return t.data.(*XType)` — a checked interface cast that panics on
    // mismatch. The port returns `Option<&XType>`; call sites that relied on
    // Go's cast panicking use `.expect(...)`/`unwrap()` (the ast crate's
    // `as_source_file` precedent), call sites guarded by flag checks use
    // `if let`/`?`.

    /// Go: `func (t *Type) AsIntrinsicType() *IntrinsicType`.
    pub fn as_intrinsic_type(&self) -> Option<&IntrinsicType> {
        match &self.data {
            TypeData::Intrinsic(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsLiteralType() *LiteralType`.
    pub fn as_literal_type(&self) -> Option<&LiteralType> {
        match &self.data {
            TypeData::Literal(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_literal_type`] — Go's pointer write sites
    /// (`t.AsLiteralType().freshType = f`) become `&mut` through the arena.
    pub fn as_literal_type_mut(&mut self) -> Option<&mut LiteralType> {
        match &mut self.data {
            TypeData::Literal(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_intrinsic_type`].
    pub fn as_intrinsic_type_mut(&mut self) -> Option<&mut IntrinsicType> {
        match &mut self.data {
            TypeData::Intrinsic(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_type_parameter`] (W2: the relater's
    /// synthetic-constraint write on cloned type parameters).
    pub fn as_type_parameter_mut(&mut self) -> Option<&mut TypeParameter> {
        match &mut self.data {
            TypeData::TypeParameter(d) => Some(d),
            _ => None,
        }
    }
    /// Mutable form of [`Type::as_interface_type`] (M6 S2: the
    /// getDeclaredTypeOfClassOrInterface thisType/instantiations writes).
    pub fn as_interface_type_mut(&mut self) -> Option<&mut InterfaceType> {
        match &mut self.data {
            TypeData::Interface(d) => Some(d),
            _ => None,
        }
    }
    /// Mutable form of [`Type::as_conditional_type`] (M6 S2: the
    /// distributive-constraint write).
    pub fn as_conditional_type_mut(&mut self) -> Option<&mut ConditionalType> {
        match &mut self.data {
            TypeData::Conditional(d) => Some(d),
            _ => None,
        }
    }
    /// Mutable form of [`Type::as_object_type`] (M6 S2: the binding-pattern
    /// index-info writes).
    pub fn as_object_type_mut(&mut self) -> Option<&mut ObjectType> {
        match &mut self.data {
            TypeData::Object(d) => Some(d),
            TypeData::TypeReference(d) => Some(&mut d.base),
            TypeData::Interface(d) => Some(&mut d.base.base),
            TypeData::Tuple(d) => Some(&mut d.base.base.base),
            TypeData::InstantiationExpression(d) => Some(&mut d.base),
            TypeData::Mapped(d) => Some(&mut d.base),
            TypeData::ReverseMapped(d) => Some(&mut d.base),
            TypeData::EvolvingArray(d) => Some(&mut d.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsUniqueESSymbolType() *UniqueESSymbolType`.
    pub fn as_unique_essymbol_type(&self) -> Option<&UniqueESSymbolType> {
        match &self.data {
            TypeData::UniqueESSymbol(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsTupleType() *TupleType`.
    pub fn as_tuple_type(&self) -> Option<&TupleType> {
        match &self.data {
            TypeData::Tuple(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsInstantiationExpressionType() *InstantiationExpressionType`.
    pub fn as_instantiation_expression_type(&self) -> Option<&InstantiationExpressionType> {
        match &self.data {
            TypeData::InstantiationExpression(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_instantiation_expression_type`] (M6 S7).
    pub fn as_instantiation_expression_type_mut(&mut self) -> Option<&mut InstantiationExpressionType> {
        match &mut self.data {
            TypeData::InstantiationExpression(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsMappedType() *MappedType`.
    pub fn as_mapped_type(&self) -> Option<&MappedType> {
        match &self.data {
            TypeData::Mapped(d) => Some(d),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_mapped_type`] (M6 S7: the mapped-type
    /// instantiation's constraint/typeParameter writes).
    pub fn as_mapped_type_mut(&mut self) -> Option<&mut MappedType> {
        match &mut self.data {
            TypeData::Mapped(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsReverseMappedType() *ReverseMappedType`.
    pub fn as_reverse_mapped_type(&self) -> Option<&ReverseMappedType> {
        match &self.data {
            TypeData::ReverseMapped(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsEvolvingArrayType() *EvolvingArrayType`.
    pub fn as_evolving_array_type(&self) -> Option<&EvolvingArrayType> {
        match &self.data {
            TypeData::EvolvingArray(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsTypeParameter() *TypeParameter`.
    pub fn as_type_parameter(&self) -> Option<&TypeParameter> {
        match &self.data {
            TypeData::TypeParameter(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsUnionType() *UnionType`.
    pub fn as_union_type(&self) -> Option<&UnionType> {
        match &self.data {
            TypeData::Union(d) => Some(d),
            _ => None,
        }
    }
    /// Mutable form of [`Type::as_union_type`] (M6 S7: the union origin and
    /// regularType writes in the union-construction engine).
    pub fn as_union_type_mut(&mut self) -> Option<&mut UnionType> {
        match &mut self.data {
            TypeData::Union(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsIntersectionType() *IntersectionType`.
    /// Mutable form of [`Type::as_intersection_type`] (M6 S5: the
    /// uniqueLiteralFilledInstantiation memoization in isReducibleIntersection).
    pub fn as_intersection_type_mut(&mut self) -> Option<&mut IntersectionType> {
        match &mut self.data {
            TypeData::Intersection(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_intersection_type(&self) -> Option<&IntersectionType> {
        match &self.data {
            TypeData::Intersection(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsIndexType() *IndexType`.
    pub fn as_index_type(&self) -> Option<&IndexType> {
        match &self.data {
            TypeData::Index(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsIndexedAccessType() *IndexedAccessType`.
    pub fn as_indexed_access_type(&self) -> Option<&IndexedAccessType> {
        match &self.data {
            TypeData::IndexedAccess(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsTemplateLiteralType() *TemplateLiteralType`.
    pub fn as_template_literal_type(&self) -> Option<&TemplateLiteralType> {
        match &self.data {
            TypeData::TemplateLiteral(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsStringMappingType() *StringMappingType`.
    pub fn as_string_mapping_type(&self) -> Option<&StringMappingType> {
        match &self.data {
            TypeData::StringMapping(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsSubstitutionType() *SubstitutionType`.
    pub fn as_substitution_type(&self) -> Option<&SubstitutionType> {
        match &self.data {
            TypeData::Substitution(d) => Some(d),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsConditionalType() *ConditionalType`.
    pub fn as_conditional_type(&self) -> Option<&ConditionalType> {
        match &self.data {
            TypeData::Conditional(d) => Some(d),
            _ => None,
        }
    }

    // Casts for embedded struct types — succeed for the same variant families
    // as Go's embedding chains (see the layout PORT note at the top).

    /// Go: `func (t *Type) AsConstrainedType() *ConstrainedType`.
    pub fn as_constrained_type(&self) -> Option<&ConstrainedType> {
        match &self.data {
            TypeData::Intrinsic(_) | TypeData::Literal(_) | TypeData::UniqueESSymbol(_) => None,
            TypeData::Object(d) => Some(&d.base.base),
            TypeData::TypeReference(d) => Some(&d.base.base.base),
            TypeData::Interface(d) => Some(&d.base.base.base.base),
            TypeData::Tuple(d) => Some(&d.base.base.base.base.base),
            TypeData::InstantiationExpression(d) => Some(&d.base.base.base),
            TypeData::Mapped(d) => Some(&d.base.base.base),
            TypeData::ReverseMapped(d) => Some(&d.base.base.base),
            TypeData::EvolvingArray(d) => Some(&d.base.base.base),
            TypeData::Union(d) => Some(&d.base.base.base),
            TypeData::Intersection(d) => Some(&d.base.base.base),
            TypeData::TypeParameter(d) => Some(&d.base),
            TypeData::Index(d) => Some(&d.base),
            TypeData::IndexedAccess(d) => Some(&d.base),
            TypeData::TemplateLiteral(d) => Some(&d.base),
            TypeData::StringMapping(d) => Some(&d.base),
            TypeData::Substitution(d) => Some(&d.base),
            TypeData::Conditional(d) => Some(&d.base),
        }
    }

    /// Go: `func (t *Type) AsStructuredType() *StructuredType`.
    pub fn as_structured_type(&self) -> Option<&StructuredType> {
        match &self.data {
            TypeData::Object(d) => Some(&d.base),
            TypeData::TypeReference(d) => Some(&d.base.base),
            TypeData::Interface(d) => Some(&d.base.base.base),
            TypeData::Tuple(d) => Some(&d.base.base.base.base),
            TypeData::InstantiationExpression(d) => Some(&d.base.base),
            TypeData::Mapped(d) => Some(&d.base.base),
            TypeData::ReverseMapped(d) => Some(&d.base.base),
            TypeData::EvolvingArray(d) => Some(&d.base.base),
            TypeData::Union(d) => Some(&d.base.base),
            TypeData::Intersection(d) => Some(&d.base.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsObjectType() *ObjectType`.
    pub fn as_object_type(&self) -> Option<&ObjectType> {
        match &self.data {
            TypeData::Object(d) => Some(d),
            TypeData::TypeReference(d) => Some(&d.base),
            TypeData::Interface(d) => Some(&d.base.base),
            TypeData::Tuple(d) => Some(&d.base.base.base),
            TypeData::InstantiationExpression(d) => Some(&d.base),
            TypeData::Mapped(d) => Some(&d.base),
            TypeData::ReverseMapped(d) => Some(&d.base),
            TypeData::EvolvingArray(d) => Some(&d.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsTypeReference() *TypeReference`.
    pub fn as_type_reference(&self) -> Option<&TypeReference> {
        match &self.data {
            TypeData::TypeReference(d) => Some(d),
            TypeData::Interface(d) => Some(&d.base),
            TypeData::Tuple(d) => Some(&d.base.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsInterfaceType() *InterfaceType`.
    pub fn as_interface_type(&self) -> Option<&InterfaceType> {
        match &self.data {
            TypeData::Interface(d) => Some(d),
            TypeData::Tuple(d) => Some(&d.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) AsUnionOrIntersectionType() *UnionOrIntersectionType`.
    pub fn as_union_or_intersection_type(&self) -> Option<&UnionOrIntersectionType> {
        match &self.data {
            TypeData::Union(d) => Some(&d.base),
            TypeData::Intersection(d) => Some(&d.base),
            _ => None,
        }
    }

    /// Mutable form of [`Type::as_union_or_intersection_type`] (M6 S7).
    pub fn as_union_or_intersection_type_mut(&mut self) -> Option<&mut UnionOrIntersectionType> {
        match &mut self.data {
            TypeData::Union(d) => Some(&mut d.base),
            TypeData::Intersection(d) => Some(&mut d.base),
            _ => None,
        }
    }

    /// Go: `func (t *Type) Distributed() []*Type`.
    pub fn distributed(&self) -> Vec<TypeId> {
        if self.flags.intersects(TypeFlags::UNION) {
            return self.as_union_type().expect("UnionType data").base.types.clone();
        }
        if self.flags.intersects(TypeFlags::NEVER) {
            return Vec::new();
        }
        vec![self.id]
    }

    // Common accessors

    /// Go: `func (t *Type) Target() *Type` — panics on unhandled kinds (Go
    /// panics too). Go's fifth case (`Object && Mapped`) is unreachable — the
    /// first case subsumes it — and is mirrored as dead code below.
    pub fn target(&self) -> Option<TypeId> {
        if self.flags.intersects(TypeFlags::OBJECT) {
            return self.as_object_type().expect("ObjectType data").target;
        }
        if self.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.as_type_parameter().expect("TypeParameter data").target;
        }
        if self.flags.intersects(TypeFlags::INDEX) {
            return self.as_index_type().expect("IndexType data").target;
        }
        if self.flags.intersects(TypeFlags::STRING_MAPPING) {
            return self.as_string_mapping_type().expect("StringMappingType data").target;
        }
        // Dead in Go too (the Object case above returns first); kept verbatim
        // for grep-parity with the Go switch.
        if self.flags.intersects(TypeFlags::OBJECT) && self.object_flags.intersects(ObjectFlags::MAPPED) {
            return self.as_mapped_type().expect("MappedType data").base.target;
        }
        panic!("Unhandled case in Type.Target");
    }

    /// Go: `func (t *Type) Mapper() *TypeMapper` — panics on unhandled kinds.
    /// TODO(port) M6: mapper.go owns the `TypeMapper` table behind
    /// [`TypeMapperId`].
    pub fn mapper(&self) -> Option<TypeMapperId> {
        if self.flags.intersects(TypeFlags::OBJECT) {
            return self.as_object_type().expect("ObjectType data").mapper;
        }
        if self.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.as_type_parameter().expect("TypeParameter data").mapper;
        }
        if self.flags.intersects(TypeFlags::CONDITIONAL) {
            return self.as_conditional_type().expect("ConditionalType data").mapper;
        }
        panic!("Unhandled case in Type.Mapper");
    }

    /// Go: `func (t *Type) Types() []*Type` — panics on unhandled kinds.
    pub fn types(&self) -> &[TypeId] {
        if self.flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            return &self.as_union_or_intersection_type().expect("UnionOrIntersectionType data").types;
        }
        if self.flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return &self.as_template_literal_type().expect("TemplateLiteralType data").types;
        }
        panic!("Unhandled case in Type.Types");
    }

    /// Go: `func (t *Type) TargetInterfaceType() *InterfaceType` — the port
    /// returns the target handle; the payload resolves through the checker
    /// arena (`checker.type_of(id).as_interface_type()`).
    pub fn target_interface_type(&self) -> TypeId {
        self.as_type_reference().expect("TypeReference data").base.target.expect("interface target")
    }

    /// Go: `func (t *Type) TargetTupleType() *TupleType` (same PORT shape as
    /// `target_interface_type`).
    pub fn target_tuple_type(&self) -> TypeId {
        self.as_type_reference().expect("TypeReference data").base.target.expect("tuple target")
    }

    /// Go: `func (t *Type) Symbol() *ast.Symbol`.
    pub fn symbol(&self) -> Option<SymbolId> {
        self.symbol
    }

    /// Go: `func (t *Type) Alias() *TypeAlias`.
    pub fn alias(&self) -> Option<&TypeAlias> {
        self.alias.as_deref()
    }

    /// Go: `func (t *Type) IsUnion() bool`.
    pub fn is_union(&self) -> bool {
        self.flags.intersects(TypeFlags::UNION)
    }

    /// Go: `func (t *Type) IsString() bool`.
    pub fn is_string(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING)
    }

    /// Go: `func (t *Type) IsIntersection() bool`.
    pub fn is_intersection(&self) -> bool {
        self.flags.intersects(TypeFlags::INTERSECTION)
    }

    /// Go: `func (t *Type) IsStringLiteral() bool`.
    pub fn is_string_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING_LITERAL)
    }

    /// Go: `func (t *Type) IsNumberLiteral() bool`.
    pub fn is_number_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::NUMBER_LITERAL)
    }

    /// Go: `func (t *Type) IsBigIntLiteral() bool`.
    pub fn is_bigint_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::BIGINT_LITERAL)
    }

    /// Go: `func (t *Type) IsEnumLiteral() bool`.
    pub fn is_enum_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::ENUM_LITERAL)
    }

    /// Go: `func (t *Type) IsBooleanLike() bool`.
    pub fn is_boolean_like(&self) -> bool {
        self.flags.intersects(TypeFlags::BOOLEAN_LIKE)
    }

    /// Go: `func (t *Type) IsStringLike() bool`.
    pub fn is_string_like(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING_LIKE)
    }

    /// Go: `func (t *Type) IsClass() bool`.
    pub fn is_class(&self) -> bool {
        self.object_flags.intersects(ObjectFlags::CLASS)
    }

    /// Go: `func (t *Type) IsTypeParameter() bool`.
    pub fn is_type_parameter(&self) -> bool {
        self.flags.intersects(TypeFlags::TYPE_PARAMETER)
    }

    /// Go: `func (t *Type) IsIndex() bool`.
    pub fn is_index(&self) -> bool {
        self.flags.intersects(TypeFlags::INDEX)
    }

    /// Go: `func (t *Type) IsTupleType() bool` — the flags half of the
    /// checker.go `isTupleType` (REFERENCE && target.TUPLE). The target's
    /// flags need the arena (§5.4), so the full check lives in
    /// [`crate::Checker::is_tuple_type`]; nothing in the spike calls this.
    #[allow(dead_code)]
    pub fn is_tuple_type(&self) -> bool {
        self.object_flags.intersects(ObjectFlags::REFERENCE)
    }
}

// TypeData

/// Go: `type TypeData interface` + the ~20 concrete impls. Variants are the
/// CONCRETE Go structs (the abstract embedded bases — ConstrainedType,
/// StructuredType, ObjectType-family sharing — port as nested `base` fields so
/// the embedded-cast chain succeeds, see the layout PORT note at the top).
/// Every payload is boxed (Go heap-allocates each impl).
#[derive(Debug)]
pub enum TypeData {
    Intrinsic(Box<IntrinsicType>),
    Literal(Box<LiteralType>),
    UniqueESSymbol(Box<UniqueESSymbolType>),
    Object(Box<ObjectType>),
    TypeReference(Box<TypeReference>),
    Interface(Box<InterfaceType>),
    Tuple(Box<TupleType>),
    InstantiationExpression(Box<InstantiationExpressionType>),
    Mapped(Box<MappedType>),
    ReverseMapped(Box<ReverseMappedType>),
    EvolvingArray(Box<EvolvingArrayType>),
    Union(Box<UnionType>),
    Intersection(Box<IntersectionType>),
    TypeParameter(Box<TypeParameter>),
    Index(Box<IndexType>),
    IndexedAccess(Box<IndexedAccessType>),
    TemplateLiteral(Box<TemplateLiteralType>),
    StringMapping(Box<StringMappingType>),
    Substitution(Box<SubstitutionType>),
    Conditional(Box<ConditionalType>),
}

// IntrinsicTypeData

/// Go: `type IntrinsicType struct { TypeBase; intrinsicName string }`.
#[derive(Debug, Default)]
pub struct IntrinsicType {
    pub intrinsic_name: String,
}

impl IntrinsicType {
    /// Go: `func (t *IntrinsicType) IntrinsicName() string`.
    pub fn intrinsic_name(&self) -> &str {
        &self.intrinsic_name
    }
}

// LiteralTypeData

/// Go: `value any // string | jsnum.Number | bool | PseudoBigInt | nil (computed enum)`.
#[derive(Clone, Debug, PartialEq)]
pub enum LiteralValue {
    String(String),
    Number(tsc_jsnum::Number),
    Boolean(bool),
    BigInt(tsc_jsnum::PseudoBigInt),
    /// Go's `nil` — the value of a computed-enum-member literal type.
    None,
}

impl LiteralValue {
    /// Map-key form: the number case canonicalizes `-0.0` to `0.0` so the key
    /// matches Go's IEEE `==` map-key semantics (`-0.0 == 0.0` is one key in
    /// Go; NaN never reaches here — it is cached separately, mirroring Go's
    /// `getNumberLiteralType` NaN comment).
    pub fn number_key(&self) -> u64 {
        match self {
            LiteralValue::Number(n) => {
                let f = n.0;
                if f == 0.0 {
                    0
                } else {
                    f.to_bits()
                }
            }
            _ => panic!("number_key on non-number literal value"),
        }
    }
}

/// Go: `type LiteralType struct { TypeBase; value; freshType *Type; regularType *Type }`.
#[derive(Debug, Default)]
pub struct LiteralType {
    /// string | jsnum.Number | bool | PseudoBigInt | nil (computed enum)
    pub value: Option<LiteralValue>,
    /// Fresh version of type
    pub fresh_type: Option<TypeId>,
    /// Regular version of type
    pub regular_type: Option<TypeId>,
}

impl LiteralType {
    /// Go: `func (t *LiteralType) Value() any` (nil → None via the value
    /// Option; the payload's inner None mirrors Go's nil `any`).
    pub fn value(&self) -> Option<&LiteralValue> {
        self.value.as_ref()
    }

    /// Go: `func (t *LiteralType) FreshType() *Type`.
    pub fn fresh_type(&self) -> Option<TypeId> {
        self.fresh_type
    }

    /// Go: `func (t *LiteralType) RegularType() *Type`.
    pub fn regular_type(&self) -> Option<TypeId> {
        self.regular_type
    }
}

/// Go: `func (t *LiteralType) String() string` — `ValueToString(t.value)`.
impl std::fmt::Display for LiteralType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.value {
            Some(v) => f.write_str(&crate::utilities::value_to_string(v)),
            // Go's ValueToString panics on unhandled types; the computed-enum
            // nil case reaches it too. Mirror with the same panic.
            None => panic!("unhandled value type in valueToString"),
        }
    }
}

// UniqueESSymbolTypeData

/// Go: `type UniqueESSymbolType struct { TypeBase; name string }`.
#[derive(Debug, Default)]
pub struct UniqueESSymbolType {
    pub name: String,
}

// ConstrainedType (type with computed base constraint)

/// Go: `type ConstrainedType struct { TypeBase; resolvedBaseConstraint *Type }`.
#[derive(Debug, Default)]
pub struct ConstrainedType {
    pub resolved_base_constraint: Option<TypeId>,
}

// StructuredType (base of all types with members)

/// Go: `type StructuredType struct { ConstrainedType; members; properties; signatures; callSignatureCount; indexInfos; objectTypeWithoutAbstractConstructSignatures }`.
#[derive(Debug, Default)]
pub struct StructuredType {
    pub base: ConstrainedType,
    pub members: SymbolTable,
    pub properties: Vec<SymbolId>,
    /// Signatures (call + construct)
    pub signatures: Vec<SignatureId>,
    /// Count of call signatures
    pub call_signature_count: i32,
    pub index_infos: Vec<Box<IndexInfo>>,
    pub object_type_without_abstract_construct_signatures: Option<TypeId>,
}

impl StructuredType {
    /// Go: `func (t *StructuredType) CallSignatures() []*Signature` — the port
    /// returns the signature-id slice (handles resolve through the checker's
    /// signature arena).
    pub fn call_signatures(&self) -> &[SignatureId] {
        &self.signatures[..self.call_signature_count as usize]
    }

    /// Go: `func (t *StructuredType) ConstructSignatures() []*Signature`.
    pub fn construct_signatures(&self) -> &[SignatureId] {
        &self.signatures[self.call_signature_count as usize..]
    }

    /// Go: `func (t *StructuredType) Properties() []*ast.Symbol`.
    pub fn properties(&self) -> &[SymbolId] {
        &self.properties
    }
}

// Except for tuple type references and reverse mapped types, all object types have an associated symbol.
// Possible object type instances are listed in the following.
//
// InterfaceType:
// ObjectFlagsClass: Originating non-generic class type
// ObjectFlagsClass|ObjectFlagsReference: Originating generic class type
// ObjectFlagsInterface: Originating non-generic interface type
// ObjectFlagsInterface|ObjectFlagsReference: Originating generic interface type
//
// TupleType:
// ObjectFlagsReference|ObjectFlagsTuple: Originating generic tuple type (synthesized)
//
// TypeReference
// ObjectFlagsReference: Instantiated generic class, interface, or tuple type
//
// ObjectType:
// ObjectFlagsAnonymous: Originating anonymous object type
// ObjectFlagsAnonymous|ObjectFlagsInstantiated: Instantiated anonymous object type
//
// MappedType:
// ObjectFlagsMapped: Originating mapped type
// ObjectFlagsMapped|ObjectFlagsInstantiated: Instantiated mapped type
//
// InstantiationExpressionType:
// ObjectFlagsAnonymous|ObjectFlagsInstantiationExpression: Originating instantiation expression type
// ObjectFlagsAnonymous|ObjectFlagsInstantiated|ObjectFlagsInstantiationExpression: Instantiated instantiation expression type
//
// ReverseMappedType:
// ObjectFlagsAnonymous|ObjectFlagsReverseMapped: Reverse mapped type
//
// EvolvingArrayType:
// ObjectFlagsEvolvingArray: Evolving array type

/// Go: `type ObjectType struct { StructuredType; target *Type; mapper *TypeMapper; instantiations map[CacheHashKey]*Type }`.
#[derive(Debug, Default)]
pub struct ObjectType {
    pub base: StructuredType,
    /// Target of instantiated type
    pub target: Option<TypeId>,
    /// Type mapper for instantiated type
    pub mapper: Option<TypeMapperId>,
    /// Map of type instantiations
    pub instantiations: Option<FxHashMap<CacheHashKey, TypeId>>,
}

// TypeReference (instantiation of an InterfaceType)

/// Go: `type TypeReference struct { ObjectType; node *ast.Node; resolvedTypeArguments []*Type }` —
/// `node` is a TypeReferenceNode | ArrayTypeNode | TupleTypeNode when deferred,
/// else nil.
#[derive(Debug, Default)]
pub struct TypeReference {
    pub base: ObjectType,
    pub node: Option<NodeId>,
    pub resolved_type_arguments: Vec<TypeId>,
}

// InterfaceType (when generic, serves as reference to instantiation of itself)

/// Go: `type InterfaceType struct { TypeReference; allTypeParameters; outerTypeParameterCount; thisType; baseTypesResolved; declaredMembersResolved; resolvedBaseConstructorType; resolvedBaseTypes; declaredMembers; declaredCallSignatures; declaredConstructSignatures; declaredIndexInfos }`.
#[derive(Debug, Default)]
pub struct InterfaceType {
    pub base: TypeReference,
    /// Type parameters (outer + local + thisType)
    pub all_type_parameters: Vec<TypeId>,
    /// Count of outer type parameters
    pub outer_type_parameter_count: i32,
    /// The "this" type (nil if none)
    pub this_type: Option<TypeId>,
    pub base_types_resolved: bool,
    pub declared_members_resolved: bool,
    pub resolved_base_constructor_type: Option<TypeId>,
    pub resolved_base_types: Vec<TypeId>,
    /// Declared members
    pub declared_members: SymbolTable,
    /// Declared call signatures
    pub declared_call_signatures: Vec<SignatureId>,
    /// Declared construct signatures
    pub declared_construct_signatures: Vec<SignatureId>,
    /// Declared index signatures
    pub declared_index_infos: Vec<Box<IndexInfo>>,
}

impl InterfaceType {
    /// Go: `func (t *InterfaceType) OuterTypeParameters() []*Type`.
    pub fn outer_type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters[..self.outer_type_parameter_count as usize]
    }

    /// Go: `func (t *InterfaceType) LocalTypeParameters() []*Type`.
    pub fn local_type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters[self.outer_type_parameter_count as usize..self.all_type_parameters.len() - 1]
    }

    /// Go: `func (t *InterfaceType) TypeParameters() []*Type`.
    pub fn type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters[..self.all_type_parameters.len() - 1]
    }

    /// Go: `func (t *InterfaceType) ThisType() *Type`.
    pub fn this_type(&self) -> Option<TypeId> {
        self.this_type
    }
}

// TupleType

impl ElementFlags {
    /// T
    pub const REQUIRED: ElementFlags = ElementFlags(1 << 0);
    /// T?
    pub const OPTIONAL: ElementFlags = ElementFlags(1 << 1);
    /// ...T[]
    pub const REST: ElementFlags = ElementFlags(1 << 2);
    /// ...T
    pub const VARIADIC: ElementFlags = ElementFlags(1 << 3);
    pub const FIXED: ElementFlags = ElementFlags(Self::REQUIRED.0 | Self::OPTIONAL.0);
    pub const VARIABLE: ElementFlags = ElementFlags(Self::REST.0 | Self::VARIADIC.0);
    pub const NON_REQUIRED: ElementFlags =
        ElementFlags(Self::OPTIONAL.0 | Self::REST.0 | Self::VARIADIC.0);
    pub const NON_REST: ElementFlags = ElementFlags(Self::REQUIRED.0 | Self::OPTIONAL.0 | Self::VARIADIC.0);
}

/// Go: `type TupleElementInfo struct { flags ElementFlags; labeledDeclaration *ast.Node }`.
#[derive(Debug, Default, Clone)]
pub struct TupleElementInfo {
    pub flags: ElementFlags,
    /// NamedTupleMember | ParameterDeclaration | nil
    pub labeled_declaration: Option<NodeId>,
}

impl TupleElementInfo {
    /// Go: `func (t *TupleElementInfo) TupleElementFlags() ElementFlags`.
    pub fn tuple_element_flags(&self) -> ElementFlags {
        self.flags
    }

    /// Go: `func (t *TupleElementInfo) LabeledDeclaration() *ast.Node`.
    pub fn labeled_declaration(&self) -> Option<NodeId> {
        self.labeled_declaration
    }
}

/// Go: `type TupleType struct { InterfaceType; elementInfos; minLength; fixedLength; combinedFlags; readonly }`.
#[derive(Debug, Default)]
pub struct TupleType {
    pub base: InterfaceType,
    pub element_infos: Vec<TupleElementInfo>,
    /// Number of required or variadic elements
    pub min_length: i32,
    /// Number of initial required or optional elements
    pub fixed_length: i32,
    pub combined_flags: ElementFlags,
    pub readonly: bool,
}

impl TupleType {
    /// Go: `func (t *TupleType) FixedLength() int`.
    pub fn fixed_length(&self) -> i32 {
        self.fixed_length
    }

    /// Go: `func (t *TupleType) IsReadonly() bool`.
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Go: `func (t *TupleType) ElementFlags() []ElementFlags`.
    pub fn element_flags(&self) -> Vec<ElementFlags> {
        self.element_infos.iter().map(|info| info.flags).collect()
    }

    /// Go: `func (t *TupleType) ElementInfos() []TupleElementInfo`.
    pub fn element_infos(&self) -> &[TupleElementInfo] {
        &self.element_infos
    }
}

// InstantiationExpressionType

/// Go: `type InstantiationExpressionType struct { ObjectType; node *ast.Node }`.
#[derive(Debug, Default)]
pub struct InstantiationExpressionType {
    pub base: ObjectType,
    pub node: Option<NodeId>,
}

// MappedType

/// Go: `type MappedType struct { ObjectType; declaration; typeParameter; constraintType; nameType; templateType; modifiersType; resolvedApparentType; containsError }`.
#[derive(Debug, Default)]
pub struct MappedType {
    pub base: ObjectType,
    /// Go: `*ast.MappedTypeNode` — a NodeId with that kind.
    pub declaration: Option<NodeId>,
    pub type_parameter: Option<TypeId>,
    pub constraint_type: Option<TypeId>,
    pub name_type: Option<TypeId>,
    pub template_type: Option<TypeId>,
    pub modifiers_type: Option<TypeId>,
    pub resolved_apparent_type: Option<TypeId>,
    pub contains_error: bool,
}

impl MappedType {
    /// Go: `func (t *MappedType) TypeParameter() *Type`.
    pub fn type_parameter(&self) -> Option<TypeId> {
        self.type_parameter
    }

    /// Go: `func (t *MappedType) ConstraintType() *Type`.
    pub fn constraint_type(&self) -> Option<TypeId> {
        self.constraint_type
    }

    /// Go: `func (t *MappedType) NameType() *Type`.
    pub fn name_type(&self) -> Option<TypeId> {
        self.name_type
    }

    /// Go: `func (t *MappedType) TemplateType() *Type`.
    pub fn template_type(&self) -> Option<TypeId> {
        self.template_type
    }

    /// Go: `func (t *MappedType) ResolveComponents(c *Checker, typ *Type)` —
    /// the first Go site that passes the checker alongside the type, the shape
    /// the §5.4 strategy generalizes. TODO(port) M6: the
    /// get*FromMappedType family (mapped-type resolution wave).
    pub fn resolve_components(&self, _c: &mut crate::Checker, _typ: TypeId) {
        panic!("TODO(port) M6 mapped-type wave: MappedType.ResolveComponents (checker.go get*FromMappedType family)")
    }
}

// ReverseMappedType

/// Go: `type ReverseMappedType struct { ObjectType; source *Type; mappedType *Type; constraintType *Type }`.
#[derive(Debug, Default)]
pub struct ReverseMappedType {
    pub base: ObjectType,
    pub source: Option<TypeId>,
    pub mapped_type: Option<TypeId>,
    pub constraint_type: Option<TypeId>,
}

// EvolvingArrayType

/// Go: `type EvolvingArrayType struct { ObjectType; elementType *Type; finalArrayType *Type }`.
#[derive(Debug, Default)]
pub struct EvolvingArrayType {
    pub base: ObjectType,
    pub element_type: Option<TypeId>,
    pub final_array_type: Option<TypeId>,
}

// UnionOrIntersectionTypeData

/// Go: `type UnionOrIntersectionType struct { StructuredType; types; propertyCache; propertyCacheWithoutFunctionPropertyAugment; resolvedProperties }`.
#[derive(Debug, Default)]
pub struct UnionOrIntersectionType {
    pub base: StructuredType,
    pub types: Vec<TypeId>,
    pub property_cache: SymbolTable,
    pub property_cache_without_function_property_augment: SymbolTable,
    pub resolved_properties: Vec<SymbolId>,
}

/// Go: `func (t *UnionOrIntersectionType) Types() []*Type`.
impl UnionOrIntersectionType {
    pub fn types(&self) -> &[TypeId] {
        &self.types
    }
}

// UnionType

/// Go: `type UnionType struct { UnionOrIntersectionType; resolvedReducedType; regularType; origin; keyPropertyName; constituentMap }`.
#[derive(Debug, Default)]
pub struct UnionType {
    pub base: UnionOrIntersectionType,
    pub resolved_reduced_type: Option<TypeId>,
    pub regular_type: Option<TypeId>,
    /// Denormalized union, intersection, or index type in which union originates
    pub origin: Option<TypeId>,
    /// Property with unique unit type that exists in every object/intersection in union type
    pub key_property_name: Option<String>,
    /// Constituents keyed by unit type discriminants
    pub constituent_map: FxHashMap<TypeId, TypeId>,
}

// IntersectionType

/// Go: `type IntersectionType struct { UnionOrIntersectionType; resolvedApparentType; uniqueLiteralFilledInstantiation }`.
#[derive(Debug, Default)]
pub struct IntersectionType {
    pub base: UnionOrIntersectionType,
    pub resolved_apparent_type: Option<TypeId>,
    /// Instantiation with type parameters mapped to never type
    pub unique_literal_filled_instantiation: Option<TypeId>,
}

// TypeParameter

/// Go: `type TypeParameter struct { ConstrainedType; constraint; target; mapper; isThisType; isDistributed; resolvedDefaultType; distributedType }`.
#[derive(Debug, Default)]
pub struct TypeParameter {
    pub base: ConstrainedType,
    pub constraint: Option<TypeId>,
    pub target: Option<TypeId>,
    pub mapper: Option<TypeMapperId>,
    pub is_this_type: bool,
    pub is_distributed: bool,
    pub resolved_default_type: Option<TypeId>,
    pub distributed_type: Option<TypeId>,
}

impl TypeParameter {
    /// Go: `func (t *TypeParameter) IsThisType() bool`.
    pub fn is_this_type(&self) -> bool {
        self.is_this_type
    }
}

// IndexFlags

impl IndexFlags {
    pub const STRINGS_ONLY: IndexFlags = IndexFlags(1 << 0);
    pub const NO_INDEX_SIGNATURES: IndexFlags = IndexFlags(1 << 1);
    pub const NO_REDUCIBLE_CHECK: IndexFlags = IndexFlags(1 << 2);
}

// IndexType

/// Go: `type IndexType struct { ConstrainedType; target *Type; indexFlags IndexFlags }`.
#[derive(Debug, Default)]
pub struct IndexType {
    pub base: ConstrainedType,
    pub target: Option<TypeId>,
    pub index_flags: IndexFlags,
}

impl IndexType {
    /// Go: `func (t *IndexType) Target() *Type`.
    pub fn target(&self) -> Option<TypeId> {
        self.target
    }
}

// IndexedAccessType

/// Go: `type IndexedAccessType struct { ConstrainedType; objectType *Type; indexType *Type; accessFlags AccessFlags }`.
#[derive(Debug, Default)]
pub struct IndexedAccessType {
    pub base: ConstrainedType,
    pub object_type: Option<TypeId>,
    pub index_type: Option<TypeId>,
    /// Only includes AccessFlags.Persistent
    pub access_flags: AccessFlags,
}

impl IndexedAccessType {
    /// Go: `func (t *IndexedAccessType) ObjectType() *Type`.
    pub fn object_type(&self) -> Option<TypeId> {
        self.object_type
    }

    /// Go: `func (t *IndexedAccessType) IndexType() *Type`.
    pub fn index_type(&self) -> Option<TypeId> {
        self.index_type
    }
}

/// Go: `type TemplateLiteralType struct { ConstrainedType; texts []string; types []*Type }`.
#[derive(Debug, Default)]
pub struct TemplateLiteralType {
    pub base: ConstrainedType,
    /// Always one element longer than types
    pub texts: Vec<String>,
    /// Always at least one element
    pub types: Vec<TypeId>,
}

impl TemplateLiteralType {
    /// Go: `func (t *TemplateLiteralType) Texts() []string`.
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    /// Go: `func (t *TemplateLiteralType) Types() []*Type`.
    pub fn types(&self) -> &[TypeId] {
        &self.types
    }
}

/// Go: `type StringMappingType struct { ConstrainedType; target *Type }`.
#[derive(Debug, Default)]
pub struct StringMappingType {
    pub base: ConstrainedType,
    pub target: Option<TypeId>,
}

impl StringMappingType {
    /// Go: `func (t *StringMappingType) Target() *Type`.
    pub fn target(&self) -> Option<TypeId> {
        self.target
    }
}

/// Go: `type SubstitutionType struct { ConstrainedType; baseType *Type; constraint *Type }`.
#[derive(Debug, Default)]
pub struct SubstitutionType {
    pub base: ConstrainedType,
    /// Target type
    pub base_type: Option<TypeId>,
    /// Constraint that target type is known to satisfy
    pub constraint: Option<TypeId>,
}

impl SubstitutionType {
    /// Go: `func (t *SubstitutionType) BaseType() *Type`.
    pub fn base_type(&self) -> Option<TypeId> {
        self.base_type
    }

    /// Go: `func (t *SubstitutionType) SubstConstraint() *Type`.
    pub fn subst_constraint(&self) -> Option<TypeId> {
        self.constraint
    }
}

/// Go: `type ConditionalRoot struct`.
#[derive(Debug, Default, Clone)]
pub struct ConditionalRoot {
    /// Go: `*ast.ConditionalTypeNode`
    pub node: Option<NodeId>,
    pub check_type: Option<TypeId>,
    pub extends_type: Option<TypeId>,
    pub is_distributive: bool,
    pub infer_type_parameters: Vec<TypeId>,
    pub outer_type_parameters: Vec<TypeId>,
    pub instantiations: Option<FxHashMap<CacheHashKey, TypeId>>,
    pub alias: Option<Box<TypeAlias>>,
}

/// Go: `type ConditionalType struct`.
#[derive(Debug, Default)]
pub struct ConditionalType {
    pub base: ConstrainedType,
    pub root: Option<Box<ConditionalRoot>>,
    pub check_type: Option<TypeId>,
    pub extends_type: Option<TypeId>,
    pub resolved_true_type: Option<TypeId>,
    pub resolved_false_type: Option<TypeId>,
    /// The `trueType` instantiated with the `combinedMapper`, if present
    pub resolved_inferred_true_type: Option<TypeId>,
    pub resolved_default_constraint: Option<TypeId>,
    pub resolved_constraint_of_distributive: Option<TypeId>,
    pub mapper: Option<TypeMapperId>,
    pub combined_mapper: Option<TypeMapperId>,
}

impl ConditionalType {
    /// Go: `func (t *ConditionalType) CheckType() *Type`.
    pub fn check_type(&self) -> Option<TypeId> {
        self.check_type
    }

    /// Go: `func (t *ConditionalType) ExtendsType() *Type`.
    pub fn extends_type(&self) -> Option<TypeId> {
        self.extends_type
    }
}

// SignatureFlags

impl SignatureFlags {
    // Propagating flags
    /// Indicates last parameter is rest parameter
    pub const HAS_REST_PARAMETER: SignatureFlags = SignatureFlags(1 << 0);
    /// Indicates signature is specialized
    pub const HAS_LITERAL_TYPES: SignatureFlags = SignatureFlags(1 << 1);
    /// Indicates signature is a construct signature
    pub const CONSTRUCT: SignatureFlags = SignatureFlags(1 << 2);
    /// Indicates signature comes from an abstract class, abstract construct signature, or abstract constructor type
    pub const ABSTRACT: SignatureFlags = SignatureFlags(1 << 3);
    // Non-propagating flags
    /// Indicates signature comes from a CallChain nested in an outer OptionalChain
    pub const IS_INNER_CALL_CHAIN: SignatureFlags = SignatureFlags(1 << 4);
    /// Indicates signature comes from a CallChain that is the outermost chain of an optional expression
    pub const IS_OUTER_CALL_CHAIN: SignatureFlags = SignatureFlags(1 << 5);
    /// Indicates signature is from a js file and has no types
    pub const IS_UNTYPED_SIGNATURE_IN_JS_FILE: SignatureFlags = SignatureFlags(1 << 6);
    /// Indicates signature comes from a non-inferrable type
    pub const IS_NON_INFERRABLE: SignatureFlags = SignatureFlags(1 << 7);
    pub const IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE: SignatureFlags = SignatureFlags(1 << 8);
    // We do not propagate `IsInnerCallChain` or `IsOuterCallChain` to instantiated signatures, as that would result in us
    // attempting to add `| undefined` on each recursive call to `getReturnTypeOfSignature` when
    // instantiating the return type.
    pub const PROPAGATING_FLAGS: SignatureFlags = SignatureFlags(
        Self::HAS_REST_PARAMETER.0
            | Self::HAS_LITERAL_TYPES.0
            | Self::CONSTRUCT.0
            | Self::ABSTRACT.0
            | Self::IS_UNTYPED_SIGNATURE_IN_JS_FILE.0
            | Self::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE.0,
    );
    pub const CALL_CHAIN_FLAGS: SignatureFlags =
        SignatureFlags(Self::IS_INNER_CALL_CHAIN.0 | Self::IS_OUTER_CALL_CHAIN.0);
}

// Signature

/// Go: `type Signature struct` — stored in the checker's signature arena
/// (`SignatureId` indexes it; Go: `core.Arena[Signature]`, the port a plain
/// `Vec<Signature>` — see checker.rs's PORT note on arenas).
#[derive(Debug, Default, Clone)]
pub struct Signature {
    pub id: SignatureId,
    pub flags: SignatureFlags,
    pub min_argument_count: i32,
    pub resolved_min_argument_count: i32,
    pub declaration: Option<NodeId>,
    pub type_parameters: Vec<TypeId>,
    pub parameters: Vec<SymbolId>,
    pub this_parameter: Option<SymbolId>,
    pub resolved_return_type: Option<TypeId>,
    pub resolved_type_predicate: Option<Box<TypePredicate>>,
    pub target: Option<SignatureId>,
    pub mapper: Option<TypeMapperId>,
    pub isolated_signature_type: Option<TypeId>,
    pub composite: Option<Box<CompositeSignature>>,
    /// Go: the `noTypePredicate` sentinel — a non-nil `*TypePredicate` in
    /// `resolvedTypePredicate` meaning "explicitly no predicate" (the
    /// union/intersection predicate combiner sets it). PORT: the nil-pointer
    /// sentinel doesn't port; an explicit flag carries the marker.
    pub no_type_predicate: bool,
}

impl Signature {
    /// Go: `func (s *Signature) Id() SignatureId`.
    pub fn id(&self) -> SignatureId {
        self.id
    }

    /// Go: `func (s *Signature) Flags() SignatureFlags`.
    pub fn flags(&self) -> SignatureFlags {
        self.flags
    }

    /// Go: `func (s *Signature) TypeParameters() []*Type` — TypeIds into the
    /// owning checker's type arena.
    pub fn type_parameters(&self) -> &[TypeId] {
        &self.type_parameters
    }

    /// Go: `func (s *Signature) Declaration() *ast.Node`.
    pub fn declaration(&self) -> Option<NodeId> {
        self.declaration
    }

    /// Go: `func (s *Signature) Target() *Signature`.
    pub fn target(&self) -> Option<SignatureId> {
        self.target
    }

    /// Go: `func (s *Signature) ThisParameter() *ast.Symbol`.
    pub fn this_parameter(&self) -> Option<SymbolId> {
        self.this_parameter
    }

    /// Go: `func (s *Signature) Parameters() []*ast.Symbol`.
    pub fn parameters(&self) -> &[SymbolId] {
        &self.parameters
    }

    /// Go: `func (s *Signature) HasRestParameter() bool`.
    pub fn has_rest_parameter(&self) -> bool {
        self.flags.intersects(SignatureFlags::HAS_REST_PARAMETER)
    }

    /// Go: `func (s *Signature) MinArgumentCount() int`.
    pub fn min_argument_count(&self) -> i32 {
        self.min_argument_count
    }
}

/// Go: `type CompositeSignature struct`.
#[derive(Debug, Default, Clone)]
pub struct CompositeSignature {
    /// True for union, false for intersection
    pub is_union: bool,
    /// Individual signatures
    pub signatures: Vec<SignatureId>,
}

/// Go: `type TypePredicateKind int32` + consts (iota order preserved).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(i32)]
pub enum TypePredicateKind {
    #[default]
    This = 0,
    Identifier = 1,
    AssertsThis = 2,
    AssertsIdentifier = 3,
}

/// Go: `type TypePredicate struct`.
#[derive(Debug, Default, Clone)]
pub struct TypePredicate {
    pub kind: TypePredicateKind,
    pub parameter_index: i32,
    pub parameter_name: String,
    pub t: Option<TypeId>,
}

impl TypePredicate {
    /// Go: `func (typePredicate *TypePredicate) Type() *Type`.
    pub fn type_(&self) -> Option<TypeId> {
        self.t
    }

    /// Go: `func (typePredicate *TypePredicate) Kind() TypePredicateKind`.
    ///
    /// PORT: `Kind()` would collide with Rust's method-name expectations for a
    /// field-less accessor; kept as `kind()` mirroring the Go accessor name.
    pub fn kind(&self) -> TypePredicateKind {
        self.kind
    }

    /// Go: `func (typePredicate *TypePredicate) ParameterIndex() int32`.
    pub fn parameter_index(&self) -> i32 {
        self.parameter_index
    }

    /// Go: `func (typePredicate *TypePredicate) ParameterName() string`.
    pub fn parameter_name(&self) -> &str {
        &self.parameter_name
    }
}

// IndexInfo

/// Go: `type IndexInfo struct` — `keyType`/`valueType` are Types;
/// `declaration` is an IndexSignatureDeclaration; `indexSymbol` is the
/// synthetic property symbol for this index signature; `components` are
/// ElementWithComputedPropertyName nodes.
#[derive(Clone, Debug, Default)]
pub struct IndexInfo {
    pub key_type: Option<TypeId>,
    pub value_type: Option<TypeId>,
    pub is_readonly: bool,
    pub declaration: Option<NodeId>,
    pub index_symbol: Option<SymbolId>,
    pub components: Vec<NodeId>,
}

impl IndexInfo {
    /// Go: `func (info *IndexInfo) KeyType() *Type`.
    pub fn key_type(&self) -> Option<TypeId> {
        self.key_type
    }

    /// Go: `func (info *IndexInfo) ValueType() *Type`.
    pub fn value_type(&self) -> Option<TypeId> {
        self.value_type
    }

    /// Go: `func (info *IndexInfo) IsReadonly() bool`.
    pub fn is_readonly(&self) -> bool {
        self.is_readonly
    }

    /// Go: `func (info *IndexInfo) Declaration() *ast.Node`.
    pub fn declaration(&self) -> Option<NodeId> {
        self.declaration
    }
}

/**
 * Ternary values are defined such that
 * x & y picks the lesser in the order False < Unknown < Maybe < True, and
 * x | y picks the greater in the order False < Unknown < Maybe < True.
 * Generally, Ternary.Maybe is used as the result of a relation that depends on itself, and
 * Ternary.Unknown is used as the result of a variance check that depends on itself. We make
 * a distinction because we don't want to cache circular variance check results.
 */
/// Go: `type Ternary int8` + consts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[repr(i8)]
pub enum Ternary {
    #[default]
    False = 0,
    Unknown = 1,
    Maybe = 3,
    True = -1,
}

/// Go: `type TypeComparer func(s *Type, t *Type, reportErrors bool) Ternary`.
pub type TypeComparer = fn(&Type, &Type, bool) -> Ternary;

/// Go: `type LanguageFeatureMinimumTargetMap struct` — a table of the minimum
/// `ScriptTarget` that supports each language feature. The `Kind`/`ScriptTarget`
/// types come from `tsc_core` (options codegen).
#[derive(Debug)]
pub struct LanguageFeatureMinimumTargetMap {
    pub exponentiation: ScriptTarget,
    pub async_functions: ScriptTarget,
    pub for_await_of: ScriptTarget,
    pub async_generators: ScriptTarget,
    pub async_iteration: ScriptTarget,
    pub object_spread_rest: ScriptTarget,
    pub regular_expression_flags_dot_all: ScriptTarget,
    pub bindingless_catch: ScriptTarget,
    pub bigint: ScriptTarget,
    pub nullish_coalesce: ScriptTarget,
    pub optional_chaining: ScriptTarget,
    pub logical_assignment: ScriptTarget,
    pub top_level_await: ScriptTarget,
    pub class_fields: ScriptTarget,
    pub private_names_and_class_static_blocks: ScriptTarget,
    pub regular_expression_flags_has_indices: ScriptTarget,
    pub shebang_comments: ScriptTarget,
    pub using_and_await_using: ScriptTarget,
    pub class_and_class_element_decorators: ScriptTarget,
    pub regular_expression_flags_unicode_sets: ScriptTarget,
}

/// Go: `var LanguageFeatureMinimumTarget = LanguageFeatureMinimumTargetMap{...}`.
pub static LANGUAGE_FEATURE_MINIMUM_TARGET: LanguageFeatureMinimumTargetMap = LanguageFeatureMinimumTargetMap {
    exponentiation: ScriptTarget::ES2016,
    async_functions: ScriptTarget::ES2017,
    for_await_of: ScriptTarget::ES2018,
    async_generators: ScriptTarget::ES2018,
    async_iteration: ScriptTarget::ES2018,
    object_spread_rest: ScriptTarget::ES2018,
    regular_expression_flags_dot_all: ScriptTarget::ES2018,
    bindingless_catch: ScriptTarget::ES2019,
    bigint: ScriptTarget::ES2020,
    nullish_coalesce: ScriptTarget::ES2020,
    optional_chaining: ScriptTarget::ES2020,
    logical_assignment: ScriptTarget::ES2021,
    top_level_await: ScriptTarget::ES2022,
    class_fields: ScriptTarget::ES2022,
    private_names_and_class_static_blocks: ScriptTarget::ES2022,
    regular_expression_flags_has_indices: ScriptTarget::ES2022,
    shebang_comments: ScriptTarget::ESNext,
    using_and_await_using: ScriptTarget::ESNext,
    class_and_class_element_decorators: ScriptTarget::ESNext,
    regular_expression_flags_unicode_sets: ScriptTarget::ESNext,
};

// Aliases for types

/// Go: `type StringLiteralType = Type`.
pub type StringLiteralType = Type;

// ────────────────────────────────────────────────────────────────────────────
// Unit tests — the ported assertions from the Go types.go surface (Go has no
// types_test.go; these pin the exact flag bit values and sort-order inputs the
// Go tables guarantee, which CompareTypes and the baselines depend on).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The TypeFlags numeric values determine CompareTypes order (the comment
    /// above the Go table); pin a spread of them to the Go constants.
    #[test]
    fn type_flag_bits_mirror_go() {
        assert_eq!(TypeFlags::ANY, TypeFlags(1 << 0));
        assert_eq!(TypeFlags::STRING_LITERAL, TypeFlags(1 << 10));
        assert_eq!(TypeFlags::UNIQUE_ESSYMBOL, TypeFlags(1 << 14));
        assert_eq!(TypeFlags::UNION, TypeFlags(1 << 27));
        assert_eq!(TypeFlags::RESERVED3, TypeFlags(1 << 31));
        assert_eq!(TypeFlags::ANY_OR_UNKNOWN, TypeFlags::ANY | TypeFlags::UNKNOWN);
        assert_eq!(TypeFlags::FRESHABLE, TypeFlags::ENUM | TypeFlags::LITERAL);
        assert_eq!(
            TypeFlags::UNIT,
            TypeFlags::ENUM | TypeFlags::LITERAL | TypeFlags::UNIQUE_ESSYMBOL | TypeFlags::NULLABLE
        );
        // Go: EnumLiteral is a plain bit (1<<15) — always COMBINED with
        // StringLiteral/NumberLiteral/Union by construction, never containing
        // Enum itself (getSortOrderFlags keys off Enum|EnumLiteral as a value).
        assert!(!TypeFlags::ENUM_LITERAL.intersects(TypeFlags::ENUM));
    }

    #[test]
    fn object_flag_bits_mirror_go() {
        assert_eq!(ObjectFlags::CLASS, ObjectFlags(1 << 0));
        assert_eq!(ObjectFlags::MEMBERS_RESOLVED, ObjectFlags(1 << 21));
        assert_eq!(ObjectFlags::CONTAINS_SPREAD, ObjectFlags(1 << 22));
        assert_eq!(ObjectFlags::FROM_TYPE_NODE, ObjectFlags(1 << 29));
        // Union/intersection flags reuse bits 22-29; the mask includes both
        // groups exactly like Go's overlapping definitions.
        assert_eq!(
            ObjectFlags::OBJECT_TYPE_KIND_MASK,
            ObjectFlags(
                ObjectFlags::CLASS_OR_INTERFACE.0
                    | ObjectFlags::REFERENCE.0
                    | ObjectFlags::TUPLE.0
                    | ObjectFlags::ANONYMOUS.0
                    | ObjectFlags::MAPPED.0
                    | ObjectFlags::REVERSE_MAPPED.0
                    | ObjectFlags::EVOLVING_ARRAY.0
                    | ObjectFlags(1 << 24).0
                    | ObjectFlags(1 << 25).0
            )
        );
    }

    /// Go: `FormatTypeFlags` + `TypeFlags.String()`.
    #[test]
    fn format_type_flags_mirrors_go() {
        assert_eq!(format_type_flags(TypeFlags::NONE), vec!["None"]);
        assert_eq!(format_type_flags(TypeFlags::ANY), vec!["Any"]);
        // FRESHABLE = Enum|Literal, so the literal-kind bits all print.
        assert_eq!(
            format_type_flags(TypeFlags::NUMBER_LITERAL | TypeFlags::FRESHABLE),
            vec!["StringLiteral", "NumberLiteral", "BigIntLiteral", "BooleanLiteral", "Enum"]
        );
        assert_eq!(TypeFlags(TypeFlags::STRING.0 | TypeFlags::NUMBER.0).to_string(), "String|Number");
        assert_eq!(TypeFlags::NONE.to_string(), "None");
    }

    /// Go: `func (v VarianceFlags) String() string`.
    #[test]
    fn variance_flags_display_mirrors_go() {
        assert_eq!(VarianceFlags::INVARIANT.to_string(), "in out");
        assert_eq!(VarianceFlags::BIVARIANT.to_string(), "[bivariant]");
        assert_eq!(VarianceFlags::CONTRAVARIANT.to_string(), "in");
        assert_eq!(VarianceFlags::COVARIANT.to_string(), "out");
        assert_eq!(VarianceFlags::INDEPENDENT.to_string(), "[independent]");
        // Go: Unmeasurable is a bare bit (1<<3); the variance mask reads
        // Invariant (0) → "in out" plus the suffix.
        assert_eq!(VarianceFlags::UNMEASURABLE.to_string(), "in out (unmeasurable)");
        assert_eq!(VarianceFlags::UNRELIABLE.to_string(), "in out (unreliable)");
        assert_eq!(
            (VarianceFlags::UNRELIABLE | VarianceFlags::COVARIANT).to_string(),
            "out (unreliable)"
        );
    }

    #[test]
    fn ternary_order_mirrors_go() {
        // x & y picks the lesser in False < Unknown < Maybe < True.
        assert!((Ternary::False as i8 & Ternary::True as i8) == Ternary::False as i8);
        // x | y picks the greater.
        assert!((Ternary::False as i8 | Ternary::True as i8) == Ternary::True as i8);
        assert_eq!(Ternary::False as i8, 0);
        assert_eq!(Ternary::Unknown as i8, 1);
        assert_eq!(Ternary::Maybe as i8, 3);
        assert_eq!(Ternary::True as i8, -1);
    }

    /// The ExternalEmitHelpers aliases share Go's values (ESDecorate ==
    /// Decorate etc.).
    #[test]
    fn emit_helper_aliases_mirror_go() {
        assert_eq!(ExternalEmitHelpers::ES_DECORATE_AND_RUN_INITIALIZERS, ExternalEmitHelpers::DECORATE);
        assert_eq!(ExternalEmitHelpers::FIRST_EMIT_HELPER, ExternalEmitHelpers::REST);
        assert_eq!(
            ExternalEmitHelpers::LAST_EMIT_HELPER,
            ExternalEmitHelpers::REWRITE_RELATIVE_IMPORT_EXTENSION
        );
        assert_eq!(ExternalEmitHelpers::FOR_AWAIT_OF_INCLUDES, ExternalEmitHelpers::ASYNC_VALUES);
    }

    #[test]
    fn language_feature_minimum_targets_mirror_go() {
        assert_eq!(LANGUAGE_FEATURE_MINIMUM_TARGET.exponentiation, ScriptTarget::ES2016);
        assert_eq!(LANGUAGE_FEATURE_MINIMUM_TARGET.bigint, ScriptTarget::ES2020);
        assert_eq!(LANGUAGE_FEATURE_MINIMUM_TARGET.using_and_await_using, ScriptTarget::ESNext);
    }

    /// `format!` of TypeFlags must not be needed anywhere else in this file —
    /// silence the unused-import lint path explicitly by exercising Write.
    #[test]
    fn write_type_flags() {
        let mut s = String::new();
        let _ = writeln!(s, "{}", TypeFlags::NUMBER);
        assert_eq!(s, "Number\n");
    }
}
