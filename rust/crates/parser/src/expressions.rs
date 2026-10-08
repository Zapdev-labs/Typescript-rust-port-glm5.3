//! Go: `tsc/internal/parser/parser.go` (part 3) — the expression grammar:
//! parseExpression through parseLiteralExpression (arrow functions, binary/
//! unary chains, member/call/optional chains, tagged templates, primaries).

use tsc_ast::OperatorPrecedence;
use tsc_ast::{Kind, ModifierList, NodeFlags, NodeId, NodeList, TokenFlags};
use tsc_core::languagevariant::LanguageVariant;
use tsc_core::text::{TextPos, TextRange};
use tsc_core::tristate::Tristate;
use tsc_scanner::token_to_text;

use crate::parser::{
    is_import_phase_meta_property, is_left_hand_side_expression, modifier_list_has_async,
    JSDocScannerInfo, Parser,
};
use crate::{
    is_keyword_or_punctuation, token_is_identifier_or_keyword, PARSE_FLAGS_AWAIT,
    PARSE_FLAGS_IGNORE_MISSING_OPEN_BRACE, PARSE_FLAGS_NONE, PARSE_FLAGS_YIELD,
    PC_ARGUMENT_EXPRESSIONS, PC_ARRAY_LITERAL_MEMBERS, PC_OBJECT_LITERAL_MEMBERS,
    PC_TYPE_ARGUMENTS,
};

/// Go: `func shouldConsumeBinaryOperator(operator ast.Kind, operatorPrecedence, currentPrecedence ast.OperatorPrecedence) bool`.
///
/// Reports whether an operator binds before the operator represented by currentPrecedence.
/// At equal precedence, only the right-associative exponentiation operator binds first.
fn should_consume_binary_operator(
    operator: Kind,
    operator_precedence: OperatorPrecedence,
    current_precedence: OperatorPrecedence,
) -> bool {
    if operator_precedence > current_precedence {
        return true;
    }
    operator_precedence == current_precedence && operator == Kind::AsteriskAsteriskToken
}

impl<'a, 'f> Parser<'a, 'f> {
    // ────────────────────────────────────────────────────────────────────────
    // Assignment expressions
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseExpression() *ast.Expression`.
    pub(crate) fn parse_expression(&mut self) -> NodeId {
        // Expression[in]:
        //      AssignmentExpression[in]
        //      Expression[in] , AssignmentExpression[in]

        // clear the decorator context when parsing Expression, as it should be unambiguous when parsing a decorator
        let save_context_flags = self.context_flags;
        self.context_flags &= !NodeFlags::DECORATOR_CONTEXT;
        let pos = self.node_pos();
        let mut expr = self.parse_assignment_expression_or_higher();
        loop {
            let operator_token = self.parse_optional_token(Kind::CommaToken);
            let Some(operator_token) = operator_token else {
                break;
            };
            let right = self.parse_assignment_expression_or_higher();
            expr = self.make_binary_expression(expr, operator_token, right, pos);
        }
        self.context_flags = save_context_flags;
        expr
    }

    /// Go: `func (p *Parser) parseExpressionAllowIn() *ast.Expression`.
    pub(crate) fn parse_expression_allow_in(&mut self) -> NodeId {
        self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, false, |p| {
            p.parse_expression()
        })
    }

    /// Go: `func (p *Parser) parseAssignmentExpressionOrHigher() *ast.Expression`.
    pub(crate) fn parse_assignment_expression_or_higher(&mut self) -> NodeId {
        self.parse_assignment_expression_or_higher_worker(
            true, /*allowReturnTypeInArrowFunction*/
        )
    }

    /// Go: `func (p *Parser) parseAssignmentExpressionOrHigherWorker(allowReturnTypeInArrowFunction bool) *ast.Expression`.
    fn parse_assignment_expression_or_higher_worker(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> NodeId {
        //  AssignmentExpression[in,yield]:
        //      1) ConditionalExpression[?in,?yield]
        //      2) LeftHandSideExpression = AssignmentExpression[?in,?yield]
        //      3) LeftHandSideExpression AssignmentOperator AssignmentExpression[?in,?yield]
        //      4) ArrowFunctionExpression[?in,?yield]
        //      5) AsyncArrowFunctionExpression[in,yield,await]
        //      6) [+Yield] YieldExpression[?In]
        //
        // Note: for ease of implementation we treat productions '2' and '3' as the same thing.
        // (i.e. they're both BinaryExpressions with an assignment operator in it).
        // First, do the simple check if we have a YieldExpression (production '6').
        if self.is_yield_expression() {
            return self.parse_yield_expression();
        }
        // Then, check if we have an arrow function (production '4' and '5') that starts with a parenthesized
        // parameter list or is an async arrow function.
        // AsyncArrowFunctionExpression:
        //      1) async[no LineTerminator here]AsyncArrowBindingIdentifier[?Yield][no LineTerminator here]=>AsyncConciseBody[?In]
        //      2) CoverCallExpressionAndAsyncArrowHead[?Yield, ?Await][no LineTerminator here]=>AsyncConciseBody[?In]
        // Production (1) of AsyncArrowFunctionExpression is parsed in "tryParseAsyncSimpleArrowFunctionExpression".
        // And production (2) is parsed in "tryParseParenthesizedArrowFunctionExpression".
        //
        // If we do successfully parse arrow-function, we must *not* recurse for productions 1, 2 or 3. An ArrowFunction is
        // not a LeftHandSideExpression, nor does it start a ConditionalExpression.  So we are done
        // with AssignmentExpression if we see one.
        if let Some(arrow_expression) = self
            .try_parse_parenthesized_arrow_function_expression(allow_return_type_in_arrow_function)
        {
            return arrow_expression;
        }
        if let Some(arrow_expression) = self
            .try_parse_async_simple_arrow_function_expression(allow_return_type_in_arrow_function)
        {
            return arrow_expression;
        }
        // Now try to see if we're in production '1', '2' or '3'.  A conditional expression can
        // start with a LogicalOrExpression, while the assignment productions can only start with
        // LeftHandSideExpressions.
        //
        // So, first, we try to just parse out a BinaryExpression.  If we get something that is a
        // LeftHandSide or higher, then we can try to parse out the assignment expression part.
        // Otherwise, we try to parse out the conditional expression bit.  We want to allow any
        // binary expression here, so we pass in the 'lowest' precedence here so that it matches
        // and consumes anything.
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let expr = self.parse_binary_expression_or_higher(OperatorPrecedence::LOWEST);
        // To avoid a look-ahead, we did not handle the case of an arrow function with a single un-parenthesized
        // parameter ('x => ...') above. We handle it here by checking if the parsed expression was a single
        // identifier and the current token is an arrow.
        let expr_kind = {
            let s = self.factory.store();
            s.node(expr).kind
        };
        if expr_kind == Kind::Identifier && self.token == Kind::EqualsGreaterThanToken {
            return self.parse_simple_arrow_function_expression(
                pos,
                expr,
                allow_return_type_in_arrow_function,
                jsdoc,
                None, /*asyncModifier*/
            );
        }
        // Now see if we might be in cases '2' or '3'.
        // If the expression was a LHS expression, and we have an assignment operator, then
        // we're in '2' or '3'. Consume the assignment and return.
        //
        // Note: we call reScanGreaterToken so that we get an appropriately merged token
        // for cases like `> > =` becoming `>>=`
        let expr_is_lhs = {
            let s = self.factory.store();
            is_left_hand_side_expression(s.node(expr))
        };
        if expr_is_lhs && tsc_ast::is_assignment_operator(self.re_scan_greater_than_token()) {
            let operator_token = self.parse_token_node();
            let right = self
                .parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function);
            return self.make_binary_expression(expr, operator_token, right, pos);
        }
        // It wasn't an assignment or a lambda.  This is a conditional expression:
        self.parse_conditional_expression_rest(expr, pos, allow_return_type_in_arrow_function)
    }

    /// Go: `func (p *Parser) isYieldExpression() bool`.
    fn is_yield_expression(&mut self) -> bool {
        if self.token == Kind::YieldKeyword {
            // If we have a 'yield' keyword, and this is a context where yield expressions are
            // allowed, then definitely parse out a yield expression.
            if self.in_yield_context() {
                return true;
            }

            // We're in a context where 'yield expr' is not allowed.  However, if we can
            // definitely tell that the user was trying to parse a 'yield expr' and not
            // just a normal expr that start with a 'yield' identifier, then parse out
            // a 'yield expr'.  We can then report an error later that they are only
            // allowed in generator expressions.
            //
            // for example, if we see 'yield(foo)', then we'll have to treat that as an
            // invocation expression of something called 'yield'.  However, if we have
            // 'yield foo' then that is not legal as a normal expression, so we can
            // definitely recognize this as a yield expression.
            //
            // for now we just check if the next token is an identifier.  More heuristics
            // can be added here later as necessary.  We just need to make sure that we
            // don't accidentally consume something legal.
            return self
                .look_ahead(|p| p.next_token_is_identifier_or_keyword_or_literal_on_same_line());
        }
        false
    }

    /// Go: `func (p *Parser) parseYieldExpression() *ast.Node`.
    fn parse_yield_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        // YieldExpression[In] :
        //      yield
        //      yield [no LineTerminator here] [Lexical goal InputElementRegExp]AssignmentExpression[?In, Yield]
        //      yield [no LineTerminator here] * [Lexical goal InputElementRegExp]AssignmentExpression[?In, Yield]
        self.next_token();

        let result: NodeId = if !self.has_preceding_line_break()
            && (self.token == Kind::AsteriskToken || self.is_start_of_expression())
        {
            let asterisk_token = self.parse_optional_token(Kind::AsteriskToken);
            let expression = self.parse_assignment_expression_or_higher();
            self.factory
                .new_yield_expression(asterisk_token, Some(expression))
        } else {
            // if the next token is not on the same line as yield.  or we don't have an '*' or
            // the start of an expression, then this is just a simple "yield" expression.
            self.factory.new_yield_expression(None, None)
        };
        self.finish_node(result, pos)
    }

    // ────────────────────────────────────────────────────────────────────────
    // Arrow functions
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) isParenthesizedArrowFunctionExpression() core.Tristate`.
    fn is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        if self.token == Kind::OpenParenToken
            || self.token == Kind::LessThanToken
            || self.token == Kind::AsyncKeyword
        {
            let state = self.mark();
            let result = self.next_is_parenthesized_arrow_function_expression();
            self.rewind(state);
            return result;
        }
        if self.token == Kind::EqualsGreaterThanToken {
            // ERROR RECOVERY TWEAK:
            // If we see a standalone => try to parse it as an arrow function expression as that's
            // likely what the user intended to write.
            return Tristate::TSTrue;
        }
        // Definitely not a parenthesized arrow function.
        Tristate::TSFalse
    }

    /// Go: `func (p *Parser) nextIsParenthesizedArrowFunctionExpression() core.Tristate`.
    fn next_is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        if self.token == Kind::AsyncKeyword {
            self.next_token();
            if self.has_preceding_line_break() {
                return Tristate::TSFalse;
            }
            if self.token != Kind::OpenParenToken && self.token != Kind::LessThanToken {
                return Tristate::TSFalse;
            }
        }
        let first = self.token;
        let second = self.next_token();
        if first == Kind::OpenParenToken {
            if second == Kind::CloseParenToken {
                // Simple cases: "() =>", "(): ", and "() {".
                // This is an arrow function with no parameters.
                // The last one is not actually an arrow function,
                // but this is probably what the user intended.
                let third = self.next_token();
                match third {
                    Kind::EqualsGreaterThanToken | Kind::ColonToken | Kind::OpenBraceToken => {
                        return Tristate::TSTrue;
                    }
                    _ => {}
                }
                return Tristate::TSFalse;
            }
            // If encounter "([" or "({", this could be the start of a binding pattern.
            // Examples:
            //      ([ x ]) => { }
            //      ({ x }) => { }
            //      ([ x ])
            //      ({ x })
            if second == Kind::OpenBracketToken || second == Kind::OpenBraceToken {
                return Tristate::TSUnknown;
            }
            // Simple case: "(..."
            // This is an arrow function with a rest parameter.
            if second == Kind::DotDotDotToken {
                return Tristate::TSTrue;
            }
            // Check for "(xxx yyy", where xxx is a modifier and yyy is an identifier. This
            // isn't actually allowed, but we want to treat it as a lambda so we can provide
            // a good error message.
            if tsc_ast::is_modifier_kind(second)
                && second != Kind::AsyncKeyword
                && self.look_ahead(|p| p.next_token_is_identifier())
            {
                if self.next_token() == Kind::AsKeyword {
                    // https://github.com/microsoft/TypeScript/issues/44466
                    return Tristate::TSFalse;
                }
                return Tristate::TSTrue;
            }
            // If we had "(" followed by something that's not an identifier,
            // then this definitely doesn't look like a lambda.  "this" is not
            // valid, but we want to parse it and then give a semantic error.
            if !self.is_identifier() && second != Kind::ThisKeyword {
                return Tristate::TSFalse;
            }
            match self.next_token() {
                Kind::ColonToken => {
                    // If we have something like "(a:", then we must have a
                    // type-annotated parameter in an arrow function expression.
                    return Tristate::TSTrue;
                }
                Kind::QuestionToken => {
                    self.next_token();
                    // If we have "(a?:" or "(a?," or "(a?=" or "(a?)" then it is definitely a lambda.
                    if self.token == Kind::ColonToken
                        || self.token == Kind::CommaToken
                        || self.token == Kind::EqualsToken
                        || self.token == Kind::CloseParenToken
                    {
                        return Tristate::TSTrue;
                    }
                    // Otherwise it is definitely not a lambda.
                    return Tristate::TSFalse;
                }
                Kind::CommaToken | Kind::EqualsToken | Kind::CloseParenToken => {
                    // If we have "(a," or "(a=" or "(a)" this *could* be an arrow function
                    return Tristate::TSUnknown;
                }
                _ => {}
            }
            // It is definitely not an arrow function
            return Tristate::TSFalse;
        }
        // If we have "<" not followed by an identifier,
        // then this definitely is not an arrow function.
        debug_assert_eq!(first, Kind::LessThanToken);
        if !self.is_identifier() && self.token != Kind::ConstKeyword {
            return Tristate::TSFalse;
        }
        // JSX overrides
        if self.language_variant == LanguageVariant::JSX {
            let is_arrow_function_in_jsx = self.look_ahead(|p| {
                p.parse_optional(Kind::ConstKeyword);
                let third = p.next_token();
                if third == Kind::ExtendsKeyword {
                    let fourth = p.next_token();
                    !matches!(
                        fourth,
                        Kind::EqualsToken | Kind::GreaterThanToken | Kind::SlashToken
                    )
                } else {
                    third == Kind::CommaToken || third == Kind::EqualsToken
                }
            });
            if is_arrow_function_in_jsx {
                return Tristate::TSTrue;
            }
            return Tristate::TSFalse;
        }
        // This *could* be a parenthesized arrow function.
        Tristate::TSUnknown
    }

    /// Go: `func (p *Parser) tryParseParenthesizedArrowFunctionExpression(allowReturnTypeInArrowFunction bool) *ast.Node`.
    fn try_parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Option<NodeId> {
        let tristate = self.is_parenthesized_arrow_function_expression();
        if tristate == Tristate::TSFalse {
            // It's definitely not a parenthesized arrow function expression.
            return None;
        }
        // If we definitely have an arrow function, then we can just parse one, not requiring a
        // following => or { token. Otherwise, we *might* have an arrow function.  Try to parse
        // it out, but don't allow any ambiguity, and return 'undefined' if this could be an
        // expression instead.
        if tristate == Tristate::TSTrue {
            return self.parse_parenthesized_arrow_function_expression(
                true, /*allowAmbiguity*/
                true, /*allowReturnTypeInArrowFunction*/
            );
        }
        let state = self.mark();
        let result = self.parse_possible_parenthesized_arrow_function_expression(
            allow_return_type_in_arrow_function,
        );
        if result.is_none() {
            self.rewind(state);
        }
        result
    }

    /// Go: `func (p *Parser) parseParenthesizedArrowFunctionExpression(allowAmbiguity, allowReturnTypeInArrowFunction bool) *ast.Node`.
    fn parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_ambiguity: bool,
        allow_return_type_in_arrow_function: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_arrow_function();
        let is_async = modifier_list_has_async(self, modifiers.as_ref());
        let signature_flags = if is_async {
            PARSE_FLAGS_AWAIT
        } else {
            PARSE_FLAGS_NONE
        };
        // Arrow functions are never generators.
        //
        // If we're speculatively parsing a signature for a parenthesized arrow function, then
        // we have to have a complete parameter list.  Otherwise we might see something like
        // a => (b => c)
        // And think that "(b =>" was actually a parenthesized arrow function with a missing
        // close paren.
        let type_parameters = self.parse_type_parameters();
        let parameters: Option<NodeList>;
        if !self.parse_expected(Kind::OpenParenToken) {
            if !allow_ambiguity {
                return None;
            }
            parameters = Some(self.create_missing_list());
        } else {
            if !allow_ambiguity {
                // Go: `maybeParameters := ...; if maybeParameters == nil {
                // return nil }` — the `?` operator is the same shape.
                parameters = Some(self.parse_parameters_worker(signature_flags, allow_ambiguity)?);
            } else {
                parameters = self.parse_parameters_worker(signature_flags, allow_ambiguity);
            }
            if !self.parse_expected(Kind::CloseParenToken) && !allow_ambiguity {
                return None;
            }
        }
        let has_return_colon = self.token == Kind::ColonToken;
        let return_type = self.parse_return_type(Kind::ColonToken, false /*isType*/);
        if return_type.is_some()
            && !allow_ambiguity
            && self.type_has_arrow_function_blocking_parse_error(return_type.unwrap())
        {
            return None;
        }
        // Parsing a signature isn't enough.
        // Parenthesized arrow signatures often look like other valid expressions.
        // For instance:
        //  - "(x = 10)" is an assignment expression parsed as a signature with a default parameter value.
        //  - "(x,y)" is a comma expression parsed as a signature with two parameters.
        //  - "a ? (b): c" will have "(b):" parsed as a signature with a return type annotation.
        //  - "a ? (b): function() {}" will too, since function() is a valid JSDoc function type.
        //  - "a ? (b): (function() {})" as well, but inside of a parenthesized type with an arbitrary amount of nesting.
        //
        // So we need just a bit of lookahead to ensure that it can only be a signature.
        // PORT: Go computes `unwrappedType` by skipping parenthesized types here but
        // never reads the result; the traversal is kept for fidelity.
        let mut unwrapped_type = return_type;
        while let Some(current) = unwrapped_type {
            let (kind, inner) = {
                let s = self.factory.store();
                let n = s.node(current);
                (
                    n.kind,
                    n.as_parenthesized_type_node()
                        .map(|d| d.type_)
                        .unwrap_or(NodeId::NONE),
                )
            };
            if kind != Kind::ParenthesizedType {
                break;
            }
            unwrapped_type = (inner != NodeId::NONE).then_some(inner);
        }
        let _ = unwrapped_type;
        if !allow_ambiguity
            && self.token != Kind::EqualsGreaterThanToken
            && self.token != Kind::OpenBraceToken
        {
            // Returning None here will cause our caller to rewind to where we started from.
            return None;
        }
        // If we have an arrow, then try to parse the body. Even if not, try to parse if we
        // have an opening brace, just in case we're in an error state.
        let last_token = self.token;
        let equals_greater_than_token = self.parse_expected_token(Kind::EqualsGreaterThanToken);

        let body: NodeId = if last_token == Kind::EqualsGreaterThanToken
            || last_token == Kind::OpenBraceToken
        {
            self.parse_arrow_function_expression_body(is_async, allow_return_type_in_arrow_function)
        } else {
            self.parse_identifier()
        };
        // Given:
        //     x ? y => ({ y }) : z => ({ z })
        // We try to parse the body of the first arrow function by looking at:
        //     ({ y }) : z => ({ z })
        // This is a valid arrow function with "z" as the return type.
        //
        // But, if we're in the true side of a conditional expression, this colon
        // terminates the expression, so we cannot allow a return type if we aren't
        // certain whether or not the preceding text was parsed as a parameter list.
        //
        // For example,
        //     a() ? (b: number, c?: string): void => d() : e
        // is determined by isParenthesizedArrowFunctionExpression to unambiguously
        // be an arrow expression, so we allow a return type.
        if !allow_return_type_in_arrow_function && has_return_colon {
            // However, if the arrow function we were able to parse is followed by another colon
            // as in:
            //     a ? (x): string => x : null
            // Then allow the arrow function, and treat the second colon as terminating
            // the conditional expression. It's okay to do this because this code would
            // be a syntax error in JavaScript (as the second colon shouldn't be there).
            if self.token != Kind::ColonToken {
                return None;
            }
        }
        let result = {
            let __hoist_1_2 = self.factory.new_arrow_function(
                modifiers,
                type_parameters,
                parameters,
                return_type,
                None, /*fullSignature*/
                equals_greater_than_token,
                Some(body),
            );
            let __hoist_1_3 = pos;
            self.finish_node(__hoist_1_2, __hoist_1_3)
        };
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        Some(result)
    }

    /// Go: `func (p *Parser) parseModifiersForArrowFunction() *ast.ModifierList`.
    fn parse_modifiers_for_arrow_function(&mut self) -> Option<ModifierList> {
        if self.token == Kind::AsyncKeyword {
            let pos = self.node_pos();
            self.next_token();
            let modifier = {
                let __hoist_1_5 = self.factory.new_modifier(Kind::AsyncKeyword);
                let __hoist_1_6 = pos;
                self.finish_node(__hoist_1_5, __hoist_1_6)
            };
            let loc = {
                let s = self.factory.store();
                s.node(modifier).loc
            };
            return Some(self.new_modifier_list(loc, vec![modifier]));
        }
        None
    }

    /// Go: `func typeHasArrowFunctionBlockingParseError(node *ast.TypeNode) bool` —
    /// "If true, we should abort parsing an error function."
    ///
    /// PORT: Go's `isMissingNodeList(node.FunctionLikeData().Parameters)` is
    /// replaced by the `missing_function_type_params` latch set recorded in
    /// `parse_function_or_constructor_type` (see the Parser field docs).
    fn type_has_arrow_function_blocking_parse_error(&mut self, node: NodeId) -> bool {
        let kind = {
            let s = self.factory.store();
            s.node(node).kind
        };
        match kind {
            Kind::TypeReference => {
                let s = self.factory.store();
                let type_name = s
                    .node(node)
                    .as_type_reference_node()
                    .expect("TypeReferenceNode data")
                    .type_name;
                tsc_ast::node_is_missing(s.node(type_name))
            }
            Kind::FunctionType | Kind::ConstructorType => {
                let inner = {
                    let s = self.factory.store();
                    let d = s.node(node);
                    if kind == Kind::FunctionType {
                        d.as_function_type_node()
                            .expect("FunctionTypeNode data")
                            .type_
                    } else {
                        d.as_constructor_type_node()
                            .expect("ConstructorTypeNode data")
                            .type_
                    }
                };
                // Go: `isMissingNodeList(node.FunctionLikeData().Parameters) ||
                // typeHasArrowFunctionBlockingParseError(node.Type())` — the
                // `missing_function_type_params` set replaces the missing-list
                // sentinel check.
                self.missing_function_type_params.has(&node)
                    || inner.is_some_and(|inner| {
                        self.type_has_arrow_function_blocking_parse_error(inner)
                    })
            }
            Kind::ParenthesizedType => {
                let inner = {
                    let s = self.factory.store();
                    s.node(node)
                        .as_parenthesized_type_node()
                        .expect("ParenthesizedTypeNode data")
                        .type_
                };
                self.type_has_arrow_function_blocking_parse_error(inner)
            }
            _ => false,
        }
    }

    /// Go: `func (p *Parser) parseArrowFunctionExpressionBody(isAsync, allowReturnTypeInArrowFunction bool) *ast.Node`.
    fn parse_arrow_function_expression_body(
        &mut self,
        is_async: bool,
        allow_return_type_in_arrow_function: bool,
    ) -> NodeId {
        if self.token == Kind::OpenBraceToken {
            return self.parse_function_block(
                if is_async {
                    PARSE_FLAGS_AWAIT
                } else {
                    PARSE_FLAGS_NONE
                },
                None, /*diagnosticMessage*/
            );
        }
        if self.token != Kind::SemicolonToken
            && self.token != Kind::FunctionKeyword
            && self.token != Kind::ClassKeyword
            && self.is_start_of_statement()
            && !self.is_start_of_expression_statement()
        {
            // Check if we got a plain statement (i.e. no expression-statements, no function/class expressions/declarations)
            //
            // Here we try to recover from a potential error situation in the case where the
            // user meant to supply a block. For example, if the user wrote:
            //
            //  a =>
            //      let v = 0;
            //  }
            //
            // they may be missing an open brace.  Check to see if that's the case so we can
            // try to recover better.  If we don't do this, then the next close curly we see may end
            // up preemptively closing the containing construct.
            //
            // Note: even when 'IgnoreMissingOpenBrace' is passed, parseBody will still error.
            return self.parse_function_block(
                PARSE_FLAGS_IGNORE_MISSING_OPEN_BRACE
                    | if is_async {
                        PARSE_FLAGS_AWAIT
                    } else {
                        PARSE_FLAGS_NONE
                    },
                None, /*diagnosticMessage*/
            );
        }
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::AWAIT_CONTEXT, is_async);
        self.set_context_flags(NodeFlags::YIELD_CONTEXT, false);
        let node =
            self.parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function);
        self.context_flags = save_context_flags;
        node
    }

    /// Go: `func (p *Parser) isStartOfExpressionStatement() bool`.
    fn is_start_of_expression_statement(&mut self) -> bool {
        // As per the grammar, none of '{' or 'function' or 'class' can start an expression statement.
        self.token != Kind::OpenBraceToken
            && self.token != Kind::FunctionKeyword
            && self.token != Kind::ClassKeyword
            && self.token != Kind::AtToken
            && self.is_start_of_expression()
    }

    /// Go: `func (p *Parser) parsePossibleParenthesizedArrowFunctionExpression(allowReturnTypeInArrowFunction bool) *ast.Node`.
    fn parse_possible_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Option<NodeId> {
        let token_pos = self.scanner.token_start() as i32;
        if self.not_parenthesized_arrow.has(&token_pos) {
            return None;
        }
        let result = self.parse_parenthesized_arrow_function_expression(
            false, /*allowAmbiguity*/
            allow_return_type_in_arrow_function,
        );
        if result.is_none() {
            self.not_parenthesized_arrow.add(token_pos);
        }
        result
    }

    /// Go: `func (p *Parser) tryParseAsyncSimpleArrowFunctionExpression(allowReturnTypeInArrowFunction bool) *ast.Node`.
    fn try_parse_async_simple_arrow_function_expression(
        &mut self,
        allow_return_type_in_arrow_function: bool,
    ) -> Option<NodeId> {
        // We do a check here so that we won't be doing unnecessarily call to "lookAhead"
        if self.token == Kind::AsyncKeyword
            && self.look_ahead(|p| p.next_is_un_parenthesized_async_arrow_function())
        {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let async_modifier = self.parse_modifiers_for_arrow_function();
            let expr = self.parse_binary_expression_or_higher(OperatorPrecedence::LOWEST);
            return Some(self.parse_simple_arrow_function_expression(
                pos,
                expr,
                allow_return_type_in_arrow_function,
                jsdoc,
                async_modifier,
            ));
        }
        None
    }

    /// Go: `func (p *Parser) nextIsUnParenthesizedAsyncArrowFunction() bool`.
    fn next_is_un_parenthesized_async_arrow_function(&mut self) -> bool {
        // AsyncArrowFunctionExpression:
        //      1) async[no LineTerminator here]AsyncArrowBindingIdentifier[?Yield][no LineTerminator here]=>AsyncConciseBody[?In]
        //      2) CoverCallExpressionAndAsyncArrowHead[?Yield, ?Await][no LineTerminator here]=>AsyncConciseBody[?In]
        if self.token == Kind::AsyncKeyword {
            self.next_token();
            // If the "async" is followed by "=>" token then it is not a beginning of an async arrow-function
            // but instead a simple arrow-function which will be parsed inside "parseAssignmentExpressionOrHigher"
            if self.has_preceding_line_break() || self.token == Kind::EqualsGreaterThanToken {
                return false;
            }
            // Check for un-parenthesized AsyncArrowFunction
            if !self.is_identifier() {
                return false;
            }
            self.next_token_without_check();
            return !self.has_preceding_line_break() && self.token == Kind::EqualsGreaterThanToken;
        }
        false
    }

    /// Go: `func (p *Parser) parseSimpleArrowFunctionExpression(pos int, identifier, ..., jsdoc, asyncModifier) *ast.Node`.
    fn parse_simple_arrow_function_expression(
        &mut self,
        pos: TextPos,
        identifier: NodeId,
        allow_return_type_in_arrow_function: bool,
        jsdoc: JSDocScannerInfo,
        async_modifier: Option<ModifierList>,
    ) -> NodeId {
        debug_assert!(
            self.token == Kind::EqualsGreaterThanToken,
            "parseSimpleArrowFunctionExpression should only have been called if we had a =>"
        );
        let identifier_pos = {
            let s = self.factory.store();
            s.node(identifier).pos()
        };
        let parameter = {
            let __hoist_1_8 = self.factory.new_parameter_declaration(
                None, /*modifiers*/
                None, /*dotDotDotToken*/
                identifier, None, /*questionToken*/
                None, /*typeNode*/
                None, /*initializer*/
            );
            let __hoist_1_9 = identifier_pos;
            self.finish_node(__hoist_1_8, __hoist_1_9)
        };
        let parameter_loc = {
            let s = self.factory.store();
            s.node(parameter).loc
        };
        let parameters = self.new_node_list(parameter_loc, vec![parameter]);
        let equals_greater_than_token = self.parse_expected_token(Kind::EqualsGreaterThanToken);
        let body = self.parse_arrow_function_expression_body(
            async_modifier.is_some(), /*isAsync*/
            allow_return_type_in_arrow_function,
        );
        let result = {
            let __hoist_1_11 = self.factory.new_arrow_function(
                async_modifier,
                None, /*typeParameters*/
                Some(parameters),
                None, /*returnType*/
                None, /*fullSignature*/
                equals_greater_than_token,
                Some(body),
            );
            let __hoist_1_12 = pos;
            self.finish_node(__hoist_1_11, __hoist_1_12)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseConditionalExpressionRest(leftOperand *ast.Expression, pos int, allowReturnTypeInArrowFunction bool) *ast.Expression`.
    fn parse_conditional_expression_rest(
        &mut self,
        left_operand: NodeId,
        pos: TextPos,
        allow_return_type_in_arrow_function: bool,
    ) -> NodeId {
        // Note: we are passed in an expression which was produced from parseBinaryExpressionOrHigher.
        let question_token = self.parse_optional_token(Kind::QuestionToken);
        let Some(question_token) = question_token else {
            return left_operand;
        };
        // Note: we explicitly 'allowIn' in the whenTrue part of the condition expression, and
        // we do not that for the 'whenFalse' part.
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DISALLOW_IN_CONTEXT, false);
        let true_expression = self.parse_assignment_expression_or_higher_worker(
            false, /*allowReturnTypeInArrowFunction*/
        );
        self.context_flags = save_context_flags;
        let colon_token = self.parse_expected_token(Kind::ColonToken);

        let res = {
            let s = self.factory.store();
            tsc_ast::node_is_present(s.node(colon_token))
        };
        let false_expression: NodeId = if res {
            self.parse_assignment_expression_or_higher_worker(allow_return_type_in_arrow_function)
        } else {
            self.create_missing_identifier()
        };
        {
            let __hoist_1_14 = self.factory.new_conditional_expression(
                left_operand,
                question_token,
                true_expression,
                colon_token,
                false_expression,
            );
            let __hoist_1_15 = pos;
            self.finish_node(__hoist_1_14, __hoist_1_15)
        }
    }

    /// Go: `func (p *Parser) parseBinaryExpressionOrHigher(precedence ast.OperatorPrecedence) *ast.Expression`.
    pub(crate) fn parse_binary_expression_or_higher(
        &mut self,
        precedence: OperatorPrecedence,
    ) -> NodeId {
        let pos = self.node_pos();
        let left_operand = self.parse_unary_expression_or_higher();
        self.parse_binary_expression_rest(precedence, left_operand, pos)
    }

    /// Go: `func (p *Parser) parseBinaryExpressionRest(precedence ast.OperatorPrecedence, leftOperand *ast.Expression, pos int) *ast.Expression`.
    fn parse_binary_expression_rest(
        &mut self,
        precedence: OperatorPrecedence,
        left_operand: NodeId,
        pos: TextPos,
    ) -> NodeId {
        let mut left_operand = left_operand;
        let mut last_operand = left_operand;
        loop {
            // We either have a binary operator here, or we're finished.  We call
            // reScanGreaterToken so that we merge token sequences like > and = into >=
            let operator = self.re_scan_greater_than_token();
            let new_precedence = tsc_ast::get_binary_operator_precedence(operator);
            // Check the precedence to see if we should "take" this operator
            // - For left associative operator (all operator but **), consume the operator,
            //   recursively call the function below, and parse binaryExpression as a rightOperand
            //   of the caller if the new precedence of the operator is greater then or equal to the current precedence.
            //   For example:
            //      a - b - c;
            //            ^token; leftOperand = b. Return b to the caller as a rightOperand
            //      a * b - c
            //            ^token; leftOperand = b. Return b to the caller as a rightOperand
            //      a - b * c;
            //            ^token; leftOperand = b. Return b * c to the caller as a rightOperand
            // - For right associative operator (**), consume the operator, recursively call the function
            //   and parse binaryExpression as a rightOperand of the caller if the new precedence of
            //   the operator is strictly grater than the current precedence
            //   For example:
            //      a ** b ** c;
            //             ^^token; leftOperand = b. Return b ** c to the caller as a rightOperand
            //      a - b ** c;
            //            ^^token; leftOperand = b. Return b ** c to the caller as a rightOperand
            //      a ** b - c
            //             ^token; leftOperand = b. Return b to the caller as a rightOperand
            if !should_consume_binary_operator(operator, new_precedence, precedence) {
                break;
            }
            if operator == Kind::InKeyword && self.in_disallow_in_context() {
                break;
            }
            if operator == Kind::AsKeyword || operator == Kind::SatisfiesKeyword {
                // Make sure we *do* perform ASI for constructs like this:
                //    var x = foo
                //    as (Bar)
                // This should be parsed as an initialized variable, followed
                // by a function call to 'as' with the argument 'Bar'
                if self.has_preceding_line_break() {
                    break;
                }
                self.next_token();
                // When we have 'a ## b as SomeType $$ c' or 'a ## b satisfies SomeType $$ c', where ## and $$
                // are binary operators, we want to stop parsing when $$ would bind before ## after erasing the
                // assertion. See https://github.com/microsoft/TypeScript/issues/63527.
                let mut last_precedence = OperatorPrecedence::HIGHEST;
                let last_kind_is_binary = {
                    let s = self.factory.store();
                    tsc_ast::is_binary_expression(s.node(last_operand))
                };
                if last_kind_is_binary {
                    let operator_token_kind = {
                        let s = self.factory.store();
                        s.node(last_operand)
                            .as_binary_expression()
                            .expect("BinaryExpression data")
                            .operator_token
                    };
                    last_precedence = tsc_ast::get_binary_operator_precedence({
                        let s = self.factory.store();
                        s.node(operator_token_kind).kind
                    });
                }
                if operator == Kind::SatisfiesKeyword {
                    let type_node = self.parse_type();
                    left_operand = self.make_satisfies_expression(left_operand, type_node);
                } else {
                    let type_node = self.parse_type();
                    left_operand = self.make_as_expression(left_operand, type_node);
                }
                // Stop if the next operator would bind before the last operator when the assertion is erased.
                let next_operator = self.re_scan_greater_than_token();
                let next_precedence = tsc_ast::get_binary_operator_precedence(next_operator);
                if should_consume_binary_operator(next_operator, next_precedence, last_precedence) {
                    break;
                }
            } else {
                let operator_token = self.parse_token_node();
                let right = self.parse_binary_expression_or_higher(new_precedence);
                left_operand =
                    self.make_binary_expression(left_operand, operator_token, right, pos);
                last_operand = left_operand;
            }
        }
        left_operand
    }

    /// Go: `func (p *Parser) makeSatisfiesExpression(expression *ast.Expression, typeNode *ast.TypeNode) *ast.Node`.
    fn make_satisfies_expression(&mut self, expression: NodeId, type_node: NodeId) -> NodeId {
        let pos = {
            let s = self.factory.store();
            s.node(expression).pos()
        };
        let finished = {
            let __hoist_1_17 = self.factory.new_satisfies_expression(expression, type_node);
            let __hoist_1_18 = pos;
            self.finish_node(__hoist_1_17, __hoist_1_18)
        };
        self.check_js_syntax(finished)
    }

    /// Go: `func (p *Parser) makeAsExpression(left *ast.Expression, right *ast.TypeNode) *ast.Node`.
    fn make_as_expression(&mut self, left: NodeId, right: NodeId) -> NodeId {
        let pos = {
            let s = self.factory.store();
            s.node(left).pos()
        };
        let finished = {
            let __hoist_1_20 = self.factory.new_as_expression(left, right);
            let __hoist_1_21 = pos;
            self.finish_node(__hoist_1_20, __hoist_1_21)
        };
        self.check_js_syntax(finished)
    }

    /// Go: `func (p *Parser) makeBinaryExpression(left, operatorToken, right *ast.Expression, pos int) *ast.Node`.
    fn make_binary_expression(
        &mut self,
        left: NodeId,
        operator_token: NodeId,
        right: NodeId,
        pos: TextPos,
    ) -> NodeId {
        {
            let __hoist_1_23 = self.factory.new_binary_expression(
                None, /*modifiers*/
                left,
                None, /*typeNode*/
                operator_token,
                right,
            );
            let __hoist_1_24 = pos;
            self.finish_node(__hoist_1_23, __hoist_1_24)
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Unary expressions
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseUnaryExpressionOrHigher() *ast.Expression`.
    pub(crate) fn parse_unary_expression_or_higher(&mut self) -> NodeId {
        // ES7 UpdateExpression:
        //      1) LeftHandSideExpression[?Yield]
        //      2) LeftHandSideExpression[?Yield][no LineTerminator here]++
        //      3) LeftHandSideExpression[?Yield][no LineTerminator here]--
        //      4) ++UnaryExpression[?Yield]
        //      5) --UnaryExpression[?Yield]
        if self.is_update_expression() {
            let pos = self.node_pos();
            let update_expression = self.parse_update_expression();
            if self.token == Kind::AsteriskAsteriskToken {
                let precedence = tsc_ast::get_binary_operator_precedence(self.token);
                return self.parse_binary_expression_rest(precedence, update_expression, pos);
            }
            return update_expression;
        }
        // ES7 UnaryExpression:
        //      1) UpdateExpression[?yield]
        //      2) delete UpdateExpression[?yield]
        //      3) void UpdateExpression[?yield]
        //      4) typeof UpdateExpression[?yield]
        //      5) + UpdateExpression[?yield]
        //      6) - UpdateExpression[?yield]
        //      7) ~ UpdateExpression[?yield]
        //      8) ! UpdateExpression[?yield]
        let unary_operator = self.token;
        let simple_unary_expression = self.parse_simple_unary_expression();
        if self.token == Kind::AsteriskAsteriskToken {
            let (kind, pos, end) = {
                let s = self.factory.store();
                let n = s.node(simple_unary_expression);
                (n.kind, n.pos(), n.end())
            };
            let pos = tsc_scanner::skip_trivia(self.source_text, pos);
            if kind == Kind::TypeAssertionExpression {
                self.parse_error_at(
                    pos,
                    end,
                    &tsc_diagnostics::A_TYPE_ASSERTION_EXPRESSION_IS_NOT_ALLOWED_IN_THE_LEFT_HAND_SIDE_OF_AN_EXPONENTIATION_EXPRESSION_CONSIDER_ENCLOSING_THE_EXPRESSION_IN_PARENTHESES,
                    &[],
                );
            } else {
                debug_assert!(is_keyword_or_punctuation(unary_operator));
                self.parse_error_at(
                    pos,
                    end,
                    &tsc_diagnostics::AN_UNARY_EXPRESSION_WITH_THE_0_OPERATOR_IS_NOT_ALLOWED_IN_THE_LEFT_HAND_SIDE_OF_AN_EXPONENTIATION_EXPRESSION_CONSIDER_ENCLOSING_THE_EXPRESSION_IN_PARENTHESES,
                    &[token_to_text(unary_operator).to_string()],
                );
            }
        }
        simple_unary_expression
    }

    /// Go: `func (p *Parser) isUpdateExpression() bool`.
    fn is_update_expression(&self) -> bool {
        match self.token {
            Kind::PlusToken
            | Kind::MinusToken
            | Kind::TildeToken
            | Kind::ExclamationToken
            | Kind::DeleteKeyword
            | Kind::TypeOfKeyword
            | Kind::VoidKeyword
            | Kind::AwaitKeyword => false,
            Kind::LessThanToken => self.language_variant == LanguageVariant::JSX,
            _ => true,
        }
    }

    /// Go: `func (p *Parser) parseUpdateExpression() *ast.Expression`.
    fn parse_update_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        if self.token == Kind::PlusPlusToken || self.token == Kind::MinusMinusToken {
            let operator = self.token;
            self.next_token();
            let operand = self.parse_left_hand_side_expression_or_higher();
            return {
                let __hoist_1_26 = self.factory.new_prefix_unary_expression(operator, operand);
                let __hoist_1_27 = pos;
                self.finish_node(__hoist_1_26, __hoist_1_27)
            };
        } else if self.language_variant == LanguageVariant::JSX
            && self.token == Kind::LessThanToken
            && self.look_ahead(|p| p.next_token_is_identifier_or_keyword_or_greater_than())
        {
            // JSXElement is part of primaryExpression
            return self.parse_jsx_element_or_self_closing_element_or_fragment(
                true,  /*inExpressionContext*/
                -1,    /*topInvalidNodePosition*/
                None,  /*openingTag*/
                false, /*mustBeUnary*/
            );
        }
        let expression = self.parse_left_hand_side_expression_or_higher();
        if (self.token == Kind::PlusPlusToken || self.token == Kind::MinusMinusToken)
            && !self.has_preceding_line_break()
        {
            let operator = self.token;
            self.next_token();
            return {
                let __hoist_1_29 = self
                    .factory
                    .new_postfix_unary_expression(expression, operator);
                let __hoist_1_30 = pos;
                self.finish_node(__hoist_1_29, __hoist_1_30)
            };
        }
        expression
    }

    /// Go: `func (p *Parser) parseSimpleUnaryExpression() *ast.Expression`.
    fn parse_simple_unary_expression(&mut self) -> NodeId {
        match self.token {
            Kind::PlusToken | Kind::MinusToken | Kind::TildeToken | Kind::ExclamationToken => {
                self.parse_prefix_unary_expression()
            }
            Kind::DeleteKeyword => self.parse_delete_expression(),
            Kind::TypeOfKeyword => self.parse_typeof_expression(),
            Kind::VoidKeyword => self.parse_void_expression(),
            Kind::LessThanToken => {
                // Just like in parseUpdateExpression, we need to avoid parsing type assertions when
                // in JSX and we see an expression like "+ <foo> bar".
                if self.language_variant == LanguageVariant::JSX {
                    return self.parse_jsx_element_or_self_closing_element_or_fragment(
                        true, /*inExpressionContext*/
                        -1,   /*topInvalidNodePosition*/
                        None, /*openingTag*/
                        true, /*mustBeUnary*/
                    );
                }
                // // This is modified UnaryExpression grammar in TypeScript
                // //  UnaryExpression (modified):
                // //      < type > UnaryExpression
                self.parse_type_assertion()
            }
            // Go: `case ast.KindAwaitKeyword: if p.isAwaitExpression() { return
            // p.parseAwaitExpression() }; fallthrough; default: return
            // p.parseUpdateExpression()` — a match guard cannot mutably
            // borrow, so the guard is an in-arm `if` (PORT).
            Kind::AwaitKeyword => {
                if self.is_await_expression() {
                    return self.parse_await_expression();
                }
                self.parse_update_expression()
            }
            _ => self.parse_update_expression(),
        }
    }

    /// Go: `func (p *Parser) parsePrefixUnaryExpression() *ast.Node`.
    pub(crate) fn parse_prefix_unary_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let operator = self.token;
        self.next_token();
        let operand = self.parse_simple_unary_expression();
        {
            let __hoist_1_32 = self.factory.new_prefix_unary_expression(operator, operand);
            let __hoist_1_33 = pos;
            self.finish_node(__hoist_1_32, __hoist_1_33)
        }
    }

    /// Go: `func (p *Parser) parseDeleteExpression() *ast.Node`.
    fn parse_delete_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        {
            let __hoist_1_35 = self.factory.new_delete_expression(expression);
            let __hoist_1_36 = pos;
            self.finish_node(__hoist_1_35, __hoist_1_36)
        }
    }

    /// Go: `func (p *Parser) parseTypeOfExpression() *ast.Node`.
    fn parse_typeof_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        {
            let __hoist_1_38 = self.factory.new_type_of_expression(expression);
            let __hoist_1_39 = pos;
            self.finish_node(__hoist_1_38, __hoist_1_39)
        }
    }

    /// Go: `func (p *Parser) parseVoidExpression() *ast.Node`.
    fn parse_void_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        {
            let __hoist_1_41 = self.factory.new_void_expression(expression);
            let __hoist_1_42 = pos;
            self.finish_node(__hoist_1_41, __hoist_1_42)
        }
    }

    /// Go: `func (p *Parser) isAwaitExpression() bool`.
    fn is_await_expression(&mut self) -> bool {
        if self.token == Kind::AwaitKeyword {
            if self.in_await_context() {
                return true;
            }
            // here we are using similar heuristics as 'isYieldExpression'
            return self
                .look_ahead(|p| p.next_token_is_identifier_or_keyword_or_literal_on_same_line());
        }
        false
    }

    /// Go: `func (p *Parser) parseAwaitExpression() *ast.Node`.
    fn parse_await_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        {
            let __hoist_1_44 = self.factory.new_await_expression(expression);
            let __hoist_1_45 = pos;
            self.finish_node(__hoist_1_44, __hoist_1_45)
        }
    }

    /// Go: `func (p *Parser) parseTypeAssertion() *ast.Node`.
    fn parse_type_assertion(&mut self) -> NodeId {
        debug_assert!(
            self.language_variant != LanguageVariant::JSX,
            "Type assertions should never be parsed in JSX; they should be parsed as comparisons or JSX elements/fragments."
        );
        let pos = self.node_pos();
        self.parse_expected(Kind::LessThanToken);
        let type_node = self.parse_type();
        self.parse_expected(Kind::GreaterThanToken);
        let expression = self.parse_simple_unary_expression();
        {
            let __hoist_1_47 = self.factory.new_type_assertion(type_node, expression);
            let __hoist_1_48 = pos;
            self.finish_node(__hoist_1_47, __hoist_1_48)
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Left-hand-side expressions
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parseLeftHandSideExpressionOrHigher() *ast.Expression`.
    pub(crate) fn parse_left_hand_side_expression_or_higher(&mut self) -> NodeId {
        // Original Ecma:
        // LeftHandSideExpression: See 11.2
        //      NewExpression
        //      CallExpression
        //
        // Our simplification:
        //
        // LeftHandSideExpression: See 11.2
        //      MemberExpression
        //      CallExpression
        //
        // See comment in parseMemberExpressionOrHigher on how we replaced NewExpression with
        // MemberExpression to make our lives easier.
        //
        // to best understand the below code, it's important to see how CallExpression expands
        // out into its own productions:
        //
        // CallExpression:
        //      MemberExpression Arguments
        //      CallExpression Arguments
        //      CallExpression[Expression]
        //      CallExpression.IdentifierName
        //      import (AssignmentExpression)
        //      super Arguments
        //      super.IdentifierName
        //
        // Because of the recursion in these calls, we need to bottom out first. There are three
        // bottom out states we can run into: 1) We see 'super' which must start either of
        // the last two CallExpression productions. 2) We see 'import' which must start import call.
        // 3)we have a MemberExpression which either completes the LeftHandSideExpression,
        // or starts the beginning of the first four CallExpression productions.
        let pos = self.node_pos();
        let expression: NodeId;
        if self.token == Kind::ImportKeyword {
            if self.look_ahead(|p| p.next_token_is_open_paren_or_less_than()) {
                // We don't want to eagerly consume all import keyword as import call expression so we look ahead to find "("
                // For example:
                //      var foo3 = require("subfolder
                //      import * as foo1 from "module-from-node
                // We want this import to be a statement rather than import call expression
                self.source_flags |= NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                expression = self.parse_keyword_expression();
            } else if self.look_ahead(|p| p.next_token_is_dot()) {
                // This is an 'import.*' metaproperty (i.e. 'import.meta')
                self.next_token(); // advance past the 'import'
                self.next_token(); // advance past the dot
                let name = self.parse_import_meta_property_name();
                expression = {
                    let __hoist_1_50 = self.factory.new_meta_property(Kind::ImportKeyword, name);
                    let __hoist_1_51 = pos;
                    self.finish_node(__hoist_1_50, __hoist_1_51)
                };
                let is_import_phase_meta_property = {
                    let s = self.factory.store();
                    is_import_phase_meta_property(s, expression)
                };
                if is_import_phase_meta_property {
                    if self.token == Kind::OpenParenToken || self.token == Kind::LessThanToken {
                        self.source_flags |= NodeFlags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
                    }
                } else {
                    self.source_flags |= NodeFlags::POSSIBLY_CONTAINS_IMPORT_META;
                }
            } else {
                expression = self.parse_member_expression_or_higher();
            }
        } else if self.token == Kind::SuperKeyword {
            expression = self.parse_super_expression();
        } else {
            expression = self.parse_member_expression_or_higher();
        }
        // Now, we *may* be complete.  However, we might have consumed the start of a
        // CallExpression or OptionalExpression.  As such, we need to consume the rest
        // of it here to be complete.
        self.parse_call_expression_rest(pos, expression)
    }

    /// Go: `func (p *Parser) nextTokenIsDot() bool`.
    fn next_token_is_dot(&mut self) -> bool {
        self.next_token() == Kind::DotToken
    }

    /// Go: `func (p *Parser) parseImportMetaPropertyName() *ast.Node`.
    fn parse_import_meta_property_name(&mut self) -> NodeId {
        let token = self.token;
        match token {
            Kind::DeferKeyword | Kind::SourceKeyword
                if self.current_import_phase_modifier() == Kind::Unknown =>
            {
                self.parse_error_at_current_token(
                    &tsc_diagnostics::KEYWORDS_CANNOT_CONTAIN_ESCAPE_CHARACTERS,
                    &[],
                );
            }
            _ => {}
        }
        self.parse_identifier_name()
    }

    /// Go: `func (p *Parser) parseSuperExpression() *ast.Expression`.
    fn parse_super_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let mut expression = self.parse_keyword_expression();
        if self.token == Kind::LessThanToken {
            let start_pos = self.node_pos();
            let type_arguments = self.try_parse_type_arguments_in_expression();
            if type_arguments.is_some() {
                ({
                    let __hoist_1_53 = start_pos;
                    let __hoist_1_54 = self.node_pos();
                    let __hoist_1_55 = &tsc_diagnostics::X_SUPER_MAY_NOT_USE_TYPE_ARGUMENTS;
                    let __hoist_1_56 = &[];
                    self.parse_error_at(__hoist_1_53, __hoist_1_54, __hoist_1_55, __hoist_1_56)
                });
                if !self.is_template_start_of_tagged_template() {
                    expression = {
                        let __hoist_1_58 = self
                            .factory
                            .new_expression_with_type_arguments(expression, type_arguments);
                        let __hoist_1_59 = pos;
                        self.finish_node(__hoist_1_58, __hoist_1_59)
                    };
                }
            }
        }
        if self.token == Kind::OpenParenToken
            || self.token == Kind::DotToken
            || self.token == Kind::OpenBracketToken
        {
            return expression;
        }
        // If we have seen "super" it must be followed by '(' or '.'.
        // If it wasn't then just try to parse out a '.' and report an error.
        self.parse_error_at_current_token(
            &tsc_diagnostics::X_SUPER_MUST_BE_FOLLOWED_BY_AN_ARGUMENT_LIST_OR_MEMBER_ACCESS,
            &[],
        );
        // private names will never work with `super` (`super.#foo`), but that's a semantic error, not syntactic
        let name = self.parse_right_side_of_dot(
            true, /*allowIdentifierNames*/
            true, /*allowPrivateIdentifiers*/
            true, /*allowUnicodeEscapeSequenceInIdentifierName*/
        );
        {
            let __hoist_1_61 = self.factory.new_property_access_expression(
                expression,
                None, /*questionDotToken*/
                name,
                NodeFlags::NONE,
            );
            let __hoist_1_62 = pos;
            self.finish_node(__hoist_1_61, __hoist_1_62)
        }
    }

    /// Go: `func (p *Parser) isTemplateStartOfTaggedTemplate() bool`.
    fn is_template_start_of_tagged_template(&self) -> bool {
        self.token == Kind::NoSubstitutionTemplateLiteral || self.token == Kind::TemplateHead
    }

    /// Go: `func (p *Parser) tryParseTypeArgumentsInExpression() *ast.NodeList`.
    fn try_parse_type_arguments_in_expression(&mut self) -> Option<NodeList> {
        // TypeArguments must not be parsed in JavaScript files to avoid ambiguity with binary operators.
        // Check the cheap preconditions before saving the parser state: unless the current token is `<`
        // (or `<<`, which reScanLessThanToken would split), there is nothing to speculatively parse and
        // the mark/rewind would be a no-op.
        if self.context_flags.intersects(NodeFlags::JAVASCRIPT_FILE)
            || (self.token != Kind::LessThanToken && self.token != Kind::LessThanLessThanToken)
        {
            return None;
        }
        let state = self.mark();
        if self.re_scan_less_than_token() == Kind::LessThanToken {
            self.next_token();
            let type_arguments =
                self.parse_delimited_list(PC_TYPE_ARGUMENTS, |p| Some(p.parse_type()));
            // If it doesn't have the closing `>` then it's definitely not an type argument list.
            if self.re_scan_greater_than_token() == Kind::GreaterThanToken {
                self.next_token();
                // We successfully parsed a type argument list. The next token determines whether we want to
                // treat it as such. If the type argument list is followed by `(` or a template literal, as in
                // `f<number>(42)`, we favor the type argument interpretation even though JavaScript would view
                // it as a relational expression.
                if self.can_follow_type_arguments_in_expression() {
                    return type_arguments;
                }
            }
        }
        self.rewind(state);
        None
    }

    /// Go: `func (p *Parser) canFollowTypeArgumentsInExpression() bool`.
    fn can_follow_type_arguments_in_expression(&mut self) -> bool {
        match self.token {
            // These tokens can follow a type argument list in a call expression:
            // foo<x>(
            // foo<T> `...`
            // foo<T> `...${100}...`
            Kind::OpenParenToken | Kind::NoSubstitutionTemplateLiteral | Kind::TemplateHead => {
                return true
            }
            // A type argument list followed by `<` never makes sense, and a type argument list followed
            // by `>` is ambiguous with a (re-scanned) `>>` operator, so we disqualify both. Also, in
            // this context, `+` and `-` are unary operators, not binary operators.
            Kind::LessThanToken | Kind::GreaterThanToken | Kind::PlusToken | Kind::MinusToken => {
                return false
            }
            _ => {}
        }
        // We favor the type argument list interpretation when it is immediately followed by
        // a line break, a binary operator, or something that can't start an expression.
        self.has_preceding_line_break()
            || self.is_binary_operator()
            || !self.is_start_of_expression()
    }

    /// Go: `func (p *Parser) parseMemberExpressionOrHigher() *ast.Node`.
    fn parse_member_expression_or_higher(&mut self) -> NodeId {
        // Note: to make our lives simpler, we decompose the NewExpression productions and
        // place ObjectCreationExpression and FunctionExpression into PrimaryExpression.
        // like so:
        //
        //   PrimaryExpression : See 11.1
        //      this
        //      Identifier
        //      Literal
        //      ArrayLiteral
        //      ObjectLiteral
        //      (Expression)
        //      FunctionExpression
        //      new MemberExpression Arguments?
        //
        //   MemberExpression : See 11.2
        //      PrimaryExpression
        //      MemberExpression[Expression]
        //      MemberExpression.IdentifierName
        //
        //   CallExpression : See 11.2
        //      MemberExpression
        //      CallExpression Arguments
        //      CallExpression[Expression]
        //      CallExpression.IdentifierName
        //
        // Technically this is ambiguous.  i.e. CallExpression defines:
        //
        //   CallExpression:
        //      CallExpression Arguments
        //
        // If you see: "new Foo()"
        //
        // Then that could be treated as a single ObjectCreationExpression, or it could be
        // treated as the invocation of "new Foo".  We disambiguate that in code (to match
        // the original grammar) by making sure that if we see an ObjectCreationExpression
        // we always consume arguments if they are there. So we treat "new Foo()" as an
        // object creation only, and not at all as an invocation.  Another way to think
        // about this is that for every "new" that we see, we will consume an argument list if
        // it is there as part of the *associated* object creation node.  Any additional
        // argument lists we see, will become invocation expressions.
        //
        // Because there are no other places in the grammar now that refer to FunctionExpression
        // or ObjectCreationExpression, it is safe to push down into the PrimaryExpression
        // production.
        //
        // Because CallExpression and MemberExpression are left recursive, we need to bottom out
        // of the recursion immediately.  So we parse out a primary expression to start with.
        let pos = self.node_pos();
        let expression = self.parse_primary_expression();
        self.parse_member_expression_rest(pos, expression, true /*allowOptionalChain*/)
    }

    /// Go: `func (p *Parser) parseMemberExpressionRest(pos int, expression *ast.Expression, allowOptionalChain bool) *ast.Expression`.
    pub(crate) fn parse_member_expression_rest(
        &mut self,
        pos: TextPos,
        mut expression: NodeId,
        allow_optional_chain: bool,
    ) -> NodeId {
        loop {
            let mut question_dot_token: Option<NodeId> = None;
            let is_property_access: bool;
            if allow_optional_chain && self.is_start_of_optional_property_or_element_access_chain()
            {
                question_dot_token = Some(self.parse_expected_token(Kind::QuestionDotToken));
                is_property_access = token_is_identifier_or_keyword(self.token);
            } else {
                is_property_access = self.parse_optional(Kind::DotToken);
            }
            if is_property_access {
                expression =
                    self.parse_property_access_expression_rest(pos, expression, question_dot_token);
                continue;
            }
            // when in the [Decorator] context, we do not parse ElementAccess as it could be part of a ComputedPropertyName
            if (question_dot_token.is_some() || !self.in_decorator_context())
                && self.parse_optional(Kind::OpenBracketToken)
            {
                expression =
                    self.parse_element_access_expression_rest(pos, expression, question_dot_token);
                continue;
            }
            if self.is_template_start_of_tagged_template() {
                // Absorb type arguments into TemplateExpression when preceding expression is ExpressionWithTypeArguments
                let question_dot_is_none = question_dot_token.is_none();
                let is_expr_with_type_args = {
                    let s = self.factory.store();
                    tsc_ast::is_expression_with_type_arguments(s.node(expression))
                };
                if question_dot_is_none && is_expr_with_type_args {
                    let (original_expression, original_type_arguments) = {
                        let s = self.factory.store();
                        let d = s
                            .node(expression)
                            .as_expression_with_type_arguments()
                            .expect("ExpressionWithTypeArguments data");
                        (d.expression, d.type_arguments.clone())
                    };
                    expression = self.parse_tagged_template_rest(
                        pos,
                        original_expression,
                        question_dot_token,
                        original_type_arguments.clone(),
                    );
                    self.unparse_expression_with_type_arguments(
                        original_expression,
                        original_type_arguments.as_ref(),
                        expression,
                    );
                } else {
                    expression =
                        self.parse_tagged_template_rest(pos, expression, question_dot_token, None);
                }
                continue;
            }
            if question_dot_token.is_none() {
                if self.token == Kind::ExclamationToken && !self.has_preceding_line_break() {
                    self.next_token();
                    expression = {
                        let __hoist_1_64 = {
                            let __hoist_2_2 = self
                                .factory
                                .new_non_null_expression(expression, NodeFlags::NONE);
                            let __hoist_2_3 = pos;
                            self.finish_node(__hoist_2_2, __hoist_2_3)
                        };
                        self.check_js_syntax(__hoist_1_64)
                    };
                    continue;
                }
                let type_arguments = self.try_parse_type_arguments_in_expression();
                if type_arguments.is_some() {
                    expression = {
                        let __hoist_1_66 = self
                            .factory
                            .new_expression_with_type_arguments(expression, type_arguments);
                        let __hoist_1_67 = pos;
                        self.finish_node(__hoist_1_66, __hoist_1_67)
                    };
                    continue;
                }
            }
            return expression;
        }
    }

    /// Go: `func (p *Parser) isStartOfOptionalPropertyOrElementAccessChain() bool`.
    fn is_start_of_optional_property_or_element_access_chain(&mut self) -> bool {
        self.token == Kind::QuestionDotToken
            && self
                .look_ahead(|p| p.next_token_is_identifier_or_keyword_or_open_bracket_or_template())
    }

    /// Go: `func (p *Parser) nextTokenIsIdentifierOrKeywordOrOpenBracketOrTemplate() bool`.
    fn next_token_is_identifier_or_keyword_or_open_bracket_or_template(&mut self) -> bool {
        self.next_token();
        token_is_identifier_or_keyword(self.token)
            || self.token == Kind::OpenBracketToken
            || self.is_template_start_of_tagged_template()
    }

    /// Go: `func (p *Parser) parsePropertyAccessExpressionRest(pos int, expression *ast.Expression, questionDotToken *ast.Node) *ast.Node`.
    fn parse_property_access_expression_rest(
        &mut self,
        pos: TextPos,
        expression: NodeId,
        question_dot_token: Option<NodeId>,
    ) -> NodeId {
        let name = self.parse_right_side_of_dot(
            true, /*allowIdentifierNames*/
            true, /*allowPrivateIdentifiers*/
            true, /*allowUnicodeEscapeSequenceInIdentifierName*/
        );
        let is_optional_chain =
            question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
        let property_access = self.factory.new_property_access_expression(
            expression,
            question_dot_token,
            name,
            if is_optional_chain {
                NodeFlags::OPTIONAL_CHAIN
            } else {
                NodeFlags::NONE
            },
        );
        if is_optional_chain {
            let name_is_private = {
                let s = self.factory.store();
                tsc_ast::is_private_identifier(s.node(name))
            };
            if name_is_private {
                let loc = {
                    let s = self.factory.store();
                    s.node(name).loc
                };
                ({
                    let __hoist_1_69 = self.skip_range_trivia(loc);
                    let __hoist_1_70 =
                        &tsc_diagnostics::AN_OPTIONAL_CHAIN_CANNOT_CONTAIN_PRIVATE_IDENTIFIERS;
                    let __hoist_1_71 = &[];
                    self.parse_error_at_range(__hoist_1_69, __hoist_1_70, __hoist_1_71)
                });
            }
        }
        let expression_is_with_type_args = {
            let s = self.factory.store();
            tsc_ast::is_expression_with_type_arguments(s.node(expression))
        };
        if expression_is_with_type_args {
            let type_arguments =
                crate::parser::type_argument_list_of(self.factory.store(), expression);
            if let Some(type_arguments) = type_arguments {
                let loc = TextRange::new(
                    type_arguments.loc.pos() - 1,
                    tsc_scanner::skip_trivia(self.source_text, type_arguments.loc.end()) + 1,
                );
                self.parse_error_at_range(
                    loc,
                    &tsc_diagnostics::AN_INSTANTIATION_EXPRESSION_CANNOT_BE_FOLLOWED_BY_A_PROPERTY_ACCESS,
                    &[],
                );
            }
        }
        self.finish_node(property_access, pos)
    }

    /// Go: `func (p *Parser) tryReparseOptionalChain(node *ast.Expression) bool`.
    fn try_reparse_optional_chain(&mut self, node: NodeId) -> bool {
        let has_optional_chain = {
            let s = self.factory.store();
            s.node(node).flags.intersects(NodeFlags::OPTIONAL_CHAIN)
        };
        if has_optional_chain {
            return true;
        }
        // check for an optional chain in a non-null expression
        let node_is_non_null = {
            let s = self.factory.store();
            tsc_ast::is_non_null_expression(s.node(node))
        };
        if node_is_non_null {
            // `expr := node.Expression(); for IsNonNullExpression(expr) &&
            // expr.Flags&OptionalChain == 0 { expr = expr.Expression() }`
            let mut expr = {
                let s = self.factory.store();
                s.node(node)
                    .as_non_null_expression()
                    .expect("NonNullExpression data")
                    .expression
            };
            loop {
                let (is_non_null, has_flag, inner) = {
                    let s = self.factory.store();
                    let n = s.node(expr);
                    (
                        tsc_ast::is_non_null_expression(n),
                        n.flags.intersects(NodeFlags::OPTIONAL_CHAIN),
                        n.as_non_null_expression()
                            .map(|d| d.expression)
                            .unwrap_or(NodeId::NONE),
                    )
                };
                if !is_non_null || has_flag {
                    break;
                }
                expr = inner;
            }
            let expr_has_flag = {
                let s = self.factory.store();
                s.node(expr).flags.intersects(NodeFlags::OPTIONAL_CHAIN)
            };
            if expr_has_flag {
                // this is part of an optional chain. Walk down from `node` to
                // `expression` and set the flag.
                let mut current = node;
                loop {
                    let still_non_null = {
                        let s = self.factory.store();
                        tsc_ast::is_non_null_expression(s.node(current))
                    };
                    if !still_non_null {
                        break;
                    }
                    {
                        let s = self.factory.store();
                        s.node_mut(current).flags |= NodeFlags::OPTIONAL_CHAIN;
                    }
                    let next = {
                        let s = self.factory.store();
                        s.node(current)
                            .as_non_null_expression()
                            .expect("NonNullExpression data")
                            .expression
                    };
                    current = next;
                }
                return true;
            }
        }
        false
    }

    /// Go: `func (p *Parser) parseElementAccessExpressionRest(pos int, expression *ast.Expression, questionDotToken *ast.Node) *ast.Node`.
    fn parse_element_access_expression_rest(
        &mut self,
        pos: TextPos,
        expression: NodeId,
        question_dot_token: Option<NodeId>,
    ) -> NodeId {
        let mut argument_expression = self.create_missing_identifier();
        if self.token == Kind::CloseBracketToken {
            let (p, e) = (self.node_pos(), self.node_pos());
            self.parse_error_at(
                p,
                e,
                &tsc_diagnostics::AN_ELEMENT_ACCESS_EXPRESSION_SHOULD_TAKE_AN_ARGUMENT,
                &[],
            );
        } else {
            argument_expression = self.parse_expression_allow_in();
        }
        self.parse_expected(Kind::CloseBracketToken);
        let is_optional_chain =
            question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
        {
            let __hoist_1_73 = self.factory.new_element_access_expression(
                expression,
                question_dot_token,
                argument_expression,
                if is_optional_chain {
                    NodeFlags::OPTIONAL_CHAIN
                } else {
                    NodeFlags::NONE
                },
            );
            let __hoist_1_74 = pos;
            self.finish_node(__hoist_1_73, __hoist_1_74)
        }
    }

    /// Go: `func (p *Parser) parseCallExpressionRest(pos int, expression *ast.Expression) *ast.Expression`.
    pub(crate) fn parse_call_expression_rest(
        &mut self,
        pos: TextPos,
        mut expression: NodeId,
    ) -> NodeId {
        loop {
            expression = self
                .parse_member_expression_rest(pos, expression /*allowOptionalChain*/, true);
            let mut type_arguments: Option<NodeList> = None;
            let question_dot_token = self.parse_optional_token(Kind::QuestionDotToken);
            if question_dot_token.is_some() {
                type_arguments = self.try_parse_type_arguments_in_expression();
                if self.is_template_start_of_tagged_template() {
                    expression = self.parse_tagged_template_rest(
                        pos,
                        expression,
                        question_dot_token,
                        type_arguments,
                    );
                    continue;
                }
            }
            if type_arguments.is_some() || self.token == Kind::OpenParenToken {
                // Absorb type arguments into CallExpression when preceding expression is ExpressionWithTypeArguments
                let question_dot_is_none = question_dot_token.is_none();
                let expression_kind = {
                    let s = self.factory.store();
                    s.node(expression).kind
                };
                if question_dot_is_none && expression_kind == Kind::ExpressionWithTypeArguments {
                    let (inner_expression, inner_type_arguments) = {
                        let s = self.factory.store();
                        let d = s
                            .node(expression)
                            .as_expression_with_type_arguments()
                            .expect("ExpressionWithTypeArguments data");
                        (d.expression, d.type_arguments.clone())
                    };
                    type_arguments = inner_type_arguments;
                    expression = inner_expression;
                }
                let inner = expression;
                let argument_list = self.parse_argument_list();
                let is_optional_chain =
                    question_dot_token.is_some() || self.try_reparse_optional_chain(expression);
                let finished = {
                    let __hoist_1_76 = self.factory.new_call_expression(
                        expression,
                        question_dot_token,
                        type_arguments.clone(),
                        argument_list,
                        if is_optional_chain {
                            NodeFlags::OPTIONAL_CHAIN
                        } else {
                            NodeFlags::NONE
                        },
                    );
                    let __hoist_1_77 = pos;
                    self.finish_node(__hoist_1_76, __hoist_1_77)
                };
                expression = self.check_js_syntax(finished);
                self.unparse_expression_with_type_arguments(
                    inner,
                    type_arguments.as_ref(),
                    expression,
                );
                continue;
            }
            if let Some(question_dot_token) = question_dot_token {
                // We parsed `?.` but then failed to parse anything, so report a missing identifier here.
                self.parse_error_at_current_token(&tsc_diagnostics::IDENTIFIER_EXPECTED, &[]);
                let name = self.create_missing_identifier();
                expression = {
                    let __hoist_1_79 = self.factory.new_property_access_expression(
                        expression,
                        Some(question_dot_token),
                        name,
                        NodeFlags::OPTIONAL_CHAIN,
                    );
                    let __hoist_1_80 = pos;
                    self.finish_node(__hoist_1_79, __hoist_1_80)
                };
            }
            break;
        }
        expression
    }

    /// Go: `func (p *Parser) parseArgumentList() *ast.NodeList`.
    fn parse_argument_list(&mut self) -> Option<NodeList> {
        self.parse_expected(Kind::OpenParenToken);
        let result = self.parse_delimited_list(PC_ARGUMENT_EXPRESSIONS, |p| {
            Some(p.parse_argument_expression())
        });
        self.parse_expected(Kind::CloseParenToken);
        result
    }

    /// Go: `func (p *Parser) parseArgumentExpression() *ast.Expression`.
    fn parse_argument_expression(&mut self) -> NodeId {
        self.do_in_context(
            NodeFlags::DISALLOW_IN_CONTEXT.union(NodeFlags::DECORATOR_CONTEXT),
            false,
            |p| p.parse_argument_or_array_literal_element(),
        )
    }

    /// Go: `func (p *Parser) parseArgumentOrArrayLiteralElement() *ast.Expression`.
    pub(crate) fn parse_argument_or_array_literal_element(&mut self) -> NodeId {
        match self.token {
            Kind::DotDotDotToken => self.parse_spread_element(),
            Kind::CommaToken => {
                let pos = self.node_pos();
                {
                    let __hoist_1_82 = self.factory.new_omitted_expression();
                    let __hoist_1_83 = pos;
                    self.finish_node(__hoist_1_82, __hoist_1_83)
                }
            }
            _ => self.parse_assignment_expression_or_higher(),
        }
    }

    /// Go: `func (p *Parser) parseSpreadElement() *ast.Node`.
    fn parse_spread_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::DotDotDotToken);
        let expression = self.parse_assignment_expression_or_higher();
        {
            let __hoist_1_85 = self.factory.new_spread_element(expression);
            let __hoist_1_86 = pos;
            self.finish_node(__hoist_1_85, __hoist_1_86)
        }
    }

    /// Go: `func (p *Parser) parseTaggedTemplateRest(pos int, tag, questionDotToken, typeArguments) *ast.Node`.
    fn parse_tagged_template_rest(
        &mut self,
        pos: TextPos,
        tag: NodeId,
        question_dot_token: Option<NodeId>,
        type_arguments: Option<NodeList>,
    ) -> NodeId {
        let template: NodeId = if self.token == Kind::NoSubstitutionTemplateLiteral {
            self.re_scan_template_token(true /*isTaggedTemplate*/);
            self.parse_literal_expression()
        } else {
            self.parse_template_expression(true /*isTaggedTemplate*/)
        };
        let tag_has_optional_chain = {
            let s = self.factory.store();
            s.node(tag).flags.intersects(NodeFlags::OPTIONAL_CHAIN)
        };
        let is_optional_chain = question_dot_token.is_some() || tag_has_optional_chain;
        let finished = {
            let __hoist_1_88 = self.factory.new_tagged_template_expression(
                tag,
                question_dot_token.unwrap_or(NodeId::NONE),
                type_arguments,
                template,
                if is_optional_chain {
                    NodeFlags::OPTIONAL_CHAIN
                } else {
                    NodeFlags::NONE
                },
            );
            let __hoist_1_89 = pos;
            self.finish_node(__hoist_1_88, __hoist_1_89)
        };
        self.check_js_syntax(finished)
    }

    /// Go: `func (p *Parser) parseTemplateExpression(isTaggedTemplate bool) *ast.Expression`.
    fn parse_template_expression(&mut self, is_tagged_template: bool) -> NodeId {
        let pos = self.node_pos();
        let head = self.parse_template_head(is_tagged_template);
        let spans = self.parse_template_spans(is_tagged_template);
        {
            let __hoist_1_91 = self.factory.new_template_expression(head, spans);
            let __hoist_1_92 = pos;
            self.finish_node(__hoist_1_91, __hoist_1_92)
        }
    }

    /// Go: `func (p *Parser) parseTemplateSpans(isTaggedTemplate bool) *ast.NodeList`.
    fn parse_template_spans(&mut self, is_tagged_template: bool) -> Option<NodeList> {
        let pos = self.node_pos();
        let mut list: Vec<NodeId> = Vec::new();
        loop {
            let span = self.parse_template_span(is_tagged_template);
            let literal_kind = {
                let s = self.factory.store();
                let span_data = s.node(span).as_template_span().expect("TemplateSpan data");
                s.node(span_data.literal).kind
            };
            list.push(span);
            if literal_kind != Kind::TemplateMiddle {
                break;
            }
        }
        Some({
            let __hoist_1_94 = TextRange::new(pos, self.node_pos());
            let __hoist_1_95 = list;
            self.new_node_list(__hoist_1_94, __hoist_1_95)
        })
    }

    /// Go: `func (p *Parser) parseTemplateSpan(isTaggedTemplate bool) *ast.Node`.
    fn parse_template_span(&mut self, is_tagged_template: bool) -> NodeId {
        let pos = self.node_pos();
        let expression = self.parse_expression_allow_in();
        let literal = self.parse_literal_of_template_span(is_tagged_template);
        {
            let __hoist_1_97 = self.factory.new_template_span(expression, literal);
            let __hoist_1_98 = pos;
            self.finish_node(__hoist_1_97, __hoist_1_98)
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Primary expressions
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (p *Parser) parsePrimaryExpression() *ast.Expression`.
    pub(crate) fn parse_primary_expression(&mut self) -> NodeId {
        // Go: `case ast.KindNoSubstitutionTemplateLiteral: if TokenFlags&IsInvalid
        // != 0 { reScanTemplateToken(false) }; fallthrough` to the literal arm.
        if self.token == Kind::NoSubstitutionTemplateLiteral
            && self
                .scanner
                .token_flags()
                .intersects(TokenFlags::IS_INVALID)
        {
            self.re_scan_template_token(false /*isTaggedTemplate*/);
        }
        match self.token {
            // PORT: Go's `fallthrough`/`break` switch is a `match` whose
            // value-arms all use `return`; the break arms (AsyncKeyword,
            // SlashToken, default) fall out with `()` and the shared trailing
            // identifier parse runs after the `match`.
            Kind::NoSubstitutionTemplateLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::StringLiteral => return self.parse_literal_expression(),
            Kind::ThisKeyword
            | Kind::SuperKeyword
            | Kind::NullKeyword
            | Kind::TrueKeyword
            | Kind::FalseKeyword => return self.parse_keyword_expression(),
            Kind::OpenParenToken => return self.parse_parenthesized_expression(),
            Kind::OpenBracketToken => return self.parse_array_literal_expression(),
            Kind::OpenBraceToken => return self.parse_object_literal_expression(),
            Kind::AsyncKeyword => {
                // Async arrow functions are parsed earlier in parseAssignmentExpressionOrHigher.
                // If we encounter `async [no LineTerminator here] function` then this is an async
                // function; otherwise, its an identifier.
                if self.look_ahead(|p| p.next_token_is_function_keyword_on_same_line()) {
                    return self.parse_function_expression();
                }
                // Go: `break` out of the switch to the trailing identifier parse below.
            }
            Kind::AtToken => return self.parse_decorated_expression(),
            Kind::ClassKeyword => return self.parse_class_expression(),
            Kind::FunctionKeyword => return self.parse_function_expression(),
            Kind::NewKeyword => return self.parse_new_expression_or_new_dot_target(),
            Kind::SlashToken | Kind::SlashEqualsToken => {
                if self.re_scan_slash_token() == Kind::RegularExpressionLiteral {
                    return self.parse_literal_expression();
                }
            }
            Kind::TemplateHead => {
                return self.parse_template_expression(false /*isTaggedTemplate*/);
            }
            Kind::PrivateIdentifier => return self.parse_private_identifier(),
            _ => {}
        }
        self.parse_identifier_with_diagnostic(Some(&tsc_diagnostics::EXPRESSION_EXPECTED), None)
    }

    /// Go: `func (p *Parser) parseParenthesizedExpression() *ast.Expression`.
    fn parse_parenthesized_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(Kind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(Kind::CloseParenToken);
        let result = {
            let __hoist_1_100 = self.factory.new_parenthesized_expression(expression);
            let __hoist_1_101 = pos;
            self.finish_node(__hoist_1_100, __hoist_1_101)
        };
        self.with_jsdoc(result, jsdoc);
        result
    }

    /// Go: `func (p *Parser) parseArrayLiteralExpression() *ast.Expression`.
    pub(crate) fn parse_array_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let open_bracket_position = self.scanner.token_start() as i32;
        let open_bracket_parsed = self.parse_expected(Kind::OpenBracketToken);
        let multi_line = self.has_preceding_line_break();
        let elements = self.parse_delimited_list(PC_ARRAY_LITERAL_MEMBERS, |p| {
            Some(p.parse_argument_or_array_literal_element())
        });
        self.parse_expected_matching_brackets(
            Kind::OpenBracketToken,
            Kind::CloseBracketToken,
            open_bracket_parsed,
            open_bracket_position,
        );
        {
            let __hoist_1_103 = self
                .factory
                .new_array_literal_expression(elements, multi_line);
            let __hoist_1_104 = pos;
            self.finish_node(__hoist_1_103, __hoist_1_104)
        }
    }

    /// Go: `func (p *Parser) parseObjectLiteralExpression() *ast.Expression`.
    pub(crate) fn parse_object_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let open_brace_position = self.scanner.token_start() as i32;
        let open_brace_parsed = self.parse_expected(Kind::OpenBraceToken);
        let multi_line = self.has_preceding_line_break();
        let properties = self.parse_delimited_list(PC_OBJECT_LITERAL_MEMBERS, |p| {
            Some(p.parse_object_literal_element())
        });
        self.parse_expected_matching_brackets(
            Kind::OpenBraceToken,
            Kind::CloseBraceToken,
            open_brace_parsed,
            open_brace_position,
        );
        {
            let __hoist_1_106 = self
                .factory
                .new_object_literal_expression(properties, multi_line);
            let __hoist_1_107 = pos;
            self.finish_node(__hoist_1_106, __hoist_1_107)
        }
    }

    /// Go: `func (p *Parser) parseObjectLiteralElement() *ast.Node`.
    pub(crate) fn parse_object_literal_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.parse_optional(Kind::DotDotDotToken) {
            let expression = self.parse_assignment_expression_or_higher();
            let result = {
                let __hoist_1_109 = self.factory.new_spread_assignment(expression);
                let __hoist_1_110 = pos;
                self.finish_node(__hoist_1_109, __hoist_1_110)
            };
            self.with_jsdoc(result, jsdoc);
            return result;
        }
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ true, /*permitConstAsModifier*/ false,
            /*stopOnStartOfClassStaticBlock*/ false,
        );
        if self.parse_contextual_modifier(Kind::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::GetAccessor,
                PARSE_FLAGS_NONE,
            );
        }
        if self.parse_contextual_modifier(Kind::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                Kind::SetAccessor,
                PARSE_FLAGS_NONE,
            );
        }
        let asterisk_token = self.parse_optional_token(Kind::AsteriskToken);
        let token_is_identifier = self.is_identifier();
        let name = self.parse_property_name();
        // Disallowing of optional property assignments and definite assignment assertion happens in the grammar checker.
        let mut postfix_token = self.parse_optional_token(Kind::QuestionToken);
        // Decorators, Modifiers, questionToken, and exclamationToken are not supported by property assignments and are reported in the grammar checker
        if postfix_token.is_none() {
            postfix_token = self.parse_optional_token(Kind::ExclamationToken);
        }
        if asterisk_token.is_some()
            || self.token == Kind::OpenParenToken
            || self.token == Kind::LessThanToken
        {
            return self.parse_method_declaration(
                pos,
                jsdoc,
                modifiers,
                asterisk_token,
                name,
                postfix_token,
                None, /*diagnosticMessage*/
            );
        }
        // check if it is short-hand property assignment or normal property assignment
        // NOTE: if token is EqualsToken it is interpreted as CoverInitializedName production
        // CoverInitializedName[Yield] :
        //     IdentifierReference[?Yield] Initializer[In, ?Yield]
        // this is necessary because ObjectLiteral productions are also used to cover grammar for ObjectAssignmentPattern

        let is_shorthand_property_assignment =
            token_is_identifier && self.token != Kind::ColonToken;
        let node: NodeId = if is_shorthand_property_assignment {
            let equals_token = self.parse_optional_token(Kind::EqualsToken);
            let mut initializer: Option<NodeId> = None;
            if equals_token.is_some() {
                initializer = Some(self.do_in_context(
                    NodeFlags::DISALLOW_IN_CONTEXT,
                    false,
                    |p| p.parse_assignment_expression_or_higher(),
                ));
            }
            self.factory.new_shorthand_property_assignment(
                modifiers,
                name,
                postfix_token,
                NodeId::NONE, /*typeNode*/
                equals_token,
                initializer,
            )
        } else {
            self.parse_expected(Kind::ColonToken);
            let initializer = self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, false, |p| {
                p.parse_assignment_expression_or_higher()
            });
            self.factory.new_property_assignment(
                modifiers,
                name,
                postfix_token,
                NodeId::NONE, /*typeNode*/
                initializer,
            )
        };
        self.finish_node(node, pos);
        self.with_jsdoc(node, jsdoc);
        node
    }

    /// Go: `func (p *Parser) parseFunctionExpression() *ast.Expression`.
    fn parse_function_expression(&mut self) -> NodeId {
        // GeneratorExpression:
        //      function* BindingIdentifier [Yield][opt](FormalParameters[Yield]){ GeneratorBody }
        //
        // FunctionExpression:
        //      function BindingIdentifier[opt](FormalParameters){ FunctionBody }
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::DECORATOR_CONTEXT, false);
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        self.parse_expected(Kind::FunctionKeyword);
        let asterisk_token = self.parse_optional_token(Kind::AsteriskToken);
        let is_generator = asterisk_token.is_some();
        let is_async = modifier_list_has_async(self, modifiers.as_ref());
        let signature_flags = (if is_generator {
            PARSE_FLAGS_YIELD
        } else {
            PARSE_FLAGS_NONE
        }) | if is_async {
            PARSE_FLAGS_AWAIT
        } else {
            PARSE_FLAGS_NONE
        };

        let name: Option<NodeId> = if is_generator && is_async {
            self.do_in_context(
                NodeFlags::YIELD_CONTEXT.union(NodeFlags::AWAIT_CONTEXT),
                true,
                |p| p.parse_optional_binding_identifier(),
            )
        } else if is_generator {
            self.do_in_context(NodeFlags::YIELD_CONTEXT, true, |p| {
                p.parse_optional_binding_identifier()
            })
        } else if is_async {
            self.do_in_context(NodeFlags::AWAIT_CONTEXT, true, |p| {
                p.parse_optional_binding_identifier()
            })
        } else {
            self.parse_optional_binding_identifier()
        };
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(signature_flags);
        let return_type = self.parse_return_type(Kind::ColonToken, false /*isType*/);
        let body = self.parse_function_block(signature_flags, None /*diagnosticMessage*/);
        self.context_flags = save_context_flags;
        let result = self.factory.new_function_expression(
            modifiers,
            asterisk_token,
            name,
            type_parameters,
            parameters,
            return_type,
            None, /*fullSignature*/
            Some(body),
        );
        self.finish_node(result, pos);
        self.with_jsdoc(result, jsdoc);
        self.check_js_syntax(result);
        result
    }

    /// Go: `func (p *Parser) parseOptionalBindingIdentifier() *ast.Node`.
    fn parse_optional_binding_identifier(&mut self) -> Option<NodeId> {
        if self.is_binding_identifier() {
            return Some(self.parse_binding_identifier());
        }
        None
    }

    /// Go: `func (p *Parser) parseDecoratedExpression() *ast.Expression`.
    fn parse_decorated_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(
            /*allowDecorators*/ true, /*permitConstAsModifier*/ false,
            /*stopOnStartOfClassStaticBlock*/ false,
        );
        if self.token == Kind::ClassKeyword {
            return self.parse_class_declaration_or_expression(
                pos,
                jsdoc,
                modifiers,
                Kind::ClassExpression,
            );
        }
        let (p, e) = (self.node_pos(), self.node_pos());
        self.parse_error_at(p, e, &tsc_diagnostics::EXPRESSION_EXPECTED, &[]);
        {
            let __hoist_1_112 = self.factory.new_missing_declaration(modifiers);
            let __hoist_1_113 = pos;
            self.finish_node(__hoist_1_112, __hoist_1_113)
        }
    }

    /// Go: `func (p *Parser) unparseExpressionWithTypeArguments(expression *ast.Node, typeArguments *ast.NodeList, result *ast.Node)`.
    ///
    /// Force overwrite the `.Parent` of the expression and type arguments to erase the fact
    /// that they may have originally been parsed as an ExpressionWithTypeArguments and be
    /// parented to such.
    fn unparse_expression_with_type_arguments(
        &mut self,
        expression: NodeId,
        type_arguments: Option<&NodeList>,
        result: NodeId,
    ) {
        if expression != NodeId::NONE {
            let s = self.factory.store();
            s.node_mut(expression).parent.set(result);
        }
        if let Some(type_arguments) = type_arguments {
            let s = self.factory.store();
            for &a in &type_arguments.nodes {
                s.node_mut(a).parent.set(result);
            }
        }
    }

    /// Go: `func (p *Parser) parseNewExpressionOrNewDotTarget() *ast.Node`.
    fn parse_new_expression_or_new_dot_target(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(Kind::NewKeyword);
        if self.parse_optional(Kind::DotToken) {
            let name = self.parse_identifier_name();
            return {
                let __hoist_1_115 = self.factory.new_meta_property(Kind::NewKeyword, name);
                let __hoist_1_116 = pos;
                self.finish_node(__hoist_1_115, __hoist_1_116)
            };
        }
        let expression_pos = self.node_pos();
        let primary = self.parse_primary_expression();
        let mut expression = self.parse_member_expression_rest(
            expression_pos,
            primary,
            false, /*allowOptionalChain*/
        );
        let mut type_arguments: Option<NodeList> = None;
        // Absorb type arguments into NewExpression when preceding expression is ExpressionWithTypeArguments
        let expression_kind = {
            let s = self.factory.store();
            s.node(expression).kind
        };
        if expression_kind == Kind::ExpressionWithTypeArguments {
            type_arguments = crate::parser::type_argument_list_of(self.factory.store(), expression);
            let inner = {
                let s = self.factory.store();
                s.node(expression)
                    .as_expression_with_type_arguments()
                    .expect("ExpressionWithTypeArguments data")
                    .expression
            };
            expression = inner;
        }
        if self.token == Kind::QuestionDotToken {
            let expression_text = tsc_scanner::utilities::get_text_of_node_from_source_text(
                self.source_text,
                self.factory.store(),
                expression,
                false, /*includeTrivia*/
            )
            .into_owned();
            self.parse_error_at_current_token(
                &tsc_diagnostics::INVALID_OPTIONAL_CHAIN_FROM_NEW_EXPRESSION_DID_YOU_MEAN_TO_CALL_0,
                &[expression_text],
            );
        }
        let mut argument_list: Option<NodeList> = None;
        if self.token == Kind::OpenParenToken {
            argument_list = self.parse_argument_list();
        }
        let result = {
            let __hoist_1_118 =
                self.factory
                    .new_new_expression(expression, type_arguments.clone(), argument_list);
            let __hoist_1_119 = pos;
            self.finish_node(__hoist_1_118, __hoist_1_119)
        };
        let result = self.check_js_syntax(result);
        self.unparse_expression_with_type_arguments(expression, type_arguments.as_ref(), result);
        result
    }

    /// Go: `func (p *Parser) parseKeywordExpression() *ast.Node`.
    pub(crate) fn parse_keyword_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let result = self.factory.new_keyword_expression(self.token);
        self.next_token();
        self.finish_node(result, pos)
    }

    /// Go: `func (p *Parser) parseLiteralExpression() *ast.Node`.
    pub(crate) fn parse_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        // Go: `text := p.scanner.TokenValue()` (raw bytes) — see the
        // `token_value_string` PORT note.
        let text = self.token_value_string();
        let token_flags = self.scanner.token_flags();

        let result: NodeId = match self.token {
            Kind::StringLiteral => self.factory.new_string_literal(&text, token_flags),
            Kind::NumericLiteral => self.factory.new_numeric_literal(&text, token_flags),
            Kind::BigIntLiteral => self.factory.new_big_int_literal(&text, token_flags),
            Kind::RegularExpressionLiteral => self
                .factory
                .new_regular_expression_literal(&text, token_flags),
            Kind::NoSubstitutionTemplateLiteral => self
                .factory
                .new_no_substitution_template_literal(&text, token_flags),
            _ => panic!("Unhandled case in parseLiteralExpression"),
        };
        self.next_token();
        self.finish_node(result, pos)
    }
}
