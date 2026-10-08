// Ported from tsc/internal/compiler/checkerpool.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - The checker crate is a parallel wave (see lib.rs): `Checker` is the
//     TODO-shaped `()` alias and the `CheckerPool` trait hands it out without
//     the release closure (Go returns `(*checker.Checker, func())`; there is
//     nothing to release until `checker.NewChecker` is ported).
//   - The `checkerPool` struct itself (createCheckers' `checker.NewChecker`
//     calls, per-checker mutexes, `fileAssociations`) is deferred with that
//     wave — it is pure bookkeeping around unported checkers. The pure FENNEL
//     association functions it is built from are ported here (the
//     checkerpool_test.go surface) so the wave only adds the struct.
//   - `tracing` is not ported (no tracing crate yet): the
//     `newCheckerPoolWithTracing` split and the tracer plumbing are dropped.
//
// The FENNEL comments below are Go's (kept verbatim in substance; see
// checkerpool.go for the exact calibrated-constant rationale).

use tsc_ast::SourceFile;

/// The checker handed out by the pool.
///
/// PORT: the checker crate is a parallel wave; `Checker` is the TODO-shaped
/// `()` alias until it lands (see the file header).
pub type Checker = ();

/// Go: `type CheckerPool interface`.
///
/// Implemented by the project system to provide checkers with request-scoped
/// lifetime and reclamation. The returned checker must not be accessed
/// concurrently; each acquisition is exclusive. Acquisitions are not
/// reentrant, even when they share a request ID. Callers must pass an already
/// acquired checker to nested operations instead of acquiring again. If file
/// is non-nil, the pool may use it as an affinity hint to return the same
/// checker for the same file across calls.
pub trait CheckerPool: Send + Sync {
    /// Go: `GetChecker(ctx context.Context, file *ast.SourceFile) (*checker.Checker, func())`.
    ///
    /// PORT: returns the `Checker` alias without the release closure — the
    /// checker wave reintroduces both (`(Arc<Checker>, ReleaseFn)`) with the
    /// locking machinery (see the file header).
    fn get_checker(&self, file: Option<&SourceFile>) -> std::sync::Arc<Checker>;
}

/*
Checker association is a balanced graph-partitioning problem (Go's comment):

  - A vertex is a source file.
  - An undirected edge connects two files for each resolved, in-program import
    entry between them. Multiple entries may connect the same pair and
    therefore strengthen their affinity. Self-imports and unresolved or
    external targets do not create edges.
  - A partition is a checker with its own symbol, type, and instantiation
    caches.

Putting related files on the same checker reduces duplicated cache
construction, but concentrating too many roots on one checker increases the
parallel critical path. We use weighted FENNEL to trade off those objectives:

  affinity(partition) - alpha * incrementalLoadPenalty(partition)

See Tsourakakis et al., "FENNEL: Streaming Graph Partitioning for Massive
Scale Graphs", WSDM 2014: https://doi.org/10.1145/2556195.2556213.

FENNEL is sensitive to stream order; see Go's checkerpool.go for the
calibration notes (MUI docs / XState per-checker completion times and the
seeded random-order testing). Stream order is part of the policy below.
*/

/*
The constants below are empirical safety factors for a work proxy that cannot
observe future semantic cache construction. They were swept across
representative projects including VS Code, TypeScript, MUI docs, XState, and
Bluesky, with 2, 4, and 8 checkers (Go's comment keeps the full sweep lists):

  - 100-byte text weight divisor.
  - 4x source-file weight.
  - 16x FENNEL penalty.
  - 12x prioritized-source penalty.
  - 4-checker cutoff.

These are project-independent operating points, not formulas derived by
FENNEL. Rebenchmark the vscode, self-compiler, mui-docs, and xstate-main
scenarios in the TypeScript-benchmarking repository at 2, 4, and 8 checkers
before changing them.
*/

/// Go: the `checkerAssociation*` consts.
pub const CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR: i64 = 100;
pub const CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER: i64 = 4;
pub const CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER: i64 = 16;
pub const CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY: i64 = 12;
pub const CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT: i64 = 4;

/// Go: `type checkerAssociationPolicy struct`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckerAssociationPolicy {
    pub prioritize_source_files: bool,
    pub source_file_weight_multiplier: i64,
    pub balance_penalty_multiplier: i64,
}

/*
getCheckerAssociationPolicy selects one of three calibrated regimes (Go's
comment):

 1. Source-dominated, any checker count:
    source files first by descending weight; unmodified source-file weight;
    checkerAssociationPrioritizedSourcePenalty.
 2. Declaration-heavy, at least checkerAssociationStrongBalanceMinCheckerCount:
    program order; checkerAssociationSourceFileWeightMultiplier;
    checkerAssociationBalancePenaltyMultiplier.
 3. Declaration-heavy, fewer checkers:
    program order; unmodified source-file weight; unscaled adapted FENNEL
    penalty.

The source-dominated test is evaluated first intentionally: projects with
very little declaration work benefit from balancing source-file roots directly
even with a small checker pool.
*/
/// Go: `func getCheckerAssociationPolicy(totalWeight, declarationWeight, checkerCount) checkerAssociationPolicy`.
pub fn checker_association_policy(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: i64,
) -> CheckerAssociationPolicy {
    if should_prioritize_source_files(total_weight, declaration_weight, checker_count) {
        return CheckerAssociationPolicy {
            prioritize_source_files: true,
            source_file_weight_multiplier: 1,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
        };
    }
    if checker_count >= CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT {
        return CheckerAssociationPolicy {
            prioritize_source_files: false,
            source_file_weight_multiplier: CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER,
        };
    }
    CheckerAssociationPolicy {
        prioritize_source_files: false,
        source_file_weight_multiplier: 1,
        balance_penalty_multiplier: 1,
    }
}

/// Go: `func getCheckerAssociationsInOrder(fileWeights, adjacentFiles, fileOrder, checkerCount, penaltyMultiplier) []int`.
///
/// Partitions the import graph using a weighted adaptation of FENNEL's
/// streaming graph-partitioning objective with gamma = 3/2. Each file is
/// placed where it has the most already-placed neighbors, minus the
/// incremental convex load penalty. The published alpha = m*sqrt(k)/n^(3/2)
/// becomes m*sqrt(k)/W^(3/2), where W is total estimated checker work.
/// penaltyMultiplier applies the empirical safety factor selected by
/// getCheckerAssociationPolicy.
///
/// A nil order means stable program order. The preferred maximum checker
/// weight is the larger of the largest file and roughly 101% of average. If
/// no checker can accept a file under that bound, the file is assigned to the
/// least-loaded checker. The 1% slack permits discrete files to pack near the
/// average while preventing affinity from deliberately creating meaningful
/// estimated imbalance. Ties are deterministic.
pub fn checker_associations_in_order(
    file_weights: &[i64],
    adjacent_files: &[Vec<i64>],
    file_order: Option<&[i64]>,
    checker_count: i64,
    penalty_multiplier: i64,
) -> Option<Vec<i64>> {
    if file_weights.is_empty() {
        // Go: returns nil.
        return None;
    }

    let mut total_weight: i64 = 0;
    let mut max_file_weight: i64 = 0;
    let mut edge_count: i64 = 0;
    for (i, weight) in file_weights.iter().enumerate() {
        total_weight += weight;
        max_file_weight = max_file_weight.max(*weight);
        edge_count += adjacent_files.get(i).map(Vec::len).unwrap_or(0) as i64;
    }

    let mut associations: Vec<i64> = vec![-1; file_weights.len()];
    let mut checker_weights = vec![0i64; checker_count as usize];
    let average_checker_weight = (total_weight + checker_count - 1) / checker_count;
    let max_checker_weight =
        max_file_weight.max(average_checker_weight + average_checker_weight / 100);
    let total_weight_float = total_weight as f64;
    let alpha = penalty_multiplier as f64 * (edge_count / 2) as f64
        * (checker_count as f64).sqrt()
        / (total_weight_float * total_weight_float.sqrt());
    let mut neighbor_counts = vec![0i64; checker_count as usize];

    for position in 0..file_weights.len() {
        let file_index = match file_order {
            Some(order) => order[position] as usize,
            None => position,
        };

        for neighbor_count in neighbor_counts.iter_mut() {
            *neighbor_count = 0;
        }
        for &adjacent_file in adjacent_files
            .get(file_index)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let checker_index = associations[adjacent_file as usize];
            if checker_index >= 0 {
                neighbor_counts[checker_index as usize] += 1;
            }
        }

        let mut best_checker: i64 = -1;
        let mut best_score = f64::NEG_INFINITY;
        for (checker_index, &checker_weight) in checker_weights.iter().enumerate() {
            if checker_weight + file_weights[file_index] > max_checker_weight {
                continue;
            }
            let old_weight = checker_weight as f64;
            let new_weight = (checker_weight + file_weights[file_index]) as f64;
            let new_penalty = new_weight * new_weight.sqrt();
            let old_penalty = old_weight * old_weight.sqrt();
            let penalty = alpha * (new_penalty - old_penalty);
            let score = neighbor_counts[checker_index] as f64 - penalty;
            if score > best_score
                || score == best_score
                    && (best_checker < 0 || checker_weight < checker_weights[best_checker as usize])
            {
                best_checker = checker_index as i64;
                best_score = score;
            }
        }
        if best_checker < 0 {
            best_checker = 0;
            for (checker_index, &checker_weight) in checker_weights.iter().enumerate().skip(1) {
                if checker_weight < checker_weights[best_checker as usize] {
                    best_checker = checker_index as i64;
                }
            }
        }
        associations[file_index] = best_checker;
        checker_weights[best_checker as usize] += file_weights[file_index];
    }
    Some(associations)
}

/// Go: `func getCheckerAssociationOrder(fileWeights, isDeclarationFile, prioritizeSourceFiles) []int`.
///
/// Places source files before declarations and orders each group by
/// descending estimated work. This exposes expensive semantic roots early,
/// when all checker loads are still available. Returning nil preserves
/// program order without allocating an index array. Program order is itself a
/// locality choice: it preserves deterministic groups produced during program
/// construction and was consistently safer for declaration-heavy projects.
pub fn checker_association_order(
    file_weights: &[i64],
    is_declaration_file: &[bool],
    prioritize_source_files: bool,
) -> Option<Vec<i64>> {
    if !prioritize_source_files {
        // Go: returns nil.
        return None;
    }
    let mut file_order: Vec<i64> = (0..file_weights.len() as i64).collect();
    // Go: sort.Slice is not stable; the comparator's total order (weight
    // desc, then index asc) makes the result deterministic, so a stable sort
    // produces the identical permutation.
    file_order.sort_by(|&left, &right| {
        let (left, right) = (left as usize, right as usize);
        if is_declaration_file[left] != is_declaration_file[right] {
            return if !is_declaration_file[left] {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if file_weights[left] != file_weights[right] {
            return file_weights[right].cmp(&file_weights[left]);
        }
        left.cmp(&right)
    });
    Some(file_order)
}

/// Go: `func getCheckerAssociationBaseWeight(nodeCount, textLength) int`.
pub fn checker_association_base_weight(node_count: i64, text_length: i64) -> i64 {
    (node_count + text_length / CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR).max(1)
}

/// Go: `func shouldPrioritizeSourceFiles(totalWeight, declarationWeight, checkerCount) bool`.
///
/// Reports whether all declaration-file base work is at most half of one
/// average checker load:
///
/// `declarationWeight <= totalWeight / (2 * checkerCount)`
///
/// This threshold separated source-dominated projects such as VS Code from
/// projects where declaration locality remained important, such as MUI docs,
/// TypeScript, and XState. Delaying at most half a checker-load of declarations
/// was the stable boundary in the cross-project sweeps.
pub fn should_prioritize_source_files(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: i64,
) -> bool {
    declaration_weight * checker_count * 2 <= total_weight
}

/// Go: `func getCheckerAssociationWeights(baseWeights, importCounts) []int`.
///
/// Combines local syntax work with syntactic import fanout. One import unit
/// is totalBaseWeight / totalImports, so imports collectively contribute
/// approximately the same vertex weight as syntax. Syntactic imports are
/// deliberately broader than getImportAdjacency's resolved, in-program edges:
/// this term estimates the work of processing module references, while
/// adjacency controls checker affinity. Normalizing the term avoids a
/// project-specific vertex-weight constant.
pub fn checker_association_weights(base_weights: &[i64], import_counts: &[i64]) -> Vec<i64> {
    let mut total_base_weight: i64 = 0;
    let mut total_imports: i64 = 0;
    for (i, &base_weight) in base_weights.iter().enumerate() {
        total_base_weight += base_weight;
        total_imports += import_counts.get(i).copied().unwrap_or(0);
    }
    let mut import_weight: i64 = 0;
    if total_imports > 0 {
        import_weight = (total_base_weight / total_imports).max(1);
    }
    (0..base_weights.len())
        .map(|i| {
            base_weights[i] + import_counts.get(i).copied().unwrap_or(0) * import_weight
        })
        .collect()
}
