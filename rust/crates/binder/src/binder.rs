// Ported from tsc/internal/binder/binder.go @ ec47d33c23e464a17cdf2475632cba629bee8763.
//
// Go name mapping (crate contract, see src/lib.rs):
//   Binder.bindFunc / sync.Pool          → dropped (plain method call per
//                                          bind; see bindSourceFile PORT)
//   *FlowNode / *FlowList                → FlowNodeId / FlowListId into the
//                                          binder's Vec arenas
//   *Symbol                              → SymbolId into the binder's
//                                          Vec<Symbol> arena
//   nil *FlowNode in Binder state        → FlowNodeId::NONE sentinel (flow.rs);
//                                          label pointers Go nil-checks
//                                          (currentBreakTarget, ...) stay
//                                          Option<FlowNodeId>
//   symbolTable param (Go *ast.SymbolTable, i.e. a map reference picked
//   per call site) → TableLoc (Locals(node) | Exports(sym) | Members(sym)
//   | GlobalExports), since every Go call site passes exactly one of
//   GetExports/GetMembers/GetLocals/file.GlobalExports
//
// PORT (arenas): Go allocates Symbols/FlowNodes/FlowLists/declaration
// slices in core.Arena arenas owned by the Binder; Go's *Symbol pointers
// survive the binder (GC objects referenced from node payloads). The port's
// node payloads carry SymbolId/FlowNodeId HANDLES, so the arenas must
// outlive the bind: BindSourceFile hands them back as BindResult (the
// per-file symbol_count already lives on the SourceFile payload). The ids
// are per-file; the M5 program merge re-bases them into the program-level
// arenas (SPEC §5.10), same scheme as flow.rs's flow-node ids.

use tsc_ast::{
    has_syntactic_modifier, is_binary_expression, is_class_declaration, is_export_assignment,
    is_export_specifier, is_function_declaration, is_function_expression, is_identifier,
    is_module_declaration, is_namespace_export, is_optional_chain, is_private_identifier,
    is_type_alias_declaration, is_variable_declaration, is_variable_statement, node_is_missing,
    node_is_present, node_text, Diagnostic, FlowNodeId, Kind, ModifierFlags, NodeFlags, NodeId,
    NodeList, NodeStore, SourceFile, Symbol, SymbolFlags, SymbolId, PatternAmbientModule,
    INTERNAL_SYMBOL_NAME_CALL, INTERNAL_SYMBOL_NAME_CLASS, INTERNAL_SYMBOL_NAME_COMPUTED,
    INTERNAL_SYMBOL_NAME_CONSTRUCTOR, INTERNAL_SYMBOL_NAME_DEFAULT, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
    INTERNAL_SYMBOL_NAME_EXPORT_STAR, INTERNAL_SYMBOL_NAME_FUNCTION, INTERNAL_SYMBOL_NAME_GLOBAL,
    INTERNAL_SYMBOL_NAME_INDEX, INTERNAL_SYMBOL_NAME_JSX_ATTRIBUTES, INTERNAL_SYMBOL_NAME_MISSING,
    INTERNAL_SYMBOL_NAME_NEW, INTERNAL_SYMBOL_NAME_OBJECT,
    INTERNAL_SYMBOL_NAME_PREFIX, INTERNAL_SYMBOL_NAME_TYPE, INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION,
};
use tsc_ast::flow::{
    FlowFlags, FlowList, FlowListId, FlowNode, FlowNodeData, FlowReduceLabelData,
    FlowSwitchClauseData,
};
use tsc_collections::Set;
use tsc_core::pattern::try_parse_pattern;
use tsc_core::text::TextRange;
use tsc_diagnostics::Message;
use tsc_tspath::remove_file_extension;

use crate::ast_shims::{
    get_assignment_declaration_kind, get_combined_modifier_flags, get_containing_class,
    get_immediately_invoked_function_expression, get_module_instance_state, get_name_of_declaration,
    has_dynamic_name, is_ambient_module, is_assignment_target, is_block_or_catch_scoped,
    is_boolean_literal, is_declaration_statement, is_destructuring_assignment, is_dotted_name,
    is_entity_name_expression, is_expression_of_optional_chain_root, is_external_module,
    is_external_or_common_js_module, is_for_in_or_of_statement, is_global_scope_augmentation,
    is_identifier_name, is_in_top_level_context, is_json_source_file, is_left_hand_side_expression,
    is_module_augmentation_external, is_nullish_coalesce, is_object_literal_method,
    is_object_literal_or_class_expression_method_or_accessor, is_optional_chain_root,
    is_outermost_optional_chain, is_part_of_parameter_declaration, is_part_of_type_query,
    is_potentially_executable_node, is_property_name_literal, is_push_or_unshift_identifier,
    is_signed_numeric_literal, is_string_literal_like, is_string_or_numeric_literal_like,
    is_variable_declaration_initialized_to_require, ModuleInstanceState, JSDeclarationKind,
};
use crate::node_data;
use crate::scanner_shims::{
    declaration_name_to_string, get_error_range_for_node, get_identifier_token,
    get_range_of_token_at_position, token_to_string,
};

// ────────────────────────────────────────────────────────────────────────────
// ContainerFlags (binder.go:16-42)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ContainerFlags int32` + iota consts (exact bit values).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct ContainerFlags(pub i32);

impl ContainerFlags {
    pub const NONE: ContainerFlags = ContainerFlags(0);
    /// The current node is a container.  It should be set as the current container (and block-
    /// container) before recursing into it.  The current node does not have locals.  Examples:
    /// Classes, ObjectLiterals, TypeLiterals, Interfaces...
    pub const IS_CONTAINER: ContainerFlags = ContainerFlags(1 << 0);
    /// The current node is a block-scoped-container.  It should be set as the current block-
    /// container before recursing into it.  Examples: Blocks (when not parented by functions),
    /// Catch clauses, For/For-in/For-of statements...
    pub const IS_BLOCK_SCOPED_CONTAINER: ContainerFlags = ContainerFlags(1 << 1);
    /// The current node is the container of a control flow path. The current control flow should
    /// be saved and restored, and a new control flow initialized within the container.
    pub const IS_CONTROL_FLOW_CONTAINER: ContainerFlags = ContainerFlags(1 << 2);
    pub const IS_FUNCTION_LIKE: ContainerFlags = ContainerFlags(1 << 3);
    pub const IS_FUNCTION_EXPRESSION: ContainerFlags = ContainerFlags(1 << 4);
    pub const HAS_LOCALS: ContainerFlags = ContainerFlags(1 << 5);
    pub const IS_INTERFACE: ContainerFlags = ContainerFlags(1 << 6);
    pub const IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR: ContainerFlags = ContainerFlags(1 << 7);
    pub const IS_THIS_CONTAINER: ContainerFlags = ContainerFlags(1 << 8);
    pub const PROPAGATES_THIS_KEYWORD: ContainerFlags = ContainerFlags(1 << 9);

    #[inline]
    pub const fn intersects(self, other: ContainerFlags) -> bool {
        (self.0 & other.0) != 0
    }

    /// Go: `flags | other` (same shape as tsc-ast's define_flags union).
    #[inline]
    pub const fn union(self, other: ContainerFlags) -> ContainerFlags {
        ContainerFlags(self.0 | other.0)
    }
}

impl std::ops::BitOr for ContainerFlags {
    type Output = ContainerFlags;
    #[inline]
    fn bitor(self, rhs: ContainerFlags) -> ContainerFlags {
        ContainerFlags(self.0 | rhs.0)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Support types
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ExpandoAssignmentInfo struct`.
#[derive(Clone, Copy)]
struct ExpandoAssignmentInfo {
    node: NodeId,
    container: NodeId,
    block_scope_container: NodeId,
}

/// Go: `type ActiveLabel struct` — the linked list is a Box chain (the Go
/// `next *ActiveLabel` pointer).
struct ActiveLabel {
    next: Option<Box<ActiveLabel>>,
    break_target: Option<FlowNodeId>,
    continue_target: Option<FlowNodeId>,
    name: String,
    referenced: bool,
}

/// Go: the `getTarget func(*ActiveLabel) *ast.FlowNode` parameter of
/// bindBreakOrContinueStatement (`(*ActiveLabel).BreakTarget` /
/// `(*ActiveLabel).ContinueTarget` at the two call sites).
#[derive(Clone, Copy, PartialEq, Eq)]
enum LabelSlot {
    Break,
    Continue,
}

/// The destination symbol table of a declareSymbol call (see the header PORT
/// note): every Go call site passes exactly one of GetExports(symbol) /
/// GetMembers(symbol) / GetLocals(container) / the file's GlobalExports.
#[derive(Clone, Copy)]
enum TableLoc {
    /// Go: `ast.GetLocals(container)` (lazily-allocated in Go; the port's
    /// payload table is always present).
    Locals(NodeId),
    /// Go: `ast.GetExports(symbol)`.
    Exports(SymbolId),
    /// Go: `ast.GetMembers(symbol)`.
    Members(SymbolId),
    /// Go: `ast.GetSymbolTable(&b.file.GlobalExports)`.
    GlobalExports,
}

/// The binder arenas, handed back to the caller (see the header PORT note):
/// node payloads reference these by SymbolId / FlowNodeId / FlowListId.
pub struct BindResult {
    pub symbol_count: i32,
    /// `SymbolId` indexes this (per-file; re-based at the M5 program merge).
    pub symbols: Vec<Symbol>,
    /// `FlowNodeId` indexes this.
    pub flow_nodes: Vec<FlowNode>,
    /// `FlowListId` indexes this.
    pub flow_lists: Vec<FlowList>,
}

// ────────────────────────────────────────────────────────────────────────────
// BindSourceFile (binder.go:94-129)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func BindSourceFile(file *ast.SourceFile)` — plus the port's arena
/// return (`Some(BindResult)` when the file was freshly bound, `None` when
/// it was already bound, mirroring Go's IsBound fast path).
///
/// PORT: Go pools Binder values in `binderPool` (sync.Pool) and reuses the
/// pre-allocated `bindFunc` closure across binds; the port plain-constructs
/// one Binder per bind (the pooled state is exactly the fields re-inited
/// below, so the pool saves one small allocation).
pub fn bind_source_file(file: &mut SourceFile) -> Option<BindResult> {
    let mut result: Option<BindResult> = None;
    file.bind_once(|file| {
        let file_node = file.as_node_id();
        let mut b = Binder::new(file);
        // Go: b.unreachableFlow = b.newFlowNode(ast.FlowFlagsUnreachable)
        b.unreachable_flow = b.new_flow_node(FlowFlags::UNREACHABLE);
        b.bind(Some(file_node));
        b.bind_deferred_expando_assignments();
        result = Some(b.into_result());
    });
    result
}

// ────────────────────────────────────────────────────────────────────────────
// Binder
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Binder struct`. `file` replaces the Go back-pointer (the port's
/// nodes resolve through the owning SourceFile's NodeStore).
pub struct Binder<'f> {
    file: &'f mut SourceFile,
    unreachable_flow: FlowNodeId,

    container: NodeId,
    this_container: Option<NodeId>,
    block_scope_container: NodeId,
    last_container: Option<NodeId>,
    current_flow: FlowNodeId,
    current_break_target: Option<FlowNodeId>,
    current_continue_target: Option<FlowNodeId>,
    current_return_target: Option<FlowNodeId>,
    current_true_target: Option<FlowNodeId>,
    current_false_target: Option<FlowNodeId>,
    current_exception_target: Option<FlowNodeId>,
    pre_switch_case_flow: FlowNodeId,
    active_label_list: Option<Box<ActiveLabel>>,
    emit_flags: NodeFlags,
    seen_this_keyword: bool,
    has_explicit_return: bool,
    has_flow_effects: bool,
    in_assignment_pattern: bool,
    seen_parse_error: bool,
    symbol_count: i32,
    not_const_enum_only_modules: Set<SymbolId>,
    symbols: Vec<Symbol>,
    flow_nodes: Vec<FlowNode>,
    flow_lists: Vec<FlowList>,
    expando_assignments: Vec<ExpandoAssignmentInfo>,
}

impl<'f> Binder<'f> {
    fn new(file: &'f mut SourceFile) -> Binder<'f> {
        // Go: zero Binder (putBinder's reset state).
        Binder {
            file,
            unreachable_flow: FlowNodeId::NONE,
            container: NodeId::NONE,
            this_container: None,
            block_scope_container: NodeId::NONE,
            last_container: None,
            current_flow: FlowNodeId::NONE,
            current_break_target: None,
            current_continue_target: None,
            current_return_target: None,
            current_true_target: None,
            current_false_target: None,
            current_exception_target: None,
            pre_switch_case_flow: FlowNodeId::NONE,
            active_label_list: None,
            emit_flags: NodeFlags::NONE,
            seen_this_keyword: false,
            has_explicit_return: false,
            has_flow_effects: false,
            in_assignment_pattern: false,
            seen_parse_error: false,
            symbol_count: 0,
            not_const_enum_only_modules: Set::new(),
            symbols: Vec::new(),
            flow_nodes: Vec::new(),
            flow_lists: Vec::new(),
            expando_assignments: Vec::new(),
        }
    }

    /// Go: `file.SymbolCount = b.symbolCount` + the arena handoff (PORT,
    /// see the header note).
    fn into_result(self) -> BindResult {
        self.file.payload_mut().symbol_count = self.symbol_count;
        BindResult {
            symbol_count: self.symbol_count,
            symbols: self.symbols,
            flow_nodes: self.flow_nodes,
            flow_lists: self.flow_lists,
        }
    }

    // ── flow arena accessors (Go: core.Arena[ast.FlowNode] / [ast.FlowList]) ──

    #[inline]
    fn flow(&self, id: FlowNodeId) -> &FlowNode {
        &self.flow_nodes[id.0 as usize]
    }

    #[inline]
    fn flow_mut(&mut self, id: FlowNodeId) -> &mut FlowNode {
        &mut self.flow_nodes[id.0 as usize]
    }

    // ── symbol table shims (Go: ast.GetExports / GetMembers / GetLocals) ──

    fn table_get(&self, table: &TableLoc, name: &str) -> Option<SymbolId> {
        match table {
            TableLoc::Locals(node) => {
                node_data::locals_of(&*self.file, *node).and_then(|l| l.get(name).copied())
            }
            TableLoc::Exports(symbol) => self.symbols[symbol.0 as usize].exports.get(name).copied(),
            TableLoc::Members(symbol) => self.symbols[symbol.0 as usize].members.get(name).copied(),
            TableLoc::GlobalExports => self.file.payload().global_exports.get(name).copied(),
        }
    }

    fn table_set(&mut self, table: TableLoc, name: &str, value: SymbolId) {
        let owned = name.to_string();
        match table {
            TableLoc::Locals(node) => {
                let Some(table) = node_data::locals_mut(&mut *self.file, node) else {
                    // Go: GetLocals panics on a non-locals container.
                    panic!("GetLocals: node kind {:?} is not a locals container", self.file.node(node).kind);
                };
                table.set(owned, value);
            }
            TableLoc::Exports(symbol) => {
                self.symbols[symbol.0 as usize].exports.set(owned, value);
            }
            TableLoc::Members(symbol) => {
                self.symbols[symbol.0 as usize].members.set(owned, value);
            }
            TableLoc::GlobalExports => {
                self.file.payload_mut().global_exports.set(owned, value);
            }
        }
    }

    /// Go: `container.Symbol()` — the binder only reads it on containers it
    /// just declared a symbol into (nil-deref in Go ≡ expect here).
    fn container_symbol(&self, container: NodeId) -> Option<SymbolId> {
        node_data::symbol_of(&*self.file, container)
    }

    fn container_symbol_expect(&self, container: NodeId) -> SymbolId {
        self.container_symbol(container)
            .unwrap_or_else(|| panic!("container kind {:?} has no symbol", self.file.node(container).kind))
    }

    // ────────────────────────────────────────────────────────────────────────
    // Symbol creation (binder.go:131-137)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) newSymbol(flags ast.SymbolFlags, name string) *ast.Symbol`.
    fn new_symbol(&mut self, flags: SymbolFlags, name: &str) -> SymbolId {
        self.symbol_count += 1;
        let symbol = Symbol {
            flags,
            name: name.to_string(),
            ..Symbol::default()
        };
        self.symbols.push(symbol);
        SymbolId(self.symbols.len() as u32 - 1)
    }

    /// Go: `func (b *Binder) declareSymbol(symbolTable, parent, node, includes, excludes) *ast.Symbol`.
    fn declare_symbol(
        &mut self,
        symbol_table: TableLoc,
        parent: Option<SymbolId>,
        node: NodeId,
        includes: SymbolFlags,
        excludes: SymbolFlags,
    ) -> SymbolId {
        self.declare_symbol_ex(
            symbol_table,
            parent,
            node,
            includes,
            excludes,
            false, /*isReplaceableByMethod*/
            false, /*isComputedName*/
        )
    }

    /// Go: `func (b *Binder) declareSymbolEx(...) *ast.Symbol` — verbatim,
    /// with the TableLoc shim standing in for the Go map reference.
    #[allow(clippy::too_many_arguments)] // Go: declareSymbolEx has 8 params
    fn declare_symbol_ex(
        &mut self,
        symbol_table: TableLoc,
        parent: Option<SymbolId>,
        node: NodeId,
        includes: SymbolFlags,
        excludes: SymbolFlags,
        is_replaceable_by_method: bool,
        is_computed_name: bool,
    ) -> SymbolId {
        debug_assert!(is_computed_name || !has_dynamic_name(&*self.file, node));
        let is_default_export = has_syntactic_modifier(self.file.node(node), ModifierFlags::DEFAULT)
            || is_export_specifier(self.file.node(node))
                && module_export_name_is_default(
                    &*self.file,
                    self.file.node(node).name().expect("ExportSpecifier name"),
                );
        // The exported symbol for an export default function/class node is always named "default"
        let name = if is_computed_name {
            INTERNAL_SYMBOL_NAME_COMPUTED.to_string()
        } else if is_default_export && parent.is_some() {
            INTERNAL_SYMBOL_NAME_DEFAULT.to_string()
        } else {
            self.get_declaration_name(node)
        };
        let mut symbol;
        if name == INTERNAL_SYMBOL_NAME_MISSING {
            symbol = Some(self.new_symbol(SymbolFlags::NONE, INTERNAL_SYMBOL_NAME_MISSING));
        } else {
            // Check and see if the symbol table already has a symbol with this name.  If not,
            // create a new symbol with this name and add it to the table.  Note that we don't
            // give the new symbol any flags *yet*.  This ensures that it will not conflict
            // with the 'excludes' flags we pass in.
            //
            // If we do get an existing symbol, see if it conflicts with the new symbol we're
            // creating.  For example, a 'var' symbol and a 'class' symbol will conflict within
            // the same symbol table.  If we have a conflict, report the issue on each
            // declaration we have for this symbol, and then create a new symbol for this
            // declaration.
            //
            // Note that when properties declared in Javascript constructors
            // (marked by isReplaceableByMethod) conflict with another symbol, the property loses.
            // Always. This allows the common Javascript pattern of overwriting a prototype method
            // with an bound instance method of the same type: `this.method = this.method.bind(this)`
            //
            // If we created a new symbol, either because we didn't have a symbol with this name
            // in the symbol table, or we conflicted with an existing symbol, then just add this
            // node as the sole declaration of the new symbol.
            //
            // Otherwise, we'll be merging into a compatible existing symbol (for example when
            // you have multiple 'vars' with the same name in the same container).  In this case
            // just add this node into the declarations list of the symbol.
            symbol = self.table_get(&symbol_table, &name);
            if symbol.is_none() {
                let fresh = self.new_symbol(SymbolFlags::NONE, &name);
                self.table_set(symbol_table, &name, fresh);
                if is_replaceable_by_method {
                    self.symbols[fresh.0 as usize].flags |= SymbolFlags::REPLACEABLE_BY_METHOD;
                }
                symbol = Some(fresh);
            } else {
                let existing = symbol.unwrap();
                let flags = self.symbols[existing.0 as usize].flags;
                if is_replaceable_by_method && !flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD) {
                    // A symbol already exists, so don't add this as a declaration.
                    return existing;
                } else if flags.intersects(excludes) {
                    if flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD) {
                        // Javascript constructor-declared symbols can be discarded in favor of
                        // prototype symbols like methods.
                        let fresh = self.new_symbol(SymbolFlags::NONE, &name);
                        self.table_set(symbol_table, &name, fresh);
                        symbol = Some(fresh);
                    } else if !(includes.intersects(SymbolFlags::VARIABLE) && flags.intersects(SymbolFlags::ASSIGNMENT)
                        || includes.intersects(SymbolFlags::ASSIGNMENT) && flags.intersects(SymbolFlags::VARIABLE))
                    {
                        // Assignment declarations are allowed to merge with variables, no matter what other flags they have.
                        // Report errors every position with duplicate declaration
                        // Report errors on previous encountered declarations
                        let mut message: &'static Message = &tsc_diagnostics::DUPLICATE_IDENTIFIER_0;
                        if flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
                            message = &tsc_diagnostics::CANNOT_REDECLARE_BLOCK_SCOPED_VARIABLE_0;
                        }
                        let mut message_needs_name = true;
                        if flags.intersects(SymbolFlags::ENUM) || includes.intersects(SymbolFlags::ENUM) {
                            message = &tsc_diagnostics::ENUM_DECLARATIONS_CAN_ONLY_MERGE_WITH_NAMESPACE_OR_OTHER_ENUM_DECLARATIONS;
                            message_needs_name = false;
                        }
                        let mut multiple_default_exports = false;
                        let existing_declarations = self.symbols[existing.0 as usize].declarations.clone();
                        if !existing_declarations.is_empty() {
                            // If the current node is a default export of some sort, then check if
                            // there are any other default exports that we need to error on.
                            // We'll know whether we have other default exports depending on if `symbol` already has a declaration list set.
                            if is_default_export {
                                message = &tsc_diagnostics::A_MODULE_CANNOT_HAVE_MULTIPLE_DEFAULT_EXPORTS;
                                message_needs_name = false;
                                multiple_default_exports = true;
                            } else {
                                // This is to properly report an error in the case "export default { }" is after export default of class declaration or function declaration.
                                // Error on multiple export default in the following case:
                                // 1. multiple export default of class declaration or function declaration by checking NodeFlags.Default
                                // 2. multiple export default of export assignment. This one doesn't have NodeFlags.Default on (as export default doesn't considered as modifiers)
                                let is_export_equals = match &self.file.node(node).data {
                                    tsc_ast::NodeData::ExportAssignment(d) => d.is_export_equals,
                                    _ => false,
                                };
                                if is_export_assignment(self.file.node(node)) && !is_export_equals {
                                    message = &tsc_diagnostics::A_MODULE_CANNOT_HAVE_MULTIPLE_DEFAULT_EXPORTS;
                                    message_needs_name = false;
                                    multiple_default_exports = true;
                                }
                            }
                        }
                        let declaration_name = get_name_of_declaration(&*self.file, node).unwrap_or(node);
                        let mut diag = if message_needs_name {
                            let display_name = self.get_display_name(node);
                            self.create_diagnostic_for_node(declaration_name, message, &[display_name])
                        } else {
                            self.create_diagnostic_for_node(declaration_name, message, &[])
                        };
                        // Go: `ast.NodeIsMissing(node.Type())`. Go's node.Type()
                        // returns nil for an ExportAssignment (`export default
                        // <expr>`), and NodeIsMissing treats nil as missing.
                        // PORT: the port's type_of dispatch returns the
                        // payload's non-Option `type_` field, which is
                        // NodeId::NONE for Go's nil — and payloads without a
                        // type slot read None (Go's nil interface). Both map to
                        // "missing" here instead of dereferencing the arena
                        // (NodeId::NONE is foreign to the arena's node()).
                        let is_missing_type = match node_data::type_of(&*self.file, node) {
                            Some(t) if !t.is_none() => node_is_missing(self.file.node(t)),
                            _ => true,
                        };
                        if is_type_alias_declaration(self.file.node(node))
                            && is_missing_type
                            && has_syntactic_modifier(self.file.node(node), ModifierFlags::EXPORT)
                            && flags.intersects(SymbolFlags::ALIAS.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE))
                        {
                            // export type T; - may have meant export type { T }?
                            let name_text = match &self.file.node(node).data {
                                tsc_ast::NodeData::TypeAliasDeclaration(d) => {
                                    node_text(&*self.file, d.name).to_string()
                                }
                                _ => unreachable!("IsTypeAliasDeclaration checked above"),
                            };
                            let hint = format!("export type {{ {name_text} }}");
                            diag.add_related_info(self.create_diagnostic_for_node(
                                node,
                                &tsc_diagnostics::DID_YOU_MEAN_0,
                                &[hint],
                            ));
                        }
                        for (index, &declaration) in existing_declarations.iter().enumerate() {
                            let decl = get_name_of_declaration(&*self.file, declaration).unwrap_or(declaration);
                            let mut d = if message_needs_name {
                                let display_name = self.get_display_name(declaration);
                                self.create_diagnostic_for_node(decl, message, &[display_name])
                            } else {
                                self.create_diagnostic_for_node(decl, message, &[])
                            };
                            if multiple_default_exports {
                                let m = if index == 0 {
                                    &tsc_diagnostics::ANOTHER_EXPORT_DEFAULT_IS_HERE
                                } else {
                                    &tsc_diagnostics::X_AND_HERE
                                };
                                d.add_related_info(self.create_diagnostic_for_node(declaration_name, m, &[]));
                            }
                            self.add_diagnostic(d);
                            if multiple_default_exports {
                                diag.add_related_info(self.create_diagnostic_for_node(
                                    decl,
                                    &tsc_diagnostics::THE_FIRST_EXPORT_DEFAULT_IS_HERE,
                                    &[],
                                ));
                            }
                        }
                        self.add_diagnostic(diag);
                        // When get or set accessor conflicts with a non-accessor or an accessor of a different kind, we mark
                        // the symbol as a full accessor such that all subsequent declarations are considered conflicting. This
                        // for example ensures that a get accessor followed by a non-accessor followed by a set accessor with the
                        // same name are all marked as duplicates.
                        let accessor = SymbolFlags::GET_ACCESSOR.union(SymbolFlags::SET_ACCESSOR);
                        if flags.intersects(accessor) && (flags & accessor) != (includes & accessor) {
                            self.symbols[existing.0 as usize].flags |= accessor;
                        }
                        let fresh = self.new_symbol(SymbolFlags::NONE, &name);
                        symbol = Some(fresh);
                    }
                }
            }
        }
        let symbol = symbol.unwrap();
        self.add_declaration_to_symbol(symbol, node, includes);
        let sym_parent = self.symbols[symbol.0 as usize].parent;
        if sym_parent.is_none() {
            self.symbols[symbol.0 as usize].parent = parent;
        } else if sym_parent != parent {
            panic!("Existing symbol parent should match new one");
        }
        symbol
    }

    // ────────────────────────────────────────────────────────────────────────
    // Declaration names (binder.go:300-375)
    // ────────────────────────────────────────────────────────────────────────

    // Should not be called on a declaration with a computed property name,
    // unless it is a well known Symbol.
    /// Go: `func (b *Binder) getDeclarationName(node *ast.Node) string`.
    fn get_declaration_name(&self, node: NodeId) -> String {
        let n = self.file.node(node);
        if is_export_assignment(n) {
            let is_export_equals = match &n.data {
                tsc_ast::NodeData::ExportAssignment(d) => d.is_export_equals,
                _ => unreachable!("IsExportAssignment checked above"),
            };
            return if is_export_equals {
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS.to_string()
            } else {
                INTERNAL_SYMBOL_NAME_DEFAULT.to_string()
            };
        }
        if let Some(name) = get_name_of_declaration(&*self.file, node) {
            if is_ambient_module(&*self.file, node) {
                let module_name = node_text(&*self.file, name);
                if is_global_scope_augmentation(n) {
                    return INTERNAL_SYMBOL_NAME_GLOBAL.to_string();
                }
                let pattern = try_parse_pattern(module_name);
                if pattern.is_valid() && pattern.star_index >= 0 {
                    let attributes = match &n.data {
                        tsc_ast::NodeData::ModuleDeclaration(d) => d.attributes,
                        _ => None,
                    };
                    if let Some(attributes) = attributes {
                        let id = crate::ast_shims::get_node_id(self.file.node(attributes));
                        return format!(
                            "{INTERNAL_SYMBOL_NAME_PREFIX}\"{module_name}\"pattern@{id}"
                        );
                    }
                }
                return format!("\"{module_name}\"");
            }
            if is_private_identifier(self.file.node(name)) {
                // containingClass exists because private names only allowed inside classes
                let Some(containing_class) = get_containing_class(&*self.file, node) else {
                    // we can get here in cases where there is already a parse error.
                    return INTERNAL_SYMBOL_NAME_MISSING.to_string();
                };
                let class_symbol = self.container_symbol_expect(containing_class);
                let description = node_text(&*self.file, name);
                return self.get_symbol_name_for_private_identifier(class_symbol, description);
            }
            if is_property_name_literal(self.file.node(name))
                || tsc_ast::is_jsx_namespaced_name(self.file.node(name))
            {
                // Go: `return name.Text()` — the KindJsxNamespacedName arm
                // of Text() is mirrored by the shim (see its PORT note).
                return crate::ast_shims::jsx_namespaced_name_text(&*self.file, name);
            }
            if self.file.node(name).kind == Kind::ComputedPropertyName {
                let name_expression =
                    node_data::expression_of(&*self.file, name).expect("ComputedPropertyName expression");
                // treat computed property names where expression is string/numeric literal as just string/numeric literal
                if is_string_or_numeric_literal_like(self.file.node(name_expression)) {
                    return node_text(&*self.file, name_expression).to_string();
                }
                if is_signed_numeric_literal(&*self.file, name_expression) {
                    let expr = self
                        .file
                        .node(name_expression)
                        .as_prefix_unary_expression()
                        .expect("PrefixUnaryExpression data");
                    let operator = token_to_string(expr.operator);
                    let operand = node_text(&*self.file, expr.operand);
                    return format!("{operator}{operand}");
                }
                panic!("Only computed properties with literal names have declaration names");
            }
            return INTERNAL_SYMBOL_NAME_MISSING.to_string();
        }
        match self.file.node(node).kind {
            Kind::Constructor => INTERNAL_SYMBOL_NAME_CONSTRUCTOR.to_string(),
            Kind::FunctionType | Kind::CallSignature => INTERNAL_SYMBOL_NAME_CALL.to_string(),
            Kind::ConstructorType | Kind::ConstructSignature => INTERNAL_SYMBOL_NAME_NEW.to_string(),
            Kind::IndexSignature => INTERNAL_SYMBOL_NAME_INDEX.to_string(),
            Kind::ExportDeclaration => INTERNAL_SYMBOL_NAME_EXPORT_STAR.to_string(),
            Kind::SourceFile | Kind::BinaryExpression => {
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS.to_string()
            }
            _ => INTERNAL_SYMBOL_NAME_MISSING.to_string(),
        }
    }

    /// Go: `func (b *Binder) getDisplayName(node *ast.Node) string`.
    fn get_display_name(&self, node: NodeId) -> String {
        let name_node = self.file.node(node).name();
        if let Some(name_node) = name_node {
            return declaration_name_to_string(&*self.file, Some(name_node));
        }
        let name = self.get_declaration_name(node);
        if name != INTERNAL_SYMBOL_NAME_MISSING {
            return name;
        }
        "(Missing)".to_string()
    }

    /// Go: `func GetSymbolNameForPrivateIdentifier(containingClassSymbol *ast.Symbol, description string) string`.
    pub fn get_symbol_name_for_private_identifier(
        &self,
        containing_class_symbol: SymbolId,
        description: &str,
    ) -> String {
        let id = crate::ast_shims::get_symbol_id(&self.symbols[containing_class_symbol.0 as usize]);
        format!("{INTERNAL_SYMBOL_NAME_PREFIX}#{id}@{description}")
    }

    // ────────────────────────────────────────────────────────────────────────
    // Declaration dispatchers (binder.go:377-451)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) declareModuleMember(node, symbolFlags, symbolExcludes) *ast.Symbol`.
    fn declare_module_member(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) -> SymbolId {
        let container = self.container;
        let has_export_modifier = get_combined_modifier_flags(&*self.file, node).intersects(ModifierFlags::EXPORT)
            || crate::ast_shims::is_implicitly_exported_jsdoc_declaration(&*self.file, node);
        if symbol_flags.intersects(SymbolFlags::ALIAS) {
            if self.file.node(node).kind == Kind::ExportSpecifier
                || self.file.node(node).kind == Kind::ImportEqualsDeclaration && has_export_modifier
            {
                let symbol = self.container_symbol_expect(container);
                return self.declare_symbol(TableLoc::Exports(symbol), Some(symbol), node, symbol_flags, symbol_excludes);
            }
            return self.declare_symbol(TableLoc::Locals(container), None /*parent*/, node, symbol_flags, symbol_excludes);
        }
        // Exported module members are given 2 symbols: A local symbol that is classified with an ExportValue flag,
        // and an associated export symbol with all the correct flags set on it. There are 2 main reasons:
        //
        //   1. We treat locals and exports of the same name as mutually exclusive within a container.
        //      That means the binder will issue a Duplicate Identifier error if you mix locals and exports
        //      with the same name in the same container.
        //      TODO: Make this a more specific error and decouple it from the exclusion logic.
        //   2. When we checkIdentifier in the checker, we set its resolved symbol to the local symbol,
        //      but return the export symbol (by calling getExportSymbolOfValueSymbolIfExported). That way
        //      when the emitter comes back to it, it knows not to qualify the name if it was found in a containing scope.
        //
        // NOTE: Nested ambient modules always should go to to 'locals' table to prevent their automatic merge
        //       during global merging in the checker. Why? The only case when ambient module is permitted inside another module is module augmentation
        //       and this case is specially handled. Module augmentations should only be merged with original module definition
        //       and should never be merged directly with other augmentation, and the latter case would be possible if automatic merge is allowed.
        if !is_ambient_module(&*self.file, node)
            && (has_export_modifier || self.file.node(container).flags.intersects(NodeFlags::EXPORT_CONTEXT))
        {
            if !node_data::is_locals_container(self.file.node(container))
                || (has_syntactic_modifier(self.file.node(node), ModifierFlags::DEFAULT)
                    && self.get_declaration_name(node) == INTERNAL_SYMBOL_NAME_MISSING)
            {
                let symbol = self.container_symbol_expect(container);
                return self.declare_symbol(TableLoc::Exports(symbol), Some(symbol), node, symbol_flags, symbol_excludes);
                // No local symbol for an unnamed default!
            }
            let export_kind = if symbol_flags.intersects(SymbolFlags::VALUE) {
                SymbolFlags::EXPORT_VALUE
            } else {
                SymbolFlags::NONE
            };
            let local = self.declare_symbol(TableLoc::Locals(container), None /*parent*/, node, export_kind, symbol_excludes);
            let container_symbol = self.container_symbol_expect(container);
            let export_symbol = self.declare_symbol(TableLoc::Exports(container_symbol), Some(container_symbol), node, symbol_flags, symbol_excludes);
            self.symbols[local.0 as usize].export_symbol = Some(export_symbol);
            node_data::set_local_symbol(&mut *self.file, node, Some(local));
            return local;
        }
        self.declare_symbol(TableLoc::Locals(container), None /*parent*/, node, symbol_flags, symbol_excludes)
    }

    /// Go: `func (b *Binder) declareClassMember(node, symbolFlags, symbolExcludes) *ast.Symbol`.
    fn declare_class_member(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) -> SymbolId {
        if crate::ast_shims::is_static(self.file.node(node)) {
            let symbol = self.container_symbol_expect(self.container);
            return self.declare_symbol(TableLoc::Exports(symbol), Some(symbol), node, symbol_flags, symbol_excludes);
        }
        let symbol = self.container_symbol_expect(self.container);
        self.declare_symbol(TableLoc::Members(symbol), Some(symbol), node, symbol_flags, symbol_excludes)
    }

    /// Go: `func (b *Binder) declareSourceFileMember(node, symbolFlags, symbolExcludes) *ast.Symbol`.
    fn declare_source_file_member(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) -> SymbolId {
        if is_external_module(self.file) {
            return self.declare_module_member(node, symbol_flags, symbol_excludes);
        }
        let file_node = self.file.as_node_id();
        self.declare_symbol(TableLoc::Locals(file_node), None /*parent*/, node, symbol_flags, symbol_excludes)
    }

    /// Go: `func (b *Binder) declareSymbolAndAddToSymbolTable(node, symbolFlags, symbolExcludes) *ast.Symbol`.
    fn declare_symbol_and_add_to_symbol_table(
        &mut self,
        node: NodeId,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) -> SymbolId {
        match self.file.node(self.container).kind {
            Kind::ModuleDeclaration => self.declare_module_member(node, symbol_flags, symbol_excludes),
            Kind::SourceFile => self.declare_source_file_member(node, symbol_flags, symbol_excludes),
            Kind::ClassExpression | Kind::ClassDeclaration => {
                self.declare_class_member(node, symbol_flags, symbol_excludes)
            }
            Kind::EnumDeclaration => {
                let symbol = self.container_symbol_expect(self.container);
                self.declare_symbol(TableLoc::Exports(symbol), Some(symbol), node, symbol_flags, symbol_excludes)
            }
            Kind::TypeLiteral
            | Kind::ObjectLiteralExpression
            | Kind::InterfaceDeclaration
            | Kind::JsxAttributes => {
                let symbol = self.container_symbol_expect(self.container);
                self.declare_symbol(TableLoc::Members(symbol), Some(symbol), node, symbol_flags, symbol_excludes)
            }
            Kind::FunctionType
            | Kind::ConstructorType
            | Kind::CallSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::ClassStaticBlockDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::JSTypeAliasDeclaration
            | Kind::MappedType => {
                self.declare_symbol(TableLoc::Locals(self.container), None /*parent*/, node, symbol_flags, symbol_excludes)
            }
            _ => panic!("Unhandled case in declareSymbolAndAddToSymbolTable"),
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Flow node constructors (binder.go:453-574)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) newFlowNode(flags ast.FlowFlags) *ast.FlowNode`.
    fn new_flow_node(&mut self, flags: FlowFlags) -> FlowNodeId {
        self.flow_nodes.push(FlowNode::new(flags));
        FlowNodeId(self.flow_nodes.len() as u32 - 1)
    }

    /// Go: `func (b *Binder) newFlowNodeEx(flags, node, antecedent) *ast.FlowNode` —
    /// the port's `node` is the FlowNodeData payload (Node / SwitchClause /
    /// ReduceLabel / None; see flow.rs).
    fn new_flow_node_ex(&mut self, flags: FlowFlags, node: FlowNodeData, antecedent: Option<FlowNodeId>) -> FlowNodeId {
        let result = self.new_flow_node(flags);
        let flow = self.flow_mut(result);
        flow.node = node;
        flow.antecedent = antecedent;
        result
    }

    /// Go: `func (b *Binder) createLoopLabel() *ast.FlowLabel`.
    fn create_loop_label(&mut self) -> FlowNodeId {
        self.new_flow_node(FlowFlags::LOOP_LABEL)
    }

    /// Go: `func (b *Binder) createBranchLabel() *ast.FlowLabel`.
    fn create_branch_label(&mut self) -> FlowNodeId {
        self.new_flow_node(FlowFlags::BRANCH_LABEL)
    }

    /// Go: `func (b *Binder) createReduceLabel(target, antecedents, antecedent) *ast.FlowNode`.
    fn create_reduce_label(&mut self, target: FlowNodeId, antecedents: Option<FlowListId>, antecedent: FlowNodeId) -> FlowNodeId {
        self.new_flow_node_ex(
            FlowFlags::REDUCE_LABEL,
            FlowNodeData::ReduceLabel(FlowReduceLabelData { target, antecedents }),
            Some(antecedent),
        )
    }

    /// Go: `func (b *Binder) createFlowCondition(flags, antecedent, expression) *ast.FlowNode`.
    fn create_flow_condition(&mut self, flags: FlowFlags, antecedent: FlowNodeId, expression: Option<NodeId>) -> FlowNodeId {
        if self.flow(antecedent).flags.intersects(FlowFlags::UNREACHABLE) {
            return antecedent;
        }
        let Some(expression) = expression else {
            if flags.intersects(FlowFlags::TRUE_CONDITION) {
                return antecedent;
            }
            return self.unreachable_flow;
        };
        let expr_kind = self.file.node(expression).kind;
        let is_nullish = crate::ast_shims::parent_of(&*self.file, expression)
            .is_some_and(|parent| is_nullish_coalesce(&*self.file, parent));
        if (expr_kind == Kind::TrueKeyword && flags.intersects(FlowFlags::FALSE_CONDITION)
            || expr_kind == Kind::FalseKeyword && flags.intersects(FlowFlags::TRUE_CONDITION))
            && !is_expression_of_optional_chain_root(&*self.file, expression)
            && !is_nullish
        {
            return self.unreachable_flow;
        }
        if !is_narrowing_expression(&*self.file, expression) {
            return antecedent;
        }
        set_flow_node_referenced(self.flow_mut(antecedent));
        self.new_flow_node_ex(flags, FlowNodeData::Node(expression), Some(antecedent))
    }

    /// Go: `func (b *Binder) createFlowMutation(flags, antecedent, node) *ast.FlowNode`.
    fn create_flow_mutation(&mut self, flags: FlowFlags, antecedent: FlowNodeId, node: NodeId) -> FlowNodeId {
        set_flow_node_referenced(self.flow_mut(antecedent));
        self.has_flow_effects = true;
        let result = self.new_flow_node_ex(flags, FlowNodeData::Node(node), Some(antecedent));
        if let Some(target) = self.current_exception_target {
            self.add_antecedent(target, result);
        }
        result
    }

    /// Go: `func (b *Binder) createFlowSwitchClause(antecedent, switchStatement, clauseStart, clauseEnd) *ast.FlowNode`.
    fn create_flow_switch_clause(
        &mut self,
        antecedent: FlowNodeId,
        switch_statement: NodeId,
        clause_start: i32,
        clause_end: i32,
    ) -> FlowNodeId {
        set_flow_node_referenced(self.flow_mut(antecedent));
        self.new_flow_node_ex(
            FlowFlags::SWITCH_CLAUSE,
            FlowNodeData::SwitchClause(FlowSwitchClauseData {
                switch_statement,
                clause_start,
                clause_end,
            }),
            Some(antecedent),
        )
    }

    /// Go: `func (b *Binder) createFlowCall(antecedent, node) *ast.FlowNode`.
    fn create_flow_call(&mut self, antecedent: FlowNodeId, node: NodeId) -> FlowNodeId {
        set_flow_node_referenced(self.flow_mut(antecedent));
        self.has_flow_effects = true;
        self.new_flow_node_ex(FlowFlags::CALL, FlowNodeData::Node(node), Some(antecedent))
    }

    /// Go: `func (b *Binder) newFlowList(head, tail) *ast.FlowList`.
    fn new_flow_list(&mut self, head: FlowNodeId, tail: Option<FlowListId>) -> FlowListId {
        self.flow_lists.push(FlowList { flow: head, next: tail });
        FlowListId(self.flow_lists.len() as u32 - 1)
    }

    /// Go: `func (b *Binder) combineFlowLists(head, tail) *ast.FlowList`.
    fn combine_flow_lists(&mut self, head: Option<FlowListId>, tail: Option<FlowListId>) -> Option<FlowListId> {
        let Some(head) = head else {
            return tail;
        };
        let (flow, next) = {
            let cell = &self.flow_lists[head.0 as usize];
            (cell.flow, cell.next)
        };
        let combined_tail = self.combine_flow_lists(next, tail);
        Some(self.new_flow_list(flow, combined_tail))
    }

    /// Go: `func (b *Binder) addAntecedent(label *ast.FlowLabel, antecedent *ast.FlowNode)`
    /// — the label is a FlowNodeId (Go nil labels crash; call sites guard).
    fn add_antecedent(&mut self, label: FlowNodeId, antecedent: FlowNodeId) {
        if self.flow(antecedent).flags.intersects(FlowFlags::UNREACHABLE) {
            return;
        }
        // If antecedent isn't already on the Antecedents list, add it to the end of the list
        let mut last: Option<FlowListId> = None;
        let mut list = self.flow(label).antecedents;
        while let Some(cell) = list {
            if self.flow_lists[cell.0 as usize].flow == antecedent {
                return;
            }
            last = Some(cell);
            list = self.flow_lists[cell.0 as usize].next;
        }
        match last {
            None => {
                let cell = self.new_flow_list(antecedent, None);
                self.flow_mut(label).antecedents = Some(cell);
            }
            Some(last) => {
                let cell = self.new_flow_list(antecedent, None);
                self.flow_lists[last.0 as usize].next = Some(cell);
            }
        }
        set_flow_node_referenced(self.flow_mut(antecedent));
    }

    /// Go: `func (b *Binder) finishFlowLabel(label *ast.FlowLabel) *ast.FlowNode`.
    fn finish_flow_label(&self, label: FlowNodeId) -> FlowNodeId {
        match self.flow(label).antecedents {
            None => self.unreachable_flow,
            Some(first) => match self.flow_lists[first.0 as usize].next {
                None => self.flow_lists[first.0 as usize].flow,
                Some(_) => label,
            },
        }
    }
}

/// Go: `func setFlowNodeReferenced(flow *ast.FlowNode)`.
fn set_flow_node_referenced(flow: &mut FlowNode) {
    // On first reference we set the Referenced flag, thereafter we set the Shared flag
    if !flow.flags.intersects(FlowFlags::REFERENCED) {
        flow.flags |= FlowFlags::REFERENCED;
    } else {
        flow.flags |= FlowFlags::SHARED;
    }
}

/// Go: `func ModuleExportNameIsDefault(node *ast.Node) bool` call — the
/// binder.go:153 use (`ast.ModuleExportNameIsDefault`) is the ast_shims
/// helper; re-bound here to keep the Go call shape visible at use sites.
fn module_export_name_is_default(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::ast_shims::module_export_name_is_default(store, node)
}

// ════════════════════════════════════════════════════════════════════════════
// bind — the node dispatch (binder.go:576-749)
// ════════════════════════════════════════════════════════════════════════════

impl<'f> Binder<'f> {
    /// Go: `func (b *Binder) bind(node *ast.Node) bool` — the node argument
    /// is Option<NodeId> (Go's nil node returns false immediately; terminal
    /// fields bind as nil tokens).
    fn bind(&mut self, node: Option<NodeId>) -> bool {
        let Some(node) = node else {
            return false;
        };
        // Even though in the AST the jsdoc @typedef node belongs to the current node,
        // its symbol might be in the same scope with the current node's symbol. Consider:
        //
        //     /** @typedef {string | number} MyType */
        //     function foo();
        //
        // Here the current node is "foo", which is a container, but the scope of "MyType" should
        // not be inside "foo". Therefore we always bind @typedef before bind the parent node,
        // and skip binding this tag later when binding all the other jsdoc tags.

        // First we bind declaration nodes to a symbol if possible. We'll both create a symbol
        // and then potentially add the symbol to an appropriate symbol table. Possible
        // destination symbol tables are:
        //
        //  1) The 'exports' table of the current container's symbol.
        //  2) The 'members' table of the current container's symbol.
        //  3) The 'locals' table of the current container.
        //
        // However, not all symbols will end up in any of these tables. 'Anonymous' symbols
        // (like TypeLiterals for example) will not be put in any table.
        let n_kind = self.file.node(node).kind;
        match n_kind {
            Kind::Identifier => {
                self.set_flow_node_field(node, self.current_flow);
                self.check_contextual_identifier(node);
            }
            Kind::ThisKeyword | Kind::SuperKeyword => {
                if n_kind == Kind::ThisKeyword {
                    self.seen_this_keyword = true;
                }
                self.set_flow_node_field(node, self.current_flow);
            }
            Kind::QualifiedName => {
                if !self.current_flow.is_none() && is_part_of_type_query(&*self.file, node) {
                    self.set_flow_node_field(node, self.current_flow);
                }
            }
            Kind::MetaProperty => {
                self.set_flow_node_field(node, self.current_flow);
            }
            Kind::PrivateIdentifier => {
                self.check_private_identifier(node);
            }
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                if !self.current_flow.is_none() && is_narrowable_reference(&*self.file, node) {
                    self.set_flow_node_field(node, self.current_flow);
                }
            }
            Kind::BinaryExpression => {
                match get_assignment_declaration_kind(&*self.file, node) {
                    JSDeclarationKind::ModuleExports => self.bind_module_exports_assignment(node),
                    JSDeclarationKind::ExportsProperty => self.bind_exports_or_object_define_property(node),
                    JSDeclarationKind::Property => self.bind_expando_property_assignment(node),
                    JSDeclarationKind::ThisProperty => self.bind_this_property_assignment(node),
                    _ => {}
                }
                self.check_strict_mode_binary_expression(node);
            }
            Kind::CatchClause => self.check_strict_mode_catch_clause(node),
            Kind::DeleteExpression => self.check_strict_mode_delete_expression(node),
            Kind::PostfixUnaryExpression => self.check_strict_mode_postfix_unary_expression(node),
            Kind::PrefixUnaryExpression => self.check_strict_mode_prefix_unary_expression(node),
            Kind::WithStatement => self.check_strict_mode_with_statement(node),
            Kind::LabeledStatement => self.check_strict_mode_labeled_statement(node),
            Kind::ThisType => {
                self.seen_this_keyword = true;
            }
            Kind::TypeParameter => self.bind_type_parameter(node),
            Kind::Parameter => self.bind_parameter(node),
            Kind::VariableDeclaration => self.bind_variable_declaration_or_binding_element(node),
            Kind::BindingElement => {
                self.set_flow_node_field(node, self.current_flow);
                self.bind_variable_declaration_or_binding_element(node);
            }
            Kind::PropertyDeclaration | Kind::PropertySignature => self.bind_property_worker(node),
            Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment => {
                self.bind_property_or_method_or_accessor(node, SymbolFlags::PROPERTY, SymbolFlags::PROPERTY_EXCLUDES);
            }
            Kind::EnumMember => {
                self.bind_property_or_method_or_accessor(node, SymbolFlags::ENUM_MEMBER, SymbolFlags::ENUM_MEMBER_EXCLUDES);
            }
            Kind::CallSignature | Kind::ConstructSignature | Kind::IndexSignature => {
                self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::SIGNATURE, SymbolFlags::NONE);
            }
            Kind::MethodDeclaration | Kind::MethodSignature => {
                let includes = SymbolFlags::METHOD.union(get_optional_symbol_flag_for_node(self, node));
                let excludes = if is_object_literal_method(&*self.file, node) {
                    SymbolFlags::VALUE
                } else {
                    SymbolFlags::METHOD_EXCLUDES
                };
                self.bind_property_or_method_or_accessor(node, includes, excludes);
            }
            Kind::FunctionDeclaration => self.bind_function_declaration(node),
            Kind::Constructor => {
                self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::CONSTRUCTOR, SymbolFlags::NONE);
            }
            Kind::GetAccessor => {
                self.bind_property_or_method_or_accessor(node, SymbolFlags::GET_ACCESSOR, SymbolFlags::GET_ACCESSOR_EXCLUDES);
            }
            Kind::SetAccessor => {
                self.bind_property_or_method_or_accessor(node, SymbolFlags::SET_ACCESSOR, SymbolFlags::SET_ACCESSOR_EXCLUDES);
            }
            Kind::FunctionType | Kind::ConstructorType => self.bind_function_or_constructor_type(node),
            Kind::TypeLiteral | Kind::MappedType => {
                self.bind_anonymous_declaration(node, SymbolFlags::TYPE_LITERAL, INTERNAL_SYMBOL_NAME_TYPE);
            }
            Kind::ObjectLiteralExpression => {
                self.bind_anonymous_declaration(node, SymbolFlags::OBJECT_LITERAL, INTERNAL_SYMBOL_NAME_OBJECT);
            }
            Kind::FunctionExpression | Kind::ArrowFunction => self.bind_function_expression(node),
            Kind::ClassExpression | Kind::ClassDeclaration => self.bind_class_like_declaration(node),
            Kind::InterfaceDeclaration => {
                self.bind_block_scoped_declaration(node, SymbolFlags::INTERFACE, SymbolFlags::INTERFACE_EXCLUDES);
            }
            Kind::CallExpression => {
                match get_assignment_declaration_kind(&*self.file, node) {
                    JSDeclarationKind::ObjectDefinePropertyValue => self.bind_expando_property_assignment(node),
                    JSDeclarationKind::ObjectDefinePropertyExports => {
                        self.bind_exports_or_object_define_property(node)
                    }
                    _ => {}
                }
                if tsc_ast::utilities::is_in_js_file(self.file.node(node)) {
                    self.bind_call_expression(node);
                }
            }
            Kind::TypeAliasDeclaration => {
                self.bind_block_scoped_declaration(node, SymbolFlags::TYPE_ALIAS, SymbolFlags::TYPE_ALIAS_EXCLUDES);
            }
            Kind::JSTypeAliasDeclaration => {
                // Top-level JSTypeAliasDeclaration nodes are processed in bindContainer
                if !tsc_ast::is_source_file(self.file.node(self.block_scope_container)) {
                    self.bind_block_scoped_declaration(node, SymbolFlags::TYPE_ALIAS, SymbolFlags::TYPE_ALIAS_EXCLUDES);
                }
            }
            Kind::EnumDeclaration => self.bind_enum_declaration(node),
            Kind::ModuleDeclaration => self.bind_module_declaration(node),
            Kind::ImportEqualsDeclaration | Kind::NamespaceImport | Kind::ImportSpecifier | Kind::ExportSpecifier => {
                self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::ALIAS, SymbolFlags::ALIAS_EXCLUDES);
            }
            Kind::NamespaceExportDeclaration => self.bind_namespace_export_declaration(node),
            Kind::ImportClause => self.bind_import_clause(node),
            Kind::ExportDeclaration => self.bind_export_declaration(node),
            Kind::ExportAssignment => self.bind_export_assignment(node),
            Kind::SourceFile => self.bind_source_file_if_external_module(),
            Kind::JsxAttributes => self.bind_jsx_attributes(node),
            Kind::JsxAttribute => self.bind_jsx_attribute(node, SymbolFlags::PROPERTY, SymbolFlags::PROPERTY_EXCLUDES),
            _ => {}
        }
        // Then we recurse into the children of the node to bind them as well. For certain
        // symbols we do specialized work when we recurse. For example, we'll keep track of
        // the current 'container' node when it changes. This helps us know which symbol table
        // a local should go into for example. Since terminal nodes are known not to have
        // children, as an optimization we don't process those.
        let mut this_node_or_any_subnodes_has_error =
            self.file.node(node).flags.intersects(NodeFlags::THIS_NODE_HAS_ERROR);
        if self.file.node(node).kind > Kind::LastToken {
            let save_seen_parse_error = self.seen_parse_error;
            self.seen_parse_error = false;
            let container_flags = get_container_flags(&*self.file, node);
            if container_flags == ContainerFlags::NONE {
                self.bind_children(node);
            } else {
                self.bind_container(node, container_flags);
            }
            if self.seen_parse_error {
                this_node_or_any_subnodes_has_error = true;
            }
            self.seen_parse_error = save_seen_parse_error;
        }
        if this_node_or_any_subnodes_has_error {
            self.file.node_mut(node).flags |= NodeFlags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR;
            self.seen_parse_error = true;
        }
        false
    }

    /// Go: `func setFlowNode(node *ast.Node, flowNode *ast.FlowNode)` — writes
    /// through FlowNodeData() when the payload carries the slot (the NONE
    /// sentinel stands in for Go's nil flow node).
    fn set_flow_node_field(&mut self, node: NodeId, flow_node: FlowNodeId) {
        node_data::set_flow_node(&mut *self.file, node, (!flow_node.is_none()).then_some(flow_node));
    }

    // ────────────────────────────────────────────────────────────────────────
    // Declaration handlers (binder.go:751-1271)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) bindPropertyWorker(node *ast.Node)`.
    fn bind_property_worker(&mut self, node: NodeId) {
        let is_auto_accessor = crate::ast_shims::is_auto_accessor_property_declaration(self.file.node(node));
        let (includes, excludes) = if is_auto_accessor {
            (SymbolFlags::ACCESSOR, SymbolFlags::ACCESSOR_EXCLUDES)
        } else {
            (SymbolFlags::PROPERTY, SymbolFlags::PROPERTY_EXCLUDES)
        };
        let includes = includes.union(get_optional_symbol_flag_for_node(self, node));
        self.bind_property_or_method_or_accessor(node, includes, excludes);
    }

    /// Go: `func (b *Binder) bindSourceFileIfExternalModule()`.
    fn bind_source_file_if_external_module(&mut self) {
        let file_node = self.file.as_node_id();
        self.set_export_context_flag(file_node);
        if is_external_or_common_js_module(self.file) {
            self.bind_source_file_as_external_module();
        } else if is_json_source_file(self.file) {
            self.bind_source_file_as_external_module();
            // Create symbol equivalent for the module.exports = {}
            let original_symbol = self.file.payload().symbol;
            let file_symbol = original_symbol.expect("JSON file module symbol");
            self.declare_symbol(
                TableLoc::Exports(file_symbol),
                original_symbol,
                file_node,
                SymbolFlags::PROPERTY,
                SymbolFlags::ALL,
            );
            // addDeclarationToSymbol overwrote the file node's symbol slot;
            // restore it (Go: `b.file.Symbol = originalSymbol`).
            self.file.payload_mut().symbol = original_symbol;
        }
    }

    /// Go: `func (b *Binder) bindSourceFileAsExternalModule()`.
    fn bind_source_file_as_external_module(&mut self) {
        let name = format!(
            "\"{}\"",
            remove_file_extension(self.file.file_name().as_string())
        );
        let file_node = self.file.as_node_id();
        self.bind_anonymous_declaration(file_node, SymbolFlags::VALUE_MODULE, &name);
    }

    /// Go: `func (b *Binder) bindModuleDeclaration(node *ast.Node)`.
    fn bind_module_declaration(&mut self, node: NodeId) {
        self.set_export_context_flag(node);
        if is_ambient_module(&*self.file, node) {
            if has_syntactic_modifier(self.file.node(node), ModifierFlags::EXPORT) {
                self.error_on_first_token(
                    node,
                    &tsc_diagnostics::X_EXPORT_MODIFIER_CANNOT_BE_APPLIED_TO_AMBIENT_MODULES_AND_MODULE_AUGMENTATIONS_SINCE_THEY_ARE_ALWAYS_VISIBLE,
                    &[],
                );
            }
            if is_module_augmentation_external(&*self.file, node) {
                self.declare_module_symbol(node);
            } else {
                let name = match &self.file.node(node).data {
                    tsc_ast::NodeData::ModuleDeclaration(d) => d.name,
                    _ => unreachable!("IsAmbientModule implies ModuleDeclaration data"),
                };
                let symbol = self.declare_symbol_and_add_to_symbol_table(
                    node,
                    SymbolFlags::VALUE_MODULE,
                    SymbolFlags::VALUE_MODULE_EXCLUDES,
                );

                if tsc_ast::is_string_literal(self.file.node(name)) {
                    let attributes = match &self.file.node(node).data {
                        tsc_ast::NodeData::ModuleDeclaration(d) => d.attributes,
                        _ => None,
                    };
                    let pattern = try_parse_pattern(node_text(&*self.file, name));
                    if !pattern.is_valid() {
                        // An invalid pattern - must have multiple wildcards.
                        self.error_on_first_token(
                            name,
                            &tsc_diagnostics::PATTERN_0_CAN_HAVE_AT_MOST_ONE_ASTERISK_CHARACTER,
                            &[node_text(&*self.file, name).to_string()],
                        );
                    } else if pattern.star_index >= 0 {
                        self.file.payload_mut().pattern_ambient_modules.push(PatternAmbientModule {
                            pattern,
                            symbol: Some(symbol),
                        });
                    } else if attributes.is_some() {
                        self.error_on_node(
                            name,
                            &tsc_diagnostics::AN_AMBIENT_MODULE_DECLARATION_WITH_IMPORT_ATTRIBUTES_MUST_USE_A_PATTERN_NAME_WITH_AN_ASTERISK_CHARACTER,
                            &[],
                        );
                    }
                }
            }
        } else {
            let state = self.declare_module_symbol(node);
            if state != ModuleInstanceState::NonInstantiated {
                let symbol = self.container_symbol_expect(node);
                // if module was already merged with some function, class or non-const enum, treat it as non-const-enum-only
                let const_enum_only_module = !self.symbols[symbol.0 as usize]
                    .flags
                    .intersects(SymbolFlags::FUNCTION.union(SymbolFlags::CLASS).union(SymbolFlags::REGULAR_ENUM))
                    // Current must be `const enum` only
                    && state == ModuleInstanceState::ConstEnumOnly
                    // Can't have been set to 'false' in a previous merged symbol. ('undefined' OK)
                    && !self.not_const_enum_only_modules.has(&symbol);
                if const_enum_only_module {
                    self.symbols[symbol.0 as usize].flags |= SymbolFlags::CONST_ENUM_ONLY_MODULE;
                } else {
                    self.symbols[symbol.0 as usize].flags =
                        SymbolFlags(self.symbols[symbol.0 as usize].flags.0 & !SymbolFlags::CONST_ENUM_ONLY_MODULE.0);
                    self.not_const_enum_only_modules.add(symbol);
                }
            }
        }
    }

    /// Go: `func (b *Binder) declareModuleSymbol(node *ast.Node) ast.ModuleInstanceState`.
    fn declare_module_symbol(&mut self, node: NodeId) -> ModuleInstanceState {
        let state = get_module_instance_state(&*self.file, node);
        let instantiated = state != ModuleInstanceState::NonInstantiated;
        let (includes, excludes) = if instantiated {
            (SymbolFlags::VALUE_MODULE, SymbolFlags::VALUE_MODULE_EXCLUDES)
        } else {
            (SymbolFlags::NAMESPACE_MODULE, SymbolFlags::NAMESPACE_MODULE_EXCLUDES)
        };
        self.declare_symbol_and_add_to_symbol_table(node, includes, excludes);
        state
    }

    /// Go: `func (b *Binder) bindNamespaceExportDeclaration(node *ast.Node)`.
    fn bind_namespace_export_declaration(&mut self, node: NodeId) {
        if self.file.node(node).modifiers().is_some() {
            self.error_on_node(node, &tsc_diagnostics::MODIFIERS_CANNOT_APPEAR_HERE, &[]);
        }
        let parent = crate::ast_shims::parent_of(&*self.file, node).expect("NamespaceExportDeclaration parent");
        if !tsc_ast::is_source_file(self.file.node(parent)) {
            self.error_on_node(node, &tsc_diagnostics::GLOBAL_MODULE_EXPORTS_MAY_ONLY_APPEAR_AT_TOP_LEVEL, &[]);
        } else if !is_external_module(self.file) {
            self.error_on_node(node, &tsc_diagnostics::GLOBAL_MODULE_EXPORTS_MAY_ONLY_APPEAR_IN_MODULE_FILES, &[]);
        } else if !self.file.payload().is_declaration_file {
            self.error_on_node(node, &tsc_diagnostics::GLOBAL_MODULE_EXPORTS_MAY_ONLY_APPEAR_IN_DECLARATION_FILES, &[]);
        } else {
            let file_symbol = self.file.payload().symbol;
            self.declare_symbol(
                TableLoc::GlobalExports,
                file_symbol,
                node,
                SymbolFlags::ALIAS,
                SymbolFlags::ALIAS_EXCLUDES,
            );
        }
    }

    /// Go: `func (b *Binder) bindImportClause(node *ast.Node)`.
    fn bind_import_clause(&mut self, node: NodeId) {
        let has_name = match &self.file.node(node).data {
            tsc_ast::NodeData::ImportClause(d) => d.name.is_some(),
            _ => false,
        };
        if has_name {
            self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::ALIAS, SymbolFlags::ALIAS_EXCLUDES);
        }
    }

    /// Go: `func (b *Binder) bindExportDeclaration(node *ast.Node)`.
    fn bind_export_declaration(&mut self, node: NodeId) {
        let export_clause = match &self.file.node(node).data {
            tsc_ast::NodeData::ExportDeclaration(d) => d.export_clause,
            _ => unreachable!("bindExportDeclaration on KindExportDeclaration"),
        };
        if self.container_symbol(self.container).is_none() {
            // Export * in some sort of block construct
            let name = self.get_declaration_name(node);
            self.bind_anonymous_declaration(node, SymbolFlags::EXPORT_STAR, &name);
        } else if export_clause.is_none() {
            // All export * declarations are collected in an __export symbol
            let container_symbol = self.container_symbol_expect(self.container);
            self.declare_symbol(
                TableLoc::Exports(container_symbol),
                Some(container_symbol),
                node,
                SymbolFlags::EXPORT_STAR,
                SymbolFlags::NONE,
            );
        } else if is_namespace_export(self.file.node(export_clause.unwrap())) {
            let container_symbol = self.container_symbol_expect(self.container);
            self.declare_symbol(
                TableLoc::Exports(container_symbol),
                Some(container_symbol),
                export_clause.unwrap(),
                SymbolFlags::ALIAS,
                SymbolFlags::ALIAS_EXCLUDES,
            );
        }
    }

    /// Go: `func (b *Binder) bindExportAssignment(node *ast.Node)`.
    fn bind_export_assignment(&mut self, node: NodeId) {
        let container = self.container;
        if self.container_symbol(container).is_none() && is_export_assignment(self.file.node(node)) {
            // Incorrect export assignment in some sort of block construct
            let name = self.get_declaration_name(node);
            self.bind_anonymous_declaration(node, SymbolFlags::VALUE, &name);
        } else {
            // If there is an `export default x;` alias declaration, can't `export default` anything else.
            // (In contrast, you can still have `export default function f() {}` and `export default interface I {}`.)
            let expression = node_data::expression_of(&*self.file, node).expect("ExportAssignment expression");
            let flags = if crate::ast_shims::expression_is_alias(&*self.file, expression) {
                SymbolFlags::ALIAS
            } else {
                SymbolFlags::PROPERTY
            };
            let container_symbol = self.container_symbol_expect(container);
            let symbol = self.declare_symbol(
                TableLoc::Exports(container_symbol),
                Some(container_symbol),
                node,
                flags,
                SymbolFlags::ALL,
            );
            let is_export_equals = match &self.file.node(node).data {
                tsc_ast::NodeData::ExportAssignment(d) => d.is_export_equals,
                _ => false,
            };
            if is_export_equals {
                // Ensure export assignments have a ValueDeclaration set.
                set_value_declaration(self, symbol, node);
            }
        }
    }

    /// Go: `func (b *Binder) bindJsxAttribute(node, symbolFlags, symbolExcludes)`.
    fn bind_jsx_attribute(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) {
        self.declare_symbol_and_add_to_symbol_table(node, symbol_flags, symbol_excludes);
    }

    /// Go: `func (b *Binder) bindJsxAttributes(node *ast.Node)`.
    fn bind_jsx_attributes(&mut self, node: NodeId) {
        self.bind_anonymous_declaration(node, SymbolFlags::OBJECT_LITERAL, INTERNAL_SYMBOL_NAME_JSX_ATTRIBUTES);
    }

    /// Go: `func (b *Binder) setExportContextFlag(node *ast.Node)`.
    fn set_export_context_flag(&mut self, node: NodeId) {
        // A declaration source file or ambient module declaration that contains no export declarations (but possibly regular
        // declarations with export modifiers) is an export context in which declarations are implicitly exported.
        let has_no_export_declarations = !self.has_export_declarations(node);
        let n = self.file.node_mut(node);
        if n.flags.intersects(NodeFlags::AMBIENT) && has_no_export_declarations {
            n.flags |= NodeFlags::EXPORT_CONTEXT;
        } else {
            n.flags = NodeFlags(n.flags.0 & !NodeFlags::EXPORT_CONTEXT.0);
        }
    }

    /// Go: `func (b *Binder) hasExportDeclarations(node *ast.Node) bool`.
    fn has_export_declarations(&self, node: NodeId) -> bool {
        let kind = self.file.node(node).kind;
        let statements: &[NodeId] = match kind {
            Kind::SourceFile => {
                let file_node = self.file.as_node_id();
                node_data::statements_of(&*self.file, file_node)
                    .map(|l| l.nodes.as_ref())
                    .unwrap_or(&[])
            }
            Kind::ModuleDeclaration => {
                let Some(body) = node_data::body_of(&*self.file, node) else {
                    return false;
                };
                if tsc_ast::is_module_block(self.file.node(body)) {
                    node_data::statements_of(&*self.file, body)
                        .map(|l| l.nodes.as_ref())
                        .unwrap_or(&[])
                } else {
                    return false;
                }
            }
            _ => return false,
        };
        statements.iter().any(|&s| {
            tsc_ast::is_export_declaration(self.file.node(s)) || is_export_assignment(self.file.node(s))
        })
    }

    /// Go: `func (b *Binder) bindFunctionExpression(node *ast.Node)`.
    fn bind_function_expression(&mut self, node: NodeId) {
        let is_async = !self.file.payload().is_declaration_file
            && !self.file.node(node).flags.intersects(NodeFlags::AMBIENT)
            && crate::ast_shims::is_async_function(&*self.file, node);
        if is_async {
            self.emit_flags |= NodeFlags::HAS_ASYNC_FUNCTIONS;
        }
        self.set_flow_node_field(node, self.current_flow);
        let mut binding_name = INTERNAL_SYMBOL_NAME_FUNCTION.to_string();
        if is_function_expression(self.file.node(node)) {
            let name = match &self.file.node(node).data {
                tsc_ast::NodeData::FunctionExpression(d) => d.name,
                _ => None,
            };
            if let Some(name) = name {
                self.check_strict_mode_function_name(node);
                binding_name = node_text(&*self.file, name).to_string();
            }
        }
        self.bind_anonymous_declaration(node, SymbolFlags::FUNCTION, &binding_name);
    }

    /// Go: `func (b *Binder) bindCallExpression(node *ast.Node)`.
    fn bind_call_expression(&mut self, node: NodeId) {
        // We're only inspecting call expressions to detect CommonJS modules, so we can skip
        // this check if we've already seen the module indicator
        if self.file.payload().common_js_module_indicator.get().is_none()
            && crate::ast_shims::is_require_call(&*self.file, node, false /*requireStringLiteralLikeArgument*/)
        {
            self.set_common_js_module_indicator(node);
        }
    }

    /// Go: `func (b *Binder) setCommonJSModuleIndicator(node *ast.Node) bool`.
    fn set_common_js_module_indicator(&mut self, node: NodeId) -> bool {
        let file_node = self.file.as_node_id();
        let indicator = self.file.payload().external_module_indicator.get();
        if indicator.is_some() && indicator != Some(file_node) {
            return false;
        }
        if self.file.payload().common_js_module_indicator.get().is_none() {
            self.file.payload_mut().common_js_module_indicator.set(Some(node));
            if self.file.payload().external_module_indicator.get().is_none() {
                self.bind_source_file_as_external_module();
            }
        }
        true
    }

    /// Go: `func (b *Binder) bindClassLikeDeclaration(node *ast.Node)`.
    fn bind_class_like_declaration(&mut self, node: NodeId) {
        let name = self.file.node(node).name();
        match self.file.node(node).kind {
            Kind::ClassDeclaration => {
                self.bind_block_scoped_declaration(node, SymbolFlags::CLASS, SymbolFlags::CLASS_EXCLUDES);
            }
            Kind::ClassExpression => {
                let name_text = match name {
                    Some(name) => node_text(&*self.file, name).to_string(),
                    None => INTERNAL_SYMBOL_NAME_CLASS.to_string(),
                };
                self.bind_anonymous_declaration(node, SymbolFlags::CLASS, &name_text);
            }
            _ => {}
        }
        let symbol = self.container_symbol_expect(node);
        // TypeScript 1.0 spec (April 2014): 8.4
        // Every class automatically contains a static property member named 'prototype', the
        // type of which is an instantiation of the class type with type Any supplied as a type
        // argument for each type parameter. It is an error to explicitly declare a static
        // property member with the name 'prototype'.
        //
        // Note: we check for this here because this class may be merging into a module.  The
        // module might have an exported variable called 'prototype'.  We can't allow that as
        // that would clash with the built-in 'prototype' for the class.
        let prototype_symbol = self.new_symbol(
            SymbolFlags::PROPERTY.union(SymbolFlags::PROTOTYPE),
            "prototype",
        );
        let symbol_export = self.symbols[symbol.0 as usize].exports.get("prototype").copied();
        if let Some(symbol_export) = symbol_export {
            let declaration = self.symbols[symbol_export.0 as usize].declarations[0];
            self.error_on_node(
                declaration,
                &tsc_diagnostics::DUPLICATE_IDENTIFIER_0,
                &[self.symbols[prototype_symbol.0 as usize].name.clone()],
            );
        }
        self.symbols[symbol.0 as usize].exports.set("prototype".to_string(), prototype_symbol);
        self.symbols[prototype_symbol.0 as usize].parent = Some(symbol);
    }

    /// Go: `func (b *Binder) bindPropertyOrMethodOrAccessor(node, symbolFlags, symbolExcludes)`.
    fn bind_property_or_method_or_accessor(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) {
        let is_async = !self.file.payload().is_declaration_file
            && !self.file.node(node).flags.intersects(NodeFlags::AMBIENT)
            && crate::ast_shims::is_async_function(&*self.file, node);
        if is_async {
            self.emit_flags |= NodeFlags::HAS_ASYNC_FUNCTIONS;
        }
        if !self.current_flow.is_none()
            && is_object_literal_or_class_expression_method_or_accessor(&*self.file, node)
        {
            self.set_flow_node_field(node, self.current_flow);
        }
        if has_dynamic_name(&*self.file, node) {
            self.bind_anonymous_declaration(node, symbol_flags, INTERNAL_SYMBOL_NAME_COMPUTED);
        } else {
            self.declare_symbol_and_add_to_symbol_table(node, symbol_flags, symbol_excludes);
        }
    }

    /// Go: `func (b *Binder) bindFunctionOrConstructorType(node *ast.Node)`.
    fn bind_function_or_constructor_type(&mut self, node: NodeId) {
        // For a given function symbol "<...>(...) => T" we want to generate a symbol identical
        // to the one we would get for: { <...>(...): T }
        //
        // We do that by making an anonymous type literal symbol, and then setting the function
        // symbol as its sole member. To the rest of the system, this symbol will be indistinguishable
        // from an actual type literal symbol you would have gotten had you used the long form.
        let name = self.get_declaration_name(node);
        let symbol = self.new_symbol(SymbolFlags::SIGNATURE, &name);
        self.add_declaration_to_symbol(symbol, node, SymbolFlags::SIGNATURE);
        let type_literal_symbol = self.new_symbol(SymbolFlags::TYPE_LITERAL, INTERNAL_SYMBOL_NAME_TYPE);
        self.add_declaration_to_symbol(type_literal_symbol, node, SymbolFlags::TYPE_LITERAL);
        let symbol_name = self.symbols[symbol.0 as usize].name.clone();
        self.symbols[type_literal_symbol.0 as usize].members.set(symbol_name, symbol);
    }

    /// Go: `func (b *Binder) addLateBoundAssignmentDeclarationToSymbol(node, symbol)`.
    fn add_late_bound_assignment_declaration_to_symbol(&mut self, node: NodeId, symbol: SymbolId) {
        let name = INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION;
        let assignment_symbol = match self.symbols[symbol.0 as usize].exports.get(name) {
            Some(&s) => s,
            None => {
                let fresh = self.new_symbol(SymbolFlags::NONE, name);
                self.symbols[symbol.0 as usize].exports.set(name.to_string(), fresh);
                fresh
            }
        };
        self.symbols[assignment_symbol.0 as usize].declarations.push(node);
    }

    /// Go: `func (b *Binder) bindModuleExportsAssignment(node *ast.Node)`.
    fn bind_module_exports_assignment(&mut self, node: NodeId) {
        if self.set_common_js_module_indicator(node) {
            let file_node = self.file.as_node_id();
            let right = self
                .file
                .node(node)
                .as_binary_expression()
                .expect("BinaryExpression data")
                .right;
            let flags = if crate::ast_shims::expression_is_alias(&*self.file, right) {
                SymbolFlags::ALIAS
            } else {
                SymbolFlags::PROPERTY
            };
            let file_symbol = self.container_symbol_expect(file_node);
            let symbol = self.declare_symbol(
                TableLoc::Exports(file_symbol),
                Some(file_symbol),
                node,
                flags,
                SymbolFlags::NONE,
            );
            set_value_declaration(self, symbol, node);
        }
    }

    /// Go: `func (b *Binder) bindExpandoPropertyAssignment(node *ast.Node)`.
    fn bind_expando_property_assignment(&mut self, node: NodeId) {
        self.expando_assignments.push(ExpandoAssignmentInfo {
            node,
            container: self.container,
            block_scope_container: self.block_scope_container,
        });
    }

    /// Go: `func (b *Binder) bindDeferredExpandoAssignments()`.
    fn bind_deferred_expando_assignments(&mut self) {
        for info in std::mem::take(&mut self.expando_assignments) {
            self.container = info.container;
            self.block_scope_container = info.block_scope_container;
            self.bind_deferred_expando_assignment(info.node);
        }
    }

    /// Go: `func (b *Binder) bindCommonJSTypeExports(moduleSymbol *ast.Symbol)` —
    /// if the given module symbol has an export= symbol, promote exports with
    /// a type or namespace meaning from the module symbol onto the export=
    /// symbol and, if any such exports exist, mark the export= symbol as a
    /// namespace module.
    fn bind_common_js_type_exports(&mut self, module_symbol: SymbolId) {
        let export_equals = self.symbols[module_symbol.0 as usize]
            .exports
            .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
            .copied();
        let Some(export_equals) = export_equals else {
            return;
        };
        let entries: Vec<(String, SymbolId)> = self.symbols[module_symbol.0 as usize]
            .exports
            .entries()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        for (name, symbol) in entries {
            if name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                && self.symbols[symbol.0 as usize]
                    .flags
                    .intersects(SymbolFlags::TYPE.union(SymbolFlags::NAMESPACE))
            {
                self.symbols[export_equals.0 as usize].exports.set(name, symbol);
                self.symbols[export_equals.0 as usize].flags |= SymbolFlags::NAMESPACE_MODULE;
            }
        }
    }

    /// Go: `func (b *Binder) bindDeferredExpandoAssignment(node *ast.Node)`.
    fn bind_deferred_expando_assignment(&mut self, node: NodeId) {
        let parent = get_parent_of_property_assignment(&*self.file, node);
        let mut symbol = self.lookup_entity(parent, self.block_scope_container);
        if symbol.is_none() {
            symbol = self.lookup_entity(parent, self.container);
        }
        let Some(symbol) = get_initializer_symbol(self, symbol) else {
            return;
        };
        if has_dynamic_name(&*self.file, node) {
            self.bind_anonymous_declaration(
                node,
                SymbolFlags::PROPERTY.union(SymbolFlags::ASSIGNMENT),
                INTERNAL_SYMBOL_NAME_COMPUTED,
            );
            self.add_late_bound_assignment_declaration_to_symbol(node, symbol);
        } else {
            // We declare expandos only when there are no non-expando declarations for that name.
            let name = self.get_declaration_name(node);
            let existing = self.symbols[symbol.0 as usize].exports.get(&name).copied();
            if existing.is_none()
                || existing.is_some_and(|s| self.symbols[s.0 as usize].flags.intersects(SymbolFlags::ASSIGNMENT))
            {
                self.declare_symbol(
                    TableLoc::Exports(symbol),
                    Some(symbol),
                    node,
                    SymbolFlags::PROPERTY.union(SymbolFlags::ASSIGNMENT),
                    SymbolFlags::PROPERTY_EXCLUDES,
                );
            }
        }
    }

    /// Go: `func (b *Binder) bindExportsOrObjectDefineProperty(node *ast.Node)`.
    fn bind_exports_or_object_define_property(&mut self, node: NodeId) {
        if self.set_common_js_module_indicator(node) {
            let file_node = self.file.as_node_id();
            let is_binary_alias = is_binary_expression(self.file.node(node)) && {
                let right = self.file.node(node).as_binary_expression().expect("BinaryExpression data").right;
                crate::ast_shims::expression_is_alias(&*self.file, right)
            };
            let flags = if is_binary_alias {
                SymbolFlags::ALIAS
            } else {
                SymbolFlags::FUNCTION_SCOPED_VARIABLE
            };
            let file_symbol = self.container_symbol_expect(file_node);
            self.declare_symbol(
                TableLoc::Exports(file_symbol),
                Some(file_symbol),
                node,
                flags,
                SymbolFlags::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
            );
        }
    }

    /// Go: `func (b *Binder) bindThisPropertyAssignment(node *ast.Node)`.
    fn bind_this_property_assignment(&mut self, node: NodeId) {
        if !tsc_ast::utilities::is_in_js_file(self.file.node(node)) {
            return;
        }
        let bin = self.file.node(node).as_binary_expression().expect("BinaryExpression data");
        let left = bin.left;
        let left_is_private = tsc_ast::is_property_access_expression(self.file.node(left))
            && tsc_ast::is_private_identifier(self.file.node(self.file.node(left).name().expect("access name")));
        if left_is_private || self.this_container.is_none() {
            return;
        }
        let (class_symbol, symbol_table) = self.get_this_class_and_symbol_table();
        if let Some(symbol_table) = symbol_table {
            if has_dynamic_name(&*self.file, node) {
                let parent_class = class_symbol.expect("class symbol for this-property assignment");
                self.declare_symbol_ex(
                    symbol_table,
                    Some(parent_class),
                    node,
                    SymbolFlags::PROPERTY,
                    SymbolFlags::NONE,
                    true,  /*isReplaceableByMethod*/
                    true,  /*isComputedName*/
                );
                self.add_late_bound_assignment_declaration_to_symbol(node, parent_class);
            } else {
                let parent_class = class_symbol.expect("class symbol for this-property assignment");
                self.declare_symbol_ex(
                    symbol_table,
                    Some(parent_class),
                    node,
                    SymbolFlags::PROPERTY.union(SymbolFlags::ASSIGNMENT),
                    SymbolFlags::NONE,
                    true,  /*isReplaceableByMethod*/
                    false, /*isComputedName*/
                );
            }
        } else {
            let kind = self.this_container.map(|c| self.file.node(c).kind);
            if kind != Some(Kind::FunctionDeclaration) && kind != Some(Kind::FunctionExpression) {
                // !!! constructor functions
                panic!(
                    "Unhandled case in bindThisPropertyAssignment: {}",
                    self.file.node(self.this_container.expect("this container checked above")).kind_string()
                );
            }
        }
    }

    /// Go: `func (b *Binder) getThisClassAndSymbolTable() (classSymbol *ast.Symbol, symbolTable ast.SymbolTable)`.
    fn get_this_class_and_symbol_table(&mut self) -> (Option<SymbolId>, Option<TableLoc>) {
        let Some(this_container) = self.this_container else {
            return (None, None);
        };
        match self.file.node(this_container).kind {
            Kind::FunctionDeclaration | Kind::FunctionExpression => {
                // !!! constructor functions
            }
            Kind::Constructor
            | Kind::PropertyDeclaration
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassStaticBlockDeclaration => {
                // this.property assignment in class member -- bind to the containing class
                let parent = crate::ast_shims::parent_of(&*self.file, this_container).expect("class member parent");
                let class_symbol = self.container_symbol_expect(parent);
                if crate::ast_shims::is_static(self.file.node(this_container)) {
                    return (Some(class_symbol), Some(TableLoc::Exports(class_symbol)));
                }
                return (Some(class_symbol), Some(TableLoc::Members(class_symbol)));
            }
            _ => {}
        }
        (None, None)
    }

    /// Go: `func (b *Binder) bindEnumDeclaration(node *ast.Node)`.
    fn bind_enum_declaration(&mut self, node: NodeId) {
        if crate::ast_shims::is_enum_const(&*self.file, node) {
            self.bind_block_scoped_declaration(node, SymbolFlags::CONST_ENUM, SymbolFlags::CONST_ENUM_EXCLUDES);
        } else {
            self.bind_block_scoped_declaration(node, SymbolFlags::REGULAR_ENUM, SymbolFlags::REGULAR_ENUM_EXCLUDES);
        }
    }

    /// Go: `func (b *Binder) bindVariableDeclarationOrBindingElement(node *ast.Node)`.
    fn bind_variable_declaration_or_binding_element(&mut self, node: NodeId) {
        let name = self.file.node(node).name();
        self.check_strict_mode_eval_or_arguments(node, name);
        if let Some(name) = name {
            if !crate::ast_shims::is_binding_pattern(self.file.node(name)) {
                if is_variable_declaration_initialized_to_require(&*self.file, node) {
                    self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::ALIAS, SymbolFlags::ALIAS_EXCLUDES);
                } else if is_block_or_catch_scoped(&*self.file, node) {
                    self.bind_block_scoped_declaration(
                        node,
                        SymbolFlags::BLOCK_SCOPED_VARIABLE,
                        SymbolFlags::BLOCK_SCOPED_VARIABLE_EXCLUDES,
                    );
                } else if is_part_of_parameter_declaration(&*self.file, node) {
                    // It is safe to walk up parent chain to find whether the node is a destructuring parameter declaration
                    // because its parent chain has already been set up, since parents are set before descending into children.
                    //
                    // If node is a binding element in parameter declaration, we need to use ParameterExcludes.
                    // Using ParameterExcludes flag allows the compiler to report an error on duplicate identifiers in Parameter Declaration
                    // For example:
                    //      function foo([a,a]) {} // Duplicate Identifier error
                    //      function bar(a,a) {}   // Duplicate Identifier error, parameter declaration in this case is handled in bindParameter
                    //                             // which correctly set excluded symbols
                    self.declare_symbol_and_add_to_symbol_table(
                        node,
                        SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                        SymbolFlags::PARAMETER_EXCLUDES,
                    );
                } else {
                    self.declare_symbol_and_add_to_symbol_table(
                        node,
                        SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                        SymbolFlags::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
                    );
                }
            }
        }
    }

    /// Go: `func (b *Binder) bindParameter(node *ast.Node)`.
    fn bind_parameter(&mut self, node: NodeId) {
        let decl_name = match &self.file.node(node).data {
            tsc_ast::NodeData::ParameterDeclaration(d) => d.name,
            _ => unreachable!("bindParameter on ParameterDeclaration"),
        };
        if !self.file.node(node).flags.intersects(NodeFlags::AMBIENT) {
            // It is a SyntaxError if the identifier eval or arguments appears within a FormalParameterList of a
            // strict mode FunctionLikeDeclaration or FunctionExpression(13.1)
            self.check_strict_mode_eval_or_arguments(node, Some(decl_name));
        }
        if crate::ast_shims::is_binding_pattern(self.file.node(decl_name)) {
            let parent = crate::ast_shims::parent_of(&*self.file, node).expect("Parameter parent");
            let index = node_data::parameters_of(&*self.file, parent)
                .and_then(|list| list.nodes.iter().position(|&p| p == node))
                .unwrap_or(0);
            let name = format!("__{index}");
            self.bind_anonymous_declaration(node, SymbolFlags::FUNCTION_SCOPED_VARIABLE, &name);
        } else {
            self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::FUNCTION_SCOPED_VARIABLE, SymbolFlags::PARAMETER_EXCLUDES);
        }
        // If this is a property-parameter, then also declare the property symbol into the
        // containing class.
        let parent = crate::ast_shims::parent_of(&*self.file, node).expect("Parameter parent");
        if crate::ast_shims::is_parameter_property_declaration(&*self.file, node, parent) {
            let class_declaration =
                crate::ast_shims::parent_of(&*self.file, parent).expect("function-like parent");
            let question_token = match &self.file.node(node).data {
                tsc_ast::NodeData::ParameterDeclaration(d) => d.question_token,
                _ => None,
            };
            let flags = SymbolFlags::PROPERTY.union(if question_token.is_some() {
                SymbolFlags::OPTIONAL
            } else {
                SymbolFlags::NONE
            });
            let class_symbol = self.container_symbol_expect(class_declaration);
            self.declare_symbol(TableLoc::Members(class_symbol), Some(class_symbol), node, flags, SymbolFlags::PROPERTY_EXCLUDES);
        }
    }

    /// Go: `func (b *Binder) bindFunctionDeclaration(node *ast.Node)`.
    fn bind_function_declaration(&mut self, node: NodeId) {
        let is_async = !self.file.payload().is_declaration_file
            && !self.file.node(node).flags.intersects(NodeFlags::AMBIENT)
            && crate::ast_shims::is_async_function(&*self.file, node);
        if is_async {
            self.emit_flags |= NodeFlags::HAS_ASYNC_FUNCTIONS;
        }
        self.check_strict_mode_function_name(node);
        self.bind_block_scoped_declaration(node, SymbolFlags::FUNCTION, SymbolFlags::FUNCTION_EXCLUDES);
    }

    /// Go: `func (b *Binder) getInferTypeContainer(node *ast.Node) *ast.Node`.
    fn get_infer_type_container(&self, node: NodeId) -> Option<NodeId> {
        let extends_type = crate::ast_shims::find_ancestor(&*self.file, node, &mut |n| {
            let Some(parent) = crate::ast_shims::parent_of(&*self.file, n) else {
                return false;
            };
            tsc_ast::is_conditional_type_node(self.file.node(parent))
                && match &self.file.node(parent).data {
                    tsc_ast::NodeData::ConditionalTypeNode(d) => d.extends_type == n,
                    _ => false,
                }
        });
        extends_type.and_then(|t| crate::ast_shims::parent_of(&*self.file, t))
    }

    /// Go: `func (b *Binder) bindAnonymousDeclaration(node, symbolFlags, name)`.
    fn bind_anonymous_declaration(&mut self, node: NodeId, symbol_flags: SymbolFlags, name: &str) {
        let symbol = self.new_symbol(symbol_flags, name);
        if symbol_flags.intersects(SymbolFlags::ENUM_MEMBER.union(SymbolFlags::CLASS_MEMBER)) {
            let container_symbol = self.container_symbol(self.container);
            self.symbols[symbol.0 as usize].parent = container_symbol;
        }
        self.add_declaration_to_symbol(symbol, node, symbol_flags);
    }

    /// Go: `func (b *Binder) bindBlockScopedDeclaration(node, symbolFlags, symbolExcludes)`.
    fn bind_block_scoped_declaration(&mut self, node: NodeId, symbol_flags: SymbolFlags, symbol_excludes: SymbolFlags) {
        match self.file.node(self.block_scope_container).kind {
            Kind::ModuleDeclaration => {
                self.declare_module_member(node, symbol_flags, symbol_excludes);
            }
            Kind::SourceFile => {
                if is_external_or_common_js_module(self.file) {
                    self.declare_module_member(node, symbol_flags, symbol_excludes);
                    return;
                }
                self.declare_symbol(
                    TableLoc::Locals(self.block_scope_container),
                    None, /*parent*/
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
            _ => {
                self.declare_symbol(
                    TableLoc::Locals(self.block_scope_container),
                    None, /*parent*/
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
        }
    }

    /// Go: `func (b *Binder) bindTypeParameter(node *ast.Node)`.
    fn bind_type_parameter(&mut self, node: NodeId) {
        let parent = crate::ast_shims::parent_of(&*self.file, node).expect("TypeParameter parent");
        if self.file.node(parent).kind == Kind::InferType {
            let Some(container) = self.get_infer_type_container(parent) else {
                let name = self.get_declaration_name(node);
                self.bind_anonymous_declaration(node, SymbolFlags::TYPE_PARAMETER, &name);
                return;
            };
            self.declare_symbol(
                TableLoc::Locals(container),
                None, /*parent*/
                node,
                SymbolFlags::TYPE_PARAMETER,
                SymbolFlags::TYPE_PARAMETER_EXCLUDES,
            );
        } else {
            self.declare_symbol_and_add_to_symbol_table(node, SymbolFlags::TYPE_PARAMETER, SymbolFlags::TYPE_PARAMETER_EXCLUDES);
        }
    }

    /// Go: `func (b *Binder) lookupEntity(node, container) *ast.Symbol`.
    fn lookup_entity(&mut self, node: NodeId, container: NodeId) -> Option<SymbolId> {
        if is_identifier(self.file.node(node)) {
            return self.lookup_name(node_text(&*self.file, node), container);
        }
        let expression = node_data::expression_of(&*self.file, node).expect("access expression");
        if self.file.node(expression).kind == Kind::ThisKeyword {
            let (_, symbol_table) = self.get_this_class_and_symbol_table();
            if let Some(symbol_table) = symbol_table {
                if let Some(name) = crate::ast_shims::get_element_or_property_access_name(&*self.file, node) {
                    let name_text = node_text(&*self.file, name).to_string();
                    return self.table_get(&symbol_table, &name_text);
                }
            }
            return None;
        }
        let entity = self.lookup_entity(expression, container);
        let symbol = get_initializer_symbol(self, entity);
        if let Some(symbol) = symbol {
            let name = crate::ast_shims::get_element_or_property_access_name(&*self.file, node)?;
            let name_text = node_text(&*self.file, name).to_string();
            return self.symbols[symbol.0 as usize].exports.get(&name_text).copied();
        }
        None
    }

    /// Go: `func (b *Binder) lookupName(name string, container *ast.Node) *ast.Symbol`.
    fn lookup_name(&self, name: &str, container: NodeId) -> Option<SymbolId> {
        if let Some(locals) = node_data::locals_of(&*self.file, container) {
            if let Some(&local) = locals.get(name) {
                // Go: `core.OrElse(local.ExportSymbol, local)`
                return Some(self.symbols[local.0 as usize].export_symbol.unwrap_or(local));
            }
        }
        if let Some(symbol) = node_data::symbol_of(&*self.file, container) {
            return self.symbols[symbol.0 as usize].exports.get(name).copied();
        }
        None
    }

    /// Go: `func (b *Binder) declareCommonJSVariable(name string)`.
    fn declare_common_js_variable(&mut self, name: &str) {
        let file_node = self.file.as_node_id();
        let has = node_data::locals_of(&*self.file, file_node)
            .is_some_and(|locals| locals.has(name));
        if !has {
            let symbol = self.new_symbol(
                SymbolFlags::FUNCTION_SCOPED_VARIABLE.union(SymbolFlags::MODULE_EXPORTS),
                name,
            );
            // Go: symbol.Declarations = b.newSingleDeclaration(b.file.AsNode())
            self.symbols[symbol.0 as usize].declarations.push(file_node);
            self.symbols[symbol.0 as usize].value_declaration = Some(file_node);
            if name == "module" {
                let exports_property =
                    self.new_symbol(SymbolFlags::MODULE_EXPORTS.union(SymbolFlags::PROPERTY), "exports");
                self.symbols[exports_property.0 as usize].declarations = self.symbols[symbol.0 as usize].declarations.clone();
                self.symbols[exports_property.0 as usize].value_declaration = Some(file_node);
                self.symbols[exports_property.0 as usize].parent = Some(symbol);
                self.symbols[symbol.0 as usize]
                    .members
                    .set("exports".to_string(), exports_property);
            }
            if let Some(locals) = node_data::locals_mut(&mut *self.file, file_node) {
                locals.set(name.to_string(), symbol);
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Strict-mode checks (binder.go:1305-1473)
    // ────────────────────────────────────────────────────────────────────────

    // The binder visits every node in the syntax tree so it is a convenient place to perform a single localized
    // check for reserved words used as identifiers in strict mode code, as well as `yield` or `await` in
    // [Yield] or [Await] contexts, respectively.
    /// Go: `func (b *Binder) checkContextualIdentifier(node *ast.Node)`.
    fn check_contextual_identifier(&mut self, node: NodeId) {
        // Report error only if there are no parse errors in file
        let n = self.file.node(node);
        if self.file.payload().diagnostics.is_empty()
            && !n.flags.intersects(NodeFlags::AMBIENT)
            && !n.flags.intersects(NodeFlags::JSDOC)
            && !is_identifier_name(&*self.file, node)
        {
            // strict mode identifiers
            let text = node_text(&*self.file, node);
            let original_keyword_kind = get_identifier_token(text);
            if original_keyword_kind == Kind::Identifier {
                return;
            }
            let name = declaration_name_to_string(&*self.file, Some(node));
            if original_keyword_kind >= Kind::FirstFutureReservedWord
                && original_keyword_kind <= Kind::LastFutureReservedWord
            {
                let message = self.get_strict_mode_identifier_message(node);
                self.error_on_node(node, message, &[name]);
            } else if original_keyword_kind == Kind::AwaitKeyword {
                if is_external_module(self.file) && is_in_top_level_context(&*self.file, node) {
                    self.error_on_node(
                        node,
                        &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_AT_THE_TOP_LEVEL_OF_A_MODULE,
                        &[name],
                    );
                } else if self.file.node(node).flags.intersects(NodeFlags::AWAIT_CONTEXT) {
                    self.error_on_node(
                        node,
                        &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_THAT_CANNOT_BE_USED_HERE,
                        &[name],
                    );
                }
            } else if original_keyword_kind == Kind::YieldKeyword
                && self.file.node(node).flags.intersects(NodeFlags::YIELD_CONTEXT)
            {
                self.error_on_node(
                    node,
                    &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_THAT_CANNOT_BE_USED_HERE,
                    &[name],
                );
            }
        }
    }

    /// Go: `func (b *Binder) checkPrivateIdentifier(node *ast.Node)`.
    fn check_private_identifier(&mut self, node: NodeId) {
        if node_text(&*self.file, node) == "#constructor" {
            // Report error only if there are no parse errors in file
            if self.file.payload().diagnostics.is_empty() {
                let name = declaration_name_to_string(&*self.file, Some(node));
                self.error_on_node(node, &tsc_diagnostics::X_CONSTRUCTOR_IS_A_RESERVED_WORD, &[name]);
            }
        }
    }

    /// Go: `func (b *Binder) getStrictModeIdentifierMessage(node *ast.Node) *diagnostics.Message`.
    fn get_strict_mode_identifier_message(&self, node: NodeId) -> &'static Message {
        // Provide specialized messages to help the user understand why we think they're in
        // strict mode.
        if get_containing_class(&*self.file, node).is_some() {
            return &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_IN_STRICT_MODE_CLASS_DEFINITIONS_ARE_AUTOMATICALLY_IN_STRICT_MODE;
        }
        if self.file.payload().external_module_indicator.get().is_some() {
            return &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_IN_STRICT_MODE_MODULES_ARE_AUTOMATICALLY_IN_STRICT_MODE;
        }
        &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_IN_STRICT_MODE
    }

    /// Go: `func (b *Binder) checkStrictModeFunctionName(node *ast.Node)`.
    fn check_strict_mode_function_name(&mut self, node: NodeId) {
        if !self.file.node(node).flags.intersects(NodeFlags::AMBIENT) {
            // It is a SyntaxError if the identifier eval or arguments appears within a FormalParameterList of a strict mode FunctionDeclaration or FunctionExpression (13.1))
            self.check_strict_mode_eval_or_arguments(node, self.file.node(node).name());
        }
    }

    /// Go: `func (b *Binder) getStrictModeBlockScopeFunctionDeclarationMessage(node)`.
    ///
    /// PORT: dead code in the Go tree as well (defined at binder.go:1380,
    /// never called anywhere in tsc/internal) — kept verbatim for parity.
    #[allow(dead_code)]
    fn get_strict_mode_block_scope_function_declaration_message(&self, node: NodeId) -> &'static Message {
        // Provide specialized messages to help the user understand why we think they're in strict mode.
        if get_containing_class(&*self.file, node).is_some() {
            return &tsc_diagnostics::FUNCTION_DECLARATIONS_ARE_NOT_ALLOWED_INSIDE_BLOCKS_IN_STRICT_MODE_WHEN_TARGETING_ES5_CLASS_DEFINITIONS_ARE_AUTOMATICALLY_IN_STRICT_MODE;
        }
        if self.file.payload().external_module_indicator.get().is_some() {
            return &tsc_diagnostics::FUNCTION_DECLARATIONS_ARE_NOT_ALLOWED_INSIDE_BLOCKS_IN_STRICT_MODE_WHEN_TARGETING_ES5_MODULES_ARE_AUTOMATICALLY_IN_STRICT_MODE;
        }
        &tsc_diagnostics::FUNCTION_DECLARATIONS_ARE_NOT_ALLOWED_INSIDE_BLOCKS_IN_STRICT_MODE_WHEN_TARGETING_ES5
    }

    /// Go: `func (b *Binder) checkStrictModeBinaryExpression(node *ast.Node)`.
    fn check_strict_mode_binary_expression(&mut self, node: NodeId) {
        let expr = self.file.node(node).as_binary_expression().expect("BinaryExpression data").clone();
        if is_left_hand_side_expression(&*self.file, expr.left)
            && tsc_ast::is_assignment_operator(self.file.node(expr.operator_token).kind)
        {
            // ECMA 262 (Annex C) The identifier eval or arguments may not appear as the LeftHandSideExpression of an
            // Assignment operator(11.13) or of a PostfixExpression(11.3)
            self.check_strict_mode_eval_or_arguments(node, Some(expr.left));
        }
    }

    /// Go: `func (b *Binder) checkStrictModeCatchClause(node *ast.Node)`.
    fn check_strict_mode_catch_clause(&mut self, node: NodeId) {
        // It is a SyntaxError if a TryStatement with a Catch occurs within strict code and the Identifier of the
        // Catch production is eval or arguments
        let variable_declaration = match &self.file.node(node).data {
            tsc_ast::NodeData::CatchClause(d) => d.variable_declaration,
            _ => unreachable!("checkStrictModeCatchClause on CatchClause"),
        };
        if let Some(variable_declaration) = variable_declaration {
            let name = self.file.node(variable_declaration).name();
            self.check_strict_mode_eval_or_arguments(node, name);
        }
    }

    /// Go: `func (b *Binder) checkStrictModeDeleteExpression(node *ast.Node)`.
    fn check_strict_mode_delete_expression(&mut self, node: NodeId) {
        // Grammar checking
        let expression = node_data::expression_of(&*self.file, node).expect("DeleteExpression expression");
        if self.file.node(expression).kind == Kind::Identifier {
            // When a delete operator occurs within strict mode code, a SyntaxError is thrown if its
            // UnaryExpression is a direct reference to a variable, function argument, or function name
            self.error_on_node(
                expression,
                &tsc_diagnostics::X_DELETE_CANNOT_BE_CALLED_ON_AN_IDENTIFIER_IN_STRICT_MODE,
                &[],
            );
        }
    }

    /// Go: `func (b *Binder) checkStrictModePostfixUnaryExpression(node *ast.Node)`.
    fn check_strict_mode_postfix_unary_expression(&mut self, node: NodeId) {
        // Grammar checking
        // The identifier eval or arguments may not appear as the LeftHandSideExpression of an
        // Assignment operator(11.13) or of a PostfixExpression(11.3) or as the UnaryExpression
        // operated upon by a Prefix Increment(11.4.4) or a Prefix Decrement(11.4.5) operator.
        let operand = self
            .file
            .node(node)
            .as_postfix_unary_expression()
            .expect("PostfixUnaryExpression data")
            .clone()
            .operand;
        self.check_strict_mode_eval_or_arguments(node, Some(operand));
    }

    /// Go: `func (b *Binder) checkStrictModePrefixUnaryExpression(node *ast.Node)`.
    fn check_strict_mode_prefix_unary_expression(&mut self, node: NodeId) {
        // Grammar checking
        let expr = self
            .file
            .node(node)
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression data")
            .clone();
        if expr.operator == Kind::PlusPlusToken || expr.operator == Kind::MinusMinusToken {
            self.check_strict_mode_eval_or_arguments(node, Some(expr.operand));
        }
    }

    /// Go: `func (b *Binder) checkStrictModeWithStatement(node *ast.Node)`.
    fn check_strict_mode_with_statement(&mut self, node: NodeId) {
        // Grammar checking for withStatement
        self.error_on_first_token(node, &tsc_diagnostics::X_WITH_STATEMENTS_ARE_NOT_ALLOWED_IN_STRICT_MODE, &[]);
    }

    /// Go: `func (b *Binder) checkStrictModeLabeledStatement(node *ast.Node)`.
    fn check_strict_mode_labeled_statement(&mut self, node: NodeId) {
        // Grammar checking for labeledStatement
        let (label, statement) = match &self.file.node(node).data {
            tsc_ast::NodeData::LabeledStatement(d) => (d.label, d.statement),
            _ => unreachable!("checkStrictModeLabeledStatement on LabeledStatement"),
        };
        if is_declaration_statement(self.file.node(statement))
            || is_variable_statement(self.file.node(statement))
        {
            self.error_on_first_token(label, &tsc_diagnostics::A_LABEL_IS_NOT_ALLOWED_HERE, &[]);
        }
    }

    /// Go: `func (b *Binder) checkStrictModeEvalOrArguments(contextNode, name *ast.Node)`.
    fn check_strict_mode_eval_or_arguments(&mut self, context_node: NodeId, name: Option<NodeId>) {
        if let Some(name) = name {
            if is_eval_or_arguments_identifier(&*self.file, name) {
                // We check first if the name is inside class declaration or class expression; if so give explicit message
                // otherwise report generic error message.
                let message = self.get_strict_mode_eval_or_arguments_message(context_node);
                let text = node_text(&*self.file, name).to_string();
                self.error_on_node(name, message, &[text]);
            }
        }
    }

    /// Go: `func (b *Binder) getStrictModeEvalOrArgumentsMessage(node) *diagnostics.Message`.
    fn get_strict_mode_eval_or_arguments_message(&self, node: NodeId) -> &'static Message {
        // Provide specialized messages to help us understand why we think they're in strict mode
        if get_containing_class(&*self.file, node).is_some() {
            return &tsc_diagnostics::CODE_CONTAINED_IN_A_CLASS_IS_EVALUATED_IN_JAVASCRIPT_S_STRICT_MODE_WHICH_DOES_NOT_ALLOW_THIS_USE_OF_0_FOR_MORE_INFORMATION_SEE_HTTPS_COLON_SLASH_SLASHDEVELOPER_MOZILLA_ORG_SLASHEN_US_SLASHDOCS_SLASHWEB_SLASHJAVASCRIPT_SLASHREFERENCE_SLASHSTRICT_MODE;
        }
        if self.file.payload().external_module_indicator.get().is_some() {
            return &tsc_diagnostics::INVALID_USE_OF_0_MODULES_ARE_AUTOMATICALLY_IN_STRICT_MODE;
        }
        &tsc_diagnostics::INVALID_USE_OF_0_IN_STRICT_MODE
    }

    // ────────────────────────────────────────────────────────────────────────
    // Container binding (binder.go:1478-1643)
    // ────────────────────────────────────────────────────────────────────────

    // All container nodes are kept on a linked list in declaration order. This list is used by
    // the getLocalNameOfContainer function in the type checker to validate that the local name
    // used for a container is unique.
    /// Go: `func (b *Binder) bindContainer(node *ast.Node, containerFlags ContainerFlags)`.
    fn bind_container(&mut self, node: NodeId, container_flags: ContainerFlags) {
        // Before we recurse into a node's children, we first save the existing parent, container
        // and block-container.  Then after we pop out of processing the children, we restore
        // these saved values.
        let save_container = self.container;
        let save_this_container = self.this_container;
        let saved_block_scope_container = self.block_scope_container;
        // Depending on what kind of node this is, we may have to adjust the current container
        // and block-container.   If the current node is a container, then it is automatically
        // considered the current block-container as well.  Also, for containers that we know
        // may contain locals, we eagerly initialize the .locals field. We do this because
        // it's highly likely that the .locals will be needed to place some child in (for example,
        // a parameter, or variable declaration).
        //
        // However, we do not proactively create the .locals for block-containers because it's
        // totally normal and common for block-containers to never actually have a block-scoped
        // variable in them.  We don't want to end up allocating an object for every 'block' we
        // run into when most of them won't be necessary.
        //
        // Finally, if this is a block-container, then we clear out any existing .locals object
        // it may contain within it.  This happens in incremental scenarios.  Because we can be
        // reusing a node from a previous compilation, that node may have had 'locals' created
        // for it.  We must clear this so we don't accidentally move any stale data forward from
        // a previous compilation.
        if container_flags.intersects(ContainerFlags::IS_CONTAINER) {
            self.container = node;
            self.block_scope_container = node;
            if container_flags.intersects(ContainerFlags::HAS_LOCALS) {
                // localsContainer := node
                // localsContainer.LocalsContainerData().locals = make(SymbolTable)
                self.add_to_container_chain(node);
            }
        } else if container_flags.intersects(ContainerFlags::IS_BLOCK_SCOPED_CONTAINER) {
            self.block_scope_container = node;
            self.add_to_container_chain(node);
        }
        if container_flags.intersects(ContainerFlags::IS_THIS_CONTAINER) {
            self.this_container = Some(node);
        }
        if container_flags.intersects(ContainerFlags::IS_CONTROL_FLOW_CONTAINER) {
            let save_current_flow = self.current_flow;
            let save_break_target = self.current_break_target;
            let save_continue_target = self.current_continue_target;
            let save_return_target = self.current_return_target;
            let save_exception_target = self.current_exception_target;
            let save_active_label_list = std::mem::take(&mut self.active_label_list);
            let save_has_explicit_return = self.has_explicit_return;
            let save_seen_this_keyword = self.seen_this_keyword;
            let is_immediately_invoked = (container_flags.intersects(ContainerFlags::IS_FUNCTION_EXPRESSION)
                && !has_syntactic_modifier(self.file.node(node), ModifierFlags::ASYNC)
                && !is_generator_function_expression(&*self.file, node)
                && get_immediately_invoked_function_expression(&*self.file, node).is_some())
                || self.file.node(node).kind == Kind::ClassStaticBlockDeclaration;
            // A non-async, non-generator IIFE is considered part of the containing control flow. Return statements behave
            // similarly to break statements that exit to a label just past the statement body.
            if !is_immediately_invoked {
                let flow_start = self.new_flow_node(FlowFlags::START);
                self.current_flow = flow_start;
                if container_flags
                    .intersects(ContainerFlags::IS_FUNCTION_EXPRESSION.union(ContainerFlags::IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR))
                {
                    let flow = self.flow_mut(flow_start);
                    flow.node = FlowNodeData::Node(node);
                }
            }
            // We create a return control flow graph for IIFEs and constructors. For constructors
            // we use the return control flow graph in strict property initialization checks.
            if is_immediately_invoked || self.file.node(node).kind == Kind::Constructor {
                self.current_return_target = Some(self.new_flow_node(FlowFlags::BRANCH_LABEL));
            } else {
                self.current_return_target = None;
            }
            self.current_exception_target = None;
            self.current_break_target = None;
            self.current_continue_target = None;
            self.active_label_list = None;
            self.has_explicit_return = false;
            self.seen_this_keyword = false;
            self.bind_children(node);
            // Reset flags (for incremental scenarios)
            let reset_mask = NodeFlags::REACHABILITY_AND_EMIT_FLAGS.union(NodeFlags::CONTAINS_THIS);
            {
                let n = self.file.node_mut(node);
                n.flags = NodeFlags(n.flags.0 & !reset_mask.0);
            }
            if !self.flow(self.current_flow).flags.intersects(FlowFlags::UNREACHABLE)
                && container_flags.intersects(ContainerFlags::IS_FUNCTION_LIKE)
                && node_data::has_body_data(self.file.node(node))
                && node_data::body_of(&*self.file, node)
                    .is_some_and(|b| node_is_present(self.file.node(b)))
            {
                {
                    let n = self.file.node_mut(node);
                    n.flags |= NodeFlags::HAS_IMPLICIT_RETURN;
                    if self.has_explicit_return {
                        n.flags |= NodeFlags::HAS_EXPLICIT_RETURN;
                    }
                }
                node_data::set_end_flow_node(&mut *self.file, node, Some(self.current_flow));
            }
            if self.seen_this_keyword {
                self.file.node_mut(node).flags |= NodeFlags::CONTAINS_THIS;
            }
            if self.file.node(node).kind == Kind::SourceFile {
                self.file.node_mut(node).flags |= self.emit_flags;
            }
            if let Some(return_target) = self.current_return_target {
                self.add_antecedent(return_target, self.current_flow);
                self.current_flow = self.finish_flow_label(return_target);
                let kind = self.file.node(node).kind;
                if kind == Kind::Constructor || kind == Kind::ClassStaticBlockDeclaration {
                    set_return_flow_node(&mut *self.file, node, self.current_flow);
                }
            }
            if !is_immediately_invoked {
                self.current_flow = save_current_flow;
            }
            self.current_break_target = save_break_target;
            self.current_continue_target = save_continue_target;
            self.current_return_target = save_return_target;
            self.current_exception_target = save_exception_target;
            self.active_label_list = save_active_label_list;
            self.has_explicit_return = save_has_explicit_return;
            if container_flags.intersects(ContainerFlags::PROPAGATES_THIS_KEYWORD) {
                self.seen_this_keyword = save_seen_this_keyword || self.seen_this_keyword;
            } else {
                self.seen_this_keyword = save_seen_this_keyword;
            }
        } else if container_flags.intersects(ContainerFlags::IS_INTERFACE) {
            let save_seen_this_keyword = self.seen_this_keyword;
            self.seen_this_keyword = false;
            self.bind_children(node);
            // ContainsThis cannot overlap with HasExtendedUnicodeEscape on Identifier
            if self.seen_this_keyword {
                self.file.node_mut(node).flags |= NodeFlags::CONTAINS_THIS;
            } else {
                let n = self.file.node_mut(node);
                n.flags = NodeFlags(n.flags.0 & !NodeFlags::CONTAINS_THIS.0);
            }
            self.seen_this_keyword = save_seen_this_keyword;
        } else {
            self.bind_children(node);
        }
        if tsc_ast::is_source_file(self.file.node(node))
            && tsc_ast::utilities::is_in_js_file(self.file.node(node))
        {
            // Binding of top-level JSTypeAliasDeclaration nodes is deferred to ensure CommonJS module
            // indicators, if any, are processed first.
            let file_node = self.file.as_node_id();
            let statements: Vec<NodeId> = node_data::statements_of(&*self.file, file_node)
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            for statement in statements {
                if self.file.node(statement).kind == Kind::JSTypeAliasDeclaration {
                    self.bind_block_scoped_declaration(
                        statement,
                        SymbolFlags::TYPE_ALIAS,
                        SymbolFlags::TYPE_ALIAS_EXCLUDES,
                    );
                }
            }
            if self.file.payload().common_js_module_indicator.get().is_some() {
                self.declare_common_js_variable("module");
                self.declare_common_js_variable("exports");
            }
        }
        if tsc_ast::is_source_file(self.file.node(node)) && is_external_or_common_js_module(self.file)
            || is_ambient_module(&*self.file, node)
        {
            let symbol = self.container_symbol_expect(node);
            self.bind_common_js_type_exports(symbol);
        }
        self.container = save_container;
        self.this_container = save_this_container;
        self.block_scope_container = saved_block_scope_container;
    }

    /// Go: `func (b *Binder) addToContainerChain(next *ast.Node)`.
    fn add_to_container_chain(&mut self, next: NodeId) {
        if let Some(last_container) = self.last_container {
            node_data::set_next_container(&mut *self.file, last_container, Some(next));
        }
        self.last_container = Some(next);
    }

    // ────────────────────────────────────────────────────────────────────────
    // Child binding + the flow-graph statement builders (binder.go:1645-2508)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) bindChildren(node *ast.Node)`.
    fn bind_children(&mut self, node: NodeId) {
        let save_in_assignment_pattern = self.in_assignment_pattern;
        // Most nodes aren't valid in an assignment pattern, so we clear the value here
        // and set it before we descend into nodes that could actually be part of an assignment pattern.
        self.in_assignment_pattern = false;

        if self.current_flow == self.unreachable_flow {
            node_data::set_flow_node(&mut *self.file, node, None);
            if is_potentially_executable_node(&*self.file, node) {
                self.file.node_mut(node).flags |= NodeFlags::UNREACHABLE;
            }
            self.bind_each_child(node);
            self.in_assignment_pattern = save_in_assignment_pattern;
            return;
        }

        let kind = self.file.node(node).kind;
        if Kind::FirstStatement <= kind && kind <= Kind::LastStatement {
            self.set_flow_node_field(node, self.current_flow);
        }

        match kind {
            Kind::WhileStatement => self.bind_while_statement(node),
            Kind::DoStatement => self.bind_do_statement(node),
            Kind::ForStatement => self.bind_for_statement(node),
            Kind::ForInStatement | Kind::ForOfStatement => self.bind_for_in_or_for_of_statement(node),
            Kind::IfStatement => self.bind_if_statement(node),
            Kind::ReturnStatement => self.bind_return_statement(node),
            Kind::ThrowStatement => self.bind_throw_statement(node),
            Kind::BreakStatement => self.bind_break_statement(node),
            Kind::ContinueStatement => self.bind_continue_statement(node),
            Kind::TryStatement => self.bind_try_statement(node),
            Kind::SwitchStatement => self.bind_switch_statement(node),
            Kind::CaseBlock => self.bind_case_block(node),
            Kind::CaseClause | Kind::DefaultClause => self.bind_case_or_default_clause(node),
            Kind::ExpressionStatement => self.bind_expression_statement(node),
            Kind::LabeledStatement => self.bind_labeled_statement(node),
            Kind::PrefixUnaryExpression => self.bind_prefix_unary_expression_flow(node),
            Kind::PostfixUnaryExpression => self.bind_postfix_unary_expression_flow(node),
            Kind::BinaryExpression => {
                if is_destructuring_assignment(&*self.file, node) {
                    // Carry over whether we are in an assignment pattern to
                    // binary expressions that could actually be an initializer
                    self.in_assignment_pattern = save_in_assignment_pattern;
                    self.bind_destructuring_assignment_flow(node);
                    return;
                }
                self.bind_binary_expression_flow(node);
            }
            Kind::DeleteExpression => self.bind_delete_expression_flow(node),
            Kind::ConditionalExpression => self.bind_conditional_expression_flow(node),
            Kind::VariableDeclaration => self.bind_variable_declaration_flow(node),
            Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
                self.bind_access_expression_flow(node)
            }
            Kind::CallExpression => self.bind_call_expression_flow(node),
            Kind::NonNullExpression => self.bind_non_null_expression_flow(node),
            Kind::SourceFile => {
                let file_node = self.file.as_node_id();
                let statements = node_data::statements_of(&*self.file, file_node).map(|l| l.nodes.to_vec());
                let end_of_file_token = self.file.payload().end_of_file_token;
                self.bind_each_statement_functions_first(statements.as_deref());
                self.bind(end_of_file_token);
            }
            Kind::Block | Kind::ModuleBlock => {
                let statements = node_data::statements_of(&*self.file, node).map(|l| l.nodes.to_vec());
                self.bind_each_statement_functions_first(statements.as_deref());
            }
            Kind::BindingElement => self.bind_binding_element_flow(node),
            Kind::Parameter => self.bind_parameter_flow(node),
            Kind::ObjectLiteralExpression
            | Kind::ArrayLiteralExpression
            | Kind::PropertyAssignment
            | Kind::SpreadElement => {
                self.in_assignment_pattern = save_in_assignment_pattern;
                self.bind_each_child(node);
                self.in_assignment_pattern = save_in_assignment_pattern;
                return;
            }
            _ => self.bind_each_child(node),
        }
        self.in_assignment_pattern = save_in_assignment_pattern;
    }

    /// Go: `func (b *Binder) bindEachChild(node *ast.Node)` — the port
    /// collects the child list before binding (the Node is borrowed from
    /// the same store the binder mutates; Go's visitor walks live pointers).
    fn bind_each_child(&mut self, node: NodeId) {
        let mut children: Vec<NodeId> = Vec::new();
        self.file.node(node).for_each_child(&mut |child| {
            children.push(child);
            false
        });
        for child in children {
            self.bind(Some(child));
        }
    }

    /// Go: `func (b *Binder) bindEach(nodes []*ast.Node)`.
    fn bind_each(&mut self, nodes: &[NodeId]) {
        for &node in nodes {
            self.bind(Some(node));
        }
    }

    /// Go: `func (b *Binder) bindNodeList(nodeList *ast.NodeList)`.
    fn bind_node_list(&mut self, node_list: Option<&NodeList>) {
        if let Some(node_list) = node_list {
            self.bind_each(&node_list.nodes);
        }
    }

    /// Go: `func (b *Binder) bindEachStatementFunctionsFirst(statements *ast.NodeList)`.
    fn bind_each_statement_functions_first(&mut self, statements: Option<&[NodeId]>) {
        let Some(statements) = statements else {
            return;
        };
        for &node in statements {
            if self.file.node(node).kind == Kind::FunctionDeclaration {
                self.bind(Some(node));
            }
        }
        for &node in statements {
            if self.file.node(node).kind != Kind::FunctionDeclaration {
                self.bind(Some(node));
            }
        }
    }

    /// Go: `func (b *Binder) setContinueTarget(node, target *ast.FlowLabel) *ast.FlowLabel`.
    fn set_continue_target(&mut self, node: NodeId, target: FlowNodeId) -> FlowNodeId {
        let mut node = node;
        let mut label = self.active_label_list.as_deref_mut();
        while let Some(l) = label {
            let Some(parent) = crate::ast_shims::parent_of(&*self.file, node) else {
                break;
            };
            if self.file.node(parent).kind != Kind::LabeledStatement {
                break;
            }
            l.continue_target = Some(target);
            node = parent;
            label = l.next.as_deref_mut();
        }
        target
    }

    /// Go: `func (b *Binder) doWithConditionalBranches(action, value, trueTarget, falseTarget)`.
    fn do_with_conditional_branches(
        &mut self,
        action: impl FnOnce(&mut Self, Option<NodeId>) -> bool,
        value: Option<NodeId>,
        true_target: Option<FlowNodeId>,
        false_target: Option<FlowNodeId>,
    ) {
        let saved_true_target = self.current_true_target;
        let saved_false_target = self.current_false_target;
        self.current_true_target = true_target;
        self.current_false_target = false_target;
        action(self, value);
        self.current_true_target = saved_true_target;
        self.current_false_target = saved_false_target;
    }

    /// Go: `func (b *Binder) bindCondition(node, trueTarget, falseTarget)`.
    fn bind_condition(
        &mut self,
        node: Option<NodeId>,
        true_target: Option<FlowNodeId>,
        false_target: Option<FlowNodeId>,
    ) {
        self.do_with_conditional_branches(Self::bind, node, true_target, false_target);
        let no_target_effects = node.map(|n| {
            !is_logical_assignment_expression(&*self.file, n)
                && !crate::ast_shims::is_logical_expression(&*self.file, n)
                && !(is_optional_chain(self.file.node(n)) && is_outermost_optional_chain(&*self.file, n))
        });
        if no_target_effects.unwrap_or(true) {
            let current = self.current_flow;
            let true_condition = self.create_flow_condition(FlowFlags::TRUE_CONDITION, current, node);
            let true_target = true_target.expect("bindCondition trueTarget");
            self.add_antecedent(true_target, true_condition);
            let current = self.current_flow;
            let false_condition = self.create_flow_condition(FlowFlags::FALSE_CONDITION, current, node);
            let false_target = false_target.expect("bindCondition falseTarget");
            self.add_antecedent(false_target, false_condition);
        }
    }

    /// Go: `func (b *Binder) bindIterativeStatement(node, breakTarget, continueTarget)`.
    fn bind_iterative_statement(&mut self, node: NodeId, break_target: Option<FlowNodeId>, continue_target: Option<FlowNodeId>) {
        let save_break_target = self.current_break_target;
        let save_continue_target = self.current_continue_target;
        self.current_break_target = break_target;
        self.current_continue_target = continue_target;
        self.bind(Some(node));
        self.current_break_target = save_break_target;
        self.current_continue_target = save_continue_target;
    }

    /// Go: `func (b *Binder) bindAssignmentTargetFlow(node *ast.Node)`.
    fn bind_assignment_target_flow(&mut self, node: NodeId) {
        let kind = self.file.node(node).kind;
        match kind {
            Kind::ArrayLiteralExpression => {
                let elements = node_data::elements_of(&*self.file, node).to_vec();
                for e in elements {
                    if self.file.node(e).kind == Kind::SpreadElement {
                        let expression =
                            node_data::expression_of(&*self.file, e).expect("SpreadElement expression");
                        self.bind_assignment_target_flow(expression);
                    } else {
                        self.bind_destructuring_target_flow(e);
                    }
                }
            }
            Kind::ObjectLiteralExpression => {
                let properties = node_data::properties_of(&*self.file, node).to_vec();
                for p in properties {
                    match self.file.node(p).kind {
                        Kind::PropertyAssignment => {
                            let initializer = node_data::initializer_of(&*self.file, p);
                            if let Some(initializer) = initializer {
                                self.bind_destructuring_target_flow(initializer);
                            }
                        }
                        Kind::ShorthandPropertyAssignment => {
                            let name = self.file.node(p).name().expect("ShorthandPropertyAssignment name");
                            self.bind_assignment_target_flow(name);
                        }
                        Kind::SpreadAssignment => {
                            let expression =
                                node_data::expression_of(&*self.file, p).expect("SpreadAssignment expression");
                            self.bind_assignment_target_flow(expression);
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                if is_narrowable_reference(&*self.file, node) {
                    let current = self.current_flow;
                    self.current_flow = self.create_flow_mutation(FlowFlags::ASSIGNMENT, current, node);
                }
            }
        }
    }

    /// Go: `func (b *Binder) bindDestructuringTargetFlow(node *ast.Node)`.
    fn bind_destructuring_target_flow(&mut self, node: NodeId) {
        let is_default_assignment = is_binary_expression(self.file.node(node))
            && self
                .file
                .node(node)
                .as_binary_expression()
                .is_some_and(|d| self.file.node(d.operator_token).kind == Kind::EqualsToken);
        if is_default_assignment {
            let left = self.file.node(node).as_binary_expression().expect("BinaryExpression data").left;
            self.bind_assignment_target_flow(left);
        } else {
            self.bind_assignment_target_flow(node);
        }
    }

    /// Go: `func (b *Binder) bindWhileStatement(node *ast.Node)`.
    fn bind_while_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_while_statement().expect("WhileStatement data").clone();
        let loop_label = self.create_loop_label();
        let pre_while_label = self.set_continue_target(node, loop_label);
        let pre_body_label = self.create_branch_label();
        let post_while_label = self.create_branch_label();
        self.add_antecedent(pre_while_label, self.current_flow);
        self.current_flow = pre_while_label;
        self.bind_condition(Some(stmt.expression), Some(pre_body_label), Some(post_while_label));
        self.current_flow = self.finish_flow_label(pre_body_label);
        self.bind_iterative_statement(stmt.statement, Some(post_while_label), Some(pre_while_label));
        self.add_antecedent(pre_while_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_while_label);
    }

    /// Go: `func (b *Binder) bindDoStatement(node *ast.Node)`.
    fn bind_do_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_do_statement().expect("DoStatement data").clone();
        let pre_do_label = self.create_loop_label();
        let branch_label = self.create_branch_label();
        let pre_condition_label = self.set_continue_target(node, branch_label);
        let post_do_label = self.create_branch_label();
        self.add_antecedent(pre_do_label, self.current_flow);
        self.current_flow = pre_do_label;
        self.bind_iterative_statement(stmt.statement, Some(post_do_label), Some(pre_condition_label));
        self.add_antecedent(pre_condition_label, self.current_flow);
        self.current_flow = self.finish_flow_label(pre_condition_label);
        self.bind_condition(Some(stmt.expression), Some(pre_do_label), Some(post_do_label));
        self.current_flow = self.finish_flow_label(post_do_label);
    }

    /// Go: `func (b *Binder) bindForStatement(node *ast.Node)`.
    fn bind_for_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_for_statement().expect("ForStatement data").clone();
        self.bind(stmt.initializer);
        if self.current_flow == self.unreachable_flow {
            // Unlike while/do, the for-loop initializer is bound inside this function before the loop's
            // flow graph is constructed. If it makes flow unreachable (e.g. a throwing IIFE), addAntecedent
            // will filter out the unreachable entry to preLoopLabel, leaving only the back-edge from the
            // incrementor. This creates a cycle with no exit that crashes isReachableFlowNodeWorker.
            // Bail out early and just bind the remaining children with unreachable flow.
            self.bind(stmt.condition);
            self.bind(Some(stmt.statement));
            self.bind(stmt.incrementor);
            return;
        }
        let loop_label = self.create_loop_label();
        let pre_loop_label = self.set_continue_target(node, loop_label);
        let pre_body_label = self.create_branch_label();
        let pre_incrementor_label = self.create_branch_label();
        let post_loop_label = self.create_branch_label();
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = pre_loop_label;
        self.bind_condition(stmt.condition, Some(pre_body_label), Some(post_loop_label));
        self.current_flow = self.finish_flow_label(pre_body_label);
        self.bind_iterative_statement(stmt.statement, Some(post_loop_label), Some(pre_incrementor_label));
        self.add_antecedent(pre_incrementor_label, self.current_flow);
        self.current_flow = self.finish_flow_label(pre_incrementor_label);
        self.bind(stmt.incrementor);
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_loop_label);
    }

    /// Go: `func (b *Binder) bindForInOrForOfStatement(node *ast.Node)`.
    fn bind_for_in_or_for_of_statement(&mut self, node: NodeId) {
        let stmt = self
            .file
            .node(node)
            .as_for_in_or_of_statement()
            .expect("ForInOrOfStatement data")
            .clone();
        self.bind(Some(stmt.expression));
        if self.current_flow == self.unreachable_flow {
            // Like the for-loop initializer, the for-in/for-of expression is bound before the loop's
            // flow graph is constructed. If it makes flow unreachable (e.g. a throwing IIFE), addAntecedent
            // will filter out the unreachable entry to preLoopLabel, leaving only the back-edge from the
            // loop body. This creates a cycle with no exit that crashes isReachableFlowNodeWorker.
            // Bail out early and just bind the remaining children with unreachable flow.
            self.bind(Some(stmt.initializer));
            self.bind(Some(stmt.statement));
            return;
        }
        let loop_label = self.create_loop_label();
        let pre_loop_label = self.set_continue_target(node, loop_label);
        let post_loop_label = self.create_branch_label();
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = pre_loop_label;
        if self.file.node(node).kind == Kind::ForOfStatement {
            self.bind(stmt.await_modifier);
        }
        self.add_antecedent(post_loop_label, self.current_flow);
        self.bind(Some(stmt.initializer));
        if self.file.node(stmt.initializer).kind != Kind::VariableDeclarationList {
            self.bind_assignment_target_flow(stmt.initializer);
        }
        self.bind_iterative_statement(stmt.statement, Some(post_loop_label), Some(pre_loop_label));
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_loop_label);
    }

    /// Go: `func (b *Binder) bindIfStatement(node *ast.Node)`.
    fn bind_if_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_if_statement().expect("IfStatement data").clone();
        let then_label = self.create_branch_label();
        let else_label = self.create_branch_label();
        let post_if_label = self.create_branch_label();
        self.bind_condition(Some(stmt.expression), Some(then_label), Some(else_label));
        self.current_flow = self.finish_flow_label(then_label);
        self.bind(Some(stmt.then_statement));
        self.add_antecedent(post_if_label, self.current_flow);
        self.current_flow = self.finish_flow_label(else_label);
        self.bind(stmt.else_statement);
        self.add_antecedent(post_if_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_if_label);
    }

    /// Go: `func (b *Binder) bindReturnStatement(node *ast.Node)`.
    fn bind_return_statement(&mut self, node: NodeId) {
        let expression = node_data::expression_of(&*self.file, node);
        self.bind(expression);
        if let Some(return_target) = self.current_return_target {
            self.add_antecedent(return_target, self.current_flow);
        }
        self.current_flow = self.unreachable_flow;
        self.has_explicit_return = true;
        self.has_flow_effects = true;
    }

    /// Go: `func (b *Binder) bindThrowStatement(node *ast.Node)`.
    fn bind_throw_statement(&mut self, node: NodeId) {
        let expression = node_data::expression_of(&*self.file, node).expect("ThrowStatement expression");
        self.bind(Some(expression));
        self.current_flow = self.unreachable_flow;
        self.has_flow_effects = true;
    }

    /// Go: `func (b *Binder) bindBreakStatement(node *ast.Node)`.
    fn bind_break_statement(&mut self, node: NodeId) {
        self.bind_break_or_continue_statement(node_data::label_of(&*self.file, node), self.current_break_target, LabelSlot::Break);
    }

    /// Go: `func (b *Binder) bindContinueStatement(node *ast.Node)`.
    fn bind_continue_statement(&mut self, node: NodeId) {
        self.bind_break_or_continue_statement(node_data::label_of(&*self.file, node), self.current_continue_target, LabelSlot::Continue);
    }

    /// Go: `func (b *Binder) bindBreakOrContinueStatement(label, currentTarget, getTarget)` —
    /// Go's `getTarget func(*ActiveLabel) *FlowNode` method value becomes the
    /// [`LabelSlot`] (the two callers select BreakTarget vs. ContinueTarget).
    fn bind_break_or_continue_statement(
        &mut self,
        label: Option<NodeId>,
        current_target: Option<FlowNodeId>,
        slot: LabelSlot,
    ) {
        self.bind(label);
        if let Some(label) = label {
            let label_text = node_text(&*self.file, label).to_string();
            if let Some(target) = self.find_active_label_target(&label_text, slot) {
                self.bind_break_or_continue_flow(Some(target));
            }
        } else {
            self.bind_break_or_continue_flow(current_target);
        }
    }

    /// Go: `func (b *Binder) findActiveLabel(name string) *ActiveLabel` +
    /// `activeLabel.referenced = true` + `getTarget(activeLabel)` — walks the
    /// active-label chain, marks the match referenced, and returns its
    /// break/continue target (None when no label carries `name`).
    fn find_active_label_target(&mut self, name: &str, slot: LabelSlot) -> Option<FlowNodeId> {
        let mut label = self.active_label_list.as_deref_mut();
        while let Some(l) = label {
            if l.name == name {
                l.referenced = true;
                return match slot {
                    LabelSlot::Break => l.break_target,
                    LabelSlot::Continue => l.continue_target,
                };
            }
            label = l.next.as_deref_mut();
        }
        None
    }

    /// Go: `func (b *Binder) bindBreakOrContinueFlow(flowLabel *ast.FlowLabel)`.
    fn bind_break_or_continue_flow(&mut self, flow_label: Option<FlowNodeId>) {
        if let Some(flow_label) = flow_label {
            self.add_antecedent(flow_label, self.current_flow);
            self.current_flow = self.unreachable_flow;
            self.has_flow_effects = true;
        }
    }

    /// Go: `func (b *Binder) bindTryStatement(node *ast.Node)`.
    fn bind_try_statement(&mut self, node: NodeId) {
        // We conservatively assume that *any* code in the try block can cause an exception, but we only need
        // to track code that causes mutations (because only mutations widen the possible control flow type of
        // a variable). The exceptionLabel is the target label for control flows that result from exceptions.
        // We add all mutation flow nodes as antecedents of this label such that we can analyze them as possible
        // antecedents of the start of catch or finally blocks. Furthermore, we add the current control flow to
        // represent exceptions that occur before any mutations.
        let stmt = self.file.node(node).as_try_statement().expect("TryStatement data").clone();
        let save_return_target = self.current_return_target;
        let save_exception_target = self.current_exception_target;
        let normal_exit_label = self.create_branch_label();
        let return_label = self.create_branch_label();
        let mut exception_label = self.create_branch_label();
        if stmt.finally_block.is_some() {
            self.current_return_target = Some(return_label);
        }
        self.add_antecedent(exception_label, self.current_flow);
        self.current_exception_target = Some(exception_label);
        self.bind(Some(stmt.try_block));
        self.add_antecedent(normal_exit_label, self.current_flow);
        if let Some(catch_clause) = stmt.catch_clause {
            // Start of catch clause is the target of exceptions from try block.
            self.current_flow = self.finish_flow_label(exception_label);
            // The currentExceptionTarget now represents control flows from exceptions in the catch clause.
            // Effectively, in a try-catch-finally, if an exception occurs in the try block, the catch block
            // acts like a second try block.
            exception_label = self.create_branch_label();
            self.add_antecedent(exception_label, self.current_flow);
            self.current_exception_target = Some(exception_label);
            self.bind(Some(catch_clause));
            self.add_antecedent(normal_exit_label, self.current_flow);
        }
        self.current_return_target = save_return_target;
        self.current_exception_target = save_exception_target;
        if let Some(finally_block) = stmt.finally_block {
            // Possible ways control can reach the finally block:
            // 1) Normal completion of try block of a try-finally or try-catch-finally
            // 2) Normal completion of catch block (following exception in try block) of a try-catch-finally
            // 3) Return in try or catch block of a try-finally or try-catch-finally
            // 4) Exception in try block of a try-finally
            // 5) Exception in catch block of a try-catch-finally
            // When analyzing a control flow graph that starts inside a finally block we want to consider all
            // five possibilities above. However, when analyzing a control flow graph that starts outside (past)
            // the finally block, we only want to consider the first two (if we're past a finally block then it
            // must have completed normally). Likewise, when analyzing a control flow graph from return statements
            // in try or catch blocks in an IIFE, we only want to consider the third. To make this possible, we
            // inject a ReduceLabel node into the control flow graph. This node contains an alternate reduced
            // set of antecedents for the pre-finally label. As control flow analysis passes by a ReduceLabel
            // node, the pre-finally label is temporarily switched to the reduced antecedent set.
            let finally_label = self.create_branch_label();
            let normal_antecedents = self.flow(normal_exit_label).antecedents;
            let exception_antecedents = self.flow(exception_label).antecedents;
            let return_antecedents = self.flow(return_label).antecedents;
            let exception_and_return = self.combine_flow_lists(exception_antecedents, return_antecedents);
            let combined = self.combine_flow_lists(normal_antecedents, exception_and_return);
            self.flow_mut(finally_label).antecedents = combined;
            self.current_flow = finally_label;
            self.bind(Some(finally_block));
            if self.flow(self.current_flow).flags.intersects(FlowFlags::UNREACHABLE) {
                // If the end of the finally block is unreachable, the end of the entire try statement is unreachable.
                self.current_flow = self.unreachable_flow;
            } else {
                // If we have an IIFE return target and return statements in the try or catch blocks, add a control
                // flow that goes back through the finally block and back through only the return statements.
                if let Some(current_return_target) = self.current_return_target {
                    let return_antecedents = self.flow(return_label).antecedents;
                    if return_antecedents.is_some() {
                        let reduce =
                            self.create_reduce_label(finally_label, return_antecedents, self.current_flow);
                        self.add_antecedent(current_return_target, reduce);
                    }
                }
                // If we have an outer exception target (i.e. a containing try-finally or try-catch-finally), add a
                // control flow that goes back through the finally block and back through each possible exception source.
                if let Some(current_exception_target) = self.current_exception_target {
                    let exception_antecedents = self.flow(exception_label).antecedents;
                    if exception_antecedents.is_some() {
                        let reduce =
                            self.create_reduce_label(finally_label, exception_antecedents, self.current_flow);
                        self.add_antecedent(current_exception_target, reduce);
                    }
                }
                // If the end of the finally block is reachable, but the end of the try and catch blocks are not,
                // convert the current flow to unreachable. For example, 'try { return 1; } finally { ... }' should
                // result in an unreachable current control flow.
                let normal_antecedents = self.flow(normal_exit_label).antecedents;
                if normal_antecedents.is_some() {
                    self.current_flow =
                        self.create_reduce_label(finally_label, normal_antecedents, self.current_flow);
                } else {
                    self.current_flow = self.unreachable_flow;
                }
            }
        } else {
            self.current_flow = self.finish_flow_label(normal_exit_label);
        }
    }

    /// Go: `func (b *Binder) bindSwitchStatement(node *ast.Node)`.
    fn bind_switch_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_switch_statement().expect("SwitchStatement data").clone();
        let post_switch_label = self.create_branch_label();
        self.bind(Some(stmt.expression));
        let save_break_target = self.current_break_target;
        let save_pre_switch_case_flow = self.pre_switch_case_flow;
        self.current_break_target = Some(post_switch_label);
        self.pre_switch_case_flow = self.current_flow;
        self.bind(Some(stmt.case_block));
        self.add_antecedent(post_switch_label, self.current_flow);
        let clauses: Vec<NodeId> = match &self.file.node(stmt.case_block).data {
            tsc_ast::NodeData::CaseBlock(d) => d
                .clauses
                .as_ref()
                .map(|list| list.nodes.to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let has_default = clauses
            .iter()
            .any(|&c| self.file.node(c).kind == Kind::DefaultClause);
        if !has_default {
            let pre = self.pre_switch_case_flow;
            let clause = self.create_flow_switch_clause(pre, node, 0, 0);
            self.add_antecedent(post_switch_label, clause);
        }
        self.current_break_target = save_break_target;
        self.pre_switch_case_flow = save_pre_switch_case_flow;
        self.current_flow = self.finish_flow_label(post_switch_label);
    }

    /// Go: `func (b *Binder) bindCaseBlock(node *ast.Node)`.
    fn bind_case_block(&mut self, node: NodeId) {
        let switch_statement = crate::ast_shims::parent_of(&*self.file, node).expect("CaseBlock parent");
        let clauses: Vec<NodeId> = match &self.file.node(node).data {
            tsc_ast::NodeData::CaseBlock(d) => d
                .clauses
                .as_ref()
                .map(|list| list.nodes.to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let switch_expression = node_data::expression_of(&*self.file, switch_statement)
            .expect("SwitchStatement expression");
        let is_narrowing_switch = self.file.node(switch_expression).kind == Kind::TrueKeyword
            || is_narrowing_expression(&*self.file, switch_expression);
        let mut fallthrough_flow = self.unreachable_flow;
        let mut i = 0usize;
        while i < clauses.len() {
            let clause_start = i;
            while i + 1 < clauses.len() {
                let current = clauses[i];
                let statements_empty = node_data::statements_of(&*self.file, current)
                    .map(|l| l.nodes.is_empty())
                    .unwrap_or(true);
                if !statements_empty {
                    break;
                }
                if fallthrough_flow == self.unreachable_flow {
                    self.current_flow = self.pre_switch_case_flow;
                }
                self.bind(Some(current));
                i += 1;
            }
            let pre_case_label = self.create_branch_label();
            let mut pre_case_flow = self.pre_switch_case_flow;
            if is_narrowing_switch {
                pre_case_flow = self.create_flow_switch_clause(
                    self.pre_switch_case_flow,
                    switch_statement,
                    clause_start as i32,
                    (i + 1) as i32,
                );
            }
            self.add_antecedent(pre_case_label, pre_case_flow);
            self.add_antecedent(pre_case_label, fallthrough_flow);
            self.current_flow = self.finish_flow_label(pre_case_label);
            let clause = clauses[i];
            self.bind(Some(clause));
            fallthrough_flow = self.current_flow;
            if !self.flow(self.current_flow).flags.intersects(FlowFlags::UNREACHABLE)
                && i != clauses.len() - 1
            {
                node_data::set_fallthrough_flow_node(&mut *self.file, clause, Some(self.current_flow));
            }
            i += 1;
        }
    }

    /// Go: `func (b *Binder) bindCaseOrDefaultClause(node *ast.Node)`.
    fn bind_case_or_default_clause(&mut self, node: NodeId) {
        let clause = self
            .file
            .node(node)
            .as_case_or_default_clause()
            .expect("CaseOrDefaultClause data")
            .clone();
        if clause.expression != NodeId::NONE {
            let save_current_flow = self.current_flow;
            self.current_flow = self.pre_switch_case_flow;
            self.bind(Some(clause.expression));
            self.current_flow = save_current_flow;
        }
        let statements = clause.statements.as_ref().map(|l| l.nodes.to_vec());
        if let Some(statements) = statements {
            self.bind_each(&statements);
        }
    }

    /// Go: `func (b *Binder) bindExpressionStatement(node *ast.Node)`.
    fn bind_expression_statement(&mut self, node: NodeId) {
        let expression = node_data::expression_of(&*self.file, node).expect("ExpressionStatement expression");
        self.bind(Some(expression));
        self.maybe_bind_expression_flow_if_call(expression);
    }

    /// Go: `func (b *Binder) maybeBindExpressionFlowIfCall(node *ast.Node)`.
    fn maybe_bind_expression_flow_if_call(&mut self, node: NodeId) {
        // A top level or comma expression call expression with a dotted function name and at least one argument
        // is potentially an assertion and is therefore included in the control flow.
        if tsc_ast::is_call_expression(self.file.node(node)) {
            let expression = node_data::expression_of(&*self.file, node).expect("CallExpression expression");
            if self.file.node(expression).kind != Kind::SuperKeyword
                && is_dotted_name(&*self.file, expression)
            {
                let current = self.current_flow;
                self.current_flow = self.create_flow_call(current, node);
            }
        }
    }

    /// Go: `func (b *Binder) bindLabeledStatement(node *ast.Node)`.
    fn bind_labeled_statement(&mut self, node: NodeId) {
        let stmt = self.file.node(node).as_labeled_statement().expect("LabeledStatement data").clone();
        let post_statement_label = self.create_branch_label();
        self.active_label_list = Some(Box::new(ActiveLabel {
            next: std::mem::take(&mut self.active_label_list),
            name: node_text(&*self.file, stmt.label).to_string(),
            break_target: Some(post_statement_label),
            continue_target: None,
            referenced: false,
        }));
        self.bind(Some(stmt.label));
        self.bind(Some(stmt.statement));
        if !self
            .active_label_list
            .as_deref()
            .expect("pushed above")
            .referenced
        {
            // Mark the label as unused; the checker will decide whether to report it
            self.file.node_mut(stmt.label).flags |= NodeFlags::UNREACHABLE;
        }
        let label = self.active_label_list.take().expect("pushed above");
        self.active_label_list = label.next;
        self.add_antecedent(post_statement_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_statement_label);
    }

    /// Go: `func (b *Binder) bindPrefixUnaryExpressionFlow(node *ast.Node)`.
    fn bind_prefix_unary_expression_flow(&mut self, node: NodeId) {
        let expr = self
            .file
            .node(node)
            .as_prefix_unary_expression()
            .expect("PrefixUnaryExpression data")
            .clone();
        if expr.operator == Kind::ExclamationToken {
            let save_true_target = self.current_true_target;
            std::mem::swap(&mut self.current_true_target, &mut self.current_false_target);
            self.bind_each_child(node);
            self.current_false_target = self.current_true_target;
            self.current_true_target = save_true_target;
        } else {
            self.bind_each_child(node);
            if expr.operator == Kind::PlusPlusToken || expr.operator == Kind::MinusMinusToken {
                self.bind_assignment_target_flow(expr.operand);
            }
        }
    }

    /// Go: `func (b *Binder) bindPostfixUnaryExpressionFlow(node *ast.Node)`.
    fn bind_postfix_unary_expression_flow(&mut self, node: NodeId) {
        let expr = self
            .file
            .node(node)
            .as_postfix_unary_expression()
            .expect("PostfixUnaryExpression data")
            .clone();
        self.bind_each_child(node);
        if expr.operator == Kind::PlusPlusToken || expr.operator == Kind::MinusMinusToken {
            self.bind_assignment_target_flow(expr.operand);
        }
    }

    /// Go: `func (b *Binder) bindDestructuringAssignmentFlow(node *ast.Node)`.
    fn bind_destructuring_assignment_flow(&mut self, node: NodeId) {
        let expr = self.file.node(node).as_binary_expression().expect("BinaryExpression data").clone();
        if self.in_assignment_pattern {
            self.in_assignment_pattern = false;
            self.bind(Some(expr.operator_token));
            self.bind(Some(expr.right));
            self.in_assignment_pattern = true;
            self.bind(Some(expr.left));
            self.bind(expr.type_);
        } else {
            self.in_assignment_pattern = true;
            self.bind(Some(expr.left));
            self.bind(expr.type_);
            self.in_assignment_pattern = false;
            self.bind(Some(expr.operator_token));
            self.bind(Some(expr.right));
        }
        self.bind_assignment_target_flow(expr.left);
    }

    /// Go: `func (b *Binder) bindBinaryExpressionFlow(node *ast.Node)`.
    fn bind_binary_expression_flow(&mut self, node: NodeId) {
        let expr = self.file.node(node).as_binary_expression().expect("BinaryExpression data").clone();
        let operator = self.file.node(expr.operator_token).kind;
        if crate::ast_shims::is_logical_or_coalescing_binary_operator(operator)
            || tsc_ast::is_logical_or_coalescing_assignment_operator(operator)
        {
            if is_top_level_logical_expression(&*self.file, node) {
                let post_expression_label = self.create_branch_label();
                let save_current_flow = self.current_flow;
                let save_has_flow_effects = self.has_flow_effects;
                self.has_flow_effects = false;
                self.bind_logical_like_expression(node, Some(post_expression_label), Some(post_expression_label));
                if self.has_flow_effects {
                    self.current_flow = self.finish_flow_label(post_expression_label);
                } else {
                    self.current_flow = save_current_flow;
                }
                self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
            } else {
                self.bind_logical_like_expression(node, self.current_true_target, self.current_false_target);
            }
        } else {
            self.bind(Some(expr.left));
            self.bind(expr.type_);
            if operator == Kind::CommaToken {
                self.maybe_bind_expression_flow_if_call(expr.left);
            }
            self.bind(Some(expr.operator_token));
            self.bind(Some(expr.right));
            if operator == Kind::CommaToken {
                self.maybe_bind_expression_flow_if_call(expr.right);
            }
            if tsc_ast::is_assignment_operator(operator) && !is_assignment_target(&*self.file, node) {
                self.bind_assignment_target_flow(expr.left);
                if operator == Kind::EqualsToken && self.file.node(expr.left).kind == Kind::ElementAccessExpression {
                    let element_access = self
                        .file
                        .node(expr.left)
                        .as_element_access_expression()
                        .expect("ElementAccessExpression data")
                        .clone();
                    if is_narrowable_operand(&*self.file, element_access.expression) {
                        let current = self.current_flow;
                        self.current_flow = self.create_flow_mutation(FlowFlags::ARRAY_MUTATION, current, node);
                    }
                }
            }
        }
    }

    /// Go: `func (b *Binder) bindLogicalLikeExpression(node, trueTarget, falseTarget)`.
    fn bind_logical_like_expression(
        &mut self,
        node: NodeId,
        true_target: Option<FlowNodeId>,
        false_target: Option<FlowNodeId>,
    ) {
        let expr = self.file.node(node).as_binary_expression().expect("BinaryExpression data").clone();
        let pre_right_label = self.create_branch_label();
        let operator = self.file.node(expr.operator_token).kind;
        if operator == Kind::AmpersandAmpersandToken || operator == Kind::AmpersandAmpersandEqualsToken {
            self.bind_condition(Some(expr.left), Some(pre_right_label), false_target);
        } else {
            self.bind_condition(Some(expr.left), true_target, Some(pre_right_label));
        }
        self.current_flow = self.finish_flow_label(pre_right_label);
        self.bind(Some(expr.operator_token));
        if tsc_ast::is_logical_or_coalescing_assignment_operator(operator) {
            self.do_with_conditional_branches(Self::bind, Some(expr.right), true_target, false_target);
            self.bind_assignment_target_flow(expr.left);
            let current = self.current_flow;
            let condition = self.create_flow_condition(FlowFlags::TRUE_CONDITION, current, Some(node));
            self.add_antecedent(true_target.expect("bindLogicalLikeExpression trueTarget"), condition);
            let current = self.current_flow;
            let condition = self.create_flow_condition(FlowFlags::FALSE_CONDITION, current, Some(node));
            self.add_antecedent(false_target.expect("bindLogicalLikeExpression falseTarget"), condition);
        } else {
            self.bind_condition(Some(expr.right), true_target, false_target);
        }
    }

    /// Go: `func (b *Binder) bindDeleteExpressionFlow(node *ast.Node)`.
    fn bind_delete_expression_flow(&mut self, node: NodeId) {
        let expression = node_data::expression_of(&*self.file, node).expect("DeleteExpression expression");
        self.bind_each_child(node);
        if self.file.node(expression).kind == Kind::PropertyAccessExpression {
            self.bind_assignment_target_flow(expression);
        }
    }

    /// Go: `func (b *Binder) bindConditionalExpressionFlow(node *ast.Node)`.
    fn bind_conditional_expression_flow(&mut self, node: NodeId) {
        let expr = self
            .file
            .node(node)
            .as_conditional_expression()
            .expect("ConditionalExpression data")
            .clone();
        let true_label = self.create_branch_label();
        let false_label = self.create_branch_label();
        let post_expression_label = self.create_branch_label();
        let save_current_flow = self.current_flow;
        let save_has_flow_effects = self.has_flow_effects;
        self.has_flow_effects = false;
        self.bind_condition(Some(expr.condition), Some(true_label), Some(false_label));
        self.current_flow = self.finish_flow_label(true_label);
        self.bind(Some(expr.question_token));
        self.bind(Some(expr.when_true));
        self.add_antecedent(post_expression_label, self.current_flow);
        self.current_flow = self.finish_flow_label(false_label);
        self.bind(Some(expr.colon_token));
        self.bind(Some(expr.when_false));
        self.add_antecedent(post_expression_label, self.current_flow);
        if self.has_flow_effects {
            self.current_flow = self.finish_flow_label(post_expression_label);
        } else {
            self.current_flow = save_current_flow;
        }
        self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
    }

    /// Go: `func (b *Binder) bindVariableDeclarationFlow(node *ast.Node)`.
    fn bind_variable_declaration_flow(&mut self, node: NodeId) {
        self.bind_each_child(node);
        let initializer = node_data::initializer_of(&*self.file, node);
        let parent = crate::ast_shims::parent_of(&*self.file, node).expect("VariableDeclaration parent");
        let grandparent = crate::ast_shims::parent_of(&*self.file, parent);
        let in_for_in_or_of = grandparent.is_some_and(|g| is_for_in_or_of_statement(self.file.node(g)));
        if initializer.is_some() || in_for_in_or_of {
            self.bind_initialized_variable_flow(node);
        }
    }

    /// Go: `func (b *Binder) bindInitializedVariableFlow(node *ast.Node)`.
    fn bind_initialized_variable_flow(&mut self, node: NodeId) {
        let name = match self.file.node(node).kind {
            Kind::VariableDeclaration => self.file.node(node).name(),
            Kind::BindingElement => self.file.node(node).name(),
            _ => None,
        };
        if let Some(name) = name {
            if crate::ast_shims::is_binding_pattern(self.file.node(name)) {
                let elements = node_data::elements_of(&*self.file, name).to_vec();
                for child in elements {
                    self.bind_initialized_variable_flow(child);
                }
                return;
            }
        }
        let current = self.current_flow;
        self.current_flow = self.create_flow_mutation(FlowFlags::ASSIGNMENT, current, node);
    }

    /// Go: `func (b *Binder) bindAccessExpressionFlow(node *ast.Node)`.
    fn bind_access_expression_flow(&mut self, node: NodeId) {
        if is_optional_chain(self.file.node(node)) {
            self.bind_optional_chain_flow(node);
        } else {
            self.bind_each_child(node);
        }
    }

    /// Go: `func (b *Binder) bindOptionalChainFlow(node *ast.Node)`.
    fn bind_optional_chain_flow(&mut self, node: NodeId) {
        if is_top_level_logical_expression(&*self.file, node) {
            let post_expression_label = self.create_branch_label();
            let save_current_flow = self.current_flow;
            let save_has_flow_effects = self.has_flow_effects;
            self.bind_optional_chain(node, Some(post_expression_label), Some(post_expression_label));
            if self.has_flow_effects {
                self.current_flow = self.finish_flow_label(post_expression_label);
            } else {
                self.current_flow = save_current_flow;
            }
            self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
        } else {
            self.bind_optional_chain(node, self.current_true_target, self.current_false_target);
        }
    }

    /// Go: `func (b *Binder) bindOptionalChain(node, trueTarget, falseTarget)`.
    fn bind_optional_chain(&mut self, node: NodeId, true_target: Option<FlowNodeId>, false_target: Option<FlowNodeId>) {
        // For an optional chain, we emulate the behavior of a logical expression:
        //
        // a?.b         -> a && a.b
        // a?.b.c       -> a && a.b.c
        // a?.b?.c      -> a && a.b && a.b.c
        // a?.[x = 1]   -> a && a[x = 1]
        //
        // To do this we descend through the chain until we reach the root of a chain (the expression with a `?.`)
        // and build it's CFA graph as if it were the first condition (`a && ...`). Then we bind the rest
        // of the node as part of the "true" branch, and continue to do so as we ascend back up to the outermost
        // chain node. We then treat the entire node as the right side of the expression.
        let mut pre_chain_label: Option<FlowNodeId> = None;
        if is_optional_chain_root(&*self.file, node) {
            pre_chain_label = Some(self.create_branch_label());
        }
        let expression = node_data::expression_of(&*self.file, node).expect("chain expression");
        let chain_true_target = pre_chain_label.or(true_target);
        self.bind_optional_expression(Some(expression), chain_true_target, false_target);
        if let Some(pre_chain_label) = pre_chain_label {
            self.current_flow = self.finish_flow_label(pre_chain_label);
        }
        self.do_with_conditional_branches(
            |b, value| b.bind_optional_chain_rest(value.expect("optional chain node")),
            Some(node),
            true_target,
            false_target,
        );
        if is_outermost_optional_chain(&*self.file, node) {
            let current = self.current_flow;
            let condition = self.create_flow_condition(FlowFlags::TRUE_CONDITION, current, Some(node));
            self.add_antecedent(true_target.expect("optional chain trueTarget"), condition);
            let current = self.current_flow;
            let condition = self.create_flow_condition(FlowFlags::FALSE_CONDITION, current, Some(node));
            self.add_antecedent(false_target.expect("optional chain falseTarget"), condition);
        }
    }

    /// Go: `func (b *Binder) bindOptionalExpression(node, trueTarget, falseTarget)`.
    fn bind_optional_expression(
        &mut self,
        node: Option<NodeId>,
        true_target: Option<FlowNodeId>,
        false_target: Option<FlowNodeId>,
    ) {
        self.do_with_conditional_branches(Self::bind, node, true_target, false_target);
        if let Some(node) = node {
            if !is_optional_chain(self.file.node(node)) || is_outermost_optional_chain(&*self.file, node) {
                let current = self.current_flow;
                let condition = self.create_flow_condition(FlowFlags::TRUE_CONDITION, current, Some(node));
                self.add_antecedent(true_target.expect("bindOptionalExpression trueTarget"), condition);
                let current = self.current_flow;
                let condition = self.create_flow_condition(FlowFlags::FALSE_CONDITION, current, Some(node));
                self.add_antecedent(false_target.expect("bindOptionalExpression falseTarget"), condition);
            }
        }
    }

    /// Go: `func (b *Binder) bindOptionalChainRest(node *ast.Node) bool`.
    fn bind_optional_chain_rest(&mut self, node: NodeId) -> bool {
        match self.file.node(node).kind {
            Kind::PropertyAccessExpression => {
                self.bind(node_data::question_dot_token_of(&*self.file, node));
                self.bind(self.file.node(node).name());
            }
            Kind::ElementAccessExpression => {
                self.bind(node_data::question_dot_token_of(&*self.file, node));
                let argument = self
                    .file
                    .node(node)
                    .as_element_access_expression()
                    .expect("ElementAccessExpression data")
                    .clone()
                    .argument_expression;
                self.bind(Some(argument));
            }
            Kind::CallExpression => {
                self.bind(node_data::question_dot_token_of(&*self.file, node));
                let type_arguments = match &self.file.node(node).data {
                    tsc_ast::NodeData::CallExpression(d) => d.type_arguments.clone(),
                    _ => None,
                };
                self.bind_node_list(type_arguments.as_ref());
                let arguments = node_data::arguments_of(&*self.file, node).to_vec();
                self.bind_each(&arguments);
            }
            _ => {}
        }
        false
    }

    /// Go: `func (b *Binder) bindCallExpressionFlow(node *ast.Node)`.
    fn bind_call_expression_flow(&mut self, node: NodeId) {
        let call = self.file.node(node).as_call_expression().expect("CallExpression data").clone();
        if is_optional_chain(self.file.node(node)) {
            self.bind_optional_chain_flow(node);
        } else {
            // If the target of the call expression is a function expression or arrow function we have
            // an immediately invoked function expression (IIFE). Initialize the flowNode property to
            // the current control flow (which includes evaluation of the IIFE arguments).
            let expr = crate::ast_shims::skip_parentheses(&*self.file, call.expression);
            if self.file.node(expr).kind == Kind::FunctionExpression
                || self.file.node(expr).kind == Kind::ArrowFunction
            {
                self.bind_node_list(call.type_arguments.as_ref());
                let arguments = call.arguments.as_ref().map(|l| l.nodes.to_vec()).unwrap_or_default();
                self.bind_each(&arguments);
                self.bind(Some(call.expression));
            } else {
                self.bind_each_child(node);
                if self.file.node(call.expression).kind == Kind::SuperKeyword {
                    let current = self.current_flow;
                    self.current_flow = self.create_flow_call(current, node);
                }
            }
        }
        if tsc_ast::is_property_access_expression(self.file.node(call.expression)) {
            let access = self
                .file
                .node(call.expression)
                .as_property_access_expression()
                .expect("PropertyAccessExpression data")
                .clone();
            if is_identifier(self.file.node(access.name))
                && is_narrowable_operand(&*self.file, access.expression)
                && is_push_or_unshift_identifier(&*self.file, access.name)
            {
                let current = self.current_flow;
                self.current_flow = self.create_flow_mutation(FlowFlags::ARRAY_MUTATION, current, node);
            }
        }
    }

    /// Go: `func (b *Binder) bindNonNullExpressionFlow(node *ast.Node)`.
    fn bind_non_null_expression_flow(&mut self, node: NodeId) {
        if is_optional_chain(self.file.node(node)) {
            self.bind_optional_chain_flow(node);
        } else {
            self.bind_each_child(node);
        }
    }

    /// Go: `func (b *Binder) bindBindingElementFlow(node *ast.Node)`.
    fn bind_binding_element_flow(&mut self, node: NodeId) {
        // When evaluating a binding pattern, the initializer is evaluated before the binding pattern, per:
        // - https://tc39.es/ecma262/#sec-destructuring-binding-patterns-runtime-semantics-iteratorbindinginitialization
        //   - `BindingElement: BindingPattern Initializer?`
        // - https://tc39.es/ecma262/#sec-runtime-semantics-keyedbindinginitialization
        //   - `BindingElement: BindingPattern Initializer?`
        let elem = self.file.node(node).as_binding_element().expect("BindingElement data").clone();
        self.bind(elem.dot_dot_dot_token);
        self.bind(elem.property_name);
        self.bind_initializer(elem.initializer);
        self.bind(elem.name);
    }

    /// Go: `func (b *Binder) bindParameterFlow(node *ast.Node)` — the leading
    /// `b.bindModifiers(param.Modifiers())` is the modifier node list bind.
    fn bind_parameter_flow(&mut self, node: NodeId) {
        let param = self
            .file
            .node(node)
            .as_parameter_declaration()
            .expect("ParameterDeclaration data")
            .clone();
        if let Some(modifiers) = param.modifiers.as_ref() {
            self.bind_each(&modifiers.nodes);
        }
        self.bind(param.dot_dot_dot_token);
        self.bind(param.question_token);
        self.bind(param.type_);
        self.bind_initializer(param.initializer);
        self.bind(Some(param.name));
    }

    // a BindingElement/Parameter does not have side effects if initializers are not evaluated and used. (see GH#49759)
    /// Go: `func (b *Binder) bindInitializer(node *ast.Node)`.
    fn bind_initializer(&mut self, node: Option<NodeId>) {
        let Some(node) = node else {
            return;
        };
        let entry_flow = self.current_flow;
        self.bind(Some(node));
        if entry_flow == self.unreachable_flow || entry_flow == self.current_flow {
            return;
        }
        let exit_flow = self.create_branch_label();
        self.add_antecedent(exit_flow, entry_flow);
        self.add_antecedent(exit_flow, self.current_flow);
        self.current_flow = self.finish_flow_label(exit_flow);
    }

    // ────────────────────────────────────────────────────────────────────────
    // Diagnostics plumbing (binder.go:2727-2745)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (b *Binder) errorOnNode(node, message, args...)`.
    fn error_on_node(&mut self, node: NodeId, message: &'static Message, args: &[String]) {
        let diagnostic = self.create_diagnostic_for_node(node, message, args);
        self.add_diagnostic(diagnostic);
    }

    /// Go: `func (b *Binder) errorOnFirstToken(node, message, args...)`.
    fn error_on_first_token(&mut self, node: NodeId, message: &'static Message, args: &[String]) {
        let pos = self.file.node(node).pos();
        let span = get_range_of_token_at_position(&*self.file, pos);
        let diagnostic = new_diagnostic(Some(self.file.as_node_id()), span, message, args);
        self.add_diagnostic(diagnostic);
    }

    // Inside the binder, we may create a diagnostic for an as-yet unbound node (with potentially no parent pointers, implying no accessible source file)
    // If so, the node _must_ be in the current file (as that's the only way anything could have traversed to it to yield it as the error node)
    // This version of `createDiagnosticForNode` uses the binder's context to account for this, and always yields correct diagnostics even in these situations.
    /// Go: `func (b *Binder) createDiagnosticForNode(node, message, args...) *ast.Diagnostic`.
    fn create_diagnostic_for_node(&self, node: NodeId, message: &'static Message, args: &[String]) -> Diagnostic {
        let loc = get_error_range_for_node(self.file, node);
        new_diagnostic(Some(self.file.as_node_id()), loc, message, args)
    }

    /// Go: `func (b *Binder) addDiagnostic(diagnostic *ast.Diagnostic)`.
    fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.file.payload_mut().bind_diagnostics.push(diagnostic);
    }
}

/// Go: `ast.NewDiagnostic(b.file, loc, message, args...)` — `StringifyArgs`
/// already ran at the call sites (all binder args are `String`s).
fn new_diagnostic(
    file: Option<NodeId>,
    loc: TextRange,
    message: &'static Message,
    args: &[String],
) -> Diagnostic {
    Diagnostic::new(
        file,
        loc,
        message.code(),
        message.category(),
        "",
        Some(message),
        "",
        message.key(),
        args.to_vec(),
    )
}

// ════════════════════════════════════════════════════════════════════════════
// GetContainerFlags (binder.go:2579-2621)
// ════════════════════════════════════════════════════════════════════════════

/// Go: `func GetContainerFlags(node *ast.Node) ContainerFlags` — store-taking
/// (Go reads `node.Parent` through the pointer graph; the port resolves the
/// parent handle through the store).
pub fn get_container_flags(store: &dyn NodeStore, node: NodeId) -> ContainerFlags {
    let n = store.node(node);
    match n.kind {
        Kind::ClassExpression | Kind::ClassDeclaration | Kind::EnumDeclaration | Kind::ObjectLiteralExpression
        | Kind::TypeLiteral | Kind::JsxAttributes => ContainerFlags::IS_CONTAINER,
        Kind::InterfaceDeclaration => ContainerFlags::IS_CONTAINER.union(ContainerFlags::IS_INTERFACE),
        Kind::ModuleDeclaration | Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration | Kind::MappedType
        | Kind::IndexSignature => ContainerFlags::IS_CONTAINER.union(ContainerFlags::HAS_LOCALS),
        Kind::SourceFile => ContainerFlags::IS_CONTAINER
            .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
            .union(ContainerFlags::HAS_LOCALS),
        Kind::GetAccessor | Kind::SetAccessor | Kind::MethodDeclaration
            if crate::ast_shims::is_object_literal_or_class_expression_method_or_accessor(store, node) =>
        {
            ContainerFlags::IS_CONTAINER
                .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
                .union(ContainerFlags::HAS_LOCALS)
                .union(ContainerFlags::IS_FUNCTION_LIKE)
                .union(ContainerFlags::IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR)
                .union(ContainerFlags::IS_THIS_CONTAINER)
        }
        Kind::GetAccessor | Kind::SetAccessor | Kind::MethodDeclaration | Kind::Constructor
        | Kind::FunctionDeclaration | Kind::ClassStaticBlockDeclaration => {
            ContainerFlags::IS_CONTAINER
                .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
                .union(ContainerFlags::HAS_LOCALS)
                .union(ContainerFlags::IS_FUNCTION_LIKE)
                .union(ContainerFlags::IS_THIS_CONTAINER)
        }
        Kind::MethodSignature | Kind::CallSignature | Kind::FunctionType | Kind::ConstructSignature
        | Kind::ConstructorType => {
            ContainerFlags::IS_CONTAINER
                .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
                .union(ContainerFlags::HAS_LOCALS)
                .union(ContainerFlags::IS_FUNCTION_LIKE)
                .union(ContainerFlags::PROPAGATES_THIS_KEYWORD)
        }
        Kind::FunctionExpression => {
            ContainerFlags::IS_CONTAINER
                .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
                .union(ContainerFlags::HAS_LOCALS)
                .union(ContainerFlags::IS_FUNCTION_LIKE)
                .union(ContainerFlags::IS_FUNCTION_EXPRESSION)
                .union(ContainerFlags::IS_THIS_CONTAINER)
        }
        Kind::ArrowFunction => {
            ContainerFlags::IS_CONTAINER
                .union(ContainerFlags::IS_CONTROL_FLOW_CONTAINER)
                .union(ContainerFlags::HAS_LOCALS)
                .union(ContainerFlags::IS_FUNCTION_LIKE)
                .union(ContainerFlags::IS_FUNCTION_EXPRESSION)
                .union(ContainerFlags::PROPAGATES_THIS_KEYWORD)
        }
        Kind::ModuleBlock => ContainerFlags::IS_CONTROL_FLOW_CONTAINER,
        Kind::PropertyDeclaration => {
            if node_data::initializer_of(store, node).is_some() {
                ContainerFlags::IS_CONTROL_FLOW_CONTAINER.union(ContainerFlags::IS_THIS_CONTAINER)
            } else {
                ContainerFlags::NONE
            }
        }
        Kind::CatchClause | Kind::ForStatement | Kind::ForInStatement | Kind::ForOfStatement | Kind::CaseBlock => {
            ContainerFlags::IS_BLOCK_SCOPED_CONTAINER.union(ContainerFlags::HAS_LOCALS)
        }
        Kind::Block => {
            let parent = n.parent.get();
            let function_like_parent = !parent.is_none()
                && (crate::ast_shims::is_function_like(store.node(parent))
                    || tsc_ast::is_class_static_block_declaration(store.node(parent)));
            if function_like_parent {
                ContainerFlags::NONE
            } else {
                ContainerFlags::IS_BLOCK_SCOPED_CONTAINER.union(ContainerFlags::HAS_LOCALS)
            }
        }
        _ => ContainerFlags::NONE,
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Narrowing predicates (binder.go:2623-2725)
// ════════════════════════════════════════════════════════════════════════════

/// Go: `func isNarrowingExpression(expr *ast.Node) bool`.
fn is_narrowing_expression(store: &dyn NodeStore, expr: NodeId) -> bool {
    match store.node(expr).kind {
        Kind::Identifier | Kind::ThisKeyword => true,
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            contains_narrowable_reference(store, expr)
        }
        Kind::CallExpression => has_narrowable_argument(store, expr),
        Kind::ParenthesizedExpression | Kind::NonNullExpression | Kind::TypeOfExpression => {
            is_narrowing_expression(store, node_data::expression_of(store, expr).expect("expression"))
        }
        Kind::BinaryExpression => {
            let bin = store.node(expr).as_binary_expression().expect("BinaryExpression data");
            is_narrowing_binary_expression(store, bin)
        }
        Kind::PrefixUnaryExpression => {
            let prefix = store.node(expr).as_prefix_unary_expression().expect("PrefixUnaryExpression data");
            prefix.operator == Kind::ExclamationToken
                && is_narrowing_expression(store, prefix.operand)
        }
        _ => false,
    }
}

/// Go: `func containsNarrowableReference(expr *ast.Node) bool`.
fn contains_narrowable_reference(store: &dyn NodeStore, expr: NodeId) -> bool {
    if is_narrowable_reference(store, expr) {
        return true;
    }
    if store.node(expr).flags.intersects(NodeFlags::OPTIONAL_CHAIN) {
        match store.node(expr).kind {
            Kind::PropertyAccessExpression
            | Kind::ElementAccessExpression
            | Kind::CallExpression
            | Kind::NonNullExpression => {
                return contains_narrowable_reference(
                    store,
                    node_data::expression_of(store, expr).expect("expression"),
                );
            }
            _ => {}
        }
    }
    false
}

/// Go: `func isNarrowableReference(node *ast.Node) bool`.
fn is_narrowable_reference(store: &dyn NodeStore, node: NodeId) -> bool {
    match store.node(node).kind {
        Kind::Identifier | Kind::ThisKeyword | Kind::SuperKeyword | Kind::MetaProperty => true,
        Kind::PropertyAccessExpression | Kind::ParenthesizedExpression | Kind::NonNullExpression => {
            is_narrowable_reference(store, node_data::expression_of(store, node).expect("expression"))
        }
        Kind::ElementAccessExpression => {
            let expr = store
                .node(node)
                .as_element_access_expression()
                .expect("ElementAccessExpression data");
            is_string_or_numeric_literal_like(store.node(expr.argument_expression))
                || is_entity_name_expression(store, expr.argument_expression)
                    && is_narrowable_reference(store, expr.expression)
        }
        Kind::BinaryExpression => {
            let expr = store.node(node).as_binary_expression().expect("BinaryExpression data");
            store.node(expr.operator_token).kind == Kind::CommaToken
                && is_narrowable_reference(store, expr.right)
                || tsc_ast::is_assignment_operator(store.node(expr.operator_token).kind)
                    && is_left_hand_side_expression(store, expr.left)
        }
        _ => false,
    }
}

/// Go: `func hasNarrowableArgument(expr *ast.Node) bool`.
fn has_narrowable_argument(store: &dyn NodeStore, expr: NodeId) -> bool {
    let call = store.node(expr).as_call_expression().expect("CallExpression data");
    let arguments: &[NodeId] = call.arguments.as_ref().map(|l| l.nodes.as_ref()).unwrap_or(&[]);
    for &argument in arguments {
        if contains_narrowable_reference(store, argument) {
            return true;
        }
    }
    if tsc_ast::is_property_access_expression(store.node(call.expression))
        && contains_narrowable_reference(
            store,
            node_data::expression_of(store, call.expression).expect("access expression"),
        )
    {
        return true;
    }
    false
}

/// Go: `func isNarrowingBinaryExpression(expr *ast.BinaryExpression) bool`.
fn is_narrowing_binary_expression(store: &dyn NodeStore, expr: &tsc_ast::BinaryExpression) -> bool {
    let operator = store.node(expr.operator_token).kind;
    match operator {
        Kind::EqualsToken
        | Kind::BarBarEqualsToken
        | Kind::AmpersandAmpersandEqualsToken
        | Kind::QuestionQuestionEqualsToken => contains_narrowable_reference(store, expr.left),
        Kind::EqualsEqualsToken
        | Kind::ExclamationEqualsToken
        | Kind::EqualsEqualsEqualsToken
        | Kind::ExclamationEqualsEqualsToken => {
            let left = crate::ast_shims::skip_parentheses(store, expr.left);
            let right = crate::ast_shims::skip_parentheses(store, expr.right);
            is_narrowable_operand(store, left)
                || is_narrowable_operand(store, right)
                || is_narrowing_type_of_operands(store, right, left)
                || is_narrowing_type_of_operands(store, left, right)
                || (is_boolean_literal(store.node(right)) && is_narrowing_expression(store, left)
                    || is_boolean_literal(store.node(left)) && is_narrowing_expression(store, right))
        }
        Kind::InstanceOfKeyword => is_narrowable_operand(store, expr.left),
        Kind::InKeyword => is_narrowing_expression(store, expr.right),
        Kind::CommaToken => is_narrowing_expression(store, expr.right),
        _ => false,
    }
}

/// Go: `func isNarrowableOperand(expr *ast.Node) bool`.
fn is_narrowable_operand(store: &dyn NodeStore, expr: NodeId) -> bool {
    match store.node(expr).kind {
        Kind::ParenthesizedExpression => {
            is_narrowable_operand(store, node_data::expression_of(store, expr).expect("expression"))
        }
        Kind::BinaryExpression => {
            let binary = store.node(expr).as_binary_expression().expect("BinaryExpression data");
            match store.node(binary.operator_token).kind {
                Kind::EqualsToken => is_narrowable_operand(store, binary.left),
                Kind::CommaToken => is_narrowable_operand(store, binary.right),
                _ => contains_narrowable_reference(store, expr),
            }
        }
        _ => contains_narrowable_reference(store, expr),
    }
}

/// Go: `func isNarrowingTypeOfOperands(expr1, expr2 *ast.Node) bool`.
fn is_narrowing_type_of_operands(store: &dyn NodeStore, expr1: NodeId, expr2: NodeId) -> bool {
    tsc_ast::is_type_of_expression(store.node(expr1))
        && is_narrowable_operand(store, node_data::expression_of(store, expr1).expect("TypeOfExpression expression"))
        && is_string_literal_like(store.node(expr2))
}

// ════════════════════════════════════════════════════════════════════════════
// Package-level helpers (binder.go:1077-1119, 1352-1454, 2494-2802)
// ════════════════════════════════════════════════════════════════════════════

/// Go: `func getParentOfPropertyAssignment(node *ast.Node) *ast.Node`.
fn get_parent_of_property_assignment(store: &dyn NodeStore, node: NodeId) -> NodeId {
    match store.node(node).kind {
        Kind::BinaryExpression => {
            let left = store.node(node).as_binary_expression().expect("BinaryExpression data").left;
            node_data::expression_of(store, left).expect("access expression")
        }
        Kind::CallExpression => node_data::arguments_of(store, node)[0],
        _ => panic!("Unhandled case in getParentOfPropertyAssignment"),
    }
}

/// Go: `func getInitializerSymbol(symbol *ast.Symbol) *ast.Symbol` — takes
/// the binder's symbol arena (`self`) since `*Symbol` became SymbolId.
fn get_initializer_symbol(binder: &Binder<'_>, symbol: Option<SymbolId>) -> Option<SymbolId> {
    let symbol = symbol?;
    let store = &*binder.file;
    let declaration = binder.symbols[symbol.0 as usize].value_declaration?;
    // For an assignment 'fn.xxx = ...', where 'fn' is a previously declared function or a previously
    // declared const variable initialized with a function expression or arrow function, we add expando
    // property declarations to the function's symbol. This also applies to class expressions in JS files,
    // and empty object literals in JS files when the declaration doesn't have a type annotation.
    if is_function_declaration(store.node(declaration))
        || tsc_ast::utilities::is_in_js_file(store.node(declaration))
            && is_class_declaration(store.node(declaration))
    {
        return Some(symbol);
    }
    if is_variable_declaration(store.node(declaration)) {
        let parent = crate::ast_shims::parent_of(store, declaration).expect("VariableDeclaration parent");
        let in_const_context = store.node(parent).flags.intersects(NodeFlags::CONST)
            || tsc_ast::utilities::is_in_js_file(store.node(declaration));
        if in_const_context {
            let initializer = node_data::initializer_of(store, declaration);
            if crate::ast_shims::is_expando_initializer(store, declaration, initializer) {
                let initializer = initializer.expect("checked above");
                return node_data::symbol_of(store, initializer);
            }
        }
        return None;
    }
    if is_binary_expression(store.node(declaration))
        && tsc_ast::utilities::is_in_js_file(store.node(declaration))
    {
        let initializer = store
            .node(declaration)
            .as_binary_expression()
            .expect("BinaryExpression data")
            .right;
        if crate::ast_shims::is_expando_initializer(store, declaration, Some(initializer)) {
            return node_data::symbol_of(store, initializer);
        }
    }
    None
}

/// Go: `func isEvalOrArgumentsIdentifier(node *ast.Node) bool`.
fn is_eval_or_arguments_identifier(store: &dyn NodeStore, node: NodeId) -> bool {
    if is_identifier(store.node(node)) {
        let text = node_text(store, node);
        return text == "eval" || text == "arguments";
    }
    false
}

/// Go: `func isUseStrictPrologueDirective(sourceFile, node) bool` —
/// Should be called only on prologue directives (ast.IsPrologueDirective(node) should be true).
fn is_use_strict_prologue_directive(source_file: &SourceFile, node: NodeId) -> bool {
    let expression = node_data::expression_of(source_file, node).expect("ExpressionStatement expression");
    let node_text = tsc_scanner::utilities::get_source_text_of_node_from_source_file(
        source_file,
        expression,
        false, /*includeTrivia*/
    );
    // Note: the node text must be exactly "use strict" or 'use strict'.  It is not ok for
    // the string to contain unicode escapes (as per ES5).
    node_text == "\"use strict\"" || node_text == "'use strict'"
}

/// Go: `func FindUseStrictPrologue(sourceFile *ast.SourceFile, statements []*ast.Node) *ast.Node`.
pub fn find_use_strict_prologue(source_file: &SourceFile, statements: &[NodeId]) -> Option<NodeId> {
    for &statement in statements {
        if crate::ast_shims::is_prologue_directive(source_file, statement) {
            if is_use_strict_prologue_directive(source_file, statement) {
                return Some(statement);
            }
        } else {
            return None;
        }
    }
    None
}

/// Go: `func setReturnFlowNode(node *ast.Node, returnFlowNode *ast.FlowNode)`.
fn set_return_flow_node(store: &mut dyn NodeStore, node: NodeId, return_flow_node: FlowNodeId) {
    node_data::set_return_flow_node(store, node, Some(return_flow_node));
}

/// Go: `func isGeneratorFunctionExpression(node *ast.Node) bool`.
fn is_generator_function_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    is_function_expression(store.node(node))
        && node_data::asterisk_token_of(store, node).is_some()
}

/// Go: `func (b *Binder) addDeclarationToSymbol(symbol *ast.Symbol, node *ast.Node, symbolFlags ast.SymbolFlags)`.
impl<'f> Binder<'f> {
    fn add_declaration_to_symbol(&mut self, symbol: SymbolId, node: NodeId, symbol_flags: SymbolFlags) {
        self.symbols[symbol.0 as usize].flags |= symbol_flags;
        node_data::set_node_symbol(&mut *self.file, node, Some(symbol));
        let declarations = &mut self.symbols[symbol.0 as usize].declarations;
        if declarations.is_empty() {
            // Go: symbol.Declarations = b.newSingleDeclaration(node) (arena
            // of one-element slices; the port keeps the Vec in the symbol).
            declarations.push(node);
        } else if !declarations.contains(&node) {
            declarations.push(node);
        }
        // On merge of const enum module with class or function, reset const enum only flag (namespaces will already recalculate)
        let flags = self.symbols[symbol.0 as usize].flags;
        if flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE)
            && flags.intersects(
                SymbolFlags::FUNCTION.union(SymbolFlags::CLASS).union(SymbolFlags::REGULAR_ENUM),
            )
        {
            self.symbols[symbol.0 as usize].flags =
                SymbolFlags(self.symbols[symbol.0 as usize].flags.0 & !SymbolFlags::CONST_ENUM_ONLY_MODULE.0);
            self.not_const_enum_only_modules.add(symbol);
        }
        if symbol_flags.intersects(SymbolFlags::VALUE) {
            set_value_declaration(self, symbol, node);
        }
    }
}

/// Go: `func SetValueDeclaration(symbol *ast.Symbol, node *ast.Node)`.
fn set_value_declaration(binder: &mut Binder<'_>, symbol: SymbolId, node: NodeId) {
    let value_declaration = binder.symbols[symbol.0 as usize].value_declaration;
    let replace = match value_declaration {
        None => true,
        Some(value_declaration) => {
            is_assignment_declaration(&*binder.file, value_declaration)
                && !is_assignment_declaration(&*binder.file, node)
                || binder.file.node(value_declaration).kind != binder.file.node(node).kind
                    && is_effective_module_declaration(&*binder.file, value_declaration)
        }
    };
    // Non-assignment declarations take precedence over assignment declarations and
    // non-namespace declarations take precedence over namespace declarations.
    if replace {
        binder.symbols[symbol.0 as usize].value_declaration = Some(node);
    }
}

/// Go: `func isAssignmentDeclaration(decl *ast.Node) bool`.
fn is_assignment_declaration(store: &dyn NodeStore, decl: NodeId) -> bool {
    is_binary_expression(store.node(decl))
        || crate::ast_shims::is_access_expression(store.node(decl))
        || is_identifier(store.node(decl))
        || tsc_ast::is_call_expression(store.node(decl))
}

/// Go: `func isEffectiveModuleDeclaration(node *ast.Node) bool`.
fn is_effective_module_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    is_module_declaration(store.node(node)) || is_identifier(store.node(node))
}

/// Go: `func getOptionalSymbolFlagForNode(node *ast.Node) ast.SymbolFlags`.
fn get_optional_symbol_flag_for_node(binder: &Binder<'_>, node: NodeId) -> SymbolFlags {
    let postfix_token = node_data::postfix_token_of(&*binder.file, node);
    let is_question = postfix_token.is_some_and(|t| binder.file.node(t).kind == Kind::QuestionToken);
    if is_question {
        SymbolFlags::OPTIONAL
    } else {
        SymbolFlags::NONE
    }
}

/// Go: `func isFunctionSymbol(symbol *ast.Symbol) bool` (checker-facing; kept
/// package-level like Go).
pub fn is_function_symbol(store: &dyn NodeStore, symbols: &[Symbol], symbol: SymbolId) -> bool {
    let Some(d) = symbols[symbol.0 as usize].value_declaration else {
        return false;
    };
    if is_function_declaration(store.node(d)) {
        return true;
    }
    if is_variable_declaration(store.node(d)) {
        let var_decl = store.node(d).as_variable_declaration().expect("VariableDeclaration data");
        if let Some(initializer) = var_decl.initializer {
            return crate::ast_shims::is_function_like(store.node(initializer));
        }
    }
    false
}

/// Go: `func isStatementCondition(node *ast.Node) bool`.
fn is_statement_condition(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(parent) = crate::ast_shims::parent_of(store, node) else {
        return false;
    };
    match store.node(parent).kind {
        Kind::IfStatement | Kind::WhileStatement | Kind::DoStatement => {
            node_data::expression_of(store, parent) == Some(node)
        }
        Kind::ForStatement => store
            .node(parent)
            .as_for_statement()
            .is_some_and(|d| d.condition == Some(node)),
        Kind::ConditionalExpression => store
            .node(parent)
            .as_conditional_expression()
            .is_some_and(|d| d.condition == node),
        _ => false,
    }
}

/// Go: `func isTopLevelLogicalExpression(node *ast.Node) bool`.
fn is_top_level_logical_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let mut node = node;
    while let Some(parent) = crate::ast_shims::parent_of(store, node) {
        let is_exclam_prefix = tsc_ast::is_prefix_unary_expression(store.node(parent))
            && store
                .node(parent)
                .as_prefix_unary_expression()
                .is_some_and(|d| d.operator == Kind::ExclamationToken);
        if tsc_ast::is_parenthesized_expression(store.node(parent)) || is_exclam_prefix {
            node = parent;
        } else {
            break;
        }
    }
    let Some(parent) = crate::ast_shims::parent_of(store, node) else {
        return true;
    };
    !is_statement_condition(store, node)
        && !crate::ast_shims::is_logical_expression(store, parent)
        && !(is_optional_chain(store.node(parent))
            && node_data::expression_of(store, parent) == Some(node))
}

/// Go: `func isLogicalAssignmentExpression(node *ast.Node) bool`.
fn is_logical_assignment_expression(store: &dyn NodeStore, node: NodeId) -> bool {
    let skipped = crate::ast_shims::skip_parentheses(store, node);
    crate::ast_shims::is_logical_or_coalescing_assignment_expression(store, skipped)
}




