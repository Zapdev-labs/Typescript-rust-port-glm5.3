// Port of tsc/internal/tsoptions/errors.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - The node-positioned diagnostics (`createDiagnosticForInvalidEnumType`,
//     `CreateDiagnosticForNodeInSourceFile`) need an ast SourceFile/Node; in
//     the M4 wave the config-parse path parses JSON text directly (see
//     tsconfigparsing.rs PORT notes), so the node-positioned arm of
//     `CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic` is served by
//     the compiler-diagnostic form until the AST-config path lands.
//   - `createUnknownOptionError`'s alternateMode/optionsNameMap plumbing
//     needs the command-line parser's worker diagnostics, which live in
//     diagnostics.rs here.

use tsc_ast::Diagnostic;
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
/// PORT: node positions require the AST config path; the compiler-diagnostic
/// form is used until that lands (see module PORT notes).
pub fn create_diagnostic_for_node_in_source_file(
    _source_file: &tsc_ast::SourceFile,
    _node: tsc_ast::NodeId,
    message: &'static Message,
    args: &[String],
) -> Diagnostic {
    new_compiler_diagnostic(message, args)
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
