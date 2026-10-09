// Port of tsc/internal/tsoptions (M4 wave C) @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The generated tables (options_generated.rs, declarations_generated.rs) are
// emitted by rust/tools/gen-options from the mechanically extracted
// tsoptions-model.json (see gen-options/data/README.md). The hand-written
// modules here carry the parsing/serializing logic around them; per-file
// PORT notes list the M4-scope deltas (see PORTING-NOTES.md for the roll-up).
//
// Not yet ported from the Go package (tracked in PORTING-NOTES.md):
//   - tsconfigparsing.go's jsonConversionNotifier surface
//     (parseOwnConfigOfJsonSourceFile's per-key option conversion with
//     positioned unknown-option/root-compiler-option diagnostics — the M6
//     config-file wave landed the AST→value conversion and both parse
//     routes; the notifier's positioned arms stay deferred)
//   - contentmapper's host.go/hostimpl.go (out of M4 scope)
//
// getFileNamesFromConfigSpecs (glob-driven file enumeration) and the
// vfsmatch SpecMatcher were ported with the showconfig parity twin
// (the twin's byte-diff requires them); see tsconfigparsing.rs and
// tsc-module's vfsmatch.rs PORT notes.

pub mod commandlineoption;
pub mod commandlineparser;
pub mod ast_shims;
pub mod declarations_generated;
pub mod diagnostics;
pub mod enummaps;
pub mod errors;
pub mod namemap;
pub mod options_generated;
pub mod parsedbuildcommandline;
pub mod parsedcommandline;
pub mod parsedoptions;
pub mod parsinghelpers;
pub mod rawcompileroptions;
pub mod showconfig;
pub mod tsconfigparsing;
pub mod wildcarddirectories;

pub use commandlineoption::{
    path_value_as_string, path_values_as_strings, CommandLineOption, CommandLineOptionKind,
    CommandLineOptionPathKind, CompilerOptionsValue, DefaultValueDescription, ElementOptionsKind,
    ExtraValidation,
};
pub use enummaps::{
    get_default_lib_file_name, get_lib_file_name, lib_files_set, libs, target_to_lib_map,
};
pub use errors::{create_diagnostic_for_node_in_source_file, create_diagnostic_for_node_in_source_file_or_compiler_diagnostic, new_compiler_diagnostic};
pub use namemap::{
    build_name_map, command_line_compiler_options_map, command_line_options_to_map,
    compiler_name_map, get_name_map_from_list, NameMap,
};
pub use options_generated::{
    compiler_options_affect_declaration_path, compiler_options_affect_emit,
    compiler_options_affect_semantic_diagnostics, get_default_compiler_options,
    get_default_type_acquisition, parse_build_options, parse_compiler_options,
    parse_type_acquisition, serialize_compiler_option_enum, serialize_compiler_options,
};
pub use commandlineparser::{
    parse_build_command_line, parse_command_line, parse_list_type_option, CommandLineParser,
};
pub use parsedbuildcommandline::ParsedBuildCommandLine;
pub use parsedcommandline::ParsedCommandLine;
pub use parsedoptions::ParsedOptions;
pub use parsinghelpers::{
    parse_content_mapper, parse_project_reference, parse_string, parse_string_array,
    parse_string_array_strict, parse_tristate, BuildOptionsParser, CompilerOptionsParser,
    OptionParser,
};
pub use rawcompileroptions::RawCompilerOptions;
pub use showconfig::{convert_to_tsconfig, TSConfig};
pub use tsconfigparsing::{
    convert_config_file_to_object, create_diagnostic_at_reference_syntax,
    for_each_property_assignment, for_each_ts_config_prop_array,
    get_options_syntax_by_array_element_value, get_parsed_command_line_of_config_file_path,
    get_ts_config_object_literal_expression, get_ts_config_prop_array_element_value,
    new_tsconfig_source_file_from_file_path, parse_config_file_text_to_json,
    parse_json_config_file_content, parse_json_source_file_config_file_content,
    tsconfig_to_source_file, ParsedCompilerOptions, ProjectReference, ResolveConfigHost,
    TsConfigSourceFile,
};
pub use wildcarddirectories::{get_wildcard_directories, is_implicit_glob, WildcardDirectoryWatcher};
