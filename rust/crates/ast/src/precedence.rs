// Ported from tsc/internal/ast/precedence.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   OperatorPrecedence (+consts)  → OperatorPrecedence (enum; Go's iota values
//                                   kept exactly — the parenthesizer compares
//                                   them with </>)
//   OperatorPrecedenceFlags        → define_flags! newtype (Go int, 2 bits)
//   TypePrecedence (+consts)       → TypePrecedence (enum; Go's iota values)
//   getOperator                    → get_operator (takes the store: Go reads
//                                   BinaryExpression.OperatorToken.Kind
//                                   through the pointer)
//   GetExpressionPrecedence        → get_expression_precedence
//   GetOperatorPrecedence          → get_operator_precedence (pure)
//   GetBinaryOperatorPrecedence    → get_binary_operator_precedence (pure)
//   GetLeftmostExpression          → get_leftmost_expression
//   GetTypeNodePrecedence          → get_type_node_precedence
//
// The grammar productions are documented on the Go constants; they are
// reproduced on the enum variants unchanged.

use crate::{Kind, Node, NodeId};
use crate::visitor::NodeStore;

/// Go: `type OperatorPrecedence int` + `const (...) = iota`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(i32)]
pub enum OperatorPrecedence {
    /// Expression:
    ///     AssignmentExpression
    ///     Expression `,` AssignmentExpression
    Comma = 0,
    /// NOTE: `Spread` is higher than `Comma` due to how it is parsed in
    /// |ElementList|.
    ///
    /// SpreadElement:
    ///     `...` AssignmentExpression
    Spread = 1,
    /// AssignmentExpression: YieldExpression
    ///
    /// YieldExpression:
    ///     `yield`
    ///     `yield` AssignmentExpression
    ///     `yield` `*` AssignmentExpression
    Yield = 2,
    /// AssignmentExpression: LeftHandSideExpression `=` AssignmentExpression
    /// AssignmentExpression: LeftHandSideExpression AssignmentOperator AssignmentExpression
    ///
    /// AssignmentOperator: one of
    ///     `*=` `/=` `%=` `+=` `-=` `<<=` `>>=` `>>>=` `&=` `^=` `|=` `**=`
    Assignment = 3,
    /// NOTE: `Conditional` is considered higher than `Assignment` here, but
    /// in reality they have the same precedence.
    ///
    /// AssignmentExpression: ConditionalExpression
    ///
    /// ConditionalExpression:
    ///     ShortCircuitExpression
    ///     ShortCircuitExpression `?` AssignmentExpression `:` AssignmentExpression
    Conditional = 4,
    /// LogicalORExpression:
    ///     LogicalANDExpression
    ///     LogicalORExpression `||` LogicalANDExpression
    LogicalOR = 5,
    /// LogicalANDExpression:
    ///     BitwiseORExpression
    ///     LogicalANDExprerssion `&&` BitwiseORExpression
    LogicalAND = 6,
    /// BitwiseORExpression:
    ///     BitwiseXORExpression
    ///     BitwiseORExpression `|` BitwiseXORExpression
    BitwiseOR = 7,
    /// BitwiseXORExpression:
    ///     BitwiseANDExpression
    ///     BitwiseXORExpression `^` BitwiseANDExpression
    BitwiseXOR = 8,
    /// BitwiseANDExpression:
    ///     EqualityExpression
    ///     BitwiseANDExpression `&` EqualityExpression
    BitwiseAND = 9,
    /// EqualityExpression:
    ///     RelationalExpression
    ///     EqualityExpression `==` RelationalExpression
    ///     EqualityExpression `!=` RelationalExpression
    ///     EqualityExpression `===` RelationalExpression
    ///     EqualityExpression `!==` RelationalExpression
    Equality = 10,
    /// RelationalExpression:
    ///     ShiftExpression
    ///     RelationalExpression `<` ShiftExpression
    ///     RelationalExpression `>` ShiftExpression
    ///     RelationalExpression `<=` ShiftExpression
    ///     RelationalExpression `>=` ShiftExpression
    ///     RelationalExpression `instanceof` ShiftExpression
    ///     RelationalExpression `in` ShiftExpression
    ///     [+TypeScript] RelationalExpression `as` Type
    Relational = 11,
    /// ShiftExpression:
    ///     AdditiveExpression
    ///     ShiftExpression `<<` AdditiveExpression
    ///     ShiftExpression `>>` AdditiveExpression
    ///     ShiftExpression `>>>` AdditiveExpression
    Shift = 12,
    /// AdditiveExpression:
    ///     MultiplicativeExpression
    ///     AdditiveExpression `+` MultiplicativeExpression
    ///     AdditiveExpression `-` MultiplicativeExpression
    Additive = 13,
    /// MultiplicativeExpression:
    ///     ExponentiationExpression
    ///     MultiplicativeExpression MultiplicativeOperator ExponentiationExpression
    ///
    /// MultiplicativeOperator: one of `*`, `/`, `%`
    Multiplicative = 14,
    /// ExponentiationExpression:
    ///     UnaryExpression
    ///     UpdateExpression `**` ExponentiationExpression
    Exponentiation = 15,
    /// UnaryExpression:
    ///     UpdateExpression
    ///     `delete` UnaryExpression
    ///     `void` UnaryExpression
    ///     `typeof` UnaryExpression
    ///     `+` UnaryExpression
    ///     `-` UnaryExpression
    ///     `~` UnaryExpression
    ///     `!` UnaryExpression
    ///     AwaitExpression
    ///
    /// UpdateExpression: // TODO: Do we need to investigate the precedence here?
    ///     `++` UnaryExpression
    ///     `--` UnaryExpression
    Unary = 16,
    /// UpdateExpression:
    ///     LeftHandSideExpression
    ///     LeftHandSideExpression `++`
    ///     LeftHandSideExpression `--`
    Update = 17,
    /// LeftHandSideExpression:
    ///     NewExpression
    ///
    /// NewExpression:
    ///     MemberExpression
    ///     `new` NewExpression
    LeftHandSide = 18,
    /// LeftHandSideExpression:
    ///     OptionalExpression
    ///
    /// OptionalExpression:
    ///     MemberExpression OptionalChain
    ///     CallExpression OptionalChain
    ///     OptionalExpression OptionalChain
    OptionalChain = 19,
    /// LeftHandSideExpression:
    ///     CallExpression
    ///
    /// CallExpression:
    ///     CoverCallExpressionAndAsyncArrowHead
    ///     SuperCall
    ///     ImportCall
    ///     CallExpression Arguments
    ///     CallExpression `[` Expression `]`
    ///     CallExpression `.` IdentifierName
    ///     CallExpression TemplateLiteral
    ///
    /// MemberExpression:
    ///     PrimaryExpression
    ///     MemberExpression `[` Expression `]`
    ///     MemberExpression `.` IdentifierName
    ///     MemberExpression TemplateLiteral
    ///     SuperProperty
    ///     MetaProperty
    ///     `new` MemberExpression Arguments
    Member = 20,
    /// TODO: JSXElement?
    ///
    /// PrimaryExpression:
    ///     `this`
    ///     IdentifierReference
    ///     Literal
    ///     ArrayLiteral
    ///     ObjectLiteral
    ///     FunctionExpression
    ///     ClassExpression
    ///     GeneratorExpression
    ///     AsyncFunctionExpression
    ///     AsyncGeneratorExpression
    ///     RegularExpressionLiteral
    ///     TemplateLiteral
    Primary = 21,
    /// PrimaryExpression:
    ///     CoverParenthesizedExpressionAndArrowParameterList
    Parentheses = 22,
    /// Go: `OperatorPrecedenceInvalid OperatorPrecedence = -1` — -1 is lower
    /// than all other precedences. Returning it will cause binary expression
    /// parsing to stop.
    Invalid = -1,
}

impl OperatorPrecedence {
    /// Go: `OperatorPrecedenceLowest = OperatorPrecedenceComma`.
    pub const LOWEST: OperatorPrecedence = OperatorPrecedence::Comma;
    /// Go: `OperatorPrecedenceHighest = OperatorPrecedenceParentheses`.
    pub const HIGHEST: OperatorPrecedence = OperatorPrecedence::Parentheses;
    /// Go: `OperatorPrecedenceDisallowComma = OperatorPrecedenceYield`.
    pub const DISALLOW_COMMA: OperatorPrecedence = OperatorPrecedence::Yield;
    /// ShortCircuitExpression:
    ///     LogicalORExpression
    ///     CoalesceExpression
    ///
    /// CoalesceExpression:
    ///     CoalesceExpressionHead `??` BitwiseORExpression
    ///
    /// CoalesceExpressionHead:
    ///     CoalesceExpression
    ///     BitwiseORExpression
    pub const COALESCE: OperatorPrecedence = OperatorPrecedence::LogicalOR;
}

/// Go: `type OperatorPrecedenceFlags int` + consts.
define_flags!(OperatorPrecedenceFlags, i32);

impl OperatorPrecedenceFlags {
    /// Go: `OperatorPrecedenceFlagsNone = 0`.
    pub const NEW_WITHOUT_ARGUMENTS: Self = Self(1 << 0);
    pub const OPTIONAL_CHAIN: Self = Self(1 << 1);
}

/// Go: `type TypePrecedence int32` + `const (...) = iota` (grammar comments
/// reproduced in the Go source and summarized here).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(i32)]
pub enum TypePrecedence {
    /// Conditional precedence (lowest) — `UnionTypeNode extends Type ? Type : Type`.
    Conditional = 0,
    /// JSDoc precedence (optional and variadic types) — `...`? Type `=`?
    JSDoc = 1,
    /// Function precedence — FunctionTypeNode / ConstructorTypeNode.
    Function = 2,
    /// Union precedence — `|`? UnionTypeNoBar.
    Union = 3,
    /// Intersection precedence — `&`? IntersectionTypeNoAmpersand.
    Intersection = 4,
    /// TypeOperatorNode precedence — `keyof`/`unique`/`readonly`/`infer`.
    TypeOperator = 5,
    /// Postfix precedence — OptionalTypeNode / ArrayTypeNode / IndexedAccessTypeNode.
    Postfix = 6,
    /// NonArray precedence (highest) — keyword types, literals, type
    /// references, parenthesized types, mapped/tuple/template types, etc.
    NonArray = 7,
}

impl TypePrecedence {
    /// Go: `TypePrecedenceLowest = TypePrecedenceConditional`.
    pub const LOWEST: TypePrecedence = TypePrecedence::Conditional;
    /// Go: `TypePrecedenceHighest = TypePrecedenceNonArray`.
    pub const HIGHEST: TypePrecedence = TypePrecedence::NonArray;
}

/// Go: `func getOperator(expression *Expression) Kind`.
pub fn get_operator(store: &dyn NodeStore, expression: NodeId) -> Kind {
    let n = store.node(expression);
    match n.kind {
        Kind::BinaryExpression => {
            store.node(n.as_binary_expression().expect("BinaryExpression data").operator_token).kind
        }
        Kind::PrefixUnaryExpression => n.as_prefix_unary_expression().expect("data").operator,
        Kind::PostfixUnaryExpression => n.as_postfix_unary_expression().expect("data").operator,
        _ => n.kind,
    }
}

/// Go: `func GetExpressionPrecedence(expression *Expression) OperatorPrecedence`
/// — gets the precedence of an expression.
pub fn get_expression_precedence(store: &dyn NodeStore, expression: NodeId) -> OperatorPrecedence {
    let n = store.node(expression);
    let operator = get_operator(store, expression);
    let mut flags = OperatorPrecedenceFlags::NONE;
    if n.kind == Kind::NewExpression
        && n.as_new_expression()
            .expect("NewExpression data")
            .arguments
            .is_none()
    {
        flags = OperatorPrecedenceFlags::NEW_WITHOUT_ARGUMENTS;
    } else if crate::utilities::is_optional_chain(n) {
        flags = OperatorPrecedenceFlags::OPTIONAL_CHAIN;
    }
    get_operator_precedence(n.kind, operator, flags)
}

/// Go: `func GetOperatorPrecedence(nodeKind Kind, operatorKind Kind, flags OperatorPrecedenceFlags) OperatorPrecedence`
/// — gets the precedence of an operator.
pub fn get_operator_precedence(
    node_kind: Kind,
    operator_kind: Kind,
    flags: OperatorPrecedenceFlags,
) -> OperatorPrecedence {
    match node_kind {
        Kind::SpreadElement => OperatorPrecedence::Spread,
        Kind::YieldExpression => OperatorPrecedence::Yield,
        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        Kind::ArrowFunction => OperatorPrecedence::Assignment,
        Kind::ConditionalExpression => OperatorPrecedence::Conditional,
        Kind::BinaryExpression => match operator_kind {
            Kind::CommaToken => OperatorPrecedence::Comma,
            Kind::EqualsToken
            | Kind::PlusEqualsToken
            | Kind::MinusEqualsToken
            | Kind::AsteriskAsteriskEqualsToken
            | Kind::AsteriskEqualsToken
            | Kind::SlashEqualsToken
            | Kind::PercentEqualsToken
            | Kind::LessThanLessThanEqualsToken
            | Kind::GreaterThanGreaterThanEqualsToken
            | Kind::GreaterThanGreaterThanGreaterThanEqualsToken
            | Kind::AmpersandEqualsToken
            | Kind::CaretEqualsToken
            | Kind::BarEqualsToken
            | Kind::BarBarEqualsToken
            | Kind::AmpersandAmpersandEqualsToken
            | Kind::QuestionQuestionEqualsToken => OperatorPrecedence::Assignment,
            _ => get_binary_operator_precedence(operator_kind),
        },
        // TODO: Should prefix `++` and `--` be moved to the `Update` precedence?
        Kind::TypeAssertionExpression
        | Kind::NonNullExpression
        | Kind::PrefixUnaryExpression
        | Kind::TypeOfExpression
        | Kind::VoidExpression
        | Kind::DeleteExpression
        | Kind::AwaitExpression => OperatorPrecedence::Unary,
        Kind::PostfixUnaryExpression => OperatorPrecedence::Update,
        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        Kind::PropertyAccessExpression | Kind::ElementAccessExpression => {
            if flags.intersects(OperatorPrecedenceFlags::OPTIONAL_CHAIN) {
                OperatorPrecedence::OptionalChain
            } else {
                OperatorPrecedence::Member
            }
        }
        Kind::CallExpression => {
            if flags.intersects(OperatorPrecedenceFlags::OPTIONAL_CHAIN) {
                OperatorPrecedence::OptionalChain
            } else {
                OperatorPrecedence::Member
            }
        }
        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        Kind::NewExpression => {
            if flags.intersects(OperatorPrecedenceFlags::NEW_WITHOUT_ARGUMENTS) {
                OperatorPrecedence::LeftHandSide
            } else {
                OperatorPrecedence::Member
            }
        }
        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        Kind::TaggedTemplateExpression | Kind::MetaProperty | Kind::ExpressionWithTypeArguments => {
            OperatorPrecedence::Member
        }
        Kind::AsExpression | Kind::SatisfiesExpression => OperatorPrecedence::Relational,
        Kind::ThisKeyword
        | Kind::SuperKeyword
        | Kind::ImportKeyword
        | Kind::Identifier
        | Kind::PrivateIdentifier
        | Kind::NullKeyword
        | Kind::TrueKeyword
        | Kind::FalseKeyword
        | Kind::NumericLiteral
        | Kind::BigIntLiteral
        | Kind::StringLiteral
        | Kind::ArrayLiteralExpression
        | Kind::ObjectLiteralExpression
        | Kind::FunctionExpression
        | Kind::ClassExpression
        | Kind::RegularExpressionLiteral
        | Kind::NoSubstitutionTemplateLiteral
        | Kind::TemplateExpression
        | Kind::OmittedExpression
        | Kind::JsxElement
        | Kind::JsxSelfClosingElement
        | Kind::JsxFragment
        | Kind::MissingDeclaration => OperatorPrecedence::Primary,
        // !!! By necessity, this differs from the old compiler to support emit. consider backporting
        Kind::ParenthesizedExpression => OperatorPrecedence::Parentheses,
        _ => OperatorPrecedence::Invalid,
    }
}

/// Go: `func GetBinaryOperatorPrecedence(operatorKind Kind) OperatorPrecedence`
/// — gets the precedence of a binary operator.
pub fn get_binary_operator_precedence(operator_kind: Kind) -> OperatorPrecedence {
    match operator_kind {
        Kind::QuestionQuestionToken => OperatorPrecedence::COALESCE,
        Kind::BarBarToken => OperatorPrecedence::LogicalOR,
        Kind::AmpersandAmpersandToken => OperatorPrecedence::LogicalAND,
        Kind::BarToken => OperatorPrecedence::BitwiseOR,
        Kind::CaretToken => OperatorPrecedence::BitwiseXOR,
        Kind::AmpersandToken => OperatorPrecedence::BitwiseAND,
        Kind::EqualsEqualsToken
        | Kind::ExclamationEqualsToken
        | Kind::EqualsEqualsEqualsToken
        | Kind::ExclamationEqualsEqualsToken => OperatorPrecedence::Equality,
        Kind::LessThanToken
        | Kind::GreaterThanToken
        | Kind::LessThanEqualsToken
        | Kind::GreaterThanEqualsToken
        | Kind::InstanceOfKeyword
        | Kind::InKeyword
        | Kind::AsKeyword
        | Kind::SatisfiesKeyword => OperatorPrecedence::Relational,
        Kind::LessThanLessThanToken
        | Kind::GreaterThanGreaterThanToken
        | Kind::GreaterThanGreaterThanGreaterThanToken => OperatorPrecedence::Shift,
        Kind::PlusToken | Kind::MinusToken => OperatorPrecedence::Additive,
        Kind::AsteriskToken | Kind::SlashToken | Kind::PercentToken => OperatorPrecedence::Multiplicative,
        Kind::AsteriskAsteriskToken => OperatorPrecedence::Exponentiation,
        // -1 is lower than all other precedences. Returning it will cause
        // binary expression parsing to stop.
        _ => OperatorPrecedence::Invalid,
    }
}

/// Go: `func GetLeftmostExpression(node *Expression, stopAtCallExpressions bool) *Expression`
/// — gets the leftmost expression of an expression, e.g. `a` in `a.b`,
/// `a[b]`, `a++`, `a+b`, `a?b:c`, `a as B`, etc.
///
/// PORT: Go routes the shared `.Expression()` accessor through the nodeData
/// interface; the port dispatches the same kind set directly.
pub fn get_leftmost_expression(
    store: &dyn NodeStore,
    start: NodeId,
    stop_at_call_expressions: bool,
) -> NodeId {
    let mut node = start;
    loop {
        let n = store.node(node);
        match n.kind {
            Kind::PostfixUnaryExpression => {
                node = n.as_postfix_unary_expression().expect("data").operand;
            }
            Kind::BinaryExpression => {
                node = n.as_binary_expression().expect("data").left;
            }
            Kind::ConditionalExpression => {
                node = n.as_conditional_expression().expect("data").condition;
            }
            Kind::TaggedTemplateExpression => {
                node = n.as_tagged_template_expression().expect("data").tag;
            }
            Kind::CallExpression if stop_at_call_expressions => return node,
            Kind::CallExpression
            | Kind::AsExpression
            | Kind::ElementAccessExpression
            | Kind::PropertyAccessExpression
            | Kind::NonNullExpression
            | Kind::PartiallyEmittedExpression
            | Kind::SatisfiesExpression => {
                node = node_expression(n);
            }
            _ => return node,
        }
    }
}

/// Go: the shared `Expression()` accessor of the expression bases
/// (LeftHandSideExpressionBase.Expression etc.).
fn node_expression(n: &Node) -> NodeId {
    match &n.data {
        crate::NodeData::CallExpression(d) => d.expression,
        crate::NodeData::AsExpression(d) => d.expression,
        crate::NodeData::ElementAccessExpression(d) => d.expression,
        crate::NodeData::PropertyAccessExpression(d) => d.expression,
        crate::NodeData::NonNullExpression(d) => d.expression,
        crate::NodeData::PartiallyEmittedExpression(d) => d.expression,
        crate::NodeData::SatisfiesExpression(d) => d.expression,
        _ => panic!("kind {:?} has no Expression()", n.kind),
    }
}

/// Go: `func GetTypeNodePrecedence(n *TypeNode) TypePrecedence` — gets the
/// precedence of a TypeNode.
pub fn get_type_node_precedence(store: &dyn NodeStore, n: NodeId) -> TypePrecedence {
    let node = store.node(n);
    match node.kind {
        Kind::ConditionalType => TypePrecedence::Conditional,
        Kind::JSDocOptionalType | Kind::JSDocVariadicType => TypePrecedence::JSDoc,
        Kind::FunctionType | Kind::ConstructorType => TypePrecedence::Function,
        Kind::UnionType => TypePrecedence::Union,
        Kind::IntersectionType => TypePrecedence::Intersection,
        Kind::TypeOperator => TypePrecedence::TypeOperator,
        Kind::InferType => {
            let type_parameter = node.as_infer_type_node().expect("InferTypeNode data").type_parameter;
            let constraint_is_some = store
                .node(type_parameter)
                .as_type_parameter_declaration()
                .expect("TypeParameterDeclaration data")
                .constraint
                .is_some();
            // `infer T extends U` must be treated as FunctionTypeNode
            // precedence as the `extends` clause eagerly consumes TypeNode
            if constraint_is_some {
                TypePrecedence::Function
            } else {
                TypePrecedence::TypeOperator
            }
        }
        Kind::IndexedAccessType | Kind::ArrayType | Kind::OptionalType => TypePrecedence::Postfix,
        Kind::TypeQuery => {
            // TypeQueryNode is actually a NonArrayType, but we treat it as
            // TypeOperatorNode precedence so that it is parenthesized when
            // used in a PostfixType context (e.g., `(typeof C)[]` instead of
            // `typeof C[]`)
            TypePrecedence::TypeOperator
        }
        Kind::AnyKeyword
        | Kind::UnknownKeyword
        | Kind::StringKeyword
        | Kind::NumberKeyword
        | Kind::BigIntKeyword
        | Kind::SymbolKeyword
        | Kind::BooleanKeyword
        | Kind::UndefinedKeyword
        | Kind::NeverKeyword
        | Kind::ObjectKeyword
        | Kind::IntrinsicKeyword
        | Kind::VoidKeyword
        | Kind::JSDocAllType
        | Kind::JSDocNullableType
        | Kind::JSDocNonNullableType
        | Kind::LiteralType
        | Kind::TypePredicate
        | Kind::TypeReference
        | Kind::TypeLiteral
        | Kind::TupleType
        | Kind::RestType
        | Kind::ParenthesizedType
        | Kind::ThisType
        | Kind::MappedType
        | Kind::NamedTupleMember
        | Kind::TemplateLiteralType
        | Kind::ImportType
        // These occur in pseudo-types like `f<T>.C`, where `f` is a generic
        // function and `C` is a local type
        | Kind::PropertyAccessExpression
        | Kind::ExpressionWithTypeArguments => TypePrecedence::NonArray,
        _ => panic!("unhandled TypeNode: {:?}", node.kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: the OperatorPrecedence iota order — the parenthesizer compares
    /// precedences with </>, so the discriminants must match Go exactly.
    #[test]
    fn operator_precedence_ordinals_mirror_go() {
        assert_eq!(OperatorPrecedence::Comma as i32, 0);
        assert_eq!(OperatorPrecedence::Spread as i32, 1);
        assert_eq!(OperatorPrecedence::Yield as i32, 2);
        assert_eq!(OperatorPrecedence::Assignment as i32, 3);
        assert_eq!(OperatorPrecedence::Conditional as i32, 4);
        assert_eq!(OperatorPrecedence::LogicalOR as i32, 5);
        assert_eq!(OperatorPrecedence::LogicalAND as i32, 6);
        assert_eq!(OperatorPrecedence::BitwiseOR as i32, 7);
        assert_eq!(OperatorPrecedence::BitwiseXOR as i32, 8);
        assert_eq!(OperatorPrecedence::BitwiseAND as i32, 9);
        assert_eq!(OperatorPrecedence::Equality as i32, 10);
        assert_eq!(OperatorPrecedence::Relational as i32, 11);
        assert_eq!(OperatorPrecedence::Shift as i32, 12);
        assert_eq!(OperatorPrecedence::Additive as i32, 13);
        assert_eq!(OperatorPrecedence::Multiplicative as i32, 14);
        assert_eq!(OperatorPrecedence::Exponentiation as i32, 15);
        assert_eq!(OperatorPrecedence::Unary as i32, 16);
        assert_eq!(OperatorPrecedence::Update as i32, 17);
        assert_eq!(OperatorPrecedence::LeftHandSide as i32, 18);
        assert_eq!(OperatorPrecedence::OptionalChain as i32, 19);
        assert_eq!(OperatorPrecedence::Member as i32, 20);
        assert_eq!(OperatorPrecedence::Primary as i32, 21);
        assert_eq!(OperatorPrecedence::Parentheses as i32, 22);
        assert_eq!(OperatorPrecedence::Invalid as i32, -1);
        assert_eq!(OperatorPrecedence::LOWEST, OperatorPrecedence::Comma);
        assert_eq!(OperatorPrecedence::HIGHEST, OperatorPrecedence::Parentheses);
        assert_eq!(OperatorPrecedence::DISALLOW_COMMA, OperatorPrecedence::Yield);
        assert_eq!(OperatorPrecedence::COALESCE, OperatorPrecedence::LogicalOR);
    }

    /// Go: TypePrecedence iota order.
    #[test]
    fn type_precedence_ordinals_mirror_go() {
        assert_eq!(TypePrecedence::Conditional as i32, 0);
        assert_eq!(TypePrecedence::JSDoc as i32, 1);
        assert_eq!(TypePrecedence::Function as i32, 2);
        assert_eq!(TypePrecedence::Union as i32, 3);
        assert_eq!(TypePrecedence::Intersection as i32, 4);
        assert_eq!(TypePrecedence::TypeOperator as i32, 5);
        assert_eq!(TypePrecedence::Postfix as i32, 6);
        assert_eq!(TypePrecedence::NonArray as i32, 7);
        assert_eq!(TypePrecedence::LOWEST, TypePrecedence::Conditional);
        assert_eq!(TypePrecedence::HIGHEST, TypePrecedence::NonArray);
    }

    /// Spot-checks of GetBinaryOperatorPrecedence against the Go table.
    #[test]
    fn binary_operator_precedence_table() {
        use OperatorPrecedence as P;
        assert_eq!(get_binary_operator_precedence(Kind::QuestionQuestionToken), P::COALESCE);
        assert_eq!(get_binary_operator_precedence(Kind::BarBarToken), P::LogicalOR);
        assert_eq!(get_binary_operator_precedence(Kind::AmpersandAmpersandToken), P::LogicalAND);
        assert_eq!(get_binary_operator_precedence(Kind::BarToken), P::BitwiseOR);
        assert_eq!(get_binary_operator_precedence(Kind::CaretToken), P::BitwiseXOR);
        assert_eq!(get_binary_operator_precedence(Kind::AmpersandToken), P::BitwiseAND);
        assert_eq!(get_binary_operator_precedence(Kind::EqualsEqualsToken), P::Equality);
        assert_eq!(get_binary_operator_precedence(Kind::InKeyword), P::Relational);
        assert_eq!(get_binary_operator_precedence(Kind::AsKeyword), P::Relational);
        assert_eq!(get_binary_operator_precedence(Kind::LessThanLessThanToken), P::Shift);
        assert_eq!(get_binary_operator_precedence(Kind::PlusToken), P::Additive);
        assert_eq!(get_binary_operator_precedence(Kind::SlashToken), P::Multiplicative);
        assert_eq!(get_binary_operator_precedence(Kind::AsteriskAsteriskToken), P::Exponentiation);
        assert_eq!(get_binary_operator_precedence(Kind::SemicolonToken), P::Invalid);
    }

    /// Spot-checks of GetOperatorPrecedence's kind dispatch (flags paths).
    #[test]
    fn operator_precedence_kind_dispatch() {
        use OperatorPrecedence as P;
        assert_eq!(get_operator_precedence(Kind::SpreadElement, Kind::DotDotDotToken, OperatorPrecedenceFlags::NONE), P::Spread);
        assert_eq!(get_operator_precedence(Kind::ArrowFunction, Kind::EqualsGreaterThanToken, OperatorPrecedenceFlags::NONE), P::Assignment);
        assert_eq!(
            get_operator_precedence(Kind::NewExpression, Kind::NewKeyword, OperatorPrecedenceFlags::NEW_WITHOUT_ARGUMENTS),
            P::LeftHandSide
        );
        assert_eq!(
            get_operator_precedence(Kind::NewExpression, Kind::NewKeyword, OperatorPrecedenceFlags::NONE),
            P::Member
        );
        assert_eq!(
            get_operator_precedence(Kind::PropertyAccessExpression, Kind::DotToken, OperatorPrecedenceFlags::OPTIONAL_CHAIN),
            P::OptionalChain
        );
        assert_eq!(
            get_operator_precedence(Kind::PropertyAccessExpression, Kind::DotToken, OperatorPrecedenceFlags::NONE),
            P::Member
        );
        assert_eq!(get_operator_precedence(Kind::Identifier, Kind::Identifier, OperatorPrecedenceFlags::NONE), P::Primary);
        assert_eq!(get_operator_precedence(Kind::ParenthesizedExpression, Kind::OpenParenToken, OperatorPrecedenceFlags::NONE), P::Parentheses);
        assert_eq!(get_operator_precedence(Kind::InterfaceDeclaration, Kind::InterfaceKeyword, OperatorPrecedenceFlags::NONE), P::Invalid);
    }

    /// GetTypeNodePrecedence: the pure-kind arms (the InferType constraint
    /// path is exercised once the parser can build those nodes).
    #[test]
    fn type_node_precedence_kind_dispatch() {
        use std::cell::Cell;

        use tsc_core::text::TextRange;

        use crate::source_file::SourceFile;

        let cases: &[(Kind, TypePrecedence)] = &[
            (Kind::ConditionalType, TypePrecedence::Conditional),
            (Kind::UnionType, TypePrecedence::Union),
            (Kind::IntersectionType, TypePrecedence::Intersection),
            (Kind::ArrayType, TypePrecedence::Postfix),
            (Kind::OptionalType, TypePrecedence::Postfix),
            (Kind::TypeQuery, TypePrecedence::TypeOperator),
            (Kind::TypeOperator, TypePrecedence::TypeOperator),
            (Kind::TypeReference, TypePrecedence::NonArray),
            (Kind::FunctionType, TypePrecedence::Function),
            (Kind::ConstructorType, TypePrecedence::Function),
            (Kind::JSDocVariadicType, TypePrecedence::JSDoc),
            (Kind::ParenthesizedType, TypePrecedence::NonArray),
        ];
        let mut file = SourceFile::new(0, Default::default(), "", None, None);
        for &(kind, expected) in cases {
            let id = NodeStore::alloc(
                &mut file,
                Node {
                    kind,
                    flags: crate::NodeFlags::NONE,
                    loc: TextRange::undefined(),
                    id: Cell::new(0),
                    parent: Cell::new(NodeId::NONE),
                    data: crate::NodeData::Token(Box::new(crate::ast_generated::Token {})),
                },
            );
            let store: &dyn NodeStore = &file;
            assert_eq!(get_type_node_precedence(store, id), expected, "{kind:?}");
        }
    }
}
