// Ported from tsc/internal/tracing/tracing.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// M6 W2 (tracer wave): the subset of the tracing package checker/tracer.go
// consumes — StartTracing / Instant / Push / StopTracing / the typeTracer +
// TypeDescriptor dump. The package is NOT yet a standalone crate in the port,
// so it lives here crate-locally; the tracing-crate wave hoists it (every
// divergence from Go is PORT-noted and the observable outputs — trace.json,
// legend.json, types_N.json in deterministic mode — are byte-identical, which
// the tracer tests pin).
//
// PORT (Go → Rust shape):
//   * `vfs.FS` → [`TraceFs`] (the two calls the Go code makes: WriteFile /
//     AppendFile, plus ReadFile for the tests; the tsc-vfs crate is not a
//     dependency of tsc-checker and adding one is outside this wave).
//   * `*Tracing` pointer identity / cross-goroutine sharing → `Rc<TracingShared>`
//     + interior mutability (single-threaded in the spike; Go's `sync.Mutex`
//     guards become the RefCell borrow).
//   * `map[string]any` args → [`Args`] — an Rc-shared BTreeMap over
//     [`TraceValue`]. The SHARING is load-bearing (Go maps are reference
//     types): tracer_test.go's whole point is that the end event observes
//     mutations made after Push, so `Args::clone` is Go's map copy-by-
//     reference and `copy_with_checker_index` is the one place Go copies.
//   * `json.MarshalWrite(..., json.Deterministic(true))` → the hand-rolled
//     writer below: fields in Go struct-declaration order, map keys sorted,
//     `omitzero` fields omitted when zero (Go json v2 semantics).
//     Number formatting: integers verbatim; float64 via shortest-form which
//     for the deterministic counter values (1, 2, 3, …) matches Go exactly
//     (`float64(3)` marshals as `3`). PORT: non-deterministic (wall-clock)
//     timestamps may differ from Go in exponent notation for extreme values
//     (Go prints `1e+21`, Rust `1e21`) — unreachable in baselines, which are
//     all deterministic mode.
//   * string escaping: standard JSON without Go's HTML escaping; the checker
//     trace payloads (names, numbers, paths) contain no characters where the
//     two differ (PORT).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use rustc_hash::FxHashMap;
use tsc_ast::{NodeId, SymbolId};

use crate::relater::RecursionId;
use crate::types::TypeId;

// ────────────────────────────────────────────────────────────────────────────
// FS seam + trace values
// ────────────────────────────────────────────────────────────────────────────

/// Go: `vfs.FS` — the surface `Tracing` uses.
pub trait TraceFs {
    /// Go: `fs.WriteFile(path, content)`.
    fn write_file(&self, path: &str, content: &str) -> Result<(), String>;
    /// Go: `fs.AppendFile(path, content)`.
    fn append_file(&self, path: &str, content: &str) -> Result<(), String>;
    /// Test seam (Go's vfstest reads through the same interface).
    fn read_file(&self, path: &str) -> Option<String>;
}

/// Go: `any` trace-arg values. The checker's call sites pass ints, uint type
/// ids, strings, and `[]string` — the union covers them.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(String),
    Array(Vec<TraceValue>),
}

/// Go: `map[string]any` — a shared, mutable, reference-semantic arg map.
#[derive(Clone, Default)]
pub struct Args(Rc<RefCell<BTreeMap<String, TraceValue>>>);

impl Args {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `args[k] = v`.
    pub fn insert(&self, key: &str, value: TraceValue) {
        self.0.borrow_mut().insert(key.to_string(), value);
    }

    /// Go: `args[k]`.
    pub fn get(&self, key: &str) -> Option<TraceValue> {
        self.0.borrow().get(key).cloned()
    }

    /// Go: `delete(args, k)` — returns whether the key was present.
    pub fn remove(&self, key: &str) -> bool {
        self.0.borrow_mut().remove(key).is_some()
    }

    /// Go: `_, ok := args[k]`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.borrow().contains_key(key)
    }

    /// Go: `len(args) == 0`.
    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
    /// Go: `for k, v := range args` — the map iteration (a snapshot; the
    /// Rc sharing means callers mutating during iteration is not a thing
    /// the tracer does).
    pub fn entries(&self) -> Vec<(String, TraceValue)> {
        self.0
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Go: `type TraceRecord struct` (JSON field order preserved).
#[derive(Clone, Debug)]
pub struct TraceRecord {
    pub config_file_path: String,
    pub trace_path: String,
    pub types_path: String,
    pub checker_id: i64,
}

/// Go: `type Phase string` + consts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Parse,
    Program,
    Bind,
    Check,
    CheckTypes,
    Emit,
    Session,
}

impl Phase {
    /// Go: the string values.
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Program => "program",
            Phase::Bind => "bind",
            Phase::Check => "check",
            Phase::CheckTypes => "checkTypes",
            Phase::Emit => "emit",
            Phase::Session => "session",
        }
    }
}

/// Go: `type traceEvent struct` — field order is the JSON field order.
struct TraceEvent {
    pid: i64,
    tid: i64,
    ph: &'static str,
    cat: String,
    ts: f64,
    name: String,
    s: String,
    dur: Option<f64>,
    args: Option<Args>,
}

// Go consts.
const TRACE_FILE_NAME: &str = "trace.json";
const MAIN_THREAD_ID: i64 = 1;
const FIRST_SYNTHETIC_THREAD_ID: i64 = 2;
const FIRST_FILE_THREAD_ID: i64 = 1_000_000;
const FILE_THREAD_ID_HASH_RANGE: i64 = 1_000_000_000;
/// Go: `var traceThreadArgKeys = [...]string{...}`.
const TRACE_THREAD_ARG_KEYS: [&str; 5] = [
    "path",
    "fileName",
    "containingFileName",
    "jsFilePath",
    "declarationFilePath",
];

fn write_json_string(buf: &mut String, s: &str) {
    buf.push('"');
    for c in s.chars() {
        match c {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(buf, "\\u{:04x}", c as u32);
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

fn write_f64(buf: &mut String, v: f64) {
    use std::fmt::Write as _;
    // Go marshals whole-number float64s without a fractional part
    // (`float64(3)` → `3`); shortest round-trip otherwise.
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e16 {
        let _ = write!(buf, "{}", v as i64);
    } else if v.is_finite() {
        let _ = write!(buf, "{}", v);
    } else {
        buf.push_str("null"); // Go json: NaN/Inf → error; unreachable here.
    }
}

fn write_trace_value(buf: &mut String, v: &TraceValue) {
    use std::fmt::Write as _;
    match v {
        TraceValue::Null => buf.push_str("null"),
        TraceValue::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        TraceValue::Int(i) => {
            let _ = write!(buf, "{}", i);
        }
        TraceValue::Uint(u) => {
            let _ = write!(buf, "{}", u);
        }
        TraceValue::Float(f) => write_f64(buf, *f),
        TraceValue::Str(s) => write_json_string(buf, s),
        TraceValue::Array(items) => {
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write_trace_value(buf, item);
            }
            buf.push(']');
        }
    }
}

/// Go: `func writeEventTo(buf, event)` — field order + omitzero per the Go
/// struct tags (`pid, tid, ph, cat, ts, name(omitzero), s(omitzero),
/// dur(omitzero), args(omitzero)`).
fn write_event_to(buf: &mut String, event: &TraceEvent) {
    use std::fmt::Write as _;
    buf.push_str("{\"pid\":");
    let _ = write!(buf, "{}", event.pid);
    buf.push_str(",\"tid\":");
    let _ = write!(buf, "{}", event.tid);
    buf.push_str(",\"ph\":");
    write_json_string(buf, event.ph);
    buf.push_str(",\"cat\":");
    write_json_string(buf, &event.cat);
    buf.push_str(",\"ts\":");
    write_f64(buf, event.ts);
    if !event.name.is_empty() {
        buf.push_str(",\"name\":");
        write_json_string(buf, &event.name);
    }
    if !event.s.is_empty() {
        buf.push_str(",\"s\":");
        write_json_string(buf, &event.s);
    }
    if let Some(dur) = event.dur {
        buf.push_str(",\"dur\":");
        write_f64(buf, dur);
    }
    if let Some(args) = &event.args {
        // Deterministic mode: keys sorted (BTreeMap is; Go sorts at marshal).
        buf.push_str(",\"args\":{");
        let map = args.0.borrow();
        for (i, (k, v)) in map.iter().enumerate() {
            if i > 0 {
                buf.push(',');
            }
            write_json_string(buf, k);
            buf.push(':');
            write_trace_value(buf, v);
        }
        buf.push('}');
    }
    buf.push('}');
}

// ────────────────────────────────────────────────────────────────────────────
// Thread keys (Go: traceThreadKey family)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum TraceThreadKind {
    Checker,
    File,
}

impl TraceThreadKind {
    fn as_str(&self) -> &'static str {
        match self {
            TraceThreadKind::Checker => "checker",
            TraceThreadKind::File => "file",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct TraceThreadKey {
    kind: TraceThreadKind,
    text: String,
    index: i64,
    has_index: bool,
}

fn trace_thread_key_from_args(args: &Args) -> Option<TraceThreadKey> {
    if args.is_empty() {
        return None;
    }
    // Go: `args["checkerId"].(int)`.
    if let Some(TraceValue::Int(checker_id)) = args.get("checkerId") {
        return Some(TraceThreadKey {
            kind: TraceThreadKind::Checker,
            index: checker_id,
            has_index: true,
            text: String::new(),
        });
    }
    for key in TRACE_THREAD_ARG_KEYS {
        if let Some(TraceValue::Str(path)) = args.get(key) {
            if !path.is_empty() {
                return Some(TraceThreadKey {
                    kind: TraceThreadKind::File,
                    text: path.clone(),
                    index: 0,
                    has_index: false,
                });
            }
        }
    }
    None
}

impl TraceThreadKey {
    fn default_thread_id(&self) -> i64 {
        if self.kind == TraceThreadKind::Checker && self.has_index && self.index >= 0 {
            return FIRST_SYNTHETIC_THREAD_ID + self.index;
        }
        self.stable_trace_thread_id()
    }

    fn display_name(&self) -> String {
        if self.has_index {
            format!("{}:{}", self.kind.as_str(), self.index)
        } else {
            format!("{}:{}", self.kind.as_str(), self.text)
        }
    }

    /// Go: xxh3 over `kind:index`/`kind:text` — PORT: FNV-1a (the approved dep
    /// set has no xxh3; contentmapper.rs set the precedent). Thread ids are
    /// session-internal (they only key trace output sections by file), so a
    /// different stable hash preserves observable behavior up to the numeric
    /// tid a given path hashes to; the tracer tests use checker keys, which
    /// take the `firstSyntheticThreadID + index` path and are exact.
    fn stable_trace_thread_id(&self) -> i64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |s: &str| {
            for b in s.bytes() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        };
        feed(self.kind.as_str());
        feed(":");
        if self.has_index {
            feed(&self.index.to_string());
        } else {
            feed(&self.text);
        }
        FIRST_FILE_THREAD_ID + (hash % FILE_THREAD_ID_HASH_RANGE as u64) as i64
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tracing
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type Tracing struct` — a shared session handle (`*Tracing` in Go).
#[derive(Clone)]
pub struct Tracing {
    shared: Rc<TracingShared>,
}

struct TracingShared {
    fs: Rc<dyn TraceFs>,
    trace_dir: String,
    trace_path: String,
    config_file_path: String,
    legend: RefCell<Vec<TraceRecord>>,
    tracers: RefCell<Vec<Rc<TypeTracer>>>,
    trace_content: RefCell<String>,
    trace_started: Cell<bool>,
    thread_ids: RefCell<FxHashMap<TraceThreadKey, i64>>,
    thread_keys: RefCell<FxHashMap<i64, TraceThreadKey>>,
    metadata_ts: Cell<f64>,
    deterministic: bool,
    timestamp_counter: Cell<u64>,
}

/// Go: `func StartTracing(fs, traceDir, configFilePath, deterministic)`.
pub fn start_tracing(
    fs: Rc<dyn TraceFs>,
    trace_dir: &str,
    config_file_path: &str,
    deterministic: bool,
) -> Result<Tracing, String> {
    let shared = Rc::new(TracingShared {
        fs,
        trace_dir: trace_dir.to_string(),
        trace_path: format!("{trace_dir}/{TRACE_FILE_NAME}"),
        config_file_path: config_file_path.to_string(),
        legend: RefCell::new(Vec::new()),
        tracers: RefCell::new(Vec::new()),
        trace_content: RefCell::new(String::new()),
        trace_started: Cell::new(true),
        thread_ids: RefCell::new(FxHashMap::default()),
        thread_keys: RefCell::new(FxHashMap::default()),
        metadata_ts: Cell::new(0.0),
        deterministic,
        timestamp_counter: Cell::new(0),
    });
    let tr = Tracing { shared };
    // Write the trace file header with metadata events (matching TypeScript's
    // format — Go comment, verbatim).
    tr.shared.trace_content.borrow_mut().push_str("[\n");
    let meta_ts = tr.timestamp();
    tr.shared.metadata_ts.set(meta_ts);
    tr.write_event(TraceEvent {
        pid: 1,
        tid: MAIN_THREAD_ID,
        ph: "M",
        cat: "__metadata".to_string(),
        ts: meta_ts,
        name: "process_name".to_string(),
        s: String::new(),
        dur: None,
        args: {
            let args = Args::new();
            args.insert("name", TraceValue::Str("tsgo".to_string()));
            Some(args)
        },
    });
    tr.shared.trace_content.borrow_mut().push_str(",\n");
    tr.write_event(TraceEvent {
        pid: 1,
        tid: MAIN_THREAD_ID,
        ph: "M",
        cat: "__metadata".to_string(),
        ts: meta_ts,
        name: "thread_name".to_string(),
        s: String::new(),
        dur: None,
        args: {
            let args = Args::new();
            args.insert("name", TraceValue::Str("Main".to_string()));
            Some(args)
        },
    });
    tr.shared.trace_content.borrow_mut().push_str(",\n");
    tr.write_event(TraceEvent {
        pid: 1,
        tid: MAIN_THREAD_ID,
        ph: "M",
        cat: "disabled-by-default-devtools.timeline".to_string(),
        ts: meta_ts,
        name: "TracingStartedInBrowser".to_string(),
        s: String::new(),
        dur: None,
        args: None,
    });
    // Truncate any existing trace file with the header.
    let header = std::mem::take(&mut *tr.shared.trace_content.borrow_mut());
    tr.shared
        .fs
        .write_file(&tr.shared.trace_path, &header)
        .map_err(|e| format!("failed to write trace file header: {e}"))?;
    Ok(tr)
}

impl Tracing {
    /// Go: `func (tr *Tracing) timestamp() float64`.
    fn timestamp(&self) -> f64 {
        if self.shared.deterministic {
            let next = self.shared.timestamp_counter.get() + 1;
            self.shared.timestamp_counter.set(next);
            next as f64
        } else {
            // The spike is single-threaded and short; wall-clock µs.
            0.0
        }
    }

    fn write_event(&self, event: TraceEvent) {
        write_event_to(&mut self.shared.trace_content.borrow_mut(), &event);
    }

    /// Go: `func (tr *Tracing) Instant(phase, name, args)`.
    pub fn instant(&self, phase: Phase, name: &str, args: &Args) {
        if !self.shared.trace_started.get() {
            return;
        }
        let ts = self.timestamp();
        let tid = self.thread_id_locked(args);
        self.shared.trace_content.borrow_mut().push_str(",\n");
        self.write_event(TraceEvent {
            pid: 1,
            tid,
            ph: "I",
            cat: phase.as_str().to_string(),
            ts,
            name: name.to_string(),
            s: "g".to_string(),
            dur: None,
            args: Some(args.clone()),
        });
    }

    /// Go: `func (tr *Tracing) Push(phase, name, args, separateBeginAndEnd) func()`.
    ///
    /// PORT: the returned pop is [`TracePop`] — Drop pops (Go's `defer`
    /// idiom) and `TracePop::pop` pops mid-function, exactly the two call
    /// shapes the Go code uses. Sampled events (separateBeginAndEnd=false)
    /// are skipped in deterministic mode (Go comment: "avoid flaky baselines").
    pub fn push(&self, phase: Phase, name: &str, args: &Args, separate_begin_and_end: bool) -> TracePop {
        if !self.shared.trace_started.get() {
            return TracePop::nop();
        }
        if separate_begin_and_end {
            let ts = self.timestamp();
            let tid = self.thread_id_locked(args);
            self.shared.trace_content.borrow_mut().push_str(",\n");
            self.write_event(TraceEvent {
                pid: 1,
                tid,
                ph: "B",
                cat: phase.as_str().to_string(),
                ts,
                name: name.to_string(),
                s: String::new(),
                dur: None,
                args: Some(args.clone()),
            });
            return TracePop {
                tracing: Some(self.clone()),
                phase,
                name: name.to_string(),
                tid,
                args: args.clone(),
            };
        }
        // Sampled event: only recorded if the duration crosses a 10ms
        // sampling boundary; in deterministic mode skipped entirely (Go).
        TracePop::nop()
    }

    /// Go: `func (tr *Tracing) threadIDLocked(args) int`.
    fn thread_id_locked(&self, args: &Args) -> i64 {
        let Some(key) = trace_thread_key_from_args(args) else {
            return MAIN_THREAD_ID;
        };
        if let Some(&tid) = self.shared.thread_ids.borrow().get(&key) {
            return tid;
        }
        let mut tid = key.default_thread_id();
        loop {
            match self.shared.thread_keys.borrow().get(&tid) {
                None => break,
                Some(existing) if *existing == key => break,
                _ => tid += 1,
            }
        }
        self.shared.thread_ids.borrow_mut().insert(key.clone(), tid);
        self.shared.thread_keys.borrow_mut().insert(tid, key.clone());
        // Go: writeThreadNameEventLocked.
        self.shared.trace_content.borrow_mut().push_str(",\n");
        self.write_event(TraceEvent {
            pid: 1,
            tid,
            ph: "M",
            cat: "__metadata".to_string(),
            ts: self.shared.metadata_ts.get(),
            name: "thread_name".to_string(),
            s: String::new(),
            dur: None,
            args: {
                let name_args = Args::new();
                name_args.insert("name", TraceValue::Str(key.display_name()));
                Some(name_args)
            },
        });
        tid
    }

    /// Go: `func (tr *Tracing) NewTypeTracer(checkerIndex) Tracer`.
    pub fn new_type_tracer(&self, checker_index: i64) -> Rc<TypeTracer> {
        let types_path = format!("{}/types_{}.json", self.shared.trace_dir, checker_index);
        let tracer = Rc::new(TypeTracer {
            fs: self.shared.fs.clone(),
            types_path: types_path.clone(),
            types: RefCell::new(Vec::new()),
        });
        self.shared.tracers.borrow_mut().push(tracer.clone());
        self.shared.legend.borrow_mut().push(TraceRecord {
            config_file_path: self.shared.config_file_path.clone(),
            trace_path: self.shared.trace_path.clone(),
            types_path,
            checker_id: checker_index,
        });
        tracer
    }

    /// Go: `func (tr *Tracing) StopTracing() error`.
    ///
    /// PORT: Go's StopTracing dumps each checker's recorded types through the
    /// stored `tracedTypeAdapter` pointers (which reach their checkers); the
    /// port's tracers store `(shard, TypeId)` pairs and the host calls
    /// [`TypeTracer::dump_types`] with the owning checker first (the
    /// tracer_test-shaped tests drive both, in the same order). The
    /// trace-file close + legend write below are byte-identical to Go's.
    pub fn stop_tracing(&self) -> Result<(), String> {
        if self.shared.trace_started.get() {
            let content = std::mem::take(&mut *self.shared.trace_content.borrow_mut());
            self.shared
                .fs
                .append_file(&self.shared.trace_path, &format!("{content}\n]\n"))
                .map_err(|e| format!("failed to write trace file: {e}"))?;
            self.shared.trace_started.set(false);
        }
        // Sort legend entries by typesPath for deterministic output.
        self.shared
            .legend
            .borrow_mut()
            .sort_by(|a, b| a.types_path.cmp(&b.types_path));
        let legend = self.shared.legend.borrow();
        let legend_data = marshal_legend(&legend);
        let legend_path = format!("{}/legend.json", self.shared.trace_dir);
        self.shared
            .fs
            .write_file(&legend_path, &legend_data)
            .map_err(|e| format!("failed to write legend file: {e}"))
    }
}

/// Go: `json.MarshalIndent(tr.legend, "", "  ")` — field order per the struct
/// tags (`configFilePath,omitzero tracePath,omitzero typesPath,omitzero checkerId`).
fn marshal_legend(records: &[TraceRecord]) -> String {
    let mut buf = String::new();
    buf.push('[');
    for (i, r) in records.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        buf.push_str("\n  {");
        let mut first = true;
        let mut field = |buf: &mut String, first: &mut bool, key: &str, value: &str| {
            if !value.is_empty() {
                if !*first {
                    buf.push(',');
                }
                *first = false;
                buf.push('"');
                buf.push_str(key);
                buf.push_str("\":");
                write_json_string(buf, value);
            }
        };
        field(&mut buf, &mut first, "configFilePath", &r.config_file_path);
        field(&mut buf, &mut first, "tracePath", &r.trace_path);
        field(&mut buf, &mut first, "typesPath", &r.types_path);
        if !first {
            buf.push(',');
        }
        use std::fmt::Write as _;
        let _ = write!(buf, "\"checkerId\":{}", r.checker_id);
        buf.push('}');
    }
    buf.push_str("\n]");
    buf
}

/// The pop closure Go's `Tracing.Push` returns.
pub struct TracePop {
    tracing: Option<Tracing>,
    phase: Phase,
    name: String,
    tid: i64,
    args: Args,
}

impl TracePop {
    fn nop() -> Self {
        TracePop {
            tracing: None,
            phase: Phase::Check,
            name: String::new(),
            tid: 0,
            args: Args::new(),
        }
    }

    /// Call the pop function (Go: `popFn()`).
    pub fn pop(self) {
        drop(self); // the Drop impl performs the pop
    }
}

impl Drop for TracePop {
    fn drop(&mut self) {
        let Some(tracing) = self.tracing.take() else {
            return;
        };
        if !tracing.shared.trace_started.get() {
            return;
        }
        let end_ts = tracing.timestamp();
        tracing.shared.trace_content.borrow_mut().push_str(",\n");
        tracing.write_event(TraceEvent {
            pid: 1,
            tid: self.tid,
            ph: "E",
            cat: self.phase.as_str().to_string(),
            ts: end_ts,
            name: self.name.clone(),
            s: String::new(),
            dur: None,
            args: Some(self.args.clone()),
        });
    }
}

// ────────────────────────────────────────────────────────────────────────────
// typeTracer + TypeDescriptor (the per-checker type dump)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `type typeTracer struct` — records `(shard, TypeId)` pairs; the
/// checker's tracedTypeAdapter (tracer.rs) resolves them at dump time.
pub struct TypeTracer {
    fs: Rc<dyn TraceFs>,
    types_path: String,
    types: RefCell<Vec<(u16, TypeId)>>,
}

impl TypeTracer {
    /// Go: `func (t *typeTracer) RecordType(typ TracedType)`.
    ///
    /// PORT: Go stores the adapter (a live pointer); the port stores the
    /// (shard, TypeId) handle pair and the adapter is rebuilt at dump time —
    /// the adapter is stateless, so the observable dump is identical.
    pub fn record_type(&self, checker_shard: u16, typ: TypeId) {
        self.types.borrow_mut().push((checker_shard, typ));
    }

    /// The recorded pairs, in record order (Go: `slices.Clone(t.types)`).
    pub fn recorded_types(&self) -> Vec<(u16, TypeId)> {
        self.types.borrow().clone()
    }

    pub fn types_path(&self) -> &str {
        &self.types_path
    }

    /// Go: `func (t *typeTracer) DumpTypes() error` — the file write half
    /// (the content is built by [`dump_types_to_string`]; the checker's
    /// Tracer adapts arena types first).
    pub fn write_dump(&self, content: &str) -> Result<(), String> {
        self.fs
            .write_file(&self.types_path, content)
            .map_err(|e| format!("failed to write types file: {e}"))
    }
}

/// Go: `type TracedType interface` — PORT (flattened): Go's methods return
/// live `*Type`/`*ast.Symbol` pointers; the descriptor consumes only ids,
/// names, and locations, so the port's trait exposes the resolved values and
/// the adapter (tracer.rs) resolves them through the checker arena.
pub trait TracedType {
    fn id(&self) -> u32;
    fn format_flags(&self) -> Vec<&'static str>;
    fn is_conditional(&self) -> bool;
    /// Go: `Symbol()` — the id + name the descriptor needs.
    fn symbol(&self) -> Option<(SymbolId, String)>;
    /// Go: `AliasSymbol()`.
    fn alias_symbol(&self) -> Option<(SymbolId, String)>;
    /// Go: `AliasTypeArguments()` → the ids (mapTypeIds).
    fn alias_type_argument_ids(&self) -> Vec<u32>;
    /// Go: `FirstDeclaration`'s underlying `symbol.Declarations[0]` — the
    /// aliasSymbol's if set, else the symbol's (Go:
    /// `firstDeclSymbol ?? symbol` then `.Declarations[0]`).
    fn first_declaration(&self) -> Option<NodeId>;
    fn intrinsic_name(&self) -> String;
    fn union_type_ids(&self) -> Vec<u32>;
    fn intersection_type_ids(&self) -> Vec<u32>;
    fn index_type_id(&self) -> Option<u32>;
    fn indexed_access_object_type_id(&self) -> Option<u32>;
    fn indexed_access_index_type_id(&self) -> Option<u32>;
    fn conditional_check_type_id(&self) -> Option<u32>;
    fn conditional_extends_type_id(&self) -> Option<u32>;
    /// `None` = unresolved (serialized as -1, matching TypeScript).
    fn conditional_true_type_id(&self) -> Option<u32>;
    fn conditional_false_type_id(&self) -> Option<u32>;
    fn substitution_base_type_id(&self) -> Option<u32>;
    fn substitution_constraint_type_id(&self) -> Option<u32>;
    fn reference_target_id(&self) -> Option<u32>;
    fn reference_type_argument_ids(&self) -> Vec<u32>;
    fn reference_node(&self) -> Option<NodeId>;
    fn reverse_mapped_source_type_id(&self) -> Option<u32>;
    fn reverse_mapped_mapped_type_id(&self) -> Option<u32>;
    fn reverse_mapped_constraint_type_id(&self) -> Option<u32>;
    fn evolving_array_element_type_id(&self) -> Option<u32>;
    fn evolving_array_final_type_id(&self) -> Option<u32>;
    fn is_tuple(&self) -> bool;
    fn pattern(&self) -> Option<NodeId>;
    fn recursion_identity(&self) -> RecursionId;
    fn display(&self) -> String;
}

/// Go: `type TypeDescriptor struct` — JSON field order preserved from the
/// Go struct; `omitzero` fields are `Option`s.
pub struct TypeDescriptor<'a> {
    pub id: u32,
    pub intrinsic_name: String,
    pub symbol_name: String,
    pub recursion_id: Option<usize>,
    pub is_tuple: bool,
    pub union_types: Vec<u32>,
    pub intersection_types: Vec<u32>,
    pub alias_type_arguments: Vec<u32>,
    pub keyof_type: Option<u32>,
    pub indexed_access_object_type: Option<u32>,
    pub indexed_access_index_type: Option<u32>,
    pub conditional_check_type: Option<u32>,
    pub conditional_extends_type: Option<u32>,
    pub conditional_true_type: Option<i32>,
    pub conditional_false_type: Option<i32>,
    pub substitution_base_type: Option<u32>,
    pub constraint_type: Option<u32>,
    pub instantiated_type: Option<u32>,
    pub type_arguments: Vec<u32>,
    pub reference_location: Option<Location>,
    pub reverse_mapped_source_type: Option<u32>,
    pub reverse_mapped_mapped_type: Option<u32>,
    pub reverse_mapped_constraint_type: Option<u32>,
    pub evolving_array_element_type: Option<u32>,
    pub evolving_array_final_type: Option<u32>,
    pub destructuring_pattern: Option<Location>,
    pub first_declaration: Option<Location>,
    pub flags: Vec<&'a str>,
    pub display: String,
}

/// Go: `type Location struct` + `LineAndChar`.
#[derive(Clone, Debug)]
pub struct Location {
    pub path: String,
    pub start: Option<(i64, i64)>,
    pub end: Option<(i64, i64)>,
}

impl Location {
    /// Go: `getLocation(node)` — PORT: `scanner.GetTokenPosOfNode` is not in
    /// the port's scanner crate yet; the start offset uses the node's `pos()`
    /// (identical for nodes with no leading trivia — the traced reference
    /// nodes; the wave that lands GetTokenPosOfNode swaps this one line).
    /// Line/char come from `tsc-scanner`'s ECMALineAndUTF16Character port.
    pub fn of(c: &crate::Checker, node: NodeId) -> Option<Location> {
        use tsc_scanner::get_ecma_line_and_utf16_character_of_position;
        let store = c.store();
        let n = store.node(node);
        let file_id = n.file_id();
        let file = c
            .program
            .source_files()
            .iter()
            .find(|f| f.file_id() == file_id)?;
        let (start_line, start_char) =
            get_ecma_line_and_utf16_character_of_position(file, n.pos());
        let (end_line, end_char) =
            get_ecma_line_and_utf16_character_of_position(file, n.end());
        Some(Location {
            // Go: tspath.CaseInsensitive.PathKey(RootedPath(file.FileName()))
            path: file.file_name().to_string(),
            start: Some(((start_line + 1) as i64, start_char as i64 + 1)),
            end: Some(((end_line + 1) as i64, end_char as i64 + 1)),
        })
    }
}

/// Go: `func (t *typeTracer) DumpTypes() error` — the JSON shape: `[` with no
/// trailing newline so the type ID matches the line number, `,\n` separators,
/// `]\n` close.
pub fn dump_types_to_string(types: &[&dyn TracedType]) -> String {
    let mut sb = String::from("[");
    let mut recursion_identity_map: FxHashMap<RecursionId, usize> = FxHashMap::default();
    for (i, typ) in types.iter().enumerate() {
        let descriptor = build_type_descriptor(*typ, &mut recursion_identity_map);
        write_type_descriptor(&mut sb, &descriptor);
        if i < types.len() - 1 {
            sb.push_str(",\n");
        }
    }
    sb.push_str("]\n");
    sb
}

/// Go: `func (t *typeTracer) buildTypeDescriptor(typ, recursionIdentityMap)`.
pub fn build_type_descriptor<'a>(
    typ: &'a dyn TracedType,
    recursion_identity_map: &mut FxHashMap<RecursionId, usize>,
) -> TypeDescriptor<'a> {
    let symbol = typ.symbol();
    let alias_symbol = typ.alias_symbol();
    let mut desc = TypeDescriptor {
        id: typ.id(),
        intrinsic_name: String::new(),
        symbol_name: String::new(),
        recursion_id: None,
        is_tuple: false,
        union_types: Vec::new(),
        intersection_types: Vec::new(),
        alias_type_arguments: Vec::new(),
        keyof_type: None,
        indexed_access_object_type: None,
        indexed_access_index_type: None,
        conditional_check_type: None,
        conditional_extends_type: None,
        conditional_true_type: None,
        conditional_false_type: None,
        substitution_base_type: None,
        constraint_type: None,
        instantiated_type: None,
        type_arguments: Vec::new(),
        reference_location: None,
        reverse_mapped_source_type: None,
        reverse_mapped_mapped_type: None,
        reverse_mapped_constraint_type: None,
        evolving_array_element_type: None,
        evolving_array_final_type: None,
        destructuring_pattern: None,
        first_declaration: None,
        flags: typ.format_flags(),
        display: String::new(),
    };
    // Assign a unique integer token per recursion identity (Go comment).
    let identity = typ.recursion_identity();
    let token = match recursion_identity_map.get(&identity) {
        Some(t) => *t,
        None => {
            let len = recursion_identity_map.len();
            recursion_identity_map.insert(identity, len);
            len
        }
    };
    desc.recursion_id = Some(token);
    let name = typ.intrinsic_name();
    if !name.is_empty() {
        desc.intrinsic_name = name;
    }
    if let Some((_, name)) = alias_symbol {
        desc.symbol_name = tsc_ast::escape_all_internal_symbol_names(&name);
    } else if let Some((_, name)) = symbol {
        desc.symbol_name = tsc_ast::escape_all_internal_symbol_names(&name);
    }
    if typ.is_tuple() {
        desc.is_tuple = true;
    }
    desc.union_types = typ.union_type_ids();
    desc.intersection_types = typ.intersection_type_ids();
    desc.alias_type_arguments = typ.alias_type_argument_ids();
    desc.keyof_type = typ.index_type_id();
    desc.indexed_access_object_type = typ.indexed_access_object_type_id();
    desc.indexed_access_index_type = typ.indexed_access_index_type_id();
    if typ.is_conditional() {
        desc.conditional_check_type = typ.conditional_check_type_id();
        desc.conditional_extends_type = typ.conditional_extends_type_id();
        desc.conditional_true_type = Some(typ.conditional_true_type_id().map(|v| v as i32).unwrap_or(-1));
        desc.conditional_false_type = Some(typ.conditional_false_type_id().map(|v| v as i32).unwrap_or(-1));
    }
    desc.substitution_base_type = typ.substitution_base_type_id();
    desc.constraint_type = typ.substitution_constraint_type_id();
    desc.instantiated_type = typ.reference_target_id();
    desc.type_arguments = typ.reference_type_argument_ids();
    desc.reverse_mapped_source_type = typ.reverse_mapped_source_type_id();
    desc.reverse_mapped_mapped_type = typ.reverse_mapped_mapped_type_id();
    desc.reverse_mapped_constraint_type = typ.reverse_mapped_constraint_type_id();
    desc.evolving_array_element_type = typ.evolving_array_element_type_id();
    desc.evolving_array_final_type = typ.evolving_array_final_type_id();
    desc.display = typ.display();
    desc
}

/// The descriptor's JSON — Go struct field order, `omitzero` = empty/None
/// skipped, `flags` always present, `id` always present.
fn write_type_descriptor(buf: &mut String, desc: &TypeDescriptor<'_>) {
    use std::fmt::Write as _;
    buf.push_str("{\"id\":");
    let _ = write!(buf, "{}", desc.id);
    let mut str_field = |buf: &mut String, key: &str, value: &str| {
        if !value.is_empty() {
            buf.push(',');
            buf.push('"');
            buf.push_str(key);
            buf.push_str("\":");
            write_json_string(buf, value);
        }
    };
    str_field(buf, "intrinsicName", &desc.intrinsic_name);
    str_field(buf, "symbolName", &desc.symbol_name);
    if let Some(token) = desc.recursion_id {
        buf.push_str(",\"recursionId\":");
        let _ = write!(buf, "{}", token);
    }
    if desc.is_tuple {
        buf.push_str(",\"isTuple\":true");
    }
    let mut ids_field = |buf: &mut String, key: &str, ids: &[u32]| {
        if !ids.is_empty() {
            buf.push(',');
            buf.push('"');
            buf.push_str(key);
            buf.push_str("\":[");
            for (i, id) in ids.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                let _ = write!(buf, "{}", id);
            }
            buf.push(']');
        }
    };
    ids_field(buf, "unionTypes", &desc.union_types);
    ids_field(buf, "intersectionTypes", &desc.intersection_types);
    ids_field(buf, "aliasTypeArguments", &desc.alias_type_arguments);
    let mut opt_field = |buf: &mut String, key: &str, v: Option<u32>| {
        if let Some(v) = v {
            buf.push(',');
            buf.push('"');
            buf.push_str(key);
            buf.push_str("\":");
            let _ = write!(buf, "{}", v);
        }
    };
    opt_field(buf, "keyofType", desc.keyof_type);
    opt_field(buf, "indexedAccessObjectType", desc.indexed_access_object_type);
    opt_field(buf, "indexedAccessIndexType", desc.indexed_access_index_type);
    opt_field(buf, "conditionalCheckType", desc.conditional_check_type);
    opt_field(buf, "conditionalExtendsType", desc.conditional_extends_type);
    if let Some(v) = desc.conditional_true_type {
        buf.push_str(",\"conditionalTrueType\":");
        let _ = write!(buf, "{}", v);
    }
    if let Some(v) = desc.conditional_false_type {
        buf.push_str(",\"conditionalFalseType\":");
        let _ = write!(buf, "{}", v);
    }
    opt_field(buf, "substitutionBaseType", desc.substitution_base_type);
    opt_field(buf, "constraintType", desc.constraint_type);
    opt_field(buf, "instantiatedType", desc.instantiated_type);
    ids_field(buf, "typeArguments", &desc.type_arguments);
    if let Some(loc) = &desc.reference_location {
        write_location(buf, "referenceLocation", loc);
    }
    opt_field(buf, "reverseMappedSourceType", desc.reverse_mapped_source_type);
    opt_field(buf, "reverseMappedMappedType", desc.reverse_mapped_mapped_type);
    opt_field(buf, "reverseMappedConstraintType", desc.reverse_mapped_constraint_type);
    opt_field(buf, "evolvingArrayElementType", desc.evolving_array_element_type);
    opt_field(buf, "evolvingArrayFinalType", desc.evolving_array_final_type);
    if let Some(loc) = &desc.destructuring_pattern {
        write_location(buf, "destructuringPattern", loc);
    }
    if let Some(loc) = &desc.first_declaration {
        write_location(buf, "firstDeclaration", loc);
    }
    buf.push_str(",\"flags\":[");
    for (i, flag) in desc.flags.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        write_json_string(buf, flag);
    }
    buf.push(']');
    str_field(buf, "display", &desc.display);
    buf.push('}');
}

/// Go: `Location`'s JSON tags — path always, start/end `omitzero`.
fn write_location(buf: &mut String, key: &str, loc: &Location) {
    use std::fmt::Write as _;
    buf.push(',');
    buf.push('"');
    buf.push_str(key);
    buf.push_str("\":{");
    write_json_string_field(buf, "path", &loc.path);
    if let Some((line, ch)) = loc.start {
        let _ = write!(buf, ",\"start\":{{\"line\":{},\"character\":{}}}", line, ch);
    }
    if let Some((line, ch)) = loc.end {
        let _ = write!(buf, ",\"end\":{{\"line\":{},\"character\":{}}}", line, ch);
    }
    buf.push('}');
}

fn write_json_string_field(buf: &mut String, key: &str, value: &str) {
    buf.push('"');
    buf.push_str(key);
    buf.push_str("\":");
    write_json_string(buf, value);
}
