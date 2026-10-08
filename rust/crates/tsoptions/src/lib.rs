// Port of tsc/internal/tsoptions (M4 wave C) @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The generated tables (options_generated.rs, declarations_generated.rs) are
// emitted by rust/tools/gen-options from the mechanically extracted
// tsoptions-model.json (see gen-options/data/README.md). The hand-written
// modules here carry the parsing/serializing logic around them; per-file
// PORT notes list the M4-scope deltas (see PORTING-NOTES.md for the roll-up).
//
// Not yet ported from the Go package (tracked in PORTING-NOTES.md):
//   - commandlineparser.go / parsedcommandline.go's watch machinery
//   - tsconfigparsing.go's AST-based JSON conversion (spans in diagnostics)
//     and getFileNamesFromConfigSpecs (glob-driven file enumeration)
//   - the full vfsmatch SpecMatcher (tsc-vfs wave)
//   - contentmapper's host.go/hostimpl.go (out of M4 scope)

pub mod commandlineoption;
pub mod declarations_generated;
pub mod diagnostics;
pub mod enummaps;
pub mod errors;
pub mod namemap;
pub mod options_generated;
pub mod parsedcommandline;
pub mod parsinghelpers;
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
pub use errors::new_compiler_diagnostic;
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
pub use parsedcommandline::ParsedCommandLine;
pub use parsinghelpers::{
    parse_content_mapper, parse_project_reference, parse_string, parse_string_array,
    parse_string_array_strict, parse_tristate, CompilerOptionsParser,
};
pub use showconfig::{convert_to_tsconfig, TSConfig};
pub use tsconfigparsing::{
    get_parsed_command_line_of_config_file_path, parse_config_file_text_to_json,
    parse_json_config_file_content, ParsedCompilerOptions, ProjectReference, ResolveConfigHost,
};
pub use wildcarddirectories::{get_wildcard_directories, is_implicit_glob, WildcardDirectoryWatcher};
