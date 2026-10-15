//! M6 W3: flow.go — the flow-graph consumer + narrowing engine.
//!
//! Go source: `tsc/internal/checker/flow.go` (2,761 lines), ported
//! function-by-function, same names snake_cased, same control flow and
//! evaluation order.
//!
//! PORT notes (decisions where Rust forces a change):
//!
//!   * FlowType — Go's `FlowType{t *Type, incomplete bool}` where `t == nil`
//!     is load-bearing (the "assignment doesn't affect reference" answer).
//!     The port carries `t: Option<TypeId>`.
//!
//!   * FlowState free list — Go pools `*FlowState` through a `next` chain
//!     (`freeFlowState`). The port keeps a `Vec<FlowState>` pool
//!     (`Checker::free_flow_states`): `get_flow_state` pops or constructs;
//!     `put_flow_state` returns. Pooling is an allocation optimization only —
//!     semantics are value-identical (each live state is exclusively owned
//!     by one invocation; nested `isReachableFlowNode`/`isPostSuperFlowNode`
//!     invocations take their own).
//!
//!   * Flow arena — Go's `*ast.FlowNode`s are GC-heap; the port's binder
//!     hands them back per-file in `BindResult{flow_nodes, flow_lists}` and
//!     the checker takes a copy at construction (the checker only READS
//!     them; `Program::flow_nodes()`/`flow_lists()` are the seam).
//!
//!   * Synthetic references (`getFlowTypeInConstructor` /
//!     `getFlowTypeInStaticBlocks`, Go's `factory.NewPropertyAccessExpression`)
//!     — the port allocates a real node through the `NodeStore::alloc` seam
//!     (`Program::node_store_mut`), mirroring Go's pointer-graph growth.
//!
//!   * `getFlowNodeOfNode` — Go reads `node.FlowNodeData()` (the
//!     StatementBase/AccessorDeclarationBase/Identifier/... embedders); the
//!     port matches every `NodeData` variant whose struct carries a
//!     `flow_node` payload field (the exact set the port's binder writes).
//!
//!   * Closures over the checker — Go's `everyType`/`someType` predicates
//!     freely call back into the checker; the port's `every_type`/`some_type`
//!     take the checker mutably, so this module carries `every_type_c` /
//!     `some_type_c` twins whose predicates receive `&mut Checker` (same
//!     evaluation order; the non-checker predicates keep using the relater
//!     forms).

use std::cell::Cell;

use tsc_ast::{
    FlowNodeId, Kind, Node, NodeData, NodeFlags as NF, NodeId, NodeStore, SymbolFlags, SymbolId,
    TextRange,
};
use tsc_ast::flow::{
    FlowFlags, FlowListId, FlowNode, FlowNodeData, FlowReduceLabelData, FlowSwitchClauseData,
};
use tsc_binder::ast_shims as binder_shims;
use tsc_binder::node_data as binder_node_data;
use tsc_diagnostics::{
    MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE, THE_CONTAINING_FUNCTION_OR_MODULE_BODY_IS_TOO_LARGE_FOR_CONTROL_FLOW_ANALYSIS,
};
use tsc_core::tristate::Tristate;

use crate::checker::{CheckMode, Checker};
use crate::relater::{self, KeyBuilder, RelationKind, TypeFacts};
use crate::todo_shapes::{CacheHashKey, CachedTypeKind, CachedTypeKey, UnionReduction};
use crate::types::{
    ExhaustiveState, NodeCheckFlags, ObjectFlags, SignatureId, SignatureKind, TypeFlags, TypeId,
    TypePredicate, TypePredicateKind,
};
use crate::utilities as u;

// ────────────────────────────────────────────────────────────────────────────
// FlowType / FlowState / SharedFlow (flow.go:14-67)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type FlowType struct { t *Type; incomplete bool }`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FlowType {
    pub(crate) t: Option<TypeId>,
    pub(crate) incomplete: bool,
}

impl FlowType {
    /// Go: `func (ft *FlowType) isNil() bool`.
    #[inline]
    pub(crate) fn is_nil(&self) -> bool {
        self.t.is_none()
    }
}

/// Go: `type SharedFlow struct { flow *ast.FlowNode; flowType FlowType }`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SharedFlow {
    pub(crate) flow: FlowNodeId,
    pub(crate) flow_type: FlowType,
}

/// Go: `type FlowState struct` (flow.go:31).
pub(crate) struct FlowState {
    pub(crate) reference: NodeId,
    pub(crate) declared_type: TypeId,
    pub(crate) initial_type: TypeId,
    pub(crate) flow_container: NodeId,
    pub(crate) ref_key: CacheHashKey,
    pub(crate) depth: usize,
    pub(crate) shared_flow_start: usize,
    pub(crate) reduce_labels: Vec<FlowReduceLabelData>,
}

/// Go: `type FlowLoopInfo struct` (flow.go:1320 region).
pub(crate) struct FlowLoopInfo {
    pub(crate) key: (FlowNodeId, CacheHashKey),
    pub(crate) types: Vec<TypeId>,
}

/// Go: `nonDottedNameCacheKey = CacheHashKey(xxh3.HashString128("?"))` — a
/// distinct fixed hash that never collides with a real dotted-name key.
/// PORT: the port's CacheHashKey is the KeyBuilder's FNV-1a 64 over written
/// payload bytes; the sentinel is a fixed pattern outside that output space
/// (the builder only ever hashes FNV-1a over bytes; the sentinel matches
/// Go's contract of "distinct, never produced by writeFlowCacheKey").
pub(crate) const NON_DOTTED_NAME_CACHE_KEY: CacheHashKey = CacheHashKey(0x3f3f_3f3f_0101_0101);

// ── every_type/some_type twins whose predicates see the checker ──

/// See the module PORT note (closures over the checker).
pub(crate) fn every_type_c(
    c: &mut Checker,
    t: TypeId,
    mut f: impl FnMut(&mut Checker, TypeId) -> bool,
) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let constituents = c.type_(t).types().to_vec();
        constituents.iter().all(|&tt| f(c, tt))
    } else {
        f(c, t)
    }
}

/// See the module PORT note (closures over the checker).
pub(crate) fn some_type_c(
    c: &mut Checker,
    t: TypeId,
    mut f: impl FnMut(&mut Checker, TypeId) -> bool,
) -> bool {
    if c.type_(t).flags.intersects(TypeFlags::UNION) {
        let constituents = c.type_(t).types().to_vec();
        constituents.iter().any(|&tt| f(c, tt))
    } else {
        f(c, t)
    }
}

/// The flow node's associated AST node (Go: `flow.Node`; nil → None).
#[inline]
fn flow_node_payload(flow: &FlowNode) -> Option<NodeId> {
    match &flow.node {
        FlowNodeData::Node(n) => Some(*n),
        _ => None,
    }
}

impl Checker {
    /// Go: `func (c *Checker) newFlowType(t *Type, incomplete bool) FlowType`.
    pub(crate) fn new_flow_type(&mut self, t: TypeId, incomplete: bool) -> FlowType {
        let t = if incomplete && self.type_(t).flags.intersects(TypeFlags::NEVER) {
            self.silent_never_type()
        } else {
            t
        };
        FlowType { t: Some(t), incomplete }
    }

    /// Go: `func (c *Checker) getFlowState() *FlowState` — PORT: the pointer
    /// free-list is a value pool (see the module header).
    pub(crate) fn get_flow_state(&mut self) -> FlowState {
        self.free_flow_states.pop().unwrap_or(FlowState {
            reference: NodeId(0),
            declared_type: TypeId(0),
            initial_type: TypeId(0),
            flow_container: NodeId(0),
            ref_key: CacheHashKey(0),
            depth: 0,
            shared_flow_start: 0,
            reduce_labels: Vec::new(),
        })
    }

    /// Go: `func (c *Checker) putFlowState(f *FlowState)`.
    pub(crate) fn put_flow_state(&mut self, f: FlowState) {
        self.free_flow_states.push(f);
    }

    // ── flow arena accessors (Go: direct `*FlowNode` reads) ──

    #[inline]
    pub(crate) fn flow(&self, id: FlowNodeId) -> &FlowNode {
        &self.flow_nodes[id.0 as usize]
    }
}

/// Go: `func getFlowNodeOfNode(node *ast.Node) *ast.FlowNode` — the general
/// payload accessor (every NodeData variant whose Go embedder carries
/// FlowNodeBase; the exact set the port's binder writes `flow_node` on).
pub(crate) fn flow_node_of_node(store: &dyn NodeStore, node: NodeId) -> Option<FlowNodeId> {
    match &store.node(node).data {
        NodeData::Identifier(d) => d.flow_node,
        NodeData::QualifiedName(d) => d.flow_node,
        NodeData::EmptyStatement(d) => d.flow_node,
        NodeData::IfStatement(d) => d.flow_node,
        NodeData::DoStatement(d) => d.flow_node,
        NodeData::WhileStatement(d) => d.flow_node,
        NodeData::ForStatement(d) => d.flow_node,
        NodeData::ForInOrOfStatement(d) => d.flow_node,
        NodeData::BreakStatement(d) => d.flow_node,
        NodeData::ContinueStatement(d) => d.flow_node,
        NodeData::ReturnStatement(d) => d.flow_node,
        NodeData::WithStatement(d) => d.flow_node,
        NodeData::SwitchStatement(d) => d.flow_node,
        NodeData::ThrowStatement(d) => d.flow_node,
        NodeData::TryStatement(d) => d.flow_node,
        NodeData::DebuggerStatement(d) => d.flow_node,
        NodeData::LabeledStatement(d) => d.flow_node,
        NodeData::ExpressionStatement(d) => d.flow_node,
        NodeData::Block(d) => d.flow_node,
        NodeData::VariableStatement(d) => d.flow_node,
        NodeData::BindingElement(d) => d.flow_node,
        NodeData::MissingDeclaration(d) => d.flow_node,
        NodeData::FunctionDeclaration(d) => d.flow_node,
        NodeData::ClassDeclaration(d) => d.flow_node,
        NodeData::InterfaceDeclaration(d) => d.flow_node,
        NodeData::TypeAliasDeclaration(d) => d.flow_node,
        NodeData::EnumDeclaration(d) => d.flow_node,
        NodeData::ModuleBlock(d) => d.flow_node,
        NodeData::NotEmittedStatement(d) => d.flow_node,
        NodeData::ImportDeclaration(d) => d.flow_node,
        NodeData::ExportAssignment(d) => d.flow_node,
        NodeData::NamespaceExportDeclaration(d) => d.flow_node,
        NodeData::GetAccessorDeclaration(d) => d.flow_node,
        NodeData::SetAccessorDeclaration(d) => d.flow_node,
        NodeData::MethodDeclaration(d) => d.flow_node,
        NodeData::KeywordExpression(d) => d.flow_node,
        NodeData::ArrowFunction(d) => d.flow_node,
        NodeData::FunctionExpression(d) => d.flow_node,
        NodeData::PropertyAccessExpression(d) => d.flow_node,
        NodeData::ElementAccessExpression(d) => d.flow_node,
        NodeData::MetaProperty(d) => d.flow_node,
        NodeData::ModuleDeclaration(d) => d.flow_node,
        NodeData::ImportEqualsDeclaration(d) => d.flow_node,
        NodeData::ExportDeclaration(d) => d.flow_node,
        _ => None,
    }
}

/// Go: `func getBranchLabelAntecedents(flow, reduceLabels) *ast.FlowList`
/// (flow.go:208). PORT: returns the list-head cell id (`None` = Go's nil
/// list).
pub(crate) fn get_branch_label_antecedents(
    c: &Checker,
    flow: FlowNodeId,
    reduce_labels: &[FlowReduceLabelData],
) -> Option<FlowListId> {
    let mut i = reduce_labels.len();
    while i != 0 {
        i -= 1;
        let data = &reduce_labels[i];
        if data.target == flow {
            return data.antecedents;
        }
    }
    c.flow(flow).antecedents
}

impl Checker {
    /// Go: `func (c *Checker) getFlowTypeOfReference(reference, declaredType) *Type`
    /// (flow.go:77). PORT (FCS tranche 12): live callers — tryGetThisTypeAtEx
    /// (checker.go:12401/12412) and getFlowTypeOfAccessExpression-adjacent
    /// reference paths; the former dead_code allow is retired.
    pub(crate) fn get_flow_type_of_reference(&mut self, reference: NodeId, declared_type: TypeId) -> TypeId {
        self.get_flow_type_of_reference_ex(reference, declared_type, declared_type, NodeId(0), None)
    }

    /// Go: `func (c *Checker) getFlowTypeOfReferenceEx(reference, declaredType, initialType, flowContainer, flowNode) *Type`
    /// (flow.go:81). PORT: Go's nil `*ast.Node` flowContainer is `NodeId(0)`
    /// (no-parent sentinel; `write_flow_cache_key` checks `!= 0`).
    pub(crate) fn get_flow_type_of_reference_ex(
        &mut self,
        reference: NodeId,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: NodeId,
        flow_node: Option<FlowNodeId>,
    ) -> TypeId {
        if self.flow_analysis_disabled {
            return self.error_type();
        }
        let flow_node = match flow_node {
            Some(node) => Some(node),
            None => flow_node_of_node(self.store(), reference),
        };
        let Some(flow_node) = flow_node else {
            return declared_type;
        };
        let mut f = self.get_flow_state();
        f.reference = reference;
        f.declared_type = declared_type;
        f.initial_type = initial_type;
        f.flow_container = flow_container;
        f.shared_flow_start = self.shared_flows.len();
        self.flow_invocation_count += 1;
        let evolved_type = self
            .get_type_at_flow_node(&mut f, flow_node)
            .t
            .unwrap_or_else(|| self.error_type());
        self.shared_flows.truncate(f.shared_flow_start);
        // When the reference is 'x' in an 'x.length', 'x.push(value)', 'x.unshift(value)' or
        // 'x[n] = value' operation, we give type 'any[]' to 'x' instead of using the type
        // determined by control flow analysis such that operations on empty arrays are possible
        // without implicit any errors and new element types can be inferred without type
        // mismatch errors.
        let result_type = if self.type_(evolved_type).object_flags.intersects(ObjectFlags::EVOLVING_ARRAY)
            && self.is_evolving_array_operation_target(reference)
        {
            self.auto_array_type.expect("auto_array_type")
        } else {
            self.finalize_evolving_array_type(evolved_type)
        };
        // Go: `resultType == c.unreachableNeverType() || parent != reference &&
        // parent.Kind == KindNonNullExpression && !(NEVER) && withFacts is NEVER`.
        let parent = self.store().node(reference).parent.get();
        let parent_is_non_null =
            parent != reference && self.store().node(parent).kind == Kind::NonNullExpression;
        let mut is_inferred_never = false;
        if parent_is_non_null && !self.type_(result_type).flags.intersects(TypeFlags::NEVER) {
            let with_facts =
                relater::get_type_with_facts(self, result_type, TypeFacts::NE_UNDEFINED_OR_NULL);
            is_inferred_never = self.type_(with_facts).flags.intersects(TypeFlags::NEVER);
        }
        if result_type == self.unreachable_never_type() || is_inferred_never {
            return declared_type;
        }
        result_type
    }

    /// Go: `func (c *Checker) getTypeAtFlowNode(f *FlowState, flow *ast.FlowNode) FlowType`
    /// (flow.go:117).
    fn get_type_at_flow_node(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        if f.depth == 2000 {
            // We have made 2000 recursive invocations. To avoid overflowing the call stack we report an error
            // and disable further control flow analysis in the containing function or module body.
            self.flow_analysis_disabled = true;
            let reference = f.reference;
            self.report_flow_control_error(reference);
            return FlowType { t: Some(self.error_type()), incomplete: false };
        }
        f.depth += 1;
        let mut shared_flow: Option<FlowNodeId> = None;
        let mut flow = flow;
        let t;
        loop {
            let flags = self.flow(flow).flags;
            if flags.intersects(FlowFlags::SHARED) {
                // We cache results of flow type resolution for shared nodes that were previously
                // visited in the same getFlowTypeOfReference invocation. A node is considered
                // shared when it is the antecedent of more than one node.
                let mut hit: Option<FlowType> = None;
                for i in f.shared_flow_start..self.shared_flows.len() {
                    if self.shared_flows[i].flow == flow {
                        hit = Some(self.shared_flows[i].flow_type);
                        break;
                    }
                }
                if let Some(cached) = hit {
                    f.depth -= 1;
                    return cached;
                }
                shared_flow = Some(flow);
            }
            let ft;
            if flags.intersects(FlowFlags::ASSIGNMENT) {
                ft = self.get_type_at_flow_assignment(f, flow);
                if ft.is_nil() {
                    flow = self.flow(flow).antecedent.expect("assignment carries antecedent");
                    continue;
                }
            } else if flags.intersects(FlowFlags::CALL) {
                ft = self.get_type_at_flow_call(f, flow);
                if ft.is_nil() {
                    flow = self.flow(flow).antecedent.expect("call carries antecedent");
                    continue;
                }
            } else if flags.intersects(FlowFlags::CONDITION) {
                ft = self.get_type_at_flow_condition(f, flow);
            } else if flags.intersects(FlowFlags::SWITCH_CLAUSE) {
                ft = self.get_type_at_switch_clause(f, flow);
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                let antecedents = get_branch_label_antecedents(self, flow, &f.reduce_labels);
                if antecedents.is_none() {
                    flow = self.flow(flow).antecedent.expect("single-antecedent branch label");
                    continue;
                }
                ft = self.get_type_at_flow_branch_label(f, flow, antecedents);
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                if self.flow(flow).antecedents.is_none() {
                    flow = self
                        .flow(flow)
                        .antecedent
                        .expect("loop label with a single antecedent carries it on antecedent");
                    continue;
                }
                ft = self.get_type_at_flow_loop_label(f, flow);
            } else if flags.intersects(FlowFlags::ARRAY_MUTATION) {
                ft = self.get_type_at_flow_array_mutation(f, flow);
                if ft.is_nil() {
                    flow = self.flow(flow).antecedent.expect("mutation carries antecedent");
                    continue;
                }
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                let data = match &self.flow(flow).node {
                    FlowNodeData::ReduceLabel(d) => d.clone(),
                    _ => panic!("reduce label flow node carries FlowReduceLabelData"),
                };
                f.reduce_labels.push(data);
                let antecedent = self.flow(flow).antecedent.expect("reduce label carries antecedent");
                ft = self.get_type_at_flow_node(f, antecedent);
                f.reduce_labels.pop();
            } else if flags.intersects(FlowFlags::START) {
                // Check if we should continue with the control flow of the containing function.
                let container = flow_node_payload(self.flow(flow));
                let mut continued = false;
                if let Some(container) = container {
                    let reference_kind = self.store().node(f.reference).kind;
                    if container != f.flow_container
                        && !tsc_ast::is_access_expression(self.store().node(f.reference))
                        && !(reference_kind == Kind::ThisKeyword
                            && self.store().node(container).kind != Kind::ArrowFunction)
                        && let Some(container_flow) = flow_node_of_node(self.store(), container)
                    {
                        flow = container_flow;
                        continued = true;
                    }
                }
                if continued {
                    continue;
                }
                // At the top of the flow we have the initial type.
                ft = FlowType { t: Some(f.initial_type), incomplete: false };
            } else {
                // Unreachable code errors are reported in the binding phase. Here we
                // simply return the non-auto declared type to reduce follow-on errors.
                ft = FlowType {
                    t: Some(self.convert_auto_to_any(f.declared_type)),
                    incomplete: false,
                };
            }
            if let Some(shared) = shared_flow {
                // Record visited node and the associated type in the cache.
                self.shared_flows.push(SharedFlow { flow: shared, flow_type: ft });
            }
            t = ft;
            break;
        }
        f.depth -= 1;
        t
    }

    /// Go: `func (c *Checker) getTypeAtFlowAssignment(f, flow) FlowType` (flow.go:220).
    fn get_type_at_flow_assignment(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        let Some(node) = flow_node_payload(self.flow(flow)) else {
            return FlowType { t: None, incomplete: false };
        };
        // Assignments only narrow the computed type if the declared type is a union type. Thus, we
        // only need to evaluate the assigned type if the declared type is a union type.
        if self.is_matching_reference(f.reference, node) {
            if !self.is_reachable_flow_node(flow) {
                return FlowType { t: Some(self.unreachable_never_type()), incomplete: false };
            }
            let antecedent = self.flow(flow).antecedent.expect("assignment carries antecedent");
            if u::get_assignment_target_kind(self.store(), node) == u::AssignmentKind::Compound {
                let flow_type = self.get_type_at_flow_node(f, antecedent);
                let base =
                    self.get_base_type_of_literal_type(flow_type.t.unwrap_or_else(|| self.never_type()));
                return self.new_flow_type(base, flow_type.incomplete);
            }
            if f.declared_type == self.auto_type() || Some(f.declared_type) == self.auto_array_type {
                if self.is_empty_array_assignment(node) {
                    return FlowType {
                        t: Some(self.get_evolving_array_type(self.never_type())),
                        incomplete: false,
                    };
                }
                let initial_or_assigned = self.get_initial_or_assigned_type(f, flow);
                let assigned_type = self.get_widened_literal_type(initial_or_assigned);
                if self.is_type_assignable_to(assigned_type, f.declared_type) {
                    return FlowType { t: Some(assigned_type), incomplete: false };
                }
                return FlowType {
                    t: Some(self.auto_array_type.expect("auto_array_type")),
                    incomplete: false,
                };
            }
            let mut t = f.declared_type;
            if u::is_in_compound_like_assignment(self.store(), node) {
                t = self.get_base_type_of_literal_type(t);
            }
            if self.type_(t).flags.intersects(TypeFlags::UNION) {
                let assigned = self.get_initial_or_assigned_type(f, flow);
                return FlowType {
                    t: Some(self.get_assignment_reduced_type(t, assigned)),
                    incomplete: false,
                };
            }
            return FlowType { t: Some(t), incomplete: false };
        }
        // We didn't have a direct match. However, if the reference is a dotted name, this
        // may be an assignment to a left hand part of the reference. For example, for a
        // reference 'x.y.z', we may be at an assignment to 'x.y' or 'x'. In that case,
        // return the declared type.
        if self.contains_matching_reference(f.reference, node) {
            if !self.is_reachable_flow_node(flow) {
                return FlowType { t: Some(self.unreachable_never_type()), incomplete: false };
            }
            // A matching dotted name might also be an expando property on a function *expression*,
            // in which case we continue control flow analysis back to the function's declaration
            if self.store().node(node).kind == Kind::VariableDeclaration
                && (self.store().node(node).flags.intersects(NF::JAVASCRIPT_FILE) || self.is_var_const_like(node))
                && let Some(init) = crate::ast_shims::initializer_of(self.store(), node)
                && binder_shims::is_function_expression_or_arrow_function(self.store().node(init))
            {
                let antecedent = self.flow(flow).antecedent.expect("assignment carries antecedent");
                return self.get_type_at_flow_node(f, antecedent);
            }
            return FlowType { t: Some(f.declared_type), incomplete: false };
        }
        // for (const _ in ref) acts as a nonnull on ref
        let parent = self.store().node(node).parent.get();
        let grandparent = self.store().node(parent).parent.get();
        if self.store().node(node).kind == Kind::VariableDeclaration
            && self.store().node(grandparent).kind == Kind::ForInStatement
            && let Some(grandparent_expr) = crate::ast_shims::expression_of(self.store(), grandparent)
            && (self.is_matching_reference(f.reference, grandparent_expr)
                || self.optional_chain_contains_reference(grandparent_expr, f.reference))
        {
            let antecedent = self.flow(flow).antecedent.expect("assignment carries antecedent");
            let at = self
                .get_type_at_flow_node(f, antecedent)
                .t
                .unwrap_or_else(|| self.never_type());
            let finalized = self.finalize_evolving_array_type(at);
            return FlowType {
                t: Some(self.get_non_nullable_type_if_needed(finalized)),
                incomplete: false,
            };
        }
        // Assignment doesn't affect reference
        FlowType { t: None, incomplete: false }
    }

    /// Go: `func (c *Checker) getInitialOrAssignedType(f, flow) *Type` (flow.go:276).
    fn get_initial_or_assigned_type(&mut self, f: &mut FlowState, flow: FlowNodeId) -> TypeId {
        let Some(node) = flow_node_payload(self.flow(flow)) else {
            return self.error_type();
        };
        let kind = self.store().node(node).kind;
        if kind == Kind::VariableDeclaration || kind == Kind::BindingElement {
            let t = self.get_initial_type_of(node);
            self.get_narrowable_type_for_reference_real(t, f.reference, CheckMode::CHECK_MODE_NORMAL)
        } else {
            let t = self.get_assigned_type(node);
            self.get_narrowable_type_for_reference_real(t, f.reference, CheckMode::CHECK_MODE_NORMAL)
        }
    }

    /// Go: `func (c *Checker) isEmptyArrayAssignment(node) bool` (flow.go:283).
    fn is_empty_array_assignment(&self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        if kind == Kind::VariableDeclaration
            && let Some(init) = crate::ast_shims::initializer_of(self.store(), node)
            && u::is_empty_array_literal(self.store(), init)
        {
            return true;
        }
        if kind != Kind::BindingElement {
            let parent = self.store().node(node).parent.get();
            if self.store().node(parent).kind == Kind::BinaryExpression {
                if let Some(binary) = self.store().node(parent).as_binary_expression() {
                    return u::is_empty_array_literal(self.store(), binary.right);
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) getTypeAtFlowCall(f, flow) FlowType` (flow.go:288).
    fn get_type_at_flow_call(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        let Some(node) = flow_node_payload(self.flow(flow)) else {
            return FlowType { t: None, incomplete: false };
        };
        if let Some(signature) = self.get_effects_signature(node) {
            let predicate = self.get_type_predicate_of_signature(signature);
            if let Some(predicate) = predicate {
                if predicate.kind == TypePredicateKind::AssertsThis
                    || predicate.kind == TypePredicateKind::AssertsIdentifier
                {
                    let antecedent = self.flow(flow).antecedent.expect("call carries antecedent");
                    let flow_type = self.get_type_at_flow_node(f, antecedent);
                    let t = self.finalize_evolving_array_type(
                        flow_type.t.unwrap_or_else(|| self.never_type()),
                    );
                    let narrowed_type;
                    if let Some(predicate_t) = predicate.t {
                        narrowed_type = self.narrow_type_by_type_predicate(f, t, &predicate, node, true, predicate_t);
                    } else if predicate.kind == TypePredicateKind::AssertsIdentifier
                        && predicate.parameter_index >= 0
                        && (predicate.parameter_index as usize)
                            < binder_node_data::arguments_of(self.store(), node).len()
                    {
                        let arg = binder_node_data::arguments_of(self.store(), node)
                            [predicate.parameter_index as usize];
                        narrowed_type = self.narrow_type_by_assertion(f, t, arg);
                    } else {
                        narrowed_type = t;
                    }
                    if narrowed_type == t {
                        return flow_type;
                    }
                    return self.new_flow_type(narrowed_type, flow_type.incomplete);
                }
                let return_type = self.get_return_type_of_signature(signature);
                if self.type_(return_type).flags.intersects(TypeFlags::NEVER) {
                    return FlowType { t: Some(self.unreachable_never_type()), incomplete: false };
                }
            }
        }
        FlowType { t: None, incomplete: false }
    }

    /// Go: `func (c *Checker) narrowTypeByTypePredicate(f, t, predicate, callExpression, assumeTrue) *Type`
    /// (flow.go:316).
    fn narrow_type_by_type_predicate(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        predicate: &TypePredicate,
        call_expression: NodeId,
        assume_true: bool,
        predicate_t: TypeId,
    ) -> TypeId {
        // Don't narrow from 'any' if the predicate type is exactly 'Object' or 'Function'
        let is_any = self.type_(t).flags.intersects(TypeFlags::ANY);
        if !(is_any
            && (Some(predicate_t) == self.global_object_type || Some(predicate_t) == self.global_function_type))
        {
            if let Some(predicate_argument) = self.get_type_predicate_argument(predicate, call_expression) {
                if self.is_matching_reference(f.reference, predicate_argument) {
                    return self.get_narrowed_type(t, predicate_t, assume_true, false);
                }
                let mut t = t;
                if self.strict_null_checks
                    && self.optional_chain_contains_reference(predicate_argument, f.reference)
                    && (assume_true && !self.has_type_facts(predicate_t, TypeFacts::EQ_UNDEFINED)
                        || !assume_true
                            && every_type_c(self, predicate_t, |c, tt| {
                                c.type_(tt).flags.intersects(TypeFlags::UNDEFINED.union(TypeFlags::NULL))
                            }))
                {
                    t = self.get_adjusted_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED_OR_NULL);
                }
                if let Some(access) = self.get_discriminant_property_access(f, predicate_argument, t) {
                    return self.narrow_type_by_discriminant(t, access, &mut |c, tt| {
                        c.get_narrowed_type(tt, predicate_t, assume_true, false)
                    });
                }
                return t;
            }
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeByAssertion(f, t, expr) *Type` (flow.go:338).
    fn narrow_type_by_assertion(&mut self, f: &mut FlowState, t: TypeId, expr: NodeId) -> TypeId {
        let node = tsc_ast::skip_parentheses(self.store(), expr);
        let kind = self.store().node(node).kind;
        if kind == Kind::FalseKeyword {
            return self.unreachable_never_type();
        }
        if kind == Kind::BinaryExpression {
            let (left, right, op) = {
                let binary = self
                    .store()
                    .node(node)
                    .as_binary_expression()
                    .expect("BinaryExpression data");
                (
                    binary.left,
                    binary.right,
                    self.store().node(binary.operator_token).kind,
                )
            };
            if op == Kind::AmpersandAmpersandToken {
                let left_narrowed = self.narrow_type_by_assertion(f, t, left);
                return self.narrow_type_by_assertion(f, left_narrowed, right);
            }
            if op == Kind::BarBarToken {
                let left_narrowed = self.narrow_type_by_assertion(f, t, left);
                let right_narrowed = self.narrow_type_by_assertion(f, t, right);
                return self.get_union_type(&[left_narrowed, right_narrowed]);
            }
        }
        self.narrow_type(f, t, node, true)
    }

    /// Go: `func (c *Checker) getTypeAtFlowCondition(f, flow) FlowType` (flow.go:354).
    fn get_type_at_flow_condition(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        let antecedent = self.flow(flow).antecedent.expect("condition carries antecedent");
        let flow_type = self.get_type_at_flow_node(f, antecedent);
        let flow_type_t = flow_type.t.unwrap_or_else(|| self.error_type());
        if self.type_(flow_type_t).flags.intersects(TypeFlags::NEVER) {
            return flow_type;
        }
        // If we have an antecedent type (meaning we're reachable in some way), we first
        // attempt to narrow the antecedent type. If that produces the never type, and if
        // the antecedent type is incomplete (i.e. a transient type in a loop), then we
        // take the type guard as an indication that control *could* reach here once we
        // have the complete type. We proceed by switching to the silent never type which
        // doesn't report errors when operators are applied to it. Note that this is the
        // *only* place a silent never type is ever generated.
        let assume_true = self.flow(flow).flags.intersects(FlowFlags::TRUE_CONDITION);
        let Some(node) = flow_node_payload(self.flow(flow)) else {
            return flow_type;
        };
        let non_evolving_type = self.finalize_evolving_array_type(flow_type_t);
        let narrowed_type = self.narrow_type(f, non_evolving_type, node, assume_true);
        if narrowed_type == non_evolving_type {
            return flow_type;
        }
        self.new_flow_type(narrowed_type, flow_type.incomplete)
    }

    /// Narrow the given type based on the given expression having the assumed boolean value.
    ///
    /// Go: `func (c *Checker) narrowType(f, t, expr, assumeTrue) *Type` (flow.go:377).
    fn narrow_type(&mut self, f: &mut FlowState, t: TypeId, expr: NodeId, assume_true: bool) -> TypeId {
        let expr_kind = self.store().node(expr).kind;
        // for `a?.b`, we emulate a synthetic `a !== null && a !== undefined` condition for `a`
        let parent = self.store().node(expr).parent.get();
        let parent_kind = self.store().node(parent).kind;
        let parent_is_nullish_coalesce = parent_kind == Kind::BinaryExpression
            && self
                .store()
                .node(parent)
                .as_binary_expression()
                .is_some_and(|b| {
                    let op = self.store().node(b.operator_token).kind;
                    (op == Kind::QuestionQuestionToken || op == Kind::QuestionQuestionEqualsToken)
                        && b.left == expr
                });
        if binder_shims::is_expression_of_optional_chain_root(self.store(), expr) || parent_is_nullish_coalesce {
            return self.narrow_type_by_optionality(f, t, expr, assume_true);
        }
        match expr_kind {
            Kind::Identifier => {
                // When narrowing a reference to a const variable, non-assigned parameter, or readonly
                // property, we inline up to five levels of aliased conditional expressions that are
                // themselves declared as const variables.
                if !self.is_matching_reference(f.reference, expr) && self.inline_level < 5 {
                    let symbol = self.get_resolved_symbol(expr);
                    if let Some(symbol) = symbol
                        && u::is_constant_variable(self, symbol)
                        && let Some(declaration) = self.program_symbol(symbol).value_declaration
                        && self.store().node(declaration).kind == Kind::VariableDeclaration
                        && crate::ast_shims::type_of_node(self.store(), declaration).is_none()
                        && let Some(initializer) = crate::ast_shims::initializer_of(self.store(), declaration)
                        && self.is_constant_reference(f.reference)
                    {
                        self.inline_level += 1;
                        let result = self.narrow_type(f, t, initializer, assume_true);
                        self.inline_level -= 1;
                        return result;
                    }
                }
                self.narrow_type_by_truthiness(f, t, expr, assume_true)
            }
            Kind::ThisKeyword
            | Kind::SuperKeyword
            | Kind::PropertyAccessExpression
            | Kind::ElementAccessExpression => self.narrow_type_by_truthiness(f, t, expr, assume_true),
            Kind::CallExpression => self.narrow_type_by_call_expression(f, t, expr, assume_true),
            Kind::ParenthesizedExpression | Kind::NonNullExpression | Kind::SatisfiesExpression => {
                let inner = crate::ast_shims::expression_of(self.store(), expr)
                    .expect("parenthesized/non-null/satisfies carries an expression");
                self.narrow_type(f, t, inner, assume_true)
            }
            Kind::BinaryExpression => {
                self.narrow_type_by_binary_expression(f, t, expr, assume_true)
            }
            Kind::PrefixUnaryExpression => {
                let prefix = self
                    .store()
                    .node(expr)
                    .as_prefix_unary_expression()
                    .expect("PrefixUnaryExpression data");
                if prefix.operator == Kind::ExclamationToken {
                    let operand = prefix.operand;
                    return self.narrow_type(f, t, operand, !assume_true);
                }
                t
            }
            _ => t,
        }
    }

    /// Go: `func (c *Checker) narrowTypeByOptionality(f, t, expr, assumePresent) *Type` (flow.go:415).
    fn narrow_type_by_optionality(&mut self, f: &mut FlowState, t: TypeId, expr: NodeId, assume_present: bool) -> TypeId {
        if self.is_matching_reference(f.reference, expr) {
            let facts = if assume_present {
                TypeFacts::NE_UNDEFINED_OR_NULL
            } else {
                TypeFacts::EQ_UNDEFINED_OR_NULL
            };
            return self.get_adjusted_type_with_facts_impl(t, facts);
        }
        if let Some(access) = self.get_discriminant_property_access(f, expr, t) {
            let facts = if assume_present {
                TypeFacts::NE_UNDEFINED_OR_NULL
            } else {
                TypeFacts::EQ_UNDEFINED_OR_NULL
            };
            return self.narrow_type_by_discriminant(t, access, &mut |c, tt| {
                relater::get_type_with_facts(c, tt, facts)
            });
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeByTruthiness(f, t, expr, assumeTrue) *Type` (flow.go:428).
    fn narrow_type_by_truthiness(&mut self, f: &mut FlowState, t: TypeId, expr: NodeId, assume_true: bool) -> TypeId {
        if self.is_matching_reference(f.reference, expr) {
            let facts = if assume_true { TypeFacts::TRUTHY } else { TypeFacts::FALSY };
            return self.get_adjusted_type_with_facts_impl(t, facts);
        }
        let mut t = t;
        if self.strict_null_checks
            && assume_true
            && self.optional_chain_contains_reference(expr, f.reference)
        {
            t = self.get_adjusted_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        if let Some(access) = self.get_discriminant_property_access(f, expr, t) {
            let facts = if assume_true { TypeFacts::TRUTHY } else { TypeFacts::FALSY };
            return self.narrow_type_by_discriminant(t, access, &mut |c, tt| {
                relater::get_type_with_facts(c, tt, facts)
            });
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeByCallExpression(f, t, callExpression, assumeTrue) *Type`
    /// (flow.go:444).
    fn narrow_type_by_call_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        call_expression: NodeId,
        assume_true: bool,
    ) -> TypeId {
        if self.has_matching_argument(call_expression, f.reference) {
            let mut predicate: Option<Box<TypePredicate>> = None;
            if assume_true || !u::is_call_chain(self.store().node(call_expression)) {
                if let Some(signature) = self.get_effects_signature(call_expression) {
                    predicate = self.get_type_predicate_of_signature(signature);
                }
            }
            if let Some(predicate) = predicate
                && (predicate.kind == TypePredicateKind::This || predicate.kind == TypePredicateKind::Identifier)
                && let Some(predicate_t) = predicate.t
            {
                return self.narrow_type_by_type_predicate(f, t, &predicate, call_expression, assume_true, predicate_t);
            }
        }
        if self.contains_missing_type(t)
            && tsc_ast::is_access_expression(self.store().node(f.reference))
            && crate::ast_shims::expression_of(self.store(), call_expression)
                .is_some_and(|e| self.store().node(e).kind == Kind::PropertyAccessExpression)
        {
            let call_access = crate::ast_shims::expression_of(self.store(), call_expression).expect("checked above");
            let Some(call_access_expression) = crate::ast_shims::expression_of(self.store(), call_access) else {
                return t;
            };
            let reference_expression =
                crate::ast_shims::expression_of(self.store(), f.reference).unwrap_or(f.reference);
            let call_access_candidate = self.get_reference_candidate(call_access_expression);
            let is_matching = self
                .is_matching_reference(reference_expression, call_access_candidate)
                && self
                    .store()
                    .node(call_access)
                    .as_property_access_expression()
                    .is_some_and(|pa| {
                        self.store().node(pa.name).kind == Kind::Identifier
                            && tsc_ast::node_text(self.store(), pa.name) == "hasOwnProperty"
                    });
            if is_matching
                && binder_node_data::arguments_of(self.store(), call_expression).len() == 1
            {
                let argument = binder_node_data::arguments_of(self.store(), call_expression)[0];
                let accessed_name = self.get_accessed_property_name(f.reference);
                if let Some(accessed_name) = accessed_name {
                    let arg_text = tsc_ast::node_text(self.store(), argument).to_string();
                    if binder_shims::is_string_literal_like(self.store().node(argument))
                        && accessed_name == arg_text
                    {
                        let facts = if assume_true { TypeFacts::NE_UNDEFINED } else { TypeFacts::EQ_UNDEFINED };
                        return relater::get_type_with_facts(self, t, facts);
                    }
                }
            }
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeByBinaryExpression(f, t, expr, assumeTrue) *Type` (flow.go:469).
    fn narrow_type_by_binary_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr_node: NodeId,
        assume_true: bool,
    ) -> TypeId {
        let expr = self
            .store()
            .node(expr_node)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .clone();
        let operator = self.store().node(expr.operator_token).kind;
        match operator {
            Kind::EqualsToken
            | Kind::BarBarEqualsToken
            | Kind::AmpersandAmpersandEqualsToken
            | Kind::QuestionQuestionEqualsToken => {
                let right_narrowed = self.narrow_type(f, t, expr.right, assume_true);
                self.narrow_type_by_truthiness(f, right_narrowed, expr.left, assume_true)
            }
            Kind::EqualsEqualsToken
            | Kind::ExclamationEqualsToken
            | Kind::EqualsEqualsEqualsToken
            | Kind::ExclamationEqualsEqualsToken => {
                let left = self.get_reference_candidate(expr.left);
                let right = self.get_reference_candidate(expr.right);
                if self.store().node(left).kind == Kind::TypeOfExpression
                    && binder_shims::is_string_literal_like(self.store().node(right))
                {
                    return self.narrow_type_by_typeof(f, t, left, operator, right, assume_true);
                }
                if self.store().node(right).kind == Kind::TypeOfExpression
                    && binder_shims::is_string_literal_like(self.store().node(left))
                {
                    return self.narrow_type_by_typeof(f, t, right, operator, left, assume_true);
                }
                if self.is_matching_reference(f.reference, left) {
                    return self.narrow_type_by_equality(t, operator, right, assume_true);
                }
                if self.is_matching_reference(f.reference, right) {
                    return self.narrow_type_by_equality(t, operator, left, assume_true);
                }
                let mut t = t;
                if self.strict_null_checks {
                    if self.optional_chain_contains_reference(left, f.reference) {
                        t = self.narrow_type_by_optional_chain_containment(f, t, operator, right, assume_true);
                    } else if self.optional_chain_contains_reference(right, f.reference) {
                        t = self.narrow_type_by_optional_chain_containment(f, t, operator, left, assume_true);
                    }
                }
                if let Some(left_access) = self.get_discriminant_property_access(f, left, t) {
                    return self.narrow_type_by_discriminant_property(t, left_access, operator, right, assume_true);
                }
                if let Some(right_access) = self.get_discriminant_property_access(f, right, t) {
                    return self.narrow_type_by_discriminant_property(t, right_access, operator, left, assume_true);
                }
                if self.is_matching_constructor_reference(f, left) {
                    return self.narrow_type_by_constructor(t, operator, right, assume_true);
                }
                if self.is_matching_constructor_reference(f, right) {
                    return self.narrow_type_by_constructor(t, operator, left, assume_true);
                }
                if binder_shims::is_boolean_literal(self.store().node(right))
                    && !tsc_ast::is_access_expression(self.store().node(left))
                {
                    return self.narrow_type_by_boolean_comparison(f, t, left, right, operator, assume_true);
                }
                if binder_shims::is_boolean_literal(self.store().node(left))
                    && !tsc_ast::is_access_expression(self.store().node(right))
                {
                    return self.narrow_type_by_boolean_comparison(f, t, right, left, operator, assume_true);
                }
                t
            }
            Kind::InstanceOfKeyword => self.narrow_type_by_instanceof(f, t, expr_node, assume_true),
            Kind::InKeyword => {
                if self.store().node(expr.left).kind == Kind::PrivateIdentifier {
                    return self.narrow_type_by_private_identifier_in_in_expression(f, t, expr_node, assume_true);
                }
                let target = self.get_reference_candidate(expr.right);
                if self.contains_missing_type(t)
                    && tsc_ast::is_access_expression(self.store().node(f.reference))
                    && self.is_matching_reference(
                        crate::ast_shims::expression_of(self.store(), f.reference).unwrap_or(f.reference),
                        target,
                    )
                {
                    let left_type = self.get_type_of_expression(expr.left);
                    if u::is_type_usable_as_property_name(self.type_(left_type)) {
                        let name = u::get_property_name_from_type(self.type_(left_type));
                        if let Some(accessed_name) = self.get_accessed_property_name(f.reference)
                            && accessed_name == name
                        {
                            let facts = if assume_true { TypeFacts::NE_UNDEFINED } else { TypeFacts::EQ_UNDEFINED };
                            return relater::get_type_with_facts(self, t, facts);
                        }
                    }
                }
                if self.is_matching_reference(f.reference, target) {
                    let left_type = self.get_type_of_expression(expr.left);
                    if u::is_type_usable_as_property_name(self.type_(left_type)) {
                        return self.narrow_type_by_in_keyword(f, t, left_type, assume_true);
                    }
                }
                t
            }
            Kind::CommaToken => self.narrow_type(f, t, expr.right, assume_true),
            Kind::AmpersandAmpersandToken => {
                // Ordinarily we won't see && and || expressions in control flow analysis because the
                // Binder breaks those expressions down to individual conditional control flows.
                // However, we may encounter them when analyzing aliased conditional expressions.
                if assume_true {
                    let left_narrowed = self.narrow_type(f, t, expr.left, true);
                    self.narrow_type(f, left_narrowed, expr.right, true)
                } else {
                    let left_narrowed = self.narrow_type(f, t, expr.left, false);
                    let right_narrowed = self.narrow_type(f, t, expr.right, false);
                    self.get_union_type(&[left_narrowed, right_narrowed])
                }
            }
            Kind::BarBarToken => {
                if assume_true {
                    let left_narrowed = self.narrow_type(f, t, expr.left, true);
                    let right_narrowed = self.narrow_type(f, t, expr.right, true);
                    self.get_union_type(&[left_narrowed, right_narrowed])
                } else {
                    let left_narrowed = self.narrow_type(f, t, expr.left, false);
                    self.narrow_type(f, left_narrowed, expr.right, false)
                }
            }
            _ => t,
        }
    }

    /// Go: `func (c *Checker) narrowTypeByEquality(t, operator, value, assumeTrue) *Type` (flow.go:556).
    fn narrow_type_by_equality(&mut self, t: TypeId, operator: Kind, value: NodeId, mut assume_true: bool) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::ANY) {
            return t;
        }
        if operator == Kind::ExclamationEqualsToken || operator == Kind::ExclamationEqualsEqualsToken {
            assume_true = !assume_true;
        }
        let value_type = self.get_type_of_expression(value);
        let double_equals = operator == Kind::EqualsEqualsToken || operator == Kind::ExclamationEqualsToken;
        if self.type_(value_type).flags.intersects(TypeFlags::NULLABLE) {
            if !self.strict_null_checks {
                return t;
            }
            let facts = if double_equals {
                if assume_true {
                    TypeFacts::EQ_UNDEFINED_OR_NULL
                } else {
                    TypeFacts::NE_UNDEFINED_OR_NULL
                }
            } else if self.type_(value_type).flags.intersects(TypeFlags::NULL) {
                if assume_true {
                    TypeFacts::EQ_NULL
                } else {
                    TypeFacts::NE_NULL
                }
            } else if assume_true {
                TypeFacts::EQ_UNDEFINED
            } else {
                TypeFacts::NE_UNDEFINED
            };
            return self.get_adjusted_type_with_facts_impl(t, facts);
        }
        if assume_true {
            if !double_equals
                && (self.type_(t).flags.intersects(TypeFlags::UNKNOWN)
                    || some_type_c(self, t, |c, tt| c.is_empty_anonymous_object_type(tt)))
            {
                let value_flags = self.type_(value_type).flags;
                if value_flags.intersects(TypeFlags::PRIMITIVE.union(TypeFlags::NON_PRIMITIVE)) {
                    return value_type;
                }
                if self.is_empty_anonymous_object_type(value_type) {
                    return value_type;
                }
                if value_flags.intersects(TypeFlags::OBJECT) {
                    return self.non_primitive_type();
                }
            }
            if !double_equals
                && self.type_(value_type).flags.intersects(TypeFlags::PRIMITIVE)
                && self.is_uniform_union_type(t)
            {
                let regular_type = self.get_regular_type_of_literal_type(value_type);
                let constituents = self.type_(t).types().to_vec();
                if relater::contains_type(self, &constituents, regular_type) {
                    return regular_type;
                }
            }
            let filtered_type = self.filter_type_mut(t, |c, tt| {
                c.are_types_comparable(tt, value_type)
                    || double_equals && is_coercible_under_double_equals(c.type_(tt), c.type_(value_type))
            });
            return self.replace_primitives_with_literals(filtered_type, value_type);
        }
        if relater::is_unit_type(self, value_type) {
            if self.is_uniform_union_type(t) {
                let regular_type = self.get_regular_type_of_literal_type(value_type);
                let filtered_type = self.remove_type(t, regular_type);
                if filtered_type != t {
                    return filtered_type;
                }
            }
            return self.filter_type_mut(t, |c, tt| {
                !(relater::is_unit_like_type(c, tt) && c.are_types_comparable(tt, value_type))
            });
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeByTypeof(f, t, typeOfExpr, operator, literal, assumeTrue) *Type`
    /// (flow.go:614).
    fn narrow_type_by_typeof(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        type_of_expr: NodeId,
        operator: Kind,
        literal: NodeId,
        mut assume_true: bool,
    ) -> TypeId {
        // We have '==', '!=', '===', or !==' operator with 'typeof xxx' and string literal operands
        if operator == Kind::ExclamationEqualsToken || operator == Kind::ExclamationEqualsEqualsToken {
            assume_true = !assume_true;
        }
        let typeof_expression = self
            .store()
            .node(type_of_expr)
            .as_type_of_expression()
            .expect("TypeOfExpression data")
            .expression;
        let target = self.get_reference_candidate(typeof_expression);
        if !self.is_matching_reference(f.reference, target) {
            let mut t = t;
            if self.strict_null_checks
                && self.optional_chain_contains_reference(target, f.reference)
                && assume_true == (tsc_ast::node_text(self.store(), literal) != "undefined")
            {
                t = self.get_adjusted_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED_OR_NULL);
            }
            if let Some(property_access) = self.get_discriminant_property_access(f, target, t) {
                return self.narrow_type_by_discriminant(t, property_access, &mut |c, tt| {
                    c.narrow_type_by_literal_expression(tt, literal, assume_true)
                });
            }
            return t;
        }
        self.narrow_type_by_literal_expression(t, literal, assume_true)
    }

    /// Go: `var typeofNEFacts = map[string]TypeFacts` (flow.go:644).
    fn typeof_ne_facts(text: &str) -> TypeFacts {
        match text {
            "string" => TypeFacts::TYPEOF_NE_STRING,
            "number" => TypeFacts::TYPEOF_NE_NUMBER,
            "bigint" => TypeFacts::TYPEOF_NE_BIGINT,
            "boolean" => TypeFacts::TYPEOF_NE_BOOLEAN,
            "symbol" => TypeFacts::TYPEOF_NE_SYMBOL,
            "undefined" => TypeFacts::NE_UNDEFINED,
            "object" => TypeFacts::TYPEOF_NE_OBJECT,
            "function" => TypeFacts::TYPEOF_NE_FUNCTION,
            _ => TypeFacts::TYPEOF_NE_HOST_OBJECT,
        }
    }

    /// Go: `func (c *Checker) narrowTypeByLiteralExpression(t, literal, assumeTrue) *Type` (flow.go:646).
    fn narrow_type_by_literal_expression(&mut self, t: TypeId, literal: NodeId, assume_true: bool) -> TypeId {
        if assume_true {
            let text = tsc_ast::node_text(self.store(), literal).to_string();
            return self.narrow_type_by_type_name(t, &text);
        }
        let facts = Self::typeof_ne_facts(tsc_ast::node_text(self.store(), literal));
        self.get_adjusted_type_with_facts_impl(t, facts)
    }

    /// Go: `func (c *Checker) narrowTypeByTypeName(t, typeName) *Type` (flow.go:657).
    fn narrow_type_by_type_name(&mut self, t: TypeId, type_name: &str) -> TypeId {
        match type_name {
            "string" => {
                let s = self.string_type();
                self.narrow_type_by_type_facts(t, s, TypeFacts::TYPEOF_EQ_STRING)
            }
            "number" => {
                let n = self.number_type();
                self.narrow_type_by_type_facts(t, n, TypeFacts::TYPEOF_EQ_NUMBER)
            }
            "bigint" => {
                let b = self.bigint_type();
                self.narrow_type_by_type_facts(t, b, TypeFacts::TYPEOF_EQ_BIGINT)
            }
            "boolean" => {
                let b = self.boolean_type();
                self.narrow_type_by_type_facts(t, b, TypeFacts::TYPEOF_EQ_BOOLEAN)
            }
            "symbol" => {
                let s = self.es_symbol_type();
                self.narrow_type_by_type_facts(t, s, TypeFacts::TYPEOF_EQ_SYMBOL)
            }
            "object" => {
                if self.type_(t).flags.intersects(TypeFlags::ANY) {
                    return t;
                }
                let non_primitive = self.non_primitive_type();
                let narrowed_object =
                    self.narrow_type_by_type_facts(t, non_primitive, TypeFacts::TYPEOF_EQ_OBJECT);
                let null_type = self.null_type();
                let narrowed_null = self.narrow_type_by_type_facts(t, null_type, TypeFacts::EQ_NULL);
                self.get_union_type(&[narrowed_object, narrowed_null])
            }
            "function" => {
                if self.type_(t).flags.intersects(TypeFlags::ANY) {
                    return t;
                }
                let function_type = self.global_function_type.expect("global_function_type");
                self.narrow_type_by_type_facts(t, function_type, TypeFacts::TYPEOF_EQ_FUNCTION)
            }
            "undefined" => {
                let undefined = self.undefined_type();
                self.narrow_type_by_type_facts(t, undefined, TypeFacts::EQ_UNDEFINED)
            }
            _ => {
                let non_primitive = self.non_primitive_type();
                self.narrow_type_by_type_facts(t, non_primitive, TypeFacts::TYPEOF_EQ_HOST_OBJECT)
            }
        }
    }

    /// Go: `func (c *Checker) narrowTypeByTypeFacts(t, impliedType, facts) *Type` (flow.go:685).
    fn narrow_type_by_type_facts(&mut self, t: TypeId, implied_type: TypeId, facts: TypeFacts) -> TypeId {
        crate::typenodes::map_type(self, t, |c, tt| {
            if c.is_type_related_to(tt, implied_type, RelationKind::StrictSubtype) {
                if c.has_type_facts(tt, facts) {
                    return tt;
                }
                return c.never_type();
            }
            if c.is_type_subtype_of(implied_type, tt) {
                return implied_type;
            }
            if c.has_type_facts(tt, facts) {
                return c.get_intersection_type(&[tt, implied_type]);
            }
            c.never_type()
        })
    }

    /// Go: `func (c *Checker) narrowTypeByDiscriminantProperty(t, access, operator, value, assumeTrue) *Type`
    /// (flow.go:702).
    fn narrow_type_by_discriminant_property(
        &mut self,
        t: TypeId,
        access: NodeId,
        operator: Kind,
        value: NodeId,
        assume_true: bool,
    ) -> TypeId {
        if (operator == Kind::EqualsEqualsEqualsToken || operator == Kind::ExclamationEqualsEqualsToken)
            && self.type_(t).flags.intersects(TypeFlags::UNION)
        {
            let key_property_name = relater::get_key_property_name_public(self, t);
            if let Some(key_property_name) = key_property_name
                && let Some(accessed_name) = self.get_accessed_property_name(access)
                && key_property_name == accessed_name
            {
                let value_type = self.get_type_of_expression(value);
                let candidate = relater::get_constituent_type_for_key_type_public(self, t, value_type);
                if let Some(candidate) = candidate {
                    if assume_true && operator == Kind::EqualsEqualsEqualsToken
                        || !assume_true && operator == Kind::ExclamationEqualsEqualsToken
                    {
                        return candidate;
                    }
                    if let Some(prop_type) = self.get_type_of_property_of_type(candidate, &key_property_name)
                        && relater::is_unit_type(self, prop_type)
                    {
                        return self.remove_type(t, candidate);
                    }
                    return t;
                }
            }
        }
        self.narrow_type_by_discriminant(t, access, &mut |c, tt| {
            c.narrow_type_by_equality(tt, operator, value, assume_true)
        })
    }

    /// Go: `func (c *Checker) narrowTypeByDiscriminant(t, access, narrowType func(t *Type) *Type) *Type`
    /// (flow.go:725). PORT: the Go closure becomes a `&mut dyn FnMut(&mut
    /// Checker, TypeId) -> TypeId` (the port's narrowing helpers take the
    /// checker explicitly).
    fn narrow_type_by_discriminant(
        &mut self,
        t: TypeId,
        access: NodeId,
        narrow_type: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> TypeId {
        let Some(prop_name) = self.get_accessed_property_name(access) else {
            return t;
        };
        let optional_chain = tsc_ast::is_optional_chain(self.store().node(access));
        let remove_nullable = self.strict_null_checks
            && (optional_chain || u::is_non_null_access(self.store(), access))
            && relater::maybe_type_of_kind(self, t, TypeFlags::NULLABLE);
        let non_null_type = if remove_nullable {
            relater::get_type_with_facts(self, t, TypeFacts::NE_UNDEFINED_OR_NULL)
        } else {
            t
        };
        let Some(prop_type) = self.get_type_of_property_of_type(non_null_type, &prop_name) else {
            return t;
        };
        let prop_type = if remove_nullable && optional_chain {
            self.get_optional_type(prop_type, false)
        } else {
            prop_type
        };
        let narrowed_prop_type = narrow_type(self, prop_type);
        self.filter_type_mut(t, |c, tt| {
            let unknown_type = c.unknown_type();
            let discriminant_type = relater::get_type_of_property_or_index_signature_of_type(c, tt, &prop_name)
                .unwrap_or(unknown_type);
            !c.type_(discriminant_type).flags.intersects(TypeFlags::NEVER)
                && !c.type_(narrowed_prop_type).flags.intersects(TypeFlags::NEVER)
                && c.are_types_comparable(narrowed_prop_type, discriminant_type)
        })
    }

    /// Go: `func (c *Checker) isMatchingConstructorReference(f, expr) bool` (flow.go:750).
    fn is_matching_constructor_reference(&mut self, f: &mut FlowState, expr: NodeId) -> bool {
        let name: Option<NodeId> = if self.store().node(expr).kind == Kind::PropertyAccessExpression {
            self.store().node(expr).as_property_access_expression().map(|p| p.name)
        } else if self.store().node(expr).kind == Kind::ElementAccessExpression
            && self
                .store()
                .node(expr)
                .as_element_access_expression()
                .is_some_and(|e| binder_shims::is_string_literal_like(self.store().node(e.argument_expression)))
        {
            self.store()
                .node(expr)
                .as_element_access_expression()
                .map(|e| e.argument_expression)
        } else {
            None
        };
        match name {
            Some(n) if tsc_ast::node_text(self.store(), n) == "constructor" => {
                let expr_expression = crate::ast_shims::expression_of(self.store(), expr);
                expr_expression.is_some_and(|e| self.is_matching_reference(f.reference, e))
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) narrowTypeByConstructor(t, operator, identifier, assumeTrue) *Type` (flow.go:760).
    fn narrow_type_by_constructor(&mut self, t: TypeId, operator: Kind, identifier: NodeId, assume_true: bool) -> TypeId {
        // Do not narrow when checking inequality.
        if assume_true && operator != Kind::EqualsEqualsToken && operator != Kind::EqualsEqualsEqualsToken
            || !assume_true
                && operator != Kind::ExclamationEqualsToken
                && operator != Kind::ExclamationEqualsEqualsToken
        {
            return t;
        }
        // Get the type of the constructor identifier expression, if it is not a function then do
        // not narrow.
        let identifier_type = self.get_type_of_expression(identifier);
        if !self.is_function_type(identifier_type) && !self.is_constructor_type(identifier_type) {
            return t;
        }
        // Get the prototype property of the type identifier so we can find out its type.
        let Some(prototype_property) = self.get_property_of_type(identifier_type, "prototype") else {
            return t;
        };
        // Get the type of the prototype, if it is undefined, or the global `Object` or `Function`
        // types then do not narrow.
        let prototype_type = self.get_type_of_symbol(prototype_property);
        let mut candidate: Option<TypeId> = None;
        if !self.type_(prototype_type).flags.intersects(TypeFlags::ANY) {
            candidate = Some(prototype_type);
        }
        let candidate = match candidate {
            Some(c) if Some(c) != self.global_object_type && Some(c) != self.global_function_type => c,
            _ => return t,
        };
        // If the type that is being narrowed is `any` then just return the `candidate` type since
        // every type is a subtype of `any`.
        if self.type_(t).flags.intersects(TypeFlags::ANY) {
            return candidate;
        }
        // Filter out types that are not considered to be "constructed by" the `candidate` type.
        self.filter_type_mut(t, |c, tt| c.is_constructed_by(tt, candidate))
    }

    /// Go: `func (c *Checker) isConstructedBy(source, target) bool` (flow.go:794).
    fn is_constructed_by(&mut self, source: TypeId, target: TypeId) -> bool {
        // If either the source or target type are a class type then we need to check that they are
        // the same exact type. This is because you may have a class `A` that defines some set of
        // properties, and another class `B` that defines the same set of properties as class `A`,
        // in that case they are structurally the same type, but when you do something like
        // `instanceOfA.constructor === B` it will return false.
        if self.type_(source).flags.intersects(TypeFlags::OBJECT)
            && self.type_(source).object_flags.intersects(ObjectFlags::CLASS)
            || self.type_(target).flags.intersects(TypeFlags::OBJECT)
                && self.type_(target).object_flags.intersects(ObjectFlags::CLASS)
        {
            return self.type_(source).symbol == self.type_(target).symbol;
        }
        // For all other types just check that the `source` type is a subtype of the `target` type.
        self.is_type_subtype_of(source, target)
    }

    /// Go: `func (c *Checker) narrowTypeByBooleanComparison(f, t, expr, boolValue, operator, assumeTrue) *Type`
    /// (flow.go:806).
    fn narrow_type_by_boolean_comparison(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: NodeId,
        bool_value: NodeId,
        operator: Kind,
        assume_true: bool,
    ) -> TypeId {
        let assume_true = (assume_true != (self.store().node(bool_value).kind == Kind::TrueKeyword))
            != (operator != Kind::ExclamationEqualsEqualsToken && operator != Kind::ExclamationEqualsToken);
        self.narrow_type(f, t, expr, assume_true)
    }

    /// Go: `func (c *Checker) narrowTypeByInstanceof(f, t, expr, assumeTrue) *Type` (flow.go:811).
    fn narrow_type_by_instanceof(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: NodeId,
        assume_true: bool,
    ) -> TypeId {
        let expr_node = expr;
        let expr = self
            .store()
            .node(expr)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .clone();
        let left = self.get_reference_candidate(expr.left);
        if !self.is_matching_reference(f.reference, left) {
            if assume_true
                && self.strict_null_checks
                && self.optional_chain_contains_reference(left, f.reference)
            {
                return self.get_adjusted_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED_OR_NULL);
            }
            return t;
        }
        let right = expr.right;
        let right_type = self.get_type_of_expression(right);
        let global_object_type = self.global_object_type.expect("global_object_type");
        if !self.is_type_derived_from(right_type, global_object_type) {
            return t;
        }
        // if the right-hand side has an object type with a custom `[Symbol.hasInstance]` method,
        // and that method has a type predicate, use the type predicate to perform narrowing.
        // This allows normal `object` types to participate in `instanceof`, as per Step 2 of
        // https://tc262.es/ecma262/#sec-instanceofoperator.
        let mut predicate: Option<Box<TypePredicate>> = None;
        if let Some(signature) = self.get_effects_signature(expr_node) {
            predicate = self.get_type_predicate_of_signature(signature);
        }
        if let Some(predicate) = predicate
            && predicate.kind == TypePredicateKind::Identifier
            && predicate.parameter_index == 0
            && let Some(predicate_t) = predicate.t
        {
            return self.get_narrowed_type(t, predicate_t, assume_true, true);
        }
        let global_function_type = self.global_function_type.expect("global_function_type");
        if !self.is_type_derived_from(right_type, global_function_type) {
            return t;
        }
        let instance_type = crate::typenodes::map_type(self, right_type, |c, tt| c.get_instance_type(tt));
        // Don't narrow from `any` if the target type is exactly `Object` or `Function`, and narrow
        // in the false branch only if the target is a non-empty object type.
        if self.type_(t).flags.intersects(TypeFlags::ANY)
            && (Some(instance_type) == self.global_object_type
                || Some(instance_type) == self.global_function_type)
            || !assume_true
                && !(self.type_(instance_type).flags.intersects(TypeFlags::OBJECT)
                    && !self.is_empty_anonymous_object_type(instance_type))
        {
            return t;
        }
        self.get_narrowed_type(t, instance_type, assume_true, true)
    }

    /// Go: `func (c *Checker) getNarrowedType(t, candidate, assumeTrue, checkDerived) *Type` (flow.go:846).
    fn get_narrowed_type(&mut self, t: TypeId, candidate: TypeId, assume_true: bool, check_derived: bool) -> TypeId {
        if !self.type_(t).flags.intersects(TypeFlags::UNION) {
            return self.get_narrowed_type_worker(t, candidate, assume_true, check_derived);
        }
        let key = (t, candidate, assume_true, check_derived);
        if let Some(narrowed) = self.narrowed_types.get(&key) {
            return *narrowed;
        }
        let narrowed_type = self.get_narrowed_type_worker(t, candidate, assume_true, check_derived);
        self.narrowed_types.insert(key, narrowed_type);
        narrowed_type
    }

    /// Go: `func (c *Checker) getNarrowedTypeWorker(t, candidate, assumeTrue, checkDerived) *Type` (flow.go:859).
    fn get_narrowed_type_worker(&mut self, t: TypeId, candidate: TypeId, assume_true: bool, check_derived: bool) -> TypeId {
        if !assume_true {
            if t == candidate {
                return self.never_type();
            }
            if check_derived {
                return self.filter_type_mut(t, |c, tt| !c.is_type_derived_from(tt, candidate));
            }
            let t = if self.type_(t).flags.intersects(TypeFlags::UNKNOWN) {
                self.unknown_union_type()
            } else {
                t
            };
            let true_type = self.get_narrowed_type(t, candidate, true, false);
            let filtered = self.filter_type_mut(t, |c, tt| !c.is_type_subset_of(tt, true_type));
            return self.recombine_unknown_type(filtered);
        }
        if self.type_(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
            return candidate;
        }
        if t == candidate {
            return candidate;
        }
        // We first attempt to filter the current type, narrowing constituents as appropriate and
        // removing constituents that are unrelated to the candidate.
        let key_property_name = if self.type_(t).flags.intersects(TypeFlags::UNION) {
            relater::get_key_property_name_public(self, t)
        } else {
            None
        };
        let key_property_name = key_property_name.unwrap_or_default();
        let narrowed_type = crate::typenodes::map_type(self, candidate, |c, n| {
            // If a discriminant property is available, use that to reduce the type.
            let mut matching = t;
            if !key_property_name.is_empty()
                && let Some(discriminant) = c.get_type_of_property_of_type(n, &key_property_name)
                && let Some(constituent) =
                    relater::get_constituent_type_for_key_type_public(c, t, discriminant)
            {
                matching = constituent;
            }
            // For each constituent t in the current type, if t and c are directly related, pick the
            // most specific of the two. When t and c are related in both directions, we prefer c
            // for type predicates because that is the asserted type, but t for `instanceof`
            // because generics aren't reflected in prototype object types.
            let directly_related = if check_derived {
                crate::typenodes::map_type(c, matching, |c, tt| {
                    if c.is_type_derived_from(tt, n) {
                        tt
                    } else if c.is_type_derived_from(n, tt) {
                        n
                    } else {
                        c.never_type()
                    }
                })
            } else {
                crate::typenodes::map_type(c, matching, |c, tt| {
                    if c.is_type_strict_subtype_of(tt, n) {
                        tt
                    } else if c.is_type_strict_subtype_of(n, tt) {
                        n
                    } else if c.is_type_subtype_of(tt, n) {
                        tt
                    } else if c.is_type_subtype_of(n, tt) {
                        n
                    } else {
                        c.never_type()
                    }
                })
            };
            if !c.type_(directly_related).flags.intersects(TypeFlags::NEVER) {
                return directly_related;
            }
            // If no constituents are directly related, create intersections for any generic
            // constituents that are related by constraint.
            crate::typenodes::map_type(c, t, |c, tt| {
                if relater::maybe_type_of_kind(c, tt, TypeFlags::INSTANTIABLE) {
                    let constraint = c.get_base_constraint_of_type(tt);
                    let related = if check_derived {
                        constraint.is_none_or(|cc| c.is_type_derived_from(n, cc))
                    } else {
                        constraint.is_none_or(|cc| c.is_type_subtype_of(n, cc))
                    };
                    if related {
                        return c.get_intersection_type(&[tt, n]);
                    }
                }
                c.never_type()
            })
        });
        // If filtering produced a non-empty type, return that. Otherwise, pick the most specific
        // of the two based on assignability, or as a last resort produce an intersection.
        if !self.type_(narrowed_type).flags.intersects(TypeFlags::NEVER) {
            return narrowed_type;
        }
        if self.is_type_subtype_of(candidate, t) {
            return candidate;
        }
        if self.is_type_assignable_to(t, candidate) {
            return t;
        }
        if self.is_type_assignable_to(candidate, t) {
            return candidate;
        }
        self.get_intersection_type(&[t, candidate])
    }

    /// Go: `func (c *Checker) getInstanceType(constructorType) *Type` (flow.go:966).
    pub(crate) fn get_instance_type(&mut self, constructor_type: TypeId) -> TypeId {
        if let Some(prototype_property_type) = self.get_type_of_property_of_type(constructor_type, "prototype")
            && !self.type_(prototype_property_type).flags.intersects(TypeFlags::ANY)
        {
            return prototype_property_type;
        }
        let construct_signatures = self.get_signatures_of_type(constructor_type, SignatureKind::Construct);
        if !construct_signatures.is_empty() {
            let return_types: Vec<TypeId> = construct_signatures
                .into_iter()
                .map(|s| {
                    let erased = relater::get_erased_signature(self, s);
                    self.get_return_type_of_signature(erased)
                })
                .collect();
            return self.get_union_type(&return_types);
        }
        // We use the empty object type to indicate we don't know the type of objects created by
        // this constructor function.
        self.empty_object_type()
    }

    /// Go: `func (c *Checker) narrowTypeByPrivateIdentifierInInExpression(f, t, expr, assumeTrue) *Type`
    /// (flow.go:982).
    fn narrow_type_by_private_identifier_in_in_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: NodeId,
        assume_true: bool,
    ) -> TypeId {
        let expr = self
            .store()
            .node(expr)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .clone();
        let target = self.get_reference_candidate(expr.right);
        if !self.is_matching_reference(f.reference, target) {
            return t;
        }
        let Some(symbol) = self.get_symbol_for_private_identifier_expression(expr.left) else {
            return t;
        };
        let Some(class_symbol) = self.program_symbol(symbol).parent else {
            return t;
        };
        let target_type = if binder_shims::has_static_modifier(
            self.store()
                .node(self.program_symbol(symbol).value_declaration.expect("private identifier declaration")),
        ) {
            self.get_type_of_symbol(class_symbol)
        } else {
            self.get_declared_type_of_symbol(class_symbol)
        };
        self.get_narrowed_type(t, target_type, assume_true, true)
    }

    /// Go: `func (c *Checker) narrowTypeByInKeyword(f, t, nameType, assumeTrue) *Type` (flow.go:1001).
    fn narrow_type_by_in_keyword(&mut self, _f: &mut FlowState, t: TypeId, name_type: TypeId, assume_true: bool) -> TypeId {
        let name = u::get_property_name_from_type(self.type_(name_type));
        let is_known_property =
            some_type_c(self, t, |c, tt| c.is_type_presence_possible(tt, &name, true));
        if is_known_property {
            // If the check is for a known property (i.e. a property declared in some constituent of
            // the target type), we filter the target type by presence of absence of the property.
            return self.filter_type_mut(t, |c, tt| c.is_type_presence_possible(tt, &name, assume_true));
        }
        if assume_true {
            // If the check is for an unknown property, we intersect the target type with
            // `Record<X, unknown>`, where X is the name of the property.
            if let Some(record_symbol) = self.get_global_record_symbol() {
                let unknown_type = self.unknown_type();
                let instantiation =
                    self.get_type_alias_instantiation(record_symbol, vec![name_type, unknown_type], None);
                return self.get_intersection_type(&[t, instantiation]);
            }
        }
        t
    }

    /// Go: `func (c *Checker) isTypePresencePossible(t, propName, assumeTrue) bool` (flow.go:1024).
    fn is_type_presence_possible(&mut self, t: TypeId, prop_name: &str, assume_true: bool) -> bool {
        if let Some(prop) = self.get_property_of_type(t, prop_name) {
            let flags = self.program_symbol(prop).flags;
            let check_flags = self.program_symbol(prop).check_flags;
            return flags.intersects(SymbolFlags::OPTIONAL)
                || check_flags.intersects(tsc_ast::CheckFlags::PARTIAL)
                || assume_true;
        }
        relater::get_applicable_index_info_for_name(self, t, prop_name).is_some() || !assume_true
    }

    /// Go: `func (c *Checker) narrowTypeByOptionalChainContainment(f, t, operator, value, assumeTrue) *Type`
    /// (flow.go:1032).
    fn narrow_type_by_optional_chain_containment(
        &mut self,
        _f: &mut FlowState,
        t: TypeId,
        operator: Kind,
        value: NodeId,
        assume_true: bool,
    ) -> TypeId {
        // We are in a branch of obj?.foo === value (or any one of the other equality operators).
        // We narrow obj as follows (Go comment, abridged — the full table is in flow.go).
        let equals_operator = operator == Kind::EqualsEqualsToken || operator == Kind::EqualsEqualsEqualsToken;
        let nullable_flags = if operator == Kind::EqualsEqualsToken || operator == Kind::ExclamationEqualsToken {
            TypeFlags::NULLABLE
        } else {
            TypeFlags::UNDEFINED
        };
        let value_type = self.get_type_of_expression(value);
        // Note that we include any and unknown in the exclusion test because their domain
        // includes null and undefined.
        let remove_nullable = equals_operator != assume_true
            && every_type_c(self, value_type, |c, tt| {
                c.type_(tt).flags.intersects(nullable_flags)
            })
            || equals_operator == assume_true
                && every_type_c(self, value_type, |c, tt| {
                    !c.type_(tt).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
                        && !c.type_(tt).flags.intersects(nullable_flags)
                });
        if remove_nullable {
            return self.get_adjusted_type_with_facts_impl(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        t
    }

    /// Go: `func (c *Checker) getTypeAtSwitchClause(f, flow) FlowType` (flow.go:1059).
    fn get_type_at_switch_clause(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        let data = match &self.flow(flow).node {
            FlowNodeData::SwitchClause(d) => d.clone(),
            _ => panic!("switch clause flow node carries FlowSwitchClauseData"),
        };
        let antecedent = self.flow(flow).antecedent.expect("switch clause carries antecedent");
        let flow_type = self.get_type_at_flow_node(f, antecedent);
        let mut t = flow_type.t.unwrap_or_else(|| self.error_type());
        let expr = tsc_ast::skip_parentheses(
            self.store(),
            crate::ast_shims::expression_of(self.store(), data.switch_statement)
                .expect("switch statement carries an expression"),
        );
        if self.is_matching_reference(f.reference, expr) {
            t = self.narrow_type_by_switch_on_discriminant(t, &data);
        } else if self.store().node(expr).kind == Kind::TypeOfExpression
            && crate::ast_shims::expression_of(self.store(), expr)
                .is_some_and(|e| self.is_matching_reference(f.reference, e))
        {
            t = self.narrow_type_by_switch_on_type_of(t, &data);
        } else if self.store().node(expr).kind == Kind::TrueKeyword {
            t = self.narrow_type_by_switch_on_true(f, t, &data);
        } else {
            if self.strict_null_checks {
                if self.optional_chain_contains_reference(expr, f.reference) {
                    t = self.narrow_type_by_switch_optional_chain_containment(t, &data, &mut |c, tt| {
                        !c.type_(tt).flags.intersects(TypeFlags::UNDEFINED.union(TypeFlags::NEVER))
                    });
                } else if self.store().node(expr).kind == Kind::TypeOfExpression
                    && let Some(inner) = crate::ast_shims::expression_of(self.store(), expr)
                    && self.optional_chain_contains_reference(inner, f.reference)
                {
                    t = self.narrow_type_by_switch_optional_chain_containment(t, &data, &mut |c, tt| {
                        !(c.type_(tt).flags.intersects(TypeFlags::NEVER)
                            || c.type_(tt).flags.intersects(TypeFlags::STRING_LITERAL)
                                && relater::get_string_literal_value(c, tt) == "undefined")
                    });
                }
            }
            if let Some(access) = self.get_discriminant_property_access(f, expr, t) {
                t = self.narrow_type_by_switch_on_discriminant_property(t, access, &data);
            }
        }
        self.new_flow_type(t, flow_type.incomplete)
    }

    /// Go: `func (c *Checker) narrowTypeBySwitchOnDiscriminant(t, data) *Type` (flow.go:1091).
    fn narrow_type_by_switch_on_discriminant(&mut self, t: TypeId, data: &FlowSwitchClauseData) -> TypeId {
        // We only narrow if all case expressions specify values with unit types, except for the
        // case where `type` is unknown. In this instance we map object types to the nonPrimitive
        // type and narrow with that.
        let switch_types = self.get_switch_clause_types(data.switch_statement);
        if switch_types.is_empty() {
            return t;
        }
        let clause_types: Vec<TypeId> =
            switch_types[data.clause_start as usize..data.clause_end as usize].to_vec();
        let has_default_clause =
            data.clause_start == data.clause_end || clause_types.iter().any(|&s| s == self.never_type());
        if self.type_(t).flags.intersects(TypeFlags::UNKNOWN) && !has_default_clause {
            let mut ground_clause_types: Option<Vec<TypeId>> = None;
            for (i, &s) in clause_types.iter().enumerate() {
                let flags = self.type_(s).flags;
                if flags.intersects(TypeFlags::PRIMITIVE.union(TypeFlags::NON_PRIMITIVE)) {
                    let v = ground_clause_types
                        .get_or_insert_with(|| clause_types[..i].to_vec());
                    v.push(s);
                } else if flags.intersects(TypeFlags::OBJECT) {
                    let non_primitive = self.non_primitive_type();
                    let v = ground_clause_types.get_or_insert_with(|| clause_types[..i].to_vec());
                    v.push(non_primitive);
                } else {
                    return t;
                }
            }
            let result = match ground_clause_types {
                None => clause_types,
                Some(v) => v,
            };
            return self.get_union_type(&result);
        }
        let discriminant_type = self.get_union_type(&clause_types);
        let mut case_type: Option<TypeId> = None;
        if self.type_(discriminant_type).flags.intersects(TypeFlags::NEVER) {
            case_type = Some(self.never_type());
        } else {
            if self.type_(discriminant_type).flags.intersects(TypeFlags::PRIMITIVE)
                && self.is_uniform_union_type(t)
            {
                let regular_type = self.get_regular_type_of_literal_type(discriminant_type);
                let constituents = self.type_(t).types().to_vec();
                if relater::contains_type(self, &constituents, regular_type) {
                    case_type = Some(regular_type);
                }
            }
            if case_type.is_none() {
                let filtered = self.filter_type_mut(t, |c, tt| {
                    c.are_types_comparable(discriminant_type, tt)
                });
                case_type = Some(self.replace_primitives_with_literals(filtered, discriminant_type));
            }
        }
        let case_type = case_type.expect("case_type is set in both arms");
        if !has_default_clause {
            return case_type;
        }
        let undefined_type = self.undefined_type();
        let default_type = self.filter_type_mut(t, |c, tt| {
            if !relater::is_unit_like_type(c, tt) {
                return true;
            }
            let u_type = if c.type_(tt).flags.intersects(TypeFlags::UNDEFINED) {
                undefined_type
            } else {
                let extracted = c.extract_unit_type(tt);
                c.get_regular_type_of_literal_type(extracted)
            };
            !switch_types.iter().any(|&st| {
                relater::is_unit_type(c, st) && c.are_types_comparable(st, u_type)
            })
        });
        if self.type_(case_type).flags.intersects(TypeFlags::NEVER) {
            return default_type;
        }
        self.get_union_type(&[case_type, default_type])
    }

    /// Go: `func (c *Checker) narrowTypeBySwitchOnTypeOf(t, data) *Type` (flow.go:1157).
    fn narrow_type_by_switch_on_type_of(&mut self, t: TypeId, data: &FlowSwitchClauseData) -> TypeId {
        let Some(witnesses) = self.get_switch_clause_type_of_witnesses(data.switch_statement) else {
            return t;
        };
        let clauses = self.switch_clauses_of(data.switch_statement);
        // Equal start and end denotes implicit fallthrough; undefined marks explicit default clause.
        let default_index = clauses
            .iter()
            .position(|clause| self.store().node(*clause).kind == Kind::DefaultClause);
        let clause_start = data.clause_start as usize;
        let clause_end = data.clause_end as usize;
        let has_default_clause = clause_start == clause_end
            || matches!(default_index, Some(i) if i >= clause_start && i < clause_end);
        if has_default_clause {
            // In the default clause we filter constituents down to those that are not-equal to
            // all handled cases.
            let not_equal_facts =
                self.get_not_equal_facts_from_typeof_switch(clause_start, clause_end, &witnesses);
            return self.filter_type_mut(t, |c, tt| c.get_type_facts(tt, not_equal_facts) == not_equal_facts);
        }
        // In the non-default cause we create a union of the type narrowed by each of the listed
        // cases.
        let clause_witnesses: Vec<String> = witnesses[clause_start..clause_end].to_vec();
        let mut narrowed: Vec<TypeId> = Vec::with_capacity(clause_witnesses.len());
        for text in &clause_witnesses {
            if !text.is_empty() {
                narrowed.push(self.narrow_type_by_type_name(t, text));
            } else {
                narrowed.push(self.never_type());
            }
        }
        self.get_union_type(&narrowed)
    }

    /// Go: `func (c *Checker) narrowTypeBySwitchOnTrue(f, t, data) *Type` (flow.go:1187).
    fn narrow_type_by_switch_on_true(&mut self, f: &mut FlowState, mut t: TypeId, data: &FlowSwitchClauseData) -> TypeId {
        let clauses = self.switch_clauses_of(data.switch_statement);
        let default_index = clauses
            .iter()
            .position(|clause| self.store().node(*clause).kind == Kind::DefaultClause);
        let clause_start = data.clause_start as usize;
        let clause_end = data.clause_end as usize;
        let has_default_clause = clause_start == clause_end
            || matches!(default_index, Some(i) if i >= clause_start && i < clause_end);
        // First, narrow away all of the cases that preceded this set of cases.
        for &clause in &clauses[..clause_start] {
            if self.store().node(clause).kind == Kind::CaseClause {
                let expression = self.case_clause_expression(clause);
                t = self.narrow_type(f, t, expression, false);
            }
        }
        // If our current set has a default, then none the other cases were hit either. There's no
        // point in narrowing by the other cases in the set, since we can get here through other
        // paths.
        if has_default_clause {
            for &clause in &clauses[clause_end..] {
                if self.store().node(clause).kind == Kind::CaseClause {
                    let expression = self.case_clause_expression(clause);
                    t = self.narrow_type(f, t, expression, false);
                }
            }
            return t;
        }
        // Now, narrow based on the cases in this set.
        let mut narrowed: Vec<TypeId> = Vec::new();
        for &clause in &clauses[clause_start..clause_end] {
            if self.store().node(clause).kind == Kind::CaseClause {
                let expression = self.case_clause_expression(clause);
                narrowed.push(self.narrow_type(f, t, expression, true));
            } else {
                narrowed.push(self.never_type());
            }
        }
        self.get_union_type(&narrowed)
    }

    /// Go: `func (c *Checker) narrowTypeBySwitchOptionalChainContainment(t, data, clauseCheck) *Type`
    /// (flow.go:1223).
    fn narrow_type_by_switch_optional_chain_containment(
        &mut self,
        t: TypeId,
        data: &FlowSwitchClauseData,
        clause_check: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> TypeId {
        let every_clause_checks = data.clause_start != data.clause_end && {
            let switch_types = self.get_switch_clause_types(data.switch_statement);
            switch_types[data.clause_start as usize..data.clause_end as usize]
                .iter()
                .all(|&s| clause_check(self, s))
        };
        if every_clause_checks {
            return relater::get_type_with_facts(self, t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        t
    }

    /// Go: `func (c *Checker) narrowTypeBySwitchOnDiscriminantProperty(t, access, data) *Type` (flow.go:1231).
    fn narrow_type_by_switch_on_discriminant_property(&mut self, t: TypeId, access: NodeId, data: &FlowSwitchClauseData) -> TypeId {
        if data.clause_start < data.clause_end && self.type_(t).flags.intersects(TypeFlags::UNION) {
            let accessed_name = self.get_accessed_property_name(access).unwrap_or_default();
            if !accessed_name.is_empty()
                && relater::get_key_property_name_public(self, t) == Some(accessed_name.clone())
            {
                let clause_types: Vec<TypeId> = self.get_switch_clause_types(data.switch_statement)
                    [data.clause_start as usize..data.clause_end as usize]
                    .to_vec();
                let mut candidates: Vec<TypeId> = Vec::with_capacity(clause_types.len());
                for s in clause_types {
                    match relater::get_constituent_type_for_key_type_public(self, t, s) {
                        Some(c) => candidates.push(c),
                        None => candidates.push(self.unknown_type()),
                    }
                }
                let candidate = self.get_union_type(&candidates);
                if candidate != self.unknown_type() {
                    return candidate;
                }
            }
        }
        self.narrow_type_by_discriminant(t, access, &mut |c, tt| {
            c.narrow_type_by_switch_on_discriminant(tt, data)
        })
    }

    /// The switch statement's clause list (Go: the CaseBlock's Clauses).
    fn switch_clauses_of(&self, node: NodeId) -> Vec<NodeId> {
        self.store()
            .node(node)
            .as_switch_statement()
            .and_then(|s| {
                self.store()
                    .node(s.case_block)
                    .as_case_block()
                    .and_then(|b| b.clauses.as_ref().map(|cl| cl.nodes.to_vec()))
            })
            .unwrap_or_default()
    }

    /// The case clause's expression (Go: `clause.Expression()`).
    fn case_clause_expression(&self, clause: NodeId) -> NodeId {
        self.store()
            .node(clause)
            .as_case_or_default_clause()
            .map(|c| c.expression)
            .expect("CaseClause carries an expression")
    }

    /// Go: `func (c *Checker) getTypeAtFlowBranchLabel(f, flow, antecedents) FlowType` (flow.go:1253).
    fn get_type_at_flow_branch_label(
        &mut self,
        f: &mut FlowState,
        _flow: FlowNodeId,
        antecedents: Option<FlowListId>,
    ) -> FlowType {
        let antecedent_start = self.antecedent_types.len();
        let mut subtype_reduction = false;
        let mut seen_incomplete = false;
        let mut bypass_flow: Option<FlowNodeId> = None;
        let mut list = antecedents;
        while let Some(cell_id) = list {
            let (antecedent, next) = {
                let cell = &self.flow_lists[cell_id.0 as usize];
                (cell.flow, cell.next)
            };
            list = next;
            let is_bypass = bypass_flow.is_none()
                && self.flow(antecedent).flags.intersects(FlowFlags::SWITCH_CLAUSE)
                && matches!(&self.flow(antecedent).node, FlowNodeData::SwitchClause(d) if d.is_empty());
            if is_bypass {
                // The antecedent is the bypass branch of a potentially exhaustive switch
                // statement.
                bypass_flow = Some(antecedent);
                continue;
            }
            let flow_type = self.get_type_at_flow_node(f, antecedent);
            // If the type at a particular antecedent path is the declared type and the reference
            // is known to always be assigned (i.e. when declared and initial types are the same),
            // there is no reason to process more antecedents since the only possible outcome is
            // subtypes that will be removed in the final union type anyway.
            if flow_type.t == Some(f.declared_type) && f.declared_type == f.initial_type {
                self.antecedent_types.truncate(antecedent_start);
                return FlowType { t: flow_type.t, incomplete: false };
            }
            let flow_type_t = flow_type.t.unwrap_or_else(|| self.never_type());
            if !self.antecedent_types[antecedent_start..].contains(&flow_type_t) {
                self.antecedent_types.push(flow_type_t);
            }
            // If an antecedent type is not a subset of the declared type, we need to perform
            // subtype reduction. This happens when a "foreign" type is injected into the control
            // flow using the instanceof operator or a user defined type predicate.
            if !self.is_type_subset_of(flow_type_t, f.initial_type) {
                subtype_reduction = true;
            }
            if flow_type.incomplete {
                seen_incomplete = true;
            }
        }
        if let Some(bypass) = bypass_flow {
            let flow_type = self.get_type_at_flow_node(f, bypass);
            // If the bypass flow contributes a type we haven't seen yet and the switch statement
            // isn't exhaustive, process the bypass flow type. Since exhaustiveness checks
            // increase the risk of circularities, we only want to perform them when they make a
            // difference.
            let bypass_switch = match &self.flow(bypass).node {
                FlowNodeData::SwitchClause(d) => Some(d.switch_statement),
                _ => None,
            };
            let flow_type_t = flow_type.t.unwrap_or_else(|| self.never_type());
            if !self.type_(flow_type_t).flags.intersects(TypeFlags::NEVER)
                && !self.antecedent_types[antecedent_start..].contains(&flow_type_t)
                && bypass_switch.is_none_or(|s| !self.is_exhaustive_switch_statement(s))
            {
                if flow_type.t == Some(f.declared_type) && f.declared_type == f.initial_type {
                    self.antecedent_types.truncate(antecedent_start);
                    return FlowType { t: flow_type.t, incomplete: false };
                }
                self.antecedent_types.push(flow_type_t);
                if !self.is_type_subset_of(flow_type_t, f.initial_type) {
                    subtype_reduction = true;
                }
                if flow_type.incomplete {
                    seen_incomplete = true;
                }
            }
        }
        let types: Vec<TypeId> = self.antecedent_types[antecedent_start..].to_vec();
        let reduction = if subtype_reduction {
            UnionReduction::Subtype
        } else {
            UnionReduction::Literal
        };
        let result = self.get_union_or_evolving_array_type(f, &types, reduction);
        let result = self.new_flow_type(result, seen_incomplete);
        self.antecedent_types.truncate(antecedent_start);
        result
    }

    /// At flow control branch or loop junctions, if the type along every antecedent code path is
    /// an evolving array type, we construct a combined evolving array type. Otherwise we finalize
    /// all evolving array types.
    ///
    /// Go: `func (c *Checker) getUnionOrEvolvingArrayType(f, types, subtypeReduction) *Type` (flow.go:1314).
    fn get_union_or_evolving_array_type(
        &mut self,
        f: &mut FlowState,
        types: &[TypeId],
        subtype_reduction: UnionReduction,
    ) -> TypeId {
        if is_evolving_array_type_list(self, types) {
            let mut element_types: Vec<TypeId> = Vec::with_capacity(types.len());
            for &t in types {
                element_types.push(self.get_element_type_of_evolving_array_type(t));
            }
            let element_union = self.get_union_type(&element_types);
            return self.get_evolving_array_type(element_union);
        }
        let finalized: Vec<TypeId> = types
            .iter()
            .map(|&t| self.finalize_evolving_array_type(t))
            .collect();
        let union = self.get_union_type_ex(finalized, subtype_reduction, None, None);
        let result = self.recombine_unknown_type(union);
        if result != f.declared_type
            && self.type_(result).flags.intersects(TypeFlags::UNION)
            && self.type_(f.declared_type).flags.intersects(TypeFlags::UNION)
        {
            let result_constituents =
                self.type_(result).as_union_type().expect("checked UNION").base.types.clone();
            let declared_constituents = self
                .type_(f.declared_type)
                .as_union_type()
                .expect("checked UNION")
                .base
                .types
                .clone();
            if result_constituents == declared_constituents {
                return f.declared_type;
            }
        }
        result
    }

    /// Go: `func (c *Checker) getTypeAtFlowLoopLabel(f, flow) FlowType` (flow.go:1325).
    fn get_type_at_flow_loop_label(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        if f.ref_key == CacheHashKey(0) {
            f.ref_key = self.get_flow_reference_key(f);
        }
        if f.ref_key == NON_DOTTED_NAME_CACHE_KEY {
            // No cache key is generated when binding patterns are in unnarrowable situations
            return FlowType { t: Some(f.declared_type), incomplete: false };
        }
        let key = (flow, f.ref_key);
        // If we have previously computed the control flow type for the reference at this flow
        // loop junction, return the cached type.
        if let Some(cached) = self.flow_loop_cache.get(&key).copied() {
            return FlowType { t: Some(cached), incomplete: false };
        }
        // If this flow loop junction and reference are already being processed, return the union
        // of the types computed for each branch so far, marked as incomplete. (Go comment,
        // flow.go:1339 — the nested-loop restart behavior.)
        for i in (0..self.flow_loop_stack.len()).rev() {
            let loop_info = &self.flow_loop_stack[i];
            if loop_info.key == key && !loop_info.types.is_empty() {
                let types = loop_info.types.clone();
                let result = self.get_union_or_evolving_array_type(f, &types, UnionReduction::Literal);
                return self.new_flow_type(result, true);
            }
        }
        // Add the flow loop junction and reference to the in-process stack and analyze each
        // antecedent code path.
        let mut antecedent_types: Vec<TypeId> = Vec::with_capacity(4);
        let mut subtype_reduction = false;
        let mut first_antecedent_type = FlowType { t: None, incomplete: false };
        let mut list = self.flow(flow).antecedents;
        while let Some(cell_id) = list {
            let (antecedent, next) = {
                let cell = &self.flow_lists[cell_id.0 as usize];
                (cell.flow, cell.next)
            };
            list = next;
            let flow_type;
            if first_antecedent_type.is_nil() {
                // The first antecedent of a loop junction is always the non-looping control flow
                // path that leads to the top.
                first_antecedent_type = self.get_type_at_flow_node(f, antecedent);
                flow_type = first_antecedent_type;
            } else {
                // All but the first antecedent are the looping control flow paths that lead back
                // to the loop junction. We track these on the flow loop stack.
                self.flow_loop_stack.push(FlowLoopInfo {
                    key,
                    types: antecedent_types.clone(),
                });
                let save_flow_type_cache = std::mem::take(&mut self.flow_type_cache);
                flow_type = self.get_type_at_flow_node(f, antecedent);
                self.flow_type_cache = save_flow_type_cache;
                self.flow_loop_stack.pop();
                // If we see a value appear in the cache it is a sign that control flow analysis
                // was restarted and completed by checkExpressionCached. We can simply pick up the
                // resulting type and bail out.
                if let Some(cached) = self.flow_loop_cache.get(&key).copied() {
                    return FlowType { t: Some(cached), incomplete: false };
                }
            }
            let flow_type_t = flow_type.t.unwrap_or_else(|| self.never_type());
            if !antecedent_types.contains(&flow_type_t) {
                antecedent_types.push(flow_type_t);
            }
            // If an antecedent type is not a subset of the declared type, we need to perform
            // subtype reduction. This happens when a "foreign" type is injected into the control
            // flow using the instanceof operator or a user defined type predicate.
            if !self.is_type_subset_of(flow_type_t, f.initial_type) {
                subtype_reduction = true;
            }
            // If the type at a particular antecedent path is the declared type there is no
            // reason to process more antecedents since the only possible outcome is subtypes
            // that will be removed in the final union type anyway.
            if flow_type.t == Some(f.declared_type) {
                break;
            }
        }
        // The result is incomplete if the first antecedent (the non-looping control flow path) is
        // incomplete.
        let reduction = if subtype_reduction {
            UnionReduction::Subtype
        } else {
            UnionReduction::Literal
        };
        let result = self.get_union_or_evolving_array_type(f, &antecedent_types, reduction);
        if first_antecedent_type.incomplete {
            return self.new_flow_type(result, true);
        }
        self.flow_loop_cache.insert(key, result);
        FlowType { t: Some(result), incomplete: false }
    }

    /// Go: `func (c *Checker) getTypeAtFlowArrayMutation(f, flow) FlowType` (flow.go:1404).
    fn get_type_at_flow_array_mutation(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        if f.declared_type == self.auto_type() || Some(f.declared_type) == self.auto_array_type {
            let Some(node) = flow_node_payload(self.flow(flow)) else {
                return FlowType { t: None, incomplete: false };
            };
            let expr = if self.store().node(node).kind == Kind::CallExpression {
                crate::ast_shims::expression_of(self.store(), node)
                    .and_then(|call| crate::ast_shims::expression_of(self.store(), call))
            } else {
                self.store()
                    .node(node)
                    .as_binary_expression()
                    .and_then(|b| crate::ast_shims::expression_of(self.store(), b.left))
            };
            let Some(expr) = expr else {
                return FlowType { t: None, incomplete: false };
            };
            let expr_candidate = self.get_reference_candidate(expr);
            if self.is_matching_reference(f.reference, expr_candidate) {
                let antecedent = self.flow(flow).antecedent.expect("mutation carries antecedent");
                let flow_type = self.get_type_at_flow_node(f, antecedent);
                let flow_type_t = flow_type.t.unwrap_or_else(|| self.never_type());
                if self.type_(flow_type_t).object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
                    let mut evolved_type = flow_type_t;
                    if self.store().node(node).kind == Kind::CallExpression {
                        let arguments: Vec<NodeId> =
                            binder_node_data::arguments_of(self.store(), node).to_vec();
                        for arg in arguments {
                            evolved_type = self.add_evolving_array_element_type(evolved_type, arg);
                        }
                    } else {
                        // We must get the context free expression type so as to not recur in an
                        // uncached fashion on the LHS (which causes exponential blowup in compile
                        // time).
                        let binary = self
                            .store()
                            .node(node)
                            .as_binary_expression()
                            .expect("BinaryExpression data")
                            .clone();
                        let element = self
                            .store()
                            .node(binary.left)
                            .as_element_access_expression()
                            .expect("ElementAccessExpression data")
                            .argument_expression;
                        let index_type = self.get_context_free_type_of_expression(element);
                        if self.is_type_assignable_to_kind(index_type, TypeFlags::NUMBER_LIKE) {
                            evolved_type = self.add_evolving_array_element_type(evolved_type, binary.right);
                        }
                    }
                    return self.new_flow_type(evolved_type, flow_type.incomplete);
                }
                return flow_type;
            }
        }
        FlowType { t: None, incomplete: false }
    }

    /// Go: `func (c *Checker) getDiscriminantPropertyAccess(f, expr, computedType) *ast.Node` (flow.go:1436).
    fn get_discriminant_property_access(
        &mut self,
        f: &mut FlowState,
        expr: NodeId,
        computed_type: TypeId,
    ) -> Option<NodeId> {
        // As long as the computed type is a subset of the declared type, we use the full declared
        // type to detect a discriminant property. In cases where the computed type isn't a
        // subset, e.g because of a preceding type predicate narrowing, we use the actual computed
        // type.
        if self.type_(f.declared_type).flags.intersects(TypeFlags::UNION)
            || self.type_(computed_type).flags.intersects(TypeFlags::UNION)
        {
            let access = self.get_candidate_discriminant_property_access(f, expr);
            if let Some(access) = access
                && let Some(name) = self.get_accessed_property_name(access)
            {
                let mut t = computed_type;
                if self.type_(f.declared_type).flags.intersects(TypeFlags::UNION)
                    && self.is_type_subset_of(computed_type, f.declared_type)
                {
                    t = f.declared_type;
                }
                if relater::is_discriminant_property(self, t, &name) {
                    return Some(access);
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) getCandidateDiscriminantPropertyAccess(f, expr) *ast.Node` (flow.go:1457).
    fn get_candidate_discriminant_property_access(&mut self, f: &mut FlowState, expr: NodeId) -> Option<NodeId> {
        let reference_kind = self.store().node(f.reference).kind;
        if matches!(
            reference_kind,
            Kind::ObjectBindingPattern
                | Kind::ArrayBindingPattern
                | Kind::FunctionExpression
                | Kind::ArrowFunction
                | Kind::MethodDeclaration
        ) {
            // When the reference is a binding pattern or function or arrow expression, we are
            // narrowing a pseudo-reference in getNarrowedTypeOfSymbol. An identifier for a
            // destructuring variable declared in the same binding pattern or parameter declared
            // in the same parameter list is a candidate.
            if self.store().node(expr).kind == Kind::Identifier {
                let symbol = self.get_resolved_symbol(expr)?;
                let exported = self.get_export_symbol_of_value_symbol_if_exported(symbol);
                let declaration = self.program_symbol(exported).value_declaration;
                if let Some(declaration) = declaration {
                    let declaration_kind = self.store().node(declaration).kind;
                    let parent = self.store().node(declaration).parent.get();
                    if (declaration_kind == Kind::BindingElement || declaration_kind == Kind::Parameter)
                        && parent == f.reference
                        && crate::ast_shims::initializer_of(self.store(), declaration).is_none()
                        && !u::has_dot_dot_dot_token(self.store(), declaration)
                    {
                        return Some(declaration);
                    }
                }
            }
            return None;
        }
        if tsc_ast::is_access_expression(self.store().node(expr)) {
            // An access expression is a candidate if the reference matches the left hand
            // expression.
            if crate::ast_shims::expression_of(self.store(), expr)
                .is_some_and(|e| self.is_matching_reference(f.reference, e))
            {
                return Some(expr);
            }
            return None;
        }
        if self.store().node(expr).kind == Kind::Identifier {
            let symbol = self.get_resolved_symbol(expr)?;
            if !u::is_constant_variable(self, symbol) {
                return None;
            }
            let declaration = self.program_symbol(symbol).value_declaration;
            let initializer =
                declaration.and_then(|d| get_candidate_variable_declaration_initializer(self.store(), d));
            // Given 'const x = obj.kind', allow 'x' as an alias for 'obj.kind'
            if let Some(initializer) = initializer
                && tsc_ast::is_access_expression(self.store().node(initializer))
                && crate::ast_shims::expression_of(self.store(), initializer)
                    .is_some_and(|ie| self.is_matching_reference(f.reference, ie))
            {
                return Some(initializer);
            }
            // Given 'const { kind: x } = obj', allow 'x' as an alias for 'obj.kind'
            if let Some(declaration) = declaration
                && self.store().node(declaration).kind == Kind::BindingElement
                && crate::ast_shims::initializer_of(self.store(), declaration).is_none()
            {
                let pattern = self.store().node(declaration).parent.get();
                let pattern_parent = self.store().node(pattern).parent.get();
                if let Some(initializer) =
                    get_candidate_variable_declaration_initializer(self.store(), pattern_parent)
                    && matches!(
                        self.store().node(initializer).kind,
                        Kind::Identifier | Kind::PropertyAccessExpression | Kind::ElementAccessExpression
                    )
                    && self.is_matching_reference(f.reference, initializer)
                {
                    return Some(declaration);
                }
            }
        }
        None
    }

    // ── The evolving array family (flow.go:1509-1588) ──

    /// An evolving array type tracks the element types that have so far been seen in an
    /// 'x.push(value)' or 'x[n] = value' operation along the control flow graph. Evolving array
    /// types are ultimately converted into manifest array types (using getFinalArrayType) and
    /// never escape the getFlowTypeOfReference function.
    ///
    /// Go: `func (c *Checker) getEvolvingArrayType(elementType) *Type` (flow.go:1509).
    fn get_evolving_array_type(&mut self, element_type: TypeId) -> TypeId {
        let key = CachedTypeKey {
            kind: CachedTypeKind::EvolvingArrayType,
            type_id: element_type.0,
        };
        if let Some(Some(cached)) = self.cached_types.get(&key) {
            return *cached;
        }
        let result = self.new_object_type(ObjectFlags::EVOLVING_ARRAY, None);
        if let crate::types::TypeData::EvolvingArray(d) = &mut self.type_mut(result).data {
            d.element_type = Some(element_type);
        }
        self.cached_types.insert(key, Some(result));
        result
    }

    /// Go: `func (c *Checker) getElementTypeOfEvolvingArrayType(t) *Type` (flow.go:1520).
    fn get_element_type_of_evolving_array_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
            return self
                .type_(t)
                .as_evolving_array_type()
                .and_then(|d| d.element_type)
                .unwrap_or_else(|| self.never_type());
        }
        self.never_type()
    }

    /// Return true if the given node is 'x' in an 'x.length', 'x.push(value)', 'x.unshift(value)'
    /// or 'x[n] = value' operation, where 'n' is an expression of type any, undefined, or a
    /// number-like type.
    ///
    /// Go: `func (c *Checker) isEvolvingArrayOperationTarget(node) bool` (flow.go:1542).
    pub(crate) fn is_evolving_array_operation_target(&mut self, node: NodeId) -> bool {
        let root = self.get_reference_root(node);
        let parent = self.store().node(root).parent.get();
        let parent_kind = self.store().node(parent).kind;
        let is_length_push_or_unshift = parent_kind == Kind::PropertyAccessExpression && {
            let pa = self
                .store()
                .node(parent)
                .as_property_access_expression()
                .expect("PropertyAccessExpression data");
            let name_text = tsc_ast::node_text(self.store(), pa.name);
            let grandparent = self.store().node(parent).parent.get();
            name_text == "length"
                || self.store().node(grandparent).kind == Kind::CallExpression
                    && self.store().node(pa.name).kind == Kind::Identifier
                    && (name_text == "push" || name_text == "unshift")
        };
        let is_element_assignment = parent_kind == Kind::ElementAccessExpression && {
            let ea = self
                .store()
                .node(parent)
                .as_element_access_expression()
                .expect("ElementAccessExpression data");
            if ea.expression != root {
                false
            } else {
                let pp = self.store().node(parent).parent.get();
                match self.store().node(pp).as_binary_expression() {
                    Some(binary)
                        if self.store().node(binary.operator_token).kind == Kind::EqualsToken
                            && binary.left == parent =>
                    {
                        !binder_shims::is_assignment_target(self.store(), pp)
                            && {
                                let argument = ea.argument_expression;
                                let index_type = self.get_type_of_expression(argument);
                                self.is_type_assignable_to_kind(index_type, TypeFlags::NUMBER_LIKE)
                            }
                    }
                    _ => false,
                }
            }
        };
        is_length_push_or_unshift || is_element_assignment
    }

    /// When adding evolving array element types we do not perform subtype reduction. Instead, we
    /// defer subtype reduction until the evolving array type is finalized into a manifest array
    /// type.
    ///
    /// Go: `func (c *Checker) addEvolvingArrayElementType(evolvingArrayType, node) *Type` (flow.go:1557).
    fn add_evolving_array_element_type(&mut self, evolving_array_type: TypeId, node: NodeId) -> TypeId {
        let expr_type = self.get_context_free_type_of_expression(node);
        let base = self.get_base_type_of_literal_type(expr_type);
        let new_element_type = relater::get_regular_type_of_object_literal(self, base);
        let element_type = self
            .type_(evolving_array_type)
            .as_evolving_array_type()
            .and_then(|d| d.element_type)
            .unwrap_or_else(|| self.never_type());
        if self.is_type_subset_of(new_element_type, element_type) {
            return evolving_array_type;
        }
        let union = self.get_union_type(&[element_type, new_element_type]);
        self.get_evolving_array_type(union)
    }

    /// Go: `func (c *Checker) finalizeEvolvingArrayType(t) *Type` (flow.go:1566).
    fn finalize_evolving_array_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
            if let Some(element_type) = self
                .type_(t)
                .as_evolving_array_type()
                .and_then(|d| d.element_type)
            {
                return self.get_final_array_type(t, element_type);
            }
        }
        t
    }

    /// Go: `func (c *Checker) getFinalArrayType(t *EvolvingArrayType) *Type` (flow.go:1573).
    /// PORT: reads the payload off the type (Go's receiver is the same struct).
    fn get_final_array_type(&mut self, t: TypeId, element_type: TypeId) -> TypeId {
        if let Some(final_array_type) = self
            .type_(t)
            .as_evolving_array_type()
            .and_then(|d| d.final_array_type)
        {
            return final_array_type;
        }
        let final_type = self.create_final_array_type(element_type);
        if let crate::types::TypeData::EvolvingArray(d) = &mut self.type_mut(t).data {
            d.final_array_type = Some(final_type);
        }
        final_type
    }

    /// Go: `func (c *Checker) createFinalArrayType(elementType) *Type` (flow.go:1580).
    fn create_final_array_type(&mut self, element_type: TypeId) -> TypeId {
        if self.type_(element_type).flags.intersects(TypeFlags::NEVER) {
            return self.auto_array_type.expect("auto_array_type");
        }
        if self.type_(element_type).flags.intersects(TypeFlags::UNION) {
            let elements = self.type_(element_type).types().to_vec();
            let union = self.get_union_type_ex(elements, UnionReduction::Subtype, None, None);
            return self.create_array_type(union);
        }
        self.create_array_type(element_type)
    }

    /// Go: `func (c *Checker) reportFlowControlError(node)` (flow.go:1590).
    fn report_flow_control_error(&mut self, node: NodeId) {
        let Some(block) = find_ancestor_id(self, node, &mut |c, n| {
            crate::statements::is_function_or_module_block(c.store(), n)
        }) else {
            return;
        };
        let file = self.store().file(node.file_id());
        let statement_list_pos = binder_node_data::statements_of(self.store(), block)
            .map(|s| s.loc.pos())
            .unwrap_or_else(|| self.store().node(block).loc.pos());
        let span = tsc_scanner::get_range_of_token_at_position(file, statement_list_pos);
        let file_id = file.as_node_id();
        let message = &THE_CONTAINING_FUNCTION_OR_MODULE_BODY_IS_TOO_LARGE_FOR_CONTROL_FLOW_ANALYSIS;
        let diagnostic = tsc_ast::Diagnostic::new(
            Some(file_id),
            span,
            message.code(),
            message.category(),
            "" /*source*/,
            Some(message),
            "" /*messageText — resolved at format time*/,
            message.key(),
            Vec::new(),
        );
        self.add_diagnostic(diagnostic);
    }

    // ── Reference matching (flow.go:1597-1905) ──

    /// Go: `func (c *Checker) isMatchingReference(source, target) bool` (flow.go:1597).
    pub(crate) fn is_matching_reference(&mut self, source: NodeId, target: NodeId) -> bool {
        let target_kind = self.store().node(target).kind;
        if matches!(target_kind, Kind::ParenthesizedExpression | Kind::NonNullExpression) {
            let inner = crate::ast_shims::expression_of(self.store(), target)
                .expect("parenthesized/non-null carries an expression");
            return self.is_matching_reference(source, inner);
        }
        if target_kind == Kind::BinaryExpression {
            let binary = self
                .store()
                .node(target)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .clone();
            let is_assignment = binder_shims::is_assignment_expression(self.store(), target, false);
            if (is_assignment && self.is_matching_reference(source, binary.left))
                || self.store().node(binary.operator_token).kind == Kind::CommaToken
                    && self.is_matching_reference(source, binary.right)
            {
                return true;
            }
            return false;
        }
        let source_kind = self.store().node(source).kind;
        match source_kind {
            Kind::MetaProperty => {
                if target_kind != Kind::MetaProperty {
                    return false;
                }
                let source_mp = self
                    .store()
                    .node(source)
                    .as_meta_property()
                    .expect("MetaProperty data")
                    .clone();
                let target_mp = self
                    .store()
                    .node(target)
                    .as_meta_property()
                    .expect("MetaProperty data")
                    .clone();
                source_mp.keyword_token == target_mp.keyword_token
                    && tsc_ast::node_text(
                        self.store(),
                        self.store().node(source).name().expect("MetaProperty carries a name"),
                    ) == tsc_ast::node_text(
                        self.store(),
                        self.store().node(target).name().expect("MetaProperty carries a name"),
                    )
            }
            Kind::Identifier | Kind::PrivateIdentifier => {
                if crate::expressions::is_this_in_type_query(self.store(), source) {
                    return target_kind == Kind::ThisKeyword;
                }
                let source_symbol = self.get_resolved_symbol(source);
                if target_kind == Kind::Identifier
                    && source_symbol.is_some_and(|s| Some(s) == self.get_resolved_symbol(target))
                {
                    return true;
                }
                if target_kind == Kind::VariableDeclaration || target_kind == Kind::BindingElement {
                    if let Some(source_symbol) = source_symbol {
                        let exported = self.get_export_symbol_of_value_symbol_if_exported(source_symbol);
                        return self.get_symbol_of_declaration_merged(target) == Some(exported);
                    }
                }
                false
            }
            Kind::ThisKeyword => target_kind == Kind::ThisKeyword,
            Kind::SuperKeyword => target_kind == Kind::SuperKeyword,
            Kind::NonNullExpression | Kind::ParenthesizedExpression | Kind::SatisfiesExpression => {
                let inner = crate::ast_shims::expression_of(self.store(), source)
                    .expect("outer expression carries an expression");
                self.is_matching_reference(inner, target)
            }
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                if let Some(source_property_name) = self.get_accessed_property_name(source) {
                    if tsc_ast::is_access_expression(self.store().node(target)) {
                        if let Some(target_property_name) = self.get_accessed_property_name(target) {
                            if target_property_name == source_property_name
                                && let Some(source_expression) =
                                    crate::ast_shims::expression_of(self.store(), source)
                                && let Some(target_expression) =
                                    crate::ast_shims::expression_of(self.store(), target)
                            {
                                return self.is_matching_reference(source_expression, target_expression);
                            }
                        }
                    }
                }
                if source_kind == Kind::ElementAccessExpression && target_kind == Kind::ElementAccessExpression
                {
                    let source_arg = self
                        .store()
                        .node(source)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression data")
                        .argument_expression;
                    let target_arg = self
                        .store()
                        .node(target)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression data")
                        .argument_expression;
                    if self.store().node(source_arg).kind == Kind::Identifier
                        && self.store().node(target_arg).kind == Kind::Identifier
                    {
                        let symbol = self.get_resolved_symbol(source_arg);
                        if let Some(symbol) = symbol
                            && self.get_resolved_symbol(target_arg) == Some(symbol)
                            && (u::is_constant_variable(self, symbol)
                                || u::is_parameter_or_mutable_local_variable(self, self.store(), symbol)
                                    && !self.is_symbol_assigned(symbol))
                            && let Some(source_expression) = crate::ast_shims::expression_of(self.store(), source)
                            && let Some(target_expression) = crate::ast_shims::expression_of(self.store(), target)
                        {
                            return self.is_matching_reference(source_expression, target_expression);
                        }
                    }
                }
                false
            }
            Kind::QualifiedName => {
                if tsc_ast::is_access_expression(self.store().node(target)) {
                    if let Some(target_property_name) = self.get_accessed_property_name(target) {
                        let source_qn = self
                            .store()
                            .node(source)
                            .as_qualified_name()
                            .expect("QualifiedName data")
                            .clone();
                        let target_expression = crate::ast_shims::expression_of(self.store(), target)
                            .expect("access expression carries an expression");
                        return tsc_ast::node_text(self.store(), source_qn.right) == target_property_name
                            && self.is_matching_reference(source_qn.left, target_expression);
                    }
                }
                false
            }
            Kind::BinaryExpression => {
                let source_binary = self
                    .store()
                    .node(source)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .clone();
                if target_kind == Kind::BinaryExpression {
                    let target_binary = self
                        .store()
                        .node(target)
                        .as_binary_expression()
                        .expect("BinaryExpression data")
                        .clone();
                    return self.store().node(source_binary.operator_token).kind
                        == self.store().node(target_binary.operator_token).kind
                        && self.store().node(source_binary.operator_token).kind == Kind::CommaToken
                        && self.is_matching_reference(source_binary.right, target);
                }
                false
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) getFlowReferenceKey(f) CacheHashKey` (flow.go:1657).
    fn get_flow_reference_key(&mut self, f: &mut FlowState) -> CacheHashKey {
        let mut b = KeyBuilder::new();
        if self.write_flow_cache_key(&mut b, f.reference, f.declared_type, f.initial_type, f.flow_container) {
            return b.hash();
        }
        NON_DOTTED_NAME_CACHE_KEY // Reference isn't a dotted name
    }

    /// Go: `func (c *Checker) writeFlowCacheKey(b, node, declaredType, initialType, flowContainer) bool`
    /// (flow.go:1665). PORT: Go's nil flowContainer is `NodeId(0)`.
    fn write_flow_cache_key(
        &mut self,
        b: &mut KeyBuilder,
        node: NodeId,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: NodeId,
    ) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::Identifier => {
                if !crate::expressions::is_this_in_type_query(self.store(), node) {
                    let symbol = self.get_resolved_symbol(node);
                    if symbol == self.unknown_symbol {
                        return false;
                    }
                    if let Some(symbol) = symbol {
                        b.write_symbol(symbol);
                    }
                }
                self.write_flow_cache_key_tail(b, declared_type, initial_type, flow_container)
            }
            Kind::ThisKeyword => {
                self.write_flow_cache_key_tail(b, declared_type, initial_type, flow_container)
            }
            Kind::NonNullExpression | Kind::ParenthesizedExpression => {
                let inner = crate::ast_shims::expression_of(self.store(), node)
                    .expect("outer expression carries an expression");
                self.write_flow_cache_key(b, inner, declared_type, initial_type, flow_container)
            }
            Kind::QualifiedName => {
                let qn = self
                    .store()
                    .node(node)
                    .as_qualified_name()
                    .expect("QualifiedName data")
                    .clone();
                if !self.write_flow_cache_key(b, qn.left, declared_type, initial_type, flow_container) {
                    return false;
                }
                b.write_byte(b'.');
                b.write_string(tsc_ast::node_text(self.store(), qn.right));
                true
            }
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                if let Some(prop_name) = self.get_accessed_property_name(node) {
                    let expression = crate::ast_shims::expression_of(self.store(), node)
                        .expect("access expression carries an expression");
                    if !self.write_flow_cache_key(b, expression, declared_type, initial_type, flow_container) {
                        return false;
                    }
                    b.write_byte(b'.');
                    b.write_string(&prop_name);
                    return true;
                }
                if kind == Kind::ElementAccessExpression {
                    let argument = self
                        .store()
                        .node(node)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression data")
                        .argument_expression;
                    if self.store().node(argument).kind == Kind::Identifier {
                        let symbol = self.get_resolved_symbol(argument);
                        if let Some(symbol) = symbol
                            && (u::is_constant_variable(self, symbol)
                                || u::is_parameter_or_mutable_local_variable(self, self.store(), symbol)
                                    && !self.is_symbol_assigned(symbol))
                        {
                            let expression = crate::ast_shims::expression_of(self.store(), node)
                                .expect("access expression carries an expression");
                            if !self.write_flow_cache_key(b, expression, declared_type, initial_type, flow_container) {
                                return false;
                            }
                            b.write_string(".@");
                            b.write_symbol(symbol);
                            return true;
                        }
                    }
                }
                false
            }
            Kind::ObjectBindingPattern
            | Kind::ArrayBindingPattern
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::MethodDeclaration => {
                b.write_node(node);
                b.write_byte(b'#');
                b.write_type(declared_type);
                true
            }
            _ => false,
        }
    }

    /// The shared identifier/this tail of writeFlowCacheKey (Go inlines it
    /// twice via fallthrough — the port can't fall through, so the tail is a
    /// helper with identical bytes).
    fn write_flow_cache_key_tail(
        &self,
        b: &mut KeyBuilder,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: NodeId,
    ) -> bool {
        b.write_byte(b':');
        b.write_type(declared_type);
        if initial_type != declared_type {
            b.write_byte(b'=');
            b.write_type(initial_type);
        }
        if flow_container != NodeId(0) {
            b.write_byte(b'@');
            b.write_node(flow_container);
        }
        true
    }

    /// Go: `func (c *Checker) getAccessedPropertyName(access) (string, bool)` (flow.go:1727).
    pub(crate) fn get_accessed_property_name(&mut self, access: NodeId) -> Option<String> {
        let kind = self.store().node(access).kind;
        if kind == Kind::PropertyAccessExpression {
            let name = self
                .store()
                .node(access)
                .as_property_access_expression()
                .expect("PropertyAccessExpression data")
                .name;
            return Some(tsc_ast::node_text(self.store(), name).to_string());
        }
        if kind == Kind::ElementAccessExpression {
            let ea = self
                .store()
                .node(access)
                .as_element_access_expression()
                .expect("ElementAccessExpression data")
                .clone();
            return self.try_get_element_access_expression_name(&ea);
        }
        if kind == Kind::BindingElement {
            return self.get_destructuring_property_name(access);
        }
        if kind == Kind::Parameter {
            let parent = self.store().node(access).parent.get();
            let parameters = crate::ast_shims::parameters_of(self.store(), parent);
            let index = parameters.iter().position(|&p| p == access)?;
            return Some(index.to_string());
        }
        None
    }

    /// Go: `func (c *Checker) tryGetElementAccessExpressionName(node) (string, bool)` (flow.go:1743).
    fn try_get_element_access_expression_name(&mut self, node: &tsc_ast::ElementAccessExpression) -> Option<String> {
        if binder_shims::is_string_or_numeric_literal_like(self.store().node(node.argument_expression)) {
            return Some(tsc_ast::node_text(self.store(), node.argument_expression).to_string());
        }
        if binder_shims::is_entity_name_expression(self.store(), node.argument_expression) {
            return self.try_get_name_from_entity_name_expression(node.argument_expression);
        }
        None
    }

    /// Go: `func (c *Checker) tryGetNameFromEntityNameExpression(node) (string, bool)` (flow.go:1753).
    pub(crate) fn try_get_name_from_entity_name_expression(&mut self, node: NodeId) -> Option<String> {
        let symbol = self.resolve_entity_name(
            node,
            SymbolFlags::VALUE,
            true,  /*ignore_errors*/
            false, /*dont_resolve_alias*/
            NodeId(0),
        );
        let symbol = symbol?;
        if !(u::is_constant_variable(self, symbol)
            || self.program_symbol(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER))
        {
            return None;
        }
        let declaration = self.program_symbol(symbol).value_declaration?;
        if let Some(t) = self.try_get_type_from_type_node(declaration)
            && let Some(name) = try_get_name_from_type(self.type_(t))
        {
            return Some(name);
        }
        // We exclude binding elements because their initializers don't solely determine their
        // types and resolving full types can cause circularities (see
        // https://github.com/microsoft/TypeScript/issues/63192).
        if u::has_only_expression_initializer(self.store().node(declaration))
            && self.store().node(declaration).kind != Kind::BindingElement
            && self.is_block_scoped_name_declared_before_use(declaration, node)
            && let Some(initializer) = crate::ast_shims::initializer_of(self.store(), declaration)
        {
            let initializer_type = self.get_type_of_expression(initializer);
            return try_get_name_from_type(self.type_(initializer_type));
        }
        if self.store().node(declaration).kind == Kind::EnumMember {
            let name = self
                .store()
                .node(declaration)
                .name()
                .expect("EnumMember carries a name");
            return Some(tsc_ast::node_text(self.store(), name).to_string());
        }
        None
    }

    /// Go: `func (c *Checker) getDestructuringPropertyName(node) (string, bool)` (flow.go:1792).
    fn get_destructuring_property_name(&mut self, node: NodeId) -> Option<String> {
        let parent = self.store().node(node).parent.get();
        let kind = self.store().node(node).kind;
        if kind == Kind::BindingElement && self.store().node(parent).kind == Kind::ObjectBindingPattern {
            let prop_name = u::get_binding_element_property_name(self.store(), node)?;
            return self.get_literal_property_name_text(prop_name);
        }
        if kind == Kind::PropertyAssignment || kind == Kind::ShorthandPropertyAssignment {
            let name = self.store().node(node).name().expect("property assignment carries a name");
            return self.get_literal_property_name_text(name);
        }
        let parent_kind = self.store().node(parent).kind;
        if parent_kind == Kind::ArrayLiteralExpression || parent_kind == Kind::ArrayBindingPattern {
            let elements = binder_node_data::elements_of(self.store(), parent);
            let index = elements.iter().position(|&e| e == node)?;
            return Some(index.to_string());
        }
        None
    }

    /// Go: `func (c *Checker) getLiteralPropertyNameText(name) (string, bool)` (flow.go:1806).
    fn get_literal_property_name_text(&mut self, name: NodeId) -> Option<String> {
        let t = self.get_literal_type_from_property_name(name);
        if self
            .type_(t)
            .flags
            .intersects(TypeFlags::STRING_LITERAL.union(TypeFlags::NUMBER_LITERAL))
        {
            return Some(relater::get_string_literal_value(self, t));
        }
        None
    }

    /// Go: `func (c *Checker) isConstantReference(node) bool` (flow.go:1814).
    pub(crate) fn is_constant_reference(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::ThisKeyword => true,
            Kind::Identifier => {
                if !crate::expressions::is_this_in_type_query(self.store(), node) {
                    let Some(symbol) = self.get_resolved_symbol(node) else {
                        return false;
                    };
                    return u::is_constant_variable(self, symbol)
                        || u::is_parameter_or_mutable_local_variable(self, self.store(), symbol)
                            && !self.is_symbol_assigned(symbol)
                        || self
                            .program_symbol(symbol)
                            .value_declaration
                            .is_some_and(|d| self.store().node(d).kind == Kind::FunctionExpression);
                }
                false
            }
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                // The resolvedSymbol property is initialized by checkPropertyAccess or
                // checkElementAccess before we get here.
                let expression = crate::ast_shims::expression_of(self.store(), node).unwrap_or(node);
                if self.is_constant_reference(expression)
                    && let Some(symbol) = self.get_resolved_symbol_or_nil(node)
                {
                    return self.is_readonly_symbol(symbol);
                }
                false
            }
            Kind::ObjectBindingPattern | Kind::ArrayBindingPattern => {
                let parent = self.store().node(node).parent.get();
                let root_declaration = binder_shims::get_root_declaration(self.store(), parent);
                let root_kind = self.store().node(root_declaration).kind;
                let root_parent = self.store().node(root_declaration).parent.get();
                if root_kind == Kind::Parameter
                    || root_kind == Kind::VariableDeclaration
                        && self.store().node(root_parent).kind == Kind::CatchClause
                {
                    return !self.is_some_symbol_assigned(root_declaration);
                }
                root_kind == Kind::VariableDeclaration && self.is_var_const_like(root_declaration)
            }
            _ => false,
        }
    }

    /// Go: `func (c *Checker) containsMatchingReference(source, target) bool` (flow.go:1841).
    fn contains_matching_reference(&mut self, mut source: NodeId, target: NodeId) -> bool {
        while tsc_ast::is_access_expression(self.store().node(source)) {
            let expression = crate::ast_shims::expression_of(self.store(), source)
                .expect("access expression carries an expression");
            source = expression;
            if self.is_matching_reference(source, target) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) optionalChainContainsReference(source, target) bool` (flow.go:1851).
    pub(crate) fn optional_chain_contains_reference(&mut self, mut source: NodeId, target: NodeId) -> bool {
        while tsc_ast::is_optional_chain(self.store().node(source)) {
            let expression = crate::ast_shims::expression_of(self.store(), source)
                .expect("optional chain carries an expression");
            source = expression;
            if self.is_matching_reference(source, target) {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getReferenceCandidate(node) *ast.Node` (flow.go:1861).
    fn get_reference_candidate(&mut self, node: NodeId) -> NodeId {
        let kind = self.store().node(node).kind;
        if kind == Kind::ParenthesizedExpression {
            let inner = crate::ast_shims::expression_of(self.store(), node)
                .expect("parenthesized carries an expression");
            return self.get_reference_candidate(inner);
        }
        if kind == Kind::BinaryExpression {
            let binary = self
                .store()
                .node(node)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .clone();
            match self.store().node(binary.operator_token).kind {
                Kind::EqualsToken
                | Kind::BarBarEqualsToken
                | Kind::AmpersandAmpersandEqualsToken
                | Kind::QuestionQuestionEqualsToken => return self.get_reference_candidate(binary.left),
                Kind::CommaToken => return self.get_reference_candidate(binary.right),
                _ => {}
            }
        }
        node
    }

    /// Go: `func (c *Checker) getReferenceRoot(node) *ast.Node` (flow.go:1876).
    fn get_reference_root(&self, node: NodeId) -> NodeId {
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        if parent_kind == Kind::ParenthesizedExpression {
            return self.get_reference_root(parent);
        }
        if parent_kind == Kind::BinaryExpression {
            if let Some(binary) = self.store().node(parent).as_binary_expression() {
                let op_kind = self.store().node(binary.operator_token).kind;
                if (op_kind == Kind::EqualsToken && binary.left == node)
                    || (op_kind == Kind::CommaToken && binary.right == node)
                {
                    return self.get_reference_root(parent);
                }
            }
        }
        node
    }

    /// Go: `func (c *Checker) hasMatchingArgument(expression, reference) bool` (flow.go:1886).
    fn has_matching_argument(&mut self, expression: NodeId, reference: NodeId) -> bool {
        let arguments: Vec<NodeId> = binder_node_data::arguments_of(self.store(), expression).to_vec();
        for argument in arguments {
            if self.is_or_contains_matching_reference(reference, argument)
                || self.optional_chain_contains_reference(argument, reference)
            {
                return true;
            }
        }
        if crate::ast_shims::expression_of(self.store(), expression)
            .is_some_and(|e| self.store().node(e).kind == Kind::PropertyAccessExpression)
            && crate::ast_shims::expression_of(self.store(), expression)
                .and_then(|e| crate::ast_shims::expression_of(self.store(), e))
                .is_some_and(|ee| self.is_or_contains_matching_reference(reference, ee))
        {
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) isOrContainsMatchingReference(source, target) bool` (flow.go:1898).
    fn is_or_contains_matching_reference(&mut self, source: NodeId, target: NodeId) -> bool {
        self.is_matching_reference(source, target) || self.contains_matching_reference(source, target)
    }

    /// Go: `func (c *Checker) replacePrimitivesWithLiterals(typeWithPrimitives, typeWithLiterals) *Type`
    /// (flow.go:1907).
    fn replace_primitives_with_literals(&mut self, type_with_primitives: TypeId, type_with_literals: TypeId) -> TypeId {
        let primitive_kinds = TypeFlags::STRING
            .union(TypeFlags::TEMPLATE_LITERAL)
            .union(TypeFlags::NUMBER)
            .union(TypeFlags::BIGINT);
        let literal_kinds = TypeFlags::STRING_LITERAL
            .union(TypeFlags::TEMPLATE_LITERAL)
            .union(TypeFlags::STRING_MAPPING)
            .union(TypeFlags::NUMBER_LITERAL)
            .union(TypeFlags::BIGINT_LITERAL);
        if relater::maybe_type_of_kind(self, type_with_primitives, primitive_kinds)
            && relater::maybe_type_of_kind(self, type_with_literals, literal_kinds)
        {
            return crate::typenodes::map_type(self, type_with_primitives, |c, t| {
                let string_kinds = TypeFlags::STRING
                    .union(TypeFlags::STRING_LITERAL)
                    .union(TypeFlags::TEMPLATE_LITERAL)
                    .union(TypeFlags::STRING_MAPPING);
                if c.type_(t).flags.intersects(TypeFlags::STRING) {
                    return relater::extract_types_of_kind(c, type_with_literals, string_kinds);
                }
                if crate::typenodes::is_pattern_literal_type(c, t)
                    && !relater::maybe_type_of_kind(
                        c,
                        type_with_literals,
                        TypeFlags::STRING
                            .union(TypeFlags::TEMPLATE_LITERAL)
                            .union(TypeFlags::STRING_MAPPING),
                    )
                {
                    return relater::extract_types_of_kind(c, type_with_literals, TypeFlags::STRING_LITERAL);
                }
                if c.type_(t).flags.intersects(TypeFlags::NUMBER) {
                    let number_kinds = TypeFlags::NUMBER.union(TypeFlags::NUMBER_LITERAL);
                    return relater::extract_types_of_kind(c, type_with_literals, number_kinds);
                }
                if c.type_(t).flags.intersects(TypeFlags::BIGINT) {
                    let bigint_kinds = TypeFlags::BIGINT.union(TypeFlags::BIGINT_LITERAL);
                    return relater::extract_types_of_kind(c, type_with_literals, bigint_kinds);
                }
                t
            });
        }
        type_with_primitives
    }

    // ── Exhaustive switches + clause types (flow.go:1933-2046) ──

    /// Go: `func (c *Checker) isExhaustiveSwitchStatement(node) bool` (flow.go:1933).
    fn is_exhaustive_switch_statement(&mut self, node: NodeId) -> bool {
        let state = self.switch_statement_links.get(node).exhaustive_state;
        if state == ExhaustiveState::Unknown {
            // Indicate resolution is in process
            self.switch_statement_links.get(node).exhaustive_state = ExhaustiveState::Computing;
            let is_exhaustive = self.compute_exhaustive_switch_statement(node);
            if self.switch_statement_links.get(node).exhaustive_state == ExhaustiveState::Computing {
                self.switch_statement_links.get(node).exhaustive_state = if is_exhaustive {
                    ExhaustiveState::True
                } else {
                    ExhaustiveState::False
                };
            }
        } else if self.switch_statement_links.get(node).exhaustive_state == ExhaustiveState::Computing {
            // Resolve circularity to false
            self.switch_statement_links.get(node).exhaustive_state = ExhaustiveState::False;
        }
        self.switch_statement_links.get(node).exhaustive_state == ExhaustiveState::True
    }

    /// Go: `func (c *Checker) computeExhaustiveSwitchStatement(node) bool` (flow.go:1949).
    fn compute_exhaustive_switch_statement(&mut self, node: NodeId) -> bool {
        let expression = crate::ast_shims::expression_of(self.store(), node)
            .expect("switch statement carries an expression");
        if self.store().node(expression).kind == Kind::TypeOfExpression {
            let Some(witnesses) = self.get_switch_clause_type_of_witnesses(node) else {
                return false;
            };
            let inner = crate::ast_shims::expression_of(self.store(), expression)
                .expect("typeof carries an expression");
            let operand_constraint = {
                let checked = self.check_expression_cached(inner);
                relater::get_base_constraint_or_type(self, checked)
            };
            // Get the not-equal flags for all handled cases.
            let not_equal_facts = self.get_not_equal_facts_from_typeof_switch(0, 0, &witnesses);
            if self.type_(operand_constraint).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                // We special case the top types to be exhaustive when all cases are handled.
                return TypeFacts::ALL_TYPEOF_NE.0 & not_equal_facts.0 == TypeFacts::ALL_TYPEOF_NE.0;
            }
            // A missing not-equal flag indicates that the type wasn't handled by some case.
            let facts = not_equal_facts;
            !some_type_c(self, operand_constraint, |c, t| c.get_type_facts(t, facts) == facts)
        } else {
            let checked = self.check_expression_cached(expression);
            let t = relater::get_base_constraint_or_type(self, checked);
            if !relater::is_literal_type(self, t) {
                return false;
            }
            let switch_types = self.get_switch_clause_types(node);
            if switch_types.is_empty()
                || switch_types.iter().any(|&s| is_neither_unit_type_nor_never(self.type_(s)))
            {
                return false;
            }
            let mapped = crate::typenodes::map_type(self, t, |c, tt| {
                c.get_regular_type_of_literal_type(tt)
            });
            self.each_type_contained_in(mapped, &switch_types)
        }
    }

    /// Go: `func (c *Checker) eachTypeContainedIn(source, types) bool` (flow.go:1978).
    fn each_type_contained_in(&self, source: TypeId, types: &[TypeId]) -> bool {
        if self.type_(source).flags.intersects(TypeFlags::UNION) {
            let constituents = self.type_(source).types();
            return !constituents.iter().any(|&t| !types.contains(&t));
        }
        types.contains(&source)
    }

    /// Go: `func (c *Checker) getSwitchClauseTypeOfWitnesses(node) []string` (flow.go:1989).
    fn get_switch_clause_type_of_witnesses(&mut self, node: NodeId) -> Option<Vec<String>> {
        if !self.switch_statement_links.get(node).witnesses_computed {
            let clauses = self.switch_clauses_of(node);
            let mut witnesses: Vec<String> = vec![String::new(); clauses.len()];
            let mut all_literals = true;
            for (i, &clause) in clauses.iter().enumerate() {
                if self.store().node(clause).kind == Kind::CaseClause {
                    let expression = self.case_clause_expression(clause);
                    if !binder_shims::is_string_literal_like(self.store().node(expression)) {
                        all_literals = false;
                        break;
                    }
                    let text = tsc_ast::node_text(self.store(), expression);
                    if !witnesses.iter().any(|w| w == text) {
                        witnesses[i] = text.to_string();
                    }
                }
            }
            let links = self.switch_statement_links.get(node);
            if all_literals {
                links.witnesses = witnesses;
            } else {
                links.witnesses.clear();
            }
            links.witnesses_computed = true;
        }
        if self.switch_statement_links.get(node).witnesses_computed {
            let witnesses = self.switch_statement_links.get(node).witnesses.clone();
            // Go: an empty (but computed) slice is distinct from nil; the
            // "not all string literals" answer is the empty non-nil slice
            // (getSwitchClauseTypeOfWitnesses returns it; callers treat it as
            // present-but-empty). The port models presence with Some.
            if witnesses.is_empty() {
                // PORT: Go returns an empty non-nil slice here ONLY when the
                // clause list is empty; the not-all-literals case returns nil
                // (witnesses = nil in Go). Track that distinction.
                let clauses_len = self.switch_clauses_of(node).len();
                if clauses_len == 0 {
                    return Some(witnesses);
                }
                return None;
            }
            Some(witnesses)
        } else {
            None
        }
    }

    /// Go: `func (c *Checker) getNotEqualFactsFromTypeofSwitch(start, end, witnesses) TypeFacts` (flow.go:2012).
    fn get_not_equal_facts_from_typeof_switch(&mut self, start: usize, end: usize, witnesses: &[String]) -> TypeFacts {
        let mut facts = TypeFacts(0);
        for (i, witness) in witnesses.iter().enumerate() {
            if (i < start || i >= end) && !witness.is_empty() {
                let f = Self::typeof_ne_facts(witness);
                facts = TypeFacts(facts.0 | f.0);
            }
        }
        facts
    }

    /// Go: `func (c *Checker) getSwitchClauseTypes(node) []*Type` (flow.go:2026).
    fn get_switch_clause_types(&mut self, node: NodeId) -> Vec<TypeId> {
        if !self.switch_statement_links.get(node).switch_types_computed {
            let clauses = self.switch_clauses_of(node);
            let types: Vec<TypeId> = clauses
                .iter()
                .map(|&clause| self.get_type_of_switch_clause(clause))
                .collect();
            let links = self.switch_statement_links.get(node);
            links.switch_types = types;
            links.switch_types_computed = true;
        }
        self.switch_statement_links.get(node).switch_types.clone()
    }

    /// Go: `func (c *Checker) getTypeOfSwitchClause(clause) *Type` (flow.go:2040).
    fn get_type_of_switch_clause(&mut self, clause: NodeId) -> TypeId {
        if self.store().node(clause).kind == Kind::CaseClause {
            let expression = self.case_clause_expression(clause);
            let t = self.get_type_of_expression(expression);
            return self.get_regular_type_of_literal_type(t);
        }
        self.never_type()
    }

    // ── Effects signatures (flow.go:2047-2143) ──

    /// Go: `func (c *Checker) getEffectsSignature(node) *Signature` (flow.go:2047).
    pub(crate) fn get_effects_signature(&mut self, node: NodeId) -> Option<SignatureId> {
        let mut signature = self.signature_node_links.get(node).effects_signature;
        if signature.is_none() {
            // A call expression parented by an expression statement is a potential assertion.
            // Other call expressions are potential type predicate function calls. In order to
            // avoid triggering circularities in control flow analysis, we use getTypeOfDottedName
            // when resolving the call target expression of an assertion.
            let mut func_type: Option<TypeId> = None;
            let node_kind = self.store().node(node).kind;
            let parent = self.store().node(node).parent.get();
            if node_kind == Kind::BinaryExpression {
                let binary = self
                    .store()
                    .node(node)
                    .as_binary_expression()
                    .expect("BinaryExpression data")
                    .clone();
                let right_type = self.check_non_null_expression(binary.right);
                func_type = self.get_symbol_has_instance_method_of_object_type(right_type);
            } else if self.store().node(parent).kind == Kind::ExpressionStatement {
                func_type = self.get_type_of_dotted_name(
                    crate::ast_shims::expression_of(self.store(), node).unwrap_or(node),
                    None,
                );
            } else if crate::ast_shims::expression_of(self.store(), node)
                .is_some_and(|e| self.store().node(e).kind != Kind::SuperKeyword)
            {
                let expression = crate::ast_shims::expression_of(self.store(), node).unwrap_or(node);
                if tsc_ast::is_optional_chain(self.store().node(node)) {
                    let expr_type = self.check_expression(expression);
                    let optional_type = self.get_optional_expression_type(expr_type, expression);
                    func_type = Some(self.check_non_null_type(optional_type, expression));
                } else {
                    func_type = Some(self.check_non_null_expression(expression));
                }
            }
            let apparent_type = func_type.map(|ft| self.get_apparent_type(ft));
            let apparent_type = apparent_type.unwrap_or_else(|| self.unknown_type());
            let signatures = self.get_signatures_of_type(apparent_type, SignatureKind::Call);
            if signatures.len() == 1
                && self.signatures[signatures[0].0 as usize].type_parameters.is_empty()
            {
                signature = Some(signatures[0]);
            } else if signatures
                .iter()
                .any(|&s| self.has_type_predicate_or_never_return_type(s))
            {
                signature =
                    self.get_resolved_signature_ex(node, None, CheckMode::CHECK_MODE_NORMAL);
            }
            if !signature.is_some_and(|s| self.has_type_predicate_or_never_return_type(s)) {
                signature = self.unknown_signature;
            }
            self.signature_node_links.get(node).effects_signature = signature;
        }
        if signature == self.unknown_signature {
            return None;
        }
        signature
    }

    /// Go: `func (c *Checker) getSymbolHasInstanceMethodOfObjectType(t) *Type` (flow.go:2093).
    fn get_symbol_has_instance_method_of_object_type(&mut self, t: TypeId) -> Option<TypeId> {
        let has_instance_property_name = self.get_property_name_for_known_symbol_name("hasInstance");
        if self.all_types_assignable_to_kind(t, TypeFlags::NON_PRIMITIVE) {
            if let Some(has_instance_property) = self.get_property_of_type(t, &has_instance_property_name) {
                let has_instance_property_type = self.get_type_of_symbol(has_instance_property);
                if !self.type_(has_instance_property_type).flags.intersects(TypeFlags::ANY)
                    && !self
                        .get_signatures_of_type(has_instance_property_type, SignatureKind::Call)
                        .is_empty()
                {
                    return Some(has_instance_property_type);
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) getPropertyNameForKnownSymbolName(symbolName) string` (flow.go:2107).
    fn get_property_name_for_known_symbol_name(&mut self, symbol_name: &str) -> String {
        if let Some(ctor_symbol) = self.get_global_es_symbol_constructor_symbol_or_nil() {
            let ctor_type = self.get_type_of_symbol(ctor_symbol);
            if let Some(unique_type) = self.get_type_of_property_of_type(ctor_type, symbol_name)
                && u::is_type_usable_as_property_name(self.type_(unique_type))
            {
                return u::get_property_name_from_type(self.type_(unique_type));
            }
        }
        format!("{}@{}", tsc_ast::INTERNAL_SYMBOL_NAME_PREFIX, symbol_name)
    }

    /// Go: `func (c *Checker) getTypeOfDottedName(node, diagnostic) *Type` (flow.go:2122).
    pub(crate) fn get_type_of_dotted_name(&mut self, node: NodeId, diagnostic: Option<NodeId>) -> Option<TypeId> {
        let flags = self.store().node(node).flags;
        if !flags.intersects(NF::IN_WITH_STATEMENT) {
            let kind = self.store().node(node).kind;
            match kind {
                Kind::Identifier => {
                    let symbol = self.get_resolved_symbol(node)?;
                    let exported = self.get_export_symbol_of_value_symbol_if_exported(symbol);
                    return self.get_explicit_type_of_symbol(exported, diagnostic);
                }
                Kind::ThisKeyword => return self.get_explicit_this_type(node),
                Kind::SuperKeyword => return Some(self.check_super_expression(node)),
                Kind::PropertyAccessExpression => {
                    let t = crate::ast_shims::expression_of(self.store(), node)
                        .and_then(|e| self.get_type_of_dotted_name(e, diagnostic))?;
                    let name = self
                        .store()
                        .node(node)
                        .as_property_access_expression()
                        .expect("PropertyAccessExpression data")
                        .name;
                    // PORT(FCS tranche 23): Go flow.go:2122-2144 — the
                    // private-identifier arm. A `#name` access resolves
                    // through the class symbol's brand name
                    // (`__#<symbolId>@#name`, binder.GetSymbolNameFor
                    // PrivateIdentifier), not the plain property table.
                    let prop = if tsc_ast::is_private_identifier(self.store().node(name)) {
                        self.type_(t).symbol.and_then(|class_symbol| {
                            let description = tsc_ast::node_text(self.store(), name);
                            let id = tsc_binder::ast_shims::get_symbol_id(
                                self.program_symbol(class_symbol),
                            );
                            let private_name = format!(
                                "{}#{id}@{description}",
                                tsc_ast::INTERNAL_SYMBOL_NAME_PREFIX
                            );
                            self.get_property_of_type(t, &private_name)
                        })
                    } else {
                        let name_text = tsc_ast::node_text(self.store(), name).to_string();
                        self.get_property_of_type(t, &name_text)
                    };
                    if let Some(prop) = prop {
                        return self.get_explicit_type_of_symbol(prop, diagnostic);
                    }
                }
                Kind::ParenthesizedExpression => {
                    return crate::ast_shims::expression_of(self.store(), node)
                        .and_then(|e| self.get_type_of_dotted_name(e, diagnostic));
                }
                _ => {}
            }
        }
        None
    }

    /// Go: `func (c *Checker) getExplicitTypeOfSymbol(symbol, diagnostic) *Type` (flow.go:2155).
    fn get_explicit_type_of_symbol(&mut self, symbol: SymbolId, diagnostic: Option<NodeId>) -> Option<TypeId> {
        let symbol = self.resolve_symbol(symbol);
        if self.resolving_explicit_type_of_symbol.has(&symbol) {
            return None;
        }
        self.resolving_explicit_type_of_symbol.add(symbol);
        let result = self.get_explicit_type_of_symbol_worker(symbol, diagnostic);
        self.resolving_explicit_type_of_symbol.delete(&symbol);
        result
    }

    /// The post-set body of getExplicitTypeOfSymbol (Go uses defer).
    fn get_explicit_type_of_symbol_worker(&mut self, symbol: SymbolId, _diagnostic: Option<NodeId>) -> Option<TypeId> {
        let flags = self.program_symbol(symbol).flags;
        if flags.intersects(
            SymbolFlags::FUNCTION
                .union(SymbolFlags::METHOD)
                .union(SymbolFlags::CLASS)
                .union(SymbolFlags::VALUE_MODULE),
        ) {
            return Some(self.get_type_of_symbol(symbol));
        }
        if flags.intersects(SymbolFlags::VARIABLE.union(SymbolFlags::PROPERTY)) {
            if self.program_symbol(symbol).check_flags.intersects(tsc_ast::CheckFlags::MAPPED) {
                let origin = self.mapped_symbol_links.get(symbol).synthetic_origin;
                if let Some(origin) = origin
                    && self.get_explicit_type_of_symbol(origin, None).is_some()
                {
                    return Some(self.get_type_of_symbol(symbol));
                }
            }
            let declaration = self.program_symbol(symbol).value_declaration;
            if let Some(declaration) = declaration {
                if self.is_declaration_with_explicit_type_annotation(declaration) {
                    return Some(self.get_type_of_symbol(symbol));
                }
                if self.store().node(declaration).kind == Kind::VariableDeclaration {
                    let parent = self.store().node(declaration).parent.get();
                    let grandparent = self.store().node(parent).parent.get();
                    if self.store().node(grandparent).kind == Kind::ForOfStatement {
                        let expression = crate::ast_shims::expression_of(self.store(), grandparent)
                            .expect("for-of carries an expression");
                        if let Some(expression_type) = self.get_type_of_dotted_name(expression, None) {
                            let use_ = if self
                                .store()
                                .node(grandparent)
                                .as_for_in_or_of_statement()
                                .is_some_and(|s| s.await_modifier.is_some())
                            {
                                crate::statements::IterationUse::FOR_AWAIT_OF
                            } else {
                                crate::statements::IterationUse::FOR_OF
                            };
                            let undefined_type = self.undefined_type();
                            let element_type = self.check_iterated_type_or_element_type(
                                use_,
                                expression_type,
                                undefined_type,
                                None,
                            );
                            return Some(element_type);
                        }
                    }
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) isDeclarationWithExplicitTypeAnnotation(node) bool` (flow.go:2197).
    fn is_declaration_with_explicit_type_annotation(&self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        (matches!(
            kind,
            Kind::VariableDeclaration
                | Kind::PropertyDeclaration
                | Kind::PropertySignature
                | Kind::Parameter
        ) && crate::ast_shims::type_of_node(self.store(), node).is_some())
            || self.is_expando_property_function_with_return_type_annotation(node)
    }

    /// Go: `func (c *Checker) isExpandoPropertyFunctionWithReturnTypeAnnotation(node) bool` (flow.go:2202).
    fn is_expando_property_function_with_return_type_annotation(&self, node: NodeId) -> bool {
        if self.store().node(node).kind == Kind::BinaryExpression {
            if let Some(binary) = self.store().node(node).as_binary_expression() {
                let right = binary.right;
                if tsc_ast::is_function_like(self.store().node(right))
                    && crate::ast_shims::type_of_node(self.store(), right).is_some()
                {
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) hasTypePredicateOrNeverReturnType(sig) bool` (flow.go:2211).
    pub(crate) fn has_type_predicate_or_never_return_type(&mut self, sig: SignatureId) -> bool {
        if self.get_type_predicate_of_signature(sig).is_some() {
            return true;
        }
        let declaration = self.signatures[sig.0 as usize].declaration;
        match declaration {
            Some(declaration) => {
                let unknown_type = self.unknown_type();
                let return_type = self
                    .get_return_type_from_annotation(declaration)
                    .unwrap_or(unknown_type);
                self.type_(return_type).flags.intersects(TypeFlags::NEVER)
            }
            None => false,
        }
    }

    /// Go: `func (c *Checker) getExplicitThisType(node) *Type` (flow.go:2215).
    fn get_explicit_this_type(&mut self, node: NodeId) -> Option<TypeId> {
        let container = binder_shims::get_this_container(self.store(), node, false, false);
        if tsc_ast::is_function_like(self.store().node(container)) {
            if let Some(signature) = self.get_signature_from_declaration(container) {
                if let Some(this_parameter) = self.signatures[signature.0 as usize].this_parameter {
                    return self.get_explicit_type_of_symbol(this_parameter, None);
                }
            }
        }
        let parent = self.store().node(container).parent.get();
        // Go: `container.Parent != nil && ast.IsClassLike(container.Parent)`
        // (flow.go:2223) — the nil-parent guard (a top-level script `this`
        // hands a NONE parent to isClassLike, whose store read panics).
        if parent != NodeId::NONE && crate::statements::is_class_like(self.store(), parent) {
            let symbol = self.get_symbol_of_declaration_merged(parent)?;
            if binder_shims::has_static_modifier(self.store().node(container)) {
                return Some(self.get_type_of_symbol(symbol));
            }
            let declared = self.get_declared_type_of_symbol(symbol);
            return self.type_(declared).as_interface_type().and_then(|i| i.this_type);
        }
        None
    }

    // ── Initial/assigned types (flow.go:2234-2464) ──

    /// Go: `func (c *Checker) getInitialType(node) *Type` (flow.go:2234).
    fn get_initial_type_of(&mut self, node: NodeId) -> TypeId {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::VariableDeclaration => self.get_initial_type_of_variable_declaration(node),
            Kind::BindingElement => self.get_initial_type_of_binding_element(node),
            _ => panic!("Unhandled case in getInitialType"),
        }
    }

    /// Go: `func (c *Checker) getInitialTypeOfVariableDeclaration(node) *Type` (flow.go:2244).
    fn get_initial_type_of_variable_declaration(&mut self, node: NodeId) -> TypeId {
        if let Some(initializer) = crate::ast_shims::initializer_of(self.store(), node) {
            return self.get_type_of_initializer(initializer);
        }
        let parent = self.store().node(node).parent.get();
        let grandparent = self.store().node(parent).parent.get();
        if self.store().node(grandparent).kind == Kind::ForInStatement {
            return self.string_type();
        }
        if self.store().node(grandparent).kind == Kind::ForOfStatement {
            let t = self.check_right_hand_side_of_for_of(grandparent);
            if t != self.error_type() {
                return t;
            }
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getTypeOfInitializer(node) *Type` (flow.go:2260).
    pub(crate) fn get_type_of_initializer(&mut self, node: NodeId) -> TypeId {
        // Return the cached type if one is available. If the type of the variable was inferred
        // from its initializer, we'll already have cached the type. Otherwise we compute it now
        // without caching such that transient types are reflected.
        if self.type_node_links.has(&node)
            && let Some(t) = self.type_node_links.get(node).resolved_type
        {
            return t;
        }
        self.get_type_of_expression(node)
    }

    /// Go: `func (c *Checker) getInitialTypeOfBindingElement(node) *Type` (flow.go:2273).
    fn get_initial_type_of_binding_element(&mut self, node: NodeId) -> TypeId {
        let pattern = self.store().node(node).parent.get();
        let pattern_parent = self.store().node(pattern).parent.get();
        let parent_type = self.get_initial_type_of(pattern_parent);
        let t = if self.store().node(pattern).kind == Kind::ObjectBindingPattern {
            let prop_name = u::get_binding_element_property_name(self.store(), node)
                .expect("binding element carries a property name");
            self.get_type_of_destructured_property(parent_type, prop_name)
        } else if !u::has_dot_dot_dot_token(self.store(), node) {
            let elements = binder_node_data::elements_of(self.store(), pattern);
            let index = elements
                .iter()
                .position(|&e| e == node)
                .expect("element is in the pattern");
            self.get_type_of_destructured_array_element(parent_type, index)
        } else {
            self.get_type_of_destructured_spread_expression(parent_type)
        };
        let initializer = crate::ast_shims::initializer_of(self.store(), node);
        self.get_type_with_default(t, initializer)
    }

    /// Go: `func (c *Checker) getAssignedType(node) *Type` (flow.go:2288).
    fn get_assigned_type(&mut self, node: NodeId) -> TypeId {
        let parent = self.store().node(node).parent.get();
        match self.store().node(parent).kind {
            Kind::ForInStatement => self.string_type(),
            Kind::ForOfStatement => {
                let t = self.check_right_hand_side_of_for_of(parent);
                if t != self.error_type() {
                    t
                } else {
                    self.error_type()
                }
            }
            Kind::BinaryExpression => self.get_assigned_type_of_binary_expression(parent),
            Kind::DeleteExpression => self.undefined_type(),
            Kind::ArrayLiteralExpression => self.get_assigned_type_of_array_literal_element(parent, node),
            Kind::SpreadElement => self.get_assigned_type_of_spread_expression(parent),
            Kind::PropertyAssignment => self.get_assigned_type_of_property_assignment(parent),
            Kind::ShorthandPropertyAssignment => {
                self.get_assigned_type_of_shorthand_property_assignment(parent)
            }
            _ => self.error_type(),
        }
    }

    /// Go: `func (c *Checker) getAssignedTypeOfBinaryExpression(node) *Type` (flow.go:2314).
    fn get_assigned_type_of_binary_expression(&mut self, node: NodeId) -> TypeId {
        let binary = self
            .store()
            .node(node)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .clone();
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        let is_destructuring_default_assignment = (parent_kind == Kind::ArrayLiteralExpression
            && self.is_destructuring_assignment_target(parent))
            || (parent_kind == Kind::PropertyAssignment
                && {
                    let pp = self.store().node(parent).parent.get();
                    self.is_destructuring_assignment_target(pp)
                });
        if is_destructuring_default_assignment {
            let assigned = self.get_assigned_type(node);
            return self.get_type_with_default(assigned, Some(binary.right));
        }
        self.get_type_of_expression(binary.right)
    }

    /// Go: `func (c *Checker) getAssignedTypeOfArrayLiteralElement(node, element) *Type` (flow.go:2323).
    fn get_assigned_type_of_array_literal_element(&mut self, node: NodeId, element: NodeId) -> TypeId {
        let t = self.get_assigned_type(node);
        let elements = binder_node_data::elements_of(self.store(), node);
        let index = elements
            .iter()
            .position(|&e| e == element)
            .expect("element is in the array");
        self.get_type_of_destructured_array_element(t, index)
    }

    /// Go: `func (c *Checker) getTypeOfDestructuredArrayElement(t, index) *Type` (flow.go:2327).
    fn get_type_of_destructured_array_element(&mut self, t: TypeId, index: usize) -> TypeId {
        if every_type_c(self, t, relater::is_tuple_like_type) {
            if let Some(element_type) = crate::typenodes::get_tuple_element_type(self, t, index) {
                return element_type;
            }
        }
        let undefined_type = self.undefined_type();
        let element_type = self.check_iterated_type_or_element_type(
            crate::statements::IterationUse::DESTRUCTURING,
            t,
            undefined_type,
            None,
        );
        self.include_undefined_in_index_signature(Some(element_type))
            .unwrap_or_else(|| self.error_type())
    }

    /// Go: `func (c *Checker) includeUndefinedInIndexSignature(t) *Type` (flow.go:2339).
    fn include_undefined_in_index_signature(&mut self, t: Option<TypeId>) -> Option<TypeId> {
        let t = t?;
        if self.compiler_options.no_unchecked_indexed_access == Tristate::TSTrue {
            let missing_type = self.missing_type();
            return Some(self.get_union_type(&[t, missing_type]));
        }
        Some(t)
    }

    /// Go: `func (c *Checker) getAssignedTypeOfSpreadExpression(node) *Type` (flow.go:2349).
    fn get_assigned_type_of_spread_expression(&mut self, node: NodeId) -> TypeId {
        let parent = self.store().node(node).parent.get();
        let t = self.get_assigned_type(parent);
        self.get_type_of_destructured_spread_expression(t)
    }

    /// Go: `func (c *Checker) getTypeOfDestructuredSpreadExpression(t) *Type` (flow.go:2353).
    fn get_type_of_destructured_spread_expression(&mut self, t: TypeId) -> TypeId {
        let undefined_type = self.undefined_type();
        let element_type = self.check_iterated_type_or_element_type(
            crate::statements::IterationUse::DESTRUCTURING,
            t,
            undefined_type,
            None,
        );
        self.create_array_type(element_type)
    }

    /// Go: `func (c *Checker) getAssignedTypeOfPropertyAssignment(node) *Type` (flow.go:2361).
    fn get_assigned_type_of_property_assignment(&mut self, node: NodeId) -> TypeId {
        let parent = self.store().node(node).parent.get();
        let name = self
            .store()
            .node(node)
            .name()
            .expect("property assignment carries a name");
        let assigned = self.get_assigned_type(parent);
        self.get_type_of_destructured_property(assigned, name)
    }

    /// Go: `func (c *Checker) getTypeOfDestructuredProperty(t, name) *Type` (flow.go:2365).
    fn get_type_of_destructured_property(&mut self, t: TypeId, name: NodeId) -> TypeId {
        let name_type = self.get_literal_type_from_property_name(name);
        if !u::is_type_usable_as_property_name(self.type_(name_type)) {
            return self.error_type();
        }
        let text = u::get_property_name_from_type(self.type_(name_type));
        if let Some(prop_type) = self.get_type_of_property_of_type(t, &text) {
            return prop_type;
        }
        if let Some(index_info) = relater::get_applicable_index_info_for_name(self, t, &text) {
            let value_type = self.index_info_value_type(index_info);
            return self
                .include_undefined_in_index_signature(Some(value_type))
                .unwrap_or_else(|| self.error_type());
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getAssignedTypeOfShorthandPropertyAssignment(node) *Type` (flow.go:2380).
    fn get_assigned_type_of_shorthand_property_assignment(&mut self, node: NodeId) -> TypeId {
        let object_assignment_initializer = self
            .store()
            .node(node)
            .as_shorthand_property_assignment()
            .and_then(|d| d.object_assignment_initializer);
        let assigned = self.get_assigned_type_of_property_assignment(node);
        self.get_type_with_default(assigned, object_assignment_initializer)
    }

    /// Go: `func (c *Checker) getTypeWithDefault(t, defaultExpression) *Type` (flow.go:2389).
    fn get_type_with_default(&mut self, t: TypeId, default_expression: Option<NodeId>) -> TypeId {
        if let Some(default_expression) = default_expression {
            let non_undefined = self.get_non_undefined_type(t);
            let default_type = self.get_type_of_expression(default_expression);
            return self.get_union_type(&[non_undefined, default_type]);
        }
        t
    }

    /// Go: `func (c *Checker) getAssignmentReducedType(declaredType, assignedType) *Type` (flow.go:2399).
    fn get_assignment_reduced_type(&mut self, declared_type: TypeId, assigned_type: TypeId) -> TypeId {
        if declared_type == assigned_type {
            return declared_type;
        }
        if self.type_(assigned_type).flags.intersects(TypeFlags::NEVER) {
            return assigned_type;
        }
        let key = (declared_type, assigned_type);
        if let Some(cached) = self.assignment_reduced_types.get(&key) {
            *cached
        } else {
            let computed = self.get_assignment_reduced_type_worker(declared_type, assigned_type);
            self.assignment_reduced_types.insert(key, computed);
            computed
        }
    }

    /// Go: `func (c *Checker) getAssignmentReducedTypeWorker(declaredType, assignedType) *Type` (flow.go:2415).
    fn get_assignment_reduced_type_worker(&mut self, declared_type: TypeId, assigned_type: TypeId) -> TypeId {
        // Remove those constituent types of declaredType to which no constituent type of
        // assignedType is assignable. For example, when a variable of type
        // number | string | boolean is assigned a value of type number | boolean, we remove
        // type string.
        let filtered_type = self.filter_type_mut(declared_type, |c, t| {
            c.type_maybe_assignable_to(assigned_type, t)
        });
        // Ensure that we narrow to fresh types if the assignment is a fresh boolean literal type.
        let mut reduced_type = filtered_type;
        if self.type_(assigned_type).flags.intersects(TypeFlags::BOOLEAN_LITERAL)
            && crate::checker::is_fresh_literal_type(self, assigned_type)
        {
            reduced_type = crate::typenodes::map_type(self, filtered_type, |c, t| {
                c.get_fresh_type_of_literal_type(t)
            });
        }
        // Our crude heuristic produces an invalid result in some cases: see GH#26130. For now,
        // when that happens, we give up and don't narrow at all. (This also means we'll never
        // narrow for erroneous assignments where the assigned type is not assignable to the
        // declared type.)
        if self.is_type_assignable_to(assigned_type, reduced_type) {
            return reduced_type;
        }
        declared_type
    }

    /// Go: `func (c *Checker) typeMaybeAssignableTo(source, target) bool` (flow.go:2434).
    fn type_maybe_assignable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        if !self.type_(source).flags.intersects(TypeFlags::UNION) {
            return self.is_type_assignable_to(source, target);
        }
        // Quick exit when source union contains the target type
        let constituents = self.type_(source).types().to_vec();
        if relater::contains_type(self, &constituents, target) {
            return true;
        }
        // Otherwise, check if any constituent type of the source union is assignable to the
        // target type
        constituents.iter().any(|&t| self.is_type_assignable_to(t, target))
    }

    /// Go: `func (c *Checker) getTypePredicateArgument(predicate, callExpression) *ast.Node` (flow.go:2451).
    fn get_type_predicate_argument(&mut self, predicate: &TypePredicate, call_expression: NodeId) -> Option<NodeId> {
        if predicate.kind == TypePredicateKind::Identifier || predicate.kind == TypePredicateKind::AssertsIdentifier {
            let arguments = binder_node_data::arguments_of(self.store(), call_expression);
            if predicate.parameter_index >= 0 && (predicate.parameter_index as usize) < arguments.len() {
                return Some(arguments[predicate.parameter_index as usize]);
            }
        } else {
            let invoked_expression = crate::ast_shims::expression_of(self.store(), call_expression)
                .map(|e| tsc_ast::skip_parentheses(self.store(), e))
                .unwrap_or(call_expression);
            if tsc_ast::is_access_expression(self.store().node(invoked_expression)) {
                let inner = crate::ast_shims::expression_of(self.store(), invoked_expression)
                    .expect("access expression carries an expression");
                return Some(tsc_ast::skip_parentheses(self.store(), inner));
            }
        }
        None
    }

    // ── Constructor/static-block flow (flow.go:2466-2511) ──

    /// Go: `func (c *Checker) getFlowTypeInConstructor(symbol, constructor) *Type` (flow.go:2466).
    pub(crate) fn get_flow_type_in_constructor(&mut self, symbol: SymbolId, constructor: NodeId) -> Option<TypeId> {
        let reference = self.new_this_property_access_synthetic(symbol, constructor);
        let flow_type = self.get_flow_type_of_property(reference, Some(symbol));
        self.report_auto_flow_type(symbol, flow_type);
        // We don't infer a type if assignments are only null or undefined.
        if every_type_c(self, flow_type, |c, t| {
            c.type_(t).flags.intersects(TypeFlags::NULLABLE)
        }) {
            return None;
        }
        Some(self.convert_auto_to_any(flow_type))
    }

    /// Go: `func (c *Checker) getFlowTypeInStaticBlocks(symbol, staticBlocks) *Type` (flow.go:2488).
    pub(crate) fn get_flow_type_in_static_blocks(&mut self, symbol: SymbolId, static_blocks: Vec<NodeId>) -> Option<TypeId> {
        for static_block in static_blocks {
            let reference = self.new_this_property_access_synthetic(symbol, static_block);
            let flow_type = self.get_flow_type_of_property(reference, Some(symbol));
            self.report_auto_flow_type(symbol, flow_type);
            // We don't infer a type if assignments are only null or undefined.
            if every_type_c(self, flow_type, |c, t| {
                c.type_(t).flags.intersects(TypeFlags::NULLABLE)
            }) {
                continue;
            }
            return Some(self.convert_auto_to_any(flow_type));
        }
        None
    }

    /// The shared noImplicitAny auto-type report of the two flow-entry points
    /// (Go inlines the same block twice).
    fn report_auto_flow_type(&mut self, symbol: SymbolId, flow_type: TypeId) {
        if self.no_implicit_any && (flow_type == self.auto_type() || Some(flow_type) == self.auto_array_type) {
            let symbol_str = relater::symbol_to_string(self, symbol);
            let type_str = relater::type_to_string(self, flow_type);
            if let Some(declaration) = self.program_symbol(symbol).value_declaration {
                self.error(declaration, &MEMBER_0_IMPLICITLY_HAS_AN_1_TYPE, &[symbol_str, type_str]);
            }
        }
    }

    /// The synthetic `this.<name>` property access Go's factory builds in
    /// getFlowTypeInConstructor / getFlowTypeInStaticBlocks. PORT: allocated
    /// through the `NodeStore::alloc` seam (a real arena node — Go's factory
    /// node is a real pointer-graph node too).
    fn new_this_property_access_synthetic(&mut self, symbol: SymbolId, parent: NodeId) -> NodeId {
        let name = self.program_symbol(symbol).name.clone();
        let name: Box<str> = name.into();
        // The `this` keyword expression.
        let this_expression = Node {
            kind: Kind::ThisKeyword,
            flags: NF::NONE,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data: NodeData::KeywordExpression(Box::new(tsc_ast::KeywordExpression { flow_node: None })),
        };
        let this_expression_id = self.program.node_store_mut().alloc(this_expression);
        // The name identifier/private identifier.
        let is_private = name.starts_with('#');
        let name_kind = if is_private { Kind::PrivateIdentifier } else { Kind::Identifier };
        let name_text: Box<str> = name;
        let name_node = Node {
            kind: name_kind,
            flags: NF::NONE,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data: if is_private {
                NodeData::PrivateIdentifier(Box::new(tsc_ast::PrivateIdentifier { text: name_text }))
            } else {
                NodeData::Identifier(Box::new(tsc_ast::Identifier {
                    text: name_text,
                    flow_node: None,
                }))
            },
        };
        let name_id = self.program.node_store_mut().alloc(name_node);
        // The property access.
        let property_access = Node {
            kind: Kind::PropertyAccessExpression,
            flags: NF::NONE,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data: NodeData::PropertyAccessExpression(Box::new(tsc_ast::PropertyAccessExpression {
                flow_node: None,
                facts: 0,
                expression: this_expression_id,
                question_dot_token: None,
                name: name_id,
            })),
        };
        let reference = self.program.node_store_mut().alloc(property_access);
        // Parent + flow-node wiring (Go: reference.Expression().Parent = reference;
        // reference.Parent = constructor; FlowNodeData().FlowNode = ReturnFlowNode).
        {
            let store = self.program.node_store_mut();
            store.node_mut(this_expression_id).parent.set(reference);
            store.node_mut(reference).parent.set(parent);
            let return_flow_node = store
                .node(parent)
                .as_constructor_declaration()
                .and_then(|d| d.return_flow_node)
                .or_else(|| {
                    store
                        .node(parent)
                        .as_class_static_block_declaration()
                        .and_then(|d| d.return_flow_node)
                });
            if let NodeData::PropertyAccessExpression(d) = &mut store.node_mut(reference).data {
                d.flow_node = return_flow_node;
            }
        }
        reference
    }

    // ── Reachability (flow.go:2513-2603) ──

    /// Go: `func (c *Checker) isReachableFlowNode(flow) bool` (flow.go:2513).
    pub(crate) fn is_reachable_flow_node(&mut self, flow: FlowNodeId) -> bool {
        let mut f = self.get_flow_state();
        let result = self.is_reachable_flow_node_worker(&mut f, flow, false);
        self.put_flow_state(f);
        self.last_flow_node = Some(flow);
        self.last_flow_node_reachable = result;
        result
    }

    /// Go: `func (c *Checker) isReachableFlowNodeWorker(f, flow, noCacheCheck) bool` (flow.go:2522).
    fn is_reachable_flow_node_worker(&mut self, f: &mut FlowState, flow: FlowNodeId, mut no_cache_check: bool) -> bool {
        let mut flow = flow;
        loop {
            if self.last_flow_node == Some(flow) {
                return self.last_flow_node_reachable;
            }
            let flags = self.flow(flow).flags;
            if flags.intersects(FlowFlags::SHARED) {
                if !no_cache_check && f.reduce_labels.is_empty() {
                    if let Some(reachable) = self.flow_node_reachable.get(&flow) {
                        return *reachable;
                    }
                    let reachable = self.is_reachable_flow_node_worker(f, flow, true);
                    self.flow_node_reachable.insert(flow, reachable);
                    return reachable;
                }
                no_cache_check = false;
            }
            if flags.intersects(FlowFlags::ASSIGNMENT.union(FlowFlags::CONDITION).union(FlowFlags::ARRAY_MUTATION)) {
                flow = self.flow(flow).antecedent.expect("carries antecedent");
            } else if flags.intersects(FlowFlags::CALL) {
                if let Some(node) = flow_node_payload(self.flow(flow))
                    && let Some(signature) = self.get_effects_signature(node)
                {
                    let predicate = self.get_type_predicate_of_signature(signature);
                    if let Some(predicate) = predicate
                        && predicate.kind == TypePredicateKind::AssertsIdentifier
                        && predicate.t.is_none()
                    {
                        let arguments = binder_node_data::arguments_of(self.store(), node);
                        if predicate.parameter_index >= 0
                            && (predicate.parameter_index as usize) < arguments.len()
                            && self.is_false_expression(arguments[predicate.parameter_index as usize])
                        {
                            return false;
                        }
                    }
                    let return_type = self.get_return_type_of_signature(signature);
                    if self.type_(return_type).flags.intersects(TypeFlags::NEVER) {
                        return false;
                    }
                }
                flow = self.flow(flow).antecedent.expect("call carries antecedent");
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                // A branching point is reachable if any branch is reachable.
                let mut list = get_branch_label_antecedents(self, flow, &f.reduce_labels);
                while let Some(cell_id) = list {
                    let (antecedent, next) = {
                        let cell = &self.flow_lists[cell_id.0 as usize];
                        (cell.flow, cell.next)
                    };
                    list = next;
                    if self.is_reachable_flow_node_worker(f, antecedent, false) {
                        return true;
                    }
                }
                return false;
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                if self.flow(flow).antecedents.is_none() {
                    return false;
                }
                // A loop is reachable if the control flow path that leads to the top is
                // reachable.
                flow = self.flow_lists[self.flow(flow).antecedents.expect("checked above").0 as usize].flow;
            } else if flags.intersects(FlowFlags::SWITCH_CLAUSE) {
                // The control flow path representing an unmatched value in a switch statement
                // with no default clause is unreachable if the switch statement is exhaustive.
                let data = match &self.flow(flow).node {
                    FlowNodeData::SwitchClause(d) => d.clone(),
                    _ => return true,
                };
                if data.clause_start == data.clause_end
                    && self.is_exhaustive_switch_statement(data.switch_statement)
                {
                    return false;
                }
                flow = self.flow(flow).antecedent.expect("switch clause carries antecedent");
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                // Cache is unreliable once we start adjusting labels
                self.last_flow_node = None;
                let data = match &self.flow(flow).node {
                    FlowNodeData::ReduceLabel(d) => d.clone(),
                    _ => panic!("reduce label flow node carries FlowReduceLabelData"),
                };
                f.reduce_labels.push(data);
                let antecedent = self.flow(flow).antecedent.expect("reduce label carries antecedent");
                let result = self.is_reachable_flow_node_worker(f, antecedent, false);
                f.reduce_labels.pop();
                return result;
            } else {
                return !flags.intersects(FlowFlags::UNREACHABLE);
            }
        }
    }

    /// Go: `func (c *Checker) isFalseExpression(expr) bool` (flow.go:2589).
    fn is_false_expression(&mut self, expr: NodeId) -> bool {
        let node = tsc_ast::skip_parentheses(self.store(), expr);
        if self.store().node(node).kind == Kind::FalseKeyword {
            return true;
        }
        if self.store().node(node).kind == Kind::BinaryExpression {
            let binary = self
                .store()
                .node(node)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .clone();
            let op = self.store().node(binary.operator_token).kind;
            return (op == Kind::AmpersandAmpersandToken
                && (self.is_false_expression(binary.left) || self.is_false_expression(binary.right)))
                || (op == Kind::BarBarToken
                    && self.is_false_expression(binary.left)
                    && self.is_false_expression(binary.right));
        }
        false
    }

    /// Go: `func (c *Checker) isPostSuperFlowNode(flow, noCacheCheck) bool` (flow.go:2604).
    #[allow(dead_code)] // The super-call spine probe lands with the W3 check-stage runner wiring.
    pub(crate) fn is_post_super_flow_node(&mut self, flow: FlowNodeId, no_cache_check: bool) -> bool {
        let mut f = self.get_flow_state();
        let result = self.is_post_super_flow_node_worker(&mut f, flow, no_cache_check);
        self.put_flow_state(f);
        result
    }

    /// Go: `func (c *Checker) isPostSuperFlowNodeWorker(f, flow, noCacheCheck) bool` (flow.go:2611).
    #[allow(dead_code)] // The super-call spine probe lands with the W3 check-stage runner wiring.
    fn is_post_super_flow_node_worker(&mut self, f: &mut FlowState, flow: FlowNodeId, mut no_cache_check: bool) -> bool {
        let mut flow = flow;
        loop {
            let flags = self.flow(flow).flags;
            if flags.intersects(FlowFlags::SHARED) {
                if !no_cache_check {
                    if let Some(post_super) = self.flow_node_post_super.get(&flow) {
                        return *post_super;
                    }
                    let post_super = self.is_post_super_flow_node_worker(f, flow, true);
                    self.flow_node_post_super.insert(flow, post_super);
                    return post_super;
                }
                no_cache_check = false;
            }
            if flags.intersects(
                FlowFlags::ASSIGNMENT
                    .union(FlowFlags::CONDITION)
                    .union(FlowFlags::ARRAY_MUTATION)
                    .union(FlowFlags::SWITCH_CLAUSE),
            ) {
                flow = self.flow(flow).antecedent.expect("carries antecedent");
            } else if flags.intersects(FlowFlags::CALL) {
                let expression = flow_node_payload(self.flow(flow))
                    .and_then(|node| crate::ast_shims::expression_of(self.store(), node));
                if expression.is_some_and(|e| self.store().node(e).kind == Kind::SuperKeyword) {
                    return true;
                }
                flow = self.flow(flow).antecedent.expect("call carries antecedent");
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                let mut list = get_branch_label_antecedents(self, flow, &f.reduce_labels);
                while let Some(cell_id) = list {
                    let (antecedent, next) = {
                        let cell = &self.flow_lists[cell_id.0 as usize];
                        (cell.flow, cell.next)
                    };
                    list = next;
                    if !self.is_post_super_flow_node_worker(f, antecedent, false) {
                        return false;
                    }
                }
                return true;
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                // A loop is post-super if the control flow path that leads to the top is
                // post-super.
                flow = self.flow_lists[self.flow(flow).antecedents.expect("loop label carries antecedents").0 as usize].flow;
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                let data = match &self.flow(flow).node {
                    FlowNodeData::ReduceLabel(d) => d.clone(),
                    _ => panic!("reduce label flow node carries FlowReduceLabelData"),
                };
                f.reduce_labels.push(data);
                let antecedent = self.flow(flow).antecedent.expect("reduce label carries antecedent");
                let result = self.is_post_super_flow_node_worker(f, antecedent, false);
                f.reduce_labels.pop();
                return result;
            } else {
                // Unreachable nodes are considered post-super to silence errors
                return flags.intersects(FlowFlags::UNREACHABLE);
            }
        }
    }

    // ── Assignment marking (flow.go:2655-2762) ──

    /// Go: `func (c *Checker) isSymbolAssignedDefinitely(symbol) bool` (flow.go:2655).
    #[allow(dead_code)] // The checkIdentifier caller loop lands with the W3 check-stage runner wiring.
    pub(crate) fn is_symbol_assigned_definitely_real(&mut self, symbol: SymbolId) -> bool {
        self.ensure_assignments_marked(symbol);
        self.marked_assignment_symbol_links.get(symbol).has_definite_assignment
    }

    /// Go: `func (c *Checker) isSymbolAssigned(symbol) bool` (flow.go:2661).
    pub(crate) fn is_symbol_assigned(&mut self, symbol: SymbolId) -> bool {
        self.ensure_assignments_marked(symbol);
        self.marked_assignment_symbol_links.get(symbol).last_assignment_pos != 0
    }

    /// Go: `func (c *Checker) isPastLastAssignment(symbol, location) bool` (flow.go:2668).
    #[allow(dead_code)] // The checkIdentifier caller loop lands with the W3 check-stage runner wiring.
    pub(crate) fn is_past_last_assignment_real(&mut self, symbol: SymbolId, location: Option<NodeId>) -> bool {
        self.ensure_assignments_marked(symbol);
        let last_assignment_pos = self.marked_assignment_symbol_links.get(symbol).last_assignment_pos;
        last_assignment_pos == 0
            || location.is_some_and(|l| (last_assignment_pos as i64) < self.store().node(l).loc.pos() as i64)
    }

    /// Go: isSomeSymbolAssigned — the flow-side probe isConstantReference uses
    /// (Go checker.go's each declared symbol of the declaration).
    fn is_some_symbol_assigned(&mut self, declaration: NodeId) -> bool {
        match self.get_symbol_of_declaration_merged(declaration) {
            Some(symbol) => self.is_symbol_assigned(symbol),
            None => false,
        }
    }

    /// Go: `func (c *Checker) ensureAssignmentsMarked(symbol)` (flow.go:2674).
    fn ensure_assignments_marked(&mut self, symbol: SymbolId) {
        let Some(value_declaration) = self.program_symbol(symbol).value_declaration else {
            return;
        };
        let Some(parent) = find_ancestor_id(self, value_declaration, &mut |c, n| {
            let cur = c.store().node(n);
            cur.kind == Kind::SourceFile || tsc_ast::is_function_like(cur)
        }) else {
            return;
        };
        if !self.node_links.get(parent).flags.intersects(NodeCheckFlags::ASSIGNMENTS_MARKED) {
            self.node_links.get(parent).flags |= NodeCheckFlags::ASSIGNMENTS_MARKED;
            if !self.has_parent_with_assignments_marked(parent) {
                self.mark_node_assignments(parent);
            }
        }
    }

    /// Go: `func (c *Checker) hasParentWithAssignmentsMarked(node) bool` (flow.go:2688).
    fn has_parent_with_assignments_marked(&mut self, node: NodeId) -> bool {
        // PORT: the walk is inlined (ast.FindAncestor with a LinkStore probe —
        // the store's Get is lazily allocating, so the callback needs &mut).
        let mut cur = self.store().node(node).parent.get();
        while !cur.is_none() {
            let kind = self.store().node(cur).kind;
            if (kind == Kind::SourceFile || tsc_ast::is_function_like_kind(kind))
                && self.node_links.get(cur).flags.intersects(NodeCheckFlags::ASSIGNMENTS_MARKED)
            {
                return true;
            }
            let next = self.store().node(cur).parent.get();
            if next.is_none() || next == cur {
                break;
            }
            cur = next;
        }
        false
    }

    /// Go: `func (c *Checker) markNodeAssignments(node)` (flow.go:2697 — the exported
    /// wrapper around markNodeAssignmentsWorker).
    fn mark_node_assignments(&mut self, node: NodeId) {
        self.mark_node_assignments_worker(node);
    }

    /// Go: `func (c *Checker) markNodeAssignmentsWorker(node) bool` (flow.go:2700).
    fn mark_node_assignments_worker(&mut self, node: NodeId) -> bool {
        let kind = self.store().node(node).kind;
        match kind {
            Kind::Identifier => {
                let assignment_kind = u::get_assignment_target_kind(self.store(), node);
                if assignment_kind != u::AssignmentKind::None {
                    let symbol = self.get_resolved_symbol(node);
                    if let Some(symbol) = symbol
                        && u::is_parameter_or_mutable_local_variable(self, self.store(), symbol)
                    {
                        // PORT: Go holds the links pointer across the ancestor
                        // walks; the port re-fetches after each &mut point
                        // (the links are value-identical arena slots).
                        let pos = self.marked_assignment_symbol_links.get(symbol).last_assignment_pos;
                        if pos == 0 || pos != i32::MAX {
                            let referencing_function = find_ancestor_id(self, node, &mut |c, n| {
                                let cur = c.store().node(n);
                                cur.kind == Kind::SourceFile || tsc_ast::is_function_like(cur)
                            });
                            let declaring_function = self
                                .program_symbol(symbol)
                                .value_declaration
                                .and_then(|d| {
                                    find_ancestor_id(self, d, &mut |c, n| {
                                        let cur = c.store().node(n);
                                        cur.kind == Kind::SourceFile
                                            || tsc_ast::is_function_like(cur)
                                    })
                                });
                            let declaration = self.program_symbol(symbol).value_declaration;
                            if referencing_function.is_some()
                                && referencing_function == declaring_function
                                && let Some(declaration) = declaration
                            {
                                let extended = self.extend_assignment_position(node, declaration);
                                self.marked_assignment_symbol_links.get(symbol).last_assignment_pos =
                                    extended;
                            } else {
                                self.marked_assignment_symbol_links.get(symbol).last_assignment_pos =
                                    i32::MAX;
                            }
                        }
                        if assignment_kind == u::AssignmentKind::Definite {
                            self.marked_assignment_symbol_links.get(symbol).has_definite_assignment =
                                true;
                        }
                    }
                }
                return false;
            }
            Kind::ExportSpecifier => {
                let export_specifier = self
                    .store()
                    .node(node)
                    .as_export_specifier()
                    .expect("ExportSpecifier data")
                    .clone();
                let export_declaration_parent = self.store().node(node).parent.get();
                let export_declaration = self.store().node(export_declaration_parent).parent.get();
                // Go: `node.PropertyNameOrName()` — PropertyName when set, else Name.
                let name = export_specifier.property_name.or(Some(export_specifier.name));
                let is_type_only = export_specifier.is_type_only;
                // PORT: Go's `exportDeclaration == nil` check is the NodeId
                // sentinel (§5.1).
                let export_is_type_only = !export_declaration.is_none()
                    && self
                        .store()
                        .node(export_declaration)
                        .as_export_declaration()
                        .is_some_and(|d| d.is_type_only);
                let module_specifier = if export_declaration.is_none() {
                    None
                } else {
                    crate::ast_shims::module_specifier_of(self.store(), export_declaration)
                };
                if !is_type_only
                    && !export_is_type_only
                    && module_specifier.is_none()
                    && name.is_some_and(|n| self.store().node(n).kind != Kind::StringLiteral)
                {
                    let symbol = self.resolve_entity_name(
                        name.expect("checked above"),
                        SymbolFlags::VALUE,
                        true, /*ignoreErrors*/
                        true, /*dontResolveAlias*/
                        NodeId(0),
                    );
                    if let Some(symbol) = symbol
                        && u::is_parameter_or_mutable_local_variable(self, self.store(), symbol)
                    {
                        self.marked_assignment_symbol_links.get(symbol).last_assignment_pos = i32::MAX;
                    }
                }
                return false;
            }
            Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::JSTypeAliasDeclaration
            | Kind::EnumDeclaration => return false,
            _ => {}
        }
        if binder_shims::is_type_node(self.store().node(node)) {
            return false;
        }
        let children: Vec<NodeId> = {
            let mut collected = Vec::new();
            self.store().node(node).for_each_child(&mut |child| {
                collected.push(child);
                false
            });
            collected
        };
        let mut result = false;
        for child in children {
            if self.mark_node_assignments_worker(child) {
                result = true;
            }
        }
        result
    }

    /// Go: `func (c *Checker) extendAssignmentPosition(node, declaration) int` (flow.go:2749).
    fn extend_assignment_position(&self, mut node: NodeId, declaration: NodeId) -> i32 {
        let mut pos = self.store().node(node).loc.pos();
        let declaration_pos = self.store().node(declaration).loc.pos();
        loop {
            let node_pos = self.store().node(node).loc.pos();
            if node_pos <= declaration_pos {
                break;
            }
            match self.store().node(node).kind {
                Kind::VariableStatement
                | Kind::ExpressionStatement
                | Kind::IfStatement
                | Kind::DoStatement
                | Kind::WhileStatement
                | Kind::ForStatement
                | Kind::ForInStatement
                | Kind::ForOfStatement
                | Kind::WithStatement
                | Kind::SwitchStatement
                | Kind::TryStatement
                | Kind::ClassDeclaration => {
                    pos = self.store().node(node).loc.end();
                }
                _ => {}
            }
            let parent = self.store().node(node).parent.get();
            if parent == node {
                break;
            }
            node = parent;
        }
        pos
    }

    // ── The checker.go predicates this wave owns (uniform/unit probes) ──

    /// Go: `func (c *Checker) isUniformUnionType(t) bool` (checker.go:28354).
    pub(crate) fn is_uniform_union_type(&mut self, t: TypeId) -> bool {
        if !self.type_(t).object_flags.intersects(ObjectFlags::PRIMITIVE_UNION) {
            return false;
        }
        if self
            .type_(t)
            .object_flags
            .intersects(ObjectFlags::IS_UNIFORM_ENUM_COMPUTED)
        {
            return self.type_(t).object_flags.intersects(ObjectFlags::IS_UNIFORM_ENUM);
        }
        let constituents = self.type_(t).types().to_vec();
        let is_uniform = self.compute_is_uniform_union_type(&constituents);
        self.type_mut(t).object_flags |= if is_uniform {
            ObjectFlags::IS_UNIFORM_ENUM_COMPUTED.union(ObjectFlags::IS_UNIFORM_ENUM)
        } else {
            ObjectFlags::IS_UNIFORM_ENUM_COMPUTED
        };
        is_uniform
    }

    /// Go: `func (c *Checker) computeIsUniformUnionType(types) bool` (checker.go:28364).
    fn compute_is_uniform_union_type(&mut self, types: &[TypeId]) -> bool {
        let mut enum_symbol: Option<SymbolId> = None;
        let mut has_string_or_number_literal = false;
        for &t in types {
            let flags = self.type_(t).flags;
            if flags.intersects(TypeFlags::ENUM_LIKE) {
                if has_string_or_number_literal {
                    return false;
                }
                let symbol = self.type_(t).symbol;
                let parent = symbol.and_then(|s| self.program_symbol(s).parent);
                if enum_symbol.is_none() {
                    enum_symbol = parent;
                } else if enum_symbol != parent {
                    return false;
                }
            } else if flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
                if enum_symbol.is_some() {
                    return false;
                }
                has_string_or_number_literal = true;
            } else {
                return false;
            }
        }
        true
    }

    /// Go: `func (c *Checker) extractUnitType(t) *Type` (checker.go:25960 region).
    pub(crate) fn extract_unit_type(&mut self, t: TypeId) -> TypeId {
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            if let Some(&u) = constituents.iter().find(|&&u| relater::is_unit_type(self, u)) {
                return u;
            }
        }
        t
    }

    // ── getNarrowableTypeForReference family (checker.go:32040) ──

    /// Go: `func (c *Checker) getNarrowableTypeForReference(t, reference, checkMode) *Type`
    /// (checker.go:32040) — REAL (W3; replaces the expressions.rs permissive
    /// identity stub, which delegates here).
    pub(crate) fn get_narrowable_type_for_reference_real(
        &mut self,
        t: TypeId,
        reference: NodeId,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut t = t;
        if self.is_no_infer_target_type(t) {
            if let Some(base_type) =
                self.type_(t).as_substitution_type().and_then(|s| s.base_type)
            {
                t = base_type;
            }
        }
        // When the type of a reference is or contains an instantiable type with a union type
        // constraint, and when the reference is in a constraint position (where it is known
        // we'll obtain the apparent type) or has a contextual type containing no top-level
        // instantiables (meaning constraints will determine assignability), we substitute
        // constraints for all instantiables in the type of the reference to give control flow
        // analysis an opportunity to narrow it further.
        let substitute_constraints = !check_mode.intersects(crate::checker::CheckMode::INFERENTIAL)
            && some_type_c(self, t, |c, tt| c.is_generic_type_with_union_constraint(tt))
            && (self.is_constraint_position(t, reference)
                || self.has_contextual_type_with_no_generic_types(reference, check_mode));
        if substitute_constraints {
            return crate::typenodes::map_type(self, t, |c, tt| {
                relater::get_base_constraint_or_type(c, tt)
            });
        }
        t
    }

    /// Go: `func (c *Checker) isConstraintPosition(t, node) bool` (checker.go:32053).
    fn is_constraint_position(&mut self, t: TypeId, node: NodeId) -> bool {
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        // In an element access obj[x], we consider obj to be in a constraint position, except
        // when obj is of a generic type without a nullable constraint and x is a generic type.
        parent_kind == Kind::PropertyAccessExpression
            || parent_kind == Kind::QualifiedName
            || ((parent_kind == Kind::CallExpression || parent_kind == Kind::NewExpression)
                && crate::ast_shims::expression_of(self.store(), parent) == Some(node))
            || parent_kind == Kind::ElementAccessExpression
                && crate::ast_shims::expression_of(self.store(), parent) == Some(node)
                && {
                    let argument = self
                        .store()
                        .node(parent)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression data")
                        .argument_expression;
                    !(some_type_c(self, t, |c, tt| {
                        c.is_generic_type_without_nullable_constraint(tt)
                    }) && {
                        let argument_type = self.get_type_of_expression(argument);
                        relater::is_generic_index_type(self, argument_type)
                    })
                }
    }

    /// Go: `func (c *Checker) isGenericTypeWithUnionConstraint(t) bool` (checker.go:32067).
    fn is_generic_type_with_union_constraint(&mut self, t: TypeId) -> bool {
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            return constituents
                .iter()
                .any(|&c| self.is_generic_type_with_union_constraint(c));
        }
        self.type_(t).flags.intersects(TypeFlags::INSTANTIABLE)
            && {
                let base = relater::get_base_constraint_or_type(self, t);
                self.type_(base)
                    .flags
                    .intersects(TypeFlags::NULLABLE.union(TypeFlags::UNION))
            }
    }

    /// Go: `func (c *Checker) isGenericTypeWithoutNullableConstraint(t) bool` (checker.go:32073).
    fn is_generic_type_without_nullable_constraint(&mut self, t: TypeId) -> bool {
        if self.type_(t).flags.intersects(TypeFlags::INTERSECTION) {
            let constituents = self.type_(t).types().to_vec();
            return constituents
                .iter()
                .any(|&c| self.is_generic_type_without_nullable_constraint(c));
        }
        self.type_(t).flags.intersects(TypeFlags::INSTANTIABLE)
            && {
                let base = relater::get_base_constraint_or_type(self, t);
                !relater::maybe_type_of_kind(self, base, TypeFlags::NULLABLE)
            }
    }

    /// Go: `func (c *Checker) hasContextualTypeWithNoGenericTypes(node, checkMode) bool` (checker.go:32079).
    fn has_contextual_type_with_no_generic_types(&mut self, node: NodeId, check_mode: CheckMode) -> bool {
        // Computing the contextual type for a child of a JSX element involves resolving the type
        // of the element's tag name, so we exclude that here to avoid circularities. If check
        // mode has `CheckMode.RestBindingElement`, we skip binding pattern contextual types, as
        // we want the type of a rest element to be generic when possible.
        let kind = self.store().node(node).kind;
        let parent = self.store().node(node).parent.get();
        let parent_kind = self.store().node(parent).kind;
        let parent_tag_is_node = (parent_kind == Kind::JsxOpeningElement
            && matches!(
                &self.store().node(parent).data,
                tsc_ast::NodeData::JsxOpeningElement(d) if d.tag_name == node
            ))
            || (parent_kind == Kind::JsxSelfClosingElement
                && matches!(
                    &self.store().node(parent).data,
                    tsc_ast::NodeData::JsxSelfClosingElement(d) if d.tag_name == node
                ));
        if matches!(kind, Kind::Identifier | Kind::PropertyAccessExpression | Kind::ElementAccessExpression)
            && !parent_tag_is_node
        {
            let context_flags = if check_mode.intersects(crate::checker::CheckMode::REST_BINDING_ELEMENT) {
                crate::types::ContextFlags::SKIP_BINDING_PATTERNS
            } else {
                crate::types::ContextFlags::NONE
            };
            if let Some(contextual_type) = self.get_contextual_type(node, context_flags) {
                return !self.is_generic_type(contextual_type);
            }
        }
        false
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (flow.go's non-receiver functions + the checker.go predicates
// this wave owns)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getCandidateVariableDeclarationInitializer(node) *ast.Node` (flow.go:1496).
fn get_candidate_variable_declaration_initializer(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    if store.node(node).kind == Kind::VariableDeclaration
        && crate::ast_shims::type_of_node(store, node).is_none()
        && let Some(initializer) = crate::ast_shims::initializer_of(store, node)
    {
        return Some(tsc_ast::skip_parentheses(store, initializer));
    }
    None
}

/// Go: `func isEvolvingArrayTypeList(types) bool` (flow.go:1527).
fn is_evolving_array_type_list(c: &Checker, types: &[TypeId]) -> bool {
    let mut has_evolving_array_type = false;
    for &t in types {
        if !c.type_(t).flags.intersects(TypeFlags::NEVER) {
            if !c.type_(t).object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
                return false;
            }
            has_evolving_array_type = true;
        }
    }
    has_evolving_array_type
}

/// Go: `func tryGetNameFromType(t) (string, bool)` (flow.go:1782; note the
/// checker.go:19128 method twin used by
/// getEffectivePropertyNameForPropertyNameNode has RAW literal payloads —
/// this one renders through AnyToString).
fn try_get_name_from_type(t: &crate::types::Type) -> Option<String> {
    if t.flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
        return t.as_unique_essymbol_type().map(|s| s.name.clone());
    }
    if t.flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
        // PORT(FCS tranche 5): Go renders the literal payload through
        // evaluator.AnyToString (RAW — no quote wrapping); the port used the
        // quoting `value_to_string`, diverging on string-literal names.
        return t
            .as_literal_type()
            .and_then(|l| l.value.as_ref())
            .map(crate::utilities::any_to_string);
    }
    None
}

/// Go: `func isCoercibleUnderDoubleEquals(source, target) bool` (flow.go:1928).
fn is_coercible_under_double_equals(source: &crate::types::Type, target: &crate::types::Type) -> bool {
    source
        .flags
        .intersects(TypeFlags::NUMBER.union(TypeFlags::STRING).union(TypeFlags::BOOLEAN_LITERAL))
        && target
            .flags
            .intersects(TypeFlags::NUMBER.union(TypeFlags::STRING).union(TypeFlags::BOOLEAN))
}

/// Go: `func isNeitherUnitTypeNorNever(t) bool` (checker.go:25940).
pub(crate) fn is_neither_unit_type_nor_never(t: &crate::types::Type) -> bool {
    !t.flags.intersects(TypeFlags::UNIT.union(TypeFlags::NEVER))
}

/// Go: `ast.FindAncestor` — the id-carrying variant this module needs (the
/// ast_shims form hands the callback a `&ast` slot; the flow walk predicates
/// read checker state by `NodeId`).
fn find_ancestor_id(
    c: &Checker,
    start: NodeId,
    f: &mut dyn FnMut(&Checker, NodeId) -> bool,
) -> Option<NodeId> {
    let mut node = start;
    loop {
        if f(c, node) {
            return Some(node);
        }
        let parent = c.store().node(node).parent.get();
        if parent.is_none() {
            return None;
        }
        node = parent;
    }
}
