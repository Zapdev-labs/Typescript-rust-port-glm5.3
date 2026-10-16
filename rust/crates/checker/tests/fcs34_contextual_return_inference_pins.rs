//! FCS34 pins (full-corpus check sweep, tranche 34): the marker-probe
//! variance-measurement residual queue's top family — the fcs33 ND pair
//! genericContextualTypes2/3 (rust-EXTRA TS2322
//! `AssignAction<unknown> -> AssignAction<{ count: number; }>`) — plus the
//! collateral it unmasked. ("FCS tranche 34" PORT notes in-tree.)
//!
//! ROOT 1 — the contextual-return inference prologue (Go checker.go:
//! 9627-9686, `inferTypeArguments`): "If a contextual type is available,
//! infer from that type to the return type of the call expression." Was
//! the dormant M6 S9 TODO in resolvecall.rs. Without it, `assign({...})`
//! at `entry?: AssignAction<TContext>` (genericContextualTypes2/3) never
//! received the RETURN_TYPE-priority candidate `TContext :=
//! { count: number }` from the entry property's contextual type, typed
//! the call `AssignAction<unknown>`, and the final reference-arm variance
//! check (AssignAction measures [in out] INVARIANT — both marker probes
//! false, `VarianceFlagsInvariant == 0` in BOTH drivers; the probes and
//! the relateVariances retry ladder were already Go-exact) fell to the
//! structural compare, which genuinely fails through the contravariant
//! `Meta<TContext>` alias — a rust-EXTRA 2322. Pre-tranche-33 the vacuous
//! alias-variance probe masked it.
//!
//! ROOT 2 — `inferredTypeParameters` (Go checker.go:9289/9313,
//! `chooseOverload`): the port passed `get_inferred_type` results as
//! getSignatureInstantiation's `inferredTypeParameters` (checker.go:
//! 19719 clones the return signature and installs the list as its
//! typeParameters) — non-parameter types there render as "(Missing type
//! parameter)". Go reads the context FIELD (populated only by
//! instantiateTypeWithSingleGenericCallSignature, checker.go:7809 — an
//! S9-family seam still unported, so the field is empty in the port).
//! Root-caused residual: genericContextualTypes1 f40/f13 (flip(zip)/
//! compose) needs that unported seam (GO keeps the contextual signature's
//! type parameters as candidates — `args=[A, B, [A, B]]` — where the port
//! erases them to `{}`); queued, evidence in ~/.fcs34-continuation.md.
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; shapes mirror
//! the conformance records genericContextualTypes2.ts and
//! genericContextualTypes1.ts (dump-diffed through both drivers).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23+ idiom) ──

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

fn check_with(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        strict: tsc_core::tristate::Tristate::TSTrue,
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
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// corpus seam reads `semantic_diagnostics()`).
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    let _ = file;
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn codes(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, args)| args.clone())
        .collect()
}

// ── ROOT 1 pins: the contextual-return inference prologue ──

/// Pin 1 (genericContextualTypes2.ts, verbatim shape): the whole unit
/// checks clean of TS2322 — `assign({...})` at
/// `entry?: AssignAction<TContext>` infers `TContext` from the contextual
/// return type (the RETURN_TYPE-priority candidate), so the call types as
/// `AssignAction<{ count: number; }>` and the entry check is identical.
/// Before the prologue the call typed as `AssignAction<unknown>` and the
/// final reference-arm variance check reported a rust-EXTRA 2322 (the
/// fcs33 newly-divergent pair).
#[test]
fn fcs34_generic_contextual_types2_clean_pin() {
    let src = "\
type LowInfer<T> = T & {};

type PartialAssigner<TContext, TKey extends keyof TContext> = (
  context: TContext
) => TContext[TKey];

type PropertyAssigner<TContext> = {
  [K in keyof TContext]?: PartialAssigner<TContext, K> | TContext[K];
};

type Meta<TContext> = {
  action: (ctx: TContext) => void
}

interface AssignAction<TContext> {
  type: \"xstate.assign\";
  exec: (arg: TContext, meta: Meta<TContext>) => void;
}

declare function assign<TContext>(
  assignment: PropertyAssigner<LowInfer<TContext>>
): AssignAction<TContext>;

type Config<TContext> = {
  context: TContext;
  entry?: AssignAction<TContext>;
};

declare function createMachine<TContext>(config: Config<TContext>): void;

createMachine<{ count: number }>({
  context: {
    count: 0,
  },
  entry: assign({
    count: (ctx: { count: number }) => ++ctx.count,
  }),
});
";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "expected no 2322 (contextual-return inference recovers the unit); got {ds:?}"
    );
}

/// Pin 2: the induced-error probe — the `assign(...)` call's return type
/// prints `AssignAction<{ count: number; }>` (TContext inferred from the
/// contextual return), NOT `AssignAction<unknown>`.
#[test]
fn fcs34_assign_call_prints_inferred_context_pin() {
    let src = "\
type LowInfer<T> = T & {};

type PartialAssigner<TContext, TKey extends keyof TContext> = (
  context: TContext
) => TContext[TKey];

type PropertyAssigner<TContext> = {
  [K in keyof TContext]?: PartialAssigner<TContext, K> | TContext[K];
};

interface AssignAction<TContext> {
  type: \"xstate.assign\";
  exec: (arg: TContext, meta: { action: (ctx: TContext) => void }) => void;
}

declare function assign<TContext>(
  assignment: PropertyAssigner<LowInfer<TContext>>
): AssignAction<TContext>;

type Config<TContext> = {
  context: TContext;
  entry?: AssignAction<TContext>;
};

declare function createMachine<TContext>(config: Config<TContext>): void;

const n: number = createMachine<{ count: number }>({
  context: {
    count: 0,
  },
  entry: assign({
    count: (ctx: { count: number }) => ++ctx.count,
  }),
});
";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "void", "createMachine returns void");
    assert_eq!(d[0][1], "number");
    // The assign() call itself must not surface a 2322 anywhere; the
    // prologue's candidate made the entry check identical.
    assert!(
        !d.iter()
            .any(|a| a.iter().any(|x| x.contains("AssignAction"))),
        "no AssignAction 2322 may remain: {d:?}"
    );
}

/// Pin 3 (genericContextualTypes1.ts f02 — the contextualSignature arm):
/// `wrap(list)` with the GENERIC contextual type `<A>(x: A) => A[]` runs
/// the prologue's generic-contextual-signature arm
/// (getSignatureInstantiationWithoutFillingInTypeArguments) and checks
/// clean — no 2322. Before the tranche the dormant prologue plus the
/// bogus inferredTypeParameters made this shape report
/// `<(Missing type parameter) ...>` extras.
#[test]
fn fcs34_generic_contextual_signature_arm_clean_pin() {
    let src = "\
type Box<T> = { value: T };

declare function wrap<A, B>(f: (a: A) => B): (a: A) => B;

declare function list<T>(a: T): T[];

const f02: <A>(x: A) => A[] = wrap(list);
";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "wrap(list) must check clean (f02); got {ds:?}"
    );
}

// ── ROOT 2 pins: the inferredTypeParameters field ──

/// Pin 4: a call whose return type is a function type instantiates to a
/// NON-generic return — the 2322 source prints `(y: number) => void`
/// with no "(Missing type parameter)" (the getSignatureInstantiation
/// inferredTypeParameters branch no longer receives inferred TYPES).
#[test]
fn fcs34_call_type_renders_without_missing_type_parameters_pin() {
    let src = "\
declare function make<T>(x: T): (y: T) => void;

const m: number = make(1);
";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "(y: number) => void");
    assert_eq!(d[0][1], "number");
    assert!(
        !d[0][0].contains("(Missing type parameter)"),
        "the call type must not render bogus type parameters: {d:?}"
    );
}
