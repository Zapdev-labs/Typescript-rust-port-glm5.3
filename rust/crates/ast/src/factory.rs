// Ported from tsc/internal/ast/ast.go (NodeFactory core + the token cache) @
// ec47d33c23e464a17cdf2475632cba629bee8763
//
// Hand-written core of the NodeFactory port. The per-kind New*/Update*
// constructors are GENERATED into factory_generated.rs by rust/tools/gen-ast
// from the same tools/scripts/tsc/ast.json that produces Go's
// ast_generated.go; this file holds what Go keeps hand-written in ast.go:
//
//   Go ast.go                                        → here
//   NewNodeFactory(hooks)                            → NodeFactory::new (PORT:
//                                                      the transformer-era hooks
//                                                      OnCreate/OnUpdate/OnClone
//                                                      are deferred — the
//                                                      parser constructs with
//                                                      none; see PORTING-NOTES)
//   f.newNode / nodeCount / textCount                → new_node (pub(crate)) /
//                                                      node_count / text_count
//   NewNodeList / NewModifierList                    → new_node_list /
//                                                      new_modifier_list
//   NewModifier                                      → new_modifier (== new_token)
//   NewSourceFile / UpdateSourceFile                 → new_source_file /
//                                                      update_source_file
//   TokenCacheKey / GetOrCreateToken / createToken   → TokenCacheKey /
//                                                      SourceFile::get_or_create_token
//                                                      / create_token
//   NewCommentRange                                  → source_file::new_comment_range
//                                                      (pre-existing free fn)
//   NodeCount/TextCount fields on SourceFile         → stamped by new_source_file
//                                                      (Go: the parser assigns
//                                                      `result.NodeCount =
//                                                      p.factory.NodeCount()`)
//
// PORT (arena, SPEC §5.1): Go's NodeFactory owns per-kind `core.Arena[T]`
// pools and hands out `*Node` pointers; the Rust factory borrows the
// NodeStore arena seam and returns `NodeId` handles into the owning
// SourceFile. The parser flow is: construct `SourceFile::new(file_id, opts,
// text, None, None)` (the file node is pre-allocated at arena slot 0 — the
// port's SourceFile owns the arena that would hold the payload, so the file
// node must exist before its children are allocated), wrap
// `NodeFactory::new(&mut file)`, build the tree, then call
// `factory.new_source_file(statements, eof)` where Go's parser calls
// `f.NewSourceFile(p.opts, p.sourceText, ...)` — opts/text already live on
// the arena owner here. Flags/loc/id/parent start at the New* zero state;
// the parser's finishNode/parent pass fills them through
// [`NodeFactory::store`].

use std::cell::Cell;

use tsc_core::text::{TextPos, TextRange};

use crate::visitor::{modifier_to_flag, NodeStore};
use crate::{
    Kind, ModifierFlags, ModifierList, Node, NodeData, NodeFlags, NodeList, NodeId, SourceFile,
    TokenFlags,
};

/// Go: `type NodeFactory struct { hooks NodeFactoryHooks; <per-kind arenas> }`.
///
/// PORT: borrows the arena (see the file header) instead of owning pools;
/// `nodeCount`/`textCount` mirror Go's factory fields exactly (they feed the
/// SourceFile's NodeCount/TextCount).
pub struct NodeFactory<'a> {
    pub(crate) store: &'a mut dyn NodeStore,
    pub(crate) node_count: u32,
    pub(crate) text_count: u32,
}

impl<'a> NodeFactory<'a> {
    /// Go: `func NewNodeFactory(hooks NodeFactoryHooks) *NodeFactory` —
    /// PORT: hooks dropped (transformer-era; the parser constructs with none).
    pub fn new(store: &'a mut dyn NodeStore) -> NodeFactory<'a> {
        NodeFactory {
            store,
            node_count: 0,
            text_count: 0,
        }
    }

    /// Go: `func (f *NodeFactory) NodeCount() int`.
    pub fn node_count(&self) -> u32 {
        self.node_count
    }

    /// Go: `func (f *NodeFactory) TextCount() int`.
    pub fn text_count(&self) -> u32 {
        self.text_count
    }

    /// The underlying arena (Go: the parser mutates the `*Node` returned by
    /// New* directly — finishNode sets Loc/Flags; the parent pass sets
    /// Parent). Generated constructors and parser-side finishers share it.
    pub fn store(&mut self) -> &mut dyn NodeStore {
        self.store
    }

    /// Go: `func (f *NodeFactory) newNode(kind Kind, data nodeData) *Node` —
    /// the node shell behind every generated New*: undefined loc, zero id,
    /// nil parent, the Flags the constructor computed.
    pub(crate) fn new_node(&mut self, kind: Kind, flags: NodeFlags, data: NodeData) -> NodeId {
        self.node_count += 1;
        self.store.alloc(Node {
            kind,
            flags,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data,
        })
    }

    /// Go: `func (f *NodeFactory) NewNodeList(nodes []*Node) *NodeList` —
    /// lists are inline values here (SPEC §5.1), so this wraps, not allocates.
    pub fn new_node_list(&mut self, nodes: Box<[NodeId]>) -> NodeList {
        NodeList {
            loc: TextRange::undefined(),
            nodes,
        }
    }

    /// Go: `func (f *NodeFactory) NewModifierList(nodes []*Node) *ModifierList`
    /// (utilities' ModifiersToFlags folded in; nil modifiers — `NodeId::NONE`
    /// — are skipped exactly as Go skips them).
    pub fn new_modifier_list(&mut self, nodes: Box<[NodeId]>) -> ModifierList {
        let mut modifier_flags = ModifierFlags::NONE;
        for &n in &nodes {
            if n == NodeId::NONE {
                continue;
            }
            modifier_flags |= modifier_to_flag(self.store.node(n).kind);
        }
        ModifierList {
            loc: TextRange::undefined(),
            nodes,
            modifier_flags,
        }
    }

    /// Go: `func (f *NodeFactory) NewModifier(kind Kind) *Node { return f.NewToken(kind) }`.
    pub fn new_modifier(&mut self, kind: Kind) -> NodeId {
        self.new_token(kind)
    }

    /// Go: `func (f *NodeFactory) NewSourceFile(opts SourceFileParseOptions, text string,
    /// statements *NodeList, endOfFileToken *TokenNode) *Node` —
    /// PORT: opts/text live on the arena owner (see the file header); this
    /// fills the pre-allocated slot-0 file node and stamps the factory
    /// counters into the payload, exactly where Go's parser assigns
    /// `result.NodeCount = p.factory.NodeCount()` after NewSourceFile.
    pub fn new_source_file(
        &mut self,
        statements: Option<NodeList>,
        end_of_file_token: Option<NodeId>,
    ) -> NodeId {
        // Go: NewSourceFile → f.newNode → nodeCount++ (the file node counts).
        self.node_count += 1;
        let id = NodeId::new(self.store.file_id(), 0);
        let n = self.store.node_mut(id);
        let d = n
            .as_source_file_mut()
            .expect("arena slot 0 is the SourceFile node");
        d.statements = statements;
        d.end_of_file_token = end_of_file_token;
        d.node_count = self.node_count as i32;
        d.text_count = self.text_count as i32;
        id
    }

    /// Go: `func (f *NodeFactory) UpdateSourceFile(node *SourceFile, statements *StatementList,
    /// endOfFileToken *TokenNode) *Node` —
    /// PORT: in-place update. Go allocates a fresh node + `copyFrom` +
    /// `updateNode`; the Rust arena-per-file holds exactly one SourceFile node
    /// (slot 0, the `nodes[0]` invariant), so the update mutates the payload
    /// in place — copyFrom semantics reduce to keeping the other fields — and
    /// node identity never changes (the transformer-era OnUpdate hook is
    /// deferred with the other hooks).
    pub fn update_source_file(
        &mut self,
        node: NodeId,
        statements: Option<NodeList>,
        end_of_file_token: Option<NodeId>,
    ) -> NodeId {
        let changed = {
            let n = self.store.node(node);
            let d = n
                .as_source_file()
                .unwrap_or_else(|| panic!("UpdateSourceFile: node {node} does not carry SourceFile data"));
            statements != d.statements || end_of_file_token != d.end_of_file_token
        };
        if !changed {
            return node;
        }
        let n = self.store.node_mut(node);
        let d = n
            .as_source_file_mut()
            .expect("UpdateSourceFile: SourceFile data");
        d.statements = statements;
        d.end_of_file_token = end_of_file_token;
        node
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Token cache (Go ast.go: TokenCacheKey/GetOrCreateToken/createToken —
// SourceFile-side, so reparse-path token creation dedups against the file)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type TokenCacheKey struct { parent *Node; loc core.TextRange }`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenCacheKey {
    pub parent: NodeId,
    pub loc: TextRange,
}

/// Go: `func createToken(kind Kind, file *SourceFile, pos, end int, flags TokenFlags) *Node`
/// — the kind-dispatched constructor behind GetOrCreateToken (verbatim kind
/// switch: literal/text-bearing kinds get their text from the source slice,
/// everything else is a plain token).
///
/// PORT: Go keeps a dedicated `file.tokenFactory` so cached tokens don't hit
/// the parse factory's counters; the scoped factory here is discarded on
/// return, keeping the parse counters untouched. The token text is copied
/// into the payload (Go slices share the source string; Rust payloads own
/// their text).
fn create_token(file: &mut SourceFile, kind: Kind, text: &str, flags: TokenFlags) -> NodeId {
    let mut f = NodeFactory::new(file);
    match kind {
        Kind::NumericLiteral => f.new_numeric_literal(text, flags),
        Kind::BigIntLiteral => f.new_big_int_literal(text, flags),
        Kind::StringLiteral => f.new_string_literal(text, flags),
        Kind::JsxText | Kind::JsxTextAllWhiteSpaces => {
            f.new_jsx_text(text, kind == Kind::JsxTextAllWhiteSpaces)
        }
        Kind::RegularExpressionLiteral => f.new_regular_expression_literal(text, flags),
        Kind::NoSubstitutionTemplateLiteral => f.new_no_substitution_template_literal(text, flags),
        Kind::TemplateHead => f.new_template_head(text, "", flags),
        Kind::TemplateMiddle => f.new_template_middle(text, "", flags),
        Kind::TemplateTail => f.new_template_tail(text, "", flags),
        Kind::Identifier => f.new_identifier(text),
        Kind::PrivateIdentifier => f.new_private_identifier(text),
        // Punctuation and keywords.
        _ => f.new_token(kind),
    }
}

impl SourceFile {
    /// Go: `func (node *SourceFile) GetOrCreateToken(kind Kind, pos int, end int,
    /// parent *Node, flags TokenFlags) *TokenNode` — gets a token from the
    /// file's token cache, or creates it. Should NOT be used for synthetic
    /// tokens absent from the file (Go comment, verbatim).
    pub fn get_or_create_token(
        &mut self,
        kind: Kind,
        pos: TextPos,
        end: TextPos,
        parent: NodeId,
        flags: TokenFlags,
    ) -> NodeId {
        let loc = TextRange::new(pos, end);
        let key = TokenCacheKey { parent, loc };
        if let Some(&token) = self.token_cache.get(&key) {
            let cached_kind = NodeStore::node(self, token).kind;
            assert_eq!(
                kind, cached_kind,
                "Token cache mismatch: {cached_kind:?} != {kind:?}"
            );
            return token;
        }
        let parent_kind = NodeStore::node(self, parent).kind_string();
        if NodeStore::node(self, parent).flags.contains(NodeFlags::REPARSED) {
            panic!("Cannot create token from reparsed node of kind {parent_kind}");
        }
        // PORT: Go passes the shared source slice; the payload owns a copy.
        let text: String = self.text[pos as usize..end as usize].to_string();
        let token = create_token(self, kind, &text, flags);
        let n = NodeStore::node_mut(self, token);
        n.loc = loc;
        n.parent.set(parent);
        self.token_cache.insert(key, token);
        token
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory_generated::{FACTORY_NEW_INVENTORY, FACTORY_UPDATE_INVENTORY};
    use crate::parseoptions::SourceFileParseOptions;

    /// The Go NodeFactory constructor inventory, hand-extracted from
    /// tsc/internal/ast @ ec47d33c (ast_generated.go's 193 New* + 166 Update*
    /// methods, in file order). Mechanical parity anchor, same pattern as the
    /// kind-ordinal mirror test: the generated inventory must equal Go's
    /// exactly, in order — if ast.json drifts from the Go factory, this fails.
    #[test]
    fn factory_inventory_mirrors_go() {
        assert_eq!(FACTORY_NEW_INVENTORY, GO_FACTORY_NEW_INVENTORY);
        assert_eq!(FACTORY_UPDATE_INVENTORY, GO_FACTORY_UPDATE_INVENTORY);
        // Spot counts (Go: 193 generated New*, 166 generated Update*).
        assert_eq!(FACTORY_NEW_INVENTORY.len(), 193);
        assert_eq!(FACTORY_UPDATE_INVENTORY.len(), 166);
        // Hand-written Go factory methods all have Rust homes:
        // NewNodeList/NewModifierList/NewModifier/NewSourceFile/UpdateSourceFile
        // are exercised by the behavioral tests below (new_source_file_* and
        // new_modifier_list_and_new_modifier); NewCommentRange is
        // source_file::new_comment_range.
    }

    /// Go: New* constructors create nodes (kind/flags/undefined loc/zero
    /// id/nil parent) with children linked by handle, in ast.json member
    /// order; string members bump textCount; every node bumps nodeCount.
    #[test]
    fn new_constructors_build_arena_nodes() {
        let mut file =
            SourceFile::new(0, SourceFileParseOptions::default(), "x + 1", None, None);
        let (x, one, plus, bin, counts) = {
            let mut f = NodeFactory::new(&mut file);
            let x = f.new_identifier("x");
            let one = f.new_numeric_literal("1", TokenFlags::NONE);
            let plus = f.new_token(Kind::PlusToken);
            let bin = f.new_binary_expression(None, x, None, plus, one);
            (x, one, plus, bin, (f.node_count(), f.text_count()))
        };
        // 4 factory nodes; 2 text-bearing (identifier + numeric literal).
        assert_eq!(counts, (4, 2));

        let n = NodeStore::node(&file, bin);
        assert_eq!(n.kind, Kind::BinaryExpression);
        assert_eq!(n.loc, TextRange::undefined());
        assert_eq!(n.parent.get(), NodeId::NONE);
        let d = n.as_binary_expression().expect("BinaryExpression data");
        assert_eq!(d.left, x);
        assert_eq!(d.operator_token, plus);
        assert_eq!(d.right, one);
        assert_eq!(d.type_, None);
        // Uncovered fields take Go's fresh-struct zero values.
        assert_eq!(d.symbol, None);
        assert_eq!(d.facts, 0);

        // for_each_child order (ast.json member order): left, operatorToken,
        // right — the None modifiers/type slots are skipped.
        let mut order: Vec<NodeId> = Vec::new();
        NodeStore::node(&file, bin).for_each_child(&mut |c| {
            order.push(c);
            false
        });
        assert_eq!(order, vec![x, plus, one]);
    }

    /// Go: `isNodeFlagsMember` members set Node.Flags (bitmask-masked where
    /// ast.json says so — PropertyAccessExpression's OptionalChain bit — and
    /// raw where not — VariableDeclarationList); `bitmask` data members
    /// (StringLiteral.TokenFlags) are masked into the data field.
    #[test]
    fn flags_members_set_node_flags_with_bitmasks() {
        let mut file = SourceFile::new(0, SourceFileParseOptions::default(), "", None, None);
        let (list, pae, lit) = {
            let mut f = NodeFactory::new(&mut file);
            let list = f.new_variable_declaration_list(None, NodeFlags::LET | NodeFlags::SYNTHESIZED);
            let obj = f.new_identifier("obj");
            let name = f.new_identifier("prop");
            let pae = f.new_property_access_expression(
                obj,
                None,
                name,
                NodeFlags::OPTIONAL_CHAIN | NodeFlags::SYNTHESIZED,
            );
            let lit = f.new_string_literal(
                "x",
                TokenFlags::UNTERMINATED | TokenFlags::SINGLE_QUOTE,
            );
            (list, pae, lit)
        };
        // No bitmask: Flags passes through.
        assert_eq!(
            NodeStore::node(&file, list).flags,
            NodeFlags::LET | NodeFlags::SYNTHESIZED
        );
        // Bitmask: only OptionalChain survives (Go `node.Flags |= flags &
        // NodeFlagsOptionalChain`).
        assert_eq!(NodeStore::node(&file, pae).flags, NodeFlags::OPTIONAL_CHAIN);
        // Data-side bitmask: TokenFlags masked to StringLiteralFlags.
        let d = NodeStore::node(&file, lit).as_string_literal().expect("data");
        assert_eq!(
            d.token_flags,
            (TokenFlags::UNTERMINATED | TokenFlags::SINGLE_QUOTE) & TokenFlags::STRING_LITERAL_FLAGS
        );
    }

    /// Go: Update* returns the original node when nothing changed (Go: the
    /// same `*Node`), else rebuilds through New* and copies Flags/Loc from the
    /// original (`updateNode`); the Kind comes from the original node, so
    /// alias-kind nodes rebuild under their own kind.
    #[test]
    fn update_identity_and_flags_loc_copy() {
        let mut file = SourceFile::new(0, SourceFileParseOptions::default(), "", None, None);
        let (x, one, plus, bin, js_kind_node) = {
            let mut f = NodeFactory::new(&mut file);
            let x = f.new_identifier("x");
            let one = f.new_identifier("y");
            let plus = f.new_token(Kind::EqualsToken);
            let bin = f.new_binary_expression(None, x, None, plus, one);
            let name = f.new_identifier("T");
            let js_kind_node = f.new_js_type_alias_declaration(None, name, None, name);
            // The parser's finishNode writes Loc/Flags through the store.
            NodeStore::node_mut(f.store(), bin).loc = TextRange::new(0, 5);
            NodeStore::node_mut(f.store(), bin).flags = NodeFlags::HAS_JSDOC;
            (x, one, plus, bin, js_kind_node)
        };

        let rebuilt = {
            let mut f = NodeFactory::new(&mut file);
            // Unchanged → the same handle.
            assert_eq!(f.update_binary_expression(bin, None, x, None, plus, one), bin);
            // Changed → a fresh node with Flags/Loc copied from the original.
            let two = f.new_identifier("z");
            let rebuilt = f.update_binary_expression(bin, None, two, None, plus, one);
            assert_ne!(rebuilt, bin);
            rebuilt
        };
        let n = NodeStore::node(&file, rebuilt);
        assert_eq!(n.loc, TextRange::new(0, 5));
        assert_eq!(n.flags, NodeFlags::HAS_JSDOC);
        assert_eq!(n.as_binary_expression().expect("data").left, NodeStore::node(&file, rebuilt).as_binary_expression().expect("data").left);

        // Alias-kind nodes keep their Kind through Update (Go's kind switch).
        let d = NodeStore::node(&file, js_kind_node);
        assert_eq!(d.kind, Kind::JSTypeAliasDeclaration);
    }

    /// Go: NewSourceFile + `result.NodeCount = p.factory.NodeCount()` — the
    /// Rust flow fills the pre-allocated slot-0 file node and stamps the
    /// counters; UpdateSourceFile is the in-place PORT (identity never
    /// changes).
    #[test]
    fn new_source_file_stamps_payload_and_counters() {
        let mut file = SourceFile::new(
            7,
            SourceFileParseOptions::default(),
            "x;",
            None,
            None,
        );
        let (stmt, eof, sf) = {
            let mut f = NodeFactory::new(&mut file);
            let x = f.new_identifier("x");
            let stmt = f.new_expression_statement(x);
            let stmts = f.new_node_list(vec![stmt].into_boxed_slice());
            let eof = f.new_token(Kind::EndOfFile);
            let sf = f.new_source_file(Some(stmts), Some(eof));
            // identifier + statement + token + the file node (Go's
            // NewSourceFile goes through f.newNode, so it counts).
            assert_eq!(f.node_count(), 4);
            assert_eq!(f.text_count(), 1);
            (stmt, eof, sf)
        };
        assert_eq!(sf, NodeId::new(7, 0));
        let d = NodeStore::node(&file, sf).as_source_file().expect("payload");
        assert_eq!(d.statements.as_ref().expect("statements").nodes.as_ref(), &[stmt]);
        assert_eq!(d.end_of_file_token, Some(eof));
        assert_eq!(d.node_count, 4);
        assert_eq!(d.text_count, 1);

        // UpdateSourceFile: in place (PORT), identity preserved.
        let (updated, stmt2) = {
            let mut f = NodeFactory::new(&mut file);
            let x2 = f.new_identifier("q");
            let stmt2 = f.new_expression_statement(x2);
            let stmts2 = f.new_node_list(vec![stmt2].into_boxed_slice());
            // Unchanged → same node.
            assert_eq!(f.update_source_file(sf, None, None), sf);
            (f.update_source_file(sf, Some(stmts2), Some(eof)), stmt2)
        };
        assert_eq!(updated, sf);
        let d = NodeStore::node(&file, sf).as_source_file().expect("payload");
        assert_eq!(d.statements.as_ref().expect("statements").nodes.as_ref(), &[stmt2]);
        assert_eq!(d.end_of_file_token, Some(eof));
    }

    /// Go: GetOrCreateToken — cache hit returns the same token (with the kind
    /// assert), created tokens copy their text from the source slice and get
    /// Loc/Parent set; the createToken kind dispatch is exercised.
    #[test]
    fn get_or_create_token_caches_and_dispatches() {
        let mut file = SourceFile::new(
            0,
            SourceFileParseOptions::default(),
            "let x = 1;",
            None,
            None,
        );
        let parent = {
            let mut f = NodeFactory::new(&mut file);
            f.new_identifier("scope")
        };
        let t1 = file.get_or_create_token(Kind::Identifier, 4, 5, parent, TokenFlags::NONE);
        let t2 = file.get_or_create_token(Kind::Identifier, 4, 5, parent, TokenFlags::NONE);
        assert_eq!(t1, t2);
        // Text comes from the source slice ("let x = 1;"[4..5] == "x"), loc
        // and parent are set (Go: token.Loc = loc; token.Parent = parent).
        let n = NodeStore::node(&file, t1);
        assert_eq!(n.kind, Kind::Identifier);
        assert_eq!(&*n.as_identifier().expect("data").text, "x");
        assert_eq!(n.loc, TextRange::new(4, 5));
        assert_eq!(n.parent.get(), parent);
        // A different loc is a different token; punctuation goes through
        // NewToken (no text payload).
        let eq = file.get_or_create_token(Kind::EqualsToken, 6, 7, parent, TokenFlags::NONE);
        assert_ne!(eq, t1);
        assert_eq!(NodeStore::node(&file, eq).kind, Kind::EqualsToken);
    }

    /// Go: GetOrCreateToken's kind assert on a poisoned cache entry.
    #[test]
    #[should_panic(expected = "Token cache mismatch")]
    fn get_or_create_token_kind_mismatch_panics() {
        let mut file = SourceFile::new(
            0,
            SourceFileParseOptions::default(),
            "let x = 1;",
            None,
            None,
        );
        let parent = {
            let mut f = NodeFactory::new(&mut file);
            f.new_identifier("scope")
        };
        let _ = file.get_or_create_token(Kind::Identifier, 4, 5, parent, TokenFlags::NONE);
        let _ = file.get_or_create_token(Kind::PrivateIdentifier, 4, 5, parent, TokenFlags::NONE);
    }

    /// Go: NewModifierList computes ModifiersToFlags; NewModifier is NewToken.
    #[test]
    fn new_modifier_list_and_new_modifier() {
        let mut file = SourceFile::new(0, SourceFileParseOptions::default(), "", None, None);
        let (list, modifier) = {
            let mut f = NodeFactory::new(&mut file);
            let export_kw = f.new_token(Kind::ExportKeyword);
            let list = f.new_modifier_list(vec![export_kw].into_boxed_slice());
            let modifier = f.new_modifier(Kind::AsyncKeyword);
            (list, modifier)
        };
        assert_eq!(list.modifier_flags, ModifierFlags::EXPORT);
        assert_eq!(list.loc, TextRange::undefined());
        assert_eq!(NodeStore::node(&file, modifier).kind, Kind::AsyncKeyword);
    }

    // The Go inventory, hand-extracted from tsc/internal/ast/ast_generated.go
    // @ ec47d33c23e464a17cdf2475632cba629bee8763 (file order). Keep in
    // lockstep with the Go file when it regenerates.
    static GO_FACTORY_NEW_INVENTORY: &[&str] = &[
                "NewToken",
        "NewIdentifier",
        "NewPrivateIdentifier",
        "NewQualifiedName",
        "NewComputedPropertyName",
        "NewDecorator",
        "NewEmptyStatement",
        "NewIfStatement",
        "NewDoStatement",
        "NewWhileStatement",
        "NewForStatement",
        "NewForInOrOfStatement",
        "NewBreakStatement",
        "NewContinueStatement",
        "NewReturnStatement",
        "NewWithStatement",
        "NewSwitchStatement",
        "NewCaseBlock",
        "NewCaseOrDefaultClause",
        "NewThrowStatement",
        "NewTryStatement",
        "NewCatchClause",
        "NewDebuggerStatement",
        "NewLabeledStatement",
        "NewExpressionStatement",
        "NewBlock",
        "NewVariableStatement",
        "NewVariableDeclaration",
        "NewVariableDeclarationList",
        "NewBindingPattern",
        "NewParameterDeclaration",
        "NewBindingElement",
        "NewMissingDeclaration",
        "NewFunctionDeclaration",
        "NewClassDeclaration",
        "NewClassExpression",
        "NewHeritageClause",
        "NewInterfaceDeclaration",
        "NewTypeAliasDeclaration",
        "NewJSTypeAliasDeclaration",
        "NewEnumMember",
        "NewEnumDeclaration",
        "NewModuleBlock",
        "NewNotEmittedStatement",
        "NewNotEmittedTypeElement",
        "NewImportDeclaration",
        "NewJSImportDeclaration",
        "NewExternalModuleReference",
        "NewNamespaceImport",
        "NewNamedImports",
        "NewExportAssignment",
        "NewNamespaceExportDeclaration",
        "NewNamespaceExport",
        "NewNamedExports",
        "NewExportSpecifier",
        "NewCallSignatureDeclaration",
        "NewConstructSignatureDeclaration",
        "NewConstructorDeclaration",
        "NewGetAccessorDeclaration",
        "NewSetAccessorDeclaration",
        "NewIndexSignatureDeclaration",
        "NewMethodSignatureDeclaration",
        "NewMethodDeclaration",
        "NewPropertySignatureDeclaration",
        "NewPropertyDeclaration",
        "NewSemicolonClassElement",
        "NewClassStaticBlockDeclaration",
        "NewOmittedExpression",
        "NewKeywordExpression",
        "NewStringLiteral",
        "NewNumericLiteral",
        "NewBigIntLiteral",
        "NewRegularExpressionLiteral",
        "NewNoSubstitutionTemplateLiteral",
        "NewBinaryExpression",
        "NewPrefixUnaryExpression",
        "NewPostfixUnaryExpression",
        "NewYieldExpression",
        "NewArrowFunction",
        "NewFunctionExpression",
        "NewAsExpression",
        "NewSatisfiesExpression",
        "NewConditionalExpression",
        "NewPropertyAccessExpression",
        "NewElementAccessExpression",
        "NewCallExpression",
        "NewNewExpression",
        "NewMetaProperty",
        "NewNonNullExpression",
        "NewSpreadElement",
        "NewTemplateExpression",
        "NewTemplateSpan",
        "NewTaggedTemplateExpression",
        "NewParenthesizedExpression",
        "NewArrayLiteralExpression",
        "NewObjectLiteralExpression",
        "NewSpreadAssignment",
        "NewPropertyAssignment",
        "NewShorthandPropertyAssignment",
        "NewDeleteExpression",
        "NewTypeOfExpression",
        "NewVoidExpression",
        "NewAwaitExpression",
        "NewTypeAssertion",
        "NewKeywordTypeNode",
        "NewUnionTypeNode",
        "NewIntersectionTypeNode",
        "NewConditionalTypeNode",
        "NewTypeOperatorNode",
        "NewInferTypeNode",
        "NewArrayTypeNode",
        "NewIndexedAccessTypeNode",
        "NewTypeReferenceNode",
        "NewExpressionWithTypeArguments",
        "NewLiteralTypeNode",
        "NewThisTypeNode",
        "NewTypePredicateNode",
        "NewImportAttribute",
        "NewImportAttributes",
        "NewTypeQueryNode",
        "NewMappedTypeNode",
        "NewTypeLiteralNode",
        "NewTupleTypeNode",
        "NewNamedTupleMember",
        "NewOptionalTypeNode",
        "NewRestTypeNode",
        "NewParenthesizedTypeNode",
        "NewFunctionTypeNode",
        "NewConstructorTypeNode",
        "NewTemplateHead",
        "NewTemplateMiddle",
        "NewTemplateTail",
        "NewTemplateLiteralTypeNode",
        "NewTemplateLiteralTypeSpan",
        "NewSyntheticExpression",
        "NewPartiallyEmittedExpression",
        "NewJsxElement",
        "NewJsxAttributes",
        "NewJsxNamespacedName",
        "NewJsxOpeningElement",
        "NewJsxSelfClosingElement",
        "NewJsxFragment",
        "NewJsxOpeningFragment",
        "NewJsxClosingFragment",
        "NewJsxAttribute",
        "NewJsxSpreadAttribute",
        "NewJsxClosingElement",
        "NewJsxExpression",
        "NewJsxText",
        "NewSyntaxList",
        "NewJSDoc",
        "NewJSDocTypeExpression",
        "NewJSDocNonNullableType",
        "NewJSDocNullableType",
        "NewJSDocAllType",
        "NewJSDocVariadicType",
        "NewJSDocOptionalType",
        "NewJSDocTypeTag",
        "NewJSDocUnknownTag",
        "NewJSDocTemplateTag",
        "NewJSDocReturnTag",
        "NewJSDocPublicTag",
        "NewJSDocPrivateTag",
        "NewJSDocProtectedTag",
        "NewJSDocReadonlyTag",
        "NewJSDocOverrideTag",
        "NewJSDocDeprecatedTag",
        "NewJSDocSeeTag",
        "NewJSDocImplementsTag",
        "NewJSDocAugmentsTag",
        "NewJSDocSatisfiesTag",
        "NewJSDocThrowsTag",
        "NewJSDocThisTag",
        "NewJSDocImportTag",
        "NewJSDocCallbackTag",
        "NewJSDocOverloadTag",
        "NewJSDocTypedefTag",
        "NewJSDocSignature",
        "NewJSDocNameReference",
        "NewModuleDeclaration",
        "NewImportEqualsDeclaration",
        "NewExportDeclaration",
        "NewImportTypeNode",
        "NewImportClause",
        "NewImportSpecifier",
        "NewJSDocText",
        "NewJSDocLink",
        "NewJSDocLinkPlain",
        "NewJSDocLinkCode",
        "NewTypeParameterDeclaration",
        "NewSyntheticReferenceExpression",
        "NewJSDocTypeLiteral",
        "NewJSDocParameterOrPropertyTag",
    ];
    static GO_FACTORY_UPDATE_INVENTORY: &[&str] = &[
                "UpdateQualifiedName",
        "UpdateComputedPropertyName",
        "UpdateDecorator",
        "UpdateIfStatement",
        "UpdateDoStatement",
        "UpdateWhileStatement",
        "UpdateForStatement",
        "UpdateForInOrOfStatement",
        "UpdateBreakStatement",
        "UpdateContinueStatement",
        "UpdateReturnStatement",
        "UpdateWithStatement",
        "UpdateSwitchStatement",
        "UpdateCaseBlock",
        "UpdateCaseOrDefaultClause",
        "UpdateThrowStatement",
        "UpdateTryStatement",
        "UpdateCatchClause",
        "UpdateLabeledStatement",
        "UpdateExpressionStatement",
        "UpdateBlock",
        "UpdateVariableStatement",
        "UpdateVariableDeclaration",
        "UpdateVariableDeclarationList",
        "UpdateBindingPattern",
        "UpdateParameterDeclaration",
        "UpdateBindingElement",
        "UpdateMissingDeclaration",
        "UpdateFunctionDeclaration",
        "UpdateClassDeclaration",
        "UpdateClassExpression",
        "UpdateHeritageClause",
        "UpdateInterfaceDeclaration",
        "UpdateTypeAliasDeclaration",
        "UpdateEnumMember",
        "UpdateEnumDeclaration",
        "UpdateModuleBlock",
        "UpdateImportDeclaration",
        "UpdateExternalModuleReference",
        "UpdateNamespaceImport",
        "UpdateNamedImports",
        "UpdateExportAssignment",
        "UpdateNamespaceExportDeclaration",
        "UpdateNamespaceExport",
        "UpdateNamedExports",
        "UpdateExportSpecifier",
        "UpdateCallSignatureDeclaration",
        "UpdateConstructSignatureDeclaration",
        "UpdateConstructorDeclaration",
        "UpdateGetAccessorDeclaration",
        "UpdateSetAccessorDeclaration",
        "UpdateIndexSignatureDeclaration",
        "UpdateMethodSignatureDeclaration",
        "UpdateMethodDeclaration",
        "UpdatePropertySignatureDeclaration",
        "UpdatePropertyDeclaration",
        "UpdateClassStaticBlockDeclaration",
        "UpdateBinaryExpression",
        "UpdatePrefixUnaryExpression",
        "UpdatePostfixUnaryExpression",
        "UpdateYieldExpression",
        "UpdateArrowFunction",
        "UpdateFunctionExpression",
        "UpdateAsExpression",
        "UpdateSatisfiesExpression",
        "UpdateConditionalExpression",
        "UpdatePropertyAccessExpression",
        "UpdateElementAccessExpression",
        "UpdateCallExpression",
        "UpdateNewExpression",
        "UpdateMetaProperty",
        "UpdateNonNullExpression",
        "UpdateSpreadElement",
        "UpdateTemplateExpression",
        "UpdateTemplateSpan",
        "UpdateTaggedTemplateExpression",
        "UpdateParenthesizedExpression",
        "UpdateArrayLiteralExpression",
        "UpdateObjectLiteralExpression",
        "UpdateSpreadAssignment",
        "UpdatePropertyAssignment",
        "UpdateShorthandPropertyAssignment",
        "UpdateDeleteExpression",
        "UpdateTypeOfExpression",
        "UpdateVoidExpression",
        "UpdateAwaitExpression",
        "UpdateTypeAssertion",
        "UpdateUnionTypeNode",
        "UpdateIntersectionTypeNode",
        "UpdateConditionalTypeNode",
        "UpdateTypeOperatorNode",
        "UpdateInferTypeNode",
        "UpdateArrayTypeNode",
        "UpdateIndexedAccessTypeNode",
        "UpdateTypeReferenceNode",
        "UpdateExpressionWithTypeArguments",
        "UpdateLiteralTypeNode",
        "UpdateTypePredicateNode",
        "UpdateImportAttribute",
        "UpdateImportAttributes",
        "UpdateTypeQueryNode",
        "UpdateMappedTypeNode",
        "UpdateTypeLiteralNode",
        "UpdateTupleTypeNode",
        "UpdateNamedTupleMember",
        "UpdateOptionalTypeNode",
        "UpdateRestTypeNode",
        "UpdateParenthesizedTypeNode",
        "UpdateFunctionTypeNode",
        "UpdateConstructorTypeNode",
        "UpdateTemplateLiteralTypeNode",
        "UpdateTemplateLiteralTypeSpan",
        "UpdateSyntheticExpression",
        "UpdatePartiallyEmittedExpression",
        "UpdateJsxElement",
        "UpdateJsxAttributes",
        "UpdateJsxNamespacedName",
        "UpdateJsxOpeningElement",
        "UpdateJsxSelfClosingElement",
        "UpdateJsxFragment",
        "UpdateJsxAttribute",
        "UpdateJsxSpreadAttribute",
        "UpdateJsxClosingElement",
        "UpdateJsxExpression",
        "UpdateSyntaxList",
        "UpdateJSDoc",
        "UpdateJSDocTypeExpression",
        "UpdateJSDocNonNullableType",
        "UpdateJSDocNullableType",
        "UpdateJSDocVariadicType",
        "UpdateJSDocOptionalType",
        "UpdateJSDocTypeTag",
        "UpdateJSDocUnknownTag",
        "UpdateJSDocTemplateTag",
        "UpdateJSDocReturnTag",
        "UpdateJSDocPublicTag",
        "UpdateJSDocPrivateTag",
        "UpdateJSDocProtectedTag",
        "UpdateJSDocReadonlyTag",
        "UpdateJSDocOverrideTag",
        "UpdateJSDocDeprecatedTag",
        "UpdateJSDocSeeTag",
        "UpdateJSDocImplementsTag",
        "UpdateJSDocAugmentsTag",
        "UpdateJSDocSatisfiesTag",
        "UpdateJSDocThrowsTag",
        "UpdateJSDocThisTag",
        "UpdateJSDocImportTag",
        "UpdateJSDocCallbackTag",
        "UpdateJSDocOverloadTag",
        "UpdateJSDocTypedefTag",
        "UpdateJSDocSignature",
        "UpdateJSDocNameReference",
        "UpdateModuleDeclaration",
        "UpdateImportEqualsDeclaration",
        "UpdateExportDeclaration",
        "UpdateImportTypeNode",
        "UpdateImportClause",
        "UpdateImportSpecifier",
        "UpdateJSDocLink",
        "UpdateJSDocLinkPlain",
        "UpdateJSDocLinkCode",
        "UpdateTypeParameterDeclaration",
        "UpdateSyntheticReferenceExpression",
        "UpdateJSDocTypeLiteral",
        "UpdateJSDocParameterOrPropertyTag",
    ];
}
