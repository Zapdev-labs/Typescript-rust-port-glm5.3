// Ported from tsc/internal/spanmap/spanmap.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Package spanmap provides bidirectional span-aware mapping between a content mapper's virtual text
// and its original, untransformed source. Unlike a source map, which records
// point correspondences and leaves spans and "no origin" implicit, a SpanMap records explicit segments
// for the parts of the virtual text that correspond to the original; positions not covered by any
// segment are synthesized (virtual content with no original counterpart). All positions are absolute
// offsets (core.TextPos), matching the compiler's TextRange model.

// Keep this in sync with spanMap.ts

use std::fmt;
use std::ops::{BitAnd, BitOr, BitOrAssign, Not};
use std::sync::OnceLock;

use tsc_core::text::{TextPos, TextRange};
use tsc_json as json;

/// Kind describes how positions inside a segment relate the virtual span to the original span.
///
/// PORT: Go's `type Kind int32` can carry out-of-range values from untrusted input
/// (Unmarshal tuples, hand-built segments) that Validate must report, so this is a
/// newtype over i32 rather than an enum.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Kind(i32);

impl Kind {
    /// KindVerbatim segments are length-preserving: the virtual and original spans have the same
    /// length and interior positions map 1:1 (OriginalPos = pos - VirtualStart + OriginalStart). A virtual span
    /// fully within a verbatim segment maps to an exact original span.
    pub const VERBATIM: Kind = Kind(0);
    /// KindAtom segments map a virtual span to an original span as a whole; interior positions are not
    /// interpolatable (the lengths may differ), so positions within clamp to the segment's endpoints.
    /// Used for renamed identifiers or short expressions.
    pub const ATOM: Kind = Kind(1);
    /// KindAlias has atom geometry, but additionally asserts that the virtual and original texts are
    /// names for the same logical entity. Diagnostic presentation may substitute the original name.
    pub const ALIAS: Kind = Kind(2);

    pub fn from_i32(value: i32) -> Kind {
        Kind(value)
    }

    pub fn as_i32(&self) -> i32 {
        self.0
    }

    pub fn is_valid(&self) -> bool {
        matches!(self.0, 0..=2)
    }
}

/// Feature selects which language-service operations may use a segment. Diagnostics are intentionally not
/// represented: diagnostics on virtual text may not opt out of reporting. Text edits additionally require exact
/// verbatim geometry regardless of feature participation.
///
/// PORT: Go's `type Feature int32` is a bitmask; the newtype implements the
/// subset of std ops the Go code spells with `|`, `&` and `&^`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Feature(i32);

impl Feature {
    pub const HOVER: Feature = Feature(1 << 0);
    pub const SIGNATURE_HELP: Feature = Feature(1 << 1);
    pub const COMPLETION: Feature = Feature(1 << 2);
    pub const DEFINITION: Feature = Feature(1 << 3);
    pub const TYPE_DEFINITION: Feature = Feature(1 << 4);
    pub const IMPLEMENTATION: Feature = Feature(1 << 5);
    pub const REFERENCES: Feature = Feature(1 << 6);
    pub const DOCUMENT_HIGHLIGHTS: Feature = Feature(1 << 7);
    pub const RENAME: Feature = Feature(1 << 8);
    pub const CALL_HIERARCHY: Feature = Feature(1 << 9);
    pub const CODE_ACTIONS: Feature = Feature(1 << 10);
    pub const FORMATTING: Feature = Feature(1 << 11);
    pub const INLAY_HINTS: Feature = Feature(1 << 12);
    pub const SEMANTIC_TOKENS: Feature = Feature(1 << 13);
    pub const FOLDING_RANGES: Feature = Feature(1 << 14);
    pub const SELECTION_RANGES: Feature = Feature(1 << 15);
    pub const LINKED_EDITING: Feature = Feature(1 << 16);
    pub const AUTO_INSERT: Feature = Feature(1 << 17);
    pub const DOCUMENT_SYMBOLS: Feature = Feature(1 << 18);
    pub const CODE_LENS: Feature = Feature(1 << 19);
    pub const NONE: Feature = Feature(0);
    pub const ALL: Feature = Feature((Self::CODE_LENS.0 << 1) - 1);

    pub fn from_i32(value: i32) -> Feature {
        Feature(value)
    }

    pub fn as_i32(&self) -> i32 {
        self.0
    }

    /// Reports whether any bit of `other` is set in this mask.
    pub fn intersects(self, other: Feature) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for Feature {
    type Output = Feature;
    fn bitor(self, rhs: Feature) -> Feature {
        Feature(self.0 | rhs.0)
    }
}

impl BitOrAssign for Feature {
    fn bitor_assign(&mut self, rhs: Feature) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Feature {
    type Output = Feature;
    fn bitand(self, rhs: Feature) -> Feature {
        Feature(self.0 & rhs.0)
    }
}

impl Not for Feature {
    type Output = Feature;
    fn not(self) -> Feature {
        Feature(!self.0)
    }
}

const FEATURE_MASK: Feature = Feature::ALL;

/// Fidelity describes how faithfully a mapped span reflects the original.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Fidelity {
    /// FidelityExact means the span fell entirely within a single verbatim segment and maps precisely.
    #[default]
    Exact,
    /// FidelityAtom means the span fell within a single atom segment and maps to that atom's span.
    Atom,
    /// FidelityApproximate means the span crossed segment boundaries; its endpoints were mapped and clamped.
    Approximate,
    /// FidelityNone means the span had no original counterpart (it was entirely synthesized).
    None,
}

impl Fidelity {
    /// IsExact reports whether the mapping was fully faithful — the input fell within a single verbatim span —
    /// so the result maps 1:1 and can host a text edit written back to the original.
    pub fn is_exact(&self) -> bool {
        *self == Fidelity::Exact
    }

    /// IsSingleSegment reports whether the input fell within one segment, verbatim or atom, so the result is a
    /// concrete location rather than a best-effort approximation across boundaries or a synthesized gap.
    pub fn is_single_segment(&self) -> bool {
        *self == Fidelity::Exact || *self == Fidelity::Atom
    }

    /// IsNone reports whether the input had no original counterpart, meaning the mapped result is a synthesized
    /// gap that does not correspond to any location in the original text.
    pub fn is_none(&self) -> bool {
        *self == Fidelity::None
    }
}

/// Segment maps the half-open virtual range [VirtualStart, VirtualEnd) to the half-open original range
/// [OriginalStart, OriginalEnd). Features controls language-service participation; diagnostics and exact edit mapping
/// deliberately bypass it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Segment {
    pub virtual_start: TextPos,
    pub virtual_end: TextPos,
    pub original_start: TextPos,
    pub original_end: TextPos,
    pub kind: Kind,
    pub features: Feature,
}

/// MappedPosition is one virtual projection of an original position and its mapping fidelity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MappedPosition {
    pub position: TextPos,
    pub fidelity: Fidelity,
}

/// MappedSpan is one virtual projection of an original range and its mapping fidelity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MappedSpan {
    pub span: TextRange,
    pub fidelity: Fidelity,
}

/// SpanMap is a sparse, ordered set of segments over a content mapper's virtual text. Segments do not
/// need to cover the whole text: any virtual position not inside a segment is synthesized (it has no
/// original counterpart). An empty SpanMap therefore describes fully synthesized virtual text.
pub struct SpanMap {
    segments: Vec<Segment>,

    // PORT: Go guards lazy construction of the interval index used for
    // original-to-virtual lookups with a sync.Once; OnceLock is the analog.
    original_index: OnceLock<OriginalIndex>,
}

/// Validation failures. A content mapper is required to provide a valid span map; these describe the
/// ways a map can be malformed, so the compiler can attribute the failure to the mapper precisely and
/// point the mapper's author at the offending location.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum MappingErrorKind {
    #[default]
    /// MappingErrorKindOverlap means the segments overlap, run backwards, or extend past the end of the
    /// virtual text (they must be ordered and disjoint in virtual space).
    Overlap,
    /// MappingErrorKindOutOfBounds means a segment's original span lies outside the original text.
    OutOfBounds,
    /// MappingErrorKindVerbatimMismatch means a verbatim segment's virtual and original text differ.
    VerbatimMismatch,
    /// MappingErrorKindKind means a segment uses an unsupported mapping kind.
    Kind,
    /// MappingErrorKindFeature means a feature annotation contains unsupported flags.
    Feature,
}

/// MappingError describes a single span map validation failure, including the offsets involved so the mapper's
/// author can locate it. VirtualPos is an offset into the virtual text; OriginalPos is an offset into the
/// original content. Either may be unused (zero) depending on Kind.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MappingError {
    pub kind: MappingErrorKind,
    pub virtual_pos: TextPos,
    pub original_pos: TextPos,
}

impl fmt::Display for MappingError {
    /// Error describes the invalid mapping and the coordinate at which it was detected.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            MappingErrorKind::Overlap => write!(
                f,
                "content mapper position mappings overlap or are out of order near virtual offset {}",
                self.virtual_pos
            ),
            MappingErrorKind::OutOfBounds => write!(
                f,
                "content mapper position mapping points outside the original content at original offset {}",
                self.original_pos
            ),
            MappingErrorKind::VerbatimMismatch => write!(
                f,
                "content mapper verbatim mapping does not match the original content at virtual offset {}, original offset {}",
                self.virtual_pos, self.original_pos
            ),
            MappingErrorKind::Kind => write!(
                f,
                "content mapper position mapping has an invalid kind at virtual offset {}",
                self.virtual_pos
            ),
            MappingErrorKind::Feature => write!(
                f,
                "content mapper position mappings have invalid features near original offset {}",
                self.original_pos
            ),
        }
    }
}

impl std::error::Error for MappingError {}

/// UnmarshalError is the port of the two error returns Go's `Unmarshal` can
/// produce: the wrapped json error and the tuple-length check.
#[derive(Debug)]
pub enum UnmarshalError {
    /// The JSON payload failed to decode.
    Json(serde_json::Error),
    /// `span map segment {index}: expected 5 or 6 values, got {got}`.
    SegmentTupleLength { index: usize, got: usize },
}

impl fmt::Display for UnmarshalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnmarshalError::Json(err) => write!(f, "{err}"),
            UnmarshalError::SegmentTupleLength { index, got } => write!(
                f,
                "span map segment {index}: expected 5 or 6 values, got {got}"
            ),
        }
    }
}

impl std::error::Error for UnmarshalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UnmarshalError::Json(err) => Some(err),
            UnmarshalError::SegmentTupleLength { .. } => None,
        }
    }
}

// PORT: Go defines every mapping method on `*SpanMap` with an explicit nil
// check, so a `var m *SpanMap` receiver maps identically. Rust has no nil
// receivers, so the nil-tolerant entry points live on `Option<&SpanMap>`
// (see `SpanMapRef`); the inherent methods on `SpanMap` carry the non-nil
// logic and are unconditionally available to owners.
pub trait SpanMapRef {
    fn virtual_to_original_span(&self, r: TextRange) -> (TextRange, Fidelity);
    fn virtual_to_original_span_for_feature(&self, r: TextRange, feature: Feature) -> (TextRange, Fidelity);
    fn virtual_to_original_position(&self, pos: TextPos) -> (TextPos, Fidelity);
    fn virtual_to_original_position_exact(&self, pos: TextPos) -> (TextPos, bool);
    fn virtual_to_original_position_for_feature(&self, pos: TextPos, feature: Feature) -> (TextPos, Fidelity);
    fn alias_for_virtual_span(&self, r: TextRange) -> Option<Segment>;
    fn original_to_virtual_positions(&self, pos: TextPos, feature: Feature) -> Vec<MappedPosition>;
    fn original_to_virtual_spans(&self, r: TextRange, feature: Feature) -> Vec<MappedSpan>;
    fn original_to_virtual_intersecting_spans(&self, r: TextRange, feature: Feature) -> Vec<MappedSpan>;
    fn validate(&self, virtual_text: &str, original_text: &str) -> Option<MappingError>;
    fn segments(&self) -> Vec<Segment>;
}

impl SpanMapRef for Option<&'_ SpanMap> {
    fn virtual_to_original_span(&self, r: TextRange) -> (TextRange, Fidelity) {
        match self {
            Some(m) => m.virtual_to_original_span(r),
            // A nil SpanMap maps identically.
            None => (r, Fidelity::Exact),
        }
    }

    fn virtual_to_original_span_for_feature(&self, r: TextRange, feature: Feature) -> (TextRange, Fidelity) {
        match self {
            Some(m) => m.virtual_to_original_span_for_feature(r, feature),
            None => (r, Fidelity::Exact),
        }
    }

    fn virtual_to_original_position(&self, pos: TextPos) -> (TextPos, Fidelity) {
        match self {
            Some(m) => m.virtual_to_original_position(pos),
            None => (pos, Fidelity::Exact),
        }
    }

    fn virtual_to_original_position_exact(&self, pos: TextPos) -> (TextPos, bool) {
        match self {
            Some(m) => m.virtual_to_original_position_exact(pos),
            // PORT: Go falls through to `return mapped, fidelity == FidelityExact`.
            None => (pos, true),
        }
    }

    fn virtual_to_original_position_for_feature(&self, pos: TextPos, feature: Feature) -> (TextPos, Fidelity) {
        match self {
            Some(m) => m.virtual_to_original_position_for_feature(pos, feature),
            None => (pos, Fidelity::Exact),
        }
    }

    fn alias_for_virtual_span(&self, r: TextRange) -> Option<Segment> {
        match self {
            Some(m) => m.alias_for_virtual_span(r),
            None => None,
        }
    }

    fn original_to_virtual_positions(&self, pos: TextPos, feature: Feature) -> Vec<MappedPosition> {
        match self {
            Some(m) => m.original_to_virtual_positions(pos, feature),
            None => vec![MappedPosition { position: pos, fidelity: Fidelity::Exact }],
        }
    }

    fn original_to_virtual_spans(&self, r: TextRange, feature: Feature) -> Vec<MappedSpan> {
        match self {
            Some(m) => m.original_to_virtual_spans(r, feature),
            None => vec![MappedSpan { span: r, fidelity: Fidelity::Exact }],
        }
    }

    fn original_to_virtual_intersecting_spans(&self, r: TextRange, feature: Feature) -> Vec<MappedSpan> {
        match self {
            Some(m) => m.original_to_virtual_intersecting_spans(r, feature),
            None => vec![MappedSpan { span: r, fidelity: Fidelity::Exact }],
        }
    }

    fn validate(&self, virtual_text: &str, original_text: &str) -> Option<MappingError> {
        match self {
            Some(m) => m.validate(virtual_text, original_text),
            // A nil map is valid.
            None => None,
        }
    }

    fn segments(&self) -> Vec<Segment> {
        match self {
            Some(m) => m.segments(),
            None => Vec::new(),
        }
    }
}

// PORT: Go's segmentIndexAt returns (int, bool) where -1 is the "no previous
// segment" sentinel; the port models that as Option<usize>. Every "inside"
// result carries Some(index), so this helper documents and enforces the
// invariant at the few call sites that need the index after checking.
#[inline]
fn expect_index(index: Option<usize>) -> usize {
    index.expect("segmentIndexAt returns an index when inside")
}

impl SpanMap {
    /// New builds a SpanMap from segments, sorted by virtual start. Segments describe only the parts of the
    /// virtual text that correspond to the original; anything not covered maps as synthesized.
    pub fn new(mut segments: Vec<Segment>) -> SpanMap {
        segments.sort_by_key(|s| s.virtual_start);
        SpanMap { segments, original_index: OnceLock::new() }
    }

    /// Segments returns the map's segments ordered by virtual start.
    pub fn segments(&self) -> Vec<Segment> {
        self.segments.clone()
    }

    /// Validate enforces the content-mapper span map contract against the virtual and original text: the
    /// segments must be ordered and disjoint in virtual space and stay within the virtual text, every
    /// original span must lie within the original text, and every verbatim segment's text must match the
    /// original exactly. Gaps are allowed (they map as synthesized) and an empty map is valid. It returns the
    /// first violation found, or None if the map is valid.
    pub fn validate(&self, virtual_text: &str, original_text: &str) -> Option<MappingError> {
        let virtual_len = TextPos::try_from(virtual_text.len()).unwrap_or(i32::MAX);
        let orig_len = TextPos::try_from(original_text.len()).unwrap_or(i32::MAX);
        let mut previous_virtual_end: TextPos = 0;
        for s in &self.segments {
            if s.virtual_start < previous_virtual_end
                || s.virtual_end < s.virtual_start
                || s.virtual_end > virtual_len
            {
                return Some(MappingError {
                    kind: MappingErrorKind::Overlap,
                    virtual_pos: s.virtual_start,
                    original_pos: 0,
                });
            }
            previous_virtual_end = s.virtual_end;
            if s.original_start < 0
                || s.original_end < s.original_start
                || s.original_end > orig_len
            {
                return Some(MappingError {
                    kind: MappingErrorKind::OutOfBounds,
                    virtual_pos: s.virtual_start,
                    original_pos: s.original_end,
                });
            }
            if !s.kind.is_valid() {
                return Some(MappingError {
                    kind: MappingErrorKind::Kind,
                    virtual_pos: s.virtual_start,
                    original_pos: s.original_start,
                });
            }
            if s.kind == Kind::VERBATIM {
                let virtual_slice = &virtual_text[s.virtual_start as usize..s.virtual_end as usize];
                let original_slice = &original_text[s.original_start as usize..s.original_end as usize];
                if s.virtual_end - s.virtual_start != s.original_end - s.original_start
                    || virtual_slice != original_slice
                {
                    return Some(MappingError {
                        kind: MappingErrorKind::VerbatimMismatch,
                        virtual_pos: s.virtual_start,
                        original_pos: s.original_start,
                    });
                }
            }
            if s.features & !FEATURE_MASK != Feature::NONE {
                return Some(MappingError {
                    kind: MappingErrorKind::Feature,
                    virtual_pos: s.virtual_start,
                    original_pos: s.original_start,
                });
            }
        }
        None
    }

    /// VirtualToOriginalSpan maps a virtual range to an original range, along with the fidelity of the result. A virtual
    /// range that lies entirely in a gap between segments (or in an empty map) is synthesized: it maps to the
    /// insertion point in the original with FidelityNone.
    pub fn virtual_to_original_span(&self, r: TextRange) -> (TextRange, Fidelity) {
        let virtual_start = r.pos();
        let virtual_end = r.end().max(virtual_start);
        if virtual_start == virtual_end {
            let (position, fidelity) = self.virtual_to_original_position(virtual_start);
            return (TextRange::new(position, position), fidelity);
        }

        let start = self.segment_index_at(virtual_start);
        let end_probe = virtual_end - 1;
        let end = self.segment_index_at(end_probe);

        if start == end {
            let (idx, inside) = start;
            if inside {
                let seg = &self.segments[expect_index(idx)];
                if seg.kind == Kind::VERBATIM {
                    let orig_start = clamp(
                        seg.original_start + (virtual_start - seg.virtual_start),
                        seg.original_start,
                        seg.original_end,
                    );
                    let orig_end = clamp(
                        seg.original_start + (virtual_end - seg.virtual_start),
                        orig_start,
                        seg.original_end,
                    );
                    return (TextRange::new(orig_start, orig_end), Fidelity::Exact);
                }
                return (TextRange::new(seg.original_start, seg.original_end), Fidelity::Atom);
            }
            // Entirely within a single synthesized gap.
            let pos = self.insertion_point(idx);
            return (TextRange::new(pos, pos), Fidelity::None);
        }

        let orig_start = self.map_low(virtual_start, start.0, start.1);
        let orig_end = self.map_high(virtual_end, end.0, end.1).max(orig_start);
        (TextRange::new(orig_start, orig_end), Fidelity::Approximate)
    }

    /// VirtualToOriginalSpanForFeature maps r only when every virtual position in the non-empty range is
    /// covered by contiguous segments participating in feature. A zero-length range requires its containing
    /// segment to participate. Diagnostics and edit write-back intentionally use VirtualToOriginalSpan instead.
    pub fn virtual_to_original_span_for_feature(
        &self,
        r: TextRange,
        feature: Feature,
    ) -> (TextRange, Fidelity) {
        let (mapped, fidelity) = self.virtual_to_original_span(r);
        if self.virtual_span_supports_feature(r, feature) {
            return (mapped, fidelity);
        }
        (mapped, Fidelity::None)
    }

    fn virtual_span_supports_feature(&self, r: TextRange, feature: Feature) -> bool {
        let start = r.pos();
        let end = r.end().max(start);
        if start == end {
            let (index, inside) = self.segment_index_at(start);
            return inside
                && supports_feature(&self.segments[expect_index(index)], feature);
        }
        let (index, inside) = self.segment_index_at(start);
        if !inside {
            return false;
        }
        let mut index = expect_index(index);
        let mut covered_through = start;
        while index < self.segments.len() && covered_through < end {
            let segment = self.segments[index];
            if segment.virtual_start > covered_through
                || segment.virtual_end <= covered_through
                || !supports_feature(&segment, feature)
            {
                return false;
            }
            covered_through = segment.virtual_end;
            index += 1;
        }
        covered_through >= end
    }

    /// VirtualToOriginalPosition maps a single virtual position to the corresponding original position, along with the
    /// fidelity of the result. It is the single-position analog of VirtualToOriginalSpan: a position in a gap (or in an empty
    /// map) is synthesized and maps to the insertion point with FidelityNone.
    pub fn virtual_to_original_position(&self, pos: TextPos) -> (TextPos, Fidelity) {
        let (idx, inside) = self.segment_index_at(pos);
        if !inside {
            return (self.insertion_point(idx), Fidelity::None);
        }
        let seg = &self.segments[expect_index(idx)];
        if seg.kind == Kind::VERBATIM {
            return (
                clamp(
                    seg.original_start + (pos - seg.virtual_start),
                    seg.original_start,
                    seg.original_end,
                ),
                Fidelity::Exact,
            );
        }
        (seg.original_start, Fidelity::Atom)
    }

    /// VirtualToOriginalPositionExact maps a position only when it is unambiguously in verbatim content.
    /// A boundary touching an atom is rejected because the same virtual position can describe either side.
    pub fn virtual_to_original_position_exact(&self, pos: TextPos) -> (TextPos, bool) {
        let (mapped, fidelity) = self.virtual_to_original_position(pos);
        if fidelity != Fidelity::Exact {
            return (mapped, false);
        }
        let (index, inside) = self.segment_index_at(pos);
        let Some(index) = index else {
            return (mapped, false);
        };
        if !inside || self.segments[index].kind != Kind::VERBATIM {
            return (mapped, false);
        }
        if index > 0 {
            let previous = self.segments[index - 1];
            if previous.virtual_end == pos
                && (previous.kind != Kind::VERBATIM
                    || previous.original_end != self.segments[index].original_start)
            {
                return (mapped, false);
            }
        }
        (mapped, true)
    }

    /// VirtualToOriginalPositionForFeature maps pos only when its virtual segment participates in feature.
    /// Diagnostics and edit write-back intentionally use VirtualToOriginalPosition instead.
    pub fn virtual_to_original_position_for_feature(
        &self,
        pos: TextPos,
        feature: Feature,
    ) -> (TextPos, Fidelity) {
        let (mapped, fidelity) = self.virtual_to_original_position(pos);
        let (index, inside) = self.segment_index_at(pos);
        if !inside || !supports_feature(&self.segments[expect_index(index)], feature) {
            return (mapped, Fidelity::None);
        }
        (mapped, fidelity)
    }

    /// AliasForVirtualSpan returns the alias segment exactly covering r. Partial overlap does not qualify:
    /// diagnostic text may be substituted only when the diagnostic identifies the complete virtual alias.
    ///
    /// PORT: Go returns (Segment, bool); the bool becomes the Option.
    pub fn alias_for_virtual_span(&self, r: TextRange) -> Option<Segment> {
        let (index, inside) = self.segment_index_at(r.pos());
        if !inside {
            return None;
        }
        let segment = self.segments[expect_index(index)];
        if segment.kind == Kind::ALIAS
            && r.pos() == segment.virtual_start
            && r.end() == segment.virtual_end
        {
            Some(segment)
        } else {
            None
        }
    }

    /// segmentIndexAt returns the index of the segment containing pos and true, or, when pos lies in a gap,
    /// the index of the segment immediately before pos (None if none) and false.
    ///
    /// PORT: Go returns (int, bool) with -1 for "no previous segment";
    /// Option<usize> carries the same information without the sentinel.
    fn segment_index_at(&self, pos: TextPos) -> (Option<usize>, bool) {
        let result = self.segments.binary_search_by(|s| s.virtual_start.cmp(&pos));
        let prev: isize = match result {
            Ok(idx) => return (Some(idx), true),
            Err(insertion) => insertion as isize - 1,
        };
        if prev >= 0
            && (pos < self.segments[prev as usize].virtual_end
                || (prev as usize == self.segments.len() - 1
                    && pos == self.segments[prev as usize].virtual_end))
        {
            return (Some(prev as usize), true);
        }
        ((prev >= 0).then_some(prev as usize), false)
    }

    /// insertionPoint returns the original offset where synthesized content following segment prev sits: the
    /// original end of that segment, or 0 before the first segment.
    fn insertion_point(&self, prev: Option<usize>) -> TextPos {
        match prev {
            None => 0,
            Some(prev) => self.segments[prev].original_end,
        }
    }

    /// mapLow maps a virtual lower range boundary to original coordinates. A boundary in a synthesized
    /// gap uses that gap's insertion point; an atom uses its original start.
    fn map_low(&self, pos: TextPos, idx: Option<usize>, inside: bool) -> TextPos {
        if !inside {
            return self.insertion_point(idx);
        }
        let seg = &self.segments[expect_index(idx)];
        if seg.kind == Kind::VERBATIM {
            return clamp(
                seg.original_start + (pos - seg.virtual_start),
                seg.original_start,
                seg.original_end,
            );
        }
        seg.original_start
    }

    /// mapHigh maps a virtual upper range boundary to original coordinates. A boundary in a synthesized
    /// gap uses that gap's insertion point; an atom uses its original end.
    fn map_high(&self, pos: TextPos, idx: Option<usize>, inside: bool) -> TextPos {
        if !inside {
            return self.insertion_point(idx);
        }
        let seg = &self.segments[expect_index(idx)];
        if seg.kind == Kind::VERBATIM {
            return clamp(
                seg.original_start + (pos - seg.virtual_start),
                seg.original_start,
                seg.original_end,
            );
        }
        seg.original_end
    }

    /// OriginalToVirtualPositions returns every virtual projection of an original position whose segment
    /// participates in feature. Segment ends are inclusive for point mapping, so a position shared by adjacent
    /// original spans returns projections from both sides. Results are ordered by virtual position. It returns
    /// no results for an uncovered position or when all touching segments reject feature.
    pub fn original_to_virtual_positions(
        &self,
        pos: TextPos,
        feature: Feature,
    ) -> Vec<MappedPosition> {
        let groups = self.orig_index().segment_groups_at_original_position(pos);
        if groups.is_empty() {
            return Vec::new();
        }
        let mut results: Vec<MappedPosition> = Vec::new();
        for group in &groups {
            for segment in &group.segments {
                if !supports_feature(segment, feature) {
                    continue;
                }
                let mut mapped = MappedPosition { position: 0, fidelity: Fidelity::Atom };
                if segment.kind == Kind::VERBATIM {
                    mapped.position = clamp(
                        segment.virtual_start + (pos - segment.original_start),
                        segment.virtual_start,
                        segment.virtual_end,
                    );
                    mapped.fidelity = Fidelity::Exact;
                } else if group.at_end {
                    mapped.position = segment.virtual_end;
                } else {
                    mapped.position = segment.virtual_start;
                }
                if !results.contains(&mapped) {
                    results.push(mapped);
                }
            }
        }
        results.sort_by_key(|m| m.position);
        results
    }

    /// OriginalToVirtualSpans returns every feature-compatible virtual projection of an original range.
    /// A range contained by one or more segments produces one exact or atom result per matching segment.
    ///
    /// A range that starts in one group and ends in another can have several possible virtual ranges. For
    /// example, suppose two original segments are each copied twice into the virtual text:
    ///
    /// ```text
    /// original:   [ A ][ B ]
    ///                [---)       range from inside A to inside B
    ///
    /// virtual:    [ A ][ B ]      [ A ][ B ]
    ///                ^   ^          ^   ^
    ///              start end      start end
    ///                1   3          11  13
    /// ```
    ///
    /// The map says that the range may start at 1 or 11 and end at 3 or 13, but it does not say which copy of A
    /// belongs with which copy of B. We choose the smallest range around each possible location, producing [1,3)
    /// and [11,13). We do not return [1,13), because it contains both smaller candidates and would include code
    /// that may be unrelated to the original range. These cross-group results have approximate fidelity.
    /// If either boundary is uncovered or disabled for feature, there are no results.
    pub fn original_to_virtual_spans(&self, r: TextRange, feature: Feature) -> Vec<MappedSpan> {
        let start = r.pos();
        let end = r.end().max(start);
        if start == end {
            return self
                .original_to_virtual_positions(start, feature)
                .into_iter()
                .map(|position| MappedSpan {
                    span: TextRange::new(position.position, position.position),
                    fidelity: position.fidelity,
                })
                .collect();
        }
        let last_character = end - 1;
        let index = self.orig_index();
        let (start_segments, start_inside) = index.segments_at_original_position(start);
        let (end_segments, end_inside) = index.segments_at_original_position(last_character);
        if !start_inside || !end_inside {
            return Vec::new();
        }
        let containing: Vec<Segment> = start_segments
            .iter()
            .filter(|segment| end <= segment.original_end)
            .copied()
            .collect();
        if !containing.is_empty() {
            let mut results = original_to_virtual_spans_in_segments(&containing, start, end, feature);
            if !results.is_empty() {
                results.sort_by_key(|m| m.span.pos());
                return results;
            }
        }
        let mut starts = original_start_projections(&start_segments, start, feature);
        let mut ends = original_end_projections(&end_segments, end, feature);
        if starts.is_empty() || ends.is_empty() {
            return Vec::new();
        }
        starts.sort_unstable();
        ends.sort_unstable();
        let mut results = Vec::with_capacity(starts.len().min(ends.len()));
        for (i, &virtual_start) in starts.iter().enumerate() {
            let end_index = ends.partition_point(|e| *e < virtual_start);
            if end_index == ends.len()
                || (i + 1 < starts.len() && starts[i + 1] <= ends[end_index])
            {
                continue;
            }
            results.push(MappedSpan {
                span: TextRange::new(virtual_start, ends[end_index]),
                fidelity: Fidelity::Approximate,
            });
        }
        results
    }

    /// OriginalToVirtualIntersectingSpans maps every feature-enabled segment intersection with r.
    /// Unlike OriginalToVirtualSpans, uncovered range endpoints do not suppress covered interior segments.
    pub fn original_to_virtual_intersecting_spans(
        &self,
        r: TextRange,
        feature: Feature,
    ) -> Vec<MappedSpan> {
        if r.pos() == r.end() {
            return self.original_to_virtual_spans(r, feature);
        }
        let mut results = Vec::new();
        for segment in &self.segments {
            if !supports_feature(segment, feature) {
                continue;
            }
            let start = r.pos().max(segment.original_start);
            let end = r.end().min(segment.original_end);
            if start >= end {
                continue;
            }
            if segment.kind == Kind::VERBATIM {
                results.push(MappedSpan {
                    span: TextRange::new(
                        segment.virtual_start + (start - segment.original_start),
                        segment.virtual_start + (end - segment.original_start),
                    ),
                    fidelity: Fidelity::Exact,
                });
            } else {
                results.push(MappedSpan {
                    span: TextRange::new(segment.virtual_start, segment.virtual_end),
                    fidelity: Fidelity::Atom,
                });
            }
        }
        results
    }

    /// origIndex builds the immutable original-text interval index on first use. Sorting dominates the O(n) tree
    /// construction, so the first lookup remains O(n log n); later point lookups visit only tree branches that can
    /// contain a match.
    fn orig_index(&self) -> &OriginalIndex {
        self.original_index.get_or_init(|| {
            let mut segments = self.segments.clone();
            segments.sort_by(|a, b| {
                a.original_start
                    .cmp(&b.original_start)
                    .then(a.original_end.cmp(&b.original_end))
                    .then(a.virtual_start.cmp(&b.virtual_start))
            });
            let mut leaf_count = 1usize;
            while leaf_count < segments.len() {
                leaf_count *= 2;
            }
            let mut max_ends = vec![0 as TextPos; 2 * leaf_count];
            for (i, segment) in segments.iter().enumerate() {
                max_ends[leaf_count + i] = segment.original_end;
            }
            for i in (1..leaf_count).rev() {
                max_ends[i] = max_ends[2 * i].max(max_ends[2 * i + 1]);
            }
            OriginalIndex { segments, leaf_count, max_ends }
        })
    }
}

/// originalStartProjections maps the inclusive start of an original range through every matching segment.
/// Verbatim segments preserve the offset within the segment; atoms map to their virtual start.
///
/// For duplicate verbatim segments, the start keeps the same relative offset in every copy:
///
/// ```text
/// original:       [---------)
///                    ^ start
///
/// virtual:    [---------)   [---------)
///                ^             ^
///              result        result
/// ```
fn original_start_projections(segments: &[Segment], start: TextPos, feature: Feature) -> Vec<TextPos> {
    let mut results = Vec::with_capacity(segments.len());
    for segment in segments {
        if !supports_feature(segment, feature) {
            continue;
        }
        if segment.kind == Kind::VERBATIM {
            results.push(clamp(
                segment.virtual_start + (start - segment.original_start),
                segment.virtual_start,
                segment.virtual_end,
            ));
        } else {
            results.push(segment.virtual_start);
        }
    }
    results
}

/// originalEndProjections maps the exclusive end of an original range through every matching segment.
/// The caller uses end-1 to find the segment containing the final character, while this helper maps the end
/// boundary itself. Verbatim segments preserve that boundary; atoms map to their virtual end.
///
/// The lookup uses end-1 so an end at a segment boundary selects the segment on its left, not the next one.
fn original_end_projections(segments: &[Segment], end: TextPos, feature: Feature) -> Vec<TextPos> {
    let mut results = Vec::with_capacity(segments.len());
    for segment in segments {
        if !supports_feature(segment, feature) {
            continue;
        }
        if segment.kind == Kind::VERBATIM {
            results.push(clamp(
                segment.virtual_start + (end - segment.original_start),
                segment.virtual_start,
                segment.virtual_end,
            ));
        } else {
            results.push(segment.virtual_end);
        }
    }
    results
}

/// originalToVirtualSpansInSegments maps a range fully contained by each segment.
fn original_to_virtual_spans_in_segments(
    segments: &[Segment],
    start: TextPos,
    end: TextPos,
    feature: Feature,
) -> Vec<MappedSpan> {
    let mut results = Vec::with_capacity(segments.len());
    for segment in segments {
        if !supports_feature(segment, feature) {
            continue;
        }
        if segment.kind == Kind::VERBATIM {
            let virtual_start = clamp(
                segment.virtual_start + (start - segment.original_start),
                segment.virtual_start,
                segment.virtual_end,
            );
            let virtual_end = clamp(
                segment.virtual_start + (end - segment.original_start),
                virtual_start,
                segment.virtual_end,
            );
            results.push(MappedSpan {
                span: TextRange::new(virtual_start, virtual_end),
                fidelity: Fidelity::Exact,
            });
        } else {
            results.push(MappedSpan {
                span: TextRange::new(segment.virtual_start, segment.virtual_end),
                fidelity: Fidelity::Atom,
            });
        }
    }
    results
}

/// sameOriginalRange reports whether two segments belong to the same duplicate group.
fn same_original_range(left: &Segment, right: &Segment) -> bool {
    left.original_start == right.original_start && left.original_end == right.original_end
}

/// originalIndex stores segments in original-text order and a complete binary tree whose leaves correspond
/// to those segments. Each internal node stores the maximum OriginalEnd below it, allowing point lookups to
/// discard a whole subtree when none of its segments can reach the queried position.
struct OriginalIndex {
    segments: Vec<Segment>,
    leaf_count: usize,
    max_ends: Vec<TextPos>,
}

/// segmentGroupAtOriginalPosition is one group of equal-range mapping segments
/// containing or touching an original-text position.
struct SegmentGroupAtOriginalPosition {
    segments: Vec<Segment>,
    at_end: bool,
}

impl OriginalIndex {
    /// segmentsAtOriginalPosition returns every mapping segment containing the original-text position pos.
    /// Segment ends are exclusive; a segment start, including a zero-length segment, is considered contained.
    fn segments_at_original_position(&self, pos: TextPos) -> (Vec<Segment>, bool) {
        // Query intervals that contain pos strictly before their exclusive end. Segments starting exactly at pos
        // are appended separately so zero-length segments are included without preventing maxEnd <= pos pruning.
        let start = self.segments.partition_point(|s| s.original_start < pos);
        let mut results = self.segments_ending_after_position(start, pos);
        let end = self.segments.partition_point(|s| s.original_start <= pos);
        results.extend_from_slice(&self.segments[start..end]);
        let found = !results.is_empty();
        (results, found)
    }

    /// segmentsEndingAfterPosition returns segments among [0, limit) whose OriginalEnd is greater than pos.
    fn segments_ending_after_position(&self, limit: usize, pos: TextPos) -> Vec<Segment> {
        let mut results = Vec::new();
        self.collect_segments_ending_at_or_after(1, 0, self.leaf_count, limit, pos, false, &mut results);
        results
    }

    /// collectSegmentsEndingAtOrAfter walks the flat max-end tree left-to-right, preserving original-text order.
    /// Nodes beyond limit or whose maximum end cannot reach pos are discarded without visiting their leaves.
    // PORT: the eight-parameter signature mirrors the Go method one-to-one.
    #[allow(clippy::too_many_arguments)]
    fn collect_segments_ending_at_or_after(
        &self,
        node: usize,
        start: usize,
        end: usize,
        limit: usize,
        pos: TextPos,
        include_end: bool,
        results: &mut Vec<Segment>,
    ) {
        if start >= limit
            || self.max_ends[node] < pos
            || (!include_end && self.max_ends[node] == pos)
        {
            return;
        }
        if end - start == 1 {
            results.push(self.segments[start]);
            return;
        }
        let middle = start + (end - start) / 2;
        self.collect_segments_ending_at_or_after(2 * node, start, middle, limit, pos, include_end, results);
        self.collect_segments_ending_at_or_after(2 * node + 1, middle, end, limit, pos, include_end, results);
    }

    /// segmentGroupsAtOriginalPosition returns every group of equal-range mapping segments containing or touching
    /// the original-text position pos. Segment ends are included for point mapping.
    ///
    /// At a shared boundary, segments ending at pos and segments starting there form separate groups:
    ///
    /// ```text
    /// original:  [--- A ---)[--- B ---]
    ///                        ^ pos
    ///
    /// virtual:   [ A1 ) [ A2 )    [ B1 ) [ B2 )
    ///              left group       right group
    ///              atEnd: true      atEnd: false
    /// ```
    fn segment_groups_at_original_position(&self, pos: TextPos) -> Vec<SegmentGroupAtOriginalPosition> {
        let limit = self.segments.partition_point(|s| s.original_start <= pos);
        let mut segments = Vec::new();
        self.collect_segments_ending_at_or_after(1, 0, self.leaf_count, limit, pos, true, &mut segments);
        let mut groups = Vec::new();
        let mut start = 0;
        while start < segments.len() {
            let mut end = start + 1;
            while end < segments.len() && same_original_range(&segments[start], &segments[end]) {
                end += 1;
            }
            let segment = segments[start];
            if pos <= segment.original_end {
                groups.push(SegmentGroupAtOriginalPosition {
                    segments: segments[start..end].to_vec(),
                    at_end: pos == segment.original_end && pos != segment.original_start,
                });
            }
            start = end;
        }
        groups
    }
}

/// supportsFeature reports whether segment participates in feature.
fn supports_feature(segment: &Segment, feature: Feature) -> bool {
    segment.features.intersects(feature)
}

/// clamp confines v to the inclusive interval [lo, hi].
fn clamp(v: TextPos, lo: TextPos, hi: TextPos) -> TextPos {
    lo.max(v.min(hi))
}

/// Unmarshal decodes a SpanMap from the JSON tuple form produced by an out-of-process content mapper.
/// Five-element tuples omit features and are normalized to FeatureAll; six-element tuples preserve the
/// explicit feature mask, including FeatureNone.
pub fn unmarshal(data: &[u8]) -> Result<SpanMap, UnmarshalError> {
    let tuples: Vec<Vec<i32>> = json::unmarshal(data, &[]).map_err(UnmarshalError::Json)?;
    let mut segments = Vec::with_capacity(tuples.len());
    for (i, t) in tuples.iter().enumerate() {
        if t.len() != 5 && t.len() != 6 {
            return Err(UnmarshalError::SegmentTupleLength { index: i, got: t.len() });
        }
        segments.push(Segment {
            virtual_start: t[0],
            virtual_end: t[0] + t[1],
            original_start: t[2],
            original_end: t[2] + t[3],
            kind: Kind::from_i32(t[4]),
            features: Feature::ALL,
        });
        if t.len() == 6 {
            segments[i].features = Feature::from_i32(t[5]);
        }
    }
    Ok(SpanMap::new(segments))
}

impl SpanMap {
    /// Marshal encodes a SpanMap into the JSON tuple form. FeatureAll uses the backward-compatible five-element
    /// tuple; every other feature mask is emitted as a sixth element.
    pub fn marshal(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut tuples: Vec<Vec<i32>> = Vec::with_capacity(self.segments.len());
        for s in &self.segments {
            let mut tuple = vec![
                s.virtual_start,
                s.virtual_end - s.virtual_start,
                s.original_start,
                s.original_end - s.original_start,
                s.kind.as_i32(),
            ];
            if s.features != Feature::ALL {
                tuple.push(s.features.as_i32());
            }
            tuples.push(tuple);
        }
        json::marshal(&tuples, &[])
    }
}
