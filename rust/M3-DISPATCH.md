# M3 Dispatch Map — scanner / factory / parser waves

Operational planning notes for the M3 porting waves (scanner → factory →
parser → Phase A race). Sources: read-only anatomy surveys of
`tsc/internal/{scanner,parser}` @ ec47d33c23e464a17cdf2475632cba629bee8763.
This file is a handoff aid, not a spec; SPEC.md §15 remains authoritative.

## Wave 1 — tsc-scanner

Scope (5 files, 4,337 LOC, all hand-written Go):
| file | LOC | role |
|---|---|---|
| scanner/scanner.go | 2918 | Scan() hot loop, ReScan* family, keyword maps, line/pos math |
| scanner/regexp.go | 1105 | RegExp literal re-scan (flags, Annex B, classes, unicode props) |
| scanner/unicodeproperties.go | 162 | ECMA-262 tables 66/67 for /u validation (uses collections.Set) |
| scanner/utilities.go | 146 | GetTextOfNodeFromSourceText, DeclarationNameToString, IsIdentifierText |
| scanner/scanner_test.go | 105 | 5 unit tests; NO testdata, NO baselines |

EXCLUDE from wave 1 (parser wave): `GetLeading/TrailingCommentRanges` (need
`ast.NodeFactory`). Leave PORT-TODO markers at the call sites.

Deps verified available in Rust already: stringutil unicode tables
(identifier_parts_generated.rs 1,394 LOC; js_case_generated.rs 3,508 LOC),
is_line_break/is_white_space_single_line/like, is_digit, is_ascii_letter,
encode_js_string_rune, is_unicode_identifier_start/part. `IsIdentifierStart/Part`
free fns are scanner-owned (port with the package).
Hard prerequisites from other crates: `ast.TokenFlags` (bit 0–18 + masks),
`ast.NodeFlags{JSDoc,Reparsed,ReparserTransformedLiteral}` bits, `ast.Kind`,
`core.ScriptTarget`, `core.LanguageVariant`, `core.TextRange`, ~30 diagnostics
keys (Asterisk_Slash_expected, Hexadecimal_digit_expected, Unterminated_*,
Unknown/Duplicate_regular_expression_flag, Invalid_character,
Unexpected_end_of_text, Unexpected_token_Did_you_mean_*).
NO ModifierFlags usage in the scanner package.

API surface for the parser wave (signatures to match Go):
NewScanner + SetText/SetOnError/SetLanguageVariant/SetScriptTarget/Reset/
ResetPos/ResetTokenState; Scan() + ReScan{LessThanToken,GreaterThanToken,
TemplateToken,AsteriskEqualsToken,SlashToken,JsxToken,HashToken,QuestionToken,
JsxAttributeValue}; ScanJsxToken(Ex)/ScanJsxIdentifier/ScanJsxAttributeValue/
ScanJSDocCommentTextToken/ScanJSDocToken/CanFollowJSDocAt;
Mark/Rewind(ScannerState); accessors (Token, TokenFlags, TokenFullStart/
Start/End, TokenText, TokenValue, TokenRange, CommentDirectives, Text,
Has* predicates incl. the JSDoc-deprecated/see/link flag setters).

Perf notes: byte fast path is `scanASCIIWhile(pred func(byte) bool)` before
utf8 rune decoding (char/charAndSize); numberCache/hexNumberCache/hexDigitCache
memo maps (allocation-heavy in Go — Rust may prefer the direct path, PORT-note
the divergence if dropped); strings.Builder only when escapes force
materialization (fast path = &str slices of source); defaultScanner()
function-constructed struct to dodge Go write barriers (Rust: plain Default).

## Wave 2 — NodeFactory (generator extension in gen-ast)

The parser calls 191 distinct `factory.New*` constructors across
parser.go/jsdoc.go/reparser.go (NewBinaryExpression, NewCallExpression,
NewSourceFile, NewToken, NewModifier, ...). Rust surface must cover ALL of
them + per-kind arenas + Update* methods where the parser uses them.
Also required by parser/Phase A: `ast.NewDiagnostic`, `ast.NodeIsPresent/
Missing`, `ast.ModifierToFlag(s)`, `ast.SetExternalModuleIndicator`,
`ast.SetParseJSDocForNode` (SourceFile mutation), `ast.TagNamesAreEquivalent`,
`ast.GetAssignmentDeclarationKind` (lives in ast/utilities.go — port the
parser-called subset of utilities.go in the parser wave if it does not land
with ast).

## Wave 3 — tsc-parser

Files: parser.go 6,887 + jsdoc.go 1,355 + utilities.go 56 + references.go 71 +
types.go 14 (ParseFlags bitmask: Yield/Await/Type/IgnoreMissingOpenBrace/
JSDoc). **reparser.go (752) is OUT of the race path** — incremental reparse
driven only from JSDoc finish paths; port it AFTER Phase A is green.

Phase A driver contract (from rust/bench/go-driver/main.go:337): the only
parser entry point is
`ParseSourceFile(opts ast.SourceFileParseOptions, sourceText string, scriptKind core.ScriptKind) -> *ast.SourceFile`
(opts: FileName tspath.RootedFilePath, PathKey tspath.PathKey,
ExternalModuleIndicatorOptions{JSX,Force}; driver fills FileName+PathKey only).
Node count = SourceFile root + ForEachChild-reachable nodes incl
EndOfFileToken, excl NodeList wrappers; errors = len(sf.Diagnostics()).

Perf notes: parserPool sync.Pool + initializeState reuse (Rust: reuse or
plain construct, PORT-note); mark/rewind/lookAhead in-place state machine
(parser.go:351-382) mutates parser state — keep the same shape; allocation
flows through NodeFactory, no strings.Builder in the parser proper.

Testdata no-panic gate (M3 exit): iterate (a) all `tsc/testdata/fixtures/**`
files with a known TS extension, (b) the 10 `tsc/internal/parser/testdata/
fuzz/FuzzParser/` seeds (Go fuzz corpus format: `go test fuzz v1` header,
`string(ext)`, `string(text)`, two bools — extension set =
AllSupportedExtensionsWithJson), optionally (c) compiler/conformance cases
with unit splitting. Go's BenchmarkParse uses 5 filefixture entries
(checker.ts, dom.generated.d.ts, Herebyfile.mjs,
jsxComplexSignatureHasApplicabilityError.tsx, +1) — port as a smoke bench.

## Phase A reminder (locked baseline, BENCHMARKS.md)

go-bench @ threads=1/K=1: 254.5 ms, 31.1 MB/s, 881,067 nodes, 0 errors,
shape_hash 487377d6e977e9ef. Gate: Rust ≤330.9 ms (1.30×) with byte-identical
nodes/errors/shape_hash; hardening ≤254.5 ms; stretch ≤203.6 ms. Race harness:
`rust/scripts/bench-parse.sh` (contract-equality gated, verdicts built in).
