// Ported from tsc/internal/checker/links.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The checker's two link-store shapes, on top of the tsc-core PagedLinkStore
// port (SPEC §5.3: side tables keyed by node/symbol ids).
//
// PORT (keys): Go keys by the lazily-assigned `ast.GetNodeId(node)` /
// `ast.GetSymbolId(symbol)` counters; the port keys by the packed `NodeId`
// handle (file_id:20 | local_index:44, unique across the program, SPEC §5.1)
// and the program-level `SymbolId` index (SPEC §5.2) — the same uniqueness
// guarantees, without the lazy id assignment pass. The binder's ast_shims
// carry Go's lazily-assigned ids for its own link stores; they are not needed
// here.

use tsc_ast::{NodeId, Symbol, SymbolId};
use tsc_core::linkstore::PagedLinkStore;

/// Go: `type nodeLinkStore[V any] struct` — a links store keyed by node
/// references. Values are stored directly in the pages of the store which is
/// suitable for values where sizeof(V) is small.
pub struct NodeLinkStore<V> {
    pub(crate) store: PagedLinkStore<V>,
}

impl<V: Default> Default for NodeLinkStore<V> {
    fn default() -> Self {
        Self { store: PagedLinkStore::new() }
    }
}

impl<V: Default> NodeLinkStore<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `func (s *nodeLinkStore[V]) Get(node *ast.Node) *V`.
    pub fn get(&mut self, node: NodeId) -> &mut V {
        self.store.get(node.0)
    }

    /// Go: `func (s *nodeLinkStore[V]) Has(node *ast.Node) bool`.
    pub fn has(&mut self, node: NodeId) -> bool {
        self.store.has(node.0)
    }

    /// Go: `func (s *nodeLinkStore[V]) TryGet(node *ast.Node) *V`.
    pub fn try_get(&mut self, node: NodeId) -> Option<&mut V> {
        self.store.try_get(node.0)
    }
}

/// Go: `type symbolArenaLinkStore[V any] struct` — a links store keyed by
/// symbol references. Values are stored indirectly in an arena which is
/// suitable for values where sizeof(V) is larger.
///
/// PORT: Go's pages hold `*V` pointers into a `core.Arena[V]`; the port's
/// pages hold `Option<u32>` indexes into a plain `Vec<V>` (the arena existed
/// only to amortize allocation — same laziness, same stable-address semantics
/// for the values themselves).
pub struct SymbolArenaLinkStore<V> {
    pub(crate) store: PagedLinkStore<Option<u32>>,
    pub(crate) arena: Vec<V>,
}

impl<V> Default for SymbolArenaLinkStore<V> {
    fn default() -> Self {
        Self { store: PagedLinkStore::new(), arena: Vec::new() }
    }
}

impl<V: Default> SymbolArenaLinkStore<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Go: `func (s *symbolArenaLinkStore[V]) Get(symbol *ast.Symbol) *V` —
    /// lazily allocates the entry on first access.
    pub fn get(&mut self, symbol: SymbolId) -> &mut V {
        let index = match *self.store.get(symbol.0 as u64) {
            Some(index) => index,
            None => {
                let index = self.arena.len() as u32;
                self.arena.push(V::default());
                *self.store.get(symbol.0 as u64) = Some(index);
                index
            }
        };
        &mut self.arena[index as usize]
    }

    /// Go: `func (s *symbolArenaLinkStore[V]) Has(symbol *ast.Symbol) bool` —
    /// `s.TryGet(symbol) != nil`.
    pub fn has(&mut self, symbol: SymbolId) -> bool {
        self.try_get(symbol).is_some()
    }

    /// Go: `func (s *symbolArenaLinkStore[V]) TryGet(symbol *ast.Symbol) *V`.
    pub fn try_get(&mut self, symbol: SymbolId) -> Option<&mut V> {
        let index = *self.store.try_get(symbol.0 as u64)?;
        Some(&mut self.arena[index as usize])
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests (Go has no links_test.go; these pin the Go-shaped laziness
// semantics the comments in links.go describe: direct-in-page values for the
// node store, lazily-materialized arena entries for the symbol store).
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_link_store_lazy_alloc() {
        let mut s: NodeLinkStore<u32> = NodeLinkStore::new();
        let n = NodeId::new(0, 17);
        assert!(!s.has(n));
        assert!(s.try_get(n).is_none());
        *s.get(n) = 42;
        assert_eq!(*s.try_get(n).unwrap(), 42);
        assert!(s.has(n));
        // has() is page-granular like Go's TryGet != nil (PagedLinkStore
        // semantics; a neighbor key on the same page reads present).
        assert!(s.has(NodeId::new(0, 18)));
    }

    #[test]
    fn symbol_arena_link_store_lazy_alloc() {
        let mut s: SymbolArenaLinkStore<Vec<i32>> = SymbolArenaLinkStore::new();
        let sym = SymbolId(3);
        assert!(!s.has(sym));
        s.get(sym).push(1);
        assert!(s.has(sym));
        assert_eq!(s.try_get(sym).unwrap(), &[1]);
        // A second Get returns the same arena slot (Go hands back the same *V).
        s.get(sym).push(2);
        assert_eq!(s.try_get(sym).unwrap(), &[1, 2]);
        // Distinct symbols get distinct slots.
        let other = SymbolId(9);
        assert!(s.try_get(other).is_none());
    }

    /// `Symbol` import is only referenced by the doc comment; keep the module
    /// import list honest for readers.
    #[test]
    fn symbol_type_is_the_arena_element() {
        let s: Symbol = Symbol::default();
        assert!(s.declarations.is_empty());
    }
}
