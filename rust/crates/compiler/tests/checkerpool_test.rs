// Ported from tsc/internal/compiler/checkerpool_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_ast::{SourceFile, SourceFileParseOptions};
use tsc_compiler::checkerpool::{
    CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER, CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
    CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER, CheckerAssociationPolicy,
    checker_association_base_weight, checker_association_order, checker_association_policy,
    checker_association_weights, checker_associations_in_order, should_prioritize_source_files,
};
use tsc_module::ResolvedModule;
use tsc_tspath::PathKey;

use tsc_compiler::Program;

/// Go: `func TestGetSourceFileForResolvedModuleUsesResolvedPath(t *testing.T)`.
///
/// PORT: Go builds a `Program` literal with a `filesByPath` entry and asserts
/// pointer identity; the port's `Program::default()` plus the pub
/// `files`/`files_by_path` fields are the same construction (the map holds
/// indexes into `files` — see program.rs), and the identity check compares
/// the returned reference with the stored file.
#[test]
fn get_source_file_for_resolved_module_uses_resolved_path() {
    let path = PathKey::from("/resolved.ts");
    let file = SourceFile::new(
        0,
        SourceFileParseOptions {
            file_name: "/different.ts".into(),
            path_key: PathKey::from("/different.ts"),
            ..Default::default()
        },
        "",
        None,
        None,
    );
    let mut program = Program::default();
    program.files.push(file);
    program.files_by_path.insert(path.clone(), 0);

    let resolved = ResolvedModule {
        resolved_file_name: "/different.ts".into(),
        resolved_path: path,
        ..Default::default()
    };
    let got = program
        .get_source_file_for_resolved_module(&resolved)
        .expect("file");
    assert!(std::ptr::eq(got, &program.files[0]));
}

/// Go: `func TestGetCheckerAssociationBaseWeight(t *testing.T)`.
#[test]
fn get_checker_association_base_weight() {
    assert_eq!(checker_association_base_weight(100, 2500), 125);
}

/// Go: `func TestShouldPrioritizeSourceFiles(t *testing.T)`.
#[test]
fn should_prioritize_source_files_test() {
    assert!(should_prioritize_source_files(1000, 100, 4));
    assert!(should_prioritize_source_files(1000, 125, 4));
    assert!(!should_prioritize_source_files(1000, 126, 4));
}

/// Go: `func TestGetCheckerAssociationPolicy(t *testing.T)`.
#[test]
fn get_checker_association_policy() {
    struct PolicyTest {
        name: &'static str,
        total_weight: i64,
        declaration_weight: i64,
        checker_count: i64,
        want: CheckerAssociationPolicy,
    }
    let tests = [
        PolicyTest {
            name: "source dominated at any checker count",
            total_weight: 1000,
            declaration_weight: 100,
            checker_count: 2,
            want: CheckerAssociationPolicy {
                prioritize_source_files: true,
                source_file_weight_multiplier: 1,
                balance_penalty_multiplier: CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
            },
        },
        PolicyTest {
            name: "declaration heavy with few checkers",
            total_weight: 1000,
            declaration_weight: 400,
            checker_count: 2,
            want: CheckerAssociationPolicy {
                prioritize_source_files: false,
                source_file_weight_multiplier: 1,
                balance_penalty_multiplier: 1,
            },
        },
        PolicyTest {
            name: "source dominated with many checkers",
            total_weight: 1000,
            declaration_weight: 50,
            checker_count: 8,
            want: CheckerAssociationPolicy {
                prioritize_source_files: true,
                source_file_weight_multiplier: 1,
                balance_penalty_multiplier: CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
            },
        },
        PolicyTest {
            name: "declaration heavy with many checkers",
            total_weight: 1000,
            declaration_weight: 400,
            checker_count: 4,
            want: CheckerAssociationPolicy {
                prioritize_source_files: false,
                source_file_weight_multiplier: CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER,
                balance_penalty_multiplier: CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER,
            },
        },
    ];
    for test in tests {
        let got = checker_association_policy(
            test.total_weight,
            test.declaration_weight,
            test.checker_count,
        );
        assert_eq!(got, test.want, "{}", test.name);
    }
}

/// Go: `func TestGetCheckerAssociationOrder(t *testing.T)`.
#[test]
fn get_checker_association_order() {
    assert_eq!(
        checker_association_order(&[5, 10, 7, 2], &[true, false, false, true], true),
        Some(vec![1, 2, 0, 3])
    );
    // Go: nil result (program order preserved without allocating).
    assert_eq!(checker_association_order(&[5], &[false], false), None);
}

/// Go: `func TestGetCheckerAssociationWeights(t *testing.T)`.
#[test]
fn get_checker_association_weights() {
    struct WeightsTest {
        name: &'static str,
        base_weights: Vec<i64>,
        import_counts: Vec<i64>,
        want: Vec<i64>,
    }
    let tests = [
        WeightsTest {
            name: "normalizes import work to syntax work",
            base_weights: vec![100, 50, 25],
            import_counts: vec![0, 1, 3],
            want: vec![100, 93, 154],
        },
        WeightsTest {
            name: "no imports preserves base weights",
            base_weights: vec![100, 50, 25],
            import_counts: vec![0, 0, 0],
            want: vec![100, 50, 25],
        },
    ];
    for test in tests {
        let got = checker_association_weights(&test.base_weights, &test.import_counts);
        assert_eq!(got, test.want, "{}", test.name);
    }
}

/// Go: `func TestGetCheckerAssociations(t *testing.T)`.
#[test]
fn get_checker_associations() {
    // Go: t.Run("empty", ...) — nil inputs produce a nil result.
    assert_eq!(
        checker_associations_in_order(&[], &[], None, 4, CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER),
        None
    );

    // Go: t.Run("balances disconnected files", ...).
    let empty_adjacency: Vec<Vec<i64>> = (0..6).map(|_| Vec::new()).collect();
    let got = checker_associations_in_order(&[1, 1, 1, 1, 1, 1], &empty_adjacency, None, 3, 1);
    assert_eq!(got, Some(vec![0, 1, 2, 0, 1, 2]));

    // Go: t.Run("uses program order", ...).
    let empty_adjacency: Vec<Vec<i64>> = (0..3).map(|_| Vec::new()).collect();
    let got = checker_associations_in_order(&[1, 3, 2], &empty_adjacency, None, 2, 1);
    assert_eq!(got, Some(vec![0, 1, 0]));

    // Go: t.Run("keeps dense components together", ...).
    let adjacency = vec![
        vec![1, 2],
        vec![0, 2],
        vec![0, 1],
        vec![4, 5],
        vec![3, 5],
        vec![3, 4],
    ];
    let got = checker_associations_in_order(&[1, 1, 1, 1, 1, 1], &adjacency, None, 2, 1);
    assert_eq!(got, Some(vec![0, 0, 0, 1, 1, 1]));

    // Go: t.Run("respects weighted balance cap", ...).
    let weights = [8, 7, 6, 5, 4, 3, 2, 1];
    let empty_adjacency: Vec<Vec<i64>> = (0..weights.len()).map(|_| Vec::new()).collect();
    let got = checker_associations_in_order(&weights, &empty_adjacency, None, 3, 1)
        .expect("associations");
    let mut loads = [0i64; 3];
    for (i, checker_index) in got.iter().enumerate() {
        loads[*checker_index as usize] += weights[i];
    }
    for (checker_index, load) in loads.iter().enumerate() {
        assert!(
            *load <= 13,
            "checker {checker_index} load = {load}, want at most 13; associations = {got:?}"
        );
    }
}
