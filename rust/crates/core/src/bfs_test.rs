// Ported from tsc/internal/core/bfs_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go's TestBreadthFirstSearchParallel subtests are ported one-to-one as
// separate #[test] functions. PORT notes: `core.Identity` (core.go, not yet
// ported) is inlined as `|n| n.clone()`; Go's `result.Path == nil` is
// asserted as `path.is_empty()` (§4.6 — nil vs empty slice is not
// distinguished by consumers).

use std::collections::HashMap;
use std::sync::Mutex;

use tsc_collections::SyncSet;

use crate::bfs::{breadth_first_search_parallel, breadth_first_search_parallel_ex, BreadthFirstSearchOptions};

type Graph = HashMap<&'static str, Vec<&'static str>>;

// Graph: A -> B, A -> C, B -> D, C -> D
fn diamond_graph() -> Graph {
    HashMap::from([
        ("A", vec!["B", "C"]),
        ("B", vec!["D"]),
        ("C", vec!["D"]),
        ("D", vec![]),
    ])
}

fn children(graph: &Graph) -> impl Fn(&&'static str) -> Vec<&'static str> + '_ {
    move |node| graph.get(*node).cloned().unwrap_or_default()
}

#[test]
fn basic_functionality_find_specific_node() {
    let graph = diamond_graph();
    let result = breadth_first_search_parallel(
        "A",
        children(&graph),
        |node| (*node == "D", true),
    );
    assert!(result.stopped, "Expected search to stop at D");
    assert_eq!(result.path, vec!["D", "B", "A"]);
}

#[test]
fn basic_functionality_visit_all_nodes() {
    let graph = diamond_graph();
    let visited_nodes = Mutex::new(Vec::new());
    let result = breadth_first_search_parallel("A", children(&graph), |node| {
        let mut visited = visited_nodes.lock().unwrap();
        visited.push(*node);
        (false, false) // Never stop early
    });

    // Should return nil since we never return true
    assert!(!result.stopped, "Expected search to not stop early");
    assert!(result.path.is_empty(), "Expected nil path when visit function never returns true");

    // Should visit all nodes exactly once
    let mut visited_nodes = visited_nodes.into_inner().unwrap();
    visited_nodes.sort_unstable();
    assert_eq!(visited_nodes, vec!["A", "B", "C", "D"]);
}

#[test]
fn early_termination() {
    // Test that nodes below the target level are not visited
    let graph = HashMap::from([
        ("Root", vec!["L1A", "L1B"]),
        ("L1A", vec!["L2A", "L2B"]),
        ("L1B", vec!["L2C"]),
        ("L2A", vec!["L3A"]),
        ("L2B", vec![]),
        ("L2C", vec![]),
        ("L3A", vec![]),
    ]);

    let visited = SyncSet::new();
    let _ = breadth_first_search_parallel_ex(
        "Root",
        children(&graph),
        |node| (*node == "L2B", true), // Stop at level 2
        BreadthFirstSearchOptions {
            visited: Some(&visited),
            ..BreadthFirstSearchOptions::default()
        },
        |n| *n, // PORT: core.Identity inlined
    );

    assert!(visited.has("Root"), "Expected to visit Root");
    assert!(visited.has("L1A"), "Expected to visit L1A");
    assert!(visited.has("L1B"), "Expected to visit L1B");
    assert!(visited.has("L2A"), "Expected to visit L2A");
    assert!(visited.has("L2B"), "Expected to visit L2B");
    // L2C is non-deterministic
    assert!(!visited.has("L3A"), "Expected not to visit L3A");
}

#[test]
fn returns_fallback_when_no_other_result_found() {
    // Test that fallback behavior works correctly
    let graph = diamond_graph();

    let visited = SyncSet::new();
    let result = breadth_first_search_parallel_ex(
        "A",
        children(&graph),
        |node| (*node == "A", false), // Record A as a fallback, but do not stop
        BreadthFirstSearchOptions {
            visited: Some(&visited),
            ..BreadthFirstSearchOptions::default()
        },
        |n| *n, // PORT: core.Identity inlined
    );

    assert!(!result.stopped, "Expected search to not stop early");
    assert_eq!(result.path, vec!["A"]);
    assert!(visited.has("B"), "Expected to visit B");
    assert!(visited.has("C"), "Expected to visit C");
    assert!(visited.has("D"), "Expected to visit D");
}

#[test]
fn returns_a_stop_result_over_a_fallback() {
    // Test that a stop result is preferred over a fallback
    let graph = diamond_graph();

    let result = breadth_first_search_parallel("A", children(&graph), |node| match *node {
        "A" => (true, false),  // Record fallback
        "D" => (true, true),   // Stop at D
        _ => (false, false),
    });

    assert!(result.stopped, "Expected search to stop at D");
    assert_eq!(result.path, vec!["D", "B", "A"]);
}
