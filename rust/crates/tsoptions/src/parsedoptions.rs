// Port of tsc/internal/tsoptions/parsedoptions.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_core::options_generated::{CompilerOptions, TypeAcquisition};
use tsc_tspath::RootedFilePath;

use crate::tsconfigparsing::ProjectReference;

/// Go: `type ParsedOptions struct`.
#[derive(Clone, Debug, Default)]
pub struct ParsedOptions {
    /// Go: `CompilerOptions *core.CompilerOptions`.
    pub compiler_options: CompilerOptions,
    /// Go: `TypeAcquisition *core.TypeAcquisition`.
    pub type_acquisition: TypeAcquisition,
    /// Go: `FileNames []tspath.RootedFilePath`.
    pub file_names: Vec<RootedFilePath>,
    /// Go: `ProjectReferences []*core.ProjectReference`.
    pub project_references: Vec<ProjectReference>,
    /// Go: `ContentMappers []*contentmapper.Mapper`.
    pub content_mappers: Vec<tsc_contentmapper::Mapper>,
}

impl ParsedOptions {
    /// Go: `func (p *ParsedOptions) Equals(other *ParsedOptions) bool`.
    ///
    /// PORT: Go compares `*core.CompilerOptions` with its generated
    /// `Equals`; the Rust core struct has no equality yet, so the compiler
    /// options are compared through their serialized (deterministic) option
    /// maps instead — same values in, same equality out.
    pub fn equals(&self, other: &ParsedOptions) -> bool {
        if !type_acquisition_equals(&self.type_acquisition, &other.type_acquisition) {
            return false;
        }
        if !compiler_options_equals(&self.compiler_options, &other.compiler_options) {
            return false;
        }
        self.file_names == other.file_names
            && project_references_equal(&self.project_references, &other.project_references)
            && content_mappers_equal(&self.content_mappers, &other.content_mappers)
    }
}

/// Go: `func (o *core.CompilerOptions) Equals(other *core.CompilerOptions) bool` —
/// see `ParsedOptions::equals` for the serialization strategy.
fn compiler_options_equals(a: &CompilerOptions, b: &CompilerOptions) -> bool {
    let config_file_path = RootedFilePath::default();
    let case_sensitivity = tsc_tspath::CaseSensitivity::default();
    let a_map = crate::options_generated::serialize_compiler_options(a, &config_file_path, &case_sensitivity);
    let b_map = crate::options_generated::serialize_compiler_options(b, &config_file_path, &case_sensitivity);
    let a_value = crate::commandlineoption::CompilerOptionsValue::Object(a_map);
    let b_value = crate::commandlineoption::CompilerOptionsValue::Object(b_map);
    a_value == b_value
}

/// Go: `func (o *core.TypeAcquisition) Equals(...)`.
fn type_acquisition_equals(a: &TypeAcquisition, b: &TypeAcquisition) -> bool {
    a.enable == b.enable
        && a.disable_filename_based_type_acquisition == b.disable_filename_based_type_acquisition
        && a.include == b.include
        && a.exclude == b.exclude
}

/// Go: `slices.EqualFunc(..., func(a, b *core.ProjectReference) bool { ... *a == *b })`.
fn project_references_equal(a: &[ProjectReference], b: &[ProjectReference]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| {
            a.path == b.path
                && a.original_path == b.original_path
                && a.circular == b.circular
        })
}

/// Go: `slices.EqualFunc(..., (*contentmapper.Mapper).Equals)`.
fn content_mappers_equal(a: &[tsc_contentmapper::Mapper], b: &[tsc_contentmapper::Mapper]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(a, b)| a.equals(b))
}
