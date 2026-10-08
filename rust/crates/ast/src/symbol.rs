// Ported from tsc/internal/ast/symbol.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   Symbol struct    → Symbol (fields per SPEC §5.2: *Symbol back-references
//                      become SymbolId handles into the program arena; the
//                      lazy id stays the `id: Cell<u64>` field)
//   SymbolTable      → OrderedMap<String, SymbolId> (Go `map[string]*Symbol`;
//                      SPEC §5.2 mandates insertion order — plain HashMap is
//                      forbidden wherever order can reach output). PORT: keys
//                      are `String` until the `Atom` interner lands (SPEC §12
//                      open question 2).
//   InternalSymbolName* consts → INTERNAL_SYMBOL_NAME_* (`&'static str`s; the
//                      "\xFE" sentinel is embedded literally)

use std::cell::Cell;

use tsc_collections::OrderedMap;

use crate::{NodeId, SymbolFlags, SymbolId};
use crate::{CheckFlags, ModifierFlags};
use crate::utilities::is_ambient_module_symbol_name;

/// SPEC §5.2: the symbol arena slot (one `Vec<Symbol>` lives on `Program`).
#[derive(Debug, Default)]
pub struct Symbol {
    pub flags: SymbolFlags,
    /// Non-zero only in transient symbols created by Checker.
    pub check_flags: CheckFlags,
    /// PORT: `Atom` (interned string, SPEC §5.2) is not yet ported; plain
    /// `String` until the interner scope decision (SPEC §12.2) lands.
    pub name: String,
    pub declarations: Vec<NodeId>,
    pub value_declaration: Option<NodeId>,
    pub members: SymbolTable,
    pub exports: SymbolTable,
    /// Go: `id atomic.Uint64` — lazily assigned (0 = unassigned).
    pub id: Cell<u64>,
    pub parent: Option<SymbolId>,
    pub export_symbol: Option<SymbolId>,
}

/// Go: `type SymbolTable map[string]*Symbol` — insertion-ordered per SPEC §5.2
/// (iteration order is observable in baselines, so a plain `HashMap` is
/// forbidden anywhere output order flows to diagnostics/emit).
pub type SymbolTable = OrderedMap<String, SymbolId>;

/// Go: `const InternalSymbolNamePrefix = "\xFE"` — invalid UTF-8 sequence that
/// will never occur as an IdentifierName. (`\u{FE}` is the same single byte
/// in Rust strings.)
pub const INTERNAL_SYMBOL_NAME_PREFIX: &str = "\u{FE}";

pub const INTERNAL_SYMBOL_NAME_CALL: &str = "\u{FE}call"; // Call signatures
pub const INTERNAL_SYMBOL_NAME_CONSTRUCTOR: &str = "\u{FE}constructor"; // Constructor implementations
pub const INTERNAL_SYMBOL_NAME_NEW: &str = "\u{FE}new"; // Constructor signatures
pub const INTERNAL_SYMBOL_NAME_INDEX: &str = "\u{FE}index"; // Index signatures
pub const INTERNAL_SYMBOL_NAME_EXPORT_STAR: &str = "\u{FE}export"; // Module export * declarations
pub const INTERNAL_SYMBOL_NAME_GLOBAL: &str = "\u{FE}global"; // Global self-reference
pub const INTERNAL_SYMBOL_NAME_MISSING: &str = "\u{FE}missing"; // Indicates missing symbol
pub const INTERNAL_SYMBOL_NAME_TYPE: &str = "\u{FE}type"; // Anonymous type literal symbol
pub const INTERNAL_SYMBOL_NAME_OBJECT: &str = "\u{FE}object"; // Anonymous object literal declaration
/// Anonymous JSX attributes object literal declaration
pub const INTERNAL_SYMBOL_NAME_JSX_ATTRIBUTES: &str = "\u{FE}jsxAttributes";
pub const INTERNAL_SYMBOL_NAME_CLASS: &str = "\u{FE}class"; // Unnamed class expression
pub const INTERNAL_SYMBOL_NAME_FUNCTION: &str = "\u{FE}function"; // Unnamed function expression
/// Computed property name declaration with dynamic name
pub const INTERNAL_SYMBOL_NAME_COMPUTED: &str = "\u{FE}computed";
pub const INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION: &str = "\u{FE}assignment"; // Assignment declarations
/// Instantiation expressions
pub const INTERNAL_SYMBOL_NAME_INSTANTIATION_EXPRESSION: &str = "\u{FE}instantiationExpression";
pub const INTERNAL_SYMBOL_NAME_IMPORT_ATTRIBUTES: &str = "\u{FE}importAttributes";
pub const INTERNAL_SYMBOL_NAME_EXPORT_EQUALS: &str = "export="; // Export assignment symbol
/// Default export symbol (technically not wholly internal, but included here for usability)
pub const INTERNAL_SYMBOL_NAME_DEFAULT: &str = "default";
pub const INTERNAL_SYMBOL_NAME_THIS: &str = "this";
pub const INTERNAL_SYMBOL_NAME_MODULE_EXPORTS: &str = "module.exports";

/// Go: `func (s *Symbol) IsExternalModule() bool`.
pub fn symbol_is_external_module(s: &Symbol) -> bool {
    s.flags.intersects(SymbolFlags::MODULE) && is_ambient_module_symbol_name(&s.name)
}

/// Go: `func (s *Symbol) IsStatic() bool`.
pub fn symbol_is_static(store: &dyn crate::NodeStore, s: &Symbol) -> bool {
    match s.value_declaration {
        Some(decl) => {
            store.node(decl).modifier_flags().intersects(ModifierFlags::STATIC)
        }
        None => false,
    }
}

/// Go: `func (s *Symbol) CombinedLocalAndExportSymbolFlags() SymbolFlags` —
/// see the comment on `declareModuleMember` in binder.go.
///
/// PORT: Go reads `s.ExportSymbol.Flags` through the pointer; the port takes
/// the program's symbol slice to resolve the `SymbolId` handle.
pub fn combined_local_and_export_symbol_flags(symbols: &[Symbol], s: &Symbol) -> SymbolFlags {
    match s.export_symbol {
        Some(export) => s.flags | symbols[export.0 as usize].flags,
        None => s.flags,
    }
}

/// Go: `func GetSourceFileOfSymbol(symbol *Symbol) *SourceFile` — returns the
/// owning file of a published binder symbol, or None for a non-file-owned
/// symbol, even if it borrows declarations from a file. Ownership recovery
/// walks only the first declaration's AST parents.
///
/// PORT: Go follows `symbol.Parent` through the `*Symbol` pointer; the port
/// takes the program's symbol slice (`Vec<Symbol>` on `Program`, SPEC §5.2)
/// to resolve the `SymbolId`.
pub fn get_source_file_of_symbol<'a>(
    store: &'a dyn crate::NodeStore,
    symbols: &'a [Symbol],
    s: &'a Symbol,
) -> Option<&'a crate::SourceFile> {
    if s.flags.intersects(SymbolFlags::TRANSIENT) {
        return None;
    }
    let mut symbol = s;
    if symbol.declarations.is_empty() {
        // A class's implicit prototype has no declaration of its own.
        debug_assert!(
            symbol.flags.intersects(SymbolFlags::PROTOTYPE),
            "File-bound symbol has no declarations"
        );
        let parent = &symbols[symbol
            .parent
            .expect("Prototype has no declaring class")
            .0 as usize];
        debug_assert!(
            parent.flags.intersects(SymbolFlags::CLASS),
            "Prototype has no declaring class"
        );
        symbol = parent;
        debug_assert!(
            !symbol.flags.intersects(SymbolFlags::TRANSIENT),
            "Prototype parent is not file-bound"
        );
        debug_assert!(!symbol.declarations.is_empty(), "Prototype parent has no declarations");
    }
    let file = crate::utilities::get_source_file_of_node(store, symbol.declarations[0]);
    debug_assert!(file.is_some(), "File-bound declaration has no source file");
    file
}

/// Go: `func SymbolName(symbol *Symbol) string`.
pub fn symbol_name(store: &dyn crate::NodeStore, s: &Symbol) -> String {
    if let Some(decl) = s.value_declaration {
        if crate::utilities::is_private_identifier_class_element_declaration(store, decl) {
            return crate::node_text(store, store.node(decl).name().expect("class element name"))
                .to_string();
        }
    }
    s.name.clone()
}

/// Go: `func EscapeAllInternalSymbolNames(name string) string` — replaces
/// internal symbol name markers ("\xFE") with "__".
pub fn escape_all_internal_symbol_names(name: &str) -> String {
    name.replace(INTERNAL_SYMBOL_NAME_PREFIX, "__")
}

/// Go: `func EscapeInternalSymbolName(name string) string`.
pub fn escape_internal_symbol_name(name: &str) -> String {
    match name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        Some(rest) => format!("__{rest}"),
        None => name.to_string(),
    }
}

/// Go: `func EscapeSymbolName(name string) string` — converts a binder symbol
/// name into its escaped "__String" form. Internal names (prefixed with the
/// "\xFE" sentinel) become "__"-prefixed, and user names that already begin
/// with "__" gain an extra leading underscore so they can be distinguished
/// from internal names.
pub fn escape_symbol_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        return format!("__{rest}");
    }
    let bytes = name.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'_' && bytes[1] == b'_' {
        return format!("_{name}");
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, flags: SymbolFlags) -> Symbol {
        Symbol {
            flags,
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn internal_symbol_names_mirror_go() {
        assert_eq!(INTERNAL_SYMBOL_NAME_PREFIX, "\u{FE}");
        assert_eq!(INTERNAL_SYMBOL_NAME_CALL, "\u{FE}call");
        assert_eq!(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS, "export=");
        assert_eq!(INTERNAL_SYMBOL_NAME_DEFAULT, "default");
        assert_eq!(INTERNAL_SYMBOL_NAME_THIS, "this");
        assert_eq!(INTERNAL_SYMBOL_NAME_MODULE_EXPORTS, "module.exports");
        assert_eq!(
            INTERNAL_SYMBOL_NAME_IMPORT_ATTRIBUTES,
            format!("{INTERNAL_SYMBOL_NAME_PREFIX}importAttributes")
        );
    }

    #[test]
    fn escape_symbol_names_mirror_go() {
        assert_eq!(escape_all_internal_symbol_names("\u{FE}abc"), "__abc");
        assert_eq!(escape_all_internal_symbol_names("\u{FE}a\u{FE}b"), "__a__b");
        assert_eq!(escape_all_internal_symbol_names("plain"), "plain");
        assert_eq!(escape_internal_symbol_name("\u{FE}abc"), "__abc");
        assert_eq!(escape_internal_symbol_name("plain"), "plain");
        // escape_symbol_name adds a leading underscore to user "__" names.
        assert_eq!(escape_symbol_name("__foo"), "___foo");
        assert_eq!(escape_symbol_name("\u{FE}foo"), "__foo");
        assert_eq!(escape_symbol_name("foo"), "foo");
        assert_eq!(escape_symbol_name("_foo"), "_foo");
    }

    #[test]
    fn external_module_check() {
        let quoted = symbol("\"foo\"", SymbolFlags::VALUE_MODULE);
        assert!(symbol_is_external_module(&quoted));
        let unquoted = symbol("foo", SymbolFlags::VALUE_MODULE);
        assert!(!symbol_is_external_module(&unquoted));
        let not_module = symbol("\"foo\"", SymbolFlags::CLASS);
        assert!(!symbol_is_external_module(&not_module));
    }

    #[test]
    fn combined_flags() {
        let s = Symbol {
            flags: SymbolFlags::FUNCTION,
            ..Default::default()
        };
        // No export symbol: own flags.
        assert_eq!(combined_local_and_export_symbol_flags(&[Symbol::default()], &s), SymbolFlags::FUNCTION);
    }

    #[test]
    fn symbol_table_is_insertion_ordered() {
        let mut table: SymbolTable = SymbolTable::new();
        table.set("z".into(), SymbolId(0));
        table.set("a".into(), SymbolId(1));
        table.set("m".into(), SymbolId(2));
        let keys: Vec<&String> = table.keys().collect();
        assert_eq!(keys, [&"z".to_string(), &"a".to_string(), &"m".to_string()]);
    }
}
