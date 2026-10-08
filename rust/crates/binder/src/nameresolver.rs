// Ported from tsc/internal/binder/nameresolver.go @ ec47d33c23e464a17cdf2475632cba629bee8763.
//
// Go name mapping: the NameResolver's function-field hooks (checker-supplied
// closures, all optional with default implementations) become Option<Box<dyn
// FnMut...>> fields; `*Symbol` becomes SymbolId into the caller's symbol
// arena (`symbols`), and Go's `ast.SymbolTable` map reference becomes a
// borrowed SymbolTable. The resolver never runs during binding — the checker
// (M6) constructs it — so the store/arena/globals are borrow fields.

use tsc_ast::{
    has_syntactic_modifier, is_class_expression, is_module_declaration, is_object_binding_pattern,
    is_parameter_declaration, is_source_file, Kind, ModifierFlags, Node, NodeFlags, NodeId,
    NodeStore, Symbol, SymbolFlags, SymbolId, SymbolTable, INTERNAL_SYMBOL_NAME_DEFAULT,
};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;

use crate::ast_shims;
use crate::node_data;

// Go hook-closure types (the checker-supplied `Option<Box<dyn FnMut...>>`
// fields below; Go declared these as named func-field types inline).
type GetSymbolOfDeclarationHook<'a> = Box<dyn FnMut(NodeId) -> Option<SymbolId> + 'a>;
type ErrorHook<'a> = Box<dyn FnMut(NodeId, &'static Message, &[String]) + 'a>;
type LookupHook<'a> = Box<dyn FnMut(&SymbolTable, &str, SymbolFlags) -> Option<SymbolId> + 'a>;
type SetRequiresScopeChangeCacheHook<'a> = Box<dyn FnMut(NodeId, Tristate) + 'a>;
type GetRequiresScopeChangeCacheHook<'a> = Box<dyn FnMut(NodeId) -> Tristate + 'a>;
type OnPropertyWithInvalidInitializerHook<'a> =
    Box<dyn FnMut(NodeId, &str, NodeId, Option<SymbolId>) -> bool + 'a>;
type OnFailedToResolveSymbolHook<'a> =
    Box<dyn FnMut(NodeId, &str, SymbolFlags, Option<&'static Message>) + 'a>;
type OnSuccessfullyResolvedSymbolHook<'a> = Box<
    dyn FnMut(NodeId, Option<SymbolId>, SymbolFlags, Option<NodeId>, Option<NodeId>, bool) + 'a,
>;

/// Go: `type NameResolver struct` — the checker-owned hook fields + the
/// port's store/arena/globals borrow parameters (see the header note).
pub struct NameResolver<'a, 'f> {
    pub compiler_options: &'a CompilerOptions,
    /// Go: `GetSymbolOfDeclaration func(*ast.Node) *ast.Symbol`.
    pub get_symbol_of_declaration: Option<GetSymbolOfDeclarationHook<'a>>,
    /// Go: `Error func(location, message, args...) *ast.Diagnostic` — the
    /// port's hook carries pre-stringified args (Go's `args ...any`).
    pub error: Option<ErrorHook<'a>>,
    /// Go: `Globals ast.SymbolTable`.
    pub globals: &'f SymbolTable,
    /// Go: `ArgumentsSymbol *ast.Symbol`.
    ///
    /// PORT: Go's default `argumentsSymbol()` synthesizes a transient
    /// `&ast.Symbol{...}` outside any arena; the port's symbols are arena
    /// handles, so the checker MUST supply this (the default panics —
    /// nothing in wave A instantiates the resolver).
    pub arguments_symbol: Option<SymbolId>,
    /// Go: `RequireSymbol *ast.Symbol`.
    pub require_symbol: Option<SymbolId>,
    /// Go: `Lookup func(symbols, name, meaning) *ast.Symbol`.
    pub lookup: Option<LookupHook<'a>>,
    /// Go: `SymbolReferenced func(symbol, meaning)`.
    pub symbol_referenced: Option<Box<dyn FnMut(SymbolId, SymbolFlags) + 'a>>,
    /// Go: `SetRequiresScopeChangeCache func(node, value core.Tristate)`.
    pub set_requires_scope_change_cache: Option<SetRequiresScopeChangeCacheHook<'a>>,
    /// Go: `GetRequiresScopeChangeCache func(node) core.Tristate`.
    pub get_requires_scope_change_cache: Option<GetRequiresScopeChangeCacheHook<'a>>,
    /// Go: `OnPropertyWithInvalidInitializer func(location, name, declaration, result) bool`.
    pub on_property_with_invalid_initializer: Option<OnPropertyWithInvalidInitializerHook<'a>>,
    /// Go: `OnFailedToResolveSymbol func(location, name, meaning, nameNotFoundMessage)`.
    pub on_failed_to_resolve_symbol: Option<OnFailedToResolveSymbolHook<'a>>,
    /// Go: `OnSuccessfullyResolvedSymbol func(location, result, meaning, lastLocation, associatedDeclarationForContainingInitializerOrBindingName, withinDeferredContext)`.
    pub on_successfully_resolved_symbol: Option<OnSuccessfullyResolvedSymbolHook<'a>>,

    // Port-only context (see the header note): the store + symbol arena the
    // default implementations and flag reads resolve through.
    pub store: &'f dyn NodeStore,
    pub symbols: &'f [Symbol],
}

impl<'a, 'f> NameResolver<'a, 'f> {
    /// Go: `func (r *NameResolver) Resolve(location, name, meaning, nameNotFoundMessage, isUse, excludeGlobals) *ast.Symbol`.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &mut self,
        location: Option<NodeId>,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> Option<SymbolId> {
        let mut location = location;
        let mut result: Option<SymbolId> = None;
        let mut last_location: Option<NodeId> = None;
        let mut last_self_reference_location: Option<NodeId> = None;
        let mut property_with_invalid_initializer: Option<NodeId> = None;
        let mut associated_declaration_for_containing_initializer_or_binding_name: Option<NodeId> = None;
        let mut within_deferred_context = false;
        // needed for did-you-mean error reporting, which gathers candidates starting from the original location
        let original_location = location;
        let name_is_const = name == "const";
        'outer: while let Some(loc) = location {
            if name_is_const && ast_shims::is_const_assertion(self.store, loc) {
                // `const` in an `as const` has no symbol, but issues no error because there is no *actual* lookup of the type
                // (it refers to the constant type of the expression instead)
                return None;
            }
            if ast_shims::is_module_or_enum_declaration(self.store.node(loc))
                && last_location.is_some()
                && self.store.node(loc).name() == last_location
            {
                // If lastLocation is the name of a namespace or enum, skip the parent since it will have is own locals that could
                // conflict.
                last_location = Some(loc);
                location = ast_shims::parent_of(self.store, loc);
                continue 'outer;
            }
            let is_module_attributes = is_module_declaration(self.store.node(loc))
                && last_location.is_some_and(|l| {
                    (match &self.store.node(loc).data {
                        tsc_ast::NodeData::ModuleDeclaration(d) => d.attributes,
                        _ => None,
                    }) == Some(l)
                });
            // Locals of a source file are not in scope (because they get merged into the global symbol table)
            if let Some(locals) = node_data::locals_of(self.store, loc)
                .cloned()
                .filter(|_| !ast_shims::is_global_source_file(self.store, loc))
            {
                result = self.lookup(&locals, name, meaning);
                if result.is_some() {
                    let mut use_result = true;
                    let result_flags = self.symbols[result.unwrap().0 as usize].flags;
                    if is_module_attributes {
                        use_result = false;
                    } else if ast_shims::is_function_like(self.store.node(loc))
                        && last_location.is_some()
                        && last_location != node_data::body_of(self.store, loc)
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
                                    self.store.node(l).flags.intersects(NodeFlags::SYNTHESIZED)
                                })
                                    || last_location == node_data::type_of(self.store, loc)
                                    || last_location.is_some_and(|l| self.store.node(l).kind == Kind::Parameter)
                                    || last_location.is_some_and(|l| self.store.node(l).kind == Kind::JSDocParameterTag)
                                    || last_location.is_some_and(|l| self.store.node(l).kind == Kind::JSDocReturnTag)
                                    || last_location.is_some_and(|l| self.store.node(l).kind == Kind::TypeParameter));
                        }
                        if meaning.intersects(result_flags) && result_flags.intersects(SymbolFlags::VARIABLE) {
                            // expression inside parameter will lookup as normal variable scope when targeting es2015+
                            if self.use_outer_variable_scope_in_parameter(result.unwrap(), loc, last_location) {
                                use_result = false;
                            } else if result_flags.intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE) {
                                // parameters are visible only inside function body, parameter list and return type
                                // technically for parameter list case here we might mix parameters and variables declared in function,
                                // however it is detected separately when checking initializers of parameters
                                // to make sure that they reference no variables declared after them.
                                let found_in_parameter = last_location.is_some_and(|l| {
                                    self.store.node(l).kind == Kind::Parameter
                                        || self.store.node(l).flags.intersects(NodeFlags::SYNTHESIZED)
                                });
                                let found_through_type = last_location == node_data::type_of(self.store, loc)
                                    && result
                                        .and_then(|r| self.symbols[r.0 as usize].value_declaration)
                                        .is_some_and(|vd| {
                                            ast_shims::find_ancestor(self.store, vd, &mut |n| {
                                                is_parameter_declaration(self.store.node(n))
                                            })
                                            .is_some()
                                        });
                                use_result = found_in_parameter || found_through_type;
                            }
                        }
                    } else if self.store.node(loc).kind == Kind::ConditionalType {
                        // A type parameter declared using 'infer T' in a conditional type is visible only in
                        // the true branch of the conditional type.
                        use_result = match &self.store.node(loc).data {
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
                within_deferred_context || get_is_deferred_context(self.store, loc, last_location);
            match self.store.node(loc).kind {
                Kind::SourceFile => {
                    let file = self.store.file(loc.file_id());
                    if ast_shims::is_external_or_common_js_module(file) {
                        // Go: fallthrough into the ModuleDeclaration case.
                        self.resolve_module_case(loc, name, meaning, is_module_attributes, &mut result);
                        if result.is_some() {
                            break 'outer;
                        }
                    }
                }
                Kind::ModuleDeclaration => {
                    self.resolve_module_case(loc, name, meaning, is_module_attributes, &mut result);
                    if result.is_some() {
                        break 'outer;
                    }
                }
                Kind::EnumDeclaration => {
                    // Go: `if enumSymbol == nil { break }` — no enum symbol
                    // means skip the arm, not the loop.
                    if let Some(enum_symbol) = self.get_symbol_of_declaration(loc) {
                        let exports = self.symbols[enum_symbol.0 as usize].exports.clone();
                        result = self.lookup(&exports, name, meaning & SymbolFlags::ENUM_MEMBER);
                        if result.is_some() {
                            if name_not_found_message.is_some()
                                && self.compiler_options.get_isolated_modules()
                                && !self.store.node(loc).flags.intersects(NodeFlags::AMBIENT)
                                && result
                                    .and_then(|r| self.symbols[r.0 as usize].value_declaration)
                                    .is_some_and(|vd| loc.file_id() != vd.file_id())
                            {
                                let isolated_modules_like_flag_name =
                                    if self.compiler_options.verbatim_module_syntax == Tristate::TSTrue {
                                        "verbatimModuleSyntax"
                                    } else {
                                        "isolatedModules"
                                    };
                                let enum_name = &self.symbols[enum_symbol.0 as usize].name;
                                let args = vec![
                                    name.to_string(),
                                    isolated_modules_like_flag_name.to_string(),
                                    format!("{enum_name}.{name}"),
                                ];
                                self.report_error(
                                    original_location,
                                    &tsc_diagnostics::CANNOT_ACCESS_0_FROM_ANOTHER_FILE_WITHOUT_QUALIFICATION_WHEN_1_IS_ENABLED_USE_2_INSTEAD,
                                    &args,
                                );
                            }
                            break 'outer;
                        }
                    }
                }
                Kind::PropertyDeclaration => {
                    if !ast_shims::is_static(self.store.node(loc)) {
                        let parent = ast_shims::parent_of(self.store, loc).expect("member parent");
                        if let Some(ctor) = ast_shims::find_constructor_declaration(self.store, parent) {
                            if let Some(ctor_locals) = node_data::locals_of(self.store, ctor) {
                                let ctor_locals = ctor_locals.clone();
                                let value_meaning = meaning & SymbolFlags::VALUE;
                                if self.lookup(&ctor_locals, name, value_meaning).is_some() {
                                    // Remember the property node, it will be used later to report appropriate error
                                    property_with_invalid_initializer = Some(loc);
                                }
                            }
                        }
                    }
                }
                Kind::ClassDeclaration | Kind::ClassExpression | Kind::InterfaceDeclaration => {
                    let container_symbol =
                        self.get_symbol_of_declaration(loc).expect("class-like symbol");
                    let members = self.symbols[container_symbol.0 as usize].members.clone();
                    result = self.lookup(&members, name, meaning & SymbolFlags::TYPE);
                    if result.is_some() {
                        if !is_type_parameter_symbol_declared_in_container(self, result.unwrap(), loc) {
                            // ignore type parameters not declared in this container
                            result = None;
                        } else {
                            if last_location.is_some_and(|l| ast_shims::is_static(self.store.node(l))) {
                                // TypeScript 1.0 spec (April 2014): 3.4.1
                                // The scope of a type parameter extends over the entire declaration with which the type
                                // parameter list is associated, with the exception of static member declarations in classes.
                                if name_not_found_message.is_some() {
                                    self.report_error(
                                        original_location,
                                        &tsc_diagnostics::STATIC_MEMBERS_CANNOT_REFERENCE_CLASS_TYPE_PARAMETERS,
                                        &[],
                                    );
                                }
                                return None;
                            }
                            break 'outer;
                        }
                    }
                    if is_class_expression(self.store.node(loc)) && meaning.intersects(SymbolFlags::CLASS) {
                        let class_name = self.store.node(loc).name();
                        if let Some(class_name) = class_name {
                            if name == tsc_ast::node_text(self.store, class_name) {
                                result = node_data::symbol_of(self.store, loc);
                                break 'outer;
                            }
                        }
                    }
                }
                Kind::ExpressionWithTypeArguments => {
                    let parent = ast_shims::parent_of(self.store, loc);
                    let is_extends_clause = parent.is_some_and(|p| {
                        tsc_ast::is_heritage_clause(self.store.node(p))
                            && match &self.store.node(p).data {
                                tsc_ast::NodeData::HeritageClause(d) => d.token == Kind::ExtendsKeyword,
                                _ => false,
                            }
                    });
                    if last_location == node_data::expression_of(self.store, loc) && is_extends_clause {
                        let container = ast_shims::parent_of(self.store, parent.unwrap())
                            .expect("heritage clause parent");
                        if ast_shims::is_class_like(self.store.node(container)) {
                            let container_symbol =
                                self.get_symbol_of_declaration(container).expect("class-like symbol");
                            let members = self.symbols[container_symbol.0 as usize].members.clone();
                            result = self.lookup(&members, name, meaning & SymbolFlags::TYPE);
                            if result.is_some() {
                                if name_not_found_message.is_some() {
                                    self.report_error(
                                        original_location,
                                        &tsc_diagnostics::BASE_CLASS_EXPRESSIONS_CANNOT_REFERENCE_CLASS_TYPE_PARAMETERS,
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
                    let grandparent = ast_shims::parent_of(self.store, loc)
                        .and_then(|p| ast_shims::parent_of(self.store, p))
                        .expect("ComputedPropertyName grandparent");
                    if ast_shims::is_class_like(self.store.node(grandparent))
                        || tsc_ast::is_interface_declaration(self.store.node(grandparent))
                    {
                        // A reference to this grandparent's type parameters would be an error
                        let grandparent_symbol =
                            self.get_symbol_of_declaration(grandparent).expect("class-like symbol");
                        let members = self.symbols[grandparent_symbol.0 as usize].members.clone();
                        result = self.lookup(&members, name, meaning & SymbolFlags::TYPE);
                        if result.is_some() {
                            if name_not_found_message.is_some() {
                                self.report_error(
                                    original_location,
                                    &tsc_diagnostics::A_COMPUTED_PROPERTY_NAME_CANNOT_REFERENCE_A_TYPE_PARAMETER_FROM_ITS_CONTAINING_TYPE,
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
                        result = Some(self.arguments_symbol());
                        break 'outer;
                    }
                }
                Kind::FunctionExpression => {
                    if meaning.intersects(SymbolFlags::VARIABLE) && name == "arguments" {
                        result = Some(self.arguments_symbol());
                        break 'outer;
                    }
                    if meaning.intersects(SymbolFlags::FUNCTION) {
                        let function_name = match &self.store.node(loc).data {
                            tsc_ast::NodeData::FunctionExpression(d) => d.name,
                            _ => None,
                        };
                        if let Some(function_name) = function_name {
                            if name == tsc_ast::node_text(self.store, function_name) {
                                result = node_data::symbol_of(self.store, loc);
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
                    if let Some(parent) = ast_shims::parent_of(self.store, loc) {
                        if self.store.node(parent).kind == Kind::Parameter {
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
                    if let Some(parent) = ast_shims::parent_of(self.store, loc) {
                        if ast_shims::is_class_element(self.store.node(parent))
                            || self.store.node(parent).kind == Kind::ClassDeclaration
                        {
                            location = Some(parent);
                        }
                    }
                }
                Kind::Parameter => {
                    let parameter_declaration = match &self.store.node(loc).data {
                        tsc_ast::NodeData::ParameterDeclaration(d) => (d.initializer, d.name),
                        _ => (None, NodeId::NONE),
                    };
                    if let Some(last) = last_location {
                        let is_initializer = Some(last) == parameter_declaration.0;
                        let is_binding_pattern_name = Some(last) == self.store.node(loc).name()
                            && ast_shims::is_binding_pattern(self.store.node(last));
                        if (is_initializer || is_binding_pattern_name)
                            && associated_declaration_for_containing_initializer_or_binding_name.is_none()
                        {
                            associated_declaration_for_containing_initializer_or_binding_name = Some(loc);
                        }
                    }
                }
                Kind::BindingElement => {
                    let binding_element = match &self.store.node(loc).data {
                        tsc_ast::NodeData::BindingElement(d) => (d.initializer, d.name),
                        _ => (None, None),
                    };
                    if let Some(last) = last_location {
                        let is_initializer = Some(last) == binding_element.0;
                        let is_binding_pattern_name = Some(last) == binding_element.1
                            && ast_shims::is_binding_pattern(self.store.node(last));
                        if (is_initializer || is_binding_pattern_name)
                            && ast_shims::is_part_of_parameter_declaration(self.store, loc)
                            && associated_declaration_for_containing_initializer_or_binding_name.is_none()
                        {
                            associated_declaration_for_containing_initializer_or_binding_name = Some(loc);
                        }
                    }
                }
                Kind::InferType => {
                    if meaning.intersects(SymbolFlags::TYPE_PARAMETER) {
                        let parameter_name = match &self.store.node(loc).data {
                            tsc_ast::NodeData::InferTypeNode(d) => match &self.store.node(d.type_parameter).data {
                                tsc_ast::NodeData::TypeParameterDeclaration(td) => td.name,
                                _ => NodeId::NONE,
                            },
                            _ => NodeId::NONE,
                        };
                        if !parameter_name.is_none() && name == tsc_ast::node_text(self.store, parameter_name) {
                            result = match &self.store.node(loc).data {
                                tsc_ast::NodeData::InferTypeNode(d) => {
                                    node_data::symbol_of(self.store, d.type_parameter)
                                }
                                _ => None,
                            };
                            break 'outer;
                        }
                    }
                }
                Kind::ExportSpecifier => {
                    let property_name = node_data::property_name_of(self.store, loc);
                    if last_location.is_some() && Some(last_location.unwrap()) == property_name {
                        let export_declaration =
                            ast_shims::parent_of(self.store, loc).and_then(|p| ast_shims::parent_of(self.store, p));
                        let module_specifier =
                            export_declaration.and_then(|ed| node_data::module_specifier_of(self.store, ed));
                        if module_specifier.is_some() {
                            location = export_declaration
                                .and_then(|ed| ast_shims::parent_of(self.store, ed));
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
            if is_self_reference_location(self.store.node(effective), last_location) {
                last_self_reference_location = Some(effective);
            }
            last_location = location;
            // !!! In Strada, JSDocTemplateTag/JSDocParameterTag/JSDocReturnTag locations skip to
            // getEffectiveContainerForJSDocTemplateTag/getHostSignatureFromJSDoc instead of location.parent.
            // This is a no-op currently because JSDoc nodes have no locals and getEffectiveJSDocHost is not
            // fully ported for JS assignment patterns.
            location = ast_shims::parent_of(self.store, effective);
        }
        // We just climbed up parents looking for the name, meaning that we started in a descendant node of `lastLocation`.
        // If `result === lastSelfReferenceLocation.symbol`, that means that we are somewhere inside `lastSelfReferenceLocation` looking up a name, and resolving to `lastLocation` itself.
        // That means that this is a self-reference of `lastLocation`, and shouldn't count this when considering whether `lastLocation` is used.
        if is_use {
            if let Some(result_symbol) = result {
                let is_self_reference = last_self_reference_location
                    .and_then(|l| node_data::symbol_of(self.store, l))
                    .is_some_and(|s| s == result_symbol);
                if !is_self_reference {
                    if let Some(symbol_referenced) = &mut self.symbol_referenced {
                        symbol_referenced(result_symbol, meaning);
                    }
                }
            }
        }
        if result.is_none() && !exclude_globals {
            result = self.lookup(self.globals, name, meaning.union(SymbolFlags::GLOBAL_LOOKUP));
        }
        if result.is_none() {
            if let Some(original_location) = original_location {
                if tsc_ast::utilities::is_in_js_file(self.store.node(original_location)) {
                    if let Some(parent) = ast_shims::parent_of(self.store, original_location) {
                        if ast_shims::is_require_call(self.store, parent, false /*requireStringLiteralLikeArgument*/) {
                            return self.require_symbol;
                        }
                    }
                }
            }
        }
        if name_not_found_message.is_some() {
            if let (Some(property), Some(on_property)) = (
                property_with_invalid_initializer,
                &mut self.on_property_with_invalid_initializer,
            ) {
                if on_property(
                    original_location.expect("propertyWithInvalidInitializer implies a location"),
                    name,
                    property,
                    result,
                ) {
                    return None;
                }
            }
            if result.is_none() {
                if let Some(on_failed) = &mut self.on_failed_to_resolve_symbol {
                    on_failed(
                        original_location.expect("nameNotFoundMessage implies a location"),
                        name,
                        meaning,
                        name_not_found_message,
                    );
                }
            } else if let Some(on_success) = &mut self.on_successfully_resolved_symbol {
                on_success(
                    original_location.expect("nameNotFoundMessage implies a location"),
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

    /// The shared `KindModuleDeclaration` switch body (Go runs it via
    /// fallthrough from `KindSourceFile` when the file is an external or
    /// CommonJS module). Sets `result`; the caller breaks the loop when it
    /// is `Some`.
    fn resolve_module_case(
        &mut self,
        location: NodeId,
        name: &str,
        meaning: SymbolFlags,
        is_module_attributes: bool,
        result: &mut Option<SymbolId>,
    ) {
        if is_module_attributes {
            // Go: break out of the switch.
            return;
        }
        let Some(module_symbol) = self.get_symbol_of_declaration(location) else {
            return;
        };
        let module_exports = self.symbols[module_symbol.0 as usize].exports.clone();
        let is_source_file_loc = is_source_file(self.store.node(location));
        let is_ambient_module_decl = is_module_declaration(self.store.node(location))
            && self.store.node(location).flags.intersects(NodeFlags::AMBIENT)
            && !ast_shims::is_global_scope_augmentation(self.store.node(location));
        if is_source_file_loc || is_ambient_module_decl {
            // It's an external module. First see if the module has an export default and if the local
            // name of that export default matches.
            *result = module_exports.get(INTERNAL_SYMBOL_NAME_DEFAULT).copied();
            if let Some(result_symbol) = *result {
                let local_symbol = get_local_symbol_for_export_default(self.store, self.symbols, result_symbol);
                let flags = self.symbols[result_symbol.0 as usize].flags;
                let local_name_matches = local_symbol
                    .map(|ls| self.symbols[ls.0 as usize].name == name)
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
            //        an alias. If we used &, we'd be throwing out symbols that have non alias aspects,
            //        which is not the desired behavior.
            let module_export = module_exports.get(name).copied();
            if let Some(module_export) = module_export {
                let flags = self.symbols[module_export.0 as usize].flags;
                if flags == SymbolFlags::ALIAS
                    && (ast_shims::get_declaration_of_kind(
                        self.store,
                        self.symbols,
                        module_export,
                        Kind::ExportSpecifier,
                    )
                    .is_some()
                        || ast_shims::get_declaration_of_kind(
                            self.store,
                            self.symbols,
                            module_export,
                            Kind::NamespaceExport,
                        )
                        .is_some())
                {
                    return; // Go: break out of the switch
                }
            }
        }
        if name != INTERNAL_SYMBOL_NAME_DEFAULT {
            let module_meaning = meaning & SymbolFlags::MODULE_MEMBER;
            *result = self.lookup(&module_exports, name, module_meaning);
            if let Some(result_symbol) = *result {
                let flags = self.symbols[result_symbol.0 as usize].flags;
                // Go: on a CommonJS source file, a non-type result falls
                // back to nil (the caller's break happens on Some(result)
                // after the return).
                if is_source_file_loc
                    && self
                        .store
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
        if last_location.is_some_and(|l| is_parameter_declaration(self.store.node(l))) {
            let body = node_data::body_of(self.store, location);
            let value_declaration = self.symbols[result.0 as usize].value_declaration;
            let in_body = body.is_some_and(|b| {
                value_declaration.is_some_and(|vd| {
                    self.store.node(vd).pos() >= self.store.node(b).pos()
                        && self.store.node(vd).end() <= self.store.node(b).end()
                })
            });
            if in_body {
                // check for several cases where we introduce temporaries that require moving the name/initializer of the parameter to the body
                // - static field in a class expression
                // - optional chaining pre-es2020
                // - nullish coalesce pre-es2020
                // - spread assignment in binding pattern pre-es2017
                let function_location = location;
                let mut declaration_requires_scope_change = Tristate::TSUnknown;
                if let Some(get_cache) = &mut self.get_requires_scope_change_cache {
                    declaration_requires_scope_change = get_cache(function_location);
                }
                if declaration_requires_scope_change == Tristate::TSUnknown {
                    let parameters = node_data::parameters_of(self.store, function_location)
                        .map(|list| list.nodes.to_vec())
                        .unwrap_or_default();
                    let any = parameters.iter().any(|&p| self.requires_scope_change(p));
                    declaration_requires_scope_change = if any { Tristate::TSTrue } else { Tristate::TSFalse };
                    if let Some(set_cache) = &mut self.set_requires_scope_change_cache {
                        set_cache(function_location, declaration_requires_scope_change);
                    }
                }
                return declaration_requires_scope_change != Tristate::TSTrue;
            }
        }
        false
    }

    /// Go: `func (r *NameResolver) requiresScopeChange(node *ast.Node) bool`.
    fn requires_scope_change(&mut self, node: NodeId) -> bool {
        let d = match &self.store.node(node).data {
            tsc_ast::NodeData::ParameterDeclaration(d) => (d.name, d.initializer),
            _ => return false,
        };
        self.requires_scope_change_worker(Some(d.0))
            || d.1.is_some() && {
                let init = d.1.unwrap();
                self.requires_scope_change_worker(Some(init))
            }
    }

    /// Go: `func (r *NameResolver) requiresScopeChangeWorker(node *ast.Node) bool`.
    fn requires_scope_change_worker(&mut self, node: Option<NodeId>) -> bool {
        let Some(node) = node else {
            return false;
        };
        match self.store.node(node).kind {
            Kind::ArrowFunction | Kind::FunctionExpression | Kind::FunctionDeclaration | Kind::Constructor => {
                false
            }
            Kind::MethodDeclaration | Kind::GetAccessor | Kind::SetAccessor | Kind::PropertyAssignment => {
                self.requires_scope_change_worker(self.store.node(node).name())
            }
            Kind::PropertyDeclaration => {
                if ast_shims::has_static_modifier(self.store.node(node)) {
                    !self.compiler_options.get_emit_standard_class_fields()
                } else {
                    self.requires_scope_change_worker(self.store.node(node).name())
                }
            }
            _ => {
                if ast_shims::is_nullish_coalesce(self.store, node)
                    || tsc_ast::utilities::is_optional_chain(self.store.node(node))
                {
                    return self.compiler_options.get_emit_script_target() < ScriptTarget::ES2020;
                }
                if tsc_ast::is_binding_element(self.store.node(node))
                    && match &self.store.node(node).data {
                        tsc_ast::NodeData::BindingElement(d) => d.dot_dot_dot_token.is_some(),
                        _ => false,
                    }
                    && ast_shims::parent_of(self.store, node)
                        .is_some_and(|p| is_object_binding_pattern(self.store.node(p)))
                {
                    return self.compiler_options.get_emit_script_target() < ScriptTarget::ES2017;
                }
                if ast_shims::is_type_node(self.store.node(node)) {
                    return false;
                }
                let mut children: Vec<NodeId> = Vec::new();
                self.store.node(node).for_each_child(&mut |child| {
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

    /// Go: `func (r *NameResolver) error(location, message, args...)`.
    fn report_error(&mut self, location: Option<NodeId>, message: &'static Message, args: &[String]) {
        if let Some(error) = &mut self.error {
            error(location.expect("error location"), message, args);
        }
        // Default implementation does not report errors
    }

    /// Go: `func (r *NameResolver) getSymbolOfDeclaration(node *ast.Node) *ast.Symbol`.
    fn get_symbol_of_declaration(&mut self, node: NodeId) -> Option<SymbolId> {
        if let Some(get) = &mut self.get_symbol_of_declaration {
            return get(node);
        }
        // Default implementation does not support merged symbols
        node_data::symbol_of(self.store, node)
    }

    /// Go: `func (r *NameResolver) lookup(symbols, name, meaning) *ast.Symbol`.
    fn lookup(&mut self, symbols: &SymbolTable, name: &str, meaning: SymbolFlags) -> Option<SymbolId> {
        if let Some(lookup) = &mut self.lookup {
            return lookup(symbols, name, meaning);
        }
        // Default implementation does not support following aliases or merged symbols
        if !meaning.is_empty() {
            if let Some(&symbol) = symbols.get(name) {
                if self.symbols[symbol.0 as usize].flags.intersects(meaning) {
                    return Some(symbol);
                }
            }
        }
        None
    }

    /// Go: `func (r *NameResolver) argumentsSymbol() *ast.Symbol`.
    fn arguments_symbol(&mut self) -> SymbolId {
        if let Some(arguments_symbol) = self.arguments_symbol {
            return arguments_symbol;
        }
        // PORT: Go synthesizes a transient *ast.Symbol outside any arena
        // (checker-owned GC object); the port's SymbolId needs an arena, so
        // the checker must supply ArgumentsSymbol up front.
        panic!("NameResolver default argumentsSymbol() requires the checker-supplied ArgumentsSymbol arena slot")
    }
}

/// Go: `func GetLocalSymbolForExportDefault(symbol *ast.Symbol) *ast.Symbol`.
pub fn get_local_symbol_for_export_default(
    store: &dyn NodeStore,
    symbols: &[Symbol],
    symbol: SymbolId,
) -> Option<SymbolId> {
    if !is_export_default_symbol(store, symbols, symbol) {
        return None;
    }
    let declarations = symbols[symbol.0 as usize].declarations.clone();
    if declarations.is_empty() {
        return None;
    }
    for decl in declarations {
        let local_symbol = node_data::local_symbol_of(store, decl);
        if local_symbol.is_some() {
            return local_symbol;
        }
    }
    None
}

/// Go: `func isExportDefaultSymbol(symbol *ast.Symbol) bool`.
fn is_export_default_symbol(store: &dyn NodeStore, symbols: &[Symbol], symbol: SymbolId) -> bool {
    let declarations = &symbols[symbol.0 as usize].declarations;
    !declarations.is_empty()
        && has_syntactic_modifier(store.node(declarations[0]), ModifierFlags::DEFAULT)
}

/// Go: `func getIsDeferredContext(location, lastLocation) bool`.
fn get_is_deferred_context(store: &dyn NodeStore, location: NodeId, last_location: Option<NodeId>) -> bool {
    let kind = store.node(location).kind;
    if kind != Kind::ArrowFunction && kind != Kind::FunctionExpression {
        // initializers in instance property declaration of class like entities are executed in constructor and thus deferred
        // A name is evaluated within the enclosing scope - so it shouldn't count as deferred
        return tsc_ast::is_type_query_node(store.node(location))
            || (ast_shims::is_function_like_declaration(store.node(location))
                || kind == Kind::PropertyDeclaration && !ast_shims::is_static(store.node(location)))
                && (last_location.is_none() || last_location != store.node(location).name());
    }
    if last_location.is_some() && last_location == store.node(location).name() {
        return false;
    }
    // generator functions and async functions are not inlined in control flow when immediately invoked
    if node_data::asterisk_token_of(store, location).is_some()
        || has_syntactic_modifier(store.node(location), ModifierFlags::ASYNC)
    {
        return true;
    }
    ast_shims::get_immediately_invoked_function_expression(store, location).is_none()
}

/// Go: `func isTypeParameterSymbolDeclaredInContainer(symbol, container) bool`.
fn is_type_parameter_symbol_declared_in_container(
    resolver: &NameResolver<'_, '_>,
    symbol: SymbolId,
    container: NodeId,
) -> bool {
    for &decl in &resolver.symbols[symbol.0 as usize].declarations {
        if resolver.store.node(decl).kind == Kind::TypeParameter {
            let parent = resolver.store.node(decl).parent.get();
            if parent == container {
                return true;
            }
        }
    }
    false
}

/// Go: `func isSelfReferenceLocation(node, lastLocation) bool`.
fn is_self_reference_location(node: &Node, last_location: Option<NodeId>) -> bool {
    match node.kind {
        Kind::Parameter => last_location.is_some() && Some(last_location.unwrap()) == node.name(),
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
