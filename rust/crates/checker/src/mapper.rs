// Ported from tsc/internal/checker/mapper.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The type-instantiation Mapper machinery — M6 W1 (foundational for the later
// relater/instantiation waves).
//
// PORT (SPEC §5.4 — the arena idiom): Go's mapper.go constructors return
// `*TypeMapper` heap objects (`m := &SimpleTypeMapper{}; m.data = m; return
// &m.TypeMapper` — the self-referential data interface). The port stores the
// mapper data in `Checker::type_mappers` and hands out `TypeMapperId`
// indices, so:
//   * `*TypeMapper` → `TypeMapperId`; pointer identity (compareTypeMappers'
//     first check, findActiveMapper) is id equality.
//   * `m.Map(t)` / `m.Kind()` / `m.MapsThisOnly()` → `Checker::mapper_map` /
//     `mapper_kind` / `mapper_maps_this_only` (the §5.4 explicit-checker
//     idiom: Composite/Inference mappers call back into the checker, which a
//     stored `&Checker` cannot express).
//   * `TypeMapperBase`'s embedded defaults (Map → identity, Kind → Unknown,
//     MapsThisOnly → false) are the enum dispatch's default arms — the
//     concrete mappers that DON'T override a method (ArrayToSingle/Deferred/
//     Function keep base Kind; Function/Merged/Composite/Inference keep base
//     MapsThisOnly) are called out arm-by-arm below, exactly like Go's
//     embedding.
//   * `newInferenceTypeMapper`'s `n *InferenceContext` → an
//     `InferenceContextId` index into `Checker::inference_contexts`
//     (todo_shapes — the inference wave owns the real design).
//   * The closures (`[]func() *Type`, `func(*Type) *Type`) take `&mut Checker`
//     at call time: Go's closures capture the checker; a Rust closure stored
//     in the checker cannot capture it (self-referential), so the checker is
//     the callback's first argument (PORT-noted per constructor).

// The mapper machinery's consumers are the M6 S7 instantiateType engine and
// the S5/inference waves (mapper.go is W1's "foundational for the later
// relater/instantiation waves" file); until those land, the surface is
// test-driven — the spike's Checker-skeleton convention of silencing the
// dead-code analysis applies (remove with the S7 wave).
#![allow(dead_code)]

use crate::todo_shapes::{InferenceContextId, InferenceInfo, TypeMapperId};
use crate::types::TypeId;
use crate::Checker;

// TypeMapperKind

/// Go: `type TypeMapperKind int32` (iota order preserved — the numeric values
/// drive `compareTypeMappers`' kind ordering).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum TypeMapperKind {
    Unknown = 0,
    Simple = 1,
    Array = 2,
    Merged = 3,
}

// TypeMapperData

/// PORT (clippy type_complexity): Go's `targets []func() *Type` thunks —
/// the checker-table thunks take the checker as their argument (see
/// `DeferredTypeMapper`).
pub(crate) type DeferredThunk = Box<dyn Fn(&mut Checker) -> TypeId>;
/// PORT (clippy type_complexity): Go's `fn func(*Type) *Type`.
pub(crate) type MapperFn = Box<dyn Fn(&mut Checker, TypeId) -> TypeId>;

/// Go: `type TypeMapperData interface` + the eight concrete impls (each of
/// which embeds `TypeMapperBase`, itself embedding `TypeMapper`; the port's
/// enum variants carry only the per-kind fields — see the module PORT note).
pub enum TypeMapperData {
    /// Go: `type SimpleTypeMapper struct { TypeMapperBase; source, target *Type }`.
    Simple {
        source: TypeId,
        target: TypeId,
    },
    /// Go: `type ArrayTypeMapper struct { TypeMapperBase; sources, targets []*Type }`.
    Array {
        sources: Vec<TypeId>,
        targets: Vec<TypeId>,
    },
    /// Go: `type ArrayToSingleTypeMapper struct { TypeMapperBase; sources []*Type; target *Type }`.
    ArrayToSingle {
        sources: Vec<TypeId>,
        target: TypeId,
    },
    /// Go: `type DeferredTypeMapper struct { TypeMapperBase; sources []*Type; targets []func() *Type }`.
    ///
    /// PORT: Go's thunks `func() *Type` capture whatever they need (usually
    /// the checker); thunks stored in the checker's table take the checker as
    /// their argument instead.
    Deferred {
        sources: Vec<TypeId>,
        targets: Vec<DeferredThunk>,
    },
    /// Go: `type FunctionTypeMapper struct { TypeMapperBase; fn func(*Type) *Type }`.
    ///
    /// PORT: same as Deferred — the function takes the checker as its first
    /// argument.
    Function {
        f: MapperFn,
    },
    /// Go: `type MergedTypeMapper struct { TypeMapperBase; m1, m2 *TypeMapper }`.
    Merged {
        m1: TypeMapperId,
        /// Go's `m2` may be nil (constructed through `mergeTypeMappers`);
        /// the `Map` deref panics in Go on nil — the port `expect`s.
        m2: Option<TypeMapperId>,
    },
    /// Go: `type CompositeTypeMapper struct { TypeMapperBase; c *Checker; m1, m2 *TypeMapper }`.
    ///
    /// PORT: the `c *Checker` field is dropped — `mapper_map` passes the
    /// checker explicitly (§5.4).
    Composite {
        m1: TypeMapperId,
        /// Go's `m2` may be nil (constructed through `combineTypeMappers`);
        /// same nil-deref/expect contract as Merged.
        m2: Option<TypeMapperId>,
    },
    /// Go: `type InferenceTypeMapper struct { TypeMapperBase; c *Checker; n *InferenceContext; fixing bool }`.
    Inference {
        /// PORT: replaces Go's `n *InferenceContext` (see the module PORT note).
        n: InferenceContextId,
        fixing: bool,
    },
}

/// The owned decision produced by a mapper's match step — needed because the
/// Deferred/Function/Composite/Inference arms must re-borrow the checker while
/// the mapper payload is borrowed from the table (Go's GC pointers have no
/// such constraint).
enum MapAction {
    /// Go: `TypeMapperBase.Map` — the identity default.
    Identity,
    Target(TypeId),
    /// Call `targets[index](&mut checker)`.
    Deferred(usize),
    /// Call `f(&mut checker, t)`.
    Function,
    Merge(TypeMapperId, Option<TypeMapperId>),
    Composite(TypeMapperId, Option<TypeMapperId>),
    Inference(InferenceContextId, bool),
}

impl TypeMapperData {
    fn plan(&self, t: TypeId) -> MapAction {
        match self {
            TypeMapperData::Simple { source, target } => {
                // Go: `if t == m.source { return m.target }; return t`.
                if t == *source {
                    MapAction::Target(*target)
                } else {
                    MapAction::Identity
                }
            }
            TypeMapperData::Array { sources, targets } => {
                // Go: `for i, s := range m.sources { if t == s { return m.targets[i] } }; return t`.
                for (i, s) in sources.iter().enumerate() {
                    if t == *s {
                        return MapAction::Target(targets[i]);
                    }
                }
                MapAction::Identity
            }
            TypeMapperData::ArrayToSingle { sources, target } => {
                // Go: `if slices.Contains(m.sources, t) { return m.target }; return t`.
                if sources.contains(&t) {
                    MapAction::Target(*target)
                } else {
                    MapAction::Identity
                }
            }
            TypeMapperData::Deferred { sources, .. } => {
                // Go: `for i, s := range m.sources { if t == s { return m.targets[i]() } }; return t`.
                for (i, s) in sources.iter().enumerate() {
                    if t == *s {
                        return MapAction::Deferred(i);
                    }
                }
                MapAction::Identity
            }
            TypeMapperData::Function { .. } => MapAction::Function,
            TypeMapperData::Merged { m1, m2 } => MapAction::Merge(*m1, *m2),
            TypeMapperData::Composite { m1, m2 } => MapAction::Composite(*m1, *m2),
            TypeMapperData::Inference { n, fixing } => MapAction::Inference(*n, *fixing),
        }
    }
}

impl Checker {
    /// Go: `func (m *TypeMapper) Map(t *Type) *Type` — the checker method form
    /// (see the module PORT note). Dispatches over the data table.
    pub(crate) fn mapper_map(&mut self, mapper: TypeMapperId, t: TypeId) -> TypeId {
        match self.type_mappers[mapper.0 as usize].plan(t) {
            MapAction::Identity => t,
            MapAction::Target(target) => target,
            MapAction::Deferred(index) => {
                // PORT: Go's thunk invocation is GC-safe; the port briefly
                // swaps the mapper out of the table so the thunk (a callback
                // taking `&mut Checker`) can borrow the checker.
                let mut data = std::mem::replace(
                    &mut self.type_mappers[mapper.0 as usize],
                    TypeMapperData::Simple { source: t, target: t },
                );
                let TypeMapperData::Deferred { targets, .. } = &mut data else {
                    unreachable!("plan said Deferred");
                };
                let result = targets[index](self);
                self.type_mappers[mapper.0 as usize] = data;
                result
            }
            MapAction::Function => {
                // PORT: same swap trick as Deferred.
                let mut data = std::mem::replace(
                    &mut self.type_mappers[mapper.0 as usize],
                    TypeMapperData::Simple { source: t, target: t },
                );
                let TypeMapperData::Function { f } = &mut data else {
                    unreachable!("plan said Function");
                };
                let result = f(self, t);
                self.type_mappers[mapper.0 as usize] = data;
                result
            }
            // Go (MergedTypeMapper): `return m.m2.Map(m.m1.Map(t))` — the nil
            // m2 deref panics in Go; the port's expect mirrors it.
            MapAction::Merge(m1, m2) => {
                let inner = self.mapper_map(m1, t);
                self.mapper_map(m2.expect("MergedTypeMapper.m2"), inner)
            }
            // Go (CompositeTypeMapper):
            //   t1 := m.m1.Map(t)
            //   if t1 != t { return m.c.instantiateType(t1, m.m2) }
            //   return m.m2.Map(t)
            MapAction::Composite(m1, m2) => {
                let t1 = self.mapper_map(m1, t);
                let m2 = m2.expect("CompositeTypeMapper.m2");
                if t1 != t {
                    self.instantiate_type(t1, m2)
                } else {
                    self.mapper_map(m2, t)
                }
            }
            // Go (InferenceTypeMapper):
            //   for i, inference := range m.n.inferences {
            //     if t == inference.typeParameter {
            //       if m.fixing && !inference.isFixed {
            //         m.c.inferFromIntraExpressionSites(m.n)
            //         clearCachedInferences(m.n.inferences)
            //         inference.isFixed = true
            //       }
            //       return m.c.getInferredType(m.n, i)
            //     }
            //   }
            //   return t
            // (the `m.c` field is dropped — §5.4; the context resolves through
            // the `inference_contexts` arena).
            MapAction::Inference(n, fixing) => {
                let context = n.0 as usize;
                for i in 0..self.inference_contexts[context].inferences.len() {
                    if t == self.inference_contexts[context].inferences[i].type_parameter {
                        if fixing && !self.inference_contexts[context].inferences[i].is_fixed {
                            // Before we commit to a particular inference (and
                            // thus lock out any further inferences), we infer
                            // from any intra-expression inference sites we
                            // have collected. (Go comment, verbatim.)
                            self.infer_from_intra_expression_sites(n);
                            clear_cached_inferences(&mut self.inference_contexts[context].inferences);
                            self.inference_contexts[context].inferences[i].is_fixed = true;
                        }
                        return self.get_inferred_type(n, i);
                    }
                }
                t
            }
        }
    }

    /// Go: `func (m *TypeMapper) Kind() TypeMapperKind`.
    pub(crate) fn mapper_kind(&self, mapper: TypeMapperId) -> TypeMapperKind {
        match &self.type_mappers[mapper.0 as usize] {
            TypeMapperData::Simple { .. } => TypeMapperKind::Simple,
            TypeMapperData::Array { .. } => TypeMapperKind::Array,
            TypeMapperData::Merged { .. } => TypeMapperKind::Merged,
            // Go: TypeMapperBase's default `TypeMapperKindUnknown`, inherited
            // by the concrete mappers that don't override Kind:
            // ArrayToSingle, Deferred, Function, Composite, Inference.
            _ => TypeMapperKind::Unknown,
        }
    }

    /// Go: `func (m *TypeMapper) MapsThisOnly() bool`.
    pub(crate) fn mapper_maps_this_only(&self, mapper: TypeMapperId) -> bool {
        use crate::utilities::is_this_type_parameter;
        match &self.type_mappers[mapper.0 as usize] {
            // Go (SimpleTypeMapper): `isThisTypeParameter(m.source)`.
            TypeMapperData::Simple { source, .. } => is_this_type_parameter(self.type_(*source)),
            // Go (Array/ArrayToSingle/DeferredTypeMapper):
            // `len(m.sources) == 1 && isThisTypeParameter(m.sources[0])`.
            TypeMapperData::Array { sources, .. }
            | TypeMapperData::ArrayToSingle { sources, .. }
            | TypeMapperData::Deferred { sources, .. } => {
                sources.len() == 1 && is_this_type_parameter(self.type_(sources[0]))
            }
            // Go: TypeMapperBase's default `false`, inherited by the concrete
            // mappers that don't override MapsThisOnly: Function, Merged,
            // Composite, Inference.
            _ => false,
        }
    }

    /// The mapper payload behind `id` — the compare-side read access for
    /// `compare_type_mappers` (utilities.go) and the tests.
    pub(crate) fn mapper_data(&self, mapper: TypeMapperId) -> &TypeMapperData {
        &self.type_mappers[mapper.0 as usize]
    }

    /// Go (utilities.go, CompareTypes' mapped-type arm):
    /// `m.data.(*CompositeTypeMapper).m2` — the checked interface cast panics
    /// on mismatch in Go; the port's expect mirrors it. The S7 wave's
    /// instantiateAnonymousType is what makes the mapped-type mappers
    /// composite (utilities.go's comment: "Compare the effective
    /// instantiation, not the identity of that fresh parameter").
    pub(crate) fn mapper_composite_m2(&self, mapper: TypeMapperId) -> Option<TypeMapperId> {
        match &self.type_mappers[mapper.0 as usize] {
            TypeMapperData::Composite { m2, .. } => *m2,
            _ => panic!("mapper is not a CompositeTypeMapper"),
        }
    }
}

// Factory functions

/// Go: `func getMappedType(t *Type, mapper *TypeMapper) *Type` — the checker
/// parameter is the §5.4 explicit-receiver idiom (Go's `mapper.Map` needs the
/// checker through the data interface).
pub(crate) fn get_mapped_type(c: &mut Checker, t: TypeId, mapper: TypeMapperId) -> TypeId {
    c.mapper_map(mapper, c.get_non_distributed_type_parameter(t))
}

impl Checker {
    /// Go: `func newTypeMapper(sources []*Type, targets []*Type) *TypeMapper`.
    pub(crate) fn new_type_mapper(
        &mut self,
        sources: &[TypeId],
        targets: &[TypeId],
    ) -> TypeMapperId {
        if sources.len() == 1 {
            self.new_simple_type_mapper(sources[0], targets[0])
        } else {
            self.new_array_type_mapper(sources, targets)
        }
    }

    /// Go: `func newSimpleTypeMapper(source *Type, target *Type) *TypeMapper`.
    pub(crate) fn new_simple_type_mapper(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers
            .push(TypeMapperData::Simple { source, target });
        id
    }

    /// Go: `func newArrayTypeMapper(sources []*Type, targets []*Type) *TypeMapper`.
    pub(crate) fn new_array_type_mapper(
        &mut self,
        sources: &[TypeId],
        targets: &[TypeId],
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Array {
            sources: sources.to_vec(),
            targets: targets.to_vec(),
        });
        id
    }

    /// Go: `func newArrayToSingleTypeMapper(sources []*Type, target *Type) *TypeMapper`.
    pub(crate) fn new_array_to_single_type_mapper(
        &mut self,
        sources: &[TypeId],
        target: TypeId,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::ArrayToSingle {
            sources: sources.to_vec(),
            target,
        });
        id
    }

    /// Go: `func newDeferredTypeMapper(sources []*Type, targets []func() *Type) *TypeMapper`
    /// — the deferred/instantiation-list mapper: targets compute lazily on
    /// first mapping (used during inference when instantiating type parameter
    /// defaults before the target types exist).
    pub(crate) fn new_deferred_type_mapper(
        &mut self,
        sources: &[TypeId],
        targets: Vec<DeferredThunk>,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Deferred {
            sources: sources.to_vec(),
            targets,
        });
        id
    }

    /// Go: `func newFunctionTypeMapper(fn func(*Type) *Type) *TypeMapper`.
    pub(crate) fn new_function_type_mapper(
        &mut self,
        f: MapperFn,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Function { f });
        id
    }

    /// Go: `func newMergedTypeMapper(m1 *TypeMapper, m2 *TypeMapper) *TypeMapper`.
    pub(crate) fn new_merged_type_mapper(
        &mut self,
        m1: TypeMapperId,
        m2: Option<TypeMapperId>,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Merged { m1, m2 });
        id
    }

    /// Go: `func newCompositeTypeMapper(c *Checker, m1 *TypeMapper, m2 *TypeMapper) *TypeMapper`
    /// — the `c` parameter is dropped (§5.4; `mapper_map` supplies it).
    pub(crate) fn new_composite_type_mapper(
        &mut self,
        m1: TypeMapperId,
        m2: Option<TypeMapperId>,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Composite { m1, m2 });
        id
    }

    /// Go: `func (c *Checker) newInferenceTypeMapper(n *InferenceContext, fixing bool) *TypeMapper`.
    pub(crate) fn new_inference_type_mapper(
        &mut self,
        n: InferenceContextId,
        fixing: bool,
    ) -> TypeMapperId {
        let id = TypeMapperId(self.type_mappers.len() as u32);
        self.type_mappers.push(TypeMapperData::Inference { n, fixing });
        id
    }

    /// Go: `func (c *Checker) combineTypeMappers(m1 *TypeMapper, m2 *TypeMapper) *TypeMapper`.
    pub(crate) fn combine_type_mappers(
        &mut self,
        m1: Option<TypeMapperId>,
        m2: Option<TypeMapperId>,
    ) -> Option<TypeMapperId> {
        if let Some(m1) = m1 {
            return Some(self.new_composite_type_mapper(m1, m2));
        }
        m2
    }

    /// Go: `func (c *Checker) mapTypeWithCompositeMapper(t *Type, m1 *TypeMapper, m2 *TypeMapper) *Type`.
    pub(crate) fn map_type_with_composite_mapper(
        &mut self,
        t: TypeId,
        m1: Option<TypeMapperId>,
        m2: TypeMapperId,
    ) -> TypeId {
        if m1.is_none() {
            return get_mapped_type(self, t, m2);
        }
        let m1 = m1.expect("just checked");
        let t1 = get_mapped_type(self, t, m1);
        if t1 != t {
            return self.instantiate_type(t1, m2);
        }
        get_mapped_type(self, t, m2)
    }

    /// Go: `func mergeTypeMappers(m1 *TypeMapper, m2 *TypeMapper) *TypeMapper` —
    /// a Checker method in the port (it allocates into the mapper table).
    pub(crate) fn merge_type_mappers(
        &mut self,
        m1: Option<TypeMapperId>,
        m2: Option<TypeMapperId>,
    ) -> Option<TypeMapperId> {
        if let Some(m1) = m1 {
            return Some(self.new_merged_type_mapper(m1, m2));
        }
        m2
    }

    /// Go: `func prependTypeMapping(source *Type, target *Type, mapper *TypeMapper) *TypeMapper`.
    pub(crate) fn prepend_type_mapping(
        &mut self,
        source: TypeId,
        target: TypeId,
        mapper: Option<TypeMapperId>,
    ) -> TypeMapperId {
        match mapper {
            None => {
                let source = self.get_non_distributed_type_parameter(source);
                self.new_simple_type_mapper(source, target)
            }
            Some(mapper) => {
                let source = self.get_non_distributed_type_parameter(source);
                let simple = self.new_simple_type_mapper(source, target);
                self.new_merged_type_mapper(simple, Some(mapper))
            }
        }
    }

    /// Go: `func appendTypeMapping(mapper *TypeMapper, source *Type, target *Type) *TypeMapper`.
    pub(crate) fn append_type_mapping(
        &mut self,
        mapper: Option<TypeMapperId>,
        source: TypeId,
        target: TypeId,
    ) -> TypeMapperId {
        match mapper {
            None => {
                let source = self.get_non_distributed_type_parameter(source);
                self.new_simple_type_mapper(source, target)
            }
            Some(mapper) => {
                let source = self.get_non_distributed_type_parameter(source);
                let simple = self.new_simple_type_mapper(source, target);
                self.new_merged_type_mapper(mapper, Some(simple))
            }
        }
    }

    /// Go: `func (c *Checker) newBackreferenceMapper(context *InferenceContext, index int) *TypeMapper` —
    /// "Maps forward-references to later types parameters to the empty object
    /// type. This is used during inference when instantiating type parameter
    /// defaults." (Go comment, verbatim.)
    pub(crate) fn new_backreference_mapper(
        &mut self,
        context: InferenceContextId,
        index: usize,
    ) -> TypeMapperId {
        // Go: `forwardInferences := context.inferences[index:]` — the tail
        // slice; the port copies the ids (Go's slice shares, but only the
        // typeParameter field is read, and `newArrayToSingleTypeMapper` copies
        // its sources anyway).
        let forward_inferences: &[InferenceInfo] =
            &self.inference_contexts[context.0 as usize].inferences[index..];
        // Go: `core.Map(forwardInferences, func(i *InferenceInfo) *Type { return i.typeParameter })`.
        let type_parameters: Vec<TypeId> = forward_inferences
            .iter()
            .map(|i| i.type_parameter)
            .collect();
        let unknown = self.unknown_type();
        self.new_array_to_single_type_mapper(&type_parameters, unknown)
    }
}

/// Go: `func clearCachedInferences(inferences []*InferenceInfo)`
/// (inference.go:1655 — M6 wave 2's file). EARLY LANDING: mapper.go's
/// InferenceTypeMapper is the only W1 caller and the body is
/// self-contained; the inference wave moves it to its home module.
pub(crate) fn clear_cached_inferences(inferences: &mut [InferenceInfo]) {
    for inference in inferences {
        if !inference.is_fixed {
            inference.inferred_type = None;
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (Go has no mapper_test.go; these pin the Go-shaped semantics the
// file's comments and embedding structure prescribe: identity defaults, kind
// inheritance from TypeMapperBase, this-only detection, merge/composite
// ordering, deferred laziness, and the backreference tail slice).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::todo_shapes::{InferenceContext, InferenceInfo};

    fn new_checker() -> Checker {
        super::tests_support::new_empty_checker()
    }

    fn map(c: &mut Checker, mapper: TypeMapperId, t: TypeId) -> TypeId {
        c.mapper_map(mapper, t)
    }

    #[test]
    fn simple_type_mapper_maps_only_its_source() {
        let mut c = new_checker();
        let a = c.string_type();
        let b = c.number_type();
        let other = c.boolean_type_or_marker();
        let m = c.new_simple_type_mapper(a, b);
        assert_eq!(map(&mut c, m, a), b);
        assert_eq!(map(&mut c, m, other), other);
        assert_eq!(c.mapper_kind(m), TypeMapperKind::Simple);
        assert!(!c.mapper_maps_this_only(m));
    }

    #[test]
    fn array_type_mapper_first_match_wins() {
        let mut c = new_checker();
        let (a, b, d) = (c.string_type(), c.number_type(), c.bigint_type());
        let (x, y) = (c.boolean_type_or_marker(), c.never_type());
        let m = c.new_array_type_mapper(&[a, b], &[x, y]);
        assert_eq!(map(&mut c, m, a), x);
        assert_eq!(map(&mut c, m, b), y);
        assert_eq!(map(&mut c, m, d), d);
        assert_eq!(c.mapper_kind(m), TypeMapperKind::Array);
    }

    #[test]
    fn array_to_single_and_deferred_inherit_unknown_kind() {
        let mut c = new_checker();
        let (a, b) = (c.string_type(), c.number_type());
        // ArrayToSingleTypeMapper overrides Map/MapsThisOnly but NOT Kind —
        // TypeMapperBase's TypeMapperKindUnknown shows through (Go embedding).
        let m1 = c.new_array_to_single_type_mapper(&[a], b);
        assert_eq!(map(&mut c, m1, a), b);
        assert_eq!(c.mapper_kind(m1), TypeMapperKind::Unknown);
        // DeferredTypeMapper likewise.
        let bigint = c.bigint_type();
        let m2 = c.new_deferred_type_mapper(&[a], vec![Box::new(move |c: &mut Checker| c.bigint_type())]);
        assert_eq!(map(&mut c, m2, a), bigint);
        assert_eq!(c.mapper_kind(m2), TypeMapperKind::Unknown);
        // FunctionTypeMapper overrides only Map — base Kind AND base
        // MapsThisOnly both show through.
        let m3 = c.new_function_type_mapper(Box::new(|_c: &mut Checker, t: TypeId| t));
        assert_eq!(map(&mut c, m3, a), a);
        assert_eq!(c.mapper_kind(m3), TypeMapperKind::Unknown);
        assert!(!c.mapper_maps_this_only(m3));
    }

    #[test]
    fn deferred_targets_compute_lazily_at_map_time() {
        let mut c = new_checker();
        let a = c.string_type();
        // The thunk reads the checker at call time (the port shape) — a
        // deferred mapper created before the target type exists resolves it
        // on first Map, the whole point of DeferredTypeMapper.
        let m = c.new_deferred_type_mapper(&[a], vec![Box::new(|c: &mut Checker| c.number_type())]);
        assert_eq!(map(&mut c, m, a), c.number_type());
        // A non-matching type never invokes the thunk.
        let bigint = c.bigint_type();
        assert_eq!(map(&mut c, m, bigint), bigint);
    }

    #[test]
    fn merged_mapper_applies_m1_then_m2() {
        let mut c = new_checker();
        let (a, b, d) = (c.string_type(), c.number_type(), c.bigint_type());
        let s1 = c.new_simple_type_mapper(a, b);
        let s2 = c.new_simple_type_mapper(b, d);
        let m = c.new_merged_type_mapper(s1, Some(s2));
        // Go: `m.m2.Map(m.m1.Map(t))` — composition order is observable.
        assert_eq!(map(&mut c, m, a), d);
        assert_eq!(map(&mut c, m, b), d);
        assert_eq!(c.mapper_kind(m), TypeMapperKind::Merged);
        assert!(!c.mapper_maps_this_only(m));
    }

    #[test]
    fn composite_mapper_instantiates_only_when_m1_changed_the_type() {
        let mut c = new_checker();
        let (a, b) = (c.string_type(), c.number_type());
        let d = c.bigint_type();
        let s1 = c.new_simple_type_mapper(a, b);
        let s2 = c.new_simple_type_mapper(b, d);
        let m = c.new_composite_type_mapper(s1, Some(s2));
        // m1 maps a → b (changed): the composite arms runs instantiateType
        // (the W1 stub is identity, so b survives unmapped-by-m2 — the stub's
        // documented behavior; S7 replaces it with real instantiation).
        assert_eq!(map(&mut c, m, a), b);
        // t unmapped by m1: m2.Map(t) runs directly.
        assert_eq!(map(&mut c, m, d), d);
    }

    #[test]
    fn merge_prepend_and_append_wrappers() {
        let mut c = new_checker();
        let (a, b, d) = (c.string_type(), c.number_type(), c.bigint_type());
        let s = c.new_simple_type_mapper(a, b);
        // mergeTypeMappers(nil, m2) = m2.
        assert_eq!(c.merge_type_mappers(None, Some(s)), Some(s));
        // mergeTypeMappers(m1, m2) builds a Merged mapper.
        let merged = c.merge_type_mappers(Some(s), Some(s)).unwrap();
        assert_eq!(c.mapper_kind(merged), TypeMapperKind::Merged);
        // prependTypeMapping puts the new simple mapping FIRST (the existing
        // mapper runs after it): d→b composed with a→b maps a→b and d→b.
        let prepended = c.prepend_type_mapping(d, b, Some(s));
        assert_eq!(map(&mut c, prepended, d), b);
        assert_eq!(map(&mut c, prepended, a), b);
        // appendTypeMapping puts it LAST (Go: `newMergedTypeMapper(mapper,
        // newSimpleTypeMapper(...))` + `m2.Map(m1.Map(t))`): the existing
        // mapper a→b runs first, the new d→a mapping after it — so d→a, and
        // a→b→b (the new mapping doesn't see the mapped output).
        let appended = c.append_type_mapping(Some(s), d, a);
        assert_eq!(map(&mut c, appended, d), a);
        assert_eq!(map(&mut c, appended, a), b);
        // Nil-mapper arms both build the bare simple mapping.
        let bare = c.prepend_type_mapping(a, b, None);
        assert_eq!(map(&mut c, bare, a), b);
    }

    #[test]
    fn combine_type_mappers() {
        let mut c = new_checker();
        let (a, b) = (c.string_type(), c.number_type());
        let s = c.new_simple_type_mapper(a, b);
        assert_eq!(c.combine_type_mappers(None, Some(s)), Some(s));
        let combined = c.combine_type_mappers(Some(s), Some(s)).unwrap();
        assert!(matches!(
            c.mapper_data(combined),
            TypeMapperData::Composite { .. }
        ));
    }

    #[test]
    fn inference_mapper_fixes_and_returns_the_stub_inferred_type() {
        let mut c = new_checker();
        let tp = c.string_type();
        let ctx = InferenceContextId(c.inference_contexts.len() as u32);
        c.inference_contexts.push(InferenceContext {
            inferences: vec![
                InferenceInfo {
                    type_parameter: tp,
                    is_fixed: false,
                    inferred_type: Some(c.boolean_type_or_marker()),
                },
                InferenceInfo {
                    type_parameter: c.number_type(),
                    is_fixed: true,
                    inferred_type: Some(c.never_type()),
                },
            ],
        });
        let fixing = c.new_inference_type_mapper(ctx, true);
        // First Map: fixing fires — the cached inference is cleared and the
        // type parameter is marked fixed (the mutation is visible through the
        // context, exactly like Go's shared []*InferenceInfo).
        assert_eq!(map(&mut c, fixing, tp), c.error_type()); // get_inferred_type W1 stub
        assert!(c.inference_contexts[ctx.0 as usize].inferences[0].is_fixed);
        assert_eq!(c.inference_contexts[ctx.0 as usize].inferences[0].inferred_type, None);
        // A non-matching type passes through untouched.
        let bigint = c.bigint_type();
        assert_eq!(map(&mut c, fixing, bigint), bigint);
        // A non-fixing mapper never touches the isFixed flag.
        let non_fixing = c.new_inference_type_mapper(ctx, false);
        c.inference_contexts[ctx.0 as usize].inferences[0].is_fixed = false;
        assert_eq!(map(&mut c, non_fixing, tp), c.error_type());
        assert!(!c.inference_contexts[ctx.0 as usize].inferences[0].is_fixed);
    }

    #[test]
    fn backreference_mapper_maps_the_inference_tail_to_unknown() {
        let mut c = new_checker();
        let ctx = InferenceContextId(c.inference_contexts.len() as u32);
        let (string, number, bigint) = (c.string_type(), c.number_type(), c.bigint_type());
        let unknown = c.unknown_type();
        c.inference_contexts.push(InferenceContext {
            inferences: vec![
                InferenceInfo { type_parameter: string, ..Default::default() },
                InferenceInfo { type_parameter: number, ..Default::default() },
                InferenceInfo { type_parameter: bigint, ..Default::default() },
            ],
        });
        let m = c.new_backreference_mapper(ctx, 1);
        // Only the tail (index 1..) maps to unknownType; the head passes
        // through — the forward-references Go's comment describes.
        assert_eq!(map(&mut c, m, string), string);
        assert_eq!(map(&mut c, m, number), unknown);
        assert_eq!(map(&mut c, m, bigint), unknown);
    }

    #[test]
    fn maps_this_only_follows_the_this_type_parameter() {
        let mut c = new_checker();
        // A "this" type parameter: TYPE_PARAMETER flag + isThisType on the
        // payload (utilities.go's isThisTypeParameter).
        let tp = c.alloc_test_type_parameter(true /*is_this_type*/);
        let m = c.new_simple_type_mapper(tp, c.number_type());
        assert!(c.mapper_maps_this_only(m));
        let m2 = c.new_array_type_mapper(&[tp], &[c.number_type()]);
        assert!(c.mapper_maps_this_only(m2));
        let m3 = c.new_array_type_mapper(&[tp, c.string_type()], &[c.number_type(), c.bigint_type()]);
        assert!(!c.mapper_maps_this_only(m3));
    }

    #[test]
    fn get_mapped_type_unwraps_distributed_type_parameters() {
        let mut c = new_checker();
        let (tp, constraint) = c.alloc_test_distributed_type_parameter();
        let m = c.new_simple_type_mapper(constraint, c.number_type());
        // getMappedType maps the CONSTRAINT of a distributed type parameter,
        // not the parameter itself (checker.go's getNonDistributedTypeParameter).
        assert_eq!(get_mapped_type(&mut c, tp, m), c.number_type());
    }

    #[test]
    fn active_mapper_stack_push_pop_and_find() {
        let mut c = new_checker();
        let (a, b) = (c.string_type(), c.number_type());
        let m1 = c.new_simple_type_mapper(a, b);
        let m2 = c.new_simple_type_mapper(b, a);
        assert_eq!(c.find_active_mapper(m1), None);
        c.push_active_mapper(m1);
        c.push_active_mapper(m2);
        // findActiveMapper finds the LAST occurrence (Go: FindLastIndex).
        assert_eq!(c.find_active_mapper(m1), Some(0));
        assert_eq!(c.find_active_mapper(m2), Some(1));
        c.pop_active_mapper();
        assert_eq!(c.find_active_mapper(m2), None);
        c.pop_active_mapper();
        assert!(c.active_mappers.is_empty());
        // The caches track the stack (checker.go keeps them parallel).
        assert_eq!(c.active_type_mappers_caches.len(), c.active_mappers.len());
        c.push_active_mapper(m1);
        c.clear_active_mapper_caches();
        assert!(c.active_type_mappers_caches[0].is_empty());
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Test support (crate-private): the no-file Program + small arena helpers the
// mapper unit tests need. Mirrors tests/spike.rs's TestProgram seam.
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::checker::{Checker, Program};
    use crate::types::{
        ObjectFlags, Type, TypeData, TypeFlags, TypeId, TypeParameter,
    };
    use tsc_core::scriptkind::ScriptKind;

    /// A no-source-file program: NewChecker seeds the symbol table and the
    /// intrinsic types through the §5.5 seam. The node store is an empty
    /// parsed file (nothing in these tests dereferences nodes, but the
    /// exports-side wrappers fetch the store eagerly).
    struct EmptyProgramImpl {
        options: tsc_core::options_generated::CompilerOptions,
        source_file: tsc_ast::SourceFile,
        symbols: Vec<tsc_ast::Symbol>,
    }

    impl Program for EmptyProgramImpl {
        fn options(&self) -> &tsc_core::options_generated::CompilerOptions {
            &self.options
        }
        fn source_files(&self) -> &[tsc_ast::SourceFile] {
            &[]
        }
        fn bind_source_files(&mut self) {}
        fn node_store(&self) -> &dyn tsc_ast::NodeStore {
            &self.source_file
        }
        fn symbol(&self, id: tsc_ast::SymbolId) -> &tsc_ast::Symbol {
            &self.symbols[id.0 as usize]
        }
        fn symbol_mut(&mut self, id: tsc_ast::SymbolId) -> &mut tsc_ast::Symbol {
            &mut self.symbols[id.0 as usize]
        }
        fn alloc_symbol(&mut self, symbol: tsc_ast::Symbol) -> tsc_ast::SymbolId {
            self.symbols.push(symbol);
            tsc_ast::SymbolId(self.symbols.len() as u32 - 1)
        }
    }

    pub(crate) fn new_empty_checker() -> Checker {
        let source_file = tsc_parser::parse_source_file(Default::default(), "", ScriptKind::JS);
        crate::new_checker(Box::new(EmptyProgramImpl {
            options: Default::default(),
            source_file,
            symbols: Vec::new(),
        }))
    }

    impl Checker {
        /// `booleanType` is M6 machinery (union construction); the mapper
        /// tests only need a type id DISTINCT from the other intrinsics, and
        /// regularFalseType is one.
        pub(crate) fn boolean_type_or_marker(&self) -> TypeId {
            self.regular_false_type()
        }

        /// Allocates a TypeParameter-carrying type with `is_this_type`.
        pub(crate) fn alloc_test_type_parameter(&mut self, is_this_type: bool) -> TypeId {
            self.alloc_test_type_parameter_full(is_this_type, false, None)
        }

        /// Allocates a distributed type parameter whose constraint is set
        /// (the getNonDistributedTypeParameter shape) — returns (type, constraint).
        pub(crate) fn alloc_test_distributed_type_parameter(&mut self) -> (TypeId, TypeId) {
            let constraint = self.bigint_type();
            let id = self.alloc_test_type_parameter_full(false, true, Some(constraint));
            (id, constraint)
        }

        fn alloc_test_type_parameter_full(
            &mut self,
            is_this_type: bool,
            is_distributed: bool,
            constraint: Option<TypeId>,
        ) -> TypeId {
            let id = TypeId(self.types.len() as u32);
            self.types.push(Type {
                flags: TypeFlags::TYPE_PARAMETER,
                object_flags: ObjectFlags::NONE,
                id,
                symbol: None,
                alias: None,
                checker_shard: self.shard,
                data: TypeData::TypeParameter(Box::new(TypeParameter {
                    is_this_type,
                    is_distributed,
                    constraint,
                    ..Default::default()
                })),
            });
            self.type_count += 1;
            id
        }
    }
}
