// Ported from tsc/internal/checker/checker.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// M6 S6 — SYMBOL/MODULE/CALL RESOLUTION (Go checker.go 8614–10258 +
// 14472–16750 + the resolver-hook block 1510–2232 + getEffectiveCallArguments
// 30591). One-.rs-per-region split per the M6-DISPATCH slice table:
//
//   Section A — resolveName: the binder NameResolver walk re-ported onto
//     Checker (see the PORT note on resolve_name_worker) + the
//     onFailedToResolveSymbol/onSuccessfullyResolvedSymbol hook family
//     (checker.go 1558–2232: the block-scope-use checks, the
//     type/namespace misuse ladders, the spelling-suggestion probes).
//   Section B — the alias/module family: getMergedSymbol/recordMergedSymbol,
//     getLateBoundSymbol, resolveSymbol(Ex), resolveAlias +
//     getTargetOfAliasDeclaration and every getTargetOf* arm, the
//     import/export specifier resolution, resolveEntityName /
//     resolveQualifiedName, getExportsOfModule(+Worker)/lateBind*,
//     resolveExternalModuleName(Worker)/resolveExternalModule,
//     resolveExternalModuleSymbol/resolveESModuleSymbol, getSymbolFlags(Ex).
//   Section C (resolvecall.rs) — getResolvedSignature/resolveCallExpression/
//     resolveNewExpression/resolveCall/chooseOverload and the
//     CallSignature/constructSignature error family.
//
// Stub directions this wave leaves in place (per the dispatch table):
//   - S7: getUnionType/getIntersectionType/createArrayType (the
//     overload-failure signature union), instantiateSignature (identity)
//   - S8: resolveStructuredTypeMembers — getSignaturesOfType reads through
//     the empty-members stub, so typed calls compose Go's
//     no-signatures paths until S8 lands; getReturnTypeOfSignature
//   - S9: getContextualType (resolveCall's contextual return inference),
//     getAwaitedTypeOfPromise (maybeAddMissingAwaitInfo stays dormant)
//   - W2 remainder: isSignatureApplicable's spread handling through
//     getSpreadArgumentType
//   - jsx wave: resolveJsxOpeningLikeElement, inferJsxTypeArguments
//   - the program/module layer: resolveExternalModule's file-resolution
//     half (program.GetResolvedModule/GetModeForUsageLocation are not on
//     the port's Program trait yet — the ambient-module half is real)
//
// PORT NOTE (resolve_name_worker): Go wires the checker's hooks into
// `binder.NameResolver` (checker.go createNameResolver, 1510). The ported
// `tsc_binder::NameResolver` struct cannot be instantiated against
// `&mut Checker`: its `store: &dyn NodeStore`, `symbols: &[Symbol]` and
// `globals: &SymbolTable` shared borrows alias the same Checker the
// `FnMut` hooks must borrow mutably (Go's closure fields have no such
// constraint), and the symbol arena is a `Vec<Symbol>` on Program — a
// `&[Symbol]` slice is invalidated when a hook (late binding,
// combineValueAndTypeSymbols) allocates. So the Resolve walk is re-ported
// here as a Checker method with the hooks as direct method calls; the
// walk itself is line-for-line the binder port (rust/crates/binder/src/
// nameresolver.rs, itself the port of tsc/internal/binder/nameresolver.go).

use tsc_ast::{
    has_syntactic_modifier, is_identifier, is_module_declaration, is_source_file, Kind,
    ModifierFlags, Node, NodeFlags as NF, NodeId, NodeStore, Symbol, SymbolFlags, SymbolId,
    SymbolTable, INTERNAL_SYMBOL_NAME_DEFAULT, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
    INTERNAL_SYMBOL_NAME_EXPORT_STAR,
};
use tsc_core::compileroptions::{
    ResolutionMode, ResolutionModeCommonJS, ResolutionModeESM, ResolutionModeNone,
};
// FCS tranche 6: the module-not-found ladder's mode derivation types.
use tsc_core::options_generated::{CompilerOptions, JsxEmit, ModuleKind, ModuleResolutionKind};
use tsc_core::tristate::Tristate;
use tsc_diagnostics as diag;
use tsc_diagnostics::Message;

use rustc_hash::{FxHashMap, FxHashSet};

use tsc_binder::ast_shims as binder_shims;
use tsc_binder::node_data as binder_node_data;

use crate::checker::{TypeSystemEntity, TypeSystemPropertyName};
use crate::types::MembersOrExportsResolutionKind as MembersOrExports;
use crate::types::{TypeFlags, TypeId};
use crate::utilities as u;
use crate::Checker;

/// Go: the `primitiveTypeAliasSuggestions` sync.OnceValue table body
/// (checker.go:1795) — (primitive, builtin) pairs; the transient
/// type-alias suggestion symbols are materialized into the checker's
/// `primitive_type_alias_suggestion_symbols` slots on first use.
const PRIMITIVE_TYPE_ALIASES: [(&str, &str); 6] = [
    ("string", "String"),
    ("number", "Number"),
    ("boolean", "Boolean"),
    ("object", "Object"),
    ("bigint", "BigInt"),
    ("symbol", "Symbol"),
];

impl Checker {
    // ────────────────────────────────────────────────────────────────────────
    // Section A — resolveName (Go checker.go:976 + binder nameresolver.go:25)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `c.resolveName` (the checker.go:660 closure field —
    /// `createNameResolver().Resolve`). Full body in resolution.rs per the
    /// module PORT note.
    #[allow(clippy::too_many_arguments)]
    /// Go: `createNameResolver().Resolve` — the normal configuration of the
    /// shared walk (exact `getSymbol` lookups, all hooks).
    pub(crate) fn resolve_name_worker(
        &mut self,
        location: Option<NodeId>,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> Option<SymbolId> {
        self.resolve_name_worker_impl(
            location,
            name,
            meaning,
            name_not_found_message,
            is_use,
            exclude_globals,
            false, /*suggestion*/
        )
    }

    /// Go: `c.resolveNameForSymbolSuggestion` (checker.go:660, set at 977 to
    /// `c.createNameResolverForSuggestion().Resolve`) — the same scope walk
    /// with `getSuggestionForSymbolNameLookup` as the Lookup hook and no
    /// success/failure hooks (unreachable there: `nameNotFoundMessage` is
    /// always nil). `isUse` is false, so no symbol-reference recording.
    fn resolve_name_for_symbol_suggestion(
        &mut self,
        location: Option<NodeId>,
        name: &str,
        meaning: SymbolFlags,
    ) -> Option<SymbolId> {
        self.resolve_name_worker_impl(
            location,
            name,
            meaning,
            None, /*name_not_found_message*/
            false, /*is_use*/
            false, /*exclude_globals*/
            true,  /*suggestion*/
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_name_worker_impl(
        &mut self,
        location: Option<NodeId>,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
        suggestion: bool,
    ) -> Option<SymbolId> {
        let mut location = location;
        let mut result: Option<SymbolId> = None;
        let mut last_location: Option<NodeId> = None;
        let mut last_self_reference_location: Option<NodeId> = None;
        let mut property_with_invalid_initializer: Option<NodeId> = None;
        let mut associated_declaration_for_containing_initializer_or_binding_name: Option<NodeId> =
            None;
        let mut within_deferred_context = false;
        // needed for did-you-mean error reporting, which gathers candidates starting from the original location
        let original_location = location;
        let name_is_const = name == "const";
        'outer: while let Some(loc) = location {
            if name_is_const && binder_shims::is_const_assertion(self.store(), loc) {
                // `const` in an `as const` has no symbol, but issues no error because there is no *actual* lookup of the type
                // (it refers to the constant type of the expression instead)
                return None;
            }
            if binder_shims::is_module_or_enum_declaration(self.store().node(loc))
                && last_location.is_some()
                && self.store().node(loc).name() == last_location
            {
                // If lastLocation is the name of a namespace or enum, skip the parent since it will have is own locals that could
                // conflict.
                last_location = Some(loc);
                location = binder_shims::parent_of(self.store(), loc);
                continue 'outer;
            }
            let is_module_attributes = is_module_declaration(self.store().node(loc))
                && last_location.is_some_and(|l| {
                    (match &self.store().node(loc).data {
                        tsc_ast::NodeData::ModuleDeclaration(d) => d.attributes,
                        _ => None,
                    }) == Some(l)
                });
            // Locals of a source file are not in scope (because they get merged into the global symbol table)
            if let Some(locals) = binder_node_data::locals_of(self.store(), loc)
                .cloned()
                .filter(|_| !binder_shims::is_global_source_file(self.store(), loc))
            {
                result = self.name_lookup(&locals, name, meaning, suggestion);
                if result.is_some() {
                    let mut use_result = true;
                    let result_flags = self.program_symbol(result.unwrap()).flags;
                    if is_module_attributes {
                        use_result = false;
                    } else if binder_shims::is_function_like(self.store().node(loc))
                        && last_location.is_some()
                        && last_location != binder_node_data::body_of(self.store(), loc)
                    {
                        // symbol lookup restrictions for function-like declarations
                        // - Type parameters of a function are in scope in the entire function declaration, including the parameter
                        //   list and return type. However, local types are only in scope in the function body.
                        // - parameters are only in the scope of function body
                        // This restriction does not apply to JSDoc comment types because they are parented
                        // at a higher level than type parameters would normally be
                        if meaning.intersects(result_flags) && result_flags.intersects(SymbolFlags::TYPE) {
                            // type parameters are visible in parameter list, return type and type parameter list.
                            // Synthetic fake scopes are added for signatures so type parameters are accessible from them.
                            use_result = result_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                                && (last_location.is_some_and(|l| {
                                    self.store().node(l).flags.intersects(NF::SYNTHESIZED)
                                })
                                    || last_location == binder_node_data::type_of(self.store(), loc)
                                    || last_location.is_some_and(|l| self.store().node(l).kind == Kind::Parameter)
                                    || last_location.is_some_and(|l| self.store().node(l).kind == Kind::JSDocParameterTag)
                                    || last_location.is_some_and(|l| self.store().node(l).kind == Kind::JSDocReturnTag)
                                    || last_location.is_some_and(|l| self.store().node(l).kind == Kind::TypeParameter));
                        }
                        if meaning.intersects(result_flags) && result_flags.intersects(SymbolFlags::VARIABLE) {
                            // expression inside parameter will lookup as normal variable scope when targeting es2015+
                            if self.use_outer_variable_scope_in_parameter(result.unwrap(), loc, last_location) {
                                use_result = false;
                            } else if result_flags.intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE) {
                                // parameters are visible only inside function body, parameter list and return type
                                // technically for parameter list case here we might mix parameters and variables declared in function,
                                // however, it is detected separately when checking initializers of parameters
                                // to make sure that they reference no variables declared after them.
                                let found_in_parameter = last_location.is_some_and(|l| {
                                    self.store().node(l).kind == Kind::Parameter
                                        || self.store().node(l).flags.intersects(NF::SYNTHESIZED)
                                });
                                let found_through_type = last_location
                                    == binder_node_data::type_of(self.store(), loc)
                                    && result
                                        .and_then(|r| self.program_symbol(r).value_declaration)
                                        .is_some_and(|vd| {
                                            binder_shims::find_ancestor(self.store(), vd, &mut |n| {
                                                tsc_ast::is_parameter_declaration(self.store().node(n))
                                            })
                                            .is_some()
                                        });
                                use_result = found_in_parameter || found_through_type;
                            }
                        }
                    } else if self.store().node(loc).kind == Kind::ConditionalType {
                        // A type parameter declared using 'infer T' in a conditional type is visible only in
                        // the true branch of the conditional type.
                        use_result = match &self.store().node(loc).data {
                            tsc_ast::NodeData::ConditionalTypeNode(d) => Some(d.true_type) == last_location,
                            _ => false,
                        };
                    }
                    if use_result {
                        break 'outer;
                    }
                    result = None;
                }
            }
            within_deferred_context =
                within_deferred_context || get_is_deferred_context(self.store(), loc, last_location);
            match self.store().node(loc).kind {
                Kind::SourceFile => {
                    let file = self.store().file(loc.file_id());
                    if binder_shims::is_external_or_common_js_module(file) {
                        // Go: fallthrough into the ModuleDeclaration case.
                        self.resolve_module_case(loc, name, meaning, is_module_attributes, &mut result, suggestion);
                        if result.is_some() {
                            break 'outer;
                        }
                    }
                }
                Kind::ModuleDeclaration => {
                    self.resolve_module_case(loc, name, meaning, is_module_attributes, &mut result, suggestion);
                    if result.is_some() {
                        break 'outer;
                    }
                }
                Kind::EnumDeclaration => {
                    // Go: `if enumSymbol == nil { break }` — no enum symbol
                    // means skip the arm, not the loop.
                    if let Some(enum_symbol) = self.get_symbol_of_declaration_merged(loc) {
                        let exports = self.program_symbol(enum_symbol).exports.clone();
                        result = self.name_lookup(&exports, name, meaning & SymbolFlags::ENUM_MEMBER, suggestion);
                        if result.is_some() {
                            if name_not_found_message.is_some()
                                && self.compiler_options.get_isolated_modules()
                                && !self.store().node(loc).flags.intersects(NF::AMBIENT)
                                && result
                                    .and_then(|r| self.program_symbol(r).value_declaration)
                                    .is_some_and(|vd| loc.file_id() != vd.file_id())
                            {
                                let isolated_modules_like_flag_name =
                                    if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue {
                                        "verbatimModuleSyntax"
                                    } else {
                                        "isolatedModules"
                                    };
                                let enum_name = self.program_symbol(enum_symbol).name.clone();
                                let args = vec![
                                    name.to_string(),
                                    isolated_modules_like_flag_name.to_string(),
                                    format!("{enum_name}.{name}"),
                                ];
                                self.error(
                                    original_location.expect("error location"),
                                    &diag::CANNOT_ACCESS_0_FROM_ANOTHER_FILE_WITHOUT_QUALIFICATION_WHEN_1_IS_ENABLED_USE_2_INSTEAD,
                                    &args,
                                );
                            }
                            break 'outer;
                        }
                    }
                }
                Kind::PropertyDeclaration => {
                    if !binder_shims::is_static(self.store().node(loc)) {
                        let parent = binder_shims::parent_of(self.store(), loc).expect("member parent");
                        if let Some(ctor) = binder_shims::find_constructor_declaration(self.store(), parent) {
                            if let Some(ctor_locals) = binder_node_data::locals_of(self.store(), ctor) {
                                let ctor_locals = ctor_locals.clone();
                                let value_meaning = meaning & SymbolFlags::VALUE;
                                if self.name_lookup(&ctor_locals, name, value_meaning, suggestion).is_some() {
                                    // Remember the property node, it will be used later to report appropriate error
                                    property_with_invalid_initializer = Some(loc);
                                }
                            }
                        }
                    }
                }
                Kind::ClassDeclaration | Kind::ClassExpression | Kind::InterfaceDeclaration => {
                    let container_symbol =
                        self.get_symbol_of_declaration_merged(loc).expect("class-like symbol");
                    let members = self.program_symbol(container_symbol).members.clone();
                    result = self.name_lookup(&members, name, meaning & SymbolFlags::TYPE, suggestion);
                    if result.is_some() {
                        if !self.is_type_parameter_symbol_declared_in_container(result.unwrap(), loc) {
                            // ignore type parameters not declared in this container
                            result = None;
                        } else {
                            if last_location.is_some_and(|l| binder_shims::is_static(self.store().node(l))) {
                                // TypeScript 1.0 spec (April 2014): 3.4.1
                                // The scope of a type parameter extends over the entire declaration with which the type
                                // parameter list is associated, with the exception of static member declarations in classes.
                                if name_not_found_message.is_some() {
                                    self.error(
                                        original_location.expect("error location"),
                                        &diag::STATIC_MEMBERS_CANNOT_REFERENCE_CLASS_TYPE_PARAMETERS,
                                        &[],
                                    );
                                }
                                return None;
                            }
                            break 'outer;
                        }
                    }
                    if tsc_ast::is_class_expression(self.store().node(loc))
                        && meaning.intersects(SymbolFlags::CLASS)
                    {
                        let class_name = self.store().node(loc).name();
                        if let Some(class_name) = class_name {
                            if name == tsc_ast::node_text(self.store(), class_name) {
                                result = binder_node_data::symbol_of(self.store(), loc);
                                break 'outer;
                            }
                        }
                    }
                }
                Kind::ExpressionWithTypeArguments => {
                    let parent = binder_shims::parent_of(self.store(), loc);
                    let is_extends_clause = parent.is_some_and(|p| {
                        tsc_ast::is_heritage_clause(self.store().node(p))
                            && match &self.store().node(p).data {
                                tsc_ast::NodeData::HeritageClause(d) => d.token == Kind::ExtendsKeyword,
                                _ => false,
                            }
                    });
                    if last_location == binder_node_data::expression_of(self.store(), loc)
                        && is_extends_clause
                    {
                        let container = binder_shims::parent_of(self.store(), parent.unwrap())
                            .expect("heritage clause parent");
                        if binder_shims::is_class_like(self.store().node(container)) {
                            let container_symbol =
                                self.get_symbol_of_declaration_merged(container).expect("class-like symbol");
                            let members = self.program_symbol(container_symbol).members.clone();
                            result = self.name_lookup(&members, name, meaning & SymbolFlags::TYPE, suggestion);
                            if result.is_some() {
                                if name_not_found_message.is_some() {
                                    self.error(
                                        original_location.expect("error location"),
                                        &diag::BASE_CLASS_EXPRESSIONS_CANNOT_REFERENCE_CLASS_TYPE_PARAMETERS,
                                        &[],
                                    );
                                }
                                return None;
                            }
                        }
                    }
                }
                // It is not legal to reference a class's own type parameters from a computed property name that
                // belongs to the class. For example:
                //
                //   function foo<T>() { return '' }
                //   class C<T> { // <-- Class's own type parameter T
                //       [foo<T>()]() { } // <-- Reference to T from class's own computed property
                //   }
                Kind::ComputedPropertyName => {
                    let grandparent = binder_shims::parent_of(self.store(), loc)
                        .and_then(|p| binder_shims::parent_of(self.store(), p))
                        .expect("ComputedPropertyName grandparent");
                    if binder_shims::is_class_like(self.store().node(grandparent))
                        || tsc_ast::is_interface_declaration(self.store().node(grandparent))
                    {
                        // A reference to this grandparent's type parameters would be an error
                        let grandparent_symbol =
                            self.get_symbol_of_declaration_merged(grandparent).expect("class-like symbol");
                        let members = self.program_symbol(grandparent_symbol).members.clone();
                        result = self.name_lookup(&members, name, meaning & SymbolFlags::TYPE, suggestion);
                        if result.is_some() {
                            if name_not_found_message.is_some() {
                                self.error(
                                    original_location.expect("error location"),
                                    &diag::A_COMPUTED_PROPERTY_NAME_CANNOT_REFERENCE_A_TYPE_PARAMETER_FROM_ITS_CONTAINING_TYPE,
                                    &[],
                                );
                            }
                            return None;
                        }
                    }
                }
                Kind::MethodDeclaration
                | Kind::Constructor
                | Kind::GetAccessor
                | Kind::SetAccessor
                | Kind::FunctionDeclaration => {
                    if meaning.intersects(SymbolFlags::VARIABLE) && name == "arguments" {
                        result = Some(self.arguments_symbol.expect("argumentsSymbol"));
                        break 'outer;
                    }
                }
                Kind::FunctionExpression => {
                    if meaning.intersects(SymbolFlags::VARIABLE) && name == "arguments" {
                        result = Some(self.arguments_symbol.expect("argumentsSymbol"));
                        break 'outer;
                    }
                    if meaning.intersects(SymbolFlags::FUNCTION) {
                        let function_name = match &self.store().node(loc).data {
                            tsc_ast::NodeData::FunctionExpression(d) => d.name,
                            _ => None,
                        };
                        if let Some(function_name) = function_name {
                            if name == tsc_ast::node_text(self.store(), function_name) {
                                result = binder_node_data::symbol_of(self.store(), loc);
                                break 'outer;
                            }
                        }
                    }
                }
                Kind::Decorator => {
                    // Decorators are resolved at the class declaration. Resolving at the parameter
                    // or member would result in looking up locals in the method.
                    //
                    //   function y() {}
                    //   class C {
                    //       method(@y x, y) {} // <-- decorator y should be resolved at the class declaration, not the parameter.
                    //   }
                    //
                    if let Some(parent) = binder_shims::parent_of(self.store(), loc) {
                        if self.store().node(parent).kind == Kind::Parameter {
                            location = Some(parent);
                        }
                    }
                    //   function y() {}
                    //   class C {
                    //       @y method(x, y) {} // <-- decorator y should be resolved at the class declaration, not the method.
                    //   }
                    //
                    // class Decorators are resolved outside of the class to avoid referencing type parameters of that class.
                    //
                    //   type T = number;
                    //   declare function y(x: T): any;
                    //   @param(1 as T) // <-- T should resolve to the type alias outside of class C
                    //   class C<T> {}
                    //
                    if let Some(parent) = binder_shims::parent_of(self.store(), loc) {
                        if binder_shims::is_class_element(self.store().node(parent))
                            || self.store().node(parent).kind == Kind::ClassDeclaration
                        {
                            location = Some(parent);
                        }
                    }
                }
                Kind::Parameter => {
                    let parameter_declaration = match &self.store().node(loc).data {
                        tsc_ast::NodeData::ParameterDeclaration(d) => (d.initializer, d.name),
                        _ => (None, NodeId::NONE),
                    };
                    if let Some(last) = last_location {
                        let is_initializer = Some(last) == parameter_declaration.0;
                        let is_binding_pattern_name = Some(last) == self.store().node(loc).name()
                            && binder_shims::is_binding_pattern(self.store().node(last));
                        if (is_initializer || is_binding_pattern_name)
                            && associated_declaration_for_containing_initializer_or_binding_name.is_none()
                        {
                            associated_declaration_for_containing_initializer_or_binding_name = Some(loc);
                        }
                    }
                }
                Kind::BindingElement => {
                    let binding_element = match &self.store().node(loc).data {
                        tsc_ast::NodeData::BindingElement(d) => (d.initializer, d.name),
                        _ => (None, None),
                    };
                    if let Some(last) = last_location {
                        let is_initializer = Some(last) == binding_element.0;
                        let is_binding_pattern_name = Some(last) == binding_element.1
                            && binder_shims::is_binding_pattern(self.store().node(last));
                        if (is_initializer || is_binding_pattern_name)
                            && binder_shims::is_part_of_parameter_declaration(self.store(), loc)
                            && associated_declaration_for_containing_initializer_or_binding_name.is_none()
                        {
                            associated_declaration_for_containing_initializer_or_binding_name = Some(loc);
                        }
                    }
                }
                Kind::InferType => {
                    if meaning.intersects(SymbolFlags::TYPE_PARAMETER) {
                        let parameter_name = match &self.store().node(loc).data {
                            tsc_ast::NodeData::InferTypeNode(d) => match &self.store().node(d.type_parameter).data {
                                tsc_ast::NodeData::TypeParameterDeclaration(td) => td.name,
                                _ => NodeId::NONE,
                            },
                            _ => NodeId::NONE,
                        };
                        if !parameter_name.is_none()
                            && name == tsc_ast::node_text(self.store(), parameter_name)
                        {
                            result = match &self.store().node(loc).data {
                                tsc_ast::NodeData::InferTypeNode(d) => {
                                    binder_node_data::symbol_of(self.store(), d.type_parameter)
                                }
                                _ => None,
                            };
                            break 'outer;
                        }
                    }
                }
                Kind::ExportSpecifier => {
                    let property_name = binder_node_data::property_name_of(self.store(), loc);
                    if last_location.is_some() && last_location == property_name {
                        let export_declaration = binder_shims::parent_of(self.store(), loc)
                            .and_then(|p| binder_shims::parent_of(self.store(), p));
                        let module_specifier =
                            export_declaration.and_then(|ed| binder_node_data::module_specifier_of(self.store(), ed));
                        if module_specifier.is_some() {
                            location = export_declaration
                                .and_then(|ed| binder_shims::parent_of(self.store(), ed));
                        }
                    }
                }
                _ => {}
            }
            // Go: `if isSelfReferenceLocation(location, lastLocation) {...};
            // lastLocation = location; location = location.Parent` — note
            // `location` (not `loc`): the Decorator/ExportSpecifier arms may
            // have reassigned it.
            let effective = location.expect("loop iterates only while location is Some");
            if is_self_reference_location(self.store().node(effective), last_location) {
                last_self_reference_location = Some(effective);
            }
            last_location = location;
            location = binder_shims::parent_of(self.store(), effective);
        }
        // We just climbed up parents looking for the name, meaning that we started in a descendant node of `lastLocation`.
        // If `result === lastSelfReferenceLocation.symbol`, that means that we are somewhere inside `lastSelfReferenceLocation` looking up a name, and resolving to `lastLocation` itself.
        // That means that this is a self-reference of `lastLocation`, and shouldn't count this when considering whether `lastLocation` is used.
        if is_use {
            if let Some(result_symbol) = result {
                let is_self_reference = last_self_reference_location
                    .and_then(|l| binder_node_data::symbol_of(self.store(), l))
                    .is_some_and(|s| s == result_symbol);
                if !is_self_reference {
                    self.symbol_referenced(result_symbol, meaning);
                }
            }
        }
        if result.is_none() && !exclude_globals {
            let globals = self.globals.clone();
            result = self.name_lookup(&globals, name, meaning.union(SymbolFlags::GLOBAL_LOOKUP), suggestion);
        }
        if result.is_none() {
            if let Some(original_location) = original_location {
                if tsc_ast::utilities::is_in_js_file(self.store().node(original_location)) {
                    if let Some(parent) = binder_shims::parent_of(self.store(), original_location) {
                        if binder_shims::is_require_call(self.store(), parent, false /*requireStringLiteralLikeArgument*/) {
                            return self.require_symbol;
                        }
                    }
                }
            }
        }
        if name_not_found_message.is_some() {
            if let Some(property) = property_with_invalid_initializer {
                if self.check_and_report_error_for_invalid_initializer(
                    original_location,
                    name,
                    property,
                    result,
                ) {
                    return None;
                }
            }
            if result.is_none() {
                // PORT: Go passes originalLocation (possibly nil — the
                // getGlobalSymbol family resolves with a nil location);
                // the hook's nil gates are on_failed_to_resolve_symbol's.
                self.on_failed_to_resolve_symbol(original_location, name, meaning, name_not_found_message);
            } else {
                self.on_successfully_resolved_symbol(
                    original_location,
                    result,
                    meaning,
                    last_location,
                    associated_declaration_for_containing_initializer_or_binding_name,
                    within_deferred_context,
                );
            }
        }
        result
    }

    /// Go: `func (r *NameResolver) symbolReferenced(symbol, meaning)` (the
    /// checker hook, checker.go:1543). pub(crate) since M6 W5 (jsx.go's
    /// markJsxAliasReferenced).
    pub(crate) fn symbol_referenced(&mut self, symbol: SymbolId, meaning: SymbolFlags) {
        self.symbol_reference_links.get(symbol).reference_kinds |= meaning;
    }

    /// Go: `func (c *Checker) markAliasSymbolAsReferenced(symbol)` (checker.go:15493) —
    /// REAL (M6 W5: jsx.go's markJsxAliasReferenced reaches it). When an
    /// alias symbol is referenced, we need to mark the entity it references
    /// as referenced and in turn repeat that until we reach a non-alias or
    /// an exported entity (Go comment, verbatim).
    pub(crate) fn mark_alias_symbol_as_referenced(&mut self, symbol: SymbolId) {
        if !self.alias_symbol_links.get(symbol).referenced {
            self.alias_symbol_links.get(symbol).referenced = true;
            let node = self.get_declaration_of_alias_symbol(symbol);
            let Some(node) = node else {
                panic!("Unexpected nil in markAliasSymbolAsReferenced");
            };
            // We defer checking of the reference of an `import =` until the
            // import itself is referenced, This way a chain of imports can
            // be elided if ultimately the final input is only used in a type
            // position. (Go comment, verbatim.)
            let module_reference = {
                let n = self.store().node(node);
                if n.kind == Kind::ImportEqualsDeclaration {
                    Some(n.as_import_equals_declaration().expect("ImportEqualsDeclaration data").module_reference)
                } else {
                    None
                }
            };
            if let Some(module_reference) = module_reference {
                if self.store().node(module_reference).kind != Kind::ExternalModuleReference {
                    let resolved = self.resolve_symbol(symbol);
                    if self.get_symbol_flags(resolved).intersects(SymbolFlags::VALUE) {
                        // import foo = <symbol>
                        let left = crate::jsx::get_first_identifier(self.store(), module_reference);
                        self.mark_identifier_alias_referenced(left);
                    }
                }
            }
        }
    }

    /// Go: `func (c *Checker) markIdentifierAliasReferenced(location *ast.IdentifierNode)` (checker.go:15483).
    pub(crate) fn mark_identifier_alias_referenced(&mut self, location: NodeId) {
        if crate::utilities::is_in_type_query(self.store(), location) {
            return;
        }
        let symbol = self.get_resolved_symbol(location);
        if symbol.is_some_and(|symbol| {
            Some(symbol) != self.arguments_symbol
                && symbol != self.unknown_symbol()
        }) {
            self.mark_alias_referenced(symbol.expect("checked"), location);
        }
    }

    /// Go: `func (c *Checker) markAliasReferenced(symbol, location)` (checker.go:15466) —
    /// REAL (M6 W5: markIdentifierAliasReferenced's chain).
    pub(crate) fn mark_alias_referenced(&mut self, symbol: SymbolId, location: NodeId) {
        if !self.can_collect_symbol_alias_accessibility_data {
            return;
        }
        let is_non_local = is_non_local_alias(self.program_symbol(symbol), SymbolFlags::VALUE);
        if is_non_local && !crate::utilities::is_in_type_query(self.store(), location) {
            let target = self.resolve_alias(symbol);
            if self
                .get_symbol_flags_ex(symbol, true /*excludeTypeOnlyMeanings*/, false /*excludeLocalMeanings*/)
                .intersects(SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE)
            {
                // An alias resolving to a const enum cannot be elided if
                // (1) 'isolatedModules' is enabled (because the const enum
                // value will not be inlined), or if (2) the alias is an
                // export of a const enum declaration that will be preserved
                // (Go comment, verbatim).
                if self.compiler_options.get_isolated_modules()
                    || self.compiler_options.should_preserve_const_enums()
                        && is_export_or_export_expression(self.store(), location)
                    || {
                        let exported =
                            self.get_export_symbol_of_value_symbol_if_exported(target);
                        !is_const_enum_or_const_enum_only_module(self, exported)
                    }
                {
                    self.mark_alias_symbol_as_referenced(symbol);
                }
            }
        }
    }

    /// Go: the shared `KindModuleDeclaration` switch body of NameResolver.Resolve
    /// (nameresolver.go — run via fallthrough from KindSourceFile when the
    /// file is an external or CommonJS module).
    fn resolve_module_case(
        &mut self,
        location: NodeId,
        name: &str,
        meaning: SymbolFlags,
        is_module_attributes: bool,
        result: &mut Option<SymbolId>,
        suggestion: bool,
    ) {
        if is_module_attributes {
            // Go: break out of the switch.
            return;
        }
        let Some(module_symbol) = self.get_symbol_of_declaration_merged(location) else {
            return;
        };
        let module_exports = self.program_symbol(module_symbol).exports.clone();
        let is_source_file_loc = is_source_file(self.store().node(location));
        let is_ambient_module_decl = is_module_declaration(self.store().node(location))
            && self.store().node(location).flags.intersects(NF::AMBIENT)
            && !binder_shims::is_global_scope_augmentation(self.store().node(location));
        if is_source_file_loc || is_ambient_module_decl {
            // It's an external module. First see if the module has an export default and if the local
            // name of that export default matches.
            *result = module_exports.get(INTERNAL_SYMBOL_NAME_DEFAULT).copied();
            if let Some(result_symbol) = *result {
                // PORT: Go's GetLocalSymbolForExportDefault reads the
                // declarations' localSymbol links through the symbol arena;
                // the binder free helper needs `&[Symbol]`, which cannot be
                // borrowed while `&mut Checker` is live, so the walk is
                // inlined (isExportDefaultSymbol + the first localSymbol).
                let declarations = self.program_symbol(result_symbol).declarations.clone();
                let local_symbol = declarations
                    .first()
                    .copied()
                    .filter(|&decl| {
                        has_syntactic_modifier(self.store().node(decl), ModifierFlags::DEFAULT)
                    })
                    .and_then(|decl| binder_node_data::local_symbol_of(self.store(), decl));
                let flags = self.program_symbol(result_symbol).flags;
                let local_name_matches = local_symbol
                    .map(|ls| self.program_symbol(ls).name == name)
                    .unwrap_or(false);
                if flags.intersects(meaning) && local_name_matches {
                    return; // caller breaks with result set
                }
                *result = None;
            }
            // Because of module/namespace merging, a module's exports are in scope,
            // yet we never want to treat an export specifier as putting a member in scope.
            // Therefore, if the name we find is purely an export specifier, it is not actually considered in scope.
            // Two things to note about this:
            //     1. We have to check this without calling getSymbol. The problem with calling getSymbol
            //        on an export specifier is that it might find the export specifier itself, and try to
            //        resolve it as an alias. This will cause the checker to consider the export specifier
            //        a circular alias reference when it might not be.
            //     2. We check === SymbolFlags.Alias in order to check that the symbol is *purely*
            //        an alias. If we used &, we would be throwing out symbols that have non alias aspects,
            //        which is not the desired behavior.
            let module_export = module_exports.get(name).copied();
            if let Some(module_export) = module_export {
                let flags = self.program_symbol(module_export).flags;
                if flags == SymbolFlags::ALIAS
                    && (self
                        .get_declaration_of_kind_of_symbol(module_export, Kind::ExportSpecifier)
                        .is_some()
                        || self
                            .get_declaration_of_kind_of_symbol(module_export, Kind::NamespaceExport)
                            .is_some())
                {
                    return; // Go: break out of the switch
                }
            }
        }
        if name != INTERNAL_SYMBOL_NAME_DEFAULT {
            let module_meaning = meaning & SymbolFlags::MODULE_MEMBER;
            *result = self.name_lookup(&module_exports, name, module_meaning, suggestion);
            if let Some(result_symbol) = *result {
                let flags = self.program_symbol(result_symbol).flags;
                // Go: on a CommonJS source file, a non-type result falls
                // back to nil (the caller's break happens on Some(result)
                // after the return).
                if is_source_file_loc
                    && self
                        .store()
                        .file(location.file_id())
                        .payload()
                        .common_js_module_indicator
                        .get()
                        .is_some()
                    && !flags.intersects(SymbolFlags::TYPE)
                {
                    *result = None;
                }
            }
        }
    }

    /// Go: `func (r *NameResolver) useOuterVariableScopeInParameter(result, location, lastLocation) bool`.
    fn use_outer_variable_scope_in_parameter(
        &mut self,
        result: SymbolId,
        location: NodeId,
        last_location: Option<NodeId>,
    ) -> bool {
        if last_location.is_some_and(|l| tsc_ast::is_parameter_declaration(self.store().node(l))) {
            let body = binder_node_data::body_of(self.store(), location);
            let value_declaration = self.program_symbol(result).value_declaration;
            let in_body = body.is_some_and(|b| {
                value_declaration.is_some_and(|vd| {
                    self.store().node(vd).pos() >= self.store().node(b).pos()
                        && self.store().node(vd).end() <= self.store().node(b).end()
                })
            });
            if in_body {
                // check for several cases where we introduce temporaries that require moving the name/initializer of the parameter to the body
                // - static field in a class expression
                // - optional chaining pre-es2020
                // - nullish coalesce pre-es2020
                // - spread assignment in binding pattern pre-es2017
                let function_location = location;
                let mut declaration_requires_scope_change =
                    self.node_links.get(function_location).declaration_requires_scope_change;
                if declaration_requires_scope_change == Tristate::TSUnknown {
                    let parameters = binder_node_data::parameters_of(self.store(), function_location)
                        .map(|list| list.nodes.to_vec())
                        .unwrap_or_default();
                    let any = parameters.iter().any(|&p| self.requires_scope_change(p));
                    declaration_requires_scope_change =
                        if any { Tristate::TSTrue } else { Tristate::TSFalse };
                    self.node_links.get(function_location).declaration_requires_scope_change =
                        declaration_requires_scope_change;
                }
                return declaration_requires_scope_change != Tristate::TSTrue;
            }
        }
        false
    }

    /// Go: `func (r *NameResolver) requiresScopeChange(node) bool`.
    fn requires_scope_change(&mut self, node: NodeId) -> bool {
        let d = match &self.store().node(node).data {
            tsc_ast::NodeData::ParameterDeclaration(d) => (d.name, d.initializer),
            _ => return false,
        };
        self.requires_scope_change_worker(Some(d.0))
            || d.1.is_some() && {
                let init = d.1.unwrap();
                self.requires_scope_change_worker(Some(init))
            }
    }

    /// Go: `func (r *NameResolver) requiresScopeChangeWorker(node) bool`.
    fn requires_scope_change_worker(&mut self, node: Option<NodeId>) -> bool {
        let Some(node) = node else {
            return false;
        };
        match self.store().node(node).kind {
            Kind::ArrowFunction | Kind::FunctionExpression | Kind::FunctionDeclaration | Kind::Constructor => {
                false
            }
            Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor | Kind::PropertyAssignment => {
                self.requires_scope_change_worker(self.store().node(node).name())
            }
            Kind::PropertyDeclaration => {
                if binder_shims::has_static_modifier(self.store().node(node)) {
                    !self.compiler_options.get_emit_standard_class_fields()
                } else {
                    self.requires_scope_change_worker(self.store().node(node).name())
                }
            }
            _ => {
                if binder_shims::is_nullish_coalesce(self.store(), node)
                    || tsc_ast::utilities::is_optional_chain(self.store().node(node))
                {
                    return self.compiler_options.get_emit_script_target()
                        < tsc_core::options_generated::ScriptTarget::ES2020;
                }
                if tsc_ast::is_binding_element(self.store().node(node))
                    && match &self.store().node(node).data {
                        tsc_ast::NodeData::BindingElement(d) => d.dot_dot_dot_token.is_some(),
                        _ => false,
                    }
                    && binder_shims::parent_of(self.store(), node)
                        .is_some_and(|p| binder_shims::is_binding_pattern(self.store().node(p)))
                {
                    return self.compiler_options.get_emit_script_target()
                        < tsc_core::options_generated::ScriptTarget::ES2017;
                }
                if binder_shims::is_type_node(self.store().node(node)) {
                    return false;
                }
                let mut children: Vec<NodeId> = Vec::new();
                self.store().node(node).for_each_child(&mut |child| {
                    children.push(child);
                    false
                });
                for child in children {
                    if self.requires_scope_change_worker(Some(child)) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Go: `func (c *Checker) checkAndReportErrorForInvalidInitializer(location, name, propertyWithInvalidInitializer, result) bool`
    /// (checker.go:1558 — the OnPropertyWithInvalidInitializer hook).
    fn check_and_report_error_for_invalid_initializer(
        &mut self,
        error_location: Option<NodeId>,
        name: &str,
        property_with_invalid_initializer: NodeId,
        result: Option<SymbolId>,
    ) -> bool {
        if !self.compiler_options.get_emit_standard_class_fields() {
            if error_location.is_some()
                && result.is_none()
                && self.check_and_report_error_for_missing_prefix(error_location.unwrap(), name)
            {
                return true;
            }
            // We have a match, but the reference occurred within a property initializer and the identifier also binds
            // to a local variable in the constructor where the code will be emitted. Note that this is actually allowed
            // with emitStandardClassFields because the scope semantics are different.
            let prop = match &self.store().node(property_with_invalid_initializer).data {
                tsc_ast::NodeData::PropertyDeclaration(d) => d,
                _ => panic!("PropertyDeclaration data"),
            };
            let prop_name = binder_shims::node_text(self.store(), prop.name).to_string();
            let message = match prop.type_ {
                Some(type_node)
                    if error_location.is_some_and(|e| {
                        let loc = self.store().node(type_node).loc;
                        loc.contains_inclusive(self.store().node(e).pos())
                    }) =>
                {
                    &diag::TYPE_OF_INSTANCE_MEMBER_VARIABLE_0_CANNOT_REFERENCE_IDENTIFIER_1_DECLARED_IN_THE_CONSTRUCTOR
                }
                _ => &diag::INITIALIZER_OF_INSTANCE_MEMBER_VARIABLE_0_CANNOT_REFERENCE_IDENTIFIER_1_DECLARED_IN_THE_CONSTRUCTOR,
            };
            if let Some(error_location) = error_location {
                self.error(error_location, message, &[prop_name, name.to_string()]);
            }
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForMissingPrefix(errorLocation, name) bool`
    /// (checker.go:1576).
    fn check_and_report_error_for_missing_prefix(&mut self, error_location: NodeId, name: &str) -> bool {
        if !is_identifier(self.store().node(error_location))
            || tsc_ast::node_text(self.store(), error_location) != name
            || u::is_type_reference_identifier(self.store(), error_location)
            || crate::utilities::is_in_type_query(self.store(), error_location)
        {
            return false;
        }
        let container =
            binder_shims::get_this_container(self.store(), error_location, false, false);
        let mut location = Some(container);
        while let Some(loc) = location
            && let Some(parent) = binder_shims::parent_of(self.store(), loc)
        {
            if binder_shims::is_class_like(self.store().node(parent)) {
                let Some(class_symbol) = self.get_symbol_of_declaration_merged(parent) else {
                    break;
                };
                // Check to see if a static member exists.
                let constructor_type = self.get_type_of_symbol(class_symbol);
                if self.get_property_of_type(constructor_type, name).is_some() {
                    let class_name = crate::relater::symbol_to_string(self, class_symbol);
                    self.error(
                        error_location,
                        &diag::CANNOT_FIND_NAME_0_DID_YOU_MEAN_THE_STATIC_MEMBER_1_0,
                        &[name.to_string(), class_name],
                    );
                    return true;
                }
                // No static member is present.
                // Check if we're in an instance method and look for a relevant instance member.
                if loc == container && !binder_shims::is_static(self.store().node(loc)) {
                    // TODO: GH#18217 (Go comment) — the instance-type probe
                    // reads the interface thisType through the S8 members
                    // seam; permissive None keeps the arm dormant.
                    // TODO(port M6 S8): getDeclaredTypeOfSymbol(...).thisType.
                    let _instance_probe = self.get_declared_type_of_symbol(class_symbol);
                }
            }
            location = binder_shims::parent_of(self.store(), loc);
        }
        false
    }

    /// Go: `func (c *Checker) onFailedToResolveSymbol(location, name, meaning, nameNotFoundMessage)`
    /// (checker.go:1608 — the OnFailedToResolveSymbol hook).
    ///
    /// PORT: `errorLocation` is Go's `*ast.Node` — NILABLE. The
    /// `getGlobalSymbol` family calls `resolveName(nil, name, meaning,
    /// diagnostic, ...)` (checker.go:1254), so a failed global resolution
    /// lands here with `None` and must flow to Go's `c.error(nil, ...)`
    /// — a location-less (global-collection) diagnostic. Every
    /// location-dereferencing arm below is gated exactly as Go's
    /// `errorLocation != nil &&` guards; the error emitters take
    /// `NodeId::NONE` for nil (safe since the `get_file_and_range`
    /// nil guard — utilities.rs).
    fn on_failed_to_resolve_symbol(
        &mut self,
        error_location: Option<NodeId>,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
    ) {
        // The `const` in a `const` assertion (`x as const`) is a syntactic marker, not a real
        // type reference, and must never be resolved or reported as an unresolvable name.
        // (Go's isConstTypeReferenceName is `node != nil &&` — None is false.)
        if error_location.is_some_and(|loc| u::is_const_type_reference_name(self.store(), loc)) {
            return;
        }
        // Go: `if errorLocation != nil && (errorLocation.Parent.Kind == ... || ladder)`.
        if error_location.is_some_and(|loc| {
            let parent_kind = binder_shims::parent_of(self.store(), loc)
                .map(|p| self.store().node(p).kind);
            parent_kind == Some(Kind::JSDocLink)
                || self.check_and_report_error_for_missing_prefix(loc, name)
                || self.check_and_report_error_for_extending_interface(loc)
                || self.check_and_report_error_for_using_type_as_namespace(loc, name, meaning)
                || self.check_and_report_error_for_exporting_primitive_type(loc, name)
                || self.check_and_report_error_for_using_namespace_as_type_or_value(loc, name, meaning)
                || self.check_and_report_error_for_using_type_as_value(loc, name, meaning)
                || self.check_and_report_error_for_using_value_as_type(loc, name, meaning)
        }) {
            return;
        }
        let mut declaration_name = name.to_string();
        if error_location.is_some_and(|loc| {
            is_identifier(self.store().node(loc)) && tsc_ast::node_text(self.store(), loc) == name
        }) {
            declaration_name = tsc_scanner::utilities::declaration_name_to_string(
                self.store(),
                error_location,
            );
        }
        // Report missing lib first
        // REAL (M6 S10, checker.go:1778): getSuggestedLibForNonExistentName —
        // the feature map's first lib for the name (getFeatureMap's
        // typeFeatures[0].lib read; utilities.rs's table).
        let suggested_lib = crate::utilities::get_suggested_lib_for_non_existent_name(name);
        if !suggested_lib.is_empty() {
            self.error(error_location.unwrap_or(NodeId::NONE), name_not_found_message.unwrap(), &[declaration_name, suggested_lib]);
            return;
        }
        // Then spelling suggestions
        let suggestion = self.get_suggested_symbol_for_nonexistent_symbol(error_location, name, meaning);
        let ambient_global_augmentation = suggestion
            .and_then(|s| self.program_symbol(s).value_declaration)
            .is_some_and(|vd| {
                binder_shims::is_ambient_module(self.store(), vd)
                    && binder_shims::is_global_scope_augmentation(self.store().node(vd))
            });
        if let Some(suggestion) = suggestion.filter(|_| !ambient_global_augmentation) {
            let suggestion_name = crate::relater::symbol_to_string(self, suggestion);
            // TODO(port M6 W5/js): isUncheckedJSSuggestion reads the JSDoc
            // host surface; permissive false (the error form).
            let is_unchecked_js = false;
            let message = if meaning == SymbolFlags::NAMESPACE {
                &diag::CANNOT_FIND_NAMESPACE_0_DID_YOU_MEAN_1
            } else if is_unchecked_js {
                &diag::COULD_NOT_FIND_NAME_0_DID_YOU_MEAN_1
            } else {
                &diag::CANNOT_FIND_NAME_0_DID_YOU_MEAN_1
            };
            // Go attaches the suggestion's "is declared here" related info
            // to the diagnostic; the port's errorOrSuggestion carrier builds
            // the diagnostic internally (spine.rs) — the related-info attach
            // rides its Diagnostic return with the nodebuilder wave.
            self.error_or_suggestion(
                !is_unchecked_js,
                error_location.unwrap_or(NodeId::NONE),
                message,
                &[declaration_name.clone(), suggestion_name],
            );
            return;
        }
        // And then fall back to unspecified "not found"
        self.error(error_location.unwrap_or(NodeId::NONE), name_not_found_message.unwrap(), &[declaration_name]);
    }

    /// Go: `func (c *Checker) checkAndReportErrorForUsingTypeAsNamespace(errorLocation, name, meaning) bool`
    /// (checker.go:1652).
    fn check_and_report_error_for_using_type_as_namespace(
        &mut self,
        error_location: NodeId,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning == SymbolFlags::NAMESPACE {
            let symbol = self
                .resolve_name_worker(
                    Some(error_location),
                    name,
                    SymbolFlags::TYPE & !SymbolFlags::NAMESPACE,
                    None,
                    false,
                    false,
                )
                .map(|s| self.resolve_symbol(s));
            if let Some(symbol) = symbol {
                let parent = binder_shims::parent_of(self.store(), error_location);
                if parent.is_some_and(|p| self.store().node(p).kind == Kind::QualifiedName) {
                    // Should only be resolving left side of qualified name as a namespace
                    let prop_name = match &self.store().node(parent.unwrap()).data {
                        tsc_ast::NodeData::QualifiedName(d) => {
                            binder_shims::node_text(self.store(), d.right).to_string()
                        }
                        _ => String::new(),
                    };
                    let declared = self.get_declared_type_of_symbol(symbol);
                    if self.get_property_of_type(declared, &prop_name).is_some() {
                        self.error(
                            parent.unwrap(),
                            &diag::CANNOT_ACCESS_0_1_BECAUSE_0_IS_A_TYPE_BUT_NOT_A_NAMESPACE_DID_YOU_MEAN_TO_RETRIEVE_THE_TYPE_OF_THE_PROPERTY_1_IN_0_WITH_0_1,
                            &[name.to_string(), prop_name],
                        );
                        return true;
                    }
                }
                self.error(
                    error_location,
                    &diag::X_0_ONLY_REFERS_TO_A_TYPE_BUT_IS_BEING_USED_AS_A_NAMESPACE_HERE,
                    &[name.to_string()],
                );
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForExportingPrimitiveType(errorLocation, name) bool`
    /// (checker.go:1673).
    fn check_and_report_error_for_exporting_primitive_type(&mut self, error_location: NodeId, name: &str) -> bool {
        let parent_kind = binder_shims::parent_of(self.store(), error_location)
            .map(|p| self.store().node(p).kind);
        if is_primitive_type_name(name) && parent_kind == Some(Kind::ExportSpecifier) {
            self.error(
                error_location,
                &diag::CANNOT_EXPORT_0_ONLY_LOCAL_DECLARATIONS_CAN_BE_EXPORTED_FROM_A_MODULE,
                &[name.to_string()],
            );
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForUsingNamespaceAsTypeOrValue(errorLocation, name, meaning) bool`
    /// (checker.go:1685).
    fn check_and_report_error_for_using_namespace_as_type_or_value(
        &mut self,
        error_location: NodeId,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::VALUE & !SymbolFlags::TYPE) {
            let symbol = self
                .resolve_name_worker(
                    Some(error_location),
                    name,
                    SymbolFlags::NAMESPACE_MODULE,
                    None,
                    false,
                    false,
                )
                .map(|s| self.resolve_symbol(s));
            if symbol.is_some() {
                // `export = ns` may legitimately reference a namespace; checkExportAssignment decides
                // whether that is an error, so don't report "cannot use namespace as a value" here.
                if !u::is_export_assignment_expression_name(self.store(), error_location) {
                    self.error(
                        error_location,
                        &diag::CANNOT_USE_NAMESPACE_0_AS_A_VALUE,
                        &[name.to_string()],
                    );
                }
                return true;
            }
        } else if meaning.intersects(SymbolFlags::TYPE & !SymbolFlags::VALUE) {
            let symbol = self
                .resolve_name_worker(Some(error_location), name, SymbolFlags::MODULE, None, false, false)
                .map(|s| self.resolve_symbol(s));
            if symbol.is_some() {
                self.error(
                    error_location,
                    &diag::CANNOT_USE_NAMESPACE_0_AS_A_TYPE,
                    &[name.to_string()],
                );
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForUsingTypeAsValue(errorLocation, name, meaning) bool`
    /// (checker.go:1706).
    fn check_and_report_error_for_using_type_as_value(
        &mut self,
        error_location: NodeId,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::VALUE) {
            if is_primitive_type_name(name) {
                let grandparent = binder_shims::parent_of(self.store(), error_location)
                    .and_then(|p| binder_shims::parent_of(self.store(), p));
                if let Some(grandparent) = grandparent
                    && tsc_ast::is_heritage_clause(self.store().node(grandparent))
                {
                    let heritage_kind = match &self.store().node(grandparent).data {
                        tsc_ast::NodeData::HeritageClause(d) => d.token,
                        _ => Kind::Unknown,
                    };
                    let container = self.store().node(grandparent).parent.get();
                    let container_kind = if container == NodeId::NONE {
                        Kind::Unknown
                    } else {
                        self.store().node(container).kind
                    };
                    if container_kind == Kind::InterfaceDeclaration && heritage_kind == Kind::ExtendsKeyword {
                        self.error(
                            error_location,
                            &diag::AN_INTERFACE_CANNOT_EXTEND_A_PRIMITIVE_TYPE_LIKE_0_IT_CAN_ONLY_EXTEND_OTHER_NAMED_OBJECT_TYPES,
                            &[name.to_string()],
                        );
                    } else if binder_shims::is_class_like(self.store().node(container))
                        && heritage_kind == Kind::ExtendsKeyword
                    {
                        self.error(
                            error_location,
                            &diag::A_CLASS_CANNOT_EXTEND_A_PRIMITIVE_TYPE_LIKE_0_CLASSES_CAN_ONLY_EXTEND_CONSTRUCTABLE_VALUES,
                            &[name.to_string()],
                        );
                    } else if binder_shims::is_class_like(self.store().node(container))
                        && heritage_kind == Kind::ImplementsKeyword
                    {
                        self.error(
                            error_location,
                            &diag::A_CLASS_CANNOT_IMPLEMENT_A_PRIMITIVE_TYPE_LIKE_0_IT_CAN_ONLY_IMPLEMENT_OTHER_NAMED_OBJECT_TYPES,
                            &[name.to_string()],
                        );
                    }
                } else {
                    self.error(
                        error_location,
                        &diag::X_0_ONLY_REFERS_TO_A_TYPE_BUT_IS_BEING_USED_AS_A_VALUE_HERE,
                        &[name.to_string()],
                    );
                }
                return true;
            }
            let symbol = self
                .resolve_name_worker(
                    Some(error_location),
                    name,
                    SymbolFlags::TYPE & !SymbolFlags::VALUE,
                    None,
                    false,
                    false,
                )
                .map(|s| self.resolve_symbol(s));
            if let Some(symbol) = symbol {
                let all_flags = self.get_symbol_flags(symbol);
                if !all_flags.intersects(SymbolFlags::VALUE) {
                    // `export = SomeType` may legitimately reference a type-only name; checkExportAssignment
                    // decides whether that is an error, so don't report "used as a value" here.
                    if u::is_export_assignment_expression_name(self.store(), error_location) {
                        return true;
                    }
                    if is_es2015_or_later_constructor_name(name) {
                        self.error(
                            error_location,
                            &diag::X_0_ONLY_REFERS_TO_A_TYPE_BUT_IS_BEING_USED_AS_A_VALUE_HERE_DO_YOU_NEED_TO_CHANGE_YOUR_TARGET_LIBRARY_TRY_CHANGING_THE_LIB_COMPILER_OPTION_TO_ES2015_OR_LATER,
                            &[name.to_string()],
                        );
                    } else if self.maybe_mapped_type(error_location, symbol) {
                        let other = if name == "K" { "P" } else { "K" };
                        self.error(
                            error_location,
                            &diag::X_0_ONLY_REFERS_TO_A_TYPE_BUT_IS_BEING_USED_AS_A_VALUE_HERE_DID_YOU_MEAN_TO_USE_1_IN_0,
                            &[name.to_string(), other.to_string()],
                        );
                    } else {
                        self.error(
                            error_location,
                            &diag::X_0_ONLY_REFERS_TO_A_TYPE_BUT_IS_BEING_USED_AS_A_VALUE_HERE,
                            &[name.to_string()],
                        );
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Go: `func (c *Checker) maybeMappedType(node, symbol) bool` (checker.go:1752).
    fn maybe_mapped_type(&mut self, node: NodeId, symbol: SymbolId) -> bool {
        let mut node = Some(node);
        loop {
            let n = node.unwrap();
            let parent = binder_shims::parent_of(self.store(), n);
            if parent.is_some()
                && (tsc_ast::is_computed_property_name(self.store().node(n))
                    || self.store().node(n).kind == Kind::PropertySignature)
            {
                node = parent;
            } else {
                break;
            }
        }
        let node = node.unwrap();
        if tsc_ast::is_type_literal_node(self.store().node(node))
            && binder_node_data::members_of(self.store(), node).len() == 1
        {
            let t = self.get_declared_type_of_symbol(symbol);
            return self.type_(t).flags.intersects(crate::types::TypeFlags::UNION)
                && self.all_types_assignable_to_kind_ex(
                    t,
                    crate::types::TypeFlags::STRING_OR_NUMBER_LITERAL,
                    true,
                );
        }
        false
    }

    /// Go: `func (c *Checker) checkAndReportErrorForUsingValueAsType(errorLocation, name, meaning) bool`
    /// (checker.go:1766).
    fn check_and_report_error_for_using_value_as_type(
        &mut self,
        error_location: NodeId,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::TYPE & !SymbolFlags::NAMESPACE) {
            let symbol = self
                .resolve_name_worker(
                    Some(error_location),
                    name,
                    !SymbolFlags::TYPE & SymbolFlags::VALUE,
                    None,
                    false,
                    false,
                )
                .map(|s| self.resolve_symbol(s));
            if let Some(symbol) = symbol
                && !self.program_symbol(symbol).flags.intersects(SymbolFlags::NAMESPACE)
            {
                self.error(
                    error_location,
                    &diag::X_0_REFERS_TO_A_VALUE_BUT_IS_BEING_USED_AS_A_TYPE_HERE_DID_YOU_MEAN_TYPEOF_0,
                    &[name.to_string()],
                );
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getEntityNameForExtendingInterface(node) *ast.Node`
    /// (checker.go:11914) — climbs up parents to a heritage clause element
    /// and returns its entity name.
    pub(crate) fn get_entity_name_for_extending_interface(&self, node: NodeId) -> Option<NodeId> {
        match self.store().node(node).kind {
            Kind::Identifier | Kind::QualifiedName | Kind::PropertyAccessExpression => {
                binder_shims::parent_of(self.store(), node)
                    .and_then(|parent| self.get_entity_name_for_extending_interface(parent))
            }
            Kind::TypeReference => match &self.store().node(node).data {
                tsc_ast::NodeData::TypeReferenceNode(d) => Some(d.type_name),
                _ => None,
            },
            Kind::ExpressionWithTypeArguments => {
                binder_node_data::expression_of(self.store(), node)
                    .filter(|&expression| {
                        tsc_ast::utilities::is_entity_name_expression(self.store(), expression)
                    })
            }
            _ => None,
        }
    }

    /// Go: `func (c *Checker) getSuggestedSymbolForNonexistentSymbol(location, outerName, meaning) *ast.Symbol`
    /// (checker.go:1785) — `c.resolveNameForSymbolSuggestion(location, ...)`.
    /// The location may be nil (the getGlobalType family resolves globals
    /// with a nil error location; the walk then falls straight to the
    /// globals lookup).
    fn get_suggested_symbol_for_nonexistent_symbol(
        &mut self,
        location: Option<NodeId>,
        outer_name: &str,
        meaning: SymbolFlags,
    ) -> Option<SymbolId> {
        self.resolve_name_for_symbol_suggestion(location, outer_name, meaning)
    }

    /// Go: the `Lookup` hook of `createNameResolverForSuggestion` — every
    /// `r.lookup(...)` site of the walk routes here in suggestion mode:
    /// `getSymbol` first (the exact match stops the walk), then the
    /// spelling suggestion over the scope's symbols (plus the primitive
    /// type-alias extras at the globals stage, gated on
    /// `meaning&SymbolFlagsGlobalLookup`).
    fn name_lookup(
        &mut self,
        symbols: &SymbolTable,
        name: &str,
        meaning: SymbolFlags,
        suggestion: bool,
    ) -> Option<SymbolId> {
        if suggestion {
            self.get_suggestion_for_symbol_name_lookup(symbols, name, meaning)
        } else {
            self.get_symbol(symbols, name, meaning)
        }
    }

    /// Go: `func (c *Checker) getSuggestionForSymbolNameLookup(symbols, name, meaning) *ast.Symbol`
    /// (checker.go:1827).
    fn get_suggestion_for_symbol_name_lookup(
        &mut self,
        symbols: &SymbolTable,
        name: &str,
        meaning: SymbolFlags,
    ) -> Option<SymbolId> {
        if let Some(symbol) = self.get_symbol(symbols, name, meaning) {
            return Some(symbol);
        }
        let mut candidates: Vec<SymbolId> = symbols.values().copied().collect();
        if meaning.intersects(SymbolFlags::GLOBAL_LOOKUP) {
            // Go: getPrimitiveTypeAliasSuggestions (checker.go:1816) — the
            // builtin→primitive transient type-alias suggestions, gated on
            // the builtin name being present in the table.
            for (i, (builtin, primitive)) in PRIMITIVE_TYPE_ALIASES.iter().enumerate() {
                if symbols.get(*builtin).is_some() {
                    if self.primitive_type_alias_suggestion_symbols[i].is_none() {
                        self.primitive_type_alias_suggestion_symbols[i] =
                            Some(self.new_symbol(SymbolFlags::TYPE_ALIAS, primitive));
                    }
                    candidates.push(self.primitive_type_alias_suggestion_symbols[i].unwrap());
                }
            }
        }
        self.get_spelling_suggestion_for_name(name, &candidates, meaning)
    }

    /// Go: `func (c *Checker) getSpellingSuggestionForName(name, symbols, meaning) *ast.Symbol`
    /// (checker.go:1844) — `core.GetSpellingSuggestion` with the symbol-aware
    /// candidate filter (getCandidateName) and `c.compareSymbols` as the
    /// tie-break. Names less than length 3 only check for case-insensitive
    /// equality, not levenshtein distance.
    pub(crate) fn get_spelling_suggestion_for_name(
        &mut self,
        name: &str,
        candidates: &[SymbolId],
        meaning: SymbolFlags,
    ) -> Option<SymbolId> {
        let rune_name: Vec<char> = name.chars().collect();
        let maximum_length_difference = std::cmp::max(2, (rune_name.len() as f64 * 0.34) as usize);
        // If the best result is worse than this, don't bother.
        let mut best_distance = (rune_name.len() as f64 * 0.4).floor() + 0.9;
        let mut best_candidate: Option<SymbolId> = None;
        let mut has_best = false;
        for &candidate in candidates {
            let candidate_name = self.get_candidate_name(candidate, meaning);
            if candidate_name.is_empty() {
                continue;
            }
            // PORT: Go's `len()` on the candidate is bytes; `len(runeName)` runes.
            let max_len = std::cmp::max(candidate_name.len(), rune_name.len());
            let min_len = std::cmp::min(candidate_name.len(), rune_name.len());
            if max_len - min_len > maximum_length_difference {
                continue;
            }
            if candidate_name == name {
                continue;
            }
            // Only consider candidates less than 3 characters long when they differ by case.
            // Otherwise, don't bother, since a user would usually notice differences of a 2-character name.
            if candidate_name.chars().count() < 3 && !candidate_name.eq_ignore_ascii_case(name) {
                continue;
            }
            let candidate_runes: Vec<char> = candidate_name.chars().collect();
            let distance =
                crate::utilities::levenshtein_with_max(&rune_name, &candidate_runes, best_distance);
            if distance < 0.0 {
                continue;
            }
            tsc_debug::assert_!(distance <= best_distance); // Else `levenshteinWithMax` should return undefined
            if distance < best_distance {
                best_distance = distance;
                best_candidate = Some(candidate);
                has_best = true;
            } else if !has_best
                || self.compare_symbols(candidate, best_candidate.unwrap())
                    == std::cmp::Ordering::Less
            {
                best_candidate = Some(candidate);
                has_best = true;
            }
        }
        best_candidate
    }

    /// Go: the `getCandidateName` closure of getSpellingSuggestionForName
    /// (checker.go:1850) — empty for internal (`\xFE`-prefixed) and quoted
    /// (late-bound) names; empty when the meaning does not match directly or
    /// through a resolvable alias.
    fn get_candidate_name(&mut self, candidate: SymbolId, meaning: SymbolFlags) -> String {
        let symbol = self.program_symbol(candidate);
        let candidate_name = symbol.name.clone();
        let flags = symbol.flags;
        if let Some(first) = candidate_name.as_bytes().first()
            && (*first == b'"' || *first == 0xFE)
        {
            return String::new();
        }
        if flags.intersects(meaning) {
            return candidate_name;
        }
        if flags.intersects(SymbolFlags::ALIAS)
            && let Some(alias) = self.try_resolve_alias(candidate)
            && self.program_symbol(alias).flags.intersects(meaning)
        {
            return candidate_name;
        }
        String::new()
    }

    /// Go: `func (c *Checker) onSuccessfullyResolvedSymbol(errorLocation, result, meaning, lastLocation, associatedDeclarationForContainingInitializerOrBindingName, withinDeferredContext)`
    /// (checker.go:1864 — the OnSuccessfullyResolvedSymbol hook).
    #[allow(clippy::too_many_arguments)]
    fn on_successfully_resolved_symbol(
        &mut self,
        error_location: Option<NodeId>,
        result: Option<SymbolId>,
        meaning: SymbolFlags,
        last_location: Option<NodeId>,
        associated_declaration_for_containing_initializer_or_binding_name: Option<NodeId>,
        within_deferred_context: bool,
    ) {
        let Some(result) = result else { return };
        let name = self.program_symbol(result).name.clone();
        let is_in_external_module = last_location.is_some_and(|l| {
            is_source_file(self.store().node(l))
                && binder_shims::is_external_or_common_js_module(self.store().file(l.file_id()))
        });
        // Only check for block-scoped variable if we have an error location and are looking for the
        // name with variable meaning
        //      For example,
        //          declare module foo {
        //              interface bar {}
        //          }
        //      const foo/*1*/: foo/*2*/.bar;
        // The foo at /*1*/ and /*2*/ will share same symbol with two meanings:
        // block-scoped variable and namespace module. However, only when we
        // try to resolve name in /*1*/ which is used in variable position,
        // we want to check for block-scoped
        if (meaning.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
            || meaning.intersects(SymbolFlags::CLASS.union(SymbolFlags::ENUM))
                && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE)
            && let Some(error_location) = error_location
        {
            let export_or_local_symbol = self.get_export_symbol_of_value_symbol_if_exported(result);
            if self
                .program_symbol(export_or_local_symbol)
                .flags
                .intersects(
                    SymbolFlags::BLOCK_SCOPED_VARIABLE
                        .union(SymbolFlags::CLASS)
                        .union(SymbolFlags::ENUM),
                )
            {
                self.check_resolved_block_scoped_variable(export_or_local_symbol, error_location);
            }
        }
        // If we're in an external module, we can't reference value symbols created from UMD export declarations
        if is_in_external_module
            && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE
            && error_location.is_some_and(|e| !self.store().node(e).flags.intersects(NF::JSDOC))
        {
            let merged = self.get_merged_symbol(result);
            let merged_decls = self.program_symbol(merged).declarations.clone();
            if !merged_decls.is_empty()
                && merged_decls.iter().all(|&d| {
                    tsc_ast::is_namespace_export_declaration(self.store().node(d))
                        || is_source_file(self.store().node(d))
                            && !self.store().file(d.file_id()).payload().global_exports.is_empty()
                })
            {
                self.error_or_suggestion(
                    self.compiler_options.allow_umd_global_access != Tristate::TSTrue,
                    error_location.unwrap(),
                    &diag::X_0_REFERS_TO_A_UMD_GLOBAL_BUT_THE_CURRENT_FILE_IS_A_MODULE_CONSIDER_ADDING_AN_IMPORT_INSTEAD,
                    std::slice::from_ref(&name),
                );
            }
        }
        // If we're in a parameter initializer or binding name, we can't reference the values of the parameter whose initializer we're within or parameters to the right
        if let Some(associated) = associated_declaration_for_containing_initializer_or_binding_name
            .filter(|_| !within_deferred_context)
            .filter(|_| (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE)
        {
            let late_bound = self.get_late_bound_symbol(result);
            let candidate = self.get_merged_symbol(late_bound);
            let root = binder_shims::get_root_declaration(self.store(), associated);
            // A parameter initializer or binding pattern initializer within a parameter cannot refer to itself
            if Some(candidate) == self.get_symbol_of_declaration_merged(associated) {
                self.error(
                    error_location.unwrap(),
                    &diag::PARAMETER_0_CANNOT_REFERENCE_ITSELF,
                    &[tsc_scanner::utilities::declaration_name_to_string(
                        self.store(),
                        self.store().node(associated).name(),
                    )],
                );
            } else {
                let candidate_value_declaration = self.program_symbol(candidate).value_declaration;
                let root_parent = binder_shims::parent_of(self.store(), root);
                let candidate_name = self.program_symbol(candidate).name.clone();
                if candidate_value_declaration.is_some_and(|vd| {
                    self.store().node(vd).pos() > self.store().node(associated).pos()
                }) && root_parent
                    .is_some_and(|rp| {
                        binder_node_data::locals_of(self.store(), rp)
                            .is_some_and(|locals| locals.get(&candidate_name) == Some(&candidate))
                    })
                {
                    self.error(
                        error_location.unwrap(),
                        &diag::PARAMETER_0_CANNOT_REFERENCE_IDENTIFIER_1_DECLARED_AFTER_IT,
                        &[
                            tsc_scanner::utilities::declaration_name_to_string(
                                self.store(),
                                self.store().node(associated).name(),
                            ),
                            tsc_scanner::utilities::declaration_name_to_string(
                                self.store(),
                                Some(error_location.unwrap()),
                            ),
                        ],
                    );
                }
            }
        }
        if meaning.intersects(SymbolFlags::VALUE)
            && self.program_symbol(result).flags.intersects(SymbolFlags::ALIAS)
            && !self.program_symbol(result).flags.intersects(SymbolFlags::VALUE)
            && let Some(error_location) = error_location
                .filter(|&e| !is_valid_type_only_alias_use_site(self.store(), e))
        {
            let type_only_declaration =
                self.get_type_only_alias_declaration_ex(result, SymbolFlags::VALUE);
            if let Some(type_only_declaration) = type_only_declaration {
                let message = match self.store().node(type_only_declaration).kind {
                    Kind::ExportSpecifier | Kind::ExportDeclaration | Kind::NamespaceExport => {
                        &diag::X_0_CANNOT_BE_USED_AS_A_VALUE_BECAUSE_IT_WAS_EXPORTED_USING_EXPORT_TYPE
                    }
                    _ => &diag::X_0_CANNOT_BE_USED_AS_A_VALUE_BECAUSE_IT_WAS_IMPORTED_USING_IMPORT_TYPE,
                };
                let diagnostic = self.error(error_location, message, std::slice::from_ref(&name));
                self.add_type_only_declaration_related_info(diagnostic, type_only_declaration, &name);
            }
        }
        // Look at 'compilerOptions.isolatedModules' and not 'getIsolatedModules(...)' (which considers 'verbatimModuleSyntax')
        // here because 'verbatimModuleSyntax' will already have an error for importing a type without 'import type'.
        if self.compiler_options.isolated_modules == Tristate::TSTrue
            && is_in_external_module
            && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE
        {
            let is_global = self
                .globals
                .get(&name)
                .is_some_and(|&s| s == result);
            let mut non_value_symbol = None;
            if is_global
                && last_location.is_some_and(|l| is_source_file(self.store().node(l)))
            {
                let locals = binder_node_data::locals_of(self.store(), last_location.unwrap())
                    .cloned()
                    .unwrap_or_default();
                non_value_symbol = locals
                    .entries()
                    .find(|(_, s)| !self.program_symbol(**s).flags.intersects(SymbolFlags::VALUE))
                    .map(|(_, s)| *s);
            }
            if let Some(non_value_symbol) = non_value_symbol {
                let declarations = self.program_symbol(non_value_symbol).declarations.clone();
                let import_decl = declarations.iter().copied().find(|&d| {
                    matches!(
                        self.store().node(d).kind,
                        Kind::ImportSpecifier | Kind::ImportClause | Kind::NamespaceImport | Kind::ImportEqualsDeclaration
                    )
                });
                if let Some(import_decl) = import_decl
                    && !crate::statements::is_type_only_import_or_export_declaration(
                        self.store(),
                        import_decl,
                    )
                {
                    self.error(
                        import_decl,
                        &diag::IMPORT_0_CONFLICTS_WITH_GLOBAL_VALUE_USED_IN_THIS_FILE_SO_MUST_BE_DECLARED_WITH_A_TYPE_ONLY_IMPORT_WHEN_ISOLATEDMODULES_IS_ENABLED,
                        std::slice::from_ref(&name),
                    );
                }
            }
        }
    }

    /// Go: `func (c *Checker) checkResolvedBlockScopedVariable(result, errorLocation)` (checker.go:1932).
    fn check_resolved_block_scoped_variable(&mut self, result: SymbolId, error_location: NodeId) {
        let flags = self.program_symbol(result).flags;
        if flags.intersects(
            SymbolFlags::FUNCTION
                .union(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
                .union(SymbolFlags::ASSIGNMENT),
        ) && flags.intersects(SymbolFlags::CLASS)
        {
            // constructor functions aren't block scoped
            return;
        }
        // Block-scoped variables cannot be used before their definition
        let declarations = self.program_symbol(result).declarations.clone();
        let declaration = declarations.iter().copied().find(|&d| {
            binder_shims::is_block_or_catch_scoped(self.store(), d)
                || binder_shims::is_class_like(self.store().node(d))
                || tsc_ast::is_enum_declaration(self.store().node(d))
        });
        let Some(declaration) = declaration else {
            panic!("checkResolvedBlockScopedVariable could not find block-scoped declaration")
        };
        if !self.store().node(declaration).flags.intersects(NF::AMBIENT)
            && !self.is_block_scoped_name_declared_before_use(declaration, error_location)
        {
            let declaration_name = tsc_scanner::utilities::declaration_name_to_string(
                self.store(),
                binder_shims::get_name_of_declaration(self.store(), declaration),
            );
            let mut diagnostic = None;
            if flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
                diagnostic = Some(self.error(
                    error_location,
                    &diag::BLOCK_SCOPED_VARIABLE_0_USED_BEFORE_ITS_DECLARATION,
                    std::slice::from_ref(&declaration_name),
                ));
            } else if flags.intersects(SymbolFlags::CLASS) {
                diagnostic = Some(self.error(
                    error_location,
                    &diag::CLASS_0_USED_BEFORE_ITS_DECLARATION,
                    std::slice::from_ref(&declaration_name),
                ));
            } else if flags.intersects(SymbolFlags::REGULAR_ENUM) {
                diagnostic = Some(self.error(
                    error_location,
                    &diag::ENUM_0_USED_BEFORE_ITS_DECLARATION,
                    std::slice::from_ref(&declaration_name),
                ));
            } else {
                debug_assert!(flags.intersects(SymbolFlags::CONST_ENUM));
                if self.compiler_options.get_isolated_modules() {
                    diagnostic = Some(self.error(
                        error_location,
                        &diag::ENUM_0_USED_BEFORE_ITS_DECLARATION,
                        std::slice::from_ref(&declaration_name),
                    ));
                }
            }
            if let Some(mut diagnostic) = diagnostic {
                diagnostic.add_related_info(u::new_diagnostic_for_node(
                    self.store(),
                    declaration,
                    &diag::X_0_IS_DECLARED_HERE,
                    vec![declaration_name],
                ));
            }
        }
    }

    /// Go: `func (c *Checker) isBlockScopedNameDeclaredBeforeUse(declaration, usage) bool`
    /// (checker.go:1966).
    pub(crate) fn is_block_scoped_name_declared_before_use(&mut self, declaration: NodeId, usage: NodeId) -> bool {
        let declaration_file = declaration.file_id();
        let use_file = usage.file_id();
        let decl_container = get_enclosing_block_scope_container(self.store(), declaration);
        if declaration_file != use_file {
            // nodes are in different files and order cannot be determined
            return true;
        }
        // deferred usage in a type context is always OK regardless of the usage position:
        if self.store().node(usage).flags.intersects(NF::JSDOC)
            || u::is_in_type_query(self.store(), usage)
            || self.is_in_ambient_or_type_node(usage)
        {
            return true;
        }
        if self.store().node(declaration).pos() <= self.store().node(usage).pos()
            && !(self.store().node(declaration).kind == Kind::PropertyDeclaration
                && binder_shims::parent_of(self.store(), usage)
                    .is_some_and(|p| u::is_this_property(self.store(), p))
                && crate::ast_shims::initializer_of(self.store(), declaration).is_none()
                && binder_node_data::postfix_token_of(self.store(), declaration).is_none())
        {
            // declaration is before usage
            match self.store().node(declaration).kind {
                Kind::BindingElement => {
                    // still might be illegal if declaration and usage are both binding elements (eg var [a = b, b = b] = [1, 2])
                    let error_binding_element = find_ancestor_kind(self.store(), usage, Kind::BindingElement);
                    if let Some(error_binding_element) = error_binding_element {
                        return binder_shims::find_ancestor(self.store(), error_binding_element, &mut |n| {
                            tsc_ast::is_binding_element(self.store().node(n))
                        })
                            != binder_shims::find_ancestor(self.store(), declaration, &mut |n| {
                                tsc_ast::is_binding_element(self.store().node(n))
                            })
                            || self.store().node(declaration).pos() < self.store().node(error_binding_element).pos();
                    }
                    // or it might be illegal if usage happens before parent variable is declared (eg var [a] = a)
                    let variable_declaration = find_ancestor_kind(self.store(), declaration, Kind::VariableDeclaration);
                    return variable_declaration
                        .is_some_and(|vd| self.is_block_scoped_name_declared_before_use(vd, usage));
                }
                Kind::VariableDeclaration => {
                    // still might be illegal if usage is in the initializer of the variable declaration (eg var a = a)
                    return !is_immediately_used_in_initializer_of_block_scoped_variable(
                        self.store(),
                        declaration,
                        usage,
                        decl_container,
                    );
                }
                _ if binder_shims::is_class_like(self.store().node(declaration)) => {
                    // still might be illegal if the usage is within a computed property name in the class (eg class A { static p = "a"; [A.p]() {} })
                    // or when used within a decorator in the class (e.g. `@dec(A.x) class A { static x = "x" }`),
                    // except when used in a function that is not an IIFE (e.g., `@dec(() => A.x) class A { ... }`)
                    let mut container = Some(usage);
                    while container.is_some_and(|c| c != declaration) {
                        let c = container.unwrap();
                        let parent = binder_shims::parent_of(self.store(), c);
                        let is_computed_member_of_class = parent.is_some_and(|p| {
                            tsc_ast::is_computed_property_name(self.store().node(c))
                                && binder_shims::parent_of(self.store(), p)
                                    .is_some_and(|gp| gp == declaration)
                        });
                        let is_decorator_of_class = !self.legacy_decorators
                            && self.store().node(c).kind == Kind::Decorator
                            && (parent.is_some_and(|p| p == declaration)
                                || parent.is_some_and(|p| {
                                    (self.store().node(p).kind == Kind::MethodDeclaration
                                        || is_accessor(self.store().node(p))
                                        || self.store().node(p).kind == Kind::PropertyDeclaration)
                                        && binder_shims::parent_of(self.store(), p)
                                            .is_some_and(|gp| gp == declaration)
                                })
                                || parent.is_some_and(|p| {
                                    self.store().node(p).kind == Kind::Parameter
                                        && binder_shims::parent_of(self.store(), p)
                                            .and_then(|gp| binder_shims::parent_of(self.store(), gp))
                                            .is_some_and(|gpp| gpp == declaration)
                                }));
                        if is_computed_member_of_class || is_decorator_of_class {
                            break;
                        }
                        container = parent;
                    }
                    let container = container.unwrap_or(declaration);
                    if container == declaration {
                        return true;
                    }
                    if !self.legacy_decorators && self.store().node(container).kind == Kind::Decorator {
                        let mut n = Some(usage);
                        while n.is_some_and(|x| x != container) {
                            if binder_shims::is_function_like(self.store().node(n.unwrap()))
                                && binder_shims::get_immediately_invoked_function_expression(
                                    self.store(),
                                    n.unwrap(),
                                )
                                .is_none()
                            {
                                break;
                            }
                            n = binder_shims::parent_of(self.store(), n.unwrap());
                        }
                        return n.is_some_and(|x| x != container);
                    }
                    return false;
                }
                _ if self.store().node(declaration).kind == Kind::PropertyDeclaration => {
                    // still might be illegal if a self-referencing property initializer (eg private x = this.x)
                    return !is_property_immediately_referenced_within_declaration(
                        self.store(),
                        declaration,
                        usage,
                        false,
                    );
                }
                _ if binder_shims::is_parameter_property_declaration(
                    self.store(),
                    declaration,
                    self.store().node(declaration).parent.get(),
                ) => {
                    // foo = this.bar is illegal in emitStandardClassFields when bar is a parameter property
                    return !(self.emit_standard_class_fields
                        && self.get_containing_class_of(declaration)
                            == self.get_containing_class_of(usage)
                        && self.is_used_in_function_or_instance_property(usage, declaration, decl_container));
                }
                _ => return true,
            }
        }
        // declaration is after usage, but it can still be legal if usage is deferred:
        // 1. inside an export specifier
        // 2. inside a function
        // 3. inside an instance property initializer, a reference to a non-instance property
        //    (except when emitStandardClassFields: true and the reference is to a parameter property)
        // 4. inside a static property initializer, a reference to a static method in the same class
        // 5. inside a TS export= declaration (since we will move the export statement during emit to avoid TDZ)
        let usage_parent = binder_shims::parent_of(self.store(), usage);
        if usage_parent.is_some_and(|p| {
            tsc_ast::is_export_specifier(self.store().node(p))
                || self.store().node(p).kind == Kind::ExportAssignment
                    && match &self.store().node(p).data {
                        tsc_ast::NodeData::ExportAssignment(d) => d.is_export_equals,
                        _ => false,
                    }
        }) {
            // export specifiers do not use the variable, they only make it available for use
            return true;
        }
        // When resolving symbols for exports, the `usage` location passed in can be the export site directly
        if self.store().node(usage).kind == Kind::ExportAssignment
            && match &self.store().node(usage).data {
                tsc_ast::NodeData::ExportAssignment(d) => d.is_export_equals,
                _ => false,
            }
        {
            return true;
        }
        if self.is_used_in_function_or_instance_property(usage, declaration, decl_container) {
            if self.emit_standard_class_fields
                && self.get_containing_class_of(declaration).is_some()
                && (self.store().node(declaration).kind == Kind::PropertyDeclaration
                    || binder_shims::is_parameter_property_declaration(
                        self.store(),
                        declaration,
                        self.store().node(declaration).parent.get(),
                    ))
            {
                return !is_property_immediately_referenced_within_declaration(
                    self.store(),
                    declaration,
                    usage,
                    true,
                );
            }
            return true;
        }
        false
    }

    /// Go: `func (c *Checker) isUsedInFunctionOrInstanceProperty(usage, declaration, declContainer) bool`
    /// (checker.go:2055).
    fn is_used_in_function_or_instance_property(
        &mut self,
        usage: NodeId,
        declaration: NodeId,
        decl_container: Option<NodeId>,
    ) -> bool {
        let mut current = Some(usage);
        while let Some(c) = current {
            if Some(c) == decl_container {
                return false; // FindAncestorQuit
            }
            if binder_shims::is_function_like(self.store().node(c)) {
                return binder_shims::get_immediately_invoked_function_expression(self.store(), c).is_none();
            }
            if self.store().node(c).kind == Kind::ClassStaticBlockDeclaration {
                return self.store().node(declaration).pos() < self.store().node(usage).pos();
            }
            let parent = binder_shims::parent_of(self.store(), c);
            if let Some(parent) =
                parent.filter(|&p| self.store().node(p).kind == Kind::PropertyDeclaration)
            {
                let initializer_of_property = crate::ast_shims::initializer_of(self.store(), parent) == Some(c);
                if initializer_of_property {
                    if binder_shims::is_static(self.store().node(parent)) {
                        if self.store().node(declaration).kind == Kind::MethodDeclaration {
                            return true;
                        }
                        if self.store().node(declaration).kind == Kind::PropertyDeclaration
                            && self.get_containing_class_of(usage)
                                == self.get_containing_class_of(declaration)
                        {
                            let prop_name = self.store().node(declaration).name();
                            if prop_name.is_some_and(|n| {
                                is_identifier(self.store().node(n))
                                    || tsc_ast::is_private_identifier(self.store().node(n))
                            }) {
                                let prop_name = prop_name.unwrap();
                                let symbol =
                                    self.get_symbol_of_declaration_merged(declaration).expect("symbol");
                                let t = self.get_type_of_symbol(symbol);
                                let class_parent = binder_shims::parent_of(self.store(), declaration)
                                    .expect("class parent");
                                let static_blocks = binder_node_data::members_of(self.store(), class_parent)
                                    .iter()
                                    .copied()
                                    .filter(|&m| self.store().node(m).kind == Kind::ClassStaticBlockDeclaration)
                                    .collect::<Vec<_>>();
                                if self.is_property_initialized_in_static_blocks(
                                    prop_name,
                                    t,
                                    &static_blocks,
                                    self.store().node(declaration).pos(),
                                    self.store().node(c).pos(),
                                ) {
                                    return true;
                                }
                            }
                        }
                    } else {
                        let is_declaration_instance_property =
                            self.store().node(declaration).kind == Kind::PropertyDeclaration
                                && !binder_shims::is_static(self.store().node(declaration));
                        if !is_declaration_instance_property
                            || self.get_containing_class_of(usage)
                                != self.get_containing_class_of(declaration)
                        {
                            return true;
                        }
                    }
                }
            }
            if let Some(parent) =
                parent.filter(|&p| self.store().node(p).kind == Kind::Decorator)
            {
                let decorator_expression = match &self.store().node(parent).data {
                    tsc_ast::NodeData::Decorator(d) => d.expression,
                    _ => NodeId::NONE,
                };
                if decorator_expression == c {
                    let decorator_parent = binder_shims::parent_of(self.store(), parent);
                    if let Some(dp) = decorator_parent {
                        if tsc_ast::is_parameter_declaration(self.store().node(dp)) {
                            if let Some(gppp) = binder_shims::parent_of(self.store(), dp)
                                .and_then(|gp| binder_shims::parent_of(self.store(), gp))
                            {
                                return self.is_used_in_function_or_instance_property(
                                    gppp,
                                    declaration,
                                    decl_container,
                                );
                            }
                            return false;
                        }
                        if self.store().node(dp).kind == Kind::MethodDeclaration {
                            if let Some(gpp) = binder_shims::parent_of(self.store(), dp) {
                                return self.is_used_in_function_or_instance_property(
                                    gpp,
                                    declaration,
                                    decl_container,
                                );
                            }
                            return false;
                        }
                    }
                }
            }
            current = parent;
        }
        false
    }

    /// Go: `ast.GetContainingClass(node)` composed for the
    /// isBlockScopedNameDeclaredBeforeUse arms (utilities.rs's free helper
    /// takes a store; this is the method form the arms use).
    fn get_containing_class_of(&self, node: NodeId) -> Option<NodeId> {
        crate::utilities::get_containing_class(self.store(), node)
    }

    /// Go: `func (c *Checker) isPropertyInitializedInStaticBlocks(propName, propType, staticBlocks, startPos, endPos) bool`
    /// (checker.go:5067 — the static-block scan this hook family needs).
    #[allow(clippy::too_many_arguments)]
    fn is_property_initialized_in_static_blocks(
        &mut self,
        prop_name: NodeId,
        prop_type: crate::types::TypeId,
        static_blocks: &[NodeId],
        start_pos: i32,
        end_pos: i32,
    ) -> bool {
        for &block in static_blocks {
            let body = match &self.store().node(block).data {
                tsc_ast::NodeData::ClassStaticBlockDeclaration(d) => d.body,
                _ => continue,
            };
            let stmts = binder_node_data::statements_of(self.store(), body)
                .map(|l| l.nodes.to_vec())
                .unwrap_or_default();
            for statement in stmts {
                if self.store().node(statement).kind == Kind::ExpressionStatement
                    && self.is_assignment_to_property_in_static_block(
                        statement,
                        prop_name,
                        prop_type,
                        start_pos,
                        end_pos,
                    )
                {
                    return true;
                }
            }
        }
        false
    }

    /// The per-statement arm of Go's isPropertyInitializedInStaticBlocks —
    /// `isAssignmentToProperty(expression, propName, propType)` over the
    /// static-block positions (checker.go:5067's inner walk).
    fn is_assignment_to_property_in_static_block(
        &mut self,
        statement: NodeId,
        prop_name: NodeId,
        prop_type: crate::types::TypeId,
        start_pos: i32,
        end_pos: i32,
    ) -> bool {
        let expression = match &self.store().node(statement).data {
            tsc_ast::NodeData::ExpressionStatement(d) => d.expression,
            _ => return false,
        };
        if !binder_shims::is_assignment_expression(self.store(), expression, false) {
            return false;
        }
        if self.store().node(expression).pos() < start_pos
            || self.store().node(expression).end() > end_pos
        {
            return false;
        }
        let (left, right) = match &self.store().node(expression).data {
            tsc_ast::NodeData::BinaryExpression(d) => (d.left, d.right),
            _ => return false,
        };
        if !binder_shims::is_assignment_expression(self.store(), right, true) {
            return false;
        }
        if self.store().node(left).name() != Some(prop_name) {
            return false;
        }
        // Go: `c.checkExpressionCached(right)` then compares to propType
        // through isTypeAssignableTo; the S8 flow of getWriteTypeOfSymbol is
        // not reachable here yet — the assignment expression's own type
        // check runs through the real expression spine.
        // TODO(port M6 W3): the flow-narrowed comparison; permissive true
        // when the expression's type is assignable (the common case).
        let _ = self.check_expression(right);
        let _ = prop_type;
        true
    }

    // ────────────────────────────────────────────────────────────────────────
    // Section A tail — the small scope-walk predicates (Go nameresolver.go
    // free functions; the binder port keeps them private, so they are
    // re-ported here).
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func isTypeParameterSymbolDeclaredInContainer(symbol, container) bool`.
    fn is_type_parameter_symbol_declared_in_container(&self, symbol: SymbolId, container: NodeId) -> bool {
        for &decl in &self.program_symbol(symbol).declarations {
            if self.store().node(decl).kind == Kind::TypeParameter
                && self.store().node(decl).parent.get() == container
            {
                return true;
            }
        }
        false
    }

    /// Go: `func (c *Checker) getSymbol(symbols, name, meaning) *ast.Symbol` (checker.go:2220).
    pub(crate) fn get_symbol(
        &mut self,
        symbols: &SymbolTable,
        name: &str,
        meaning: SymbolFlags,
    ) -> Option<SymbolId> {
        if !meaning.is_empty() {
            if let Some(&symbol) = symbols.get(name) {
                let symbol = self.get_merged_symbol(symbol);
                let flags = self.program_symbol(symbol).flags;
                if flags.intersects(meaning) {
                    return Some(symbol);
                }
                if flags.intersects(SymbolFlags::ALIAS) {
                    let target_flags = self.get_symbol_flags(symbol);
                    // `targetFlags` will be `SymbolFlags.All` if an error occurred in alias resolution; this avoids cascading errors
                    if target_flags.intersects(meaning) {
                        return Some(symbol);
                    }
                }
            }
        }
        // return nil if we can't find a symbol
        None
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Free helpers (Go nameresolver.go / utilities.go)
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func getIsDeferredContext(location, lastLocation) bool` (nameresolver.go).
fn get_is_deferred_context(store: &dyn NodeStore, location: NodeId, last_location: Option<NodeId>) -> bool {
    let kind = store.node(location).kind;
    if kind != Kind::ArrowFunction && kind != Kind::FunctionExpression {
        // initializers in instance property declaration of class like entities are executed in constructor and thus deferred
        // A name is evaluated within the enclosing scope - so it shouldn't count as deferred
        return tsc_ast::is_type_query_node(store.node(location))
            || (binder_shims::is_function_like_declaration(store.node(location))
                || kind == Kind::PropertyDeclaration && !binder_shims::is_static(store.node(location)))
                && (last_location.is_none() || last_location != store.node(location).name());
    }
    if last_location.is_some() && last_location == store.node(location).name() {
        return false;
    }
    // generator functions and async functions are not inlined in control flow when immediately invoked
    if binder_node_data::asterisk_token_of(store, location).is_some()
        || has_syntactic_modifier(store.node(location), ModifierFlags::ASYNC)
    {
        return true;
    }
    binder_shims::get_immediately_invoked_function_expression(store, location).is_none()
}

/// Go: `func isSelfReferenceLocation(node, lastLocation) bool` (nameresolver.go).
fn is_self_reference_location(node: &Node, last_location: Option<NodeId>) -> bool {
    match node.kind {
        Kind::Parameter => last_location.is_some() && last_location == node.name(),
        Kind::FunctionDeclaration
        | Kind::ClassDeclaration
        | Kind::InterfaceDeclaration
        | Kind::EnumDeclaration
        | Kind::TypeAliasDeclaration
        | Kind::JSTypeAliasDeclaration
        | Kind::ModuleDeclaration => {
            // For `namespace N { N; }`
            true
        }
        _ => false,
    }
}

/// Go: `func isPrimitiveTypeName(s string) bool` (checker.go:1681).
fn is_primitive_type_name(s: &str) -> bool {
    s == "any" || s == "string" || s == "number" || s == "boolean" || s == "never" || s == "unknown"
}

/// Go: `func isES2015OrLaterConstructorName(s string) bool` (checker.go:1748).
fn is_es2015_or_later_constructor_name(s: &str) -> bool {
    s == "Promise" || s == "Symbol" || s == "Map" || s == "WeakMap" || s == "Set" || s == "WeakSet"
}

/// Go: `func isImmediatelyUsedInInitializerOfBlockScopedVariable(declaration, usage, declContainer) bool`
/// (checker.go:2116).
fn is_immediately_used_in_initializer_of_block_scoped_variable(
    store: &dyn NodeStore,
    declaration: NodeId,
    usage: NodeId,
    decl_container: Option<NodeId>,
) -> bool {
    let grandparent = binder_shims::parent_of(store, declaration)
        .and_then(|p| binder_shims::parent_of(store, p));
    let grandparent_kind = grandparent.map(|g| store.node(g).kind);
    if matches!(
        grandparent_kind,
        Some(Kind::VariableStatement) | Some(Kind::ForStatement) | Some(Kind::ForOfStatement)
    ) {
        // variable statement/for/for-of statement case,
        // use site should not be inside variable declaration (initializer of declaration or binding element)
        if is_same_scope_descendent_of(store, usage, Some(declaration), decl_container) {
            return true;
        }
    }
    // ForIn/ForOf case - use site should not be used in expression part
    grandparent.is_some_and(|g| {
        binder_shims::is_for_in_or_of_statement(store.node(g))
            && crate::ast_shims::initializer_of(store, g).is_some_and(|expr| {
                is_same_scope_descendent_of(store, usage, Some(expr), decl_container)
            })
    })
}

/// Go: `func isSameScopeDescendentOf(initial, parent, stopAt) bool` (checker.go:2134).
fn is_same_scope_descendent_of(
    store: &dyn NodeStore,
    initial: NodeId,
    parent: Option<NodeId>,
    stop_at: Option<NodeId>,
) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    let mut n = Some(initial);
    while let Some(current) = n {
        if current == parent {
            return true;
        }
        let is_function_like_stop = binder_shims::is_function_like(store.node(current))
            && (binder_shims::get_immediately_invoked_function_expression(store, current).is_none()
                || tsc_ast::functionflags::get_function_flags(store, current)
                    .intersects(tsc_ast::functionflags::FunctionFlags::ASYNC_GENERATOR));
        if Some(current) == stop_at || is_function_like_stop {
            return false;
        }
        n = binder_shims::parent_of(store, current);
    }
    false
}

/// Go: `func isPropertyImmediatelyReferencedWithinDeclaration(declaration, usage, stopAtAnyPropertyDeclaration) bool`
/// (checker.go:2150).
fn is_property_immediately_referenced_within_declaration(
    store: &dyn NodeStore,
    declaration: NodeId,
    usage: NodeId,
    stop_at_any_property_declaration: bool,
) -> bool {
    // always legal if usage is after declaration
    if store.node(usage).end() > store.node(declaration).end() {
        return false;
    }
    // still might be legal if usage is deferred (e.g. x: any = () => this.x)
    // otherwise illegal if immediately referenced within the declaration (e.g. x: any = this.x)
    let mut node = Some(usage);
    while node.is_some_and(|n| n != declaration) {
        let n = node.unwrap();
        match store.node(n).kind {
            Kind::ArrowFunction => return false,
            Kind::PropertyDeclaration => {
                // even when stopping at any property declaration, they need to come from the same class
                let declaration_parent = binder_shims::parent_of(store, declaration);
                let node_parent = binder_shims::parent_of(store, n);
                return stop_at_any_property_declaration
                    && ((store.node(declaration).kind == Kind::PropertyDeclaration
                        && node_parent == declaration_parent)
                        || (binder_shims::is_parameter_property_declaration(
                            store,
                            declaration,
                            store.node(declaration).parent.get(),
                        ) && declaration_parent.is_some_and(|dp| {
                            binder_shims::parent_of(store, dp) == node_parent
                        })));
            }
            Kind::Block => {
                let parent_kind = binder_shims::parent_of(store, n).map(|p| store.node(p).kind);
                if matches!(
                    parent_kind,
                    Some(Kind::MethodDeclaration) | Some(Kind::GetAccessor) | Some(Kind::SetAccessor)
                ) {
                    return false;
                }
            }
            _ => {}
        }
        node = binder_shims::parent_of(store, n);
    }
    true
}

// ────────────────────────────────────────────────────────────────────────────
// Local ast-package helpers (Go ast/utilities.go + checker.go probes the
// slice needs; none of the landed crates exports them yet — they fold into
// the owning crate's utilities wave when it lands).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func ast.GetEnclosingBlockScopeContainer(node) *Node` (ast/utilities.go)
/// — `FindAncestor(node.Parent, IsBlockScope)`.
pub(crate) fn get_enclosing_block_scope_container(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    let mut current = binder_shims::parent_of(store, node)?;
    loop {
        let parent = binder_shims::parent_of(store, current);
        // Go: IsBlockScope(current, current.Parent) — IsLocalsContainer, or a
        // module block inside a module declaration, or an interface.
        let is_block_scope = binder_node_data::is_locals_container(store.node(current))
            || (store.node(current).kind == Kind::ModuleBlock
                && parent.is_some_and(|p| store.node(p).kind == Kind::ModuleDeclaration))
            || store.node(current).kind == Kind::InterfaceDeclaration;
        if is_block_scope {
            return Some(current);
        }
        current = parent?;
    }
}

/// Go: `func ast.FindAncestorKind(node, kind) *Node` (ast/utilities.go).
fn find_ancestor_kind(store: &dyn NodeStore, node: NodeId, kind: Kind) -> Option<NodeId> {
    binder_shims::find_ancestor(store, node, &mut |n| store.node(n).kind == kind)
}

/// Go: `func ast.IsAccessor(node) bool` (ast/astnav — the two-kind probe).
fn is_accessor(node: &Node) -> bool {
    node.kind == Kind::GetAccessor || node.kind == Kind::SetAccessor
}

/// Go: `func ast.IsValidTypeOnlyAliasUseSite(useSite) bool` (ast/utilities.go:3200).
/// PORT (FCS tranche 44): the real predicate (was the TODO(port M6 S6
/// remainder) permissive-true stub that kept the TS1361 "imported using
/// import type" arm in onSuccessfullyResolvedSymbol dormant).
fn is_valid_type_only_alias_use_site(store: &dyn NodeStore, use_site: NodeId) -> bool {
    let node = store.node(use_site);
    node.flags.intersects(NF::AMBIENT.union(NF::JSDOC))
        || binder_shims::is_part_of_type_query(store, use_site)
        || is_identifier_in_non_emitting_heritage_clause(store, use_site)
        || is_part_of_possibly_valid_type_or_abstract_computed_property_name(store, use_site)
        || !(crate::grammarchecks::is_expression_node(store, use_site)
            || is_shorthand_property_name_use_site(store, use_site))
}

/// Go: `func ast.isIdentifierInNonEmittingHeritageClause(node) bool` (ast/utilities.go:3208).
fn is_identifier_in_non_emitting_heritage_clause(store: &dyn NodeStore, node: NodeId) -> bool {
    if !is_identifier(store.node(node)) {
        return false;
    }
    let mut current = binder_shims::parent_of(store, node);
    while let Some(parent) = current {
        if matches!(
            store.node(parent).kind,
            Kind::PropertyAccessExpression | Kind::ExpressionWithTypeArguments
        ) {
            current = binder_shims::parent_of(store, parent);
        } else {
            break;
        }
    }
    let Some(hc) = current else { return false };
    let hc_node = store.node(hc);
    if !tsc_ast::is_heritage_clause(hc_node) {
        return false;
    }
    let token_is_implements = hc_node
        .as_heritage_clause()
        .is_some_and(|d| d.token == Kind::ImplementsKeyword);
    token_is_implements
        || binder_shims::parent_of(store, hc)
            .is_some_and(|gp| tsc_ast::is_interface_declaration(store.node(gp)))
}

/// Go: `func ast.isPartOfPossiblyValidTypeOrAbstractComputedPropertyName(node) bool`
/// (ast/utilities.go:3219).
fn is_part_of_possibly_valid_type_or_abstract_computed_property_name(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let mut current = node;
    while matches!(
        store.node(current).kind,
        Kind::Identifier | Kind::PropertyAccessExpression
    ) {
        let Some(parent) = binder_shims::parent_of(store, current) else {
            return false;
        };
        current = parent;
    }
    if store.node(current).kind != Kind::ComputedPropertyName {
        return false;
    }
    let Some(parent) = binder_shims::parent_of(store, current) else {
        return false;
    };
    if has_syntactic_modifier(store.node(parent), ModifierFlags::ABSTRACT) {
        return true;
    }
    binder_shims::parent_of(store, parent).is_some_and(|gp| {
        matches!(
            store.node(gp).kind,
            Kind::InterfaceDeclaration | Kind::TypeLiteral
        )
    })
}

/// Go: `func ast.isShorthandPropertyNameUseSite(useSite) bool` (ast/utilities.go:3232).
fn is_shorthand_property_name_use_site(store: &dyn NodeStore, use_site: NodeId) -> bool {
    is_identifier(store.node(use_site))
        && binder_shims::parent_of(store, use_site).is_some_and(|parent| {
            let parent_node = store.node(parent);
            tsc_ast::is_shorthand_property_assignment(parent_node)
                && parent_node
                    .as_shorthand_property_assignment()
                    .is_some_and(|d| d.name == use_site)
        })
}

// ════════════════════════════════════════════════════════════════════════════
// Section B — the alias/module family (Go checker.go 14472–16750)
// ════════════════════════════════════════════════════════════════════════════

impl Checker {
    /// Go: `func (c *Checker) getMergedSymbol(symbol *ast.Symbol) *ast.Symbol`
    /// (checker.go:14624). Real body over the mergedSymbols table.
    pub(crate) fn get_merged_symbol_real(&self, symbol: SymbolId) -> SymbolId {
        if let Some(&merged) = self.merged_symbols.get(&symbol) {
            return merged;
        }
        symbol
    }

    /// Go: `func (c *Checker) recordMergedSymbol(target, source) *ast.Symbol`
    /// (checker.go:14638).
    pub(crate) fn record_merged_symbol(&mut self, target: SymbolId, source: SymbolId) {
        self.merged_symbols.insert(source, target);
    }

    /// Go: `func (c *Checker) getParentOfSymbol(symbol) *ast.Symbol` (checker.go:14630).
    pub(crate) fn get_parent_of_symbol(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        let parent = self.program_symbol(symbol).parent?;
        let late_bound = self.get_late_bound_symbol(parent);
        Some(self.get_merged_symbol(late_bound))
    }

    /// Go: `func (c *Checker) getResolvedTarget(symbol) *ast.Symbol` (checker.go:14641).
    pub(crate) fn get_resolved_target(&mut self, symbol: SymbolId) -> SymbolId {
        let merged = self.get_merged_symbol(symbol);
        let resolved = self.resolve_symbol(merged);
        self.get_merged_symbol(resolved)
    }

    /// Go: `func (c *Checker) getSymbolIfSameReference(s1, s2) *ast.Symbol` (checker.go:14645).
    pub(crate) fn get_symbol_if_same_reference(&mut self, s1: SymbolId, s2: SymbolId) -> Option<SymbolId> {
        if self.get_resolved_target(s1) == self.get_resolved_target(s2) {
            Some(s1)
        } else {
            None
        }
    }

    /// Go: `func (c *Checker) getSymbolOfDeclaration(node) *ast.Symbol` (checker.go:14659)
    /// — the merged+late-bound composition. PORT: Go's name collides with the
    /// exports.rs public twin (GetSymbolOfDeclaration); the internal body is
    /// named `..._merged` (the landed call sites keep their carrier).
    ///
    /// PORT(FCS tranche 14 — the merged-symbol sweep): Go has ONE
    /// `getSymbolOfDeclaration` (merged+late-bound at every call site), but
    /// also reads the raw `node.Symbol()` payload inline in many bodies.
    /// The per-site audit (Go twin body: `getSymbolOfDeclaration(` method
    /// call vs `.Symbol()` raw read) flipped ~60 Rust sites to this merged
    /// twin (checker.go citations per site: checkClassLikeDeclaration,
    /// checkObjectTypeForDuplicateDeclarations, checkAliasSymbol 6938,
    /// getSignatureFromDeclaration's accessor arm 20288, isSomeSymbol
    /// Assigned 32025, getSymbolsInScope 87/89/104,
    /// getReturnTypeFromAnnotation 20471, checkUnusedClassMembers 7324,
    /// and the rest of the audit table); sites where Go reads raw
    /// `.Symbol()` (getSignatureFromDeclaration's param symbols 20260/20269,
    /// checkParameter, createUnionOrIntersectionProperty,
    /// markPropertyAsReferenced, getTypeFromTypeLiteral..., the
    /// typeofsymbol.rs worker's file-symbol read, etc.) deliberately keep
    /// the raw carrier — mirror Go, not a blanket swap.
    pub(crate) fn get_symbol_of_declaration_merged(&mut self, node: NodeId) -> Option<SymbolId> {
        let symbol = crate::typeofsymbol::get_symbol_of_declaration(self, node)?;
        let late_bound = self.get_late_bound_symbol(symbol);
        Some(self.get_merged_symbol(late_bound))
    }

    /// Go: `func (c *Checker) getSymbolOfNode(node) *ast.Symbol` (checker.go:14682)
    /// — `node.DeclarationData().Symbol` then merged+late-bound.
    pub(crate) fn get_symbol_of_node_real(&mut self, node: NodeId) -> Option<SymbolId> {
        let symbol = crate::typeofsymbol::payload_symbol(self.store().node(node))?;
        let late_bound = self.get_late_bound_symbol(symbol);
        Some(self.get_merged_symbol(late_bound))
    }

    /// Go: `func (c *Checker) getLateBoundSymbol(symbol) *ast.Symbol` (checker.go:14694).
    pub(crate) fn get_late_bound_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        let (flags, name, declarations) = {
            let s = self.program_symbol(symbol);
            (s.flags, s.name.clone(), s.declarations.clone())
        };
        if !flags.intersects(SymbolFlags::CLASS_MEMBER)
            || name != tsc_ast::INTERNAL_SYMBOL_NAME_COMPUTED
        {
            return symbol;
        }
        if self.late_bound_links.get(symbol).late_symbol.is_none()
            && declarations
                .iter()
                .any(|&d| self.has_late_bindable_name(d))
        {
            // force late binding of members/exports. This will set the late-bound symbol
            let parent = self.get_merged_symbol(
                self.program_symbol(symbol).parent.unwrap_or(symbol),
            );
            if declarations
                .iter()
                .any(|&d| binder_shims::has_static_modifier(self.store().node(d)))
            {
                self.get_exports_of_symbol(parent);
            } else {
                self.get_members_of_symbol(parent);
            }
        }
        if self.late_bound_links.get(symbol).late_symbol.is_none() {
            self.late_bound_links.get(symbol).late_symbol = Some(symbol);
        }
        self.late_bound_links.get(symbol).late_symbol.unwrap()
    }

    /// Go: `func (c *Checker) resolveSymbol(symbol) *ast.Symbol` (checker.go:14755).
    pub(crate) fn resolve_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        self.resolve_symbol_ex(symbol, false)
    }

    /// Go: `func (c *Checker) resolveSymbolEx(symbol, dontResolveAlias) *ast.Symbol` (checker.go:14759).
    pub(crate) fn resolve_symbol_ex(&mut self, symbol: SymbolId, dont_resolve_alias: bool) -> SymbolId {
        if !dont_resolve_alias
            && is_non_local_alias(
                self.program_symbol(symbol),
                SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
            )
        {
            return self.resolve_alias(symbol);
        }
        symbol
    }

    /// Go: `func (c *Checker) getDeclarationOfKind`-shaped read over the
    /// symbol's declarations (ast.GetDeclarationOfKind).
    pub(crate) fn get_declaration_of_kind_of_symbol(&self, symbol: SymbolId, kind: Kind) -> Option<NodeId> {
        self.program_symbol(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| self.store().node(d).kind == kind)
    }

    // ── resolveAlias + the getTargetOf* family ──

    /// Go: `func (c *Checker) resolveAlias(symbol) *ast.Symbol` (checker.go:16652).
    pub(crate) fn resolve_alias_real(&mut self, symbol: SymbolId) -> SymbolId {
        debug_assert!(
            self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS),
            "Should only get alias here"
        );
        if self.alias_symbol_links.get(symbol).alias_target.is_some() {
            return self.alias_symbol_links.get(symbol).alias_target.unwrap();
        }
        if !self.push_type_resolution(
            TypeSystemEntity::Symbol(symbol),
            TypeSystemPropertyName::AliasTarget,
        ) {
            return self.unknown_symbol();
        }
        let node = self.get_declaration_of_alias_symbol(symbol);
        let Some(node) = node else {
            panic!(
                "Unexpected nil in resolveAlias for symbol: {}",
                crate::relater::symbol_to_string(self, symbol)
            );
        };
        let target = self.get_target_of_alias_declaration(Some(node));
        let target = match target {
            Some(target)
                if is_non_local_alias(
                    self.program_symbol(target),
                    SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                ) =>
            {
                // When the target is a pure alias, we transitively resolve and propagate any typeOnlyDeclaration
                Some(self.resolve_indirection_alias(symbol, target))
            }
            _ => target,
        };
        let target = target.unwrap_or_else(|| self.unknown_symbol());
        self.alias_symbol_links.get(symbol).alias_target = Some(target);
        if !self.pop_type_resolution() {
            let name = crate::relater::symbol_to_string(self, symbol);
            self.error(node, &diag::CIRCULAR_DEFINITION_OF_IMPORT_ALIAS_0, &[name]);
            self.alias_symbol_links.get(symbol).alias_target = Some(self.unknown_symbol());
        }
        self.alias_symbol_links.get(symbol).alias_target.unwrap()
    }

    /// Go: `func (c *Checker) resolveIndirectionAlias(source, target) *ast.Symbol` (checker.go:16679).
    pub(crate) fn resolve_indirection_alias(&mut self, source: SymbolId, target: SymbolId) -> SymbolId {
        let resolved = self.resolve_alias_real(target);
        let result = self.get_merged_symbol(resolved);
        if self.alias_symbol_links.get(target).type_only_declaration.is_some()
            && self.alias_symbol_links.get(source).type_only_declaration.is_none()
        {
            let type_only = self.alias_symbol_links.get(target).type_only_declaration;
            self.alias_symbol_links.get(source).type_only_declaration = type_only;
        }
        result
    }

    /// Go: `func (c *Checker) tryResolveAlias(symbol) *ast.Symbol` (checker.go:16689).
    #[allow(dead_code)] // the spelling-suggestion probe lands with the S6 remainder
    pub(crate) fn try_resolve_alias(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        if self.alias_symbol_links.get(symbol).alias_target.is_some()
            || !self.has_in_flight_alias_resolution(symbol)
        {
            return Some(self.resolve_alias_real(symbol));
        }
        None
    }

    /// The findResolutionCycleStartIndex probe tryResolveAlias needs (Go
    /// checker.go:16692's `< 0` check); checker.rs's detector is private, so
    /// the scan is re-read off the resolution stack.
    fn has_in_flight_alias_resolution(&mut self, symbol: SymbolId) -> bool {
        let mut i = self.type_resolutions.len() as isize - 1;
        while i >= self.resolution_start as isize {
            let resolution = self.type_resolutions[i as usize];
            if resolution.target == TypeSystemEntity::Symbol(symbol)
                && resolution.property_name == TypeSystemPropertyName::AliasTarget
            {
                return true;
            }
            i -= 1;
        }
        false
    }

    /// Go: `func (c *Checker) resolveAliasWithDeprecationCheck(symbol, location) *ast.Symbol`
    /// (checker.go:16697).
    pub(crate) fn resolve_alias_with_deprecation_check_real(
        &mut self,
        symbol: SymbolId,
        location: NodeId,
    ) -> SymbolId {
        if !self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS)
            || self.is_deprecated_symbol_stub(symbol)
            || self.get_declaration_of_alias_symbol(symbol).is_none()
        {
            return symbol;
        }
        let target_symbol = self.resolve_alias_real(symbol);
        if target_symbol == self.unknown_symbol() {
            return target_symbol;
        }
        let mut symbol = symbol;
        while self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS) {
            if let Some(target) = self.get_immediate_aliased_symbol(symbol) {
                if target == target_symbol {
                    break;
                }
                if !self.program_symbol(target).declarations.is_empty() {
                    // TODO(port M6 S6 remainder): isDeprecatedSymbol +
                    // addDeprecatedSuggestion — the @deprecated JSDoc walk;
                    // permissive not-deprecated keeps the suggestion dormant.
                    if self.is_deprecated_symbol_stub(target) || symbol == target_symbol {
                        break;
                    } else {
                        symbol = target;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        let _ = location;
        target_symbol
    }

    /// Go: `func (c *Checker) isDeprecatedSymbol(symbol) bool` (checker.go:14325)
    /// — TODO(port M6 S6 remainder): the JSDoc @deprecated scan; permissive
    /// false (no symbol is deprecated) keeps every deprecation arm dormant.
    fn is_deprecated_symbol_stub(&mut self, symbol: SymbolId) -> bool {
        let _ = symbol;
        false
    }

    /// Go: `func (c *Checker) getImmediateAliasedSymbol(symbol) *ast.Symbol` (checker.go:2199).
    pub(crate) fn get_immediate_aliased_symbol_real(&mut self, symbol: SymbolId) -> Option<SymbolId> {
        if self.alias_symbol_links.get(symbol).immediate_target.is_none() {
            let node = self.get_declaration_of_alias_symbol(symbol)?;
            let target = self.get_target_of_alias_declaration(Some(node));
            self.alias_symbol_links.get(symbol).immediate_target = target;
        }
        self.alias_symbol_links.get(symbol).immediate_target
    }

    /// Go: `func (c *Checker) getTypeOnlyAliasDeclaration(symbol) *ast.Node` (checker.go:2177).
    pub(crate) fn get_type_only_alias_declaration_real(&mut self, symbol: SymbolId) -> Option<NodeId> {
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS) {
            self.resolve_alias_real(symbol);
            return self.alias_symbol_links.get(symbol).type_only_declaration;
        }
        None
    }

    /// Go: `func (c *Checker) getTypeOnlyAliasDeclarationEx(symbol, meaning) *ast.Node` (checker.go:2187).
    pub(crate) fn get_type_only_alias_declaration_ex(
        &mut self,
        mut symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Option<NodeId> {
        while self.program_symbol(symbol).flags.intersects(SymbolFlags::ALIAS)
            && !self.program_symbol(symbol).flags.intersects(meaning)
        {
            let resolved = self.resolve_alias_real(symbol);
            let type_only = self.alias_symbol_links.get(symbol).type_only_declaration;
            if type_only.is_some() {
                return type_only;
            }
            symbol = resolved;
        }
        None
    }

    /// Go: `func (c *Checker) addTypeOnlyDeclarationRelatedInfo(diagnostic, typeOnlyDeclaration, name) *ast.Diagnostic`
    /// (checker.go:2212).
    pub(crate) fn add_type_only_declaration_related_info(
        &mut self,
        mut diagnostic: tsc_ast::Diagnostic,
        type_only_declaration: NodeId,
        name: &str,
    ) -> tsc_ast::Diagnostic {
        let kind = self.store().node(type_only_declaration).kind;
        let is_export = matches!(
            kind,
            Kind::ExportSpecifier | Kind::ExportDeclaration | Kind::NamespaceExport
        );
        let message = if is_export {
            &diag::X_0_WAS_EXPORTED_HERE
        } else {
            &diag::X_0_WAS_IMPORTED_HERE
        };
        diagnostic.add_related_info(u::new_diagnostic_for_node(
            self.store(),
            type_only_declaration,
            message,
            vec![name.to_string()],
        ));
        diagnostic
    }

    /// Go: `func (c *Checker) markSymbolOfAliasDeclarationIfTypeOnly(aliasDeclaration, exportStarDeclaration) bool`
    /// (checker.go:15377).
    pub(crate) fn mark_symbol_of_alias_declaration_if_type_only(
        &mut self,
        alias_declaration: Option<NodeId>,
        export_star_declaration: Option<NodeId>,
    ) -> bool {
        let Some(alias_declaration) = alias_declaration.filter(|&n| is_declaration_node(self.store(), n)) else {
            return false;
        };
        // If the declaration itself is type-only, mark it and return. No need to check what it resolves to.
        let Some(source_symbol) = self.get_symbol_of_declaration_merged(alias_declaration) else {
            return false;
        };
        if self.alias_symbol_links.get(source_symbol).type_only_declaration.is_none()
            && crate::statements::is_type_only_import_or_export_declaration(
                self.store(),
                alias_declaration,
            )
        {
            self.alias_symbol_links.get(source_symbol).type_only_declaration = Some(alias_declaration);
            return true;
        }
        if self.alias_symbol_links.get(source_symbol).type_only_declaration.is_none()
            && let Some(export_star_declaration) = export_star_declaration
        {
            self.alias_symbol_links.get(source_symbol).type_only_declaration =
                Some(export_star_declaration);
            return true;
        }
        self.alias_symbol_links.get(source_symbol).type_only_declaration.is_some()
    }

    /// Go: `func (c *Checker) getTargetOfAliasDeclaration(node) *ast.Symbol` (checker.go:16122).
    pub(crate) fn get_target_of_alias_declaration(&mut self, node: Option<NodeId>) -> Option<SymbolId> {
        let node = node?;
        match self.store().node(node).kind {
            Kind::ImportEqualsDeclaration | Kind::VariableDeclaration => {
                self.get_target_of_import_equals_declaration(node)
            }
            Kind::ImportClause => self.get_target_of_import_clause(node),
            Kind::NamespaceImport => self.get_target_of_namespace_import(node),
            Kind::NamespaceExport => self.get_target_of_namespace_export(node),
            Kind::ImportSpecifier | Kind::BindingElement => {
                self.get_target_of_import_specifier(node)
            }
            Kind::ExportSpecifier => self.get_target_of_export_specifier(
                node,
                SymbolFlags::VALUE.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE),
                true,
            ),
            Kind::ExportAssignment => self.get_target_of_export_assignment(node),
            Kind::BinaryExpression => self.get_target_of_binary_expression(node),
            Kind::NamespaceExportDeclaration => {
                self.get_target_of_namespace_export_declaration(node)
            }
            Kind::ShorthandPropertyAssignment => {
                let name = self.store().node(node).name().expect("shorthand name");
                self.resolve_entity_name(
                    name,
                    SymbolFlags::VALUE.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE),
                    true,
                    true,
                    NodeId::NONE,
                )
            }
            Kind::PropertyAssignment => {
                let initializer = crate::ast_shims::initializer_of(self.store(), node)?;
                self.get_target_of_alias_like_expression(initializer)
            }
            Kind::ElementAccessExpression | Kind::PropertyAccessExpression => {
                self.get_target_of_access_expression(node)
            }
            _ => panic!(
                "Unhandled case in getTargetOfAliasDeclaration: {}",
                self.store().node(node).kind_string()
            ),
        }
    }

    /// Go: `func (c *Checker) getTargetOfImportEqualsDeclaration(node) *ast.Symbol` (checker.go:14790).
    pub(crate) fn get_target_of_import_equals_declaration(&mut self, node: NodeId) -> Option<SymbolId> {
        // Node is ImportEqualsDeclaration | VariableDeclaration
        let module_reference = match &self.store().node(node).data {
            tsc_ast::NodeData::ImportEqualsDeclaration(d) => Some(d.module_reference),
            tsc_ast::NodeData::VariableDeclaration(_) => None,
            _ => return None,
        };
        let is_external_module_reference = module_reference
            .is_some_and(|m| self.store().node(m).kind == Kind::ExternalModuleReference);
        if self.store().node(node).kind == Kind::VariableDeclaration || is_external_module_reference {
            let mut module_reference = get_external_module_require_argument(self.store(), node);
            if module_reference.is_none() {
                module_reference =
                    get_external_module_import_equals_declaration_expression(self.store(), node);
            }
            let immediate = self.resolve_external_module_name_ex(node, module_reference, false, None);
            let resolved = self.resolve_external_module_symbol_ex(immediate, true);
            if let Some(resolved) = resolved
                && tsc_core::options_generated::ModuleKind::Node20 <= self.module_kind
                    && self.module_kind <= tsc_core::options_generated::ModuleKind::NodeNext
            {
                let module_exports = self.get_export_of_module(
                    resolved,
                    tsc_ast::INTERNAL_SYMBOL_NAME_MODULE_EXPORTS,
                    node,
                    true,
                );
                if let Some(module_exports) = module_exports {
                    return Some(module_exports);
                }
            }
            self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
            return resolved;
        }
        let module_reference = module_reference.expect("ImportEqualsDeclaration moduleReference");
        let resolved =
            self.get_symbol_of_part_of_right_hand_side_of_import_equals(module_reference);
        self.check_and_report_error_for_resolving_import_alias_to_type_only_symbol(node, resolved);
        resolved
    }

    /// Go: `func (c *Checker) getSymbolOfPartOfRightHandSideOfImportEquals(entityName) *ast.Symbol`
    /// (checker.go:14767). This function is only for imports with entity names.
    pub(crate) fn get_symbol_of_part_of_right_hand_side_of_import_equals(
        &mut self,
        mut entity_name: NodeId,
    ) -> Option<SymbolId> {
        // There are three things we might try to look for. In the following examples,
        // the search term is enclosed in |...|:
        //
        //     import a = |b|; // Namespace
        //     import a = |b.c|; // Value, type, namespace
        //     import a = |b.c|.d; // Namespace
        if is_identifier(self.store().node(entity_name))
            && is_right_side_of_qualified_name_or_property_access(self.store(), entity_name)
        {
            entity_name = self.store().node(entity_name).parent.get();
        }
        // Check for case 1 and 3 in the above example
        if is_identifier(self.store().node(entity_name))
            || self
                .store()
                .node(self.store().node(entity_name).parent.get())
                .kind
                == Kind::QualifiedName
        {
            return self.resolve_entity_name(
                entity_name,
                SymbolFlags::NAMESPACE,
                false,
                true,
                NodeId::NONE,
            );
        }
        // Case 2 in above example
        // entityName.kind could be a QualifiedName or a Missing identifier
        debug_assert!(
            self.store().node(self.store().node(entity_name).parent.get()).kind
                == Kind::ImportEqualsDeclaration
        );
        self.resolve_entity_name(
            entity_name,
            SymbolFlags::VALUE.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE),
            false,
            true,
            NodeId::NONE,
        )
    }

    /// Go: `func (c *Checker) checkAndReportErrorForResolvingImportAliasToTypeOnlySymbol(node, resolved)`
    /// (checker.go:14805).
    fn check_and_report_error_for_resolving_import_alias_to_type_only_symbol(
        &mut self,
        node: NodeId,
        _resolved: Option<SymbolId>,
    ) {
        let (module_reference, _) = match &self.store().node(node).data {
            tsc_ast::NodeData::ImportEqualsDeclaration(d) => (d.module_reference, ()),
            _ => return,
        };
        let mut name = module_reference;
        loop {
            if let Some(type_only_declaration) = self.get_type_only_declaration_of_entity_name(name)
            {
                let is_export = matches!(
                    self.store().node(type_only_declaration).kind,
                    Kind::ExportSpecifier | Kind::ExportDeclaration
                );
                let (message, related_message) = if is_export {
                    (
                        &diag::AN_IMPORT_ALIAS_CANNOT_REFERENCE_A_DECLARATION_THAT_WAS_EXPORTED_USING_EXPORT_TYPE,
                        &diag::X_0_WAS_EXPORTED_HERE,
                    )
                } else {
                    (
                        &diag::AN_IMPORT_ALIAS_CANNOT_REFERENCE_A_DECLARATION_THAT_WAS_IMPORTED_USING_IMPORT_TYPE,
                        &diag::X_0_WAS_IMPORTED_HERE,
                    )
                };
                // TODO: how to get name for export *?
                let mut related_name = "*".to_string();
                if self.store().node(type_only_declaration).kind != Kind::ExportDeclaration {
                    related_name = binder_shims::node_text(
                        self.store(),
                        self.store().node(type_only_declaration).name().unwrap_or(type_only_declaration),
                    )
                    .to_string();
                }
                let mut diagnostic = self.error(
                    module_reference,
                    message,
                    &[],
                );
                diagnostic.add_related_info(u::new_diagnostic_for_node(
                    self.store(),
                    type_only_declaration,
                    related_message,
                    vec![related_name],
                ));
                let _ = diagnostic;
                break;
            }
            if is_identifier(self.store().node(name)) {
                break;
            }
            name = match &self.store().node(name).data {
                tsc_ast::NodeData::QualifiedName(d) => d.left,
                _ => break,
            };
        }
    }

    /// Go: `func (c *Checker) getTypeOnlyDeclarationOfEntityName(name) *ast.Node` (checker.go:14842).
    fn get_type_only_declaration_of_entity_name(&mut self, name: NodeId) -> Option<NodeId> {
        let symbol = self.resolve_entity_name(
            name,
            SymbolFlags::VALUE.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE),
            true,
            true,
            NodeId::NONE,
        )?;
        self.get_type_only_alias_declaration_real(symbol)
    }

    /// Go: `func (c *Checker) getTargetOfImportClause(node) *ast.Symbol` (checker.go:14850).
    pub(crate) fn get_target_of_import_clause(&mut self, node: NodeId) -> Option<SymbolId> {
        let phase_modifier = match &self.store().node(node).data {
            tsc_ast::NodeData::ImportClause(d) => d.phase_modifier,
            _ => return None,
        };
        if phase_modifier == Some(Kind::SourceKeyword) {
            // Go (checker.go:14852-14860): the source-phase alias target is
            // a synthesized function-scoped variable whose resolvedType is
            // the global AbstractModuleSource (reportErrors — the machinery
            // 2318 in the lib-less twin). PORT(FCS tranche 7) — REAL now
            // (was the permissive None seam).
            let alias = self.get_symbol_of_declaration_merged(node)?;
            if self.alias_symbol_links.get(alias).immediate_target.is_some() {
                return self.alias_symbol_links.get(alias).immediate_target;
            }
            let name = match &self.store().node(node).data {
                tsc_ast::NodeData::ImportClause(d) => d.name,
                _ => return None,
            };
            let name = name?;
            let name_text = tsc_ast::node_text(self.store(), name).to_string();
            let symbol = self.new_symbol(SymbolFlags::FUNCTION_SCOPED_VARIABLE, &name_text);
            let declarations = self.program_symbol(alias).declarations.clone();
            self.program_symbol_mut(symbol).declarations = declarations;
            let abstract_module_source = self.get_global_abstract_module_source_type();
            self.value_symbol_links.get(symbol).resolved_type = Some(abstract_module_source);
            self.alias_symbol_links.get(alias).immediate_target = Some(symbol);
            return Some(symbol);
        }
        let parent = self.store().node(node).parent.get();
        let module_specifier = get_module_specifier_from_node(self.store(), parent)?;
        let attributes_type = crate::statements::get_import_attributes(self.store(), parent)
            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
        let module_symbol = self.resolve_external_module_name_ex(
            node,
            Some(module_specifier),
            false,
            attributes_type,
        )?;
        self.get_target_of_module_default(module_symbol, node, true)
    }

    /// Go: `func (c *Checker) getTargetOfModuleDefault(moduleSymbol, node, dontResolveAlias) *ast.Symbol`
    /// (checker.go:14867).
    pub(crate) fn get_target_of_module_default(
        &mut self,
        module_symbol: SymbolId,
        node: NodeId,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        let declarations = self.program_symbol(module_symbol).declarations.clone();
        let file = declarations
            .iter()
            .copied()
            .find(|&d| is_source_file(self.store().node(d)));
        let specifier = self.get_module_specifier_for_import_or_export(node);
        let export_module_dot_exports_symbol = None;
        // TODO(port M6 S6 remainder): the transpiled-`module.exports` default
        // arm reads program.GetImpliedNodeFormatForEmit + the emit-syntax
        // resolver; permissive None until the program seam grows them.
        if u::is_shorthand_ambient_module_symbol(self, self.store(), module_symbol) {
            // !!! exportDefaultSymbol = moduleSymbol
            // Does nothing?
        }
        let export_default_symbol = export_module_dot_exports_symbol.or_else(|| {
            self.resolve_export_by_name(
                module_symbol,
                INTERNAL_SYMBOL_NAME_DEFAULT,
                Some(node),
                dont_resolve_alias,
            )
        });
        let Some(specifier) = specifier else {
            return export_default_symbol;
        };
        // node is ImportClause | ImportSpecifier | ExportSpecifier
        let node_kind = self.store().node(node).kind;
        let attributes_container = match node_kind {
            Kind::ImportClause => binder_shims::parent_of(self.store(), node),
            Kind::ImportSpecifier => binder_shims::parent_of(self.store(), node)
                .and_then(|p| binder_shims::parent_of(self.store(), p))
                .and_then(|p| binder_shims::parent_of(self.store(), p)),
            _ => binder_shims::parent_of(self.store(), node)
                .and_then(|p| binder_shims::parent_of(self.store(), p)),
        };
        let import_attributes_type = attributes_container
            .and_then(|container| crate::statements::get_import_attributes(self.store(), container))
            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
        let has_default_only = self.is_only_importable_as_default_real(
            specifier,
            Some(module_symbol),
            import_attributes_type,
        );
        let has_synthetic_default =
            self.can_have_synthetic_default(file, module_symbol, dont_resolve_alias, specifier);
        if export_default_symbol.is_none() && !has_synthetic_default && !has_default_only {
            if node_kind == Kind::ImportClause {
                self.report_non_default_export(module_symbol, node);
            } else {
                let name = if matches!(node_kind, Kind::ImportSpecifier | Kind::ExportSpecifier)
                    || node_kind == Kind::NamespaceExport
                {
                    binder_node_data::property_name_or_name(self.store(), node).unwrap_or(node)
                } else {
                    self.store().node(node).name().unwrap_or(node)
                };
                self.error_no_module_member_symbol(module_symbol, module_symbol, node, name);
            }
        } else if has_synthetic_default || has_default_only {
            // per emit behavior, a synthetic default overrides a "real" .default member if `__esModule` is not present
            let resolved = self
                .resolve_external_module_symbol_ex(Some(module_symbol), dont_resolve_alias)
                .filter(|&s| s != module_symbol)
                .or_else(|| Some(self.resolve_symbol_ex(module_symbol, dont_resolve_alias)))?;
            self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
            return Some(resolved);
        }
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        export_default_symbol
    }

    /// Go: `func (c *Checker) reportNonDefaultExport(moduleSymbol, node)` (checker.go:14930).
    fn report_non_default_export(&mut self, module_symbol: SymbolId, node: NodeId) {
        let module_exports = self.program_symbol(module_symbol).exports.clone();
        let node_symbol_name = self
            .get_symbol_of_declaration_merged(node)
            .map(|s| self.program_symbol(s).name.clone());
        if let Some(name) = node_symbol_name.as_ref()
            && module_exports.get(name).is_some()
        {
            let module_name = crate::relater::symbol_to_string(self, module_symbol);
            let symbol_name = node_symbol_name.clone().unwrap_or_default();
            self.error(
                node,
                &diag::MODULE_0_HAS_NO_DEFAULT_EXPORT_DID_YOU_MEAN_TO_USE_IMPORT_1_FROM_0_INSTEAD,
                &[module_name, symbol_name],
            );
        } else {
            let module_name = crate::relater::symbol_to_string(self, module_symbol);
            let name = self.store().node(node).name().unwrap_or(node);
            let mut diagnostic =
                self.error(name, &diag::MODULE_0_HAS_NO_DEFAULT_EXPORT, std::slice::from_ref(&module_name));
            if let Some(&export_star) = module_exports.get(tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_STAR)
            {
                let star_declarations = self.program_symbol(export_star).declarations.clone();
                let default_export = star_declarations.iter().copied().find(|&decl| {
                    if self.store().node(decl).kind != Kind::ExportDeclaration
                        || binder_node_data::module_specifier_of(self.store(), decl).is_none()
                    {
                        return false;
                    }
                    let module_specifier =
                        binder_node_data::module_specifier_of(self.store(), decl).unwrap();
                    let attributes_type =
                        crate::statements::get_import_attributes(self.store(), decl)
                            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
                    self.resolve_external_module_name_ex(decl, Some(module_specifier), false, attributes_type)
                        .is_some_and(|resolved| {
                            self.program_symbol(resolved)
                                .exports
                                .get(INTERNAL_SYMBOL_NAME_DEFAULT)
                                .is_some()
                        })
                });
                if let Some(default_export) = default_export {
                    diagnostic.add_related_info(u::new_diagnostic_for_node(
                        self.store(),
                        default_export,
                        &diag::X_EXPORT_ASTERISK_DOES_NOT_RE_EXPORT_A_DEFAULT,
                        vec![],
                    ));
                }
            }
            let _ = diagnostic;
        }
    }

    /// Go: `func (c *Checker) resolveExportByName(moduleSymbol, name, sourceNode, dontResolveAlias) *ast.Symbol`
    /// (checker.go:14940).
    pub(crate) fn resolve_export_by_name(
        &mut self,
        module_symbol: SymbolId,
        name: &str,
        source_node: Option<NodeId>,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        let export_value = self
            .program_symbol(module_symbol)
            .exports
            .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
            .copied();
        let export_symbol = if let Some(export_value) = export_value {
            let export_value_type = self.get_type_of_symbol(export_value);
            // TODO(port M6 S8): getPropertyOfTypeEx's
            // skipObjectFunctionPropertyAugment/includeTypeOnlyMembers read
            // through resolveStructuredTypeMembers (empty until S8) — the
            // plain getPropertyOfType carrier composes the same answer.
            self.get_property_of_type(export_value_type, name)
        } else {
            self.program_symbol(module_symbol).exports.get(name).copied()
        };
        let resolved = self.resolve_symbol_ex(export_symbol.unwrap_or_else(|| self.unknown_symbol()), dont_resolve_alias);
        let resolved = (resolved != self.unknown_symbol()).then_some(resolved);
        self.mark_symbol_of_alias_declaration_if_type_only(source_node, None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfNamespaceImport(node) *ast.Symbol` (checker.go:14917).
    /// PORT (FCS tranche 44): Go marks the alias type-only UNCONDITIONALLY
    /// after the resolution pair (checker.go:14921) — a module that fails to
    /// resolve (`resolveExternalModuleName` nil) still marks, keeping the
    /// TS1361 arm alive for `import type * as ns` value uses; the former
    /// `?`-gated shape returned before the marking.
    pub(crate) fn get_target_of_namespace_import(&mut self, node: NodeId) -> Option<SymbolId> {
        let module_specifier = self.get_module_specifier_for_import_or_export(node);
        let parent_parent = binder_shims::parent_of(self.store(), node)
            .and_then(|p| binder_shims::parent_of(self.store(), p));
        let attributes_type = parent_parent
            .and_then(|pp| crate::statements::get_import_attributes(self.store(), pp))
            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
        let immediate = module_specifier.and_then(|specifier| {
            self.resolve_external_module_name_ex(node, Some(specifier), false, attributes_type)
        });
        let resolved = self.resolve_es_module_symbol(immediate, node, module_specifier);
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfNamespaceExport(node) *ast.Symbol` (checker.go:14925).
    /// PORT (FCS tranche 44): the same unconditional marking (Go
    /// checker.go:14930).
    pub(crate) fn get_target_of_namespace_export(&mut self, node: NodeId) -> Option<SymbolId> {
        let module_specifier = self.get_module_specifier_for_import_or_export(node);
        let parent = binder_shims::parent_of(self.store(), node);
        let attributes_type = parent
            .and_then(|p| crate::statements::get_import_attributes(self.store(), p))
            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
        let immediate = module_specifier.and_then(|specifier| {
            self.resolve_external_module_name_ex(node, Some(specifier), false, attributes_type)
        });
        let resolved = self.resolve_es_module_symbol(immediate, node, module_specifier);
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfImportSpecifier(node) *ast.Symbol` (checker.go:14977).
    pub(crate) fn get_target_of_import_specifier(&mut self, node: NodeId) -> Option<SymbolId> {
        let name = binder_node_data::property_name_or_name(self.store(), node)?;
        if binder_shims::module_export_name_is_default(self.store(), name) {
            if let Some(specifier) = self.get_module_specifier_for_import_or_export(node) {
                let root_parent = binder_shims::parent_of(self.store(), node)
                    .and_then(|p| binder_shims::parent_of(self.store(), p))
                    .and_then(|p| binder_shims::parent_of(self.store(), p));
                let attributes_type = root_parent
                    .and_then(|p| crate::statements::get_import_attributes(self.store(), p))
                    .and_then(|a| self.get_type_from_import_attributes(Some(a)));
                let module_symbol = self.resolve_external_module_name_ex(
                    node,
                    Some(specifier),
                    false,
                    attributes_type,
                )?;
                return self.get_target_of_module_default(module_symbol, node, true);
            }
        }
        let mut root = binder_shims::parent_of(self.store(), node)
            .and_then(|p| binder_shims::parent_of(self.store(), p))
            .and_then(|p| binder_shims::parent_of(self.store(), p));
        if tsc_ast::is_binding_element(self.store().node(node)) {
            root = Some(binder_shims::get_root_declaration(self.store(), node));
        }
        let resolved = self.get_external_module_member(root?, node, true);
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getExternalModuleMember(node, specifier, dontResolveAlias) *ast.Symbol`
    /// (checker.go:15005).
    pub(crate) fn get_external_module_member(
        &mut self,
        node: NodeId,
        specifier: NodeId,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        // node is ImportDeclaration | ExportDeclaration | VariableDeclaration
        // specifier is ImportSpecifier | ExportSpecifier | BindingElement | PropertyAccessExpression
        let mut module_specifier = get_external_module_require_argument(self.store(), node);
        if module_specifier.is_none() {
            module_specifier = crate::statements::get_external_module_name(self.store(), node);
        }
        let module_specifier = module_specifier?;
        let import_attributes_type = crate::statements::get_import_attributes(self.store(), node)
            .and_then(|a| self.get_type_from_import_attributes(Some(a)));
        let module_symbol = self.resolve_external_module_name_ex(
            node,
            Some(module_specifier),
            false,
            import_attributes_type,
        );
        let name = if self.store().node(specifier).kind != Kind::PropertyAccessExpression {
            binder_node_data::property_name_or_name(self.store(), specifier)
        } else {
            self.store().node(specifier).name()
        }?;
        if !is_identifier(self.store().node(name))
            && self.store().node(name).kind != Kind::StringLiteral
        {
            return None;
        }
        let name_text = binder_shims::node_text(self.store(), name).to_string();
        let target_symbol =
            self.resolve_es_module_symbol(module_symbol, specifier, Some(module_specifier));
        let target_symbol = target_symbol?;
        // Note: The empty string is a valid module export name:
        //
        //   import { "" as foo } from "./foo";
        //   export { foo as "" };
        //
        if !name_text.is_empty() || self.store().node(name).kind == Kind::StringLiteral {
            if let Some(module_symbol) = module_symbol
                .filter(|&m| u::is_shorthand_ambient_module_symbol(self, self.store(), m))
            {
                return Some(module_symbol);
            }
            let mut symbol_from_variable;
            // First check if module was specified with "export=". If so, get the member from the resolved type
            if module_symbol
                .is_some_and(|m| {
                    self.program_symbol(m)
                        .exports
                        .has(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
                })
            {
                let target_type = self.get_type_of_symbol(target_symbol);
                // TODO(port M6 S8): getPropertyOfTypeEx reads through the
                // structured-members seam (as resolveExportByName above).
                symbol_from_variable = self.get_property_of_type(target_type, &name_text);
            } else {
                symbol_from_variable = self.get_property_of_variable(target_symbol, &name_text);
            }
            // if symbolFromVariable is export - get its final target
            symbol_from_variable = match symbol_from_variable {
                Some(s) if s != self.unknown_symbol() => {
                    Some(self.resolve_symbol_ex(s, dont_resolve_alias))
                }
                _ => symbol_from_variable,
            };
            let mut export_container = Some(target_symbol);
            if module_symbol
                .is_some_and(|m| {
                    self.program_symbol(m)
                        .exports
                        .has(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
                })
            {
                // For `export =` modules, supplemental type/namespace exports live on the original module symbol.
                export_container = module_symbol;
            }
            let mut symbol_from_module = export_container.and_then(|container| {
                self.get_export_of_module(container, &name_text, specifier, dont_resolve_alias)
            });
            if symbol_from_module.is_none() && name_text == INTERNAL_SYMBOL_NAME_DEFAULT {
                let file = module_symbol.and_then(|m| {
                    self.program_symbol(m)
                        .declarations
                        .iter()
                        .copied()
                        .find(|&d| is_source_file(self.store().node(d)))
                });
                if self.is_only_importable_as_default_real(
                    module_specifier,
                    module_symbol,
                    import_attributes_type,
                ) || self.can_have_synthetic_default(
                    file,
                    module_symbol.unwrap_or(target_symbol),
                    dont_resolve_alias,
                    module_specifier,
                ) {
                    symbol_from_module = self
                        .resolve_external_module_symbol_ex(module_symbol, dont_resolve_alias)
                        .filter(|&s| s != self.unknown_symbol())
                        .or_else(|| {
                            Some(self.resolve_symbol_ex(
                                module_symbol.unwrap_or(target_symbol),
                                dont_resolve_alias,
                            ))
                        })
                        .filter(|&s| s != self.unknown_symbol());
                }
            }
            let mut symbol = symbol_from_variable;
            if let Some(from_module) = symbol_from_module {
                symbol = Some(from_module);
                if let Some(from_variable) = symbol_from_variable {
                    symbol = Some(self.combine_value_and_type_symbols(from_variable, from_module));
                }
            }
            if matches!(
                self.store().node(specifier).kind,
                Kind::ImportSpecifier | Kind::ExportSpecifier
            ) && self.is_only_importable_as_default_real(
                module_specifier,
                module_symbol,
                import_attributes_type,
            ) && name_text != INTERNAL_SYMBOL_NAME_DEFAULT
            {
                let module_kind = crate::statements::module_kind_string(self.module_kind);
                self.error(
                    name,
                    &diag::NAMED_IMPORTS_FROM_A_JSON_FILE_INTO_AN_ECMASCRIPT_MODULE_ARE_NOT_ALLOWED_WHEN_MODULE_IS_SET_TO_0,
                    &[module_kind],
                );
            } else if symbol.is_none()
                && module_symbol.is_some()
            {
                self.error_no_module_member_symbol(
                    module_symbol.unwrap(),
                    target_symbol,
                    node,
                    name,
                );
            }
            return symbol;
        }
        None
    }

    /// Go: `func (c *Checker) getPropertyOfVariable(symbol, name) *ast.Symbol` (checker.go:15070).
    pub(crate) fn get_property_of_variable(&mut self, symbol: SymbolId, name: &str) -> Option<SymbolId> {
        if !self.program_symbol(symbol).flags.intersects(SymbolFlags::VARIABLE) {
            return None;
        }
        let type_annotation = self
            .program_symbol(symbol)
            .value_declaration
            .and_then(|d| {
                match &self.store().node(d).data {
                    tsc_ast::NodeData::VariableDeclaration(v) => v.type_,
                    _ => None,
                }
            })?;
        let annotation_type = self.get_type_from_type_node(type_annotation);
        let symbol = self.get_property_of_type(annotation_type, name);
        symbol
            .filter(|&s| s != self.unknown_symbol())
            .map(|s| self.resolve_symbol(s))
    }

    /// Go: `func (c *Checker) combineValueAndTypeSymbols(valueSymbol, typeSymbol) *ast.Symbol`
    /// (checker.go:15090).
    pub(crate) fn combine_value_and_type_symbols(
        &mut self,
        value_symbol: SymbolId,
        type_symbol: SymbolId,
    ) -> SymbolId {
        let unknown = self.unknown_symbol();
        if value_symbol == unknown && type_symbol == unknown {
            return unknown;
        }
        if self.program_symbol(type_symbol).flags.intersects(SymbolFlags::VALUE) {
            return type_symbol;
        }
        if self
            .program_symbol(value_symbol)
            .flags
            .intersects(SymbolFlags::TYPE.union(SymbolFlags::NAMESPACE))
        {
            return value_symbol;
        }
        let (value_flags, value_name, value_declarations, value_parent, value_value_declaration, value_exports) = {
            let s = self.program_symbol(value_symbol);
            (
                s.flags,
                s.name.clone(),
                s.declarations.clone(),
                s.parent,
                s.value_declaration,
                s.exports.clone(),
            )
        };
        let (type_flags, type_declarations, type_parent, type_members) = {
            let s = self.program_symbol(type_symbol);
            (s.flags, s.declarations.clone(), s.parent, s.members.clone())
        };
        debug_assert!(!value_declarations.is_empty() || !type_declarations.is_empty());
        // Go: slices.Compact(slices.Concat(...)) — dedupe adjacent pointers;
        // SymbolIds are unique per declaration here.
        let mut declarations = value_declarations;
        for d in type_declarations {
            if !declarations.contains(&d) {
                declarations.push(d);
            }
        }
        let result = self.new_symbol(value_flags | type_flags, &value_name);
        self.program_symbol_mut(result).declarations = declarations;
        self.program_symbol_mut(result).parent = value_parent.or(type_parent);
        self.program_symbol_mut(result).value_declaration = value_value_declaration;
        self.program_symbol_mut(result).members = type_members;
        self.program_symbol_mut(result).exports = value_exports;
        result
    }

    /// Go: `func (c *Checker) getExportOfModule(symbol, nameText, specifier, dontResolveAlias) *ast.Symbol`
    /// (checker.go:15120).
    pub(crate) fn get_export_of_module(
        &mut self,
        symbol: SymbolId,
        name_text: &str,
        specifier: NodeId,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::MODULE) {
            let exports = self.get_exports_of_symbol(symbol);
            let export_symbol = exports.get(name_text).copied();
            let resolved = match export_symbol {
                Some(s) => {
                    let r = self.resolve_symbol_ex(s, dont_resolve_alias);
                    (r != self.unknown_symbol()).then_some(r)
                }
                None => None,
            };
            let export_star_declaration = self
                .module_symbol_links
                .get(symbol)
                .type_only_export_star_map
                .as_ref()
                .and_then(|m| m.get(name_text))
                .copied();
            self.mark_symbol_of_alias_declaration_if_type_only(Some(specifier), export_star_declaration);
            return resolved;
        }
        None
    }

    /// Go: `func (c *Checker) isOnlyImportableAsDefault(usage, resolvedModule, importAttributesType) bool`
    /// (checker.go:15134).
    pub(crate) fn is_only_importable_as_default_real(
        &mut self,
        usage: NodeId,
        mut resolved_module: Option<SymbolId>,
        import_attributes_type: Option<crate::types::TypeId>,
    ) -> bool {
        // In Node.js, JSON modules don't get named exports
        if tsc_core::options_generated::ModuleKind::Node16 <= self.module_kind
            && self.module_kind <= tsc_core::options_generated::ModuleKind::NodeNext
        {
            // TODO(port M6 S6 remainder): getEmitSyntaxForModuleSpecifierExpression
            // reads program.GetEmitSyntaxForUsageLocation — the program seam;
            // permissive ModuleKind::None keeps the arm dormant.
            let _usage_mode = tsc_core::options_generated::ModuleKind::None;
            if resolved_module.is_none() {
                resolved_module = self.resolve_external_module_name_ex(
                    usage,
                    Some(usage),
                    true,
                    import_attributes_type,
                );
            }
            let target_file = resolved_module.and_then(|m| {
                self.program_symbol(m)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| is_source_file(self.store().node(d)))
            });
            return target_file.is_some_and(|file| {
                let sf = tsc_ast::utilities::get_source_file_of_node(self.store(), file);
                let Some(sf) = sf else {
                    return false;
                };
                // TODO(port M6 S6 remainder): SourceFileNodeData carries no
                // jsonSourceFile marker yet; the extension probes below cover
                // the Go check (`.json` / `.d.json.ts`).
                let name = sf.file_name().as_string();
                name.ends_with(".json") || name.ends_with(".d.json.ts")
            });
        }
        false
    }

    /// Go: `func (c *Checker) canHaveSyntheticDefault(file, moduleSymbol, dontResolveAlias, usage) bool`
    /// (checker.go:15174).
    pub(crate) fn can_have_synthetic_default(
        &mut self,
        file: Option<NodeId>,
        module_symbol: SymbolId,
        dont_resolve_alias: bool,
        usage: NodeId,
    ) -> bool {
        // TODO(port M6 S6 remainder): the usage/target mode reads
        // (program.GetImpliedNodeFormatForEmit / GetEmitModuleFormatOfFile /
        // project-reference redirects) are program-seam; the arm below is the
        // declaration-file / ambient-module composition Go reaches when the
        // mode probes yield ModuleKindNone.
        let is_declaration_file = file.is_none()
            || file.is_some_and(|f| self.store().file(f.file_id()).payload().is_declaration_file);
        // Declaration files (and ambient modules)
        if is_declaration_file {
            // Definitely cannot have a synthetic default if they have a syntactic default member specified
            let default_export_symbol = self.resolve_export_by_name(
                module_symbol,
                INTERNAL_SYMBOL_NAME_DEFAULT,
                None,
                true,
            );
            if let Some(default_export_symbol) = default_export_symbol {
                let declarations = self.program_symbol(default_export_symbol).declarations.clone();
                if declarations.iter().any(|&d| u::is_syntactic_default(self.store(), d)) {
                    return false;
                }
            }
            // It _might_ still be incorrect to assume there is no __esModule marker on the import at runtime, even if there is no `default` member
            // So we check a bit more,
            if self
                .resolve_export_by_name(module_symbol, "__esModule", None, dont_resolve_alias)
                .is_some()
            {
                // If there is an `__esModule` specified in the declaration (meaning someone explicitly added it or wrote it in their code),
                // it definitely is a module and does not have a synthetic default
                return false;
            }
            return true;
        }
        // TypeScript files never have a synthetic default (as they are always emitted with an __esModule marker) _unless_ they contain an export= statement
        if !tsc_ast::utilities::is_in_js_file(self.store().node(usage)) {
            return self.has_export_assignment_symbol(module_symbol);
        }

        // JS files have a synthetic default if they do not contain ES2015+ module syntax (export = is not valid in js) _and_ do not have an __esModule marker
        let external_module_indicator_none = file.is_none_or(|f| {
            let sf = self.store().file(f.file_id());
            sf.payload().external_module_indicator.get().is_none()
        });
        external_module_indicator_none
            && self
                .resolve_export_by_name(module_symbol, "__esModule", None, dont_resolve_alias)
                .is_none()
    }

    /// Go: `func hasExportAssignmentSymbol(moduleSymbol) bool` (utilities.go).
    fn has_export_assignment_symbol(&mut self, module_symbol: SymbolId) -> bool {
        self.program_symbol(module_symbol)
            .exports
            .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
            .is_some()
    }

    /// Go: `func (c *Checker) errorNoModuleMemberSymbol(moduleSymbol, targetSymbol, node, name)`
    /// (checker.go:15189).
    #[allow(clippy::too_many_arguments)]
    fn error_no_module_member_symbol(
        &mut self,
        module_symbol: SymbolId,
        target_symbol: SymbolId,
        // PORT: Go's errorNoModuleMemberSymbol carries the containing node for
        // the `File '...' is not a module` arm; the port reports at `name`
        // (the arm fires there too — S6's diagnostic surface).
        _node: NodeId,
        name: NodeId,
    ) {
        if self.compiler_options.no_check == Tristate::TSTrue {
            return;
        }
        let module_name = self.get_fully_qualified_name(module_symbol);
        let declaration_name = tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(name));
        let suggestion = if is_identifier(self.store().node(name)) {
            self.get_suggested_symbol_for_nonexistent_module(name, target_symbol)
        } else {
            None
        };
        if let Some(suggestion) = suggestion {
            let suggestion_name = crate::relater::symbol_to_string(self, suggestion);
            let mut diagnostic = self.error(
                name,
                &diag::X_0_HAS_NO_EXPORTED_MEMBER_NAMED_1_DID_YOU_MEAN_2,
                &[module_name, declaration_name.clone(), suggestion_name.clone()],
            );
            if let Some(value_declaration) = self.program_symbol(suggestion).value_declaration {
                diagnostic.add_related_info(u::new_diagnostic_for_node(
                    self.store(),
                    value_declaration,
                    &diag::X_0_IS_DECLARED_HERE,
                    vec![suggestion_name],
                ));
            }
            let _ = diagnostic;
        } else if self
            .program_symbol(module_symbol)
            .exports
            .has(INTERNAL_SYMBOL_NAME_DEFAULT)
        {
            self.error(
                name,
                &diag::MODULE_0_HAS_NO_EXPORTED_MEMBER_1_DID_YOU_MEAN_TO_USE_IMPORT_1_FROM_0_INSTEAD,
                &[module_name, declaration_name],
            );
        } else {
            self.report_non_exported_member(name, &declaration_name, module_symbol, &module_name);
        }
    }

    /// Go: `func (c *Checker) reportNonExportedMember(name, declarationName, moduleSymbol, moduleName)`
    /// (checker.go:15212).
    fn report_non_exported_member(
        &mut self,
        name: NodeId,
        declaration_name: &str,
        module_symbol: SymbolId,
        module_name: &str,
    ) {
        let name_text = binder_shims::node_text(self.store(), name).to_string();
        let local_symbol = self
            .program_symbol(module_symbol)
            .value_declaration
            .and_then(|vd| binder_node_data::locals_of(self.store(), vd))
            .and_then(|locals| locals.get(&name_text).copied());
        let exports = self.program_symbol(module_symbol).exports.clone();
        if let Some(local_symbol) = local_symbol {
            if let Some(&exported_equals_symbol) = exports.get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS) {
                if self.get_symbol_if_same_reference(exported_equals_symbol, local_symbol).is_some() {
                    self.report_invalid_import_equals_export_member(name, declaration_name, module_name);
                } else {
                    self.error(
                        name,
                        &diag::MODULE_0_HAS_NO_EXPORTED_MEMBER_1,
                        &[module_name.to_string(), declaration_name.to_string()],
                    );
                }
            } else {
                let exported_symbol = exports.entries().find(|(_, symbol)| {
                    self.get_symbol_if_same_reference(**symbol, local_symbol).is_some()
                });
                let mut diagnostic = if let Some((_, exported_symbol)) = exported_symbol {
                    let exported_symbol = *exported_symbol;
                    let exported_name = crate::relater::symbol_to_string(self, exported_symbol);
                    self.error(
                        name,
                        &diag::MODULE_0_DECLARES_1_LOCALLY_BUT_IT_IS_EXPORTED_AS_2,
                        &[module_name.to_string(), declaration_name.to_string(), exported_name],
                    )
                } else {
                    self.error(
                        name,
                        &diag::MODULE_0_DECLARES_1_LOCALLY_BUT_IT_IS_NOT_EXPORTED,
                        &[module_name.to_string(), declaration_name.to_string()],
                    )
                };
                let local_declarations = self.program_symbol(local_symbol).declarations.clone();
                for (i, decl) in local_declarations.iter().enumerate() {
                    let message = if i == 0 {
                        &diag::X_0_IS_DECLARED_HERE
                    } else {
                        &diag::X_AND_HERE
                    };
                    diagnostic.add_related_info(u::new_diagnostic_for_node(
                        self.store(),
                        *decl,
                        message,
                        vec![declaration_name.to_string()],
                    ));
                }
                let _ = diagnostic;
            }
        } else {
            self.error(
                name,
                &diag::MODULE_0_HAS_NO_EXPORTED_MEMBER_1,
                &[module_name.to_string(), declaration_name.to_string()],
            );
        }
    }

    /// Go: `func (c *Checker) reportInvalidImportEqualsExportMember(name, declarationName, moduleName)`
    /// (checker.go:15236).
    fn report_invalid_import_equals_export_member(
        &mut self,
        name: NodeId,
        declaration_name: &str,
        module_name: &str,
    ) {
        if self.module_kind >= tsc_core::options_generated::ModuleKind::ES2015 {
            self.error(
                name,
                &diag::X_0_CAN_ONLY_BE_IMPORTED_BY_USING_A_DEFAULT_IMPORT,
                &[declaration_name.to_string()],
            );
        } else if tsc_ast::utilities::is_in_js_file(self.store().node(name)) {
            self.error(
                name,
                &diag::X_0_CAN_ONLY_BE_IMPORTED_BY_USING_A_REQUIRE_CALL_OR_BY_USING_A_DEFAULT_IMPORT,
                &[declaration_name.to_string()],
            );
        } else {
            self.error(
                name,
                &diag::X_0_CAN_ONLY_BE_IMPORTED_BY_USING_IMPORT_1_REQUIRE_2_OR_A_DEFAULT_IMPORT,
                &[declaration_name.to_string(), declaration_name.to_string(), module_name.to_string()],
            );
        }
    }

    /// Go: `func (c *Checker) getTargetOfExportSpecifier(node, meaning, dontResolveAlias) *ast.Symbol`
    /// (checker.go:15238).
    pub(crate) fn get_target_of_export_specifier(
        &mut self,
        node: NodeId,
        meaning: SymbolFlags,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        let name = binder_node_data::property_name_or_name(self.store(), node)?;
        if binder_shims::module_export_name_is_default(self.store(), name) {
            if let Some(specifier) = self.get_module_specifier_for_import_or_export(node) {
                let parent_parent = binder_shims::parent_of(self.store(), node)
                    .and_then(|p| binder_shims::parent_of(self.store(), p));
                let attributes_type = parent_parent
                    .and_then(|pp| crate::statements::get_import_attributes(self.store(), pp))
                    .and_then(|a| self.get_type_from_import_attributes(Some(a)));
                let module_symbol = self.resolve_external_module_name_ex(
                    node,
                    Some(specifier),
                    false,
                    attributes_type,
                )?;
                return self.get_target_of_module_default(module_symbol, node, dont_resolve_alias);
            }
        }
        let export_declaration = binder_shims::parent_of(self.store(), node)
            .and_then(|p| binder_shims::parent_of(self.store(), p))?;
        let has_module_specifier =
            binder_node_data::module_specifier_of(self.store(), export_declaration).is_some();
        let resolved = if has_module_specifier {
            self.get_external_module_member(export_declaration, node, dont_resolve_alias)
        } else if self.store().node(name).kind == Kind::StringLiteral {
            None
        } else {
            self.resolve_entity_name(name, meaning, false, dont_resolve_alias, NodeId::NONE)
        };
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfExportAssignment(node) *ast.Symbol` (checker.go:15270).
    pub(crate) fn get_target_of_export_assignment(&mut self, node: NodeId) -> Option<SymbolId> {
        // An `export =` / `export default` inside a namespace/module block is a grammar error;
        // checkExportAssignment reports it and returns without resolving the expression. Mirror that
        // bail-out here (using the same container computation) so that alias resolution triggered by
        // the emit resolver does not resolve — and report "Cannot find name" diagnostics on — the
        // expression, which would produce diagnostics inconsistent with checking.
        if is_contained_by_namespace(self.store(), node) {
            return None;
        }
        let expression = match &self.store().node(node).data {
            tsc_ast::NodeData::ExportAssignment(d) => d.expression,
            _ => return None,
        };
        let resolved = self.get_target_of_alias_like_expression(expression);
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfBinaryExpression(node) *ast.Symbol` (checker.go:15284).
    pub(crate) fn get_target_of_binary_expression(&mut self, node: NodeId) -> Option<SymbolId> {
        let right = match &self.store().node(node).data {
            tsc_ast::NodeData::BinaryExpression(d) => d.right,
            _ => return None,
        };
        let resolved = self.get_target_of_alias_like_expression(right);
        self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
        resolved
    }

    /// Go: `func (c *Checker) getTargetOfAliasLikeExpression(expression) *ast.Symbol` (checker.go:15290).
    pub(crate) fn get_target_of_alias_like_expression(&mut self, expression: NodeId) -> Option<SymbolId> {
        if tsc_ast::is_class_expression(self.store().node(expression)) {
            let t = self.check_expression_cached(expression);
            return self.type_(t).symbol();
        }
        if !binder_shims::is_entity_name(self.store().node(expression))
            && !tsc_ast::utilities::is_entity_name_expression(self.store(), expression)
        {
            return None;
        }
        let alias_like = self.resolve_entity_name(
            expression,
            SymbolFlags::VALUE.union(SymbolFlags::TYPE).union(SymbolFlags::NAMESPACE),
            true,
            true,
            NodeId::NONE,
        );
        if let Some(alias_like) = alias_like {
            return Some(alias_like);
        }
        self.check_expression_cached(expression);
        self.get_resolved_symbol_or_nil(expression)
    }

    /// Go: `func (c *Checker) getTargetOfNamespaceExportDeclaration(node) *ast.Symbol` (checker.go:15305).
    pub(crate) fn get_target_of_namespace_export_declaration(&mut self, node: NodeId) -> Option<SymbolId> {
        let parent = binder_shims::parent_of(self.store(), node)?;
        if can_have_symbol(self.store(), parent) {
            let symbol = crate::typeofsymbol::get_symbol_of_declaration(self, parent)?;
            let resolved = self.resolve_external_module_symbol_ex(Some(symbol), true);
            self.mark_symbol_of_alias_declaration_if_type_only(Some(node), None);
            return resolved;
        }
        None
    }

    /// Go: `func (c *Checker) getTargetOfAccessExpression(node) *ast.Symbol` (checker.go:15314).
    pub(crate) fn get_target_of_access_expression(&mut self, node: NodeId) -> Option<SymbolId> {
        let parent = binder_shims::parent_of(self.store(), node)?;
        if self.store().node(parent).kind == Kind::BinaryExpression {
            let (left, operator_token) = match &self.store().node(parent).data {
                tsc_ast::NodeData::BinaryExpression(d) => (d.left, d.operator_token),
                _ => return None,
            };
            if left == node && self.store().node(operator_token).kind == Kind::EqualsToken {
                let right = match &self.store().node(parent).data {
                    tsc_ast::NodeData::BinaryExpression(d) => d.right,
                    _ => return None,
                };
                return self.get_target_of_alias_like_expression(right);
            }
        }
        None
    }

    /// Go: `func (c *Checker) getModuleSpecifierForImportOrExport(node) *ast.Node` (checker.go:15324).
    pub(crate) fn get_module_specifier_for_import_or_export(&mut self, node: NodeId) -> Option<NodeId> {
        match self.store().node(node).kind {
            Kind::ImportClause => {
                let parent = self.store().node(node).parent.get();
                get_module_specifier_from_node(self.store(), parent)
            }
            Kind::ImportEqualsDeclaration => {
                let module_reference = match &self.store().node(node).data {
                    tsc_ast::NodeData::ImportEqualsDeclaration(d) => Some(d.module_reference),
                    _ => None,
                }?;
                if self.store().node(module_reference).kind == Kind::ExternalModuleReference {
                    binder_node_data::expression_of(self.store(), module_reference)
                } else {
                    None
                }
            }
            Kind::NamespaceImport => {
                let parent_parent = binder_shims::parent_of(self.store(), node)
                    .and_then(|p| binder_shims::parent_of(self.store(), p))?;
                get_module_specifier_from_node(self.store(), parent_parent)
            }
            Kind::ImportSpecifier => {
                let named_import = binder_shims::parent_of(self.store(), node)?;
                let import_clause = binder_shims::parent_of(self.store(), named_import)?;
                let root = binder_shims::parent_of(self.store(), import_clause)?;
                get_module_specifier_from_node(self.store(), root)
            }
            Kind::NamespaceExport => {
                let parent = binder_shims::parent_of(self.store(), node)?;
                get_module_specifier_from_node(self.store(), parent)
            }
            Kind::ExportSpecifier => {
                let export_decl = binder_shims::parent_of(self.store(), node)?;
                let parent_parent = binder_shims::parent_of(self.store(), export_decl)?;
                get_module_specifier_from_node(self.store(), parent_parent)
            }
            _ => panic!(
                "Unhandled case in getModuleSpecifierForImportOrExport: {}",
                self.store().node(node).kind_string()
            ),
        }
    }

    /// Go: `func (c *Checker) hasLateBindableName(node) bool` (checker.go:20352).
    pub(crate) fn has_late_bindable_name(&mut self, node: NodeId) -> bool {
        let Some(name) = binder_shims::get_name_of_declaration(self.store(), node) else {
            return false;
        };
        self.is_late_bindable_name(name)
    }

    /// Go: `func (c *Checker) isLateBindableName(node) bool` (checker.go:20362).
    pub(crate) fn is_late_bindable_name(&mut self, node: NodeId) -> bool {
        if !is_late_bindable_ast(self.store(), node) {
            return false;
        }
        if tsc_ast::is_computed_property_name(self.store().node(node)) {
            let t = self.check_computed_property_name(node);
            u::is_type_usable_as_property_name(self.type_(t))
        } else {
            let argument = match &self.store().node(node).data {
                tsc_ast::NodeData::ElementAccessExpression(e) => Some(e.argument_expression),
                _ => None,
            };
            let Some(argument) = argument else {
                return false;
            };
            let t = self.check_expression_cached(argument);
            u::is_type_usable_as_property_name(self.type_(t))
        }
    }

    /// Go: `func (c *Checker) hasLateBindableIndexSignature(node) bool` (checker.go:20377).
    pub(crate) fn has_late_bindable_index_signature(&mut self, node: NodeId) -> bool {
        let Some(name) = binder_shims::get_name_of_declaration(self.store(), node) else {
            return false;
        };
        self.is_late_bindable_index_signature(name)
    }

    /// Go: `func (c *Checker) isLateBindableIndexSignature(node) bool` (checker.go:20385).
    fn is_late_bindable_index_signature(&mut self, node: NodeId) -> bool {
        if !is_late_bindable_ast(self.store(), node) {
            return false;
        }
        // Go: `isTypeUsableAsIndexSignatureDeclaration` — the name's type
        // must be assignable to stringNumberSymbolType. W3T7: was the S7
        // permissive-false stub; the REAL arm un-dormants the
        // ES5SymbolProperty5 computed `[Symbol.iterator]()` member's
        // synthetic symbol-keyed index info.
        let t = if tsc_ast::is_computed_property_name(self.store().node(node)) {
            self.check_computed_property_name(node)
        } else {
            let arg = self
                .store()
                .node(node)
                .as_element_access_expression()
                .expect("isLateBindableAST narrows to ElementAccessExpression")
                .argument_expression;
            self.check_expression_cached(arg)
        };
        let string_number_symbol = self.string_number_symbol_type();
        self.is_type_assignable_to(t, string_number_symbol)
    }

    /// Go: `func (c *Checker) getResolvedSymbolOrNil(node) *ast.Symbol`
    /// (checker.go:14174).
    ///
    /// PORT (FCS): Go's body is the raw LINK READ
    /// (`c.symbolNodeLinks.Get(node).resolvedSymbol`) — it never resolves
    /// (getResolvedSymbol's resolveName/node.Text() path panics in Go on
    /// non-name nodes, so callers reach it only for already-checked nodes;
    /// the former delegation to get_resolved_symbol made the
    /// getTargetOfAliasLikeExpression fallback panic on PropertyAccess).
    pub(crate) fn get_resolved_symbol_or_nil(&mut self, node: NodeId) -> Option<SymbolId> {
        let symbol = self.symbol_node_links.get(node).resolved_symbol;
        symbol.filter(|&s| s != self.unknown_symbol())
    }

    // ────────────────────────────────────────────────────────────────────────
    // Section B2 — resolveEntityName + the exports/members + external-module
    // resolution family (Go checker.go:15395–16770)
    // ────────────────────────────────────────────────────────────────────────

    /// Go: `func (c *Checker) resolveEntityName(name, meaning, ignoreErrors, dontResolveAlias, location) *ast.Symbol`
    /// (checker.go:16158).
    #[allow(unused_assignments)] // the match arms all assign `symbol`; Go's
    // declaration-then-switch shape keeps the initialized binding.
    pub(crate) fn resolve_entity_name(
        &mut self,
        name: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
        dont_resolve_alias: bool,
        location: NodeId,
    ) -> Option<SymbolId> {
        if tsc_ast::node_is_missing(self.store().node(name)) {
            return None;
        }
        let mut symbol: Option<SymbolId> = None;
        match self.store().node(name).kind {
            Kind::Identifier => {
                let mut message: Option<&'static Message> = None;
                if !ignore_errors {
                    if meaning == SymbolFlags::NAMESPACE || u::node_is_synthesized(self.store().node(name)) {
                        message = Some(&diag::CANNOT_FIND_NAMESPACE_0);
                    } else {
                        let first = crate::statements::get_first_identifier(self.store(), name);
                        message = Some(self.get_cannot_find_name_diagnostic_for_name(first));
                    }
                }
                let resolve_location = if location == NodeId::NONE {
                    name
                } else {
                    location
                };
                let name_text = tsc_ast::node_text(self.store(), name).to_string();
                if meaning == SymbolFlags::NAMESPACE {
                    symbol = self
                        .resolve_name_worker(
                            Some(resolve_location),
                            &name_text,
                            meaning,
                            None,
                            true,
                            false,
                        )
                        .map(|s| self.get_merged_symbol(s));
                    if symbol.is_none() {
                        let alias = self
                            .resolve_name_worker(
                                Some(resolve_location),
                                &name_text,
                                SymbolFlags::ALIAS,
                                None,
                                true,
                                false,
                            )
                            .map(|s| self.get_merged_symbol(s));
                        if let Some(alias) = alias
                            && self.program_symbol(alias).name
                                == *tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                        {
                            // resolve typedefs exported from commonjs, stored on the module symbol
                            symbol = self.program_symbol(alias).parent;
                        }
                    }
                    if symbol.is_none() && message.is_some() {
                        self.resolve_name_worker(
                            Some(resolve_location),
                            &name_text,
                            meaning,
                            message,
                            true,
                            false,
                        );
                    }
                } else {
                    symbol = self
                        .resolve_name_worker(
                            Some(resolve_location),
                            &name_text,
                            meaning,
                            message,
                            true,
                            false,
                        )
                        .map(|s| self.get_merged_symbol(s));
                }
            }
            Kind::QualifiedName => {
                let (left, right) = match &self.store().node(name).data {
                    tsc_ast::NodeData::QualifiedName(q) => (q.left, q.right),
                    _ => return None,
                };
                symbol =
                    self.resolve_qualified_name(name, left, right, meaning, ignore_errors, location);
            }
            Kind::PropertyAccessExpression => {
                let (expression, prop_name) = match &self.store().node(name).data {
                    tsc_ast::NodeData::PropertyAccessExpression(p) => {
                        (p.expression, p.name)
                    }
                    _ => return None,
                };
                symbol = self.resolve_qualified_name(
                    name,
                    expression,
                    prop_name,
                    meaning,
                    ignore_errors,
                    location,
                );
            }
            _ => panic!(
                "Unknown entity name kind: {}",
                self.store().node(name).kind_string()
            ),
        }
        if let Some(symbol_id) = symbol.filter(|&s| s != self.unknown_symbol()) {
            let mut symbol_id = symbol_id;
            let name_parent = self.store().node(name).parent.get();
            // PORT (FCS): Go's guards here are the LOC-based
            // `ast.NodeIsSynthesized(name)` (pos/end < 0 — utilities.go:2766),
            // NOT the NodeFlagsSynthesized bit: the checker-copied entity
            // names (parse_isolated_entity_name's markAsSynthetic loc
            // (-1,-1), jsx.go:1084) are excluded by the loc probe only. The
            // flag bit misport let a synthetic alias-resolving name reach
            // getAliasDeclarationFromName, whose parent walk hit the
            // parentless copied QualifiedName (Go: node.Parent nil).
            if !u::node_is_synthesized(self.store().node(name))
                && binder_shims::is_entity_name(self.store().node(name))
                && (self.program_symbol(symbol_id).flags.intersects(SymbolFlags::ALIAS)
                    || name_parent != NodeId::NONE
                        && self.store().node(name_parent).kind == Kind::ExportAssignment)
            {
                let alias_declaration =
                    u::get_alias_declaration_from_name(self.store(), name);
                self.mark_symbol_of_alias_declaration_if_type_only(alias_declaration, None);
            }
            // We know a symbol with the given meaning exists along the alias chain, so resolve until we find it.
            while !self.program_symbol(symbol_id).flags.intersects(meaning)
                && !dont_resolve_alias
                && self.program_symbol(symbol_id).flags.intersects(SymbolFlags::ALIAS)
            {
                symbol_id = self.resolve_alias_real(symbol_id);
            }
            return Some(symbol_id);
        }
        None
    }

    /// Go: `func (c *Checker) resolveQualifiedName(name, left, right, meaning, ignoreErrors, location) *ast.Symbol`
    /// (checker.go:16214).
    fn resolve_qualified_name(
        &mut self,
        name: NodeId,
        left: NodeId,
        right: NodeId,
        meaning: SymbolFlags,
        ignore_errors: bool,
        location: NodeId,
    ) -> Option<SymbolId> {
        let namespace =
            self.resolve_entity_name(left, SymbolFlags::NAMESPACE, ignore_errors, false, location)?;
        if tsc_ast::node_is_missing(self.store().node(right)) {
            return None;
        }
        if namespace == self.unknown_symbol() {
            return Some(namespace);
        }
        // TODO(port M6 W1/js): the commonjs-require namespace redirection reads
        // isCommonJSRequire + the module-resolution kinds; permissive skip
        // keeps the JS arm dormant (the typed path never enters it).
        let text = tsc_ast::node_text(self.store(), right).to_string();
        let exports = self.get_exports_of_symbol(namespace);
        let mut symbol = self
            .get_symbol(&exports, &text, meaning)
            .map(|s| self.get_merged_symbol(s));
        if symbol.is_none() && self.program_symbol(namespace).flags.intersects(SymbolFlags::ALIAS)
        {
            // `namespace` can be resolved further if there was a symbol merge with a re-export
            let resolved = self.resolve_alias_real(namespace);
            let exports = self.get_exports_of_symbol(resolved);
            symbol = self
                .get_symbol(&exports, &text, meaning)
                .map(|s| self.get_merged_symbol(s));
        }
        if symbol.is_none() && !ignore_errors {
            let namespace_name = self.get_fully_qualified_name(namespace);
            let declaration_name =
                tsc_scanner::utilities::declaration_name_to_string(self.store(), Some(right));
            // TODO(port M6 S6 remainder): getSuggestedSymbolForNonexistentModule
            // needs the getSpellingSuggestion engine (getExportsOfModule +
            // Levenshtein over export names); permissive None keeps the
            // "did you mean" arm dormant.
            let suggestion_for_nonexistent_module: Option<SymbolId> = None;
            if let Some(suggestion) = suggestion_for_nonexistent_module {
                let suggestion_name = crate::relater::symbol_to_string(self, suggestion);
                self.error(
                    right,
                    &diag::X_0_HAS_NO_EXPORTED_MEMBER_NAMED_1_DID_YOU_MEAN_2,
                    &[namespace_name, declaration_name, suggestion_name],
                );
                return None;
            }
            // TODO(port M6 S6 remainder): the typeof-suggestion arm
            // (canSuggestTypeof reads globalObjectType + tryGetQualifiedNameAsValue)
            // rides the S7 global-classes seam; permissive skip.
            if meaning.intersects(SymbolFlags::NAMESPACE) {
                let name_parent = self.store().node(name).parent.get();
                if self.store().node(name_parent).kind == Kind::QualifiedName {
                    let exports = self.get_exports_of_symbol(namespace);
                    let exported_type_symbol = self
                        .get_symbol(&exports, &text, SymbolFlags::TYPE)
                        .map(|s| self.get_merged_symbol(s));
                    if let Some(exported_type_symbol) = exported_type_symbol {
                        let (qualified_left, qualified_right) =
                            match &self.store().node(name_parent).data {
                                tsc_ast::NodeData::QualifiedName(q) => (q.left, q.right),
                                _ => (name_parent, name_parent),
                            };
                        let _ = qualified_left;
                        let right_text =
                            tsc_ast::node_text(self.store(), qualified_right).to_string();
                        let type_name =
                            crate::relater::symbol_to_string(self, exported_type_symbol);
                        self.error(
                            qualified_right,
                            &diag::CANNOT_ACCESS_0_1_BECAUSE_0_IS_A_TYPE_BUT_NOT_A_NAMESPACE_DID_YOU_MEAN_TO_RETRIEVE_THE_TYPE_OF_THE_PROPERTY_1_IN_0_WITH_0_1,
                            &[type_name, right_text],
                        );
                        return None;
                    }
                }
            }
            self.error(
                right,
                &diag::NAMESPACE_0_HAS_NO_EXPORTED_MEMBER_1,
                &[namespace_name, declaration_name],
            );
        }
        symbol
    }

    /// Go: `func (c *Checker) getExportsOfSymbol(symbol) ast.SymbolTable`
    /// (checker.go:16306). PORT: Go hands back the live table; the port
    /// snapshots the entries (S6's callers only read).
    pub(crate) fn get_exports_of_symbol(&mut self, symbol: SymbolId) -> SymbolTable {
        if self
            .program_symbol(symbol)
            .flags
            .intersects(SymbolFlags::LATE_BINDING_CONTAINER)
        {
            return self.get_resolved_members_or_exports_of_symbol(
                symbol,
                MembersOrExports::ResolvedExports,
            );
        }
        if self.program_symbol(symbol).flags.intersects(SymbolFlags::MODULE) {
            return self.get_exports_of_module(symbol);
        }
        self.program_symbol(symbol).exports.clone()
    }

    /// Go: `func (c *Checker) getResolvedMembersOrExportsOfSymbol(symbol, resolutionKind) ast.SymbolTable`
    /// (checker.go:16316).
    pub(crate) fn get_resolved_members_or_exports_of_symbol(
        &mut self,
        symbol: SymbolId,
        resolution_kind: MembersOrExports,
    ) -> SymbolTable {
        let index = resolution_kind as usize;
        if self.members_and_exports_links.get(symbol).0[index].is_empty() {
            let is_static = resolution_kind == MembersOrExports::ResolvedExports;
            let mut early_symbols = self.program_symbol(symbol).exports.clone();
            if !is_static {
                early_symbols = self.program_symbol(symbol).members.clone();
            } else if self.program_symbol(symbol).flags.intersects(SymbolFlags::MODULE) {
                let (exports, _) = self.get_exports_of_module_worker(symbol);
                early_symbols = exports;
            }
            self.members_and_exports_links.get(symbol).0[index] = early_symbols.clone();
            // fill in any as-yet-unresolved late-bound members.
            let mut late_symbols: SymbolTable = SymbolTable::new();
            let declarations = self.program_symbol(symbol).declarations.clone();
            for decl in declarations {
                let members: Vec<NodeId> =
                    u::get_members_of_declaration(self.store(), decl).to_vec();
                for member in members {
                    let has_static = binder_shims::has_static_modifier(self.store().node(member));
                    if is_static == has_static {
                        if self.has_late_bindable_name(member) {
                            self.late_bind_member(symbol, &early_symbols, &mut late_symbols, member);
                        } else if self.has_late_bindable_index_signature(member) {
                            self.late_bind_index_signature(
                                symbol,
                                &early_symbols,
                                &mut late_symbols,
                                member,
                            );
                        }
                    }
                }
            }
            if is_static {
                let assignment_symbol = self
                    .program_symbol(symbol)
                    .exports
                    .get(tsc_ast::INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION)
                    .copied();
                if let Some(assignment_symbol) = assignment_symbol {
                    let decls = self.program_symbol(assignment_symbol).declarations.clone();
                    for member in decls {
                        if self.has_late_bindable_name(member) {
                            self.late_bind_member(symbol, &early_symbols, &mut late_symbols, member);
                        }
                    }
                }
            }
            let combined = combine_symbol_tables(&early_symbols, &late_symbols);
            self.members_and_exports_links.get(symbol).0[index] = combined.clone();
            return combined;
        }
        self.members_and_exports_links.get(symbol).0[index].clone()
    }

    /// Go: `func (c *Checker) lateBindMember(parent, earlySymbols, lateSymbols, decl) *ast.Symbol`
    /// (checker.go:16391). PORT: Go mutates the live `lateSymbols` table; the
    /// port snapshots pass `&mut SymbolTable` through.
    fn late_bind_member(
        &mut self,
        parent: SymbolId,
        early_symbols: &SymbolTable,
        late_symbols: &mut SymbolTable,
        decl: NodeId,
    ) -> SymbolId {
        let decl_symbol = crate::typeofsymbol::payload_symbol(self.store().node(decl))
            .expect("The member is expected to have a symbol.");
        if self.symbol_node_links.get(decl).resolved_symbol.is_none() {
            // In the event we attempt to resolve the late-bound name of this member recursively,
            // fall back to the early-bound name of this member.
            self.symbol_node_links.get(decl).resolved_symbol = Some(decl_symbol);
            let decl_name = if tsc_ast::is_binary_expression(self.store().node(decl)) {
                match &self.store().node(decl).data {
                    tsc_ast::NodeData::BinaryExpression(b) => b.left,
                    _ => decl,
                }
            } else {
                self.store().node(decl).name().unwrap_or(decl)
            };
            let t = if tsc_ast::is_element_access_expression(self.store().node(decl_name)) {
                let argument = match &self.store().node(decl_name).data {
                    tsc_ast::NodeData::ElementAccessExpression(e) => e.argument_expression,
                    _ => NodeId::NONE,
                };
                if argument == NodeId::NONE {
                    self.error_type()
                } else {
                    self.check_expression_cached(argument)
                }
            } else {
                self.check_computed_property_name(decl_name)
            };
            if u::is_type_usable_as_property_name(self.type_(t)) {
                let member_name = u::get_property_name_from_type(self.type_(t));
                let symbol_flags = self.program_symbol(decl_symbol).flags;
                // Get or add a late-bound symbol for the member. This allows us to merge late-bound accessor declarations.
                let late_symbol = late_symbols.get(&member_name).copied();
                let late_symbol = match late_symbol {
                    Some(s) => s,
                    None => {
                        let s = self.new_symbol_ex(
                            SymbolFlags::NONE,
                            &member_name,
                            tsc_ast::CheckFlags::LATE,
                        );
                        late_symbols.set(member_name.clone(), s);
                        s
                    }
                };
                // Report an error if there's a symbol declaration with the same name and conflicting flags.
                let early_symbol = early_symbols.get(&member_name).copied();
                if self.program_symbol(late_symbol).flags.intersects(get_excluded_symbol_flags(
                    symbol_flags,
                )) {
                    // If we have an existing early-bound member, combine its declarations so that we can
                    // report an error at each declaration.
                    let mut declarations: Vec<NodeId> = match early_symbol {
                        Some(early) => {
                            let mut d = self.program_symbol(early).declarations.clone();
                            d.extend(self.program_symbol(late_symbol).declarations.clone());
                            d
                        }
                        None => self.program_symbol(late_symbol).declarations.clone(),
                    };
                    let mut name = member_name.clone();
                    if self.type_(t).flags.intersects(TypeFlags::UNIQUE_ESSYMBOL) {
                        name = tsc_scanner::utilities::declaration_name_to_string(
                            self.store(),
                            Some(decl_name),
                        );
                    }
                    for d in &declarations {
                        let report_at = binder_shims::get_name_of_declaration(self.store(), *d)
                            .unwrap_or(*d);
                        self.error(report_at, &diag::DUPLICATE_IDENTIFIER_0, &[name.clone()]);
                    }
                    let report_decl = binder_shims::get_name_of_declaration(self.store(), decl_name)
                        .unwrap_or(decl_name);
                    self.error(report_decl, &diag::DUPLICATE_IDENTIFIER_0, &[name.clone()]);
                    let late_flags = self.program_symbol(late_symbol).flags;
                    if late_flags.intersects(SymbolFlags::ACCESSOR)
                        && (late_flags & SymbolFlags::ACCESSOR)
                            != (symbol_flags & SymbolFlags::ACCESSOR)
                    {
                        self.program_symbol_mut(late_symbol).flags |= SymbolFlags::ACCESSOR;
                    }
                    declarations.clear();
                    let _ = declarations;
                    let replacement = self.new_symbol_ex(
                        SymbolFlags::NONE,
                        &member_name,
                        tsc_ast::CheckFlags::LATE,
                    );
                    late_symbols.set(member_name.clone(), replacement);
                    self.finish_late_bound_member(
                        replacement,
                        decl,
                        decl_symbol,
                        symbol_flags,
                        parent,
                        t,
                    );
                    return replacement;
                }
                self.finish_late_bound_member(
                    late_symbol,
                    decl,
                    decl_symbol,
                    symbol_flags,
                    parent,
                    t,
                );
                return late_symbol;
            }
        }
        self.symbol_node_links.get(decl).resolved_symbol.unwrap()
    }

    /// The shared tail of Go's lateBindMember (the valueSymbolLinks.nameType
    /// write + addDeclarationToLateBoundSymbol composition).
    fn finish_late_bound_member(
        &mut self,
        late_symbol: SymbolId,
        decl: NodeId,
        decl_symbol: SymbolId,
        symbol_flags: SymbolFlags,
        parent: SymbolId,
        t: TypeId,
    ) {
        self.value_symbol_links.get(late_symbol).name_type = Some(t);
        self.add_declaration_to_late_bound_symbol(late_symbol, decl, symbol_flags);
        if self.program_symbol(late_symbol).parent.is_none() {
            self.program_symbol_mut(late_symbol).parent = Some(parent);
        }
        self.symbol_node_links.get(decl).resolved_symbol = Some(late_symbol);
        let _ = decl_symbol;
    }

    /// Go: `func (c *Checker) lateBindIndexSignature(parent, earlySymbols, lateSymbols, decl)`
    /// (checker.go:16454).
    fn late_bind_index_signature(
        &mut self,
        parent: SymbolId,
        early_symbols: &SymbolTable,
        late_symbols: &mut SymbolTable,
        decl: NodeId,
    ) {
        let _ = parent;
        // First, late bind the index symbol itself, if needed
        let index_symbol = match late_symbols
            .get(tsc_ast::INTERNAL_SYMBOL_NAME_INDEX)
            .copied()
        {
            Some(s) => s,
            None => {
                let early = early_symbols
                    .get(tsc_ast::INTERNAL_SYMBOL_NAME_INDEX)
                    .copied();
                let index_symbol = match early {
                    None => self.new_symbol_ex(
                        SymbolFlags::NONE,
                        tsc_ast::INTERNAL_SYMBOL_NAME_INDEX,
                        tsc_ast::CheckFlags::LATE,
                    ),
                    Some(early) => {
                        let cloned = self.clone_symbol(early);
                        self.program_symbol_mut(cloned).check_flags |= tsc_ast::CheckFlags::LATE;
                        cloned
                    }
                };
                late_symbols.set(
                    tsc_ast::INTERNAL_SYMBOL_NAME_INDEX.to_string(),
                    index_symbol,
                );
                index_symbol
            }
        };
        // Then just add the computed name as a late bound declaration
        let decl_symbol = crate::typeofsymbol::payload_symbol(self.store().node(decl));
        let replaceable = decl_symbol
            .map(|s| {
                self.program_symbol(s)
                    .flags
                    .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
            })
            .unwrap_or(false);
        let existing = self.program_symbol(index_symbol).declarations.clone();
        if existing.is_empty() || !replaceable {
            self.program_symbol_mut(index_symbol).declarations.push(decl);
        }
    }

    /// Go: `func (c *Checker) addDeclarationToLateBoundSymbol(symbol, member, symbolFlags)`
    /// (checker.go:16481).
    fn add_declaration_to_late_bound_symbol(
        &mut self,
        symbol: SymbolId,
        member: NodeId,
        symbol_flags: SymbolFlags,
    ) {
        debug_assert!(
            self.program_symbol(symbol)
                .check_flags
                .intersects(tsc_ast::CheckFlags::LATE),
            "Expected a late-bound symbol."
        );
        let member_symbol =
            crate::typeofsymbol::payload_symbol(self.store().node(member));
        // PORT (FCS tranche 13): Go checker.go:16484 —
        // `c.lateBoundLinks.Get(member.Symbol()).lateSymbol = symbol`, the
        // raw-per-declaration "__computed" symbol → late-merged member
        // mapping that getLateBoundSymbol (checker.go:14694) reads. The
        // port previously never wrote it, so getSymbolOfDeclaration's
        // merged+late-bound composition resolved dynamically-named accessors
        // to their lone per-declaration symbols — paired `get [m]` /
        // `set [m](v)` never merged (the rust-extra 7032+7006 pairs on the
        // esDecorators classDeclaration-accessors records).
        if let Some(member_symbol) = member_symbol {
            self.late_bound_links.get(member_symbol).late_symbol = Some(symbol);
        }
        let member_flags = member_symbol
            .map(|s| self.program_symbol(s).flags)
            .unwrap_or(SymbolFlags::NONE);
        let replaceable = member_symbol
            .is_some_and(|s| {
                self.program_symbol(s)
                    .flags
                    .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
            });
        let existing = self.program_symbol(symbol).declarations.clone();
        if existing.is_empty() || !replaceable {
            self.program_symbol_mut(symbol).flags |= symbol_flags;
            self.program_symbol_mut(symbol).declarations.push(member);
        } else if self.program_symbol(symbol).flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
            && member_flags.intersects(SymbolFlags::METHOD)
        {
            // Remove all replacable-by-method members, along with their flags.
            let mut declarations: Vec<NodeId> = existing
                .into_iter()
                .filter(|&d| {
                    crate::typeofsymbol::payload_symbol(self.store().node(d))
                        .map(|s| {
                            !self
                                .program_symbol(s)
                                .flags
                                .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
                        })
                        .unwrap_or(true)
                })
                .collect();
            declarations.push(member);
            let old_flags = self.program_symbol(symbol).flags;
            let mut flags = SymbolFlags::TRANSIENT;
            for d in &declarations {
                if let Some(s) = crate::typeofsymbol::payload_symbol(self.store().node(*d)) {
                    flags |= self.program_symbol(s).flags;
                }
            }
            if old_flags.intersects(SymbolFlags::ACCESSOR) {
                flags |= SymbolFlags::ACCESSOR;
            }
            self.program_symbol_mut(symbol).flags = flags;
            self.program_symbol_mut(symbol).declarations = declarations;
        }
        if symbol_flags.intersects(SymbolFlags::VALUE) {
            // PORT (FCS tranche 47): Go ends addDeclarationToLateBoundSymbol
            // with `binder.SetValueDeclaration(symbol, member)` (checker.go:16503
            // → binder.go:4306) — a PRECEDENCE gate, not an overwrite: the
            // existing value declaration only loses to a non-assignment
            // declaration, or to a different-kind non-module declaration
            // against an effective module declaration. The port previously
            // overwrote unconditionally, flipping `value_declaration` to the
            // LAST late-bound declaration and inverting the
            // errorNextVariableOrPropertyDeclarationMustHaveSameType
            // first/next roles (TS2717 fired on the first declaration with
            // swapped type args instead of the subsequent one).
            let value_declaration = self.program_symbol(symbol).value_declaration;
            let replace = match value_declaration {
                None => true,
                Some(vd) => {
                    crate::spine::is_assignment_declaration(self.store(), vd)
                        && !crate::spine::is_assignment_declaration(self.store(), member)
                        || self.store().node(vd).kind != self.store().node(member).kind
                            && crate::spine::is_effective_module_declaration(self.store(), vd)
                }
            };
            if replace {
                self.program_symbol_mut(symbol).value_declaration = Some(member);
            }
        }
    }

    // Go: `func (c *Checker) getMembersOfSymbol(symbol) ast.SymbolTable` —
    // REAL body (checker.go:16510); the typenodes.rs carrier
    // `get_members_of_symbol` snapshots this through the
    // ResolvedMembers resolution.

    /// Go: `func (c *Checker) getExportsOfModule(moduleSymbol) ast.SymbolTable`
    /// (checker.go:16517).
    pub(crate) fn get_exports_of_module(&mut self, module_symbol: SymbolId) -> SymbolTable {
        if !self
            .module_symbol_links
            .get(module_symbol)
            .resolved_exports_computed
        {
            let (exports, type_only_export_star_map) =
                self.get_exports_of_module_worker(module_symbol);
            {
                let links = self.module_symbol_links.get(module_symbol);
                links.resolved_exports = exports.clone();
                links.type_only_export_star_map = type_only_export_star_map;
                links.resolved_exports_computed = true;
            }
        }
        self.module_symbol_links
            .get(module_symbol)
            .resolved_exports
            .clone()
    }

    /// Go: `func (c *Checker) getExportsOfModuleWorker(moduleSymbol) (exports, typeOnlyExportStarMap)`
    /// (checker.go:16534).
    fn get_exports_of_module_worker(
        &mut self,
        module_symbol: SymbolId,
    ) -> (SymbolTable, Option<FxHashMap<String, NodeId>>) {
        let mut visited_symbols: Vec<SymbolId> = Vec::new();
        let mut non_type_only_names: FxHashSet<String> = FxHashSet::default();
        let mut type_only_export_star_map: Option<FxHashMap<String, NodeId>> = None;

        // The recursive `visit` walk, unrolled iteratively (Rust closures
        // borrowing `self` recursively don't compose; the Go recursion is a
        // plain call chain, so a helper method carries it).
        let original_module = {
            let export_equals = self
                .program_symbol(module_symbol)
                .exports
                .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
                .copied();
            match export_equals {
                Some(e) if self.resolve_symbol_ex(e, false) != self.unknown_symbol() => {
                    Some(module_symbol)
                }
                _ => None,
            }
        };
        // A module defined by an 'export=' consists of one export that needs to be resolved
        let resolved = self.resolve_external_module_symbol_ex(Some(module_symbol), false);
        let mut exports = self.get_exports_of_module_visit(
            resolved,
            NodeId::NONE,
            false,
            &mut visited_symbols,
            &mut non_type_only_names,
            &mut type_only_export_star_map,
        );
        if exports.is_empty() {
            exports = SymbolTable::new();
        }
        // A CommonJS module defined by an 'export=' might also export typedefs, stored on the original module
        if let Some(original_module) = original_module {
            let original_exports = self.program_symbol(original_module).exports.clone();
            if original_exports.size() > 1 {
                for (name, symbol) in original_exports.entries() {
                    if name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                        || name == tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_STAR
                    {
                        continue;
                    }
                    let flags = self.get_symbol_flags_ex(*symbol, false, false);
                    if flags.intersects(SymbolFlags::TYPE | SymbolFlags::NAMESPACE)
                        && !flags.intersects(SymbolFlags::VALUE)
                        && exports.get(name).is_none()
                    {
                        exports.set(name.clone(), *symbol);
                    }
                }
            }
        }
        if let Some(map) = type_only_export_star_map.as_mut() {
            for name in non_type_only_names {
                map.remove(&name);
            }
        }
        (exports, type_only_export_star_map)
    }

    /// The recursive `visit` closure of Go's getExportsOfModuleWorker
    /// (checker.go:16534's inner walk).
    fn get_exports_of_module_visit(
        &mut self,
        symbol: Option<SymbolId>,
        export_star: NodeId,
        is_type_only: bool,
        visited_symbols: &mut Vec<SymbolId>,
        non_type_only_names: &mut FxHashSet<String>,
        type_only_export_star_map: &mut Option<FxHashMap<String, NodeId>>,
    ) -> SymbolTable {
        if !is_type_only && let Some(sym) = symbol {
            // Add non-type-only names before checking if we've visited this module,
            // because we might have visited it via an 'export type *', and visiting
            // again with 'export *' will override the type-onlyness of its exports.
            for name in self.program_symbol(sym).exports.keys() {
                non_type_only_names.insert(name.clone());
            }
        }
        let Some(symbol) = symbol else {
            return SymbolTable::new();
        };
        if self.program_symbol(symbol).exports.is_empty()
            || visited_symbols.contains(&symbol)
        {
            return SymbolTable::new();
        }
        visited_symbols.push(symbol);
        let mut symbols = self.program_symbol(symbol).exports.clone();
        // All export * declarations are collected in an __export symbol by the binder
        let export_stars = self
            .program_symbol(symbol)
            .exports
            .get(tsc_ast::INTERNAL_SYMBOL_NAME_EXPORT_STAR)
            .copied();
        if let Some(export_stars) = export_stars {
            let mut nested_symbols = SymbolTable::new();
            let mut lookup_table: FxHashMap<String, ExportCollision> = FxHashMap::default();
            let export_star_decls = self.program_symbol(export_stars).declarations.clone();
            for node in export_star_decls {
                let module_specifier = binder_node_data::module_specifier_of(self.store(), node);
                let attributes_type = crate::statements::get_import_attributes(self.store(), node)
                    .and_then(|a| self.get_type_from_import_attributes(Some(a)));
                let resolved_module = self.resolve_external_module_name_ex(
                    node,
                    module_specifier,
                    false,
                    attributes_type,
                );
                let node_is_type_only =
                    crate::statements::is_type_only_import_or_export_declaration(self.store(), node);
                let exported_symbols = self.get_exports_of_module_visit(
                    resolved_module,
                    node,
                    is_type_only || node_is_type_only,
                    visited_symbols,
                    non_type_only_names,
                    type_only_export_star_map,
                );
                self.extend_export_symbols(
                    &mut nested_symbols,
                    &exported_symbols,
                    Some(&mut lookup_table),
                    Some(node),
                );
            }
            for (id, s) in &lookup_table {
                // It's not an error if the file with multiple `export *`s with duplicate names exports a member with that name itself
                if *id == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                    || s.exports_with_duplicate.is_empty()
                    || symbols.get(id).is_some()
                {
                    continue;
                }
                let specifier_text = s.specifier_text.clone();
                for node in &s.exports_with_duplicate {
                    self.error(
                        *node,
                        &diag::MODULE_0_HAS_ALREADY_EXPORTED_A_MEMBER_NAMED_1_CONSIDER_EXPLICITLY_RE_EXPORTING_TO_RESOLVE_THE_AMBIGUITY,
                        &[specifier_text.clone(), id.clone()],
                    );
                }
            }
            self.extend_export_symbols(&mut symbols, &nested_symbols, None, None);
        }
        if export_star != NodeId::NONE
            && crate::statements::is_type_only_import_or_export_declaration(self.store(), export_star)
        {
            if type_only_export_star_map.is_none() {
                *type_only_export_star_map = Some(FxHashMap::default());
            }
            let map = type_only_export_star_map.as_mut().unwrap();
            for name in symbols.keys() {
                map.insert(name.clone(), export_star);
            }
        }
        symbols
    }

    /// Go: `func (c *Checker) extendExportSymbols(target, source, lookupTable, exportNode)`
    /// (checker.go:16621).
    fn extend_export_symbols(
        &mut self,
        target: &mut SymbolTable,
        source: &SymbolTable,
        lookup_table: Option<&mut FxHashMap<String, ExportCollision>>,
        export_node: Option<NodeId>,
    ) {
        let mut lookup_table = lookup_table;
        for (id, source_symbol) in source.entries() {
            if *id == INTERNAL_SYMBOL_NAME_DEFAULT {
                continue;
            }
            let target_symbol = target.get(id).copied();
            match target_symbol {
                None => {
                    target.set(id.clone(), *source_symbol);
                    if let (Some(table), Some(node)) = (lookup_table.as_deref_mut(), export_node) {
                        let specifier = binder_node_data::module_specifier_of(self.store(), node);
                        let specifier_text = specifier
                            .map(|s| tsc_ast::node_text(self.store(), s).to_string())
                            .unwrap_or_default();
                        table.insert(
                            id.clone(),
                            ExportCollision {
                                specifier_text,
                                exports_with_duplicate: Vec::new(),
                            },
                        );
                    }
                }
                Some(target_symbol) => {
                    if let (Some(table), Some(node)) = (lookup_table.as_deref_mut(), export_node) {
                        let resolved_target = self.resolve_symbol(target_symbol);
                        let resolved_source = self.resolve_symbol(*source_symbol);
                        if resolved_target != resolved_source {
                            let entry = table
                                .entry(id.clone())
                                .or_insert_with(|| ExportCollision {
                                    specifier_text: String::new(),
                                    exports_with_duplicate: Vec::new(),
                                });
                            entry.exports_with_duplicate.push(node);
                        }
                    }
                }
            }
        }
    }

    // Go: `func (c *Checker) getSymbolFlags(symbol) ast.SymbolFlags` (checker.go:16749)
    // — the exports.rs carrier delegates to typeofsymbol's getSymbolFlagsEx;
    // no separate body needed here.

    // ── external-module resolution (Go checker.go:15395–16060) ──

    /// Go: `func (c *Checker) resolveExternalModuleName(location, moduleReferenceExpression, ignoreErrors, importAttributesType) *ast.Symbol`
    /// (checker.go:15395).
    pub(crate) fn resolve_external_module_name_ex(
        &mut self,
        location: NodeId,
        module_reference_expression: Option<NodeId>,
        ignore_errors: bool,
        import_attributes_type: Option<TypeId>,
    ) -> Option<SymbolId> {
        // W3T7: getCannotResolveModuleNameErrorForSpecificModule's
        // node-core-module probe (Go checker.go:15404 — core.NodeCoreModules
        // 'fs' → the TS2591 @types/node hint; UsesWildcardTypes picks the
        // no-tsconfig variant, the only one the harness reaches).
        let error_message: Option<&'static Message> =
            Some(self.get_cannot_resolve_module_name_error_for_specific_module(
                module_reference_expression,
            ));
        let ignore_errors = ignore_errors || self.compiler_options.no_check == Tristate::TSTrue;
        let not_found = if ignore_errors { None } else { error_message };
        self.resolve_external_module_name_worker(
            location,
            module_reference_expression,
            not_found,
            ignore_errors,
            false,
            import_attributes_type,
        )
    }

    /// Go: `func (c *Checker) getCannotResolveModuleNameErrorForSpecificModule(moduleName) *diagnostics.Message`
    /// (checker.go:15404).
    fn get_cannot_resolve_module_name_error_for_specific_module(
        &self,
        module_name: Option<NodeId>,
    ) -> &'static Message {
        if let Some(module_name) = module_name
            && tsc_ast::is_string_literal(self.store().node(module_name))
            && tsc_core::nodemodules::node_core_modules()
                .has(tsc_ast::node_text(self.store(), module_name))
        {
            if self.compiler_options.uses_wildcard_types() {
                return &diag::CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE;
            }
            return &diag::CANNOT_FIND_NAME_0_DO_YOU_NEED_TO_INSTALL_TYPE_DEFINITIONS_FOR_NODE_TRY_NPM_I_SAVE_DEV_TYPES_SLASHNODE_AND_THEN_ADD_NODE_TO_THE_TYPES_FIELD_IN_YOUR_TSCONFIG;
        }
        &diag::CANNOT_FIND_MODULE_0_OR_ITS_CORRESPONDING_TYPE_DECLARATIONS
    }

    /// Go: `func (c *Checker) resolveExternalModuleNameWorker(location, moduleReferenceExpression, moduleNotFoundError, ignoreErrors, isForAugmentation, importAttributesType) *ast.Symbol`
    /// (checker.go:15416).
    pub(crate) fn resolve_external_module_name_worker(
        &mut self,
        location: NodeId,
        module_reference_expression: Option<NodeId>,
        module_not_found_error: Option<&'static Message>,
        ignore_errors: bool,
        is_for_augmentation: bool,
        import_attributes_type: Option<TypeId>,
    ) -> Option<SymbolId> {
        let module_reference = module_reference_expression?;
        if tsc_ast::utilities::is_string_literal_like(self.store().node(module_reference)) {
            let parent = self.store().node(module_reference).parent.get();
            if parent != NodeId::NONE && is_source_phase_import(self.store(), parent) {
                return None;
            }
            let module_reference_text =
                tsc_ast::node_text(self.store(), module_reference).to_string();
            let error_node = if !ignore_errors {
                Some(module_reference)
            } else {
                None
            };
            return self.resolve_external_module(
                location,
                &module_reference_text,
                module_not_found_error,
                error_node,
                is_for_augmentation,
                import_attributes_type,
            );
        }
        None
    }

    /// Go: `func (c *Checker) getResolutionModeOverride(node *ast.ImportAttributes, reportErrors bool) core.ResolutionMode`
    /// (checker.go:3386) — the reporting wrapper over the ast walk: with
    /// reportErrors the grammarErrorOnNode callback fires TS1453 on a
    /// resolution-mode value that is neither "import" nor "require".
    /// PORT(FCS tranche 28).
    pub(crate) fn get_resolution_mode_override_checked(
        &mut self,
        node: NodeId,
        report_errors: bool,
    ) -> ResolutionMode {
        if !report_errors {
            let (mode, _) = get_resolution_mode_override(self.store(), node);
            return mode;
        }
        // Collect the grammar-error site during the borrow-free walk, then
        // emit after the store borrow ends (Go's callback fires inline; the
        // emission is order-equivalent — the error is the walk's only
        // side effect).
        let mut grammar_error: Option<(NodeId, &'static Message)> = None;
        let (mode, _) = get_resolution_mode_override_ex(
            self.store(),
            node,
            Some(|value: NodeId, message: &'static Message| {
                grammar_error = Some((value, message));
            }),
        );
        if let Some((value, message)) = grammar_error {
            // Go: c.grammarErrorOnNode (checker.go:3391) — skipped when the
            // source file has parse diagnostics.
            self.grammar_error_on_node(value, message, &[]);
        }
        mode
    }

    /// Go: `func (c *Checker) resolveExternalModule(location, moduleReference, moduleNotFoundError, errorNode, isForAugmentation, importAttributesType) *ast.Symbol`
    /// (checker.go:15453). REAL as of FCS tranche 7: the ambient-module
    /// half, the resolved-module half (program.GetResolvedModule /
    /// GetSourceFileForResolvedModule through the Program trait's
    /// FCS-tranche-7 seam) and the module-not-found ladder are all Go-exact;
    /// the twin streams (Go's confSemanticDiagnosticsOfUnit map-FS program)
    /// are the bar.
    pub(crate) fn resolve_external_module(
        &mut self,
        // PORT: `location` is the ancestor-walk root of Go's context-specifier
        // derivation (checker.go:15470-15506) — REAL as of FCS tranche 6
        // (the mode derivation reads it); the resolved-module lookup reads
        // the mode as its cache key (checker.go:15511).
        location: NodeId,
        module_reference: &str,
        module_not_found_error: Option<&'static Message>,
        error_node: Option<NodeId>,
        // PORT: `isForAugmentation` selects the augmentation-flavored
        // untyped-module diagnostic (checker.go:15702).
        is_for_augmentation: bool,
        import_attributes_type: Option<TypeId>,
    ) -> Option<SymbolId> {
        if let Some(error_node) = error_node.filter(|_| module_reference.starts_with("@types/")) {
            let without_at_type_prefix = &module_reference["@types/".len()..];
            self.error(
                error_node,
                &diag::CANNOT_IMPORT_TYPE_DECLARATION_FILES_CONSIDER_IMPORTING_0_INSTEAD_OF_1,
                &[without_at_type_prefix.to_string(), module_reference.to_string()],
            );
        }
        let import_attributes_type =
            import_attributes_type.unwrap_or_else(|| self.empty_object_type());

        let ambient_module = self.try_find_ambient_module_real(module_reference, true);
        if let Some(ambient_module) = ambient_module {
            return self.try_resolve_pattern_ambient_module(
                Some(ambient_module),
                module_reference,
                import_attributes_type,
            );
        }

        // Go: importingSourceFile + the usage-location mode
        // (checker.go:15508-15510) — the mode the resolution cache is keyed
        // by (the file loader derived the same mode when it filled
        // p.resolvedModules).
        let importing_source_file = self.get_source_file_node_of(location);
        let mode = self.get_mode_for_usage_location(importing_source_file, location);

        // Go: `resolvedModule := c.program.GetResolvedModule(...)`
        // (checker.go:15511) — Go's nil (cache miss) is the trait's None;
        // the loader's recorded failure is Go's `&module.ResolvedModule{}`
        // (the trait's `Some` of the empty record).
        let resolved_module = self
            .program
            .get_resolved_module(importing_source_file, module_reference, mode);

        // Go: the resolution diagnostic (checker.go:15513-15515) —
        // module.GetResolutionDiagnostic (module/util.go:132), guarded by
        // errorNode != nil && resolvedModule.IsResolved().
        let resolution_diagnostic = if error_node.is_some()
            && tsc_module::ResolvedModule::is_resolved(resolved_module.as_deref())
        {
            let resolved = resolved_module.as_deref().expect("is_resolved above");
            let importing_file = self.source_file_of(importing_source_file);
            tsc_module::get_resolution_diagnostic(&self.compiler_options, resolved, importing_file)
        } else {
            None
        };

        // Go: `if resolvedModule.IsResolved() && (resolutionDiagnostic == nil ||
        // resolutionDiagnostic == Module_0_was_resolved_to_1_but_jsx_is_not_set)`
        // (checker.go:15517-15519) — the message comparison is by code
        // (Go compares the *diagnostics.Message pointers).
        let jsx_not_set_code = diag::MODULE_0_WAS_RESOLVED_TO_1_BUT_JSX_IS_NOT_SET.code();
        let source_file = if tsc_module::ResolvedModule::is_resolved(resolved_module.as_deref())
            && (resolution_diagnostic.is_none()
                || resolution_diagnostic.is_some_and(|m| m.code() == jsx_not_set_code))
        {
            resolved_module
                .as_deref()
                .and_then(|resolved| self.program.get_source_file_for_resolved_module(resolved))
        } else {
            None
        };

        if let Some(source_file) = source_file {
            let resolved = resolved_module.as_deref().expect("is_resolved above");
            // If there's a resolutionDiagnostic we need to report it even if a
            // sourceFile is found (checker.go:15525-15527).
            if let (Some(error_node), Some(resolution_diagnostic)) = (error_node, resolution_diagnostic)
            {
                self.error(
                    error_node,
                    resolution_diagnostic,
                    &[
                        module_reference.to_string(),
                        resolved.resolved_file_name.as_string().to_string(),
                    ],
                );
            }

            // Go: `if errorNode != nil && !resolvedModule.IsCustomResolution`
            // (checker.go:15529) — the resolved-using-ts-extension and
            // rewriteRelativeImportExtensions arms. PORT(FCS tranche 7).
            if let Some(error_node) = error_node.filter(|_| !resolved.is_custom_resolution) {
                let store = self.store();
                let emittable_import_ancestor =
                    find_ancestor_matching(store, location, is_emittable_import);
                if resolved.resolved_using_ts_extension
                    && tsc_tspath::is_declaration_file_name(module_reference)
                {
                    if emittable_import_ancestor.is_some() {
                        // Go: TryExtractTSExtension + the panic when the
                        // extraction fails (checker.go:15535).
                        let ts_extension = tsc_tspath::try_extract_ts_extension(module_reference);
                        assert!(
                            !ts_extension.is_empty(),
                            "should be able to extract TS extension from string that passes IsDeclarationFileName"
                        );
                        let suggested =
                            self.get_suggested_import_source(module_reference, ts_extension, mode);
                        self.error(
                            error_node,
                            &diag::A_DECLARATION_FILE_CANNOT_BE_IMPORTED_WITHOUT_IMPORT_TYPE_DID_YOU_MEAN_TO_IMPORT_AN_IMPLEMENTATION_FILE_0_INSTEAD,
                            &[suggested],
                        );
                    }
                } else if resolved.resolved_using_ts_extension && {
                    let importing_name = self.program.source_file_name(importing_source_file);
                    !self
                        .compiler_options
                        .allow_importing_ts_extensions_from(&tsc_tspath::RootedFilePath::from(
                            importing_name.as_str(),
                        ))
                } {
                    if emittable_import_ancestor.is_some() {
                        let mut ts_extension =
                            tsc_tspath::try_extract_ts_extension(module_reference).to_string();
                        if ts_extension.is_empty() {
                            // Go: best-effort strings.Contains fallback
                            // (checker.go:15551) — wildcard patterns matching
                            // a TS extension mid-specifier.
                            for ext in tsc_tspath::SUPPORTED_TS_EXTENSIONS_FLAT {
                                if module_reference.contains(ext) {
                                    ts_extension = ext.to_string();
                                    break;
                                }
                            }
                        }
                        assert!(
                            !ts_extension.is_empty(),
                            "should be able to extract TS extension from string when resolvedUsingTsExtension is true"
                        );
                        self.error(
                            error_node,
                            &diag::AN_IMPORT_PATH_CAN_ONLY_END_WITH_A_0_EXTENSION_WHEN_ALLOWIMPORTINGTSEXTENSIONS_IS_ENABLED,
                            &[ts_extension],
                        );
                    }
                } else if self.compiler_options.rewrite_relative_import_extensions.is_true()
                    && store.node(location).flags & NF::AMBIENT == NF::NONE
                    && !tsc_tspath::is_declaration_file_name(module_reference)
                    && !is_literal_import_type_node(store, location)
                    && !is_part_of_type_only_import_or_export_declaration(store, location)
                {
                    let should_rewrite =
                        should_rewrite_module_specifier(module_reference, &self.compiler_options);
                    if !resolved.resolved_using_ts_extension && should_rewrite {
                        // Go: relativeToSourceFile = the resolved file's
                        // relative module specifier
                        // (checker.go:15573-15579) — CaseSensitivity() is the
                        // twin FS's case-sensitive read.
                        let importing_name = self.program.source_file_name(importing_source_file);
                        let relative_to_source_file = tsc_tspath::CaseSensitivity::CaseSensitive
                            .relative_path_from_file(
                                &tsc_tspath::RootedFilePath::from(importing_name.as_str()),
                                &resolved.resolved_file_name,
                            )
                            .map(|relative| {
                                relative.as_module_specifier().as_string().to_string()
                            })
                            .unwrap_or_else(|| {
                                resolved.resolved_file_name.as_string().to_string()
                            });
                        self.error(
                            error_node,
                            &diag::THIS_RELATIVE_IMPORT_PATH_IS_UNSAFE_TO_REWRITE_BECAUSE_IT_LOOKS_LIKE_A_FILE_NAME_BUT_ACTUALLY_RESOLVES_TO_0,
                            &[relative_to_source_file],
                        );
                    } else if resolved.resolved_using_ts_extension
                        && !should_rewrite
                        && self.program.source_file_may_be_emitted(source_file)
                    {
                        self.error(
                            error_node,
                            &diag::THIS_IMPORT_USES_A_0_EXTENSION_TO_RESOLVE_TO_AN_INPUT_TYPESCRIPT_FILE_BUT_WILL_NOT_BE_REWRITTEN_DURING_EMIT_BECAUSE_IT_IS_NOT_A_RELATIVE_PATH,
                            &[tsc_tspath::get_any_extension_from_path(
                                module_reference,
                                &[],
                                tsc_tspath::CaseSensitivity::CaseSensitive,
                            )
                            .to_string()],
                        );
                    }
                    // Go's third arm (resolvedUsingTsExtension && shouldRewrite)
                    // reads program.GetRedirectForResolution — the per-unit
                    // twin has no project references (no redirects), so the
                    // arm is a no-op there. PORT(FCS tranche 7).
                }
            }

            // Go: `if sourceFile.Symbol != nil` (checker.go:15584) — the
            // resolved module's own symbol.
            let file_symbol = self
                .source_file_of(source_file)
                .and_then(|sf| sf.node(source_file).as_source_file().expect("SourceFile data").symbol);
            if let Some(file_symbol) = file_symbol {
                if let Some(error_node) = error_node {
                    // Go: the external-library untyped-import suggestion
                    // (checker.go:15585-15587) — errorOnImplicitAnyModule with
                    // isError=false routes to the suggestion collection
                    // (invisible to the twin records; both drivers agree).
                    if resolved.is_external_library_import
                        && !resolution_extension_is_ts_or_json(&resolved.extension)
                    {
                        self.error_on_implicit_any_module(
                            false,
                            error_node,
                            mode,
                            resolved,
                            module_reference,
                        );
                    }
                    // Go: the moduleKind Node16/Node18 sync-import mismatch
                    // block (checker.go:15588-15626). Unreachable in the
                    // per-unit twin (the resolved file IS the importing
                    // file, so the two default modes can never disagree),
                    // but ported Go-exactly. PORT(FCS tranche 7).
                    if self.module_kind == ModuleKind::Node16
                        || self.module_kind == ModuleKind::Node18
                    {
                        let is_sync_import = {
                            let store = self.store();
                            self.default_resolution_mode_for_file(importing_source_file)
                                == ModuleKind::CommonJS
                                && find_ancestor_matching(store, location, |store, node| {
                                    crate::resolvecall::is_import_call(store, node)
                                })
                                .is_none()
                                || find_ancestor_matching(store, location, |_, node| {
                                    tsc_ast::is_import_equals_declaration(store.node(node))
                                })
                                .is_some()
                        };
                        let override_host = find_ancestor_matching(
                            self.store(),
                            location,
                            is_resolution_mode_override_host,
                        );
                        if is_sync_import
                            && self.default_resolution_mode_for_file(source_file)
                                == ModuleKind::ESNext
                            && !has_resolution_mode_override(self.store(), override_host)
                        {
                            if find_ancestor_kind(self.store(), location, Kind::ImportEqualsDeclaration)
                                .is_some()
                            {
                                // ImportEquals in an ESM file resolving to another ESM file
                                self.error(
                                    error_node,
                                    &diag::MODULE_0_CANNOT_BE_IMPORTED_USING_THIS_CONSTRUCT_THE_SPECIFIER_ONLY_RESOLVES_TO_AN_ES_MODULE_WHICH_CANNOT_BE_IMPORTED_WITH_REQUIRE_USE_AN_ECMASCRIPT_IMPORT_INSTEAD,
                                    &[module_reference.to_string()],
                                );
                            } else {
                                // CJS file resolving to an ESM file
                                let diagnostic_details = {
                                    let importing_name =
                                        self.program.source_file_name(importing_source_file);
                                    let ext = tsc_tspath::try_get_extension_from_path(
                                        &importing_name,
                                    )
                                    .to_string();
                                    if ext == tsc_tspath::EXTENSION_TS
                                        || ext == tsc_tspath::EXTENSION_JS
                                        || ext == tsc_tspath::EXTENSION_TSX
                                        || ext == tsc_tspath::EXTENSION_JSX
                                    {
                                        Some(self.create_mode_mismatch_details(importing_source_file, error_node))
                                    } else {
                                        None
                                    }
                                };
                                let message = match override_host {
                                    Some(host)
                                        if self.store().node(host).kind == Kind::ImportDeclaration =>
                                    {
                                        let import_clause = match &self.store().node(host).data {
                                            tsc_ast::NodeData::ImportDeclaration(d) => d.import_clause,
                                            _ => None,
                                        };
                                        if import_clause.is_some_and(|clause| {
                                            // Go: ImportClause().IsTypeOnly() — the
                                            // port models the `type` phase modifier.
                                            matches!(&self.store().node(clause).data,
                                                tsc_ast::NodeData::ImportClause(c)
                                                    if c.phase_modifier == Some(Kind::TypeKeyword))
                                        }) {
                                            &diag::TYPE_ONLY_IMPORT_OF_AN_ECMASCRIPT_MODULE_FROM_A_COMMONJS_MODULE_MUST_HAVE_A_RESOLUTION_MODE_ATTRIBUTE
                                        } else {
                                            &diag::THE_CURRENT_FILE_IS_A_COMMONJS_MODULE_WHOSE_IMPORTS_WILL_PRODUCE_REQUIRE_CALLS_HOWEVER_THE_REFERENCED_FILE_IS_AN_ECMASCRIPT_MODULE_AND_CANNOT_BE_IMPORTED_WITH_REQUIRE_CONSIDER_WRITING_A_DYNAMIC_IMPORT_0_CALL_INSTEAD
                                        }
                                    }
                                    Some(host) if self.store().node(host).kind == Kind::ImportType => {
                                        &diag::TYPE_IMPORT_OF_AN_ECMASCRIPT_MODULE_FROM_A_COMMONJS_MODULE_MUST_HAVE_A_RESOLUTION_MODE_ATTRIBUTE
                                    }
                                    _ => {
                                        &diag::THE_CURRENT_FILE_IS_A_COMMONJS_MODULE_WHOSE_IMPORTS_WILL_PRODUCE_REQUIRE_CALLS_HOWEVER_THE_REFERENCED_FILE_IS_AN_ECMASCRIPT_MODULE_AND_CANNOT_BE_IMPORTED_WITH_REQUIRE_CONSIDER_WRITING_A_DYNAMIC_IMPORT_0_CALL_INSTEAD
                                    }
                                };
                                let diagnostic = u::new_diagnostic_chain_for_node(
                                    diagnostic_details.as_ref(),
                                    self.store(),
                                    error_node,
                                    message,
                                    vec![module_reference.to_string()],
                                );
                                self.add_diagnostic(diagnostic);
                            }
                        }
                    }
                }
                return self.try_resolve_pattern_ambient_module(
                    Some(self.get_merged_symbol(file_symbol)),
                    module_reference,
                    import_attributes_type,
                );
            }
            // Go: the resolved file has no module symbol (a script) —
            // checker.go:15658-15666.
            let pattern_ambient_module =
                self.try_resolve_pattern_ambient_module(None, module_reference, import_attributes_type);
            if pattern_ambient_module.is_some() {
                return pattern_ambient_module;
            }
            if let (Some(error_node), Some(_)) = (error_node, module_not_found_error) {
                if !is_side_effect_import(self.store(), error_node) {
                    self.error(
                        error_node,
                        &diag::FILE_0_IS_NOT_A_MODULE,
                        &[resolved.resolved_file_name.as_string().to_string()],
                    );
                }
            }
            return None;
        }

        // Go: the not-resolved tail (checker.go:15668-15746). PORT(FCS
        // tranche 7): the resolved-module arms are REAL now; the
        // projectReference redirect arm reads
        // program.GetProjectReferenceFromSource — the per-unit twin has no
        // project references (Go's twin redirect is nil there too).
        let pattern_ambient_module =
            self.try_resolve_pattern_ambient_module(None, module_reference, import_attributes_type);
        if pattern_ambient_module.is_some() {
            return pattern_ambient_module;
        }

        let error_node = error_node?;

        // Go: the untyped-module arm (checker.go:15697-15711) — a resolved
        // JS-family module without declarations (or the 7016 diagnostic
        // itself; Go compares the message pointer, Rust the code).
        let module_not_found_7016_code =
            diag::COULD_NOT_FIND_A_DECLARATION_FILE_FOR_MODULE_0_1_IMPLICITLY_HAS_AN_ANY_TYPE.code();
        let untyped_module = (tsc_module::ResolvedModule::is_resolved(resolved_module.as_deref())
            && !resolution_extension_is_ts_or_json(
                &resolved_module.as_deref().expect("is_resolved above").extension,
            )
            && resolution_diagnostic.is_none())
            || resolution_diagnostic.is_some_and(|m| m.code() == module_not_found_7016_code);
        if untyped_module {
            let resolved = resolved_module.as_deref().expect("is_resolved above");
            if is_for_augmentation {
                self.error(
                    error_node,
                    &diag::INVALID_MODULE_NAME_IN_AUGMENTATION_MODULE_0_RESOLVES_TO_AN_UNTYPED_MODULE_AT_1_WHICH_CANNOT_BE_AUGMENTED,
                    &[
                        module_reference.to_string(),
                        resolved.resolved_file_name.as_string().to_string(),
                    ],
                );
            } else {
                self.error_on_implicit_any_module(
                    self.compiler_options.no_implicit_any == Tristate::TSTrue
                        && module_not_found_error.is_some(),
                    error_node,
                    mode,
                    resolved,
                    module_reference,
                );
            }
            return None;
        }

        if let Some(message) = module_not_found_error {
            // Go: the projectReference redirect probe (checker.go:15714)
            // — the per-unit twin has no project references; skip (the
            // same reads fail in Go's twin).
            if let Some(resolution_diagnostic) = resolution_diagnostic {
                let resolved = resolved_module.as_deref().expect("is_resolved above");
                self.error(
                    error_node,
                    resolution_diagnostic,
                    &[
                        module_reference.to_string(),
                        resolved.resolved_file_name.as_string().to_string(),
                    ],
                );
            } else {
                // Go: `isExtensionlessRelativePathImport` (checker.go:15727).
                let is_extensionless_relative_path_import = tsc_tspath::path_is_relative(module_reference)
                    && !tsc_tspath::has_extension(module_reference);
                // Go: `resolutionIsNode16OrNext` — the checker's own
                // moduleResolutionKind field (checker.go:15728).
                let resolution_is_node16_or_next = self.module_resolution_kind
                    == ModuleResolutionKind::Node16
                    || self.module_resolution_kind == ModuleResolutionKind::NodeNext;
                if !self.compiler_options.get_resolve_json_module()
                    && tsc_tspath::file_extension_is(module_reference, tsc_tspath::EXTENSION_JSON)
                {
                    self.error(
                        error_node,
                        &diag::CANNOT_FIND_MODULE_0_CONSIDER_USING_RESOLVEJSONMODULE_TO_IMPORT_MODULE_WITH_JSON_EXTENSION,
                        &[module_reference.to_string()],
                    );
                } else if mode == ResolutionModeESM
                    && resolution_is_node16_or_next
                    && is_extensionless_relative_path_import
                {
                    let mut suggested_ext = String::new();
                    if !tsc_tspath::has_trailing_directory_separator(module_reference) {
                        let file_name = self.program.source_file_name(importing_source_file);
                        let absolute_ref = tsc_tspath::RootedFilePath::from(file_name.as_str())
                            .directory()
                            .resolve_file(module_reference);
                        suggested_ext = self.get_suggested_import_extension(&absolute_ref);
                    }
                    if !suggested_ext.is_empty() {
                        self.error(
                            error_node,
                            &diag::RELATIVE_IMPORT_PATHS_NEED_EXPLICIT_FILE_EXTENSIONS_IN_ECMASCRIPT_IMPORTS_WHEN_MODULERESOLUTION_IS_NODE16_OR_NODENEXT_DID_YOU_MEAN_0,
                            &[format!("{module_reference}{suggested_ext}")],
                        );
                    } else {
                        self.error(
                            error_node,
                            &diag::RELATIVE_IMPORT_PATHS_NEED_EXPLICIT_FILE_EXTENSIONS_IN_ECMASCRIPT_IMPORTS_WHEN_MODULERESOLUTION_IS_NODE16_OR_NODENEXT_CONSIDER_ADDING_AN_EXTENSION_TO_THE_IMPORT_PATH,
                            &[],
                        );
                    }
                } else if resolved_module
                    .as_deref()
                    .is_some_and(|resolved| !resolved.alternate_result.is_empty())
                {
                    // Go: the AlternateResult chain (checker.go:15740) —
                    // createModuleNotFoundChain + the chain-topped
                    // moduleNotFoundError.
                    let error_info = self.create_module_not_found_chain(
                        resolved_module.as_deref(),
                        error_node,
                        module_reference,
                        mode,
                        module_reference,
                    );
                    let diagnostic = u::new_diagnostic_chain_for_node(
                        Some(&error_info),
                        self.store(),
                        error_node,
                        message,
                        vec![module_reference.to_string()],
                    );
                    self.add_diagnostic(diagnostic);
                } else {
                    self.error(error_node, message, &[module_reference.to_string()]);
                }
            }
        }
        None
    }

    /// Go: `func (c *Checker) getSuggestedImportSource(moduleReference, tsExtension, mode) string`
    /// (checker.go:15815). PORT(FCS tranche 7) — the TS2305 declaration
    /// arm's suggested source.
    fn get_suggested_import_source(
        &self,
        module_reference: &str,
        ts_extension: &str,
        mode: ResolutionMode,
    ) -> String {
        let import_source_without_extension =
            tsc_tspath::remove_extension(module_reference, ts_extension).to_string();

        // Direct users to import source with .js extension if outputting an
        // ES module. @see https://github.com/microsoft/TypeScript/issues/42151
        if self.module_kind.is_non_node_esm() || mode == ModuleKind::ESNext {
            let prefer_ts = tsc_tspath::is_declaration_file_name(module_reference)
                && self.compiler_options.get_allow_importing_ts_extensions();
            let ext = match ts_extension {
                ext if ext == tsc_tspath::EXTENSION_MTS || ext == tsc_tspath::EXTENSION_DMTS => {
                    if prefer_ts { ".mts" } else { ".mjs" }
                }
                ext if ext == tsc_tspath::EXTENSION_CTS || ext == tsc_tspath::EXTENSION_DCTS => {
                    if prefer_ts { ".cts" } else { ".cjs" }
                }
                _ => if prefer_ts { ".ts" } else { ".js" },
            };

            return format!("{import_source_without_extension}{ext}");
        }

        import_source_without_extension
    }

    /// Go: `func (c *Checker) errorOnImplicitAnyModule(isError, errorNode, mode, resolvedModule, moduleReference)`
    /// (checker.go:15862). PORT(FCS tranche 7): the chain entry
    /// (createModuleNotFoundChain) is invisible to the twin records — the
    /// top message (the 7016) is what renders — and Go's isError=false arm
    /// routes to the suggestion collection, which the driver twins never
    /// read.
    fn error_on_implicit_any_module(
        &mut self,
        is_error: bool,
        error_node: NodeId,
        mode: ResolutionMode,
        resolved_module: &tsc_module::ResolvedModule,
        module_reference: &str,
    ) {
        if is_side_effect_import(self.store(), error_node) {
            return;
        }

        let error_info = if !tsc_tspath::is_external_module_name_relative(module_reference)
            && !resolved_module.package_id.name.is_empty()
        {
            Some(self.create_module_not_found_chain(
                Some(resolved_module),
                error_node,
                module_reference,
                mode,
                &resolved_module.package_id.name,
            ))
        } else {
            None
        };
        let diagnostic = u::new_diagnostic_chain_for_node(
            error_info.as_ref(),
            self.store(),
            error_node,
            &diag::COULD_NOT_FIND_A_DECLARATION_FILE_FOR_MODULE_0_1_IMPLICITLY_HAS_AN_ANY_TYPE,
            vec![
                module_reference.to_string(),
                resolved_module.resolved_file_name.as_string().to_string(),
            ],
        );
        self.add_error_or_suggestion(is_error, diagnostic);
    }

    /// Go: `func (c *Checker) createModuleNotFoundChain(resolvedModule, errorNode, moduleReference, mode, packageName) *ast.Diagnostic`
    /// (checker.go:15883) + `CreateModuleNotFoundChain` (utilities.go:1839).
    /// PORT(FCS tranche 7): the chain entry; program.GetPackagesMap() is
    /// the per-unit twin's EMPTY map (resolution only ever hits the unit
    /// itself, whose package_id is empty), so the packages arms reduce to
    /// the default npm-suggestion arm — the same answer Go's twin produces.
    fn create_module_not_found_chain(
        &mut self,
        resolved_module: Option<&tsc_module::ResolvedModule>,
        error_node: NodeId,
        module_reference: &str,
        // PORT: Go re-fetches program.GetResolvedModule(file, ref, mode) for
        // the chain; the port receives the same resolved module directly, so
        // the mode key is not read here.
        _mode: ResolutionMode,
        package_name: &str,
    ) -> tsc_ast::Diagnostic {
        if let Some(resolved_module) =
            resolved_module.filter(|resolved| !resolved.alternate_result.is_empty())
        {
            let mut package_name = package_name.to_string();
            if resolved_module
                .alternate_result
                .contains_lowercase_directory_sequence("/node_modules/@types/")
            {
                package_name = format!(
                    "@types/{}",
                    tsc_module::mangle_scoped_package_name(&package_name)
                );
            }
            return u::new_diagnostic_chain_for_node(
                None,
                self.store(),
                error_node,
                &diag::THERE_ARE_TYPES_AT_0_BUT_THIS_RESULT_COULD_NOT_BE_RESOLVED_WHEN_RESPECTING_PACKAGE_JSON_EXPORTS_THE_1_LIBRARY_MAY_NEED_TO_UPDATE_ITS_PACKAGE_JSON_OR_TYPINGS,
                vec![
                    resolved_module.alternate_result.as_string().to_string(),
                    package_name,
                ],
            );
        }
        // Go: packagesMap lookups (GetTypesPackageName / mangle) — the
        // twin's map is empty, so the fall-through arm.
        u::new_diagnostic_chain_for_node(
            None,
            self.store(),
            error_node,
            &diag::TRY_NPM_I_SAVE_DEV_TYPES_SLASH_1_IF_IT_EXISTS_OR_ADD_A_NEW_DECLARATION_D_TS_FILE_CONTAINING_DECLARE_MODULE_0,
            vec![
                module_reference.to_string(),
                tsc_module::mangle_scoped_package_name(package_name),
            ],
        )
    }

    /// Go: `func (c *Checker) createModeMismatchDetails(sourceFile, errorNode) *ast.Diagnostic`
    /// (checker.go:15901) + `CreateModeMismatchDetails` (utilities.go:1875).
    /// The chain anchors at the error node (Go's NewDiagnosticForNode).
    /// PORT(FCS tranche 7): the chain entry of the node16/18 mismatch arm
    /// (unreachable in the per-unit twin — the importing file IS the
    /// resolved file — but ported Go-exactly).
    fn create_mode_mismatch_details(&self, source_file: NodeId, error_node: NodeId) -> tsc_ast::Diagnostic {
        let file_name = self.program.source_file_name(source_file);
        let ext = tsc_tspath::try_get_extension_from_path(&file_name).to_string();
        let target_ext = if ext == tsc_tspath::EXTENSION_TS {
            tsc_tspath::EXTENSION_MTS
        } else if ext == tsc_tspath::EXTENSION_JS {
            tsc_tspath::EXTENSION_MJS
        } else {
            ""
        };
        let meta = self.program.source_file_meta_data(source_file);
        let package_json_type = meta.package_json_type.as_str();
        let package_json_directory = &meta.package_json_directory;

        if !package_json_directory.as_string().is_empty() && package_json_type.is_empty() {
            let package_json = package_json_directory
                .resolve_file("package.json")
                .as_string()
                .to_string();
            if !target_ext.is_empty() {
                return u::new_diagnostic_chain_for_node(
                    None,
                    self.store(),
                    source_file,
                    &diag::TO_CONVERT_THIS_FILE_TO_AN_ECMASCRIPT_MODULE_CHANGE_ITS_FILE_EXTENSION_TO_0_OR_ADD_THE_FIELD_TYPE_COLON_MODULE_TO_1,
                    vec![target_ext.to_string(), package_json],
                );
            }
            return u::new_diagnostic_chain_for_node(
                None,
                self.store(),
                error_node,
                &diag::TO_CONVERT_THIS_FILE_TO_AN_ECMASCRIPT_MODULE_ADD_THE_FIELD_TYPE_COLON_MODULE_TO_0,
                vec![package_json],
            );
        }
        if !target_ext.is_empty() {
            return u::new_diagnostic_chain_for_node(
                None,
                self.store(),
                error_node,
                &diag::TO_CONVERT_THIS_FILE_TO_AN_ECMASCRIPT_MODULE_CHANGE_ITS_FILE_EXTENSION_TO_0_OR_CREATE_A_LOCAL_PACKAGE_JSON_FILE_WITH_TYPE_COLON_MODULE,
                vec![target_ext.to_string()],
            );
        }
        u::new_diagnostic_chain_for_node(
            None,
            self.store(),
            error_node,
            &diag::TO_CONVERT_THIS_FILE_TO_AN_ECMASCRIPT_MODULE_CREATE_A_LOCAL_PACKAGE_JSON_FILE_WITH_TYPE_COLON_MODULE,
            vec![],
        )
    }

    /// Go: `program.GetDefaultResolutionModeForFile(file)` (program.go:1800
    /// → fileloader.go:1029's getDefaultResolutionModeForFile) — read through
    /// the FCS-tranche-6/7 program seams (source_file_name +
    /// source_file_meta_data + the case's own options — the per-unit twin's
    /// per-file options ARE the case's options: no project references).
    fn default_resolution_mode_for_file(&self, file: NodeId) -> ResolutionMode {
        if import_syntax_affects_module_resolution(&self.compiler_options) {
            let file_name = self.program.source_file_name(file);
            let meta = self.program.source_file_meta_data(file);
            get_implied_node_format_for_emit_worker(
                &file_name,
                self.compiler_options.get_emit_module_kind(),
                &meta,
            )
        } else {
            ResolutionModeNone
        }
    }

    // ── The resolution-mode derivation (Go checker.go:15470-15506's
    // context-specifier derivation + compiler/fileloader.go:1029-1100's
    // getModeForUsageLocation/getDefaultResolutionModeForFile chain) —
    // REAL as of FCS tranche 6 (feeds the module-not-found ladder) ──

    /// Go: `mode` of `resolveExternalModule` (checker.go:15506-15510) —
    /// `getModeForUsageLocation(importingSourceFile, contextSpecifier)` when
    /// the context specifier is a string literal, else
    /// `getDefaultResolutionModeForFile(importingSourceFile)`.
    fn get_mode_for_usage_location(
        &self,
        importing_source_file: NodeId,
        location: NodeId,
    ) -> ResolutionMode {
        let store = self.store();
        let context_specifier = get_context_specifier(store, location).filter(|specifier| {
            tsc_ast::utilities::is_string_literal_like(store.node(*specifier))
        });
        let Some(context_specifier) = context_specifier else {
            // Go: getDefaultResolutionModeForFile (fileloader.go:1029-1035)
            // — GetImpliedNodeFormatForEmitWorker under node16/nodenext
            // (or the exports/imports resolvers).
            return if import_syntax_affects_module_resolution(&self.compiler_options) {
                let file_name = self.program.source_file_name(importing_source_file);
                let meta = self.program.source_file_meta_data(importing_source_file);
                get_implied_node_format_for_emit_worker(
                    &file_name,
                    self.compiler_options.get_emit_module_kind(),
                    &meta,
                )
            } else {
                ResolutionModeNone
            };
        };
        // Go: getModeForUsageLocation (fileloader.go:1037-1067).
        let parent = store.node(context_specifier).parent.get();
        if parent != NodeId::NONE {
            let parent_node = store.node(parent);
            if (tsc_ast::is_import_declaration(parent_node)
                || parent_node.kind == Kind::JSImportDeclaration
                || tsc_ast::is_export_declaration(parent_node)
                || tsc_ast::is_jsdoc_import_tag(parent_node))
                && crate::statements::is_exclusively_type_only_import_or_export(store, parent)
            {
                // Go: the type-only declaration's attributes override.
                let attributes = match &parent_node.data {
                    tsc_ast::NodeData::ImportDeclaration(d) => d.attributes,
                    tsc_ast::NodeData::ExportDeclaration(d) => d.attributes,
                    tsc_ast::NodeData::JSDocImportTag(d) => d.attributes,
                    _ => None,
                };
                if let Some(attributes) = attributes {
                    let (override_mode, ok) = get_resolution_mode_override(store, attributes);
                    if ok {
                        return override_mode;
                    }
                }
            }
            if tsc_ast::is_literal_type_node(parent_node) {
                // Go: the import-type literal argument's attributes override.
                let grandparent = parent_node.parent.get();
                if grandparent != NodeId::NONE
                    && tsc_ast::is_import_type_node(store.node(grandparent))
                    && let tsc_ast::NodeData::ImportTypeNode(d) = &store.node(grandparent).data
                    && let Some(attributes) = d.attributes
                {
                    let (override_mode, ok) = get_resolution_mode_override(store, attributes);
                    if ok {
                        return override_mode;
                    }
                }
            }
        }
        if import_syntax_affects_module_resolution(&self.compiler_options) {
            let file_name = self.program.source_file_name(importing_source_file);
            let meta = self.program.source_file_meta_data(importing_source_file);
            get_emit_syntax_for_usage_location_worker(
                store,
                context_specifier,
                &file_name,
                &meta,
                &self.compiler_options,
            )
        } else {
            ResolutionModeNone
        }
    }

    /// Go: `func (c *Checker) getSuggestedImportExtension(extensionlessImportPath) string`
    /// (checker.go:15838) — the FileExists probe ladder through the program
    /// seam. REAL as of FCS tranche 6; the twin answer ("the unit file
    /// exists") is exactly Go's per-unit twin resolution ("./index" from
    /// index.mts suggests ".mjs" because index.mts is the only file in the
    /// map FS).
    fn get_suggested_import_extension(
        &self,
        extensionless_import_path: &tsc_tspath::RootedFilePath,
    ) -> String {
        let exists =
            |ext: &str| self.program.file_exists(extensionless_import_path.append_suffix(ext).as_string());
        if exists(".mts") {
            return ".mjs".to_string();
        }
        if exists(".ts") {
            return ".js".to_string();
        }
        if exists(".cts") {
            return ".cjs".to_string();
        }
        if exists(".mjs") {
            return ".mjs".to_string();
        }
        if exists(".js") {
            return ".js".to_string();
        }
        if exists(".cjs") {
            return ".cjs".to_string();
        }
        if exists(".tsx") {
            // Go: IfElse(Jsx == JsxEmitPreserve, ".jsx", ".js").
            return if self.compiler_options.jsx == JsxEmit::Preserve {
                ".jsx".to_string()
            } else {
                ".js".to_string()
            };
        }
        if exists(".jsx") {
            return ".jsx".to_string();
        }
        if exists(".json") {
            return ".json".to_string();
        }
        String::new()
    }

    /// Go: `func (c *Checker) tryResolvePatternAmbientModule(resolvedSymbol, moduleReference, importAttributesType) *ast.Symbol`
    /// (checker.go:15759). PORT: the wildcard pattern-ambient-module set
    /// (c.patternAmbientModules) and the assignability probe ride the S7
    /// union/relater + pattern-machinery seams; permissive pass-through of
    /// resolvedSymbol keeps non-pattern modules exact.
    /// PORT (FCS tranche 32): pub(crate) — resolveHelpersModule's
    /// nil-location ambient-only path reads it.
    pub(crate) fn try_resolve_pattern_ambient_module(
        &mut self,
        resolved_symbol: Option<SymbolId>,
        _module_reference: &str,
        _import_attributes_type: TypeId,
    ) -> Option<SymbolId> {
        // TODO(port M6 S7/pattern seam): patternAmbientModules +
        // patternAmbientModuleAugmentations/Targets and the
        // isTypeAssignableTo(attributes, moduleAttributes) probe.
        resolved_symbol
    }

    /// Go: `func (c *Checker) tryFindAmbientModule(moduleName, withAugmentations) *ast.Symbol`
    /// (checker.go:15910).
    pub(crate) fn try_find_ambient_module_real(
        &mut self,
        module_name: &str,
        with_augmentations: bool,
    ) -> Option<SymbolId> {
        if tsc_tspath::is_external_module_name_relative(module_name) {
            return None;
        }
        let quoted = format!("\"{module_name}\"");
        let symbol = self.get_symbol(&self.globals.clone(), &quoted, SymbolFlags::VALUE_MODULE);
        // merged symbol is module declaration symbol combined with all augmentations
        if with_augmentations {
            return symbol.map(|s| self.get_merged_symbol(s));
        }
        symbol
    }

    /// Go: `func (c *Checker) resolveExternalModuleSymbol(moduleSymbol, dontResolveAlias) *ast.Symbol`
    /// (checker.go:15942).
    pub(crate) fn resolve_external_module_symbol_ex(
        &mut self,
        module_symbol: Option<SymbolId>,
        dont_resolve_alias: bool,
    ) -> Option<SymbolId> {
        if let Some(module_symbol) = module_symbol {
            let export_equals = self
                .program_symbol(module_symbol)
                .exports
                .get(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
                .copied();
            if let Some(export_equals) = export_equals {
                let resolved = self.resolve_symbol_ex(export_equals, dont_resolve_alias);
                if resolved != self.unknown_symbol() {
                    return Some(self.get_merged_symbol(resolved));
                }
            }
            return Some(module_symbol);
        }
        None
    }

    /// Go: `func (c *Checker) resolveESModuleSymbol(moduleSymbol, node, moduleSpecifier) *ast.Symbol`
    /// (checker.go:15954). The export=/alias composition is REAL; the
    /// module-type wrapping half (getTypeWithSyntheticDefaultOnly/
    /// cloneTypeAsModuleType/getSpreadType) rides the S7 type-construction
    /// seam — permissive: the resolved symbol is returned unwrapped.
    pub(crate) fn resolve_es_module_symbol(
        &mut self,
        module_symbol: Option<SymbolId>,
        node: NodeId,
        module_specifier: Option<NodeId>,
    ) -> Option<SymbolId> {
        let mut symbol = self.resolve_external_module_symbol_ex(module_symbol, true);
        let needs_indirection = symbol.is_some_and(|s| {
            is_non_local_alias(
                self.program_symbol(s),
                SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
            )
        });
        if needs_indirection {
            // When the module has an export= with a pure alias, we transitively resolve and propagate any typeOnlyDeclaration
            let alias_symbol = self.get_symbol_of_declaration_merged(node)?;
            let resolved = symbol.unwrap();
            let result = self.resolve_indirection_alias(alias_symbol, resolved);
            symbol = Some(self.get_merged_symbol(result));
        }
        if let Some(symbol) = symbol {
            // TODO(port M6 S7): the synthetic-default/`module.exports` wrapper
            // half below reads getTypeOfSymbol + the S7 structured-type
            // machinery (hasSignatures/getSignaturesOfStructuredType are the
            // S8 members seam); permissive: return the resolved symbol
            // unwrapped, matching Go when none of the wrapper arms fire.
            let reference_parent = module_specifier
                .map(|m| self.store().node(m).parent.get())
                .unwrap_or(NodeId::NONE);
            let _ = reference_parent;
            return Some(symbol);
        }
        None
    }

    /// Go: `func (c *Checker) getSuggestedSymbolForNonexistentModule(name, targetModule) *ast.Symbol`
    /// (checker.go:16286). TODO(port M6 S6 remainder): the
    /// getSpellingSuggestionForName engine (Levenshtein over export names);
    /// permissive None keeps the "Did you mean" arms dormant.
    pub(crate) fn get_suggested_symbol_for_nonexistent_module(
        &mut self,
        _name: NodeId,
        _target_module: SymbolId,
    ) -> Option<SymbolId> {
        None
    }

    /// Go: `func (c *Checker) cloneSymbol(symbol) *ast.Symbol` (checker.go's
    /// cloneSymbol) — the lateBindIndexSignature clone path.
    pub(crate) fn clone_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        let (flags, name, declarations, parent, value_declaration, members, exports) = {
            let s = self.program_symbol(symbol);
            (
                s.flags,
                s.name.clone(),
                s.declarations.clone(),
                s.parent,
                s.value_declaration,
                s.members.clone(),
                s.exports.clone(),
            )
        };
        let result = self.new_symbol(flags, &name);
        {
            let s = self.program_symbol_mut(result);
            s.declarations = declarations;
            s.parent = parent;
            s.value_declaration = value_declaration;
            s.members = members;
            s.exports = exports;
        }
        self.record_merged_symbol(result, symbol);
        result
    }
}

/// Go: `type ExportCollision struct` (checker.go:16531).
struct ExportCollision {
    specifier_text: String,
    exports_with_duplicate: Vec<NodeId>,
}

/// Go: `func getExcludedSymbolFlags(flags)` (checker.go:14552).
pub(crate) fn get_excluded_symbol_flags(flags: SymbolFlags) -> SymbolFlags {
    let mut result = SymbolFlags::NONE;
    if flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
        result |= SymbolFlags::BLOCK_SCOPED_VARIABLE_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE) {
        result |= SymbolFlags::FUNCTION_SCOPED_VARIABLE_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::PROPERTY) {
        result |= SymbolFlags::PROPERTY_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::ENUM_MEMBER) {
        result |= SymbolFlags::ENUM_MEMBER_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::FUNCTION) {
        result |= SymbolFlags::FUNCTION_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::CLASS) {
        result |= SymbolFlags::CLASS_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::INTERFACE) {
        result |= SymbolFlags::INTERFACE_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::REGULAR_ENUM) {
        result |= SymbolFlags::REGULAR_ENUM_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::CONST_ENUM) {
        result |= SymbolFlags::CONST_ENUM_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::VALUE_MODULE) {
        result |= SymbolFlags::VALUE_MODULE_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::METHOD) {
        result |= SymbolFlags::METHOD_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::GET_ACCESSOR) {
        result |= SymbolFlags::GET_ACCESSOR_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::SET_ACCESSOR) {
        result |= SymbolFlags::SET_ACCESSOR_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
        result |= SymbolFlags::TYPE_PARAMETER_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::TYPE_ALIAS) {
        result |= SymbolFlags::TYPE_ALIAS_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::ALIAS) {
        result |= SymbolFlags::ALIAS_EXCLUDES;
    }
    if flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD) {
        result &= !SymbolFlags::METHOD;
    }
    result
}

/// Go: `func combineSymbolTables(early, late) ast.SymbolTable` (the
/// getResolvedMembersOrExportsOfSymbol tail).
fn combine_symbol_tables(early: &SymbolTable, late: &SymbolTable) -> SymbolTable {
    let mut result = early.clone();
    for (name, symbol) in late.entries() {
        if name == INTERNAL_SYMBOL_NAME_EXPORT_STAR || name == INTERNAL_SYMBOL_NAME_DEFAULT {
            continue;
        }
        result.set(name.clone(), *symbol);
    }
    result
}

/// Go: `func isLateBindableAST(node) bool` (checker.go:20403).
fn is_late_bindable_ast(store: &dyn NodeStore, node: NodeId) -> bool {
    let expr = match &store.node(node).data {
        tsc_ast::NodeData::ComputedPropertyName(c) => Some(c.expression),
        tsc_ast::NodeData::ElementAccessExpression(e) => Some(e.argument_expression),
        _ => None,
    };
    expr.is_some_and(|e| tsc_ast::utilities::is_entity_name_expression(store, e))
}

/// Go: `func getModuleSpecifierFromNode(node) *ast.Node` (checker.go:15346).
pub(crate) fn get_module_specifier_from_node(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    match store.node(node).kind {
        Kind::ImportDeclaration | Kind::JSImportDeclaration | Kind::ExportDeclaration => {
            binder_node_data::module_specifier_of(store, node)
        }
        _ => panic!(
            "Unhandled case in getModuleSpecifierFromNode: {}",
            store.node(node).kind_string()
        ),
    }
}

// ---------------------------------------------------------------------------
// Local free helpers
// ---------------------------------------------------------------------------

/// Go: `func IsNonLocalAlias(symbol, excludes)` (ast/utilities.go).
fn is_non_local_alias(symbol: &Symbol, excludes: SymbolFlags) -> bool {
    symbol.flags.intersects(SymbolFlags::ALIAS)
        && (symbol.flags & (SymbolFlags::ALIAS | excludes)) == SymbolFlags::ALIAS
        || symbol.flags.intersects(SymbolFlags::ALIAS)
            && symbol.flags.intersects(SymbolFlags::ASSIGNMENT)
}

/// Go: `func isExportOrExportExpression(location) bool` (checker.go:28885) —
/// M6 W5: markAliasReferenced's const-enum-export test.
fn is_export_or_export_expression(store: &dyn NodeStore, location: NodeId) -> bool {
    // Go: FindAncestor with the parent-kind walk (checker.go:28885-28897).
    let mut n = location;
    loop {
        let parent = store.node(n).parent.get();
        if parent == NodeId::NONE {
            return false;
        }
        let pn = store.node(parent);
        if pn.kind == Kind::ExportAssignment {
            let d = pn.as_export_assignment().expect("ExportAssignment data");
            if d.expression == n && crate::utilities::is_entity_name_expression(store, n) {
                return true;
            }
        } else if pn.kind == Kind::ExportSpecifier {
            let d = pn.as_export_specifier().expect("ExportSpecifier data");
            if d.name == n || d.property_name == Some(n) {
                return true;
            }
        }
        n = parent;
    }
}

/// Go: `func isConstEnumOrConstEnumOnlyModule(s *ast.Symbol) bool` (emitresolver.go).
/// M6 W5: markAliasReferenced's elision test.
fn is_const_enum_or_const_enum_only_module(c: &Checker, symbol: SymbolId) -> bool {
    let flags = c.program_symbol(symbol).flags;
    flags.intersects(SymbolFlags::CONST_ENUM | SymbolFlags::CONST_ENUM_ONLY_MODULE)
}

/// Go: `func GetExternalModuleRequireArgument(node) *ast.Node` (utilities.go):
/// the first argument of the require call a variable is initialized to.
fn get_external_module_require_argument(store: &dyn NodeStore, node: NodeId) -> Option<NodeId> {
    if !binder_shims::is_variable_declaration_initialized_to_require(store, node) {
        return None;
    }
    let initializer = crate::ast_shims::initializer_of(store, node)?;
    let arguments = binder_node_data::arguments_of(store, initializer);
    arguments.first().copied()
}

/// Go: `func GetExternalModuleImportEqualsDeclarationExpression(node) *ast.Node`
/// (ast/utilities.go).
fn get_external_module_import_equals_declaration_expression(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    let tsc_ast::NodeData::ImportEqualsDeclaration(d) = &store.node(node).data else {
        return None;
    };
    let tsc_ast::NodeData::ExternalModuleReference(m) = &store.node(d.module_reference).data
    else {
        return None;
    };
    Some(m.expression)
}

/// Go: `ast.IsSourcePhaseImport(parent)` (the import-declaration phase probe
/// resolveExternalModuleNameWorker needs) — the phase marker rides the
/// import clause.
fn is_source_phase_import(store: &dyn NodeStore, node: NodeId) -> bool {
    let import_clause = match &store.node(node).data {
        tsc_ast::NodeData::ImportDeclaration(d) => d.import_clause,
        _ => return false,
    };
    let Some(import_clause) = import_clause else {
        return false;
    };
    matches!(
        &store.node(import_clause).data,
        tsc_ast::NodeData::ImportClause(c) if c.phase_modifier == Some(Kind::SourceKeyword)
    )
}

/// Go: `func isContainedByNamespace(node) bool` (checker.go).
fn is_contained_by_namespace(store: &dyn NodeStore, node: NodeId) -> bool {
    let Some(mut container) = binder_shims::parent_of(store, node) else {
        return false;
    };
    if !tsc_ast::is_source_file(store.node(container)) {
        container = binder_shims::parent_of(store, container).unwrap_or(container);
    }
    tsc_ast::is_module_declaration(store.node(container))
        && !binder_shims::is_ambient_module(store, container)
}

/// Go: `func IsRightSideOfQualifiedNameOrPropertyAccess(node) bool` (ast).
fn is_right_side_of_qualified_name_or_property_access(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    let Some(parent) = binder_shims::parent_of(store, node) else {
        return false;
    };
    match &store.node(parent).data {
        tsc_ast::NodeData::QualifiedName(q) => q.right == node,
        tsc_ast::NodeData::PropertyAccessExpression(p) => p.name == node,
        _ => false,
    }
}

/// Go: `func IsDeclarationNode(node) bool` (ast) — PORT: approximation via the
/// declaration-data payload rather than Go's static kind list (every kind in
/// Go's list carries `DeclarationData` in the Rust AST).
fn is_declaration_node(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::typeofsymbol::payload_symbol(store.node(node)).is_some()
}

/// Go: `func CanHaveSymbol(node) bool` (ast) — PORT: payload-based like
/// `is_declaration_node` (Go's kind list is exactly the payload-carrying set).
fn can_have_symbol(store: &dyn NodeStore, node: NodeId) -> bool {
    crate::typeofsymbol::payload_symbol(store.node(node)).is_some()
}

// ────────────────────────────────────────────────────────────────────────────
// The resolution-mode derivation free helpers (FCS tranche 6) — Go
// checker.go:15470-15506 (the context-specifier derivation) +
// compiler/fileloader.go:1029-1100 (the mode workers) + ast/utilities.go's
// ShouldTransformImportCall + ast/ast.go's
// ImportAttributesNode.GetResolutionModeOverride. Local copies of the
// compiler crate's ast_shims (checker cannot depend on tsc-compiler).
// ────────────────────────────────────────────────────────────────────────────

/// Go: checker.go:15470-15506 — the context-specifier derivation of
/// `resolveExternalModule`.
fn get_context_specifier(store: &dyn NodeStore, location: NodeId) -> Option<NodeId> {
    let location_node = store.node(location);
    let parent = location_node.parent.get();
    let is_name_of_module_declaration = parent != NodeId::NONE && {
        let parent_node = store.node(parent);
        tsc_ast::is_module_declaration(parent_node)
            && matches!(&parent_node.data, tsc_ast::NodeData::ModuleDeclaration(d) if d.name == location)
    };
    if tsc_ast::utilities::is_string_literal_like(location_node)
        || is_name_of_module_declaration
    {
        return Some(location);
    }
    if tsc_ast::is_module_declaration(location_node) {
        return match &location_node.data {
            tsc_ast::NodeData::ModuleDeclaration(d) => Some(d.name),
            _ => None,
        };
    }
    if is_literal_import_type_node(store, location) {
        // Go: location.AsImportTypeNode().Argument.AsLiteralTypeNode().Literal.
        return match &location_node.data {
            tsc_ast::NodeData::ImportTypeNode(d) => {
                match &store.node(d.argument).data {
                    tsc_ast::NodeData::LiteralTypeNode(lit) => Some(lit.literal),
                    _ => None,
                }
            }
            _ => None,
        };
    }
    if is_variable_declaration_initialized_to_bare_or_accessed_require(store, location) {
        return get_module_specifier_of_bare_or_accessed_require(store, location);
    }
    // Go: the four FindAncestor probes (checker.go:15493-15505).
    if let Some(ancestor) = find_ancestor_matching(store, location, |store, node| {
        crate::resolvecall::is_import_call(store, node)
    }) {
        // Go: contextSpecifier = ancestor.Arguments()[0].
        if let tsc_ast::NodeData::CallExpression(call) = &store.node(ancestor).data
            && let Some(arguments) = &call.arguments
            && let Some(first) = arguments.nodes.first()
        {
            return Some(*first);
        }
        return None;
    }
    if let Some(ancestor) = find_ancestor_matching(store, location, |store, node| {
        crate::statements::is_import_declaration_or_js_import_declaration(store, node)
    }) {
        return crate::ast_shims::module_specifier_of(store, ancestor);
    }
    if let Some(ancestor) = find_ancestor_matching(store, location, |_, node| {
        tsc_ast::is_export_declaration(store.node(node))
    }) {
        return crate::ast_shims::module_specifier_of(store, ancestor);
    }
    if let Some(ancestor) = find_ancestor_matching(store, location, |_, node| {
        tsc_ast::is_import_equals_declaration(store.node(node))
    }) {
        // Go: the external-module-reference arm's Expression().
        if let tsc_ast::NodeData::ImportEqualsDeclaration(d) = &store.node(ancestor).data
            && store.node(d.module_reference).kind == Kind::ExternalModuleReference
            && let tsc_ast::NodeData::ExternalModuleReference(reference) =
                &store.node(d.module_reference).data
        {
            return Some(reference.expression);
        }
        return None;
    }
    None
}

/// Go: `ast.FindAncestor` — the local walk (the checker's ast_shims copy
/// takes a `&Node`-only predicate; these probes read child nodes).
fn find_ancestor_matching(
    store: &dyn NodeStore,
    node: NodeId,
    predicate: impl Fn(&dyn NodeStore, NodeId) -> bool,
) -> Option<NodeId> {
    let mut node = node;
    loop {
        if predicate(store, node) {
            return Some(node);
        }
        let parent = store.node(node).parent.get();
        if parent == NodeId::NONE {
            return None;
        }
        node = parent;
    }
}

/// Go: `func IsLiteralImportTypeNode(node) bool` (ast/utilities.go:1338).
fn is_literal_import_type_node(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    tsc_ast::is_import_type_node(n)
        && match &n.data {
            tsc_ast::NodeData::ImportTypeNode(d) => {
                let argument = store.node(d.argument);
                tsc_ast::is_literal_type_node(argument)
                    && match &argument.data {
                        tsc_ast::NodeData::LiteralTypeNode(lit) => {
                            store.node(lit.literal).kind == Kind::StringLiteral
                        }
                        _ => false,
                    }
            }
            _ => false,
        }
}

/// Go: `func IsVariableDeclarationInitializedToBareOrAccessedRequire(node) bool`
/// (ast/utilities.go:2912).
fn is_variable_declaration_initialized_to_bare_or_accessed_require(
    store: &dyn NodeStore,
    node: NodeId,
) -> bool {
    is_variable_declaration_initialized_with_require_helper(store, node, true)
}

/// Go: `func isVariableDeclarationInitializedWithRequireHelper(node, allowAccessedRequire) bool`
/// (ast/utilities.go:2916).
fn is_variable_declaration_initialized_with_require_helper(
    store: &dyn NodeStore,
    node: NodeId,
    allow_accessed_require: bool,
) -> bool {
    let n = store.node(node);
    if !tsc_ast::utilities::is_in_js_file(n) {
        return false;
    }
    if n.kind != Kind::VariableDeclaration {
        return false;
    }
    let Some(mut initializer) = crate::ast_shims::initializer_of(store, node) else {
        return false;
    };
    if allow_accessed_require {
        initializer = get_leftmost_access_expression(store, initializer);
    }
    // Go: node.Parent.Parent.ModifierFlags()&ModifierFlagsExport == 0.
    let list = n.parent.get();
    let statement = if list != NodeId::NONE {
        store.node(list).parent.get()
    } else {
        NodeId::NONE
    };
    let export_modifier = statement != NodeId::NONE
        && has_syntactic_modifier(store.node(statement), ModifierFlags::EXPORT);
    // Go: node.Type() == nil.
    let type_annotation = match &n.data {
        tsc_ast::NodeData::VariableDeclaration(d) => d.type_,
        _ => None,
    };
    !export_modifier && type_annotation.is_none() && is_require_call(store, initializer, true)
}

/// Go: `func GetModuleSpecifierOfBareOrAccessedRequire(node) *Node`
/// (ast/utilities.go:2936).
fn get_module_specifier_of_bare_or_accessed_require(
    store: &dyn NodeStore,
    node: NodeId,
) -> Option<NodeId> {
    if is_variable_declaration_initialized_with_require_helper(store, node, false) {
        let initializer = crate::ast_shims::initializer_of(store, node)?;
        return call_first_argument(store, initializer);
    }
    if is_variable_declaration_initialized_with_require_helper(store, node, true) {
        let initializer = crate::ast_shims::initializer_of(store, node)?;
        let leftmost = get_leftmost_access_expression(store, initializer);
        if is_require_call(store, leftmost, true) {
            return call_first_argument(store, leftmost);
        }
    }
    None
}

/// Go: `node.Arguments()[0]` on a require call.
fn call_first_argument(store: &dyn NodeStore, call: NodeId) -> Option<NodeId> {
    match &store.node(call).data {
        tsc_ast::NodeData::CallExpression(call) => call
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.nodes.first())
            .copied(),
        _ => None,
    }
}

/// Go: `func GetLeftmostAccessExpression(expr) *Node` (ast/utilities.go:2973).
fn get_leftmost_access_expression(store: &dyn NodeStore, expression: NodeId) -> NodeId {
    let mut expression = expression;
    while tsc_ast::is_access_expression(store.node(expression)) {
        let next = crate::ast_shims::expression_of(store, expression);
        match next {
            Some(next) => expression = next,
            None => break,
        }
    }
    expression
}

/// Go: `func IsRequireCall(node, requireStringLiteralLikeArgument) bool` (ast)
/// — the binder's ast_shims copy.
fn is_require_call(store: &dyn NodeStore, node: NodeId, require_string_literal_like_argument: bool) -> bool {
    binder_shims::is_require_call(store, node, require_string_literal_like_argument)
}

/// Go: `func (node *ImportAttributesNode) GetResolutionModeOverride(nil)`
/// (ast/ast.go:2217) — the local copy of the compiler crate's port
/// (`grammarErrorOnNode` is dropped exactly like there; the checker.go
/// callers pass nil).
fn get_resolution_mode_override(store: &dyn NodeStore, node: NodeId) -> (ResolutionMode, bool) {
    get_resolution_mode_override_ex::<fn(NodeId, &'static Message)>(store, node, None)
}

/// Go: `func (node *ImportAttributesNode) GetResolutionModeOverride(grammarErrorOnNode)`
/// (ast/ast.go:2217) — the callback-bearing form for the checker.go:3391
/// caller (checkImportType) that reports TS1453 on a bad resolution-mode
/// value. PORT(FCS tranche 28): the callback arm was dropped before.
fn get_resolution_mode_override_ex<F: FnMut(NodeId, &'static Message)>(
    store: &dyn NodeStore,
    node: NodeId,
    mut grammar_error_on_node: Option<F>,
) -> (ResolutionMode, bool) {
    if node == NodeId::NONE {
        return (ResolutionModeNone, false);
    }
    let attributes = match &store.node(node).data {
        tsc_ast::NodeData::ImportAttributes(attributes) => match &attributes.attributes {
            Some(list) => list.nodes.as_ref(),
            None => return (ResolutionModeNone, false),
        },
        _ => return (ResolutionModeNone, false),
    };
    let attribute = attributes.iter().copied().find(|&attribute| {
        store.node(attribute).kind == Kind::ImportAttribute
            && matches!(&store.node(attribute).data,
                tsc_ast::NodeData::ImportAttribute(a) if tsc_ast::node_text(store, a.name) == "resolution-mode")
    });
    let Some(attribute) = attribute else {
        return (ResolutionModeNone, false);
    };
    let value = match &store.node(attribute).data {
        tsc_ast::NodeData::ImportAttribute(a) => a.value,
        _ => return (ResolutionModeNone, false),
    };
    if !tsc_ast::utilities::is_string_literal_like(store.node(value)) {
        return (ResolutionModeNone, false);
    }
    let text = tsc_ast::node_text(store, value);
    // Go: the grammarErrorOnNode arm — only reached when the caller passes
    // the callback (checker.go:3391's reportErrors form).
    if text != "import" && text != "require" {
        if let Some(grammar_error_on_node) = grammar_error_on_node.as_mut() {
            grammar_error_on_node(
                value,
                &diag::X_RESOLUTION_MODE_SHOULD_BE_EITHER_REQUIRE_OR_IMPORT,
            );
        }
        return (ResolutionModeNone, false);
    }
    if text == "import" {
        (ResolutionModeESM, true)
    } else {
        (ResolutionModeCommonJS, true)
    }
}

/// Go: `func importSyntaxAffectsModuleResolution(options) bool`
/// (compiler/fileloader.go:1069).
fn import_syntax_affects_module_resolution(options: &CompilerOptions) -> bool {
    let module_resolution = options.get_module_resolution_kind();
    (ModuleResolutionKind::Node16 <= module_resolution
        && module_resolution <= ModuleResolutionKind::NodeNext)
        || options.get_resolve_package_json_exports()
        || options.get_resolve_package_json_imports()
}

/// Go: `func getEmitSyntaxForUsageLocationWorker(fileName, meta, usage, options)`
/// (compiler/fileloader.go:1075).
fn get_emit_syntax_for_usage_location_worker(
    store: &dyn NodeStore,
    usage: NodeId,
    file_name: &str,
    meta: &tsc_ast::SourceFileMetaData,
    options: &CompilerOptions,
) -> ResolutionMode {
    let parent = store.node(usage).parent.get();
    // Go: IsRequireCall(usage.Parent, false) || IsExternalModuleReference(
    // usage.Parent) && IsImportEqualsDeclaration(usage.Parent.Parent).
    if parent != NodeId::NONE
        && (is_require_call(store, parent, false)
            || (tsc_ast::is_external_module_reference(store.node(parent))
                && store.node(parent).parent.get() != NodeId::NONE
                && tsc_ast::is_import_equals_declaration(
                    store.node(store.node(parent).parent.get()),
                )))
    {
        return ResolutionModeCommonJS;
    }
    // Go: fileEmitMode := GetEmitModuleFormatOfFileWorker(...) — the
    // implied-for-emit worker with the emit-module-kind fallback
    // (ast/utilities.go:2645).
    let file_emit_mode = {
        let implied = get_implied_node_format_for_emit_worker(
            file_name,
            options.get_emit_module_kind(),
            meta,
        );
        if implied != ModuleKind::None {
            implied
        } else {
            options.get_emit_module_kind()
        }
    };
    emit_syntax_tail(file_emit_mode, store, parent, options)
}

/// Go: getEmitSyntaxForUsageLocationWorker's import-call + module-kind tail
/// (fileloader.go:1083-1100). `usage` itself is already reduced to its
/// parent here (Go's WalkUpParenthesizedExpressions(usage.Parent) start).
fn emit_syntax_tail(
    file_emit_mode: ModuleKind,
    store: &dyn NodeStore,
    parent: NodeId,
    options: &CompilerOptions,
) -> ResolutionMode {
    if parent != NodeId::NONE {
        // Go: `if call := WalkUpParenthesizedExpressions(usage.Parent);
        // IsImportCall(call)`.
        let call = crate::ast_shims::walk_up_parenthesized_expressions(store, parent)
            .unwrap_or(parent);
        if crate::resolvecall::is_import_call(store, call) {
            if is_source_phase_import_call(store, call) {
                return ResolutionModeESM;
            }
            // Go: ShouldTransformImportCall(fileName, options, fileEmitMode).
            let module_kind = options.get_emit_module_kind();
            let should_transform = if (ModuleKind::Node16 <= module_kind
                && module_kind <= ModuleKind::NodeNext)
                || module_kind == ModuleKind::Preserve
            {
                false
            } else {
                file_emit_mode < ModuleKind::ES2015
            };
            if should_transform {
                return ResolutionModeCommonJS;
            }
            return ResolutionModeESM;
        }
    }
    if file_emit_mode == ModuleKind::CommonJS {
        ResolutionModeCommonJS
    } else if file_emit_mode.is_non_node_esm() || file_emit_mode == ModuleKind::Preserve {
        ResolutionModeESM
    } else {
        ResolutionModeNone
    }
}

/// Go: `func ast.GetImpliedNodeFormatForEmitWorker(fileName, emitModuleKind,
/// sourceFileMetaData)` (ast/utilities.go:2653) — the local copy of the
/// compiler crate's port (checker cannot depend on tsc-compiler).
fn get_implied_node_format_for_emit_worker(
    file_name: &str,
    emit_module_kind: ModuleKind,
    source_file_meta_data: &tsc_ast::SourceFileMetaData,
) -> ResolutionMode {
    if ModuleKind::Node16 <= emit_module_kind && emit_module_kind <= ModuleKind::NodeNext {
        return source_file_meta_data.implied_node_format;
    }
    if source_file_meta_data.implied_node_format == ResolutionModeCommonJS
        && (source_file_meta_data.package_json_type == "commonjs"
            || tsc_tspath::file_extension_is_one_of(
                file_name,
                &[tsc_tspath::EXTENSION_CJS, tsc_tspath::EXTENSION_CTS],
            ))
    {
        return ResolutionModeCommonJS;
    }
    if source_file_meta_data.implied_node_format == ResolutionModeESM
        && (source_file_meta_data.package_json_type == "module"
            || tsc_tspath::file_extension_is_one_of(
                file_name,
                &[tsc_tspath::EXTENSION_MJS, tsc_tspath::EXTENSION_MTS],
            ))
    {
        return ResolutionModeESM;
    }
    ResolutionModeNone
}

/// Go: `func IsSourcePhaseImportCall(node) bool` (ast) — the local copy
/// (the compiler crate's ast_shims port; `import source` meta property).
/// FCS tranche 7: pub(crate) — checkImportCallExpression's source-phase
/// short-circuit (checker.go:8523) reads it.
pub(crate) fn is_source_phase_import_call(store: &dyn NodeStore, node: NodeId) -> bool {
    if store.node(node).kind != Kind::CallExpression {
        return false;
    }
    match &store.node(node).data {
        tsc_ast::NodeData::CallExpression(call) => {
            let n = store.node(call.expression);
            n.kind == Kind::MetaProperty
                && matches!(&n.data, tsc_ast::NodeData::MetaProperty(meta)
                    if meta.keyword_token == Kind::ImportKeyword
                        && tsc_ast::node_text(store, meta.name) == "source")
        }
        _ => false,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// The resolved-module free helpers (FCS tranche 7) — Go
// checker.go:15525-15711's local predicates + checker/utilities.go's
// isSideEffectImport + ast/utilities.go's IsEmittableImport /
// IsTypeOnlyImportDeclaration / IsResolutionModeOverrideHost /
// HasResolutionModeOverride + core.ShouldRewriteModuleSpecifier. Local
// copies (the ast crate does not carry them yet).
// ────────────────────────────────────────────────────────────────────────────

/// Go: `func isSideEffectImport(node *ast.Node) bool` (checker/utilities.go:228)
/// — FindAncestor(node, ast.IsImportDeclaration) is the KindImportDeclaration
/// predicate (JS imports carry KindJSImportDeclaration in Go's AST and do
/// not match — mirrored exactly).
fn is_side_effect_import(store: &dyn NodeStore, node: NodeId) -> bool {
    let ancestor = find_ancestor_matching(store, node, |_, node| {
        tsc_ast::is_import_declaration(store.node(node))
    });
    match ancestor {
        Some(ancestor) => match &store.node(ancestor).data {
            tsc_ast::NodeData::ImportDeclaration(d) => d.import_clause.is_none(),
            _ => false,
        },
        None => false,
    }
}

/// Go: `func resolutionExtensionIsTSOrJson(ext string) bool` (checker.go:15811).
fn resolution_extension_is_ts_or_json(ext: &str) -> bool {
    tsc_tspath::extension_is_ts(ext) || ext == tsc_tspath::EXTENSION_JSON
}

/// Go: `func IsEmittableImport(node *Node) bool` (ast/utilities.go) — the
/// find_ancestor_matching predicate shape (KindImportDeclaration only —
/// Go's IsImportDeclaration excludes the JS kind).
fn is_emittable_import(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::ImportDeclaration => {
            match &n.data {
                tsc_ast::NodeData::ImportDeclaration(d) => d
                    .import_clause
                    .is_some_and(|clause| {
                        // Go: ImportClause().IsTypeOnly() — the port models
                        // the `type` phase modifier.
                        !matches!(&store.node(clause).data,
                            tsc_ast::NodeData::ImportClause(c)
                                if c.phase_modifier == Some(Kind::TypeKeyword))
                    }),
                _ => false,
            }
        }
        Kind::ExportDeclaration | Kind::ImportEqualsDeclaration => {
            match &n.data {
                tsc_ast::NodeData::ExportDeclaration(d) => !d.is_type_only,
                tsc_ast::NodeData::ImportEqualsDeclaration(d) => !d.is_type_only,
                _ => false,
            }
        }
        Kind::CallExpression => crate::resolvecall::is_import_call(store, node),
        _ => false,
    }
}

/// Go: the `IsTypeOnly()` read over the type-only-bearing payloads — the
/// local payload-shape copy (Go reads the shared modifier field).
fn node_is_type_only(store: &dyn NodeStore, node: NodeId) -> bool {
    match &store.node(node).data {
        // Go: node.IsTypeOnly() — the port models the `type` keyword as the
        // ImportClause/ImportSpecifier phase modifier elsewhere; the
        // declaration payloads carry the flag directly.
        tsc_ast::NodeData::ImportDeclaration(d) => d
            .import_clause
            .is_some_and(|clause| {
                matches!(&store.node(clause).data,
                    tsc_ast::NodeData::ImportClause(c)
                        if c.phase_modifier == Some(Kind::TypeKeyword))
            }),
        tsc_ast::NodeData::ExportDeclaration(d) => d.is_type_only,
        tsc_ast::NodeData::ImportClause(d) => d.phase_modifier == Some(Kind::TypeKeyword),
        tsc_ast::NodeData::ImportEqualsDeclaration(d) => d.is_type_only,
        tsc_ast::NodeData::ImportSpecifier(d) => d.is_type_only,
        tsc_ast::NodeData::ExportSpecifier(d) => d.is_type_only,
        _ => false,
    }
}

/// Go: `func IsTypeOnlyImportDeclaration(node *Node) bool` (ast/utilities.go).
fn is_type_only_import_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::ImportSpecifier => {
            let parent = n.parent.get();
            let grandparent = if parent != NodeId::NONE {
                store.node(parent).parent.get()
            } else {
                NodeId::NONE
            };
            node_is_type_only(store, node)
                || (grandparent != NodeId::NONE && node_is_type_only(store, grandparent))
        }
        Kind::NamespaceImport => {
            let parent = n.parent.get();
            parent != NodeId::NONE && node_is_type_only(store, parent)
        }
        Kind::ImportClause | Kind::ImportEqualsDeclaration => node_is_type_only(store, node),
        _ => false,
    }
}

/// Go: `func isTypeOnlyExportDeclaration(node *Node) bool` (ast/utilities.go).
fn is_type_only_export_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    let n = store.node(node);
    match n.kind {
        Kind::ExportSpecifier => {
            let parent = n.parent.get();
            let grandparent = if parent != NodeId::NONE {
                store.node(parent).parent.get()
            } else {
                NodeId::NONE
            };
            node_is_type_only(store, node)
                || (grandparent != NodeId::NONE && node_is_type_only(store, grandparent))
        }
        Kind::ExportDeclaration => {
            match &n.data {
                tsc_ast::NodeData::ExportDeclaration(d) => {
                    d.is_type_only && d.module_specifier.is_some() && d.export_clause.is_none()
                }
                _ => false,
            }
        }
        Kind::NamespaceExport => {
            let parent = n.parent.get();
            parent != NodeId::NONE && node_is_type_only(store, parent)
        }
        _ => false,
    }
}

/// Go: `func IsPartOfTypeOnlyImportOrExportDeclaration(node *Node) bool`
/// (ast/utilities.go — FindAncestor(node, IsTypeOnlyImportOrExportDeclaration) != nil,
/// where IsTypeOnlyImportOrExportDeclaration = IsTypeOnlyImportDeclaration ||
/// isTypeOnlyExportDeclaration).
fn is_part_of_type_only_import_or_export_declaration(store: &dyn NodeStore, node: NodeId) -> bool {
    find_ancestor_matching(store, node, |store, node| {
        is_type_only_import_declaration(store, node)
            || is_type_only_export_declaration(store, node)
    })
    .is_some()
}

/// Go: `func IsResolutionModeOverrideHost(node *Node) bool` (ast/utilities.go).
fn is_resolution_mode_override_host(store: &dyn NodeStore, node: NodeId) -> bool {
    matches!(
        store.node(node).kind,
        Kind::ImportType
            | Kind::ExportDeclaration
            | Kind::ImportDeclaration
            | Kind::JSImportDeclaration
    )
}

/// Go: `func HasResolutionModeOverride(node *Node) bool` (ast/utilities.go) —
/// the import-attributes `resolution-mode` probe.
fn has_resolution_mode_override(store: &dyn NodeStore, node: Option<NodeId>) -> bool {
    let Some(node) = node else {
        return false;
    };
    let attributes = match &store.node(node).data {
        tsc_ast::NodeData::ImportTypeNode(d) => d.attributes,
        tsc_ast::NodeData::ImportDeclaration(d) => d.attributes,
        tsc_ast::NodeData::ExportDeclaration(d) => d.attributes,
        _ => None,
    };
    if let Some(attributes) = attributes {
        let (_, ok) = get_resolution_mode_override(store, attributes);
        return ok;
    }
    false
}

/// Go: `func ShouldRewriteModuleSpecifier(specifier, compilerOptions) bool`
/// (core/core.go) — the local copy of the core read.
fn should_rewrite_module_specifier(specifier: &str, compiler_options: &CompilerOptions) -> bool {
    compiler_options.rewrite_relative_import_extensions.is_true()
        && tsc_tspath::path_is_relative(specifier)
        && !tsc_tspath::is_declaration_file_name(specifier)
        && tsc_tspath::has_ts_file_extension(specifier)
}
