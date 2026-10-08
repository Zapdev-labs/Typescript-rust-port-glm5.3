//! Go: `tsc/internal/parser/parser.go` (part 2) — the type grammar:
//! parseType through parseModifiersForConstructorType, plus the type-related
//! predicates (isStartOfType, isStartOfParameter) and the shared template
//! token primitives (parseTemplateHead / parseTemplateMiddleOrTail /
//! parseLiteralOfTemplateSpan) that Go keeps in this region and the
//! expression half reuses.

use tsc_ast::{Kind, ModifierList, NodeFlags, NodeId, NodeList, TokenFlags};
use tsc_core::text::{TextPos, TextRange};
use tsc_diagnostics::Message;
use tsc_scanner::token_to_text;

use crate::parser::{new_diagnostic, JSDocScannerInfo, Parser};
use crate::{
    token_is_identifier_or_keyword, ParseFlags, PARSE_FLAGS_AWAIT,
    PARSE_FLAGS_IGNORE_MISSING_OPEN_BRACE, PARSE_FLAGS_NONE, PARSE_FLAGS_TYPE, PARSE_FLAGS_YIELD,
    PC_IMPORT_ATTRIBUTES, PC_PARAMETERS, PC_TUPLE_ELEMENT_TYPES, PC_TYPE_ARGUMENTS,
    PC_TYPE_MEMBERS, PC_TYPE_PARAMETERS,
};

impl<'a, 'f> Parser<'a, 'f> {
    // ────────────────────────────────────────────────────────────────────────
    // Types
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseType() *ast.TypeNode`.
    pub(crate) fn parse_type(&mut self) -> NodeId {
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::TYPE_EXCLUDES_FLAGS, false);
        let mut type_node: NodeId;
        if self.is_start_of_function_type_or_constructor_type() {
            type_node = self.parse_function_or_constructor_type();
        } else {
            let pos = self.node_pos();
            type_node = self.parse_union_type_or_higher();
            if !self.in_disallow_conditional_types_context()
                && !self.has_preceding_line_break()
                && self.parse_optional(Kind::ExtendsKeyword)
            {
                // The type following 'extends' is not permitted to be another conditional type
                let extends_type =
                    self.do_in_context(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT, true, |p| {
                        p.parse_type()
                    });
                self.parse_expected(Kind::QuestionToken);
                let true_type =
                    self.do_in_context(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT, false, |p| {
                        p.parse_type()
                    });
                self.parse_expected(Kind::ColonToken);
                let false_type =
                    self.do_in_context(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT, false, |p| {
                        p.parse_type()
                    });
                let conditional_type = {
                    let __hoist_1_2 = self.factory.new_conditional_type_node(
                        type_node,
                        extends_type,
                        true_type,
                        false_type,
                    );
                    let __hoist_1_3 = pos;
                    self.finish_node(__hoist_1_2, __hoist_1_3)
                };
                type_node = conditional_type;
            }
        }
        self.context_flags = save_context_flags;
        type_node
    }

    /// Go: `func (p *Parser) parseUnionTypeOrHigher() *ast.TypeNode`.
    pub(crate) fn parse_union_type_or_higher(&mut self) -> NodeId {
        self.parse_union_or_intersection_type(Kind::BarToken, |p| {
            p.parse_intersection_type_or_higher()
        })
    }

    /// Go: `func (p *Parser) parseIntersectionTypeOrHigher() *ast.TypeNode`.
    pub(crate) fn parse_intersection_type_or_higher(&mut self) -> NodeId {
        self.parse_union_or_intersection_type(Kind::AmpersandToken, |p| {
            p.parse_type_operator_or_higher()
        })
    }

    /// Go: `func (p *Parser) parseUnionOrIntersectionType(operator ast.Kind, parseConstituentType func(p *Parser) *ast.TypeNode) *ast.TypeNode`.
    pub(crate) fn parse_union_or_intersection_type<F>(
        &mut self,
        operator: Kind,
        mut parse_constituent_type: F,
    ) -> NodeId
    where
        F: FnMut(&mut Parser<'a, 'f>) -> NodeId,
    {
        let pos = self.node_pos();
        let is_union_type = operator == Kind::BarToken;
        let has_leading_operator = self.parse_optional(operator);
        let mut type_node: NodeId;
        if has_leading_operator {
            type_node = self.parse_function_or_constructor_type_to_error(
                is_union_type,
                &mut parse_constituent_type,
            );
        } else {
            type_node = parse_constituent_type(self);
        }
        if self.token == operator || has_leading_operator {
            let mut types: Vec<NodeId> = Vec::with_capacity(8);
            types.push(type_node);
            while self.parse_optional(operator) {
                types.push(self.parse_function_or_constructor_type_to_error(
                    is_union_type,
                    &mut parse_constituent_type,
                ));
            }
            let list = {
                let __hoist_1_5 = TextRange::new(pos, self.node_pos());
                let __hoist_1_6 = types;
                self.new_node_list(__hoist_1_5, __hoist_1_6)
            };
            type_node = self.create_union_or_intersection_type_node(operator, Some(list));
            self.finish_node(type_node, pos);
        }
        type_node
    }

    /// Go: `func (p *Parser) createUnionOrIntersectionTypeNode(operator ast.Kind, types *ast.NodeList) *ast.Node`.
    fn create_union_or_intersection_type_node(
        &mut self,
        operator: Kind,
        types: Option<NodeList>,
    ) -> NodeId {
        match operator {
            Kind::BarToken => self.factory.new_union_type_node(types),
            Kind::AmpersandToken => self.factory.new_intersection_type_node(types),
            _ => panic!("Unhandled case in createUnionOrIntersectionType"),
        }
    }

    /// Go: `func (p *Parser) parseTypeOperatorOrHigher() *ast.TypeNode`.
    pub(crate) fn parse_type_operator_or_higher(&mut self) -> NodeId {
        let operator = self.token;
        match operator {
            Kind::KeyOfKeyword | Kind::UniqueKeyword | Kind::ReadonlyKeyword => {
                self.parse_type_operator(operator)
            }
            Kind::InferKeyword => self.parse_infer_type(),
            _ => self.do_in_context(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT, false, |p| {
                p.parse_postfix_type_or_higher()
            }),
        }
    }

    /// Go: `func (p *Parser) parseTypeOperator(operator ast.Kind) *ast.Node`.
    fn parse_type_operator(&mut self, operator: Kind) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(operator);
        let inner = self.parse_type_operator_or_higher();
        {
            let __hoist_1_8 = self.factory.new_type_operator_node(operator, inner);
            let __hoist_1_9 = pos;
            self.finish_node(__hoist_1_8, __hoist_1_9)
        }
    }

    /// Go: `func (p *Parser) parseInferType() *ast.Node`.
    fn parse_infer_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::InferKeyword);
        let type_parameter = self.parse_type_parameter_of_infer_type();
        {
            let __hoist_1_11 = self.factory.new_infer_type_node(type_parameter);
            let __hoist_1_12 = pos;
            self.finish_node(__hoist_1_11, __hoist_1_12)
        }
    }

    /// Go: `func (p *Parser) parseTypeParameterOfInferType() *ast.Node`.
    fn parse_type_parameter_of_infer_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = self.parse_identifier();
        let constraint = self.try_parse_constraint_of_infer_type();
        {
            let __hoist_1_14 = self.factory.new_type_parameter_declaration(
                None, /*modifiers*/
                name, constraint, None, /*expression*/
                None, /*defaultType*/
            );
            let __hoist_1_15 = pos;
            self.finish_node(__hoist_1_14, __hoist_1_15)
        }
    }

    /// Go: `func (p *Parser) tryParseConstraintOfInferType() *ast.Node`.
    fn try_parse_constraint_of_infer_type(&mut self) -> Option<NodeId> {
        let state = self.mark();
        if self.parse_optional(Kind::ExtendsKeyword) {
            let constraint =
                self.do_in_context(NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT, true, |p| {
                    p.parse_type()
                });
            if self.in_disallow_conditional_types_context() || self.token != Kind::QuestionToken {
                return Some(constraint);
            }
        }
        self.rewind(state);
        None
    }

    /// Go: `func (p *Parser) parsePostfixTypeOrHigher() *ast.Node`.
    pub(crate) fn parse_postfix_type_or_higher(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut type_node = self.parse_non_array_type();
        while !self.has_preceding_line_break() {
            match self.token {
                Kind::ExclamationToken => {
                    self.next_token();
                    type_node = {
                        let __hoist_1_17 = self.factory.new_jsdoc_non_nullable_type(type_node);
                        let __hoist_1_18 = pos;
                        self.finish_node(__hoist_1_17, __hoist_1_18)
                    };
                }
                Kind::QuestionToken => {
                    // If next token is start of a type we have a conditional type
                    if self.look_ahead(|p| p.next_is_start_of_type()) {
                        return type_node;
                    }
                    self.next_token();
                    type_node = {
                        let __hoist_1_20 = self.factory.new_jsdoc_nullable_type(type_node);
                        let __hoist_1_21 = pos;
                        self.finish_node(__hoist_1_20, __hoist_1_21)
                    };
                }
                Kind::OpenBracketToken => {
                    self.parse_expected(Kind::OpenBracketToken);
                    if self.is_start_of_type(false /*inStartOfParameter*/) {
                        let index_type = self.parse_type();
                        self.parse_expected(Kind::CloseBracketToken);
                        type_node = {
                            let __hoist_1_23 = self
                                .factory
                                .new_indexed_access_type_node(type_node, index_type);
                            let __hoist_1_24 = pos;
                            self.finish_node(__hoist_1_23, __hoist_1_24)
                        };
                    } else {
                        self.parse_expected(Kind::CloseBracketToken);
                        type_node = {
                            let __hoist_1_26 = self.factory.new_array_type_node(type_node);
                            let __hoist_1_27 = pos;
                            self.finish_node(__hoist_1_26, __hoist_1_27)
                        };
                    }
                }
                _ => return type_node,
            }
        }
        type_node
    }

    /// Go: `func (p *Parser) nextIsStartOfType() bool`.
    fn next_is_start_of_type(&mut self) -> bool {
        self.next_token();
        self.is_start_of_type(false /*inStartOfParameter*/)
    }

    /// Go: `func (p *Parser) parseNonArrayType() *ast.Node`.
    pub(crate) fn parse_non_array_type(&mut self) -> NodeId {
        match self.token {
            Kind::AnyKeyword
            | Kind::UnknownKeyword
            | Kind::StringKeyword
            | Kind::NumberKeyword
            | Kind::BigIntKeyword
            | Kind::SymbolKeyword
            | Kind::BooleanKeyword
            | Kind::UndefinedKeyword
            | Kind::NeverKeyword
            | Kind::ObjectKeyword => {
                let state = self.mark();
                let keyword_type_node = self.parse_keyword_type_node();
                // If these are followed by a dot then parse these out as a dotted type reference instead
                if self.token != Kind::DotToken {
                    return keyword_type_node;
                }
                self.rewind(state);
                self.parse_type_reference()
            }
            // Go: `case ast.KindAsteriskEqualsToken: p.scanner.ReScanAsteriskEqualsToken(); fallthrough`.
            //
            // PORT: Go leaves `p.token` stale after the rescan (the scanner's
            // token is updated; `p.token` only refreshes at the next
            // `nextToken`), and `parseJSDocAllType` immediately advances — so
            // not assigning `self.token` here is load-bearing (assigning it
            // would double-advance).
            Kind::AsteriskEqualsToken | Kind::AsteriskToken => {
                if self.token == Kind::AsteriskEqualsToken {
                    self.scanner.re_scan_asterisk_equals_token();
                }
                self.parse_jsdoc_all_type()
            }
            // Go: `case ast.KindQuestionQuestionToken: p.scanner.ReScanQuestionToken(); fallthrough`.
            // Same staleness rule as above.
            Kind::QuestionQuestionToken | Kind::QuestionToken => {
                if self.token == Kind::QuestionQuestionToken {
                    self.scanner.re_scan_question_token();
                }
                self.parse_jsdoc_nullable_type()
            }
            Kind::ExclamationToken => self.parse_jsdoc_non_nullable_type(),
            Kind::NoSubstitutionTemplateLiteral
            | Kind::StringLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::NullKeyword => self.parse_literal_type_node(false /*negative*/),
            Kind::MinusToken => {
                if self.look_ahead(|p| p.next_token_is_numeric_or_big_int_literal()) {
                    return self.parse_literal_type_node(true /*negative*/);
                }
                self.parse_type_reference()
            }
            Kind::VoidKeyword => self.parse_keyword_type_node(),
            Kind::ThisKeyword => {
                let this_keyword = self.parse_this_type_node();
                if self.token == Kind::IsKeyword && !self.has_preceding_line_break() {
                    return self.parse_this_type_predicate(this_keyword);
                }
                this_keyword
            }
            Kind::TypeOfKeyword => {
                if self.look_ahead(|p| p.next_is_start_of_type_of_import_type()) {
                    return self.parse_import_type();
                }
                self.parse_type_query()
            }
            Kind::OpenBraceToken => {
                if self.look_ahead(|p| p.next_is_start_of_mapped_type()) {
                    return self.parse_mapped_type();
                }
                self.parse_type_literal()
            }
            Kind::OpenBracketToken => self.parse_tuple_type(),
            Kind::OpenParenToken => self.parse_parenthesized_type(),
            Kind::ImportKeyword => self.parse_import_type(),
            Kind::AssertsKeyword => {
                if self.look_ahead(|p| p.next_token_is_identifier_or_keyword_on_same_line()) {
                    return self.parse_asserts_type_predicate();
                }
                self.parse_type_reference()
            }
            Kind::TemplateHead => self.parse_template_type(),
            _ => self.parse_type_reference(),
        }
    }

    /// Go: `func (p *Parser) parseKeywordTypeNode() *ast.Node`.
    pub(crate) fn parse_keyword_type_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        let result = self.factory.new_keyword_type_node(self.token);
        self.next_token();
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) parseThisTypeNode() *ast.Node`.
    pub(crate) fn parse_this_type_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        {
            let __hoist_1_29 = self.factory.new_this_type_node();
            let __hoist_1_30 = pos;
            self.finish_node(__hoist_1_29, __hoist_1_30)
        }
    }

    /// Go: `func (p *Parser) parseThisTypePredicate(lhs *ast.Node) *ast.Node`.
    fn parse_this_type_predicate(&mut self, lhs: NodeId) -> NodeId {
        self.next_token();
        let type_node = self.parse_type();
        let lhs_pos = {
            let s = self.factory.store();
            s.node(lhs).pos()
        };
        {
            let __hoist_1_32 = self.factory.new_type_predicate_node(
                None, /*assertsModifier*/
                lhs,
                Some(type_node),
            );
            let __hoist_1_33 = lhs_pos;
            self.finish_node(__hoist_1_32, __hoist_1_33)
        }
    }

    /// Go: `func (p *Parser) parseJSDocAllType() *ast.Node`.
    fn parse_jsdoc_all_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        {
            let __hoist_1_35 = self.factory.new_jsdoc_all_type();
            let __hoist_1_36 = pos;
            self.finish_node(__hoist_1_35, __hoist_1_36)
        }
    }

    /// Go: `func (p *Parser) parseJSDocNonNullableType() *ast.TypeNode`.
    pub(crate) fn parse_jsdoc_non_nullable_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let inner = self.parse_type_operator_or_higher();
        {
            let __hoist_1_38 = self.factory.new_jsdoc_non_nullable_type(inner);
            let __hoist_1_39 = pos;
            self.finish_node(__hoist_1_38, __hoist_1_39)
        }
    }

    /// Go: `func (p *Parser) parseJSDocNullableType() *ast.Node`.
    pub(crate) fn parse_jsdoc_nullable_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        // skip the ?
        self.next_token();
        let inner = self.parse_type_operator_or_higher();
        {
            let __hoist_1_41 = self.factory.new_jsdoc_nullable_type(inner);
            let __hoist_1_42 = pos;
            self.finish_node(__hoist_1_41, __hoist_1_42)
        }
    }

    /// Go: `func (p *Parser) parseJSDocType() *ast.TypeNode`.
    pub(crate) fn parse_jsdoc_type(&mut self) -> NodeId {
        self.scanner.set_skip_jsdoc_leading_asterisks(true);
        let pos = self.node_pos();

        let has_dot_dot_dot = self.parse_optional(Kind::DotDotDotToken);
        let mut t = self.parse_type_or_type_predicate();
        self.scanner.set_skip_jsdoc_leading_asterisks(false);
        if has_dot_dot_dot {
            t = {
                let __hoist_1_44 = self.factory.new_jsdoc_variadic_type(t);
                let __hoist_1_45 = pos;
                self.finish_node(__hoist_1_44, __hoist_1_45)
            };
        }
        if self.token == Kind::EqualsToken {
            self.next_token();
            return {
                let __hoist_1_47 = self.factory.new_jsdoc_optional_type(t);
                let __hoist_1_48 = pos;
                self.finish_node(__hoist_1_47, __hoist_1_48)
            };
        }
        t
    }

    /// Go: `func (p *Parser) parseLiteralTypeNode(negative bool) *ast.Node`.
    pub(crate) fn parse_literal_type_node(&mut self, negative: bool) -> NodeId {
        let pos = self.node_pos();
        if negative {
            self.next_token();
        }
        let mut expression: NodeId;
        if self.token == Kind::TrueKeyword
            || self.token == Kind::FalseKeyword
            || self.token == Kind::NullKeyword
        {
            expression = self.parse_keyword_expression();
        } else {
            expression = self.parse_literal_expression();
        }
        if negative {
            expression = {
                let __hoist_1_50 = self
                    .factory
                    .new_prefix_unary_expression(Kind::MinusToken, expression);
                let __hoist_1_51 = pos;
                self.finish_node(__hoist_1_50, __hoist_1_51)
            };
        }
        {
            let __hoist_1_53 = self.factory.new_literal_type_node(expression);
            let __hoist_1_54 = pos;
            self.finish_node(__hoist_1_53, __hoist_1_54)
        }
    }

    /// Go: `func (p *Parser) parseTypeReference() *ast.Node`.
    pub(crate) fn parse_type_reference(&mut self) -> NodeId {
        let pos = self.node_pos();
        let type_name = self.parse_entity_name_of_type_reference();
        let type_arguments = self.parse_type_arguments_of_type_reference();
        {
            let __hoist_1_56 = self
                .factory
                .new_type_reference_node(type_name, type_arguments);
            let __hoist_1_57 = pos;
            self.finish_node(__hoist_1_56, __hoist_1_57)
        }
    }

    /// Go: `func (p *Parser) parseEntityNameOfTypeReference() *ast.Node`.
    fn parse_entity_name_of_type_reference(&mut self) -> NodeId {
        self.parse_entity_name(
            true,  /*allowReservedWords*/
            false, /*allowPrivateName*/
            Some(&tsc_diagnostics::TYPE_EXPECTED),
        )
    }

    /// Go: `func (p *Parser) parseEntityName(allowReservedWords, allowPrivateName bool, diagnosticMessage *diagnostics.Message) *ast.Node`.
    pub(crate) fn parse_entity_name(
        &mut self,
        allow_reserved_words: bool,
        allow_private_name: bool,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        let pos = self.node_pos();
        let mut entity: NodeId;
        if allow_reserved_words {
            entity = self.parse_identifier_name_with_diagnostic(diagnostic_message);
        } else {
            entity = self.parse_identifier_with_diagnostic(diagnostic_message, None);
        }
        while self.parse_optional(Kind::DotToken) {
            if self.token == Kind::LessThanToken {
                // The entity is part of a JSDoc-style generic. We will use the gap between `typeName` and
                // `typeArguments` to report it as a grammar error in the checker.
                break;
            }
            let right = self.parse_right_side_of_dot(
                allow_reserved_words,
                allow_private_name,
                true, /*allowUnicodeEscapeSequenceInIdentifierName*/
            );
            entity = {
                let __hoist_1_59 = self.factory.new_qualified_name(entity, right);
                let __hoist_1_60 = pos;
                self.finish_node(__hoist_1_59, __hoist_1_60)
            };
        }
        entity
    }

    // ────────────────────────────────────────────────────────────────────────
    // Rescans
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) reScanLessThanToken() ast.Kind`.
    pub(crate) fn re_scan_less_than_token(&mut self) -> Kind {
        self.token = self.scanner.re_scan_less_than_token();
        self.token
    }

    /// Go: `func (p *Parser) reScanGreaterThanToken() ast.Kind`.
    pub(crate) fn re_scan_greater_than_token(&mut self) -> Kind {
        self.token = self.scanner.re_scan_greater_than_token();
        self.token
    }

    /// Go: `func (p *Parser) reScanSlashToken() ast.Kind` — the Go scanner's
    /// variadic `reportErrors` is omitted at this call site (falsy default).
    pub(crate) fn re_scan_slash_token(&mut self) -> Kind {
        self.token = self
            .scanner
            .re_scan_slash_token(false /*shouldReportErrors*/);
        self.token
    }

    /// Go: `func (p *Parser) reScanTemplateToken(isTaggedTemplate bool) ast.Kind`.
    pub(crate) fn re_scan_template_token(&mut self, is_tagged_template: bool) -> Kind {
        self.token = self.scanner.re_scan_template_token(is_tagged_template);
        self.token
    }

    // ────────────────────────────────────────────────────────────────────────
    // Type arguments / import types
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseTypeArgumentsOfTypeReference() *ast.NodeList`.
    fn parse_type_arguments_of_type_reference(&mut self) -> Option<NodeList> {
        if !self.has_preceding_line_break() && self.re_scan_less_than_token() == Kind::LessThanToken
        {
            return self.parse_type_arguments();
        }
        None
    }

    /// Go: `func (p *Parser) parseTypeArguments() *ast.NodeList`.
    pub(crate) fn parse_type_arguments(&mut self) -> Option<NodeList> {
        if self.token == Kind::LessThanToken {
            return self.parse_bracketed_list(
                PC_TYPE_ARGUMENTS,
                |p| Some(p.parse_type()),
                Kind::LessThanToken,
                Kind::GreaterThanToken,
            );
        }
        None
    }

    /// Go: `func (p *Parser) nextIsStartOfTypeOfImportType() bool`.
    fn next_is_start_of_type_of_import_type(&mut self) -> bool {
        self.next_token();
        self.token == Kind::ImportKeyword
    }

    /// Go: `func (p *Parser) parseImportType() *ast.Node`.
    pub(crate) fn parse_import_type(&mut self) -> NodeId {
        self.source_flags |= NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
        let pos = self.node_pos();
        let is_type_of = self.parse_optional(Kind::TypeOfKeyword);
        self.parse_expected(Kind::ImportKeyword);
        self.parse_expected(Kind::OpenParenToken);
        let type_node = self.parse_type();
        let mut attributes: Option<NodeId> = None;
        if self.parse_optional(Kind::CommaToken) {
            let open_brace_position = self.scanner.token_start() as i32;
            self.parse_expected(Kind::OpenBraceToken);
            let current_token = self.token;
            if current_token == Kind::WithKeyword || current_token == Kind::AssertKeyword {
                if current_token == Kind::AssertKeyword {
                    self.parse_error_at_current_token(
                        &tsc_diagnostics::IMPORT_ASSERTIONS_HAVE_BEEN_REPLACED_BY_IMPORT_ATTRIBUTES_USE_WITH_INSTEAD_OF_ASSERT,
                        &[],
                    );
                }
                self.next_token();
            } else {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::X_0_EXPECTED,
                    &[token_to_text(Kind::WithKeyword).to_string()],
                );
            }
            self.parse_expected(Kind::ColonToken);
            attributes = Some(self.parse_import_attributes(current_token, true /*skipKeyword*/));
            self.parse_optional(Kind::CommaToken);
            if !self.parse_expected(Kind::CloseBraceToken) {
                let diagnostics_len = self.sink.borrow().diagnostics.len();
                if diagnostics_len != 0 {
                    let last_index = diagnostics_len - 1;
                    if self.last_diagnostic_code(last_index) == tsc_diagnostics::X_0_EXPECTED.code()
                    {
                        let related = new_diagnostic(
                            TextRange::new(open_brace_position, open_brace_position),
                            &tsc_diagnostics::THE_PARSER_EXPECTED_TO_FIND_A_1_TO_MATCH_THE_0_TOKEN_HERE,
                            &["{".to_string(), "}".to_string()],
                        );
                        self.add_related_info(last_index, related);
                    }
                }
            }
        }
        self.parse_expected(Kind::CloseParenToken);
        let mut qualifier: Option<NodeId> = None;
        if self.parse_optional(Kind::DotToken) {
            qualifier = Some(self.parse_entity_name_of_type_reference());
        }
        let type_arguments = self.parse_type_arguments_of_type_reference();
        {
            let __hoist_1_62 = self.factory.new_import_type_node(
                is_type_of,
                type_node,
                attributes,
                qualifier,
                type_arguments,
            );
            let __hoist_1_63 = pos;
            self.finish_node(__hoist_1_62, __hoist_1_63)
        }
    }

    /// Go: `func (p *Parser) parseImportAttribute() *ast.Node`.
    pub(crate) fn parse_import_attribute(&mut self) -> Option<NodeId> {
        let pos = self.node_pos();
        let mut name: Option<NodeId> = None;
        if token_is_identifier_or_keyword(self.token) {
            name = Some(self.parse_identifier_name());
        } else if self.token == Kind::StringLiteral {
            name = Some(self.parse_literal_expression());
        }
        if name.is_some() {
            self.parse_expected(Kind::ColonToken);
        } else {
            self.parse_error_at_current_token(
                &tsc_diagnostics::IDENTIFIER_OR_STRING_LITERAL_EXPECTED,
                &[],
            );
        }
        let value = self.parse_assignment_expression_or_higher();
        Some({
            let __hoist_1_65 = self
                .factory
                .new_import_attribute(name.unwrap_or(NodeId::NONE), value);
            let __hoist_1_66 = pos;
            self.finish_node(__hoist_1_65, __hoist_1_66)
        })
    }

    /// Go: `func (p *Parser) parseImportAttributes(token ast.Kind, skipKeyword bool) *ast.Node`.
    pub(crate) fn parse_import_attributes(&mut self, token: Kind, skip_keyword: bool) -> NodeId {
        let pos = self.node_pos();
        if !skip_keyword {
            self.parse_expected(token);
        }
        let elements: Option<NodeList>;
        let mut multi_line = false;
        let open_brace_position = self.scanner.token_start() as i32;
        if self.parse_expected(Kind::OpenBraceToken) {
            multi_line = self.has_preceding_line_break();
            elements =
                self.parse_delimited_list(PC_IMPORT_ATTRIBUTES, |p| p.parse_import_attribute());
            if !self.parse_expected(Kind::CloseBraceToken) {
                let diagnostics_len = self.sink.borrow().diagnostics.len();
                if diagnostics_len != 0 {
                    let last_index = diagnostics_len - 1;
                    if self.last_diagnostic_code(last_index) == tsc_diagnostics::X_0_EXPECTED.code()
                    {
                        let related = new_diagnostic(
                            TextRange::new(open_brace_position, open_brace_position),
                            &tsc_diagnostics::THE_PARSER_EXPECTED_TO_FIND_A_1_TO_MATCH_THE_0_TOKEN_HERE,
                            &["{".to_string(), "}".to_string()],
                        );
                        self.add_related_info(last_index, related);
                    }
                }
            }
        } else {
            elements = Some(self.parse_empty_node_list());
        }
        {
            let __hoist_1_68 = self
                .factory
                .new_import_attributes(token, elements, multi_line);
            let __hoist_1_69 = pos;
            self.finish_node(__hoist_1_68, __hoist_1_69)
        }
    }

    /// Go: `func (p *Parser) parseTypeQuery() *ast.Node`.
    pub(crate) fn parse_type_query(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::TypeOfKeyword);
        let entity_name = self.parse_entity_name(
            true, /*allowReservedWords*/
            true, /*allowPrivateName*/
            None, /*diagnosticMessage*/
        );
        // Make sure we perform ASI to prevent parsing the next line's type arguments as part of an instantiation expression
        let mut type_arguments: Option<NodeList> = None;
        if !self.has_preceding_line_break() {
            type_arguments = self.parse_type_arguments();
        }
        {
            let __hoist_1_71 = self
                .factory
                .new_type_query_node(entity_name, type_arguments);
            let __hoist_1_72 = pos;
            self.finish_node(__hoist_1_71, __hoist_1_72)
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Mapped types / type members
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) nextIsStartOfMappedType() bool`.
    fn next_is_start_of_mapped_type(&mut self) -> bool {
        self.next_token();
        if self.token == Kind::PlusToken || self.token == Kind::MinusToken {
            return self.next_token() == Kind::ReadonlyKeyword;
        }
        if self.token == Kind::ReadonlyKeyword {
            self.next_token();
        }
        self.token == Kind::OpenBracketToken
            && self.next_token_is_identifier()
            && self.next_token() == Kind::InKeyword
    }

    /// Go: `func (p *Parser) parseMappedType() *ast.Node`.
    fn parse_mapped_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenBraceToken);
        let mut readonly_token: Option<NodeId> = None; // ReadonlyKeyword | PlusToken | MinusToken
        if self.token == Kind::ReadonlyKeyword
            || self.token == Kind::PlusToken
            || self.token == Kind::MinusToken
        {
            readonly_token = Some(self.parse_token_node());
            let readonly_kind = {
                let s = self.factory.store();
                s.node(readonly_token.unwrap()).kind
            };
            if readonly_kind != Kind::ReadonlyKeyword {
                self.parse_expected(Kind::ReadonlyKeyword);
            }
        }
        self.parse_expected(Kind::OpenBracketToken);
        let type_parameter = self.parse_mapped_type_parameter();
        let mut name_type: Option<NodeId> = None;
        if self.parse_optional(Kind::AsKeyword) {
            name_type = Some(self.parse_type());
        }
        self.parse_expected(Kind::CloseBracketToken);
        let mut question_token: Option<NodeId> = None; // QuestionToken | PlusToken | MinusToken
        if self.token == Kind::QuestionToken
            || self.token == Kind::PlusToken
            || self.token == Kind::MinusToken
        {
            question_token = Some(self.parse_token_node());
            let question_kind = {
                let s = self.factory.store();
                s.node(question_token.unwrap()).kind
            };
            if question_kind != Kind::QuestionToken {
                self.parse_expected(Kind::QuestionToken);
            }
        }
        let type_node = self.parse_type_annotation();
        self.parse_semicolon();
        let members = self.parse_list(PC_TYPE_MEMBERS, |p| Some(p.parse_type_member()));
        self.parse_expected(Kind::CloseBraceToken);
        {
            let __hoist_1_74 = self.factory.new_mapped_type_node(
                readonly_token,
                type_parameter,
                name_type,
                question_token,
                type_node,
                members,
            );
            let __hoist_1_75 = pos;
            self.finish_node(__hoist_1_74, __hoist_1_75)
        }
    }

    /// Go: `func (p *Parser) parseMappedTypeParameter() *ast.Node`.
    fn parse_mapped_type_parameter(&mut self) -> NodeId {
        let pos = self.node_pos();
        let name = self.parse_identifier_name();
        self.parse_expected(Kind::InKeyword);
        let type_node = self.parse_type();
        {
            let __hoist_1_77 = self.factory.new_type_parameter_declaration(
                None, /*modifiers*/
                name,
                Some(type_node),
                None, /*expression*/
                None, /*defaultType*/
            );
            let __hoist_1_78 = pos;
            self.finish_node(__hoist_1_77, __hoist_1_78)
        }
    }

    /// Go: `func (p *Parser) parseTypeMember() *ast.Node`.
    pub(crate) fn parse_type_member(&mut self) -> NodeId {
        if self.token == Kind::OpenParenToken || self.token == Kind::LessThanToken {
            return self.parse_signature_member(Kind::CallSignature);
        }
        if self.token == Kind::NewKeyword
            && self.look_ahead(|p| p.next_token_is_open_paren_or_less_than())
        {
            return self.parse_signature_member(Kind::ConstructSignature);
        }
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        if self.parse_contextual_modifier(Kind::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::GetAccessor,
                PARSE_FLAGS_TYPE,
            );
        }
        if self.parse_contextual_modifier(Kind::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::SetAccessor,
                PARSE_FLAGS_TYPE,
            );
        }
        if self.is_index_signature() {
            return self.parse_index_signature_declaration(pos, jsdoc, modifiers);
        }
        self.parse_property_or_method_signature(pos, jsdoc, modifiers)
    }

    /// Go: `func (p *Parser) nextTokenIsOpenParenOrLessThan() bool`.
    pub(crate) fn next_token_is_open_paren_or_less_than(&mut self) -> bool {
        self.next_token();
        self.token == Kind::OpenParenToken || self.token == Kind::LessThanToken
    }

    /// Go: `func (p *Parser) parseSignatureMember(kind ast.Kind) *ast.Node`.
    fn parse_signature_member(&mut self, kind: Kind) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if kind == Kind::ConstructSignature {
            self.parse_expected(Kind::NewKeyword);
        }
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(PARSE_FLAGS_TYPE);
        let type_node = self.parse_return_type(Kind::ColonToken, true /*isType*/);
        self.parse_type_member_semicolon();
        let result = if kind == Kind::CallSignature {
            self.factory
                .new_call_signature_declaration(type_parameters, parameters, type_node)
        } else {
            self.factory
                .new_construct_signature_declaration(type_parameters, parameters, type_node)
        };
        self.finish_node(result, pos);
        self.with_jsdoc(result, jsdoc);
        result
    }

    // ────────────────────────────────────────────────────────────────────────
    // Type parameters / parameters
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseTypeParameters() *ast.NodeList`.
    pub(crate) fn parse_type_parameters(&mut self) -> Option<NodeList> {
        if self.token == Kind::LessThanToken {
            return self.parse_bracketed_list(
                PC_TYPE_PARAMETERS,
                |p| Some(p.parse_type_parameter()),
                Kind::LessThanToken,
                Kind::GreaterThanToken,
            );
        }
        None
    }

    /// Go: `func (p *Parser) parseTypeParameter() *ast.Node`.
    pub(crate) fn parse_type_parameter(&mut self) -> NodeId {
        let pos = self.node_pos();
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ false, /*permitConstAsModifier*/ true,
            /*stopOnStartOfClassStaticBlock*/ false,
        );
        let name = self.parse_identifier();
        let mut constraint: Option<NodeId> = None;
        let mut expression: Option<NodeId> = None;
        if self.parse_optional(Kind::ExtendsKeyword) {
            // It's not uncommon for people to write improper constraints to a generic.  If the
            // user writes a constraint that is an expression and not an actual type, then parse
            // it out as an expression (so we can recover well), but report that a type is needed
            // instead.
            if self.is_start_of_type(false /*inStartOfParameter*/) || !self.is_start_of_expression()
            {
                constraint = Some(self.parse_type());
            } else {
                // It was not a type, and it looked like an expression.  Parse out an expression
                // here so we recover well.  Note: it is important that we call parseUnaryExpression
                // and not parseExpression here.  If the user has:
                //
                //      <T extends "">
                //
                // We do *not* want to consume the `>` as we're consuming the expression for "".
                expression = Some(self.parse_unary_expression_or_higher());
            }
        }
        let mut default_type: Option<NodeId> = None;
        if self.parse_optional(Kind::EqualsToken) {
            default_type = Some(self.parse_type());
        }
        let result = self.factory.new_type_parameter_declaration(
            modifiers,
            name,
            constraint,
            expression,
            default_type,
        );
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) parseParameters(flags ParseFlags) *ast.NodeList`.
    ///
    /// PORT: Go distinguishes the missing-list sentinel here (see the
    /// `last_parameters_list_missing` field docs); the latch is set for both
    /// the missing-open-paren case and the failed-element case (Go's
    /// `isMissingNodeList` also treats a nil list as missing) and consumed by
    /// `parse_function_or_constructor_type`.
    pub(crate) fn parse_parameters(&mut self, flags: ParseFlags) -> Option<NodeList> {
        self.last_parameters_list_missing = false;
        if self.parse_expected(Kind::OpenParenToken) {
            let parameters = self.parse_parameters_worker(flags, true /*allowAmbiguity*/);
            self.parse_expected(Kind::CloseParenToken);
            if parameters.is_none() {
                self.last_parameters_list_missing = true;
            }
            return parameters;
        }
        self.last_parameters_list_missing = true;
        Some(self.create_missing_list())
    }

    /// Go: `func (p *Parser) parseParametersWorker(flags ParseFlags, allowAmbiguity bool) *ast.NodeList`.
    pub(crate) fn parse_parameters_worker(
        &mut self,
        flags: ParseFlags,
        allow_ambiguity: bool,
    ) -> Option<NodeList> {
        let in_await_context = self.context_flags.intersects(NodeFlags::AWAIT_CONTEXT);
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, flags & PARSE_FLAGS_YIELD != 0);
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, flags & PARSE_FLAGS_AWAIT != 0);
        let parameters = self.parse_delimited_list(PC_PARAMETERS, |p| {
            let parameter = p.parse_parameter_ex(in_await_context, allow_ambiguity);
            if let Some(parameter) = parameter {
                if flags & PARSE_FLAGS_TYPE == 0 {
                    p.check_js_syntax(parameter);
                }
                return Some(parameter);
            }
            None
        });
        self.context_flags = save_context_flags;
        parameters
    }

    /// Go: `func (p *Parser) parseParameter() *ast.Node`.
    pub(crate) fn parse_parameter(&mut self) -> Option<NodeId> {
        self.parse_parameter_ex(
            false, /*inOuterAwaitContext*/
            true,  /*allowAmbiguity*/
        )
    }

    /// Go: `func (p *Parser) parseParameterEx(inOuterAwaitContext, allowAmbiguity bool) *ast.Node`.
    fn parse_parameter_ex(
        &mut self,
        in_outer_await_context: bool,
        allow_ambiguity: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        // FormalParameter [Yield,Await]:
        //      BindingElement[?Yield,Await]
        // Decorators are parsed in the outer [Await] context, the rest of the parameter is parsed in the function's [Await] context.
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, in_outer_await_context);
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ true, /*permitConstAsModifier*/ false,
            /*stopOnStartOfClassStaticBlock*/ false,
        );
        self.context_flags = save_context_flags;
        if self.token == Kind::ThisKeyword {
            let name = self.create_identifier(true /*isIdentifier*/);
            let type_annotation = self.parse_type_annotation();
            let result = self.factory.new_parameter_declaration(
                modifiers.clone(),
                None, /*dotDotDotToken*/
                name,
                None, /*questionToken*/
                type_annotation,
                None, /*initializer*/
            );
            if let Some(modifiers) = modifiers.as_ref() {
                let first_loc = {
                    let s = self.factory.store();
                    s.node(modifiers.nodes[0]).loc
                };
                self.parse_error_at_range(
                    first_loc,
                    &tsc_diagnostics::NEITHER_DECORATORS_NOR_MODIFIERS_MAY_BE_APPLIED_TO_THIS_PARAMETERS,
                    &[],
                );
            }
            let finished = self.finish_node(result, pos);
            self.with_jsdoc(finished, jsdoc);
            return Some(finished);
        }
        let dot_dot_dot_token = self.parse_optional_token(Kind::DotDotDotToken);
        if !allow_ambiguity && !self.is_parameter_name_start() {
            return None;
        }
        let name = self.parse_name_of_parameter(modifiers.as_ref());
        let question_token = self.parse_optional_token(Kind::QuestionToken);
        let type_annotation = self.parse_type_annotation();
        let initializer = self.parse_initializer();
        let result = self.factory.new_parameter_declaration(
            modifiers,
            dot_dot_dot_token,
            name,
            question_token,
            type_annotation,
            (initializer != NodeId::NONE).then_some(initializer),
        );
        let finished = self.finish_node(result, pos);
        self.with_jsdoc(finished, jsdoc);
        Some(finished)
    }

    /// Go: `func (p *Parser) isParameterNameStart() bool`.
    fn is_parameter_name_start(&mut self) -> bool {
        // Be permissive about await and yield by calling isBindingIdentifier instead of isIdentifier; disallowing
        // them during a speculative parse leads to many more follow-on errors than allowing the function to parse then later
        // complaining about the use of the keywords.
        self.is_binding_identifier()
            || self.token == Kind::OpenBracketToken
            || self.token == Kind::OpenBraceToken
    }

    /// Go: `func (p *Parser) parseNameOfParameter(modifiers *ast.ModifierList) *ast.Node`.
    fn parse_name_of_parameter(&mut self, modifiers: Option<&ModifierList>) -> NodeId {
        // FormalParameter [Yield,Await]:
        //      BindingElement[?Yield,?Await]
        let name = self.parse_identifier_or_pattern_with_diagnostic(Some(
            &tsc_diagnostics::PRIVATE_IDENTIFIERS_CANNOT_BE_USED_AS_PARAMETERS,
        ));
        let (name_len, modifiers_is_none) = {
            let s = self.factory.store();
            (s.node(name).loc.len(), modifiers.is_none())
        };
        if name_len == 0 && modifiers_is_none && tsc_ast::is_modifier_kind(self.token) {
            // in cases like
            // 'use strict'
            // function foo(static)
            // isParameter('static') == true, because of isModifier('static')
            // however 'static' is not a legal identifier in a strict mode.
            // so result of this function will be Parameter (flags = 0, name = missing, type = undefined, initializer = undefined)
            // and current token will not change => parsing of the enclosing parameter list will last till the end of time (or OOM)
            // to avoid this we'll advance cursor to the next token.
            self.next_token();
        }
        name
    }

    /// Go: `func (p *Parser) parseReturnType(returnToken ast.Kind, isType bool) *ast.TypeNode`.
    pub(crate) fn parse_return_type(
        &mut self,
        return_token: Kind,
        is_type: bool,
    ) -> Option<NodeId> {
        if self.should_parse_return_type(return_token, is_type) {
            return Some(self.do_in_context(
                NodeFlags::DISALLOW_CONDITIONAL_TYPES_CONTEXT,
                false,
                |p| p.parse_type_or_type_predicate(),
            ));
        }
        None
    }

    /// Go: `func (p *Parser) shouldParseReturnType(returnToken ast.Kind, isType bool) bool`.
    fn should_parse_return_type(&mut self, return_token: Kind, is_type: bool) -> bool {
        if return_token == Kind::EqualsGreaterThanToken {
            self.parse_expected(return_token);
            true
        } else if self.parse_optional(Kind::ColonToken) {
            true
        } else if is_type && self.token == Kind::EqualsGreaterThanToken {
            // This is easy to get backward, especially in type contexts, so parse the type anyway
            self.parse_error_at_current_token(
                &tsc_diagnostics::X_0_EXPECTED,
                &[token_to_text(Kind::ColonToken).to_string()],
            );
            self.next_token();
            true
        } else {
            false
        }
    }

    /// Go: `func (p *Parser) parseTypeOrTypePredicate() *ast.TypeNode`.
    fn parse_type_or_type_predicate(&mut self) -> NodeId {
        if self.is_identifier() {
            let state = self.mark();
            let pos = self.node_pos();
            let id = self.parse_identifier();
            if self.token == Kind::IsKeyword && !self.has_preceding_line_break() {
                self.next_token();
                let type_node = self.parse_type();
                return {
                    let __hoist_1_80 = self.factory.new_type_predicate_node(
                        None, /*assertsModifier*/
                        id,
                        Some(type_node),
                    );
                    let __hoist_1_81 = pos;
                    self.finish_node(__hoist_1_80, __hoist_1_81)
                };
            }
            self.rewind(state);
        }
        self.parse_type()
    }

    /// Go: `func (p *Parser) parseTypeMemberSemicolon()`.
    pub(crate) fn parse_type_member_semicolon(&mut self) {
        // We allow type members to be separated by commas or (possibly ASI) semicolons.
        // First check if it was a comma.  If so, we're done with the member.
        if self.parse_optional(Kind::CommaToken) {
            return;
        }
        // Didn't have a comma.  We must have a (possible ASI) semicolon.
        self.parse_semicolon();
    }

    /// Go: `func (p *Parser) parseAccessorDeclaration(pos int, jsdoc, modifiers, kind, flags) *ast.Node`.
    pub(crate) fn parse_accessor_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
        kind: Kind,
        flags: ParseFlags,
    ) -> NodeId {
        let name = self.parse_property_name();
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(PARSE_FLAGS_NONE);
        let return_type = self.parse_return_type(Kind::ColonToken, false /*isType*/);
        let body = self.parse_function_block_or_semicolon(flags, None /*diagnosticMessage*/);
        // Keep track of `typeParameters` (for both) and `type` (for setters) if they were parsed those indicate grammar errors
        let result = if kind == Kind::GetAccessor {
            self.factory.new_get_accessor_declaration(
                modifiers,
                name,
                type_parameters,
                parameters,
                return_type,
                None, /*fullSignature*/
                body,
            )
        } else {
            self.factory.new_set_accessor_declaration(
                modifiers,
                name,
                type_parameters,
                parameters,
                return_type,
                None, /*fullSignature*/
                body,
            )
        };
        let finished = self.finish_node(result, pos);
        self.with_jsdoc(finished, jsdoc);
        if flags & PARSE_FLAGS_TYPE == 0 {
            self.check_js_syntax(finished);
        }
        finished
    }

    /// Go: `func (p *Parser) parsePropertyName() *ast.Node`.
    pub(crate) fn parse_property_name(&mut self) -> NodeId {
        let save_has_await_identifier = self.statement_has_await_identifier;
        let prop = self.parse_property_name_worker(true /*allowComputedPropertyNames*/);
        self.statement_has_await_identifier = save_has_await_identifier;
        prop
    }

    /// Go: `func (p *Parser) parsePropertyNameWorker(allowComputedPropertyNames bool) *ast.Node`.
    fn parse_property_name_worker(&mut self, allow_computed_property_names: bool) -> NodeId {
        if self.token == Kind::StringLiteral
            || self.token == Kind::NumericLiteral
            || self.token == Kind::BigIntLiteral
        {
            return self.parse_literal_expression();
        }
        if allow_computed_property_names && self.token == Kind::OpenBracketToken {
            return self.parse_computed_property_name();
        }
        if self.token == Kind::PrivateIdentifier {
            return self.parse_private_identifier();
        }
        self.parse_identifier_name()
    }

    /// Go: `func (p *Parser) parseComputedPropertyName() *ast.Node`.
    pub(crate) fn parse_computed_property_name(&mut self) -> NodeId {
        // PropertyName [Yield]:
        //      LiteralPropertyName
        //      ComputedPropertyName[?Yield]
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenBracketToken);
        // We parse any expression (including a comma expression). But the grammar
        // says that only an assignment expression is allowed, so the grammar checker
        // will error if it sees a comma expression.
        let expression = self.parse_expression_allow_in();
        self.parse_expected(Kind::CloseBracketToken);
        {
            let __hoist_1_83 = self.factory.new_computed_property_name(expression);
            let __hoist_1_84 = pos;
            self.finish_node(__hoist_1_83, __hoist_1_84)
        }
    }

    /// Go: `func (p *Parser) parseFunctionBlockOrSemicolon(flags ParseFlags, diagnosticMessage) *ast.Node`.
    pub(crate) fn parse_function_block_or_semicolon(
        &mut self,
        flags: ParseFlags,
        diagnostic_message: Option<&'static Message>,
    ) -> Option<NodeId> {
        if self.token != Kind::OpenBraceToken {
            if flags & PARSE_FLAGS_TYPE != 0 {
                self.parse_type_member_semicolon();
                return None;
            }
            if self.can_parse_semicolon() {
                self.parse_semicolon();
                return None;
            }
        }
        Some(self.parse_function_block(flags, diagnostic_message))
    }

    /// Go: `func (p *Parser) parseFunctionBlock(flags ParseFlags, diagnosticMessage) *ast.Node`.
    pub(crate) fn parse_function_block(
        &mut self,
        flags: ParseFlags,
        diagnostic_message: Option<&'static Message>,
    ) -> NodeId {
        let save_context_flags = self.context_flags;
        let save_has_await_identifier = self.statement_has_await_identifier;
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, flags & PARSE_FLAGS_YIELD != 0);
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, flags & PARSE_FLAGS_AWAIT != 0);
        // We may be in a [Decorator] context when parsing a function expression or
        // arrow function. The body of the function is not in [Decorator] context.
        self.set_context_flags(NodeFlags::DECORATOR_CONTEXT, false);
        let block = self.parse_block(
            flags & PARSE_FLAGS_IGNORE_MISSING_OPEN_BRACE != 0,
            diagnostic_message,
        );
        self.context_flags = save_context_flags;
        self.statement_has_await_identifier = save_has_await_identifier;
        block
    }

    // ────────────────────────────────────────────────────────────────────────
    // Index signatures / type members
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) isIndexSignature() bool`.
    pub(crate) fn is_index_signature(&mut self) -> bool {
        self.token == Kind::OpenBracketToken
            && self.look_ahead(|p| p.next_is_unambiguously_index_signature())
    }

    /// Go: `func (p *Parser) nextIsUnambiguouslyIndexSignature() bool`.
    fn next_is_unambiguously_index_signature(&mut self) -> bool {
        // The only allowed sequence is:
        //
        //   [id:
        //
        // However, for error recovery, we also check the following cases:
        //
        //   [...
        //   [id,
        //   [id?,
        //   [id?:
        //   [id?]
        //   [public id
        //   [private id
        //   [protected id
        //   []
        //
        self.next_token();
        if self.token == Kind::DotDotDotToken || self.token == Kind::CloseBracketToken {
            return true;
        }
        if tsc_ast::is_modifier_kind(self.token) {
            self.next_token();
            if self.is_identifier() {
                return true;
            }
        } else if !self.is_identifier() {
            return false;
        } else {
            // Skip the identifier
            self.next_token();
        }
        // A colon signifies a well formed indexer
        // A comma should be a badly formed indexer because comma expressions are not allowed
        // in computed properties.
        if self.token == Kind::ColonToken || self.token == Kind::CommaToken {
            return true;
        }
        // Question mark could be an indexer with an optional property,
        // or it could be a conditional expression in a computed property.
        if self.token != Kind::QuestionToken {
            return false;
        }
        // If any of the following tokens are after the question mark, it cannot
        // be a conditional expression, so treat it as an indexer.
        self.next_token();
        self.token == Kind::ColonToken
            || self.token == Kind::CommaToken
            || self.token == Kind::CloseBracketToken
    }

    /// Go: `func (p *Parser) parseIndexSignatureDeclaration(pos int, jsdoc, modifiers) *ast.Node`.
    pub(crate) fn parse_index_signature_declaration(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let parameters = self.parse_bracketed_list(
            PC_PARAMETERS,
            |p| p.parse_parameter(),
            Kind::OpenBracketToken,
            Kind::CloseBracketToken,
        );
        let type_node = self.parse_type_annotation();
        self.parse_type_member_semicolon();
        let result = {
            let __hoist_1_86 = self
                .factory
                .new_index_signature_declaration(modifiers, parameters, type_node);
            let __hoist_1_87 = pos;
            self.finish_node(__hoist_1_86, __hoist_1_87)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parsePropertyOrMethodSignature(pos int, jsdoc, modifiers) *ast.Node`.
    pub(crate) fn parse_property_or_method_signature(
        &mut self,
        pos: TextPos,
        jsdoc: JSDocScannerInfo,
        modifiers: Option<ModifierList>,
    ) -> NodeId {
        let name = self.parse_property_name();
        let question_token = self.parse_optional_token(Kind::QuestionToken);

        let result: NodeId =
            if self.token == Kind::OpenParenToken || self.token == Kind::LessThanToken {
                // Method signatures don't exist in expression contexts.  So they have neither
                // [Yield] nor [Await]
                let type_parameters = self.parse_type_parameters();
                let parameters = self.parse_parameters(PARSE_FLAGS_TYPE);
                let return_type = self.parse_return_type(Kind::ColonToken, true /*isType*/);
                self.factory.new_method_signature_declaration(
                    modifiers,
                    name,
                    question_token,
                    type_parameters,
                    parameters,
                    return_type,
                )
            } else {
                let type_node = self.parse_type_annotation();
                // Although type literal properties cannot not have initializers, we attempt
                // to parse an initializer so we can report in the checker that an interface
                // property or type literal property cannot have an initializer.
                let mut initializer = NodeId::NONE;
                if self.token == Kind::EqualsToken {
                    initializer = self.parse_initializer();
                }
                self.factory.new_property_signature_declaration(
                    modifiers,
                    name,
                    question_token,
                    type_node.unwrap_or(NodeId::NONE),
                    initializer,
                )
            };
        self.parse_type_member_semicolon();
        let finished = self.finish_node(result, pos);
        self.with_jsdoc(finished, jsdoc);
        finished
    }

    /// Go: `func (p *Parser) parseTypeLiteral() *ast.Node`.
    pub(crate) fn parse_type_literal(&mut self) -> NodeId {
        let pos = self.node_pos();
        let members = self.parse_object_type_members();
        {
            let __hoist_1_89 = self.factory.new_type_literal_node(members);
            let __hoist_1_90 = pos;
            self.finish_node(__hoist_1_89, __hoist_1_90)
        }
    }

    /// Go: `func (p *Parser) parseObjectTypeMembers() *ast.NodeList`.
    pub(crate) fn parse_object_type_members(&mut self) -> Option<NodeList> {
        if self.parse_expected(Kind::OpenBraceToken) {
            let members = self.parse_list(PC_TYPE_MEMBERS, |p| Some(p.parse_type_member()));
            self.parse_expected(Kind::CloseBraceToken);
            return members;
        }
        Some(self.create_missing_list())
    }

    // ────────────────────────────────────────────────────────────────────────
    // Tuple types
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseTupleType() *ast.Node`.
    fn parse_tuple_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let elements = self.parse_bracketed_list(
            PC_TUPLE_ELEMENT_TYPES,
            |p| Some(p.parse_tuple_element_name_or_tuple_element_type()),
            Kind::OpenBracketToken,
            Kind::CloseBracketToken,
        );
        {
            let __hoist_1_92 = self.factory.new_tuple_type_node(elements);
            let __hoist_1_93 = pos;
            self.finish_node(__hoist_1_92, __hoist_1_93)
        }
    }

    /// Go: `func (p *Parser) parseTupleElementNameOrTupleElementType() *ast.Node`.
    fn parse_tuple_element_name_or_tuple_element_type(&mut self) -> NodeId {
        if self.look_ahead(|p| p.scan_start_of_named_tuple_element()) {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let dot_dot_dot_token = self.parse_optional_token(Kind::DotDotDotToken);
            let name = self.parse_identifier_name();
            let question_token = self.parse_optional_token(Kind::QuestionToken);
            self.parse_expected(Kind::ColonToken);
            let type_node = self.parse_tuple_element_type();
            let result = {
                let __hoist_1_95 = self.factory.new_named_tuple_member(
                    dot_dot_dot_token,
                    name,
                    question_token,
                    type_node,
                );
                let __hoist_1_96 = pos;
                self.finish_node(__hoist_1_95, __hoist_1_96)
            };
            self.with_jsdoc(result, jsdoc);
            return result;
        }
        self.parse_tuple_element_type()
    }

    /// Go: `func (p *Parser) scanStartOfNamedTupleElement() bool`.
    fn scan_start_of_named_tuple_element(&mut self) -> bool {
        if self.token == Kind::DotDotDotToken {
            return token_is_identifier_or_keyword(self.next_token())
                && self.next_token_is_colon_or_question_colon();
        }
        token_is_identifier_or_keyword(self.token) && self.next_token_is_colon_or_question_colon()
    }

    /// Go: `func (p *Parser) nextTokenIsColonOrQuestionColon() bool`.
    fn next_token_is_colon_or_question_colon(&mut self) -> bool {
        // Go: `return p.nextToken() == ast.KindColonToken ||
        //        p.token == ast.KindQuestionToken && p.nextToken() == ast.KindColonToken`
        self.next_token() == Kind::ColonToken
            || self.token == Kind::QuestionToken && self.next_token() == Kind::ColonToken
    }

    /// Go: `func (p *Parser) parseTupleElementType() *ast.TypeNode`.
    fn parse_tuple_element_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        if self.parse_optional(Kind::DotDotDotToken) {
            let inner = self.parse_type();
            return {
                let __hoist_1_98 = self.factory.new_rest_type_node(inner);
                let __hoist_1_99 = pos;
                self.finish_node(__hoist_1_98, __hoist_1_99)
            };
        }
        let type_node = self.parse_type();
        // `if ast.IsJSDocNullableType(typeNode) && typeNode.Pos() == typeNode.Type().Pos()`
        // — the nullable wrapper is reparented into an OptionalTypeNode sharing
        // its loc/flags (the original node is dropped from the tree).
        let is_named_optional_candidate = {
            let s = self.factory.store();
            let n = s.node(type_node);
            tsc_ast::is_jsdoc_nullable_type(n)
                && n.as_jsdoc_nullable_type()
                    .is_some_and(|d| d.type_ != NodeId::NONE && s.node(d.type_).pos() == n.pos())
        };
        if is_named_optional_candidate {
            let inner = {
                let s = self.factory.store();
                s.node(type_node)
                    .as_jsdoc_nullable_type()
                    .expect("JSDocNullableType data")
                    .type_
            };
            let node = self.factory.new_optional_type_node(inner);
            let (flags, loc) = {
                let s = self.factory.store();
                let n = s.node(type_node);
                (n.flags, n.loc)
            };
            {
                let s = self.factory.store();
                let n = s.node_mut(node);
                n.flags = flags;
                n.loc = loc;
            }
            {
                let s = self.factory.store();
                s.node_mut(inner).parent.set(node);
            }
            return node;
        }
        type_node
    }

    /// Go: `func (p *Parser) parseParenthesizedType() *ast.Node`.
    fn parse_parenthesized_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::OpenParenToken);
        let type_node = self.parse_type();
        self.parse_expected(Kind::CloseParenToken);
        {
            let __hoist_1_101 = self.factory.new_parenthesized_type_node(type_node);
            let __hoist_1_102 = pos;
            self.finish_node(__hoist_1_101, __hoist_1_102)
        }
    }

    /// Go: `func (p *Parser) parseAssertsTypePredicate() *ast.TypeNode`.
    fn parse_asserts_type_predicate(&mut self) -> NodeId {
        let pos = self.node_pos();
        let asserts_modifier = self.parse_expected_token(Kind::AssertsKeyword);

        let parameter_name: NodeId = if self.token == Kind::ThisKeyword {
            self.parse_this_type_node()
        } else {
            self.parse_identifier()
        };
        let mut type_node: Option<NodeId> = None;
        if self.parse_optional(Kind::IsKeyword) {
            type_node = Some(self.parse_type());
        }
        {
            let __hoist_1_104 = self.factory.new_type_predicate_node(
                Some(asserts_modifier),
                parameter_name,
                type_node,
            );
            let __hoist_1_105 = pos;
            self.finish_node(__hoist_1_104, __hoist_1_105)
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Template literal types (the primitives are shared with expressions.rs)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseTemplateType() *ast.Node`.
    fn parse_template_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let head = self.parse_template_head(false /*isTaggedTemplate*/);
        let spans = self.parse_template_type_spans();
        {
            let __hoist_1_107 = self.factory.new_template_literal_type_node(head, spans);
            let __hoist_1_108 = pos;
            self.finish_node(__hoist_1_107, __hoist_1_108)
        }
    }

    /// Go: `func (p *Parser) parseTemplateHead(isTaggedTemplate bool) *ast.Node`.
    pub(crate) fn parse_template_head(&mut self, is_tagged_template: bool) -> NodeId {
        if !is_tagged_template
            && self
                .scanner
                .token_flags()
                .intersects(TokenFlags::IS_INVALID)
        {
            self.re_scan_template_token(false /*isTaggedTemplate*/);
        }
        let pos = self.node_pos();
        let text = self.token_value_string();
        let raw_text = self.get_template_literal_raw_text(2 /*endLength*/);
        let result = {
            let __hoist_1_110 = &text;
            let __hoist_1_111 = &raw_text;
            let __hoist_1_112 = self.scanner.token_flags();
            self.factory
                .new_template_head(__hoist_1_110, __hoist_1_111, __hoist_1_112)
        };
        self.next_token();
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) getTemplateLiteralRawText(endLength int) string`.
    ///
    /// PORT: Go slices the scanner's token text in place (a borrowed string
    /// would extend the `&self` borrow past the following `next_token`, so the
    /// slice is copied).
    pub(crate) fn get_template_literal_raw_text(&self, mut end_length: usize) -> String {
        let token_text = self.scanner.token_text();
        if self
            .scanner
            .token_flags()
            .intersects(TokenFlags::UNTERMINATED)
        {
            end_length = 0;
        }
        token_text[1..token_text.len() - end_length].to_string()
    }

    /// Go: `func (p *Parser) parseTemplateTypeSpans() *ast.NodeList`.
    fn parse_template_type_spans(&mut self) -> Option<NodeList> {
        let pos = self.node_pos();
        let mut list: Vec<NodeId> = Vec::new();
        loop {
            let span = self.parse_template_type_span();
            let literal_kind = {
                let s = self.factory.store();
                let span_data = s
                    .node(span)
                    .as_template_literal_type_span()
                    .expect("TemplateLiteralTypeSpan data");
                s.node(span_data.literal).kind
            };
            list.push(span);
            if literal_kind != Kind::TemplateMiddle {
                break;
            }
        }
        Some({
            let __hoist_1_114 = TextRange::new(pos, self.node_pos());
            let __hoist_1_115 = list;
            self.new_node_list(__hoist_1_114, __hoist_1_115)
        })
    }

    /// Go: `func (p *Parser) parseTemplateTypeSpan() *ast.Node`.
    fn parse_template_type_span(&mut self) -> NodeId {
        let pos = self.node_pos();
        let type_node = self.parse_type();
        let literal = self.parse_literal_of_template_span(false /*isTaggedTemplate*/);
        {
            let __hoist_1_117 = self
                .factory
                .new_template_literal_type_span(type_node, literal);
            let __hoist_1_118 = pos;
            self.finish_node(__hoist_1_117, __hoist_1_118)
        }
    }

    /// Go: `func (p *Parser) parseLiteralOfTemplateSpan(isTaggedTemplate bool) *ast.Node`.
    pub(crate) fn parse_literal_of_template_span(&mut self, is_tagged_template: bool) -> NodeId {
        if self.token == Kind::CloseBraceToken {
            self.re_scan_template_token(is_tagged_template);
            return self.parse_template_middle_or_tail();
        }
        self.parse_error_at_current_token(
            &tsc_diagnostics::X_0_EXPECTED,
            &[token_to_text(Kind::CloseBraceToken).to_string()],
        );
        let pos = self.node_pos();
        {
            let __hoist_1_120 = self.factory.new_template_tail("", "", TokenFlags::NONE);
            let __hoist_1_121 = pos;
            self.finish_node(__hoist_1_120, __hoist_1_121)
        }
    }

    /// Go: `func (p *Parser) parseTemplateMiddleOrTail() *ast.Node`.
    pub(crate) fn parse_template_middle_or_tail(&mut self) -> NodeId {
        let pos = self.node_pos();

        let result: NodeId = if self.token == Kind::TemplateMiddle {
            let text = self.token_value_string();
            let raw_text = self.get_template_literal_raw_text(2 /*endLength*/);
            {
                let __hoist_1_123 = &text;
                let __hoist_1_124 = &raw_text;
                let __hoist_1_125 = self.scanner.token_flags();
                self.factory
                    .new_template_middle(__hoist_1_123, __hoist_1_124, __hoist_1_125)
            }
        } else {
            let text = self.token_value_string();
            let raw_text = self.get_template_literal_raw_text(1 /*endLength*/);
            {
                let __hoist_1_127 = &text;
                let __hoist_1_128 = &raw_text;
                let __hoist_1_129 = self.scanner.token_flags();
                self.factory
                    .new_template_tail(__hoist_1_127, __hoist_1_128, __hoist_1_129)
            }
        };
        self.next_token();
        self.finish_node(result, pos)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Function/constructor types
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseFunctionOrConstructorTypeToError(isInUnionType bool, parseConstituentType func(p *Parser) *ast.TypeNode) *ast.TypeNode`.
    fn parse_function_or_constructor_type_to_error<F>(
        &mut self,
        is_in_union_type: bool,
        parse_constituent_type: &mut F,
    ) -> NodeId
    where
        F: FnMut(&mut Parser<'a, 'f>) -> NodeId,
    {
        // the function type and constructor type shorthand notation
        // are not allowed directly in unions and intersections, but we'll
        // try to parse them gracefully and issue a helpful message.
        if self.is_start_of_function_type_or_constructor_type() {
            let type_node = self.parse_function_or_constructor_type();
            let type_node_kind = {
                let s = self.factory.store();
                s.node(type_node).kind
            };
            let diagnostic: &'static Message = if type_node_kind == Kind::FunctionType {
                if is_in_union_type {
                    &tsc_diagnostics::FUNCTION_TYPE_NOTATION_MUST_BE_PARENTHESIZED_WHEN_USED_IN_A_UNION_TYPE
                } else {
                    &tsc_diagnostics::FUNCTION_TYPE_NOTATION_MUST_BE_PARENTHESIZED_WHEN_USED_IN_AN_INTERSECTION_TYPE
                }
            } else if is_in_union_type {
                &tsc_diagnostics::CONSTRUCTOR_TYPE_NOTATION_MUST_BE_PARENTHESIZED_WHEN_USED_IN_A_UNION_TYPE
            } else {
                &tsc_diagnostics::CONSTRUCTOR_TYPE_NOTATION_MUST_BE_PARENTHESIZED_WHEN_USED_IN_AN_INTERSECTION_TYPE
            };
            let loc = {
                let s = self.factory.store();
                s.node(type_node).loc
            };
            self.parse_error_at_range(loc, diagnostic, &[]);
            return type_node;
        }
        parse_constituent_type(self)
    }

    /// Go: `func (p *Parser) isStartOfFunctionTypeOrConstructorType() bool`.
    pub(crate) fn is_start_of_function_type_or_constructor_type(&mut self) -> bool {
        self.token == Kind::LessThanToken
            || self.token == Kind::OpenParenToken
                && self.look_ahead(|p| p.next_is_unambiguously_start_of_function_type())
            || self.token == Kind::NewKeyword
            || self.token == Kind::AbstractKeyword
                && self.look_ahead(|p| p.next_token_is_new_keyword())
    }

    /// Go: `func (p *Parser) parseFunctionOrConstructorType() *ast.TypeNode`.
    pub(crate) fn parse_function_or_constructor_type(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_constructor_type();
        let is_constructor_type = self.parse_optional(Kind::NewKeyword);
        debug_assert!(
            modifiers.is_none() || is_constructor_type,
            "Per isStartOfFunctionTypeOrConstructorType, a function type cannot have modifiers."
        );
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(PARSE_FLAGS_TYPE);
        // PORT: Go's `isMissingNodeList` sentinel — the latch was set by the
        // `parse_parameters` call above (its only consumer is
        // `type_has_arrow_function_blocking_parse_error`, which consults this
        // set since the empty-but-present Rust list is indistinguishable from
        // a successfully-parsed `()`).
        let missing_params = self.last_parameters_list_missing;
        let parameters = Some(parameters.unwrap_or_else(|| self.create_missing_list()));
        let return_type =
            self.parse_return_type(Kind::EqualsGreaterThanToken, false /*isType*/);
        let result = if is_constructor_type {
            self.factory.new_constructor_type_node(
                modifiers,
                type_parameters,
                parameters,
                return_type,
            )
        } else {
            self.factory
                .new_function_type_node(type_parameters, parameters, return_type)
        };
        if missing_params {
            self.missing_function_type_params.add(result);
        }
        self.finish_node(result, pos);
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseModifiersForConstructorType() *ast.ModifierList`.
    fn parse_modifiers_for_constructor_type(&mut self) -> Option<ModifierList> {
        if self.token == Kind::AbstractKeyword {
            let pos = self.node_pos();
            let modifier = self.factory.new_modifier(self.token);
            self.next_token();
            self.finish_node(modifier, pos);
            let loc = {
                let s = self.factory.store();
                s.node(modifier).loc
            };
            return Some(self.new_modifier_list(loc, vec![modifier]));
        }
        None
    }

    /// Go: `func (p *Parser) nextTokenIsNewKeyword() bool`.
    fn next_token_is_new_keyword(&mut self) -> bool {
        self.next_token() == Kind::NewKeyword
    }

    /// Go: `func (p *Parser) nextIsUnambiguouslyStartOfFunctionType() bool`.
    fn next_is_unambiguously_start_of_function_type(&mut self) -> bool {
        self.next_token();
        if self.token == Kind::CloseParenToken || self.token == Kind::DotDotDotToken {
            // ( )
            // ( ...
            return true;
        }
        if self.skip_parameter_start() {
            // We successfully skipped modifiers (if any) and an identifier or binding pattern,
            // now see if we have something that indicates a parameter declaration
            if self.token == Kind::ColonToken
                || self.token == Kind::CommaToken
                || self.token == Kind::QuestionToken
                || self.token == Kind::EqualsToken
            {
                // ( xxx :
                // ( xxx ,
                // ( xxx ?
                // ( xxx =
                return true;
            }
            if self.token == Kind::CloseParenToken
                && self.next_token() == Kind::EqualsGreaterThanToken
            {
                // ( xxx ) =>
                return true;
            }
        }
        false
    }

    /// Go: `func (p *Parser) skipParameterStart() bool`.
    fn skip_parameter_start(&mut self) -> bool {
        if tsc_ast::is_modifier_kind(self.token) {
            // Skip modifiers
            self.parse_modifiers();
        }
        self.parse_optional(Kind::DotDotDotToken);
        if self.is_identifier() || self.token == Kind::ThisKeyword {
            self.next_token();
            return true;
        }
        if self.token == Kind::OpenBracketToken || self.token == Kind::OpenBraceToken {
            // Return true if we can parse an array or object binding pattern with no errors
            let previous_error_count = self.sink.borrow().diagnostics.len();
            self.parse_identifier_or_pattern();
            return previous_error_count == self.sink.borrow().diagnostics.len();
        }
        false
    }

    // ────────────────────────────────────────────────────────────────────────
    // Type-related predicates (Go keeps these near the other is* helpers)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) isStartOfType(inStartOfParameter bool) bool`.
    pub(crate) fn is_start_of_type(&mut self, in_start_of_parameter: bool) -> bool {
        match self.token {
            Kind::AnyKeyword
            | Kind::UnknownKeyword
            | Kind::StringKeyword
            | Kind::NumberKeyword
            | Kind::BigIntKeyword
            | Kind::BooleanKeyword
            | Kind::ReadonlyKeyword
            | Kind::SymbolKeyword
            | Kind::UniqueKeyword
            | Kind::VoidKeyword
            | Kind::UndefinedKeyword
            | Kind::NullKeyword
            | Kind::ThisKeyword
            | Kind::TypeOfKeyword
            | Kind::NeverKeyword
            | Kind::OpenBraceToken
            | Kind::OpenBracketToken
            | Kind::LessThanToken
            | Kind::BarToken
            | Kind::AmpersandToken
            | Kind::NewKeyword
            | Kind::StringLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::TrueKeyword
            | Kind::FalseKeyword
            | Kind::ObjectKeyword
            | Kind::AsteriskToken
            | Kind::QuestionToken
            | Kind::ExclamationToken
            | Kind::DotDotDotToken
            | Kind::InferKeyword
            | Kind::ImportKeyword
            | Kind::AssertsKeyword
            | Kind::NoSubstitutionTemplateLiteral
            | Kind::TemplateHead => true,
            Kind::FunctionKeyword => !in_start_of_parameter,
            Kind::MinusToken => {
                !in_start_of_parameter
                    && self.look_ahead(|p| p.next_token_is_numeric_or_big_int_literal())
            }
            Kind::OpenParenToken => {
                // Only consider '(' the start of a type if followed by ')', '...', an identifier, a modifier,
                // or something that starts a type. We don't want to consider things like '(1)' a type.
                !in_start_of_parameter
                    && self.look_ahead(|p| p.next_is_parenthesized_or_function_type())
            }
            _ => self.is_identifier(),
        }
    }

    /// Go: `func (p *Parser) nextTokenIsNumericOrBigIntLiteral() bool`.
    fn next_token_is_numeric_or_big_int_literal(&mut self) -> bool {
        self.next_token();
        self.token == Kind::NumericLiteral || self.token == Kind::BigIntLiteral
    }

    /// Go: `func (p *Parser) nextIsParenthesizedOrFunctionType() bool`.
    fn next_is_parenthesized_or_function_type(&mut self) -> bool {
        self.next_token();
        self.token == Kind::CloseParenToken
            || self.is_start_of_parameter(false /*isJSDocParameter*/)
            || self.is_start_of_type(false /*inStartOfParameter*/)
    }

    /// Go: `func (p *Parser) isStartOfParameter(isJSDocParameter bool) bool`.
    pub(crate) fn is_start_of_parameter(&mut self, is_jsdoc_parameter: bool) -> bool {
        self.token == Kind::DotDotDotToken
            || self.is_binding_identifier_or_private_identifier_or_pattern()
            || tsc_ast::is_modifier_kind(self.token)
            || self.token == Kind::AtToken
            || self.is_start_of_type(!is_jsdoc_parameter /*inStartOfParameter*/)
    }
}
