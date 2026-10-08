// Ported from tsc/internal/ast/ast.go (SourceFile + the file-scoped support
// types) @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   SourceFile struct         → split: SourceFileNodeData (the NodeData payload —
//                               everything reachable through `node.AsSourceFile()`,
//                               i.e. the parser/binder-visible fields) + SourceFile
//                               (the arena owner: text, parse options, lazy
//                               caches, the cross-package data map). Go has a
//                               single struct; the payload cannot contain the
//                               arena that stores the payload (SPEC §5.1
//                               arena-per-file), hence the split.
//   ECMALineMap               → SourceFile::ecma_line_map (core.go is not yet
//                               ported, so ComputeECMALineStarts lives here)
//   GetPositionMap            → SourceFile::get_position_map
//   HasIdentifier             → SourceFile::has_identifier
//   GetOrComputeData          → SourceFile::get_or_compute_data
//   NewSourceFile             → SourceFile::new (the NodeFactory is deferred)
//   CommentRange/CheckJsDirective/FileReference/Pragma/PragmaKindFlags/
//   PragmaArgumentSpecification/PragmaSpecification/PatternAmbientModule/
//   SourceFileMetaData/CommentDirective → same names in this module
//
// DEFERRED (separate M2 tasks, noted in PORTING-NOTES):
//   - content-mapper surface (ContentMapperSourceFileInfo, SpanMap,
//     OriginalText, MappedDiagnosticDirective) — needs the spanmap package.
//   - GetOrCreateToken/createToken + tokenCache — needs NodeFactory.
//   - GetNameTable / GetDeclarationMap — deep utilities.go dependency chains.
//   - resolveJSDoc — parser-owned registration hook (parseJSDocForNode).
//   - ReparsedClones, Hash, language-service caches.

use std::any::Any;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use tsc_core::languagevariant::LanguageVariant;
use tsc_core::pattern::Pattern;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::text::{TextPos, TextRange};
use tsc_core::tristate::Tristate;
use tsc_tspath::{PathKey, RootedDirectoryPath, RootedFilePath};

use crate::diagnostic::Diagnostic;
use crate::parseoptions::SourceFileParseOptions;
use crate::positionmap::{compute_position_map, PositionMap};
use crate::{Kind, Node, NodeData, NodeId, NodeList, SymbolId, SymbolTable};
use crate::visitor::{visit, visit_node_list, NodeStore, NodeVisitor};

// ────────────────────────────────────────────────────────────────────────────
// File-scoped support types (Go ast.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type CommentRange struct { core.TextRange; Kind; HasTrailingNewLine }`
/// (flattened embed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentRange {
    pub loc: TextRange,
    pub kind: Kind,
    pub has_trailing_new_line: bool,
}

impl CommentRange {
    /// Go: the embedded `core.TextRange` accessors.
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }
    pub fn end(&self) -> TextPos {
        self.loc.end()
    }
}

/// Go: `func (f *NodeFactory) NewCommentRange(kind Kind, pos int, end int, hasTrailingNewLine bool) CommentRange`.
pub fn new_comment_range(kind: Kind, pos: TextPos, end: TextPos, has_trailing_new_line: bool) -> CommentRange {
    CommentRange {
        loc: TextRange::new(pos, end),
        kind,
        has_trailing_new_line,
    }
}

/// Go: `type CheckJsDirective struct { Enabled bool; Range CommentRange }`.
#[derive(Clone, Debug)]
pub struct CheckJsDirective {
    pub enabled: bool,
    pub range: CommentRange,
}

/// Go: `type CommentDirectiveKind int32` + iota consts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(i32)]
pub enum CommentDirectiveKind {
    /// Go: `CommentDirectiveKindUnknown CommentDirectiveKind = iota`.
    #[default]
    Unknown = 0,
    /// Go: `CommentDirectiveKindExpectError`.
    ExpectError = 1,
    /// Go: `CommentDirectiveKindIgnore`.
    Ignore = 2,
}

/// Go: `type CommentDirective struct { Loc core.TextRange; Kind CommentDirectiveKind }`.
#[derive(Clone, Debug, Default)]
pub struct CommentDirective {
    pub loc: TextRange,
    pub kind: CommentDirectiveKind,
}

/// Go: `type FileReference struct { core.TextRange; FileName; ResolutionMode; Preserve }`
/// (flattened embed).
#[derive(Clone, Debug, Default)]
pub struct FileReference {
    pub loc: TextRange,
    pub file_name: String,
    pub mode: tsc_core::compileroptions::ResolutionMode,
    pub preserve: bool,
}

impl FileReference {
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }
    pub fn end(&self) -> TextPos {
        self.loc.end()
    }
}

/// Go: `type PragmaArgument struct { core.TextRange; Name; Value }` (flattened).
#[derive(Clone, Debug, Default)]
pub struct PragmaArgument {
    pub loc: TextRange,
    pub name: String,
    pub value: String,
}

impl PragmaArgument {
    pub fn pos(&self) -> TextPos {
        self.loc.pos()
    }
    pub fn end(&self) -> TextPos {
        self.loc.end()
    }
}

/// Go: `type Pragma struct { CommentRange; Name; Args map[string]PragmaArgument }`.
///
/// PORT: `Args` uses std `HashMap` — Go map iteration order is randomized
/// there too, and args are only ever looked up by name (never iterated into
/// output), so ordering is not observable.
#[derive(Clone, Debug)]
pub struct Pragma {
    // CommentRange (flattened embed)
    pub loc: TextRange,
    pub range_kind: Kind,
    pub has_trailing_new_line: bool,
    pub name: String,
    pub args: HashMap<String, PragmaArgument>,
}

/// Go: `type PragmaKindFlags = uint8` + consts.
pub type PragmaKindFlags = u8;
pub const PRAGMA_KIND_TRIPLE_SLASH_XML: PragmaKindFlags = 1 << 0;
pub const PRAGMA_KIND_SINGLE_LINE: PragmaKindFlags = 1 << 1;
pub const PRAGMA_KIND_MULTI_LINE: PragmaKindFlags = 1 << 2;
pub const PRAGMA_KIND_FLAGS_NONE: PragmaKindFlags = 0;
pub const PRAGMA_KIND_ALL: PragmaKindFlags =
    PRAGMA_KIND_TRIPLE_SLASH_XML | PRAGMA_KIND_SINGLE_LINE | PRAGMA_KIND_MULTI_LINE;
pub const PRAGMA_KIND_DEFAULT: PragmaKindFlags = PRAGMA_KIND_ALL;

/// Go: `type PragmaArgumentSpecification struct`.
#[derive(Clone, Debug, Default)]
pub struct PragmaArgumentSpecification {
    pub name: String,
    pub optional: bool,
    pub capture_span: bool,
}

/// Go: `type PragmaSpecification struct { Args; Kind }`.
#[derive(Clone, Debug, Default)]
pub struct PragmaSpecification {
    pub args: Vec<PragmaArgumentSpecification>,
    pub kind: PragmaKindFlags,
}

impl PragmaSpecification {
    /// Go: `func (spec *PragmaSpecification) IsTripleSlash() bool`.
    pub fn is_triple_slash(&self) -> bool {
        (self.kind & PRAGMA_KIND_TRIPLE_SLASH_XML) > 0
    }
}

/// Go: `type PatternAmbientModule struct { Pattern; Symbol *Symbol }` — the
/// symbol becomes a `SymbolId` handle (SPEC §5.2).
#[derive(Clone, Debug, Default)]
pub struct PatternAmbientModule {
    pub pattern: Pattern,
    pub symbol: Option<SymbolId>,
}

/// Go: `type SourceFileMetaData struct`.
#[derive(Clone, Debug, Default)]
pub struct SourceFileMetaData {
    pub package_json_type: String,
    pub package_json_directory: RootedDirectoryPath,
    pub implied_node_format: tsc_core::compileroptions::ResolutionMode,
}

// ────────────────────────────────────────────────────────────────────────────
// SourceFileNodeData — the NodeData payload (everything reachable through
// `node.AsSourceFile()`)
// ────────────────────────────────────────────────────────────────────────────

/// The `Kind::SourceFile` node payload — Go's `SourceFile` struct minus the
/// fields the arena owner must keep (text, parse options, lazy caches).
/// Fields are grouped by the Go struct's section comments (NewSourceFile /
/// parser / binder).
#[derive(Clone, Debug, Default)]
pub struct SourceFileNodeData {
    // DeclarationBase
    pub symbol: Option<SymbolId>,
    pub local_symbol: Option<SymbolId>,
    // LocalsContainerBase
    pub locals: SymbolTable,
    pub next_container: Option<NodeId>,
    // CompositeBase
    pub facts: u32,

    // Fields set by NewSourceFile
    pub statements: Option<NodeList>,
    pub end_of_file_token: Option<NodeId>,

    // Fields set by parser
    pub script_kind: ScriptKind,
    pub language_variant: LanguageVariant,
    pub is_declaration_file: bool,
    pub uses_uri_style_node_core_modules: Tristate,
    pub identifier_count: i32,
    pub imports: Vec<NodeId>,
    pub module_augmentations: Vec<NodeId>,
    pub ambient_module_names: Vec<String>,
    pub comment_directives: Vec<CommentDirective>,
    pub has_lazy_jsdoc: bool,
    pub jsdoc_cache: Option<HashMap<NodeId, Vec<NodeId>>>,
    pub pragmas: Vec<Pragma>,
    pub referenced_files: Vec<FileReference>,
    pub type_reference_directives: Vec<FileReference>,
    pub lib_reference_directives: Vec<FileReference>,
    pub check_js_directive: Option<CheckJsDirective>,
    pub node_count: i32,
    pub text_count: i32,
    pub common_js_module_indicator: Cell<Option<NodeId>>,
    // If this is the SourceFile itself, then this module was "forced"
    // to be an external module (previously "true").
    pub external_module_indicator: Cell<Option<NodeId>>,

    // Diagnostics (Go: parser/binder-set on the SourceFile struct)
    pub diagnostics: Vec<Diagnostic>,
    pub js_diagnostics: Vec<Diagnostic>,
    pub jsdoc_diagnostics: Vec<Diagnostic>,
    pub bind_diagnostics: Vec<Diagnostic>,

    // Fields set by binder
    pub is_bound: Cell<bool>,
    pub symbol_count: i32,
    pub pattern_ambient_modules: Vec<PatternAmbientModule>,
    pub global_exports: SymbolTable,
}

impl SourceFileNodeData {
    /// Go: `func (node *SourceFile) ForEachChild(v Visitor) bool`.
    pub fn for_each_child(&self, v: &mut dyn FnMut(NodeId) -> bool) -> bool {
        if visit_node_list(v, self.statements.as_ref()) {
            return true;
        }
        visit(v, self.end_of_file_token)
    }

    /// Go: `func (node *SourceFile) VisitEachChild(v *NodeVisitor) *Node` —
    /// `v.Factory.UpdateSourceFile(node, v.visitTopLevelStatements(node.Statements),
    /// v.visitToken(node.EndOfFileToken))`. `None` = no child changed.
    ///
    /// The fresh payload mirrors Go's `NewSourceFile` + `copyFrom`: the
    /// copyFrom-copied fields carry over, everything else resets to the
    /// NewSourceFile zero value (copyFrom deliberately skips binder results,
    /// counts, and diagnostics).
    pub fn visit_each_child(&self, v: &mut dyn NodeVisitor) -> Option<Self> {
        let statements = v.visit_top_level_statements(self.statements.as_ref());
        let end_of_file_token = v.visit_token(self.end_of_file_token);
        if statements == self.statements && end_of_file_token == self.end_of_file_token {
            return None;
        }
        Some(Self {
            statements,
            end_of_file_token,
            // copyFrom(node)
            language_variant: self.language_variant,
            script_kind: self.script_kind,
            is_declaration_file: self.is_declaration_file,
            uses_uri_style_node_core_modules: self.uses_uri_style_node_core_modules,
            imports: self.imports.clone(),
            module_augmentations: self.module_augmentations.clone(),
            ambient_module_names: self.ambient_module_names.clone(),
            comment_directives: self.comment_directives.clone(),
            pragmas: self.pragmas.clone(),
            referenced_files: self.referenced_files.clone(),
            type_reference_directives: self.type_reference_directives.clone(),
            lib_reference_directives: self.lib_reference_directives.clone(),
            common_js_module_indicator: self.common_js_module_indicator.clone(),
            external_module_indicator: self.external_module_indicator.clone(),
            // Not copied by copyFrom — NewSourceFile zero values.
            ..Default::default()
        })
    }
}

// ────────────────────────────────────────────────────────────────────────────
// SourceFile — the arena owner
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type SourceFileDataKey[T any] struct` — the zero-sized marker carries
/// the type; the u64 key comes from the same global counter.
pub struct SourceFileDataKey<T> {
    key: u64,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> Clone for SourceFileDataKey<T> {
    fn clone(&self) -> Self {
        SourceFileDataKey {
            key: self.key,
            _marker: std::marker::PhantomData,
        }
    }
}

static SOURCE_FILE_DATA_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Go: `func NewSourceFileDataKey[T any]() *SourceFileDataKey[T]`.
pub fn new_source_file_data_key<T>() -> SourceFileDataKey<T> {
    SourceFileDataKey {
        key: SOURCE_FILE_DATA_KEY_COUNTER.fetch_add(1, Ordering::Relaxed),
        _marker: std::marker::PhantomData,
    }
}

/// SPEC §5.1: Go's `*SourceFile` — the arena owner. `nodes[0]` is always the
/// `Kind::SourceFile` node itself (Go's `file.AsNode()` → `NodeId
/// ::new(file_id, 0)`).
pub struct SourceFile {
    file_id: u32,
    /// The node arena (SPEC §5.1). Public for the parser factory.
    pub nodes: Vec<Node>,
    parse_options: SourceFileParseOptions,
    text: String,
    // Lazy caches (Go sync.Once fields)
    ecma_line_map: OnceLock<Vec<TextPos>>,
    position_map: OnceLock<PositionMap>,
    identifiers: OnceLock<HashSet<String>>,
    // Go: `data map[sourceFileDataKey]any` — cross-package lazy cells.
    data: HashMap<u64, OnceLock<Box<dyn Any + Send + Sync>>>,
}

impl SourceFile {
    /// Go: `func (f *NodeFactory) NewSourceFile(opts SourceFileParseOptions, text string,
    /// statements *NodeList, endOfFileToken *TokenNode) *Node` — the port
    /// constructs the owning arena directly (the factory is a separate task);
    /// `file_id` assigns the NodeId file component (Go bakes the pointer
    /// instead, SPEC §5.1).
    pub fn new(
        file_id: u32,
        opts: SourceFileParseOptions,
        text: impl Into<String>,
        statements: Option<NodeList>,
        end_of_file_token: Option<NodeId>,
    ) -> SourceFile {
        let data = SourceFileNodeData {
            statements,
            end_of_file_token,
            ..Default::default()
        };
        let file_node = Node {
            kind: Kind::SourceFile,
            flags: crate::NodeFlags::NONE,
            loc: TextRange::undefined(),
            id: Cell::new(0),
            parent: Cell::new(NodeId::NONE),
            data: NodeData::SourceFile(Box::new(data)),
        };
        SourceFile {
            file_id,
            nodes: vec![file_node],
            parse_options: opts,
            text: text.into(),
            ecma_line_map: OnceLock::new(),
            position_map: OnceLock::new(),
            identifiers: OnceLock::new(),
            data: HashMap::new(),
        }
    }

    /// Go: `file.AsNode()` — the file's own node handle.
    pub fn as_node_id(&self) -> NodeId {
        NodeId::new(self.file_id, 0)
    }

    pub fn file_id(&self) -> u32 {
        self.file_id
    }

    /// The file's payload (Go `node.AsSourceFile()`).
    pub fn payload(&self) -> &SourceFileNodeData {
        self.nodes[0].as_source_file().expect("nodes[0] is the SourceFile node")
    }

    /// The file's payload, mutably.
    pub fn payload_mut(&mut self) -> &mut SourceFileNodeData {
        self.nodes[0].as_source_file_mut().expect("nodes[0] is the SourceFile node")
    }

    /// Go: `func (node *SourceFile) Text() string`.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Go: `func (node *SourceFile) ParseOptions() SourceFileParseOptions`.
    pub fn parse_options(&self) -> &SourceFileParseOptions {
        &self.parse_options
    }

    /// Go: `func (node *SourceFile) FileName() tspath.RootedFilePath`.
    pub fn file_name(&self) -> &RootedFilePath {
        &self.parse_options.file_name
    }

    /// Go: `func (node *SourceFile) PathKey() tspath.PathKey`.
    pub fn path_key(&self) -> &PathKey {
        &self.parse_options.path_key
    }

    /// Go: `func (node *SourceFile) IsJS() bool`.
    pub fn is_js(&self) -> bool {
        crate::utilities::is_source_file_js(self.payload().script_kind)
    }

    /// Go: `func (node *SourceFile) Diagnostics() []*Diagnostic`.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.payload().diagnostics
    }

    /// Go: `func (node *SourceFile) SetDiagnostics(diags []*Diagnostic)`.
    pub fn set_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        self.payload_mut().diagnostics = diags;
    }

    /// Go: `func (node *SourceFile) JSDiagnostics() []*Diagnostic`.
    pub fn js_diagnostics(&self) -> &[Diagnostic] {
        &self.payload().js_diagnostics
    }

    /// Go: `func (node *SourceFile) SetJSDiagnostics(diags []*Diagnostic)`.
    pub fn set_js_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        self.payload_mut().js_diagnostics = diags;
    }

    /// Go: `func (node *SourceFile) JSDocDiagnostics() []*Diagnostic`.
    pub fn jsdoc_diagnostics(&self) -> &[Diagnostic] {
        &self.payload().jsdoc_diagnostics
    }

    /// Go: `func (node *SourceFile) SetJSDocDiagnostics(diags []*Diagnostic)`.
    pub fn set_jsdoc_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        self.payload_mut().jsdoc_diagnostics = diags;
    }

    /// Go: `func (node *SourceFile) BindDiagnostics() []*Diagnostic`.
    pub fn bind_diagnostics(&self) -> &[Diagnostic] {
        &self.payload().bind_diagnostics
    }

    /// Go: `func (node *SourceFile) SetBindDiagnostics(diags []*Diagnostic)`.
    pub fn set_bind_diagnostics(&mut self, diags: Vec<Diagnostic>) {
        self.payload_mut().bind_diagnostics = diags;
    }

    /// Go: `func (node *SourceFile) SetJSDocCache(cache map[*Node][]*Node)`.
    pub fn set_jsdoc_cache(&mut self, cache: HashMap<NodeId, Vec<NodeId>>) {
        self.payload_mut().jsdoc_cache = Some(cache);
    }

    /// Go: `func (node *SourceFile) SetHasLazyJSDoc(lazy bool)`.
    pub fn set_has_lazy_jsdoc(&mut self, lazy: bool) {
        self.payload_mut().has_lazy_jsdoc = lazy;
    }

    /// Go: `func (node *SourceFile) IsBound() bool`.
    pub fn is_bound(&self) -> bool {
        self.payload().is_bound.get()
    }

    /// Go: `func (node *SourceFile) BindOnce(bind func())`.
    pub fn bind_once(&mut self, bind: impl FnOnce(&mut SourceFile)) {
        if !self.is_bound() {
            bind(self);
            self.payload_mut().is_bound.set(true);
        }
    }

    /// Go: `func (node *SourceFile) ECMALineMap() []core.TextPos` —
    /// core.go is not yet ported, so ComputeECMALineStarts lives here
    /// (TODO(porting) dedup into the core.go port).
    pub fn ecma_line_map(&self) -> &[TextPos] {
        self.ecma_line_map.get_or_init(|| compute_ecma_line_starts(&self.text))
    }

    /// Go: `func (file *SourceFile) GetPositionMap() *PositionMap`.
    pub fn get_position_map(&self) -> &PositionMap {
        self.position_map.get_or_init(|| compute_position_map(self.text.as_bytes()))
    }

    /// Go: `func (file *SourceFile) HasIdentifier(name string) bool` +
    /// `collectIdentifiersForSourceFile`.
    pub fn has_identifier(&self, name: &str) -> bool {
        let identifiers = self.identifiers.get_or_init(|| self.collect_identifiers());
        identifiers.contains(name)
    }

    fn collect_identifiers(&self) -> HashSet<String> {
        let mut identifiers = HashSet::new();
        let store: &dyn NodeStore = self;
        let mut visit = |node: NodeId| -> bool {
            let n = store.node(node);
            match &n.data {
                NodeData::Identifier(_)
                | NodeData::PrivateIdentifier(_)
                | NodeData::StringLiteral(_)
                | NodeData::NumericLiteral(_)
                | NodeData::BigIntLiteral(_)
                | NodeData::NoSubstitutionTemplateLiteral(_) => {
                    identifiers.insert(crate::node_text(store, node).to_string());
                }
                _ => {}
            }
            false
        };
        self.nodes[0].for_each_child(&mut visit);
        identifiers
    }

    /// Go: `func (file *SourceFile) GetOrComputeData[T any](key *SourceFileDataKey[T],
    /// compute func(*SourceFile) T) T`.
    pub fn get_or_compute_data<T: Clone + Send + Sync + 'static>(
        &mut self,
        key: &SourceFileDataKey<T>,
        compute: impl FnOnce(&SourceFile) -> T,
    ) -> T {
        // Fast path: the cell exists and is populated.
        if let Some(value) = self
            .data
            .get(&key.key)
            .and_then(|cell| cell.get())
            .and_then(|boxed| boxed.downcast_ref::<T>())
        {
            return value.clone();
        }
        // Slow path: compute, then store (a racing double-compute overwrites
        // harmlessly; the OnceLock keeps the winner's value for later reads).
        let value = compute(self);
        let cell = self.data.entry(key.key).or_default();
        let _ = cell.set(Box::new(value));
        // The panic mirrors Go's `cell.(*sourceFileDataCell[T])` type
        // assertion (impossible against a well-typed key).
        cell.get()
            .expect("OnceLock was just set")
            .downcast_ref::<T>()
            .expect("SourceFileDataKey type mismatch")
            .clone()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// NodeStore (SPEC §5.1 — the arena seam)
// ────────────────────────────────────────────────────────────────────────────

impl NodeStore for SourceFile {
    fn node(&self, id: NodeId) -> &Node {
        assert_eq!(
            id.file_id(),
            self.file_id,
            "NodeId {} is foreign to file {}",
            id,
            self.file_id
        );
        &self.nodes[id.local_index() as usize]
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        assert_eq!(id.file_id(), self.file_id, "NodeId {} is foreign to file {}", id, self.file_id);
        &mut self.nodes[id.local_index() as usize]
    }

    fn file(&self, file_id: u32) -> &SourceFile {
        assert_eq!(file_id, self.file_id, "file {} is foreign to this arena", file_id);
        self
    }

    fn alloc(&mut self, node: Node) -> NodeId {
        let index = self.nodes.len() as u64;
        self.nodes.push(node);
        NodeId::new(self.file_id, index)
    }
}

impl Node {
    /// Go: `func (n *Node) AsSourceFile() *SourceFile` — the mutable form
    /// (binder-era `Cell` field writes).
    pub fn as_source_file_mut(&mut self) -> Option<&mut SourceFileNodeData> {
        match &mut self.data {
            NodeData::SourceFile(d) => Some(&mut **d),
            _ => None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ComputeECMALineStarts (Go core.go — core.go is not yet ported; this is the
// tsc-ast-local copy, TODO(porting) dedup into the core.go port)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ComputeECMALineStarts(text string) ECMALineStarts` — the start
/// position of every line, where a line break is `\r\n`, `\r`, `\n`, or any
/// Unicode line terminator (unlike Go's own scanner semantics, this matches
/// ECMAScript).
pub fn compute_ecma_line_starts(text: &str) -> Vec<TextPos> {
    // Go: make([]TextPos, 0, strings.Count(text, "\n")+1)
    let mut result = Vec::with_capacity(text.bytes().filter(|&b| b == b'\n').count() + 1);
    let bytes = text.as_bytes();
    let text_len = bytes.len() as TextPos;
    let mut pos: TextPos = 0;
    let mut line_start: TextPos = 0;
    while pos < text_len {
        let b = bytes[pos as usize];
        if b < 0x80 {
            pos += 1;
            if b == b'\r' {
                if pos < text_len && bytes[pos as usize] == b'\n' {
                    pos += 1;
                }
                result.push(line_start);
                line_start = pos;
            } else if b == b'\n' {
                result.push(line_start);
                line_start = pos;
            }
        } else {
            let (ch, size) = tsc_stringutil::decode_js_string_rune(&bytes[pos as usize..]);
            pos += size as TextPos;
            // Lone surrogates decode to chars outside `char`'s range; they are
            // never line breaks, so the `unwrap_or` fallback is behaviorally
            // inert (Go's IsLineBreak takes a rune).
            if tsc_stringutil::is_line_break(char::from_u32(ch as u32).unwrap_or('\0')) {
                result.push(line_start);
                line_start = pos;
            }
        }
    }
    result.push(line_start);
    result
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: `func (f *NodeFactory) NewSourceFile(...)` smoke — the file node is
    /// arena slot 0 and round-trips through the NodeStore seam.
    #[test]
    fn new_source_file_builds_arena() {
        let opts = SourceFileParseOptions::default();
        let file = SourceFile::new(7, opts, "const x = 1;\n", None, None);
        assert_eq!(file.as_node_id(), NodeId::new(7, 0));
        assert_eq!(file.file_id(), 7);
        assert_eq!(file.text(), "const x = 1;\n");
        let node = NodeStore::node(&file, file.as_node_id());
        assert_eq!(node.kind, Kind::SourceFile);
        let payload = node.as_source_file().expect("payload");
        assert!(payload.statements.is_none());
        assert!(payload.end_of_file_token.is_none());
    }

    /// Go: ComputeECMALineStarts semantics — \n, \r\n, \r, and Unicode
    /// (U+2028) breaks; a final entry for the trailing (possibly empty) line.
    #[test]
    fn ecma_line_starts_lf_crlf_cr() {
        assert_eq!(compute_ecma_line_starts("a\nb\nc"), vec![0, 2, 4]);
        assert_eq!(compute_ecma_line_starts("a\r\nb\rc"), vec![0, 3, 5]);
        assert_eq!(compute_ecma_line_starts(""), vec![0]);
        assert_eq!(compute_ecma_line_starts("\n"), vec![0, 1]);
        // U+2028 LINE SEPARATOR (UTF-8 3 bytes)
        assert_eq!(compute_ecma_line_starts("a\u{2028}b"), vec![0, 4]);
    }

    /// Go: `func (node *SourceFile) ECMALineMap()` — lazily computed, cached.
    #[test]
    fn ecma_line_map_is_lazy_and_cached() {
        let file = SourceFile::new(0, SourceFileParseOptions::default(), "a\nb", None, None);
        let map = file.ecma_line_map();
        assert_eq!(map, &[0, 2]);
        // Same allocation on the second call (cached, not recomputed).
        let again = file.ecma_line_map();
        assert!(std::ptr::eq(map, again));
    }

    /// Go: `func (file *SourceFile) GetPositionMap()` — lazily computed.
    #[test]
    fn position_map_accessor() {
        let file = SourceFile::new(0, SourceFileParseOptions::default(), "café", None, None);
        assert!(!file.get_position_map().is_ascii_only());
        let file = SourceFile::new(0, SourceFileParseOptions::default(), "abc", None, None);
        assert!(file.get_position_map().is_ascii_only());
    }

    /// Go: `func (file *SourceFile) HasIdentifier(name string)` +
    /// `collectIdentifiersForSourceFile` — walks the file node's children.
    #[test]
    fn has_identifier_collects_from_children() {
        let mut file = SourceFile::new(3, SourceFileParseOptions::default(), "", None, None);
        // Build a tiny tree: file -> identifier "x" and a string literal "y".
        let identifier = {
            let d = crate::ast_generated::Identifier {
                flow_node: None,
                text: "x".into(),
            };
            file.alloc(Node {
                kind: Kind::Identifier,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::new(0, 1),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: NodeData::Identifier(Box::new(d)),
            })
        };
        let literal = {
            let d = crate::ast_generated::StringLiteral {
                text: "y".into(),
                token_flags: crate::TokenFlags::NONE,
            };
            file.alloc(Node {
                kind: Kind::StringLiteral,
                flags: crate::NodeFlags::NONE,
                loc: TextRange::new(1, 2),
                id: Cell::new(0),
                parent: Cell::new(NodeId::NONE),
                data: NodeData::StringLiteral(Box::new(d)),
            })
        };
        // Attach them as the file's statements so the child walk finds them.
        let statements = NodeList {
            loc: TextRange::new(0, 2),
            nodes: vec![identifier, literal].into_boxed_slice(),
        };
        file.payload_mut().statements = Some(statements);
        assert!(file.has_identifier("x"));
        assert!(file.has_identifier("y"));
        assert!(!file.has_identifier("z"));
    }

    /// Go: `func (file *SourceFile) GetOrComputeData(...)` — once per key.
    #[test]
    fn get_or_compute_data_runs_once() {
        let mut file = SourceFile::new(0, SourceFileParseOptions::default(), "", None, None);
        let key = new_source_file_data_key::<i32>();
        let mut calls = 0;
        let v = file.get_or_compute_data(&key, |_| {
            calls += 1;
            42
        });
        assert_eq!(v, 42);
        assert_eq!(calls, 1);
        // Second call reuses the cell.
        let v2 = file.get_or_compute_data(&key, |_| {
            calls += 1;
            -1
        });
        assert_eq!(v2, 42);
        assert_eq!(calls, 1);
    }

    /// Go: BindOnce/IsBound — the bind closure runs at most once and flips the
    /// bound flag even when it does nothing.
    #[test]
    fn bind_once_runs_once() {
        let mut file = SourceFile::new(0, SourceFileParseOptions::default(), "", None, None);
        assert!(!file.is_bound());
        file.bind_once(|_| {});
        assert!(file.is_bound());
        let mut ran = 0;
        file.bind_once(|_| ran += 1);
        assert_eq!(ran, 0);
    }

    /// PragmaSpecification.IsTripleSlash and the PragmaKindFlags bit layout.
    #[test]
    fn pragma_kind_flags_mirror_go() {
        assert_eq!(PRAGMA_KIND_TRIPLE_SLASH_XML, 1);
        assert_eq!(PRAGMA_KIND_SINGLE_LINE, 1 << 1);
        assert_eq!(PRAGMA_KIND_MULTI_LINE, 1 << 2);
        assert_eq!(PRAGMA_KIND_ALL, 7);
        assert_eq!(PRAGMA_KIND_DEFAULT, PRAGMA_KIND_ALL);
        let mut spec = PragmaSpecification::default();
        assert!(!spec.is_triple_slash());
        spec.kind = PRAGMA_KIND_TRIPLE_SLASH_XML | PRAGMA_KIND_SINGLE_LINE;
        assert!(spec.is_triple_slash());
    }
}
