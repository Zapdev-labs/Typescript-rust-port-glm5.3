// Benchmark driver for the Rust port ("tsrs") — SPEC.md §15.
//
// Subcommands:
//   micro --op <name> [--iters N] [--samples N]      Phase 0 micro benches
//   parse --corpus <dir> [--threads N] [--iters K]   Phase A corpus parse
//     [--iterations K] [--json]
//
// Timing protocol mirrors the Go side (`go test -bench -count=N`):
// 2 warmup invocations, then N samples of fixed-iteration loops; report the
// median ns/op. Every op closure returns a checksum that is folded into an
// accumulator consumed by std::hint::black_box so the optimizer cannot elide
// the measured work.
//
// Output (stdout, TSV): `median_ns_per_op<TAB>min<TAB>max<TAB>op<TAB>samples`

use std::hint::black_box;
use std::time::Instant;

mod conformance_seam;
mod micro;
mod parse;
mod parser_seam;
mod program_seam;
mod showconfig_seam;
mod symbols_seam;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        usage();
    };
    match cmd.as_str() {
        "micro" => micro::run(&args[1..]),
        "parse" => parse::run(&args[1..]),
        "dump" => parse::run_dump(&args[1..]),
        "showconfig" => showconfig_seam::run_showconfig(&args[1..]),
        "resolvereact" => showconfig_seam::run_resolvereact(&args[1..]),
        "symbols" => symbols_seam::run_symbols(&args[1..]),
        "program" => {
            program_seam::run_program(&args[1..]);
            program_seam::print_program_profile();
        }
        "conformance" => conformance_seam::run_conformance(&args[1..]),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: tsc-bench micro --op <name> [--iters N] [--samples N]");
    eprintln!("       tsc-bench parse --corpus <dir> [--threads N] [--iterations K] [--iters K] [--json]");
    eprintln!("       tsc-bench dump --corpus <dir>   (per-file NDJSON: nodes, diagnostics, histogram)");
    eprintln!("       tsc-bench showconfig --corpus <dir> | --tsconfig <path>   (per-config NDJSON: marshaled TSConfig)");
    eprintln!("       tsc-bench resolvereact --corpus <dir>   (per-specifier NDJSON + summary)");
    eprintln!("       tsc-bench symbols --corpus <dir> [--fuzz <dir>]   (per-file NDJSON: binder symbol tables)");
    eprintln!("       tsc-bench program --corpus <dir> | --file <path>   (per-root NDJSON: Program load/bind/parse-diag counts)");
    eprintln!("       tsc-bench conformance --corpus <dir> --baselines <dir> [--suite S] [--limit N] [--filter P] [--stage parse|bind] [--local-root <dir>]");
    std::process::exit(2);
}

/// Bench framework: median-of-samples over fixed-iteration loops.
pub struct Bench {
    pub op: String,
    pub iters: u64,
    pub samples: u32,
}

impl Bench {
    /// Runs `f` and returns the median ns/op.
    pub fn run(&self, mut f: impl FnMut() -> u64) -> f64 {
        for _ in 0..2 {
            black_box(f());
        }
        let mut times: Vec<f64> = Vec::with_capacity(self.samples as usize);
        for _ in 0..self.samples {
            let start = Instant::now();
            let mut acc = 0u64;
            for _ in 0..self.iters {
                acc = acc.wrapping_add(f());
            }
            let elapsed = start.elapsed().as_nanos() as f64;
            black_box(acc);
            times.push(elapsed / self.iters as f64);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        let median = times[times.len() / 2];
        let min = times[0];
        let max = times[times.len() - 1];
        println!("{median:.1}\t{min:.1}\t{max:.1}\t{}\t{}", self.op, self.samples);
        median
    }
}
