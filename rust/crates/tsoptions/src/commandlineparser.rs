// Port of tsc/internal/tsoptions/commandlineparser.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go's `any` option values are the closed `CompilerOptionsValue` enum
//     (commandlineoption.rs). List results (`[]any` of validated strings in
//     Go) surface as `StrArray`.
//   - `validateJsonOptionValue`/`convertJsonOptionOfEnumType` live in Go's
//     tsconfigparsing.go; they are re-homed here (next to their only M4
//     caller) with the `node`/`sourceFile` params dropped — the command-line
//     path never has AST nodes, and the AST-config path is future work (see
//     tsconfigparsing.rs PORT notes).
//   - `locale.Parse` (Go: internal/locale) has a Rust counterpart in the
//     `tsc-locale` crate; the locale extra-validation arm uses it directly.

use rustc_hash::FxHashSet;
use tsc_ast::Diagnostic;
use tsc_collections::OrderedMap;
use tsc_core::options_generated::{BuildOptions, CompilerOptions};
use tsc_diagnostics::Message;
use tsc_tspath::{
    to_rooted_file_path, CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath,
};

use crate::commandlineoption::{
    CommandLineOption, CommandLineOptionKind, CompilerOptionsValue, ExtraValidation,
};
use crate::diagnostics::{
    build_options_did_you_mean_diagnostics, compiler_options_did_you_mean_diagnostics,
    ParseCommandLineWorkerDiagnostics,
};
use crate::errors::{
    create_diagnostic_for_invalid_enum_type,
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic, create_unknown_option_error,
    get_compiler_option_value_type_string, new_compiler_diagnostic,
};
use crate::namemap::{build_name_map, compiler_name_map, NameMap};
use crate::parsedbuildcommandline::ParsedBuildCommandLine;
use crate::parsedcommandline::ParsedCommandLine;
use crate::options_generated::parse_compiler_options;
use crate::parsinghelpers::{
    convert_map_to_options, convert_to_options_with_absolute_paths, CompilerOptionsParser,
};

/// Go: `type commandLineParser struct`.
pub struct CommandLineParser<'a> {
    worker_diagnostics: &'static ParseCommandLineWorkerDiagnostics,
    /// Go: `optionsMap *NameMap` (built from the worker's declarations).
    pub options_map: &'static NameMap,
    fs: &'a dyn tsc_vfs::Fs,
    current_directory: RootedDirectoryPath,
    /// Go: `options *collections.OrderedMap[string, any]`.
    pub options: OrderedMap<String, CompilerOptionsValue>,
    /// Go: `fileNames []string`.
    pub file_names: Vec<String>,
    /// Go: `errors []*ast.Diagnostic`.
    pub errors: Vec<Diagnostic>,
    /// Go: `responseFileStack collections.Set[tspath.PathKey]`.
    response_file_stack: FxHashSet<PathKey>,
}

impl<'a> CommandLineParser<'a> {
    /// Go: the `parseCommandLineWorker` constructor half (the parse itself is
    /// `parse_strings`).
    pub fn new(
        parse_command_line_with_diagnostics: &'static ParseCommandLineWorkerDiagnostics,
        fs: &'a dyn tsc_vfs::Fs,
        current_directory: RootedDirectoryPath,
    ) -> Self {
        CommandLineParser {
            worker_diagnostics: parse_command_line_with_diagnostics,
            options_map: parse_command_line_with_diagnostics.name_map(),
            fs,
            current_directory,
            options: OrderedMap::new(),
            file_names: Vec::new(),
            errors: Vec::new(),
            response_file_stack: FxHashSet::default(),
        }
    }

    /// Go: `func (p *commandLineParser) AlternateMode()`.
    fn alternate_mode(&self) -> Option<&crate::diagnostics::AlternateModeDiagnostics> {
        self.worker_diagnostics.did_you_mean.alternate_mode.as_ref()
    }

    /// Go: `func (p *commandLineParser) UnknownOptionDiagnostic()`.
    fn unknown_option_diagnostic(&self) -> &'static Message {
        self.worker_diagnostics.did_you_mean.unknown_option_diagnostic
    }

    /// Go: `func (p *commandLineParser) UnknownDidYouMeanDiagnostic()`.
    fn unknown_did_you_mean_diagnostic(&self) -> &'static Message {
        self.worker_diagnostics
            .did_you_mean
            .unknown_did_you_mean_diagnostic
    }

    /// Go: `func (p *commandLineParser) createUnknownOptionError(...)`.
    fn create_unknown_option_error(
        &self,
        unknown_option: &str,
        unknown_option_error_text: &str,
    ) -> Diagnostic {
        create_unknown_option_error(
            unknown_option,
            self.unknown_option_diagnostic(),
            unknown_option_error_text,
            None,
            None,
            self.alternate_mode(),
            Some(self.unknown_did_you_mean_diagnostic()),
            Some(self.options_map),
        )
    }

    /// Go: `func (p *commandLineParser) parseStrings(args []string)`.
    pub fn parse_strings(&mut self, args: &[String]) {
        let mut i = 0;
        while i < args.len() {
            let s = args[i].clone();
            i += 1;
            if s.is_empty() {
                continue;
            }
            match s.as_bytes()[0] {
                b'@' => {
                    let file_name = s[1..].to_string();
                    self.parse_response_file(&file_name);
                }
                b'-' => {
                    let input_option_name = get_input_option_name(&s);
                    if let Some(opt) =
                        self.options_map.get_option_declaration_from_name(&input_option_name, true)
                    {
                        i = self.parse_option_value(
                            args,
                            i,
                            opt,
                            self.worker_diagnostics.option_type_mismatch_diagnostic,
                        );
                    } else {
                        self.errors
                            .push(self.create_unknown_option_error(&input_option_name, &s));
                    }
                }
                _ => self.file_names.push(s),
            }
        }
    }

    /// Go: `func (p *commandLineParser) parseResponseFile(fileName string)`.
    fn parse_response_file(&mut self, file_name: &str) {
        let typed_file_name = if !file_name.is_empty() {
            to_rooted_file_path(file_name, &self.current_directory)
        } else {
            tsc_tspath::rooted_file_path_from_path(self.current_directory.as_path())
        };
        let path = self
            .fs
            .case_sensitivity()
            .path_key(&typed_file_name.as_path());
        if self.response_file_stack.contains(&path) {
            return;
        }
        self.response_file_stack.insert(path.clone());
        self.parse_response_file_body(file_name, &typed_file_name);
        self.response_file_stack.remove(&path);
    }

    /// The body of `parseResponseFile` after the cycle check — split so the
    /// stack removal runs after the recursive `parse_strings` (Go: `defer`).
    fn parse_response_file_body(&mut self, file_name: &str, typed_file_name: &RootedFilePath) {
        let (file_contents, mut errors) = try_read_file(
            typed_file_name,
            &mut |path| self.fs.read_file(path),
            std::mem::take(&mut self.errors),
        );

        if file_contents.is_empty() {
            self.errors = errors;
            return;
        }

        let mut args: Vec<String> = Vec::new();
        // Go: `text := []rune(fileContents)` — rune-wise scanning.
        let text: Vec<char> = file_contents.chars().collect();
        let text_length = text.len();
        let mut pos = 0;
        while pos < text_length {
            while pos < text_length && text[pos] <= ' ' {
                pos += 1;
            }
            if pos >= text_length {
                break;
            }
            let start = pos;
            if text[pos] == '"' {
                pos += 1;
                while pos < text_length && text[pos] != '"' {
                    pos += 1;
                }
                if pos < text_length {
                    args.push(text[start + 1..pos].iter().collect());
                    pos += 1;
                } else {
                    errors.push(new_compiler_diagnostic(
                        &tsc_diagnostics::UNTERMINATED_QUOTED_STRING_IN_RESPONSE_FILE_0,
                        &[file_name.to_string()],
                    ));
                }
            } else {
                while pos < text_length && text[pos] > ' ' {
                    pos += 1;
                }
                args.push(text[start..pos].iter().collect());
            }
        }
        self.errors.append(&mut errors);
        self.parse_strings(&args);
    }

    /// Go: `func (p *commandLineParser) parseOptionValue(args, i, opt, diag) int`.
    fn parse_option_value(
        &mut self,
        args: &[String],
        mut i: usize,
        opt: &'static CommandLineOption,
        diag: &'static Message,
    ) -> usize {
        if opt.is_tsconfig_only && i <= args.len() {
            let opt_value = if i < args.len() {
                args[i].as_str()
            } else {
                ""
            };
            if opt_value == "null" {
                self.options
                    .set(opt.name.to_string(), CompilerOptionsValue::Null);
                i += 1;
            } else if opt.kind == CommandLineOptionKind::Boolean {
                if opt_value == "false" {
                    self.options
                        .set(opt.name.to_string(), CompilerOptionsValue::Bool(false));
                    i += 1;
                } else {
                    if opt_value == "true" {
                        i += 1;
                    }
                    self.errors.push(new_compiler_diagnostic(
                        &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_SPECIFIED_IN_TSCONFIG_JSON_FILE_OR_SET_TO_FALSE_OR_NULL_ON_COMMAND_LINE,
                        &[opt.name.to_string()],
                    ));
                }
            } else {
                self.errors.push(new_compiler_diagnostic(
                    &tsc_diagnostics::OPTION_0_CAN_ONLY_BE_SPECIFIED_IN_TSCONFIG_JSON_FILE_OR_SET_TO_NULL_ON_COMMAND_LINE,
                    &[opt.name.to_string()],
                ));
                if !opt_value.is_empty() && !opt_value.starts_with('-') {
                    i += 1;
                }
            }
        } else {
            // Check to see if no argument was provided (e.g. "--locale" is
            // the last command-line argument).
            if i >= args.len() {
                if opt.kind != CommandLineOptionKind::Boolean {
                    self.errors.push(new_compiler_diagnostic(
                        diag,
                        &[
                            opt.name.to_string(),
                            get_compiler_option_value_type_string(opt),
                        ],
                    ));
                    if opt.kind == CommandLineOptionKind::List {
                        self.options.set(
                            opt.name.to_string(),
                            CompilerOptionsValue::StrArray(Vec::new()),
                        );
                    } else if opt.kind == CommandLineOptionKind::Enum {
                        self.errors
                            .push(create_diagnostic_for_invalid_enum_type(opt, None, None));
                    }
                } else {
                    self.options
                        .set(opt.name.to_string(), CompilerOptionsValue::Bool(true));
                }
                return i;
            }
            if args[i] != "null" {
                match opt.kind {
                    CommandLineOptionKind::Number => {
                        // !!! Make sure this parseInt matches JS parseInt
                        match args[i].parse::<i64>() {
                            Ok(num) => {
                                if num >= opt.min_value {
                                    self.options
                                        .set(opt.name.to_string(), CompilerOptionsValue::Int(num));
                                } else {
                                    self.errors.push(new_compiler_diagnostic(
                                        &tsc_diagnostics::OPTION_0_REQUIRES_VALUE_TO_BE_GREATER_THAN_1,
                                        &[opt.name.to_string(), opt.min_value.to_string()],
                                    ));
                                }
                            }
                            Err(_) => {
                                self.errors.push(new_compiler_diagnostic(
                                    diag,
                                    &[opt.name.to_string(), "number".to_string()],
                                ));
                            }
                        }
                        i += 1;
                    }
                    CommandLineOptionKind::Boolean => {
                        // boolean flag has optional value true, false, others
                        let opt_value = args[i].as_str();

                        // check next argument as boolean flag value
                        if opt_value == "false" {
                            self.options
                                .set(opt.name.to_string(), CompilerOptionsValue::Bool(false));
                        } else {
                            self.options
                                .set(opt.name.to_string(), CompilerOptionsValue::Bool(true));
                        }
                        // try to consume next argument as value for boolean
                        // flag; do not consume argument if it is not "true"
                        // or "false"
                        if opt_value == "false" || opt_value == "true" {
                            i += 1;
                        }
                    }
                    CommandLineOptionKind::String => {
                        let (value, new_errors) = validate_json_option_value(
                            opt,
                            CompilerOptionsValue::Str(args[i].clone()),
                        );
                        if new_errors.is_empty() {
                            if let Some(value) = value {
                                self.options.set(opt.name.to_string(), value);
                            }
                        } else {
                            self.errors.extend(new_errors);
                        }
                        i += 1;
                    }
                    CommandLineOptionKind::List => {
                        let (result, new_errors) = self.parse_list_type_option(opt, &args[i]);
                        let consumed = match &result {
                            CompilerOptionsValue::StrArray(values) => !values.is_empty(),
                            _ => true,
                        } || !new_errors.is_empty();
                        self.options.set(opt.name.to_string(), result);
                        self.errors.extend(new_errors);
                        if consumed {
                            i += 1;
                        }
                    }
                    CommandLineOptionKind::ListOrElement => {
                        // If not a primitive, the possible types are
                        // specified in what is effectively a map of options.
                        panic!("listOrElement not supported here");
                    }
                    // enum (Go: the default arm).
                    _ => {
                        let trimmed: String = args[i]
                            .trim_matches(tsc_stringutil::is_white_space_like)
                            .to_string();
                        let (value, new_errors) =
                            convert_json_option_of_enum_type(opt, &trimmed);
                        self.options.set(opt.name.to_string(), value);
                        self.errors.extend(new_errors);
                        i += 1;
                    }
                }
            } else {
                self.options
                    .set(opt.name.to_string(), CompilerOptionsValue::Null);
                i += 1;
            }
        }
        i
    }

    /// Go: `func (p *commandLineParser) parseListTypeOption(opt, value)`.
    fn parse_list_type_option(
        &mut self,
        opt: &'static CommandLineOption,
        value: &str,
    ) -> (CompilerOptionsValue, Vec<Diagnostic>) {
        parse_list_type_option(opt, value)
    }
}

/// Go: `func ParseCommandLine(commandLine, fs, currentDirectory) *ParsedCommandLine`.
pub fn parse_command_line(
    command_line: &[String],
    fs: &dyn tsc_vfs::Fs,
    current_directory: &RootedDirectoryPath,
) -> ParsedCommandLine {
    let mut parser = CommandLineParser::new(
        compiler_options_did_you_mean_diagnostics(),
        fs,
        current_directory.clone(),
    );
    parser.parse_strings(command_line);

    let mut options = parser.options.clone();
    convert_to_options_with_absolute_paths(
        &mut options,
        crate::namemap::command_line_compiler_options_map(),
        current_directory,
    );
    // Go: convertMapToOptions(options, &compilerOptionsParser{...}).
    let mut compiler_options = CompilerOptions::default();
    {
        let mut parser_impl = CompilerOptionsParser {
            compiler_options: &mut compiler_options,
            unresolved_paths: None,
        };
        convert_map_to_options(&options, &mut parser_impl);
    }

    let case_sensitivity: CaseSensitivity = fs.case_sensitivity();
    let mut result = ParsedCommandLine::new(
        RootedFilePath::default(),
        current_directory.clone(),
        case_sensitivity,
    );
    result.compiler_options.options = compiler_options;
    result
        .file_names
        .extend(parser.file_names.iter().map(|file_name| {
            to_rooted_file_path(file_name, current_directory)
        }));
    result.root_file_names_for_diagnostics = parser.file_names.clone();
    result.errors = parser.errors;
    result.raw = parser.options.clone();
    result
}

/// Go: `func ParseBuildCommandLine(commandLine, fs, currentDirectory) *ParsedBuildCommandLine`.
pub fn parse_build_command_line(
    command_line: &[String],
    fs: &dyn tsc_vfs::Fs,
    current_directory: &RootedDirectoryPath,
) -> ParsedBuildCommandLine {
    let mut parser = CommandLineParser::new(
        build_options_did_you_mean_diagnostics(),
        fs,
        current_directory.clone(),
    );
    parser.parse_strings(command_line);

    let mut compiler_options = CompilerOptions::default();
    for (key, value) in parser.options.entries() {
        let build_option = build_name_map().get(key);
        let is_tsc_build_option = build_option.is_some_and(|option| {
            std::ptr::eq(option, &crate::declarations_generated::TSC_BUILD_OPTION)
        });
        let is_compiler_option = compiler_name_map().get(key).is_some();
        if is_tsc_build_option || is_compiler_option {
            parse_compiler_options(key, value, &mut compiler_options);
        }
    }

    // Go: convertMapToOptions(parser.options, &buildOptionsParser{...}).
    let mut build_options = BuildOptions::default();
    {
        let mut parser_impl = crate::parsinghelpers::BuildOptionsParser {
            build_options: &mut build_options,
        };
        convert_map_to_options(&parser.options, &mut parser_impl);
    }

    let mut result = ParsedBuildCommandLine::new(
        build_options,
        compiler_options,
        parser.file_names.clone(),
        parser.errors,
        parser.options.clone(),
        current_directory.clone(),
    );

    if result.projects.is_empty() {
        // tsc -b invoked with no extra arguments; act as if invoked with "tsc -b ."
        result.projects.push(".".to_string());
    }

    // Nonsensical combinations
    if result.build_options.clean.is_true() && result.build_options.force.is_true() {
        result.errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::OPTIONS_0_AND_1_CANNOT_BE_COMBINED,
            &["clean".to_string(), "force".to_string()],
        ));
    }
    if result.build_options.clean.is_true() && result.build_options.verbose.is_true() {
        result.errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::OPTIONS_0_AND_1_CANNOT_BE_COMBINED,
            &["clean".to_string(), "verbose".to_string()],
        ));
    }
    if result.build_options.clean.is_true() && result.compiler_options.watch.is_true() {
        result.errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::OPTIONS_0_AND_1_CANNOT_BE_COMBINED,
            &["clean".to_string(), "watch".to_string()],
        ));
    }
    if result.compiler_options.watch.is_true() && result.build_options.dry.is_true() {
        result.errors.push(new_compiler_diagnostic(
            &tsc_diagnostics::OPTIONS_0_AND_1_CANNOT_BE_COMBINED,
            &["watch".to_string(), "dry".to_string()],
        ));
    }

    result
}

/// Go: `func getInputOptionName(input string) string` — removes at most two
/// leading '-' from the input string.
fn get_input_option_name(input: &str) -> String {
    let mut stripped = input;
    for _ in 0..2 {
        if let Some(rest) = stripped.strip_prefix('-') {
            stripped = rest;
        } else {
            break;
        }
    }
    stripped.to_string()
}

/// Go: `func tryReadFile(fileName, readFile, errors) (string, []*ast.Diagnostic)` —
/// adds a compiler diagnostic if the file cannot be read.
fn try_read_file(
    file_name: &RootedFilePath,
    read_file: &mut dyn FnMut(&RootedFilePath) -> Option<String>,
    mut errors: Vec<Diagnostic>,
) -> (String, Vec<Diagnostic>) {
    match read_file(file_name) {
        Some(text) => (text, errors),
        None => {
            // !!! Divergence: the returned error will not give a useful message
            errors.push(new_compiler_diagnostic(
                &tsc_diagnostics::CANNOT_READ_FILE_0,
                &[file_name.as_string().to_string()],
            ));
            (String::new(), errors)
        }
    }
}

/// Go: `func ParseListTypeOption(opt *CommandLineOption, value string) ([]any, []*ast.Diagnostic)`.
///
/// The list elements surface as `StrArray` (Go's `[]any` holds validated
/// strings here — see module PORT notes).
pub fn parse_list_type_option(
    opt: &'static CommandLineOption,
    value: &str,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    let value = value.trim();
    let mut errors: Vec<Diagnostic> = Vec::new();
    if value.starts_with('-') {
        return (CompilerOptionsValue::StrArray(Vec::new()), errors);
    }
    if opt.kind == CommandLineOptionKind::ListOrElement && !value.contains(',') {
        let (val, err) =
            validate_json_option_value(opt, CompilerOptionsValue::Str(value.to_string()));
        if !err.is_empty() {
            return (CompilerOptionsValue::StrArray(Vec::new()), err);
        }
        let CompilerOptionsValue::Str(val) = val.unwrap_or(CompilerOptionsValue::Null) else {
            return (CompilerOptionsValue::StrArray(Vec::new()), errors);
        };
        return (CompilerOptionsValue::StrArray(vec![val]), errors);
    }
    if value.is_empty() {
        return (CompilerOptionsValue::StrArray(Vec::new()), errors);
    }
    let values: Vec<&str> = value.split(',').collect();
    let element = opt.elements().expect("list option without element");
    match element.kind {
        CommandLineOptionKind::String => {
            let mut elements: Vec<String> = Vec::new();
            for v in &values {
                let (val, err) =
                    validate_json_option_value(element, CompilerOptionsValue::Str((*v).to_string()));
                if let CompilerOptionsValue::Str(s) = val.unwrap_or(CompilerOptionsValue::Null) {
                    if err.is_empty() && !s.is_empty() {
                        elements.push(s);
                        continue;
                    }
                }
                errors.extend(err);
            }
            (CompilerOptionsValue::StrArray(elements), errors)
        }
        CommandLineOptionKind::Boolean
        | CommandLineOptionKind::Object
        | CommandLineOptionKind::Number => {
            // do nothing: only string and enum/object types currently allowed
            // as list entries. !!! we don't actually have number list
            // options, so I didn't implement number list parsing.
            panic!("List of {} is not yet supported.", element.kind.as_str());
        }
        _ => {
            let mut result: Vec<String> = Vec::new();
            for v in &values {
                let trimmed: String = v
                    .trim_matches(tsc_stringutil::is_white_space_like)
                    .to_string();
                let (val, err) = convert_json_option_of_enum_type(element, &trimmed);
                if let CompilerOptionsValue::Str(s) = val {
                    if err.is_empty() && !s.is_empty() {
                        result.push(s);
                        continue;
                    }
                }
                errors.extend(err);
            }
            (CompilerOptionsValue::StrArray(result), errors)
        }
    }
}

/// Go: `func convertJsonOptionOfEnumType(opt, value, valueExpression, sourceFile) (any, []*ast.Diagnostic)`.
///
/// PORT: the AST params are dropped (command-line path only in this wave).
pub fn convert_json_option_of_enum_type(
    opt: &'static CommandLineOption,
    value: &str,
) -> (CompilerOptionsValue, Vec<Diagnostic>) {
    if value.is_empty() {
        return (CompilerOptionsValue::Null, Vec::new());
    }
    let key = value.to_ascii_lowercase();
    let Some(type_map) = opt.enum_map() else {
        return (CompilerOptionsValue::Null, Vec::new());
    };
    if let Some(val) = type_map.get(&key) {
        let (value, errors) = validate_json_option_value(opt, val.clone());
        return (value.unwrap_or(CompilerOptionsValue::Null), errors);
    }
    (
        CompilerOptionsValue::Null,
        vec![create_diagnostic_for_invalid_enum_type(opt, None, None)],
    )
}

/// Go: `func validateJsonOptionValue(opt, val, valueExpression, sourceFile) (any, []*ast.Diagnostic)`.
///
/// PORT: the AST params are dropped (command-line path only in this wave).
pub fn validate_json_option_value(
    opt: &'static CommandLineOption,
    val: CompilerOptionsValue,
) -> (Option<CompilerOptionsValue>, Vec<Diagnostic>) {
    if val.is_null() {
        return (None, Vec::new());
    }

    let mut errors: Vec<Diagnostic> = Vec::new();

    match opt.extra_validation {
        ExtraValidation::Locale => {
            let CompilerOptionsValue::Str(locale_str) = &val else {
                return (None, Vec::new());
            };
            if !tsc_locale::parse(locale_str).1 {
                errors.push(create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                    None,
                    None,
                    &tsc_diagnostics::LOCALE_MUST_BE_AN_IETF_BCP_47_LANGUAGE_TAG_EXAMPLES_COLON_0_1,
                    &["en".to_string(), "ja-jp".to_string()],
                ));
            }
        }
        ExtraValidation::None => {}
    }

    if !errors.is_empty() {
        return (None, errors);
    }
    (Some(val), errors)
}
