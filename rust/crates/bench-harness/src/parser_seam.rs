//! The parser seam (Phase A race driver — SPEC.md §15, M3-DISPATCH.md Wave 3).
//!
//! EVERY parse in the `tsc-bench parse` driver routes through
//! [`parse_source_file`] here — this module is the ONLY file that needs to
//! change when the tsc-parser crate lands: delete the `panic!` body and
//! forward to the real parser (or delete this module and repoint
//! [`bench_parse`]'s call at `tsc_parser::parse_source_file`).
//!
//! # The EXACT signature the parser crate must expose
//!
//! Mirror of `parser.ParseSourceFile` (tsc/internal/parser/parser.go:134;
//! see rust/M3-DISPATCH.md "Phase A reminder"):
//!
//! ```ignore
//! pub fn parse_source_file(
//!     opts: tsc_ast::parseoptions::SourceFileParseOptions,
//!     source_text: &str,
//!     script_kind: tsc_core::scriptkind::ScriptKind,
//! ) -> tsc_ast::source_file::SourceFile
//! ```
//!
//! Contract notes (rust/bench/DRIVER-CONTRACT.md rule 4):
//! - `opts` carries `file_name: tsc_tspath::RootedFilePath` +
//!   `path_key: tsc_tspath::PathKey` only — the driver zeroes
//!   `external_module_indicator_options` (Go: the struct zero value).
//! - Go returns `*ast.SourceFile` and the driver defends against `nil`
//!   (+1 error). The Go parser never actually returns nil (parse failures
//!   surface through `SourceFile.Diagnostics()`), so the Rust seam returns
//!   the arena owner **by value, non-optional** — the nil branch has no
//!   Rust counterpart (PORT-note; `errors` always come from
//!   `SourceFile::diagnostics().len()`).

use tsc_ast::{SourceFile, SourceFileParseOptions};
use tsc_core::scriptkind::ScriptKind;

use crate::parse::BenchFile;

/// Parser seam — see the module docs for the exact signature the tsc-parser
/// crate must provide. M3 Wave 3 landed (commit 5c88631062); the seam now
/// forwards to the real parser. The driver machinery (corpus walk,
/// threading, counting, shape_hash, JSON) is unchanged and stays exercised
/// end-to-end in `parse.rs`'s tests via the injected fake parser.
pub fn parse_source_file(
    opts: SourceFileParseOptions,
    source_text: &str,
    script_kind: ScriptKind,
) -> SourceFile {
    tsc_parser::parse_source_file(opts, source_text, script_kind)
}

/// The [`crate::parse::ParseFn`] adapter used by the real driver: builds the
/// contract parse options from the indexed file and routes through the seam.
pub(crate) fn bench_parse(f: &BenchFile) -> SourceFile {
    parse_source_file(
        SourceFileParseOptions {
            file_name: f.file_name.clone(),
            path_key: f.path_key.clone(),
            // CONTRACT rule 4: external-module indicator options zeroed.
            external_module_indicator_options: Default::default(),
        },
        &f.text,
        f.script_kind,
    )
}
