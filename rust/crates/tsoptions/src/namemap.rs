// Port of tsc/internal/tsoptions/namemap.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go has two lookup shapes: `*NameMap` (ordered, short-name aware —
//     CompilerNameMap/BuildNameMap) and `CommandLineOptionNameMap`
//     (map[string]*CommandLineOption — what `commandLineOptionsToMap` builds
//     and what `ElementOptions` holds). Both collapse into this one `NameMap`
//     type: the ordered map is keyed by lowercase name and the map-only
//     `GetSpellingSuggestion` surface is implemented over the same keys.
//   - Go builds all maps at package init; the port builds them lazily under
//     `OnceLock` (the tables they read are themselves `static`s).
//   - `GetSpellingSuggestion` delegates to the scanner port's
//     `get_spelling_suggestion_for_strings` (Go: `core.GetSpellingSuggestion`,
//     whose candidate comparison lives in scanner's levenshtein shim).

use std::sync::OnceLock;

use rustc_hash::FxHashMap;
use tsc_collections::OrderedMap;

use crate::commandlineoption::{CommandLineOption, ElementOptionsKind};
use crate::declarations_generated::{
    COMPILEONSAVECOMMANDLINEOPTION, COMPILEROPTIONSDECLARATION, EXTENDSOPTIONDECLARATION,
    OPTIONS_FOR_BUILD, TYPE_ACQUISITION_DECLS, TYPEACQUISITIONDECLARATION,
};

/// Go: `type NameMap struct` (+ `CommandLineOptionNameMap`, see PORT notes).
pub struct NameMap {
    /// Keyed by `strings.ToLower(option.Name)` — Go: `optionsNames`.
    options_names: OrderedMap<String, &'static CommandLineOption>,
    /// Go: `shortOptionNames` (shortName → full name).
    short_option_names: FxHashMap<&'static str, &'static str>,
}

impl NameMap {
    /// Go: `func (nm *NameMap) Get(name string) *CommandLineOption` —
    /// `GetOrZero` means a missing key reads as `None` in the port.
    pub fn get(&self, name: &str) -> Option<&'static CommandLineOption> {
        let lower = name.to_ascii_lowercase();
        self.options_names.get(&lower).copied()
    }

    /// Go: `func (nm *NameMap) GetFromShort(shortName string) *CommandLineOption`.
    pub fn get_from_short(&self, short_name: &str) -> Option<&'static CommandLineOption> {
        self.short_option_names
            .get(short_name)
            .and_then(|name| self.get(name))
    }

    /// Go: `func (nm *NameMap) GetOptionDeclarationFromName(optionName string, allowShort bool)`.
    pub fn get_option_declaration_from_name(
        &self,
        option_name: &str,
        allow_short: bool,
    ) -> Option<&'static CommandLineOption> {
        let mut option_name = option_name.to_ascii_lowercase();
        if allow_short {
            if let Some(short) = self.short_option_names.get(option_name.as_str()) {
                option_name = short.to_ascii_lowercase();
            }
        }
        self.get(&option_name)
    }

    /// Go: `func (m CommandLineOptionNameMap) GetSpellingSuggestion(name string)` —
    /// the best candidate is looked back up in the map (keys are lowercase).
    pub fn get_spelling_suggestion(&self, name: &str) -> Option<&'static CommandLineOption> {
        let suggestion =
            tsc_scanner::go_shims::get_spelling_suggestion_for_strings(name, self.keys());
        if suggestion.is_empty() {
            return None;
        }
        self.get(&suggestion)
    }

    /// The (lowercase) option names, in insertion order — Go iterates the
    /// map keys (unordered); ordered here, which only fixes iteration.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.options_names.keys().map(|k| k.as_str())
    }

    /// All declarations, in insertion order (Go: callers iterate the
    /// declaration slices; the map order is what `commandLineOptionsToMap`
    /// preserves).
    pub fn values(&self) -> impl Iterator<Item = &'static CommandLineOption> + '_ {
        self.options_names.values().copied()
    }
}

/// Go: `func GetNameMapFromList(optDecls []*CommandLineOption) *NameMap`.
pub fn get_name_map_from_list(opt_decls: &[&'static CommandLineOption]) -> NameMap {
    let mut options_names =
        OrderedMap::<String, &'static CommandLineOption>::with_size_hint(opt_decls.len());
    let mut short_option_names = FxHashMap::default();
    for option in opt_decls {
        options_names.set(option.name.to_ascii_lowercase(), option);
        if !option.short_name.is_empty() {
            short_option_names.insert(option.short_name, option.name);
        }
    }
    NameMap {
        options_names,
        short_option_names,
    }
}

/// Go: `func commandLineOptionsToMap(compilerOptions []*CommandLineOption) CommandLineOptionNameMap` —
/// see the module PORT notes for the collapse of the two map shapes.
pub fn command_line_options_to_map(
    compiler_options: &[&'static CommandLineOption],
) -> NameMap {
    get_name_map_from_list(compiler_options)
}

/// Go: `var CompilerNameMap = GetNameMapFromList(OptionsDeclarations)`.
pub fn compiler_name_map() -> &'static NameMap {
    static MAP: OnceLock<NameMap> = OnceLock::new();
    MAP.get_or_init(|| get_name_map_from_list(&crate::declarations_generated::options_declarations()))
}

/// The map the generated parsing tables look options up in — Go:
/// `var CommandLineCompilerOptionsMap = commandLineOptionsToMap(OptionsDeclarations)`
/// (tsconfigparsing.go; the full common+compiler declaration list).
pub fn command_line_compiler_options_map() -> &'static NameMap {
    static MAP: OnceLock<NameMap> = OnceLock::new();
    MAP.get_or_init(|| command_line_options_to_map(&crate::declarations_generated::options_declarations()))
}

/// Go: `var BuildNameMap = GetNameMapFromList(BuildOpts)` where
/// `BuildOpts = slices.Concat(commonOptionsWithBuild, OptionsForBuild)`.
pub fn build_name_map() -> &'static NameMap {
    static MAP: OnceLock<NameMap> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut all: Vec<&'static CommandLineOption> =
            crate::declarations_generated::COMMON_OPTIONS_WITH_BUILD.to_vec();
        all.extend_from_slice(crate::declarations_generated::OPTIONS_FOR_BUILD);
        get_name_map_from_list(&all)
    })
}

/// The lazily-built element-options maps (Go: the `ElementOptions` field's
/// map value, built at package init via `commandLineOptionsToMap`).
pub fn element_options(kind: ElementOptionsKind) -> Option<&'static NameMap> {
    match kind {
        ElementOptionsKind::None => None,
        // Go: compilerOptionsDeclaration.ElementOptions = CommandLineCompilerOptionsMap.
        ElementOptionsKind::CompilerOptions => Some(command_line_compiler_options_map()),
        // Go: typeAcquisitionDeclaration.ElementOptions =
        //   commandLineOptionsToMap(typeAcquisitionDecls).
        ElementOptionsKind::TypeAcquisition => {
            static MAP: OnceLock<NameMap> = OnceLock::new();
            Some(MAP.get_or_init(|| command_line_options_to_map(TYPE_ACQUISITION_DECLS)))
        }
        // Go: extendsOptionDeclaration.ElementOptions =
        //   commandLineOptionsToMap([]*CommandLineOption{{extends element}}).
        ElementOptionsKind::Extends => {
            static MAP: OnceLock<NameMap> = OnceLock::new();
            Some(MAP.get_or_init(|| {
                command_line_options_to_map(&[
                    crate::declarations_generated::command_line_option_elements("extends")
                        .expect("extends element"),
                ])
            }))
        }
        // Go: tsconfigRootOptionsMap.ElementOptions = commandLineOptionsToMap(
        //   []*CommandLineOption{compilerOptionsDeclaration, typeAcquisitionDeclaration,
        //   extendsOptionDeclaration, references, contentMappers, files, include,
        //   exclude, compileOnSaveCommandLineOption}).
        ElementOptionsKind::TsconfigRoot => {
            static MAP: OnceLock<NameMap> = OnceLock::new();
            Some(MAP.get_or_init(|| {
                let references = crate::declarations_generated::command_line_option_elements("references")
                    .expect("references element");
                let content_mappers = crate::declarations_generated::command_line_option_elements("contentMappers")
                    .expect("contentMappers element");
                let files = crate::declarations_generated::command_line_option_elements("files")
                    .expect("files element");
                let include = crate::declarations_generated::command_line_option_elements("include")
                    .expect("include element");
                let exclude = crate::declarations_generated::command_line_option_elements("exclude")
                    .expect("exclude element");
                command_line_options_to_map(&[
                    &COMPILEROPTIONSDECLARATION,
                    &TYPEACQUISITIONDECLARATION,
                    &EXTENDSOPTIONDECLARATION,
                    references,
                    content_mappers,
                    files,
                    include,
                    exclude,
                    &COMPILEONSAVECOMMANDLINEOPTION,
                ])
            }))
        }
    }
}

/// Go: `var OptionsForBuild` access for `build_opts()` consumers.
pub fn options_for_build() -> &'static [&'static CommandLineOption] {
    OPTIONS_FOR_BUILD
}
