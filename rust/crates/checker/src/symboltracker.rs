// Ported from tsc/internal/checker/symboltracker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The symbol tracker harness — M6 W1. `SymbolTrackerImpl` wraps a
// nodebuilder-side `nodebuilder.SymbolTracker` (the interface the
// printer/emit-resolver consumes) and records the tracked symbols into the
// nodebuilder context for late-painted statements.
//
// PORT (ownership): Go's `SymbolTrackerImpl` holds `context
// *NodeBuilderContext` — a shared mutable pointer into the nodebuilder
// serialization state. The port holds `Rc<RefCell<NodeBuilderContext>>`
// (the checker is single-threaded; the standard Rust shape for a Go shared
// pointer). Go's `*ast.Symbol` splits into the payload reference plus the
// `SymbolId` the tracked args record (§5.5 — see the trait PORT note in
// todo_shapes). The nodebuilder wave (M6 wave 4) revisits both shapes when
// it lands the real context.

use std::cell::RefCell;
use std::rc::Rc;

use tsc_ast::{NodeId, Symbol, SymbolFlags, SymbolId};

use crate::todo_shapes::{NodeBuilderContext, SymbolTracker, TrackedSymbolArgs};

/// Go: `type SymbolTrackerImpl struct { context *NodeBuilderContext; inner
/// nodebuilder.SymbolTracker; DisableTrackSymbol bool }`.
///
/// The `'inner` lifetime is the Rust bound for the boxed inner tracker (Go's
/// interface indirection through `*SymbolTrackerImpl` layers — see
/// `take_inner_if_impl`).
pub struct SymbolTrackerImpl<'inner> {
    /// PORT: Go's `context *NodeBuilderContext` — see the module PORT note.
    context: Rc<RefCell<NodeBuilderContext>>,
    inner: Option<Box<dyn SymbolTracker<'inner> + 'inner>>,
    pub disable_track_symbol: bool,
}

/// Go: `func NewSymbolTrackerImpl(context *NodeBuilderContext, tracker nodebuilder.SymbolTracker) *SymbolTrackerImpl`.
///
/// Go's constructor repeatedly unwraps `SymbolTrackerImpl` layers from
/// `tracker` (`for { t, ok := tracker.(*SymbolTrackerImpl); if !ok { break };
/// tracker = t.inner }`) so re-wrapping is idempotent — the port does the
/// same through the trait's `take_inner_if_impl` seam (see todo_shapes).
pub fn new_symbol_tracker_impl<'inner>(
    context: Rc<RefCell<NodeBuilderContext>>,
    tracker: Option<Box<dyn SymbolTracker<'inner> + 'inner>>,
) -> SymbolTrackerImpl<'inner> {
    let mut tracker = tracker;
    loop {
        let Some(mut t) = tracker else { break };
        // Go: `t, ok := tracker.(*SymbolTrackerImpl); if !ok { break };
        // tracker = t.inner` — take the inner tracker and drop the wrapper.
        match t.take_inner_if_impl() {
            Some(nested) => tracker = Some(nested),
            None => {
                tracker = Some(t);
                break;
            }
        }
    }
    SymbolTrackerImpl {
        context,
        inner: tracker,
        disable_track_symbol: false,
    }
}

impl<'inner> SymbolTracker<'inner> for SymbolTrackerImpl<'inner> {
    /// Go: `func (this *SymbolTrackerImpl) TrackSymbol(symbol *ast.Symbol, enclosingDeclaration *ast.Node, meaning ast.SymbolFlags) bool`.
    #[allow(clippy::too_many_arguments)]
    fn track_symbol(
        &mut self,
        symbol: &Symbol,
        symbol_id: SymbolId,
        enclosing_declaration: Option<NodeId>,
        meaning: SymbolFlags,
    ) -> bool {
        if !self.disable_track_symbol {
            if let Some(inner) = self.inner.as_mut() {
                if inner.track_symbol(symbol, symbol_id, enclosing_declaration, meaning) {
                    self.on_diagnostic_reported();
                    return true;
                }
            }
            // Skip recording type parameters as they dont contribute to late
            // painted statements (Go comment, verbatim).
            if !symbol.flags.intersects(SymbolFlags::TYPE_PARAMETER) {
                self.context
                    .borrow_mut()
                    .tracked_symbols
                    .push(TrackedSymbolArgs {
                        symbol: symbol_id,
                        enclosing_declaration,
                        meaning,
                    });
            }
        }
        false
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportInaccessibleThisError()`.
    fn report_inaccessible_this_error(&mut self) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_inaccessible_this_error();
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportPrivateInBaseOfClassExpression(propertyName string)`.
    fn report_private_in_base_of_class_expression(&mut self, property_name: &str) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_private_in_base_of_class_expression(property_name);
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportInaccessibleUniqueSymbolError()`.
    fn report_inaccessible_unique_symbol_error(&mut self) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_inaccessible_unique_symbol_error();
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportCyclicStructureError()`.
    fn report_cyclic_structure_error(&mut self) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_cyclic_structure_error();
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportLikelyUnsafeImportRequiredError(specifier string, symbolName string)`.
    fn report_likely_unsafe_import_required_error(&mut self, specifier: &str, symbol_name: &str) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_likely_unsafe_import_required_error(specifier, symbol_name);
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportTruncationError()`.
    fn report_truncation_error(&mut self) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_truncation_error();
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportNonlocalAugmentation(containingFile *ast.SourceFile, parentSymbol *ast.Symbol, augmentingSymbol *ast.Symbol)`.
    fn report_nonlocal_augmentation(
        &mut self,
        containing_file: NodeId,
        parent_symbol: &Symbol,
        augmenting_symbol: &Symbol,
    ) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_nonlocal_augmentation(containing_file, parent_symbol, augmenting_symbol);
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportNonSerializableProperty(propertyName string)`.
    fn report_non_serializable_property(&mut self, property_name: &str) {
        self.on_diagnostic_reported();
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_non_serializable_property(property_name);
    }

    /// Go: `func (this *SymbolTrackerImpl) ReportInferenceFallback(node *ast.Node)`.
    fn report_inference_fallback(&mut self, node: NodeId) {
        let Some(inner) = self.inner.as_mut() else { return };
        inner.report_inference_fallback(node);
    }

    /// Go: `func (this *SymbolTrackerImpl) PushErrorFallbackNode(node *ast.Node)`.
    fn push_error_fallback_node(&mut self, node: NodeId) {
        let Some(inner) = self.inner.as_mut() else { return };
        inner.push_error_fallback_node(node);
    }

    /// Go: `func (this *SymbolTrackerImpl) PopErrorFallbackNode()`.
    fn pop_error_fallback_node(&mut self) {
        let Some(inner) = self.inner.as_mut() else { return };
        inner.pop_error_fallback_node();
    }

    /// PORT (the unwrap seam): returns the inner tracker so
    /// `new_symbol_tracker_impl`'s loop can strip impl layers (Go:
    /// `tracker = t.inner`).
    fn take_inner_if_impl(&mut self) -> Option<Box<dyn SymbolTracker<'inner> + 'inner>> {
        self.inner.take()
    }
}

impl SymbolTrackerImpl<'_> {
    /// Go: `func (this *SymbolTrackerImpl) onDiagnosticReported()` —
    /// `this.context.reportedDiagnostic = true`.
    fn on_diagnostic_reported(&mut self) {
        self.context.borrow_mut().reported_diagnostic = true;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (Go has no symboltracker_test.go; these pin the harness
// semantics symboltracker.go's body prescribes: inner-tracker precedence, the
// type-parameter skip, DisableTrackSymbol, the unwrap loop, and the
// reportedDiagnostic flag).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A test mock for the nodebuilder-side inner tracker: counts reports and
    /// answers `track_symbol` with a configured result.
    struct MockTracker {
        tracked: Vec<(SymbolId, Option<NodeId>, SymbolFlags)>,
        report_track: bool,
        reports: usize,
    }

    impl SymbolTracker<'_> for MockTracker {
        fn track_symbol(
            &mut self,
            _symbol: &Symbol,
            symbol_id: SymbolId,
            enclosing_declaration: Option<NodeId>,
            meaning: SymbolFlags,
        ) -> bool {
            self.tracked.push((symbol_id, enclosing_declaration, meaning));
            self.report_track
        }
        fn report_inaccessible_this_error(&mut self) {
            self.reports += 1;
        }
        fn report_private_in_base_of_class_expression(&mut self, _property_name: &str) {
            self.reports += 1;
        }
        fn report_inaccessible_unique_symbol_error(&mut self) {
            self.reports += 1;
        }
        fn report_cyclic_structure_error(&mut self) {
            self.reports += 1;
        }
        fn report_likely_unsafe_import_required_error(&mut self, _specifier: &str, _symbol_name: &str) {
            self.reports += 1;
        }
        fn report_truncation_error(&mut self) {
            self.reports += 1;
        }
        fn report_nonlocal_augmentation(
            &mut self,
            _containing_file: NodeId,
            _parent_symbol: &Symbol,
            _augmenting_symbol: &Symbol,
        ) {
            self.reports += 1;
        }
        fn report_non_serializable_property(&mut self, _property_name: &str) {
            self.reports += 1;
        }
        fn report_inference_fallback(&mut self, _node: NodeId) {}
        fn push_error_fallback_node(&mut self, _node: NodeId) {}
        fn pop_error_fallback_node(&mut self) {}
    }

    fn mock<'a>(report_track: bool) -> Box<dyn SymbolTracker<'a> + 'a> {
        Box::new(MockTracker {
            tracked: Vec::new(),
            report_track,
            reports: 0,
        })
    }

    fn value_symbol() -> Symbol {
        Symbol { flags: SymbolFlags::PROPERTY, ..Default::default() }
    }

    fn type_parameter_symbol() -> Symbol {
        Symbol { flags: SymbolFlags::TYPE_PARAMETER, ..Default::default() }
    }

    #[test]
    fn track_symbol_records_into_the_context_without_an_inner() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), None);
        let sym = value_symbol();
        assert!(!tracker.track_symbol(&sym, SymbolId(1), None, SymbolFlags::VALUE));
        assert_eq!(context.borrow().tracked_symbols.len(), 1);
        assert_eq!(context.borrow().tracked_symbols[0].symbol, SymbolId(1));
        assert_eq!(context.borrow().tracked_symbols[0].meaning, SymbolFlags::VALUE);
        assert!(!context.borrow().reported_diagnostic);
    }

    #[test]
    fn track_symbol_skips_type_parameters() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), None);
        let sym = type_parameter_symbol();
        tracker.track_symbol(&sym, SymbolId(2), None, SymbolFlags::TYPE);
        assert!(context.borrow().tracked_symbols.is_empty());
    }

    #[test]
    fn disable_track_symbol_suppresses_everything() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), None);
        tracker.disable_track_symbol = true;
        let sym = value_symbol();
        assert!(!tracker.track_symbol(&sym, SymbolId(1), None, SymbolFlags::VALUE));
        assert!(context.borrow().tracked_symbols.is_empty());
    }

    #[test]
    fn inner_handled_tracking_reports_the_diagnostic_and_short_circuits() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), Some(mock(true /*report_track*/)));
        let sym = value_symbol();
        // Go: `if this.inner != nil && this.inner.TrackSymbol(...) {
        // this.onDiagnosticReported(); return true }`.
        assert!(tracker.track_symbol(&sym, SymbolId(1), None, SymbolFlags::VALUE));
        assert!(context.borrow().reported_diagnostic);
        assert!(context.borrow().tracked_symbols.is_empty());
    }

    #[test]
    fn inner_unhandled_tracking_still_records() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), Some(mock(false)));
        let sym = value_symbol();
        assert!(!tracker.track_symbol(&sym, SymbolId(1), None, SymbolFlags::VALUE));
        assert_eq!(context.borrow().tracked_symbols.len(), 1);
        assert!(!context.borrow().reported_diagnostic);
    }

    #[test]
    fn report_methods_flag_the_context_either_way() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), Some(mock(true)));
        tracker.report_inaccessible_this_error();
        assert!(context.borrow().reported_diagnostic);
        // With no inner, the report still flags the context (Go:
        // onDiagnosticReported runs BEFORE the nil check).
        let mut bare = new_symbol_tracker_impl(context.clone(), None);
        bare.report_cyclic_structure_error();
        assert!(context.borrow().reported_diagnostic);
    }

    #[test]
    fn fallback_methods_forward_only() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        let mut tracker = new_symbol_tracker_impl(context.clone(), None);
        // No inner: pure pass-throughs — no diagnostic flag (Go: no
        // onDiagnosticReported call in these three).
        tracker.report_inference_fallback(NodeId::new(0, 1));
        tracker.push_error_fallback_node(NodeId::new(0, 1));
        tracker.pop_error_fallback_node();
        assert!(!context.borrow().reported_diagnostic);
    }

    #[test]
    fn new_symbol_tracker_impl_unwraps_existing_impl_layers() {
        let context = Rc::new(RefCell::new(NodeBuilderContext::default()));
        // Wrap a SymbolTrackerImpl around a mock, then wrap the constructor
        // around THAT — Go's unwrap loop keeps only the innermost
        // non-SymbolTrackerImpl tracker, so the rewrapped impl behaves like a
        // single layer: one inner.track_symbol call per TrackSymbol.
        let wrapped = new_symbol_tracker_impl(context.clone(), Some(mock(true)));
        let mut tracker = new_symbol_tracker_impl(context.clone(), Some(Box::new(wrapped)));
        let sym = value_symbol();
        assert!(tracker.track_symbol(&sym, SymbolId(1), None, SymbolFlags::VALUE));
        // The mock's handled-tracking propagated through exactly one layer.
        assert!(context.borrow().reported_diagnostic);
        assert!(context.borrow().tracked_symbols.is_empty());
    }
}
