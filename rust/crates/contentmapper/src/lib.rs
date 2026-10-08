// Port of tsc/internal/contentmapper/contentmapper.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go's `TransformIdentity` returns an `xxh3.Uint128`. The approved dep
//     set (SPEC §5.11) has no xxh3 (and there is no network to add it), so
//     the port hashes the same byte layout with 128-bit FNV-1a — a pure
//     function of the same inputs, but NOT the same fingerprint Go computes.
//     Callers must not compare across implementations. (PORTING-NOTES.)
//   - Go's `MarshalDeclaredOptions` walks the struct with reflection; the
//     generated `compiler_option_fields_generated.rs` (from gen-options)
//     performs the same per-field marshal as an exhaustive match over the
//     132 json tags.
//   - The `Host` machinery (spawning mapper packages over JSON-RPC via
//     internal/ipc — host.go/hostimpl.go) is out of the M4 wave: it needs
//     process spawning + ipc, which the Rust side does not have yet. Not
//     ported; see PORTING-NOTES.
//   - `json.Value` is `Box<serde_json::value::RawValue>` via the tsc-json
//     facade, exactly as in the Go json package.
//   - Go's nil-vs-empty slice/Value distinction collapses: unset reads as
//     `None`/empty Vec (documented in PORTING-NOTES).

pub mod compiler_option_fields_generated;
pub mod field_json;

use std::sync::OnceLock;

use tsc_collections::{new_set_from_items, OrderedMap, Set};
use tsc_core::options_generated::CompilerOptions;
use tsc_json::Value;
use tsc_tspath::RootedDirectoryPath;

/// Go: `var ErrProjectUnavailable = errors.New("content mapper project is unavailable")`.
pub const ERR_PROJECT_UNAVAILABLE: &str = "content mapper project is unavailable";

/// Go: `type Definition struct` — a content mapper as declared in tsconfig.
#[derive(Clone, Debug, Default)]
pub struct Definition {
    pub package: String,
    pub extensions: Vec<String>,
    pub options: Option<Value>,
}

/// Go: `type Manifest struct` — the package.json-side mapper description.
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub exec: Vec<String>,
    pub compiler_options: Vec<String>,
    pub dynamic_config: bool,
}

/// Go: `type Mapper struct` — a resolved content mapper.
#[derive(Clone, Debug, Default)]
pub struct Mapper {
    pub definition: Definition,
    pub manifest: Manifest,
    /// Go: `PackageDirectory tspath.RootedDirectoryPath`.
    pub package_directory: RootedDirectoryPath,
    /// Go: `ContributionID string` — LSP extension-provided.
    pub contribution_id: String,
}

static SUPPORTED_VIRTUAL_EXTENSIONS: OnceLock<Set<&'static str>> = OnceLock::new();

/// Go: `func IsSupportedVirtualExtension(extension string) bool`.
pub fn is_supported_virtual_extension(extension: &str) -> bool {
    SUPPORTED_VIRTUAL_EXTENSIONS
        .get_or_init(|| {
            new_set_from_items([
                ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".json",
            ])
        })
        .has(extension)
}

impl Mapper {
    /// Go: the embedded `Definition`/`Manifest` field promotions — exposed
    /// as accessors for the flattening Go gives via embedding.
    pub fn package(&self) -> &str {
        &self.definition.package
    }
    pub fn extensions(&self) -> &[String] {
        &self.definition.extensions
    }
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
    pub fn version(&self) -> &str {
        &self.manifest.version
    }

    /// Go: `func (m *Mapper) DiagnosticName() string`.
    pub fn diagnostic_name(&self) -> &str {
        if !self.manifest.name.is_empty() {
            &self.manifest.name
        } else if !self.definition.package.is_empty() {
            &self.definition.package
        } else {
            &self.contribution_id
        }
    }

    /// Go: `func (m *Mapper) Identity() string`.
    pub fn identity(&self) -> String {
        if !self.contribution_id.is_empty() {
            return format!("{} ({})", self.contribution_id, self.manifest_identity());
        }
        self.manifest_identity()
    }

    fn manifest_identity(&self) -> String {
        if self.manifest.name.is_empty() {
            String::new()
        } else if self.manifest.version.is_empty() {
            self.manifest.name.clone()
        } else {
            format!("{}@{}", self.manifest.name, self.manifest.version)
        }
    }

    /// Go: `func (m *Mapper) Equals(other *Mapper) bool`.
    ///
    /// PORT: Go compares nil-vs-empty slice/Value pairs for distinction that
    /// the port collapses; the remaining comparison is the full content.
    pub fn equals(&self, other: &Mapper) -> bool {
        self.definition.package == other.definition.package
            && self.definition.extensions == other.definition.extensions
            && self.definition.options.is_some() == other.definition.options.is_some()
            && self.manifest.name == other.manifest.name
            && self.manifest.version == other.manifest.version
            && self.manifest.exec == other.manifest.exec
            && self.manifest.compiler_options == other.manifest.compiler_options
            && self.manifest.dynamic_config == other.manifest.dynamic_config
            && self.package_directory == other.package_directory
            && self.contribution_id == other.contribution_id
    }

    /// Go: `func (m *Mapper) TransformIdentity(options *core.CompilerOptions) xxh3.Uint128` —
    /// see the module PORT notes for the hash substitution.
    pub fn transform_identity(&self, options: Option<&CompilerOptions>) -> Uint128 {
        let (declared, _) = self.marshal_declared_options(options);
        let options_json = marshal_declared_to_json(&declared);
        let mut buf = Vec::with_capacity(self.identity().len() + 2 + options_json.len());
        buf.extend_from_slice(self.identity().as_bytes());
        buf.push(0);
        if let Some(options) = &self.definition.options {
            buf.extend_from_slice(options.get().as_bytes());
        }
        buf.push(0);
        buf.extend_from_slice(&options_json);
        fnv1a_128(&buf)
    }

    /// Go: `func (m *Mapper) MarshalDeclaredOptions(options *core.CompilerOptions) (*collections.OrderedMap[string, json.Value], error)`.
    pub fn marshal_declared_options(
        &self,
        options: Option<&CompilerOptions>,
    ) -> (OrderedMap<String, Value>, Option<String>) {
        let mut out = OrderedMap::<String, Value>::with_size_hint(self.manifest.compiler_options.len());
        let Some(options) = options else {
            return (out, None);
        };
        if self.manifest.compiler_options.is_empty() {
            return (out, None);
        }
        for name in &self.manifest.compiler_options {
            let Some(raw) = compiler_option_fields_generated::marshal_compiler_option_field(
                options, name,
            ) else {
                continue;
            };
            out.set(name.clone(), raw);
        }
        (out, None)
    }
}

/// The port's stand-in for `xxh3.Uint128` (see module PORT notes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Uint128 {
    pub hi: u64,
    pub lo: u64,
}

/// 128-bit FNV-1a over `buf` — deterministic, dependency-free.
fn fnv1a_128(buf: &[u8]) -> Uint128 {
    // FNV-1a 128-bit parameters (offset basis / prime).
    const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0b14_264b_ce67_6e6c_6c6c;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET_BASIS;
    for &b in buf {
        hash ^= u128::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    Uint128 {
        hi: (hash >> 64) as u64,
        lo: hash as u64,
    }
}

/// Marshal the declared-options map (ordered) as compact JSON.
fn marshal_declared_to_json(declared: &OrderedMap<String, Value>) -> Vec<u8> {
    use std::fmt::Write;
    let mut out = String::with_capacity(32);
    out.push('{');
    let mut first = true;
    for (key, value) in declared.entries() {
        if !first {
            out.push(',');
        }
        first = false;
        let _ = write!(out, "{}:", serde_json::json!(key));
        out.push_str(value.get());
    }
    out.push('}');
    out.into_bytes()
}
