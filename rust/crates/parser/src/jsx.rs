//! Go: `tsc/internal/parser/parser.go` (part 4) — the JSX family:
//! parseJsxElementOrSelfClosingElementOrFragment through parseJsxClosingFragment,
//! including the mismatched-tag restructure (`(<div>(...<span>...</div>)) -->
//! (<div>(...<span>...</>)</div>)`).

use tsc_ast::{tag_names_are_equivalent, Kind, NodeFlags, NodeId, NodeList};
use tsc_core::text::TextRange;

use crate::parser::Parser;
use crate::{PC_JSX_ATTRIBUTES, PC_JSX_CHILDREN};

impl<'a, 'f> Parser<'a, 'f> {
    /// Go: `func (p *Parser) parseJsxElementOrSelfClosingElementOrFragment(inExpressionContext bool, topInvalidNodePosition int, openingTag *ast.Node, mustBeUnary bool) *ast.Expression`.
    pub(crate) fn parse_jsx_element_or_self_closing_element_or_fragment(
        &mut self,
        in_expression_context: bool,
        top_invalid_node_position: i32,
        opening_tag: Option<NodeId>,
        must_be_unary: bool,
    ) -> NodeId {
        let pos = self.node_pos();
        let opening = self
            .parse_jsx_opening_or_self_closing_element_or_opening_fragment(in_expression_context);
        let opening_kind = {
            let s = self.factory.store();
            s.node(opening).kind
        };
        let mut result: NodeId;
        match opening_kind {
            Kind::JsxOpeningElement => {
                let mut children = self.parse_jsx_children(opening);
                let closing_element: NodeId;
                let last_child = children.nodes.last().copied();
                let mut restructure = false;
                if let Some(last_child) = last_child {
                    restructure = {
                        let s = self.factory.store();
                        let n = s.node(last_child);
                        n.kind == Kind::JsxElement && {
                            let d = n.as_jsx_element().expect("JsxElement data");
                            let opening_tag_name = s
                                .node(opening)
                                .as_jsx_opening_element()
                                .expect("JsxOpeningElement data")
                                .tag_name;
                            let last_opening_tag_name = s
                                .node(d.opening_element)
                                .as_jsx_opening_element()
                                .expect("JsxOpeningElement data")
                                .tag_name;
                            !tag_names_are_equivalent(s, last_opening_tag_name, d.closing_element)
                                && tag_names_are_equivalent(s, opening_tag_name, d.closing_element)
                        }
                    };
                }
                if restructure {
                    let last_child = children.nodes.last().copied().expect("checked above");
                    // when an unclosed JsxOpeningElement incorrectly parses its parent's JsxClosingElement,
                    // restructure (<div>(...<span>...</div>)) --> (<div>(...<span>...</>)</div>)
                    // (no need to error; the parent will error)
                    let end = {
                        let s = self.factory.store();
                        let d = s
                            .node(last_child)
                            .as_jsx_element()
                            .expect("JsxElement data");
                        d.children.as_ref().map(|c| c.loc.end()).unwrap_or_default()
                    };
                    let missing_identifier = self.new_identifier("");
                    let missing_identifier =
                        self.finish_node_with_end(missing_identifier, end, end);
                    let new_closing_element =
                        self.factory.new_jsx_closing_element(missing_identifier);
                    let new_closing_element =
                        self.finish_node_with_end(new_closing_element, end, end);
                    let (last_opening_element, last_children, last_opening_pos) = {
                        let s = self.factory.store();
                        let d = s
                            .node(last_child)
                            .as_jsx_element()
                            .expect("JsxElement data");
                        (
                            d.opening_element,
                            d.children.clone(),
                            s.node(d.opening_element).pos(),
                        )
                    };
                    let new_last = self.factory.new_jsx_element(
                        last_opening_element,
                        last_children.clone(),
                        new_closing_element,
                    );
                    let new_last = self.finish_node_with_end(new_last, last_opening_pos, end);
                    // force reset parent pointers from discarded parse result
                    {
                        let s = self.factory.store();
                        s.node_mut(last_opening_element).parent.set(new_last);
                        if let Some(c) = last_children.as_ref() {
                            for &child in &c.nodes {
                                s.node_mut(child).parent.set(new_last);
                            }
                        }
                        s.node_mut(new_closing_element).parent.set(new_last);
                    }
                    let new_last_end = {
                        let s = self.factory.store();
                        s.node(new_last).end()
                    };
                    let mut nodes = children.nodes.to_vec();
                    nodes.pop();
                    nodes.push(new_last);
                    children =
                        self.new_node_list(TextRange::new(children.loc.pos(), new_last_end), nodes);
                    closing_element = {
                        let s = self.factory.store();
                        s.node(last_child)
                            .as_jsx_element()
                            .expect("JsxElement data")
                            .closing_element
                    };
                } else {
                    closing_element =
                        self.parse_jsx_closing_element(opening, in_expression_context);
                    let (opening_tag_name, closing_tag_name) = {
                        let s = self.factory.store();
                        (
                            s.node(opening)
                                .as_jsx_opening_element()
                                .expect("JsxOpeningElement data")
                                .tag_name,
                            s.node(closing_element)
                                .as_jsx_closing_element()
                                .expect("JsxClosingElement data")
                                .tag_name,
                        )
                    };
                    if !tag_names_are_equivalent(
                        self.factory.store(),
                        opening_tag_name,
                        closing_tag_name,
                    ) {
                        let opening_tag_is_match = match opening_tag {
                            Some(opening_tag) => {
                                let s = self.factory.store();
                                tsc_ast::is_jsx_opening_element(s.node(opening_tag)) && {
                                    let parent_tag_name = s
                                        .node(opening_tag)
                                        .as_jsx_opening_element()
                                        .expect("JsxOpeningElement data")
                                        .tag_name;
                                    tag_names_are_equivalent(s, closing_tag_name, parent_tag_name)
                                }
                            }
                            None => false,
                        };
                        if opening_tag_is_match {
                            // opening incorrectly matched with its parent's closing -- put error on opening
                            let (loc, text) = {
                                let s = self.factory.store();
                                let tag_name = s
                                    .node(opening)
                                    .as_jsx_opening_element()
                                    .expect("JsxOpeningElement data")
                                    .tag_name;
                                (
                                    s.node(tag_name).loc,
                                    tsc_scanner::utilities::get_text_of_node_from_source_text(
                                        self.source_text,
                                        s,
                                        tag_name,
                                        false, /*includeTrivia*/
                                    )
                                    .into_owned(),
                                )
                            };
                            self.parse_error_at_range(
                                loc,
                                &tsc_diagnostics::JSX_ELEMENT_0_HAS_NO_CORRESPONDING_CLOSING_TAG,
                                &[text],
                            );
                        } else {
                            // other opening/closing mismatches -- put error on closing
                            let (closing_tag_loc, opening_tag_text) = {
                                let s = self.factory.store();
                                let opening_tag_name = s
                                    .node(opening)
                                    .as_jsx_opening_element()
                                    .expect("JsxOpeningElement data")
                                    .tag_name;
                                let closing_tag_name = s
                                    .node(closing_element)
                                    .as_jsx_closing_element()
                                    .expect("JsxClosingElement data")
                                    .tag_name;
                                (
                                    s.node(closing_tag_name).loc,
                                    tsc_scanner::utilities::get_text_of_node_from_source_text(
                                        self.source_text,
                                        s,
                                        opening_tag_name,
                                        false, /*includeTrivia*/
                                    )
                                    .into_owned(),
                                )
                            };
                            self.parse_error_at_range(
                                closing_tag_loc,
                                &tsc_diagnostics::EXPECTED_CORRESPONDING_JSX_CLOSING_TAG_FOR_0,
                                &[opening_tag_text],
                            );
                        }
                    }
                }
                result = {
                    let __hoist_1_2 =
                        self.factory
                            .new_jsx_element(opening, Some(children), closing_element);
                    let __hoist_1_3 = pos;
                    self.finish_node(__hoist_1_2, __hoist_1_3)
                };
                // force reset parent pointers from possibly discarded parse result
                {
                    let s = self.factory.store();
                    s.node_mut(closing_element).parent.set(result);
                }
            }
            Kind::JsxOpeningFragment => {
                let children = self.parse_jsx_children(opening);
                let closing_fragment = self.parse_jsx_closing_fragment(in_expression_context);
                result = {
                    let __hoist_1_5 =
                        self.factory
                            .new_jsx_fragment(opening, Some(children), closing_fragment);
                    let __hoist_1_6 = pos;
                    self.finish_node(__hoist_1_5, __hoist_1_6)
                };
            }
            Kind::JsxSelfClosingElement => {
                // Nothing else to do for self-closing elements
                result = opening;
            }
            _ => panic!("Unhandled case in parseJsxElementOrSelfClosingElementOrFragment"),
        }
        // If the user writes the invalid code '<div></div><div></div>' in an expression context (i.e. not wrapped in
        // an enclosing tag), we'll naively try to parse   ^ this as a 'less than' operator and the remainder of the tag
        // as garbage, which will cause the formatter to badly mangle the JSX. Perform a speculative parse of a JSX
        // element if we see a < token so that we can wrap it in a synthetic binary expression so the formatter
        // does less damage and we can report a better error.
        // Since JSX elements are invalid < operands anyway, this lookahead parse will only occur in error scenarios
        // of one sort or another.
        // If we are in a unary context, we can't do this recovery; the binary expression we return here is not
        // a valid UnaryExpression and will cause problems later.
        if !must_be_unary && in_expression_context && self.token == Kind::LessThanToken {
            let mut top_bad_pos = top_invalid_node_position;
            if top_bad_pos < 0 {
                top_bad_pos = {
                    let s = self.factory.store();
                    s.node(result).pos()
                };
            }
            let invalid_element = self.parse_jsx_element_or_self_closing_element_or_fragment(
                /*inExpressionContext*/ true,
                top_bad_pos,
                None,
                false,
            );
            let (invalid_pos, invalid_end) = {
                let s = self.factory.store();
                let n = s.node(invalid_element);
                (n.pos(), n.end())
            };
            let operator_token = self.factory.new_token(Kind::CommaToken);
            {
                let s = self.factory.store();
                let t = s.node_mut(operator_token);
                t.loc = TextRange::new(invalid_pos, invalid_pos);
            }
            self.parse_error_at(
                tsc_scanner::skip_trivia(self.source_text, top_bad_pos),
                invalid_end,
                &tsc_diagnostics::JSX_EXPRESSIONS_MUST_HAVE_ONE_PARENT_ELEMENT,
                &[],
            );
            result = {
                let __hoist_1_8 = self.factory.new_binary_expression(
                    None, /*modifiers*/
                    result,
                    None, /*typeNode*/
                    operator_token,
                    invalid_element,
                );
                let __hoist_1_9 = pos;
                self.finish_node(__hoist_1_8, __hoist_1_9)
            };
        }
        result
    }

    /// Go: `func (p *Parser) parseJsxChildren(openingTag *ast.Expression) *ast.NodeList`.
    fn parse_jsx_children(&mut self, opening_tag: NodeId) -> NodeList {
        let pos = self.node_pos();
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << PC_JSX_CHILDREN;
        let mut list: Vec<NodeId> = Vec::new();
        loop {
            // Go: `currentToken := p.scanner.ReScanJsxToken(true)` — the
            // parser-level `p.token` is deliberately NOT updated here; the
            // child parsers keep it in sync on their exit paths (see
            // parse_jsx_text / parse_jsx_expression / parse_jsx_closing_element).
            let current_token = self
                .scanner
                .re_scan_jsx_token(true /*allowMultilineJsxText*/);
            let child = self.parse_jsx_child(opening_tag, current_token);
            let Some(child) = child else { break };
            let mut stop = false;
            {
                let s = self.factory.store();
                let child_kind = s.node(child).kind;
                if child_kind == Kind::JsxElement
                    && tsc_ast::is_jsx_opening_element(s.node(opening_tag))
                {
                    let d = s.node(child).as_jsx_element().expect("JsxElement data");
                    let opening_tag_name = s
                        .node(opening_tag)
                        .as_jsx_opening_element()
                        .expect("JsxOpeningElement data")
                        .tag_name;
                    let child_opening_tag_name = s
                        .node(d.opening_element)
                        .as_jsx_opening_element()
                        .expect("JsxOpeningElement data")
                        .tag_name;
                    // stop after parsing a mismatched child like <div>...(<span></div>) in order to reattach the </div> higher
                    stop = !tag_names_are_equivalent(s, child_opening_tag_name, d.closing_element)
                        && tag_names_are_equivalent(s, opening_tag_name, d.closing_element);
                }
            }
            list.push(child);
            if stop {
                break;
            }
        }
        self.parsing_contexts = save_parsing_contexts;
        {
            let __hoist_1_11 = TextRange::new(pos, self.node_pos());
            let __hoist_1_12 = list;
            self.new_node_list(__hoist_1_11, __hoist_1_12)
        }
    }

    /// Go: `func (p *Parser) parseJsxChild(openingTag *ast.Node, token ast.Kind) *ast.Expression`.
    fn parse_jsx_child(&mut self, opening_tag: NodeId, token: Kind) -> Option<NodeId> {
        match token {
            Kind::EndOfFile => {
                // If we hit EOF, issue the error at the tag that lacks the closing element
                // rather than at the end of the file (which is useless)
                let is_opening_fragment = {
                    let s = self.factory.store();
                    tsc_ast::is_jsx_opening_fragment(s.node(opening_tag))
                };
                if is_opening_fragment {
                    let loc = {
                        let s = self.factory.store();
                        s.node(opening_tag).loc
                    };
                    self.parse_error_at_range(
                        loc,
                        &tsc_diagnostics::JSX_FRAGMENT_HAS_NO_CORRESPONDING_CLOSING_TAG,
                        &[],
                    );
                } else {
                    // We want the error span to cover only 'Foo.Bar' in < Foo.Bar >
                    // or to cover only 'Foo' in < Foo >
                    let (tag_loc, tag_text) = {
                        let s = self.factory.store();
                        let tag_name = s
                            .node(opening_tag)
                            .as_jsx_opening_element()
                            .expect("JsxOpeningElement data")
                            .tag_name;
                        let n = s.node(tag_name);
                        (
                            n.loc,
                            tsc_scanner::utilities::get_text_of_node_from_source_text(
                                self.source_text,
                                s,
                                tag_name,
                                false, /*includeTrivia*/
                            )
                            .into_owned(),
                        )
                    };
                    let start = tsc_scanner::skip_trivia(self.source_text, tag_loc.pos())
                        .min(tag_loc.end());
                    self.parse_error_at(
                        start,
                        tag_loc.end(),
                        &tsc_diagnostics::JSX_ELEMENT_0_HAS_NO_CORRESPONDING_CLOSING_TAG,
                        &[tag_text],
                    );
                }
                None
            }
            Kind::LessThanSlashToken | Kind::ConflictMarkerTrivia => None,
            Kind::JsxText | Kind::JsxTextAllWhiteSpaces => Some(self.parse_jsx_text()),
            Kind::OpenBraceToken => self.parse_jsx_expression(false /*inExpressionContext*/),
            Kind::LessThanToken => {
                Some(self.parse_jsx_element_or_self_closing_element_or_fragment(
                    false, /*inExpressionContext*/
                    -1,    /*topInvalidNodePosition*/
                    Some(opening_tag),
                    false,
                ))
            }
            _ => panic!("Unhandled case in parseJsxChild"),
        }
    }

    /// Go: `func (p *Parser) parseJsxText() *ast.Node`.
    fn parse_jsx_text(&mut self) -> NodeId {
        let pos = self.node_pos();
        // Go: `p.scanner.TokenValue()`.
        let text = self.token_value_string();
        let result = self
            .factory
            .new_jsx_text(&text, self.token == Kind::JsxTextAllWhiteSpaces);
        self.scan_jsx_text();
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) parseJsxExpression(inExpressionContext bool) *ast.Node`.
    fn parse_jsx_expression(&mut self, in_expression_context: bool) -> Option<NodeId> {
        let pos = self.node_pos();
        if !self.parse_expected(Kind::OpenBraceToken) {
            return None;
        }
        let mut dot_dot_dot_token: Option<NodeId> = None;
        let mut expression: Option<NodeId> = None;
        if self.token != Kind::CloseBraceToken {
            if !in_expression_context {
                dot_dot_dot_token = self.parse_optional_token(Kind::DotDotDotToken);
            }
            // Only an AssignmentExpression is valid here per the JSX spec,
            // but we can unambiguously parse a comma sequence and provide
            // a better error message in grammar checking.
            expression = Some(self.parse_expression());
        }
        if in_expression_context {
            self.parse_expected(Kind::CloseBraceToken);
        } else if self.parse_expected_without_advancing(Kind::CloseBraceToken) {
            // manually advance the scanner in order to look for jsx text inside jsx
            self.scan_jsx_text();
        }
        Some({
            let __hoist_1_14 = self
                .factory
                .new_jsx_expression(dot_dot_dot_token, expression);
            let __hoist_1_15 = pos;
            self.finish_node(__hoist_1_14, __hoist_1_15)
        })
    }

    /// Go: `func (p *Parser) scanJsxText() ast.Kind`.
    fn scan_jsx_text(&mut self) -> Kind {
        self.token = self.scanner.scan_jsx_token();
        self.token
    }

    /// Go: `func (p *Parser) scanJsxIdentifier() ast.Kind`.
    fn scan_jsx_identifier(&mut self) -> Kind {
        self.token = self.scanner.scan_jsx_identifier();
        self.token
    }

    /// Go: `func (p *Parser) scanJsxAttributeValue() ast.Kind`.
    fn scan_jsx_attribute_value(&mut self) -> Kind {
        self.token = self.scanner.scan_jsx_attribute_value();
        self.token
    }

    /// Go: `func (p *Parser) parseJsxClosingElement(open *ast.Node, inExpressionContext bool) *ast.Node`.
    fn parse_jsx_closing_element(&mut self, open: NodeId, in_expression_context: bool) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::LessThanSlashToken);
        let tag_name = self.parse_jsx_element_name();
        let open_tag_name = {
            let s = self.factory.store();
            s.node(open)
                .as_jsx_opening_element()
                .expect("JsxOpeningElement data")
                .tag_name
        };
        if self.parse_expected_with_diagnostic(
            Kind::GreaterThanToken,
            None,  /*diagnosticMessage*/
            false, /*shouldAdvance*/
        ) {
            // manually advance the scanner in order to look for jsx text inside jsx
            if in_expression_context
                || !tag_names_are_equivalent(self.factory.store(), open_tag_name, tag_name)
            {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        {
            let __hoist_1_17 = self.factory.new_jsx_closing_element(tag_name);
            let __hoist_1_18 = pos;
            self.finish_node(__hoist_1_17, __hoist_1_18)
        }
    }

    /// Go: `func (p *Parser) parseJsxOpeningOrSelfClosingElementOrOpeningFragment(inExpressionContext bool) *ast.Expression`.
    fn parse_jsx_opening_or_self_closing_element_or_opening_fragment(
        &mut self,
        in_expression_context: bool,
    ) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::LessThanToken);
        if self.token == Kind::GreaterThanToken {
            // See below for explanation of scanJsxText
            self.scan_jsx_text();
            return {
                let __hoist_1_20 = self.factory.new_jsx_opening_fragment();
                let __hoist_1_21 = pos;
                self.finish_node(__hoist_1_20, __hoist_1_21)
            };
        }
        let tag_name = self.parse_jsx_element_name();
        let mut type_arguments: Option<NodeList> = None;
        if !self.context_flags.intersects(NodeFlags::JAVASCRIPT_FILE) {
            type_arguments = self.parse_type_arguments();
        }
        let attributes = self.parse_jsx_attributes();

        let result: NodeId = if self.token == Kind::GreaterThanToken {
            // Closing tag, so scan the immediately-following text with the JSX scanning instead
            // of regular scanning to avoid treating illegal characters (e.g. '#') as immediate
            // scanning errors
            self.scan_jsx_text();
            self.factory
                .new_jsx_opening_element(tag_name, type_arguments, attributes)
        } else {
            self.parse_expected(Kind::SlashToken);
            if self.parse_expected_without_advancing(Kind::GreaterThanToken) {
                if in_expression_context {
                    self.next_token();
                } else {
                    self.scan_jsx_text();
                }
            }
            self.factory
                .new_jsx_self_closing_element(tag_name, type_arguments, attributes)
        };
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) parseJsxElementName() *ast.Expression`.
    fn parse_jsx_element_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        // JsxElement can have name in the form of
        //      propertyAccessExpression
        //      primaryExpression in the form of an identifier and "this" keyword
        // We can't just simply use parseLeftHandSideExpressionOrHigher because then we will start consider class,function etc as a keyword
        // We only want to consider "this" as a primaryExpression
        let initial_expression = self.parse_jsx_tag_name();
        let initial_is_namespaced = {
            let s = self.factory.store();
            tsc_ast::is_jsx_namespaced_name(s.node(initial_expression))
        };
        if initial_is_namespaced {
            // `a:b.c` is invalid syntax, don't even look for the `.` if we parse `a:b`, and let `parseAttribute` report "unexpected :" instead.
            return initial_expression;
        }
        let mut expression = initial_expression;
        while self.parse_optional(Kind::DotToken) {
            let name = self.parse_right_side_of_dot(
                true,  /*allowIdentifierNames*/
                false, /*allowPrivateIdentifiers*/
                false, /*allowUnicodeEscapeSequenceInIdentifierName*/
            );
            expression = {
                let __hoist_1_23 = self.factory.new_property_access_expression(
                    expression,
                    None,
                    name,
                    NodeFlags::NONE,
                );
                let __hoist_1_24 = pos;
                self.finish_node(__hoist_1_23, __hoist_1_24)
            };
        }
        expression
    }

    /// Go: `func (p *Parser) parseJsxTagName() *ast.Expression`.
    fn parse_jsx_tag_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let is_this = self.token == Kind::ThisKeyword;
        let tag_name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(Kind::ColonToken) {
            self.scan_jsx_identifier();
            let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
            return {
                let __hoist_1_26 = self.factory.new_jsx_namespaced_name(tag_name, name);
                let __hoist_1_27 = pos;
                self.finish_node(__hoist_1_26, __hoist_1_27)
            };
        }
        if is_this {
            let result = self.factory.new_keyword_expression(Kind::ThisKeyword);
            return self.finish_node(result, pos);
        }
        tag_name
    }

    /// Go: `func (p *Parser) parseJsxAttributes() *ast.Node`.
    fn parse_jsx_attributes(&mut self) -> NodeId {
        let pos = self.node_pos();
        let properties = self.parse_list(PC_JSX_ATTRIBUTES, |p| Some(p.parse_jsx_attribute()));
        {
            let __hoist_1_29 = self.factory.new_jsx_attributes(properties);
            let __hoist_1_30 = pos;
            self.finish_node(__hoist_1_29, __hoist_1_30)
        }
    }

    /// Go: `func (p *Parser) parseJsxAttribute() *ast.Node`.
    fn parse_jsx_attribute(&mut self) -> NodeId {
        if self.token == Kind::OpenBraceToken {
            return self.parse_jsx_spread_attribute();
        }
        let pos = self.node_pos();
        let name = self.parse_jsx_attribute_name();
        let initializer = self.parse_jsx_attribute_value();
        {
            let __hoist_1_32 = self.factory.new_jsx_attribute(name, initializer);
            let __hoist_1_33 = pos;
            self.finish_node(__hoist_1_32, __hoist_1_33)
        }
    }

    /// Go: `func (p *Parser) parseJsxSpreadAttribute() *ast.Node`.
    fn parse_jsx_spread_attribute(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenBraceToken);
        self.parse_expected(Kind::DotDotDotToken);
        let expression = self.parse_expression();
        self.parse_expected(Kind::CloseBraceToken);
        {
            let __hoist_1_35 = self.factory.new_jsx_spread_attribute(expression);
            let __hoist_1_36 = pos;
            self.finish_node(__hoist_1_35, __hoist_1_36)
        }
    }

    /// Go: `func (p *Parser) parseJsxAttributeName() *ast.Node`.
    fn parse_jsx_attribute_name(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.scan_jsx_identifier();
        let attr_name = self.parse_identifier_name_error_on_unicode_escape_sequence();
        if self.parse_optional(Kind::ColonToken) {
            self.scan_jsx_identifier();
            let name = self.parse_identifier_name_error_on_unicode_escape_sequence();
            return {
                let __hoist_1_38 = self.factory.new_jsx_namespaced_name(attr_name, name);
                let __hoist_1_39 = pos;
                self.finish_node(__hoist_1_38, __hoist_1_39)
            };
        }
        attr_name
    }

    /// Go: `func (p *Parser) parseJsxAttributeValue() *ast.Expression`.
    fn parse_jsx_attribute_value(&mut self) -> Option<NodeId> {
        if self.token == Kind::EqualsToken {
            if self.scan_jsx_attribute_value() == Kind::StringLiteral {
                return Some(self.parse_literal_expression());
            }
            if self.token == Kind::OpenBraceToken {
                return self.parse_jsx_expression(/*inExpressionContext*/ true);
            }
            if self.token == Kind::LessThanToken {
                // An attribute value must be a single JsxAttributeValue, so don't allow the sibling-element
                // recovery to wrap it in a synthetic binary expression.
                return Some(self.parse_jsx_element_or_self_closing_element_or_fragment(
                    true, /*inExpressionContext*/
                    -1,   /*topInvalidNodePosition*/
                    None, /*openingTag*/
                    true, /*mustBeUnary*/
                ));
            }
            self.parse_error_at_current_token(&tsc_diagnostics::X_OR_JSX_ELEMENT_EXPECTED, &[]);
        }
        None
    }

    /// Go: `func (p *Parser) parseJsxClosingFragment(inExpressionContext bool) *ast.Node`.
    fn parse_jsx_closing_fragment(&mut self, in_expression_context: bool) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::LessThanSlashToken);
        if self.parse_expected_with_diagnostic(
            Kind::GreaterThanToken,
            Some(&tsc_diagnostics::EXPECTED_CORRESPONDING_CLOSING_TAG_FOR_JSX_FRAGMENT),
            false, /*shouldAdvance*/
        ) {
            // manually advance the scanner in order to look for jsx text inside jsx
            if in_expression_context {
                self.next_token();
            } else {
                self.scan_jsx_text();
            }
        }
        {
            let __hoist_1_41 = self.factory.new_jsx_closing_fragment();
            let __hoist_1_42 = pos;
            self.finish_node(__hoist_1_41, __hoist_1_42)
        }
    }
}
