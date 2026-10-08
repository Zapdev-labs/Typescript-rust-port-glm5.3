//! Go: `tsc/internal/parser/jsdoc.go` — the JSDoc comment parser.
//!
//! PORT: Go registers `parseJSDocForNode` with the ast package
//! (`ast.SetParseJSDocForNode`) to lazily parse JSDoc on first
//! `Node.JSDoc()` access in TS files. The Rust ast crate has no such global
//! hook, so lazy per-node JSDoc is unavailable until a follow-up wires an
//! equivalent seam; the eager paths (JS files, and TS comments containing
//! @see/@link) are fully implemented by `with_jsdoc`.
//!
//! Go's slice/string arena reuse (`jsdocCommentsSpace`, `stringSliceArena`)
//! is GC-era space optimization with no observable semantics; the port uses
//! plain `Vec`s.

use tsc_ast::{
    is_jsdoc_return_tag, is_jsdoc_type_tag, Diagnostic, Kind, NodeFlags, NodeId, NodeList,
    NodeStore,
};
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::Message;

use crate::parser::{
    is_reserved_word, new_diagnostic, JSDocInfo, JSDocScannerInfo, Parser,
    JSDOC_SCANNER_INFO_HAS_DEPRECATED, JSDOC_SCANNER_INFO_HAS_JSDOC,
    JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK,
};
use crate::token_is_identifier_or_keyword;
use crate::PC_JSDOC_COMMENT;

/// Go: `type jsdocState int32` + consts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JSDocState {
    BeginningOfLine,
    SawAsterisk,
    SavingComments,
    SavingBackticks,
}

/// Go: `type propertyLikeParse int32` + consts (bit flags).
pub(crate) type PropertyLikeParse = i32;
pub(crate) const PROPERTY_LIKE_PARSE_PROPERTY: PropertyLikeParse = 1;
pub(crate) const PROPERTY_LIKE_PARSE_PARAMETER: PropertyLikeParse = 2;
pub(crate) const PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER: PropertyLikeParse = 4;

/// Go: `func (p *Parser) withJSDoc(node *ast.Node, info jsdocScannerInfo) []*ast.Node`.
impl<'a, 'f> Parser<'a, 'f> {
    pub(crate) fn with_jsdoc(&mut self, node: NodeId, info: JSDocScannerInfo) -> Vec<NodeId> {
        if info & JSDOC_SCANNER_INFO_HAS_JSDOC == 0 {
            return Vec::new();
        }

        // For TS/TSX files, defer JSDoc parsing to first access, unless the comment
        // contains @see/@link (needed for unused-identifier checks).
        // @deprecated is detected via cheap text scan to set PossiblyContainsDeprecatedTag;
        // callers must confirm via JSDoc lookup.
        if !self.is_javascript() {
            {
                let s = self.factory.store();
                let n = s.node_mut(node);
                n.flags |= NodeFlags::HAS_JSDOC;
                if info & JSDOC_SCANNER_INFO_HAS_DEPRECATED != 0 {
                    n.flags |= NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG;
                }
            }
            if info & JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK == 0 {
                return Vec::new();
            }
            // Fall through to eager parse for @see/@link
        }

        let ranges = crate::get_jsdoc_comment_ranges(self.factory.store(), node, self.source_text);
        // PORT: Go reuses `jsdocCommentRangesSpace`; the port allocates.

        // Should only be called once per node
        self.has_deprecated_tag = false;
        let mut jsdoc: Vec<NodeId> = Vec::with_capacity(ranges.len());
        let mut pos: TextPos = {
            let s = self.factory.store();
            s.node(node).pos()
        };
        for comment in ranges {
            if let Some(parsed) =
                self.parse_jsdoc_comment(node, comment.loc.pos(), comment.loc.end(), pos)
            {
                {
                    let s = self.factory.store();
                    s.node_mut(parsed).parent.set(node);
                }
                pos = {
                    let s = self.factory.store();
                    s.node(parsed).end()
                };
                jsdoc.push(parsed);
            }
        }
        if !jsdoc.is_empty() {
            {
                let s = self.factory.store();
                let n = s.node_mut(node);
                if !n.flags.intersects(NodeFlags::HAS_JSDOC) {
                    n.flags |= NodeFlags::HAS_JSDOC;
                }
                if self.has_deprecated_tag {
                    n.flags |= NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG;
                }
            }
            self.has_deprecated_tag = false;
            if self.is_javascript() {
                self.reparse_tags(node, &jsdoc);
            }
            self.jsdoc_infos.push(JSDocInfo {
                parent: node,
                js_docs: jsdoc.clone(),
            });
            return jsdoc;
        }
        Vec::new()
    }

    /// Go: reparser.go `func (p *Parser) reparseTags(node *ast.Node, jsdoc []*ast.Node)`
    /// — the reparser.go wave is explicitly out of scope (see the lib.rs PORT
    /// banner); this is a no-op so the call sites stay identical.
    fn reparse_tags(&mut self, _node: NodeId, _jsdoc: &[NodeId]) {}

    /// Go: `func (p *Parser) parseJSDocTypeExpression(mayOmitBraces bool) *ast.Node`.
    fn parse_jsdoc_type_expression(&mut self, may_omit_braces: bool) -> NodeId {
        let pos = self.node_pos();

        let has_brace: bool = if may_omit_braces {
            self.parse_optional(Kind::OpenBraceToken)
        } else {
            self.parse_expected(Kind::OpenBraceToken)
        };
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::JSDOC, true);
        let t = self.parse_jsdoc_type();
        self.context_flags = save_context_flags;
        if has_brace {
            self.parse_expected_jsdoc(Kind::CloseBraceToken);
        }

        {
            let __hoist_1_2 = self.factory.new_jsdoc_type_expression(t);
            let __hoist_1_3 = pos;
            self.finish_node(__hoist_1_2, __hoist_1_3)
        }
    }

    /// Go: `func (p *Parser) parseJSDocNameReference() *ast.Node`.
    fn parse_jsdoc_name_reference(&mut self) -> NodeId {
        let pos = self.node_pos();
        let has_brace = self.parse_optional(Kind::OpenBraceToken);
        let entity_name = self.parse_jsdoc_link_name();
        if has_brace {
            self.parse_expected_jsdoc(Kind::CloseBraceToken);
        }
        self.scanner.reset_pos(self.scanner.token_full_start());
        self.next_token_jsdoc();
        // Go: NewJSDocNameReference(entityName) — the nil case (missing name
        // after a brace) maps to the NONE sentinel node handle.
        {
            let __hoist_1_5 = self
                .factory
                .new_jsdoc_name_reference(entity_name.unwrap_or(NodeId::NONE));
            let __hoist_1_6 = pos;
            self.finish_node(__hoist_1_5, __hoist_1_6)
        }
    }

    /// Go: `func (p *Parser) parseJSDocComment(parent *ast.Node, start, end, fullStart int) *ast.Node`.
    /// Pass end=-1 to parse the text to the end.
    fn parse_jsdoc_comment(
        &mut self,
        _parent: NodeId,
        start: i32,
        end: i32,
        full_start: i32,
    ) -> Option<NodeId> {
        let end = if end == -1 {
            self.source_text.len() as i32
        } else {
            end
        };
        // Check for /** (JSDoc opening part)
        if !crate::is_jsdoc_like_text(&self.source_text[start as usize..]) {
            // TODO(Go): This should be a panic, unless parseSingleJSDocComment is calling this (not ported yet)
            return None;
        }

        let save_source_text = self.source_text;
        let save_token = self.token;
        let save_context_flags = self.context_flags;
        let save_parsing_contexts = self.parsing_contexts;
        let save_scanner_state = self.scanner.mark();
        let save_diagnostics_length = self.sink.borrow().diagnostics.len();
        let save_has_parse_error = self.sink.borrow().has_parse_error;
        let save_has_await_identifier = self.statement_has_await_identifier;

        // initial indent is start+4 to account for leading `/** `
        // + 1 because \n is one character before the first character in the line and,
        // if there is no \n before start, -1 is one index before the first character in the string
        let initial_indent = start + 4
            - (save_source_text[..start as usize]
                .rfind('\n')
                .map(|i| i as i32)
                .unwrap_or(-1)
                + 1);
        // -2 for trailing `*/`
        // PORT: Go rebinds `p.sourceText` to the truncated comment; the port
        // re-points only the scanner (the parser's `source_text` readers are
        // index-based and provably unaffected, see parse_jsdoc_see_tag).
        self.scanner
            .set_text(&save_source_text[..(end - 2) as usize]);
        // +3 for leading `/**`
        self.scanner.reset_pos((start + 3) as usize);
        self.set_context_flags(NodeFlags::JSDOC, true);
        self.parsing_contexts |= 1 << PC_JSDOC_COMMENT;

        let comment = self.parse_jsdoc_comment_worker(start, end, full_start, initial_indent);
        // move jsdoc diagnostics to jsdocDiagnostics -- for JS files only
        {
            let mut sink = self.sink.borrow_mut();
            if self.context_flags.intersects(NodeFlags::JAVASCRIPT_FILE) {
                let drained: Vec<Diagnostic> =
                    sink.diagnostics.drain(save_diagnostics_length..).collect();
                sink.jsdoc_diagnostics.extend(drained);
            } else {
                sink.diagnostics.truncate(save_diagnostics_length);
            }
        }

        self.scanner.set_text(save_source_text);
        self.parsing_contexts = save_parsing_contexts;
        self.context_flags = save_context_flags;
        self.scanner.rewind(save_scanner_state);
        self.token = save_token;
        self.sink.borrow_mut().has_parse_error = save_has_parse_error;
        self.statement_has_await_identifier = save_has_await_identifier;

        Some(comment)
    }

    /// Go: `func (p *Parser) parseJSDocCommentWorker(start, end, fullStart, indent int) *ast.Node`.
    ///
    /// @param indent - the number of spaces to consider as the margin (applies to non-first lines only)
    fn parse_jsdoc_comment_worker(
        &mut self,
        start: i32,
        end: i32,
        full_start: i32,
        indent: i32,
    ) -> NodeId {
        // Initially we can parse out a tag.  We also have seen a starting asterisk.
        // This is so that /** * @type */ doesn't parse.
        let mut tags: Vec<NodeId> = Vec::with_capacity(1);
        let mut tags_pos: i32 = -1;
        let mut tags_end: i32 = -1;
        let mut state = JSDocState::SawAsterisk;
        let mut backtick_count: i32 = 0;
        let mut in_fenced_code_block = false;
        let mut comment_parts: Vec<NodeId> = Vec::with_capacity(1);
        let mut comments: Vec<String> = Vec::new();
        let mut comments_pos: i32 = -1;
        let mut link_end: i32 = start;
        let mut margin: i32 = -1;
        let mut indent = indent;

        self.next_token_jsdoc();
        while self.parse_optional_jsdoc(Kind::WhitespaceTrivia) {}
        if self.parse_optional_jsdoc(Kind::NewLineTrivia) {
            state = JSDocState::BeginningOfLine;
            indent = 0;
        }
        loop {
            // Detect fenced code blocks by counting consecutive backtick tokens.
            // Three or more consecutive backticks toggle the fenced code block state.
            if self.token != Kind::BacktickToken && backtick_count > 0 {
                if backtick_count >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backtick_count = 0;
            }
            match self.token {
                Kind::AtToken => {
                    if in_fenced_code_block || !self.scanner.can_follow_jsdoc_at() {
                        if in_fenced_code_block {
                            state = JSDocState::SavingBackticks;
                        } else {
                            state = JSDocState::SavingComments;
                        }
                        push_comment(
                            &mut comments,
                            &mut margin,
                            &mut indent,
                            self.scanner.token_text(),
                        );
                    } else {
                        remove_trailing_whitespace(&mut comments);
                        if comments_pos == -1 {
                            comments_pos = self.node_pos();
                        }
                        let tag = self.parse_tag(&tags, indent);
                        if tags_pos == -1 {
                            tags_pos = {
                                let s = self.factory.store();
                                s.node(tag).pos()
                            };
                        }
                        tags.push(tag);
                        tags_end = {
                            let s = self.factory.store();
                            s.node(tag).end()
                        };
                        // NOTE: According to usejsdoc.org, a tag goes to end of line, except the last tag.
                        // Real-world comments may break this rule, so "BeginningOfLine" will not be a real line beginning
                        // for malformed examples like `/** @param {string} x @returns {number} the length */`
                        state = JSDocState::BeginningOfLine;
                        margin = -1;
                    }
                }
                Kind::NewLineTrivia => {
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                    state = JSDocState::BeginningOfLine;
                    indent = 0;
                }
                Kind::AsteriskToken => {
                    let asterisk = self.scanner.token_text();
                    if state == JSDocState::SawAsterisk {
                        // If we've already seen an asterisk, then we can no longer parse a tag on this line
                        state = JSDocState::SavingComments;
                        push_comment(&mut comments, &mut margin, &mut indent, asterisk);
                    } else {
                        debug_assert_eq!(
                            state,
                            JSDocState::BeginningOfLine,
                            "state must be BeginningOfLine"
                        );
                        // Ignore the first asterisk on a line
                        state = JSDocState::SawAsterisk;
                        indent += asterisk.len() as i32;
                    }
                }
                Kind::WhitespaceTrivia => {
                    if state == JSDocState::SavingComments || state == JSDocState::SavingBackticks {
                        panic!("whitespace shouldn't come from the scanner while saving top-level comment text");
                    }
                    // only collect whitespace if we're already saving comments or have just crossed the comment indent margin
                    let whitespace = self.scanner.token_text();
                    if margin > -1 && indent + whitespace.len() as i32 > margin {
                        let mut existing_indent = margin - indent;
                        if existing_indent < 0 {
                            existing_indent += whitespace.len() as i32;
                        }
                        if existing_indent < 0 {
                            existing_indent = 0;
                        }
                        comments.push(whitespace[existing_indent as usize..].to_string());
                    }
                    indent += whitespace.len() as i32;
                }
                Kind::EndOfFile => break,
                Kind::JSDocCommentTextToken => {
                    if state != JSDocState::SavingBackticks {
                        if in_fenced_code_block {
                            state = JSDocState::SavingBackticks;
                        } else {
                            state = JSDocState::SavingComments;
                        }
                    }
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        &self.token_value_string(),
                    );
                }
                Kind::BacktickToken => {
                    backtick_count += 1;
                    if state == JSDocState::SavingBackticks {
                        state = JSDocState::SavingComments;
                    } else {
                        state = JSDocState::SavingBackticks;
                    }
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                Kind::OpenBraceToken if in_fenced_code_block => {
                    state = JSDocState::SavingBackticks;
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                Kind::OpenBraceToken => {
                    state = JSDocState::SavingComments;
                    let comment_end = self.scanner.token_full_start() as i32;
                    let link_start = self.scanner.token_end() as i32 - 1;
                    let link = self.parse_jsdoc_link(link_start);
                    if let Some(link) = link {
                        if link_end == start {
                            remove_leading_newlines(&mut comments);
                        }
                        let jsdoc_text = {
                            let __hoist_1_8 =
                                self.factory.new_jsdoc_text(boxed_strings(comments.clone()));
                            let __hoist_1_9 = link_end;
                            let __hoist_1_10 = comment_end;
                            self.finish_node_with_end(__hoist_1_8, __hoist_1_9, __hoist_1_10)
                        };
                        comment_parts.push(jsdoc_text);
                        comment_parts.push(link);
                        comments.clear();
                        link_end = self.scanner.token_end() as i32;
                    } else {
                        // Go: fallthrough to the default comment-text handling
                        enter_saving_state(&mut state, in_fenced_code_block);
                        push_comment(
                            &mut comments,
                            &mut margin,
                            &mut indent,
                            self.scanner.token_text(),
                        );
                    }
                }
                _ => {
                    // Anything else is doc comment text. We just save it. Because it
                    // wasn't a tag, we can no longer parse a tag on this line until we hit the next
                    // line break.
                    enter_saving_state(&mut state, in_fenced_code_block);
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
            }
            if state == JSDocState::SavingComments || state == JSDocState::SavingBackticks {
                self.next_jsdoc_comment_text_token(state == JSDocState::SavingBackticks);
            } else {
                self.next_token_jsdoc();
            }
        }

        if comments_pos == -1 {
            comments_pos = self.scanner.token_full_start() as i32;
        }

        if !comments.is_empty() {
            let last = comments.len() - 1;
            comments[last] = comments[last].trim_end().to_string();
            let jsdoc_text = {
                let __hoist_1_12 = self.factory.new_jsdoc_text(boxed_strings(comments));
                let __hoist_1_13 = link_end;
                let __hoist_1_14 = comments_pos;
                self.finish_node_with_end(__hoist_1_12, __hoist_1_13, __hoist_1_14)
            };
            comment_parts.push(jsdoc_text);
        }

        if !comment_parts.is_empty() && !tags.is_empty() && comments_pos == -1 {
            panic!("having parsed tags implies that the end of the comment span should be set");
        }

        let mut tags_node_list: Option<NodeList> = None;
        if tags_pos != -1 {
            tags_node_list = Some(self.new_node_list(TextRange::new(tags_pos, tags_end), tags));
        }

        let comment_list = self.new_node_list(TextRange::new(start, comments_pos), comment_parts);
        let jsdoc_comment = self.factory.new_jsdoc(Some(comment_list), tags_node_list);
        self.finish_node_with_end(jsdoc_comment, full_start, end)
    }

    /// Go: `func (p *Parser) isNextNonwhitespaceTokenEndOfFile() bool`.
    fn is_next_nonwhitespace_token_end_of_file(&mut self) -> bool {
        // We must use infinite lookahead, as there could be any number of newlines :(
        loop {
            self.next_token_jsdoc();
            if self.token == Kind::EndOfFile {
                return true;
            }
            if !(self.token == Kind::WhitespaceTrivia || self.token == Kind::NewLineTrivia) {
                return false;
            }
        }
    }

    /// Go: `func (p *Parser) skipWhitespace()`.
    fn skip_whitespace(&mut self) {
        if (self.token == Kind::WhitespaceTrivia || self.token == Kind::NewLineTrivia)
            && self.look_ahead(|p| p.is_next_nonwhitespace_token_end_of_file())
        {
            return;
            // Don't skip whitespace prior to EoF (or end of comment) - that shouldn't be included in any node's range
        }
        while self.token == Kind::WhitespaceTrivia || self.token == Kind::NewLineTrivia {
            self.next_token_jsdoc();
        }
    }

    /// Go: `func (p *Parser) skipWhitespaceOrAsterisk() string`.
    fn skip_whitespace_or_asterisk(&mut self) -> String {
        if (self.token == Kind::WhitespaceTrivia || self.token == Kind::NewLineTrivia)
            && self.look_ahead(|p| p.is_next_nonwhitespace_token_end_of_file())
        {
            return String::new();
            // Don't skip whitespace prior to EoF (or end of comment) - that shouldn't be included in any node's range
        }

        let mut preceding_line_break = self.scanner.has_preceding_line_break();
        let mut seen_line_break = false;
        let mut indents: Vec<String> = Vec::with_capacity(4);
        while (preceding_line_break && self.token == Kind::AsteriskToken)
            || self.token == Kind::WhitespaceTrivia
            || self.token == Kind::NewLineTrivia
        {
            indents.push(self.scanner.token_text().to_string());
            if self.token == Kind::NewLineTrivia {
                preceding_line_break = true;
                seen_line_break = true;
                indents.clear();
            } else if self.token == Kind::AsteriskToken {
                preceding_line_break = false;
            }
            self.next_token_jsdoc();
        }
        if seen_line_break {
            indents.concat()
        } else {
            String::new()
        }
    }

    /// Go: `func (p *Parser) parseTag(tags []*ast.Node, margin int) *ast.Node`.
    fn parse_tag(&mut self, tags: &[NodeId], margin: i32) -> NodeId {
        if self.token != Kind::AtToken {
            panic!("should be called only at the start of a tag");
        }
        let start = self.scanner.token_start() as i32;
        self.next_token_jsdoc();

        let tag_name =
            self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED));
        let indent_text = self.skip_whitespace_or_asterisk();

        let tag_name_text = {
            let s = self.factory.store();
            s.node(tag_name)
                .as_identifier()
                .expect("Identifier data")
                .text
                .to_string()
        };

        let tag: NodeId = match tag_name_text.as_str() {
            "implements" => self.parse_implements_tag(start, tag_name, margin, &indent_text),
            "augments" | "extends" => {
                self.parse_augments_tag(start, tag_name, margin, &indent_text)
            }
            "public" => self.parse_simple_tag(
                start,
                |p, tag_name, comments| p.factory.new_jsdoc_public_tag(tag_name, comments),
                tag_name,
                margin,
                &indent_text,
            ),
            "private" => self.parse_simple_tag(
                start,
                |p, tag_name, comments| p.factory.new_jsdoc_private_tag(tag_name, comments),
                tag_name,
                margin,
                &indent_text,
            ),
            "protected" => self.parse_simple_tag(
                start,
                |p, tag_name, comments| p.factory.new_jsdoc_protected_tag(tag_name, comments),
                tag_name,
                margin,
                &indent_text,
            ),
            "readonly" => self.parse_simple_tag(
                start,
                |p, tag_name, comments| p.factory.new_jsdoc_readonly_tag(tag_name, comments),
                tag_name,
                margin,
                &indent_text,
            ),
            "override" => self.parse_simple_tag(
                start,
                |p, tag_name, comments| p.factory.new_jsdoc_override_tag(tag_name, comments),
                tag_name,
                margin,
                &indent_text,
            ),
            "deprecated" => {
                self.has_deprecated_tag = true;
                self.parse_simple_tag(
                    start,
                    |p, tag_name, comments| p.factory.new_jsdoc_deprecated_tag(tag_name, comments),
                    tag_name,
                    margin,
                    &indent_text,
                )
            }
            "this" => self.parse_this_tag(start, tag_name, margin, &indent_text),
            "arg" | "argument" | "param" => self.parse_parameter_or_property_tag(
                start,
                tag_name,
                PROPERTY_LIKE_PARSE_PARAMETER,
                margin,
            ),
            "return" | "returns" => {
                self.parse_return_tag(tags, start, tag_name, margin, &indent_text)
            }
            "template" => self.parse_template_tag(start, tag_name, margin, &indent_text),
            "type" => self.parse_type_tag(tags, start, tag_name, margin, &indent_text),
            "typedef" => self.parse_typedef_tag(start, tag_name, margin, &indent_text),
            "callback" => self.parse_callback_tag(start, tag_name, margin, &indent_text),
            "overload" => self.parse_overload_tag(start, tag_name, margin, &indent_text),
            "satisfies" => self.parse_satisfies_tag(start, tag_name, margin, &indent_text),
            "see" => self.parse_jsdoc_see_tag(start, tag_name, margin, &indent_text),
            "exception" | "throws" => self.parse_throws_tag(start, tag_name, margin, &indent_text),
            "import" => self.parse_jsdoc_import_tag(start, tag_name, margin, &indent_text),
            _ => self.parse_unknown_tag(start, tag_name, margin, &indent_text),
        };
        tag
    }

    /// Go: `func (p *Parser) parseTrailingTagComments(pos, end, margin int, indentText string) *ast.NodeList`.
    fn parse_trailing_tag_comments(
        &mut self,
        pos: i32,
        end: i32,
        margin: i32,
        indent_text: &str,
    ) -> Option<NodeList> {
        // some tags, like typedef and callback, have already parsed their comments earlier
        let mut margin = margin;
        if indent_text.is_empty() {
            margin += end - pos;
        }
        let mut initial_margin: Option<String> = None;
        if (margin as usize) < indent_text.len() {
            initial_margin = Some(indent_text[margin as usize..].to_string());
        }
        self.parse_tag_comments(margin, initial_margin.as_deref())
    }

    /// Go: `func (p *Parser) parseTagComments(indent int, initialMargin *string) *ast.NodeList`.
    fn parse_tag_comments(
        &mut self,
        indent: i32,
        initial_margin: Option<&str>,
    ) -> Option<NodeList> {
        let comments_pos: i32 = self.node_pos();
        let mut comments: Vec<String> = Vec::new();
        let mut parts: Vec<NodeId> = Vec::new();
        let mut link_end: i32 = -1;
        let mut state = JSDocState::BeginningOfLine;
        let mut backtick_count: i32 = 0;
        let mut in_fenced_code_block = false;
        if indent < 0 {
            panic!("indent must be a natural number");
        }
        let mut margin: i32 = -1;
        let mut indent = indent;

        if let Some(initial_margin) = initial_margin {
            // jump straight to saving comments if there is some initial indentation
            if !initial_margin.is_empty() {
                push_comment(&mut comments, &mut margin, &mut indent, initial_margin);
            }
            state = JSDocState::SawAsterisk;
        }
        loop {
            // Detect fenced code blocks by counting consecutive backtick tokens.
            // Three or more consecutive backticks toggle the fenced code block state.
            if self.token != Kind::BacktickToken && backtick_count > 0 {
                if backtick_count >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backtick_count = 0;
            }
            match self.token {
                Kind::NewLineTrivia => {
                    state = JSDocState::BeginningOfLine;
                    // don't use pushComment here because we want to keep the margin unchanged
                    comments.push(self.scanner.token_text().to_string());
                    indent = 0;
                }
                Kind::AtToken => {
                    if !in_fenced_code_block && self.scanner.can_follow_jsdoc_at() {
                        self.scanner.reset_pos(self.scanner.token_end() - 1);
                        break;
                    }
                    if in_fenced_code_block {
                        state = JSDocState::SavingBackticks;
                    } else {
                        state = JSDocState::SavingComments;
                    }
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                Kind::EndOfFile => {
                    // Done
                    break;
                }
                Kind::WhitespaceTrivia => {
                    if state == JSDocState::SavingComments || state == JSDocState::SavingBackticks {
                        panic!(
                            "whitespace shouldn't come from the scanner while saving comment text"
                        );
                    }
                    let whitespace = self.scanner.token_text();
                    // if the whitespace crosses the margin, take only the whitespace that passes the margin
                    if margin > -1 && indent + whitespace.len() as i32 > margin {
                        comments.push(whitespace[(margin - indent).max(0) as usize..].to_string());
                        if in_fenced_code_block {
                            state = JSDocState::SavingBackticks;
                        } else {
                            state = JSDocState::SavingComments;
                        }
                    }
                    indent += whitespace.len() as i32;
                }
                Kind::OpenBraceToken if in_fenced_code_block => {
                    state = JSDocState::SavingBackticks;
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                Kind::OpenBraceToken => {
                    state = JSDocState::SavingComments;
                    let comment_end = self.scanner.token_full_start() as i32;
                    let link_start = self.scanner.token_end() as i32 - 1;
                    let link = self.parse_jsdoc_link(link_start);
                    if let Some(link) = link {
                        let comment_start = if link_end > -1 {
                            link_end
                        } else {
                            comments_pos
                        };
                        let text = {
                            let __hoist_1_16 =
                                self.factory.new_jsdoc_text(boxed_strings(comments.clone()));
                            let __hoist_1_17 = comment_start;
                            let __hoist_1_18 = comment_end;
                            self.finish_node_with_end(__hoist_1_16, __hoist_1_17, __hoist_1_18)
                        };
                        parts.push(text);
                        parts.push(link);
                        comments.clear();
                        link_end = self.scanner.token_end() as i32;
                    } else {
                        push_comment(
                            &mut comments,
                            &mut margin,
                            &mut indent,
                            self.scanner.token_text(),
                        );
                    }
                }
                Kind::BacktickToken => {
                    backtick_count += 1;
                    if state == JSDocState::SavingBackticks {
                        state = JSDocState::SavingComments;
                    } else {
                        state = JSDocState::SavingBackticks;
                    }
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                Kind::JSDocCommentTextToken => {
                    if state != JSDocState::SavingBackticks {
                        enter_saving_state(&mut state, in_fenced_code_block);
                        // leading identifiers start recording as well
                    }
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        &self.token_value_string(),
                    );
                }
                Kind::AsteriskToken if state == JSDocState::BeginningOfLine => {
                    // leading asterisks start recording on the *next* (non-whitespace) token
                    state = JSDocState::SawAsterisk;
                    indent += 1;
                }
                Kind::AsteriskToken => {
                    // record the * as a comment (Go: fallthrough to the default arm)
                    enter_saving_state(&mut state, in_fenced_code_block);
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
                _ => {
                    enter_saving_state(&mut state, in_fenced_code_block);
                    // leading identifiers start recording as well
                    push_comment(
                        &mut comments,
                        &mut margin,
                        &mut indent,
                        self.scanner.token_text(),
                    );
                }
            }
            if state == JSDocState::SavingComments || state == JSDocState::SavingBackticks {
                self.next_jsdoc_comment_text_token(state == JSDocState::SavingBackticks);
            } else {
                self.next_token_jsdoc();
            }
        }

        remove_leading_newlines(&mut comments);
        remove_trailing_whitespace(&mut comments);
        if !comments.is_empty() {
            let comment_start = if link_end > -1 {
                link_end
            } else {
                comments_pos
            };
            let text = {
                let __hoist_1_20 = self.factory.new_jsdoc_text(boxed_strings(comments));
                let __hoist_1_21 = comment_start;
                self.finish_node(__hoist_1_20, __hoist_1_21)
            };
            parts.push(text);
        }

        if !parts.is_empty() {
            let end = self.scanner.token_end() as i32;
            return Some(self.new_node_list(TextRange::new(comments_pos, end), parts));
        }
        None
    }

    /// Go: `func (p *Parser) parseJSDocLink(start int) *ast.Node`.
    fn parse_jsdoc_link(&mut self, start: i32) -> Option<NodeId> {
        let state = self.mark();
        let (link_type, ok) = self.parse_jsdoc_link_prefix();
        if !ok {
            self.rewind(state);
            return None;
        }
        self.next_token_jsdoc();
        // start at token after link, then skip any whitespace
        self.skip_whitespace();
        let name = self.parse_jsdoc_link_name();
        let mut text: Vec<Box<str>> = Vec::new();
        while self.token != Kind::CloseBraceToken
            && self.token != Kind::NewLineTrivia
            && self.token != Kind::EndOfFile
        {
            text.push(self.scanner.token_text().to_string().into_boxed_str());
            self.next_token_jsdoc(); // Couldn't this be nextTokenCommentJSDoc?
        }
        let create: NodeId = match link_type.as_str() {
            "link" => self.factory.new_jsdoc_link(name, text),
            "linkcode" => self.factory.new_jsdoc_link_code(name, text),
            _ => self.factory.new_jsdoc_link_plain(name, text),
        };
        let end = self.scanner.token_end() as i32;
        Some(self.finish_node_with_end(create, start, end))
    }

    /// Go: `func (p *Parser) parseJSDocLinkName() *ast.Node`.
    fn parse_jsdoc_link_name(&mut self) -> Option<NodeId> {
        if token_is_identifier_or_keyword(self.token) {
            let pos = self.node_pos();
            let mut name = self.parse_identifier_name();
            while self.parse_optional(Kind::DotToken) {
                let right: NodeId = if self.token == Kind::PrivateIdentifier {
                    self.create_missing_identifier()
                } else {
                    self.parse_identifier_name()
                };
                name = {
                    let __hoist_1_23 = self.factory.new_qualified_name(name, right);
                    let __hoist_1_24 = pos;
                    self.finish_node(__hoist_1_23, __hoist_1_24)
                };
            }
            while self.token == Kind::PrivateIdentifier {
                self.scanner.re_scan_hash_token();
                self.next_token_jsdoc();
                let right = self.parse_identifier();
                name = {
                    let __hoist_1_26 = self.factory.new_qualified_name(name, right);
                    let __hoist_1_27 = pos;
                    self.finish_node(__hoist_1_26, __hoist_1_27)
                };
            }
            return Some(name);
        }
        None
    }

    /// Go: `func (p *Parser) parseJSDocLinkPrefix() (string, bool)`.
    fn parse_jsdoc_link_prefix(&mut self) -> (String, bool) {
        self.skip_whitespace_or_asterisk();
        if self.token == Kind::OpenBraceToken
            && self.next_token_jsdoc() == Kind::AtToken
            && token_is_identifier_or_keyword(self.next_token_jsdoc())
        {
            let kind = self.token_value_string();
            if is_jsdoc_link_tag(&kind) {
                return (kind, true);
            }
        }
        ("NONE".to_string(), false)
    }

    /// Go: `func (p *Parser) parseUnknownTag(start int, tagName *ast.IdentifierNode, indent int, indentText string) *ast.Node`.
    fn parse_unknown_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let comments = {
            let __hoist_1_29 = start;
            let __hoist_1_30 = self.node_pos();
            let __hoist_1_31 = indent;
            let __hoist_1_32 = indent_text;
            self.parse_trailing_tag_comments(__hoist_1_29, __hoist_1_30, __hoist_1_31, __hoist_1_32)
        };
        {
            let __hoist_1_34 = self.factory.new_jsdoc_unknown_tag(tag_name, comments);
            let __hoist_1_35 = start;
            self.finish_node(__hoist_1_34, __hoist_1_35)
        }
    }

    /// Go: `func (p *Parser) tryParseTypeExpression() *ast.Node`.
    fn try_parse_type_expression(&mut self) -> Option<NodeId> {
        self.skip_whitespace_or_asterisk();
        if self.token == Kind::OpenBraceToken {
            Some(self.parse_jsdoc_type_expression(false /*mayOmitBraces*/))
        } else {
            None
        }
    }

    /// Go: `func (p *Parser) parseBracketNameInPropertyAndParamTag(target propertyLikeParse) (name *ast.EntityName, isBracketed bool)`.
    fn parse_bracket_name_in_property_and_param_tag(
        &mut self,
        target: PropertyLikeParse,
    ) -> (NodeId, bool) {
        // Looking for something like '[foo]', 'foo', '[foo.bar]' or 'foo.bar'
        let is_bracketed = self.parse_optional_jsdoc(Kind::OpenBracketToken);
        if is_bracketed {
            self.skip_whitespace();
        }
        // a markdown-quoted name: `arg` is not legal jsdoc, but occurs in the wild
        let is_backquoted = self.parse_optional_jsdoc(Kind::BacktickToken);
        let name = self.parse_jsdoc_entity_name(if target == PROPERTY_LIKE_PARSE_PARAMETER {
            None
        } else {
            Some(&tsc_diagnostics::IDENTIFIER_EXPECTED)
        });
        if is_backquoted {
            self.parse_expected_token_jsdoc(Kind::BacktickToken);
        }
        if is_bracketed {
            self.skip_whitespace();
            // May have an optional default, e.g. '[foo = 42]'
            if self.parse_optional_token(Kind::EqualsToken).is_some() {
                self.parse_expression();
            }

            self.parse_expected(Kind::CloseBracketToken);
        }

        (name, is_bracketed)
    }

    /// Go: `func (p *Parser) parseParameterOrPropertyTag(start int, tagName, target propertyLikeParse, indent int) *ast.Node`.
    fn parse_parameter_or_property_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        target: PropertyLikeParse,
        indent: i32,
    ) -> NodeId {
        let mut type_expression = self.try_parse_type_expression();
        let mut is_name_first = type_expression.is_none();
        self.skip_whitespace_or_asterisk();

        let (name, is_bracketed) = self.parse_bracket_name_in_property_and_param_tag(target);
        let indent_text = self.skip_whitespace_or_asterisk();

        if is_name_first
            && self.look_ahead(|p| {
                let (_, ok) = p.parse_jsdoc_link_prefix();
                !ok
            })
        {
            type_expression = self.try_parse_type_expression();
        }

        let comment = {
            let __hoist_1_37 = start;
            let __hoist_1_38 = self.node_pos();
            let __hoist_1_39 = indent;
            let __hoist_1_40 = &indent_text;
            self.parse_trailing_tag_comments(__hoist_1_37, __hoist_1_38, __hoist_1_39, __hoist_1_40)
        };

        let nested_type_literal =
            self.parse_nested_type_literal(type_expression, Some(name), target, indent);
        if let Some(nested_type_literal) = nested_type_literal {
            type_expression = Some(nested_type_literal);
            is_name_first = true;
        }
        let kind = if target == PROPERTY_LIKE_PARSE_PROPERTY {
            Kind::JSDocPropertyTag
        } else {
            Kind::JSDocParameterTag
        };
        let result = self.factory.new_jsdoc_parameter_or_property_tag(
            kind,
            tag_name,
            name,
            is_bracketed,
            type_expression,
            is_name_first,
            comment,
        );
        self.finish_node(result, start)
    }

    /// Go: `func (p *Parser) parseNestedTypeLiteral(typeExpression *ast.Node, name *ast.EntityName, target propertyLikeParse, indent int) *ast.Node`.
    fn parse_nested_type_literal(
        &mut self,
        type_expression: Option<NodeId>,
        name: Option<NodeId>,
        target: PropertyLikeParse,
        indent: i32,
    ) -> Option<NodeId> {
        let is_object_like = type_expression
            .map(|te| {
                let inner = {
                    let s = self.factory.store();
                    s.node(te)
                        .as_jsdoc_type_expression()
                        .expect("JSDocTypeExpression data")
                        .type_
                };
                is_object_or_object_array_type_reference(self.factory.store(), inner)
            })
            .unwrap_or(false);
        if is_object_like {
            let pos = self.node_pos();
            let mut children: Vec<NodeId> = Vec::new();
            loop {
                let state = self.mark();
                let child = self.parse_child_parameter_or_property_tag(target, indent, name);
                let Some(child) = child else {
                    self.rewind(state);
                    break;
                };
                let child_kind = {
                    let s = self.factory.store();
                    s.node(child).kind
                };
                match child_kind {
                    Kind::JSDocParameterTag | Kind::JSDocPropertyTag => {
                        children.push(child);
                    }
                    Kind::JSDocTemplateTag => {
                        let tag_name_loc = {
                            let s = self.factory.store();
                            let tag_name = s
                                .node(child)
                                .as_jsdoc_template_tag()
                                .expect("JSDocTemplateTag data")
                                .tag_name;
                            s.node(tag_name).loc
                        };
                        self.parse_error_at_range(
                            tag_name_loc,
                            &tsc_diagnostics::A_JSDOC_TEMPLATE_TAG_MAY_NOT_FOLLOW_A_TYPEDEF_CALLBACK_OR_OVERLOAD_TAG,
                            &[],
                        );
                    }
                    _ => {}
                }
            }
            if !children.is_empty() {
                let inner = {
                    let s = self.factory.store();
                    let te = type_expression.expect("checked above");
                    s.node(te)
                        .as_jsdoc_type_expression()
                        .expect("JSDocTypeExpression data")
                        .type_
                };
                let inner_kind = {
                    let s = self.factory.store();
                    s.node(inner).kind
                };
                let literal = {
                    let __hoist_1_42 = self
                        .factory
                        .new_jsdoc_type_literal(children, inner_kind == Kind::ArrayType);
                    let __hoist_1_43 = pos;
                    self.finish_node(__hoist_1_42, __hoist_1_43)
                };
                return Some({
                    let __hoist_1_45 = self.factory.new_jsdoc_type_expression(literal);
                    let __hoist_1_46 = pos;
                    self.finish_node(__hoist_1_45, __hoist_1_46)
                });
            }
        }
        None
    }

    /// Go: `func (p *Parser) parseReturnTag(previousTags []*ast.Node, start int, tagName, indent int, indentText string) *ast.Node`.
    fn parse_return_tag(
        &mut self,
        previous_tags: &[NodeId],
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let has_return_tag = {
            let s = self.factory.store();
            previous_tags
                .iter()
                .any(|&t| is_jsdoc_return_tag(s.node(t)))
        };
        if has_return_tag {
            let tag_name_pos = {
                let s = self.factory.store();
                s.node(tag_name).pos()
            };
            let text = {
                let s = self.factory.store();
                s.node(tag_name)
                    .as_identifier()
                    .expect("Identifier data")
                    .text
                    .to_string()
            };
            ({
                let __hoist_1_48 = tag_name_pos;
                let __hoist_1_49 = self.scanner.token_start() as i32;
                let __hoist_1_50 = &tsc_diagnostics::X_0_TAG_ALREADY_SPECIFIED;
                let __hoist_1_51 = &[text];
                self.parse_error_at(__hoist_1_48, __hoist_1_49, __hoist_1_50, __hoist_1_51)
            });
        }

        let type_expression = self.try_parse_type_expression();
        let comments = {
            let __hoist_1_53 = start;
            let __hoist_1_54 = self.node_pos();
            let __hoist_1_55 = indent;
            let __hoist_1_56 = indent_text;
            self.parse_trailing_tag_comments(__hoist_1_53, __hoist_1_54, __hoist_1_55, __hoist_1_56)
        };
        {
            let __hoist_1_58 =
                self.factory
                    .new_jsdoc_return_tag(tag_name, type_expression, comments);
            let __hoist_1_59 = start;
            self.finish_node(__hoist_1_58, __hoist_1_59)
        }
    }

    /// Go: `func (p *Parser) parseTypeTag(previousTags []*ast.Node, start int, tagName, indent int, indentText string) *ast.Node`.
    /// Pass indent=-1 to skip parsing trailing comments (as when a type tag is nested in a typedef).
    fn parse_type_tag(
        &mut self,
        previous_tags: &[NodeId],
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let has_type_tag = {
            let s = self.factory.store();
            previous_tags.iter().any(|&t| is_jsdoc_type_tag(s.node(t)))
        };
        if has_type_tag {
            let tag_name_pos = {
                let s = self.factory.store();
                s.node(tag_name).pos()
            };
            let text = {
                let s = self.factory.store();
                s.node(tag_name)
                    .as_identifier()
                    .expect("Identifier data")
                    .text
                    .to_string()
            };
            ({
                let __hoist_1_61 = tag_name_pos;
                let __hoist_1_62 = self.scanner.token_start() as i32;
                let __hoist_1_63 = &tsc_diagnostics::X_0_TAG_ALREADY_SPECIFIED;
                let __hoist_1_64 = &[text];
                self.parse_error_at(__hoist_1_61, __hoist_1_62, __hoist_1_63, __hoist_1_64)
            });
        }

        let type_expression = self.parse_jsdoc_type_expression(true);
        let mut comments: Option<NodeList> = None;
        if indent != -1 {
            comments = {
                let __hoist_1_66 = start;
                let __hoist_1_67 = self.node_pos();
                let __hoist_1_68 = indent;
                let __hoist_1_69 = indent_text;
                self.parse_trailing_tag_comments(
                    __hoist_1_66,
                    __hoist_1_67,
                    __hoist_1_68,
                    __hoist_1_69,
                )
            };
        }
        {
            let __hoist_1_71 = self
                .factory
                .new_jsdoc_type_tag(tag_name, type_expression, comments);
            let __hoist_1_72 = start;
            self.finish_node(__hoist_1_71, __hoist_1_72)
        }
    }

    /// Go: `func (p *Parser) parseSeeTag(start int, tagName, indent int, indentText string) *ast.Node`.
    ///
    /// Named `parse_jsdoc_see_tag` to avoid clashing with the (unused) Go
    /// `parseSeeTag` name family in this module.
    fn parse_jsdoc_see_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let has_name_reference = (self.is_identifier()
            && !self.source_text[self.scanner.token_end()..].starts_with("://"))
            || (self.token == Kind::OpenBraceToken
                && self.look_ahead(|p| p.next_token_is_identifier_or_keyword()));
        let mut name_expression: Option<NodeId> = None;
        if has_name_reference {
            name_expression = Some(self.parse_jsdoc_name_reference());
        }
        let comments = {
            let __hoist_1_74 = start;
            let __hoist_1_75 = self.node_pos();
            let __hoist_1_76 = indent;
            let __hoist_1_77 = indent_text;
            self.parse_trailing_tag_comments(__hoist_1_74, __hoist_1_75, __hoist_1_76, __hoist_1_77)
        };
        {
            let __hoist_1_79 = self.factory.new_jsdoc_see_tag(
                tag_name,
                name_expression.unwrap_or(NodeId::NONE),
                comments,
            );
            let __hoist_1_80 = start;
            self.finish_node(__hoist_1_79, __hoist_1_80)
        }
    }

    /// Go: `func (p *Parser) parseImplementsTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_implements_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let class_name = self.parse_expression_with_type_arguments_for_augments();
        let comments = {
            let __hoist_1_82 = start;
            let __hoist_1_83 = self.node_pos();
            let __hoist_1_84 = margin;
            let __hoist_1_85 = indent_text;
            self.parse_trailing_tag_comments(__hoist_1_82, __hoist_1_83, __hoist_1_84, __hoist_1_85)
        };
        {
            let __hoist_1_87 = self
                .factory
                .new_jsdoc_implements_tag(tag_name, class_name, comments);
            let __hoist_1_88 = start;
            self.finish_node(__hoist_1_87, __hoist_1_88)
        }
    }

    /// Go: `func (p *Parser) parseAugmentsTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_augments_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let class_name = self.parse_expression_with_type_arguments_for_augments();
        let comments = {
            let __hoist_1_90 = start;
            let __hoist_1_91 = self.node_pos();
            let __hoist_1_92 = margin;
            let __hoist_1_93 = indent_text;
            self.parse_trailing_tag_comments(__hoist_1_90, __hoist_1_91, __hoist_1_92, __hoist_1_93)
        };
        {
            let __hoist_1_95 = self
                .factory
                .new_jsdoc_augments_tag(tag_name, class_name, comments);
            let __hoist_1_96 = start;
            self.finish_node(__hoist_1_95, __hoist_1_96)
        }
    }

    /// Go: `func (p *Parser) parseSatisfiesTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_satisfies_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let type_expression = self.parse_jsdoc_type_expression(false);
        let comments = {
            let __hoist_1_98 = start;
            let __hoist_1_99 = self.node_pos();
            let __hoist_1_100 = margin;
            let __hoist_1_101 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_98,
                __hoist_1_99,
                __hoist_1_100,
                __hoist_1_101,
            )
        };
        {
            let __hoist_1_103 =
                self.factory
                    .new_jsdoc_satisfies_tag(tag_name, type_expression, comments);
            let __hoist_1_104 = start;
            self.finish_node(__hoist_1_103, __hoist_1_104)
        }
    }

    /// Go: `func (p *Parser) parseThrowsTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_throws_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let type_expression = self.try_parse_type_expression();
        let comment = {
            let __hoist_1_106 = start;
            let __hoist_1_107 = self.node_pos();
            let __hoist_1_108 = margin;
            let __hoist_1_109 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_106,
                __hoist_1_107,
                __hoist_1_108,
                __hoist_1_109,
            )
        };
        {
            let __hoist_1_111 =
                self.factory
                    .new_jsdoc_throws_tag(tag_name, type_expression, comment);
            let __hoist_1_112 = start;
            self.finish_node(__hoist_1_111, __hoist_1_112)
        }
    }

    /// Go: `func (p *Parser) parseImportTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_jsdoc_import_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let after_import_tag_pos = self.scanner.token_full_start() as i32;

        let mut identifier: Option<NodeId> = None;
        if self.is_identifier() {
            identifier = Some(self.parse_identifier());
        }

        let import_clause = self.try_parse_import_clause(
            identifier,
            after_import_tag_pos,
            Kind::TypeKeyword,
            true, /*skipJSDocLeadingAsterisks*/
        );
        let module_specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();

        let comments = {
            let __hoist_1_114 = start;
            let __hoist_1_115 = self.node_pos();
            let __hoist_1_116 = margin;
            let __hoist_1_117 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_114,
                __hoist_1_115,
                __hoist_1_116,
                __hoist_1_117,
            )
        };
        {
            let __hoist_1_119 = self.factory.new_jsdoc_import_tag(
                tag_name,
                import_clause,
                module_specifier,
                attributes,
                comments,
            );
            let __hoist_1_120 = start;
            self.finish_node(__hoist_1_119, __hoist_1_120)
        }
    }

    /// Go: `func (p *Parser) parseExpressionWithTypeArgumentsForAugments() *ast.Node`.
    fn parse_expression_with_type_arguments_for_augments(&mut self) -> NodeId {
        let used_brace = self.parse_optional(Kind::OpenBraceToken);
        let pos = self.node_pos();
        let expression = self.parse_property_access_entity_name_expression();
        self.scanner.set_skip_jsdoc_leading_asterisks(true);
        let type_arguments = self.parse_type_arguments();
        self.scanner.set_skip_jsdoc_leading_asterisks(false);
        let node = {
            let __hoist_1_122 = self
                .factory
                .new_expression_with_type_arguments(expression, type_arguments);
            let __hoist_1_123 = pos;
            self.finish_node(__hoist_1_122, __hoist_1_123)
        };
        if used_brace {
            self.skip_whitespace();
            self.parse_expected(Kind::CloseBraceToken);
        }
        node
    }

    /// Go: `func (p *Parser) parsePropertyAccessEntityNameExpression() *ast.Node`.
    fn parse_property_access_entity_name_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut node =
            self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED));
        while self.parse_optional(Kind::DotToken) {
            let name =
                self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED));
            node = {
                let __hoist_1_125 =
                    self.factory
                        .new_property_access_expression(node, None, name, NodeFlags::NONE);
                let __hoist_1_126 = pos;
                self.finish_node(__hoist_1_125, __hoist_1_126)
            };
        }
        node
    }

    /// Go: `func (p *Parser) parseSimpleTag(start int, createTag func(tagName, comment) *ast.Node, tagName, margin int, indentText string) *ast.Node`.
    fn parse_simple_tag<F>(
        &mut self,
        start: i32,
        create_tag: F,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId
    where
        F: FnOnce(&mut Parser<'a, 'f>, NodeId, Option<NodeList>) -> NodeId,
    {
        let comments = {
            let __hoist_1_128 = start;
            let __hoist_1_129 = self.node_pos();
            let __hoist_1_130 = margin;
            let __hoist_1_131 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_128,
                __hoist_1_129,
                __hoist_1_130,
                __hoist_1_131,
            )
        };
        let result = create_tag(self, tag_name, comments);
        self.finish_node(result, start)
    }

    /// Go: `func (p *Parser) parseThisTag(start int, tagName, margin int, indentText string) *ast.Node`.
    fn parse_this_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        margin: i32,
        indent_text: &str,
    ) -> NodeId {
        let type_expression = self.parse_jsdoc_type_expression(true);
        self.skip_whitespace();
        let comments = {
            let __hoist_1_133 = start;
            let __hoist_1_134 = self.node_pos();
            let __hoist_1_135 = margin;
            let __hoist_1_136 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_133,
                __hoist_1_134,
                __hoist_1_135,
                __hoist_1_136,
            )
        };
        let result = self
            .factory
            .new_jsdoc_this_tag(tag_name, type_expression, comments);
        self.finish_node(result, start)
    }

    /// Go: `func (p *Parser) parseJSDocTypeNameWithNamespace(nested bool) *ast.Node`.
    fn parse_jsdoc_type_name_with_namespace(&mut self, nested: bool) -> Option<NodeId> {
        let start = self.scanner.token_start() as i32;
        if !token_is_identifier_or_keyword(self.token) {
            return None;
        }
        let type_name_or_namespace_name = self.parse_jsdoc_identifier_name(None);
        if self.parse_optional_jsdoc(Kind::DotToken) {
            let body = self.parse_jsdoc_type_name_with_namespace(true /*nested*/);
            let jsdoc_namespace_node = self.factory.new_module_declaration(
                None,                   /*modifiers*/
                Kind::NamespaceKeyword, /*keyword*/
                type_name_or_namespace_name,
                None, /*attributes*/
                body,
            );
            if nested {
                let s = self.factory.store();
                s.node_mut(jsdoc_namespace_node).flags |= NodeFlags::NESTED_NAMESPACE;
            }
            return Some(self.finish_node(jsdoc_namespace_node, start));
        }
        if nested {
            let s = self.factory.store();
            s.node_mut(type_name_or_namespace_name).flags |=
                NodeFlags::IDENTIFIER_IS_IN_JSDOC_NAMESPACE;
        }
        Some(type_name_or_namespace_name)
    }

    /// Go: `func (p *Parser) parseTypedefTag(start int, tagName, indent int, indentText string) *ast.Node`.
    fn parse_typedef_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let mut type_expression = self.try_parse_type_expression();
        self.skip_whitespace_or_asterisk();
        let mut full_name = self.parse_jsdoc_type_name_with_namespace(false /*nested*/);
        if full_name.is_none() {
            full_name =
                Some(self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED)));
        }
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);

        let mut end: i32 = -1;
        let mut has_children = false;
        let type_expression_is_object_like = type_expression
            .map(|te| {
                let inner = {
                    let s = self.factory.store();
                    s.node(te)
                        .as_jsdoc_type_expression()
                        .expect("JSDocTypeExpression data")
                        .type_
                };
                is_object_or_object_array_type_reference(self.factory.store(), inner)
            })
            .unwrap_or(true);
        if type_expression.is_none() || type_expression_is_object_like {
            let mut child_type_tag: Option<NodeId> = None;
            let mut jsdoc_property_tags: Vec<NodeId> = Vec::new();
            loop {
                let state = self.mark();
                let child = self.parse_child_property_tag(indent);
                let Some(child) = child else {
                    self.rewind(state);
                    break;
                };
                has_children = true;
                let child_kind = {
                    let s = self.factory.store();
                    s.node(child).kind
                };
                match child_kind {
                    Kind::JSDocTemplateTag => {
                        let tag_name_loc = {
                            let s = self.factory.store();
                            let tag_name = s
                                .node(child)
                                .as_jsdoc_template_tag()
                                .expect("JSDocTemplateTag data")
                                .tag_name;
                            s.node(tag_name).loc
                        };
                        self.parse_error_at_range(
                            tag_name_loc,
                            &tsc_diagnostics::A_JSDOC_TEMPLATE_TAG_MAY_NOT_FOLLOW_A_TYPEDEF_CALLBACK_OR_OVERLOAD_TAG,
                            &[],
                        );
                    }
                    Kind::JSDocTypeTag => {
                        if child_type_tag.is_none() {
                            child_type_tag = Some(child);
                        } else {
                            let last_error = self.parse_error_at_current_token(
                                &tsc_diagnostics::A_JSDOC_TYPEDEF_COMMENT_MAY_NOT_CONTAIN_MULTIPLE_TYPE_TAGS,
                                &[],
                            );
                            if let Some(last_error) = last_error {
                                let related = new_diagnostic(
                                    TextRange::new(0, 0),
                                    &tsc_diagnostics::THE_TAG_WAS_FIRST_SPECIFIED_HERE,
                                    &[],
                                );
                                self.add_related_info(last_error, related);
                            }
                        }
                    }
                    _ => {
                        jsdoc_property_tags.push(child);
                    }
                }
            }
            if has_children {
                let is_array_type = type_expression
                    .map(|te| {
                        let s = self.factory.store();
                        let inner = s
                            .node(te)
                            .as_jsdoc_type_expression()
                            .expect("JSDocTypeExpression data")
                            .type_;
                        s.node(inner).kind == Kind::ArrayType
                    })
                    .unwrap_or(false);
                let jsdoc_type_literal = self
                    .factory
                    .new_jsdoc_type_literal(jsdoc_property_tags.clone(), is_array_type);
                let child_type_tag_type_expression = child_type_tag.map(|ctt| {
                    let s = self.factory.store();
                    s.node(ctt)
                        .as_jsdoc_type_tag()
                        .expect("JSDocTypeTag data")
                        .type_expression
                });
                let child_type_tag_is_object_like = child_type_tag_type_expression
                    .map(|te| {
                        let inner = {
                            let s = self.factory.store();
                            s.node(te)
                                .as_jsdoc_type_expression()
                                .expect("JSDocTypeExpression data")
                                .type_
                        };
                        !is_object_or_object_array_type_reference(self.factory.store(), inner)
                    })
                    .unwrap_or(false);
                if child_type_tag_is_object_like {
                    type_expression = child_type_tag_type_expression;
                } else {
                    // !!! This differs from Strada but prevents a crash
                    let pos = if !jsdoc_property_tags.is_empty() {
                        let s = self.factory.store();
                        s.node(jsdoc_property_tags[0]).pos()
                    } else {
                        start
                    };
                    type_expression = Some(self.finish_node(jsdoc_type_literal, pos));
                }
                end = {
                    let s = self.factory.store();
                    s.node(type_expression.expect("just set")).end()
                };
            }
        }

        // Only include the characters between the name end and the next token if a comment was actually parsed out - otherwise it's just whitespace
        if end == -1 {
            // Go: `if hasChildren && typeExpression != nil { ... } else if
            // comment != nil { ... } else if fullName != nil { ... } else if
            // typeExpression != nil { ... } else { ... }` — flattened into a
            // tuple match (Go's is_some/unwrap chains trigger clippy).
            end = match (has_children, type_expression, comment.is_some(), full_name) {
                (true, Some(te), _, _) => {
                    let s = self.factory.store();
                    s.node(te).end()
                }
                (_, _, true, _) => self.node_pos(),
                (_, _, _, Some(fname)) => {
                    let s = self.factory.store();
                    s.node(fname).end()
                }
                (_, Some(te), _, _) => {
                    let s = self.factory.store();
                    s.node(te).end()
                }
                _ => {
                    let s = self.factory.store();
                    s.node(tag_name).end()
                }
            };
        }

        if comment.is_none() {
            comment = self.parse_trailing_tag_comments(start, end, indent, indent_text);
        }

        let typedef_tag = {
            let __hoist_1_138 =
                self.factory
                    .new_jsdoc_typedef_tag(tag_name, type_expression, full_name, comment);
            let __hoist_1_139 = start;
            let __hoist_1_140 = end;
            self.finish_node_with_end(__hoist_1_138, __hoist_1_139, __hoist_1_140)
        };
        if let Some(type_expression) = type_expression {
            // forcibly overwrite parent potentially set by inner type expression parse
            let s = self.factory.store();
            s.node_mut(type_expression).parent.set(typedef_tag);
        }
        typedef_tag
    }

    /// Go: `func (p *Parser) parseCallbackTagParameters(indent int) *ast.NodeList`.
    fn parse_callback_tag_parameters(&mut self, indent: i32) -> NodeList {
        let mut parameters: Vec<NodeId> = Vec::new();
        let pos = self.node_pos();
        loop {
            let state = self.mark();
            let child = self.parse_child_parameter_or_property_tag(
                PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER,
                indent,
                None,
            );
            let Some(child) = child else {
                self.rewind(state);
                break;
            };
            let child_kind = {
                let s = self.factory.store();
                s.node(child).kind
            };
            if child_kind == Kind::JSDocTemplateTag {
                let tag_name_loc = {
                    let s = self.factory.store();
                    let tag_name = s
                        .node(child)
                        .as_jsdoc_template_tag()
                        .expect("JSDocTemplateTag data")
                        .tag_name;
                    s.node(tag_name).loc
                };
                self.parse_error_at_range(
                    tag_name_loc,
                    &tsc_diagnostics::A_JSDOC_TEMPLATE_TAG_MAY_NOT_FOLLOW_A_TYPEDEF_CALLBACK_OR_OVERLOAD_TAG,
                    &[],
                );
            } else {
                parameters.push(child);
            }
        }
        {
            let __hoist_1_142 = TextRange::new(pos, self.node_pos());
            let __hoist_1_143 = parameters;
            self.new_node_list(__hoist_1_142, __hoist_1_143)
        }
    }

    /// Go: `func (p *Parser) parseJSDocSignature(start int, indent int) *ast.Node`.
    fn parse_jsdoc_signature(&mut self, start: i32, indent: i32) -> NodeId {
        let parameters = self.parse_callback_tag_parameters(indent);
        let mut return_tag: Option<NodeId> = None;
        let state = self.mark();
        if self.parse_optional_jsdoc(Kind::AtToken) {
            let tag = self.parse_tag(&[], indent);
            let tag_kind = {
                let s = self.factory.store();
                s.node(tag).kind
            };
            if tag_kind == Kind::JSDocReturnTag {
                return_tag = Some(tag);
            }
        }
        if return_tag.is_none() {
            self.rewind(state);
        }
        {
            let __hoist_1_145 = self.factory.new_jsdoc_signature(
                None, /*typeParameters*/
                Some(parameters),
                return_tag,
            );
            let __hoist_1_146 = start;
            self.finish_node(__hoist_1_145, __hoist_1_146)
        }
    }

    /// Go: `func (p *Parser) parseCallbackTag(start int, tagName, indent int, indentText string) *ast.Node`.
    fn parse_callback_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        let mut full_name = self.parse_jsdoc_type_name_with_namespace(false /*nested*/);
        if full_name.is_none() {
            full_name =
                Some(self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED)));
        }
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let type_expression = {
            let __hoist_1_148 = self.node_pos();
            let __hoist_1_149 = indent;
            self.parse_jsdoc_signature(__hoist_1_148, __hoist_1_149)
        };
        if comment.is_none() {
            comment = {
                let __hoist_1_151 = start;
                let __hoist_1_152 = self.node_pos();
                let __hoist_1_153 = indent;
                let __hoist_1_154 = indent_text;
                self.parse_trailing_tag_comments(
                    __hoist_1_151,
                    __hoist_1_152,
                    __hoist_1_153,
                    __hoist_1_154,
                )
            };
        }

        let end: i32 = if comment.is_some() {
            self.node_pos()
        } else {
            {
                let s = self.factory.store();
                s.node(type_expression).end()
            }
        };
        {
            let __hoist_1_156 =
                self.factory
                    .new_jsdoc_callback_tag(tag_name, type_expression, full_name, comment);
            let __hoist_1_157 = start;
            let __hoist_1_158 = end;
            self.finish_node_with_end(__hoist_1_156, __hoist_1_157, __hoist_1_158)
        }
    }

    /// Go: `func (p *Parser) parseOverloadTag(start int, tagName, indent int, indentText string) *ast.Node`.
    fn parse_overload_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let type_expression = self.parse_jsdoc_signature(start, indent);
        if comment.is_none() {
            comment = {
                let __hoist_1_160 = start;
                let __hoist_1_161 = self.node_pos();
                let __hoist_1_162 = indent;
                let __hoist_1_163 = indent_text;
                self.parse_trailing_tag_comments(
                    __hoist_1_160,
                    __hoist_1_161,
                    __hoist_1_162,
                    __hoist_1_163,
                )
            };
        }

        let end: i32 = if comment.is_some() {
            self.node_pos()
        } else {
            {
                let s = self.factory.store();
                s.node(type_expression).end()
            }
        };
        {
            let __hoist_1_165 =
                self.factory
                    .new_jsdoc_overload_tag(tag_name, type_expression, comment);
            let __hoist_1_166 = start;
            let __hoist_1_167 = end;
            self.finish_node_with_end(__hoist_1_165, __hoist_1_166, __hoist_1_167)
        }
    }

    /// Go: `func (p *Parser) parseChildPropertyTag(indent int) *ast.Node`.
    fn parse_child_property_tag(&mut self, indent: i32) -> Option<NodeId> {
        self.parse_child_parameter_or_property_tag(PROPERTY_LIKE_PARSE_PROPERTY, indent, None)
    }

    /// Go: `func (p *Parser) parseChildParameterOrPropertyTag(target propertyLikeParse, indent int, name *ast.EntityName) *ast.Node`.
    fn parse_child_parameter_or_property_tag(
        &mut self,
        target: PropertyLikeParse,
        indent: i32,
        name: Option<NodeId>,
    ) -> Option<NodeId> {
        let mut can_parse_tag = true;
        let mut seen_asterisk = false;
        loop {
            match self.next_token_jsdoc() {
                Kind::AtToken => {
                    if can_parse_tag && self.scanner.can_follow_jsdoc_at() {
                        let child = self.try_parse_child_tag(target, indent);
                        if let Some(child) = child {
                            if let Some(name) = name {
                                let (child_kind, child_name_is_identifier, child_name_left_matches) = {
                                    let s = self.factory.store();
                                    let child_kind = s.node(child).kind;
                                    if child_kind == Kind::JSDocParameterTag
                                        || child_kind == Kind::JSDocPropertyTag
                                    {
                                        let d = s
                                            .node(child)
                                            .as_jsdoc_parameter_or_property_tag()
                                            .expect("JSDocParameterOrPropertyTag data");
                                        let child_name = d.name;
                                        let is_ident = tsc_ast::is_identifier(s.node(child_name));
                                        let left_matches = !is_ident
                                            && texts_equal(
                                                s,
                                                name,
                                                s.node(child_name)
                                                    .as_qualified_name()
                                                    .expect("QualifiedName data")
                                                    .left,
                                            );
                                        (child_kind, is_ident, left_matches)
                                    } else {
                                        (child_kind, false, false)
                                    }
                                };
                                if (child_kind == Kind::JSDocParameterTag
                                    || child_kind == Kind::JSDocPropertyTag)
                                    && (child_name_is_identifier || !child_name_left_matches)
                                {
                                    return None;
                                }
                            }
                        }
                        return child;
                    }
                    seen_asterisk = false;
                }
                Kind::NewLineTrivia => {
                    can_parse_tag = true;
                    seen_asterisk = false;
                }
                Kind::AsteriskToken => {
                    if seen_asterisk {
                        can_parse_tag = false;
                    }
                    seen_asterisk = true;
                }
                Kind::Identifier => {
                    can_parse_tag = false;
                }
                Kind::EndOfFile => return None,
                _ => {}
            }
        }
    }

    /// Go: `func (p *Parser) tryParseChildTag(target propertyLikeParse, indent int) *ast.Node`.
    fn try_parse_child_tag(&mut self, target: PropertyLikeParse, indent: i32) -> Option<NodeId> {
        if self.token != Kind::AtToken {
            panic!("should only be called when at @");
        }
        let start = self.scanner.token_full_start() as i32;
        self.next_token_jsdoc();

        let tag_name =
            self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED));
        let indent_text = self.skip_whitespace_or_asterisk();
        let tag_name_text = {
            let s = self.factory.store();
            s.node(tag_name)
                .as_identifier()
                .expect("Identifier data")
                .text
                .to_string()
        };

        let t: PropertyLikeParse = match tag_name_text.as_str() {
            "type" => {
                if target == PROPERTY_LIKE_PARSE_PROPERTY {
                    return Some(self.parse_type_tag(&[], start, tag_name, -1, ""));
                }
                return None;
            }
            "prop" | "property" => PROPERTY_LIKE_PARSE_PROPERTY,
            "arg" | "argument" | "param" => {
                PROPERTY_LIKE_PARSE_PARAMETER | PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER
            }
            "template" => {
                return Some(self.parse_template_tag(start, tag_name, indent, &indent_text));
            }
            "this" => {
                return Some(self.parse_this_tag(start, tag_name, indent, &indent_text));
            }
            _ => return None,
        };
        if (target & t) == 0 {
            return None;
        }
        Some(self.parse_parameter_or_property_tag(start, tag_name, target, indent))
    }

    /// Go: `func (p *Parser) parseTemplateTagTypeParameter() *ast.Node`.
    fn parse_template_tag_type_parameter(&mut self) -> Option<NodeId> {
        let type_parameter_pos = self.node_pos();
        let is_bracketed = self.parse_optional_jsdoc(Kind::OpenBracketToken);
        if is_bracketed {
            self.skip_whitespace();
        }

        let modifiers = self.parse_modifiers_ex(false, true /*permitConstAsModifier*/, false);
        let name = self.parse_jsdoc_identifier_name(Some(
            &tsc_diagnostics::UNEXPECTED_TOKEN_A_TYPE_PARAMETER_NAME_WAS_EXPECTED_WITHOUT_CURLY_BRACES,
        ));
        let mut default_type: Option<NodeId> = None;
        if is_bracketed {
            self.skip_whitespace();
            self.parse_expected(Kind::EqualsToken);
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::JSDOC, true);
            default_type = Some(self.parse_jsdoc_type());
            self.context_flags = save_context_flags;
            self.parse_expected(Kind::CloseBracketToken);
        }

        let name_is_missing = {
            let s = self.factory.store();
            tsc_ast::node_is_missing(s.node(name))
        };
        if name_is_missing {
            return None;
        }
        Some({
            let __hoist_1_169 = self.factory.new_type_parameter_declaration(
                modifiers,
                name,
                None, /*constraint*/
                None, /*expression*/
                default_type,
            );
            let __hoist_1_170 = type_parameter_pos;
            self.finish_node(__hoist_1_169, __hoist_1_170)
        })
    }

    /// Go: `func (p *Parser) parseTemplateTagTypeParameters() *ast.TypeParameterList`.
    fn parse_template_tag_type_parameters(&mut self) -> NodeList {
        let mut nodes: Vec<NodeId> = Vec::new();
        loop {
            self.skip_whitespace();
            if let Some(node) = self.parse_template_tag_type_parameter() {
                nodes.push(node);
            }
            self.skip_whitespace_or_asterisk();
            if !self.parse_optional_jsdoc(Kind::CommaToken) {
                break;
            }
        }
        // PORT: Go constructs a zero-value `ast.TypeParameterList{}` (no loc).
        self.new_node_list(TextRange::new(0, 0), nodes)
    }

    /// Go: `func (p *Parser) parseTemplateTag(start int, tagName, indent int, indentText string) *ast.Node`.
    fn parse_template_tag(
        &mut self,
        start: i32,
        tag_name: NodeId,
        indent: i32,
        indent_text: &str,
    ) -> NodeId {
        // The template tag looks like one of the following:
        //   @template T,U,V
        //   @template {Constraint} T
        //
        // According to the [closure docs](https://github.com/google/closure-compiler/wiki/Generic-Types#multiple-bounded-template-types):
        //   > Multiple bounded generics cannot be declared on the same line. For the sake of clarity, if multiple templates share the same
        //   > type bound they must be declared on separate lines.
        //
        // TODO: Determine whether we should enforce this in the checker.
        // TODO: Consider moving the `constraint` to the first type parameter as we could then remove `getEffectiveConstraintOfTypeParameter`.
        // TODO: Consider only parsing a single type parameter if there is a constraint.
        let mut constraint: Option<NodeId> = None;
        if self.token == Kind::OpenBraceToken {
            constraint = Some(self.parse_jsdoc_type_expression(false));
        }
        let type_parameters = self.parse_template_tag_type_parameters();
        let comments = {
            let __hoist_1_172 = start;
            let __hoist_1_173 = self.node_pos();
            let __hoist_1_174 = indent;
            let __hoist_1_175 = indent_text;
            self.parse_trailing_tag_comments(
                __hoist_1_172,
                __hoist_1_173,
                __hoist_1_174,
                __hoist_1_175,
            )
        };
        let result = self.factory.new_jsdoc_template_tag(
            tag_name,
            constraint.unwrap_or(NodeId::NONE),
            Some(type_parameters),
            comments,
        );
        self.finish_node(result, start)
    }

    /// Go: `func (p *Parser) parseOptionalJsdoc(t ast.Kind) bool`.
    fn parse_optional_jsdoc(&mut self, t: Kind) -> bool {
        if self.token == t {
            self.next_token_jsdoc();
            return true;
        }
        false
    }

    /// Go: `func (p *Parser) parseJSDocEntityName(diagnosticMessage *diagnostics.Message) *ast.EntityName`.
    fn parse_jsdoc_entity_name(&mut self, diagnostic_message: Option<&'static Message>) -> NodeId {
        let mut entity = self.parse_jsdoc_identifier_name(diagnostic_message);
        if self.parse_optional(Kind::OpenBracketToken) {
            self.parse_expected(Kind::CloseBracketToken);
            // Note that y[] is accepted as an entity name, but the postfix brackets are not saved for checking.
            // Technically usejsdoc.org requires them for specifying a property of a type equivalent to Array<{ x: ...}>
            // but it's not worth it to enforce that restriction.
        }
        while self.parse_optional(Kind::DotToken) {
            let name =
                self.parse_jsdoc_identifier_name(Some(&tsc_diagnostics::IDENTIFIER_EXPECTED));
            if self.parse_optional(Kind::OpenBracketToken) {
                self.parse_expected(Kind::CloseBracketToken);
            }
            let pos = {
                let s = self.factory.store();
                s.node(entity).pos()
            };
            entity = {
                let __hoist_1_177 = self.factory.new_qualified_name(entity, name);
                let __hoist_1_178 = pos;
                self.finish_node(__hoist_1_177, __hoist_1_178)
            };
        }
        entity
    }

    /// Go: `func (p *Parser) parseJSDocIdentifierName(diagnosticMessage *diagnostics.Message) *ast.IdentifierNode`.
    fn parse_jsdoc_identifier_name(
        &mut self,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        if !token_is_identifier_or_keyword(self.token) {
            match diagnostic_message {
                Some(message) => {
                    self.parse_error_at_current_token(message, &[]);
                }
                None => {
                    if is_reserved_word(self.token) {
                        ({
                            let __hoist_1_180 =
                            &tsc_diagnostics::IDENTIFIER_EXPECTED_0_IS_A_RESERVED_WORD_THAT_CANNOT_BE_USED_HERE;
                            let __hoist_1_181 = &[self.scanner.token_text().to_string()];
                            self.parse_error_at_current_token(__hoist_1_180, __hoist_1_181)
                        });
                    }
                }
            }
            let id = self.new_identifier("");
            let pos = self.node_pos();
            return self.finish_node(id, pos);
        }
        let pos = self.scanner.token_start() as i32;
        let end = self.scanner.token_end() as i32;
        let text = self.token_value_string();
        self.next_token_jsdoc();
        let id = self.new_identifier(&text);
        self.finish_node_with_end(id, pos, end)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Package-level helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isJSDocLinkTag(kind string) bool`.
fn is_jsdoc_link_tag(kind: &str) -> bool {
    kind == "link" || kind == "linkcode" || kind == "linkplain"
}

/// Go: `func isObjectOrObjectArrayTypeReference(node *ast.TypeNode) bool`.
fn is_object_or_object_array_type_reference(s: &dyn NodeStore, node: NodeId) -> bool {
    let kind = s.node(node).kind;
    match kind {
        Kind::ObjectKeyword => true,
        Kind::ArrayType => {
            let element_type = s
                .node(node)
                .as_array_type_node()
                .expect("ArrayTypeNode data")
                .element_type;
            is_object_or_object_array_type_reference(s, element_type)
        }
        _ => {
            if tsc_ast::is_type_reference_node(s.node(node)) {
                let reference = s
                    .node(node)
                    .as_type_reference_node()
                    .expect("TypeReferenceNode data");
                let type_name_is_object = tsc_ast::is_identifier(s.node(reference.type_name))
                    && s.node(reference.type_name)
                        .as_identifier()
                        .expect("Identifier data")
                        .text
                        .as_ref()
                        == "Object";
                type_name_is_object && reference.type_arguments.is_none()
            } else {
                false
            }
        }
    }
}

/// Go: `func textsEqual(a, b *ast.EntityName) bool`.
fn texts_equal(s: &dyn NodeStore, mut a: NodeId, mut b: NodeId) -> bool {
    loop {
        let a_is_identifier = tsc_ast::is_identifier(s.node(a));
        let b_is_identifier = tsc_ast::is_identifier(s.node(b));
        if a_is_identifier && b_is_identifier {
            break;
        }
        if !a_is_identifier && !b_is_identifier {
            let (a_right, a_left) = {
                let d = s.node(a).as_qualified_name().expect("QualifiedName data");
                let right_text = s
                    .node(d.right)
                    .as_identifier()
                    .expect("Identifier data")
                    .text
                    .clone();
                (right_text, d.left)
            };
            let (b_right, b_left) = {
                let d = s.node(b).as_qualified_name().expect("QualifiedName data");
                let right_text = s
                    .node(d.right)
                    .as_identifier()
                    .expect("Identifier data")
                    .text
                    .clone();
                (right_text, d.left)
            };
            if a_right == b_right {
                a = a_left;
                b = b_left;
                continue;
            }
        }
        return false;
    }
    let a_text = s
        .node(a)
        .as_identifier()
        .expect("Identifier data")
        .text
        .clone();
    let b_text = s
        .node(b)
        .as_identifier()
        .expect("Identifier data")
        .text
        .clone();
    a_text == b_text
}

/// Go: `func removeLeadingNewlines(comments []string) []string`.
fn remove_leading_newlines(comments: &mut Vec<String>) {
    let mut i = 0;
    while i < comments.len() && comments[i].trim_start_matches(['\r', '\n']).is_empty() {
        i += 1;
    }
    comments.drain(0..i);
}

/// Go: `func trimEnd(s string) string` — TrimRightFunc(IsWhiteSpaceLike).
fn trim_end(s: &str) -> &str {
    s.trim_end_matches(|c: char| tsc_stringutil::is_white_space_like(c))
}

/// Go: `func removeTrailingWhitespace(comments []string) []string`.
fn remove_trailing_whitespace(comments: &mut Vec<String>) {
    let mut end = comments.len();
    for i in (0..comments.len()).rev() {
        let trimmed = trim_end(&comments[i]).to_string();
        if trimmed.is_empty() {
            end = i;
        } else {
            comments[i] = trimmed;
            break;
        }
    }
    comments.truncate(end);
}

/// Go: the `pushComment` closure in the two comment state machines.
fn push_comment(comments: &mut Vec<String>, margin: &mut i32, indent: &mut i32, text: &str) {
    if *margin == -1 {
        *margin = *indent;
    }
    comments.push(text.to_string());
    *indent += text.len() as i32;
}

/// Go: the shared `if state != jsdocStateSavingBackticks { ... }` state entry
/// used by the default arms of the comment state machines.
fn enter_saving_state(state: &mut JSDocState, in_fenced_code_block: bool) {
    if *state != JSDocState::SavingBackticks {
        if in_fenced_code_block {
            *state = JSDocState::SavingBackticks;
        } else {
            *state = JSDocState::SavingComments;
        }
    }
}

/// Go: `p.stringSliceArena.Clone(comments)` → `Vec<Box<str>>`
/// (the `JSDocText`/`JSDocLink*` payloads).
fn boxed_strings(comments: Vec<String>) -> Vec<Box<str>> {
    comments.into_iter().map(|s| s.into_boxed_str()).collect()
}
