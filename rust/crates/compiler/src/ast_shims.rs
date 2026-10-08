// Crate-local port of the ast-utilities the compiler layer needs.
//
// Sources (Go):
//   tsc/internal/ast/utilities.go — GetImpliedNodeFormatForFile,
//     GetEmitModuleFormatOfFileWorker, GetImpliedNodeFormatForEmitWorker,
//     GetJSXImplicitImportBase, GetJSXRuntimeImport, GetPragmaFromSourceFile,
//     GetPragmaArgument, IsRequireCall, IsCheckJSEnabledForFile,
//     IsPlainJSFile, IsExclusivelyTypeOnlyImportOrExport, IsSourcePhaseImport,
//     IsSourcePhaseImportCall, IsExternalModule, IsExternalOrCommonJSModule,
//     IsJsonSourceFile, WalkUpParenthesizedExpressions, ShouldTransformImportCall
//   tsc/internal/ast/parseoptions.go — GetExternalModuleIndicatorOptions,
//     isFileForcedToBeModuleByFormat
//   tsc/internal/ast/ast.go — ImportAttributesNode.GetResolutionModeOverride
//   tsc/internal/core/core.go — GetScriptKindFromFileName,
//     EnsureScriptKindFromFileName
//
// PORT: these live crate-local following the binder crate's ast_shims.rs
// precedent — the Go functions live in the ast package but the Rust ast
// crate (M2 wave) does not yet export them. They are dedup candidates: when
// the ast crate grows them, delete the copies here (PORTING-NOTES).

use tsc_ast::{
    Kind, NodeData, NodeId, NodeStore, Pragma, SourceFile, SourceFileMetaData,
    node_text,
};
use tsc_core::compileroptions::{
    ModuleKind, ModuleKindCommonJS, ModuleKindESNext, ModuleKindNone, ModuleKindNode16,
    ModuleKindNodeNext, ModuleKindPreserve, ModuleKindES2015, ResolutionMode, ResolutionModeCommonJS,
    ResolutionModeESM, ResolutionModeNone,
};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_tspath::{
    EXTENSION_CJS, EXTENSION_CTS, EXTENSION_DCTS, EXTENSION_DMTS, EXTENSION_DTS, EXTENSION_JS,
    EXTENSION_JSON, EXTENSION_JSX, EXTENSION_MJS, EXTENSION_MTS, EXTENSION_TS, EXTENSION_TSX,
    RootedFilePath,
};

// ────────────────────────────────────────────────────────────────────────────
// core/core.go
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetScriptKindFromFileName(fileName) ScriptKind` (core.go).
pub fn get_script_kind_from_file_name(file_name: &RootedFilePath) -> ScriptKind {
    // Go: `fileName.AnyExtension(nil, tspath.CaseSensitive)` — the longest
    // known extension.
    let extension = file_name.any_extension(&[], tsc_tspath::CaseSensitivity::CaseSensitive);
    if !extension.is_empty() {
        match extension.to_lowercase().as_str() {
            EXTENSION_JS | EXTENSION_CJS | EXTENSION_MJS => return ScriptKind::JS,
            EXTENSION_JSX => return ScriptKind::JSX,
            EXTENSION_TS | EXTENSION_CTS | EXTENSION_MTS => return ScriptKind::TS,
            EXTENSION_TSX => return ScriptKind::TSX,
            EXTENSION_JSON => return ScriptKind::JSON,
            _ => {}
        }
    }
    ScriptKind::Unknown
}

/// Go: `func EnsureScriptKindFromFileName(fileName) ScriptKind` (core.go).
pub fn ensure_script_kind_from_file_name(file_name: &RootedFilePath) -> ScriptKind {
    match get_script_kind_from_file_name(file_name) {
        ScriptKind::Unknown => ScriptKind::TS,
        kind => kind,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ast/utilities.go — implied-node-format helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetImpliedNodeFormatForFile(fileName, packageJsonType) core.ModuleKind`.
pub fn get_implied_node_format_for_file(
    file_name: &RootedFilePath,
    package_json_type: &str,
) -> ModuleKind {
    let mut implied_node_format = ModuleKind(ResolutionModeNone);
    if file_name.extension_is_one_of(&[EXTENSION_DMTS, EXTENSION_MTS, EXTENSION_MJS]) {
        implied_node_format = ModuleKind(ResolutionModeESM);
    } else if file_name.extension_is_one_of(&[EXTENSION_DCTS, EXTENSION_CTS, EXTENSION_CJS]) {
        implied_node_format = ModuleKind(ResolutionModeCommonJS);
    } else if file_name
        .extension_is_one_of(&[EXTENSION_DTS, EXTENSION_TS, EXTENSION_TSX, EXTENSION_JS, EXTENSION_JSX])
    {
        implied_node_format = ModuleKind(if package_json_type == "module" {
            ResolutionModeESM
        } else {
            ResolutionModeCommonJS
        });
    }
    implied_node_format
}

/// Go: `func GetEmitModuleFormatOfFileWorker(fileName, options, sourceFileMetaData) core.ModuleKind`.
pub fn get_emit_module_format_of_file_worker(
    file_name: &RootedFilePath,
    options: &CompilerOptions,
    source_file_meta_data: &SourceFileMetaData,
) -> ModuleKind {
    let result = get_implied_node_format_for_emit_worker(
        file_name,
        options.get_emit_module_kind(),
        source_file_meta_data,
    );
    if result != ResolutionModeNone {
        return ModuleKind(result);
    }
    options.get_emit_module_kind()
}

/// Go: `func GetImpliedNodeFormatForEmitWorker(fileName, emitModuleKind, sourceFileMetaData) core.ResolutionMode`.
pub fn get_implied_node_format_for_emit_worker(
    file_name: &RootedFilePath,
    emit_module_kind: ModuleKind,
    source_file_meta_data: &SourceFileMetaData,
) -> ResolutionMode {
    if ModuleKindNode16 <= emit_module_kind && emit_module_kind <= ModuleKindNodeNext {
        return source_file_meta_data.implied_node_format.0;
    }
    if source_file_meta_data.implied_node_format == ModuleKind(ResolutionModeCommonJS)
        && (source_file_meta_data.package_json_type == "commonjs"
            || file_name.extension_is_one_of(&[EXTENSION_CJS, EXTENSION_CTS]))
    {
        return ResolutionModeCommonJS;
    }
    if source_file_meta_data.implied_node_format == ModuleKind(ResolutionModeESM)
        && (source_file_meta_data.package_json_type == "module"
            || file_name.extension_is_one_of(&[EXTENSION_MJS, EXTENSION_MTS]))
    {
        return ResolutionModeESM;
    }
    ResolutionModeNone
}

// ────────────────────────────────────────────────────────────────────────────
// ast/parseoptions.go — external-module-indicator options
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetExternalModuleIndicatorOptions(fileName, options, metadata) ExternalModuleIndicatorOptions`.
pub fn get_external_module_indicator_options(
    file_name: &RootedFilePath,
    options: &CompilerOptions,
    metadata: &SourceFileMetaData,
) -> tsc_ast::parseoptions::ExternalModuleIndicatorOptions {
    use tsc_core::compileroptions::ModuleDetectionKind;

    if file_name.is_declaration_file() {
        return Default::default();
    }

    match options.get_emit_module_detection_kind() {
        ModuleDetectionKind::Force => {
            // All non-declaration files are modules, declaration files still do
            // the usual isFileProbablyExternalModule.
            tsc_ast::parseoptions::ExternalModuleIndicatorOptions { jsx: false, force: true }
        }
        ModuleDetectionKind::Legacy => Default::default(),
        ModuleDetectionKind::Auto => {
            // If module is nodenext or node16, all esm format files are modules
            // If jsx is react-jsx or react-jsxdev then jsx tags force module-ness
            // otherwise, the presence of import or export statements (or
            // import.meta) implies module-ness
            tsc_ast::parseoptions::ExternalModuleIndicatorOptions {
                jsx: options.jsx
                    == tsc_core::options_generated::JsxEmit::ReactJSX
                    || options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev,
                force: is_file_forced_to_be_module_by_format(file_name, options, metadata),
            }
        }
    }
}

/// Go: `isFileForcedToBeModuleByFormatExtensions`.
const IS_FILE_FORCED_TO_BE_MODULE_BY_FORMAT_EXTENSIONS: &[&str] =
    &[EXTENSION_CJS, EXTENSION_CTS, EXTENSION_MJS, EXTENSION_MTS];

/// Go: `func isFileForcedToBeModuleByFormat(fileName, options, metadata) bool`.
fn is_file_forced_to_be_module_by_format(
    file_name: &RootedFilePath,
    options: &CompilerOptions,
    metadata: &SourceFileMetaData,
) -> bool {
    // Excludes declaration files - they still require an explicit `export {}`
    // or the like for back compat purposes. The only non-declaration files
    // _not_ forced to be a module are `.js` files that aren't esm-mode
    // (meaning not in a `type: module` scope).
    get_implied_node_format_for_emit_worker(file_name, options.get_emit_module_kind(), metadata)
        == ResolutionModeESM
        || file_name
            .extension_is_one_of(IS_FILE_FORCED_TO_BE_MODULE_BY_FORMAT_EXTENSIONS)
}

// ────────────────────────────────────────────────────────────────────────────
// ast/utilities.go — module-ness
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsExternalModule(file *SourceFile) bool`.
pub fn is_external_module(file: &SourceFile) -> bool {
    file.payload().external_module_indicator.get().is_some()
}

/// Go: `func IsExternalOrCommonJSModule(file *SourceFile) bool`.
pub fn is_external_or_common_js_module(file: &SourceFile) -> bool {
    file.payload().external_module_indicator.get().is_some()
        || file.payload().common_js_module_indicator.get().is_some()
}

/// Go: `func IsJsonSourceFile(file *SourceFile) bool`.
pub fn is_json_source_file(file: &SourceFile) -> bool {
    file.payload().script_kind == ScriptKind::JSON
}

// ────────────────────────────────────────────────────────────────────────────
// ast/utilities.go — JSX imports
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func GetJSXImplicitImportBase(compilerOptions, file) string`.
pub fn get_jsx_implicit_import_base(
    compiler_options: &CompilerOptions,
    file: &SourceFile,
) -> String {
    let jsx_import_source_pragma = get_pragma_from_source_file(file, "jsximportsource");
    let jsx_runtime_pragma = get_pragma_from_source_file(file, "jsxruntime");
    if get_pragma_argument(jsx_runtime_pragma.as_ref(), "factory") == "classic" {
        return String::new();
    }
    if compiler_options.jsx == tsc_core::options_generated::JsxEmit::ReactJSX
        || compiler_options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev
        || !compiler_options.jsx_import_source.is_empty()
        || jsx_import_source_pragma.is_some()
        || get_pragma_argument(jsx_runtime_pragma.as_ref(), "factory") == "automatic"
    {
        let mut result = get_pragma_argument(jsx_import_source_pragma.as_ref(), "factory");
        if result.is_empty() {
            result = compiler_options.jsx_import_source.clone();
        }
        if result.is_empty() {
            result = "react".to_string();
        }
        return result;
    }
    String::new()
}

/// Go: `func GetJSXRuntimeImport(base string, options *core.CompilerOptions) string`.
pub fn get_jsx_runtime_import(base: &str, options: &CompilerOptions) -> String {
    if base.is_empty() {
        return base.to_string();
    }
    format!(
        "{base}/{}",
        if options.jsx == tsc_core::options_generated::JsxEmit::ReactJSXDev {
            "jsx-dev-runtime"
        } else {
            "jsx-runtime"
        }
    )
}

/// Go: `func GetPragmaFromSourceFile(file *SourceFile, name string) *Pragma`.
fn get_pragma_from_source_file(file: &SourceFile, name: &str) -> Option<Pragma> {
    let mut result = None;
    // Last one wins.
    for pragma in &file.payload().pragmas {
        if pragma.name == name {
            result = Some(pragma.clone());
        }
    }
    result
}

/// Go: `func GetPragmaArgument(pragma *Pragma, name string) string`.
fn get_pragma_argument(pragma: Option<&Pragma>, name: &str) -> String {
    if let Some(pragma) = pragma {
        if let Some(arg) = pragma.args.get(name) {
            return arg.value.clone();
        }
    }
    String::new()
}

// ────────────────────────────────────────────────────────────────────────────
// ast/utilities.go — resolution-mode helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func IsRequireCall(node *Node, requireStringLiteralLikeArgument bool) bool`.
pub fn is_require_call(
    store: &dyn NodeStore,
    node: NodeId,
    require_string_literal_like_argument: bool,
) -> bool {
    let n = store.node(node);
    if n.kind != Kind::CallExpression {
        return false;
    }
    let call = match &n.data {
        NodeData::CallExpression(call) => call,
        _ => return false,
    };
    let expression = store.node(call.expression);
    if expression.kind != Kind::Identifier || node_text(store, call.expression) != "require" {
        return false;
    }
    let arguments = match &call.arguments {
        Some(list) => list.nodes.as_ref(),
        None => return false,
    };
    if arguments.len() != 1 {
        return false;
    }
    !require_string_literal_like_argument || is_string_literal_like(store.node(arguments[0]))
}

/// Go: `func IsStringLiteralLike(node *Node) bool` (ast).
fn is_string_literal_like(node: &tsc_ast::Node) -> bool {
    matches!(
        node.kind,
        Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral | Kind::TemplateHead
    )
}

/// Go: `func WalkUpParenthesizedExpressions(node *Expression) *Node`.
pub fn walk_up_parenthesized_expressions(store: &dyn NodeStore, node: NodeId) -> NodeId {
    let mut node = node;
    loop {
        let parent = store.node(node).parent.get();
        if parent != NodeId::NONE && store.node(node).kind == Kind::ParenthesizedExpression {
            node = parent;
        } else {
            return node;
        }
    }
}

/// Go: `func ShouldTransformImportCall(fileName, options, impliedNodeFormatForEmit) bool`.
pub fn should_transform_import_call(
    _file_name: &RootedFilePath,
    options: &CompilerOptions,
    implied_node_format_for_emit: ModuleKind,
) -> bool {
    let module_kind = options.get_emit_module_kind();
    if (ModuleKindNode16 <= module_kind && module_kind <= ModuleKindNodeNext)
        || module_kind == ModuleKindPreserve
    {
        return false;
    }
    implied_node_format_for_emit < ModuleKindES2015
}

/// Go: `func IsCheckJSEnabledForFile(sourceFile, compilerOptions) bool`.
pub fn is_check_js_enabled_for_file(
    source_file: &SourceFile,
    compiler_options: &CompilerOptions,
) -> bool {
    if let Some(directive) = &source_file.payload().check_js_directive {
        return directive.enabled;
    }
    compiler_options.check_js == Tristate::TSTrue
}

/// Go: `func IsPlainJSFile(file *SourceFile, checkJs core.Tristate) bool`.
pub fn is_plain_js_file(file: &SourceFile, check_js: Tristate) -> bool {
    let payload = file.payload();
    (payload.script_kind == ScriptKind::JS || payload.script_kind == ScriptKind::JSX)
        && payload.check_js_directive.is_none()
        && check_js == Tristate::TSUnknown
}

/// Go: `func IsExclusivelyTypeOnlyImportOrExport(node *Node) bool` — the
/// `ImportClause.IsTypeOnly()` arm is `PhaseModifier == KindTypeKeyword`
/// (ast.go: `func (n *Node) IsTypeOnly() bool`).
pub fn is_exclusively_type_only_import_or_export(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::ExportDeclaration => {
            matches!(&n.data, NodeData::ExportDeclaration(d) if d.is_type_only)
        }
        Kind::ImportDeclaration | Kind::JSImportDeclaration => {
            if let NodeData::ImportDeclaration(d) = &n.data {
                if let Some(clause) = d.import_clause {
                    // Go: `importClause.AsImportClause().IsTypeOnly()`.
                    return matches!(
                        &store.node(clause).data,
                        NodeData::ImportClause(c) if c.phase_modifier == Some(Kind::TypeKeyword)
                    );
                }
            }
            false
        }
        Kind::JSDocImportTag => {
            if let NodeData::JSDocImportTag(d) = &n.data {
                if let Some(clause) = d.import_clause {
                    return matches!(
                        &store.node(clause).data,
                        NodeData::ImportClause(c) if c.phase_modifier == Some(Kind::TypeKeyword)
                    );
                }
            }
            false
        }
        _ => false,
    }
}

/// Go: `func IsSourcePhaseImport(node *Node) bool`.
pub fn is_source_phase_import(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind == Kind::ImportDeclaration {
        if let NodeData::ImportDeclaration(d) = &n.data {
            if let Some(clause) = d.import_clause {
                return matches!(
                    &store.node(clause).data,
                    NodeData::ImportClause(c) if c.phase_modifier == Some(Kind::SourceKeyword)
                );
            }
        }
        return false;
    }
    is_source_phase_import_call(store, node)
}

/// Go: `func IsSourcePhaseImportCall(node *Node) bool`.
pub fn is_source_phase_import_call(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    if n.kind != Kind::CallExpression {
        return false;
    }
    if let NodeData::CallExpression(call) = &n.data {
        return is_import_source_meta_property(store, call.expression);
    }
    false
}

/// Go: `func IsImportSourceMetaProperty(node *Node) bool` —
/// `isImportMetaProperty(node, "source")`.
fn is_import_source_meta_property(store: &dyn NodeStore, node: NodeId) -> bool {
    is_import_meta_property(store, node, "source")
}

/// Go: `func isImportMetaProperty(node *Node, name string) bool`.
fn is_import_meta_property(store: &dyn NodeStore, node: NodeId, name: &str) -> bool {
    let n = store.node(node);
    if n.kind != Kind::MetaProperty {
        return false;
    }
    if let NodeData::MetaProperty(meta) = &n.data {
        return meta.keyword_token == Kind::ImportKeyword && node_text(store, meta.name) == name;
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// ast/ast.go — ImportAttributesNode.GetResolutionModeOverride
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func (node *ImportAttributesNode) GetResolutionModeOverride(grammarErrorOnNode) (core.ResolutionMode, bool)`.
///
/// PORT: `grammarErrorOnNode` is dropped (the only compiler caller passes nil;
/// the grammar-error arm is unreachable from this crate — PORTING-NOTES).
pub fn get_resolution_mode_override(store: &dyn NodeStore, node: NodeId) -> (ResolutionMode, bool) {
    if node == NodeId::NONE {
        return (ResolutionModeNone, false);
    }
    let n = store.node(node);
    let attributes = match &n.data {
        NodeData::ImportAttributes(attributes) => match &attributes.attributes {
            Some(list) => list.nodes.as_ref(),
            None => return (ResolutionModeNone, false),
        },
        _ => return (ResolutionModeNone, false),
    };
    let attribute = attributes.iter().copied().find(|&attribute| {
        store.node(attribute).kind == Kind::ImportAttribute
            && matches!(&store.node(attribute).data, NodeData::ImportAttribute(a) if node_text(store, a.name) == "resolution-mode")
    });
    let attribute = match attribute {
        Some(attribute) => attribute,
        None => return (ResolutionModeNone, false),
    };
    let value = match &store.node(attribute).data {
        NodeData::ImportAttribute(a) => a.value,
        _ => return (ResolutionModeNone, false),
    };
    if !is_string_literal_like(store.node(value)) {
        return (ResolutionModeNone, false);
    }
    let text = node_text(store, value);
    if text != "import" && text != "require" {
        // Go: grammarErrorOnNode(elem.Value, X_resolution_mode_should_be...)
        // — unreachable from this crate's callers (all pass nil).
        return (ResolutionModeNone, false);
    }
    if text == "import" {
        (ResolutionModeESM, true)
    } else {
        (ResolutionModeCommonJS, true)
    }
}

/// Go: `func NodeIsSynthesized(node *Node) bool` —
/// `PositionIsSynthesized(node.Loc.Pos()) || PositionIsSynthesized(node.Loc.End())`.
pub fn node_is_synthesized(node: &tsc_ast::Node) -> bool {
    is_synthesized_position(node.loc.pos()) || is_synthesized_position(node.loc.end())
}

/// Go: `func PositionIsSynthesized(pos int) bool` — `pos < 0`.
fn is_synthesized_position(pos: tsc_core::text::TextPos) -> bool {
    pos < 0
}
