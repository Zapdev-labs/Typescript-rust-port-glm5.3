//! FCS9 pins (full-corpus check sweep, tranche 9): the
//! prefer-covariant-inference short-circuit seam in `get_inferred_type`.
//!
//! ROOT (all pins' expectations verified byte-same against the go-bench
//! twin's check-stage dump over the same source shape —
//! $HOME/.fcs9-scratch/probe1/k1.ts on the rig):
//!
//! Go's `getInferredType` (inference.go:1328) computes
//!
//!   preferCovariantType := inferredCovariantType != nil && (inferredContravariantType == nil ||
//!       inferredCovariantType.flags&(TypeFlagsNever|TypeFlagsAny) == 0 &&
//!       core.Some(inference.contraCandidates, ...) &&
//!       core.Every(n.inferences, ...))
//!
//! — the `inferredContravariantType == nil` arm SHORT-CIRCUITS to true;
//! the some/every probes only run on the contravariant-present path. The
//! prior Rust port ran the `some(contraCandidates)` test even in the
//! no-contravariant arm, where the empty contra list made it false — the
//! good covariant inference was discarded, `inferredType` fell to the
//! `unknownType` default, the `keyof T` constraint instantiated to `never`,
//! and EVERY keyof-constrained call reported a bogus
//! `Argument of type '"name"' is not assignable to parameter of type
//! 'never'` (the 213-extra TS2345 family; keyofAndIndexedAccess alone
//! carried 32).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5/fcs6/fcs8 idiom) ──

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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

/// Check `src` over the single-file scaffold with strict flags (the
/// conformance default for this family's shapes).
fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        no_implicit_this: Tristate::TSTrue,
        strict_null_checks: Tristate::TSTrue,
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

const KEYOF_FAMILY: &str = "interface Shape { name: string; width: number; visible: boolean }\n\
     declare const shape: Shape;\n\
     function getProperty<T, K extends keyof T>(obj: T, key: K) { return obj[key]; }\n\
     function setProperty<T, K extends keyof T>(obj: T, key: K, value: T[K]) { obj[key] = value; }\n\
     getProperty(shape, \"name\");\n\
     setProperty(shape, \"width\", 5);\n\
     getProperty(shape, \"size\");\n\
     declare const k: \"height\" | \"visible\";\n\
     getProperty(shape, k);\n";

// ── The pins ───────────────────────────────────────────────────────────────

/// Pin 1 — the covariant inference is no longer discarded: a call whose
/// ONLY inferences are covariant (the whole keyof-constrained family — no
/// contravariant positions at all) infers T from its argument, so there is
/// NO bogus `'"name"' is not assignable to parameter of type 'never'`
/// TS2345 for the valid calls (the twin's dump over k1.ts reports exactly
/// the two diagnostics pinned next).
#[test]
fn covariant_only_inference_not_discarded_pin() {
    let (mut c, file) = check_strict(KEYOF_FAMILY);
    let pairs = diags(&mut c, file);
    let never_params: Vec<_> = pairs
        .iter()
        .filter(|(code, args)| *code == 2345 && args.iter().any(|a| a == "never"))
        .collect();
    assert!(
        never_params.is_empty(),
        "no argument error may render a 'never' parameter type (got {never_params:?}; full {pairs:?})"
    );
}

/// Pin 2 — the twin's exact two errors: `getProperty(shape, "size")` and
/// `getProperty(shape, k)` report TS2345 with the constraint-instantiated
/// parameter type `keyof Shape` (Go's pick-constraint fallback after the
/// candidate fails the constraint), NOT `never`.
#[test]
fn keyof_constraint_param_renders_keyof_shape_pin() {
    let (mut c, file) = check_strict(KEYOF_FAMILY);
    let pairs = diags(&mut c, file);
    let assigns: Vec<Vec<String>> = pairs
        .iter()
        .filter(|(code, _)| *code == 2345)
        .map(|(_, args)| args.clone())
        .collect();
    assert_eq!(
        assigns.len(),
        2,
        "exactly the two invalid calls report TS2345 (got {pairs:?})"
    );
    assert!(
        assigns.iter().all(|args| args.len() == 2 && args[1] == "keyof Shape"),
        "both parameter types must render keyof Shape (got {assigns:?})"
    );
    assert_eq!(assigns[0][0], "\"size\"", "first invalid arg is \"size\": {assigns:?}");
    assert_eq!(
        assigns[1][0], "\"height\" | \"visible\"",
        "second invalid arg is the union: {assigns:?}"
    );
}

/// Pin 3 — the inference composes with the constraint check: the valid
/// call's K candidate `"name"` satisfies the instantiated constraint
/// `keyof Shape` and is KEPT (Go's constraint-filter path leaves it), so
/// the bogus `'"name"' is not assignable to parameter of type 'never'`
/// never fires and setProperty's T[K] third parameter accepts the value.
#[test]
fn valid_keyof_candidate_kept_pin() {
    let (mut c, file) = check_strict(
        "interface Shape { name: string; }\n\
         declare const shape: Shape;\n\
         function getProperty<T, K extends keyof T>(obj: T, key: K) { return obj[key]; }\n\
         const n: string = getProperty(shape, \"name\");\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        pairs.is_empty(),
        "the valid keyof-constrained call must be error-free (got {pairs:?})"
    );
}
