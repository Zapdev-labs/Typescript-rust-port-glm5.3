//! M6 W5 — the JSX checking family (Go `tsc/internal/checker/jsx.go`,
//! 1,489 lines) + `markJsxAliasReferenced` (Go checker.go:29030) +
//! `getApplicableIndexSymbol` (Go checker.go:32639 — the single caller is
//! jsx.go's `getIntrinsicTagSymbol`; PORT-noted here) + the
//! `getBestMatchIndexedAccessTypeOrUndefined` helper (Go relater.go:618,
//! required by `elaborateJsxComponents`/`elaborateIterableOrArrayLikeTarget-
//! Elementwise`).
//!
//! Go source: `tsc/internal/checker/jsx.go` @ d6ed1c9a2edd6b8caa1d871ce449cffc1f81cc7f,
//! ported function-by-function, same names snake_cased, same control flow
//! and evaluation order.
//!
//! PORT notes (decisions where Rust forces a change):
//!
//!   * `getJsxType` — Go returns `errorType` for a missing namespace member
//!     (never nil), so `get_jsx_type` returns `TypeId` and Go's
//!     `getJsxElementTypeAt(...) == nil` comparisons (checkJsxPreconditions,
//!     checkJsxFragment, getJsxStatelessElementTypeAt) are dead in Go too —
//!     mirrored as no-ops with the Go source kept in comments.
//!
//!   * `getJsxNamespace` (the internal string-valued walk) collides with the
//!     public `GetJsxNamespace` carrier in exports.rs; the internal one is
//!     `get_jsx_namespace_string` (PORT note at the definition).
//!
//!   * `parseIsolatedEntityName` — Go's parser allocates into the shared
//!     global node arena, so the returned entity name is directly usable by
//!     the checker. The Rust parser hands back a node in ITS OWN per-file
//!     store (`tsc_parser::parse_isolated_entity_name` returns
//!     `(SourceFile, NodeId)`), so the port deep-copies the
//!     Identifier/QualifiedName subtree into the program's node store and
//!     marks it synthetic (Go `markAsSynthetic`: loc (-1,-1) recursively —
//!     `TextRange::undefined()` is exactly (-1,-1)). Entity names are
//!     bounded (identifier chains), so the copy is a two-arm recursion.
//!
//!   * `generateJsxChildren` — Go's `iter.Seq[JsxElaborationElement]`
//!     yields lazily, but the only observable behavior is the emitted
//!     sequence (the `memberOffset` skip logic); the port collects the
//!     sequence eagerly (`Vec<JsxElaborationElement>`).
//!
//!   * `JsxElaborationElement.createDiagnostic` — Go carries a closure
//!     (`func(prop) *ast.Diagnostic`) used for the JSX-text-child
//!     "components don't accept text" diagnostic; the port carries the
//!     captured values (tag-name node, children prop name, children target
//!     type) in `invalid_text_child` and builds the diagnostic at the
//!     report site (Go computes the same strings on first call — identical
//!     results whenever called).
//!
//!   * `markAsSynthetic` — Go's `ForEachChild` walk over the full node
//!     graph; the port only ever synthesizes entity names (copied during
//!     `parse_isolated_entity_name` with `undefined()` locs) and the
//!     fragment synthetic-expression argument (parent-loc'd per Go's
//!     `createSyntheticExpression`), so the recursive relabelling is folded
//!     into the copy.

use std::cell::Cell;

use tsc_ast::{
    Kind, Node, NodeData, NodeFlags as NF, NodeId, NodeStore, SymbolFlags, SymbolId,
    SymbolTable, TextRange,
};
use tsc_core::options_generated::JsxEmit;
use tsc_diagnostics as diag;
use tsc_diagnostics::Message;

use crate::checker::Checker;
use crate::relater::RelationKind;
use crate::checker::CheckMode;
use crate::types::{ContextFlags, ObjectFlags, SignatureFlags, TypeFlags};

// ────────────────────────────────────────────────────────────────────────────
// JsxFlags / JsxReferenceKind / JsxElementLinks / JsxNames / ReactNames
// (Go jsx.go:17-62)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type JsxFlags uint32` + consts (jsx.go:19-25).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsxFlags(pub u32);

impl JsxFlags {
    pub const NONE: JsxFlags = JsxFlags(0);
    /// An element from a named property of the JSX.IntrinsicElements interface.
    pub const INTRINSIC_NAMED_ELEMENT: JsxFlags = JsxFlags(1 << 0);
    /// An element inferred from the string index signature of the
    /// JSX.IntrinsicElements interface.
    pub const INTRINSIC_INDEXED_ELEMENT: JsxFlags = JsxFlags(1 << 1);
    pub const INTRINSIC_ELEMENT: JsxFlags =
        JsxFlags(Self::INTRINSIC_NAMED_ELEMENT.0 | Self::INTRINSIC_INDEXED_ELEMENT.0);

    pub fn intersects(self, other: JsxFlags) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for JsxFlags {
    type Output = JsxFlags;
    fn bitor(self, rhs: JsxFlags) -> JsxFlags {
        JsxFlags(self.0 | rhs.0)
    }
}
impl std::ops::BitOrAssign for JsxFlags {
    fn bitor_assign(&mut self, rhs: JsxFlags) {
        self.0 |= rhs.0;
    }
}

/// Go: `type JsxReferenceKind int32` + iota consts (jsx.go:27-33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum JsxReferenceKind {
    Component = 0,
    Function = 1,
    Mixed = 2,
}

/// Go: `type JsxElementLinks struct` (jsx.go:35-42) — the real shape
/// (replaces todo_shapes.rs's unit placeholder; the store lands on
/// `Checker::jsx_element_links`).
#[derive(Debug, Default)]
pub struct JsxElementLinks {
    /// Flags for the JSX element.
    pub jsx_flags: JsxFlags,
    /// Resolved element attributes type of a JSX opening-like element.
    pub resolved_jsx_element_attributes_type: Option<crate::types::TypeId>,
    /// Resolved JSX namespace symbol for this node.
    pub jsx_namespace: Option<SymbolId>,
    /// Resolved module symbol the implicit JSX import of this file should
    /// refer to.
    pub jsx_implicit_import_container: Option<SymbolId>,
    /// The first JSX tag in the file.
    pub first_jsx_tag_in_file: Option<NodeId>,
}

/// Go: `var JsxNames = struct{...}` (jsx.go:44-58).
pub mod jsx_names {
    pub const JSX: &str = "JSX";
    pub const INTRINSIC_ELEMENTS: &str = "IntrinsicElements";
    pub const ELEMENT_CLASS: &str = "ElementClass";
    pub const ELEMENT_ATTRIBUTES_PROPERTY_NAME_CONTAINER: &str = "ElementAttributesProperty";
    pub const ELEMENT_CHILDREN_ATTRIBUTE_NAME_CONTAINER: &str = "ElementChildrenAttribute";
    pub const ELEMENT: &str = "Element";
    pub const ELEMENT_TYPE: &str = "ElementType";
    pub const INTRINSIC_ATTRIBUTES: &str = "IntrinsicAttributes";
    pub const INTRINSIC_CLASS_ATTRIBUTES: &str = "IntrinsicClassAttributes";
    pub const LIBRARY_MANAGED_ATTRIBUTES: &str = "LibraryManagedAttributes";
}

/// Go: `var ReactNames = struct{...}` (jsx.go:60-64).
mod react_names {
    pub const FRAGMENT: &str = "Fragment";
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go ast/utilities.go's JSX readers + the jsx-local shims)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetSemanticJsxChildren(children) []*JsxChild` (ast/utilities.go:3865).
pub(crate) fn get_semantic_jsx_children(store: &dyn NodeStore, children: &[NodeId]) -> Vec<NodeId> {
    children
        .iter()
        .copied()
        .filter(|&child| {
            let n = store.node(child);
            match n.kind {
                Kind::JsxExpression => n.as_jsx_expression().is_some_and(|d| d.expression.is_some()),
                Kind::JsxText => !n
                    .as_jsx_text()
                    .expect("JsxText data")
                    .contains_only_trivia_white_spaces,
                _ => true,
            }
        })
        .collect()
}

/// Go: `func (c *Checker) markAsSynthetic(node) bool` (jsx.go:1449) — the
/// relabelling is folded into the entity-name copy: the synthetic nodes are
/// created with `TextRange::undefined()` locs (== Go's (-1,-1)) at the copy
/// sites (see the module PORT note); no separate walk is needed.
//
// ────────────────────────────────────────────────────────────────────────────
// The `Node.TagName()` reads for the JSX opening/closing kinds
// (Go `NodeData` accessors).
pub(crate) fn jsx_tag_name_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::JsxOpeningElement => Some(n.as_jsx_opening_element().expect("data").tag_name),
        Kind::JsxSelfClosingElement => Some(n.as_jsx_self_closing_element().expect("data").tag_name),
        Kind::JsxClosingElement => Some(n.as_jsx_closing_element().expect("data").tag_name),
        _ => None,
    }
}

/// The `Node.Attributes()` read (JsxOpeningElement/JsxSelfClosingElement).
pub(crate) fn jsx_attributes_of(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::JsxOpeningElement => Some(n.as_jsx_opening_element().expect("data").attributes),
        Kind::JsxSelfClosingElement => Some(n.as_jsx_self_closing_element().expect("data").attributes),
        _ => None,
    }
}

/// The `Node.Children()` read (JsxElement/JsxFragment).
pub(crate) fn jsx_children_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    match n.kind {
        Kind::JsxElement => n
            .as_jsx_element()
            .expect("data")
            .children
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        Kind::JsxFragment => n
            .as_jsx_fragment()
            .expect("data")
            .children
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The `Node.Properties()` read for JsxAttributes + the JSX opening-like
/// kinds (Go `Node.Properties()` resolves the attributes list).
pub(crate) fn jsx_properties_of(store: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = store.node(node);
    if n.kind == Kind::JsxAttributes {
        return n
            .as_jsx_attributes()
            .expect("data")
            .properties
            .as_ref()
            .map(|l| l.nodes.to_vec())
            .unwrap_or_default();
    }
    // JsxOpeningLikeElement.Properties() — Go reads Attributes().Properties().
    match jsx_attributes_of(store, node) {
        Some(attributes) => jsx_properties_of(store, attributes),
        None => Vec::new(),
    }
}

/// Go: `func GetPragmaFromSourceFile(file, name) *Pragma` (ast) — the
/// checker-local twin of the compiler crate's shim (last-one-wins).
pub(crate) fn get_pragma_from_source_file(
    file: &tsc_ast::SourceFile,
    name: &str,
) -> Option<tsc_ast::Pragma> {
    let mut result = None;
    for pragma in &file.payload().pragmas {
        if pragma.name == name {
            result = Some(pragma.clone());
        }
    }
    result
}

/// Go: `func GetFirstIdentifier(entity *ast.EntityName) *ast.Node` (ast).
pub(crate) fn get_first_identifier(store: &dyn NodeStore, entity: NodeId) -> NodeId {
    let mut node = entity;
    loop {
        let n = store.node(node);
        if n.kind == Kind::QualifiedName {
            node = n.as_qualified_name().expect("data").left;
        } else {
            return node;
        }
    }
}

/// Go: `ast.IsJsxAttributeLike(node)` — JsxAttribute | JsxSpreadAttribute.
pub(crate) fn is_jsx_attribute_like(n: &Node) -> bool {
    matches!(n.kind, Kind::JsxAttribute | Kind::JsxSpreadAttribute)
}

// ────────────────────────────────────────────────────────────────────────────
// The check family (Go jsx.go:67-140)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) checkJsxElement(node, checkMode) *Type` (jsx.go:67).
    pub(crate) fn check_jsx_element(&mut self, node: NodeId, check_mode: CheckMode) -> crate::types::TypeId {
        let _ = check_mode;
        self.check_node_deferred(node);
        self.get_jsx_element_type_at(node)
    }

    /// Go: `func (c *Checker) checkJsxElementDeferred(node)` (jsx.go:72).
    pub(crate) fn check_jsx_element_deferred(&mut self, node: NodeId) {
        let (opening_element, closing_element) = {
            let d = self.store().node(node).as_jsx_element().expect("JsxElement data");
            (d.opening_element, d.closing_element)
        };
        self.check_jsx_opening_like_element_or_opening_fragment(opening_element);
        // Perform resolution on the closing tag so that rename/go to
        // definition/etc work (Go comment).
        let closing_tag_name = jsx_tag_name_of(self.store(), closing_element)
            .expect("JsxClosingElement has a tag name");
        if crate::utilities::is_jsx_intrinsic_tag_name(self.store(), closing_tag_name) {
            self.get_intrinsic_tag_symbol(closing_element);
        } else {
            let _ = self.check_expression(closing_tag_name);
        }
        self.check_jsx_children(node, CheckMode::CHECK_MODE_NORMAL);
    }

    /// Go: `func (c *Checker) checkJsxExpression(node, checkMode) *Type` (jsx.go:84).
    pub(crate) fn check_jsx_expression(&mut self, node: NodeId, check_mode: CheckMode) -> crate::types::TypeId {
        self.check_grammar_jsx_expression(node);
        let expression = self.store().node(node).as_jsx_expression().expect("data").expression;
        let Some(expression) = expression else {
            return self.error_type();
        };
        let t = self.check_expression_ex(expression, check_mode);
        let dot_dot_dot = self
            .store()
            .node(node)
            .as_jsx_expression()
            .expect("data")
            .dot_dot_dot_token
            .is_some();
        // Go: `t != c.anyType` — IDENTITY, not a flags test: the error type
        // carries TypeFlagsAny, so a flag-based isTypeAny would wrongly
        // exempt errorType (the missing-JSX-Element spread child must fire
        // TS2609 exactly as Go). PORT(FCS tranche 48).
        if dot_dot_dot && t != self.any_type() && !self.is_array_type(t) {
            self.error(node, &diag::JSX_SPREAD_CHILD_MUST_BE_AN_ARRAY_TYPE, &[]);
        }
        t
    }

    /// Go: `func (c *Checker) checkJsxSelfClosingElement(node, checkMode) *Type` (jsx.go:96).
    pub(crate) fn check_jsx_self_closing_element(
        &mut self,
        node: NodeId,
        check_mode: CheckMode,
    ) -> crate::types::TypeId {
        let _ = check_mode;
        self.check_node_deferred(node);
        self.get_jsx_element_type_at(node)
    }

    /// Go: `func (c *Checker) checkJsxSelfClosingElementDeferred(node)` (jsx.go:101).
    pub(crate) fn check_jsx_self_closing_element_deferred(&mut self, node: NodeId) {
        self.check_jsx_opening_like_element_or_opening_fragment(node);
    }

    /// Go: `func (c *Checker) checkJsxFragment(node) *Type` (jsx.go:105).
    pub(crate) fn check_jsx_fragment(&mut self, node: NodeId) -> crate::types::TypeId {
        let opening_fragment = self
            .store()
            .node(node)
            .as_jsx_fragment()
            .expect("data")
            .opening_fragment;
        self.check_jsx_opening_like_element_or_opening_fragment(opening_fragment);
        // by default, jsx:'react' will use jsxFactory = React.createElement and
        // jsxFragmentFactory = React.Fragment. if jsxFactory compiler option is
        // provided, ensure jsxFragmentFactory compiler option or @jsxFrag pragma
        // is provided too (Go comment).
        let node_source_file =
            tsc_ast::utilities::get_source_file_of_node(self.store(), node);
        if self.compiler_options.get_jsx_transform_enabled()
            && (!self.compiler_options.jsx_factory.is_empty()
                || node_source_file
                    .as_ref()
                    .is_some_and(|f| get_pragma_from_source_file(f, "jsx").is_some()))
            && self.compiler_options.jsx_fragment_factory.is_empty()
            && node_source_file
                .as_ref()
                .is_none_or(|f| get_pragma_from_source_file(f, "jsxfrag").is_none())
        {
            let message: &'static Message =
                if !self.compiler_options.jsx_factory.is_empty() {
                    &diag::THE_JSXFRAGMENTFACTORY_COMPILER_OPTION_MUST_BE_PROVIDED_TO_USE_JSX_FRAGMENTS_WITH_THE_JSXFACTORY_COMPILER_OPTION
                } else {
                    &diag::AN_JSXFRAG_PRAGMA_IS_REQUIRED_WHEN_USING_AN_JSX_PRAGMA_WITH_JSX_FRAGMENTS
                };
            self.error(node, message, &[]);
        }
        self.check_jsx_children(node, CheckMode::CHECK_MODE_NORMAL);
        let t = self.get_jsx_element_type_at(node);
        if self.is_error_type(t) {
            self.any_type()
        } else {
            t
        }
    }

    /// Go: `func (c *Checker) checkJsxAttributes(node, checkMode) *Type` (jsx.go:125).
    pub(crate) fn check_jsx_attributes(&mut self, node: NodeId, check_mode: CheckMode) -> crate::types::TypeId {
        self.check_node_deferred(node);
        let parent = self.store().node(node).parent.get();
        self.create_jsx_attributes_type_from_attributes_property(parent, check_mode)
    }

    /// Go: `func (c *Checker) checkJsxAttribute(node, checkMode) *Type` (jsx.go:1008).
    pub(crate) fn check_jsx_attribute(&mut self, node: NodeId, check_mode: CheckMode) -> crate::types::TypeId {
        let initializer = self.store().node(node).as_jsx_attribute().expect("data").initializer;
        if let Some(initializer) = initializer {
            return self.check_expression_for_mutable_location(initializer, check_mode);
        }
        // <Elem attr /> is sugar for <Elem attr={true} /> (Go comment).
        self.true_type()
    }

    /// Go: `func (c *Checker) checkJsxChildren(node, checkMode) []*Type` (jsx.go:1015).
    pub(crate) fn check_jsx_children(&mut self, node: NodeId, check_mode: CheckMode) -> Vec<crate::types::TypeId> {
        let children = jsx_children_of(self.store(), node);
        let mut child_types: Vec<crate::types::TypeId> = Vec::new();
        for child in children {
            let n = self.store().node(child);
            // In React, JSX text that contains only whitespaces will be
            // ignored so we don't want to type-check that because then type
            // of children property will have constituent of string type
            // (Go comment).
            if n.kind == Kind::JsxText {
                if !n.as_jsx_text().expect("data").contains_only_trivia_white_spaces {
                    child_types.push(self.string_type());
                }
            } else if n.kind == Kind::JsxExpression
                && n.as_jsx_expression().expect("data").expression.is_none()
            {
                // empty jsx expressions don't *really* count as present children.
                continue;
            } else {
                child_types.push(self.check_expression_for_mutable_location(child, check_mode));
            }
        }
        child_types
    }

    // ────────────────────────────────────────────────────────────────────────
    // The opening-like-element resolution driver (Go jsx.go:143-242)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) checkJsxOpeningLikeElementOrOpeningFragment(node)` (jsx.go:143).
    pub(crate) fn check_jsx_opening_like_element_or_opening_fragment(&mut self, node: NodeId) {
        let is_node_opening_like_element =
            tsc_ast::utilities::is_jsx_opening_like_element(self.store().node(node));
        if is_node_opening_like_element {
            self.check_grammar_jsx_element(node);
        }
        self.check_jsx_preconditions(node);
        self.mark_jsx_alias_referenced(node);
        let sig = self.get_resolved_signature(node);
        self.check_deprecated_signature(sig, node);
        if is_node_opening_like_element {
            let element_type_constraint = self.get_jsx_element_type_type_at(node);
            if let Some(element_type_constraint) = element_type_constraint {
                let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
                let tag_type: crate::types::TypeId =
                    if crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name) {
                        // PORT: FCS tranche 52 — Go jsx.go:145 uses
                        // `tagName.Text()` (the DECODED identifier text, so a
                        // tag written `<\u0061-b>` resolves against "a-b" in
                        // IntrinsicElements). `get_text_of_node` slices the
                        // RAW source span and kept the escape, so the tag
                        // name never matched (unicodeEscapesInJsxtags x6).
                        let text = crate::ast_shims::jsx_name_text(self.store(), tag_name);
                        self.get_string_literal_type(&text)
                    } else {
                        self.check_expression(tag_name)
                    };
                let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
                if !self.check_type_related_to_ex(
                    tag_type,
                    element_type_constraint,
                    RelationKind::Assignable,
                    Some(tag_name),
                    Some(&diag::ITS_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT_TYPE),
                    Some(&mut diags),
                ) {
                    let tag_name_text =
                        crate::ast_shims::get_text_of_node(self.store(), tag_name);
                    let diagnostic = crate::utilities::new_diagnostic_chain_for_node(
                        diags.first(),
                        self.store(),
                        tag_name,
                        &diag::X_0_CANNOT_BE_USED_AS_A_JSX_COMPONENT,
                        vec![tag_name_text],
                    );
                    self.add_diagnostic(diagnostic);
                }
            } else {
                let return_type = sig
                    .map(|s| self.get_return_type_of_signature(s))
                    .unwrap_or_else(|| self.any_type());
                let reference_kind = self.get_jsx_reference_kind(node);
                self.check_jsx_return_assignable_to_appropriate_bound(
                    reference_kind,
                    return_type,
                    node,
                );
            }
        }
    }

    /// Go: `func (c *Checker) checkJsxPreconditions(errorNode)` (jsx.go:174).
    fn check_jsx_preconditions(&mut self, error_node: NodeId) {
        // Preconditions for using JSX.
        if self.compiler_options.jsx == JsxEmit::None {
            self.error(error_node, &diag::CANNOT_USE_JSX_UNLESS_THE_JSX_FLAG_IS_PROVIDED, &[]);
        }
        // Go: `c.getJsxElementTypeAt(errorNode) == nil` — dead in Go too
        // (getJsxType never returns nil; it returns errorType); mirrored as
        // the isErrorType probe on the same value.
        let _ = self.no_implicit_any && {
            let t = self.get_jsx_element_type_at(error_node);
            self.is_error_type(t)
        };
    }

    /// Go: `func (c *Checker) checkJsxReturnAssignableToAppropriateBound(refKind, elemInstanceType, openingLikeElement)` (jsx.go:184).
    fn check_jsx_return_assignable_to_appropriate_bound(
        &mut self,
        ref_kind: JsxReferenceKind,
        elem_instance_type: crate::types::TypeId,
        opening_like_element: NodeId,
    ) {
        let mut diags: Vec<tsc_ast::Diagnostic> = Vec::new();
        match ref_kind {
            JsxReferenceKind::Function => {
                let sfc_return_constraint = self.get_jsx_stateless_element_type_at(opening_like_element);
                if let Some(sfc_return_constraint) = sfc_return_constraint {
                    let tag_name = jsx_tag_name_of(self.store(), opening_like_element)
                        .expect("tag name");
                    self.check_type_related_to_ex(
                        elem_instance_type,
                        sfc_return_constraint,
                        RelationKind::Assignable,
                        Some(tag_name),
                        Some(&diag::ITS_RETURN_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT),
                        Some(&mut diags),
                    );
                }
            }
            JsxReferenceKind::Component => {
                let class_constraint = self.get_jsx_element_class_type_at(opening_like_element);
                if let Some(class_constraint) = class_constraint {
                    // Issue an error if this return type isn't assignable to
                    // JSX.ElementClass, failing that (Go comment).
                    let tag_name = jsx_tag_name_of(self.store(), opening_like_element)
                        .expect("tag name");
                    self.check_type_related_to_ex(
                        elem_instance_type,
                        class_constraint,
                        RelationKind::Assignable,
                        Some(tag_name),
                        Some(&diag::ITS_INSTANCE_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT),
                        Some(&mut diags),
                    );
                }
            }
            _ => {
                let sfc_return_constraint = self.get_jsx_stateless_element_type_at(opening_like_element);
                let class_constraint = self.get_jsx_element_class_type_at(opening_like_element);
                if sfc_return_constraint.is_none() || class_constraint.is_none() {
                    return;
                }
                let combined = self.get_union_type(&[
                    sfc_return_constraint.expect("checked above"),
                    class_constraint.expect("checked above"),
                ]);
                let tag_name = jsx_tag_name_of(self.store(), opening_like_element)
                    .expect("tag name");
                self.check_type_related_to_ex(
                    elem_instance_type,
                    combined,
                    RelationKind::Assignable,
                    Some(tag_name),
                    Some(&diag::ITS_ELEMENT_TYPE_0_IS_NOT_A_VALID_JSX_ELEMENT),
                    Some(&mut diags),
                );
            }
        }
        if !diags.is_empty() {
            let tag_name =
                jsx_tag_name_of(self.store(), opening_like_element).expect("tag name");
            let tag_name_text = crate::ast_shims::get_text_of_node(self.store(), tag_name);
            let diagnostic = crate::utilities::new_diagnostic_chain_for_node(
                diags.first(),
                self.store(),
                tag_name,
                &diag::X_0_CANNOT_BE_USED_AS_A_JSX_COMPONENT,
                vec![tag_name_text],
            );
            self.add_diagnostic(diagnostic);
        }
    }

    /// Go: `func (c *Checker) markJsxAliasReferenced(node)` (checker.go:29030).
    fn mark_jsx_alias_referenced(&mut self, node: NodeId) {
        if self.get_jsx_namespace_container_for_implicit_import(Some(node)).is_some() {
            return;
        }
        // The reactNamespace/jsxFactory's root symbol should be marked as
        // 'used' so we don't incorrectly elide its import. And if there is
        // no reactNamespace/jsxFactory's symbol in scope when targeting
        // React emit, we should issue an error (Go comment).
        let jsx_factory_ref_err: Option<&'static Message> =
            if self.compiler_options.jsx == JsxEmit::React {
                Some(&diag::THIS_JSX_TAG_REQUIRES_0_TO_BE_IN_SCOPE_BUT_IT_COULD_NOT_BE_FOUND)
            } else {
                None
            };
        let jsx_factory_namespace = self.get_jsx_namespace_string(Some(node));
        let jsx_factory_location = if tsc_ast::utilities::is_jsx_opening_like_element(self.store().node(node)) {
            jsx_tag_name_of(self.store(), node).unwrap_or(node)
        } else {
            node
        };
        let should_factory_ref_err = self.compiler_options.jsx != JsxEmit::Preserve
            && self.compiler_options.jsx != JsxEmit::ReactNative;
        // #38720/60122, allow null as jsxFragmentFactory.
        let mut jsx_factory_sym = None;
        let is_opening_fragment_with_null = self.store().node(node).kind == Kind::JsxOpeningFragment
            && jsx_factory_namespace == "null";
        if !is_opening_fragment_with_null {
            let mut flags = SymbolFlags::VALUE;
            if !should_factory_ref_err {
                flags &= !SymbolFlags::ENUM;
            }
            jsx_factory_sym = self.resolve_name_worker(
                Some(jsx_factory_location),
                &jsx_factory_namespace,
                flags,
                jsx_factory_ref_err,
                true,  /*isUse*/
                false, /*excludeGlobals*/
            );
        }
        if let Some(jsx_factory_sym) = jsx_factory_sym {
            // Mark local symbol as referenced here because it might not
            // have been marked if jsx emit was not jsxFactory as there wont
            // be error being emitted (Go comment).
            self.symbol_referenced(jsx_factory_sym, SymbolFlags::ALL);
            // If react/jsxFactory symbol is alias, mark it as referenced.
            if self.can_collect_symbol_alias_accessibility_data
                && self.program_symbol(jsx_factory_sym).flags.intersects(SymbolFlags::ALIAS)
                && self
                    .get_type_only_alias_declaration_real(jsx_factory_sym)
                    .is_none()
            {
                self.mark_alias_symbol_as_referenced(jsx_factory_sym);
            }
        }
        // if JsxFragment, additionally mark jsx pragma as referenced, since
        // `getJsxNamespace` above would have resolved to only the fragment
        // factory if they are distinct (Go comment).
        if self.store().node(node).kind == Kind::JsxOpeningFragment {
            let file_id = tsc_ast::utilities::get_source_file_of_node(self.store(), node)
                .map(|f| f.as_node_id());
            let entity = file_id.map(|f| self.get_jsx_factory_entity(Some(f)));
            if let Some(Some(entity)) = entity {
                let local_jsx_namespace = {
                    let first = get_first_identifier(self.store(), entity);
                    tsc_ast::node_text(self.store(), first).to_string()
                };
                let mut flags = SymbolFlags::VALUE;
                if !should_factory_ref_err {
                    flags &= !SymbolFlags::ENUM;
                }
                self.resolve_name_worker(
                    Some(jsx_factory_location),
                    &local_jsx_namespace,
                    flags,
                    jsx_factory_ref_err,
                    true,  /*isUse*/
                    false, /*excludeGlobals*/
                );
            }
        }
    }

    /// Go: `func (c *Checker) inferJsxTypeArguments(node, signature, checkMode, context) []*Type` (jsx.go:212).
    pub(crate) fn infer_jsx_type_arguments(
        &mut self,
        node: NodeId,
        signature: crate::types::SignatureId,
        check_mode: CheckMode,
        context: crate::todo_shapes::InferenceContextId,
    ) -> Vec<crate::types::TypeId> {
        let param_type = self.get_effective_first_argument_for_jsx_signature(signature, node);
        let attributes = jsx_attributes_of(self.store(), node).expect("attributes");
        let check_attr_type = self.check_expression_with_contextual_type(
            attributes,
            Some(param_type),
            Some(context),
            check_mode,
        );
        self.infer_types(
            context,
            check_attr_type,
            param_type,
            crate::inference::InferencePriority::NONE,
            false,
        );
        self.get_inferred_types(context)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The contextual-typing family (Go jsx.go:246-302)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getContextualTypeForJsxExpression(node, contextFlags) *Type` (jsx.go:246).
    pub(crate) fn get_contextual_type_for_jsx_expression(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<crate::types::TypeId> {
        let parent = self.store().node(node).parent.get();
        let parent_node = self.store().node(parent);
        if is_jsx_attribute_like(parent_node) {
            return self.get_contextual_type(node, context_flags);
        }
        if parent_node.kind == Kind::JsxElement {
            return self.get_contextual_type_for_child_jsx_expression(parent, node, context_flags);
        }
        None
    }

    /// Go: `func (c *Checker) getContextualTypeForJsxAttribute(attribute, contextFlags) *Type` (jsx.go:255).
    pub(crate) fn get_contextual_type_for_jsx_attribute(
        &mut self,
        attribute: NodeId,
        context_flags: ContextFlags,
    ) -> Option<crate::types::TypeId> {
        // When we trying to resolve JsxOpeningLikeElement as a stateless
        // function element, we will already give its attributes a contextual
        // type which is a type of the parameter of the signature we are
        // trying out. If there is no contextual type (e.g. we are trying to
        // resolve stateful component), get attributes type from resolving
        // element's tagName (Go comment).
        if self.store().node(attribute).kind == Kind::JsxAttribute {
            let parent = self.store().node(attribute).parent.get();
            let attributes_type =
                self.get_apparent_type_of_contextual_type(parent, context_flags)?;
            if crate::utilities::is_type_any(Some(self.type_(attributes_type))) {
                return None;
            }
            let name = {
                let d = self.store().node(attribute).as_jsx_attribute().expect("data");
                
                crate::ast_shims::jsx_name_text(self.store(), d.name)
            };
            return self.get_type_of_property_of_contextual_type(attributes_type, &name);
        }
        let parent = self.store().node(attribute).parent.get();
        self.get_contextual_type(parent, context_flags)
    }

    /// Go: `func (c *Checker) getContextualJsxElementAttributesType(node, contextFlags) *Type` (jsx.go:266).
    pub(crate) fn get_contextual_jsx_element_attributes_type(
        &mut self,
        node: NodeId,
        context_flags: ContextFlags,
    ) -> Option<crate::types::TypeId> {
        if self.store().node(node).kind == Kind::JsxOpeningElement
            && context_flags != ContextFlags::IGNORE_NODE_INFERENCES
        {
            let parent = self.store().node(node).parent.get();
            let index =
                self.find_contextual_node(parent, context_flags == ContextFlags::NONE)?;
            // Contextually applied type is moved from attributes up to the
            // outer jsx attributes so when walking up from the children they
            // get hit _However_ to hit them from the _attributes_ we must
            // look for them here; otherwise we'll used the declared type (as
            // below) instead! (Go comment.)
            return self.contextual_infos[index].t;
        }
        self.get_contextual_type_for_argument_at_index(node, 0)
    }

    /// Go: `func (c *Checker) getContextualTypeForChildJsxExpression(node, child, contextFlags) *Type` (jsx.go:279).
    pub(crate) fn get_contextual_type_for_child_jsx_expression(
        &mut self,
        node: NodeId,
        child: NodeId,
        context_flags: ContextFlags,
    ) -> Option<crate::types::TypeId> {
        let opening_element_attributes = {
            let d = self.store().node(node).as_jsx_element().expect("data");
            
            jsx_attributes_of(self.store(), d.opening_element)
                .expect("opening element attributes")
        };
        let attributes_type =
            self.get_apparent_type_of_contextual_type(opening_element_attributes, context_flags);
        // JSX expression is in children of JSX Element, we will look for
        // an "children" attribute (we get the name from
        // JSX.ElementAttributesProperty) (Go comment).
        let jsx_children_property_name = {
            let ns = self.get_jsx_namespace_at(Some(node));
            self.get_jsx_element_children_property_name(ns)
        };
        let attributes_type = attributes_type?;
        if crate::utilities::is_type_any(Some(self.type_(attributes_type))) {
            return None;
        }
        if jsx_children_property_name == tsc_ast::INTERNAL_SYMBOL_NAME_MISSING
            || jsx_children_property_name.is_empty()
        {
            return None;
        }
        let real_children = {
            let children = jsx_children_of(self.store(), node);
            get_semantic_jsx_children(self.store(), &children)
        };
        let child_index = real_children
            .iter()
            .position(|&c| c == child)
            .map(|i| i as i64)
            .unwrap_or(-1);
        let child_field_type =
            self.get_type_of_property_of_contextual_type(attributes_type, &jsx_children_property_name)?;
        if real_children.len() == 1 {
            return Some(child_field_type);
        }
        // Go: mapTypeEx over the array-like constituents with noReductions.
        let number_literal = self.get_number_literal_type(tsc_jsnum::Number(child_index as f64));
        self.map_type_ex(
            child_field_type,
            &mut |c, t| {
                if relater_is_array_like_type(c, t) {
                    Some(relater_get_indexed_access_type(c, t, number_literal))
                } else {
                    Some(t)
                }
            },
            true, /*noReductions*/
        )
    }

    /// Go: `func (c *Checker) discriminateContextualTypeByJSXAttributes(node, contextualType) *Type` (jsx.go:296).
    pub(crate) fn discriminate_contextual_type_by_jsx_attributes(
        &mut self,
        node: NodeId,
        contextual_type: crate::types::TypeId,
    ) -> Option<crate::types::TypeId> {
        let key = (node, contextual_type);
        if let Some(discriminated) = self.discriminated_contextual_types.get(&key) {
            return *discriminated;
        }
        let jsx_children_property_name = {
            let ns = self.get_jsx_namespace_at(Some(node));
            self.get_jsx_element_children_property_name(ns)
        };
        // Go: discriminantProperties — node.Properties() filtered to the
        // JSX attributes whose symbol is a discriminant property of the
        // contextual type (an absent initializer is `true`).
        let properties = jsx_properties_of(self.store(), node);
        let mut discriminant_properties: Vec<NodeId> = Vec::new();
        for p in properties {
            // Go (jsx.go:272): `symbol := p.Symbol(); if symbol == nil ||
            // !ast.IsJsxAttribute(p) { return false }` — the kind guard runs
            // BEFORE the JsxAttribute data is read, so a JsxSpreadAttribute
            // property in the list is simply skipped. PORT (FCS tranche 19):
            // the expect() previously ran BEFORE the kind check, so the
            // newly-live rest-argument contextual pass (getSpreadArgumentType
            // checking a spread JSX-attributes argument) panicked on the
            // spread attribute's node data.
            if self.store().node(p).kind != Kind::JsxAttribute {
                continue;
            }
            let Some(symbol) = self
                .store()
                .node(p)
                .as_jsx_attribute()
                .expect("JsxAttribute data")
                .symbol
            else {
                continue;
            };
            let initializer = self.store().node(p).as_jsx_attribute().expect("data").initializer;
            let possibly_discriminant = match initializer {
                None => true,
                Some(init) => self.is_possibly_discriminant_value(init),
            };
            let name = self.program_symbol(symbol).name.clone();
            if possibly_discriminant
                && crate::relater::is_discriminant_property(self, contextual_type, &name)
            {
                discriminant_properties.push(p);
            }
        }
        // Go: discriminantMembers — the contextual type's optional members
        // absent from the node's symbol's member table, skipping the
        // children property when the containing element has semantic children.
        let node_symbol = self
            .store()
            .node(node)
            .as_jsx_attributes()
            .expect("data")
            .symbol;
        let element = {
            let opening = self.store().node(node).parent.get();
            
            self.store().node(opening).parent.get()
        };
        let element_has_semantic_children = self.store().node(element).kind == Kind::JsxElement
            && !get_semantic_jsx_children(self.store(), &jsx_children_of(self.store(), element))
                .is_empty();
        let node_symbol_members: Vec<(String, SymbolId)> = node_symbol
            .map(|symbol| {
                self.program_symbol(symbol)
                    .members
                    .entries()
                    .map(|(k, &v)| (k.clone(), v))
                    .collect()
            })
            .unwrap_or_default();
        let mut discriminant_members: Vec<SymbolId> = Vec::new();
        if node_symbol.is_some() {
            for s in crate::relater::get_properties_of_type(self, contextual_type) {
                let name = self.program_symbol(s).name.clone();
                if !self.program_symbol(s).flags.intersects(SymbolFlags::OPTIONAL) {
                    continue;
                }
                if name == jsx_children_property_name
                    && element_has_semantic_children
                {
                    continue;
                }
                if node_symbol_members.iter().any(|(k, _)| *k == name) {
                    continue;
                }
                if crate::relater::is_discriminant_property(self, contextual_type, &name) {
                    discriminant_members.push(s);
                }
            }
        }
        // Go: ObjectLiteralDiscriminator{props, members} → the shared
        // discriminateTypeByDiscriminableItems closure triple (identical to
        // the object-literal discriminator's).
        let props = discriminant_properties;
        let members = discriminant_members;
        let (props_snapshot, members_snapshot) = (props.clone(), members.clone());
        let discriminated = crate::relater::discriminate_type_by_discriminable_items_with_closures(
            self,
            contextual_type,
            props_snapshot.len() + members_snapshot.len(),
            |c, index| {
                if index < props_snapshot.len() {
                    let p = props_snapshot[index];
                    let symbol = c
                        .store()
                        .node(p)
                        .as_jsx_attribute()
                        .expect("data")
                        .symbol
                        .expect("attribute symbol");
                    c.program_symbol(symbol).name.clone()
                } else {
                    let s = members_snapshot[index - props_snapshot.len()];
                    c.program_symbol(s).name.clone()
                }
            },
            |c, index, t| {
                let prop_type = if index < props_snapshot.len() {
                    let p = props_snapshot[index];
                    let initializer = c.store().node(p).as_jsx_attribute().expect("data").initializer;
                    match initializer {
                        Some(initializer) => c.get_context_free_type_of_expression(initializer),
                        // JsxAttribute without initializer is always true.
                        None => c.true_type(),
                    }
                } else {
                    c.undefined_type()
                };
                for s in c.type_(prop_type).distributed() {
                    if c.is_type_assignable_to(s, t) {
                        return true;
                    }
                }
                false
            },
        );
        self.discriminated_contextual_types.insert(key, Some(discriminated));
        Some(discriminated)
    }
}

// Local aliases (kept out of the import list to avoid ambiguity with the
// method surfaces of the same name).
use crate::relater::get_indexed_access_type as relater_get_indexed_access_type;
use crate::relater::is_array_like_type as relater_is_array_like_type;

// ────────────────────────────────────────────────────────────────────────────
// The elaboration family (Go jsx.go:330-560)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type JsxElaborationElement struct` (jsx.go:430) — `createDiagnostic`
/// is the captured-values PORT shape (see the module PORT note).
struct JsxElaborationElement {
    error_node: Option<NodeId>,
    inner_expression: Option<NodeId>,
    name_type: Option<crate::types::TypeId>,
    /// Go's `createDiagnostic` closure values (the JSX-text-child "components
    /// don't accept text" factory): (tag-name node, children prop name,
    /// children target type).
    invalid_text_child: Option<(NodeId, String, crate::types::TypeId)>,
}

impl Checker {
    /// Go: `func (c *Checker) elaborateJsxComponents(node, source, target, relation, diagnosticOutput) bool` (jsx.go:330).
    pub(crate) fn elaborate_jsx_components(
        &mut self,
        node: NodeId,
        source: crate::types::TypeId,
        target: crate::types::TypeId,
        relation: RelationKind,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        let mut reported_error = false;
        let properties = jsx_properties_of(self.store(), node);
        for prop in properties {
            let is_spread = self.store().node(prop).kind == Kind::JsxSpreadAttribute;
            let name_text = if is_spread {
                String::new()
            } else {
                let name_node = self
                    .store()
                    .node(prop)
                    .name()
                    .expect("attribute name");
                crate::ast_shims::jsx_name_text(self.store(), name_node)
            };
            if !is_spread && !crate::relater::is_hyphenated_jsx_name(&name_text) {
                // Go: getStringLiteralType never answers nil (checker.go:25839),
                // so Go's `nameType != nil` guard is vacuous; the NEVER flag
                // check is the live half.
                let name_type = self.get_string_literal_type(&name_text);
                if !self.type_(name_type).flags.intersects(TypeFlags::NEVER) {
                    let initializer = self
                        .store()
                        .node(prop)
                        .as_jsx_attribute()
                        .expect("JsxAttribute data")
                        .initializer;
                    let name_node = self.store().node(prop).name().expect("attribute name");
                    reported_error = self.elaborate_element(
                        source,
                        target,
                        relation,
                        Some(name_node),
                        initializer,
                        name_type,
                        None, /*errorMessage*/
                        None, /*diagnosticFactory — the non-text elaborations don't build one*/
                        diagnostic_output.as_deref_mut(),
                    ) || reported_error;
                }
            }
        }
        let parent = self.store().node(node).parent.get();
        if self.store().node(parent).kind == Kind::JsxOpeningElement {
            let containing_element = self.store().node(parent).parent.get();
            if self.store().node(containing_element).kind == Kind::JsxElement {
                // Containing JSXElement (Go comment).
                let children_prop_name = {
                    let ns = self.get_jsx_namespace_at(Some(node));
                    let name = self.get_jsx_element_children_property_name(ns);
                    if name == tsc_ast::INTERNAL_SYMBOL_NAME_MISSING {
                        "children".to_string()
                    } else {
                        name
                    }
                };
                let children_name_type = self.get_string_literal_type(&children_prop_name);
                let children_target_type =
                    crate::relater::get_indexed_access_type(self, target, children_name_type);
                let valid_children =
                    get_semantic_jsx_children(self.store(), &jsx_children_of(self.store(), containing_element));
                if valid_children.is_empty() {
                    return reported_error;
                }
                let more_than_one_real_children = valid_children.len() > 1;
                let iterable_type = self.get_global_iterable_type();
                let (array_like_target_parts, non_array_like_target_parts) =
                    if iterable_type != self.empty_generic_type() {
                        let any_type = self.any_type();
                        let any_iterable = self.create_iterable_type(any_type);
                        (
                            Some(self.filter_type_mut(children_target_type, |c, t| {
                                c.is_type_assignable_to(t, any_iterable)
                            })),
                            Some(self.filter_type_mut(children_target_type, |c, t| {
                                !c.is_type_assignable_to(t, any_iterable)
                            })),
                        )
                    } else {
                        (
                            Some(self.filter_type_mut(children_target_type, |c, t| {
                                crate::typenodes::is_array_or_tuple_like_type(c, t)
                            })),
                            Some(self.filter_type_mut(children_target_type, |c, t| {
                                !crate::typenodes::is_array_or_tuple_like_type(c, t)
                            })),
                        )
                    };
                // Go: getInvalidTextualChildDiagnostic — the lazily-built
                // "components don't accept text" message args (tag name,
                // children prop name, children target type).
                let get_invalid_text_args = |c: &mut Checker| -> Vec<String> {
                    let tag_name = jsx_tag_name_of(c.store(), parent).expect("tag name");
                    let tag_name_text = crate::ast_shims::get_text_of_node(c.store(), tag_name);
                    let children_target_type_string =
                        crate::nodebuilder::type_to_string(c, children_target_type);
                    vec![tag_name_text, children_prop_name.clone(), children_target_type_string]
                };
                let children_target_type_string =
                    crate::nodebuilder::type_to_string(self, children_target_type);
                let _ = get_invalid_text_args;
                if more_than_one_real_children {
                    if array_like_target_parts != Some(self.never_type()) {
                        let child_types =
                            self.check_jsx_children(containing_element, CheckMode::CHECK_MODE_NORMAL);
                        let real_source = self.create_tuple_type(&child_types);
                        let children = self.generate_jsx_children(
                            containing_element,
                            parent,
                            children_prop_name.clone(),
                            children_target_type,
                        );
                        reported_error = self.elaborate_iterable_or_array_like_target_elementwise(
                            &children,
                            real_source,
                            array_like_target_parts.expect("set above"),
                            relation,
                            diagnostic_output.as_deref_mut(),
                        ) || reported_error;
                    } else {
                        let source_children =
                            crate::relater::get_indexed_access_type(self, source, children_name_type);
                        if !self.is_type_related_to(
                            source_children,
                            children_target_type,
                            relation,
                        ) {
                        // arity mismatch (Go comment).
                        let opening_element_tag_name = {
                            let d = self
                                .store()
                                .node(containing_element)
                                .as_jsx_element()
                                .expect("data");
                            jsx_tag_name_of(self.store(), d.opening_element).expect("tag name")
                        };
                        let diagnostic = self.error(
                            opening_element_tag_name,
                            &diag::THIS_JSX_TAG_S_0_PROP_EXPECTS_A_SINGLE_CHILD_OF_TYPE_1_BUT_MULTIPLE_CHILDREN_WERE_PROVIDED,
                            &[children_prop_name.clone(), children_target_type_string.clone()],
                        );
                        crate::relater::report_diagnostic(
                            self,
                            Some(diagnostic),
                            diagnostic_output.as_deref_mut(),
                        );
                        reported_error = true;
                        }
                    }
                } else if non_array_like_target_parts != Some(self.never_type()) {
                    let child = valid_children[0];
                    let e = self.get_elaboration_element_for_jsx_child(
                        child,
                        children_name_type,
                        children_prop_name.clone(),
                        children_target_type,
                        parent,
                    );
                    if e.error_node.is_some() {
                        reported_error = self.elaborate_element(
                            source,
                            target,
                            relation,
                            e.error_node,
                            e.inner_expression,
                            e.name_type.expect("nameType present when errorNode set"),
                            None,
                            e.invalid_text_child,
                            diagnostic_output.as_deref_mut(),
                        ) || reported_error;
                    }
                } else {
                    let source_children =
                        crate::relater::get_indexed_access_type(self, source, children_name_type);
                    if !self.is_type_related_to(
                        source_children,
                        children_target_type,
                        relation,
                    ) {
                    // arity mismatch (Go comment).
                    let opening_element_tag_name = {
                        let d = self
                            .store()
                            .node(containing_element)
                            .as_jsx_element()
                            .expect("data");
                        jsx_tag_name_of(self.store(), d.opening_element).expect("tag name")
                    };
                    let diagnostic = self.error(
                        opening_element_tag_name,
                        &diag::THIS_JSX_TAG_S_0_PROP_EXPECTS_TYPE_1_WHICH_REQUIRES_MULTIPLE_CHILDREN_BUT_ONLY_A_SINGLE_CHILD_WAS_PROVIDED,
                        &[children_prop_name.clone(), children_target_type_string.clone()],
                    );
                    crate::relater::report_diagnostic(
                        self,
                        Some(diagnostic),
                        diagnostic_output,
                    );
                    reported_error = true;
                    }
                }
            }
        }
        reported_error
    }

    /// Go: `func (c *Checker) generateJsxChildren(node, getInvalidTextDiagnostic) iter.Seq[JsxElaborationElement]` (jsx.go:440) —
    /// the eagerly-collected PORT shape (see the module PORT note).
    fn generate_jsx_children(
        &mut self,
        node: NodeId,
        // PORT(FCS tranche 3): Go passes the lazily-invoked
        // getInvalidTextDiagnostic factory, which closes over the ORIGINAL
        // elaborateJsxComponents `node.Parent` (the JsxOpeningElement whose
        // TagName the "don't accept text" message reads, jsx.go:333). The
        // port materializes the tag-name node instead — it must be the
        // ORIGINAL opening element, not `node.parent` (the containing
        // element's parent, which is not an opening element — the expect
        // in the JsxText arm panicked on it).
        opening_element: NodeId,
        children_prop_name: String,
        children_target_type: crate::types::TypeId,
    ) -> Vec<JsxElaborationElement> {
        let children = jsx_children_of(self.store(), node);
        let mut result: Vec<JsxElaborationElement> = Vec::new();
        let mut member_offset: i64 = 0;
        for (i, child) in children.iter().enumerate() {
            let name_type =
                self.get_number_literal_type(tsc_jsnum::Number((i as i64 - member_offset) as f64));
            let e = self.get_elaboration_element_for_jsx_child(
                *child,
                name_type,
                children_prop_name.clone(),
                children_target_type,
                opening_element,
            );
            if e.error_node.is_some() {
                result.push(e);
            } else {
                member_offset += 1;
            }
        }
        result
    }

    /// Go: `func (c *Checker) getElaborationElementForJsxChild(child, nameType, getInvalidTextDiagnostic) JsxElaborationElement` (jsx.go:459).
    #[allow(clippy::too_many_arguments)]
    fn get_elaboration_element_for_jsx_child(
        &mut self,
        child: NodeId,
        name_type: crate::types::TypeId,
        children_prop_name: String,
        children_target_type: crate::types::TypeId,
        opening_element: NodeId,
    ) -> JsxElaborationElement {
        let kind = self.store().node(child).kind;
        match kind {
            // child is of the type of the expression.
            Kind::JsxExpression => {
                let inner_expression = self.store().node(child).as_jsx_expression().expect("data").expression;
                JsxElaborationElement {
                    error_node: Some(child),
                    inner_expression,
                    name_type: Some(name_type),
                    invalid_text_child: None,
                }
            }
            Kind::JsxText => {
                if self
                    .store()
                    .node(child)
                    .as_jsx_text()
                    .expect("data")
                    .contains_only_trivia_white_spaces
                {
                    // Whitespace only jsx text isn't real jsx text (Go).
                    return JsxElaborationElement {
                        error_node: None,
                        inner_expression: None,
                        name_type: None,
                        invalid_text_child: None,
                    };
                }
                // child is a string (Go). The diagnostic factory captures
                // (tag name node, children prop name, children target type).
                let tag_name = jsx_tag_name_of(self.store(), opening_element).expect("tag name");
                JsxElaborationElement {
                    error_node: Some(child),
                    inner_expression: None,
                    name_type: Some(name_type),
                    invalid_text_child: Some((tag_name, children_prop_name, children_target_type)),
                }
            }
            Kind::JsxElement | Kind::JsxSelfClosingElement | Kind::JsxFragment => {
                // child is of type JSX.Element (Go).
                JsxElaborationElement {
                    error_node: Some(child),
                    inner_expression: Some(child),
                    name_type: Some(name_type),
                    invalid_text_child: None,
                }
            }
            _ => panic!("Unhandled case in getElaborationElementForJsxChild"),
        }
        // Go: `panic("Unhandled case in getElaborationElementForJsxChild")`
        // — unreachable through the parser's child kinds; the match above
        // covers every NodeData payload a JsxElement child can carry.
    }

    /// Go: `func (c *Checker) elaborateIterableOrArrayLikeTargetElementwise(iterator, source, target, relation, diagnosticOutput) bool` (jsx.go:476).
    fn elaborate_iterable_or_array_like_target_elementwise(
        &mut self,
        iterator: &[JsxElaborationElement],
        source: crate::types::TypeId,
        target: crate::types::TypeId,
        relation: RelationKind,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        let tuple_or_array_like_target_parts = self.filter_type_mut(target, |c, t| {
            crate::typenodes::is_array_or_tuple_like_type(c, t)
        });
        let non_tuple_or_array_like_target_parts = self.filter_type_mut(target, |c, t| {
            !crate::typenodes::is_array_or_tuple_like_type(c, t)
        });
        // If `nonTupleOrArrayLikeTargetParts` is not `never`, then that
        // should mean `Iterable` is defined (Go comment).
        let mut iteration_type: Option<crate::types::TypeId> = None;
        if non_tuple_or_array_like_target_parts != self.never_type() {
            iteration_type = self.get_iteration_type_of_iterable(
                crate::statements::IterationUse::FOR_OF,
                crate::statements::IterationTypeKind::Yield,
                non_tuple_or_array_like_target_parts,
                None, /*errorNode*/
            );
        }
        let mut reported_error = false;
        for e in iterator {
            let prop = e.error_node;
            let next = e.inner_expression;
            let name_type = e.name_type;
            let mut target_prop_type = iteration_type;
            let mut target_indexed_prop_type: Option<crate::types::TypeId> = None;
            if tuple_or_array_like_target_parts != self.never_type() {
                if let Some(name_type) = name_type {
                    target_indexed_prop_type = self.get_best_match_indexed_access_type_or_undefined(
                        source,
                        tuple_or_array_like_target_parts,
                        name_type,
                    );
                }
            }
            if let Some(t) = target_indexed_prop_type.filter(|&t| {
                !self.type_(t).flags.intersects(TypeFlags::INDEXED_ACCESS)
            }) {
                if let Some(iteration_type) = iteration_type {
                    target_prop_type = Some(self.get_union_type(&[iteration_type, t]));
                } else {
                    target_prop_type = Some(t);
                }
            }
            let Some(target_prop_type) = target_prop_type else {
                continue;
            };
            let Some(name_type) = name_type else {
                continue;
            };
            let source_prop_type = self.get_indexed_access_type_or_undefined(
                source,
                name_type,
                crate::types::AccessFlags::NONE,
                None,
            );
            let Some(source_prop_type) = source_prop_type else {
                continue;
            };
            let prop_name = self.get_property_name_from_index(name_type, None /*accessNode*/);
            if !self.check_type_related_to(source_prop_type, target_prop_type, relation, None /*errorNode*/) {
                let elaborated = next.is_some()
                    && self.elaborate_error(
                        next,
                        source_prop_type,
                        target_prop_type,
                        relation,
                        None, /*headMessage*/
                        diagnostic_output.as_deref_mut(),
                    );
                reported_error = true;
                if !elaborated {
                    // Issue error on the prop itself, since the prop
                    // couldn't elaborate the error. Use the expression
                    // type, if available (Go comment).
                    let mut specific_source = source_prop_type;
                    if let Some(next) = next {
                        specific_source =
                            self.check_expression_for_mutable_location_with_contextual_type(
                                next,
                                source_prop_type,
                            );
                    }
                    if let Some((tag_name, children_prop_name, children_target_type)) =
                        e.invalid_text_child.as_ref()
                    {
                        // Use the custom diagnostic factory if provided
                        // (e.g., for JSX text children with dynamic error
                        // messages) — Go's createDiagnostic(prop).
                        let _ = tag_name;
                        let children_target_type_string =
                            crate::nodebuilder::type_to_string(self, *children_target_type);
                        let diagnostic = crate::utilities::new_diagnostic_for_node(
                            self.store(),
                            prop.expect("errorNode present"),
                            &diag::X_0_COMPONENTS_DON_T_ACCEPT_TEXT_AS_CHILD_ELEMENTS_TEXT_IN_JSX_HAS_THE_TYPE_STRING_BUT_THE_EXPECTED_TYPE_OF_1_IS_2,
                            vec![
                                crate::ast_shims::get_text_of_node(self.store(), *tag_name),
                                children_prop_name.clone(),
                                children_target_type_string,
                            ],
                        );
                        crate::relater::report_diagnostic(
                            self,
                            Some(diagnostic),
                            diagnostic_output.as_deref_mut(),
                        );
                    } else if self.exact_optional_property_types
                        && self.is_exact_optional_property_mismatch(Some(specific_source), Some(target_prop_type))
                    {
                        let specific_source_string =
                            crate::nodebuilder::type_to_string(self, specific_source);
                        let target_prop_type_string =
                            crate::nodebuilder::type_to_string(self, target_prop_type);
                        let diagnostic = crate::utilities::new_diagnostic_for_node(
                            self.store(),
                            prop.expect("errorNode present"),
                            &diag::TYPE_0_IS_NOT_ASSIGNABLE_TO_TYPE_1_WITH_EXACTOPTIONALPROPERTYTYPES_COLON_TRUE_CONSIDER_ADDING_UNDEFINED_TO_THE_TYPE_OF_THE_TARGET,
                            vec![specific_source_string, target_prop_type_string],
                        );
                        crate::relater::report_diagnostic(
                            self,
                            Some(diagnostic),
                            diagnostic_output.as_deref_mut(),
                        );
                    } else {
                        let target_optional_symbol = self
                            .get_property_of_type(tuple_or_array_like_target_parts, &prop_name)
                            .unwrap_or_else(|| self.unknown_symbol());
                        let target_is_optional = prop_name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING
                            && self.program_symbol(target_optional_symbol).flags.intersects(SymbolFlags::OPTIONAL);
                        let source_optional_symbol = self
                            .get_property_of_type(source, &prop_name)
                            .unwrap_or_else(|| self.unknown_symbol());
                        let source_is_optional = prop_name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING
                            && self.program_symbol(source_optional_symbol).flags.intersects(SymbolFlags::OPTIONAL);
                        let target_prop_type = self.remove_missing_type(target_prop_type, target_is_optional);
                        let source_prop_type = self
                            .remove_missing_type(source_prop_type, target_is_optional && source_is_optional);
                        let prop = prop.expect("errorNode present");
                        let result = self.check_type_related_to_ex(
                            specific_source,
                            target_prop_type,
                            relation,
                            Some(prop),
                            None,
                            diagnostic_output.as_deref_mut(),
                        );
                        if result && specific_source != source_prop_type {
                            // If for whatever reason the expression type
                            // doesn't yield an error, make sure we still
                            // issue an error on the sourcePropType (Go).
                            self.check_type_related_to_ex(
                                source_prop_type,
                                target_prop_type,
                                relation,
                                Some(prop),
                                None,
                                diagnostic_output.as_deref_mut(),
                            );
                        }
                    }
                }
            }
        }
        reported_error
    }

    /// Go: `func (c *Checker) getSuggestedSymbolForNonexistentJSXAttribute(name, containingType) *ast.Symbol` (jsx.go:548).
    pub(crate) fn get_suggested_symbol_for_nonexistent_jsx_attribute(
        &mut self,
        name: &str,
        containing_type: crate::types::TypeId,
    ) -> Option<SymbolId> {
        let properties = crate::relater::get_properties_of_type(self, containing_type);
        let jsx_specific: Option<SymbolId> = match name {
            "for" => properties
                .iter()
                .copied()
                .find(|&x| self.program_symbol(x).name == "htmlFor"),
            "class" => properties
                .iter()
                .copied()
                .find(|&x| self.program_symbol(x).name == "className"),
            _ => None,
        };
        if jsx_specific.is_some() {
            return jsx_specific;
        }
        self.get_spelling_suggestion_for_name(name, &properties, SymbolFlags::VALUE)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Fragment factory + opening-like-element resolution (Go jsx.go:566-680)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getJSXFragmentType(node) *Type` (jsx.go:566).
    pub(crate) fn get_jsx_fragment_type(&mut self, node: NodeId) -> crate::types::TypeId {
        // An opening fragment is required in order for `getJsxNamespace` to
        // give the fragment factory (Go comment).
        let file = tsc_ast::utilities::get_source_file_of_node(self.store(), node)
            .expect("node file")
            .as_node_id();
        if let Some(jsx_fragment_type) = self.source_file_links.get(file).jsx_fragment_type {
            return jsx_fragment_type;
        }
        let jsx_fragment_factory_name = self.get_jsx_namespace_string(Some(node));
        // #38720/60122, allow null as jsxFragmentFactory (Go comment).
        let should_resolve_factory_reference = (self.compiler_options.jsx == JsxEmit::React
            || !self.compiler_options.jsx_fragment_factory.is_empty())
            && jsx_fragment_factory_name != "null";
        if !should_resolve_factory_reference {
            let any_type = self.any_type();
            self.source_file_links.get(file).jsx_fragment_type = Some(any_type);
            return any_type;
        }
        let mut jsx_factory_symbol = self.get_jsx_namespace_container_for_implicit_import(Some(node));
        if jsx_factory_symbol.is_none() {
            let should_module_ref_err = self.compiler_options.jsx != JsxEmit::Preserve
                && self.compiler_options.jsx != JsxEmit::ReactNative;
            let mut flags = SymbolFlags::VALUE;
            if !should_module_ref_err {
                flags &= !SymbolFlags::ENUM;
            }
            jsx_factory_symbol = self.resolve_name_worker(
                Some(node),
                &jsx_fragment_factory_name,
                flags,
                Some(&diag::USING_JSX_FRAGMENTS_REQUIRES_FRAGMENT_FACTORY_0_TO_BE_IN_SCOPE_BUT_IT_COULD_NOT_BE_FOUND),
                true,  /*isUse*/
                false, /*excludeGlobals*/
            );
        }
        let Some(jsx_factory_symbol) = jsx_factory_symbol else {
            let error_type = self.error_type();
            self.source_file_links.get(file).jsx_fragment_type = Some(error_type);
            return error_type;
        };
        if self.program_symbol(jsx_factory_symbol).name == react_names::FRAGMENT {
            let t = self.get_type_of_symbol(jsx_factory_symbol);
            self.source_file_links.get(file).jsx_fragment_type = Some(t);
            return t;
        }
        let mut resolved_alias = jsx_factory_symbol;
        if self.program_symbol(jsx_factory_symbol).flags.intersects(SymbolFlags::ALIAS) {
            resolved_alias = self.resolve_alias(jsx_factory_symbol);
        }
        let react_exports = self.get_exports_of_symbol(resolved_alias);
        let type_symbol = self.get_symbol(
            &react_exports,
            react_names::FRAGMENT,
            SymbolFlags::BLOCK_SCOPED_VARIABLE,
        );
        let t = match type_symbol {
            Some(type_symbol) => self.get_type_of_symbol(type_symbol),
            None => self.error_type(),
        };
        self.source_file_links.get(file).jsx_fragment_type = Some(t);
        t
    }

    /// Go: `func (c *Checker) resolveJsxOpeningLikeElement(node, candidatesOutArray, checkMode) *Signature` (jsx.go:622).
    pub(crate) fn resolve_jsx_opening_like_element(
        &mut self,
        node: NodeId,
        candidates_out_array: Option<&mut Vec<crate::types::SignatureId>>,
        check_mode: CheckMode,
    ) -> Option<crate::types::SignatureId> {
        let is_jsx_open_fragment = self.store().node(node).kind == Kind::JsxOpeningFragment;
        
        let expr_types: crate::types::TypeId = if !is_jsx_open_fragment {
            let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
            if crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name) {
                let result = self.get_intrinsic_attributes_type_from_jsx_opening_like_element(node);
                let fake_signature = self.create_signature_for_jsx_intrinsic(node, result);
                let attributes = jsx_attributes_of(self.store(), node).expect("attributes");
                let effective_first_arg =
                    self.get_effective_first_argument_for_jsx_signature(fake_signature, node);
                let check_expr_type = self.check_expression_with_contextual_type(
                    attributes,
                    Some(effective_first_arg),
                    None, /*inferenceContext*/
                    CheckMode::CHECK_MODE_NORMAL,
                );
                self.check_type_assignable_to_and_optionally_elaborate(
                    check_expr_type,
                    result,
                    Some(tag_name),
                    Some(attributes),
                    None,
                    None,
                );
                let type_arguments =
                    crate::statements::type_arguments_of(self.store(), node).to_vec();
                if !type_arguments.is_empty() {
                    self.check_source_elements(&type_arguments);
                    let source_file =
                        tsc_ast::utilities::get_source_file_of_node(self.store(), node)
                            .expect("node file");
                    // Go: `loc := core.NewTextRange(scanner.SkipTrivia(
                    //     sourceFile.Text(), typeArgumentList.Loc.Pos()),
                    //     typeArgumentList.Loc.End())` — the type-argument
                    // list node's span, SkipTrivia'd at the start.
                    let (loc_pos, loc_end) = {
                        let n = self.store().node(node);
                        let type_argument_list = match n.kind {
                            Kind::JsxOpeningElement => n
                                .as_jsx_opening_element()
                                .expect("data")
                                .type_arguments
                                .as_ref()
                                .map(|l| l.loc),
                            _ => n
                                .as_jsx_self_closing_element()
                                .expect("data")
                                .type_arguments
                                .as_ref()
                                .map(|l| l.loc),
                        }
                        .expect("type argument list present");
                        let pos = tsc_scanner::skip_trivia(
                            source_file.text(),
                            type_argument_list.pos(),
                        );
                        (pos, type_argument_list.end())
                    };
                    let loc = TextRange::new(loc_pos, loc_end);
                    let diagnostic = tsc_ast::Diagnostic::new(
                        Some(source_file.as_node_id()),
                        loc,
                        diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1.code(),
                        diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1.category(),
                        "",
                        Some(&diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1),
                        "",
                        diag::EXPECTED_0_TYPE_ARGUMENTS_BUT_GOT_1.key(),
                        vec!["0".to_string(), type_arguments.len().to_string()],
                    );
                    self.add_diagnostic(diagnostic);
                }
                return Some(fake_signature);
            }
            self.check_expression(tag_name)
        } else {
            self.get_jsx_fragment_type(node)
        };
        let apparent_type = self.get_apparent_type(expr_types);
        if self.is_error_type(apparent_type) {
            return self.resolve_error_call(node);
        }
        let signatures = self.get_uninstantiated_jsx_signatures_of_type(expr_types, node);
        if self.is_untyped_function_call(expr_types, apparent_type, signatures.len(), 0 /*constructSignatures*/) {
            return self.resolve_untyped_call(node);
        }
        if signatures.is_empty() {
            // We found no signatures at all, which is an error (Go comment).
            let text = if is_jsx_open_fragment {
                crate::ast_shims::get_text_of_node(self.store(), node)
            } else {
                let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
                crate::ast_shims::get_text_of_node(self.store(), tag_name)
            };
            let error_node = if is_jsx_open_fragment {
                node
            } else {
                jsx_tag_name_of(self.store(), node).expect("tag name")
            };
            self.error(
                error_node,
                &diag::JSX_ELEMENT_TYPE_0_DOES_NOT_HAVE_ANY_CONSTRUCT_OR_CALL_SIGNATURES,
                &[text],
            );
            return self.resolve_error_call(node);
        }
        self.resolve_call(
            node,
            &signatures,
            candidates_out_array,
            check_mode,
            SignatureFlags::NONE,
            None,
        )
    }

    /// Go: `func (c *Checker) checkApplicableSignatureForJsxCallLikeElement(node, signature, relation, checkMode, reportErrors, diagnosticOutput) bool` (jsx.go:666).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check_applicable_signature_for_jsx_call_like_element(
        &mut self,
        node: NodeId,
        signature: crate::types::SignatureId,
        relation: RelationKind,
        check_mode: CheckMode,
        report_errors: bool,
        mut diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        // Stateless function components can have maximum of three arguments:
        // "props", "context", and "updater". However "context" and "updater"
        // are implicit and can't be specify by users. Only the first
        // parameter, props, can be specified by users through attributes
        // property (Go comment).
        let param_type = self.get_effective_first_argument_for_jsx_signature(signature, node);
        
        let attributes_type: crate::types::TypeId = if self.store().node(node).kind == Kind::JsxOpeningFragment {
            self.create_jsx_attributes_type_from_attributes_property(node, CheckMode::CHECK_MODE_NORMAL)
        } else {
            let attributes = jsx_attributes_of(self.store(), node).expect("attributes");
            self.check_expression_with_contextual_type(
                attributes,
                Some(param_type),
                None, /*inferenceContext*/
                check_mode,
            )
        };
        let check_attributes_type: crate::types::TypeId =
            if check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE) {
                crate::relater::get_regular_type_of_object_literal(self, attributes_type)
            } else {
                attributes_type
            };
        // Go: checkTagNameDoesNotExpectTooManyArguments closure.
        if !self.check_tag_name_does_not_expect_too_many_arguments(
            node,
            report_errors,
            diagnostic_output.as_deref_mut(),
        ) {
            return false;
        }
        let error_node: Option<NodeId> = if report_errors {
            if self.store().node(node).kind == Kind::JsxOpeningFragment {
                Some(node)
            } else {
                jsx_tag_name_of(self.store(), node)
            }
        } else {
            None
        };
        let attributes: Option<NodeId> = if self.store().node(node).kind == Kind::JsxOpeningFragment {
            None
        } else {
            jsx_attributes_of(self.store(), node)
        };
        self.check_type_related_to_and_optionally_elaborate(
            check_attributes_type,
            param_type,
            relation,
            error_node,
            attributes,
            None, /*headMessage*/
            diagnostic_output,
        )
    }

    /// Go: `checkApplicableSignatureForJsxCallLikeElement`'s
    /// `checkTagNameDoesNotExpectTooManyArguments` closure (jsx.go:686-750).
    fn check_tag_name_does_not_expect_too_many_arguments(
        &mut self,
        node: NodeId,
        report_errors: bool,
        diagnostic_output: Option<&mut Vec<tsc_ast::Diagnostic>>,
    ) -> bool {
        if self.get_jsx_namespace_container_for_implicit_import(Some(node)).is_some() {
            // factory is implicitly jsx/jsxdev - assume it fits the bill,
            // since we don't strongly look for the jsx/jsxs/jsxDEV factory
            // APIs anywhere else (at least not yet) (Go comment).
            return true;
        }
        // We assume fragments have the correct arity since the node does not
        // have attributes (Go comment).
        let node_kind = self.store().node(node).kind;
        let mut tag_type: Option<crate::types::TypeId> = None;
        if (node_kind == Kind::JsxOpeningElement || node_kind == Kind::JsxSelfClosingElement) && {
            let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
            !(crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name)
                || self.store().node(tag_name).kind == Kind::JsxNamespacedName)
        } {
            let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
            tag_type = Some(self.check_expression(tag_name));
        }
        let Some(tag_type) = tag_type else {
            return true;
        };
        let tag_call_signatures =
            self.get_signatures_of_type(tag_type, crate::types::SignatureKind::Call);
        if tag_call_signatures.is_empty() {
            return true;
        }
        let factory = self.get_jsx_factory_entity(Some(node));
        let Some(factory) = factory else {
            return true;
        };
        let factory_symbol = self.resolve_entity_name(
            factory,
            SymbolFlags::VALUE,
            true,  /*ignoreErrors*/
            false, /*dontResolveAlias*/
            node,
        );
        let Some(factory_symbol) = factory_symbol else {
            return true;
        };
        let factory_type = self.get_type_of_symbol(factory_symbol);
        let call_signatures =
            self.get_signatures_of_type(factory_type, crate::types::SignatureKind::Call);
        if call_signatures.is_empty() {
            return true;
        }
        let mut has_first_param_signatures = false;
        let mut max_param_count: i32 = 0;
        // Check that _some_ first parameter expects a FC-like thing, and
        // that some overload of the SFC expects an acceptable number of
        // arguments (Go comment).
        for &sig in &call_signatures {
            let firstparam = self.get_type_at_position(sig, 0);
            let signatures_of_param =
                self.get_signatures_of_type(firstparam, crate::types::SignatureKind::Call);
            if signatures_of_param.is_empty() {
                continue;
            }
            for &param_sig in &signatures_of_param {
                has_first_param_signatures = true;
                if self.has_effective_rest_parameter(param_sig) {
                    // some signature has a rest param, so function
                    // components can have an arbitrary number of arguments.
                    return true;
                }
                let param_count = self.get_parameter_count(param_sig);
                if param_count > max_param_count {
                    max_param_count = param_count;
                }
            }
        }
        if !has_first_param_signatures {
            // Not a single signature had a first parameter which expected a
            // signature - for back compat, and to guard against generic
            // factories which won't have signatures directly, do not error.
            return true;
        }
        let mut absolute_min_arg_count = i32::MAX;
        for &tag_sig in &tag_call_signatures {
            let tag_required_arg_count = self.get_min_argument_count(tag_sig);
            if tag_required_arg_count < absolute_min_arg_count {
                absolute_min_arg_count = tag_required_arg_count;
            }
        }
        if absolute_min_arg_count <= max_param_count {
            // some signature accepts the number of arguments the function
            // component provides (Go comment).
            return true;
        }
        if report_errors {
            let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
            // We will not report errors in this function for fragments,
            // since we do not check them in this function (Go comment).
            let tag_name_string = crate::utilities::entity_name_to_string(self.store(), tag_name);
            let factory_string = crate::utilities::entity_name_to_string(self.store(), factory);
            let mut diagnostic = crate::utilities::new_diagnostic_for_node(
                self.store(),
                tag_name,
                &diag::TAG_0_EXPECTS_AT_LEAST_1_ARGUMENTS_BUT_THE_JSX_FACTORY_2_PROVIDES_AT_MOST_3,
                vec![
                    tag_name_string.clone(),
                    absolute_min_arg_count.to_string(),
                    factory_string,
                    max_param_count.to_string(),
                ],
            );
            let tag_name_symbol = self.get_symbol_at_location_stub(tag_name, false);
            if let Some(tag_name_symbol) =
                tag_name_symbol.filter(|&s| self.program_symbol(s).value_declaration.is_some())
            {
                let value_declaration =
                    self.program_symbol(tag_name_symbol).value_declaration.expect("checked");
                diagnostic.add_related_info(crate::relater::diagnostic_for_node(
                    self,
                    Some(value_declaration),
                    &diag::X_0_IS_DECLARED_HERE,
                    &[crate::relater::DiagArg::Str(tag_name_string.clone())],
                ));
            }
            crate::relater::report_diagnostic(self, Some(diagnostic), diagnostic_output);
        }
        false
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The attributes type (Go jsx.go:764-1006)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) createJsxAttributesTypeFromAttributesProperty(openingLikeElement, checkMode) *Type` (jsx.go:764).
    ///
    /// Get attributes type of the JSX opening-like element. The result is
    /// from resolving "attributes" property of the opening-like element
    /// (Go doc comment). Because this function calls getSpreadType, it needs
    /// to use the same checks as checkObjectLiteral, which also calls
    /// getSpreadType (Go remark).
    pub(crate) fn create_jsx_attributes_type_from_attributes_property(
        &mut self,
        opening_like_element: NodeId,
        check_mode: CheckMode,
    ) -> crate::types::TypeId {
        let strict_null_checks = self.strict_null_checks;
        let mut all_attributes_table: Option<SymbolTable> =
            strict_null_checks.then(SymbolTable::new);
        let mut attributes_table = SymbolTable::new();
        let mut attributes_symbol: Option<SymbolId> = None;
        let mut attribute_parent = opening_like_element;
        let mut spread = self.empty_jsx_object_type();
        let mut has_spread_any_type = false;
        let mut type_to_intersect: Option<crate::types::TypeId> = None;
        let mut explicitly_specify_children_attribute = false;
        let mut object_flags = ObjectFlags::JSX_ATTRIBUTES;
        let jsx_children_property_name = {
            let ns = self.get_jsx_namespace_at(Some(opening_like_element));
            self.get_jsx_element_children_property_name(ns)
        };
        let is_jsx_open_fragment = self.store().node(opening_like_element).kind == Kind::JsxOpeningFragment;
        // Go: createJsxAttributesType closure.
        // Go: the closure MUTATES the captured `objectFlags`
        // (`objectFlags |= ObjectFlagsFreshLiteral`), so the fresh flag
        // persists into the later getSpreadType calls (checker.go:13693's
        // `... | objectFlags`) — the PORT's macro mutates `object_flags`
        // in place to keep the Go-visible flag evolution (jsx.go:731-738).
        macro_rules! create_jsx_attributes_type {
            ($c:expr) => {{
                object_flags |= ObjectFlags::FRESH_LITERAL;
                let members = attributes_table.clone();
                let symbol = attributes_symbol;
                let result = $c.new_anonymous_type_with_members(symbol, members, object_flags);
                $c.type_mut(result).object_flags |=
                    ObjectFlags::OBJECT_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
                result
            }};
        }
        if !is_jsx_open_fragment {
            let attributes = jsx_attributes_of(self.store(), opening_like_element)
                .expect("attributes");
            attributes_symbol = self
                .store()
                .node(attributes)
                .as_jsx_attributes()
                .expect("data")
                .symbol;
            attribute_parent = attributes;
            let contextual_type = self.get_contextual_type(attributes, ContextFlags::NONE);
            // Create anonymous type from given attributes symbol table
            // (Go doc comment).
            let attribute_decls = jsx_properties_of(self.store(), attributes);
            for attribute_decl in attribute_decls {
                // PORT (FCS tranche 24): Go jsx.go:736-739 reads
                // `member := attributeDecl.Symbol()` and branches on
                // `ast.IsJsxAttribute(attributeDecl)` FIRST — the symbol is
                // only used inside the attribute arm; the SPREAD arm runs
                // unconditionally. The former port hoisted the member
                // lookup (name() → JsxAttribute payload symbol) above the
                // kind branch and `continue`d when it resolved to None — a
                // JsxSpreadAttribute has neither a name nor a JsxAttribute
                // payload symbol, so EVERY spread attribute was silently
                // dropped from the attributes type (JSX spread props never
                // merged; the intraExpressionInferencesJsx `{...{a, b}}`
                // shape collapsed to `{}`).
                if self.store().node(attribute_decl).kind == Kind::JsxAttribute {
                    let member = self
                        .store()
                        .node(attribute_decl)
                        .name()
                        .map(|_| ())
                        .and_then(|()| self.get_symbol_of_declaration(attribute_decl))
                        .or_else(|| {
                            // Go reads attributeDecl.Symbol() — the binder
                            // payload symbol (JsxAttribute/JsxSpreadAttribute).
                            self.store()
                                .node(attribute_decl)
                                .as_jsx_attribute()
                                .and_then(|d| d.symbol)
                        });
                    let Some(member) = member else {
                        continue;
                    };
                    let expr_type = self.check_jsx_attribute(attribute_decl, check_mode);
                    object_flags |= self.type_(expr_type).object_flags & ObjectFlags::PROPAGATING_FLAGS;
                    let member_flags = self.program_symbol(member).flags;
                    let member_name = self.program_symbol(member).name.clone();
                    let attribute_symbol = self.new_symbol(
                        SymbolFlags::PROPERTY.union(member_flags),
                        &member_name,
                    );
                    {
                        // PORT: the member's snapshot is cloned first (the
                        // &mut symbol borrow can't coexist with program
                        // reads; Go mutates through pointers).
                        let (declarations, member_parent, member_value_declaration) = {
                            let m = self.program_symbol(member);
                            (m.declarations.clone(), m.parent, m.value_declaration)
                        };
                        let symbol = self.program_symbol_mut(attribute_symbol);
                        symbol.declarations = declarations;
                        symbol.parent = member_parent;
                        if let Some(value_declaration) = member_value_declaration {
                            symbol.value_declaration = Some(value_declaration);
                        }
                    }
                    {
                        let links = self.value_symbol_links.get(attribute_symbol);
                        links.resolved_type = Some(expr_type);
                        links.target = Some(member);
                    }
                    attributes_table.set(member_name.clone(), attribute_symbol);
                    if let Some(all) = all_attributes_table.as_mut() {
                        all.set(member_name.clone(), attribute_symbol);
                    }
                    let attribute_name_text = {
                        let name = self
                            .store()
                            .node(attribute_decl)
                            .as_jsx_attribute()
                            .expect("data")
                            .name;
                        crate::ast_shims::jsx_name_text(self.store(), name)
                    };
                    if attribute_name_text == jsx_children_property_name {
                        explicitly_specify_children_attribute = true;
                    }
                    if contextual_type.is_some()
                        && check_mode.intersects(CheckMode::INFERENTIAL)
                        && !check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
                        && self.is_context_sensitive(attribute_decl)
                    {
                        let attributes = jsx_attributes_of(self.store(), opening_like_element)
                            .expect("attributes");
                        let inference_context = self
                            .get_inference_context(attributes)
                            .expect("In CheckMode.Inferential we should always have an inference context");
                        // In CheckMode.Inferential we should always have an
                        // inference context (Go comment).
                        let inference_node = {
                            let initializer = self
                                .store()
                                .node(attribute_decl)
                                .as_jsx_attribute()
                                .expect("data")
                                .initializer
                                .expect("context-sensitive attribute initializer");
                            self.store()
                                .node(initializer)
                                .as_jsx_expression()
                                .expect("JsxExpression initializer")
                                .expression
                                .expect("expression")
                        };
                        self.add_intra_expression_inference_site(inference_context, inference_node, expr_type);
                    }
                } else {
                    debug_assert!(self.store().node(attribute_decl).kind == Kind::JsxSpreadAttribute);
                    if !attributes_table.is_empty() {
                        let spread_type = create_jsx_attributes_type!(self);
                        spread = self.get_spread_type(
                            spread,
                            spread_type,
                            attributes_symbol,
                            object_flags,
                            false, /*readonly*/
                        );
                        attributes_table = SymbolTable::new();
                    }
                    let expression = self
                        .store()
                        .node(attribute_decl)
                        .as_jsx_spread_attribute()
                        .expect("data")
                        .expression;
                    let checked =
                        self.check_expression_ex(expression, check_mode & CheckMode::INFERENTIAL);
                    let expr_type = self.get_reduced_type(checked);
                    if crate::utilities::is_type_any(Some(self.type_(expr_type))) {
                        has_spread_any_type = true;
                    }
                    if self.is_valid_spread_type(expr_type) {
                        spread = self.get_spread_type(
                            spread,
                            expr_type,
                            attributes_symbol,
                            object_flags,
                            false, /*readonly*/
                        );
                        if let Some(all) = all_attributes_table.as_ref() {
                            let all = all.clone();
                            self.check_spread_prop_overrides(expr_type, &all, attribute_decl);
                        }
                    } else {
                        self.error(
                            expression,
                            &diag::SPREAD_TYPES_MAY_ONLY_BE_CREATED_FROM_OBJECT_TYPES,
                            &[],
                        );
                        type_to_intersect = match type_to_intersect {
                            Some(t) => {
                                let intersection =
                                    self.get_intersection_type(&[t, expr_type]);
                                Some(intersection)
                            }
                            None => Some(expr_type),
                        };
                    }
                }
            }
            if !has_spread_any_type && !attributes_table.is_empty() {
                let spread_type = create_jsx_attributes_type!(self);
                spread = self.get_spread_type(
                    spread,
                    spread_type,
                    attributes_symbol,
                    object_flags,
                    false, /*readonly*/
                );
            }
        }
        // Go: parentHasSemanticJsxChildren closure — handle children attribute.
        let parent_has_semantic_jsx_children = |c: &Checker, opening_like_element: NodeId| -> bool {
            let parent = c.store().node(opening_like_element).parent.get();
            if parent.is_none() {
                return false;
            }
            let children: Vec<NodeId> = {
                let p = c.store().node(parent);
                match p.kind {
                    Kind::JsxElement => {
                        // We have to check that openingElement of the parent
                        // is the one we are visiting as this may not be true
                        // for selfClosingElement (Go comment).
                        if p.as_jsx_element().expect("data").opening_element == opening_like_element {
                            jsx_children_of(c.store(), parent)
                        } else {
                            Vec::new()
                        }
                    }
                    Kind::JsxFragment => {
                        if p.as_jsx_fragment().expect("data").opening_fragment == opening_like_element {
                            jsx_children_of(c.store(), parent)
                        } else {
                            Vec::new()
                        }
                    }
                    _ => Vec::new(),
                }
            };
            !get_semantic_jsx_children(c.store(), &children).is_empty()
        };
        if parent_has_semantic_jsx_children(self, opening_like_element) {
            let element_parent = self.store().node(opening_like_element).parent.get();
            let child_types = self.check_jsx_children(element_parent, check_mode);
            if !has_spread_any_type
                && jsx_children_property_name != tsc_ast::INTERNAL_SYMBOL_NAME_MISSING
                && !jsx_children_property_name.is_empty()
            {
                // Error if there is a attribute named "children" explicitly
                // specified and children element. This is because children
                // element will overwrite the value from attributes. Note: we
                // will not warn "children" attribute overwritten if
                // "children" attribute is specified in object spread (Go).
                if explicitly_specify_children_attribute {
                    self.error(
                        attribute_parent,
                        &diag::X_0_ARE_SPECIFIED_TWICE_THE_ATTRIBUTE_NAMED_0_WILL_BE_OVERWRITTEN,
                        std::slice::from_ref(&jsx_children_property_name),
                    );
                }
                let mut children_contextual_type: Option<crate::types::TypeId> = None;
                if self.store().node(opening_like_element).kind == Kind::JsxOpeningElement {
                    let attributes = jsx_attributes_of(self.store(), opening_like_element)
                        .expect("attributes");
                    if let Some(contextual_type) = self.get_apparent_type_of_contextual_type(
                        attributes,
                        ContextFlags::NONE,
                    ) {
                        children_contextual_type = self.get_type_of_property_of_contextual_type(
                            contextual_type,
                            &jsx_children_property_name,
                        );
                    }
                }
                // If there are children in the body of JSX element, create
                // dummy attribute "children" with the union of children types
                // so that it will pass the attribute checking process (Go).
                let children_prop_symbol =
                    self.new_symbol(SymbolFlags::PROPERTY, &jsx_children_property_name);
                {
                    // Go: `links.resolvedType = ...` — the match arms call
                    // checker machinery, so the value is computed before the
                    // link borrow (PORT note: evaluation order preserved).
                    let resolved = match child_types.len() {
                        1 => Some(child_types[0]),
                        _ => {
                            // Go: `someType(childrenContextualType,
                            // c.isTupleLikeType)` then the tuple/array default.
                            let is_tuple_like = match children_contextual_type {
                                Some(t) => crate::relater::some_type_with_mut(self, t, |c, s| {
                                    crate::relater::is_tuple_like_type(c, s)
                                }),
                                None => false,
                            };
                            if is_tuple_like {
                                Some(self.create_tuple_type(&child_types))
                            } else {
                                let union = self.get_union_type(&child_types);
                                Some(self.create_array_type(union))
                            }
                        }
                    };
                    self.value_symbol_links.get(children_prop_symbol).resolved_type = resolved;
                }
                // Fake up a property declaration for the children (Go).
                let children_name_node = {
                    let name: Box<str> = jsx_children_property_name.clone().into();
                    Node {
                        kind: Kind::Identifier,
                        flags: NF::NONE,
                        loc: TextRange::undefined(),
                        id: Cell::new(0),
                        parent: Cell::new(NodeId::NONE),
                        data: NodeData::Identifier(Box::new(tsc_ast::Identifier {
                            text: name,
                            flow_node: None,
                        })),
                    }
                };
                let children_name_id = self.program.node_store_mut().alloc(children_name_node);
                let property_signature = Node {
                    kind: Kind::PropertySignature,
                    flags: NF::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: NodeData::PropertySignatureDeclaration(Box::new(
                        tsc_ast::PropertySignatureDeclaration {
                            symbol: Some(children_prop_symbol),
                            modifiers: None,
                            name: children_name_id,
                            postfix_token: None,
                            type_: NodeId::NONE,
                            initializer: NodeId::NONE,
                        },
                    )),
                };
                let value_declaration = self.program.node_store_mut().alloc(property_signature);
                {
                    let store = self.program.node_store_mut();
                    store.node_mut(children_name_id).parent.set(value_declaration);
                    store.node_mut(value_declaration).parent.set(attribute_parent);
                }
                self.program_symbol_mut(children_prop_symbol).value_declaration = Some(value_declaration);
                let mut child_prop_map = SymbolTable::new();
                child_prop_map.set(jsx_children_property_name.clone(), children_prop_symbol);
                let child_prop_type = self.new_anonymous_type_with_members(
                    attributes_symbol,
                    child_prop_map,
                    ObjectFlags::NONE,
                );
                let propagating = self.get_propagating_flags_of_types(&child_types, TypeFlags::NONE);
                spread = self.get_spread_type(
                    spread,
                    child_prop_type,
                    attributes_symbol,
                    object_flags | propagating,
                    false, /*readonly*/
                );
            }
        }
        if has_spread_any_type {
            return self.any_type();
        }
        if let Some(type_to_intersect) = type_to_intersect {
            if spread != self.empty_jsx_object_type() {
                return self.get_intersection_type(&[type_to_intersect, spread]);
            }
            return type_to_intersect;
        }
        if spread == self.empty_jsx_object_type() {
            return create_jsx_attributes_type!(self);
        }
        spread
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Signature/props-type resolution (Go jsx.go:1029-1159)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getUninstantiatedJsxSignaturesOfType(elementType, caller) []*Signature` (jsx.go:1029).
    pub(crate) fn get_uninstantiated_jsx_signatures_of_type(
        &mut self,
        element_type: crate::types::TypeId,
        caller: NodeId,
    ) -> Vec<crate::types::SignatureId> {
        let flags = self.type_(element_type).flags;
        if flags.intersects(TypeFlags::STRING) {
            return vec![self.any_signature.expect("anySignature")];
        }
        if flags.intersects(TypeFlags::STRING_LITERAL) {
            let intrinsic_type = self
                .get_intrinsic_attributes_type_from_string_literal_type(element_type, caller);
            let Some(intrinsic_type) = intrinsic_type else {
                let literal_value =
                    crate::relater::get_string_literal_value(self, element_type);
                self.error(
                    caller,
                    &diag::PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                    &[
                        literal_value,
                        format!("JSX.{}", jsx_names::INTRINSIC_ELEMENTS),
                    ],
                );
                return Vec::new();
            };
            let fake_signature = self.create_signature_for_jsx_intrinsic(caller, intrinsic_type);
            return vec![fake_signature];
        }
        let apparent_elem_type = self.get_apparent_type(element_type);
        // Resolve the signatures, preferring constructor (Go comment).
        let mut signatures =
            self.get_signatures_of_type(apparent_elem_type, crate::types::SignatureKind::Construct);
        if signatures.is_empty() {
            // No construct signatures, try call signatures (Go comment).
            signatures =
                self.get_signatures_of_type(apparent_elem_type, crate::types::SignatureKind::Call);
        }
        if signatures.is_empty()
            && self.type_(apparent_elem_type).flags.intersects(TypeFlags::UNION)
        {
            // If each member has some combination of new/call signatures;
            // make a union signature list for those (Go comment).
            let constituents: Vec<crate::types::TypeId> =
                self.type_(apparent_elem_type).types().to_vec();
            let signature_lists: Vec<Vec<crate::types::SignatureId>> = constituents
                .iter()
                .map(|&t| self.get_uninstantiated_jsx_signatures_of_type(t, caller))
                .collect();
            return self.get_union_signatures(&signature_lists);
        }
        signatures
    }

    /// Go: `func (c *Checker) getEffectiveFirstArgumentForJsxSignature(signature, node) *Type` (jsx.go:1060).
    pub(crate) fn get_effective_first_argument_for_jsx_signature(
        &mut self,
        signature: crate::types::SignatureId,
        node: NodeId,
    ) -> crate::types::TypeId {
        if self.store().node(node).kind == Kind::JsxOpeningFragment
            || self.get_jsx_reference_kind(node) != JsxReferenceKind::Component
        {
            return self.get_jsx_props_type_from_call_signature(signature, node);
        }
        self.get_jsx_props_type_from_class_type(signature, node)
    }

    /// Go: `func (c *Checker) getJsxPropsTypeFromCallSignature(sig, context) *Type` (jsx.go:1067).
    fn get_jsx_props_type_from_call_signature(
        &mut self,
        sig: crate::types::SignatureId,
        context: NodeId,
    ) -> crate::types::TypeId {
        let unknown_type = self.unknown_type();
        let props_type =
            self.get_type_of_first_parameter_of_signature_with_fallback(sig, unknown_type);
        let ns = self.get_jsx_namespace_at(Some(context));
        let props_type = self.get_jsx_managed_attributes_from_located_attributes(context, ns, props_type);
        let intrinsic_attribs = self.get_jsx_type(jsx_names::INTRINSIC_ATTRIBUTES, Some(context));
        if !self.is_error_type(intrinsic_attribs) {
            return crate::relater::intersect_types(self, intrinsic_attribs, props_type)
                .unwrap_or_else(|| self.error_type());
        }
        props_type
    }

    /// Go: `func (c *Checker) getJsxPropsTypeFromClassType(sig, context) *Type` (jsx.go:1079).
    fn get_jsx_props_type_from_class_type(
        &mut self,
        sig: crate::types::SignatureId,
        context: NodeId,
    ) -> crate::types::TypeId {
        let ns = self.get_jsx_namespace_at(Some(context));
        let forced_lookup_location = self.get_jsx_element_properties_name(ns);
        let attributes_type: Option<crate::types::TypeId>;
        let unknown_type = self.unknown_type();
        match forced_lookup_location.as_str() {
            tsc_ast::INTERNAL_SYMBOL_NAME_MISSING => {
                attributes_type =
                    Some(self.get_type_of_first_parameter_of_signature_with_fallback(sig, unknown_type));
            }
            "" => {
                attributes_type = Some(self.get_return_type_of_signature(sig));
            }
            _ => {
                attributes_type =
                    self.get_jsx_props_type_for_signature_from_member(sig, &forced_lookup_location);
                let has_attributes = {
                    let attributes = jsx_attributes_of(self.store(), context)
                        .expect("attributes");
                    !jsx_properties_of(self.store(), attributes).is_empty()
                };
                if attributes_type.is_none() && has_attributes {
                    // There is no property named 'props' on this instance
                    // type (Go comment).
                    self.error(
                        context,
                        &diag::JSX_ELEMENT_CLASS_DOES_NOT_SUPPORT_ATTRIBUTES_BECAUSE_IT_DOES_NOT_HAVE_A_0_PROPERTY,
                        std::slice::from_ref(&forced_lookup_location),
                    );
                }
            }
        }
        let Some(mut attributes_type) = attributes_type else {
            return unknown_type;
        };
        attributes_type =
            self.get_jsx_managed_attributes_from_located_attributes(context, ns, attributes_type);
        if crate::utilities::is_type_any(Some(self.type_(attributes_type))) {
            // Props is of type 'any' or unknown (Go comment).
            return attributes_type;
        }
        // Normal case -- add in IntrinsicClassAttributes<T> and
        // IntrinsicAttributes (Go comment).
        let mut apparent_attributes_type = attributes_type;
        let intrinsic_class_attribs = self.get_jsx_type(jsx_names::INTRINSIC_CLASS_ATTRIBUTES, Some(context));
        if !self.is_error_type(intrinsic_class_attribs) {
            let type_params = self
                .type_(intrinsic_class_attribs)
                .symbol
                .map(|symbol| {
                    self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol)
                })
                .unwrap_or_default();
            let host_class_type = self.get_return_type_of_signature(sig);
            
            let library_managed_attribute_type: crate::types::TypeId = if !type_params.is_empty() {
                // apply JSX.IntrinsicClassAttributes<hostClassType, ...> (Go).
                let min_type_argument_count = self.get_min_type_argument_count(&type_params);
                let in_js = tsc_ast::utilities::is_in_js_file(self.store().node(context));
                let inferred_args = self.fill_missing_type_arguments(
                    &[host_class_type],
                    &type_params,
                    min_type_argument_count,
                    in_js,
                );
                let mapper = self.new_type_mapper(&type_params, &inferred_args);
                self.instantiate_type(intrinsic_class_attribs, mapper)
            } else {
                intrinsic_class_attribs
            };
            apparent_attributes_type = crate::relater::intersect_types(
                self,
                library_managed_attribute_type,
                apparent_attributes_type,
            )
            .unwrap_or_else(|| self.error_type());
        }
        let intrinsic_attribs = self.get_jsx_type(jsx_names::INTRINSIC_ATTRIBUTES, Some(context));
        if !self.is_error_type(intrinsic_attribs) {
            apparent_attributes_type = crate::relater::intersect_types(
                self,
                intrinsic_attribs,
                apparent_attributes_type,
            )
            .unwrap_or_else(|| self.error_type());
        }
        apparent_attributes_type
    }

    /// Go: `func (c *Checker) getJsxPropsTypeForSignatureFromMember(sig, forcedLookupLocation) *Type` (jsx.go:1121).
    fn get_jsx_props_type_for_signature_from_member(
        &mut self,
        sig: crate::types::SignatureId,
        forced_lookup_location: &str,
    ) -> Option<crate::types::TypeId> {
        let composite = self.signatures[sig.0 as usize].composite.clone();
        if let Some(composite) = composite {
            // JSX Elements using the legacy `props`-field based lookup (eg,
            // react class components) need to treat the `props` member as an
            // input instead of an output position when resolving the
            // signature. We need to go back to the input signatures of the
            // composite signature, get the type of `props` on each return type
            // individually, and then _intersect them_, rather than union them
            // (as would normally occur for a union signature). It's an
            // unfortunate quirk of looking in the output of the signature
            // for the type we want to use for the input. The default behavior
            // of `getTypeOfFirstParameterOfSignatureWithFallback` when no
            // `props` member name is defined is much more sane (Go comment).
            let mut results: Vec<crate::types::TypeId> = Vec::new();
            for &signature in &composite.signatures {
                let instance = self.get_return_type_of_signature(signature);
                if crate::utilities::is_type_any(Some(self.type_(instance))) {
                    return Some(instance);
                }
                let prop_type = self.get_type_of_property_of_type(instance, forced_lookup_location);
                let prop_type = prop_type?;
                results.push(prop_type);
            }
            // Same result for both union and intersection signatures (Go).
            return Some(self.get_intersection_type(&results));
        }
        let instance_type = self.get_return_type_of_signature(sig);
        if crate::utilities::is_type_any(Some(self.type_(instance_type))) {
            return Some(instance_type);
        }
        self.get_type_of_property_of_type(instance_type, forced_lookup_location)
    }

    /// Go: `func (c *Checker) getJsxManagedAttributesFromLocatedAttributes(context, ns, attributesType) *Type` (jsx.go:1161).
    fn get_jsx_managed_attributes_from_located_attributes(
        &mut self,
        context: NodeId,
        ns: Option<SymbolId>,
        attributes_type: crate::types::TypeId,
    ) -> crate::types::TypeId {
        let managed_sym = self.get_jsx_library_managed_attributes(ns);
        if let Some(managed_sym) = managed_sym {
            let ctor_type = self.get_static_type_of_referenced_jsx_constructor(context);
            let in_js = tsc_ast::utilities::is_in_js_file(self.store().node(context));
            let result = self.instantiate_alias_or_interface_with_defaults(
                managed_sym,
                &[ctor_type, attributes_type],
                in_js,
            );
            if let Some(result) = result {
                return result;
            }
        }
        attributes_type
    }

    /// Go: `func (c *Checker) instantiateAliasOrInterfaceWithDefaults(managedSym, typeArguments, inJavaScript) *Type` (jsx.go:1171).
    fn instantiate_alias_or_interface_with_defaults(
        &mut self,
        managed_sym: SymbolId,
        type_arguments: &[crate::types::TypeId],
        in_javascript: bool,
    ) -> Option<crate::types::TypeId> {
        let declared_managed_type = crate::relater::get_declared_type_of_symbol(self, managed_sym);
        // fetches interface type, or initializes symbol links type parameters
        // (Go comment).
        if self.program_symbol(managed_sym).flags.intersects(SymbolFlags::TYPE_ALIAS) {
            let params = self.type_alias_links.get(managed_sym).type_parameters.clone();
            if params.len() >= type_arguments.len() {
                let args = self.fill_missing_type_arguments(
                    type_arguments,
                    &params,
                    type_arguments.len(),
                    in_javascript,
                );
                if args.is_empty() {
                    return Some(declared_managed_type);
                }
                return Some(self.get_type_alias_instantiation(managed_sym, args, None));
            }
        }
        let interface_type_parameters: Vec<crate::types::TypeId> = self
            .type_(declared_managed_type)
            .as_interface_type()
            .map(|i| i.all_type_parameters.clone())
            .unwrap_or_default();
        if self.type_(declared_managed_type).object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
            && interface_type_parameters.len() >= type_arguments.len()
        {
            let args = self.fill_missing_type_arguments(
                type_arguments,
                &interface_type_parameters,
                type_arguments.len(),
                in_javascript,
            );
            return Some(self.create_type_reference(declared_managed_type, args));
        }
        None
    }

    /// Go: `func (c *Checker) getJsxLibraryManagedAttributes(jsxNamespace) *ast.Symbol` (jsx.go:1152).
    fn get_jsx_library_managed_attributes(
        &mut self,
        jsx_namespace: Option<SymbolId>,
    ) -> Option<SymbolId> {
        let jsx_namespace = jsx_namespace?;
        let exports = self.get_exports_of_symbol(jsx_namespace);
        self.get_symbol(&exports, jsx_names::LIBRARY_MANAGED_ATTRIBUTES, SymbolFlags::TYPE)
    }

    /// Go: `func (c *Checker) getJsxElementTypeSymbol(jsxNamespace) *ast.Symbol` (jsx.go:1161/1058).
    fn get_jsx_element_type_symbol(&mut self, jsx_namespace: Option<SymbolId>) -> Option<SymbolId> {
        // JSX.ElementType [symbol] (Go comment).
        let jsx_namespace = jsx_namespace?;
        let exports = self.get_exports_of_symbol(jsx_namespace);
        self.get_symbol(&exports, jsx_names::ELEMENT_TYPE, SymbolFlags::TYPE)
    }

    /// Go: `func (c *Checker) getJsxElementPropertiesName(jsxNamespace) string` (jsx.go:1103).
    fn get_jsx_element_properties_name(&mut self, jsx_namespace: Option<SymbolId>) -> String {
        self.get_name_from_jsx_element_attributes_container(
            jsx_names::ELEMENT_ATTRIBUTES_PROPERTY_NAME_CONTAINER,
            jsx_namespace,
        )
    }

    /// Go: `func (c *Checker) getJsxElementChildrenPropertyName(jsxNamespace) string` (jsx.go:1112).
    fn get_jsx_element_children_property_name(&mut self, jsx_namespace: Option<SymbolId>) -> String {
        if self.compiler_options.jsx == JsxEmit::ReactJSX
            || self.compiler_options.jsx == JsxEmit::ReactJSXDev
        {
            // In these JsxEmit modes the children property is fixed to
            // 'children' (Go comment).
            return "children".to_string();
        }
        self.get_name_from_jsx_element_attributes_container(
            jsx_names::ELEMENT_CHILDREN_ATTRIBUTE_NAME_CONTAINER,
            jsx_namespace,
        )
    }

    /// Go: `func (c *Checker) getNameFromJsxElementAttributesContainer(nameOfAttribPropContainer, jsxNamespace) string` (jsx.go:1132).
    fn get_name_from_jsx_element_attributes_container(
        &mut self,
        name_of_attrib_prop_container: &str,
        jsx_namespace: Option<SymbolId>,
    ) -> String {
        // JSX.ElementAttributesProperty | JSX.ElementChildrenAttribute
        // [symbol] (Go comment).
        let Some(jsx_namespace) = jsx_namespace else {
            return tsc_ast::INTERNAL_SYMBOL_NAME_MISSING.to_string();
        };
        let exports = self.get_exports_of_symbol(jsx_namespace);
        let jsx_element_attrib_prop_interface_sym =
            self.get_symbol(&exports, name_of_attrib_prop_container, SymbolFlags::TYPE);
        if let Some(jsx_element_attrib_prop_interface_sym) = jsx_element_attrib_prop_interface_sym {
            let jsx_element_attrib_prop_interface_type =
                crate::relater::get_declared_type_of_symbol(self, jsx_element_attrib_prop_interface_sym);
            let properties_of_jsx_element_attrib_prop_interface =
                crate::relater::get_properties_of_type(self, jsx_element_attrib_prop_interface_type);
            // Element Attributes has zero properties, so the element
            // attributes type will be the class instance type (Go).
            if properties_of_jsx_element_attrib_prop_interface.is_empty() {
                return String::new();
            }
            if properties_of_jsx_element_attrib_prop_interface.len() == 1 {
                return self
                    .program_symbol(properties_of_jsx_element_attrib_prop_interface[0])
                    .name
                    .clone();
            }
            if properties_of_jsx_element_attrib_prop_interface.len() > 1 {
                let declarations = self
                    .program_symbol(jsx_element_attrib_prop_interface_sym)
                    .declarations
                    .clone();
                if !declarations.is_empty() {
                    // More than one property on ElementAttributesProperty is
                    // an error (Go comment).
                    self.error(
                        declarations[0],
                        &diag::THE_GLOBAL_TYPE_JSX_0_MAY_NOT_HAVE_MORE_THAN_ONE_PROPERTY,
                        &[name_of_attrib_prop_container.to_string()],
                    );
                }
            }
        }
        tsc_ast::INTERNAL_SYMBOL_NAME_MISSING.to_string()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Intrinsic elements + the element-type/namespace family (Go jsx.go:1161-1489)
// ────────────────────────────────────────────────────────────────────────────

impl Checker {
    /// Go: `func (c *Checker) getStaticTypeOfReferencedJsxConstructor(context) *Type` (jsx.go:1178).
    fn get_static_type_of_referenced_jsx_constructor(
        &mut self,
        context: NodeId,
    ) -> crate::types::TypeId {
        if self.store().node(context).kind == Kind::JsxOpeningFragment {
            return self.get_jsx_fragment_type(context);
        }
        let tag_name = jsx_tag_name_of(self.store(), context).expect("tag name");
        if crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name) {
            let result = self.get_intrinsic_attributes_type_from_jsx_opening_like_element(context);
            let fake_signature = self.create_signature_for_jsx_intrinsic(context, result);
            return self.get_or_create_type_from_signature(fake_signature);
        }
        let tag_type = self.check_expression_cached(tag_name);
        if self.type_(tag_type).flags.intersects(TypeFlags::STRING_LITERAL) {
            let result = self.get_intrinsic_attributes_type_from_string_literal_type(tag_type, context);
            let Some(result) = result else {
                return self.error_type();
            };
            let fake_signature = self.create_signature_for_jsx_intrinsic(context, result);
            return self.get_or_create_type_from_signature(fake_signature);
        }
        tag_type
    }

    /// Go: `func (c *Checker) getIntrinsicAttributesTypeFromStringLiteralType(t, location) *Type` (jsx.go:1202).
    fn get_intrinsic_attributes_type_from_string_literal_type(
        &mut self,
        t: crate::types::TypeId,
        location: NodeId,
    ) -> Option<crate::types::TypeId> {
        // If the elemType is a stringLiteral type, we can then provide a
        // check to make sure that the string literal type is one of the Jsx
        // intrinsic element type. For example:
        //      var CustomTag: "h1" = "h1";
        //      <CustomTag> Hello World </CustomTag> (Go comment).
        let intrinsic_elements_type = self.get_jsx_type(jsx_names::INTRINSIC_ELEMENTS, Some(location));
        if !self.is_error_type(intrinsic_elements_type) {
            let string_literal_type_name = crate::relater::get_string_literal_value(self, t);
            let intrinsic_prop =
                self.get_property_of_type(intrinsic_elements_type, &string_literal_type_name);
            if let Some(intrinsic_prop) = intrinsic_prop {
                return Some(self.get_type_of_symbol(intrinsic_prop));
            }
            let string_type = self.string_type();
            let index_signature_type =
                self.get_index_type_of_type(intrinsic_elements_type, string_type);
            if let Some(index_signature_type) = index_signature_type {
                return Some(index_signature_type);
            }
            return None;
        }
        // If we need to report an error, we already done so here. So just
        // return any to prevent any more error downstream (Go comment).
        Some(self.any_type())
    }

    /// Go: `func (c *Checker) getJsxReferenceKind(node) JsxReferenceKind` (jsx.go:1228).
    pub(crate) fn get_jsx_reference_kind(&mut self, node: NodeId) -> JsxReferenceKind {
        let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
        if crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name) {
            return JsxReferenceKind::Mixed;
        }
        let tag_type = {
            let t = self.check_expression(tag_name);
            self.get_apparent_type(t)
        };
        if !self
            .get_signatures_of_type(tag_type, crate::types::SignatureKind::Construct)
            .is_empty()
        {
            return JsxReferenceKind::Component;
        }
        if !self
            .get_signatures_of_type(tag_type, crate::types::SignatureKind::Call)
            .is_empty()
        {
            return JsxReferenceKind::Function;
        }
        JsxReferenceKind::Mixed
    }

    /// Go: `func (c *Checker) createSignatureForJSXIntrinsic(node, result) *Signature` (jsx.go:1248).
    fn create_signature_for_jsx_intrinsic(
        &mut self,
        node: NodeId,
        result: crate::types::TypeId,
    ) -> crate::types::SignatureId {
        let error_type = self.error_type();
        let element_type = match self.get_jsx_namespace_at(Some(node)) {
            Some(namespace) => {
                let exports = self.get_exports_of_symbol(namespace);
                self.get_symbol(&exports, jsx_names::ELEMENT, SymbolFlags::TYPE)
                    .map(|type_symbol| crate::relater::get_declared_type_of_symbol(self, type_symbol))
                    .unwrap_or(error_type)
            }
            None => error_type,
        };
        let parameter_symbol = self.new_symbol(SymbolFlags::FUNCTION_SCOPED_VARIABLE, "props");
        self.value_symbol_links.get(parameter_symbol).resolved_type = Some(result);
        self.new_signature(
            SignatureFlags::NONE,
            None,             /*declaration*/
            Vec::new(),       /*typeParameters*/
            None,             /*thisParameter*/
            vec![parameter_symbol],
            Some(element_type),
            1, /*minArgumentCount*/
        )
    }

    /// Go: `func (c *Checker) getIntrinsicAttributesTypeFromJsxOpeningLikeElement(node) *Type` (jsx.go:1266).
    fn get_intrinsic_attributes_type_from_jsx_opening_like_element(
        &mut self,
        node: NodeId,
    ) -> crate::types::TypeId {
        let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
        debug_assert!(crate::utilities::is_jsx_intrinsic_tag_name(self.store(), tag_name));
        if let Some(resolved) = self
            .jsx_element_links
            .get(node)
            .resolved_jsx_element_attributes_type
        {
            return resolved;
        }
        let symbol = self.get_intrinsic_tag_symbol(node);
        let links_flags = self.jsx_element_links.get(node).jsx_flags;
        if links_flags.intersects(JsxFlags::INTRINSIC_NAMED_ELEMENT) {
            // Go: `core.OrElse(c.getTypeOfSymbol(symbol), c.errorType)` —
            // getTypeOfSymbol never answers nil, so the OrElse arm is Go's
            // belt-and-braces (mirrored by the direct read).
            let resolved = self.get_type_of_symbol(symbol);
            self.jsx_element_links.get(node).resolved_jsx_element_attributes_type = Some(resolved);
            return resolved;
        }
        if links_flags.intersects(JsxFlags::INTRINSIC_INDEXED_ELEMENT) {
            let intrinsic_elements_type = self.get_jsx_type(jsx_names::INTRINSIC_ELEMENTS, Some(node));
            // PORT: FCS tranche 52 — Go jsx.go:1200 passes
            // `node.TagName().Text()` (decoded), not the raw source span.
            let tag_name_text = crate::ast_shims::jsx_name_text(self.store(), tag_name);
            let index_info = crate::relater::get_applicable_index_info_for_name(
                self,
                intrinsic_elements_type,
                &tag_name_text,
            );
            if let Some(index_info) = index_info {
                let value_type = self.index_infos[index_info.0 as usize]
                    .value_type
                    .unwrap_or_else(|| self.error_type());
                self.jsx_element_links.get(node).resolved_jsx_element_attributes_type =
                    Some(value_type);
                return value_type;
            }
        }
        let error_type = self.error_type();
        self.jsx_element_links.get(node).resolved_jsx_element_attributes_type = Some(error_type);
        error_type
    }

    /// Go: `func (c *Checker) getIntrinsicTagSymbol(node) *ast.Symbol` (jsx.go:1291).
    fn get_intrinsic_tag_symbol(&mut self, node: NodeId) -> SymbolId {
        if let Some(resolved_symbol) = self.symbol_node_links.get(node).resolved_symbol {
            return resolved_symbol;
        }
        let intrinsic_elements_type = self.get_jsx_type(jsx_names::INTRINSIC_ELEMENTS, Some(node));
        if !self.is_error_type(intrinsic_elements_type) {
            // Property case (Go comment).
            let tag_name = jsx_tag_name_of(self.store(), node).expect("tag name");
            let tag_name_node = self.store().node(tag_name);
            if !(tag_name_node.kind == Kind::Identifier
                || tag_name_node.kind == Kind::JsxNamespacedName)
            {
                panic!("Invalid tag name");
            }
            // PORT: FCS tranche 52 — Go jsx.go:1228 `propName := tagName.Text()`
            // (decoded identifier text; the raw source span kept `\u0061` and
            // the IntrinsicElements lookup missed — unicodeEscapesInJsxtags).
            let prop_name = crate::ast_shims::jsx_name_text(self.store(), tag_name);
            let intrinsic_prop = self.get_property_of_type(intrinsic_elements_type, &prop_name);
            if let Some(intrinsic_prop) = intrinsic_prop {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_NAMED_ELEMENT;
                self.symbol_node_links.get(node).resolved_symbol = Some(intrinsic_prop);
                return intrinsic_prop;
            }
            // Intrinsic string indexer case (Go comment).
            let string_literal_type = self.get_string_literal_type(&prop_name);
            let index_symbol = self.get_applicable_index_symbol(intrinsic_elements_type, string_literal_type);
            if let Some(index_symbol) = index_symbol {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_INDEXED_ELEMENT;
                self.symbol_node_links.get(node).resolved_symbol = Some(index_symbol);
                return index_symbol;
            }
            let prop_or_index = crate::relater::get_type_of_property_or_index_signature_of_type(
                self,
                intrinsic_elements_type,
                &prop_name,
            );
            if prop_or_index.is_some() {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_INDEXED_ELEMENT;
                let symbol = self.type_(intrinsic_elements_type)
                    .symbol
                    .unwrap_or_else(|| self.unknown_symbol());
                self.symbol_node_links.get(node).resolved_symbol = Some(symbol);
                return symbol;
            }
            // Wasn't found (Go comment).
            self.error(
                node,
                &diag::PROPERTY_0_DOES_NOT_EXIST_ON_TYPE_1,
                &[prop_name, format!("JSX.{}", jsx_names::INTRINSIC_ELEMENTS)],
            );
            let unknown_symbol = self.unknown_symbol();
            self.symbol_node_links.get(node).resolved_symbol = Some(unknown_symbol);
            return unknown_symbol;
        }
        if self.no_implicit_any {
            self.error(
                node,
                &diag::JSX_ELEMENT_IMPLICITLY_HAS_TYPE_ANY_BECAUSE_NO_INTERFACE_JSX_0_EXISTS,
                &[jsx_names::INTRINSIC_ELEMENTS.to_string()],
            );
        }
        let unknown_symbol = self.unknown_symbol();
        self.symbol_node_links.get(node).resolved_symbol = Some(unknown_symbol);
        unknown_symbol
    }

    /// Go: `func (c *Checker) getApplicableIndexSymbol(t, keyType) *ast.Symbol` (checker.go:32639) —
    /// the S10 helper; the only checker-internal caller is
    /// `getIntrinsicTagSymbol` (jsx.go), so it lands with this wave.
    fn get_applicable_index_symbol(
        &mut self,
        t: crate::types::TypeId,
        key_type: crate::types::TypeId,
    ) -> Option<SymbolId> {
        let any_base_type_index_info = self.any_base_type_index_info;
        let info = crate::relater::get_applicable_index_info(self, t, key_type);
        if let Some(info) = info.filter(|&i| Some(i) != any_base_type_index_info) {
            if self.index_infos[info.0 as usize].index_symbol.is_none() {
                // Go: the declaration list — the info's own declaration, or
                // every applicable index signature declaration of the type.
                let mut declarations: Vec<NodeId> = Vec::new();
                if let Some(declaration) = self.index_infos[info.0 as usize].declaration {
                    declarations.push(declaration);
                } else {
                    for i in crate::relater::get_index_infos_of_type(self, t) {
                        if let Some(declaration) = self.index_infos[i.0 as usize].declaration {
                            let is_applicable = {
                                let key = self.index_infos[i.0 as usize]
                                    .key_type
                                    .unwrap_or_else(|| self.any_type());
                                self.is_applicable_index_type(key_type, key)
                            };
                            if is_applicable {
                                declarations.push(declaration);
                            }
                        }
                    }
                }
                if !declarations.is_empty() {
                    let symbol = self.new_symbol(SymbolFlags::PROPERTY, tsc_ast::INTERNAL_SYMBOL_NAME_INDEX);
                    self.program_symbol_mut(symbol).check_flags |=
                        tsc_ast::CheckFlags::INDEX_SYMBOL;
                    self.program_symbol_mut(symbol).declarations = declarations.clone();
                    self.program_symbol_mut(symbol).value_declaration = Some(declarations[0]);
                    self.program_symbol_mut(symbol).parent =
                        self.type_(t).symbol;
                    let value_type = self.index_infos[info.0 as usize]
                        .value_type
                        .unwrap_or_else(|| self.error_type());
                    self.value_symbol_links.get(symbol).resolved_type = Some(value_type);
                    self.index_infos[info.0 as usize].index_symbol = Some(symbol);
                }
            }
            return self.index_infos[info.0 as usize].index_symbol;
        }
        None
    }

    // ── The element-type bound family (Go jsx.go:1336-1352) ──

    /// Go: `func (c *Checker) getJsxStatelessElementTypeAt(location) *Type` (jsx.go:1336).
    fn get_jsx_stateless_element_type_at(
        &mut self,
        location: NodeId,
    ) -> Option<crate::types::TypeId> {
        let jsx_element_type = self.get_jsx_element_type_at(location);
        let null_type = self.null_type();
        Some(self.get_union_type(&[jsx_element_type, null_type]))
    }

    /// Go: `func (c *Checker) getJsxElementClassTypeAt(location) *Type` (jsx.go:1343).
    fn get_jsx_element_class_type_at(&mut self, location: NodeId) -> Option<crate::types::TypeId> {
        let t = self.get_jsx_type(jsx_names::ELEMENT_CLASS, Some(location));
        if self.is_error_type(t) {
            return None;
        }
        Some(t)
    }

    /// Go: `func (c *Checker) getJsxElementTypeAt(location) *Type` (jsx.go:1350).
    pub(crate) fn get_jsx_element_type_at(&mut self, location: NodeId) -> crate::types::TypeId {
        self.get_jsx_type(jsx_names::ELEMENT, Some(location))
    }

    /// Go: `func (c *Checker) getJsxElementTypeTypeAt(location) *Type` (jsx.go:1354).
    fn get_jsx_element_type_type_at(&mut self, location: NodeId) -> Option<crate::types::TypeId> {
        let ns = self.get_jsx_namespace_at(Some(location))?;
        let sym = self.get_jsx_element_type_symbol(Some(ns))?;
        let in_js = tsc_ast::utilities::is_in_js_file(self.store().node(location));
        let t = self.instantiate_alias_or_interface_with_defaults(sym, &[], in_js);
        
        t.filter(|&t| !self.is_error_type(t))
    }

    /// Go: `func (c *Checker) getJsxType(name, location) *Type` (jsx.go:1362).
    pub(crate) fn get_jsx_type(&mut self, name: &str, location: Option<NodeId>) -> crate::types::TypeId {
        if let Some(namespace) = self.get_jsx_namespace_at(location) {
            let exports = self.get_exports_of_symbol(namespace);
            if let Some(type_symbol) = self.get_symbol(&exports, name, SymbolFlags::TYPE) {
                return crate::relater::get_declared_type_of_symbol(self, type_symbol);
            }
        }
        self.error_type()
    }

    /// Go: `func (c *Checker) getJsxNamespaceAt(location) *ast.Symbol` (jsx.go:1371).
    pub(crate) fn get_jsx_namespace_at(&mut self, location: Option<NodeId>) -> Option<SymbolId> {
        let unknown_symbol = self.unknown_symbol();
        let cached = location.map(|l| self.jsx_element_links.get(l).jsx_namespace);
        if let Some(Some(jsx_namespace)) = cached.filter(|ns| *ns != Some(unknown_symbol)) {
            return Some(jsx_namespace);
        }
        // Go: `if links == nil || links.jsxNamespace != c.unknownSymbol` —
        // the resolution half runs only when there are no links yet, or the
        // cached namespace isn't the unknown marker.
        if cached.is_none() || cached != Some(Some(unknown_symbol)) {
            let mut resolved_namespace =
                self.get_jsx_namespace_container_for_implicit_import(location);
            if resolved_namespace.is_none() || resolved_namespace == Some(unknown_symbol) {
                let namespace_name = self.get_jsx_namespace_string(location);
                resolved_namespace = self.resolve_name_worker(
                    location,
                    &namespace_name,
                    SymbolFlags::NAMESPACE,
                    None, /*nameNotFoundMessage*/
                    false, /*isUse*/
                    false, /*excludeGlobals*/
                );
            }
            if let Some(resolved_namespace) = resolved_namespace {
                let resolved = self.resolve_symbol(resolved_namespace);
                let exports = self.get_exports_of_symbol(resolved);
                let candidate = self
                    .get_symbol(&exports, jsx_names::JSX, SymbolFlags::NAMESPACE)
                    .map(|s| self.resolve_symbol(s));
                if let Some(candidate) = candidate.filter(|&c| c != unknown_symbol) {
                    if let Some(location) = location {
                        self.jsx_element_links.get(location).jsx_namespace = Some(candidate);
                    }
                    return Some(candidate);
                }
            }
            if let Some(location) = location {
                self.jsx_element_links.get(location).jsx_namespace = Some(unknown_symbol);
            }
        }
        // JSX global fallback (Go comment).
        
        self
            .get_global_symbol(jsx_names::JSX, SymbolFlags::NAMESPACE, None /*diagnostic*/)
            .map(|s| self.resolve_symbol(s))
            .filter(|&s| s != unknown_symbol)
    }

    /// Go: `func (c *Checker) getJsxNamespace(location) string` (jsx.go:1404) —
    /// PORT: renamed `get_jsx_namespace_string` (the public
    /// `GetJsxNamespace` carrier in exports.rs owns the Go-colliding name).
    pub(crate) fn get_jsx_namespace_string(&mut self, location: Option<NodeId>) -> String {
        if let Some(location) = location {
            let file_id = tsc_ast::utilities::get_source_file_of_node(self.store(), location)
                .map(|f| f.as_node_id());
            if let Some(file_id) = file_id {
                if self.store().node(location).kind == Kind::JsxOpeningFragment {
                    let links_namespace = self
                        .source_file_links
                        .get(file_id)
                        .local_jsx_fragment_namespace
                        .clone();
                    if !links_namespace.is_empty() {
                        return links_namespace;
                    }
                    let jsx_fragment_pragma = self
                        .source_file_of(file_id)
                        .and_then(|f| get_pragma_from_source_file(f, "jsxfrag"));
                    if let Some(jsx_fragment_pragma) = jsx_fragment_pragma {
                        let factory = jsx_fragment_pragma
                            .args
                            .get("factory")
                            .map(|a| a.value.clone())
                            .unwrap_or_default();
                        let parsed = self.parse_isolated_entity_name(&factory);
                        if let Some(parsed) = parsed {
                            let first = get_first_identifier(self.store(), parsed);
                            let namespace =
                                tsc_ast::node_text(self.store(), first).to_string();
                            self.source_file_links.get(file_id).local_jsx_fragment_factory = Some(parsed);
                            self.source_file_links.get(file_id).local_jsx_fragment_namespace = namespace.clone();
                            return namespace;
                        }
                    }
                    let entity = self.get_jsx_fragment_factory_entity(Some(location));
                    if let Some(entity) = entity {
                        self.source_file_links.get(file_id).local_jsx_fragment_factory = Some(entity);
                        let first = get_first_identifier(self.store(), entity);
                        let namespace = tsc_ast::node_text(self.store(), first).to_string();
                        self.source_file_links.get(file_id).local_jsx_fragment_namespace = namespace.clone();
                        return namespace;
                    }
                } else {
                    let local_jsx_namespace = self.get_local_jsx_namespace(file_id);
                    if !local_jsx_namespace.is_empty() {
                        self.source_file_links.get(file_id).local_jsx_namespace = local_jsx_namespace.clone();
                        return local_jsx_namespace;
                    }
                }
            }
        }
        if self.jsx_namespace_field.is_empty() {
            self.jsx_namespace_field = "React".to_string();
            if !self.compiler_options.jsx_factory.is_empty() {
                let factory = self.compiler_options.jsx_factory.clone();
                let parsed = self.parse_isolated_entity_name(&factory);
                if let Some(parsed) = parsed {
                    let first = get_first_identifier(self.store(), parsed);
                    self.jsx_namespace_field =
                        tsc_ast::node_text(self.store(), first).to_string();
                    self.jsx_factory_entity_field = Some(parsed);
                }
            } else if !self.compiler_options.react_namespace.is_empty() {
                self.jsx_namespace_field = self.compiler_options.react_namespace.clone();
            }
        }
        if self.jsx_factory_entity_field.is_none() {
            // Go: factory.NewQualifiedName(NewIdentifier(_jsxNamespace),
            // NewIdentifier("createElement")) — the synthetic default
            // factory entity.
            let namespace = self.jsx_namespace_field.clone();
            let namespace_node = {
                let text: Box<str> = namespace.into();
                Node {
                    kind: Kind::Identifier,
                    flags: NF::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: NodeData::Identifier(Box::new(tsc_ast::Identifier {
                        text,
                        flow_node: None,
                    })),
                }
            };
            let namespace_id = self.program.node_store_mut().alloc(namespace_node);
            let right_node = {
                let text: Box<str> = "createElement".into();
                Node {
                    kind: Kind::Identifier,
                    flags: NF::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: NodeData::Identifier(Box::new(tsc_ast::Identifier {
                        text,
                        flow_node: None,
                    })),
                }
            };
            let right_id = self.program.node_store_mut().alloc(right_node);
            let qualified_name = Node {
                kind: Kind::QualifiedName,
                flags: NF::NONE,
                loc: TextRange::undefined(),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: NodeData::QualifiedName(Box::new(tsc_ast::QualifiedName {
                    flow_node: None,
                    facts: 0,
                    left: namespace_id,
                    right: right_id,
                })),
            };
            let qualified_id = self.program.node_store_mut().alloc(qualified_name);
            {
                let store = self.program.node_store_mut();
                store.node_mut(namespace_id).parent.set(qualified_id);
                store.node_mut(right_id).parent.set(qualified_id);
            }
            self.jsx_factory_entity_field = Some(qualified_id);
        }
        self.jsx_namespace_field.clone()
    }

    /// Go: `func (c *Checker) getLocalJsxNamespace(file) string` (jsx.go:1453).
    fn get_local_jsx_namespace(&mut self, file: NodeId) -> String {
        let links_namespace = self.source_file_links.get(file).local_jsx_namespace.clone();
        if !links_namespace.is_empty() {
            return links_namespace;
        }
        let jsx_pragma = self
            .source_file_of(file)
            .and_then(|f| get_pragma_from_source_file(f, "jsx"));
        if let Some(jsx_pragma) = jsx_pragma {
            let factory = jsx_pragma
                .args
                .get("factory")
                .map(|a| a.value.clone())
                .unwrap_or_default();
            let parsed = self.parse_isolated_entity_name(&factory);
            if let Some(parsed) = parsed {
                let first = get_first_identifier(self.store(), parsed);
                let namespace = tsc_ast::node_text(self.store(), first).to_string();
                self.source_file_links.get(file).local_jsx_factory = Some(parsed);
                self.source_file_links.get(file).local_jsx_namespace = namespace.clone();
                return namespace;
            }
        }
        String::new()
    }

    /// Go: `func (c *Checker) getJsxFactoryEntity(location) *ast.Node` (jsx.go:1466).
    pub(crate) fn get_jsx_factory_entity(&mut self, location: Option<NodeId>) -> Option<NodeId> {
        if let Some(location) = location {
            let _ = self.get_jsx_namespace_string(Some(location));
            let file_id = tsc_ast::utilities::get_source_file_of_node(self.store(), location)
                .map(|f| f.as_node_id());
            if let Some(file_id) = file_id {
                let local_jsx_factory = self.source_file_links.get(file_id).local_jsx_factory;
                if local_jsx_factory.is_some() {
                    return local_jsx_factory;
                }
            }
        }
        self.jsx_factory_entity_field
    }

    /// Go: `func (c *Checker) getJsxFragmentFactoryEntity(location) *ast.EntityName` (jsx.go:1476).
    pub(crate) fn get_jsx_fragment_factory_entity(&mut self, location: Option<NodeId>) -> Option<NodeId> {
        if let Some(location) = location {
            let file_id = tsc_ast::utilities::get_source_file_of_node(self.store(), location)
                .map(|f| f.as_node_id());
            if let Some(file_id) = file_id {
                let local_fragment_factory =
                    self.source_file_links.get(file_id).local_jsx_fragment_factory;
                if local_fragment_factory.is_some() {
                    return local_fragment_factory;
                }
                let jsx_frag_pragma = self
                    .source_file_of(file_id)
                    .and_then(|f| get_pragma_from_source_file(f, "jsxfrag"));
                if let Some(jsx_frag_pragma) = jsx_frag_pragma {
                    let factory = jsx_frag_pragma
                        .args
                        .get("factory")
                        .map(|a| a.value.clone())
                        .unwrap_or_default();
                    let parsed = self.parse_isolated_entity_name(&factory);
                    self.source_file_links.get(file_id).local_jsx_fragment_factory = parsed;
                    return parsed;
                }
            }
        }
        if !self.compiler_options.jsx_fragment_factory.is_empty() {
            let factory = self.compiler_options.jsx_fragment_factory.clone();
            return self.parse_isolated_entity_name(&factory);
        }
        None
    }

    /// Go: `func (c *Checker) parseIsolatedEntityName(name) *ast.Node` (jsx.go:1491) —
    /// the subtree-copy PORT shape (see the module PORT note).
    fn parse_isolated_entity_name(&mut self, name: &str) -> Option<NodeId> {
        let (source_file, node_id) = tsc_parser::parse_isolated_entity_name(name)?;
        // Copy the entity-name subtree into the program's node store (the
        // parser's arena is per-file; the checker's nodes live in the program
        // store — Go shares one arena).
        self.copy_entity_name_from(&source_file, node_id)
    }

    /// The entity-name subtree copy (Identifier | QualifiedName) with
    /// synthetic (-1,-1) locs (Go's `markAsSynthetic` on the parse result).
    fn copy_entity_name_from(
        &mut self,
        src: &tsc_ast::SourceFile,
        node_id: NodeId,
    ) -> Option<NodeId> {
        let n = src.node(node_id);
        match (&n.kind, &n.data) {
            (Kind::Identifier, NodeData::Identifier(d)) => {
                let node = Node {
                    kind: Kind::Identifier,
                    flags: NF::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: NodeData::Identifier(Box::new(tsc_ast::Identifier {
                        text: d.text.clone(),
                        flow_node: None,
                    })),
                };
                Some(self.program.node_store_mut().alloc(node))
            }
            (Kind::QualifiedName, NodeData::QualifiedName(d)) => {
                let left = self.copy_entity_name_from(src, d.left)?;
                let right = self.copy_entity_name_from(src, d.right)?;
                let node = Node {
                    kind: Kind::QualifiedName,
                    flags: NF::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: NodeData::QualifiedName(Box::new(tsc_ast::QualifiedName {
                        flow_node: None,
                        facts: 0,
                        left,
                        right,
                    })),
                };
                let qualified_id = self.program.node_store_mut().alloc(node);
                {
                    let store = self.program.node_store_mut();
                    store.node_mut(left).parent.set(qualified_id);
                    store.node_mut(right).parent.set(qualified_id);
                }
                Some(qualified_id)
            }
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getJsxNamespaceContainerForImplicitImport(location) *ast.Symbol` (jsx.go:1511).
    pub(crate) fn get_jsx_namespace_container_for_implicit_import(
        &mut self,
        location: Option<NodeId>,
    ) -> Option<SymbolId> {
        let file = tsc_ast::utilities::get_source_file_of_node(
            self.store(),
            location.expect("jsx locations are always file-bound"),
        )?
        .as_node_id();
        let unknown_symbol = self.unknown_symbol();
        if let Some(jsx_implicit_import_container) =
            self.jsx_element_links.get(file).jsx_implicit_import_container
        {
            return if jsx_implicit_import_container == unknown_symbol {
                None
            } else {
                Some(jsx_implicit_import_container)
            };
        }
        let mut canonical_error_tag = self.jsx_element_links.get(file).first_jsx_tag_in_file;
        if canonical_error_tag.is_none() {
            // Go: the file-wide walk for the first JSX tag (`visit` in
            // getJsxNamespaceContainerForImplicitImport); fragments issue
            // errors on the opening fragment instead of the whole tag (to
            // match strada).
            let mut first_tag: Option<NodeId> = None;
            let file_data = self.store().node(file).as_source_file().expect("file");
            file_data.for_each_child(&mut |child| {
                // PORT(FCS, 2026-10-18): the visitor answers `true` to STOP the
                // walk (Go's visit returns true once firstJSXTagInFile is set).
                // The prior `!` inversion stopped the walk on the first
                // child subtree WITHOUT a JSX tag — files whose first
                // statement isn't JSX found no canonical error tag, and the
                // react-jsx/jsxdev runtime resolution then passed NONE down
                // resolveExternalModule (the 13-record panic family).
                find_first_jsx_tag(self.store(), child, &mut first_tag)
            });
            if let Some(tag) = first_tag {
                self.jsx_element_links.get(file).first_jsx_tag_in_file = Some(tag);
                canonical_error_tag = Some(tag);
            }
        }
        // Go: `if moduleReference == "" { return nil }` — no links write.
        let module_reference = self.program.get_jsx_runtime_import_specifier(file)?;
        if module_reference.is_empty() {
            return None;
        }
        let error_message: &'static Message =
            &diag::THIS_JSX_TAG_REQUIRES_THE_MODULE_PATH_0_TO_EXIST_BUT_NONE_COULD_BE_FOUND_MAKE_SURE_YOU_HAVE_TYPES_FOR_THE_APPROPRIATE_PACKAGE_INSTALLED;
        // Go: `c.resolveExternalModule(core.OrElse(specifier, canonicalErrorTag), ...)`.
        // PORT: the Rust Program trait's JSX-runtime seam returns the module
        // reference string only (the compiler twin's specifier NODE rides
        // the program merge), so the location is the canonical error tag.
        let mod_symbol = self.resolve_external_module(
            canonical_error_tag.unwrap_or(NodeId::NONE),
            &module_reference,
            Some(error_message),
            canonical_error_tag,
            false,
            None, /*importAttributesType*/
        );
        let mut result: Option<SymbolId> = None;
        if let Some(mod_symbol) = mod_symbol.filter(|&m| m != unknown_symbol) {
            let merged = self.resolve_symbol(mod_symbol);
            result = Some(self.get_merged_symbol(merged));
        }
        let stored = result.unwrap_or(unknown_symbol);
        self.jsx_element_links.get(file).jsx_implicit_import_container = Some(stored);
        result
    }
    /// Go: `func (c *Checker) createIterableType(iteratedType) *Type` (checker.go:15036) —
    /// REAL (M6 W5: jsx.go's elaborateJsxComponents any-iterable arm).
    pub(crate) fn create_iterable_type(
        &mut self,
        iterated_type: crate::types::TypeId,
    ) -> crate::types::TypeId {
        let g = self.get_global_iterable_type_checked();
        self.create_type_from_generic_global_type(
            Some(g),
            vec![iterated_type, self.void_type(), self.undefined_type()],
        )
    }
}

/// Go: `links.firstJSXTagInFile = node` — the walk helper
/// (`visit` in getJsxNamespaceContainerForImplicitImport). Returns true when
/// the walk should stop (a first JSX tag was found).
fn find_first_jsx_tag(store: &dyn NodeStore, node: NodeId, first_tag: &mut Option<NodeId>) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::JsxElement | Kind::JsxSelfClosingElement => {
            *first_tag = Some(node);
            true
        }
        Kind::JsxFragment => {
            let opening_fragment = n.as_jsx_fragment().expect("data").opening_fragment;
            *first_tag = Some(opening_fragment);
            true
        }
        _ => {
            // Go: `node.ForEachChild(visit)` — recurse into the children;
            // the visitor answers `true` to stop the walk.
            store.node(node).for_each_child(&mut |child| {
                find_first_jsx_tag(store, child, first_tag)
            })
        }
    }
}
