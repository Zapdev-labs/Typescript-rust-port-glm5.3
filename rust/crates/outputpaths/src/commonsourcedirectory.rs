// Ported from tsc/internal/outputpaths/commonsourcedirectory.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_core::options_generated::CompilerOptions;
use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath};

/// Go: `func computeCommonSourceDirectoryOfFilenames(fileNames, currentDirectory, caseSensitivity)`.
fn compute_common_source_directory_of_file_names(
    file_names: &[RootedFilePath],
    current_directory: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> RootedDirectoryPath {
    if file_names.is_empty() {
        // Can happen when all input files are .d.ts files.
        return current_directory.clone();
    }
    case_sensitivity.common_directory_of_files(file_names)
}

/// Go: `func GetComputedCommonSourceDirectory(emittedFiles, currentDirectory, caseSensitivity)`.
pub fn get_computed_common_source_directory(
    emitted_files: &[RootedFilePath],
    current_directory: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> RootedDirectoryPath {
    compute_common_source_directory_of_file_names(emitted_files, current_directory, case_sensitivity)
}

/// Go: `func GetCommonSourceDirectory(options, files, currentDirectory, caseSensitivity, checkSourceFilesBelongToPath)`.
///
/// PORT: Go passes `files func() []tspath.RootedFilePath` (a lazy thunk) and a
/// nil-able `checkSourceFilesBelongToPath` callback; the port takes
/// `Option<&mut dyn FnMut(...)>` for the nil-ness and calls the files closure
/// eagerly at each use site exactly where Go invokes it.
pub fn get_common_source_directory(
    options: &CompilerOptions,
    files: &dyn Fn() -> Vec<RootedFilePath>,
    current_directory: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
    check_source_files_belong_to_path: Option<&mut dyn FnMut(&[RootedFilePath], &RootedDirectoryPath) -> bool>,
) -> RootedDirectoryPath {
    let common_source_directory;
    if !options.root_dir.is_empty() {
        // If a rootDir is specified use it as the commonSourceDirectory
        common_source_directory = options.root_dir.clone();
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), &common_source_directory);
        }
    } else if !options.config_file_path.is_empty() {
        // If the rootDir is not specified, then the common source directory is the directory of the config file.
        common_source_directory = options.config_file_path.directory();
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), &common_source_directory);
        }
    } else {
        common_source_directory = compute_common_source_directory_of_file_names(
            &files(),
            current_directory,
            case_sensitivity,
        );
    }

    common_source_directory
}
