// Ported from tsc/internal/checker/tracer.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The checker's trace surface for `--generateTrace` (M6 W2 — landed first in
// the wave: its output is the SPEC's debugging lifeline and the engine waves'
// trace call sites below route through it). File map: tracer.go → tracer.rs;
// the tracing-session machinery it consumes is the crate-local
// `tracing.rs` port of tsc/internal/tracing (see its header PORT notes).

use std::rc::Rc;

use tsc_ast::SymbolId;

use crate::relater::{get_recursion_identity, RecursionId};
use crate::tracing::{Args, Phase, TracePop, TraceValue, TracedType, Tracing, TypeTracer};
use crate::types::{ObjectFlags, TypeFlags, TypeId};
use crate::Checker;

/// Go: `type Tracer struct { tracing *tracing.Tracing; recorder tracing.Tracer; checkerIndex int }`.
///
/// PORT: `*tracing.Tracing` → the shared [`Tracing`] handle (Rc; see
/// tracing.rs); `tracing.Tracer` (the recorder interface) → the concrete
/// [`Rc<TypeTracer>`] — the checker's Tracer is its only construction site.
/// A nil `*Tracer` is Go's no-op; the port is `Option<Tracer>` on the checker.
pub struct Tracer {
    tracing: Tracing,
    recorder: Rc<TypeTracer>,
    checker_index: i64,
}

/// Go: `func NewTracer(tr *tracing.Tracing, checkerIndex int) *Tracer`.
pub fn new_tracer(tr: &Tracing, checker_index: i64) -> Tracer {
    Tracer {
        tracing: tr.clone(),
        recorder: tr.new_type_tracer(checker_index),
        checker_index,
    }
}

impl Tracer {
    /// Go: `func (t *Tracer) RecordType(typ *Type)`.
    ///
    /// PORT: the adapter is rebuilt at dump time from the stored
    /// (checker_shard, TypeId) pair (see tracing.rs's TypeTracer note), so
    /// record stores the handle pair; the shard is the Go adapter's
    /// `t.checker` identity.
    pub fn record_type(&self, checker_shard: u16, typ: TypeId) {
        self.recorder.record_type(checker_shard, typ);
    }

    /// Go: `func (t *Tracer) Push(phase, name, args, separateBeginAndEnd) func()`.
    pub fn push(&self, phase: Phase, name: &str, args: &Args, separate_begin_and_end: bool) -> TracerPop {
        if !separate_begin_and_end {
            let copied = self.copy_with_checker_index(args);
            return TracerPop {
                inner: Some(self.tracing.push(phase, name, &copied, false)),
                end_args: None,
                checker_index: self.checker_index,
            };
        }
        // Go: args are shared with the caller for the lifetime of the pop
        // (temporarilyAddCheckerIndex mutates the caller's map; the end-event
        // closure re-adds the id before popping and restores after) — the
        // Args Rc sharing reproduces the map aliasing exactly.
        let previous = temporarily_add_checker_index(args, self.checker_index);
        let pop = self.tracing.push(phase, name, args, true);
        restore_checker_index(args, previous);
        TracerPop {
            inner: Some(pop),
            end_args: Some(args.clone()),
            checker_index: self.checker_index,
        }
    }

    /// Go: `func (t *Tracer) Instant(phase, name, args)`.
    pub fn instant(&self, phase: Phase, name: &str, args: &Args) {
        self.tracing.instant(phase, name, &self.copy_with_checker_index(args));
    }

    /// Go: `func (t *Tracer) copyWithCheckerIndex(args) map[string]any` —
    /// the one place Go copies the map.
    fn copy_with_checker_index(&self, args: &Args) -> Args {
        let with_checker_index = Args::new();
        for (key, value) in args.entries() {
            with_checker_index.insert(&key, value);
        }
        with_checker_index.insert("checkerId", TraceValue::Int(self.checker_index));
        with_checker_index
    }
}

/// Go: the `func()` Tracer.Push returns.
pub struct TracerPop {
    inner: Option<TracePop>,
    /// Go: the `args` the end closure re-adds `checkerId` to (the shared
    /// caller map); None for the copied-args (non-separate) path.
    end_args: Option<Args>,
    checker_index: i64,
}

impl TracerPop {
    /// Call the pop function (Go: `popFn()`); dropping also pops (Go's `defer`).
    pub fn pop(self) {
        drop(self);
    }
}

impl Drop for TracerPop {
    fn drop(&mut self) {
        if let Some(args) = &self.end_args {
            let previous = temporarily_add_checker_index(args, self.checker_index);
            // The inner pop (E event) serializes args NOW, checkerId present.
            if let Some(inner) = self.inner.take() {
                inner.pop();
            }
            restore_checker_index(args, previous);
        } else if let Some(inner) = self.inner.take() {
            inner.pop();
        }
    }
}

/// Go: `func (t *Tracer) temporarilyAddCheckerIndex(args) (map, func())` —
/// the port returns the previous value to restore (Go's closure capture).
fn temporarily_add_checker_index(args: &Args, checker_index: i64) -> Option<TraceValue> {
    let previous = args.get("checkerId");
    args.insert("checkerId", TraceValue::Int(checker_index));
    previous
}

fn restore_checker_index(args: &Args, previous: Option<TraceValue>) {
    match previous {
        Some(value) => args.insert("checkerId", value),
        None => {
            args.remove("checkerId");
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// tracedTypeAdapter (Go: adapts a Type to tracing.TracedType)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type tracedTypeAdapter struct { t *Type; checker *Checker }` — built
/// at dump time over the stored handle pair (see the RecordType PORT note);
/// the checker's shard must match the recording checker's.
pub(crate) struct TracedTypeAdapter<'a> {
    c: &'a Checker,
    t: TypeId,
}

/// Go: `func wrapType(t *Type) tracing.TracedType` — nil in, nil out is the
/// dump site's `if t == nil` guard; the port's recorder never stores nil.
pub(crate) fn wrap_type<'a>(c: &'a Checker, checker_shard: u16, t: TypeId) -> TracedTypeAdapter<'a> {
    debug_assert_eq!(c.shard, checker_shard, "cross-shard trace dump");
    TracedTypeAdapter { c, t }
}

impl TracedTypeAdapter<'_> {
    fn ty(&self) -> &crate::types::Type {
        self.c.type_(self.t)
    }
}

impl TracedType for TracedTypeAdapter<'_> {
    fn id(&self) -> u32 {
        self.t.0
    }

    fn format_flags(&self) -> Vec<&'static str> {
        crate::types::format_type_flags(self.ty().flags)
    }

    fn is_conditional(&self) -> bool {
        self.ty().flags.intersects(TypeFlags::CONDITIONAL)
    }

    fn symbol(&self) -> Option<(SymbolId, String)> {
        self.ty()
            .symbol
            .map(|s| (s, self.c.program_symbol(s).name.clone()))
    }

    fn alias_symbol(&self) -> Option<(SymbolId, String)> {
        let alias = self.ty().alias.as_deref()?;
        alias
            .symbol
            .map(|s| (s, self.c.program_symbol(s).name.clone()))
    }

    fn alias_type_argument_ids(&self) -> Vec<u32> {
        match self.ty().alias.as_deref() {
            Some(alias) if !alias.type_arguments.is_empty() => {
                alias.type_arguments.iter().map(|t| t.0).collect()
            }
            _ => Vec::new(),
        }
    }

    fn first_declaration(&self) -> Option<tsc_ast::NodeId> {
        // Go: firstDeclSymbol = aliasSymbol ?? symbol; Declarations[0].
        let symbol = self
            .ty()
            .alias
            .as_deref()
            .and_then(|a| a.symbol)
            .or(self.ty().symbol)?;
        let declarations = self.c.program_symbol(symbol).declarations.clone();
        declarations.first().copied()
    }

    fn intrinsic_name(&self) -> String {
        if !self.ty().flags.intersects(TypeFlags::INTRINSIC) {
            return String::new();
        }
        self.ty()
            .as_intrinsic_type()
            .map(|d| d.intrinsic_name.clone())
            .unwrap_or_default()
    }

    fn union_type_ids(&self) -> Vec<u32> {
        if !self.ty().flags.intersects(TypeFlags::UNION) {
            return Vec::new();
        }
        self.ty()
            .as_union_type()
            .map(|u| u.base.types.iter().map(|t| t.0).collect())
            .unwrap_or_default()
    }

    fn intersection_type_ids(&self) -> Vec<u32> {
        if !self.ty().flags.intersects(TypeFlags::INTERSECTION) {
            return Vec::new();
        }
        self.ty()
            .as_intersection_type()
            .map(|u| u.base.types.iter().map(|t| t.0).collect())
            .unwrap_or_default()
    }

    fn index_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::INDEX) {
            return None;
        }
        self.ty().as_index_type().and_then(|d| d.target).map(|t| t.0)
    }

    fn indexed_access_object_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return None;
        }
        self.ty()
            .as_indexed_access_type()
            .and_then(|d| d.object_type)
            .map(|t| t.0)
    }

    fn indexed_access_index_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return None;
        }
        self.ty()
            .as_indexed_access_type()
            .and_then(|d| d.index_type)
            .map(|t| t.0)
    }

    fn conditional_check_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::CONDITIONAL) {
            return None;
        }
        self.ty()
            .as_conditional_type()
            .and_then(|d| d.check_type)
            .map(|t| t.0)
    }

    fn conditional_extends_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::CONDITIONAL) {
            return None;
        }
        self.ty()
            .as_conditional_type()
            .and_then(|d| d.extends_type)
            .map(|t| t.0)
    }

    fn conditional_true_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::CONDITIONAL) {
            return None;
        }
        self.ty()
            .as_conditional_type()
            .and_then(|d| d.resolved_true_type)
            .map(|t| t.0)
    }

    fn conditional_false_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::CONDITIONAL) {
            return None;
        }
        self.ty()
            .as_conditional_type()
            .and_then(|d| d.resolved_false_type)
            .map(|t| t.0)
    }

    fn substitution_base_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::SUBSTITUTION) {
            return None;
        }
        self.ty()
            .as_substitution_type()
            .and_then(|d| d.base_type)
            .map(|t| t.0)
    }

    fn substitution_constraint_type_id(&self) -> Option<u32> {
        if !self.ty().flags.intersects(TypeFlags::SUBSTITUTION) {
            return None;
        }
        self.ty()
            .as_substitution_type()
            .and_then(|d| d.constraint)
            .map(|t| t.0)
    }

    fn reference_target_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REFERENCE)
        {
            return None;
        }
        ty.as_type_reference().and_then(|d| d.base.target).map(|t| t.0)
    }

    fn reference_type_argument_ids(&self) -> Vec<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REFERENCE)
        {
            return Vec::new();
        }
        ty.as_type_reference()
            .map(|d| d.resolved_type_arguments.iter().map(|t| t.0).collect())
            .unwrap_or_default()
    }

    fn reference_node(&self) -> Option<tsc_ast::NodeId> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REFERENCE)
        {
            return None;
        }
        ty.as_type_reference().and_then(|d| d.node)
    }

    fn reverse_mapped_source_type_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REVERSE_MAPPED)
        {
            return None;
        }
        ty.as_reverse_mapped_type().and_then(|d| d.source).map(|t| t.0)
    }

    fn reverse_mapped_mapped_type_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REVERSE_MAPPED)
        {
            return None;
        }
        ty.as_reverse_mapped_type().and_then(|d| d.mapped_type).map(|t| t.0)
    }

    fn reverse_mapped_constraint_type_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::REVERSE_MAPPED)
        {
            return None;
        }
        ty.as_reverse_mapped_type()
            .and_then(|d| d.constraint_type)
            .map(|t| t.0)
    }

    fn evolving_array_element_type_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::EVOLVING_ARRAY)
        {
            return None;
        }
        ty.as_evolving_array_type().and_then(|d| d.element_type).map(|t| t.0)
    }

    fn evolving_array_final_type_id(&self) -> Option<u32> {
        let ty = self.ty();
        if !ty.flags.intersects(TypeFlags::OBJECT)
            || !ty.object_flags.intersects(ObjectFlags::EVOLVING_ARRAY)
        {
            return None;
        }
        ty.as_evolving_array_type().and_then(|d| d.final_array_type).map(|t| t.0)
    }

    fn is_tuple(&self) -> bool {
        self.ty().object_flags.intersects(ObjectFlags::TUPLE)
    }

    fn pattern(&self) -> Option<tsc_ast::NodeId> {
        // Go: a.checker.patternForType[a.t] — the checker's patternForType
        // map lands with the destructuring-typing wave.
        // TODO(port M6 S2): patternForType (destructuring pattern tracking).
        None
    }

    fn recursion_identity(&self) -> RecursionId {
        get_recursion_identity(self.c, self.t)
    }

    fn display(&self) -> String {
        // Go: TypeToString for Anonymous|Literal|Template|Union|Intersection
        // types (with the recover-suppressed panic). The port's TypeToString
        // is the nodebuilder wave (M6 W4); until it lands the display field
        // is empty — the trace tests PORT-note this and the end-to-end trace
        // diff (S1's checkSourceFile) re-verifies the field once W4 lands.
        // TODO(port M6 W4): checker.TypeToString (nodebuilder type-to-string).
        String::new()
    }
}

impl TypeTracer {
    /// Go: `func (t *typeTracer) DumpTypes() error` — the host calls this for
    /// each checker's tracer before `Tracing::stop_tracing` (see tracing.rs's
    /// StopTracing PORT note for why the checker is a parameter here).
    pub fn dump_types(&self, c: &Checker) -> Result<(), String> {
        let recorded = self.recorded_types();
        if recorded.is_empty() {
            return Ok(());
        }
        let wrappers: Vec<_> = recorded
            .iter()
            .map(|&(shard, t)| wrap_type(c, shard, t))
            .collect();
        let adapters: Vec<&dyn TracedType> = wrappers
            .iter()
            .map(|w| w as &dyn TracedType)
            .collect();
        let content = crate::tracing::dump_types_to_string(&adapters);
        self.write_dump(&content)
    }
}
