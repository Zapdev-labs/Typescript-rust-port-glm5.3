// Port of tsc/internal/tsoptions/commandlineoption.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go's `type CompilerOptionsValue any` becomes a closed enum: the dynamic
// types that actually flow through the option tables (string, bool, int,
// float, []string, []any, map[string]any, the paths map, []PluginImport,
// Tristate, and the 6 core option enums). Go values that do not match any
// arm surface as `Null`/`Str` etc. exactly where Go's type assertions
// return zero values.
//
// PORT notes:
//   - `PathValueAsString`/`PathValuesAsStrings` accept the typed path forms;
//     in the port those flow as their JSON (string) form, so both helpers
//     read `Str`/`StrArray` — the strings are identical.
//   - `CommandLineOptionNameMap` (Go's map[string]*CommandLineOption) and
//     `*NameMap` collapse into one `NameMap` type (namemap.rs); Go's
//     `Get`/`GetSpellingSuggestion` surface is preserved.

use tsc_collections::OrderedMap;
use tsc_core::options_generated::{
    JsxEmit, ModuleDetectionKind, ModuleKind, ModuleResolutionKind, NewLineKind, PluginImport,
    ScriptTarget,
};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath, SourceMapLocation};

/// Go: `type CommandLineOptionKind string` + the `CommandLineOptionType*`
/// constants. The generator's literals use the same member names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandLineOptionKind {
    String,
    Number,
    Boolean,
    Object,
    List,
    ListOrElement,
    /// Go: `CommandLineOptionTypeEnum` (map-backed).
    Enum,
}

impl CommandLineOptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CommandLineOptionKind::String => "string",
            CommandLineOptionKind::Number => "number",
            CommandLineOptionKind::Boolean => "boolean",
            CommandLineOptionKind::Object => "object",
            CommandLineOptionKind::List => "list",
            CommandLineOptionKind::ListOrElement => "listOrElement",
            CommandLineOptionKind::Enum => "enum",
        }
    }
}

/// Go: `type CommandLineOptionPathKind uint8` + constants (0 = None).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CommandLineOptionPathKind {
    #[default]
    None,
    File,
    Directory,
    FileOrDirectory,
    SourceMapLocation,
    FileSpec,
    PathPattern,
    ResolvedPathPattern,
    ConfigLocator,
}

impl CommandLineOptionPathKind {
    /// Go: `func (k CommandLineOptionPathKind) IsRooted() bool`.
    pub fn is_rooted(self) -> bool {
        matches!(
            self,
            CommandLineOptionPathKind::File
                | CommandLineOptionPathKind::Directory
                | CommandLineOptionPathKind::FileOrDirectory
                | CommandLineOptionPathKind::ResolvedPathPattern
        )
    }

    /// Go: `func (k CommandLineOptionPathKind) IsFileSystemPath() bool`.
    pub fn is_file_system_path(self) -> bool {
        matches!(
            self,
            CommandLineOptionPathKind::File
                | CommandLineOptionPathKind::Directory
                | CommandLineOptionPathKind::FileOrDirectory
        )
    }
}

/// Go: `func PathValueAsString(value any) (string, bool)`.
/// PORT: typed paths flow as their JSON (string) form, so this reads `Str`.
pub fn path_value_as_string(value: &CompilerOptionsValue) -> Option<&str> {
    match value {
        CompilerOptionsValue::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Go: `func PathValuesAsStrings(value any) ([]string, bool)`.
pub fn path_values_as_strings(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    match value {
        CompilerOptionsValue::StrArray(v) => Some(v.clone()),
        _ => None,
    }
}

/// Go: `type CommandLineOption struct` — all fields are const-constructible so
/// the generated tables can live in `static`s (the generated literals are
/// fully explicit; see declarations_generated.rs PORT notes).
#[derive(Clone, Copy, Debug)]
pub struct CommandLineOption {
    pub name: &'static str,
    pub short_name: &'static str,
    pub kind: CommandLineOptionKind,
    pub path_kind: CommandLineOptionPathKind,
    pub is_file_path: bool,
    pub is_tsconfig_only: bool,
    pub is_command_line_only: bool,
    pub description: Option<&'static Message>,
    /// Go: `DefaultValueDescription any`.
    pub default_value_description: Option<DefaultValueDescription>,
    pub show_in_simplified_help_view: bool,
    pub category: Option<&'static Message>,
    /// Go: `extraValidation extraValidation` (unexported in Go; carried here
    /// because the generated tables must express it).
    pub extra_validation: ExtraValidation,
    pub min_value: i64,
    pub list_preserve_falsy_values: bool,
    /// Go: `ElementOptions CommandLineOptionNameMap` — the port names the
    /// lazily-built map (Go builds them at package init); the accessor is
    /// `element_options` in namemap.rs.
    pub element_options: ElementOptionsKind,
}

/// Go: `type extraValidation string` + `extraValidationLocale`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ExtraValidation {
    #[default]
    None,
    Locale,
}

/// Which lazily-built name map a declaration's `ElementOptions` refers to
/// (Go stores the built `CommandLineOptionNameMap` directly).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ElementOptionsKind {
    #[default]
    None,
    CompilerOptions,
    TypeAcquisition,
    Extends,
    TsconfigRoot,
}

/// Go: `DefaultValueDescription any` — the closed set the generator emits.
#[derive(Clone, Copy, Debug)]
pub enum DefaultValueDescription {
    Bool(bool),
    Int(i64),
    Str(&'static str),
    Message(&'static Message),
    /// Go: `core.TSUnknown`.
    Tristate(Tristate),
    /// Go: `core.ScriptTargetLatestStandard`.
    ScriptTarget(ScriptTarget),
}

impl CommandLineOption {
    /// Go: `func (o *CommandLineOption) DeprecatedKeys() *collections.Set[string]`.
    /// PORT: returns the generated entry slice (`has` via `contains`).
    pub fn deprecated_keys(&self) -> Option<&'static [&'static str]> {
        if self.kind != CommandLineOptionKind::Enum {
            return None;
        }
        crate::declarations_generated::command_line_option_deprecated(self.name)
    }

    /// Go: `func (o *CommandLineOption) EnumMap() *collections.OrderedMap[string, any]`.
    pub fn enum_map(&self) -> Option<&'static OrderedMap<String, CompilerOptionsValue>> {
        if self.kind != CommandLineOptionKind::Enum {
            return None;
        }
        crate::declarations_generated::command_line_option_enum_map(self.name)
    }

    /// Go: `func (o *CommandLineOption) Elements() *CommandLineOption`.
    pub fn elements(&self) -> Option<&'static CommandLineOption> {
        if self.kind != CommandLineOptionKind::List && self.kind != CommandLineOptionKind::ListOrElement
        {
            return None;
        }
        crate::declarations_generated::command_line_option_elements(self.name)
    }

    /// Go: `func (o *CommandLineOption) DisallowNullOrUndefined() bool`.
    pub fn disallow_null_or_undefined(&self) -> bool {
        self.name == "extends"
    }

    /// The lazily-built element-options name map (Go: the `ElementOptions`
    /// field's map value).
    pub fn element_options_map(&self) -> Option<&'static crate::namemap::NameMap> {
        crate::namemap::element_options(self.element_options)
    }
}

/// Go: `type CompilerOptionsValue any` — see the module PORT notes.
#[derive(Clone, Debug, Default)]
pub enum CompilerOptionsValue {
    #[default]
    Null,
    Bool(bool),
    /// Go: `int`.
    Int(i64),
    /// Go: `float64`.
    Float(f64),
    Str(String),
    /// Go: `[]string` (a `[]any` whose elements are all strings reads the
    /// same; the distinction never survives a Go type assertion).
    StrArray(Vec<String>),
    /// Go: `[]any`.
    Array(Vec<CompilerOptionsValue>),
    /// Go: `map[string]any` (`*collections.OrderedMap[string, any]`).
    Object(OrderedMap<String, CompilerOptionsValue>),
    /// Go: `*collections.OrderedMap[string, []string]` (the `paths` option).
    PathsMap(OrderedMap<String, Vec<String>>),
    PluginImports(Vec<PluginImport>),
    Tristate(Tristate),
    JsxEmit(JsxEmit),
    ModuleKind(ModuleKind),
    ModuleResolutionKind(ModuleResolutionKind),
    ModuleDetectionKind(ModuleDetectionKind),
    NewLineKind(NewLineKind),
    ScriptTarget(ScriptTarget),
}

impl CompilerOptionsValue {
    /// Go: `value == nil`.
    pub fn is_null(&self) -> bool {
        matches!(self, CompilerOptionsValue::Null)
    }

    /// Go: `floatOrInt32ToFlag[T](value)` — returns the flag enum already
    /// carried, or converts a number (JSON numbers parse as float64 in Go;
    /// the port keeps both int and float forms).
    fn as_flag<E, F>(&self, from_number: F) -> E
    where
        F: Fn(f64) -> E,
    {
        match self {
            CompilerOptionsValue::Float(f) => from_number(*f),
            other => panic!(
                "tsoptions: floatOrInt32ToFlag: value is not a number: {other:?}"
            ),
        }
    }

    pub fn as_jsx_emit_flag(&self) -> JsxEmit {
        match self {
            CompilerOptionsValue::JsxEmit(v) => *v,
            other => other.as_flag(|f| {
                JsxEmit::from_i32(f as i32).unwrap_or_else(|| panic!("invalid JsxEmit value {f}"))
            }),
        }
    }

    pub fn as_module_kind_flag(&self) -> ModuleKind {
        match self {
            CompilerOptionsValue::ModuleKind(v) => *v,
            other => other.as_flag(|f| {
                ModuleKind::from_i32(f as i32)
                    .unwrap_or_else(|| panic!("invalid ModuleKind value {f}"))
            }),
        }
    }

    pub fn as_module_resolution_kind_flag(&self) -> ModuleResolutionKind {
        match self {
            CompilerOptionsValue::ModuleResolutionKind(v) => *v,
            other => other.as_flag(|f| {
                ModuleResolutionKind::from_i32(f as i32).unwrap_or_else(|| {
                    panic!("invalid ModuleResolutionKind value {f}")
                })
            }),
        }
    }

    pub fn as_module_detection_kind_flag(&self) -> ModuleDetectionKind {
        match self {
            CompilerOptionsValue::ModuleDetectionKind(v) => *v,
            other => other.as_flag(|f| {
                ModuleDetectionKind::from_i32(f as i32).unwrap_or_else(|| {
                    panic!("invalid ModuleDetectionKind value {f}")
                })
            }),
        }
    }

    pub fn as_new_line_kind_flag(&self) -> NewLineKind {
        match self {
            CompilerOptionsValue::NewLineKind(v) => *v,
            other => other.as_flag(|f| {
                NewLineKind::from_i32(f as i32)
                    .unwrap_or_else(|| panic!("invalid NewLineKind value {f}"))
            }),
        }
    }

    pub fn as_script_target_flag(&self) -> ScriptTarget {
        match self {
            CompilerOptionsValue::ScriptTarget(v) => *v,
            other => other.as_flag(|f| {
                ScriptTarget::from_i32(f as i32)
                    .unwrap_or_else(|| panic!("invalid ScriptTarget value {f}"))
            }),
        }
    }
}

/// Go's `PathValueAsString` accepts the four typed path forms; the port
/// carries them as their string form (see module PORT notes), so a helper
/// that re-wraps a typed path for the value model:
pub fn rooted_path_as_value(path: &RootedPath) -> CompilerOptionsValue {
    CompilerOptionsValue::Str(path.as_string().to_owned())
}

/// See `rooted_path_as_value`.
pub fn rooted_file_path_as_value(path: &RootedFilePath) -> CompilerOptionsValue {
    CompilerOptionsValue::Str(path.as_string().to_owned())
}

/// See `rooted_path_as_value`.
pub fn rooted_directory_path_as_value(path: &RootedDirectoryPath) -> CompilerOptionsValue {
    CompilerOptionsValue::Str(path.as_string().to_owned())
}

/// See `rooted_path_as_value`.
pub fn source_map_location_as_value(path: &SourceMapLocation) -> CompilerOptionsValue {
    CompilerOptionsValue::Str(path.as_string().to_owned())
}
