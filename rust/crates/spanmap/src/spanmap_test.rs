// Ported from tsc/internal/spanmap/spanmap_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go table tests with t.Run subtests are flattened into loops inside one
// #[test] per Go test function. `BenchmarkOriginalToVirtualPositionNearEnd`
// has no Rust bench harness counterpart in this workspace (Phase 0 benches
// live in tsc-bench); it is ported as a correctness smoke test with the same
// segment geometry.

use super::*;
use tsc_core::text::{TextPos, TextRange};

fn seg(
    virtual_start: TextPos,
    virtual_end: TextPos,
    original_start: TextPos,
    original_end: TextPos,
    kind: Kind,
    features: Feature,
) -> Segment {
    Segment {
        virtual_start,
        virtual_end,
        original_start,
        original_end,
        kind,
        features,
    }
}

#[test]
fn virtual_to_original_span_verbatim() {
    // Virtual [0,10) is a verbatim copy of original [100,110).
    let m = SpanMap::new(vec![seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL)]);

    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(3, 7));
    assert_eq!(got.pos(), 103);
    assert_eq!(got.end(), 107);
    assert_eq!(fidelity, Fidelity::Exact);
}

#[test]
fn virtual_to_original_span_atom() {
    // Virtual [0,3) is a synthesized gap; [3,14) ("MyComponent") is an atom of the original [60,71).
    let m = SpanMap::new(vec![seg(3, 14, 60, 71, Kind::ATOM, Feature::ALL)]);

    // A span inside the atom maps to the whole atom span.
    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(5, 9));
    assert_eq!(got.pos(), 60);
    assert_eq!(got.end(), 71);
    assert_eq!(fidelity, Fidelity::Atom);
}

#[test]
fn virtual_alias() {
    let m = SpanMap::new(vec![seg(3, 6, 10, 11, Kind::ALIAS, Feature::ALL)]);

    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(3, 6));
    assert_eq!(got, TextRange::new(10, 11));
    assert_eq!(fidelity, Fidelity::Atom);
    let alias = m.alias_for_virtual_span(TextRange::new(3, 6));
    assert!(alias.is_some());
    assert_eq!(alias.unwrap().kind, Kind::ALIAS);
    let partial = m.alias_for_virtual_span(TextRange::new(4, 6));
    assert!(partial.is_none());

    let data = m.marshal().unwrap();
    let decoded = unmarshal(&data).unwrap();
    assert_eq!(decoded.segments()[0].kind, Kind::ALIAS);
}

#[test]
fn virtual_to_original_span_synthesized_gap() {
    // A gap between two verbatim segments is synthesized: it maps to the insertion point (the preceding
    // segment's original end) with no fidelity.
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL),
        seg(20, 30, 200, 210, Kind::VERBATIM, Feature::ALL),
    ]);

    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(12, 15));
    assert_eq!(got.pos(), 110);
    assert_eq!(got.end(), 110);
    assert_eq!(fidelity, Fidelity::None);
}

#[test]
fn original_to_virtual_intersecting_spans_allows_uncovered_endpoints() {
    let m = SpanMap::new(vec![seg(
        10,
        20,
        100,
        110,
        Kind::VERBATIM,
        Feature::SEMANTIC_TOKENS | Feature::INLAY_HINTS,
    )]);

    for feature in [Feature::SEMANTIC_TOKENS, Feature::INLAY_HINTS] {
        let got = m.original_to_virtual_intersecting_spans(TextRange::new(90, 120), feature);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].span, TextRange::new(10, 20));
        assert_eq!(got[0].fidelity, Fidelity::Exact);
    }
}

#[test]
fn virtual_to_original_span_empty_is_synthesized() {
    // An empty map describes fully synthesized output: everything maps to the start with no fidelity.
    let m = SpanMap::new(Vec::new());
    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(5, 10));
    assert_eq!(got.pos(), 0);
    assert_eq!(got.end(), 0);
    assert_eq!(fidelity, Fidelity::None);
}

#[test]
fn virtual_to_original_span_crossing_segments() {
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::NONE),
        seg(10, 20, 200, 210, Kind::VERBATIM, Feature::NONE),
    ]);

    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(5, 15));
    assert_eq!(got.pos(), 105);
    assert_eq!(got.end(), 205);
    assert_eq!(fidelity, Fidelity::Approximate);
}

#[test]
fn virtual_to_original_span_nil_identity() {
    let m: Option<&SpanMap> = None;
    let (got, fidelity) = m.virtual_to_original_span(TextRange::new(3, 7));
    assert_eq!(got.pos(), 3);
    assert_eq!(got.end(), 7);
    assert_eq!(fidelity, Fidelity::Exact);
}

#[test]
fn virtual_to_original_position() {
    // Virtual [0,10) is a verbatim copy of original [100,110); [10,20) is an atom of original [200,210).
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL),
        seg(20, 30, 200, 210, Kind::ATOM, Feature::ALL),
    ]);

    let test_cases = [
        ("verbatim interpolates", 3, 103, Fidelity::Exact),
        ("atom maps to its start", 25, 200, Fidelity::Atom),
        ("gap maps to insertion point", 15, 110, Fidelity::None),
    ];
    for (name, pos, want, want_fidelity) in test_cases {
        let (got, fidelity) = m.virtual_to_original_position(pos);
        assert_eq!(got, want, "{name}");
        assert_eq!(fidelity, want_fidelity, "{name}");
        // VirtualToOriginalPosition must agree with VirtualToOriginalSpan on a zero-length range.
        let (span, span_fidelity) = m.virtual_to_original_span(TextRange::new(pos, pos));
        assert_eq!(got, span.pos(), "{name}");
        assert_eq!(fidelity, span_fidelity, "{name}");
    }
}

#[test]
fn virtual_to_original_position_exact() {
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL),
        seg(10, 20, 110, 120, Kind::ATOM, Feature::ALL),
        seg(20, 30, 120, 130, Kind::VERBATIM, Feature::ALL),
    ]);

    for (pos, want, ok) in [
        (5, 105, true),
        (10, 110, false),
        (15, 110, false),
        (20, 120, false),
        (25, 125, true),
    ] {
        let (got, exact_ok) = m.virtual_to_original_position_exact(pos);
        assert_eq!(got, want);
        assert_eq!(exact_ok, ok);
    }
}

#[test]
fn virtual_to_original_position_exact_rejects_discontinuous_boundary() {
    let m = SpanMap::new(vec![
        seg(0, 10, 0, 10, Kind::VERBATIM, Feature::NONE),
        seg(10, 20, 100, 110, Kind::VERBATIM, Feature::NONE),
    ]);

    let (mapped, ok) = m.virtual_to_original_position_exact(10);
    assert_eq!(mapped, 100);
    assert!(!ok);
}

#[test]
fn zero_length_spans_at_segment_ends() {
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::HOVER),
        seg(20, 30, 200, 210, Kind::VERBATIM, Feature::HOVER),
    ]);

    let (position, fidelity) = m.virtual_to_original_position(30);
    assert_eq!(position, 210);
    assert_eq!(fidelity, Fidelity::Exact);
    let (virtual_span, fidelity) = m.virtual_to_original_span(TextRange::new(30, 30));
    assert_eq!(virtual_span, TextRange::new(210, 210));
    assert_eq!(fidelity, Fidelity::Exact);

    for (name, original_end) in [("before gap", 110), ("final", 210)] {
        let positions = m.original_to_virtual_positions(original_end, Feature::HOVER);
        let spans = m.original_to_virtual_spans(TextRange::new(original_end, original_end), Feature::HOVER);
        assert_eq!(positions.len(), 1, "{name}");
        assert_eq!(spans.len(), 1, "{name}");
        assert_eq!(
            spans[0].span,
            TextRange::new(positions[0].position, positions[0].position)
        );
        assert_eq!(spans[0].fidelity, positions[0].fidelity);
    }
}

#[test]
fn map_position_nil_identity() {
    let m: Option<&SpanMap> = None;
    let (got, fidelity) = m.virtual_to_original_position(7);
    assert_eq!(got, 7);
    assert_eq!(fidelity, Fidelity::Exact);
}

#[test]
fn original_to_virtual_span_verbatim() {
    // Virtual [0,10) is a verbatim copy of original [100,110).
    let m = SpanMap::new(vec![seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL)]);

    let results = m.original_to_virtual_spans(TextRange::new(103, 107), Feature::ALL);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].span.pos(), 3);
    assert_eq!(results[0].span.end(), 7);
    assert_eq!(results[0].fidelity, Fidelity::Exact);
}

#[test]
fn original_to_virtual_span_atom() {
    // Virtual [3,14) is an atom of the original [60,71).
    let m = SpanMap::new(vec![seg(3, 14, 60, 71, Kind::ATOM, Feature::ALL)]);

    // A span inside the original atom maps to the whole virtual span.
    let results = m.original_to_virtual_spans(TextRange::new(63, 67), Feature::ALL);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].span.pos(), 3);
    assert_eq!(results[0].span.end(), 14);
    assert_eq!(results[0].fidelity, Fidelity::Atom);
}

#[test]
fn original_to_virtual_span_gap() {
    // An original range with no covering segment has no virtual counterpart.
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL),
        seg(20, 30, 200, 210, Kind::VERBATIM, Feature::ALL),
    ]);

    assert_eq!(
        m.original_to_virtual_spans(TextRange::new(150, 160), Feature::ALL).len(),
        0
    );
}

#[test]
fn original_to_virtual_span_nil_identity() {
    let m: Option<&SpanMap> = None;
    let results = m.original_to_virtual_spans(TextRange::new(3, 7), Feature::ALL);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].span.pos(), 3);
    assert_eq!(results[0].span.end(), 7);
    assert_eq!(results[0].fidelity, Fidelity::Exact);
}

#[test]
fn original_to_virtual_positions() {
    // Original [100,110) is a verbatim copy of virtual [0,10); [200,210) is an atom of virtual [20,30).
    let m = SpanMap::new(vec![
        seg(0, 10, 100, 110, Kind::VERBATIM, Feature::ALL),
        seg(20, 30, 200, 210, Kind::ATOM, Feature::ALL),
    ]);

    let test_cases = [
        ("verbatim interpolates", 103, 3, Fidelity::Exact),
        ("atom maps to its start", 205, 20, Fidelity::Atom),
        ("gap has no projection", 150, 0, Fidelity::None),
    ];
    for (name, pos, want, want_fidelity) in test_cases {
        let positions = m.original_to_virtual_positions(pos, Feature::ALL);
        let spans = m.original_to_virtual_spans(TextRange::new(pos, pos), Feature::ALL);
        if want_fidelity.is_none() {
            assert_eq!(positions.len(), 0, "{name}");
            assert_eq!(spans.len(), 0, "{name}");
            continue;
        }
        assert_eq!(positions.len(), 1, "{name}");
        assert_eq!(positions[0].position, want, "{name}");
        assert_eq!(positions[0].fidelity, want_fidelity, "{name}");
        assert_eq!(spans.len(), 1, "{name}");
        assert_eq!(spans[0].span.pos(), want, "{name}");
        assert_eq!(spans[0].fidelity, want_fidelity, "{name}");
    }
}

#[test]
fn original_to_virtual_positions_at_endpoint() {
    let m = SpanMap::new(vec![
        seg(2, 5, 10, 13, Kind::VERBATIM, Feature::COMPLETION),
        seg(8, 11, 13, 16, Kind::VERBATIM, Feature::COMPLETION),
        seg(20, 23, 30, 35, Kind::ATOM, Feature::COMPLETION),
    ]);

    assert_eq!(
        m.original_to_virtual_positions(13, Feature::COMPLETION),
        vec![
            MappedPosition { position: 5, fidelity: Fidelity::Exact },
            MappedPosition { position: 8, fidelity: Fidelity::Exact },
        ]
    );
    assert_eq!(
        m.original_to_virtual_positions(35, Feature::COMPLETION),
        vec![MappedPosition { position: 23, fidelity: Fidelity::Atom }]
    );

    let filtered = SpanMap::new(vec![
        seg(20, 23, 10, 13, Kind::VERBATIM, Feature::HOVER),
        seg(2, 5, 13, 16, Kind::VERBATIM, Feature::COMPLETION),
    ]);
    assert_eq!(
        filtered.original_to_virtual_positions(13, Feature::ALL),
        vec![
            MappedPosition { position: 2, fidelity: Fidelity::Exact },
            MappedPosition { position: 23, fidelity: Fidelity::Exact },
        ]
    );
    assert_eq!(
        filtered.original_to_virtual_positions(13, Feature::COMPLETION),
        vec![MappedPosition { position: 2, fidelity: Fidelity::Exact }]
    );
}

#[test]
fn original_to_virtual_duplicate_group() {
    let m = SpanMap::new(vec![
        seg(0, 3, 10, 13, Kind::VERBATIM, Feature::DEFINITION),
        seg(10, 13, 10, 13, Kind::VERBATIM, Feature::HOVER),
        seg(20, 25, 10, 13, Kind::ATOM, Feature::DEFINITION),
    ]);

    let semantic = m.original_to_virtual_positions(11, Feature::HOVER);
    assert_eq!(semantic.len(), 1);
    assert_eq!(semantic[0].position, 11);
    assert_eq!(semantic[0].fidelity, Fidelity::Exact);

    let navigation = m.original_to_virtual_positions(11, Feature::DEFINITION);
    assert_eq!(navigation.len(), 2);
    assert_eq!(navigation[0].position, 1);
    assert_eq!(navigation[0].fidelity, Fidelity::Exact);
    assert_eq!(navigation[1].position, 20);
    assert_eq!(navigation[1].fidelity, Fidelity::Atom);

    let spans = m.original_to_virtual_spans(TextRange::new(10, 13), Feature::DEFINITION);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].span.pos(), 0);
    assert_eq!(spans[0].span.end(), 3);
    assert_eq!(spans[1].span.pos(), 20);
    assert_eq!(spans[1].span.end(), 25);
}

#[test]
fn original_to_virtual_overlapping_spans() {
    let m = SpanMap::new(vec![
        seg(0, 6, 0, 6, Kind::VERBATIM, Feature::HOVER),
        seg(10, 12, 2, 4, Kind::VERBATIM, Feature::HOVER),
        seg(20, 24, 3, 7, Kind::VERBATIM, Feature::HOVER),
    ]);

    assert_eq!(
        m.original_to_virtual_positions(3, Feature::HOVER),
        vec![
            MappedPosition { position: 3, fidelity: Fidelity::Exact },
            MappedPosition { position: 11, fidelity: Fidelity::Exact },
            MappedPosition { position: 20, fidelity: Fidelity::Exact },
        ]
    );
    let spans = m.original_to_virtual_spans(TextRange::new(3, 4), Feature::HOVER);
    let want_spans = [
        MappedSpan { span: TextRange::new(3, 4), fidelity: Fidelity::Exact },
        MappedSpan { span: TextRange::new(11, 12), fidelity: Fidelity::Exact },
        MappedSpan { span: TextRange::new(20, 21), fidelity: Fidelity::Exact },
    ];
    assert_eq!(spans.len(), want_spans.len());
    for i in 0..spans.len() {
        assert_eq!(spans[i], want_spans[i]);
    }
}

#[test]
fn original_to_virtual_position_finds_early_covering_segment() {
    // Binary search lands near [90,95), which does not contain 97. The interval index must still find the
    // earlier [0,100) segment without scanning every segment whose start precedes the query.
    let m = SpanMap::new(vec![
        seg(0, 100, 0, 100, Kind::VERBATIM, Feature::HOVER),
        seg(100, 105, 80, 85, Kind::VERBATIM, Feature::HOVER),
        seg(105, 110, 90, 95, Kind::VERBATIM, Feature::HOVER),
        seg(110, 113, 100, 103, Kind::VERBATIM, Feature::HOVER),
    ]);

    assert_eq!(
        m.original_to_virtual_positions(97, Feature::HOVER),
        vec![MappedPosition { position: 97, fidelity: Fidelity::Exact }]
    );
    let spans = m.original_to_virtual_spans(TextRange::new(97, 98), Feature::HOVER);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        spans[0],
        MappedSpan { span: TextRange::new(97, 98), fidelity: Fidelity::Exact }
    );

    // Point lookup includes both sides of a shared endpoint, including an early interval found through the
    // max-end tree. Nonempty span lookup treats segment ends as exclusive and uses only the right segment.
    assert_eq!(
        m.original_to_virtual_positions(100, Feature::HOVER),
        vec![
            MappedPosition { position: 100, fidelity: Fidelity::Exact },
            MappedPosition { position: 110, fidelity: Fidelity::Exact },
        ]
    );
    let spans = m.original_to_virtual_spans(TextRange::new(100, 101), Feature::HOVER);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        spans[0],
        MappedSpan { span: TextRange::new(110, 111), fidelity: Fidelity::Exact }
    );
}

#[test]
fn original_to_virtual_position_near_end_benchmark_smoke() {
    // PORT of BenchmarkOriginalToVirtualPositionNearEnd: same 10,000-segment
    // geometry and worst-case (last) lookup, run as a correctness smoke test.
    const SEGMENT_COUNT: usize = 10_000;
    let mut segments = Vec::with_capacity(SEGMENT_COUNT);
    for i in 0..SEGMENT_COUNT {
        let start = (2 * i) as TextPos;
        segments.push(seg(start, start + 1, start, start + 1, Kind::VERBATIM, Feature::HOVER));
    }
    let m = SpanMap::new(segments);
    let position = (2 * (SEGMENT_COUNT - 1)) as TextPos;
    // Build the lazy index outside the loop (Go: outside the benchmark timer).
    let _ = m.original_to_virtual_positions(position, Feature::HOVER);
    for _ in 0..1_000 {
        assert_eq!(
            m.original_to_virtual_positions(position, Feature::HOVER),
            vec![MappedPosition { position, fidelity: Fidelity::Exact }]
        );
    }
}

#[test]
fn original_to_virtual_overlap_falls_back_from_disabled_container() {
    let m = SpanMap::new(vec![
        seg(0, 6, 0, 6, Kind::VERBATIM, Feature::DEFINITION),
        seg(10, 13, 0, 3, Kind::VERBATIM, Feature::HOVER),
        seg(13, 16, 3, 6, Kind::VERBATIM, Feature::HOVER),
    ]);

    let spans = m.original_to_virtual_spans(TextRange::new(1, 5), Feature::HOVER);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        spans[0],
        MappedSpan { span: TextRange::new(11, 15), fidelity: Fidelity::Approximate }
    );
}

#[test]
fn original_to_virtual_cross_group_projections() {
    let m = SpanMap::new(vec![
        seg(0, 2, 0, 2, Kind::VERBATIM, Feature::HOVER),
        seg(2, 4, 2, 4, Kind::VERBATIM, Feature::HOVER),
        seg(10, 12, 0, 2, Kind::VERBATIM, Feature::HOVER),
        seg(12, 14, 2, 4, Kind::VERBATIM, Feature::HOVER),
    ]);

    let spans = m.original_to_virtual_spans(TextRange::new(1, 3), Feature::HOVER);
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].span, TextRange::new(1, 3));
    assert_eq!(spans[1].span, TextRange::new(11, 13));
    for mapped in &spans {
        assert_eq!(mapped.fidelity, Fidelity::Approximate);
    }
}

#[test]
fn original_to_virtual_explicit_zero_features() {
    let m = SpanMap::new(vec![seg(0, 3, 10, 13, Kind::VERBATIM, Feature::NONE)]);

    assert_eq!(m.original_to_virtual_positions(11, Feature::HOVER).len(), 0);
    assert_eq!(m.original_to_virtual_positions(11, Feature::DEFINITION).len(), 0);
    assert_eq!(
        m.original_to_virtual_spans(TextRange::new(10, 13), Feature::HOVER).len(),
        0
    );

    let data = m.marshal().unwrap();
    assert_eq!(String::from_utf8(data.clone()).unwrap(), "[[0,3,10,3,0,0]]");
    let decoded = unmarshal(&data).unwrap();
    let segments = decoded.segments();
    assert_eq!(segments[0].features, Feature::NONE);

    let legacy = unmarshal(b"[[0,3,10,3,0]]").unwrap();
    assert_eq!(legacy.segments()[0].features, Feature::ALL);
    assert_eq!(legacy.original_to_virtual_positions(11, Feature::HOVER).len(), 1);
}

#[test]
fn feature_participation_original_and_virtual() {
    let m = SpanMap::new(vec![
        seg(0, 3, 10, 13, Kind::VERBATIM, Feature::HOVER),
        seg(3, 6, 20, 23, Kind::VERBATIM, Feature::COMPLETION),
    ]);

    assert_eq!(m.original_to_virtual_positions(11, Feature::HOVER).len(), 1);
    assert_eq!(m.original_to_virtual_positions(11, Feature::COMPLETION).len(), 0);

    let (mapped, fidelity) = m.virtual_to_original_span_for_feature(TextRange::new(0, 3), Feature::HOVER);
    assert_eq!(mapped, TextRange::new(10, 13));
    assert_eq!(fidelity, Fidelity::Exact);
    let (_, fidelity) = m.virtual_to_original_span_for_feature(TextRange::new(0, 3), Feature::COMPLETION);
    assert_eq!(fidelity, Fidelity::None);

    // Diagnostics and edit safety use unfiltered geometry and cannot be disabled by feature flags.
    let (mapped, fidelity) = m.virtual_to_original_span(TextRange::new(0, 3));
    assert_eq!(mapped, TextRange::new(10, 13));
    assert_eq!(fidelity, Fidelity::Exact);
}

#[test]
fn original_to_virtual_span_round_trip() {
    // Original spans are out of order relative to virtual spans, exercising the reverse index.
    let m = SpanMap::new(vec![
        seg(0, 10, 200, 210, Kind::VERBATIM, Feature::ALL),
        seg(10, 20, 100, 110, Kind::VERBATIM, Feature::ALL),
    ]);

    for r in [TextRange::new(2, 8), TextRange::new(12, 18)] {
        let (orig, fidelity) = m.virtual_to_original_span(r);
        assert_eq!(fidelity, Fidelity::Exact);
        let back = m.original_to_virtual_spans(orig, Feature::ALL);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].fidelity, Fidelity::Exact);
        assert_eq!(back[0].span.pos(), r.pos());
        assert_eq!(back[0].span.end(), r.end());
    }
}

#[test]
fn marshal_round_trip() {
    let original = SpanMap::new(vec![
        seg(3, 14, 60, 71, Kind::ATOM, Feature::NONE),
        seg(14, 24, 71, 81, Kind::VERBATIM, Feature::NONE),
    ]);

    let data = original.marshal().unwrap();
    let decoded = unmarshal(&data).unwrap();

    for r in [TextRange::new(1, 2), TextRange::new(4, 10), TextRange::new(16, 20)] {
        let (want_range, want_fidelity) = original.virtual_to_original_span(r);
        let (got_range, got_fidelity) = decoded.virtual_to_original_span(r);
        assert_eq!(got_range, want_range);
        assert_eq!(got_fidelity, want_fidelity);
    }
}

#[test]
fn validate() {
    const TRANSFORMED: &str = "const greeting = 1;\n";
    const ORIGINAL: &str = "<x>const greeting = 1;\n</x>";
    let script_start = 3; // index of "const" in original

    struct Case {
        name: &'static str,
        segs: Vec<Segment>,
        want_kind: MappingErrorKind,
        want_ok: bool,
    }
    let cases = vec![
        Case {
            name: "valid verbatim",
            segs: vec![seg(
                0,
                TRANSFORMED.len() as TextPos,
                script_start as TextPos,
                (script_start + TRANSFORMED.len()) as TextPos,
                Kind::VERBATIM,
                Feature::NONE,
            )],
            want_kind: MappingErrorKind::Overlap,
            want_ok: true,
        },
        Case { name: "empty is valid", segs: Vec::new(), want_kind: MappingErrorKind::Overlap, want_ok: true },
        Case {
            name: "gap is allowed",
            segs: vec![seg(3, TRANSFORMED.len() as TextPos, 0, 0, Kind::ATOM, Feature::NONE)],
            want_kind: MappingErrorKind::Overlap,
            want_ok: true,
        },
        Case {
            name: "overlap",
            segs: vec![
                seg(0, 10, 0, 0, Kind::ATOM, Feature::NONE),
                seg(5, TRANSFORMED.len() as TextPos, 0, 0, Kind::ATOM, Feature::NONE),
            ],
            want_kind: MappingErrorKind::Overlap,
            want_ok: false,
        },
        Case {
            name: "original out of bounds",
            segs: vec![seg(
                0,
                TRANSFORMED.len() as TextPos,
                0,
                (ORIGINAL.len() + 10) as TextPos,
                Kind::ATOM,
                Feature::NONE,
            )],
            want_kind: MappingErrorKind::OutOfBounds,
            want_ok: false,
        },
        Case {
            name: "verbatim text mismatch",
            segs: vec![seg(
                0,
                TRANSFORMED.len() as TextPos,
                0,
                TRANSFORMED.len() as TextPos,
                Kind::VERBATIM,
                Feature::NONE,
            )],
            want_kind: MappingErrorKind::VerbatimMismatch,
            want_ok: false,
        },
        Case {
            name: "unknown kind",
            segs: vec![seg(0, 1, 0, 1, Kind::from_i32(3), Feature::NONE)],
            want_kind: MappingErrorKind::Kind,
            want_ok: false,
        },
    ];

    for case in cases {
        let problem = SpanMap::new(case.segs).validate(TRANSFORMED, ORIGINAL);
        if case.want_ok {
            assert!(problem.is_none(), "{}: expected valid, got {problem:?}", case.name);
            continue;
        }
        let problem = problem.expect(case.name);
        assert_eq!(problem.kind, case.want_kind, "{}", case.name);
    }
}

#[test]
fn validate_original_overlap_and_features() {
    struct Case {
        name: &'static str,
        segments: Vec<Segment>,
        want_kind: MappingErrorKind,
        valid: bool,
    }
    let cases = vec![
        Case {
            name: "identical duplicate group",
            segments: vec![
                seg(0, 3, 0, 3, Kind::VERBATIM, Feature::DEFINITION),
                seg(3, 6, 0, 3, Kind::VERBATIM, Feature::HOVER),
            ],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "partial original overlap is valid",
            segments: vec![
                seg(0, 3, 0, 3, Kind::ATOM, Feature::NONE),
                seg(3, 6, 2, 5, Kind::ATOM, Feature::NONE),
            ],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "nested original overlap is valid",
            segments: vec![
                seg(0, 5, 0, 5, Kind::ATOM, Feature::NONE),
                seg(5, 6, 1, 4, Kind::ATOM, Feature::NONE),
            ],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "duplicate without explicit features is tolerant",
            segments: vec![
                seg(0, 3, 0, 3, Kind::ATOM, Feature::NONE),
                seg(3, 6, 0, 3, Kind::ATOM, Feature::DEFINITION),
            ],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "duplicate with shared feature members is tolerant",
            segments: vec![
                seg(0, 3, 0, 3, Kind::ATOM, Feature::HOVER),
                seg(3, 6, 0, 3, Kind::ATOM, Feature::HOVER | Feature::DEFINITION),
            ],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "features on sole cover are valid",
            segments: vec![seg(0, 3, 0, 3, Kind::ATOM, Feature::DEFINITION)],
            want_kind: MappingErrorKind::Overlap,
            valid: true,
        },
        Case {
            name: "unknown feature flag",
            segments: vec![seg(0, 3, 0, 3, Kind::ATOM, Feature::from_i32(1 << 22))],
            want_kind: MappingErrorKind::Feature,
            valid: false,
        },
    ];

    for case in cases {
        let problem = SpanMap::new(case.segments).validate("abcabc", "abcdef");
        if case.valid {
            assert!(problem.is_none(), "{}: expected valid, got {problem:?}", case.name);
            continue;
        }
        let problem = problem.expect(case.name);
        assert_eq!(problem.kind, case.want_kind, "{}", case.name);
    }
}

#[test]
fn validate_nil_is_valid() {
    let m: Option<&SpanMap> = None;
    assert!(m.validate("abc", "abc").is_none());
}
