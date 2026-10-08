// Ported from tsc/internal/ast/functionflags.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Exact bit values from Go (`type FunctionFlags uint32`), plus
// `GetFunctionFlags` — the one function defined in that file.

use crate::modifierflags::has_syntactic_modifier;
use crate::{Kind, ModifierFlags, NodeId};
use crate::visitor::NodeStore;

define_flags!(FunctionFlags, u32);

impl FunctionFlags {
    pub const NORMAL: FunctionFlags = FunctionFlags(0);
    pub const GENERATOR: FunctionFlags = FunctionFlags(1 << 0);
    pub const ASYNC: FunctionFlags = FunctionFlags(1 << 1);
    pub const INVALID: FunctionFlags = FunctionFlags(1 << 2);
    pub const ASYNC_GENERATOR: FunctionFlags = FunctionFlags(Self::ASYNC.0 | Self::GENERATOR.0);
}

/// Go: `func GetFunctionFlags(node *Node) FunctionFlags`.
///
/// PORT: Go reads the shared `*BodyBase` embed (`node.BodyData()`), which the
/// port flattens per-struct; the kinds carrying a body are matched directly.
/// The async check mirrors the Go switch (FunctionDeclaration/
/// FunctionExpression/MethodDeclaration check the asterisk token and fall
/// through to ArrowFunction's async check; the other body-bearing kinds —
/// ConstructorDeclaration, Get/SetAccessor, ModuleDeclaration — only take the
/// missing-body INVALID bit).
pub fn get_function_flags(store: &dyn NodeStore, node: NodeId) -> FunctionFlags {
    let n = store.node(node);
    let (asterisk, has_body, has_async) = match n.kind {
        Kind::FunctionDeclaration => {
            let d = n.as_function_declaration().expect("FunctionDeclaration");
            (d.asterisk_token.is_some(), d.body.is_some(), has_syntactic_modifier(n, ModifierFlags::ASYNC))
        }
        Kind::FunctionExpression => {
            let d = n.as_function_expression().expect("FunctionExpression");
            (d.asterisk_token.is_some(), d.body.is_some(), has_syntactic_modifier(n, ModifierFlags::ASYNC))
        }
        Kind::MethodDeclaration => {
            let d = n.as_method_declaration().expect("MethodDeclaration");
            (d.asterisk_token.is_some(), d.body.is_some(), has_syntactic_modifier(n, ModifierFlags::ASYNC))
        }
        Kind::ArrowFunction => {
            let d = n.as_arrow_function().expect("ArrowFunction");
            (false, d.body.is_some(), has_syntactic_modifier(n, ModifierFlags::ASYNC))
        }
        Kind::Constructor => {
            let d = n.as_constructor_declaration().expect("ConstructorDeclaration");
            (false, d.body.is_some(), false)
        }
        Kind::GetAccessor => {
            let d = n.as_get_accessor_declaration().expect("GetAccessorDeclaration");
            (false, d.body.is_some(), false)
        }
        Kind::SetAccessor => {
            let d = n.as_set_accessor_declaration().expect("SetAccessorDeclaration");
            (false, d.body.is_some(), false)
        }
        Kind::ModuleDeclaration => {
            let d = n.as_module_declaration().expect("ModuleDeclaration");
            (false, d.body.is_some(), false)
        }
        // Go: `data := node.BodyData(); if data == nil { return FunctionFlagsInvalid }`
        _ => return FunctionFlags::INVALID,
    };
    let mut flags = FunctionFlags::NORMAL;
    if asterisk {
        flags |= FunctionFlags::GENERATOR;
    }
    if has_async {
        flags |= FunctionFlags::ASYNC;
    }
    // Go: `if data.Body == nil { flags |= FunctionFlagsInvalid }`
    if !has_body {
        flags |= FunctionFlags::INVALID;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_flags_bit_values_mirror_go() {
        assert_eq!(FunctionFlags::NORMAL, FunctionFlags(0));
        assert_eq!(FunctionFlags::GENERATOR, FunctionFlags(1 << 0));
        assert_eq!(FunctionFlags::ASYNC, FunctionFlags(1 << 1));
        assert_eq!(FunctionFlags::INVALID, FunctionFlags(1 << 2));
        assert_eq!(FunctionFlags::ASYNC_GENERATOR, FunctionFlags::ASYNC | FunctionFlags::GENERATOR);
    }
}
