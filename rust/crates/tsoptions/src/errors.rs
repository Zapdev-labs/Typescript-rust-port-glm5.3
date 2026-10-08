// Port of tsc/internal/tsoptions/errors.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - `CreateDiagnosticForNodeInSourceFile` is the REAL positioned form
//     since the M6 config-node wave (the config file parses as a JSON
//     SourceFile — see tsconfigparsing.rs); `CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic`
//     degrades to the compiler-diagnostic form exactly on Go's nil
//     file/node inputs. `createDiagnosticForInvalidEnumType`'s M4 callers
//     pass `None` (the command-line path — Go passes nil there too).
//   - `createUnknownOptionError`'s alternateMode/optionsNameMap plumbing
//     needs the command-line parser's worker diagnostics, which live in
//     diagnostics.rs here.

use tsc_ast::{Diagnostic, NodeStore};
use tsc_core::text::TextRange;
use tsc_diagnostics::Message;

use crate::commandlineoption::{CommandLineOption, CommandLineOptionKind};

/// Go: `func formatEnumTypeKeys(opt *CommandLineOption, keys []string) string`.
pub fn format_enum_type_keys(opt: &CommandLineOption, keys: &[String]) -> String {
    let keys: Vec<&String> = match opt.deprecated_keys() {
        Some(deprecated) => keys
            .iter()
            .filter(|key| !deprecated.contains(&key.as_str()))
            .collect(),
        None => keys.iter().collect(),
    };
    format!("'{}'", keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join("', '"))
}

/// Go: `func getCompilerOptionValueTypeString(option *CommandLineOption) string`.
pub fn get_compiler_option_value_type_string(option: &CommandLineOption) -> String {
    match option.kind {
        CommandLineOptionKind::ListOrElement => format!(
            "{} or Array",
            get_compiler_option_value_type_string(
                option
                    .elements()
                    .expect("listOrElement option without element")
            )
        ),
        CommandLineOptionKind::List => "Array".to_string(),
        other => other.as_str().to_string(),
    }
}

/// Go: `func CreateDiagnosticForNodeInSourceFile(sourceFile, node, message, args...)`.
///
/// PORT (M6 config-node wave): now positioned like Go —
/// `scanner.SkipTrivia(text, node.Loc.Pos())` .. `node.End()` — where Go's
/// config-parse diagnostics carried no spans (the pre-AST-config M4 form),
/// this restores the exact node range against the config source file.
pub fn create_diagnostic_for_node_in_source_file(
    source_file: &tsc_ast::SourceFile,
    node: tsc_ast::NodeId,
    message: &'static Message,
    args: &[String],
) -> Diagnostic {
    let node = source_file.node(node);
    let loc = tsc_core::text::TextRange::new(
        tsc_scanner::skip_trivia(source_file.text(), node.loc.pos()),
        node.loc.end(),
    );
    let text = tsc_diagnostics::format(message.text(), args);
    Diagnostic::new(
        Some(source_file.as_node_id()),
        loc,
        message.code(),
        message.category(),
        "",
        Some(message),
        text,
        message.key(),
        args.to_vec(),
    )
}

/// Go: `func CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic(...)`.
pub fn create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
    source_file: Option<&tsc_ast::SourceFile>,
    node: Option<tsc_ast::NodeId>,
    message: &'static Message,
    args: &[String],
) -> Diagnostic {
    if let (Some(file), Some(node)) = (source_file, node) {
        return create_diagnostic_for_node_in_source_file(file, node, message, args);
    }
    new_compiler_diagnostic(message, args)
}

/// Go: `ast.NewCompilerDiagnostic(message, args...)`.
pub fn new_compiler_diagnostic(message: &'static Message, args: &[String]) -> Diagnostic {
    let text = tsc_diagnostics::format(message.text(), args);
    Diagnostic::new(
        None,
        TextRange::new(0, 0),
        message.code(),
        message.category(),
        "",
        Some(message),
        text,
        message.key(),
        args.to_vec(),
    )
}

/// Go: `func extraKeyDiagnostics(s string) *diagnostics.Message`.
pub fn extra_key_diagnostics(s: &str) -> Option<&'static Message> {
    match s {
        "compilerOptions" => Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0),
        "typeAcquisition" => Some(&tsc_diagnostics::UNKNOWN_TYPE_ACQUISITION_OPTION_0),
        "buildOptions" => Some(&tsc_diagnostics::UNKNOWN_BUILD_OPTION_0),
        _ => None,
    }
}

/// Go: `func extraKeyDidYouMeanDiagnostics(s string) *diagnostics.Message`.
pub fn extra_key_did_you_mean_diagnostics(s: &str) -> Option<&'static Message> {
    match s {
        "compilerOptions" => Some(&tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0_DID_YOU_MEAN_1),
        "typeAcquisition" => {
            Some(&tsc_diagnostics::UNKNOWN_TYPE_ACQUISITION_OPTION_0_DID_YOU_MEAN_1)
        }
        "buildOptions" => Some(&tsc_diagnostics::UNKNOWN_BUILD_OPTION_0_DID_YOU_MEAN_1),
        _ => None,
    }
}

/// Go: `func createUnknownOptionError(unknownOption, unknownOptionDiagnostic, unknownOptionErrorText, node, sourceFile, alternateMode, unknownDidYouMeanDiagnostic, optionsNameMap)`.
///
/// PORT: `node`/`sourceFile` are `None` on the command-line path this crate
/// serves in M4 (see module PORT notes); the alternate-mode and
/// spelling-suggestion arms are exact.
#[allow(clippy::too_many_arguments)]
pub fn create_unknown_option_error(
    unknown_option: &str,
    unknown_option_diagnostic: &'static Message,
    unknown_option_error_text: &str,
    source_file: Option<&tsc_ast::SourceFile>,
    node: Option<tsc_ast::NodeId>,
    alternate_mode: Option<&crate::diagnostics::AlternateModeDiagnostics>,
    unknown_did_you_mean_diagnostic: Option<&'static Message>,
    options_name_map: Option<&crate::namemap::NameMap>,
) -> Diagnostic {
    if let Some(alternate_mode) = alternate_mode {
        // Go: `alternateMode.optionsNameMap` (never nil in the two worker
        // bundles this port wires).
        let other_option = (alternate_mode.options_name_map)().get(unknown_option);
        if let Some(other_option) = other_option {
            // tscbuildoption
            let diagnostic = if other_option.name == "build" {
                &tsc_diagnostics::OPTION_BUILD_MUST_BE_THE_FIRST_COMMAND_LINE_ARGUMENT
            } else {
                alternate_mode.diagnostic
            };
            return create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                source_file,
                node,
                diagnostic,
                &[unknown_option.to_string()],
            );
        }
    }
    let unknown_option_error_text = if unknown_option_error_text.is_empty() {
        unknown_option
    } else {
        unknown_option_error_text
    };
    if let (Some(diagnostic), Some(map)) = (unknown_did_you_mean_diagnostic, options_name_map) {
        if let Some(possible_option) = map.get_spelling_suggestion(unknown_option) {
            return create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                source_file,
                node,
                diagnostic,
                &[
                    unknown_option_error_text.to_string(),
                    possible_option.name.to_string(),
                ],
            );
        }
    }
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        source_file,
        node,
        unknown_option_diagnostic,
        &[unknown_option_error_text.to_string()],
    )
}

/// Go: `func createDiagnosticForInvalidEnumType(opt, sourceFile, node)`.
pub fn create_diagnostic_for_invalid_enum_type(
    opt: &CommandLineOption,
    source_file: Option<&tsc_ast::SourceFile>,
    node: Option<tsc_ast::NodeId>,
) -> Diagnostic {
    let names_of_type: Vec<String> = opt
        .enum_map()
        .expect("enum option without map")
        .keys()
        .map(|k| k.to_string())
        .collect();
    let string_names = format_enum_type_keys(opt, &names_of_type);
    let opt_name = format!("--{}", opt.name);
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        source_file,
        node,
        &tsc_diagnostics::ARGUMENT_FOR_0_OPTION_MUST_BE_COLON_1,
        &[opt_name, string_names],
    )
}
