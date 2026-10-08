// Module rust-bench is the Go-side benchmark driver (`go-bench`) for the
// performance program (rust/SPEC.md §15, Phase A). It lives at
// rust/bench/go-driver but its module path must sit under the
// `github.com/microsoft/TypeScript/tsc/` prefix: Go's internal-package rule
// only allows importing `tsc/internal/...` from import paths rooted at
// `github.com/microsoft/TypeScript/tsc/`, and this driver imports the
// reference Go port's parser/ast/core/tspath packages. The root go.work
// `use` block maps this module path to this directory; no tsc/ sources are
// involved.

module github.com/microsoft/TypeScript/tsc/rust-bench

go 1.27
