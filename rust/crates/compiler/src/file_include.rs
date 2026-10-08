// Ported from tsc/internal/compiler/fileInclude.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go threads `*FileIncludeReason` and `*processingDiagnostic` as shared
//     pointer identities (map keys, `collections.Set` members). The port
//     uses `Arc<FileIncludeReason>` and keys those maps by
//     `Arc::as_ptr() as usize` (pointer identity, exactly Go's map keying).
//   - `referenceFileLocation.file` is a program file index (`usize` into
//     `Program.files`); Go's `*ast.SourceFile` resolves through the pointer
//     graph. Node handles resolve against that file's arena.
//   - `referencedFileData.synthetic` is `Option<String>` (the synthetic
//     module-reference text); Go stores the synthetic `*ast.Node` and reads
//     its text/loc through the pointer. The port's synthetic nodes DO live in
//     the owning file's arena (create_synthetic_import), but the reason only
//     ever reads the text, so the string carries it.
//   - `toRelatedInfo`'s config-file syntax arms need the tsoptions
//     AST-config surface (GetTsConfigPropArrayElementValue,
//     GetOptionsSyntaxByArrayElementValue, ConfigFile), which is not ported;
//     those arms degrade to Go's nil-ConfigFile behavior (see PORTING-NOTES).

use std::sync::{Arc, Mutex};

use tsc_ast::{Diagnostic, NodeId, SourceFile};
use tsc_module::PackageId;
use tsc_tspath::{PathKey, RootedDirectoryPath, RootedFilePath};

use crate::diagnostics_collection::FileDiagnostic;
use crate::program::Program;

/// Go: `type fileIncludeKind int` + consts (iota order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum FileIncludeKind {
    // References from file
    Import = 0,
    ReferenceFile = 1,
    TypeReferenceDirective = 2,
    LibReferenceDirective = 3,

    RootFile = 4,
    LibFile = 5,
    AutomaticTypeDirectiveFile = 6,
    ContentMapperSupplemental = 7,
}

/// Go: `type FileIncludeReason struct`.
#[derive(Clone, Debug)]
pub struct FileIncludeReason {
    pub kind: FileIncludeKind,
    pub index: usize,
    pub is_default_lib: bool,
    pub referenced_file: Option<ReferencedFileData>,
    pub automatic_type_directive: Option<AutomaticTypeDirectiveFileData>,
    pub canonical_source_file: PathKey,
}

impl FileIncludeReason {
    /// Go: `func (r *FileIncludeReason) asIndex() int`.
    pub fn as_index(&self) -> usize {
        self.index
    }

    /// Go: `func (r *FileIncludeReason) asLibFileIndex() (int, bool)`.
    pub fn as_lib_file_index(&self) -> (usize, bool) {
        (self.index, !self.is_default_lib)
    }

    /// Go: `func (r *FileIncludeReason) isReferencedFile() bool` — the Go
    /// nil-receiver check is the caller's `Option` handling; the method itself
    /// is the kind check.
    pub fn is_referenced_file(&self) -> bool {
        self.kind <= FileIncludeKind::LibReferenceDirective
    }

    /// Go: `func (r *FileIncludeReason) asReferencedFileData() *referencedFileData`.
    pub fn as_referenced_file_data(&self) -> Option<&ReferencedFileData> {
        self.referenced_file.as_ref()
    }

    /// Go: `func (r *FileIncludeReason) asAutomaticTypeDirectiveFileData()`.
    pub fn as_automatic_type_directive_file_data(&self) -> Option<&AutomaticTypeDirectiveFileData> {
        self.automatic_type_directive.as_ref()
    }
}

/// Go: `type referencedFileData struct`.
#[derive(Clone, Debug)]
pub struct ReferencedFileData {
    pub file: PathKey,
    pub index: i64,
    /// Go: `synthetic *ast.Node` — the synthetic module-reference text
    /// (see the file header PORT note).
    pub synthetic: Option<String>,
}

/// Go: `type automaticTypeDirectiveFileData struct`.
#[derive(Clone, Debug)]
pub struct AutomaticTypeDirectiveFileData {
    pub type_reference: String,
    pub package_id: PackageId,
}

/// Go: `type referenceFileLocation struct`.
///
/// PORT: `file` is a program file index; `node` resolves against that file's
/// arena. `synthetic_text` carries Go's synthesized-node text.
#[derive(Clone, Debug)]
pub struct ReferenceFileLocation {
    pub file: usize,
    pub node: Option<NodeId>,
    /// Go: the synthesized-node arm of `text()`/`diagnosticAt` — the synthetic
    /// module reference text, present iff `is_synthetic`.
    pub synthetic_text: Option<String>,
    pub r#ref: Option<tsc_ast::FileReference>,
    pub package_id: PackageId,
    pub is_synthetic: bool,
}

impl ReferenceFileLocation {
    /// Go: `func (r *referenceFileLocation) text() string`.
    pub fn text(&self, file: &SourceFile) -> String {
        if let Some(node) = self.node {
            if let Some(synthetic_text) = &self.synthetic_text {
                return format!("\"{synthetic_text}\"");
            }
            let text = file.text();
            let start = tsc_scanner::skip_trivia(text, node.pos() as i32) as usize;
            let end = node.end() as usize;
            text[start..end].to_string()
        } else if let Some(reference) = &self.r#ref {
            let text = file.text();
            let start = reference.pos() as usize;
            let end = reference.end() as usize;
            text[start..end].to_string()
        } else {
            unreachable!("referenceFileLocation must have a node or a ref")
        }
    }

    /// Go: `func (r *referenceFileLocation) diagnosticAt(message, args...)`.
    pub fn diagnostic_at(
        &self,
        file: &SourceFile,
        message: &'static tsc_diagnostics::Message,
        args: &[String],
    ) -> FileDiagnostic {
        if let Some(node) = self.node {
            // Go: tsoptions.CreateDiagnosticForNodeInSourceFile(file, node,
            // ...) — PORT: the tsoptions surface is the compiler-diagnostic
            // form until the AST-config path lands; the location degrades
            // with it.
            let loc = file.node(node).loc;
            FileDiagnostic::file_diagnostic(file, loc, message, args)
        } else if let Some(reference) = &self.r#ref {
            FileDiagnostic::file_diagnostic(file, reference.loc, message, args)
        } else {
            unreachable!("referenceFileLocation must have a node or a ref")
        }
    }
}

impl FileIncludeReason {
    /// Go: `func (r *FileIncludeReason) getReferencedLocation(program *Program) *referenceFileLocation`.
    pub fn get_referenced_location(self: &Arc<Self>, program: &Program) -> ReferenceFileLocation {
        let reference = self.as_referenced_file_data().expect("referencedFileData");
        let file_index = program
            .file_index_by_path(&reference.file)
            .expect("file not found in program");
        let file = &program.files[file_index];
        let index = reference.index;
        match self.kind {
            FileIncludeKind::Import => {
                let mut specifier = None;
                let mut is_synthetic = false;
                let mut synthetic_text = None;
                if let Some(synthetic) = &reference.synthetic {
                    // Go: ref.synthetic != nil → specifier = ref.synthetic (a
                    // node in the file's arena whose parent is the synthetic
                    // ImportDeclaration). The port carries the text; the
                    // parent-kind and mode computations are the synthetic
                    // variants (see Program::get_mode_for_synthetic_import).
                    synthetic_text = Some(synthetic.clone());
                    is_synthetic = true;
                } else if (index as usize) < file.imports().len() {
                    specifier = Some(file.imports()[index as usize]);
                } else {
                    let mut aug_index = file.imports().len() as i64;
                    for imp in &file.payload().module_augmentations {
                        if file.node(*imp).kind == tsc_ast::Kind::StringLiteral {
                            if aug_index == index {
                                specifier = Some(*imp);
                                break;
                            }
                            aug_index += 1;
                        }
                    }
                }
                let package_id = if is_synthetic {
                    let mode = program.get_mode_for_synthetic_import(file_index);
                    program
                        .get_resolved_module(file_index, synthetic_text.as_deref().expect("text"), mode)
                        .map(|resolution| resolution.package_id.clone())
                        .unwrap_or_default()
                } else {
                    match specifier {
                        Some(specifier) => program
                            .get_resolved_module_from_module_specifier(file_index, specifier)
                            .map(|resolution| resolution.package_id.clone())
                            .unwrap_or_default(),
                        None => PackageId::default(),
                    }
                };
                ReferenceFileLocation {
                    file: file_index,
                    node: specifier,
                    synthetic_text,
                    r#ref: None,
                    package_id,
                    is_synthetic,
                }
            }
            FileIncludeKind::ReferenceFile => ReferenceFileLocation {
                file: file_index,
                node: None,
                synthetic_text: None,
                r#ref: Some(file.payload().referenced_files[index as usize].clone()),
                package_id: PackageId::default(),
                is_synthetic: false,
            },
            FileIncludeKind::TypeReferenceDirective => ReferenceFileLocation {
                file: file_index,
                node: None,
                synthetic_text: None,
                r#ref: Some(file.payload().type_reference_directives[index as usize].clone()),
                package_id: PackageId::default(),
                is_synthetic: false,
            },
            FileIncludeKind::LibReferenceDirective => ReferenceFileLocation {
                file: file_index,
                node: None,
                synthetic_text: None,
                r#ref: Some(file.payload().lib_reference_directives[index as usize].clone()),
                package_id: PackageId::default(),
                is_synthetic: false,
            },
            kind => panic!("unknown reason: {kind:?}"),
        }
    }

    /// Go: `func (r *FileIncludeReason) toDiagnostic(program, relativeFileName, relativeTo)`.
    pub fn to_diagnostic(
        self: &Arc<Self>,
        program: &Program,
        relative_file_name: bool,
        relative_to: RootedDirectoryPath,
    ) -> FileDiagnostic {
        let cache = program
            .include_processor
            .reason_diagnostics
            .lock()
            .expect("reasonDiagnostics poisoned");
        let key = IncludeReasonDiagnosticKey {
            reason: Arc::as_ptr(self) as usize,
            relative_file_name,
            relative_to: relative_to.as_string().to_string(),
        };
        if let Some(diagnostic) = cache.get(&key) {
            return diagnostic.clone();
        }
        drop(cache);
        let diagnostic = self.compute_diagnostic(program, &|file_name: &RootedFilePath| {
            if relative_file_name {
                if let Some(relative) =
                    program
                        .case_sensitivity
                        .relative_path_from_directory(&relative_to, file_name)
                {
                    return relative.as_string().to_string();
                }
            }
            file_name.as_string().to_string()
        });
        let mut cache = program
            .include_processor
            .reason_diagnostics
            .lock()
            .expect("reasonDiagnostics poisoned");
        cache.entry(key).or_insert_with(|| diagnostic).clone()
    }

    /// Go: `func (r *FileIncludeReason) computeDiagnostic(program, toFileName)`.
    fn compute_diagnostic(
        self: &Arc<Self>,
        program: &Program,
        to_file_name: &dyn Fn(&RootedFilePath) -> String,
    ) -> FileDiagnostic {
        if self.is_referenced_file() {
            return self.compute_reference_file_diagnostic(program, to_file_name);
        }
        let options = program.options();
        match self.kind {
            FileIncludeKind::RootFile => {
                // Go: program.opts.Config.ConfigFile != nil → the matched-spec
                // diagnostics; the AST-config surface (GetMatchedFileSpec /
                // GetMatchedIncludeSpec) is not ported — the nil-ConfigFile arm.
                FileDiagnostic::compiler_diagnostic(
                    &tsc_diagnostics::ROOT_FILE_SPECIFIED_FOR_COMPILATION,
                    &[],
                )
            }
            FileIncludeKind::AutomaticTypeDirectiveFile => {
                let data = self.as_automatic_type_directive_file_data().expect("data");
                if !options.uses_wildcard_types() {
                    if !data.package_id.name.is_empty() {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::ENTRY_POINT_OF_TYPE_LIBRARY_0_SPECIFIED_IN_COMPILEROPTIONS_WITH_PACKAGEID_1,
                            &[data.type_reference.clone(), data.package_id.to_string()],
                        )
                    } else {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::ENTRY_POINT_OF_TYPE_LIBRARY_0_SPECIFIED_IN_COMPILEROPTIONS,
                            &[data.type_reference.clone()],
                        )
                    }
                } else if !data.package_id.name.is_empty() {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::ENTRY_POINT_FOR_IMPLICIT_TYPE_LIBRARY_0_WITH_PACKAGEID_1,
                        &[data.type_reference.clone(), data.package_id.to_string()],
                    )
                } else {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::ENTRY_POINT_FOR_IMPLICIT_TYPE_LIBRARY_0,
                        &[data.type_reference.clone()],
                    )
                }
            }
            FileIncludeKind::LibFile => {
                let (index, ok) = self.as_lib_file_index();
                if ok {
                    let lib = program
                        .options()
                        .lib
                        .as_ref()
                        .and_then(|libs| libs.get(index))
                        .cloned()
                        .unwrap_or_default();
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::LIBRARY_0_SPECIFIED_IN_COMPILEROPTIONS,
                        &[lib],
                    )
                } else {
                    let target = options.get_emit_script_target().to_string();
                    if !target.is_empty() {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::DEFAULT_LIBRARY_FOR_TARGET_0,
                            &[target],
                        )
                    } else {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::DEFAULT_LIBRARY,
                            &[],
                        )
                    }
                }
            }
            FileIncludeKind::ContentMapperSupplemental => {
                let canonical = program
                    .get_source_file_by_path(&self.canonical_source_file)
                    .expect("canonical source file");
                FileDiagnostic::compiler_diagnostic(
                    &tsc_diagnostics::SUPPLEMENTAL_VIRTUAL_FILE_PRODUCED_BY_THE_CONTENT_MAPPER_FOR_FILE_0,
                    &[to_file_name(canonical.file_name())],
                )
            }
            kind => panic!("unknown reason: {kind:?}"),
        }
    }

    /// Go: `func (r *FileIncludeReason) computeReferenceFileDiagnostic(program, toFileName)`.
    fn compute_reference_file_diagnostic(
        self: &Arc<Self>,
        program: &Program,
        to_file_name: &dyn Fn(&RootedFilePath) -> String,
    ) -> FileDiagnostic {
        let reference_location = program.include_processor.get_reference_location(self, program);
        let file = &program.files[reference_location.file];
        let reference_text = reference_location.text(file);
        let file_name = to_file_name(file.file_name());
        let package_id = reference_location.package_id.to_string();
        match self.kind {
            FileIncludeKind::Import => {
                if !reference_location.is_synthetic {
                    if !reference_location.package_id.name.is_empty() {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1_WITH_PACKAGEID_2,
                            &[reference_text, file_name, package_id],
                        )
                    } else {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1,
                            &[reference_text, file_name],
                        )
                    }
                } else if program
                    .import_helpers_import_specifiers
                    .get(file.path_key())
                    .is_some_and(|specifier| {
                        // Go: specifier == referenceLocation.node (pointer
                        // identity against the synthetic import node).
                        Some(&specifier) == reference_location.synthetic_text.as_ref()
                    })
                {
                    if !reference_location.package_id.name.is_empty() {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1_WITH_PACKAGEID_2_TO_IMPORT_IMPORTHELPERS_AS_SPECIFIED_IN_COMPILEROPTIONS,
                            &[reference_text, file_name, package_id],
                        )
                    } else {
                        FileDiagnostic::compiler_diagnostic(
                            &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1_TO_IMPORT_IMPORTHELPERS_AS_SPECIFIED_IN_COMPILEROPTIONS,
                            &[reference_text, file_name],
                        )
                    }
                } else if !reference_location.package_id.name.is_empty() {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1_WITH_PACKAGEID_2_TO_IMPORT_JSX_AND_JSXS_FACTORY_FUNCTIONS,
                        &[reference_text, file_name, package_id],
                    )
                } else {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::IMPORTED_VIA_0_FROM_FILE_1_TO_IMPORT_JSX_AND_JSXS_FACTORY_FUNCTIONS,
                        &[reference_text, file_name],
                    )
                }
            }
            FileIncludeKind::ReferenceFile => FileDiagnostic::compiler_diagnostic(
                &tsc_diagnostics::REFERENCED_VIA_0_FROM_FILE_1,
                &[reference_text, file_name],
            ),
            FileIncludeKind::TypeReferenceDirective => {
                if !reference_location.package_id.name.is_empty() {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::TYPE_LIBRARY_REFERENCED_VIA_0_FROM_FILE_1_WITH_PACKAGEID_2,
                        &[reference_text, file_name, package_id],
                    )
                } else {
                    FileDiagnostic::compiler_diagnostic(
                        &tsc_diagnostics::TYPE_LIBRARY_REFERENCED_VIA_0_FROM_FILE_1,
                        &[reference_text, file_name],
                    )
                }
            }
            FileIncludeKind::LibReferenceDirective => {
                FileDiagnostic::compiler_diagnostic(
                    &tsc_diagnostics::LIBRARY_REFERENCED_VIA_0_FROM_FILE_1,
                    &[reference_text, file_name],
                )
            }
            kind => panic!("unknown reason: {kind:?}"),
        }
    }

    /// Go: `func (r *FileIncludeReason) toRelatedInfo(program)`.
    pub fn to_related_info(self: &Arc<Self>, program: &Program) -> Option<Diagnostic> {
        if self.is_referenced_file() {
            return self.compute_reference_file_related_info(program);
        }
        // Go: the config-file syntax arms (GetTsConfigPropArrayElementValue,
        // GetOptionsSyntaxByArrayElementValue, ForEachPropertyAssignment) need
        // the AST-config surface; without it the arms degrade to Go's
        // nil-ConfigFile behavior (PORTING-NOTES).
        None
    }

    /// Go: `func (r *FileIncludeReason) computeReferenceFileRelatedInfo(program)`.
    fn compute_reference_file_related_info(self: &Arc<Self>, program: &Program) -> Option<Diagnostic> {
        let reference_location = program.include_processor.get_reference_location(self, program);
        if reference_location.is_synthetic {
            return None;
        }
        let file = &program.files[reference_location.file];
        let message: &'static tsc_diagnostics::Message = match self.kind {
            FileIncludeKind::Import => &tsc_diagnostics::FILE_IS_INCLUDED_VIA_IMPORT_HERE,
            FileIncludeKind::ReferenceFile => &tsc_diagnostics::FILE_IS_INCLUDED_VIA_REFERENCE_HERE,
            FileIncludeKind::TypeReferenceDirective => {
                &tsc_diagnostics::FILE_IS_INCLUDED_VIA_TYPE_LIBRARY_REFERENCE_HERE
            }
            FileIncludeKind::LibReferenceDirective => {
                &tsc_diagnostics::FILE_IS_INCLUDED_VIA_LIBRARY_REFERENCE_HERE
            }
            kind => panic!("unknown reason: {kind:?}"),
        };
        // Go: referenceLocation.diagnosticAt(message) — the file-located
        // diagnostic form.
        let loc = if let Some(node) = reference_location.node {
            file.node(node).loc
        } else {
            reference_location.r#ref.as_ref().expect("ref").loc
        };
        let text = tsc_diagnostics::format(message.text(), &[]);
        Some(Diagnostic::new(
            Some(file.as_node_id()),
            loc,
            message.code(),
            message.category(),
            "",
            Some(message),
            text,
            message.key(),
            vec![],
        ))
    }
}

/// Go: `type includeReasonDiagnosticKey struct` (includeprocessor.go).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IncludeReasonDiagnosticKey {
    pub reason: usize,
    pub relative_file_name: bool,
    pub relative_to: String,
}

/// Go: the includeProcessor `reasonDiagnostics` map — `SyncMap` with
/// interior mutability.
pub type ReasonDiagnostics = Mutex<std::collections::HashMap<IncludeReasonDiagnosticKey, FileDiagnostic>>;
