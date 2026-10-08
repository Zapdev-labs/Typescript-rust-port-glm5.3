//! Go: `tsc/internal/parser/parser.go` (part 1) — the `Parser` core: state
//! machine (mark/rewind/lookAhead), diagnostic plumbing, list machinery,
//! token consumption, node finishing, and the `ParseSourceFile` worker.

use std::cell::RefCell;
use std::rc::Rc;

use tsc_ast::{
    Diagnostic, Kind, ModifierList, NodeFlags, NodeId, NodeList, NodeStore, SourceFile,
    SourceFileParseOptions,
};
use tsc_collections::Set;
use tsc_core::languagevariant::LanguageVariant;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::Message;
use tsc_scanner::{token_to_text, Scanner, ScannerState};

use crate::{
    get_language_variant, is_keyword_or_punctuation, token_is_identifier_or_keyword,
    ParsingContext, PARSE_FLAGS_AWAIT, PARSE_FLAGS_NONE, PARSE_FLAGS_YIELD,
    PC_ARGUMENT_EXPRESSIONS, PC_ARRAY_BINDING_ELEMENTS, PC_ARRAY_LITERAL_MEMBERS,
    PC_BLOCK_STATEMENTS, PC_CLASS_MEMBERS, PC_COUNT, PC_ENUM_MEMBERS, PC_HERITAGE_CLAUSES,
    PC_HERITAGE_CLAUSE_ELEMENT, PC_IMPORT_ATTRIBUTES, PC_IMPORT_OR_EXPORT_SPECIFIERS,
    PC_JSDOC_COMMENT, PC_JSDOC_PARAMETERS, PC_JSX_ATTRIBUTES, PC_JSX_CHILDREN,
    PC_OBJECT_BINDING_ELEMENTS, PC_OBJECT_LITERAL_MEMBERS, PC_PARAMETERS, PC_REST_PROPERTIES,
    PC_SOURCE_ELEMENTS, PC_SWITCH_CLAUSES, PC_SWITCH_CLAUSE_STATEMENTS, PC_TUPLE_ELEMENT_TYPES,
    PC_TYPE_ARGUMENTS, PC_TYPE_MEMBERS, PC_TYPE_PARAMETERS, PC_VARIABLE_DECLARATIONS,
};

// ────────────────────────────────────────────────────────────────────────────
// jsdocScannerInfo
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type jsdocScannerInfo uint8` + consts.
pub(crate) type JSDocScannerInfo = u8;
pub(crate) const JSDOC_SCANNER_INFO_HAS_JSDOC: JSDocScannerInfo = 1 << 0;
pub(crate) const JSDOC_SCANNER_INFO_HAS_DEPRECATED: JSDocScannerInfo = 1 << 1;
pub(crate) const JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK: JSDocScannerInfo = 1 << 2;

/// Go: `type JSDocInfo struct { parent *ast.Node; jsDocs []*ast.Node }`.
pub(crate) struct JSDocInfo {
    pub parent: NodeId,
    pub js_docs: Vec<NodeId>,
}

// ────────────────────────────────────────────────────────────────────────────
// Diagnostics plumbing
// ────────────────────────────────────────────────────────────────────────────

/// The three parser-owned diagnostic streams + the sticky `hasParseError`
/// flag (Go: Parser fields). Split into a `RefCell`'d sink so the scanner's
/// error callback (which cannot borrow the `Parser`) and `mark`/`rewind`
/// truncation share the same storage.
#[derive(Default)]
pub(crate) struct DiagnosticSink {
    pub diagnostics: Vec<Diagnostic>,
    pub js_diagnostics: Vec<Diagnostic>,
    pub jsdoc_diagnostics: Vec<Diagnostic>,
    pub has_parse_error: bool,
}

/// Go: `ast.NewDiagnostic(nil, loc, message, args...)` — `StringifyArgs`
/// already ran at the call sites (all parser args are `String`s), and the
/// messageText stays empty exactly like Go (the localized text comes from
/// `message`).
pub(crate) fn new_diagnostic(
    loc: TextRange,
    message: &'static Message,
    args: &[String],
) -> Diagnostic {
    Diagnostic::new(
        None,
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

/// The shared body of `parseErrorAtRange` / the scanner's `scanError`.
pub(crate) fn parse_error_into(
    sink: &mut DiagnosticSink,
    loc: TextRange,
    message: &'static Message,
    args: &[String],
) -> Option<usize> {
    // Don't report another error if it would just be at the same location as the last error
    let result = if sink.diagnostics.is_empty()
        || sink.diagnostics[sink.diagnostics.len() - 1].pos() != loc.pos()
    {
        let diagnostic = new_diagnostic(loc, message, args);
        sink.diagnostics.push(diagnostic);
        Some(sink.diagnostics.len() - 1)
    } else {
        None
    };
    sink.has_parse_error = true;
    result
}

// ────────────────────────────────────────────────────────────────────────────
// ParserState (mark/rewind)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type ParserState struct`.
pub(crate) struct ParserState<'a> {
    pub scanner_state: ScannerState<'a>,
    pub context_flags: NodeFlags,
    pub diagnostics_len: usize,
    pub js_diagnostics_len: usize,
    pub jsdoc_infos_len: usize,
    pub statement_has_await_identifier: bool,
    pub has_parse_error: bool,
    // PORT: Go also tracks `reparsedClonesLen`; reparsed clones are part of
    // the skipped reparser.go wave, so the field is dropped (the vec is
    // always empty).
}

// ────────────────────────────────────────────────────────────────────────────
// Parser
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Parser struct`. The `NodeFactory` borrows the owning
/// `SourceFile` arena; the scanner borrows the source text.
///
/// PORT: Go pools `Parser` values in `parserPool` (`sync.Pool`) and reuses
/// the scanner + closure slots across parses; the port plain-constructs one
/// `Parser` per parse. Every parse allocates a fresh arena-owning
/// `SourceFile` anyway (Go's pooled state is the `Scanner` and two closures),
/// so the pool saves only two small allocations — a wash against the Rc
/// indirection the port needs for the scanner error callback.
pub(crate) struct Parser<'a, 'f> {
    pub factory: tsc_ast::NodeFactory<'f>,
    pub scanner: Scanner<'a>,

    /// The parser-owned diagnostic streams (Go: Parser fields). Shared with
    /// the scanner's error callback through the Rc.
    pub(crate) sink: Rc<RefCell<DiagnosticSink>>,

    pub opts: SourceFileParseOptions,
    pub source_text: &'a str,

    pub script_kind: ScriptKind,
    pub language_variant: LanguageVariant,

    pub token: Kind,
    pub source_flags: NodeFlags,
    pub context_flags: NodeFlags,
    pub parsing_contexts: i32,
    pub statement_has_await_identifier: bool,
    pub has_deprecated_tag: bool,

    pub identifier_count: i32,
    pub not_parenthesized_arrow: Set<i32>,
    pub jsdoc_infos: Vec<JSDocInfo>,
    pub possible_await_spans: Vec<i32>,
    /// Go: `reparseList` — only ever extended by `reparseTags` (the skipped
    /// reparser.go wave); kept so `parseListIndex`'s propagation plumbing is
    /// already in place when that wave lands.
    pub reparse_list: Vec<NodeId>,
    /// PORT: Go distinguishes "missing" node lists from empty ones via the
    /// `missingListNodes` sentinel (a cap==1 backing array trick, which has
    /// no `Box<[NodeId]>` equivalent). The only consumer is
    /// `typeHasArrowFunctionBlockingParseError` (FunctionType/ConstructorType
    /// parameters); the flag is latched by `parse_parameters` and consumed
    /// immediately by `parse_function_or_constructor_type`.
    pub last_parameters_list_missing: bool,
    pub missing_function_type_params: Set<NodeId>,
}

impl<'a, 'f> Parser<'a, 'f> {
    /// Go: `getParser` + `p.initializeState(opts, sourceText, scriptKind)`
    /// (the pool is dropped, see the struct PORT note). `f` is the file the
    /// `NodeFactory` borrows — the caller pre-allocated it with slot 0.
    pub fn new(
        file: &'f mut SourceFile,
        opts: SourceFileParseOptions,
        source_text: &'a str,
        script_kind: ScriptKind,
    ) -> Parser<'a, 'f> {
        let sink = Rc::new(RefCell::new(DiagnosticSink::default()));
        let mut p = Parser {
            factory: tsc_ast::NodeFactory::new(file),
            scanner: Scanner::new(),
            sink: Rc::clone(&sink),
            opts,
            source_text,
            script_kind,
            language_variant: LanguageVariant::Standard,
            token: Kind::Unknown,
            source_flags: NodeFlags::NONE,
            context_flags: NodeFlags::NONE,
            parsing_contexts: 0,
            statement_has_await_identifier: false,
            has_deprecated_tag: false,
            identifier_count: 0,
            not_parenthesized_arrow: Set::new(),
            jsdoc_infos: Vec::new(),
            possible_await_spans: Vec::new(),
            reparse_list: Vec::new(),
            last_parameters_list_missing: false,
            missing_function_type_params: Set::new(),
        };
        p.initialize_state();
        // Go: p.initializeClosures wires p.scanError into the scanner; the
        // Rust callback shares the sink through the Rc (it cannot borrow
        // `p`, which the scanner lives inside).
        let error_sink = Rc::clone(&sink);
        p.scanner.set_on_error(move |message, pos, length, args| {
            // Go: p.scanError → p.parseErrorAtRange(NewTextRange(pos, pos+length), ...)
            let mut sink = error_sink.borrow_mut();
            parse_error_into(&mut sink, TextRange::new(pos, pos + length), message, args);
        });
        p
    }

    /// The number of parse diagnostics currently recorded (Go:
    /// `len(p.diagnostics)`) — used by the `ParseIsolatedEntityName` entry.
    pub(crate) fn sink_diagnostics_len(&self) -> usize {
        self.sink.borrow().diagnostics.len()
    }

    pub(crate) fn is_javascript(&self) -> bool {
        self.script_kind == ScriptKind::JS || self.script_kind == ScriptKind::JSX
    }

    /// Go: `func (p *Parser) initializeState(...)`.
    fn initialize_state(&mut self) {
        if self.script_kind == ScriptKind::Unknown {
            panic!(
                "ScriptKind must be specified when parsing source file: {}",
                self.opts.file_name.as_string()
            );
        }
        self.language_variant = get_language_variant(self.script_kind);
        self.context_flags = match self.script_kind {
            ScriptKind::JS | ScriptKind::JSX => NodeFlags::JAVASCRIPT_FILE,
            ScriptKind::JSON => NodeFlags::JAVASCRIPT_FILE.union(NodeFlags::JSON_FILE),
            _ => NodeFlags::NONE,
        };
        self.scanner.reset();
        self.scanner.set_text(self.source_text);
        // SetOnError was wired in `new` (it cannot be re-borrowed here).
        self.scanner.set_language_variant(self.language_variant);
    }

    // ────────────────────────────────────────────────────────────────────────
    // Errors
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseErrorAt(pos, end int, message, args...) *ast.Diagnostic`.
    pub(crate) fn parse_error_at(
        &mut self,
        pos: i32,
        end: i32,
        message: &'static Message,
        args: &[String],
    ) -> Option<usize> {
        self.parse_error_at_range(TextRange::new(pos, end), message, args)
    }

    /// Go: `func (p *Parser) parseErrorAtCurrentToken(message, args...) *ast.Diagnostic`.
    pub(crate) fn parse_error_at_current_token(
        &mut self,
        message: &'static Message,
        args: &[String],
    ) -> Option<usize> {
        {
            let __hoist_1_2 = self.scanner.token_range();
            let __hoist_1_3 = message;
            let __hoist_1_4 = args;
            self.parse_error_at_range(__hoist_1_2, __hoist_1_3, __hoist_1_4)
        }
    }

    /// Go: `func (p *Parser) parseErrorAtRange(loc, message, args...) *ast.Diagnostic`.
    /// PORT: returns the index of the appended diagnostic (Go returns the
    /// pointer) so `AddRelatedInfo` call sites can mutate the stored entry.
    pub(crate) fn parse_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static Message,
        args: &[String],
    ) -> Option<usize> {
        let mut sink = self.sink.borrow_mut();
        parse_error_into(&mut sink, loc, message, args)
    }

    /// Go: `lastError.AddRelatedInfo(related)` where `lastError` came from a
    /// `parseErrorAt*` index.
    pub(crate) fn add_related_info(&mut self, index: usize, related: Diagnostic) {
        self.sink.borrow_mut().diagnostics[index].add_related_info(related);
    }

    /// Go: `len(p.diagnostics)` (used by the import-attribute related-info
    /// paths that read "the last diagnostic").
    pub(crate) fn last_diagnostic_code(&self, index: usize) -> i32 {
        self.sink.borrow().diagnostics[index].code()
    }

    // ────────────────────────────────────────────────────────────────────────
    // State machine
    // ────────────────────────────────────────────────────────────────────────

    pub(crate) fn mark(&self) -> ParserState<'a> {
        ParserState {
            scanner_state: self.scanner.mark(),
            context_flags: self.context_flags,
            diagnostics_len: self.sink.borrow().diagnostics.len(),
            js_diagnostics_len: self.sink.borrow().js_diagnostics.len(),
            jsdoc_infos_len: self.jsdoc_infos.len(),
            statement_has_await_identifier: self.statement_has_await_identifier,
            has_parse_error: self.sink.borrow().has_parse_error,
        }
    }

    pub(crate) fn rewind(&mut self, state: ParserState<'a>) {
        self.scanner.rewind(state.scanner_state);
        self.token = self.scanner.token();
        self.context_flags = state.context_flags;
        {
            let mut sink = self.sink.borrow_mut();
            sink.diagnostics.truncate(state.diagnostics_len);
            sink.js_diagnostics.truncate(state.js_diagnostics_len);
        }
        self.jsdoc_infos.truncate(state.jsdoc_infos_len);
        self.statement_has_await_identifier = state.statement_has_await_identifier;
        self.sink.borrow_mut().has_parse_error = state.has_parse_error;
    }

    pub(crate) fn look_ahead(
        &mut self,
        callback: impl FnOnce(&mut Parser<'a, 'f>) -> bool,
    ) -> bool {
        let state = self.mark();
        let result = callback(self);
        self.rewind(state);
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Token advance
    // ────────────────────────────────────────────────────────────────────────

    pub(crate) fn next_token(&mut self) -> Kind {
        // if the keyword had an escape
        if tsc_ast::is_keyword_kind(self.token)
            && (self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape())
        {
            // issue a parse error for the escape
            self.parse_error_at_current_token(
                &tsc_diagnostics::KEYWORDS_CANNOT_CONTAIN_ESCAPE_CHARACTERS,
                &[],
            );
        }
        self.token = self.scanner.scan();
        self.token
    }

    pub(crate) fn next_token_without_check(&mut self) -> Kind {
        self.token = self.scanner.scan();
        self.token
    }

    pub(crate) fn next_token_jsdoc(&mut self) -> Kind {
        self.token = self.scanner.scan_jsdoc_token();
        self.token
    }

    pub(crate) fn next_jsdoc_comment_text_token(&mut self, in_backticks: bool) -> Kind {
        self.token = self.scanner.scan_jsdoc_comment_text_token(in_backticks);
        self.token
    }

    pub(crate) fn node_pos(&self) -> TextPos {
        self.scanner.token_full_start() as i32
    }

    pub(crate) fn has_preceding_line_break(&self) -> bool {
        self.scanner.has_preceding_line_break()
    }

    pub(crate) fn jsdoc_scanner_info(&self) -> JSDocScannerInfo {
        if !self.scanner.has_preceding_jsdoc_comment() {
            return 0;
        }
        let mut info = JSDOC_SCANNER_INFO_HAS_JSDOC;
        if self.scanner.has_preceding_jsdoc_with_deprecated_tag() {
            info |= JSDOC_SCANNER_INFO_HAS_DEPRECATED;
        }
        if self.scanner.has_preceding_jsdoc_with_see_or_link() {
            info |= JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK;
        }
        info
    }

    /// Go: `scanner.TokenValue()` (bytes) → String.
    ///
    /// PORT: Go's scanner produces raw bytes for string literals (which Go
    /// strings allow); the Rust scanner returns `&[u8]` and the lossy
    /// conversion maps any lone-surrogate CESU-8 bytes to U+FFFD. The parser
    /// only ever re-stores these bytes into Identifier/StringLiteral text,
    /// so a lone-surrogate source yields replacement chars instead of Go's
    /// unpaired surrogate — noted in PORTING-NOTES.
    pub(crate) fn token_value_string(&self) -> String {
        String::from_utf8_lossy(self.scanner.token_value()).into_owned()
    }

    // ────────────────────────────────────────────────────────────────────────
    // parseJSONText
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseJSONText() *ast.SourceFile` — PORT: returns
    /// nothing; the fresh SourceFile node Go allocates is the arena's slot-0
    /// node here (see `finish_source_file`).
    pub(crate) fn parse_json_text(&mut self) {
        let pos = self.node_pos();
        let statements: NodeList;
        let eof: NodeId;
        if self.token == Kind::EndOfFile {
            statements = {
                let __hoist_1_6 = TextRange::new(pos, self.node_pos());
                let __hoist_1_7 = Vec::new();
                self.new_node_list(__hoist_1_6, __hoist_1_7)
            };
            eof = self.parse_token_node();
        } else {
            // Go: `var expressions any // []*ast.Expression | *ast.Expression`
            let mut expressions: Vec<NodeId> = Vec::new();
            let mut multiple = false;
            while self.token != Kind::EndOfFile {
                let expression: NodeId = match self.token {
                    Kind::OpenBracketToken => self.parse_array_literal_expression(),
                    Kind::TrueKeyword | Kind::FalseKeyword | Kind::NullKeyword => {
                        self.parse_token_node()
                    }
                    Kind::MinusToken => {
                        if self.look_ahead(|p| {
                            p.next_token() == Kind::NumericLiteral
                                && p.next_token() != Kind::ColonToken
                        }) {
                            self.parse_prefix_unary_expression()
                        } else {
                            self.parse_object_literal_expression()
                        }
                    }
                    Kind::NumericLiteral | Kind::StringLiteral => {
                        // Go: fallthrough — parse as a literal only when the
                        // value is not the key of a following `:`.
                        if self.look_ahead(|p| p.next_token() != Kind::ColonToken) {
                            self.parse_literal_expression()
                        } else {
                            self.parse_object_literal_expression()
                        }
                    }
                    _ => self.parse_object_literal_expression(),
                };
                // Error recovery: collect multiple top-level expressions
                if !expressions.is_empty() {
                    multiple = true;
                }
                if expressions.is_empty() && self.token != Kind::EndOfFile {
                    self.parse_error_at_current_token(&tsc_diagnostics::UNEXPECTED_TOKEN, &[]);
                }
                expressions.push(expression);
            }
            let _ = multiple;
            let expression: NodeId = if multiple {
                let elements = {
                    let __hoist_1_9 = TextRange::new(pos, self.node_pos());
                    let __hoist_1_10 = expressions;
                    self.new_node_list(__hoist_1_9, __hoist_1_10)
                };
                {
                    let __hoist_1_12 = self
                        .factory
                        .new_array_literal_expression(Some(elements), false);
                    let __hoist_1_13 = pos;
                    self.finish_node(__hoist_1_12, __hoist_1_13)
                }
            } else {
                expressions
                    .into_iter()
                    .next()
                    .expect("at least one expression")
            };
            let statement = {
                let __hoist_1_15 = self.factory.new_expression_statement(expression);
                let __hoist_1_16 = pos;
                self.finish_node(__hoist_1_15, __hoist_1_16)
            };
            statements = {
                let __hoist_1_18 = TextRange::new(pos, self.node_pos());
                let __hoist_1_19 = vec![statement];
                self.new_node_list(__hoist_1_18, __hoist_1_19)
            };
            eof = self.parse_expected_token(Kind::EndOfFile);
        }
        let node = {
            let __hoist_1_21 = self.factory.new_source_file(Some(statements), Some(eof));
            let __hoist_1_22 = pos;
            self.finish_node(__hoist_1_21, __hoist_1_22)
        };
        // result := node.AsSourceFile()
        if let Some(first) = self
            .factory
            .store()
            .node(node)
            .as_source_file()
            .and_then(|d| d.statements.as_ref())
            .and_then(|l| l.nodes.first().copied())
        {
            let expression = {
                let s = self.factory.store();
                s.node(first)
                    .as_expression_statement()
                    .expect("ExpressionStatement data")
                    .expression
            };
            self.validate_json_value(node, expression);
        }
        self.finish_source_file(node, false);
    }

    /// Go: `func getErrorSpanForNode(sourceText string, node *ast.Node) core.TextRange`.
    fn get_error_span_for_node(&mut self, node: NodeId) -> TextRange {
        let (pos, end, missing) = {
            let s = self.factory.store();
            let n = s.node(node);
            (n.pos(), n.end(), tsc_ast::node_is_missing(n))
        };
        let pos = if !missing {
            tsc_scanner::skip_trivia(self.source_text, pos)
        } else {
            pos
        };
        TextRange::new(pos, end)
    }

    /// Go: `func (p *Parser) validateJsonValue(sourceFile *ast.SourceFile, valueExpression *ast.Expression)`.
    fn validate_json_value(&mut self, source_file: NodeId, value_expression: NodeId) {
        let (kind, expr) = {
            let s = self.factory.store();
            let n = s.node(value_expression);
            (n.kind, value_expression)
        };
        let _ = expr;
        match kind {
            Kind::TrueKeyword | Kind::FalseKeyword | Kind::NullKeyword | Kind::NumericLiteral => {
                return;
            }
            Kind::StringLiteral => {
                if !self.is_double_quoted_string(value_expression) {
                    let span = self.get_error_span_for_node(value_expression);
                    let mut sink = self.sink.borrow_mut();
                    sink.diagnostics.push(Diagnostic::new(
                        Some(source_file),
                        span,
                        tsc_diagnostics::STRING_LITERAL_WITH_DOUBLE_QUOTES_EXPECTED.code(),
                        tsc_diagnostics::STRING_LITERAL_WITH_DOUBLE_QUOTES_EXPECTED.category(),
                        "",
                        Some(&tsc_diagnostics::STRING_LITERAL_WITH_DOUBLE_QUOTES_EXPECTED),
                        "",
                        tsc_diagnostics::STRING_LITERAL_WITH_DOUBLE_QUOTES_EXPECTED.key(),
                        Vec::new(),
                    ));
                }
                return;
            }
            Kind::PrefixUnaryExpression => {
                let (operator, operand) = {
                    let s = self.factory.store();
                    let d = s
                        .node(value_expression)
                        .as_prefix_unary_expression()
                        .expect("PrefixUnaryExpression data");
                    (d.operator, d.operand)
                };
                let operand_kind = {
                    let s = self.factory.store();
                    s.node(operand).kind
                };
                if operator == Kind::MinusToken && operand_kind == Kind::NumericLiteral {
                    return;
                }
                // else fall through to the trailing error
            }
            Kind::ObjectLiteralExpression => {
                self.validate_json_object_literal(source_file, value_expression);
                return;
            }
            Kind::ArrayLiteralExpression => {
                let elements: Vec<NodeId> = {
                    let s = self.factory.store();
                    let d = s
                        .node(value_expression)
                        .as_array_literal_expression()
                        .expect("ArrayLiteralExpression data");
                    d.elements
                        .as_ref()
                        .map(|l| l.nodes.to_vec())
                        .unwrap_or_default()
                };
                for element in elements {
                    self.validate_json_value(source_file, element);
                }
                return;
            }
            _ => {}
        }
        let span = self.get_error_span_for_node(value_expression);
        let mut sink = self.sink.borrow_mut();
        sink.diagnostics.push(new_diagnostic(
            span,
            &tsc_diagnostics::PROPERTY_VALUE_CAN_ONLY_BE_STRING_LITERAL_NUMERIC_LITERAL_TRUE_FALSE_NULL_OBJECT_LITERAL_OR_ARRAY_LITERAL,
            &[],
        ));
    }

    /// Go: `func isDoubleQuotedString(node *ast.Node) bool`.
    fn is_double_quoted_string(&mut self, node: NodeId) -> bool {
        let s = self.factory.store();
        let n = s.node(node);
        tsc_ast::is_string_literal(n)
            && !n
                .as_string_literal()
                .expect("StringLiteral data")
                .token_flags
                .intersects(tsc_ast::TokenFlags::SINGLE_QUOTE)
    }

    /// Go: `validateJsonObjectLiteral` — validates properties of a JSON object literal.
    fn validate_json_object_literal(&mut self, source_file: NodeId, node: NodeId) {
        let properties: Vec<NodeId> = {
            let s = self.factory.store();
            let d = s
                .node(node)
                .as_object_literal_expression()
                .expect("ObjectLiteralExpression data");
            d.properties
                .as_ref()
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default()
        };
        for element in properties {
            let (kind, name) = {
                let s = self.factory.store();
                let n = s.node(element);
                (n.kind, node_name_of(s, element))
            };
            if kind != Kind::PropertyAssignment {
                let span = self.get_error_span_for_node(element);
                self.sink.borrow_mut().diagnostics.push(new_diagnostic(
                    span,
                    &tsc_diagnostics::PROPERTY_ASSIGNMENT_EXPECTED,
                    &[],
                ));
                continue;
            }
            if let Some(name) = name {
                if !self.is_double_quoted_string(name) {
                    let span = self.get_error_span_for_node(name);
                    self.sink.borrow_mut().diagnostics.push(new_diagnostic(
                        span,
                        &tsc_diagnostics::STRING_LITERAL_WITH_DOUBLE_QUOTES_EXPECTED,
                        &[],
                    ));
                }
            }
            let initializer = {
                let s = self.factory.store();
                s.node(element)
                    .as_property_assignment()
                    .expect("PropertyAssignment data")
                    .initializer
            };
            self.validate_json_value(source_file, initializer);
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // parseSourceFileWorker
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseSourceFileWorker() *ast.SourceFile` — PORT:
    /// returns nothing; the worker updates the arena's slot-0 SourceFile node
    /// in place (Go allocates a fresh node from the factory).
    pub(crate) fn parse_source_file_worker(&mut self) {
        let is_declaration_file = self.opts.file_name.is_declaration_file();
        if is_declaration_file {
            self.context_flags |= NodeFlags::AMBIENT;
        }
        let pos = self.node_pos();
        let statements =
            self.parse_list_index(PC_SOURCE_ELEMENTS, |p, i| p.parse_toplevel_statement(i));
        let end = self.node_pos();
        let end_jsdoc = self.jsdoc_scanner_info();
        let eof = self.parse_token_node();
        self.with_jsdoc(eof, end_jsdoc);
        let res = {
            let s = self.factory.store();
            s.node(eof).kind != Kind::EndOfFile
        };
        if res {
            panic!("Expected end of file token from scanner.");
        }
        // (Go: reparseList append — always empty until the reparser wave.)
        let node = {
            let __hoist_1_24 = {
                let __hoist_2_2 = Some(self.new_node_list(TextRange::new(pos, end), statements));
                let __hoist_2_3 = Some(eof);
                self.factory.new_source_file(__hoist_2_2, __hoist_2_3)
            };
            let __hoist_1_25 = pos;
            self.finish_node(__hoist_1_24, __hoist_1_25)
        };
        self.finish_source_file(node, is_declaration_file);
        let needs_reparse = {
            let s = self.factory.store();
            let d = s.node(node).as_source_file().expect("SourceFile data");
            !d.is_declaration_file
                && d.external_module_indicator.get().is_some()
                && !self.possible_await_spans.is_empty()
        };
        if needs_reparse {
            let reparse = self.reparse_top_level_await(node);
            let reparse = self.finish_node(reparse, pos);
            // Go: `if node != reparse { result = reparse.AsSourceFile();
            // p.finishSourceFile(result, ...) }` — Go's NewSourceFile always
            // allocates a fresh node so the guard is constant-true; with the
            // in-place update the ids match, so finishSourceFile re-runs
            // unconditionally (PORT).
            let _ = reparse;
            self.finish_source_file(node, is_declaration_file);
        }
        super::references::collect_external_module_references(self.factory.store(), node);
        let is_in_js = {
            let s = self.factory.store();
            tsc_ast::is_in_js_file(s.node(node))
        };
        if is_in_js {
            let js_diagnostics = std::mem::take(&mut self.sink.borrow_mut().js_diagnostics);
            let js_diagnostics = attach_file_to_diagnostics(js_diagnostics, node);
            let s = self.factory.store();
            s.node_mut(node)
                .as_source_file_mut()
                .expect("SourceFile data")
                .js_diagnostics = js_diagnostics;
        }
    }

    /// Go: `func (p *Parser) finishSourceFile(result *ast.SourceFile, isDeclarationFile bool)`.
    fn finish_source_file(&mut self, result: NodeId, is_declaration_file: bool) {
        let pragmas = self.get_comment_pragmas(self.source_text);
        {
            let s = self.factory.store();
            let d = s
                .node_mut(result)
                .as_source_file_mut()
                .expect("SourceFile data");
            d.comment_directives = self.scanner.comment_directives().to_vec();
            d.pragmas = pragmas;
        }
        self.process_pragmas_into_fields(result);
        let diagnostics = std::mem::take(&mut self.sink.borrow_mut().diagnostics);
        let jsdoc_diagnostics = std::mem::take(&mut self.sink.borrow_mut().jsdoc_diagnostics);
        let diagnostics = attach_file_to_diagnostics(diagnostics, result);
        let jsdoc_diagnostics = attach_file_to_diagnostics(jsdoc_diagnostics, result);
        let jsdoc_cache = self.create_jsdoc_cache();
        let (node_count, text_count) = (
            self.factory.node_count() as i32,
            self.factory.text_count() as i32,
        );
        let is_js = self.is_javascript();
        {
            let s = self.factory.store();
            s.node_mut(result).flags |= self.source_flags;
            let d = s
                .node_mut(result)
                .as_source_file_mut()
                .expect("SourceFile data");
            d.diagnostics = diagnostics;
            d.jsdoc_diagnostics = jsdoc_diagnostics;
            d.is_declaration_file = is_declaration_file;
            d.language_variant = self.language_variant;
            d.script_kind = self.script_kind;
            d.node_count = node_count;
            d.text_count = text_count;
            d.identifier_count = self.identifier_count;
            d.jsdoc_cache = Some(jsdoc_cache);
            // For non-JS files, enable lazy JSDoc parsing on demand
            if !is_js {
                d.has_lazy_jsdoc = true;
            }
        }
        tsc_ast::set_external_module_indicator(
            self.factory.store(),
            result,
            self.opts.external_module_indicator_options,
        );
    }

    /// Go: `func (p *Parser) createJSDocCache() map[*ast.Node][]*ast.Node`.
    fn create_jsdoc_cache(&self) -> std::collections::HashMap<NodeId, Vec<NodeId>> {
        if self.jsdoc_infos.is_empty() {
            return std::collections::HashMap::new();
        }
        let mut result = std::collections::HashMap::with_capacity(self.jsdoc_infos.len());
        for info in &self.jsdoc_infos {
            result.insert(info.parent, info.js_docs.clone());
        }
        result
    }

    /// Go: `func (p *Parser) parseToplevelStatement(i int) *ast.Node`.
    fn parse_toplevel_statement(&mut self, i: usize) -> Option<NodeId> {
        self.statement_has_await_identifier = false;
        let statement = self.parse_statement();
        // Reparsed nodes (e.g. JSDoc @typedef) produced while parsing this
        // statement are inserted into the statement list before this
        // statement, so account for them when recording the statement's
        // index for possibleAwaitSpans.
        let i = i + self.reparse_list.len();
        if self.statement_has_await_identifier {
            let has_await_context = {
                let s = self.factory.store();
                s.node(statement).flags.intersects(NodeFlags::AWAIT_CONTEXT)
            };
            if !has_await_context {
                let i = i as i32;
                let spans = &mut self.possible_await_spans;
                if spans.is_empty() || spans[spans.len() - 1] != i {
                    spans.push(i);
                    spans.push(i + 1);
                } else {
                    let last = spans.len() - 1;
                    spans[last] = i + 1;
                }
            }
        }
        Some(statement)
    }

    /// Go: `func (p *Parser) reparseTopLevelAwait(sourceFile *ast.SourceFile) *ast.Node`
    /// — returns the (in-place updated, PORT) SourceFile node.
    fn reparse_top_level_await(&mut self, source_file: NodeId) -> NodeId {
        if self.possible_await_spans.len() % 2 == 1 {
            panic!("possibleAwaitSpans malformed: odd number of indices, not paired into spans.");
        }
        let mut statements: Vec<NodeId> = Vec::new();
        let saved_parse_diagnostics: Vec<Diagnostic> =
            std::mem::take(&mut self.sink.borrow_mut().diagnostics);

        let (statement_nodes, statements_loc, end_of_file_token) = {
            let s = self.factory.store();
            let d = s
                .node(source_file)
                .as_source_file()
                .expect("SourceFile data");
            (
                d.statements
                    .as_ref()
                    .map(|l| l.nodes.to_vec())
                    .unwrap_or_default(),
                d.statements.as_ref().map(|l| l.loc).unwrap_or_default(),
                d.end_of_file_token,
            )
        };

        let node_pos_of = |p: &mut Self, id: NodeId| -> TextPos {
            let s = p.factory.store();
            s.node(id).pos()
        };
        let node_end_of = |p: &mut Self, id: NodeId| -> TextPos {
            let s = p.factory.store();
            s.node(id).end()
        };

        let mut after_await_statement = 0usize;
        let mut i = 0usize;
        while i < self.possible_await_spans.len() {
            let next_await_statement = self.possible_await_spans[i] as usize;
            // append all non-await statements between afterAwaitStatement and nextAwaitStatement
            let prev_statement = statement_nodes[after_await_statement];
            let next_statement = statement_nodes[next_await_statement];
            statements
                .extend_from_slice(&statement_nodes[after_await_statement..next_await_statement]);

            // append all diagnostics associated with the copied range
            let prev_pos = node_pos_of(self, prev_statement);
            let next_pos = node_pos_of(self, next_statement);
            let diagnostic_start = saved_parse_diagnostics
                .iter()
                .position(|d| d.pos() >= prev_pos);
            if let Some(diagnostic_start) = diagnostic_start {
                let diagnostic_end = saved_parse_diagnostics[diagnostic_start..]
                    .iter()
                    .position(|d| d.pos() >= next_pos);
                let slice = match diagnostic_end {
                    Some(diagnostic_end) => {
                        &saved_parse_diagnostics
                            [diagnostic_start..diagnostic_start + diagnostic_end]
                    }
                    None => &saved_parse_diagnostics[diagnostic_start..],
                };
                self.sink
                    .borrow_mut()
                    .diagnostics
                    .extend(slice.iter().cloned());
            }

            let mut state = self.mark();
            // reparse all statements between start and pos. We skip existing
            // diagnostics for the same range and allow the parser to generate new ones.
            self.context_flags |= NodeFlags::AWAIT_CONTEXT;
            let next_statement_pos = node_pos_of(self, next_statement);
            self.scanner.reset_pos(next_statement_pos as usize);
            self.next_token();

            after_await_statement = self.possible_await_spans[i + 1] as usize;
            while self.token != Kind::EndOfFile {
                let start_pos = self.scanner.token_full_start();
                let statement = self.parse_statement();
                statements.push(statement);
                if start_pos == self.scanner.token_full_start() {
                    self.next_token();
                }
                if after_await_statement < statement_nodes.len() {
                    let last_await_statement = statement_nodes[after_await_statement - 1];
                    let statement_end = node_end_of(self, statement);
                    let last_end = node_end_of(self, last_await_statement);
                    if statement_end == last_end {
                        // done reparsing this section
                        break;
                    }
                    if statement_end > last_end {
                        // we ate into the next statement, so we must continue
                        // reparsing the next span
                        i += 2;
                        if i < self.possible_await_spans.len() {
                            after_await_statement = self.possible_await_spans[i + 1] as usize;
                        } else {
                            after_await_statement = statement_nodes.len();
                        }
                    }
                }
            }

            // Keep diagnostics from the reparse
            state.diagnostics_len = self.sink.borrow().diagnostics.len();
            self.rewind(state);
            i += 2;
        }

        // append all statements between pos and the end of the list
        if after_await_statement < statement_nodes.len() {
            let prev_statement = statement_nodes[after_await_statement];
            statements.extend_from_slice(&statement_nodes[after_await_statement..]);

            // append all diagnostics associated with the copied range
            let prev_pos = node_pos_of(self, prev_statement);
            let diagnostic_start = saved_parse_diagnostics
                .iter()
                .position(|d| d.pos() >= prev_pos);
            if let Some(diagnostic_start) = diagnostic_start {
                self.sink
                    .borrow_mut()
                    .diagnostics
                    .extend(saved_parse_diagnostics[diagnostic_start..].iter().cloned());
            }
        }

        // Go: p.factory.NewSourceFile(...) allocates a fresh file node; the
        // port updates slot 0 in place (PORT — see parseSourceFileWorker).
        let list = self.new_node_list(statements_loc, statements.clone());
        self.factory
            .update_source_file(source_file, Some(list), end_of_file_token);
        {
            let s = self.factory.store();
            for statement in statements {
                // force (re)set parent to reparsed source file
                s.node_mut(statement).parent.set(source_file);
            }
        }
        source_file
    }

    // ────────────────────────────────────────────────────────────────────────
    // Lists
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseListIndex(kind ParsingContext, parseElement func(p *Parser, index int) *ast.Node) []*ast.Node`.
    pub(crate) fn parse_list_index<F>(
        &mut self,
        kind: ParsingContext,
        mut parse_element: F,
    ) -> Vec<NodeId>
    where
        F: FnMut(&mut Parser<'a, 'f>, usize) -> Option<NodeId>,
    {
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << kind;
        let mut outer_reparse_list = std::mem::take(&mut self.reparse_list);
        let mut list: Vec<NodeId> = Vec::with_capacity(16);
        while !self.is_list_terminator(kind) {
            if self.is_list_element(kind, false /*in_error_recovery*/) {
                let elt = parse_element(self, list.len()).expect("non-nil list element");
                if !self.reparse_list.is_empty() {
                    for e in std::mem::take(&mut self.reparse_list) {
                        // Propagate @typedef type alias declarations outwards
                        // to a context that permits them.
                        let is_reparse_hoist = {
                            let s = self.factory.store();
                            let n = s.node(e);
                            (tsc_ast::is_js_type_alias_declaration(n)
                                || tsc_ast::is_js_import_declaration(n))
                                && kind != PC_SOURCE_ELEMENTS
                                && kind != PC_BLOCK_STATEMENTS
                        };
                        if is_reparse_hoist {
                            outer_reparse_list.push(e);
                        } else {
                            list.push(e);
                        }
                    }
                }
                list.push(elt);
                continue;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.reparse_list = outer_reparse_list;
        self.parsing_contexts = save_parsing_contexts;
        list
    }

    /// Go: `func (p *Parser) parseList(kind ParsingContext, parseElement func(p *Parser) *ast.Node) *ast.NodeList`.
    pub(crate) fn parse_list<F>(
        &mut self,
        kind: ParsingContext,
        mut parse_element: F,
    ) -> Option<NodeList>
    where
        F: FnMut(&mut Parser<'a, 'f>) -> Option<NodeId>,
    {
        let pos = self.node_pos();
        let nodes = self.parse_list_index(kind, |p, _i| parse_element(p));
        Some({
            let __hoist_1_27 = TextRange::new(pos, self.node_pos());
            let __hoist_1_28 = nodes;
            self.new_node_list(__hoist_1_27, __hoist_1_28)
        })
    }

    /// Go: `func (p *Parser) parseDelimitedList(...)` — return a non-nil (but
    /// possibly empty) list if parsing was successful, or None if
    /// parseElement failed.
    pub(crate) fn parse_delimited_list<F>(
        &mut self,
        kind: ParsingContext,
        mut parse_element: F,
    ) -> Option<NodeList>
    where
        F: FnMut(&mut Parser<'a, 'f>) -> Option<NodeId>,
    {
        let pos = self.node_pos();
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << kind;
        let mut list: Vec<NodeId> = Vec::with_capacity(16);
        loop {
            if self.is_list_element(kind, false /*in_error_recovery*/) {
                let start_pos = self.node_pos();
                let element = parse_element(self);
                let Some(element) = element else {
                    self.parsing_contexts = save_parsing_contexts;
                    // Return nil to indicate parseElement failed
                    return None;
                };
                list.push(element);
                if self.parse_optional(Kind::CommaToken) {
                    // No need to check for a zero length node since we know we parsed a comma
                    continue;
                }
                if self.is_list_terminator(kind) {
                    break;
                }
                // We didn't get a comma, and the list wasn't terminated, explicitly parse
                // out a comma so we give a good error message.
                if self.token != Kind::CommaToken && kind == PC_ENUM_MEMBERS {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::AN_ENUM_MEMBER_NAME_MUST_BE_FOLLOWED_BY_A_OR,
                        &[],
                    );
                } else {
                    self.parse_expected(Kind::CommaToken);
                }
                // If the token was a semicolon, and the caller allows that, then skip it and
                // continue.  This ensures we get back on track and don't result in tons of
                // parse errors.  For example, this can happen when people do things like use
                // a semicolon to delimit object literal members.   Note: we'll have already
                // reported an error when we called parseExpected above.
                if (kind == PC_OBJECT_LITERAL_MEMBERS || kind == PC_IMPORT_ATTRIBUTES)
                    && self.token == Kind::SemicolonToken
                    && !self.has_preceding_line_break()
                {
                    self.next_token();
                }
                if start_pos == self.node_pos() {
                    // What we're parsing isn't actually remotely recognizable as a element
                    // and we've consumed no tokens whatsoever. Consume a token to advance the
                    // parser in some way and avoid an infinite loop. This can happen when
                    // we're speculatively parsing parenthesized expressions which we think
                    // may be arrow functions, or when a modifier keyword which is disallowed
                    // as a parameter name (ie, `static` in strict mode) is supplied.
                    self.next_token();
                }
                continue;
            }
            if self.is_list_terminator(kind) {
                break;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.parsing_contexts = save_parsing_contexts;
        Some({
            let __hoist_1_30 = TextRange::new(pos, self.node_pos());
            let __hoist_1_31 = list;
            self.new_node_list(__hoist_1_30, __hoist_1_31)
        })
    }

    /// Go: `func (p *Parser) parseBracketedList(...)` — return a non-nil (but
    /// possibly empty) NodeList if parsing was successful, a missing NodeList
    /// if the opening token wasn't found, or None if parseElement failed.
    pub(crate) fn parse_bracketed_list<F>(
        &mut self,
        kind: ParsingContext,
        mut parse_element: F,
        opening: Kind,
        closing: Kind,
    ) -> Option<NodeList>
    where
        F: FnMut(&mut Parser<'a, 'f>) -> Option<NodeId>,
    {
        if self.parse_expected(opening) {
            let result = self.parse_delimited_list(kind, |p| parse_element(p));
            self.parse_expected(closing);
            return result;
        }
        Some(self.create_missing_list())
    }

    pub(crate) fn parse_empty_node_list(&mut self) -> NodeList {
        let pos = self.node_pos();
        self.new_node_list(TextRange::new(pos, pos), Vec::new())
    }

    /// Go: `func (p *Parser) createMissingList() *ast.NodeList` — PORT: the
    /// Go missing-list sentinel (see the Parser field docs) is represented by
    /// the `last_parameters_list_missing` latch set in `parse_parameters`.
    pub(crate) fn create_missing_list(&mut self) -> NodeList {
        self.parse_empty_node_list()
    }

    /// Returns true if we should abort parsing.
    /// Go: `func (p *Parser) abortParsingListOrMoveToNextToken(kind ParsingContext) bool`.
    fn abort_parsing_list_or_move_to_next_token(&mut self, kind: ParsingContext) -> bool {
        self.parsing_context_errors(kind);
        if self.is_in_some_parsing_context() {
            return true;
        }
        self.next_token();
        false
    }

    /// True if positioned at element or terminator of the current list or any
    /// enclosing list. Go: `func (p *Parser) isInSomeParsingContext() bool`.
    fn is_in_some_parsing_context(&mut self) -> bool {
        // We should be in at least one parsing context, be it SourceElements
        // while parsing a SourceFile, or JSDocComment when lazily parsing JSDoc.
        debug_assert!(self.parsing_contexts != 0, "Missing parsing context");
        for kind in 0..PC_COUNT {
            if self.parsing_contexts & (1 << kind) != 0
                && (self.is_list_element(kind, true /*inErrorRecovery*/)
                    || self.is_list_terminator(kind))
            {
                return true;
            }
        }
        false
    }

    /// Go: `func (p *Parser) parsingContextErrors(context ParsingContext)`.
    fn parsing_context_errors(&mut self, context: ParsingContext) {
        match context {
            PC_SOURCE_ELEMENTS => {
                if self.token == Kind::DefaultKeyword {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::X_0_EXPECTED,
                        &["export".to_string()],
                    );
                } else {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::DECLARATION_OR_STATEMENT_EXPECTED,
                        &[],
                    );
                }
            }
            PC_BLOCK_STATEMENTS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::DECLARATION_OR_STATEMENT_EXPECTED,
                    &[],
                );
            }
            PC_SWITCH_CLAUSES => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_CASE_OR_DEFAULT_EXPECTED,
                    &[],
                );
            }
            PC_SWITCH_CLAUSE_STATEMENTS => {
                self.parse_error_at_current_token(&tsc_diagnostics::STATEMENT_EXPECTED, &[]);
            }
            PC_REST_PROPERTIES | PC_TYPE_MEMBERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::PROPERTY_OR_SIGNATURE_EXPECTED,
                    &[],
                );
            }
            PC_CLASS_MEMBERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::UNEXPECTED_TOKEN_A_CONSTRUCTOR_METHOD_ACCESSOR_OR_PROPERTY_WAS_EXPECTED,
                    &[],
                );
            }
            PC_ENUM_MEMBERS => {
                self.parse_error_at_current_token(&tsc_diagnostics::ENUM_MEMBER_EXPECTED, &[]);
            }
            PC_HERITAGE_CLAUSE_ELEMENT => {
                self.parse_error_at_current_token(&tsc_diagnostics::EXPRESSION_EXPECTED, &[]);
            }
            PC_VARIABLE_DECLARATIONS => {
                if tsc_ast::is_keyword_kind(self.token) {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::X_0_IS_NOT_ALLOWED_AS_A_VARIABLE_DECLARATION_NAME,
                        &[token_to_text(self.token).to_string()],
                    );
                } else {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::VARIABLE_DECLARATION_EXPECTED,
                        &[],
                    );
                }
            }
            PC_OBJECT_BINDING_ELEMENTS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::PROPERTY_DESTRUCTURING_PATTERN_EXPECTED,
                    &[],
                );
            }
            PC_ARRAY_BINDING_ELEMENTS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::ARRAY_ELEMENT_DESTRUCTURING_PATTERN_EXPECTED,
                    &[],
                );
            }
            PC_ARGUMENT_EXPRESSIONS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::ARGUMENT_EXPRESSION_EXPECTED,
                    &[],
                );
            }
            PC_OBJECT_LITERAL_MEMBERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::PROPERTY_ASSIGNMENT_EXPECTED,
                    &[],
                );
            }
            PC_ARRAY_LITERAL_MEMBERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::EXPRESSION_OR_COMMA_EXPECTED,
                    &[],
                );
            }
            PC_JSDOC_PARAMETERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::PARAMETER_DECLARATION_EXPECTED,
                    &[],
                );
            }
            PC_PARAMETERS => {
                if tsc_ast::is_keyword_kind(self.token) {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::X_0_IS_NOT_ALLOWED_AS_A_PARAMETER_NAME,
                        &[token_to_text(self.token).to_string()],
                    );
                } else {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::PARAMETER_DECLARATION_EXPECTED,
                        &[],
                    );
                }
            }
            PC_TYPE_PARAMETERS => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::TYPE_PARAMETER_DECLARATION_EXPECTED,
                    &[],
                );
            }
            PC_TYPE_ARGUMENTS => {
                self.parse_error_at_current_token(&tsc_diagnostics::TYPE_ARGUMENT_EXPECTED, &[]);
            }
            PC_TUPLE_ELEMENT_TYPES => {
                self.parse_error_at_current_token(&tsc_diagnostics::TYPE_EXPECTED, &[]);
            }
            PC_HERITAGE_CLAUSES => {
                self.parse_error_at_current_token(&tsc_diagnostics::UNEXPECTED_TOKEN_EXPECTED, &[]);
            }
            PC_IMPORT_OR_EXPORT_SPECIFIERS => {
                if self.token == Kind::FromKeyword {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::X_0_EXPECTED,
                        &["}".to_string()],
                    );
                } else {
                    self.parse_error_at_current_token(&tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
                }
            }
            PC_JSX_ATTRIBUTES | PC_JSX_CHILDREN | PC_JSDOC_COMMENT => {
                self.parse_error_at_current_token(&tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
            }
            PC_IMPORT_ATTRIBUTES => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::IDENTIFIER_OR_STRING_LITERAL_EXPECTED,
                    &[],
                );
            }
            _ => panic!("Unhandled case in parsingContextErrors"),
        }
    }

    /// Go: `func (p *Parser) isListElement(parsingContext ParsingContext, inErrorRecovery bool) bool`.
    fn is_list_element(
        &mut self,
        parsing_context: ParsingContext,
        in_error_recovery: bool,
    ) -> bool {
        match parsing_context {
            PC_SOURCE_ELEMENTS | PC_BLOCK_STATEMENTS | PC_SWITCH_CLAUSE_STATEMENTS => {
                // If we're in error recovery, then we don't want to treat ';' as an
                // empty statement. The problem is that ';' can show up in far too many
                // contexts, and if we see one and assume it's a statement, then we may
                // bail out inappropriately from whatever we're parsing.  For example,
                // if we have a semicolon in the middle of a class, then we really
                // don't want to assume the class is over and we're on a statement in
                // the outer module.  We just want to consume and move on.
                !(self.token == Kind::SemicolonToken && in_error_recovery)
                    && self.is_start_of_statement()
            }
            PC_SWITCH_CLAUSES => {
                self.token == Kind::CaseKeyword || self.token == Kind::DefaultKeyword
            }
            PC_TYPE_MEMBERS => self.look_ahead(|p| p.scan_type_member_start()),
            PC_CLASS_MEMBERS => {
                // We allow semicolons as class elements (as specified by ES6) as long
                // as we're not in error recovery.  If we're in error recovery, we
                // don't want an errant semicolon to be treated as a class member (since
                // they're almost always used for statements.
                self.look_ahead(|p| p.scan_class_member_start())
                    || self.token == Kind::SemicolonToken && !in_error_recovery
            }
            PC_ENUM_MEMBERS => {
                // Include open bracket computed properties. This technically also
                // lets in indexers, which would be a candidate for improved error
                // reporting.
                self.token == Kind::OpenBracketToken || self.is_literal_property_name()
            }
            PC_OBJECT_LITERAL_MEMBERS => match self.token {
                // Not an object literal member, but don't want to close the object
                // (see `tests/cases/fourslash/completionsDotInObjectLiteral.ts`)
                Kind::OpenBracketToken
                | Kind::AsteriskToken
                | Kind::DotDotDotToken
                | Kind::DotToken => true,
                _ => self.is_literal_property_name(),
            },
            PC_REST_PROPERTIES => self.is_literal_property_name(),
            PC_OBJECT_BINDING_ELEMENTS => {
                self.token == Kind::OpenBracketToken
                    || self.token == Kind::DotDotDotToken
                    || self.is_literal_property_name()
            }
            PC_IMPORT_ATTRIBUTES => self.is_import_attribute_name(),
            PC_HERITAGE_CLAUSE_ELEMENT => {
                // If we see `{ ... }` then only consume it as an expression if it is
                // followed by `,` or `{`. That way we won't consume the body of a
                // class in its heritage clause.
                if self.token == Kind::OpenBraceToken {
                    return self.is_valid_heritage_clause_object_literal();
                }
                if !in_error_recovery {
                    return self.is_start_of_left_hand_side_expression()
                        && !self.is_heritage_clause_extends_or_implements_keyword();
                }
                // If we're in error recovery we tighten up what we're willing to match.
                // That way we don't treat something like "this" as a valid heritage
                // clause element during recovery.
                self.is_identifier() && !self.is_heritage_clause_extends_or_implements_keyword()
            }
            PC_VARIABLE_DECLARATIONS => {
                self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            PC_ARRAY_BINDING_ELEMENTS => {
                self.token == Kind::CommaToken
                    || self.token == Kind::DotDotDotToken
                    || self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            PC_TYPE_PARAMETERS => {
                self.token == Kind::InKeyword
                    || self.token == Kind::ConstKeyword
                    || self.is_identifier()
            }
            PC_ARRAY_LITERAL_MEMBERS | PC_ARGUMENT_EXPRESSIONS => {
                // Not an array literal member, but don't want to close the array (see
                // `tests/cases/fourslash/completionsDotInArrayLiteralInObjectLiteral.ts`)
                if parsing_context == PC_ARRAY_LITERAL_MEMBERS
                    && (self.token == Kind::CommaToken || self.token == Kind::DotToken)
                {
                    return true;
                }
                self.token == Kind::DotDotDotToken || self.is_start_of_expression()
            }
            PC_PARAMETERS => self.is_start_of_parameter(false /*isJSDocParameter*/),
            PC_JSDOC_PARAMETERS => self.is_start_of_parameter(true /*isJSDocParameter*/),
            PC_TYPE_ARGUMENTS | PC_TUPLE_ELEMENT_TYPES => {
                self.token == Kind::CommaToken
                    || self.is_start_of_type(false /*inStartOfParameter*/)
            }
            PC_HERITAGE_CLAUSES => self.is_heritage_clause(),
            PC_IMPORT_OR_EXPORT_SPECIFIERS => {
                // bail out if the next token is [FromKeyword StringLiteral].
                // That means we're in something like `import { from "mod"`. Stop
                // here can give better error message.
                if self.token == Kind::FromKeyword
                    && self.look_ahead(|p| p.next_token_is_token_string_literal())
                {
                    return false;
                }
                if self.token == Kind::StringLiteral {
                    return true; // For "arbitrary module namespace identifiers"
                }
                token_is_identifier_or_keyword(self.token)
            }
            PC_JSX_ATTRIBUTES => {
                token_is_identifier_or_keyword(self.token) || self.token == Kind::OpenBraceToken
            }
            PC_JSX_CHILDREN | PC_JSDOC_COMMENT => true,
            _ => panic!("Unhandled case in isListElement"),
        }
    }

    /// Go: `func (p *Parser) isListTerminator(kind ParsingContext) bool`.
    fn is_list_terminator(&mut self, kind: ParsingContext) -> bool {
        if self.token == Kind::EndOfFile {
            return true;
        }
        match kind {
            PC_BLOCK_STATEMENTS
            | PC_SWITCH_CLAUSES
            | PC_TYPE_MEMBERS
            | PC_CLASS_MEMBERS
            | PC_ENUM_MEMBERS
            | PC_OBJECT_LITERAL_MEMBERS
            | PC_OBJECT_BINDING_ELEMENTS
            | PC_IMPORT_OR_EXPORT_SPECIFIERS
            | PC_IMPORT_ATTRIBUTES => self.token == Kind::CloseBraceToken,
            PC_SWITCH_CLAUSE_STATEMENTS => {
                self.token == Kind::CloseBraceToken
                    || self.token == Kind::CaseKeyword
                    || self.token == Kind::DefaultKeyword
            }
            PC_HERITAGE_CLAUSE_ELEMENT => {
                self.token == Kind::OpenBraceToken
                    || self.token == Kind::ExtendsKeyword
                    || self.token == Kind::ImplementsKeyword
            }
            PC_VARIABLE_DECLARATIONS => {
                // If we can consume a semicolon (either explicitly, or with ASI), then
                // consider us done with parsing the list of variable declarators.
                // In the case where we're parsing the variable declarator of a 'for-in'
                // statement, we are done if we see an 'in' keyword in front of us. Same
                // with for-of
                // ERROR RECOVERY TWEAK:
                // For better error recovery, if we see an '=>' then we just stop
                // immediately.  We've got an arrow function here and it's going to be
                // very unlikely that we'll resynchronize and get another variable
                // declaration.
                self.can_parse_semicolon()
                    || self.token == Kind::InKeyword
                    || self.token == Kind::OfKeyword
                    || self.token == Kind::EqualsGreaterThanToken
            }
            PC_TYPE_PARAMETERS => {
                // Tokens other than '>' are here for better error recovery
                self.token == Kind::GreaterThanToken
                    || self.token == Kind::OpenParenToken
                    || self.token == Kind::OpenBraceToken
                    || self.token == Kind::ExtendsKeyword
                    || self.token == Kind::ImplementsKeyword
            }
            PC_ARGUMENT_EXPRESSIONS => {
                // Tokens other than ')' are here for better error recovery
                self.token == Kind::CloseParenToken || self.token == Kind::SemicolonToken
            }
            PC_ARRAY_LITERAL_MEMBERS | PC_TUPLE_ELEMENT_TYPES | PC_ARRAY_BINDING_ELEMENTS => {
                self.token == Kind::CloseBracketToken
            }
            PC_JSDOC_PARAMETERS | PC_PARAMETERS | PC_REST_PROPERTIES => {
                // Tokens other than ')' and ']' (the latter for index signatures) are
                // here for better error recovery
                self.token == Kind::CloseParenToken || self.token == Kind::CloseBracketToken
            }
            PC_TYPE_ARGUMENTS => {
                // All other tokens should cause the type-argument to terminate except comma token
                self.token != Kind::CommaToken
            }
            PC_HERITAGE_CLAUSES => {
                self.token == Kind::OpenBraceToken || self.token == Kind::CloseBraceToken
            }
            PC_JSX_ATTRIBUTES => {
                self.token == Kind::GreaterThanToken || self.token == Kind::SlashToken
            }
            PC_JSX_CHILDREN => {
                self.token == Kind::LessThanToken && self.look_ahead(|p| p.next_token_is_slash())
            }
            _ => false,
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Expected-token helpers
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseExpectedJSDoc(kind ast.Kind) bool`.
    pub(crate) fn parse_expected_jsdoc(&mut self, kind: Kind) -> bool {
        if self.token == kind {
            self.next_token_jsdoc();
            return true;
        }
        if !is_keyword_or_punctuation(kind) {
            panic!("Invalid JSDoc kind: expected keyword or punctuation");
        }
        self.parse_error_at_current_token(
            &tsc_diagnostics::X_0_EXPECTED,
            &[token_to_text(kind).to_string()],
        );
        false
    }

    /// Go: `func (p *Parser) parseExpectedMatchingBrackets(...)`.
    pub(crate) fn parse_expected_matching_brackets(
        &mut self,
        open_kind: Kind,
        close_kind: Kind,
        open_parsed: bool,
        open_position: i32,
    ) {
        if self.token == close_kind {
            self.next_token();
            return;
        }
        let last_error = self.parse_error_at_current_token(
            &tsc_diagnostics::X_0_EXPECTED,
            &[token_to_text(close_kind).to_string()],
        );
        if !open_parsed {
            return;
        }
        if let Some(last_error) = last_error {
            let related = new_diagnostic(
                TextRange::new(open_position, open_position),
                &tsc_diagnostics::THE_PARSER_EXPECTED_TO_FIND_A_1_TO_MATCH_THE_0_TOKEN_HERE,
                &[
                    token_to_text(open_kind).to_string(),
                    token_to_text(close_kind).to_string(),
                ],
            );
            self.add_related_info(last_error, related);
        }
    }

    /// Go: `func (p *Parser) parseOptional(token ast.Kind) bool`.
    pub(crate) fn parse_optional(&mut self, token: Kind) -> bool {
        if self.token == token {
            self.next_token();
            return true;
        }
        false
    }

    /// Go: `func (p *Parser) parseExpected(kind ast.Kind) bool`.
    pub(crate) fn parse_expected(&mut self, kind: Kind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, true)
    }

    /// Go: `func (p *Parser) parseExpectedWithoutAdvancing(kind ast.Kind) bool`.
    pub(crate) fn parse_expected_without_advancing(&mut self, kind: Kind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, false)
    }

    /// Go: `func (p *Parser) parseExpectedWithDiagnostic(kind, message, shouldAdvance) bool`.
    pub(crate) fn parse_expected_with_diagnostic(
        &mut self,
        kind: Kind,
        message: Option<&'static Message>,
        should_advance: bool,
    ) -> bool {
        if self.token == kind {
            if should_advance {
                self.next_token();
            }
            return true;
        }
        // Report specific message if provided with one. Otherwise, report generic fallback message.
        match message {
            Some(message) => {
                self.parse_error_at_current_token(message, &[]);
            }
            None => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_0_EXPECTED,
                    &[token_to_text(kind).to_string()],
                );
            }
        }
        false
    }

    /// Go: `func (p *Parser) parseTokenNode() *ast.Node`.
    pub(crate) fn parse_token_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        {
            let __hoist_1_33 = self.factory.new_token(kind);
            let __hoist_1_34 = pos;
            self.finish_node(__hoist_1_33, __hoist_1_34)
        }
    }

    /// Go: `func (p *Parser) parseExpectedToken(kind ast.Kind) *ast.Node`.
    pub(crate) fn parse_expected_token(&mut self, kind: Kind) -> NodeId {
        let token = self.parse_optional_token(kind);
        match token {
            Some(token) => token,
            None => {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_0_EXPECTED,
                    &[token_to_text(kind).to_string()],
                );
                let pos = self.node_pos();
                {
                    let __hoist_1_36 = self.factory.new_token(kind);
                    let __hoist_1_37 = pos;
                    self.finish_node(__hoist_1_36, __hoist_1_37)
                }
            }
        }
    }

    /// Go: `func (p *Parser) parseOptionalToken(kind ast.Kind) *ast.Node`.
    pub(crate) fn parse_optional_token(&mut self, kind: Kind) -> Option<NodeId> {
        if self.token == kind {
            Some(self.parse_token_node())
        } else {
            None
        }
    }

    /// Go: `func (p *Parser) parseExpectedTokenJSDoc(kind ast.Kind) *ast.Node`.
    pub(crate) fn parse_expected_token_jsdoc(&mut self, kind: Kind) -> NodeId {
        match self.parse_optional_token_jsdoc(kind) {
            Some(optional) => optional,
            None => {
                if !is_keyword_or_punctuation(kind) {
                    panic!("expected keyword or punctuation");
                }
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_0_EXPECTED,
                    &[token_to_text(kind).to_string()],
                );
                let pos = self.node_pos();
                {
                    let __hoist_1_39 = self.factory.new_token(kind);
                    let __hoist_1_40 = pos;
                    self.finish_node(__hoist_1_39, __hoist_1_40)
                }
            }
        }
    }

    /// Go: `func (p *Parser) parseOptionalTokenJSDoc(kind ast.Kind) *ast.Node`.
    pub(crate) fn parse_optional_token_jsdoc(&mut self, kind: Kind) -> Option<NodeId> {
        if self.token == kind {
            Some(self.parse_token_node())
        } else {
            None
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Node finishing
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) finishNode(node *ast.Node, pos int) *ast.Node`.
    pub(crate) fn finish_node(&mut self, node: NodeId, pos: TextPos) -> NodeId {
        let end = self.node_pos();
        self.finish_node_with_end(node, pos, end)
    }

    /// Go: `func (p *Parser) finishNodeWithEnd(node *ast.Node, pos, end int) *ast.Node`.
    pub(crate) fn finish_node_with_end(
        &mut self,
        node: NodeId,
        pos: TextPos,
        end: TextPos,
    ) -> NodeId {
        let (context_flags, has_parse_error) = {
            let sink = self.sink.borrow();
            (self.context_flags, sink.has_parse_error)
        };
        let mut cleared_error = false;
        {
            let s = self.factory.store();
            let n = s.node_mut(node);
            n.loc = TextRange::new(pos, end);
            n.flags |= context_flags;
            if has_parse_error {
                n.flags |= NodeFlags::THIS_NODE_HAS_ERROR;
                cleared_error = true;
            }
        }
        if cleared_error {
            self.sink.borrow_mut().has_parse_error = false;
        }
        self.override_parent_in_immediate_children(node);
        node
    }

    /// Go: `func (p *Parser) overrideParentInImmediateChildren(node *ast.Node)`.
    fn override_parent_in_immediate_children(&mut self, node: NodeId) {
        let mut children: Vec<NodeId> = Vec::new();
        {
            let s = self.factory.store();
            s.node(node).for_each_child(&mut |child| {
                // PORT: generated `for_each_child` visits non-Option NodeId
                // fields unconditionally; where Go stored nil there (e.g.
                // PropertyAssignment.Type), the handle is `NodeId::NONE`.
                // Go's `Visit` skips nil children; the port skips NONE the
                // same way (resolving NONE would hit the slot-0 placeholder).
                if child != NodeId::NONE {
                    children.push(child);
                }
                false
            });
        }
        {
            let s = self.factory.store();
            for child in children {
                s.node_mut(child).parent.set(node);
            }
        }
    }

    /// Go: `func (p *Parser) newNodeList(loc core.TextRange, nodes []*ast.Node) *ast.NodeList`.
    pub(crate) fn new_node_list(&mut self, loc: TextRange, nodes: Vec<NodeId>) -> NodeList {
        NodeList {
            loc,
            nodes: nodes.into_boxed_slice(),
        }
    }

    /// Go: `func (p *Parser) newModifierList(loc core.TextRange, nodes []*ast.Node) *ast.ModifierList`.
    pub(crate) fn new_modifier_list(&mut self, loc: TextRange, nodes: Vec<NodeId>) -> ModifierList {
        let mut list = self.factory.new_modifier_list(nodes.into_boxed_slice());
        list.loc = loc;
        list
    }

    /// Go: `func (p *Parser) newIdentifier(text string) *ast.Node`.
    pub(crate) fn new_identifier(&mut self, text: &str) -> NodeId {
        self.identifier_count += 1;
        if text == "await" {
            self.statement_has_await_identifier = true;
        }
        self.factory.new_identifier(text)
    }

    /// Go: `func (p *Parser) createMissingIdentifier() *ast.Node`.
    pub(crate) fn create_missing_identifier(&mut self) -> NodeId {
        let id = self.new_identifier("");
        let pos = self.node_pos();
        self.finish_node(id, pos)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Contexts
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) setContextFlags(flags ast.NodeFlags, value bool)`.
    pub(crate) fn set_context_flags(&mut self, flags: NodeFlags, value: bool) {
        if value {
            self.context_flags |= flags;
        } else {
            self.context_flags &= !flags;
        }
    }

    /// Go: `func (p *Parser) doInContext[T any](flags ast.NodeFlags, value bool, f func(p *Parser) T) T`.
    pub(crate) fn do_in_context<T>(
        &mut self,
        flags: NodeFlags,
        value: bool,
        f: impl FnOnce(&mut Parser<'a, 'f>) -> T,
    ) -> T {
        let save_context_flags = self.context_flags;
        self.set_context_flags(flags, value);
        let result = f(self);
        self.context_flags = save_context_flags;
        result
    }

    pub(crate) fn in_yield_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::YIELD_CONTEXT)
    }

    pub(crate) fn in_disallow_in_context(&self) -> bool {
        self.context_flags
            .intersects(NodeFlags::DISALLOW_IN_CONTEXT)
    }

    pub(crate) fn in_disallow_conditional_types_context(&self) -> bool {
        self.context_flags
            .intersects(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT)
    }

    pub(crate) fn in_decorator_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::DECORATOR_CONTEXT)
    }

    pub(crate) fn in_await_context(&self) -> bool {
        self.context_flags.intersects(NodeFlags::AWAIT_CONTEXT)
    }

    /// Go: `func (p *Parser) skipRangeTrivia(textRange core.TextRange) core.TextRange`.
    pub(crate) fn skip_range_trivia(&self, text_range: TextRange) -> TextRange {
        TextRange::new(
            tsc_scanner::skip_trivia(self.source_text, text_range.pos()),
            text_range.end(),
        )
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Package-level helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isReservedWord(token ast.Kind) bool` — ignore strict mode flag
/// because we will report an error in type checker instead.
pub(crate) fn is_reserved_word(token: Kind) -> bool {
    Kind::FirstReservedWord <= token && token <= Kind::LastReservedWord
}

/// Go: `func attachFileToDiagnostics(diagnostics []*ast.Diagnostic, file *ast.SourceFile) []*ast.Diagnostic`.
pub(crate) fn attach_file_to_diagnostics(
    mut diagnostics: Vec<Diagnostic>,
    file: NodeId,
) -> Vec<Diagnostic> {
    for d in &mut diagnostics {
        d.set_file(Some(file));
        // PORT: `related_information` has no mutable accessor — round-trip
        // through `related_information()` + `set_related_info` (Go mutates
        // the shared `[]*Diagnostic` in place).
        if !d.related_information().is_empty() {
            let mut related = Vec::with_capacity(d.related_information().len());
            for r in d.related_information() {
                let mut r = r.clone();
                r.set_file(Some(file));
                related.push(r);
            }
            d.set_related_info(related);
        }
    }
    diagnostics
}

/// Go `node.Name()` restricted to the kinds the parser reads it for
/// (PORT: the wholesale ast accessor set lands with the utilities.go port;
/// this is the parser's minimal subset, dedup candidate).
pub(crate) fn node_name_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        Kind::InterfaceDeclaration => Some(
            n.as_interface_declaration()
                .expect("InterfaceDeclaration data")
                .name,
        ),
        Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => Some(
            n.as_type_alias_declaration()
                .expect("TypeAliasDeclaration data")
                .name,
        ),
        Kind::EnumDeclaration => Some(n.as_enum_declaration().expect("EnumDeclaration data").name),
        Kind::ModuleDeclaration => Some(
            n.as_module_declaration()
                .expect("ModuleDeclaration data")
                .name,
        ),
        Kind::NamespaceExportDeclaration => Some(
            n.as_namespace_export_declaration()
                .expect("NamespaceExportDeclaration data")
                .name,
        ),
        Kind::PropertyAssignment => Some(
            n.as_property_assignment()
                .expect("PropertyAssignment data")
                .name,
        ),
        Kind::ShorthandPropertyAssignment => Some(
            n.as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment data")
                .name,
        ),
        _ => None,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// parser.go part 2 — statements, declarations, members, imports/exports
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isDeclareModifier(modifier *ast.Node) bool`.
fn is_declare_modifier(modifier: &tsc_ast::Node) -> bool {
    modifier.kind == Kind::DeclareKeyword
}

/// Go: `func isExportModifier(modifier *ast.Node) bool`.
fn is_export_modifier(modifier: &tsc_ast::Node) -> bool {
    modifier.kind == Kind::ExportKeyword
}

/// Go: `func isAsyncModifier(modifier *ast.Node) bool`.
fn is_async_modifier(modifier: &tsc_ast::Node) -> bool {
    modifier.kind == Kind::AsyncKeyword
}

impl<'a, 'f> Parser<'a, 'f> {
    /// Go: `func (p *Parser) parseStatement() *ast.Statement`.
    pub(crate) fn parse_statement(&mut self) -> NodeId {
        match self.token {
            Kind::SemicolonToken => self.parse_empty_statement(),
            Kind::OpenBraceToken => self.parse_block(false /*ignoreMissingOpenBrace*/, None),
            Kind::VarKeyword => {
                let pos = self.node_pos();
                let jsdoc = self.jsdoc_scanner_info();
                self.parse_variable_statement(pos, jsdoc, None)
            }
            Kind::LetKeyword => {
                if self.is_let_declaration() {
                    let pos = self.node_pos();
                    let jsdoc = self.jsdoc_scanner_info();
                    self.parse_variable_statement(pos, jsdoc, None)
                } else {
                    self.parse_expression_or_labeled_statement()
                }
            }
            Kind::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    let pos = self.node_pos();
                    let jsdoc = self.jsdoc_scanner_info();
                    self.parse_variable_statement(pos, jsdoc, None)
                } else {
                    self.parse_expression_or_labeled_statement()
                }
            }
            Kind::UsingKeyword => {
                if self.is_using_declaration() {
                    let pos = self.node_pos();
                    let jsdoc = self.jsdoc_scanner_info();
                    self.parse_variable_statement(pos, jsdoc, None)
                } else {
                    self.parse_expression_or_labeled_statement()
                }
            }
            Kind::FunctionKeyword => {
                let pos = self.node_pos();
                let jsdoc = self.jsdoc_scanner_info();
                self.parse_function_declaration(pos, jsdoc, None)
            }
            Kind::ClassKeyword => {
                let pos = self.node_pos();
                let jsdoc = self.jsdoc_scanner_info();
                self.parse_class_declaration(pos, jsdoc, None)
            }
            Kind::IfKeyword => self.parse_if_statement(),
            Kind::DoKeyword => self.parse_do_statement(),
            Kind::WhileKeyword => self.parse_while_statement(),
            Kind::ForKeyword => self.parse_for_or_for_in_or_for_of_statement(),
            Kind::ContinueKeyword => self.parse_continue_statement(),
            Kind::BreakKeyword => self.parse_break_statement(),
            Kind::ReturnKeyword => self.parse_return_statement(),
            Kind::WithKeyword => self.parse_with_statement(),
            Kind::SwitchKeyword => self.parse_switch_statement(),
            Kind::ThrowKeyword => self.parse_throw_statement(),
            Kind::TryKeyword | Kind::CatchKeyword | Kind::FinallyKeyword => {
                self.parse_try_statement()
            }
            Kind::DebuggerKeyword => self.parse_debugger_statement(),
            Kind::AtToken => self.parse_declaration(),
            Kind::AsyncKeyword
            | Kind::InterfaceKeyword
            | Kind::TypeKeyword
            | Kind::ModuleKeyword
            | Kind::NamespaceKeyword
            | Kind::DeclareKeyword
            | Kind::ConstKeyword
            | Kind::EnumKeyword
            | Kind::ExportKeyword
            | Kind::ImportKeyword
            | Kind::PrivateKeyword
            | Kind::ProtectedKeyword
            | Kind::PublicKeyword
            | Kind::AbstractKeyword
            | Kind::AccessorKeyword
            | Kind::StaticKeyword
            | Kind::ReadonlyKeyword
            | Kind::GlobalKeyword => {
                if self.is_start_of_declaration() {
                    self.parse_declaration()
                } else {
                    self.parse_expression_or_labeled_statement()
                }
            }
            _ => self.parse_expression_or_labeled_statement(),
        }
    }

    /// Go: `func (p *Parser) parseDeclaration() *ast.Statement`.
    pub(crate) fn parse_declaration(&mut self) -> NodeId {
        // `parseListElement` attempted to get the reused node at this position,
        // but the ambient context flag was not yet set, so the node appeared
        // not reusable in that context.
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ true, /*permitConstAsModifier*/ false,
            /*stopOnStartOfClassStaticBlock*/ false,
        );
        let is_ambient = modifiers.as_ref().is_some_and(|m| {
            m.nodes.iter().any(|&m| {
                let s = self.factory.store();
                is_declare_modifier(s.node(m))
            })
        });
        if is_ambient {
            // !!! incremental parsing
            // node := p.tryReuseAmbientDeclaration(pos)
            // if node {
            // 	return node
            // }
            let modifier_nodes = modifiers
                .as_ref()
                .map(|m| m.nodes.to_vec())
                .unwrap_or_default();
            {
                let s = self.factory.store();
                for m in modifier_nodes {
                    s.node_mut(m).flags |= NodeFlags::AMBIENT;
                }
            }
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::AMBIENT, true);
            let result = self.parse_declaration_worker(pos, jsdoc, modifiers);
            self.context_flags = save_context_flags;
            result
        } else {
            self.parse_declaration_worker(pos, jsdoc, modifiers)
        }
    }

    /// Go: `func (p *Parser) parseDeclarationWorker(pos int, jsdoc jsdocScannerInfo, modifiers *ast.ModifierList) *ast.Statement`.
    pub(crate) fn parse_declaration_worker(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        match self.token {
            Kind::VarKeyword | Kind::LetKeyword | Kind::ConstKeyword | Kind::UsingKeyword => {
                self.parse_variable_statement(pos, jsdoc, modifiers)
            }
            Kind::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    self.parse_variable_statement(pos, jsdoc, modifiers)
                } else {
                    self.parse_declaration_unhandled(pos, jsdoc, modifiers)
                }
            }
            Kind::FunctionKeyword => self.parse_function_declaration(pos, jsdoc, modifiers),
            Kind::ClassKeyword => self.parse_class_declaration(pos, jsdoc, modifiers),
            Kind::InterfaceKeyword => self.parse_interface_declaration(pos, jsdoc, modifiers),
            Kind::TypeKeyword => self.parse_type_alias_declaration(pos, jsdoc, modifiers),
            Kind::EnumKeyword => self.parse_enum_declaration(pos, jsdoc, modifiers),
            Kind::GlobalKeyword | Kind::ModuleKeyword | Kind::NamespaceKeyword => {
                self.parse_module_declaration(pos, jsdoc, modifiers)
            }
            Kind::ImportKeyword => {
                self.parse_import_declaration_or_import_equals_declaration(pos, jsdoc, modifiers)
            }
            Kind::ExportKeyword => {
                self.next_token();
                match self.token {
                    Kind::DefaultKeyword | Kind::EqualsToken => {
                        self.parse_export_assignment(pos, jsdoc, modifiers)
                    }
                    Kind::AsKeyword => {
                        self.parse_namespace_export_declaration(pos, jsdoc, modifiers)
                    }
                    _ => self.parse_export_declaration(pos, jsdoc, modifiers),
                }
            }
            _ => self.parse_declaration_unhandled(pos, jsdoc, modifiers),
        }
    }

    /// The shared tail of Go `parseDeclarationWorker`: either an incomplete
    /// `MissingDeclaration` for recovery, or the "unhandled case" panic.
    fn parse_declaration_unhandled(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        if modifiers.is_some() {
            // We reached this point because we encountered decorators and/or
            // modifiers and assumed a declaration would follow. For recovery
            // and error reporting purposes, return an incomplete declaration.
            let (p, e) = (self.node_pos(), self.node_pos());
            self.parse_error_at(p, e, &tsc_diagnostics::DECLARATION_EXPECTED, &[]);
            let result = {
                let __hoist_1_42 = self.factory.new_missing_declaration(modifiers);
                let __hoist_1_43 = pos;
                self.finish_node(__hoist_1_42, __hoist_1_43)
            };
            self.with_jsdoc(result, jsdoc);
            return result;
        }
        panic!("Unhandled case in parseDeclarationWorker")
    }

    /// Go: `func (p *Parser) parseBlock(ignoreMissingOpenBrace bool, diagnosticMessage *diagnostics.Message) *ast.Node`.
    pub(crate) fn parse_block(
        &mut self,
        ignore_missing_open_brace: bool,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let open_brace_position = self.scanner.token_start() as i32;
        let open_brace_parsed = self.parse_expected_with_diagnostic(
            Kind::OpenBraceToken,
            diagnostic_message,
            true, /*shouldAdvance*/
        );
        let mut multi_line = false;
        if open_brace_parsed || ignore_missing_open_brace {
            multi_line = self.has_preceding_line_break();
            let statements = self.parse_list(PC_BLOCK_STATEMENTS, |p| Some(p.parse_statement()));
            self.parse_expected_matching_brackets(
                Kind::OpenBraceToken,
                Kind::CloseBraceToken,
                open_brace_parsed,
                open_brace_position,
            );
            let result = {
                let __hoist_1_45 = self.factory.new_block(statements, multi_line);
                let __hoist_1_46 = pos;
                self.finish_node(__hoist_1_45, __hoist_1_46)
            };
            self.with_jsdoc(result, jsdoc);
            if self.token == Kind::EqualsToken {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::DECLARATION_OR_STATEMENT_EXPECTED_THIS_FOLLOWS_A_BLOCK_OF_STATEMENTS_SO_IF_YOU_INTENDED_TO_WRITE_A_DESTRUCTURING_ASSIGNMENT_YOU_MIGHT_NEED_TO_WRAP_THE_WHOLE_ASSIGNMENT_IN_PARENTHESES,
                    &[],
                );
                self.next_token();
            }
            return result;
        }
        let missing = self.create_missing_list();
        let result = {
            let __hoist_1_48 = self.factory.new_block(Some(missing), multi_line);
            let __hoist_1_49 = pos;
            self.finish_node(__hoist_1_48, __hoist_1_49)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseEmptyStatement() *ast.Node`.
    fn parse_empty_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::SemicolonToken);
        let result = {
            let __hoist_1_51 = self.factory.new_empty_statement();
            let __hoist_1_52 = pos;
            self.finish_node(__hoist_1_51, __hoist_1_52)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseIfStatement() *ast.Node`.
    fn parse_if_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::IfKeyword);
        let open_paren_position = self.scanner.token_start() as i32;
        let open_paren_parsed = self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            Kind::OpenParenToken,
            Kind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let then_statement = self.parse_statement();
        let else_statement = if self.parse_optional(Kind::ElseKeyword) {
            Some(self.parse_statement())
        } else {
            None
        };
        let result = {
            let __hoist_1_54 =
                self.factory
                    .new_if_statement(expression, then_statement, else_statement);
            let __hoist_1_55 = pos;
            self.finish_node(__hoist_1_54, __hoist_1_55)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseDoStatement() *ast.Node`.
    fn parse_do_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::DoKeyword);
        let statement = self.parse_statement();
        self.parse_expected(Kind::WhileKeyword);
        let open_paren_position = self.scanner.token_start() as i32;
        let open_paren_parsed = self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            Kind::OpenParenToken,
            Kind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        // From: https://mail.mozilla.org/pipermail/es-discuss/2011-August/016188.html
        // 157 min --- All allen at wirfs-brock.com CONF --- "do{;}while(false)false" prohibited in
        // spec but allowed in consensus reality. Approved -- this is the de-facto standard whereby
        //  do;while(0)x will have a semicolon inserted before x.
        self.parse_optional(Kind::SemicolonToken);
        let result = {
            let __hoist_1_57 = self.factory.new_do_statement(statement, expression);
            let __hoist_1_58 = pos;
            self.finish_node(__hoist_1_57, __hoist_1_58)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseWhileStatement() *ast.Node`.
    fn parse_while_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::WhileKeyword);
        let open_paren_position = self.scanner.token_start() as i32;
        let open_paren_parsed = self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            Kind::OpenParenToken,
            Kind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let statement = self.parse_statement();
        let result = {
            let __hoist_1_60 = self.factory.new_while_statement(expression, statement);
            let __hoist_1_61 = pos;
            self.finish_node(__hoist_1_60, __hoist_1_61)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseForOrForInOrForOfStatement() *ast.Node`.
    fn parse_for_or_for_in_or_for_of_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::ForKeyword);
        let await_token = self.parse_optional_token(Kind::AwaitKeyword);
        self.parse_expected(Kind::OpenParenToken);
        let mut initializer = NodeId::NONE;
        if self.token != Kind::SemicolonToken {
            let is_var_decl = self.token == Kind::VarKeyword
                || self.token == Kind::LetKeyword
                || self.token == Kind::ConstKeyword;
            let is_using = self.token == Kind::UsingKeyword
                && self.look_ahead(|p| {
                    p.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of()
                });
            let is_await_using = self.token == Kind::AwaitKeyword
                && self.look_ahead(|p| {
                    p.next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line()
                });
            if is_var_decl || is_using || is_await_using {
                initializer =
                    self.parse_variable_declaration_list(true /*inForStatementInitializer*/);
            } else {
                initializer = self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, true, |p| {
                    p.parse_expression()
                });
            }
        }

        let for_of = (await_token.is_some() && self.parse_expected(Kind::OfKeyword))
            || (await_token.is_none() && self.parse_optional(Kind::OfKeyword));
        let result: NodeId = if for_of {
            let expression = self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, false, |p| {
                p.parse_assignment_expression_or_higher()
            });
            self.parse_expected(Kind::CloseParenToken);
            let statement = self.parse_statement();
            self.factory.new_for_in_or_of_statement(
                Kind::ForOfStatement,
                await_token,
                initializer,
                expression,
                statement,
            )
        } else if self.parse_optional(Kind::InKeyword) {
            let expression = self.parse_expression_allow_in();
            self.parse_expected(Kind::CloseParenToken);
            let statement = self.parse_statement();
            self.factory.new_for_in_or_of_statement(
                Kind::ForInStatement,
                None, /*awaitToken*/
                initializer,
                expression,
                statement,
            )
        } else {
            self.parse_expected(Kind::SemicolonToken);
            let mut condition = NodeId::NONE;
            if self.token != Kind::SemicolonToken && self.token != Kind::CloseParenToken {
                condition = self.parse_expression_allow_in();
            }
            self.parse_expected(Kind::SemicolonToken);
            let mut incrementor = NodeId::NONE;
            if self.token != Kind::CloseParenToken {
                incrementor = self.parse_expression_allow_in();
            }
            self.parse_expected(Kind::CloseParenToken);
            let statement = self.parse_statement();
            self.factory.new_for_statement(
                (initializer != NodeId::NONE).then_some(initializer),
                (condition != NodeId::NONE).then_some(condition),
                (incrementor != NodeId::NONE).then_some(incrementor),
                statement,
            )
        };
        let result = self.finish_node(result, pos);
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseBreakStatement() *ast.Node`.
    fn parse_break_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::BreakKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let result = {
            let __hoist_1_63 = self.factory.new_break_statement(label);
            let __hoist_1_64 = pos;
            self.finish_node(__hoist_1_63, __hoist_1_64)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseContinueStatement() *ast.Node`.
    fn parse_continue_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::ContinueKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let result = {
            let __hoist_1_66 = self.factory.new_continue_statement(label);
            let __hoist_1_67 = pos;
            self.finish_node(__hoist_1_66, __hoist_1_67)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseIdentifierUnlessAtSemicolon() *ast.Node`.
    fn parse_identifier_unless_at_semicolon(&mut self) -> Option<NodeId> {
        if !self.can_parse_semicolon() {
            Some(self.parse_identifier())
        } else {
            None
        }
    }

    /// Go: `func (p *Parser) parseReturnStatement() *ast.Node`.
    fn parse_return_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::ReturnKeyword);
        let mut expression = NodeId::NONE;
        if !self.can_parse_semicolon() {
            expression = self.parse_expression_allow_in();
        }
        self.parse_semicolon();
        let result = {
            let __hoist_1_69 = self
                .factory
                .new_return_statement((expression != NodeId::NONE).then_some(expression));
            let __hoist_1_70 = pos;
            self.finish_node(__hoist_1_69, __hoist_1_70)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseWithStatement() *ast.Node`.
    fn parse_with_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::WithKeyword);
        let open_paren_position = self.scanner.token_start() as i32;
        let open_paren_parsed = self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            Kind::OpenParenToken,
            Kind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let statement =
            self.do_in_context(NodeFlags::IN_WITH_STATEMENT, true, |p| p.parse_statement());
        let result = {
            let __hoist_1_72 = self.factory.new_with_statement(expression, statement);
            let __hoist_1_73 = pos;
            self.finish_node(__hoist_1_72, __hoist_1_73)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseCaseClause() *ast.Node`.
    fn parse_case_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::CaseKeyword);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(Kind::ColonToken);
        let statements =
            self.parse_list(PC_SWITCH_CLAUSE_STATEMENTS, |p| Some(p.parse_statement()));
        let result = {
            let __hoist_1_75 =
                self.factory
                    .new_case_or_default_clause(Kind::CaseClause, expression, statements);
            let __hoist_1_76 = pos;
            self.finish_node(__hoist_1_75, __hoist_1_76)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseDefaultClause() *ast.Node`.
    fn parse_default_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::DefaultKeyword);
        self.parse_expected(Kind::ColonToken);
        let statements =
            self.parse_list(PC_SWITCH_CLAUSE_STATEMENTS, |p| Some(p.parse_statement()));
        let result = {
            let __hoist_1_78 = self.factory.new_case_or_default_clause(
                Kind::DefaultClause,
                NodeId::NONE,
                statements,
            );
            let __hoist_1_79 = pos;
            self.finish_node(__hoist_1_78, __hoist_1_79)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseCaseOrDefaultClause() *ast.Node`.
    fn parse_case_or_default_clause(&mut self) -> Option<NodeId> {
        if self.token == Kind::CaseKeyword {
            Some(self.parse_case_clause())
        } else {
            Some(self.parse_default_clause())
        }
    }

    /// Go: `func (p *Parser) parseCaseBlock() *ast.Node`.
    fn parse_case_block(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::OpenBraceToken);
        let clauses = self.parse_list(PC_SWITCH_CLAUSES, |p| p.parse_case_or_default_clause());
        self.parse_expected(Kind::CloseBraceToken);
        let result = {
            let __hoist_1_81 = self.factory.new_case_block(clauses);
            let __hoist_1_82 = pos;
            self.finish_node(__hoist_1_81, __hoist_1_82)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseSwitchStatement() *ast.Node`.
    fn parse_switch_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::SwitchKeyword);
        self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(Kind::CloseParenToken);
        let case_block = self.parse_case_block();
        let result = {
            let __hoist_1_84 = self.factory.new_switch_statement(expression, case_block);
            let __hoist_1_85 = pos;
            self.finish_node(__hoist_1_84, __hoist_1_85)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseThrowStatement() *ast.Node`.
    fn parse_throw_statement(&mut self) -> NodeId {
        // ThrowStatement[Yield] :
        //      throw [no LineTerminator here]Expression[In, ?Yield];
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::ThrowKeyword);
        // Because of automatic semicolon insertion, we need to report error if this
        // throw could be terminated with a semicolon.  Note: we can't call 'parseExpression'
        // directly as that might consume an expression on the following line.
        // Instead, we create a "missing" identifier, but don't report an error. The actual error
        // will be reported in the grammar walker.
        let expression: NodeId = if !self.has_preceding_line_break() {
            self.parse_expression_allow_in()
        } else {
            self.create_missing_identifier()
        };
        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let result = {
            let __hoist_1_87 = self.factory.new_throw_statement(expression);
            let __hoist_1_88 = pos;
            self.finish_node(__hoist_1_87, __hoist_1_88)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseTryStatement() *ast.Node` (TODO: Review for error recovery).
    fn parse_try_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::TryKeyword);
        let try_block = self.parse_block(false /*ignoreMissingOpenBrace*/, None);
        let mut catch_clause = None;
        if self.token == Kind::CatchKeyword {
            catch_clause = Some(self.parse_catch_clause());
        }
        // If we don't have a catch clause, then we must have a finally clause.  Try to parse
        // one out no matter what.
        let mut finally_block = None;
        if catch_clause.is_none() || self.token == Kind::FinallyKeyword {
            self.parse_expected_with_diagnostic(
                Kind::FinallyKeyword,
                Some(&tsc_diagnostics::X_CATCH_OR_FINALLY_EXPECTED),
                true, /*shouldAdvance*/
            );
            finally_block = Some(self.parse_block(false /*ignoreMissingOpenBrace*/, None));
        }
        let result = {
            let __hoist_1_90 =
                self.factory
                    .new_try_statement(try_block, catch_clause, finally_block);
            let __hoist_1_91 = pos;
            self.finish_node(__hoist_1_90, __hoist_1_91)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseCatchClause() *ast.Node`.
    fn parse_catch_clause(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::CatchKeyword);
        let mut variable_declaration = None;
        if self.parse_optional(Kind::OpenParenToken) {
            variable_declaration = self.parse_variable_declaration();
            self.parse_expected(Kind::CloseParenToken);
        }
        let block = self.parse_block(false /*ignoreMissingOpenBrace*/, None);
        {
            let __hoist_1_93 = self.factory.new_catch_clause(variable_declaration, block);
            let __hoist_1_94 = pos;
            self.finish_node(__hoist_1_93, __hoist_1_94)
        }
    }

    /// Go: `func (p *Parser) parseDebuggerStatement() *ast.Node`.
    fn parse_debugger_statement(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::DebuggerKeyword);
        self.parse_semicolon();
        let result = {
            let __hoist_1_96 = self.factory.new_debugger_statement();
            let __hoist_1_97 = pos;
            self.finish_node(__hoist_1_96, __hoist_1_97)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseExpressionOrLabeledStatement() *ast.Statement`.
    fn parse_expression_or_labeled_statement(&mut self) -> NodeId {
        // Avoiding having to do the lookahead for a labeled statement by just
        // trying to parse out an expression, seeing if it is identifier and
        // then seeing if it is followed by a colon.
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let has_paren = self.token == Kind::OpenParenToken;
        let expression = self.parse_expression();
        let is_identifier = {
            let s = self.factory.store();
            s.node(expression).kind == Kind::Identifier
        };
        if is_identifier && self.parse_optional(Kind::ColonToken) {
            let statement = self.parse_statement();
            let result = {
                let __hoist_1_99 = self.factory.new_labeled_statement(expression, statement);
                let __hoist_1_100 = pos;
                self.finish_node(__hoist_1_99, __hoist_1_100)
            };
            self.with_jsdoc(result, jsdoc);
            return result;
        }
        if !self.try_parse_semicolon() {
            self.parse_error_for_missing_semicolon_after(expression);
        }
        let result = {
            let __hoist_1_102 = self.factory.new_expression_statement(expression);
            let __hoist_1_103 = pos;
            self.finish_node(__hoist_1_102, __hoist_1_103)
        };
        let mut jsdoc = jsdoc;
        if has_paren {
            jsdoc &= !JSDOC_SCANNER_INFO_HAS_JSDOC;
        }
        self.with_jsdoc(result, jsdoc);
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Variable statements / binding patterns
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseVariableStatement(pos int, jsdoc, modifiers) *ast.Node`.
    pub(crate) fn parse_variable_statement(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let declaration_list =
            self.parse_variable_declaration_list(false /*inForStatementInitializer*/);
        self.parse_semicolon();
        let result = {
            let __hoist_1_105 = self
                .factory
                .new_variable_statement(modifiers, declaration_list);
            let __hoist_1_106 = pos;
            self.finish_node(__hoist_1_105, __hoist_1_106)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseVariableDeclarationList(inForStatementInitializer bool) *ast.Node`.
    pub(crate) fn parse_variable_declaration_list(
        &mut self,
        in_for_statement_initializer: bool,
    ) -> NodeId {
        let pos = self.node_pos();
        let mut flags = NodeFlags::NONE;
        match self.token {
            Kind::VarKeyword => {}
            Kind::LetKeyword => flags = NodeFlags::LET,
            Kind::ConstKeyword => flags = NodeFlags::CONST,
            Kind::UsingKeyword => flags = NodeFlags::USING,
            Kind::AwaitKeyword => {
                // Go: `break` leaves flags at their zero value and flows to
                // the shared nextToken below (the caller has already verified
                // this is an `await using` declaration in practice).
                if self.is_await_using_declaration() {
                    flags = NodeFlags::AWAIT_USING;
                    self.next_token();
                }
            }
            _ => panic!("Unhandled case in parseVariableDeclarationList"),
        }
        self.next_token();
        // The user may have written the following:
        //
        //    for (let of X) { }
        //
        // In this case, we want to parse an empty declaration list, and then parse 'of'
        // as a keyword. The reason this is not automatic is that 'of' is a valid identifier.
        // So we need to look ahead to determine if 'of' should be treated as a keyword in
        // this context.
        // The checker will then give an error that there is an empty declaration list.
        let declarations: Option<NodeList> = if self.token == Kind::OfKeyword
            && self.look_ahead(|p| p.next_is_identifier_and_close_paren())
        {
            Some(self.create_missing_list())
        } else {
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, in_for_statement_initializer);
            let declarations = {
                let __hoist_1_108 = PC_VARIABLE_DECLARATIONS;
                let __hoist_1_109 = if in_for_statement_initializer {
                    |p: &mut Parser<'a, 'f>| p.parse_variable_declaration()
                } else {
                    |p: &mut Parser<'a, 'f>| p.parse_variable_declaration_allow_exclamation()
                };
                self.parse_delimited_list(__hoist_1_108, __hoist_1_109)
            };
            self.context_flags = save_context_flags;
            declarations
        };
        {
            let __hoist_1_111 = self
                .factory
                .new_variable_declaration_list(declarations, flags);
            let __hoist_1_112 = pos;
            self.finish_node(__hoist_1_111, __hoist_1_112)
        }
    }

    /// Go: `func (p *Parser) nextIsIdentifierAndCloseParen() bool`.
    fn next_is_identifier_and_close_paren(&mut self) -> bool {
        self.next_token_is_identifier() && self.next_token() == Kind::CloseParenToken
    }

    pub(crate) fn next_token_is_identifier(&mut self) -> bool {
        self.next_token();
        self.is_identifier()
    }

    /// Go: `func (p *Parser) nextTokenIsSlash() bool`.
    pub(crate) fn next_token_is_slash(&mut self) -> bool {
        self.next_token() == Kind::SlashToken
    }

    /// Go: `func (p *Parser) parseVariableDeclaration() *ast.Node`.
    pub(crate) fn parse_variable_declaration(&mut self) -> Option<NodeId> {
        Some(self.parse_variable_declaration_worker(false /*allowExclamation*/))
    }

    /// Go: `func (p *Parser) parseVariableDeclarationAllowExclamation() *ast.Node`.
    pub(crate) fn parse_variable_declaration_allow_exclamation(&mut self) -> Option<NodeId> {
        Some(self.parse_variable_declaration_worker(true /*allowExclamation*/))
    }

    /// Go: `func (p *Parser) parseVariableDeclarationWorker(allowExclamation bool) *ast.Node`.
    fn parse_variable_declaration_worker(&mut self, allow_exclamation: bool) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            &tsc_diagnostics::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_IN_VARIABLE_DECLARATIONS,
        ));
        let mut exclamation_token = None;
        let name_is_identifier = {
            let s = self.factory.store();
            s.node(name).kind == Kind::Identifier
        };
        if allow_exclamation
            && name_is_identifier
            && self.token == Kind::ExclamationToken
            && !self.has_preceding_line_break()
        {
            exclamation_token = Some(self.parse_token_node());
        }
        let type_node = self.parse_type_annotation();
        let mut initializer = NodeId::NONE;
        if self.token != Kind::InKeyword && self.token != Kind::OfKeyword {
            initializer = self.parse_initializer();
        }
        let result = {
            let __hoist_1_114 = self.factory.new_variable_declaration(
                name,
                exclamation_token,
                type_node,
                (initializer != NodeId::NONE).then_some(initializer),
            );
            let __hoist_1_115 = pos;
            self.finish_node(__hoist_1_114, __hoist_1_115)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseIdentifierOrPattern() *ast.Node`.
    pub(crate) fn parse_identifier_or_pattern(&mut self) -> NodeId {
        self.parse_identifier_or_pattern_with_diagnostic(None)
    }

    /// Go: `func (p *Parser) parseIdentifierOrPatternWithDiagnostic(privateIdentifierDiagnosticMessage) *ast.Node`.
    pub(crate) fn parse_identifier_or_pattern_with_diagnostic(
        &mut self,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        if self.token == Kind::OpenBracketToken {
            return self.parse_array_binding_pattern();
        }
        if self.token == Kind::OpenBraceToken {
            return self.parse_object_binding_pattern();
        }
        self.parse_binding_identifier_with_diagnostic(private_identifier_diagnostic_message)
    }

    /// Go: `func (p *Parser) parseArrayBindingPattern() *ast.Node`.
    pub(crate) fn parse_array_binding_pattern(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenBracketToken);
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(PC_ARRAY_BINDING_ELEMENTS, |p| {
            p.parse_array_binding_element()
        });
        self.context_flags = save_context_flags;
        self.parse_expected(Kind::CloseBracketToken);
        {
            let __hoist_1_117 = self
                .factory
                .new_binding_pattern(Kind::ArrayBindingPattern, elements);
            let __hoist_1_118 = pos;
            self.finish_node(__hoist_1_117, __hoist_1_118)
        }
    }

    /// Go: `func (p *Parser) parseArrayBindingElement() *ast.Node`.
    fn parse_array_binding_element(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let mut dot_dot_dot_token = None;
        let mut name = NodeId::NONE;
        let mut initializer = NodeId::NONE;
        if self.token != Kind::CommaToken {
            // These are all nil for a missing element
            dot_dot_dot_token = self.parse_optional_token(Kind::DotDotDotToken);
            name = self.parse_identifier_or_pattern();
            initializer = self.parse_initializer();
        }
        Some({
            let __hoist_1_120 = self.factory.new_binding_element(
                dot_dot_dot_token,
                None, /*propertyName*/
                (name != NodeId::NONE).then_some(name),
                (initializer != NodeId::NONE).then_some(initializer),
            );
            let __hoist_1_121 = pos;
            self.finish_node(__hoist_1_120, __hoist_1_121)
        })
    }

    /// Go: `func (p *Parser) parseObjectBindingPattern() *ast.Node`.
    pub(crate) fn parse_object_binding_pattern(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenBraceToken);
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let elements = self.parse_delimited_list(PC_OBJECT_BINDING_ELEMENTS, |p| {
            p.parse_object_binding_element()
        });
        self.context_flags = save_context_flags;
        self.parse_expected(Kind::CloseBraceToken);
        {
            let __hoist_1_123 = self
                .factory
                .new_binding_pattern(Kind::ObjectBindingPattern, elements);
            let __hoist_1_124 = pos;
            self.finish_node(__hoist_1_123, __hoist_1_124)
        }
    }

    /// Go: `func (p *Parser) parseObjectBindingElement() *ast.Node`.
    fn parse_object_binding_element(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let dot_dot_dot_token = self.parse_optional_token(Kind::DotDotDotToken);
        let token_is_identifier = self.is_binding_identifier();
        let parsed_property_name = self.parse_property_name();
        let (property_name, name) = if token_is_identifier && self.token != Kind::ColonToken {
            (None, parsed_property_name)
        } else {
            self.parse_expected(Kind::ColonToken);
            (
                Some(parsed_property_name),
                self.parse_identifier_or_pattern(),
            )
        };
        let initializer = self.parse_initializer();
        Some({
            let __hoist_1_126 = self.factory.new_binding_element(
                dot_dot_dot_token,
                property_name,
                Some(name),
                (initializer != NodeId::NONE).then_some(initializer),
            );
            let __hoist_1_127 = pos;
            self.finish_node(__hoist_1_126, __hoist_1_127)
        })
    }

    /// Go: `func (p *Parser) parseInitializer() *ast.Expression`.
    pub(crate) fn parse_initializer(&mut self) -> NodeId {
        if self.parse_optional(Kind::EqualsToken) {
            return self.parse_assignment_expression_or_higher();
        }
        NodeId::NONE
    }

    /// Go: `func (p *Parser) parseTypeAnnotation() *ast.TypeNode`.
    pub(crate) fn parse_type_annotation(&mut self) -> Option<NodeId> {
        if self.parse_optional(Kind::ColonToken) {
            return Some(self.parse_type());
        }
        None
    }

    // ────────────────────────────────────────────────────────────────────────
    // Functions / classes / members
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseFunctionDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    pub(crate) fn parse_function_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected(Kind::FunctionKeyword);
        let asterisk_token = self.parse_optional_token(Kind::AsteriskToken);
        // We don't parse the name here in await context, instead we will
        // report a grammar error in the checker.
        let has_default = modifiers
            .as_ref()
            .is_some_and(|m| m.modifier_flags.intersects(tsc_ast::ModifierFlags::DEFAULT));
        let mut name = None;
        if !has_default || self.is_binding_identifier() {
            name = Some(self.parse_binding_identifier());
        }
        let signature_flags = if asterisk_token.is_some() {
            PARSE_FLAGS_YIELD
        } else {
            PARSE_FLAGS_NONE
        } | if modifiers
            .as_ref()
            .is_some_and(|m| m.modifier_flags.intersects(tsc_ast::ModifierFlags::ASYNC))
        {
            PARSE_FLAGS_AWAIT
        } else {
            PARSE_FLAGS_NONE
        };
        let type_parameters = self.parse_type_parameters();
        let save_context_flags = self.context_flags;
        if modifiers
            .as_ref()
            .is_some_and(|m| m.modifier_flags.intersects(tsc_ast::ModifierFlags::EXPORT))
        {
            self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        }
        let parameters = self.parse_parameters(signature_flags);
        let return_type = self.parse_return_type(Kind::ColonToken, false /*isType*/);
        let body = self.parse_function_block_or_semicolon(
            signature_flags,
            Some(&tsc_diagnostics::X_OR_EXPECTED),
        );
        self.context_flags = save_context_flags;
        let result = {
            let __hoist_1_129 = self.factory.new_function_declaration(
                modifiers,
                asterisk_token,
                name,
                type_parameters,
                parameters,
                return_type,
                None, /*fullSignature*/
                body,
            );
            let __hoist_1_130 = pos;
            self.finish_node(__hoist_1_129, __hoist_1_130)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseClassDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    pub(crate) fn parse_class_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_class_declaration_or_expression(pos, jsdoc, modifiers, Kind::ClassDeclaration)
    }

    /// Go: `func (p *Parser) parseClassExpression() *ast.Node`.
    pub(crate) fn parse_class_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_class_declaration_or_expression(pos, jsdoc, None, Kind::ClassExpression)
    }

    /// Go: `func (p *Parser) parseClassDeclarationOrExpression(pos int, jsdoc, modifiers, kind) *ast.Node`.
    pub(crate) fn parse_class_declaration_or_expression(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        kind: Kind,
    ) -> NodeId {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.parse_expected(Kind::ClassKeyword);
        // We don't parse the name here in await context, instead we will
        // report a grammar error in the checker.
        let name = self.parse_name_of_class_declaration_or_expression();
        let type_parameters = self.parse_type_parameters();
        let has_export = modifiers.as_ref().is_some_and(|m| {
            m.nodes.iter().any(|&n| {
                let s = self.factory.store();
                is_export_modifier(s.node(n))
            })
        });
        if has_export
            && self.parsing_contexts & (1 << PC_SOURCE_ELEMENTS) != 0
            && self.parsing_contexts
                & ((1 << PC_BLOCK_STATEMENTS) | (1 << PC_SWITCH_CLAUSE_STATEMENTS))
                == 0
        {
            self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true /*value*/);
        }
        let heritage_clauses = self.parse_heritage_clauses(false /*isInterface*/);
        let members: Option<NodeList>;
        if self.parse_expected(Kind::OpenBraceToken) {
            // ClassTail[Yield,Await] : (Modified) See 14.5
            //      ClassHeritage[?Yield,?Await]opt { ClassBody[?Yield,?Await]opt }
            members = self.parse_list(PC_CLASS_MEMBERS, |p| p.parse_class_element());
            self.parse_expected(Kind::CloseBraceToken);
        } else {
            members = Some(self.create_missing_list());
        }
        self.context_flags = save_context_flags;
        let has_ambient = modifiers.as_ref().is_some_and(|m| {
            tsc_ast::modifiers_to_flags(self.factory.store(), &m.nodes)
                .intersects(tsc_ast::ModifierFlags::AMBIENT)
        });
        if has_ambient {
            self.statement_has_await_identifier = save_has_await_identifier;
        }
        let result = if kind == Kind::ClassDeclaration {
            self.factory.new_class_declaration(
                modifiers.clone(),
                name,
                type_parameters.clone(),
                heritage_clauses.clone(),
                members.clone(),
            )
        } else {
            self.factory.new_class_expression(
                modifiers.clone(),
                name,
                type_parameters.clone(),
                heritage_clauses.clone(),
                members.clone(),
            )
        };
        let result = self.finish_node(result, pos);
        self.with_jsdoc(result, jsdoc);
        let is_js = {
            let s = self.factory.store();
            s.node(result).flags.intersects(NodeFlags::JAVASCRIPT_FILE)
        };
        if is_js {
            self.check_js_syntax(result);
            if let Some(clauses) = heritage_clauses.as_ref() {
                for &clause in &clauses.nodes {
                    let (token, types) = {
                        let s = self.factory.store();
                        let d = s
                            .node(clause)
                            .as_heritage_clause()
                            .expect("HeritageClause data");
                        (d.token, d.types.as_ref().map(|l| l.nodes.to_vec()))
                    };
                    if token == Kind::ExtendsKeyword {
                        if let Some(types) = types {
                            for expr in types {
                                self.check_js_syntax(expr);
                            }
                        }
                    }
                }
            }
        }
        result
    }

    /// Go: `func (p *Parser) parseNameOfClassDeclarationOrExpression() *ast.Node`.
    fn parse_name_of_class_declaration_or_expression(&mut self) -> Option<NodeId> {
        // implements is a future reserved word so
        // 'class implements' might mean either
        // - class expression with omitted name, 'implements' starts heritage clause
        // - class with name 'implements'
        // 'isImplementsClause' helps to disambiguate between these two cases
        if self.is_binding_identifier() && !self.is_implements_clause() {
            let save_has_await_identifier = self.statement_has_await_identifier;
            let id = {
                let __hoist_1_132 = self.is_binding_identifier();
                self.create_identifier(__hoist_1_132)
            };
            self.statement_has_await_identifier = save_has_await_identifier;
            return Some(id);
        }
        None
    }

    /// Go: `func (p *Parser) isImplementsClause() bool`.
    fn is_implements_clause(&mut self) -> bool {
        self.token == Kind::ImplementsKeyword
            && self.look_ahead(|p| p.next_token_is_identifier_or_keyword())
    }

    /// Go: `func (p *Parser) parseHeritageClauses(isInterface bool) *ast.NodeList`.
    pub(crate) fn parse_heritage_clauses(&mut self, is_interface: bool) -> Option<NodeList> {
        // ClassTail[Yield,Await] : (Modified) See 14.5
        //      ClassHeritage[?Yield,?Await]opt { ClassBody[?Yield,?Await]opt }
        if self.is_heritage_clause() {
            return self.parse_list(PC_HERITAGE_CLAUSES, move |p| {
                Some(p.parse_heritage_clause(is_interface))
            });
        }
        None
    }

    /// Go: `func (p *Parser) parseHeritageClause(isInterface bool) *ast.Node`.
    fn parse_heritage_clause(&mut self, is_interface: bool) -> NodeId {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        let types = if is_type_heritage_clause(is_interface, kind) {
            self.parse_delimited_list(PC_HERITAGE_CLAUSE_ELEMENT, |p| {
                Some(p.parse_type_heritage_clause_element())
            })
        } else {
            self.parse_delimited_list(PC_HERITAGE_CLAUSE_ELEMENT, |p| {
                Some(p.parse_expression_with_type_arguments())
            })
        };
        let result = {
            let __hoist_1_134 = self.factory.new_heritage_clause(kind, types);
            let __hoist_1_135 = pos;
            self.finish_node(__hoist_1_134, __hoist_1_135)
        };
        self.check_js_syntax(result)
    }

    /// Go: `func (p *Parser) parseTypeHeritageClauseElement() *ast.HeritageClauseElement`.
    fn parse_type_heritage_clause_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let expression_with_type_arguments = self.parse_expression_with_type_arguments();
        let expression = {
            let s = self.factory.store();
            s.node(expression_with_type_arguments)
                .as_expression_with_type_arguments()
                .expect("ExpressionWithTypeArguments data")
                .expression
        };
        if !is_valid_heritage_type_reference_expression(self.factory.store(), expression) {
            return expression_with_type_arguments;
        }
        let type_arguments = {
            let s = self.factory.store();
            s.node(expression_with_type_arguments)
                .as_expression_with_type_arguments()
                .expect("ExpressionWithTypeArguments data")
                .type_arguments
                .clone()
        };
        let type_name = self.convert_entity_name_expression_to_entity_name(expression);
        {
            let __hoist_1_137 = self
                .factory
                .new_type_reference_node(type_name, type_arguments);
            let __hoist_1_138 = pos;
            self.finish_node(__hoist_1_137, __hoist_1_138)
        }
    }

    /// Go: `func (p *Parser) convertEntityNameExpressionToEntityName(node *ast.Node) *ast.Node`.
    fn convert_entity_name_expression_to_entity_name(&mut self, node: NodeId) -> NodeId {
        let is_identifier = {
            let s = self.factory.store();
            tsc_ast::is_identifier(s.node(node))
        };
        if is_identifier {
            return node;
        }
        let (expression, name, pos, end) = {
            let s = self.factory.store();
            let d = s
                .node(node)
                .as_property_access_expression()
                .expect("PropertyAccessExpression data");
            (d.expression, d.name, s.node(node).pos(), s.node(node).end())
        };
        let result = {
            let __hoist_1_140 = self.convert_entity_name_expression_to_entity_name(expression);
            let __hoist_1_141 = name;
            self.factory
                .new_qualified_name(__hoist_1_140, __hoist_1_141)
        };
        self.finish_node_with_end(result, pos, end)
    }

    /// Go: `func (p *Parser) parseExpressionWithTypeArguments() *ast.Node`.
    pub(crate) fn parse_expression_with_type_arguments(&mut self) -> NodeId {
        let pos = self.node_pos();
        let expression = self.parse_left_hand_side_expression_or_higher();
        let is_already = {
            let s = self.factory.store();
            tsc_ast::is_expression_with_type_arguments(s.node(expression))
        };
        if is_already {
            return expression;
        }
        let type_arguments = self.parse_type_arguments();
        {
            let __hoist_1_143 = self
                .factory
                .new_expression_with_type_arguments(expression, type_arguments);
            let __hoist_1_144 = pos;
            self.finish_node(__hoist_1_143, __hoist_1_144)
        }
    }

    /// Go: `func (p *Parser) parseClassElement() *ast.Node`.
    fn parse_class_element(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.token == Kind::SemicolonToken {
            self.next_token();
            let result = {
                let __hoist_1_146 = self.factory.new_semicolon_class_element();
                let __hoist_1_147 = pos;
                self.finish_node(__hoist_1_146, __hoist_1_147)
            };
            self.with_jsdoc(result, jsdoc);
            return Some(result);
        }
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ true, /*permitConstAsModifier*/ true,
            /*stopOnStartOfClassStaticBlock*/ true,
        );
        if self.token == Kind::StaticKeyword && self.look_ahead(|p| p.next_token_is_open_brace()) {
            return Some(self.parse_class_static_block_declaration(pos, jsdoc, modifiers));
        }
        if self.parse_contextual_modifier(Kind::GetKeyword) {
            return Some(self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::GetAccessor,
                PARSE_FLAGS_NONE,
            ));
        }
        if self.parse_contextual_modifier(Kind::SetKeyword) {
            return Some(self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::SetAccessor,
                PARSE_FLAGS_NONE,
            ));
        }
        if self.token == Kind::ConstructorKeyword || self.token == Kind::StringLiteral {
            if let Some(constructor_declaration) =
                self.try_parse_constructor_declaration(pos, jsdoc, modifiers.clone())
            {
                return Some(constructor_declaration);
            }
        }
        if self.is_index_signature() {
            let result = self.parse_index_signature_declaration(pos, jsdoc, modifiers);
            return Some(self.check_js_syntax(result));
        }
        // It is very important that we check this *after* checking indexers because
        // the [ token can start an index signature or a computed property name
        if token_is_identifier_or_keyword(self.token)
            || self.token == Kind::StringLiteral
            || self.token == Kind::NumericLiteral
            || self.token == Kind::BigIntLiteral
            || self.token == Kind::AsteriskToken
            || self.token == Kind::OpenBracketToken
        {
            let is_ambient = modifiers.as_ref().is_some_and(|m| {
                m.nodes.iter().any(|&n| {
                    let s = self.factory.store();
                    is_declare_modifier(s.node(n))
                })
            });
            if is_ambient {
                let modifier_nodes = modifiers
                    .as_ref()
                    .map(|m| m.nodes.to_vec())
                    .unwrap_or_default();
                {
                    let s = self.factory.store();
                    for m in modifier_nodes {
                        s.node_mut(m).flags |= NodeFlags::AMBIENT;
                    }
                }
                let save_context_flags = self.context_flags;
                self.set_context_flags(NodeFlags::AMBIENT, true);
                let result = self.parse_property_or_method_declaration(pos, jsdoc, modifiers);
                self.context_flags = save_context_flags;
                return Some(result);
            } else {
                return Some(self.parse_property_or_method_declaration(pos, jsdoc, modifiers));
            }
        }
        if modifiers.is_some() {
            // treat this as a property declaration with a missing name.
            let (p, e) = (self.node_pos(), self.node_pos());
            self.parse_error_at(p, e, &tsc_diagnostics::DECLARATION_EXPECTED, &[]);
            let name = self.create_missing_identifier();
            return Some(self.parse_property_declaration(
                pos,
                jsdoc,
                modifiers,
                Some(name),
                None, /*questionToken*/
            ));
        }
        // 'isClassMemberStart' should have hinted not to attempt parsing.
        panic!("Should not have attempted to parse class member declaration.")
    }

    /// Go: `func (p *Parser) parseClassStaticBlockDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_class_static_block_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected_token(Kind::StaticKeyword);
        let body = self.parse_class_static_block_body();
        let result = {
            let __hoist_1_149 = self
                .factory
                .new_class_static_block_declaration(modifiers, body);
            let __hoist_1_150 = pos;
            self.finish_node(__hoist_1_149, __hoist_1_150)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseClassStaticBlockBody() *ast.Node`.
    fn parse_class_static_block_body(&mut self) -> NodeId {
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, false);
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let body = self.parse_block(
            false, /*ignoreMissingOpenBrace*/
            None,  /*diagnosticMessage*/
        );
        self.context_flags = save_context_flags;
        body
    }

    /// Go: `func (p *Parser) tryParseConstructorDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn try_parse_constructor_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> Option<NodeId> {
        let state = self.mark();
        let is_constructor = self.token == Kind::ConstructorKeyword
            || (self.token == Kind::StringLiteral
                && self.scanner.token_value() == b"constructor"
                && self.look_ahead(|p| p.next_token_is_open_paren()));
        if is_constructor {
            self.next_token();
            let type_parameters = self.parse_type_parameters();
            let parameters = self.parse_parameters(PARSE_FLAGS_NONE);
            let return_type = self.parse_return_type(Kind::ColonToken, false /*isType*/);
            let body = self.parse_function_block_or_semicolon(
                PARSE_FLAGS_NONE,
                Some(&tsc_diagnostics::X_OR_EXPECTED),
            );
            let result = {
                let __hoist_1_152 = self.factory.new_constructor_declaration(
                    modifiers,
                    type_parameters,
                    parameters,
                    return_type,
                    None, /*fullSignature*/
                    body,
                );
                let __hoist_1_153 = pos;
                self.finish_node(__hoist_1_152, __hoist_1_153)
            };
            self.with_jsdoc(result, jsdoc);
            self.check_js_syntax(result);
            return Some(result);
        }
        self.rewind(state);
        None
    }

    /// Go: `func (p *Parser) nextTokenIsOpenParen() bool`.
    fn next_token_is_open_paren(&mut self) -> bool {
        self.next_token() == Kind::OpenParenToken
    }

    /// Go: `func (p *Parser) parsePropertyOrMethodDeclaration(...) *ast.Node`.
    fn parse_property_or_method_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let asterisk_token = self.parse_optional_token(Kind::AsteriskToken);
        let name = self.parse_property_name();
        // Note: this is not legal as per the grammar.  But we allow it in the
        // parser and report an error in the grammar checker.
        let question_token = self.parse_optional_token(Kind::QuestionToken);
        let has_method_start = asterisk_token.is_some()
            || self.token == Kind::OpenParenToken
            || self.token == Kind::LessThanToken;
        if has_method_start {
            self.parse_method_declaration(
                pos,
                jsdoc,
                modifiers,
                asterisk_token,
                name,
                question_token,
                Some(&tsc_diagnostics::X_OR_EXPECTED),
            )
        } else {
            self.parse_property_declaration(pos, jsdoc, modifiers, Some(name), question_token)
        }
    }

    /// Go: `func (p *Parser) parseMethodDeclaration(...) *ast.Node`.
    ///
    /// PORT: eight params mirror Go's parseMethodDeclaration signature
    /// (parser.go); clippy's 7-arg cap is waived to keep the Go shape.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn parse_method_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        asterisk_token: Option<NodeId>,
        name: NodeId,
        question_token: Option<NodeId>,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        let signature_flags = if asterisk_token.is_some() {
            PARSE_FLAGS_YIELD
        } else {
            PARSE_FLAGS_NONE
        } | if modifier_list_has_async(self, modifiers.as_ref()) {
            PARSE_FLAGS_AWAIT
        } else {
            PARSE_FLAGS_NONE
        };
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(signature_flags);
        let type_node = self.parse_return_type(Kind::ColonToken, false /*isType*/);
        let body = self.parse_function_block_or_semicolon(signature_flags, diagnostic_message);
        let result = {
            let __hoist_1_155 = self.factory.new_method_declaration(
                modifiers,
                asterisk_token,
                name,
                question_token,
                type_parameters,
                parameters,
                type_node,
                None, /*fullSignature*/
                body,
            );
            let __hoist_1_156 = pos;
            self.finish_node(__hoist_1_155, __hoist_1_156)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parsePropertyDeclaration(...) *ast.Node`.
    fn parse_property_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        name: Option<NodeId>,
        question_token: Option<NodeId>,
    ) -> NodeId {
        let mut postfix_token = question_token;
        if postfix_token.is_none() && !self.has_preceding_line_break() {
            postfix_token = self.parse_optional_token(Kind::ExclamationToken);
        }
        let type_node = self.parse_type_annotation();
        let initializer = self.do_in_context(
            NodeFlags::YIELD_CONTEXT
                .union(NodeFlags::AWAIT_CONTEXT)
                .union(NodeFlags::DISALLOW_IN_CONTEXT),
            false,
            |p| p.parse_initializer(),
        );
        let name = name.expect("property declaration name");
        self.parse_semicolon_after_property_name(name, type_node, initializer);
        let result = {
            let __hoist_1_158 = self.factory.new_property_declaration(
                modifiers,
                name,
                postfix_token,
                type_node,
                (initializer != NodeId::NONE).then_some(initializer),
            );
            let __hoist_1_159 = pos;
            self.finish_node(__hoist_1_158, __hoist_1_159)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseSemicolonAfterPropertyName(name *ast.Node, typeNode *ast.TypeNode, initializer *ast.Expression)`.
    fn parse_semicolon_after_property_name(
        &mut self,
        name: NodeId,
        type_node: Option<NodeId>,
        initializer: NodeId,
    ) {
        if self.token == Kind::AtToken && !self.has_preceding_line_break() {
            self.parse_error_at_current_token(
                &tsc_diagnostics::DECORATORS_MUST_PRECEDE_THE_NAME_AND_ALL_KEYWORDS_OF_PROPERTY_DECLARATIONS,
                &[],
            );
            return;
        }
        if self.token == Kind::OpenParenToken {
            self.parse_error_at_current_token(
                &tsc_diagnostics::CANNOT_START_A_FUNCTION_CALL_IN_A_TYPE_ANNOTATION,
                &[],
            );
            self.next_token();
            return;
        }
        if type_node.is_some() && !self.can_parse_semicolon() {
            if initializer != NodeId::NONE {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_0_EXPECTED,
                    &[token_to_text(Kind::SemicolonToken).to_string()],
                );
            } else {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::EXPECTED_FOR_PROPERTY_INITIALIZER,
                    &[],
                );
            }
            return;
        }
        if self.try_parse_semicolon() {
            return;
        }
        if initializer != NodeId::NONE {
            self.parse_error_at_current_token(
                &tsc_diagnostics::X_0_EXPECTED,
                &[token_to_text(Kind::SemicolonToken).to_string()],
            );
            return;
        }
        self.parse_error_for_missing_semicolon_after(name);
    }

    /// Go: `func (p *Parser) parseErrorForMissingSemicolonAfter(node *ast.Node)`.
    fn parse_error_for_missing_semicolon_after(&mut self, node: NodeId) {
        let (kind, pos, end) = {
            let s = self.factory.store();
            let n = s.node(node);
            (n.kind, n.pos(), n.end())
        };
        // Tagged template literals are sometimes used in places where only
        // simple strings are allowed, i.e.:
        //   module `M1` {
        //   ^^^^^^^^^^^ This block is parsed as a template literal like module`M1`.
        if kind == Kind::TaggedTemplateExpression {
            let template = {
                let s = self.factory.store();
                let d = s
                    .node(node)
                    .as_tagged_template_expression()
                    .expect("TaggedTemplateExpression data");
                d.template
            };
            let template_loc = {
                let s = self.factory.store();
                s.node(template).loc
            };
            ({
                let __hoist_1_161 = self.skip_range_trivia(template_loc);
                let __hoist_1_162 =
                    &tsc_diagnostics::MODULE_DECLARATION_NAMES_MAY_ONLY_USE_OR_QUOTED_STRINGS;
                let __hoist_1_163 = &[];
                self.parse_error_at_range(__hoist_1_161, __hoist_1_162, __hoist_1_163)
            });
            return;
        }
        // Otherwise, if this isn't a well-known keyword-like identifier, give
        // the generic fallback message.
        let expression_text = if kind == Kind::Identifier {
            tsc_ast::node_text(self.factory.store(), node).to_string()
        } else {
            String::new()
        };
        if expression_text.is_empty() {
            self.parse_error_at_current_token(
                &tsc_diagnostics::X_0_EXPECTED,
                &[token_to_text(Kind::SemicolonToken).to_string()],
            );
            return;
        }
        let pos = tsc_scanner::skip_trivia(self.source_text, pos);
        // Some known keywords are likely signs of syntax being used improperly.
        match expression_text.as_str() {
            "const" | "let" | "var" => {
                self.parse_error_at(
                    pos,
                    end,
                    &tsc_diagnostics::VARIABLE_DECLARATION_NOT_ALLOWED_AT_THIS_LOCATION,
                    &[],
                );
                return;
            }
            "declare" => {
                // If a declared node failed to parse, it would have emitted a
                // diagnostic already.
                return;
            }
            "interface" => {
                self.parse_error_for_invalid_name(
                    &tsc_diagnostics::INTERFACE_NAME_CANNOT_BE_0,
                    &tsc_diagnostics::INTERFACE_MUST_BE_GIVEN_A_NAME,
                    Kind::OpenBraceToken,
                );
                return;
            }
            "is" => {
                let token_start = self.scanner.token_start() as i32;
                self.parse_error_at(
                    pos,
                    token_start,
                    &tsc_diagnostics::A_TYPE_PREDICATE_IS_ONLY_ALLOWED_IN_RETURN_TYPE_POSITION_FOR_FUNCTIONS_AND_METHODS,
                    &[],
                );
                return;
            }
            "module" | "namespace" => {
                self.parse_error_for_invalid_name(
                    &tsc_diagnostics::NAMESPACE_NAME_CANNOT_BE_0,
                    &tsc_diagnostics::NAMESPACE_MUST_BE_GIVEN_A_NAME,
                    Kind::OpenBraceToken,
                );
                return;
            }
            "type" => {
                self.parse_error_for_invalid_name(
                    &tsc_diagnostics::TYPE_ALIAS_NAME_CANNOT_BE_0,
                    &tsc_diagnostics::TYPE_ALIAS_MUST_BE_GIVEN_A_NAME,
                    Kind::EqualsToken,
                );
                return;
            }
            _ => {}
        }
        // The user alternatively might have misspelled or forgotten to add a
        // space after a common keyword.
        let mut suggestion = tsc_scanner::go_shims::get_spelling_suggestion_for_strings(
            &expression_text,
            tsc_scanner::get_viable_keyword_suggestions().into_iter(),
        );
        if suggestion.is_empty() {
            suggestion = get_space_suggestion(&expression_text);
        }
        if !suggestion.is_empty() {
            self.parse_error_at(
                pos,
                end,
                &tsc_diagnostics::UNKNOWN_KEYWORD_OR_IDENTIFIER_DID_YOU_MEAN_0,
                &[suggestion],
            );
            return;
        }
        // Unknown tokens are handled with their own errors in the scanner
        if self.token == Kind::Unknown {
            return;
        }
        // Otherwise, we know this some kind of unknown word, not just a
        // missing expected semicolon.
        self.parse_error_at(
            pos,
            end,
            &tsc_diagnostics::UNEXPECTED_KEYWORD_OR_IDENTIFIER,
            &[],
        );
    }

    /// Go: `func (p *Parser) parseErrorForInvalidName(nameDiagnostic, blankDiagnostic, tokenIfBlankName ast.Kind)`.
    fn parse_error_for_invalid_name(
        &mut self,
        name_diagnostic: &'static Message,
        blank_diagnostic: &'static Message,
        token_if_blank_name: Kind,
    ) {
        if self.token == token_if_blank_name {
            self.parse_error_at_current_token(blank_diagnostic, &[]);
        } else {
            ({
                let __hoist_1_165 = name_diagnostic;
                let __hoist_1_166 = &[self.token_value_string()];
                self.parse_error_at_current_token(__hoist_1_165, __hoist_1_166)
            });
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Interface / type alias / enum
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseInterfaceDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_interface_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected(Kind::InterfaceKeyword);
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        let heritage_clauses = self.parse_heritage_clauses(true /*isInterface*/);
        let members = self.parse_object_type_members();
        let result = {
            let __hoist_1_168 = self.factory.new_interface_declaration(
                modifiers,
                name,
                type_parameters,
                heritage_clauses,
                members,
            );
            let __hoist_1_169 = pos;
            self.finish_node(__hoist_1_168, __hoist_1_169)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseTypeAliasDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_type_alias_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected(Kind::TypeKeyword);
        if self.has_preceding_line_break() {
            self.parse_error_at_current_token(&tsc_diagnostics::LINE_BREAK_NOT_PERMITTED_HERE, &[]);
        }
        let name = self.parse_identifier();
        let type_parameters = self.parse_type_parameters();
        self.parse_expected(Kind::EqualsToken);
        let type_node: NodeId =
            if self.token == Kind::IntrinsicKeyword && self.look_ahead(|p| p.next_is_not_dot()) {
                self.parse_keyword_type_node()
            } else {
                self.parse_type()
            };
        self.parse_semicolon();
        let result = {
            let __hoist_1_171 = self.factory.new_type_alias_declaration(
                modifiers,
                name,
                type_parameters,
                type_node,
            );
            let __hoist_1_172 = pos;
            self.finish_node(__hoist_1_171, __hoist_1_172)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) nextIsNotDot() bool`.
    fn next_is_not_dot(&mut self) -> bool {
        self.next_token() != Kind::DotToken
    }

    /// Go: `func (p *Parser) parseEnumMember() *ast.Node`.
    pub(crate) fn parse_enum_member(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let name = self.parse_property_name();
        let initializer = self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, false, |p| {
            p.parse_initializer()
        });
        let result = {
            let __hoist_1_174 = self
                .factory
                .new_enum_member(name, (initializer != NodeId::NONE).then_some(initializer));
            let __hoist_1_175 = pos;
            self.finish_node(__hoist_1_174, __hoist_1_175)
        };
        self.with_jsdoc(result, jsdoc);
        Some(result)
    }

    /// Go: `func (p *Parser) parseEnumDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_enum_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.parse_expected(Kind::EnumKeyword);
        let name = self.parse_identifier();
        let members: Option<NodeList>;
        if self.parse_expected(Kind::OpenBraceToken) {
            let save_context_flags = self.context_flags;
            self.set_context_flags(
                NodeFlags::YIELD_CONTEXT.union(NodeFlags::AWAIT_CONTEXT),
                false,
            );
            members = self.parse_delimited_list(PC_ENUM_MEMBERS, |p| p.parse_enum_member());
            self.context_flags = save_context_flags;
            self.parse_expected(Kind::CloseBraceToken);
        } else {
            members = Some(self.create_missing_list());
        }
        let result = {
            let __hoist_1_177 = self.factory.new_enum_declaration(modifiers, name, members);
            let __hoist_1_178 = pos;
            self.finish_node(__hoist_1_177, __hoist_1_178)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Modules / imports / exports
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseModuleDeclaration(pos int, jsdoc, modifiers) *ast.Statement`.
    fn parse_module_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let mut keyword = Kind::ModuleKeyword;
        if self.token == Kind::GlobalKeyword {
            // global augmentation
            return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
        } else if self.parse_optional(Kind::NamespaceKeyword) {
            keyword = Kind::NamespaceKeyword;
        } else {
            self.parse_expected(Kind::ModuleKeyword);
            if self.token == Kind::StringLiteral {
                return self.parse_ambient_external_module_declaration(pos, jsdoc, modifiers);
            }
        }
        self.parse_module_or_namespace_declaration(
            pos, jsdoc, modifiers, false, /*nested*/
            keyword,
        )
    }

    /// Go: `func (p *Parser) parseAmbientExternalModuleDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_ambient_external_module_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let name: NodeId;
        let mut keyword = Kind::ModuleKeyword;
        let save_has_await_identifier = self.statement_has_await_identifier;
        if self.token == Kind::GlobalKeyword {
            // parse 'global' as name of global scope augmentation
            name = self.parse_identifier();
            keyword = Kind::GlobalKeyword;
        } else {
            // parse string literal
            name = self.parse_literal_expression();
        }
        let mut attributes = None;
        if keyword == Kind::ModuleKeyword && self.parse_optional(Kind::WithKeyword) {
            attributes = Some(self.parse_type_literal());
        }

        let body: Option<NodeId> = if self.token == Kind::OpenBraceToken {
            Some(self.parse_module_block())
        } else {
            self.parse_semicolon();
            None
        };
        let result = {
            let __hoist_1_180 = self
                .factory
                .new_module_declaration(modifiers, keyword, name, attributes, body);
            let __hoist_1_181 = pos;
            self.finish_node(__hoist_1_180, __hoist_1_181)
        };
        self.with_jsdoc(result, jsdoc);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    /// Go: `func (p *Parser) parseModuleBlock() *ast.Node`.
    fn parse_module_block(&mut self) -> NodeId {
        let pos = self.node_pos();
        let statements: Option<NodeList>;
        if self.parse_expected(Kind::OpenBraceToken) {
            statements = self.parse_list(PC_BLOCK_STATEMENTS, |p| Some(p.parse_statement()));
            self.parse_expected(Kind::CloseBraceToken);
        } else {
            statements = Some(self.create_missing_list());
        }
        {
            let __hoist_1_183 = self.factory.new_module_block(statements);
            let __hoist_1_184 = pos;
            self.finish_node(__hoist_1_183, __hoist_1_184)
        }
    }

    /// Go: `func (p *Parser) parseModuleOrNamespaceDeclaration(pos int, jsdoc, modifiers, nested bool, keyword) *ast.Node`.
    fn parse_module_or_namespace_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        nested: bool,
        keyword: Kind,
    ) -> NodeId {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let name: NodeId = if nested {
            self.parse_identifier_name()
        } else {
            self.parse_identifier()
        };

        let body: NodeId = if self.parse_optional(Kind::DotToken) {
            let implicit_export = self.factory.new_modifier(Kind::ExportKeyword);
            let export_pos = self.node_pos();
            {
                let s = self.factory.store();
                let n = s.node_mut(implicit_export);
                n.loc = TextRange::new(export_pos, export_pos);
                n.flags |= NodeFlags::REPARSED;
            }
            let implicit_export_loc = {
                let s = self.factory.store();
                s.node(implicit_export).loc
            };
            let implicit_modifiers =
                self.new_modifier_list(implicit_export_loc, vec![implicit_export]);
            let pos = self.node_pos();
            self.parse_module_or_namespace_declaration(
                pos,
                0, /*jsdoc*/
                Some(implicit_modifiers),
                true, /*nested*/
                keyword,
            )
        } else {
            self.parse_module_block()
        };
        let result = {
            let __hoist_1_186 =
                self.factory
                    .new_module_declaration(modifiers, keyword, name, None, Some(body));
            let __hoist_1_187 = pos;
            self.finish_node(__hoist_1_186, __hoist_1_187)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    /// Go: `func (p *Parser) parseImportDeclarationOrImportEqualsDeclaration(...) *ast.Statement`.
    fn parse_import_declaration_or_import_equals_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected(Kind::ImportKeyword);
        let after_import_pos = self.node_pos();
        // We don't parse the identifier here in await context, instead we will
        // report a grammar error in the checker.
        let save_has_await_identifier = self.statement_has_await_identifier;
        let phase_modifier_candidate = self.current_import_phase_modifier();
        let mut identifier: Option<NodeId> = None;
        if self.is_identifier() {
            identifier = Some(self.parse_identifier());
        }
        let mut phase_modifier = Kind::Unknown;
        let identifier_text = identifier
            .map(|id| tsc_ast::node_text(self.factory.store(), id).to_string())
            .unwrap_or_default();
        let type_phase_case = identifier.is_some()
            && identifier_text == "type"
            && (self.token != Kind::FromKeyword
                || self.is_identifier()
                    && self.look_ahead(|p| p.next_token_is_from_keyword_or_equals_token()))
            && (self.is_identifier()
                || self.token_after_import_definitely_produces_import_declaration());
        if type_phase_case {
            phase_modifier = Kind::TypeKeyword;
            identifier = None;
            if self.is_identifier() {
                identifier = Some(self.parse_identifier());
            }
        } else if identifier.is_some()
            && phase_modifier_candidate != Kind::Unknown
            && self.should_parse_import_phase_modifier()
        {
            phase_modifier = phase_modifier_candidate;
            identifier = None;
            if self.is_identifier() {
                identifier = Some(self.parse_identifier());
            }
        }
        if let Some(identifier) = identifier {
            if self.token_after_imported_identifier_allows_import_equals_declaration()
                && phase_modifier != Kind::DeferKeyword
                && phase_modifier != Kind::SourceKeyword
            {
                let import_equals = self.parse_import_equals_declaration(
                    pos,
                    jsdoc,
                    modifiers,
                    identifier,
                    phase_modifier == Kind::TypeKeyword,
                );
                let import_equals = self.check_js_syntax(import_equals);
                // Import= declaration is always parsed in an Await context, no
                // need to reparse
                self.statement_has_await_identifier = save_has_await_identifier;
                return import_equals;
            }
        }
        let import_clause = self.try_parse_import_clause(
            identifier,
            after_import_pos,
            phase_modifier,
            false, /*skipJSDocLeadingAsterisks*/
        );
        // import clause is always parsed in an Await context
        self.statement_has_await_identifier = save_has_await_identifier;
        let module_specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();
        self.parse_semicolon();
        let result = {
            let __hoist_1_189 = self.factory.new_import_declaration(
                modifiers,
                import_clause,
                module_specifier,
                attributes,
            );
            let __hoist_1_190 = pos;
            self.finish_node(__hoist_1_189, __hoist_1_190)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) nextTokenIsFromKeywordOrEqualsToken() bool`.
    fn next_token_is_from_keyword_or_equals_token(&mut self) -> bool {
        self.next_token();
        self.token == Kind::FromKeyword || self.token == Kind::EqualsToken
    }

    /// Go: `func (p *Parser) shouldParseImportPhaseModifier() bool`.
    fn should_parse_import_phase_modifier(&mut self) -> bool {
        let token = self.token;
        match token {
            Kind::CommaToken | Kind::EqualsToken => return false,
            Kind::FromKeyword
                if self.look_ahead(|p| p.next_token_is_token_string_literal()) =>
            {
                return false;
            }
            _ => {}
        }
        true
    }

    /// Go: `func (p *Parser) currentImportPhaseModifier() ast.Kind`.
    pub(crate) fn current_import_phase_modifier(&self) -> Kind {
        match self.scanner.token_text() {
            "defer" => Kind::DeferKeyword,
            "source" => Kind::SourceKeyword,
            _ => Kind::Unknown,
        }
    }

    /// Go: `func (p *Parser) tokenAfterImportDefinitelyProducesImportDeclaration() bool`.
    fn token_after_import_definitely_produces_import_declaration(&self) -> bool {
        self.token == Kind::AsteriskToken || self.token == Kind::OpenBraceToken
    }

    /// Go: `func (p *Parser) tokenAfterImportedIdentifierAllowsImportEqualsDeclaration() bool`.
    fn token_after_imported_identifier_allows_import_equals_declaration(&self) -> bool {
        !matches!(self.token, Kind::CommaToken | Kind::FromKeyword)
    }

    /// Go: `func (p *Parser) parseImportEqualsDeclaration(pos int, jsdoc, modifiers, identifier, isTypeOnly) *ast.Node`.
    fn parse_import_equals_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        identifier: NodeId,
        is_type_only: bool,
    ) -> NodeId {
        self.parse_expected(Kind::EqualsToken);
        let module_reference = self.parse_module_reference();
        self.parse_semicolon();
        let result = {
            let __hoist_1_192 = self.factory.new_import_equals_declaration(
                modifiers,
                is_type_only,
                identifier,
                module_reference,
            );
            let __hoist_1_193 = pos;
            self.finish_node(__hoist_1_192, __hoist_1_193)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseModuleReference() *ast.Node`.
    fn parse_module_reference(&mut self) -> NodeId {
        if self.token == Kind::RequireKeyword && self.look_ahead(|p| p.next_token_is_open_paren()) {
            return self.parse_external_module_reference();
        }
        self.parse_entity_name(
            false, /*allowReservedWords*/
            false, /*allowPrivateName*/
            None,  /*diagnosticMessage*/
        )
    }

    /// Go: `func (p *Parser) parseExternalModuleReference() *ast.Node`.
    fn parse_external_module_reference(&mut self) -> NodeId {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let pos = self.node_pos();
        self.parse_expected(Kind::RequireKeyword);
        self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_module_specifier();
        self.parse_expected(Kind::CloseParenToken);
        let result = {
            let __hoist_1_195 = self.factory.new_external_module_reference(expression);
            let __hoist_1_196 = pos;
            self.finish_node(__hoist_1_195, __hoist_1_196)
        };
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    /// Go: `func (p *Parser) parseModuleSpecifier() *ast.Expression`.
    pub(crate) fn parse_module_specifier(&mut self) -> NodeId {
        if self.token == Kind::StringLiteral {
            return self.parse_literal_expression();
        }
        // We allow arbitrary expressions here, even though the grammar only
        // allows string literals.  We check to ensure that it is only a string
        // literal later in the grammar check pass.
        self.parse_expression()
    }

    /// Go: `func (p *Parser) tryParseImportClause(identifier *ast.Node, pos int, phaseModifier ast.Kind, skipJSDocLeadingAsterisks bool) *ast.Node`.
    pub(crate) fn try_parse_import_clause(
        &mut self,
        identifier: Option<NodeId>,
        pos: TextPos,
        phase_modifier: Kind,
        skip_jsdoc_leading_asterisks: bool,
    ) -> Option<NodeId> {
        // ImportDeclaration:
        //  import ImportClause from ModuleSpecifier ;
        //  import ModuleSpecifier;
        if identifier.is_some()
            || self.token == Kind::AsteriskToken
            || self.token == Kind::OpenBraceToken
        {
            let import_clause = self.parse_import_clause(
                identifier,
                pos,
                phase_modifier,
                skip_jsdoc_leading_asterisks,
            );
            self.parse_expected(Kind::FromKeyword);
            return Some(import_clause);
        }
        if phase_modifier == Kind::DeferKeyword || phase_modifier == Kind::SourceKeyword {
            let clause = self.factory.new_import_clause(
                Some(phase_modifier),
                None, /*name*/
                None, /*namedBindings*/
            );
            return Some(self.finish_node(clause, pos));
        }
        None
    }

    /// Go: `func (p *Parser) parseImportClause(identifier *ast.Node, pos int, phaseModifier ast.Kind, skipJSDocLeadingAsterisks bool) *ast.Node`.
    fn parse_import_clause(
        &mut self,
        identifier: Option<NodeId>,
        pos: TextPos,
        phase_modifier: Kind,
        skip_jsdoc_leading_asterisks: bool,
    ) -> NodeId {
        // ImportClause:
        //  ImportedDefaultBinding
        //  NameSpaceImport
        //  NamedImports
        //  ImportedDefaultBinding, NameSpaceImport
        //  ImportedDefaultBinding, NamedImports
        // If there was no default import or if there is comma token after default import
        // parse namespace or named imports
        let mut named_bindings = None;
        let save_has_await_identifier = self.statement_has_await_identifier;
        if identifier.is_none() || self.parse_optional(Kind::CommaToken) {
            if skip_jsdoc_leading_asterisks {
                self.scanner.set_skip_jsdoc_leading_asterisks(true);
            }
            if self.token == Kind::AsteriskToken {
                named_bindings = Some(self.parse_namespace_import());
            } else {
                named_bindings = Some(self.parse_named_imports());
            }
            if skip_jsdoc_leading_asterisks {
                self.scanner.set_skip_jsdoc_leading_asterisks(false);
            }
        }
        let result = {
            let __hoist_1_198 = self.factory.new_import_clause(
                // Go: the PhaseModifier field is a plain `ast.Kind`
                // (KindUnknown = absent); the Rust AST models absence as
                // None (PORT).
                (phase_modifier != Kind::Unknown).then_some(phase_modifier),
                identifier,
                named_bindings,
            );
            let __hoist_1_199 = pos;
            self.finish_node(__hoist_1_198, __hoist_1_199)
        };
        self.statement_has_await_identifier = save_has_await_identifier;
        result
    }

    /// Go: `func (p *Parser) parseNamespaceImport() *ast.Node`.
    fn parse_namespace_import(&mut self) -> NodeId {
        // NameSpaceImport:
        //  * as ImportedBinding
        let pos = self.node_pos();
        self.parse_expected(Kind::AsteriskToken);
        self.parse_expected(Kind::AsKeyword);
        let name = self.parse_identifier();
        {
            let __hoist_1_201 = self.factory.new_namespace_import(name);
            let __hoist_1_202 = pos;
            self.finish_node(__hoist_1_201, __hoist_1_202)
        }
    }

    /// Go: `func (p *Parser) parseNamedImports() *ast.Node`.
    fn parse_named_imports(&mut self) -> NodeId {
        let pos = self.node_pos();
        // NamedImports:
        //  { }
        //  { ImportsList }
        //  { ImportsList, }
        let imports = self.parse_bracketed_list(
            PC_IMPORT_OR_EXPORT_SPECIFIERS,
            |p| p.parse_import_specifier(),
            Kind::OpenBraceToken,
            Kind::CloseBraceToken,
        );
        {
            let __hoist_1_204 = self.factory.new_named_imports(imports);
            let __hoist_1_205 = pos;
            self.finish_node(__hoist_1_204, __hoist_1_205)
        }
    }

    /// Go: `func (p *Parser) parseImportSpecifier() *ast.Node`.
    pub(crate) fn parse_import_specifier(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let (is_type_only, property_name, name) =
            self.parse_import_or_export_specifier(Kind::ImportSpecifier);
        let name_is_identifier = {
            let s = self.factory.store();
            s.node(name).kind == Kind::Identifier
        };
        let identifier_name: NodeId = if name_is_identifier {
            name
        } else {
            let name_loc = {
                let s = self.factory.store();
                s.node(name).loc
            };
            ({
                let __hoist_1_207 = self.skip_range_trivia(name_loc);
                let __hoist_1_208 = &tsc_diagnostics::IDENTIFIER_EXPECTED;
                let __hoist_1_209 = &[];
                self.parse_error_at_range(__hoist_1_207, __hoist_1_208, __hoist_1_209)
            });
            let identifier_name = self.new_identifier("");
            let name_pos = {
                let s = self.factory.store();
                s.node(name).pos()
            };
            self.finish_node(identifier_name, name_pos)
        };
        let result = {
            let __hoist_1_211 =
                self.factory
                    .new_import_specifier(is_type_only, property_name, identifier_name);
            let __hoist_1_212 = pos;
            self.finish_node(__hoist_1_211, __hoist_1_212)
        };
        Some(self.check_js_syntax(result))
    }

    /// Go: `func (p *Parser) parseImportOrExportSpecifier(kind ast.Kind) (isTypeOnly bool, propertyName *ast.Node, name *ast.Node)`.
    fn parse_import_or_export_specifier(&mut self, kind: Kind) -> (bool, Option<NodeId>, NodeId) {
        // ImportSpecifier:
        //   BindingIdentifier
        //   ModuleExportName as BindingIdentifier
        // ExportSpecifier:
        //   ModuleExportName
        //   ModuleExportName as ModuleExportName
        let mut is_type_only = false;
        let mut property_name: Option<NodeId> = None;
        let mut can_parse_as_keyword = true;
        let disallow_keywords = kind == Kind::ImportSpecifier;
        let (mut name, mut name_ok) = self.parse_module_export_name(disallow_keywords);
        let name_is_type = {
            let s = self.factory.store();
            let n = s.node(name);
            n.kind == Kind::Identifier && tsc_ast::node_text(s, name) == "type"
        };
        if name_is_type {
            // If the first token of an import specifier is 'type', there are a
            // lot of possibilities, especially if we see 'as' afterwards:
            //
            // import { type } from "mod";          - isTypeOnly: false,   name: type
            // import { type as } from "mod";       - isTypeOnly: true,    name: as
            // import { type as as } from "mod";    - isTypeOnly: false,   name: as,    propertyName: type
            // import { type as as as } from "mod"; - isTypeOnly: true,    name: as,    propertyName: as
            if self.token == Kind::AsKeyword {
                // { type as ...? }
                let first_as = self.parse_identifier_name();
                if self.token == Kind::AsKeyword {
                    // { type as as ...? }
                    let second_as = self.parse_identifier_name();
                    if self.can_parse_module_export_name() {
                        // { type as as something }
                        // { type as as "something" }
                        is_type_only = true;
                        property_name = Some(first_as);
                        let parsed = self.parse_module_export_name(disallow_keywords);
                        name = parsed.0;
                        name_ok = parsed.1;
                        can_parse_as_keyword = false;
                    } else {
                        // { type as as }
                        property_name = Some(name);
                        name = second_as;
                        can_parse_as_keyword = false;
                    }
                } else if self.can_parse_module_export_name() {
                    // { type as something }
                    // { type as "something" }
                    property_name = Some(name);
                    can_parse_as_keyword = false;
                    let parsed = self.parse_module_export_name(disallow_keywords);
                    name = parsed.0;
                    name_ok = parsed.1;
                } else {
                    // { type as }
                    is_type_only = true;
                    name = first_as;
                }
            } else if self.can_parse_module_export_name() {
                // { type something ...? }
                // { type "something" ...? }
                is_type_only = true;
                let parsed = self.parse_module_export_name(disallow_keywords);
                name = parsed.0;
                name_ok = parsed.1;
            }
        }
        if can_parse_as_keyword && self.token == Kind::AsKeyword {
            property_name = Some(name);
            self.parse_expected(Kind::AsKeyword);
            let parsed = self.parse_module_export_name(disallow_keywords);
            name = parsed.0;
            name_ok = parsed.1;
        }
        if !name_ok {
            let name_loc = {
                let s = self.factory.store();
                s.node(name).loc
            };
            ({
                let __hoist_1_214 = self.skip_range_trivia(name_loc);
                let __hoist_1_215 = &tsc_diagnostics::IDENTIFIER_EXPECTED;
                let __hoist_1_216 = &[];
                self.parse_error_at_range(__hoist_1_214, __hoist_1_215, __hoist_1_216)
            });
        }
        (is_type_only, property_name, name)
    }

    /// Go: `func (p *Parser) canParseModuleExportName() bool`.
    fn can_parse_module_export_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == Kind::StringLiteral
    }

    /// Go: `func (p *Parser) parseModuleExportName(disallowKeywords bool) (node *ast.Node, nameOk bool)`.
    fn parse_module_export_name(&mut self, disallow_keywords: bool) -> (NodeId, bool) {
        let mut name_ok = true;
        if self.token == Kind::StringLiteral {
            return (self.parse_literal_expression(), name_ok);
        }
        if disallow_keywords && tsc_ast::is_keyword_kind(self.token) && !self.is_identifier() {
            name_ok = false;
        }
        (self.parse_identifier_name(), name_ok)
    }

    /// Go: `func (p *Parser) tryParseImportAttributes() *ast.Node`.
    pub(crate) fn try_parse_import_attributes(&mut self) -> Option<NodeId> {
        if self.token == Kind::WithKeyword
            || (self.token == Kind::AssertKeyword && !self.has_preceding_line_break())
        {
            if self.token == Kind::AssertKeyword {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::IMPORT_ASSERTIONS_HAVE_BEEN_REPLACED_BY_IMPORT_ATTRIBUTES_USE_WITH_INSTEAD_OF_ASSERT,
                    &[],
                );
            }
            return Some(self.parse_import_attributes(self.token, false /*skipKeyword*/));
        }
        None
    }

    /// Go: `func (p *Parser) parseExportAssignment(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_export_assignment(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let is_export_equals = if self.parse_optional(Kind::EqualsToken) {
            true
        } else {
            self.parse_expected(Kind::DefaultKeyword);
            false
        };
        let expression = self.parse_assignment_expression_or_higher();
        self.parse_semicolon();
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        let result = {
            let __hoist_1_218 = self.factory.new_export_assignment(
                modifiers,
                is_export_equals,
                NodeId::NONE, /*typeNode*/
                expression,
            );
            let __hoist_1_219 = pos;
            self.finish_node(__hoist_1_218, __hoist_1_219)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseNamespaceExportDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_namespace_export_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        self.parse_expected(Kind::AsKeyword);
        self.parse_expected(Kind::NamespaceKeyword);
        let save_has_await_identifier = self.statement_has_await_identifier;
        let name = self.parse_identifier();
        self.statement_has_await_identifier = save_has_await_identifier;
        self.parse_semicolon();
        // NamespaceExportDeclaration nodes cannot have decorators or modifiers,
        // we attach them here so we can report them in the grammar checker
        let result = {
            let __hoist_1_221 = self
                .factory
                .new_namespace_export_declaration(modifiers, name);
            let __hoist_1_222 = pos;
            self.finish_node(__hoist_1_221, __hoist_1_222)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseExportDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    fn parse_export_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, true);
        let mut export_clause = None;
        let mut module_specifier = NodeId::NONE;
        let mut attributes = None;
        let is_type_only = self.parse_optional(Kind::TypeKeyword);
        let namespace_export_pos = self.node_pos();
        if self.parse_optional(Kind::AsteriskToken) {
            if self.parse_optional(Kind::AsKeyword) {
                export_clause = Some(self.parse_namespace_export(namespace_export_pos));
            }
            self.parse_expected(Kind::FromKeyword);
            module_specifier = self.parse_module_specifier();
        } else {
            export_clause = Some(self.parse_named_exports());
            // It is not uncommon to accidentally omit the 'from' keyword.
            // Additionally, in editing scenarios, the 'from' keyword can be
            // parsed as a named export when the export clause is unterminated
            // (i.e. `export { from "moduleName";`). If we don't have a 'from'
            // keyword, see if we have a string literal such that ASI won't
            // take effect.
            if self.token == Kind::FromKeyword
                || (self.token == Kind::StringLiteral && !self.has_preceding_line_break())
            {
                self.parse_expected(Kind::FromKeyword);
                module_specifier = self.parse_module_specifier();
            }
        }
        if module_specifier != NodeId::NONE
            && (self.token == Kind::WithKeyword || self.token == Kind::AssertKeyword)
            && !self.has_preceding_line_break()
        {
            if self.token == Kind::AssertKeyword {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::IMPORT_ASSERTIONS_HAVE_BEEN_REPLACED_BY_IMPORT_ATTRIBUTES_USE_WITH_INSTEAD_OF_ASSERT,
                    &[],
                );
            }
            attributes = Some(self.parse_import_attributes(self.token, false /*skipKeyword*/));
        }
        self.parse_semicolon();
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        let result = {
            let __hoist_1_224 = self.factory.new_export_declaration(
                modifiers,
                is_type_only,
                export_clause,
                (module_specifier != NodeId::NONE).then_some(module_specifier),
                attributes,
            );
            let __hoist_1_225 = pos;
            self.finish_node(__hoist_1_224, __hoist_1_225)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseNamespaceExport(pos int) *ast.Node`.
    fn parse_namespace_export(&mut self, pos: TextPos) -> NodeId {
        let (export_name, _) = self.parse_module_export_name(false /*disallowKeywords*/);
        {
            let __hoist_1_227 = self.factory.new_namespace_export(export_name);
            let __hoist_1_228 = pos;
            self.finish_node(__hoist_1_227, __hoist_1_228)
        }
    }

    /// Go: `func (p *Parser) parseNamedExports() *ast.Node`.
    fn parse_named_exports(&mut self) -> NodeId {
        let pos = self.node_pos();
        // NamedImports:
        //  { }
        //  { ImportsList }
        //  { ImportsList, }
        let exports = self.parse_bracketed_list(
            PC_IMPORT_OR_EXPORT_SPECIFIERS,
            |p| p.parse_export_specifier(),
            Kind::OpenBraceToken,
            Kind::CloseBraceToken,
        );
        {
            let __hoist_1_230 = self.factory.new_named_exports(exports);
            let __hoist_1_231 = pos;
            self.finish_node(__hoist_1_230, __hoist_1_231)
        }
    }

    /// Go: `func (p *Parser) parseExportSpecifier() *ast.Node`.
    pub(crate) fn parse_export_specifier(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let (is_type_only, property_name, name) =
            self.parse_import_or_export_specifier(Kind::ExportSpecifier);
        let result = {
            let __hoist_1_233 =
                self.factory
                    .new_export_specifier(is_type_only, property_name, name);
            let __hoist_1_234 = pos;
            self.finish_node(__hoist_1_233, __hoist_1_234)
        };
        self.with_jsdoc(result, jsdoc);
        Some(self.check_js_syntax(result))
    }
}

/// Go: `func isTypeHeritageClause(isInterface bool, token ast.Kind) bool`.
fn is_type_heritage_clause(is_interface: bool, token: Kind) -> bool {
    (is_interface && token == Kind::ExtendsKeyword)
        || (!is_interface && token == Kind::ImplementsKeyword)
}

/// Go: `func isValidHeritageTypeReferenceExpression(node *ast.Node) bool`.
fn is_valid_heritage_type_reference_expression(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    if tsc_ast::is_identifier(n) {
        return tsc_ast::node_is_present(n);
    }
    if tsc_ast::is_property_access_expression(n) {
        let d = n
            .as_property_access_expression()
            .expect("PropertyAccessExpression data");
        let name = s.node(d.name);
        tsc_ast::node_is_present(name)
            && !tsc_ast::is_optional_chain(n)
            && is_valid_heritage_type_reference_expression(s, d.expression)
    } else {
        false
    }
}

/// Go: `func modifierListHasAsync(modifiers *ast.ModifierList) bool`.
pub(crate) fn modifier_list_has_async(p: &mut Parser, modifiers: Option<&ModifierList>) -> bool {
    modifiers.is_some_and(|m| {
        m.nodes.iter().any(|&n| {
            let s = p.factory.store();
            is_async_modifier(s.node(n))
        })
    })
}

/// Go: `func getSpaceSuggestion(expressionText string) string`.
fn get_space_suggestion(expression_text: &str) -> String {
    for keyword in tsc_scanner::get_viable_keyword_suggestions() {
        if expression_text.len() > keyword.len() + 2 && expression_text.starts_with(keyword) {
            return format!("{} {}", keyword, &expression_text[keyword.len()..]);
        }
    }
    String::new()
}

// ────────────────────────────────────────────────────────────────────────────
// parser.go part 3 — modifiers, semicolons, identifiers, predicates,
// pragmas, and JS-syntax checks
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isKeyword(token ast.Kind) bool` — `ast.IsKeyword`
/// (PORT: dedup candidate into the utilities.go port).
pub(crate) fn is_keyword(token: Kind) -> bool {
    Kind::FirstKeyword <= token && token <= Kind::LastKeyword
}

/// Go: `ast.IsClassMemberModifier` (PORT: dedup candidate).
pub(crate) fn is_class_member_modifier(token: Kind) -> bool {
    tsc_ast::visitor::modifier_to_flag(token)
        .intersects(tsc_ast::ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
        || token == Kind::StaticKeyword
        || token == Kind::OverrideKeyword
        || token == Kind::AccessorKeyword
}

/// Go: `ast.IsFunctionLikeKind` (PORT: dedup candidate; the wholesale
/// utilities.go port owns the final home).
pub(crate) fn is_function_like_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::MethodSignature
            | Kind::CallSignature
            | Kind::JSDocSignature
            | Kind::ConstructSignature
            | Kind::IndexSignature
            | Kind::FunctionType
            | Kind::ConstructorType
    ) || is_function_like_declaration_kind(kind)
}

/// Go: `isFunctionLikeDeclarationKind`.
fn is_function_like_declaration_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Constructor
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction
    )
}

/// Go: `isLeftHandSideExpressionKind` (PORT: dedup candidate).
pub(crate) fn is_left_hand_side_expression_kind(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::PropertyAccessExpression
            | Kind::ElementAccessExpression
            | Kind::NewExpression
            | Kind::CallExpression
            | Kind::JsxElement
            | Kind::JsxSelfClosingElement
            | Kind::JsxFragment
            | Kind::TaggedTemplateExpression
            | Kind::ArrayLiteralExpression
            | Kind::ParenthesizedExpression
            | Kind::ObjectLiteralExpression
            | Kind::ClassExpression
            | Kind::FunctionExpression
            | Kind::Identifier
            | Kind::PrivateIdentifier
            | Kind::RegularExpressionLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::StringLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::TemplateExpression
            | Kind::FalseKeyword
            | Kind::NullKeyword
            | Kind::ThisKeyword
            | Kind::TrueKeyword
            | Kind::SuperKeyword
            | Kind::NonNullExpression
            | Kind::ExpressionWithTypeArguments
            | Kind::MetaProperty
            | Kind::ImportKeyword
            | Kind::MissingDeclaration
    )
}

/// Go: `ast.IsLeftHandSideExpression` (SkipPartiallyEmittedExpressions is a
/// no-op on freshly parsed trees).
pub(crate) fn is_left_hand_side_expression(node: &tsc_ast::Node) -> bool {
    is_left_hand_side_expression_kind(node.kind)
}

/// Go: `ast.IsFunctionLike` (PORT: dedup candidate).
pub(crate) fn is_function_like(node: &tsc_ast::Node) -> bool {
    is_function_like_kind(node.kind)
}

/// Go: `ast.IsStringLiteralLike` (PORT: dedup candidate).
pub(crate) fn is_string_literal_like(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral
    )
}

/// Go: `ast.IsQuestionToken` (PORT: dedup candidate; unused by this wave's
/// grammar but part of the Go predicate set — allow(dead_code) until the
/// remaining ast accessor surface lands).
#[allow(dead_code)]
pub(crate) fn is_question_token(node: &tsc_ast::Node) -> bool {
    node.kind == Kind::QuestionToken
}

/// Go: `ast.IsAmbientModule` (PORT: dedup candidate).
pub(crate) fn is_ambient_module(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    if !tsc_ast::is_module_declaration(n) {
        return false;
    }
    let d = n.as_module_declaration().expect("ModuleDeclaration data");
    let name_kind = s.node(d.name).kind;
    name_kind == Kind::StringLiteral || is_global_scope_augmentation(s, node)
}

/// Go: `ast.IsGlobalScopeAugmentation`.
fn is_global_scope_augmentation(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    let d = n.as_module_declaration().expect("ModuleDeclaration data");
    d.keyword == Kind::GlobalKeyword
        && d.body
            .is_some_and(|body| s.node(body).kind == Kind::ModuleBlock)
}

/// Go: `ast.IsAnyImportOrReExport` (PORT: dedup candidate).
pub(crate) fn is_any_import_or_re_export(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::ImportDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::JSImportDeclaration
            | Kind::ExportDeclaration
    )
}

/// Go: `ast.GetExternalModuleName` (PORT: dedup candidate) — Go dispatches
/// to the per-kind `ModuleSpecifier()` accessor; `export { a };` has none
/// (Go returns nil), so this is an Option.
pub(crate) fn get_external_module_name(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        // JSImportDeclaration is the kind-alias node carrying
        // ImportDeclaration data (factory kind-switch).
        Kind::ImportDeclaration | Kind::JSImportDeclaration => {
            let d = n.as_import_declaration().expect("ImportDeclaration data");
            Some(d.module_specifier)
        }
        Kind::ExportDeclaration => {
            let d = n.as_export_declaration().expect("ExportDeclaration data");
            d.module_specifier
        }
        Kind::ImportEqualsDeclaration => {
            let d = n
                .as_import_equals_declaration()
                .expect("ImportEqualsDeclaration data");
            let module_reference = s.node(d.module_reference);
            if module_reference.kind == Kind::ExternalModuleReference {
                Some(
                    module_reference
                        .as_external_module_reference()
                        .expect("ExternalModuleReference data")
                        .expression,
                )
            } else {
                None
            }
        }
        Kind::ImportType => {
            let d = n.as_import_type_node().expect("ImportTypeNode data");
            // Go: getImportTypeNodeLiteral — the argument of an import type.
            let argument = s.node(d.argument);
            if argument.kind == Kind::StringLiteral {
                Some(d.argument)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Go: `ast.IsImportPhaseMetaProperty` (PORT: dedup candidate).
pub(crate) fn is_import_phase_meta_property(s: &dyn NodeStore, node: NodeId) -> bool {
    let n = s.node(node);
    if n.kind != Kind::MetaProperty {
        return false;
    }
    let d = n.as_meta_property().expect("MetaProperty data");
    let is_import = d.keyword_token == Kind::ImportKeyword;
    let name_text = tsc_ast::node_text(s, d.name);
    is_import && (name_text == "defer" || name_text == "source")
}

/// Go: `ast.CanHaveDecorators` (PORT: dedup candidate).
pub(crate) fn can_have_decorators(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::Parameter
            | Kind::PropertyDeclaration
            | Kind::MethodDeclaration
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::ClassExpression
            | Kind::ClassDeclaration
    )
}

/// Go: `ast.CanHaveIllegalDecorators` (PORT: dedup candidate).
pub(crate) fn can_have_illegal_decorators(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::PropertyAssignment
            | Kind::ShorthandPropertyAssignment
            | Kind::FunctionDeclaration
            | Kind::Constructor
            | Kind::IndexSignature
            | Kind::ClassStaticBlockDeclaration
            | Kind::MissingDeclaration
            | Kind::VariableStatement
            | Kind::InterfaceDeclaration
            | Kind::TypeAliasDeclaration
            | Kind::EnumDeclaration
            | Kind::ModuleDeclaration
            | Kind::ImportEqualsDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
            | Kind::NamespaceExportDeclaration
            | Kind::ExportDeclaration
            | Kind::ExportAssignment
    )
}

/// Go: `ast.IsModifier` (a non-decortor modifier node).
pub(crate) fn is_modifier(node: &tsc_ast::Node) -> bool {
    node.kind != Kind::Decorator
        && tsc_ast::visitor::modifier_to_flag(node.kind) != tsc_ast::ModifierFlags::NONE
}

impl<'a, 'f> Parser<'a, 'f> {
    // ────────────────────────────────────────────────────────────────────────
    // Modifiers
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseModifiers() *ast.ModifierList`.
    pub(crate) fn parse_modifiers(&mut self) -> Option<ModifierList> {
        self.parse_modifiers_ex(false, false, false)
    }

    /// Go: `func (p *Parser) parseModifiersEx(allowDecorators, permitConstAsModifier, stopOnStartOfClassStaticBlock bool) *ast.ModifierList`.
    pub(crate) fn parse_modifiers_ex(
        &mut self,
        allow_decorators: bool,
        permit_const_as_modifier: bool,
        stop_on_start_of_class_static_block: bool,
    ) -> Option<ModifierList> {
        let mut has_leading_modifier = false;
        let mut has_trailing_decorator = false;
        let mut has_trailing_modifier = false;
        let mut has_static_modifier = false;
        // Decorators should be contiguous in a list of modifiers but can
        // potentially appear in two places (i.e., `[...leadingDecorators,
        // ...leadingModifiers, ...trailingDecorators, ...trailingModifiers]`).
        // The leading modifiers *should* only contain `export` and `default`
        // when trailingDecorators are present, but we'll handle errors for any
        // other leading modifiers in the checker. It is illegal to have both
        // leadingDecorators and trailingDecorators, but we will report that as
        // a grammar check in the checker. parse leading decorators
        let pos = self.node_pos();
        let mut list: Vec<NodeId> = Vec::with_capacity(16);
        loop {
            if allow_decorators && self.token == Kind::AtToken && !has_trailing_modifier {
                let decorator = self.parse_decorator();
                list.push(decorator);
                if has_leading_modifier {
                    has_trailing_decorator = true;
                }
            } else {
                let modifier = self.try_parse_modifier(
                    has_static_modifier,
                    permit_const_as_modifier,
                    stop_on_start_of_class_static_block,
                );
                let Some(modifier) = modifier else { break };
                let res = {
                    let s = self.factory.store();
                    s.node(modifier).kind == Kind::StaticKeyword
                };
                if res {
                    has_static_modifier = true;
                }
                list.push(modifier);
                if has_trailing_decorator {
                    has_trailing_modifier = true;
                } else {
                    has_leading_modifier = true;
                }
            }
        }
        if !list.is_empty() {
            let loc = TextRange::new(pos, self.node_pos());
            return Some(self.new_modifier_list(loc, list));
        }
        None
    }

    /// Go: `func (p *Parser) parseDecorator() *ast.Node`.
    fn parse_decorator(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::AtToken);
        let expression = self.do_in_context(NodeFlags::DECORATOR_CONTEXT, true, |p| {
            p.parse_decorator_expression()
        });
        {
            let __hoist_1_236 = self.factory.new_decorator(expression);
            let __hoist_1_237 = pos;
            self.finish_node(__hoist_1_236, __hoist_1_237)
        }
    }

    /// Go: `func (p *Parser) parseDecoratorExpression() *ast.Expression`.
    fn parse_decorator_expression(&mut self) -> NodeId {
        if self.in_await_context() && self.token == Kind::AwaitKeyword {
            // `@await` is disallowed in an [Await] context, but can cause
            // parsing to go off the rails. This simply parses the missing
            // identifier and moves on.
            let pos = self.node_pos();
            let await_expression = self.parse_identifier_with_diagnostic(
                Some(&tsc_diagnostics::EXPRESSION_EXPECTED),
                None,
            );
            self.next_token();
            let member_expression = self.parse_member_expression_rest(
                pos,
                await_expression,
                true, /*allowOptionalChain*/
            );
            return self.parse_call_expression_rest(pos, member_expression);
        }
        self.parse_left_hand_side_expression_or_higher()
    }

    /// Go: `func (p *Parser) tryParseModifier(hasSeenStaticModifier, permitConstAsModifier, stopOnStartOfClassStaticBlock bool) *ast.Node`.
    ///
    /// PORT: Go's two separate `else if` arms that both `return nil`
    /// (parser.go, tryParseModifier) are kept as-is — clippy's
    /// if-same-then-else is waived to preserve the Go statement shape.
    #[allow(clippy::if_same_then_else)]
    fn try_parse_modifier(
        &mut self,
        has_seen_static_modifier: bool,
        permit_const_as_modifier: bool,
        stop_on_start_of_class_static_block: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let kind = self.token;
        if self.token == Kind::ConstKeyword && permit_const_as_modifier {
            // We need to ensure that any subsequent modifiers appear on the
            // same line so that when 'const' is a standalone declaration, we
            // don't issue an error.
            if !self.look_ahead(|p| p.next_token_is_on_same_line_and_can_follow_modifier()) {
                return None;
            } else {
                self.next_token();
            }
        } else if stop_on_start_of_class_static_block
            && self.token == Kind::StaticKeyword
            && self.look_ahead(|p| p.next_token_is_open_brace())
        {
            return None;
        } else if has_seen_static_modifier && self.token == Kind::StaticKeyword {
            return None;
        } else {
            if !self.parse_any_contextual_modifier() {
                return None;
            }
        }
        Some({
            let __hoist_1_239 = self.factory.new_modifier(kind);
            let __hoist_1_240 = pos;
            self.finish_node(__hoist_1_239, __hoist_1_240)
        })
    }

    /// Go: `func (p *Parser) parseContextualModifier(t ast.Kind) bool`.
    pub(crate) fn parse_contextual_modifier(&mut self, t: Kind) -> bool {
        let state = self.mark();
        if self.token == t && self.next_token_can_follow_modifier() {
            return true;
        }
        self.rewind(state);
        false
    }

    /// Go: `func (p *Parser) parseAnyContextualModifier() bool`.
    fn parse_any_contextual_modifier(&mut self) -> bool {
        let state = self.mark();
        if tsc_ast::is_modifier_kind(self.token) && self.next_token_can_follow_modifier() {
            return true;
        }
        self.rewind(state);
        false
    }

    /// Go: `func (p *Parser) nextTokenCanFollowModifier() bool`.
    fn next_token_can_follow_modifier(&mut self) -> bool {
        match self.token {
            Kind::ConstKeyword => {
                // 'const' is only a modifier if followed by 'enum'.
                self.next_token() == Kind::EnumKeyword
            }
            Kind::ExportKeyword => {
                self.next_token();
                if self.token == Kind::DefaultKeyword {
                    self.look_ahead(|p| p.next_token_can_follow_default_keyword())
                } else if self.token == Kind::TypeKeyword {
                    self.look_ahead(|p| p.next_token_can_follow_export_modifier())
                } else {
                    self.can_follow_export_modifier()
                }
            }
            Kind::DefaultKeyword => self.next_token_can_follow_default_keyword(),
            Kind::StaticKeyword => {
                self.next_token();
                self.can_follow_modifier()
            }
            Kind::GetKeyword | Kind::SetKeyword => {
                self.next_token();
                self.can_follow_get_or_set_keyword()
            }
            _ => self.next_token_is_on_same_line_and_can_follow_modifier(),
        }
    }

    /// Go: `func (p *Parser) nextTokenCanFollowDefaultKeyword() bool`.
    fn next_token_can_follow_default_keyword(&mut self) -> bool {
        match self.next_token() {
            Kind::ClassKeyword | Kind::FunctionKeyword | Kind::InterfaceKeyword | Kind::AtToken => {
                true
            }
            Kind::AbstractKeyword => {
                self.look_ahead(|p| p.next_token_is_class_keyword_on_same_line())
            }
            Kind::AsyncKeyword => {
                self.look_ahead(|p| p.next_token_is_function_keyword_on_same_line())
            }
            _ => false,
        }
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrKeyword() bool`.
    pub(crate) fn next_token_is_identifier_or_keyword(&mut self) -> bool {
        token_is_identifier_or_keyword(self.next_token())
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrKeywordOrGreaterThan() bool`.
    pub(crate) fn next_token_is_identifier_or_keyword_or_greater_than(&mut self) -> bool {
        crate::token_is_identifier_or_keyword_or_greater_than(self.next_token())
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrKeywordOnSameLine() bool`.
    pub(crate) fn next_token_is_identifier_or_keyword_on_same_line(&mut self) -> bool {
        self.next_token_is_identifier_or_keyword() && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrKeywordOrLiteralOnSameLine() bool`.
    pub(crate) fn next_token_is_identifier_or_keyword_or_literal_on_same_line(&mut self) -> bool {
        (self.next_token_is_identifier_or_keyword()
            || self.token == Kind::NumericLiteral
            || self.token == Kind::BigIntLiteral
            || self.token == Kind::StringLiteral)
            && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenIsClassKeywordOnSameLine() bool`.
    fn next_token_is_class_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == Kind::ClassKeyword && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenIsFunctionKeywordOnSameLine() bool`.
    pub(crate) fn next_token_is_function_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == Kind::FunctionKeyword && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenCanFollowExportModifier() bool`.
    fn next_token_can_follow_export_modifier(&mut self) -> bool {
        self.next_token();
        self.can_follow_export_modifier()
    }

    /// Go: `func (p *Parser) canFollowExportModifier() bool`.
    fn can_follow_export_modifier(&self) -> bool {
        self.token == Kind::AtToken
            || (self.token != Kind::AsteriskToken
                && self.token != Kind::AsKeyword
                && self.token != Kind::OpenBraceToken
                && self.can_follow_modifier())
    }

    /// Go: `func (p *Parser) canFollowModifier() bool`.
    fn can_follow_modifier(&self) -> bool {
        self.token == Kind::OpenBracketToken
            || self.token == Kind::OpenBraceToken
            || self.token == Kind::AsteriskToken
            || self.token == Kind::DotDotDotToken
            || self.is_literal_property_name()
    }

    /// Go: `func (p *Parser) canFollowGetOrSetKeyword() bool`.
    fn can_follow_get_or_set_keyword(&self) -> bool {
        self.token == Kind::OpenBracketToken || self.is_literal_property_name()
    }

    /// Go: `func (p *Parser) nextTokenIsOnSameLineAndCanFollowModifier() bool`.
    fn next_token_is_on_same_line_and_can_follow_modifier(&mut self) -> bool {
        self.next_token();
        if self.has_preceding_line_break() {
            return false;
        }
        self.can_follow_modifier()
    }

    /// Go: `func (p *Parser) nextTokenIsOpenBrace() bool`.
    fn next_token_is_open_brace(&mut self) -> bool {
        self.next_token() == Kind::OpenBraceToken
    }

    // ────────────────────────────────────────────────────────────────────────
    // Semicolons
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) canParseSemicolon() bool`.
    pub(crate) fn can_parse_semicolon(&self) -> bool {
        // If there's a real semicolon, then we can always parse it out.
        // We can parse out an optional semicolon in ASI cases in the following cases.
        self.token == Kind::SemicolonToken
            || self.token == Kind::CloseBraceToken
            || self.token == Kind::EndOfFile
            || self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) tryParseSemicolon() bool`.
    pub(crate) fn try_parse_semicolon(&mut self) -> bool {
        if !self.can_parse_semicolon() {
            return false;
        }
        if self.token == Kind::SemicolonToken {
            // consume the semicolon if it was explicitly provided.
            self.next_token();
        }
        true
    }

    /// Go: `func (p *Parser) parseSemicolon() bool`.
    pub(crate) fn parse_semicolon(&mut self) -> bool {
        self.try_parse_semicolon() || self.parse_expected(Kind::SemicolonToken)
    }

    /// Go: `func (p *Parser) isLiteralPropertyName() bool`.
    pub(crate) fn is_literal_property_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token)
            || self.token == Kind::StringLiteral
            || self.token == Kind::NumericLiteral
            || self.token == Kind::BigIntLiteral
    }

    // ────────────────────────────────────────────────────────────────────────
    // Identifiers
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseRightSideOfDot(allowIdentifierNames, allowPrivateIdentifiers, allowUnicodeEscapeSequenceInIdentifierName bool) *ast.Node`.
    pub(crate) fn parse_right_side_of_dot(
        &mut self,
        allow_identifier_names: bool,
        allow_private_identifiers: bool,
        allow_unicode_escape_sequence_in_identifier_name: bool,
    ) -> NodeId {
        // Technically a keyword is valid here as all identifiers and keywords
        // are identifier names. However, often we'll encounter this in error
        // situations when the identifier or keyword is actually starting
        // another valid construct.
        //
        // So, we check for the following specific case:
        //
        //      name.
        //      identifierOrKeyword identifierNameOrKeyword
        //
        // Note: the newlines are important here.  For example, if that above
        // code were rewritten into:
        //
        //      name.identifierOrKeyword
        //      identifierNameOrKeyword
        //
        // Then we would consider it valid.  That's because ASI would take
        // effect and the code would be implicitly:
        // "name.identifierOrKeyword; identifierNameOrKeyword".  In the first
        // case though, ASI will not take effect because there is not a line
        // terminator after the identifier or keyword.
        if self.has_preceding_line_break()
            && token_is_identifier_or_keyword(self.token)
            && self.look_ahead(|p| p.next_token_is_identifier_or_keyword_on_same_line())
        {
            // Report that we need an identifier.  However, report it right
            // after the dot, and not on the next token.  This is because the
            // next token might actually be an identifier and the error would
            // be quite confusing.
            let (p, e) = (self.node_pos(), self.node_pos());
            self.parse_error_at(p, e, &tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
            return self.create_missing_identifier();
        }
        if self.token == Kind::PrivateIdentifier {
            let node = self.parse_private_identifier();
            if allow_private_identifiers {
                return node;
            }
            let (p, e) = (self.node_pos(), self.node_pos());
            self.parse_error_at(p, e, &tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
            return self.create_missing_identifier();
        }
        if allow_identifier_names {
            if allow_unicode_escape_sequence_in_identifier_name {
                return self.parse_identifier_name();
            }
            return self.parse_identifier_name_error_on_unicode_escape_sequence();
        }
        let save_has_await_identifier = self.statement_has_await_identifier;
        let id = self.parse_identifier();
        self.statement_has_await_identifier = save_has_await_identifier;
        id
    }

    /// Go: `func (p *Parser) parsePrivateIdentifier() *ast.Node`.
    pub(crate) fn parse_private_identifier(&mut self) -> NodeId {
        let pos = self.node_pos();
        let text = self.token_value_string();
        self.next_token();
        {
            let __hoist_1_242 = self.factory.new_private_identifier(&text);
            let __hoist_1_243 = pos;
            self.finish_node(__hoist_1_242, __hoist_1_243)
        }
    }

    /// Go: `func (p *Parser) parseIdentifierNameErrorOnUnicodeEscapeSequence() *ast.Node`.
    pub(crate) fn parse_identifier_name_error_on_unicode_escape_sequence(&mut self) -> NodeId {
        if self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape() {
            self.parse_error_at_current_token(
                &tsc_diagnostics::UNICODE_ESCAPE_SEQUENCE_CANNOT_APPEAR_HERE,
                &[],
            );
        }
        self.create_identifier(token_is_identifier_or_keyword(self.token))
    }

    /// Go: `func (p *Parser) parseBindingIdentifier() *ast.Node`.
    pub(crate) fn parse_binding_identifier(&mut self) -> NodeId {
        self.parse_binding_identifier_with_diagnostic(None)
    }

    /// Go: `func (p *Parser) parseBindingIdentifierWithDiagnostic(privateIdentifierDiagnosticMessage) *ast.Node`.
    pub(crate) fn parse_binding_identifier_with_diagnostic(
        &mut self,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let id = {
            let __hoist_1_245 = self.is_binding_identifier();
            let __hoist_1_246 =
            None /*diagnosticMessage*/;
            let __hoist_1_247 = private_identifier_diagnostic_message;
            self.create_identifier_with_diagnostic(__hoist_1_245, __hoist_1_246, __hoist_1_247)
        };
        self.statement_has_await_identifier = save_has_await_identifier;
        id
    }

    /// Go: `func (p *Parser) parseIdentifierName() *ast.Node`.
    pub(crate) fn parse_identifier_name(&mut self) -> NodeId {
        self.parse_identifier_name_with_diagnostic(None)
    }

    /// Go: `func (p *Parser) parseIdentifierNameWithDiagnostic(diagnosticMessage) *ast.Node`.
    pub(crate) fn parse_identifier_name_with_diagnostic(
        &mut self,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        self.create_identifier_with_diagnostic(
            token_is_identifier_or_keyword(self.token),
            diagnostic_message,
            None,
        )
    }

    /// Go: `func (p *Parser) parseIdentifier() *ast.Node`.
    pub(crate) fn parse_identifier(&mut self) -> NodeId {
        self.parse_identifier_with_diagnostic(None, None)
    }

    /// Go: `func (p *Parser) parseIdentifierWithDiagnostic(diagnosticMessage, privateIdentifierDiagnosticMessage) *ast.Node`.
    pub(crate) fn parse_identifier_with_diagnostic(
        &mut self,
        diagnostic_message: Option<&'static Message>,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        {
            let __hoist_1_249 = self.is_identifier();
            let __hoist_1_250 = diagnostic_message;
            let __hoist_1_251 = private_identifier_diagnostic_message;
            self.create_identifier_with_diagnostic(__hoist_1_249, __hoist_1_250, __hoist_1_251)
        }
    }

    /// Go: `func (p *Parser) createIdentifier(isIdentifier bool) *ast.Node`.
    pub(crate) fn create_identifier(&mut self, is_identifier: bool) -> NodeId {
        self.create_identifier_with_diagnostic(is_identifier, None, None)
    }

    /// Go: `func (p *Parser) createIdentifierWithDiagnostic(isIdentifier bool, diagnosticMessage, privateIdentifierDiagnosticMessage) *ast.Node`.
    pub(crate) fn create_identifier_with_diagnostic(
        &mut self,
        is_identifier: bool,
        diagnostic_message: Option<&'static Message>,
        private_identifier_diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        if is_identifier {
            let pos = if self.scanner.has_preceding_jsdoc_leading_asterisks() {
                self.scanner.token_start() as i32
            } else {
                self.node_pos()
            };
            let text = self.token_value_string();
            self.next_token_without_check();
            let id = self.new_identifier(&text);
            return self.finish_node(id, pos);
        }
        if self.token == Kind::PrivateIdentifier {
            match private_identifier_diagnostic_message {
                Some(message) => {
                    self.parse_error_at_current_token(message, &[]);
                }
                None => {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::PRIVATE_IDENTIFIERS_ARE_NOT_ALLOWED_OUTSIDE_CLASS_BODIES,
                        &[],
                    );
                }
            }
            return self.create_identifier(true /*isIdentifier*/);
        }
        // Only for end of file because the error gets reported incorrectly on
        // embedded script tags.
        let report_at_current_position = self.token == Kind::EndOfFile;
        match diagnostic_message {
            Some(diagnostic_message) => {
                if report_at_current_position {
                    let pos = self.scanner.token_full_start() as i32;
                    self.parse_error_at(pos, pos, diagnostic_message, &[]);
                } else {
                    self.parse_error_at_current_token(diagnostic_message, &[]);
                }
            }
            None => {
                if is_reserved_word(self.token) {
                    let text = self.scanner.token_text().to_string();
                    if report_at_current_position {
                        let pos = self.scanner.token_full_start() as i32;
                        self.parse_error_at(
                            pos,
                            pos,
                            &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_THAT_CANNOT_BE_USED_HERE,
                            &[text],
                        );
                    } else {
                        self.parse_error_at_current_token(
                            &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_THAT_CANNOT_BE_USED_HERE,
                            &[text],
                        );
                    }
                } else {
                    if report_at_current_position {
                        let pos = self.scanner.token_full_start() as i32;
                        self.parse_error_at(pos, pos, &tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
                    } else {
                        self.parse_error_at_current_token(
                            &tsc_diagnostics::IDENTIFIER_EXPECTED,
                            &[],
                        );
                    }
                }
            }
        }
        self.create_missing_identifier()
    }

    // ────────────────────────────────────────────────────────────────────────
    // is* predicates
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) isStartOfStatement() bool`.
    pub(crate) fn is_start_of_statement(&mut self) -> bool {
        match self.token {
            // 'catch' and 'finally' do not actually indicate that the code is
            // part of a statement, however, we say they are here so that we
            // may gracefully parse them and error later.
            Kind::AtToken
            | Kind::SemicolonToken
            | Kind::OpenBraceToken
            | Kind::VarKeyword
            | Kind::LetKeyword
            | Kind::UsingKeyword
            | Kind::FunctionKeyword
            | Kind::ClassKeyword
            | Kind::EnumKeyword
            | Kind::IfKeyword
            | Kind::DoKeyword
            | Kind::WhileKeyword
            | Kind::ForKeyword
            | Kind::ContinueKeyword
            | Kind::BreakKeyword
            | Kind::ReturnKeyword
            | Kind::WithKeyword
            | Kind::SwitchKeyword
            | Kind::ThrowKeyword
            | Kind::TryKeyword
            | Kind::DebuggerKeyword
            | Kind::CatchKeyword
            | Kind::FinallyKeyword => true,
            Kind::ImportKeyword => {
                self.is_start_of_declaration()
                    || self.is_next_token_open_paren_or_less_than_or_dot()
            }
            Kind::ConstKeyword | Kind::ExportKeyword => self.is_start_of_declaration(),
            Kind::AsyncKeyword
            | Kind::DeclareKeyword
            | Kind::InterfaceKeyword
            | Kind::ModuleKeyword
            | Kind::NamespaceKeyword
            | Kind::TypeKeyword
            | Kind::GlobalKeyword
            | Kind::DeferKeyword
            | Kind::SourceKeyword => {
                // When these don't start a declaration, they're an identifier
                // in an expression statement
                true
            }
            Kind::AccessorKeyword
            | Kind::PublicKeyword
            | Kind::PrivateKeyword
            | Kind::ProtectedKeyword
            | Kind::StaticKeyword
            | Kind::ReadonlyKeyword => {
                // When these don't start a declaration, they may be the start
                // of a class member if an identifier immediately follows.
                // Otherwise they're an identifier in an expression statement.
                self.is_start_of_declaration()
                    || !self.look_ahead(|p| p.next_token_is_identifier_or_keyword_on_same_line())
            }
            _ => self.is_start_of_expression(),
        }
    }

    /// Go: `func (p *Parser) isStartOfDeclaration() bool`.
    pub(crate) fn is_start_of_declaration(&mut self) -> bool {
        self.look_ahead(|p| p.scan_start_of_declaration())
    }

    /// Go: `func (p *Parser) scanStartOfDeclaration() bool`.
    fn scan_start_of_declaration(&mut self) -> bool {
        loop {
            match self.token {
                Kind::VarKeyword
                | Kind::LetKeyword
                | Kind::ConstKeyword
                | Kind::FunctionKeyword
                | Kind::ClassKeyword
                | Kind::EnumKeyword => return true,
                Kind::UsingKeyword => return self.is_using_declaration(),
                Kind::AwaitKeyword => return self.is_await_using_declaration(),
                // 'declare', 'module', 'namespace', 'interface'* and 'type' are
                // all legal JavaScript identifiers; however, an identifier cannot
                // be followed by another identifier on the same line. This is
                // what we count on to parse out the respective declarations.
                Kind::InterfaceKeyword
                | Kind::TypeKeyword
                | Kind::DeferKeyword
                | Kind::SourceKeyword => return self.next_token_is_identifier_on_same_line(),
                Kind::ModuleKeyword | Kind::NamespaceKeyword => {
                    return self.next_token_is_identifier_or_string_literal_on_same_line()
                }
                Kind::AbstractKeyword
                | Kind::AccessorKeyword
                | Kind::AsyncKeyword
                | Kind::DeclareKeyword
                | Kind::PrivateKeyword
                | Kind::ProtectedKeyword
                | Kind::PublicKeyword
                | Kind::ReadonlyKeyword => {
                    let previous_token = self.token;
                    self.next_token();
                    // ASI takes effect for this modifier.
                    if self.has_preceding_line_break() {
                        return false;
                    }
                    if previous_token == Kind::DeclareKeyword && self.token == Kind::TypeKeyword {
                        // If we see 'declare type', then commit to parsing a
                        // type alias. parseTypeAliasDeclaration will report
                        // Line_break_not_permitted_here if needed.
                        return true;
                    }
                    continue;
                }
                Kind::GlobalKeyword => {
                    self.next_token();
                    return self.token == Kind::OpenBraceToken
                        || self.token == Kind::Identifier
                        || self.token == Kind::ExportKeyword;
                }
                Kind::ImportKeyword => {
                    self.next_token();
                    return self.token == Kind::DeferKeyword
                        || self.token == Kind::SourceKeyword
                        || self.token == Kind::StringLiteral
                        || self.token == Kind::AsteriskToken
                        || self.token == Kind::OpenBraceToken
                        || token_is_identifier_or_keyword(self.token);
                }
                Kind::ExportKeyword => {
                    self.next_token();
                    if self.token == Kind::EqualsToken
                        || self.token == Kind::AsteriskToken
                        || self.token == Kind::OpenBraceToken
                        || self.token == Kind::DefaultKeyword
                        || self.token == Kind::AsKeyword
                        || self.token == Kind::AtToken
                    {
                        return true;
                    }
                    if self.token == Kind::TypeKeyword {
                        self.next_token();
                        return self.token == Kind::AsteriskToken
                            || self.token == Kind::OpenBraceToken
                            || (self.is_identifier() && !self.has_preceding_line_break());
                    }
                    continue;
                }
                Kind::StaticKeyword => {
                    self.next_token();
                    continue;
                }
                _ => return false,
            }
        }
    }

    /// Go: `func (p *Parser) isStartOfExpression() bool`.
    pub(crate) fn is_start_of_expression(&mut self) -> bool {
        if self.is_start_of_left_hand_side_expression() {
            return true;
        }
        match self.token {
            Kind::PlusToken
            | Kind::MinusToken
            | Kind::TildeToken
            | Kind::ExclamationToken
            | Kind::DeleteKeyword
            | Kind::TypeOfKeyword
            | Kind::VoidKeyword
            | Kind::PlusPlusToken
            | Kind::MinusMinusToken
            | Kind::LessThanToken
            | Kind::AwaitKeyword
            | Kind::YieldKeyword
            | Kind::PrivateIdentifier
            | Kind::AtToken => {
                // Yield/await always starts an expression.  Either it is an
                // identifier (in which case it is definitely an expression).
                // Or it's a keyword (either because we're in a generator or
                // async function, or in strict mode (or both)) and it started a
                // yield or await expression.
                return true;
            }
            _ => {}
        }
        // Error tolerance.  If we see the start of some binary operator, we
        // consider that the start of an expression.  That way we'll parse out
        // a missing identifier, give a good message about an identifier being
        // missing, and then consume the rest of the binary expression.
        if self.is_binary_operator() {
            return true;
        }
        self.is_identifier()
    }

    /// Go: `func (p *Parser) isStartOfLeftHandSideExpression() bool`.
    pub(crate) fn is_start_of_left_hand_side_expression(&mut self) -> bool {
        match self.token {
            Kind::ThisKeyword
            | Kind::SuperKeyword
            | Kind::NullKeyword
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::StringLiteral
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::TemplateHead
            | Kind::OpenParenToken
            | Kind::OpenBracketToken
            | Kind::OpenBraceToken
            | Kind::FunctionKeyword
            | Kind::ClassKeyword
            | Kind::NewKeyword
            | Kind::SlashToken
            | Kind::SlashEqualsToken
            | Kind::Identifier => true,
            Kind::ImportKeyword => self.is_next_token_open_paren_or_less_than_or_dot(),
            _ => self.is_identifier(),
        }
    }

    /// Go: `func (p *Parser) isNextTokenOpenParenOrLessThanOrDot() bool`.
    fn is_next_token_open_paren_or_less_than_or_dot(&mut self) -> bool {
        self.look_ahead(|p| p.next_token_is_open_paren_or_less_than_or_dot())
    }

    /// Go: `func (p *Parser) nextTokenIsOpenParenOrLessThanOrDot() bool`.
    fn next_token_is_open_paren_or_less_than_or_dot(&mut self) -> bool {
        matches!(
            self.next_token(),
            Kind::OpenParenToken | Kind::LessThanToken | Kind::DotToken
        )
    }

    /// Go: `func (p *Parser) isBinaryOperator() bool`.
    pub(crate) fn is_binary_operator(&self) -> bool {
        if self.in_disallow_in_context() && self.token == Kind::InKeyword {
            return false;
        }
        tsc_ast::get_binary_operator_precedence(self.token) != tsc_ast::OperatorPrecedence::Invalid
    }

    /// Go: `func (p *Parser) isIdentifier() bool` — ignore strict mode flag
    /// because we will report an error in type checker instead.
    pub(crate) fn is_identifier(&self) -> bool {
        if self.token == Kind::Identifier {
            return true;
        }
        // If we have a 'yield' keyword, and we're in the [yield] context, then
        // 'yield' is considered a keyword and is not an identifier.
        // If we have a 'await' keyword, and we're in the [Await] context, then
        // 'await' is considered a keyword and is not an identifier.
        if (self.token == Kind::YieldKeyword && self.in_yield_context())
            || (self.token == Kind::AwaitKeyword && self.in_await_context())
        {
            return false;
        }
        self.token > Kind::LastReservedWord
    }

    /// Go: `func (p *Parser) isBindingIdentifier() bool` — `let await`/`let
    /// yield` in [Yield] or [Await] are allowed here and disallowed in the
    /// binder.
    pub(crate) fn is_binding_identifier(&self) -> bool {
        self.token == Kind::Identifier || self.token > Kind::LastReservedWord
    }

    /// Go: `func (p *Parser) isBindingIdentifierOrPrivateIdentifierOrPattern() bool`.
    pub(crate) fn is_binding_identifier_or_private_identifier_or_pattern(&self) -> bool {
        self.token == Kind::OpenBraceToken
            || self.token == Kind::OpenBracketToken
            || self.token == Kind::PrivateIdentifier
            || self.is_binding_identifier()
    }

    /// Go: `func (p *Parser) isImportAttributeName() bool`.
    fn is_import_attribute_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == Kind::StringLiteral
    }

    /// Go: `func (p *Parser) isValidHeritageClauseObjectLiteral() bool`.
    fn is_valid_heritage_clause_object_literal(&mut self) -> bool {
        self.look_ahead(|p| p.next_is_valid_heritage_clause_object_literal())
    }

    /// Go: `func (p *Parser) nextIsValidHeritageClauseObjectLiteral() bool`.
    fn next_is_valid_heritage_clause_object_literal(&mut self) -> bool {
        if self.next_token() == Kind::CloseBraceToken {
            // if we see "extends {}" then only treat the {} as what we're
            // extending (and not the class body) if we have:
            //
            //      extends {} {
            //      extends {},
            //      extends {} extends
            //      extends {} implements
            let next = self.next_token();
            return matches!(
                next,
                Kind::CommaToken
                    | Kind::OpenBraceToken
                    | Kind::ExtendsKeyword
                    | Kind::ImplementsKeyword
            );
        }
        true
    }

    /// Go: `func (p *Parser) isHeritageClause() bool`.
    fn is_heritage_clause(&self) -> bool {
        self.token == Kind::ExtendsKeyword || self.token == Kind::ImplementsKeyword
    }

    /// Go: `func (p *Parser) isHeritageClauseExtendsOrImplementsKeyword() bool`.
    fn is_heritage_clause_extends_or_implements_keyword(&mut self) -> bool {
        self.is_heritage_clause() && self.look_ahead(|p| p.next_is_start_of_expression())
    }

    /// Go: `func (p *Parser) nextIsStartOfExpression() bool`.
    fn next_is_start_of_expression(&mut self) -> bool {
        self.next_token();
        self.is_start_of_expression()
    }

    /// Go: `func (p *Parser) isUsingDeclaration() bool` — 'using' always starts
    /// a lexical declaration if followed by an identifier. We also eagerly
    /// parse |ObjectBindingPattern| so that we can report a grammar error
    /// during check. We don't parse out |ArrayBindingPattern| since it
    /// potentially conflicts with element access (i.e., `using[x]`).
    pub(crate) fn is_using_declaration(&mut self) -> bool {
        self.look_ahead(|p| {
            p.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
                false, /*disallowOf*/
            )
        })
    }

    /// Go: `func (p *Parser) nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLine(disallowOf bool) bool`.
    pub(crate) fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
        &mut self,
        disallow_of: bool,
    ) -> bool {
        self.next_token();
        if disallow_of && self.token == Kind::OfKeyword {
            return self.look_ahead(|p| p.next_token_is_equals_or_semicolon_or_colon_token());
        }
        (self.is_binding_identifier() || self.token == Kind::OpenBraceToken)
            && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenIsEqualsOrSemicolonOrColonToken() bool`.
    fn next_token_is_equals_or_semicolon_or_colon_token(&mut self) -> bool {
        self.next_token();
        matches!(
            self.token,
            Kind::EqualsToken | Kind::SemicolonToken | Kind::ColonToken
        )
    }

    /// Go: `nextTokenIsBindingIdentifierOrStartOfDestructuringOnSameLineDisallowOf`.
    pub(crate) fn next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of(
        &mut self,
    ) -> bool {
        self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
            true, /*disallowOf*/
        )
    }

    /// Go: `func (p *Parser) isAwaitUsingDeclaration() bool`.
    pub(crate) fn is_await_using_declaration(&mut self) -> bool {
        self.look_ahead(|p| {
            p.next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line()
        })
    }

    /// Go: `nextIsUsingKeywordThenBindingIdentifierOrStartOfObjectDestructuringOnSameLine`.
    fn next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line(
        &mut self,
    ) -> bool {
        self.next_token() == Kind::UsingKeyword
            && self.next_token_is_binding_identifier_or_start_of_destructuring_on_same_line(
                false, /*disallowOf*/
            )
    }

    /// Go: `func (p *Parser) isLetDeclaration() bool` — in ES6 'let' always
    /// starts a lexical declaration if followed by an identifier or { or [.
    pub(crate) fn is_let_declaration(&mut self) -> bool {
        self.look_ahead(|p| p.next_token_is_binding_identifier_or_start_of_destructuring())
    }

    /// Go: `func (p *Parser) nextTokenIsBindingIdentifierOrStartOfDestructuring() bool`.
    fn next_token_is_binding_identifier_or_start_of_destructuring(&mut self) -> bool {
        self.next_token();
        self.is_binding_identifier()
            || self.token == Kind::OpenBraceToken
            || self.token == Kind::OpenBracketToken
    }

    /// Go: `func (p *Parser) nextTokenIsTokenStringLiteral() bool`.
    pub(crate) fn next_token_is_token_string_literal(&mut self) -> bool {
        self.next_token() == Kind::StringLiteral
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOnSameLine() bool`.
    fn next_token_is_identifier_on_same_line(&mut self) -> bool {
        self.next_token();
        self.is_identifier() && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrStringLiteralOnSameLine() bool`.
    fn next_token_is_identifier_or_string_literal_on_same_line(&mut self) -> bool {
        self.next_token();
        (self.is_identifier() || self.token == Kind::StringLiteral)
            && !self.has_preceding_line_break()
    }

    /// Go: `func (p *Parser) scanTypeMemberStart() bool`.
    fn scan_type_member_start(&mut self) -> bool {
        // Return true if we have the start of a signature member
        if matches!(
            self.token,
            Kind::OpenParenToken | Kind::LessThanToken | Kind::GetKeyword | Kind::SetKeyword
        ) {
            return true;
        }
        let mut id_token = false;
        // Eat up all modifiers, but hold on to the last one in case it is
        // actually an identifier
        while tsc_ast::is_modifier_kind(self.token) {
            id_token = true;
            self.next_token();
        }
        // Index signatures and computed property names are type members
        if self.token == Kind::OpenBracketToken {
            return true;
        }
        // Try to get the first property-like token following all modifiers
        if self.is_literal_property_name() {
            id_token = true;
            self.next_token();
        }
        // If we were able to get any potential identifier, check that it is
        // the start of a member declaration
        if id_token {
            return matches!(
                self.token,
                Kind::OpenParenToken
                    | Kind::LessThanToken
                    | Kind::QuestionToken
                    | Kind::ColonToken
                    | Kind::CommaToken
            ) || self.can_parse_semicolon();
        }
        false
    }

    /// Go: `func (p *Parser) scanClassMemberStart() bool`.
    fn scan_class_member_start(&mut self) -> bool {
        let mut id_token = Kind::Unknown;
        if self.token == Kind::AtToken {
            return true;
        }
        // Eat up all modifiers, but hold on to the last one in case it is
        // actually an identifier.
        while tsc_ast::is_modifier_kind(self.token) {
            id_token = self.token;
            // If the idToken is a class modifier (protected, private, public,
            // and static), it is certain that we are starting to parse class
            // member. This allows better error recovery
            if is_class_member_modifier(id_token) {
                return true;
            }
            self.next_token();
        }
        if self.token == Kind::AsteriskToken {
            return true;
        }
        // Try to get the first property-like token following all modifiers.
        // This can either be an identifier or the 'get' or 'set' keywords.
        if self.is_literal_property_name() {
            id_token = self.token;
            self.next_token();
        }
        // Index signatures and computed properties are class members; we can parse.
        if self.token == Kind::OpenBracketToken {
            return true;
        }
        // If we were able to get any potential identifier...
        if id_token != Kind::Unknown {
            // If we have a non-keyword identifier, or if we have an accessor,
            // then it's safe to parse.
            if !is_keyword(id_token) || id_token == Kind::SetKeyword || id_token == Kind::GetKeyword
            {
                return true;
            }
            // If it *is* a keyword, but not an accessor, check a little
            // farther along to see if it should actually be parsed as a class
            // member.
            match self.token {
                Kind::OpenParenToken // Method declaration
                | Kind::LessThanToken // Generic Method declaration
                | Kind::ExclamationToken // Non-null assertion on property name
                | Kind::ColonToken // Type Annotation for declaration
                | Kind::EqualsToken // Initializer for declaration
                | Kind::QuestionToken => return true, // Not valid, but permitted so that it gets caught later on.
                _ => {}
            }
            // Covers
            //  - Semicolons     (declaration termination)
            //  - Closing braces (end-of-class, must be declaration)
            //  - End-of-files   (not valid, but permitted so that it gets caught later on)
            //  - Line-breaks    (enabling *automatic semicolon insertion*)
            return self.can_parse_semicolon();
        }
        false
    }

    // ────────────────────────────────────────────────────────────────────────
    // Pragmas
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func getCommentPragmas(f *ast.NodeFactory, sourceText string) (pragmas []ast.Pragma)`.
    /// PORT: Go's `*NodeFactory` parameter exists only for NewCommentRange;
    /// the Rust free function needs no factory.
    pub(crate) fn get_comment_pragmas(&self, source_text: &str) -> Vec<tsc_ast::Pragma> {
        let mut pragmas = Vec::new();
        for comment_range in crate::comment_ranges::get_leading_comment_ranges(source_text, 0) {
            let comment =
                &source_text[comment_range.loc.pos() as usize..comment_range.loc.end() as usize];
            pragmas.extend(extract_pragmas(&comment_range, comment));
        }
        pragmas
    }

    /// Go: `func (p *Parser) processPragmasIntoFields(context *ast.SourceFile)`.
    fn process_pragmas_into_fields(&mut self, context: NodeId) {
        let pragmas: Vec<tsc_ast::Pragma> = {
            let s = self.factory.store();
            s.node(context)
                .as_source_file()
                .expect("SourceFile data")
                .pragmas
                .clone()
        };
        {
            let s = self.factory.store();
            let d = s
                .node_mut(context)
                .as_source_file_mut()
                .expect("SourceFile data");
            d.check_js_directive = None;
            d.referenced_files = Vec::new();
            d.type_reference_directives = Vec::new();
            d.lib_reference_directives = Vec::new();
        }
        for pragma in pragmas {
            match pragma.name.as_str() {
                "reference" => {
                    let types = pragma.args.get("types").cloned();
                    let lib = pragma.args.get("lib").cloned();
                    let path = pragma.args.get("path").cloned();
                    let resolution_mode = pragma.args.get("resolution-mode").cloned();
                    let preserve = pragma.args.get("preserve").cloned();
                    let no_default_lib = pragma.args.get("no-default-lib").cloned();
                    if no_default_lib.is_some_and(|v| v.value == "true") {
                        // Ignored.
                    } else if let Some(types) = types {
                        let parsed = if let Some(resolution_mode) = resolution_mode {
                            self.parse_resolution_mode(
                                &resolution_mode.value,
                                resolution_mode.loc.pos(),
                                resolution_mode.loc.end(),
                            )
                        } else {
                            tsc_core::compileroptions::ResolutionModeNone
                        };
                        let preserve = preserve.is_some_and(|v| v.value == "true");
                        let s = self.factory.store();
                        s.node_mut(context)
                            .as_source_file_mut()
                            .expect("SourceFile data")
                            .type_reference_directives
                            .push(tsc_ast::FileReference {
                                loc: types.loc,
                                file_name: types.value,
                                mode: parsed,
                                preserve,
                            });
                    } else if let Some(lib) = lib {
                        let preserve = preserve.is_some_and(|v| v.value == "true");
                        let s = self.factory.store();
                        s.node_mut(context)
                            .as_source_file_mut()
                            .expect("SourceFile data")
                            .lib_reference_directives
                            .push(tsc_ast::FileReference {
                                loc: lib.loc,
                                file_name: lib.value,
                                mode: tsc_core::compileroptions::ResolutionModeNone,
                                preserve,
                            });
                    } else if let Some(path) = path {
                        let preserve = preserve.is_some_and(|v| v.value == "true");
                        let s = self.factory.store();
                        s.node_mut(context)
                            .as_source_file_mut()
                            .expect("SourceFile data")
                            .referenced_files
                            .push(tsc_ast::FileReference {
                                loc: path.loc,
                                file_name: path.value,
                                mode: tsc_core::compileroptions::ResolutionModeNone,
                                preserve,
                            });
                    } else {
                        self.parse_error_at_range(
                            pragma.loc,
                            &tsc_diagnostics::INVALID_REFERENCE_DIRECTIVE_SYNTAX,
                            &[],
                        );
                    }
                }
                "ts-check" | "ts-nocheck" => {
                    // _last_ of either nocheck or check in a file is the "winner"
                    let replace = {
                        let s = self.factory.store();
                        let d = s.node(context).as_source_file().expect("SourceFile data");
                        d.check_js_directive
                            .as_ref()
                            .is_none_or(|c| pragma.loc.pos() > c.range.loc.pos())
                    };
                    if replace {
                        let s = self.factory.store();
                        s.node_mut(context)
                            .as_source_file_mut()
                            .expect("SourceFile data")
                            .check_js_directive = Some(tsc_ast::CheckJsDirective {
                            enabled: pragma.name == "ts-check",
                            range: tsc_ast::CommentRange {
                                loc: pragma.loc,
                                kind: pragma.range_kind,
                                has_trailing_new_line: pragma.has_trailing_new_line,
                            },
                        });
                    }
                }
                "jsx" | "jsxfrag" | "jsximportsource" | "jsxruntime" => {
                    // Nothing to do here
                }
                unknown => panic!("Unhandled pragma kind: {}", unknown),
            }
        }
    }

    /// Go: `func (p *Parser) parseResolutionMode(mode string, pos int, end int) (resolutionKind core.ResolutionMode)`.
    fn parse_resolution_mode(
        &mut self,
        mode: &str,
        pos: i32,
        end: i32,
    ) -> tsc_core::compileroptions::ResolutionMode {
        if mode == "import" {
            return tsc_core::compileroptions::ResolutionModeESM;
        }
        if mode == "require" {
            return tsc_core::compileroptions::ResolutionModeCommonJS;
        }
        self.parse_error_at(
            pos,
            end,
            &tsc_diagnostics::X_RESOLUTION_MODE_SHOULD_BE_EITHER_REQUIRE_OR_IMPORT,
            &[],
        );
        tsc_core::compileroptions::ResolutionModeNone
    }

    // ────────────────────────────────────────────────────────────────────────
    // JS syntax checks
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) jsErrorAtRange(loc core.TextRange, message *diagnostics.Message, args ...any)`.
    fn js_error_at_range(&mut self, loc: TextRange, message: &'static Message, args: &[String]) {
        let diagnostic = new_diagnostic(
            TextRange::new(
                tsc_scanner::skip_trivia(self.source_text, loc.pos()),
                loc.end(),
            ),
            message,
            args,
        );
        self.sink.borrow_mut().js_diagnostics.push(diagnostic);
    }

    /// Go: `func (p *Parser) checkJSDecoratorSyntax(node *ast.Node)`.
    fn check_js_decorator_syntax(&mut self, node: NodeId) {
        let modifiers: Vec<NodeId> = {
            let s = self.factory.store();
            modifier_nodes_of(s, node)
        };
        if modifiers.is_empty() {
            return;
        }
        let can_have_illegal = {
            let s = self.factory.store();
            can_have_illegal_decorators(s.node(node))
        };
        if can_have_illegal {
            for modifier in modifiers {
                let is_decorator = {
                    let s = self.factory.store();
                    tsc_ast::is_decorator(s.node(modifier))
                };
                if is_decorator {
                    let loc = {
                        let s = self.factory.store();
                        s.node(modifier).loc
                    };
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::DECORATORS_ARE_NOT_VALID_HERE,
                        &[],
                    );
                    break;
                }
            }
        } else {
            let can_have = {
                let s = self.factory.store();
                can_have_decorators(s.node(node))
            };
            if can_have {
                let decorator_index = modifiers.iter().position(|&m| {
                    let s = self.factory.store();
                    tsc_ast::is_decorator(s.node(m))
                });
                if let Some(decorator_index) = decorator_index {
                    let node_kind = {
                        let s = self.factory.store();
                        s.node(node).kind
                    };
                    if node_kind == Kind::ClassDeclaration {
                        let export_index = modifiers.iter().position(|&m| {
                            let s = self.factory.store();
                            is_export_modifier(s.node(m))
                        });
                        if let Some(export_index) = export_index {
                            let default_index = modifiers.iter().position(|&m| {
                                let s = self.factory.store();
                                s.node(m).kind == Kind::DefaultKeyword
                            });
                            if decorator_index > export_index
                                && default_index
                                    .is_some_and(|default_index| decorator_index < default_index)
                            {
                                // Decorator between `export` and `default`
                                let loc = {
                                    let s = self.factory.store();
                                    s.node(modifiers[decorator_index]).loc
                                };
                                self.js_error_at_range(
                                    loc,
                                    &tsc_diagnostics::DECORATORS_ARE_NOT_VALID_HERE,
                                    &[],
                                );
                            } else if decorator_index < export_index {
                                // Find a trailing decorator after the export keyword
                                let mut trailing_decorator_index = None;
                                for (i, &m) in
                                    modifiers.iter().enumerate().skip(export_index)
                                {
                                    let is_decorator = {
                                        let s = self.factory.store();
                                        tsc_ast::is_decorator(s.node(m))
                                    };
                                    if is_decorator {
                                        trailing_decorator_index = Some(i);
                                        break;
                                    }
                                }
                                if let Some(trailing_decorator_index) = trailing_decorator_index {
                                    let (trailing_loc, decorator_loc) = {
                                        let s = self.factory.store();
                                        (
                                            s.node(modifiers[trailing_decorator_index]).loc,
                                            s.node(modifiers[decorator_index]).loc,
                                        )
                                    };
                                    let mut diag = new_diagnostic(
                                        TextRange::new(
                                            tsc_scanner::skip_trivia(self.source_text, trailing_loc.pos()),
                                            trailing_loc.end(),
                                        ),
                                        &tsc_diagnostics::DECORATORS_MAY_NOT_APPEAR_AFTER_EXPORT_OR_EXPORT_DEFAULT_IF_THEY_ALSO_APPEAR_BEFORE_EXPORT,
                                        &[],
                                    );
                                    diag.add_related_info(new_diagnostic(
                                        TextRange::new(
                                            tsc_scanner::skip_trivia(
                                                self.source_text,
                                                decorator_loc.pos(),
                                            ),
                                            decorator_loc.end(),
                                        ),
                                        &tsc_diagnostics::DECORATOR_USED_BEFORE_EXPORT_HERE,
                                        &[],
                                    ));
                                    self.sink.borrow_mut().js_diagnostics.push(diag);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Go: `func (p *Parser) checkJSSyntax(node *ast.Node) *ast.Node`.
    pub(crate) fn check_js_syntax(&mut self, node: NodeId) -> NodeId {
        let (kind, flags, loc) = {
            let s = self.factory.store();
            let n = s.node(node);
            (n.kind, n.flags, n.loc)
        };
        if !flags.intersects(NodeFlags::JAVASCRIPT_FILE)
            || flags.intersects(NodeFlags::JSDOC.union(NodeFlags::REPARSED))
        {
            return node;
        }
        match kind {
            Kind::Parameter | Kind::PropertyDeclaration | Kind::MethodDeclaration => {
                let (question_kind, _question_flags, question_loc, question_reparsed) = match kind {
                    Kind::Parameter => {
                        let s = self.factory.store();
                        let d = s
                            .node(node)
                            .as_parameter_declaration()
                            .expect("ParameterDeclaration data");
                        match d.question_token {
                            Some(t) => (
                                s.node(t).kind,
                                s.node(t).flags,
                                s.node(t).loc,
                                s.node(t).flags.intersects(NodeFlags::REPARSED),
                            ),
                            None => (Kind::Unknown, NodeFlags::NONE, loc, false),
                        }
                    }
                    _ => {
                        // PropertyDeclaration / MethodDeclaration store the
                        // optional marker in PostfixToken.
                        let s = self.factory.store();
                        let postfix = postfix_token_of(s, node);
                        match postfix {
                            Some(t) => (
                                s.node(t).kind,
                                s.node(t).flags,
                                s.node(t).loc,
                                s.node(t).flags.intersects(NodeFlags::REPARSED),
                            ),
                            None => (Kind::Unknown, NodeFlags::NONE, loc, false),
                        }
                    }
                };
                if question_kind == Kind::QuestionToken && !question_reparsed {
                    self.js_error_at_range(
                        question_loc,
                        &tsc_diagnostics::THE_0_MODIFIER_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &["?".to_string()],
                    );
                }
                self.check_js_signature_or_type(node, kind, loc);
            }
            Kind::MethodSignature
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionExpression
            | Kind::FunctionDeclaration
            | Kind::ArrowFunction
            | Kind::VariableDeclaration
            | Kind::IndexSignature => {
                self.check_js_signature_or_type(node, kind, loc);
            }
            Kind::ImportDeclaration => {
                let is_type_only = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_import_declaration()
                        .expect("ImportDeclaration data");
                    d.import_clause
                        .is_some_and(|clause| import_clause_is_type_only(s, clause))
                };
                if is_type_only {
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &["import type".to_string()],
                    );
                }
            }
            Kind::ExportDeclaration => {
                let is_type_only = {
                    let s = self.factory.store();
                    s.node(node)
                        .as_export_declaration()
                        .expect("ExportDeclaration data")
                        .is_type_only
                };
                if is_type_only {
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &["export type".to_string()],
                    );
                }
            }
            Kind::ImportSpecifier => {
                let is_type_only = {
                    let s = self.factory.store();
                    s.node(node)
                        .as_import_specifier()
                        .expect("ImportSpecifier data")
                        .is_type_only
                };
                if is_type_only {
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &["import...type".to_string()],
                    );
                }
            }
            Kind::ExportSpecifier => {
                let is_type_only = {
                    let s = self.factory.store();
                    s.node(node)
                        .as_export_specifier()
                        .expect("ExportSpecifier data")
                        .is_type_only
                };
                if is_type_only {
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &["export...type".to_string()],
                    );
                }
            }
            Kind::ImportEqualsDeclaration => {
                self.js_error_at_range(
                    loc,
                    &tsc_diagnostics::X_IMPORT_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[],
                );
            }
            Kind::ExportAssignment => {
                let is_export_equals = {
                    let s = self.factory.store();
                    s.node(node)
                        .as_export_assignment()
                        .expect("ExportAssignment data")
                        .is_export_equals
                };
                if is_export_equals {
                    self.js_error_at_range(
                        loc,
                        &tsc_diagnostics::X_EXPORT_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &[],
                    );
                }
            }
            Kind::HeritageClause => {
                let (token, clause_loc) = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_heritage_clause()
                        .expect("HeritageClause data");
                    (d.token, s.node(node).loc)
                };
                if token == Kind::ImplementsKeyword {
                    self.js_error_at_range(
                        clause_loc,
                        &tsc_diagnostics::X_IMPLEMENTS_CLAUSES_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &[],
                    );
                }
            }
            Kind::InterfaceDeclaration => {
                let name_loc = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_interface_declaration()
                        .expect("InterfaceDeclaration data");
                    s.node(d.name).loc
                };
                self.js_error_at_range(
                    name_loc,
                    &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &["interface".to_string()],
                );
            }
            Kind::ModuleDeclaration => {
                let (keyword, name_loc) = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_module_declaration()
                        .expect("ModuleDeclaration data");
                    (d.keyword, s.node(d.name).loc)
                };
                self.js_error_at_range(
                    name_loc,
                    &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[token_to_text(keyword).to_string()],
                );
            }
            Kind::TypeAliasDeclaration => {
                let name_loc = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_type_alias_declaration()
                        .expect("TypeAliasDeclaration data");
                    s.node(d.name).loc
                };
                self.js_error_at_range(
                    name_loc,
                    &tsc_diagnostics::TYPE_ALIASES_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[],
                );
            }
            Kind::EnumDeclaration => {
                let name_loc = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_enum_declaration()
                        .expect("EnumDeclaration data");
                    s.node(d.name).loc
                };
                self.js_error_at_range(
                    name_loc,
                    &tsc_diagnostics::X_0_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &["enum".to_string()],
                );
            }
            Kind::NonNullExpression => {
                self.js_error_at_range(
                    loc,
                    &tsc_diagnostics::NON_NULL_ASSERTIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[],
                );
            }
            Kind::AsExpression => {
                let type_loc = {
                    let s = self.factory.store();
                    let d = s.node(node).as_as_expression().expect("AsExpression data");
                    s.node(d.type_).loc
                };
                self.js_error_at_range(type_loc, &tsc_diagnostics::TYPE_ASSERTION_EXPRESSIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES, &[]);
            }
            Kind::SatisfiesExpression => {
                let type_loc = {
                    let s = self.factory.store();
                    let d = s
                        .node(node)
                        .as_satisfies_expression()
                        .expect("SatisfiesExpression data");
                    s.node(d.type_).loc
                };
                self.js_error_at_range(type_loc, &tsc_diagnostics::TYPE_SATISFACTION_EXPRESSIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES, &[]);
            }
            _ => {}
        }
        // Check decorator placement in JS files
        self.check_js_decorator_syntax(node);
        // Check absence of type parameters, type arguments and non-JavaScript modifiers
        match kind {
            Kind::ClassDeclaration
            | Kind::ClassExpression
            | Kind::MethodDeclaration
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
            | Kind::FunctionExpression
            | Kind::FunctionDeclaration
            | Kind::ArrowFunction => {
                let (type_parameter_list, modifier_nodes) = {
                    let s = self.factory.store();
                    (type_parameter_list_of(s, node), modifier_nodes_of(s, node))
                };
                if let Some(list) = type_parameter_list {
                    let has_non_reparsed = list.nodes.iter().any(|&n| {
                        let s = self.factory.store();
                        !s.node(n).flags.intersects(NodeFlags::REPARSED)
                    });
                    if has_non_reparsed {
                        self.js_error_at_range(
                            list.loc,
                            &tsc_diagnostics::TYPE_PARAMETER_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                            &[],
                        );
                    }
                }
                for modifier in modifier_nodes {
                    let (modifier_flags, modifier_kind, modifier_loc, modifier_is_decorator) = {
                        let s = self.factory.store();
                        let n = s.node(modifier);
                        (
                            n.flags.intersects(NodeFlags::REPARSED),
                            n.kind,
                            n.loc,
                            n.kind == Kind::Decorator,
                        )
                    };
                    if !modifier_flags
                        && !modifier_is_decorator
                        && tsc_ast::visitor::modifier_to_flag(modifier_kind)
                            .intersects(tsc_ast::ModifierFlags::JAVASCRIPT)
                    {
                        self.js_error_at_range(
                            modifier_loc,
                            &tsc_diagnostics::THE_0_MODIFIER_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                            &[token_to_text(modifier_kind).to_string()],
                        );
                    }
                }
            }
            Kind::VariableStatement | Kind::PropertyDeclaration => {
                let modifier_nodes = {
                    let s = self.factory.store();
                    modifier_nodes_of(s, node)
                };
                for modifier in modifier_nodes {
                    let (is_reparsed, modifier_kind, modifier_loc) = {
                        let s = self.factory.store();
                        let n = s.node(modifier);
                        (n.flags.intersects(NodeFlags::REPARSED), n.kind, n.loc)
                    };
                    if !is_reparsed
                        && modifier_kind != Kind::Decorator
                        && tsc_ast::visitor::modifier_to_flag(modifier_kind)
                            .intersects(tsc_ast::ModifierFlags::JAVASCRIPT)
                    {
                        self.js_error_at_range(
                            modifier_loc,
                            &tsc_diagnostics::THE_0_MODIFIER_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                            &[token_to_text(modifier_kind).to_string()],
                        );
                    }
                }
            }
            Kind::Parameter => {
                let has_modifier = {
                    let s = self.factory.store();
                    modifier_nodes_of(s, node)
                        .iter()
                        .any(|&m| is_modifier(s.node(m)))
                };
                if has_modifier {
                    let modifiers_loc = {
                        let s = self.factory.store();
                        let d = s
                            .node(node)
                            .as_parameter_declaration()
                            .expect("ParameterDeclaration data");
                        d.modifiers.as_ref().expect("has modifiers").loc
                    };
                    self.js_error_at_range(
                        modifiers_loc,
                        &tsc_diagnostics::PARAMETER_MODIFIERS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                        &[],
                    );
                }
            }
            Kind::CallExpression
            | Kind::NewExpression
            | Kind::ExpressionWithTypeArguments
            | Kind::JsxSelfClosingElement
            | Kind::JsxOpeningElement
            | Kind::TaggedTemplateExpression => {
                let list = {
                    let s = self.factory.store();
                    type_argument_list_of(s, node)
                };
                if let Some(list) = list {
                    let has_non_reparsed = list.nodes.iter().any(|&n| {
                        let s = self.factory.store();
                        !s.node(n).flags.intersects(NodeFlags::REPARSED)
                    });
                    if has_non_reparsed {
                        self.js_error_at_range(
                            list.loc,
                            &tsc_diagnostics::TYPE_ARGUMENTS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                            &[],
                        );
                    }
                }
            }
            _ => {}
        }
        node
    }

    /// The shared `IsFunctionLike && Body() == nil` / `Type() != nil` check
    /// from Go checkJSSyntax's fallthrough arms.
    fn check_js_signature_or_type(&mut self, node: NodeId, kind: Kind, loc: TextRange) {
        let is_function_like = {
            let s = self.factory.store();
            is_function_like(s.node(node))
        };
        if is_function_like {
            let body = {
                let s = self.factory.store();
                body_of(s, node)
            };
            if body.is_none() {
                self.js_error_at_range(
                    loc,
                    &tsc_diagnostics::SIGNATURE_DECLARATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[],
                );
                return;
            }
        }
        let type_node = {
            let s = self.factory.store();
            type_of(s, node)
        };
        if let Some(t) = type_node {
            let (t_flags, t_loc) = {
                let s = self.factory.store();
                let n = s.node(t);
                (n.flags.intersects(NodeFlags::REPARSED), n.loc)
            };
            if !t_flags {
                self.js_error_at_range(
                    t_loc,
                    &tsc_diagnostics::TYPE_ANNOTATIONS_CAN_ONLY_BE_USED_IN_TYPESCRIPT_FILES,
                    &[],
                );
            }
        }
        let _ = kind;
    }
}

/// Go `node.ModifierNodes()` (PORT: dedup candidate into the ast accessor set).
pub(crate) fn modifier_nodes_of(s: &dyn NodeStore, node: NodeId) -> Vec<NodeId> {
    let n = s.node(node);
    let modifiers = match n.kind {
        Kind::VariableStatement => n
            .as_variable_statement()
            .expect("VariableStatement data")
            .modifiers
            .as_ref(),
        Kind::Parameter => n
            .as_parameter_declaration()
            .expect("ParameterDeclaration data")
            .modifiers
            .as_ref(),
        Kind::PropertyDeclaration => n
            .as_property_declaration()
            .expect("PropertyDeclaration data")
            .modifiers
            .as_ref(),
        Kind::MethodDeclaration => n
            .as_method_declaration()
            .expect("MethodDeclaration data")
            .modifiers
            .as_ref(),
        Kind::Constructor => n
            .as_constructor_declaration()
            .expect("ConstructorDeclaration data")
            .modifiers
            .as_ref(),
        Kind::GetAccessor => n
            .as_get_accessor_declaration()
            .expect("GetAccessorDeclaration data")
            .modifiers
            .as_ref(),
        Kind::SetAccessor => n
            .as_set_accessor_declaration()
            .expect("SetAccessorDeclaration data")
            .modifiers
            .as_ref(),
        Kind::FunctionDeclaration => n
            .as_function_declaration()
            .expect("FunctionDeclaration data")
            .modifiers
            .as_ref(),
        Kind::FunctionExpression => n
            .as_function_expression()
            .expect("FunctionExpression data")
            .modifiers
            .as_ref(),
        Kind::ArrowFunction => n
            .as_arrow_function()
            .expect("ArrowFunction data")
            .modifiers
            .as_ref(),
        Kind::ClassDeclaration => n
            .as_class_declaration()
            .expect("ClassDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ClassExpression => n
            .as_class_expression()
            .expect("ClassExpression data")
            .modifiers
            .as_ref(),
        Kind::InterfaceDeclaration => n
            .as_interface_declaration()
            .expect("InterfaceDeclaration data")
            .modifiers
            .as_ref(),
        Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => n
            .as_type_alias_declaration()
            .expect("TypeAliasDeclaration data")
            .modifiers
            .as_ref(),
        Kind::EnumDeclaration => n
            .as_enum_declaration()
            .expect("EnumDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ModuleDeclaration => n
            .as_module_declaration()
            .expect("ModuleDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ImportDeclaration => n
            .as_import_declaration()
            .expect("ImportDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ImportEqualsDeclaration => n
            .as_import_equals_declaration()
            .expect("ImportEqualsDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ExportDeclaration => n
            .as_export_declaration()
            .expect("ExportDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ExportAssignment => n
            .as_export_assignment()
            .expect("ExportAssignment data")
            .modifiers
            .as_ref(),
        Kind::NamespaceExportDeclaration => n
            .as_namespace_export_declaration()
            .expect("NamespaceExportDeclaration data")
            .modifiers
            .as_ref(),
        Kind::IndexSignature => n
            .as_index_signature_declaration()
            .expect("IndexSignatureDeclaration data")
            .modifiers
            .as_ref(),
        Kind::TypeParameter => n
            .as_type_parameter_declaration()
            .expect("TypeParameterDeclaration data")
            .modifiers
            .as_ref(),
        Kind::PropertySignature => n
            .as_property_signature_declaration()
            .expect("PropertySignatureDeclaration data")
            .modifiers
            .as_ref(),
        Kind::MethodSignature => n
            .as_method_signature_declaration()
            .expect("MethodSignatureDeclaration data")
            .modifiers
            .as_ref(),
        Kind::MissingDeclaration => n
            .as_missing_declaration()
            .expect("MissingDeclaration data")
            .modifiers
            .as_ref(),
        Kind::ClassStaticBlockDeclaration => n
            .as_class_static_block_declaration()
            .expect("ClassStaticBlockDeclaration data")
            .modifiers
            .as_ref(),
        Kind::PropertyAssignment => n
            .as_property_assignment()
            .expect("PropertyAssignment data")
            .modifiers
            .as_ref(),
        Kind::ShorthandPropertyAssignment => n
            .as_shorthand_property_assignment()
            .expect("ShorthandPropertyAssignment data")
            .modifiers
            .as_ref(),
        _ => None,
    };
    modifiers.map(|m| m.nodes.to_vec()).unwrap_or_default()
}

/// Go `node.PostfixToken()` (PORT: dedup candidate).
pub(crate) fn postfix_token_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        Kind::MethodDeclaration => {
            n.as_method_declaration()
                .expect("MethodDeclaration data")
                .postfix_token
        }
        Kind::ShorthandPropertyAssignment => {
            n.as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment data")
                .postfix_token
        }
        Kind::MethodSignature => {
            n.as_method_signature_declaration()
                .expect("MethodSignatureDeclaration data")
                .postfix_token
        }
        Kind::PropertySignature => {
            n.as_property_signature_declaration()
                .expect("PropertySignatureDeclaration data")
                .postfix_token
        }
        Kind::PropertyAssignment => {
            n.as_property_assignment()
                .expect("PropertyAssignment data")
                .postfix_token
        }
        Kind::PropertyDeclaration => {
            n.as_property_declaration()
                .expect("PropertyDeclaration data")
                .postfix_token
        }
        Kind::EnumMember => n.as_enum_member().expect("EnumMember data").postfix_token,
        Kind::GetAccessor => {
            n.as_get_accessor_declaration()
                .expect("GetAccessorDeclaration data")
                .postfix_token
        }
        Kind::SetAccessor => {
            n.as_set_accessor_declaration()
                .expect("SetAccessorDeclaration data")
                .postfix_token
        }
        _ => None,
    }
}

/// Go `node.QuestionToken()` (PORT: dedup candidate; unused by this wave's
/// grammar — allow(dead_code) until the remaining ast accessor surface lands).
#[allow(dead_code)]
pub(crate) fn question_token_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        Kind::Parameter => {
            n.as_parameter_declaration()
                .expect("ParameterDeclaration data")
                .question_token
        }
        Kind::ConditionalExpression => {
            let token = n
                .as_conditional_expression()
                .expect("ConditionalExpression data")
                .question_token;
            (token != NodeId::NONE).then_some(token)
        }
        Kind::MappedType => {
            n.as_mapped_type_node()
                .expect("MappedTypeNode data")
                .question_token
        }
        Kind::NamedTupleMember => {
            n.as_named_tuple_member()
                .expect("NamedTupleMember data")
                .question_token
        }
        _ => postfix_token_of(s, node).filter(|t| s.node(*t).kind == Kind::QuestionToken),
    }
}

/// Go `node.Body()` (PORT: dedup candidate).
pub(crate) fn body_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        Kind::Constructor => Some(
            n.as_constructor_declaration()
                .expect("ConstructorDeclaration data")
                .body?,
        ),
        Kind::MethodDeclaration => Some(
            n.as_method_declaration()
                .expect("MethodDeclaration data")
                .body?,
        ),
        Kind::GetAccessor => Some(
            n.as_get_accessor_declaration()
                .expect("GetAccessorDeclaration data")
                .body?,
        ),
        Kind::SetAccessor => Some(
            n.as_set_accessor_declaration()
                .expect("SetAccessorDeclaration data")
                .body?,
        ),
        Kind::FunctionDeclaration => Some(
            n.as_function_declaration()
                .expect("FunctionDeclaration data")
                .body?,
        ),
        Kind::FunctionExpression => Some(
            n.as_function_expression()
                .expect("FunctionExpression data")
                .body?,
        ),
        Kind::ArrowFunction => Some(n.as_arrow_function().expect("ArrowFunction data").body?),
        Kind::ClassStaticBlockDeclaration => Some(
            n.as_class_static_block_declaration()
                .expect("ClassStaticBlockDeclaration data")
                .body,
        ),
        _ => None,
    }
}

/// Go `node.Type()` (parser subset; PORT: dedup candidate).
pub(crate) fn type_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = s.node(node);
    match n.kind {
        Kind::VariableDeclaration => Some(
            n.as_variable_declaration()
                .expect("VariableDeclaration data")
                .type_?,
        ),
        Kind::Parameter => Some(
            n.as_parameter_declaration()
                .expect("ParameterDeclaration data")
                .type_?,
        ),
        Kind::PropertySignature => Some(
            n.as_property_signature_declaration()
                .expect("PropertySignatureDeclaration data")
                .type_,
        ),
        Kind::PropertyDeclaration => Some(
            n.as_property_declaration()
                .expect("PropertyDeclaration data")
                .type_?,
        ),
        Kind::AsExpression => Some(n.as_as_expression().expect("AsExpression data").type_),
        Kind::SatisfiesExpression => Some(
            n.as_satisfies_expression()
                .expect("SatisfiesExpression data")
                .type_,
        ),
        _ => None,
    }
}

/// Go `node.TypeParameterList()` (parser subset; PORT: dedup candidate).
pub(crate) fn type_parameter_list_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeList> {
    let n = s.node(node);
    match n.kind {
        Kind::ClassDeclaration => n
            .as_class_declaration()
            .expect("ClassDeclaration data")
            .type_parameters
            .clone(),
        Kind::ClassExpression => n
            .as_class_expression()
            .expect("ClassExpression data")
            .type_parameters
            .clone(),
        Kind::InterfaceDeclaration => n
            .as_interface_declaration()
            .expect("InterfaceDeclaration data")
            .type_parameters
            .clone(),
        Kind::TypeAliasDeclaration | Kind::JSTypeAliasDeclaration => n
            .as_type_alias_declaration()
            .expect("TypeAliasDeclaration data")
            .type_parameters
            .clone(),
        Kind::FunctionDeclaration => n
            .as_function_declaration()
            .expect("FunctionDeclaration data")
            .type_parameters
            .clone(),
        Kind::FunctionExpression => n
            .as_function_expression()
            .expect("FunctionExpression data")
            .type_parameters
            .clone(),
        Kind::ArrowFunction => n
            .as_arrow_function()
            .expect("ArrowFunction data")
            .type_parameters
            .clone(),
        Kind::MethodDeclaration => n
            .as_method_declaration()
            .expect("MethodDeclaration data")
            .type_parameters
            .clone(),
        Kind::Constructor => n
            .as_constructor_declaration()
            .expect("ConstructorDeclaration data")
            .type_parameters
            .clone(),
        Kind::GetAccessor => n
            .as_get_accessor_declaration()
            .expect("GetAccessorDeclaration data")
            .type_parameters
            .clone(),
        Kind::SetAccessor => n
            .as_set_accessor_declaration()
            .expect("SetAccessorDeclaration data")
            .type_parameters
            .clone(),
        Kind::MethodSignature => n
            .as_method_signature_declaration()
            .expect("MethodSignatureDeclaration data")
            .type_parameters
            .clone(),
        Kind::CallSignature => n
            .as_call_signature_declaration()
            .expect("CallSignatureDeclaration data")
            .type_parameters
            .clone(),
        Kind::ConstructSignature => n
            .as_construct_signature_declaration()
            .expect("ConstructSignatureDeclaration data")
            .type_parameters
            .clone(),
        _ => None,
    }
}

/// Go `node.TypeArgumentList()` (parser subset; PORT: dedup candidate).
pub(crate) fn type_argument_list_of(s: &dyn NodeStore, node: NodeId) -> Option<NodeList> {
    let n = s.node(node);
    match n.kind {
        Kind::CallExpression => n
            .as_call_expression()
            .expect("CallExpression data")
            .type_arguments
            .clone(),
        Kind::NewExpression => n
            .as_new_expression()
            .expect("NewExpression data")
            .type_arguments
            .clone(),
        Kind::TaggedTemplateExpression => n
            .as_tagged_template_expression()
            .expect("TaggedTemplateExpression data")
            .type_arguments
            .clone(),
        Kind::TypeReference => n
            .as_type_reference_node()
            .expect("TypeReferenceNode data")
            .type_arguments
            .clone(),
        Kind::ExpressionWithTypeArguments => n
            .as_expression_with_type_arguments()
            .expect("ExpressionWithTypeArguments data")
            .type_arguments
            .clone(),
        Kind::ImportType => n
            .as_import_type_node()
            .expect("ImportTypeNode data")
            .type_arguments
            .clone(),
        Kind::TypeQuery => n
            .as_type_query_node()
            .expect("TypeQueryNode data")
            .type_arguments
            .clone(),
        Kind::JsxOpeningElement => n
            .as_jsx_opening_element()
            .expect("JsxOpeningElement data")
            .type_arguments
            .clone(),
        Kind::JsxSelfClosingElement => n
            .as_jsx_self_closing_element()
            .expect("JsxSelfClosingElement data")
            .type_arguments
            .clone(),
        _ => None,
    }
}

/// Go `ImportClause.IsTypeOnly()` — the phase modifier is `type`.
pub(crate) fn import_clause_is_type_only(s: &dyn NodeStore, clause: NodeId) -> bool {
    let n = s.node(clause);
    if n.kind != Kind::ImportClause {
        return false;
    }
    let d = n.as_import_clause().expect("ImportClause data");
    d.phase_modifier == Some(Kind::TypeKeyword)
}

// ────────────────────────────────────────────────────────────────────────────
// extractPragmas helpers (parser.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func extractPragmas(commentRange ast.CommentRange, text string) []ast.Pragma`.
fn extract_pragmas(comment_range: &tsc_ast::CommentRange, text: &str) -> Vec<tsc_ast::Pragma> {
    if comment_range.kind == Kind::SingleLineCommentTrivia {
        let mut pos = 2;
        let triple_slash = match_at(text, pos, "/");
        if triple_slash {
            pos += 1;
        }
        pos = skip_blanks(text, pos);
        if triple_slash && match_at(text, pos, "<") {
            let tag_name = extract_name(text, pos + 1);
            if tag_name != "reference" {
                return Vec::new();
            }
            pos += 10;
            let mut args = std::collections::HashMap::new();
            loop {
                pos = skip_blanks(text, pos);
                if match_at(text, pos, "/>") {
                    break;
                }
                let arg_name = extract_name(text, pos);
                if arg_name.is_empty() {
                    break;
                }
                pos = skip_blanks(text, pos + arg_name.len());
                if !match_at(text, pos, "=") {
                    break;
                }
                pos = skip_blanks(text, pos + 1);
                let Some((value, value_len)) = extract_quoted_string(text, pos) else {
                    break;
                };
                args.insert(
                    arg_name.clone(),
                    tsc_ast::PragmaArgument {
                        loc: TextRange::new(
                            comment_range.loc.pos() + pos as i32 + 1,
                            comment_range.loc.pos() + pos as i32 + 1 + value_len as i32,
                        ),
                        name: arg_name,
                        value,
                    },
                );
                pos += value_len + 2;
            }
            return vec![tsc_ast::Pragma {
                loc: comment_range.loc,
                range_kind: comment_range.kind,
                has_trailing_new_line: comment_range.has_trailing_new_line,
                name: "reference".to_string(),
                args,
            }];
        }
        if match_at(text, pos, "@") {
            pos += 1;
            let pragma_name = extract_name(text, pos);
            if !(pragma_name == "ts-check" || pragma_name == "ts-nocheck") {
                return Vec::new();
            }
            return vec![tsc_ast::Pragma {
                loc: comment_range.loc,
                range_kind: comment_range.kind,
                has_trailing_new_line: comment_range.has_trailing_new_line,
                name: pragma_name,
                args: std::collections::HashMap::new(),
            }];
        }
    }
    if comment_range.kind == Kind::MultiLineCommentTrivia {
        let text = text.strip_suffix("*/").unwrap_or(text);
        let mut pos = 2;
        let mut pragmas = Vec::new();
        loop {
            pos = match skip_to(text, pos, "@") {
                Some(p) => p,
                None => break,
            };
            // Mirrors the /@(\S+)(\s+(?:\S.*)?)?$/gm pragma regex used by
            // TypeScript: the '@' must be immediately followed by a
            // non-whitespace pragma name, and the remainder of the line is
            // consumed as that pragma's arguments. As a consequence, only the
            // first '@'-token on a line is considered, so an unrelated
            // '@token' earlier on the line (e.g. an email address) prevents a
            // later '@jsx' on the same line from being treated as a pragma.
            let name_pos = pos + 1;
            let name_end = skip_non_blanks(text, name_pos);
            if name_end == name_pos {
                pos += 1;
                continue;
            }
            let line_end = line_end_pos(text, pos);
            let pragma_name = text[name_pos..name_end].to_lowercase();
            if matches!(
                pragma_name.as_str(),
                "jsx" | "jsxfrag" | "jsximportsource" | "jsxruntime"
            ) {
                let start = skip_blanks(text, name_end);
                let arg_end = skip_non_blanks(text, start);
                if arg_end != start {
                    let mut args = std::collections::HashMap::with_capacity(1);
                    args.insert(
                        "factory".to_string(),
                        tsc_ast::PragmaArgument {
                            loc: TextRange::new(
                                comment_range.loc.pos() + start as i32,
                                comment_range.loc.pos() + arg_end as i32,
                            ),
                            name: "factory".to_string(),
                            value: text[start..arg_end].to_string(),
                        },
                    );
                    pragmas.push(tsc_ast::Pragma {
                        loc: comment_range.loc,
                        range_kind: comment_range.kind,
                        has_trailing_new_line: comment_range.has_trailing_new_line,
                        name: pragma_name,
                        args,
                    });
                }
            }
            pos = line_end;
        }
        return pragmas;
    }
    Vec::new()
}

/// Go: `func match(text string, pos int, s string) bool`.
fn match_at(text: &str, pos: usize, s: &str) -> bool {
    text[pos.min(text.len())..].starts_with(s)
}

/// Go: `func skipBlanks(text string, pos int) int`.
fn skip_blanks(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = pos;
    while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
        pos += 1;
    }
    pos
}

/// Go: `func skipNonBlanks(text string, pos int) int`.
fn skip_non_blanks(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = pos;
    while pos < bytes.len()
        && bytes[pos] != b' '
        && bytes[pos] != b'\t'
        && bytes[pos] != b'\r'
        && bytes[pos] != b'\n'
    {
        pos += 1;
    }
    pos
}

/// Go: `func skipTo(text string, pos int, s string) int` (−1 → `None`).
fn skip_to(text: &str, pos: usize, s: &str) -> Option<usize> {
    if pos >= text.len() {
        return None;
    }
    text[pos..].find(s).map(|i| pos + i)
}

/// Go: `func lineEndPos(text string, pos int) int`.
fn line_end_pos(text: &str, pos: usize) -> usize {
    for (i, ch) in text[pos..].char_indices() {
        if tsc_stringutil::is_line_break(ch) {
            return pos + i;
        }
    }
    text.len()
}

/// Go: `func extractName(text string, pos int) string`.
fn extract_name(text: &str, pos: usize) -> String {
    let bytes = text.as_bytes();
    let start = pos;
    let mut pos = pos;
    while pos < bytes.len()
        && (bytes[pos].is_ascii_uppercase()
            || bytes[pos].is_ascii_lowercase()
            || bytes[pos] == b'-')
    {
        pos += 1;
    }
    text[start..pos].to_lowercase()
}

/// Go: `func extractQuotedString(text string, pos int) (string, bool)` —
/// returns the value and its rune length.
fn extract_quoted_string(text: &str, pos: usize) -> Option<(String, usize)> {
    if pos == text.len() {
        return None;
    }
    let quote = text.as_bytes()[pos];
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let start = pos + 1;
    let mut pos = start;
    let bytes = text.as_bytes();
    while pos < bytes.len() && bytes[pos] != quote {
        pos += 1;
    }
    if pos == bytes.len() {
        return None;
    }
    Some((text[start..pos].to_string(), pos - start))
}
