// Port of tsc/internal/tsoptions/rawcompileroptions.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go's `RawCompilerOptions` wraps `*collections.OrderedMap[string, any]`
//     and round-trips through the internal/json codec (MarshalJSONTo /
//     UnmarshalJSONFrom). The port's value model is `CompilerOptionsValue`
//     (commandlineoption.rs), so both codec arms operate on that tree:
//     `marshal` emits via showconfig's deterministic marshaller, and
//     `unmarshal` reads the crate's order-preserving JSON text parser.
//   - `Finalize`'s loop is exact: unknown keys are dropped unless they are
//     case-insensitive matches (key != option.Name), and the three legacy
//     string keys (`configFilePath`, `allowNonTsExtensions`,
//     `suppressOutputPathCheck`) pass through.

use tsc_ast::Diagnostic;
use tsc_collections::OrderedMap;
use tsc_core::options_generated::CompilerOptions;
use tsc_tspath::{get_normalized_absolute_path, RootedDirectoryPath};

use crate::commandlineoption::{CommandLineOptionKind, CommandLineOptionPathKind, CompilerOptionsValue};
use crate::namemap::command_line_compiler_options_map;
use crate::options_generated::parse_compiler_options;
use crate::parsinghelpers::{
    get_substituted_path_with_config_dir_template, parse_string_array,
    starts_with_config_dir_template,
};

/// Go: `type RawCompilerOptions struct` — the JSON/API representation of
/// compiler options. Filesystem paths remain strings until `finalize`
/// resolves them against a base directory.
#[derive(Clone, Debug, Default)]
pub struct RawCompilerOptions {
    values: OrderedMap<String, CompilerOptionsValue>,
}

impl RawCompilerOptions {
    /// The empty representation (Go's zero value).
    pub fn new() -> Self {
        RawCompilerOptions {
            values: OrderedMap::new(),
        }
    }

    /// Read access to the raw entries, in insertion order.
    pub fn entries(&self) -> impl Iterator<Item = (&String, &CompilerOptionsValue)> {
        self.values.entries()
    }

    /// Set a raw value (Go: the json decoder's `values.Set`).
    pub fn set(&mut self, key: String, value: CompilerOptionsValue) {
        self.values.set(key, value);
    }

    /// Go: `func (o *RawCompilerOptions) MarshalJSONTo(enc) error` — the
    /// marshalled form of the raw map, via the deterministic marshaller.
    pub fn marshal(&self) -> String {
        crate::showconfig::marshal_value(&CompilerOptionsValue::Object(self.values.clone()), 0)
    }

    /// Go: `func (o *RawCompilerOptions) UnmarshalJSONFrom(dec) error`.
    ///
    /// The Go decoder special-cases `paths` to decode as an ordered map; the
    /// port's JSON parser produces `PathsMap` for map-valued `paths` via the
    /// object-shape rule (any object value decodes as `Object`; `parseStringMap`
    /// accepts both shapes at use time — see the parse helpers' PORT notes).
    pub fn unmarshal(&mut self, json_text: &str) -> Result<(), String> {
        let value = crate::tsconfigparsing::parse_json_text(json_text)?;
        match value {
            CompilerOptionsValue::Object(map) => {
                for (key, value) in map.entries() {
                    self.values.set(key.clone(), value.clone());
                }
                Ok(())
            }
            CompilerOptionsValue::Null => Ok(()),
            _ => Err(
                "cannot unmarshal non-object JSON value into RawCompilerOptions".to_string(),
            ),
        }
    }

    /// Go: `func (o *RawCompilerOptions) Finalize(basePath) (*core.CompilerOptions, []*ast.Diagnostic)`.
    pub fn finalize(&self, base_path: &RootedDirectoryPath) -> (CompilerOptions, Vec<Diagnostic>) {
        let mut options = CompilerOptions::default();
        for (key, value) in self.values.entries() {
            let option = command_line_compiler_options_map().get(key);
            let option = match option {
                // Go: `if option != nil && key != option.Name { continue }` —
                // a case-insensitive-only match does not finalize.
                Some(option) if key != option.name => continue,
                option => option,
            };
            let mut path_kind = CommandLineOptionPathKind::None;
            if let Some(option) = option {
                path_kind = option.path_kind;
                if option.kind == CommandLineOptionKind::List {
                    if let Some(element) = option.elements() {
                        path_kind = element.path_kind;
                    }
                }
            } else if key == "configFilePath" {
                path_kind = CommandLineOptionPathKind::File;
            } else if key != "allowNonTsExtensions" && key != "suppressOutputPathCheck" {
                continue;
            }
            let mut value = value.clone();
            if path_kind.is_rooted() {
                if let Some(option) = option
                    && option.kind == CommandLineOptionKind::List
                {
                    value = CompilerOptionsValue::StrArray(
                        parse_string_array(&value)
                            .into_iter()
                            .map(|path| {
                                if starts_with_config_dir_template(&path) {
                                    get_substituted_path_with_config_dir_template(&path, base_path)
                                } else {
                                    get_normalized_absolute_path(&path, base_path).into_owned()
                                }
                            })
                            .collect(),
                    );
                } else if let CompilerOptionsValue::Str(path) = &value
                    && !path.is_empty()
                {
                    value = if starts_with_config_dir_template(path) {
                        CompilerOptionsValue::Str(get_substituted_path_with_config_dir_template(
                            path, base_path,
                        ))
                    } else {
                        CompilerOptionsValue::Str(
                            get_normalized_absolute_path(path, base_path).into_owned(),
                        )
                    };
                }
            }
            parse_compiler_options(key, &value, &mut options);
        }
        (options, Vec::new())
    }
}
