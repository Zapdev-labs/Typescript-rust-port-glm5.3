// Ported from tsc/internal/ast/parseoptions.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   SourceFileParseOptions            → SourceFileParseOptions
//   ExternalModuleIndicatorOptions    → ExternalModuleIndicatorOptions
//   SetExternalModuleIndicator        → set_external_module_indicator
//   getExternalModuleIndicator        → get_external_module_indicator
//   isFileProbablyExternalModule      → is_file_probably_external_module
//   isAnExternalModuleIndicatorNode   → is_an_external_module_indicator_node
//   getImportMetaIfNecessary          → get_import_meta_if_necessary
//   isFileModuleFromUsingJSXTag       → is_file_module_from_using_jsx_tag
//   walkTreeForJSXTags                 → walk_tree_for_jsx_tags
//   findChildNode                     → crate::utilities::find_child_node
//   IsImportMeta                      → crate::utilities::is_import_meta
//
// DEFERRED: `GetExternalModuleIndicatorOptions` / `isFileForcedToBeModuleByFormat`
// depend on core.CompilerOptions + ModuleDetectionKind (core/options.go is a
// later porting task); the parser task passes an already-populated
// ExternalModuleIndicatorOptions instead.

use tsc_core::scriptkind::ScriptKind;
use tsc_tspath::RootedFilePath;

use crate::{Kind, Node, NodeFlags, NodeId};
use crate::modifierflags::has_syntactic_modifier;
use crate::{ModifierFlags, NodeStore};

#[derive(Debug, Default, Clone)]
pub struct SourceFileParseOptions {
    pub file_name: RootedFilePath,
    pub path_key: tsc_tspath::PathKey,
    pub external_module_indicator_options: ExternalModuleIndicatorOptions,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExternalModuleIndicatorOptions {
    pub jsx: bool,
    pub force: bool,
}

/// Go: `func SetExternalModuleIndicator(file *SourceFile, opts ExternalModuleIndicatorOptions)`.
pub fn set_external_module_indicator(store: &mut dyn NodeStore, file: NodeId, opts: ExternalModuleIndicatorOptions) {
    let node = get_external_module_indicator(store, file, opts);
    let file_data = store.node(file).as_source_file().expect("SourceFile data for a SourceFile node");
    // Go: file.ExternalModuleIndicator = node — the field is a Cell on the
    // payload (SPEC §5.2 Cell-fields); reach through the mutable accessor.
    store.node_mut(file)
        .as_source_file_mut()
        .expect("SourceFile data")
        .external_module_indicator
        .set(node.unwrap_or(NodeId::NONE));
}

/// Go: `func getExternalModuleIndicator(file *SourceFile, opts ExternalModuleIndicatorOptions) *Node`.
fn get_external_module_indicator(
    store: &mut dyn NodeStore,
    file: NodeId,
    opts: ExternalModuleIndicatorOptions,
) -> Option<NodeId> {
    if store.node(file).as_source_file().expect("SourceFile data").script_kind == ScriptKind::JSON {
        return None;
    }
    if let Some(node) = is_file_probably_external_module(store, file) {
        return Some(node);
    }
    if store.node(file).as_source_file().expect("SourceFile data").is_declaration_file {
        return None;
    }
    if opts.jsx {
        if let Some(node) = is_file_module_from_using_jsx_tag(store, file) {
            return Some(node);
        }
    }
    if opts.force {
        // Go: file.AsNode()
        return Some(file);
    }
    None
}

/// Go: `func isFileProbablyExternalModule(sourceFile *SourceFile) *Node`.
fn is_file_probably_external_module(store: &mut dyn NodeStore, file: NodeId) -> Option<NodeId> {
    let statements = store
        .node(file)
        .as_source_file()
        .expect("SourceFile data")
        .statements
        .clone();
    if let Some(list) = &statements {
        for &statement in &list.nodes {
            if is_an_external_module_indicator_node(store, statement) {
                return Some(statement);
            }
        }
    }
    get_import_meta_if_necessary(store, file)
}

/// Go: `func isAnExternalModuleIndicatorNode(node *Node) bool`.
fn is_an_external_module_indicator_node(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    has_syntactic_modifier(n, ModifierFlags::EXPORT)
        || (crate::is_import_equals_declaration(n)
            && crate::is_external_module_reference(
                n.as_import_equals_declaration()
                    .expect("ImportEqualsDeclaration data")
                    .module_reference
                    .expect("ModuleReference is never nil in bound nodes"),
            ))
        || crate::is_import_declaration(n)
        || crate::is_export_assignment(n)
        || crate::is_export_declaration(n)
}

/// Go: `func getImportMetaIfNecessary(sourceFile *SourceFile) *Node`.
fn get_import_meta_if_necessary(store: &mut dyn NodeStore, file: NodeId) -> Option<NodeId> {
    if store.node(file).flags.intersects(NodeFlags::POSSIBLY_CONTAINS_IMPORT_META) {
        let store: &dyn NodeStore = &*store;
        return crate::utilities::find_child_node(store, file, &mut |id| {
            crate::utilities::is_import_meta(store, id)
        });
    }
    None
}

/// Go: `func isFileModuleFromUsingJSXTag(file *SourceFile) *Node`.
fn is_file_module_from_using_jsx_tag(store: &mut dyn NodeStore, file: NodeId) -> Option<NodeId> {
    walk_tree_for_jsx_tags(store, file)
}

/// Go: `func walkTreeForJSXTags(node *Node) *Node` — "somewhat unavoidable full
/// tree walk to locate a JSX tag".
///
/// PORT: Go short-circuits subtrees without `SubtreeContainsJsx` facts; the
/// facts subsystem (subtreefacts.go) is deferred, so the port walks the whole
/// tree (identical results, no early-out). TODO(porting) restore the
/// SubtreeFacts gate with the subtreefacts.go port.
fn walk_tree_for_jsx_tags(store: &mut dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let n = store.node(node);
    if crate::utilities::is_jsx_opening_like_element(n) || crate::utilities::is_jsx_fragment(n) {
        return Some(node);
    }
    let mut found = None;
    n.for_each_child(&mut |child| {
        if let Some(hit) = walk_tree_for_jsx_tags(store, child) {
            found = Some(hit);
            return true;
        }
        false
    });
    found
}

/// Go: `func (file *SourceFile) IsDefaultLibrary` and friends — declaration/
/// library classification lives on the SourceFile payload (source_file.rs).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_module_indicator_options_default() {
        let opts = ExternalModuleIndicatorOptions::default();
        assert!(!opts.jsx);
        assert!(!opts.force);
    }
}
