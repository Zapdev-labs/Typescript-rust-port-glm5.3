// Ported from tsc/internal/compiler/processingDiagnostic.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::collections::HashSet;
use std::sync::Arc;

use tsc_ast::Diagnostic;
use tsc_tspath::PathKey;

use crate::diagnostics_collection::FileDiagnostic;
use crate::file_include::{FileIncludeKind, FileIncludeReason};
use crate::program::Program;

/// Go: `type processingDiagnosticKind int` + consts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum ProcessingDiagnosticKind {
    /// Go: `processingDiagnosticKindUnknownReference`.
    UnknownReference = 0,
    /// Go: `processingDiagnosticKindExplainingFileInclude`.
    ExplainingFileInclude = 1,
}

/// Go: `type processingDiagnostic struct`.
#[derive(Clone, Debug)]
pub struct ProcessingDiagnostic {
    pub kind: ProcessingDiagnosticKind,
    pub reason: Option<Arc<FileIncludeReason>>,
    pub explanation: Option<IncludeExplainingDiagnostic>,
}

impl ProcessingDiagnostic {
    /// Go: `func (d *processingDiagnostic) asFileIncludeReason()`.
    pub fn as_file_include_reason(&self) -> Option<&Arc<FileIncludeReason>> {
        self.reason.as_ref()
    }

    /// Go: `func (d *processingDiagnostic) asIncludeExplainingDiagnostic()`.
    pub fn as_include_explaining_diagnostic(&self) -> Option<&IncludeExplainingDiagnostic> {
        self.explanation.as_ref()
    }

    /// Go: `func (d *processingDiagnostic) toDiagnostic(program *Program)`.
    pub fn to_diagnostic(&self, program: &Program) -> FileDiagnostic {
        match self.kind {
            ProcessingDiagnosticKind::UnknownReference => {
                let reference = self.as_file_include_reason().expect("reason");
                let loc = program
                    .include_processor
                    .get_reference_location(reference, program);
                let file = &program.files[loc.file];
                match reference.kind {
                    FileIncludeKind::TypeReferenceDirective => loc.diagnostic_at(
                        file,
                        &tsc_diagnostics::CANNOT_FIND_TYPE_DEFINITION_FILE_FOR_0,
                        &[loc.r#ref.as_ref().expect("ref").file_name.clone()],
                    ),
                    FileIncludeKind::LibReferenceDirective => {
                        let lib_name = tsc_tspath::to_file_name_lower_case(
                            loc.r#ref.as_ref().expect("ref").file_name.as_str(),
                        );
                        let unqualified_lib_name = lib_name
                            .strip_prefix("lib.")
                            .unwrap_or(&lib_name)
                            .strip_suffix(".d.ts")
                            .unwrap_or_default()
                            .to_string();
                        let suggestion = tsc_scanner::go_shims::get_spelling_suggestion_for_strings(
                            &unqualified_lib_name,
                            tsc_tsoptions::libs().iter().map(String::as_str),
                        );
                        let message = if !suggestion.is_empty() {
                            &tsc_diagnostics::CANNOT_FIND_LIB_DEFINITION_FOR_0_DID_YOU_MEAN_1
                        } else {
                            &tsc_diagnostics::CANNOT_FIND_LIB_DEFINITION_FOR_0
                        };
                        loc.diagnostic_at(
                            file,
                            message,
                            &[lib_name.to_string(), suggestion.to_string()],
                        )
                    }
                    _ => panic!("unknown include kind"),
                }
            }
            ProcessingDiagnosticKind::ExplainingFileInclude => {
                self.create_diagnostic_explaining_file(program)
            }
        }
    }

    /// Go: `func (d *processingDiagnostic) createDiagnosticExplainingFile(program *Program)`.
    fn create_diagnostic_explaining_file(&self, program: &Program) -> FileDiagnostic {
        use std::collections::HashMap;

        let diag = self.as_include_explaining_diagnostic().expect("explanation");
        let mut include_details: Vec<Diagnostic> = Vec::new();
        let mut related_info: Vec<Diagnostic> = Vec::new();
        let mut redirect_info: Vec<Diagnostic> = Vec::new();
        let mut preferred_location: Option<Arc<FileIncludeReason>> = None;
        let mut seen_reasons: HashSet<usize> = HashSet::new();

        if diag
            .diagnostic_reason
            .as_ref()
            .is_some_and(|reason| reason.is_referenced_file())
            && !program
                .include_processor
                .get_reference_location(diag.diagnostic_reason.as_ref().expect("reason"), program)
                .is_synthetic
        {
            preferred_location = diag.diagnostic_reason.clone();
        }

        let mut process_related_info = |preferred_location: &mut Option<Arc<FileIncludeReason>>,
                                        include_reason: &Arc<FileIncludeReason>| {
            if preferred_location.is_none()
                && include_reason.is_referenced_file()
                && !program
                    .include_processor
                    .get_reference_location(include_reason, program)
                    .is_synthetic
            {
                *preferred_location = Some(include_reason.clone());
            } else if preferred_location.as_ref().map(Arc::as_ptr) != Some(Arc::as_ptr(include_reason)) {
                if let Some(info) = program.include_processor.get_related_info(include_reason, program) {
                    related_info.push(info);
                }
            }
        };
        // Go's processInclude closure (seenReasons dedup +
        // toDiagnostic + processRelatedInfo) is folded into the two loops
        // below (the port's borrows do not admit the shared closure).

        // !!! todo sheetal caching — not ported (Go TODO).

        if !diag.file.as_string().is_empty() {
            let reasons = program
                .file_include_reasons
                .get(&diag.file)
                .cloned()
                .unwrap_or_default();
            include_details = Vec::with_capacity(reasons.len());
            for reason in &reasons {
                // Go: processInclude(reason).
                if !seen_reasons.insert(Arc::as_ptr(reason) as usize) {
                    continue;
                }
                include_details.push(reason.to_diagnostic(program, false, Default::default()).diagnostic);
                process_related_info(&mut preferred_location, reason);
            }
            redirect_info = program.include_processor.explain_redirect_and_implied_format(
                program,
                &diag.file,
                // Go: func(fileName tspath.RootedFilePath) string { return fileName.AsString() }
                &|file_name: &tsc_tspath::RootedFilePath| file_name.as_string().to_string(),
            );
        }
        if let Some(diagnostic_reason) = &diag.diagnostic_reason {
            // Go: processInclude(diag.diagnosticReason).
            if seen_reasons.insert(Arc::as_ptr(diagnostic_reason) as usize) {
                include_details.push(
                    diagnostic_reason
                        .to_diagnostic(program, false, Default::default())
                        .diagnostic,
                );
                process_related_info(&mut preferred_location, diagnostic_reason);
            }
        }

        let mut chain: Vec<Diagnostic> = Vec::new();
        if !include_details.is_empty()
            && (preferred_location.is_none() || seen_reasons.len() != 1)
        {
            let mut file_reason =
                FileDiagnostic::compiler_diagnostic(&tsc_diagnostics::THE_FILE_IS_IN_THE_PROGRAM_BECAUSE_COLON, &[]).diagnostic;
            file_reason.set_message_chain(include_details);
            chain.push(file_reason);
        }
        chain.extend(redirect_info);

        let mut result_file_name: Option<tsc_tspath::RootedFilePath> = None;
        let mut result_diagnostic: Option<Diagnostic> = None;
        let args: Vec<String> = diag.args.clone();
        if let Some(preferred_location) = &preferred_location {
            let loc = program
                .include_processor
                .get_reference_location(preferred_location, program);
            let file = &program.files[loc.file];
            result_file_name = Some(file.file_name().clone());
            result_diagnostic = Some(loc.diagnostic_at(file, diag.message, &args).diagnostic);
        }
        let mut result = result_diagnostic
            .unwrap_or_else(|| FileDiagnostic::compiler_diagnostic(diag.message, &args).diagnostic);
        if !chain.is_empty() {
            result.set_message_chain(chain);
        }
        if !related_info.is_empty() {
            result.set_related_info(related_info);
        }
        FileDiagnostic {
            file_name: result_file_name,
            diagnostic: result,
        }
    }
}

/// Go: `type includeExplainingDiagnostic struct`.
#[derive(Clone, Debug)]
pub struct IncludeExplainingDiagnostic {
    pub file: PathKey,
    pub diagnostic_reason: Option<Arc<FileIncludeReason>>,
    pub message: &'static tsc_diagnostics::Message,
    pub args: Vec<String>,
}
