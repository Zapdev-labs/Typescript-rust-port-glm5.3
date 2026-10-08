// Ported from tsc/internal/ast/symbolflags.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Exact bit values from Go (`type SymbolFlags uint32`).

define_flags!(SymbolFlags, u32);

impl SymbolFlags {
    pub const FUNCTION_SCOPED_VARIABLE: SymbolFlags = SymbolFlags(1 << 0); // Variable (var) or parameter
    pub const BLOCK_SCOPED_VARIABLE: SymbolFlags = SymbolFlags(1 << 1); // A block-scoped variable (let or const)
    pub const PROPERTY: SymbolFlags = SymbolFlags(1 << 2); // Property or enum member
    pub const ENUM_MEMBER: SymbolFlags = SymbolFlags(1 << 3); // Enum member
    pub const FUNCTION: SymbolFlags = SymbolFlags(1 << 4); // Function
    pub const CLASS: SymbolFlags = SymbolFlags(1 << 5); // Class
    pub const INTERFACE: SymbolFlags = SymbolFlags(1 << 6); // Interface
    pub const CONST_ENUM: SymbolFlags = SymbolFlags(1 << 7); // Const enum
    pub const REGULAR_ENUM: SymbolFlags = SymbolFlags(1 << 8); // Enum
    pub const VALUE_MODULE: SymbolFlags = SymbolFlags(1 << 9); // Instantiated module
    pub const NAMESPACE_MODULE: SymbolFlags = SymbolFlags(1 << 10); // Uninstantiated module
    pub const TYPE_LITERAL: SymbolFlags = SymbolFlags(1 << 11); // Type Literal or mapped type
    pub const OBJECT_LITERAL: SymbolFlags = SymbolFlags(1 << 12); // Object Literal
    pub const METHOD: SymbolFlags = SymbolFlags(1 << 13); // Method
    pub const CONSTRUCTOR: SymbolFlags = SymbolFlags(1 << 14); // Constructor
    pub const GET_ACCESSOR: SymbolFlags = SymbolFlags(1 << 15); // Get accessor
    pub const SET_ACCESSOR: SymbolFlags = SymbolFlags(1 << 16); // Set accessor
    pub const SIGNATURE: SymbolFlags = SymbolFlags(1 << 17); // Call, construct, or index signature
    pub const TYPE_PARAMETER: SymbolFlags = SymbolFlags(1 << 18); // Type parameter
    pub const TYPE_ALIAS: SymbolFlags = SymbolFlags(1 << 19); // Type alias
    /// Exported value marker (see comment in declareModuleMember in binder)
    pub const EXPORT_VALUE: SymbolFlags = SymbolFlags(1 << 20);
    /// An alias for another symbol (see comment in isAliasSymbolDeclaration in checker)
    pub const ALIAS: SymbolFlags = SymbolFlags(1 << 21);
    /// Prototype property (no source representation)
    pub const PROTOTYPE: SymbolFlags = SymbolFlags(1 << 22);
    /// Export * declaration
    pub const EXPORT_STAR: SymbolFlags = SymbolFlags(1 << 23);
    /// Optional property
    pub const OPTIONAL: SymbolFlags = SymbolFlags(1 << 24);
    /// Transient symbol (created during type check)
    pub const TRANSIENT: SymbolFlags = SymbolFlags(1 << 25);
    /// Assignment to property on function acting as declaration (eg `func.prop = 1`)
    pub const ASSIGNMENT: SymbolFlags = SymbolFlags(1 << 26);
    /// Symbol for CommonJS `module` of `module.exports`
    pub const MODULE_EXPORTS: SymbolFlags = SymbolFlags(1 << 27);
    /// Module contains only const enums or other modules with only const enums
    pub const CONST_ENUM_ONLY_MODULE: SymbolFlags = SymbolFlags(1 << 28);
    pub const REPLACEABLE_BY_METHOD: SymbolFlags = SymbolFlags(1 << 29);
    /// Flag to signal this is a global lookup
    pub const GLOBAL_LOOKUP: SymbolFlags = SymbolFlags(1 << 30);
    /// All flags except SymbolFlagsGlobalLookup (Go: `1<<30 - 1`)
    pub const ALL: SymbolFlags = SymbolFlags((1 << 30) - 1);

    pub const ENUM: SymbolFlags = SymbolFlags(Self::REGULAR_ENUM.0 | Self::CONST_ENUM.0);
    pub const VARIABLE: SymbolFlags = SymbolFlags(Self::FUNCTION_SCOPED_VARIABLE.0 | Self::BLOCK_SCOPED_VARIABLE.0);
    pub const VALUE: SymbolFlags = SymbolFlags(
        (Self::VARIABLE.0
            | Self::PROPERTY.0
            | Self::ENUM_MEMBER.0
            | Self::OBJECT_LITERAL.0
            | Self::FUNCTION.0
            | Self::CLASS.0
            | Self::ENUM.0
            | Self::VALUE_MODULE.0
            | Self::METHOD.0
            | Self::GET_ACCESSOR.0
            | Self::SET_ACCESSOR.0) as u32,
    );
    pub const TYPE: SymbolFlags = SymbolFlags(
        (Self::CLASS.0
            | Self::INTERFACE.0
            | Self::ENUM.0
            | Self::ENUM_MEMBER.0
            | Self::TYPE_LITERAL.0
            | Self::TYPE_PARAMETER.0
            | Self::TYPE_ALIAS.0) as u32,
    );
    pub const NAMESPACE: SymbolFlags = SymbolFlags(Self::VALUE_MODULE.0 | Self::NAMESPACE_MODULE.0 | Self::ENUM.0);
    pub const MODULE: SymbolFlags = SymbolFlags(Self::VALUE_MODULE.0 | Self::NAMESPACE_MODULE.0);
    pub const ACCESSOR: SymbolFlags = SymbolFlags(Self::GET_ACCESSOR.0 | Self::SET_ACCESSOR.0);

    // Variables can be redeclared, but can not redeclare a block-scoped declaration with the
    // same name, or any other value that is not a variable, e.g. ValueModule or Class
    pub const FUNCTION_SCOPED_VARIABLE_EXCLUDES: SymbolFlags =
        SymbolFlags(Self::VALUE.0 & !Self::FUNCTION_SCOPED_VARIABLE.0);

    // Block-scoped declarations are not allowed to be re-declared
    // they can not merge with anything in the value space
    pub const BLOCK_SCOPED_VARIABLE_EXCLUDES: SymbolFlags = Self::VALUE;

    pub const PARAMETER_EXCLUDES: SymbolFlags = Self::VALUE;
    pub const PROPERTY_EXCLUDES: SymbolFlags = SymbolFlags(Self::VALUE.0 & !(Self::PROPERTY.0 | Self::ACCESSOR.0));
    pub const ENUM_MEMBER_EXCLUDES: SymbolFlags = SymbolFlags(Self::VALUE.0 | Self::TYPE.0);
    pub const FUNCTION_EXCLUDES: SymbolFlags =
        SymbolFlags(Self::VALUE.0 & !(Self::FUNCTION.0 | Self::VALUE_MODULE.0 | Self::CLASS.0));
    /// class-interface mergability done in checker.ts
    pub const CLASS_EXCLUDES: SymbolFlags = SymbolFlags(
        ((Self::VALUE.0 | Self::TYPE.0) & !(Self::VALUE_MODULE.0 | Self::INTERFACE.0 | Self::FUNCTION.0)) as u32,
    );
    pub const INTERFACE_EXCLUDES: SymbolFlags = SymbolFlags(Self::TYPE.0 & !(Self::INTERFACE.0 | Self::CLASS.0));
    /// regular enums merge only with regular enums and modules
    pub const REGULAR_ENUM_EXCLUDES: SymbolFlags = SymbolFlags(
        ((Self::VALUE.0 | Self::TYPE.0) & !(Self::REGULAR_ENUM.0 | Self::VALUE_MODULE.0)) as u32,
    );
    /// const enums merge only with const enums
    pub const CONST_ENUM_EXCLUDES: SymbolFlags = SymbolFlags((Self::VALUE.0 | Self::TYPE.0) & !Self::CONST_ENUM.0);
    pub const VALUE_MODULE_EXCLUDES: SymbolFlags = SymbolFlags(
        (Self::VALUE.0 & !(Self::FUNCTION.0 | Self::CLASS.0 | Self::REGULAR_ENUM.0 | Self::VALUE_MODULE.0)) as u32,
    );
    pub const NAMESPACE_MODULE_EXCLUDES: SymbolFlags = SymbolFlags::NONE;
    pub const METHOD_EXCLUDES: SymbolFlags = SymbolFlags(Self::VALUE.0 & !Self::METHOD.0);
    pub const GET_ACCESSOR_EXCLUDES: SymbolFlags =
        SymbolFlags(Self::VALUE.0 & !(Self::SET_ACCESSOR.0 | Self::PROPERTY.0));
    pub const SET_ACCESSOR_EXCLUDES: SymbolFlags =
        SymbolFlags(Self::VALUE.0 & !(Self::GET_ACCESSOR.0 | Self::PROPERTY.0));
    pub const ACCESSOR_EXCLUDES: SymbolFlags = SymbolFlags(Self::VALUE.0 & !Self::PROPERTY.0);
    pub const TYPE_PARAMETER_EXCLUDES: SymbolFlags = SymbolFlags(Self::TYPE.0 & !Self::TYPE_PARAMETER.0);
    pub const TYPE_ALIAS_EXCLUDES: SymbolFlags = Self::TYPE;
    pub const ALIAS_EXCLUDES: SymbolFlags = Self::ALIAS;
    pub const MODULE_MEMBER: SymbolFlags = SymbolFlags(
        (Self::VARIABLE.0
            | Self::FUNCTION.0
            | Self::CLASS.0
            | Self::INTERFACE.0
            | Self::ENUM.0
            | Self::MODULE.0
            | Self::TYPE_ALIAS.0
            | Self::ALIAS.0) as u32,
    );
    pub const EXPORT_HAS_LOCAL: SymbolFlags =
        SymbolFlags(Self::FUNCTION.0 | Self::CLASS.0 | Self::ENUM.0 | Self::VALUE_MODULE.0);
    pub const BLOCK_SCOPED: SymbolFlags =
        SymbolFlags(Self::BLOCK_SCOPED_VARIABLE.0 | Self::CLASS.0 | Self::ENUM.0);
    pub const PROPERTY_OR_ACCESSOR: SymbolFlags = SymbolFlags(Self::PROPERTY.0 | Self::ACCESSOR.0);
    pub const CLASS_MEMBER: SymbolFlags = SymbolFlags(Self::METHOD.0 | Self::ACCESSOR.0 | Self::PROPERTY.0);
    pub const EXPORT_SUPPORTS_DEFAULT_MODIFIER: SymbolFlags =
        SymbolFlags(Self::CLASS.0 | Self::FUNCTION.0 | Self::INTERFACE.0);
    /// Go: `^SymbolFlagsExportSupportsDefaultModifier`
    pub const EXPORT_DOES_NOT_SUPPORT_DEFAULT_MODIFIER: SymbolFlags =
        SymbolFlags(!Self::EXPORT_SUPPORTS_DEFAULT_MODIFIER.0);
    pub const LATE_BINDING_CONTAINER: SymbolFlags = SymbolFlags(
        (Self::CLASS.0 | Self::INTERFACE.0 | Self::TYPE_LITERAL.0 | Self::OBJECT_LITERAL.0 | Self::FUNCTION.0) as u32,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_flags_bit_values_mirror_go() {
        assert_eq!(SymbolFlags::FUNCTION_SCOPED_VARIABLE, SymbolFlags(1 << 0));
        assert_eq!(SymbolFlags::ALIAS, SymbolFlags(1 << 21));
        assert_eq!(SymbolFlags::PROTOTYPE, SymbolFlags(1 << 22));
        assert_eq!(SymbolFlags::TRANSIENT, SymbolFlags(1 << 25));
        assert_eq!(SymbolFlags::REPLACEABLE_BY_METHOD, SymbolFlags(1 << 29));
        assert_eq!(SymbolFlags::GLOBAL_LOOKUP, SymbolFlags(1 << 30));
        // Go: `SymbolFlagsAll = 1<<30 - 1`
        assert_eq!(SymbolFlags::ALL, SymbolFlags((1 << 30) - 1));
    }

    #[test]
    fn symbol_flag_composites_mirror_go() {
        assert_eq!(SymbolFlags::VARIABLE, SymbolFlags::FUNCTION_SCOPED_VARIABLE | SymbolFlags::BLOCK_SCOPED_VARIABLE);
        assert_eq!(SymbolFlags::ENUM, SymbolFlags::REGULAR_ENUM | SymbolFlags::CONST_ENUM);
        assert_eq!(SymbolFlags::ACCESSOR, SymbolFlags::GET_ACCESSOR | SymbolFlags::SET_ACCESSOR);
        assert_eq!(SymbolFlags::NAMESPACE, SymbolFlags::VALUE_MODULE | SymbolFlags::NAMESPACE_MODULE | SymbolFlags::ENUM);
        // Go: `SymbolFlagsFunctionScopedVariableExcludes = SymbolFlagsValue & ^SymbolFlagsFunctionScopedVariable`
        assert_eq!(
            SymbolFlags::FUNCTION_SCOPED_VARIABLE_EXCLUDES,
            SymbolFlags::VALUE & !SymbolFlags::FUNCTION_SCOPED_VARIABLE
        );
        assert_eq!(SymbolFlags::NAMESPACE_MODULE_EXCLUDES, SymbolFlags::NONE);
        assert_eq!(SymbolFlags::TYPE_ALIAS_EXCLUDES, SymbolFlags::TYPE);
        assert_eq!(SymbolFlags::ALIAS_EXCLUDES, SymbolFlags::ALIAS);
        assert_eq!(
            SymbolFlags::EXPORT_DOES_NOT_SUPPORT_DEFAULT_MODIFIER,
            !SymbolFlags::EXPORT_SUPPORTS_DEFAULT_MODIFIER
        );
        // ALL covers every bit below GlobalLookup and excludes it.
        assert_eq!(SymbolFlags::ALL, SymbolFlags::GLOBAL_LOOKUP - SymbolFlags(1));
        assert!(!SymbolFlags::ALL.intersects(SymbolFlags::GLOBAL_LOOKUP));
    }
}
