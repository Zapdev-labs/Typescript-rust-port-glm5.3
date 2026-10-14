//! FCS14 pins (full-corpus check sweep, tranche 14): the `typeof globalThis`
//! expando family (checker.go:974's `globalThisSymbol.Exports = c.globals`
//! aliasing, checker.go:11573's property-access arm, checker.go:27679's
//! element-access conjunction).
//!
//! ROOTS (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the ~/s4-rig-scratch/fcs14-probe battery on the rig,
//! PROBES-IDENTICAL on the four globalThis cases):
//!
//! 1. Go aliases ONE map between `c.globals` and `globalThisSymbol.Exports`
//!    (checker.go:974); the port's SymbolTable is a value type, so the
//!    file-merged globals never reached the symbol's exports and EVERY
//!    `typeof globalThis` member lookup failed (spurious TS2339 on
//!    `globalThis.x` for same-file `var x`). spine.rs `globals_set` now
//!    mirrors each post-init globals write into the exports.
//!
//! 2. checker.go:11573: an unknown property on `typeof globalThis` with a
//!    BLOCK-SCOPED export reports TS2339; otherwise noImplicitAny reports
//!    the no-index-signature TS7017 (and silent when noImplicitAny is off).
//!
//! 3. checker.go:27679: the element-access 2339 needs the full conjunction
//!    (globalThis receiver && hasPropName && block-scoped export); a
//!    var-declared name resolves through the members, so `globalThis['x']`
//!    is silent.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs13 idiom) ──

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
}

impl Program for TestProgram {
    fn options(&self) -> &CompilerOptions {
        &self.options
    }
    fn source_files(&self) -> &[SourceFile] {
        std::slice::from_ref(&self.source_file)
    }
    fn bind_source_files(&mut self) {}
    fn node_store(&self) -> &dyn NodeStore {
        &self.source_file
    }
    fn node_store_mut(&mut self) -> &mut dyn NodeStore {
        &mut self.source_file
    }
    fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
        (&self.bind.flow_nodes, &self.bind.flow_lists)
    }
    fn symbol(&self, id: SymbolId) -> &Symbol {
        if id.0 < self.binder_symbol_count {
            &self.bind.symbols[id.0 as usize]
        } else {
            &self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        if id.0 < self.binder_symbol_count {
            &mut self.bind.symbols[id.0 as usize]
        } else {
            &mut self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.checker_symbols.push(symbol);
        SymbolId(self.binder_symbol_count + self.checker_symbols.len() as u32 - 1)
    }
    fn source_file_name(&self, _file: tsc_ast::NodeId) -> String {
        self.name.as_string().to_string()
    }
    fn source_file_meta_data(&self, _file: tsc_ast::NodeId) -> tsc_ast::SourceFileMetaData {
        tsc_ast::SourceFileMetaData {
            package_json_type: String::new(),
            package_json_directory: Default::default(),
            implied_node_format: ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

/// Check `src` over the single-file scaffold (no libs — the corpus seam),
/// with noImplicitAny pinned by the caller.
fn check_opts(src: &str, no_implicit_any: Tristate) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any,
        ..CompilerOptions::default()
    };
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the globals↔exports aliasing + the 11573 property-access arm ──

const GLOBALTHIS_MIXED_PROPERTIES: &str = "var x = 1;\n\
const y = 2;\n\
globalThis.x;\n\
globalThis.y;\n\
globalThis['x'];\n\
globalThis['y'];\n\
globalThis.zzz;\n";

#[test]
fn globalthis_var_visible_block_scoped_2339_unknown_7017_pin() {
    // checker.go:974 + 11573 + 27679 (probe battery: globalThisBlockscoped
    // Properties / globalThisVarDeclaration shapes, PROBES-IDENTICAL):
    // `var x` is a non-block-scoped global — visible as a property of
    // `typeof globalThis` through the aliased exports (varsOnly members,
    // checker.go:21075), so `globalThis.x` and `globalThis['x']` are
    // SILENT; `const y` is block-scoped, so the property arm (11573) and
    // the element arm (27679) both report TS2339 with the property name and
    // "typeof globalThis"; the absent `zzz` falls to the noImplicitAny
    // TS7017 no-index-signature leg of 11573.
    let (mut c, file) = check_opts(GLOBALTHIS_MIXED_PROPERTIES, Tristate::TSTrue);
    let ds = diags(&mut c, file);
    let y2339 = |ds: &Vec<(i32, Vec<String>)>| {
        ds.iter()
            .filter(|(code, args)| {
                *code == 2339
                    && args.first().is_some_and(|a| a == "y")
                    && args.get(1).is_some_and(|a| a == "typeof globalThis")
            })
            .count()
    };
    assert_eq!(y2339(&ds), 2, "property + element access on 'y' report 2339: {ds:?}");
    assert!(
        ds.iter().any(|(code, args)| *code == 7017
            && args.first().is_some_and(|a| a == "typeof globalThis")),
        "the unknown property reports TS7017 under noImplicitAny: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, args)| *code == 2339
            && args.first().is_some_and(|a| a == "x")),
        "var-declared globals are properties of typeof globalThis: {ds:?}"
    );
}

// ── Pin 2: the noImplicitAny gate on the 11573 arm ──

const GLOBALTHIS_UNKNOWN_SLOPPY: &str = "globalThis.hi;\n";

#[test]
fn globalthis_unknown_silent_without_noimplicitany_pin() {
    // globalThisUnknown (probe battery, PROBES-IDENTICAL): without
    // noImplicitAny, the 11573 arm's else-leg is gated off, so an unknown
    // property on `typeof globalThis` is SILENT with type any — the port
    // previously reported TS2339 unconditionally.
    let (mut c, file) = check_opts(GLOBALTHIS_UNKNOWN_SLOPPY, Tristate::TSFalse);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2339 || *code == 7017),
        "sloppy-mode unknown globalThis access is silent: {ds:?}"
    );
}
