// Ported from tsc/internal/compiler/includeprocessor.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go's `collections.SyncMap` caches keyed by `*FileIncludeReason` /
//     `includeReasonDiagnosticKey` become `Mutex<HashMap<..>>` keyed by
//     `Arc::as_ptr() as usize` (pointer identity, Go's map keying).
//   - `compilerOptionsSyntax` (the tsconfig `compilerOptions` object-literal
//     node, via `tsoptions.ForEachTsConfigPropArray` + ConfigFile) needs the
//     AST-config surface, which is not ported; the syntax cache is dropped
//     (Go's nil-ConfigFile behavior — see PORTING-NOTES).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tsc_ast::Diagnostic;
use tsc_tspath::PathKey;

use crate::diagnostics_collection::{DiagnosticsCollection, FileDiagnostic};
use crate::file_include::{FileIncludeKind, FileIncludeReason, IncludeReasonDiagnosticKey, ReferenceFileLocation};
use crate::processing_diagnostic::{IncludeExplainingDiagnostic, ProcessingDiagnostic, ProcessingDiagnosticKind};
use crate::program::Program;

/// Go: `type fileIncludeData struct`.
#[derive(Default)]
pub struct FileIncludeData {
    /// Go: `fileIncludeReasons map[tspath.PathKey][]*FileIncludeReason`.
    pub file_include_reasons: rustc_hash::FxHashMap<PathKey, Vec<Arc<FileIncludeReason>>>,
    /// Go: `processingDiagnostics []*processingDiagnostic`.
    pub processing_diagnostics: Vec<ProcessingDiagnostic>,
}

impl FileIncludeData {
    /// Go: `func (i *fileIncludeData) addProcessingDiagnostic(d ...*processingDiagnostic)`.
    pub fn add_processing_diagnostic(&mut self, diagnostics: Vec<ProcessingDiagnostic>) {
        self.processing_diagnostics.extend(diagnostics);
    }

    /// Go: `func (i *fileIncludeData) addProcessingDiagnosticsForFileCasing(file, existingCasing, currentCasing, reason)`.
    pub fn add_processing_diagnostics_for_file_casing(
        &mut self,
        file: PathKey,
        existing_casing: &str,
        current_casing: &str,
        reason: Option<Arc<FileIncludeReason>>,
    ) {
        let has_referenced = self
            .file_include_reasons
            .get(&file)
            .is_some_and(|reasons| reasons.iter().any(|reason| reason.is_referenced_file()));
        if reason.as_ref().is_none_or(|r| !r.is_referenced_file()) && has_referenced {
            self.add_processing_diagnostic(vec![ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file,
                    diagnostic_reason: reason,
                    message: &tsc_diagnostics::ALREADY_INCLUDED_FILE_NAME_0_DIFFERS_FROM_FILE_NAME_1_ONLY_IN_CASING,
                    args: vec![existing_casing.to_string(), current_casing.to_string()],
                }),
            }]);
        } else {
            self.add_processing_diagnostic(vec![ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::ExplainingFileInclude,
                reason: None,
                explanation: Some(IncludeExplainingDiagnostic {
                    file,
                    diagnostic_reason: reason,
                    message: &tsc_diagnostics::FILE_NAME_0_DIFFERS_FROM_ALREADY_INCLUDED_FILE_NAME_1_ONLY_IN_CASING,
                    args: vec![current_casing.to_string(), existing_casing.to_string()],
                }),
            }]);
        }
    }
}

/// Go: `type includeProcessor struct`.
#[derive(Default)]
pub struct IncludeProcessor {
    /// Go: `reasonDiagnostics collections.SyncMap[includeReasonDiagnosticKey, *ast.Diagnostic]`.
    pub reason_diagnostics: Mutex<HashMap<IncludeReasonDiagnosticKey, FileDiagnostic>>,
    /// Go: `reasonToReferenceLocation collections.SyncMap[*FileIncludeReason, *referenceFileLocation]`.
    reason_to_reference_location: Mutex<HashMap<usize, ReferenceFileLocation>>,
    /// Go: `includeReasonToRelatedInfo collections.SyncMap[*FileIncludeReason, *ast.Diagnostic]`.
    include_reason_to_related_info: Mutex<HashMap<usize, Option<Diagnostic>>>,
    /// Go: `redirectAndFileFormat collections.SyncMap[tspath.PathKey, []*ast.Diagnostic]`.
    redirect_and_file_format: Mutex<HashMap<PathKey, Vec<Diagnostic>>>,
    /// Go: `computedDiagnostics *ast.DiagnosticsCollection` + `computedDiagnosticsOnce sync.Once`.
    computed_diagnostics: OnceLock<DiagnosticsCollection>,
    // Go: `compilerOptionsSyntax *ast.ObjectLiteralExpression` +
    // `compilerOptionsSyntaxOnce` — dropped (AST-config surface not ported;
    // see the file header).
}

impl IncludeProcessor {
    /// Go: `func (i *includeProcessor) getDiagnostics(p *Program) *ast.DiagnosticsCollection`.
    pub fn get_diagnostics(&self, program: &Program) -> &DiagnosticsCollection {
        self.computed_diagnostics.get_or_init(|| {
            let collection = DiagnosticsCollection::default();
            for diagnostic in &program.processing_diagnostics {
                collection.add(diagnostic.to_diagnostic(program));
            }
            for (path, resolutions) in &program.resolved_modules {
                for resolution in resolutions.values() {
                    if let Some(resolution) = resolution {
                        for diagnostic in &resolution.resolution_diagnostics {
                            // PORT (file ids): the resolver's diagnostics carry
                            // a NodeId file that cannot resolve to a path
                            // (parser file_id gap); pair them with the
                            // containing file's name here.
                            let file_name = program
                                .get_source_file_by_path(path)
                                .map(|file| file.file_name().clone());
                            collection.add(FileDiagnostic {
                                file_name,
                                diagnostic: (**diagnostic).clone(),
                            });
                        }
                    }
                }
            }
            for (path, type_resolutions) in &program.type_resolutions_in_file {
                for resolution in type_resolutions.values() {
                    if let Some(resolution) = resolution {
                        for diagnostic in &resolution.resolution_diagnostics {
                            let file_name = program
                                .get_source_file_by_path(path)
                                .map(|file| file.file_name().clone());
                            collection.add(FileDiagnostic {
                                file_name,
                                diagnostic: (**diagnostic).clone(),
                            });
                        }
                    }
                }
            }
            collection
        })
    }

    /// Go: `func (i *includeProcessor) getReferenceLocation(r, program) *referenceFileLocation`.
    pub fn get_reference_location(
        &self,
        reason: &Arc<FileIncludeReason>,
        program: &Program,
    ) -> ReferenceFileLocation {
        let key = Arc::as_ptr(reason) as usize;
        let mut cache = self
            .reason_to_reference_location
            .lock()
            .expect("reasonToReferenceLocation poisoned");
        if let Some(location) = cache.get(&key) {
            return location.clone();
        }
        let location = reason.get_referenced_location(program);
        cache.insert(key, location.clone());
        location
    }

    /// Go: `func (i *includeProcessor) getRelatedInfo(r, program) *ast.Diagnostic`.
    pub fn get_related_info(
        &self,
        reason: &Arc<FileIncludeReason>,
        program: &Program,
    ) -> Option<Diagnostic> {
        let key = Arc::as_ptr(reason) as usize;
        let mut cache = self
            .include_reason_to_related_info
            .lock()
            .expect("includeReasonToRelatedInfo poisoned");
        if let Some(info) = cache.get(&key) {
            return info.clone();
        }
        let info = reason.to_related_info(program);
        cache.insert(key, info.clone());
        info
    }

    /// Go: `func (i *includeProcessor) explainRedirectAndImpliedFormat(program, filePath, toFileName)`.
    pub fn explain_redirect_and_implied_format(
        &self,
        program: &Program,
        file_path: &PathKey,
        to_file_name: &dyn Fn(&tsc_tspath::RootedFilePath) -> String,
    ) -> Vec<Diagnostic> {
        {
            let cache = self
                .redirect_and_file_format
                .lock()
                .expect("redirectAndFileFormat poisoned");
            if let Some(existing) = cache.get(file_path) {
                return existing.clone();
            }
        }

        // Go: `var file ast.HasFileName` — either the redirect target or the
        // program file itself.
        let redirect_file = program.redirect_files_by_path.get(file_path);
        let source_file_index = program.file_index_by_path(file_path);
        if redirect_file.is_none() && source_file_index.is_none() {
            return Vec::new();
        }

        let mut result: Vec<Diagnostic> = Vec::new();
        let file_name = |path: &PathKey| -> tsc_tspath::RootedFilePath {
            program
                .get_source_file_by_path(path)
                .map(|file| file.file_name().clone())
                .or_else(|| {
                    program
                        .redirect_files_by_path
                        .get(path)
                        .map(|redirect| redirect.file_name.clone())
                })
                .unwrap_or_default()
        };
        let file_path_name = if redirect_file.is_some() {
            file_name(file_path)
        } else {
            program.files[source_file_index.expect("index")].file_name().clone()
        };

        // GetSourceOfProjectReferenceIfOutputIncluded: Go compares
        // `source != file.FileName()`.
        let source = program.get_source_of_project_reference_if_output_included(
            file_path,
            &file_path_name,
        );
        if source != file_path_name {
            result.push(
                FileDiagnostic::compiler_diagnostic(
                    &tsc_diagnostics::FILE_IS_OUTPUT_OF_PROJECT_REFERENCE_SOURCE_0,
                    &[to_file_name(&source)],
                )
                .diagnostic,
            );
        }

        if let Some(redirect_file) = redirect_file {
            let target_file = program.get_source_file_by_path(&redirect_file.target);
            let target_name = target_file
                .map(|file| file.file_name().clone())
                .unwrap_or_default();
            result.push(
                FileDiagnostic::compiler_diagnostic(
                    &tsc_diagnostics::FILE_REDIRECTS_TO_FILE_0,
                    &[to_file_name(&target_name)],
                )
                .diagnostic,
            );
        }

        if let Some(index) = source_file_index {
            let file = &program.files[index];
            if crate::ast_shims::is_external_or_common_js_module(file) {
                let meta_data = program.get_source_file_meta_data(file.path_key());
                let package_json_directory = &meta_data.package_json_directory;
                let package_json =
                    package_json_directory.resolve_file("package.json");
                use tsc_core::compileroptions::ModuleKindCommonJS;
                use tsc_core::options_generated::ModuleKindESNext;
                match program.get_implied_node_format_for_emit_by_path(file.path_key()) {
                    ModuleKindESNext => {
                        if meta_data.package_json_type == "module" {
                            result.push(
                                FileDiagnostic::compiler_diagnostic(
                                    &tsc_diagnostics::FILE_IS_ECMASCRIPT_MODULE_BECAUSE_0_HAS_FIELD_TYPE_WITH_VALUE_MODULE,
                                    &[to_file_name(&package_json)],
                                )
                                .diagnostic,
                            );
                        }
                    }
                    ModuleKindCommonJS => {
                        if !meta_data.package_json_type.is_empty() {
                            result.push(
                                FileDiagnostic::compiler_diagnostic(
                                    &tsc_diagnostics::FILE_IS_COMMONJS_MODULE_BECAUSE_0_HAS_FIELD_TYPE_WHOSE_VALUE_IS_NOT_MODULE,
                                    &[to_file_name(&package_json)],
                                )
                                .diagnostic,
                            );
                        } else if !package_json_directory.as_string().is_empty() {
                            if meta_data.package_json_type.is_empty() {
                                result.push(
                                    FileDiagnostic::compiler_diagnostic(
                                        &tsc_diagnostics::FILE_IS_COMMONJS_MODULE_BECAUSE_0_DOES_NOT_HAVE_FIELD_TYPE,
                                        &[to_file_name(&package_json)],
                                    )
                                    .diagnostic,
                                );
                            }
                        } else {
                            result.push(
                                FileDiagnostic::compiler_diagnostic(
                                    &tsc_diagnostics::FILE_IS_COMMONJS_MODULE_BECAUSE_PACKAGE_JSON_WAS_NOT_FOUND,
                                    &[],
                                )
                                .diagnostic,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut cache = self
            .redirect_and_file_format
            .lock()
            .expect("redirectAndFileFormat poisoned");
        cache
            .entry(file_path.clone())
            .or_insert_with(|| result.clone());
        result
    }
}

/// Go: the `fileIncludeKind*` reason kinds — re-exported for the loader's
/// reason construction.
pub use crate::file_include::FileIncludeKind as IncludeKind;
