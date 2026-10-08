// Package compiler implements the TypeScript compiler.
//
// Ported from tsc/internal/compiler @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// File map (Go → Rust):
//   pkg.go                                   → lib.rs (this file)
//   program.go                               → program.rs
//   filesparser.go                           → files_parser.rs
//   fileloader.go                            → file_loader.rs
//   checkerpool.go                           → checkerpool.rs
//   fileInclude.go                           → file_include.rs
//   includeprocessor.go                      → include_processor.rs
//   processingDiagnostic.go                  → processing_diagnostic.rs
//   host.go                                  → host.rs
//   emitHost.go                              → emit_host.rs
//   projectreferencefilemapper.go             → project_reference_file_mapper.rs
//   projectreferenceparser.go                 → project_reference_parser.rs
//   projectreferencedtsfakinghost.go         → project_reference_dts_faking_host.rs
//   emitter.go                               → emitter.rs (the program-level bits:
//                                              SourceFileMayBeEmittedHost,
//                                              sourceFileMayBeEmitted,
//                                              getSourceFilesToEmit; the emit
//                                              machinery itself is the M7
//                                              boundary — see PORTING-NOTES)
//   ast utilities the loader needs (Go:      → ast_shims.rs (crate-local, the
//   ast/utilities.go, ast/parseoptions.go,     binder ast_shims precedent;
//   core/core.go, ast/ast.go)                  dedup candidates — PORTING-NOTES)
//   ast/diagnostic.go DiagnosticsCollection   → diagnostics_collection.rs
//   (the collection struct only; the struct    (crate-local; the ast crate
//    lives in the Rust ast crate already)       ports only the struct)
//
// Known seams (each PORT-noted at its site and in PORTING-NOTES.md):
//   - tracing is not ported (no tracing crate yet); the loader drops the
//     tracing hooks and CompilerHost keeps the Trace callback.
//   - The checker crate is a parallel wave; CheckerPool is a TODO-shaped
//     trait handing out unit checkers (`Checker = ()` alias). The pure FENNEL
//     association functions the pool is built from are ported
//     (checkerpool.rs); the pool struct itself (per-checker mutexes,
//     `checker.NewChecker` calls, `fileAssociations`) lands with that wave.
//   - The content-mapper host machinery (contentmapper.Project, transform
//     pipeline, failure-budget error types) is not ported; content-mapper
//     paths degrade to non-mapped loads exactly as Go's zero-config case.
//   - tsoptions' config-node surface (TsConfigSourceFile + the config
//     syntax walks) landed with the M6 removed-option wave;
//     verifyCompilerOptions' non-removed option arms still degrade (the
//     full AST-config wave — see program.rs PORT notes for the plug
//     points), and direct ParseJsonConfigFileContent callers still see
//     Go's nil-ConfigFile arm (no config source file).
//   - cachedvfs + symlinks are not ported; the project-reference dts-faking
//     host is the identity host (see project_reference_dts_faking_host.rs).
//   - emitter.go's emit pipeline, printer/sourcemap/transformer dependencies,
//     and Program.Emit are the M7 boundary.

pub mod ast_shims;
pub mod checkerpool;
pub mod diagnostics_collection;
pub mod emit_host;
pub mod emitter;
pub mod file_include;
pub mod file_loader;
pub mod files_parser;
pub mod host;
pub mod include_processor;
pub mod processing_diagnostic;
pub mod program;
pub mod project_reference_dts_faking_host;
pub mod project_reference_file_mapper;
pub mod project_reference_parser;

pub use ast_shims::{ensure_script_kind_from_file_name, get_script_kind_from_file_name};
pub use checkerpool::{
    Checker, CheckerPool, checker_association_base_weight,
    checker_association_order as get_checker_association_order, checker_association_policy,
    checker_association_weights, checker_associations_in_order,
    should_prioritize_source_files, CheckerAssociationPolicy,
};
pub use diagnostics_collection::{
    compare_diagnostics, equal_diagnostics, equal_diagnostics_no_related_info,
    sort_and_deduplicate_diagnostics, DiagnosticsCollection, FileDiagnostic,
};
pub use emit_host::{new_emit_host, EmitHost, EmitHostImpl};
pub use emitter::{
    get_source_files_to_emit, is_source_file_not_json, source_file_may_be_emitted, EmitOnly,
    SourceFileMayBeEmittedHost,
};
pub use file_loader::{
    process_all_program_files, DuplicateSourceFile, LibFile, LibResolution, ProcessedFiles,
};
pub use host::new_compiler_host;
pub use program::{
    combine_emit_results, get_diagnostics_of_any_program, handle_no_emit_options, new_program,
    EmitOptions, EmitResult, Program, ProgramConfig, ProgramFactories, ProgramHosts, ProgramLike,
    ProgramOptions, SourceMapEmitResult, WriteFile, WriteFileData,
};
pub use project_reference_file_mapper::SourceOutputAndProjectReference;
