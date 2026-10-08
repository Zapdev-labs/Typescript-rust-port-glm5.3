// Ported from tsc/internal/core/nodemodules.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go's `map[string]bool` literals become source-ordered static slices
// (Go map iteration order is randomized, so order was never observable), and
// the merged `NodeCoreModules` map becomes a lazily-built `SyncSet`
// (`sync.OnceValue` → `OnceLock`).

use std::sync::OnceLock;

use tsc_collections::SyncSet;

// require('module').builtinModules.filter(x => !x.match(/^(?:_|node:)/))
pub static UNPREFIXED_NODE_CORE_MODULES: &[&str] = &[
    "assert",
    "assert/strict",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "dns/promises",
    "domain",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "inspector",
    "inspector/promises",
    "module",
    "net",
    "os",
    "path",
    "path/posix",
    "path/win32",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "readline/promises",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "util/types",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

// require('module').builtinModules.filter(x => x.startsWith('node:'))
pub static EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES: &[&str] =
    &["node:quic", "node:sea", "node:sqlite", "node:test", "node:test/reporters"];

// PORT: Go's `NodeCoreModules()` is a `sync.OnceValue`-computed
// map[string]bool holding both unprefixed and "node:"-prefixed keys plus the
// exclusively-prefixed set; the port computes the equivalent SyncSet once
// (keyed by String — the "node:"-prefixed entries are dynamic).
pub fn node_core_modules() -> &'static SyncSet<String> {
    static NODE_CORE_MODULES: OnceLock<SyncSet<String>> = OnceLock::new();
    NODE_CORE_MODULES.get_or_init(|| {
        let node_core_modules = SyncSet::new();
        for unprefixed in UNPREFIXED_NODE_CORE_MODULES {
            node_core_modules.add((*unprefixed).to_owned());
            node_core_modules.add(format!("node:{unprefixed}"));
        }
        for exclusively_prefixed in EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES {
            node_core_modules.add((*exclusively_prefixed).to_owned());
        }
        node_core_modules
    })
}

pub fn non_relative_module_name_for_typing_cache(module_name: &str) -> &str {
    if node_core_modules().has(module_name) {
        return "node";
    }
    module_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_modules_contain_unprefixed_and_prefixed() {
        assert_eq!(UNPREFIXED_NODE_CORE_MODULES.len(), 54);
        assert_eq!(EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES.len(), 5);
        let m = node_core_modules();
        assert!(m.has("fs"));
        assert!(m.has("node:fs"));
        assert!(m.has("node:test"));
        assert!(!m.has("not-a-module"));
    }

    #[test]
    fn non_relative_module_name_for_typing_cache_maps() {
        assert_eq!(non_relative_module_name_for_typing_cache("fs"), "node");
        assert_eq!(non_relative_module_name_for_typing_cache("node:fs"), "node");
        assert_eq!(
            non_relative_module_name_for_typing_cache("node:test"),
            "node"
        );
        assert_eq!(
            non_relative_module_name_for_typing_cache("some/package"),
            "some/package"
        );
    }
}
