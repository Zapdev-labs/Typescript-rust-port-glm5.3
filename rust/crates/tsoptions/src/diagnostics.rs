// Port of tsc/internal/tsoptions/diagnostics.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The did-you-mean diagnostic bundles for the two parser modes (compiler
// options vs build options). The Go file wires them at package init; the
// port exposes them as `OnceLock` accessors over the same tables.

use std::sync::OnceLock;

use tsc_diagnostics::Message;

use crate::commandlineoption::CommandLineOption;
use crate::namemap::{build_name_map, compiler_name_map, NameMap};

/// Go: `type DidYouMeanOptionsDiagnostics struct`.
#[derive(Clone)]
pub struct DidYouMeanOptionsDiagnostics {
    pub alternate_mode: Option<AlternateModeDiagnostics>,
    pub option_declarations: &'static [&'static CommandLineOption],
    pub unknown_option_diagnostic: &'static Message,
    pub unknown_did_you_mean_diagnostic: &'static Message,
}

/// Go: `type AlternateModeDiagnostics struct`.
#[derive(Clone)]
pub struct AlternateModeDiagnostics {
    pub diagnostic: &'static Message,
    /// Go: `optionsNameMap *NameMap` — lazily built in the port.
    pub options_name_map: fn() -> &'static NameMap,
}

/// Go: `type ParseCommandLineWorkerDiagnostics struct`.
pub struct ParseCommandLineWorkerDiagnostics {
    pub did_you_mean: DidYouMeanOptionsDiagnostics,
    /// Go: `optionsNameMap *NameMap` + `sync.Once` — lazy in both languages.
    pub options_name_map: fn() -> &'static NameMap,
    pub option_type_mismatch_diagnostic: &'static Message,
}

impl ParseCommandLineWorkerDiagnostics {
    /// Go: the `optionsNameMap` lazy initializer.
    pub fn name_map(&self) -> &'static NameMap {
        (self.options_name_map)()
    }
}

/// Go: `var CompilerOptionsDidYouMeanDiagnostics = getParseCommandLineWorkerDiagnostics(OptionsDeclarations)`.
pub fn compiler_options_did_you_mean_diagnostics() -> &'static ParseCommandLineWorkerDiagnostics {
    static DIAGNOSTICS: OnceLock<ParseCommandLineWorkerDiagnostics> = OnceLock::new();
    DIAGNOSTICS.get_or_init(|| {
        ParseCommandLineWorkerDiagnostics {
            did_you_mean: DidYouMeanOptionsDiagnostics {
                alternate_mode: Some(AlternateModeDiagnostics {
                    diagnostic: &tsc_diagnostics::COMPILER_OPTION_0_MAY_ONLY_BE_USED_WITH_BUILD,
                    options_name_map: build_name_map,
                }),
                option_declarations: compiler_decls(),
                unknown_option_diagnostic: &tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0,
                unknown_did_you_mean_diagnostic:
                    &tsc_diagnostics::UNKNOWN_COMPILER_OPTION_0_DID_YOU_MEAN_1,
            },
            options_name_map: compiler_name_map,
            option_type_mismatch_diagnostic: &tsc_diagnostics::COMPILER_OPTION_0_EXPECTS_AN_ARGUMENT,
        }
    })
}

/// Go: `var buildOptionsDidYouMeanDiagnostics = ...`.
pub fn build_options_did_you_mean_diagnostics() -> &'static ParseCommandLineWorkerDiagnostics {
    static DIAGNOSTICS: OnceLock<ParseCommandLineWorkerDiagnostics> = OnceLock::new();
    DIAGNOSTICS.get_or_init(|| {
        ParseCommandLineWorkerDiagnostics {
            did_you_mean: DidYouMeanOptionsDiagnostics {
                alternate_mode: Some(AlternateModeDiagnostics {
                    diagnostic: &tsc_diagnostics::COMPILER_OPTION_0_MAY_NOT_BE_USED_WITH_BUILD,
                    options_name_map: compiler_name_map,
                }),
                option_declarations: build_decls(),
                unknown_option_diagnostic: &tsc_diagnostics::UNKNOWN_BUILD_OPTION_0,
                unknown_did_you_mean_diagnostic:
                    &tsc_diagnostics::UNKNOWN_BUILD_OPTION_0_DID_YOU_MEAN_1,
            },
            options_name_map: build_name_map,
            option_type_mismatch_diagnostic: &tsc_diagnostics::BUILD_OPTION_0_REQUIRES_A_VALUE_OF_TYPE_1,
        }
    })
}

fn compiler_decls() -> &'static [&'static CommandLineOption] {
    static DECLS: OnceLock<&'static [&'static CommandLineOption]> = OnceLock::new();
    DECLS.get_or_init(|| {
        Box::leak(crate::declarations_generated::options_declarations().into_boxed_slice())
    })
}

fn build_decls() -> &'static [&'static CommandLineOption] {
    static DECLS: OnceLock<&'static [&'static CommandLineOption]> = OnceLock::new();
    DECLS.get_or_init(|| {
        let mut all: Vec<&'static CommandLineOption> =
            crate::declarations_generated::COMMON_OPTIONS_WITH_BUILD.to_vec();
        all.extend_from_slice(crate::declarations_generated::OPTIONS_FOR_BUILD);
        Box::leak(all.into_boxed_slice())
    })
}
