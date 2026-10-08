// gen-ast — Rust AST code generator (SPEC §5.8, §6).
//
// Reads tools/scripts/tsc/ast.json — the SAME input as
// tools/scripts/tsc/generate-go-ast.ts; Go output is never an input (SPEC §6)
// — and emits:
//
//   - rust/crates/ast/src/kind_generated.rs  (mirror of kind_generated.go +
//     kind_stringer_generated.go: Kind enum with identical ordinals, marker
//     consts, kind-union type aliases, kind guards, name()/Display)
//   - rust/crates/ast/src/ast_generated.rs  (node structs with flattened
//     base-struct composition, NodeData enum, as_/is_ accessors,
//     for_each_child)
//   - rust/crates/ast/src/factory_generated.rs (NodeFactory New*/Update*
//     constructors — mirror of ast_generated.go's factory methods, incl. the
//     kind-alias constructors — over the hand-written NodeFactory core in
//     factory.rs)
//
// Deterministic and idempotent; `--check` verifies the checked-in output is
// current (exit 1 with a diff summary if stale).

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use indexmap::IndexMap;
use serde::Deserialize;

// ────────────────────────────────────────────────────────────────────────────
// JSON model (mirrors tools/scripts/tsc/ast.schema.json; only the fields the
// Go generator consumes are modeled)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AstJson {
    #[serde(default)]
    bases: IndexMap<String, BaseEntry>,
    #[serde(default)]
    kinds: Option<KindDef>,
    nodes: NodesDef,
}

#[derive(Deserialize)]
struct KindDef {
    elements: Vec<KindElement>,
    markers: Vec<KindMarker>,
    #[serde(default)]
    aliases: IndexMap<String, KindAliasValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KindElement {
    Name(String),
    Detailed {
        name: Option<String>,
        comment: Option<String>,
    },
}

#[derive(Deserialize)]
struct KindMarker {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KindAliasValue {
    List(Vec<String>),
    Range {
        range: Vec<String>,
    },
}

#[derive(Deserialize)]
struct BaseEntry {
    #[serde(default)]
    extends: Vec<String>,
    #[serde(default)]
    fields: IndexMap<String, FieldDef>,
}

#[derive(Deserialize, Clone)]
struct FieldDef {
    #[serde(rename = "type")]
    ty: TypeSpec,
    #[serde(default)]
    list: Option<String>,
    #[serde(default)]
    optional: Option<bool>,
    #[serde(rename = "private", default)]
    private_: bool,
    #[serde(rename = "goOnly", default)]
    go_only: bool,
    #[serde(rename = "noGo", default)]
    no_go: bool,
    #[serde(rename = "noTS", default)]
    no_ts: bool,
    #[serde(rename = "noFactory", default)]
    no_factory: bool,
    /// Value mask applied by the factory when assigning this member (go:
    /// generate-go-ast's `bitmask` attribute, e.g. `TokenFlagsStringLiteralFlags`;
    /// resolved to a flags const at emission).
    #[serde(default)]
    bitmask: Option<String>,
    /// Routing hint for the generated VisitEachChild (go: generate-go-ast's
    /// `visit` attribute; "modifiers" | "parameters" | "functionBody" |
    /// "embeddedStatement" | "iterationBody" | "topLevelStatements").
    #[serde(default)]
    visit: Option<String>,
}

#[derive(Deserialize)]
struct NodesDef {
    definitions: IndexMap<String, NodeDef>,
    #[serde(default)]
    aliases: IndexMap<String, NodeAliasValue>,
    #[serde(default, rename = "listAliases")]
    list_aliases: IndexMap<String, String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NodeAliasValue {
    Base {
        base: String,
    },
    Union(Vec<String>),
}

#[derive(Deserialize, Clone)]
struct NodeDef {
    #[serde(default)]
    kind: Option<TypeSpec>,
    #[serde(default)]
    extends: Vec<String>,
    #[serde(default)]
    members: Vec<MemberDef>,
    #[serde(default, rename = "typeParameters")]
    type_parameters: Vec<TypeParamDef>,
    #[serde(default, rename = "instantiationAliases")]
    instantiation_aliases: IndexMap<String, String>,
    #[serde(default, rename = "handWritten")]
    hand_written: bool,
    #[serde(default, rename = "handWrittenVisitor")]
    hand_written_visitor: bool,
}

#[derive(Deserialize, Clone)]
struct MemberDef {
    name: String,
    #[serde(default)]
    inherited: bool,
    #[serde(rename = "type", default)]
    ty: Option<TypeSpec>,
    #[serde(default)]
    optional: Option<bool>,
    #[serde(default)]
    list: Option<String>,
    #[serde(rename = "private", default)]
    private_: bool,
    #[serde(rename = "goOnly", default)]
    go_only: bool,
    #[serde(rename = "noGo", default)]
    no_go: bool,
    #[serde(rename = "noTS", default)]
    no_ts: bool,
    #[serde(rename = "noFactory", default)]
    no_factory: bool,
    #[serde(default)]
    bitmask: Option<String>,
    /// Routing hint for the generated VisitEachChild (see FieldDef::visit).
    #[serde(default)]
    visit: Option<String>,
}

#[derive(Deserialize, Clone)]
struct TypeParamDef {
    name: String,
    constraint: String,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum TypeSpec {
    One(String),
    Many(Vec<String>),
}

// ────────────────────────────────────────────────────────────────────────────
// Resolved type model (mirror of schema.ts's Type classes)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
// The variant names mirror schema.ts's Type classes one-for-one (TypeParam,
// Alias, List), so the enum-variant-names lint is expected here.
#[allow(clippy::enum_variant_names)]
enum Type {
    Primitive(String),
    /// `value` is "Kind" or "SyntaxKind.X".
    Kind(String),
    Node(String),
    TypeParam {
        constraint: Box<Type>,
    },
    Alias {
        name: String,
        resolved: Box<Type>,
    },
    Union(Vec<Type>),
    List {
        element: Box<Type>,
        /// "NodeList" | "ModifierList" | "raw"
        list_kind: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BaseKind {
    Primitive,
    Kind,
    Node,
    List,
}

fn base_kind(t: &Type) -> BaseKind {
    match t {
        Type::Primitive(_) => BaseKind::Primitive,
        Type::Kind(_) => BaseKind::Kind,
        Type::Node(_) => BaseKind::Node,
        Type::TypeParam { constraint } => base_kind(constraint),
        Type::Alias { resolved, .. } => base_kind(resolved),
        Type::List { element, .. } => {
            if base_kind(element) == BaseKind::Node {
                BaseKind::List
            } else {
                base_kind(element)
            }
        }
        Type::Union(types) => {
            let mut all_same = true;
            let first = base_kind(&types[0]);
            for t in types {
                if base_kind(t) != first {
                    all_same = false;
                }
            }
            if all_same {
                return first;
            }
            if types.iter().any(|t| matches!(base_kind(t), BaseKind::List | BaseKind::Node)) {
                return BaseKind::Node;
            }
            if types.iter().any(|t| base_kind(t) == BaseKind::Kind) {
                return BaseKind::Kind;
            }
            BaseKind::Primitive
        }
    }
}

fn kind_name_of(value: &str) -> &str {
    value.strip_prefix("SyntaxKind.").unwrap_or(value)
}

// ────────────────────────────────────────────────────────────────────────────
// Schema API (mirror of the SchemaAPI parts generate-go-ast.ts exercises)
// ────────────────────────────────────────────────────────────────────────────

struct KindAliasInfo {
    name: String,
    members: Vec<String>,
    range: Option<(String, String)>,
}

struct Schema {
    json: AstJson,
    kind_elements: Vec<(Option<String>, Option<String>)>,
    kind_marker_names: HashSet<String>,
    kind_aliases: Vec<KindAliasInfo>,
    kind_alias_names: HashSet<String>,
    kind_element_names: Vec<String>,
    /// kind name -> (instantiation alias name, node key)
    syntax_kind_to_node: HashMap<String, (Option<String>, String)>,
    instantiation_alias: HashMap<String, String>,
    list_alias_by_key: HashMap<String, String>,
}

const PRIMITIVES: [&str; 8] = [
    "any", "bool", "boolean", "int", "ModifierFlags", "NodeFlags", "string", "TokenFlags",
];

impl Schema {
    fn load(json: AstJson) -> Schema {
        let kind_elements = json
            .kinds
            .as_ref()
            .map(|k| {
                k.elements
                    .iter()
                    .map(|e| match e {
                        KindElement::Name(n) => (Some(n.clone()), None),
                        KindElement::Detailed { name, comment } => {
                            (name.clone(), comment.clone())
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let kind_element_names = kind_elements
            .iter()
            .filter_map(|(n, _)| n.clone())
            .collect::<Vec<_>>();
        let kind_marker_names = json
            .kinds
            .as_ref()
            .map(|k| k.markers.iter().map(|m| m.name.clone()).collect())
            .unwrap_or_default();

        // kind aliases (ranges expand to member lists, like kindAliases() in schema.ts)
        let mut kind_aliases = Vec::new();
        if let Some(k) = json.kinds.as_ref() {
            for (name, value) in &k.aliases {
                match value {
                    KindAliasValue::List(members) => {
                        kind_aliases.push(KindAliasInfo {
                            name: name.clone(),
                            members: members.clone(),
                            range: None,
                        });
                    }
                    KindAliasValue::Range { range } => {
                        let first = &range[0];
                        let last = &range[1];
                        let first_idx = kind_element_names
                            .iter()
                            .position(|n| n == resolve_marker_value(&json, first, &kind_marker_names))
                            .unwrap_or_else(|| panic!("Range alias {name}: cannot resolve range [{first}, {last}]"));
                        let last_idx = kind_element_names
                            .iter()
                            .position(|n| n == resolve_marker_value(&json, last, &kind_marker_names))
                            .unwrap_or_else(|| panic!("Range alias {name}: cannot resolve range [{first}, {last}]"));
                        kind_aliases.push(KindAliasInfo {
                            name: name.clone(),
                            members: kind_element_names[first_idx..=last_idx].to_vec(),
                            range: Some((first.clone(), last.clone())),
                        });
                    }
                }
            }
        }

        // Build the transient maps in exactly the order schema.ts's
        // constructor does (first registration wins).
        let mut syntax_kind_to_node: HashMap<String, (Option<String>, String)> = HashMap::new();
        let mut instantiation_alias: HashMap<String, String> = HashMap::new();
        let mut kind_alias_names: HashSet<String> = HashSet::new();
        for a in &kind_aliases {
            kind_alias_names.insert(a.name.clone());
        }
        for (node_name, def) in &json.nodes.definitions {
            for (alias_name, type_arg) in &def.instantiation_aliases {
                instantiation_alias.insert(alias_name.clone(), node_name.clone());
                if kind_alias_names.contains(type_arg) {
                    for kt in expand_kind_alias_members(&kind_aliases, &kind_alias_names, &kind_element_names, type_arg) {
                        let k = kind_name_of(&kt).to_string();
                        syntax_kind_to_node.entry(k).or_insert((Some(alias_name.clone()), node_name.clone()));
                    }
                } else {
                    syntax_kind_to_node
                        .entry(type_arg.clone())
                        .or_insert((Some(alias_name.clone()), node_name.clone()));
                }
            }
            let primary_kind = match &def.kind {
                Some(TypeSpec::Many(v)) => v[0].clone(),
                Some(TypeSpec::One(s)) => s.clone(),
                None => node_name.clone(),
            };
            syntax_kind_to_node
                .entry(primary_kind)
                .or_insert((None, node_name.clone()));
            if let Some(TypeSpec::Many(v)) = &def.kind {
                for k in &v[1..] {
                    syntax_kind_to_node
                        .entry(k.clone())
                        .or_insert((None, node_name.clone()));
                }
            }
        }

        // list alias map: type cache key of the element type -> alias name
        let mut list_alias_by_key = HashMap::new();
        for (alias_name, element_type) in &json.nodes.list_aliases {
            let key = type_cache_key(&resolve_type_name(
                &json,
                &kind_aliases,
                &kind_alias_names,
                &kind_element_names,
                &syntax_kind_to_node,
                &instantiation_alias,
                element_type,
                None,
                false,
            ));
            list_alias_by_key.insert(key, alias_name.clone());
        }

        Schema {
            kind_elements,
            kind_marker_names,
            kind_aliases,
            kind_alias_names,
            kind_element_names,
            syntax_kind_to_node,
            instantiation_alias,
            list_alias_by_key,
            json,
        }
    }

    // ── kinds ──────────────────────────────────────────────────────────────

    fn has_kind_element(&self, name: &str) -> bool {
        self.kind_element_names.iter().any(|n| n == name)
    }

    fn get_kind_alias(&self, name: &str) -> Option<&KindAliasInfo> {
        self.kind_aliases.iter().find(|a| a.name == name)
    }

    fn has_kind_alias(&self, name: &str) -> bool {
        self.kind_alias_names.contains(name)
    }

    fn expand_kind_alias_members(&self, name: &str) -> Vec<String> {
        expand_kind_alias_members(&self.kind_aliases, &self.kind_alias_names, &self.kind_element_names, name)
    }

    // ── nodes / bases ──────────────────────────────────────────────────────

    fn nodes(&self) -> impl Iterator<Item = (&String, &NodeDef)> {
        self.json.nodes.definitions.iter()
    }

    fn node_def(&self, key: &str) -> &NodeDef {
        self.json
            .nodes
            .definitions
            .get(key)
            .unwrap_or_else(|| panic!("unknown node {key}"))
    }

    fn base(&self, key: &str) -> Option<&BaseEntry> {
        self.json.bases.get(key)
    }

    fn syntax_kind_name(&self, key: &str) -> String {
        match &self.node_def(key).kind {
            Some(TypeSpec::Many(v)) => v[0].clone(),
            Some(TypeSpec::One(s)) => s.clone(),
            None => key.to_string(),
        }
    }

    fn kind_aliases_of_node(&self, key: &str) -> Vec<String> {
        match &self.node_def(key).kind {
            Some(TypeSpec::Many(v)) => v[1..].to_vec(),
            _ => vec![],
        }
    }

    /// The `Kind`/`kind` member of a node, if any (schema.ts `kindMember()`).
    fn kind_member(&self, key: &str) -> Option<&MemberDef> {
        self.node_def(key)
            .members
            .iter()
            .find(|m| m.name == "Kind" || m.name == "kind")
    }

    /// kindTypes(): all Kind types carried by the node's kind member.
    fn kind_types(&self, key: &str) -> Vec<String> {
        let default = format!("SyntaxKind.{}", self.syntax_kind_name(key));
        let kind_type = match self.kind_member(key) {
            Some(m) => self.resolve_member(key, m).declared,
            None => Type::Kind(default.clone()),
        };
        let mut collected = Vec::new();
        collect_kind_types(&kind_type, &mut collected);
        if collected.is_empty() {
            collected.push(default);
        }
        // dedup by value, preserving order
        let mut seen = HashSet::new();
        collected.retain(|v| seen.insert(v.clone()));
        collected
    }

    /// allKinds(): kindTypes plus kind aliases.
    fn all_kinds(&self, key: &str) -> Vec<String> {
        let mut kinds = self.kind_types(key);
        for a in self.kind_aliases_of_node(key) {
            kinds.push(format!("SyntaxKind.{a}"));
        }
        kinds
    }

    fn kind_type_is_type_parameter(&self, key: &str) -> bool {
        match self.kind_member(key) {
            Some(m) => matches!(self.resolve_member(key, m).declared, Type::TypeParam { .. }),
            None => false,
        }
    }

    // ── embedding / flattening (mirror of goEmbeds et al.) ─────────────────

    fn is_inherited_declaration_marker(&self, key: &str) -> bool {
        match self.base(key) {
            Some(b) => {
                b.fields.is_empty() && b.extends.len() == 1 && b.extends[0] == "DeclarationBase"
            }
            None => false,
        }
    }

    fn is_zero_size_base(&self, key: &str) -> bool {
        match self.base(key) {
            Some(b) => {
                b.fields.is_empty()
                    && (b.extends.is_empty() || self.is_inherited_declaration_marker(key))
            }
            None => false,
        }
    }

    fn expand_go_extends(&self, keys: &[String]) -> Vec<String> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for key in keys {
            let expanded: Vec<String> = if self.is_inherited_declaration_marker(key) {
                let mut v = vec![key.clone()];
                v.extend(self.base(key).unwrap().extends.iter().cloned());
                v
            } else {
                vec![key.clone()]
            };
            for e in expanded {
                if seen.insert(e.clone()) {
                    result.push(e);
                }
            }
        }
        result
    }

    fn go_embeds(&self, keys: &[String]) -> Vec<String> {
        let expanded = self.expand_go_extends(keys);
        let mut zero_sized = Vec::new();
        let mut non_zero_sized = Vec::new();
        for key in expanded {
            if self.is_zero_size_base(&key) {
                zero_sized.push(key);
            } else {
                non_zero_sized.push(key);
            }
        }
        zero_sized.extend(non_zero_sized);
        zero_sized
    }

    fn base_go_embeds(&self, key: &str) -> Vec<String> {
        if self.is_inherited_declaration_marker(key) {
            vec![]
        } else {
            self.go_embeds(&self.base(key).unwrap().extends)
        }
    }

    // ── type resolution (mirror of SchemaAPI.resolveType) ──────────────────

    fn resolve_type(&self, spec: &TypeSpec, node_ctx: Option<&str>, as_node: bool) -> Type {
        resolve_type(
            &self.json,
            &self.kind_aliases,
            &self.kind_alias_names,
            &self.kind_element_names,
            &self.syntax_kind_to_node,
            &self.instantiation_alias,
            spec,
            node_ctx,
            as_node,
        )
    }

    fn resolve_type_name(&self, name: &str, node_ctx: Option<&str>, as_node: bool) -> Type {
        self.resolve_type(&TypeSpec::One(name.to_string()), node_ctx, as_node)
    }

    /// resolveNodeTypeForSyntaxKind
    fn resolve_node_type_for_syntax_kind(&self, kind: &str) -> Type {
        match self.syntax_kind_to_node.get(kind) {
            None => panic!("kind alias member \"{kind}\" does not resolve to a node type"),
            Some((alias_name, node_name)) => match alias_name {
                Some(a) => Type::Alias {
                    name: a.clone(),
                    resolved: Box::new(Type::Node(node_name.clone())),
                },
                None => Type::Node(node_name.clone()),
            },
        }
    }

    fn list_alias_name(&self, element: &Type) -> Option<String> {
        self.list_alias_by_key.get(&type_cache_key(element)).cloned()
    }

    // ── member resolution (mirror of MemberInfo) ───────────────────────────

    /// inheritedField(name): walk the node's (or a base's) extends chain.
    fn inherited_field(&self, key: &str, name: &str) -> Option<(String, String)> {
        let extends = match self.json.nodes.definitions.get(key) {
            Some(def) => &def.extends,
            None => &self.base(key)?.extends,
        };
        for ext in extends {
            if let Some(b) = self.base(ext) {
                if b.fields.contains_key(name) {
                    return Some((ext.clone(), name.to_string()));
                }
            }
            if let Some(r) = self.inherited_field(ext, name) {
                return Some(r);
            }
        }
        None
    }

    /// Resolve a node member (MemberInfo over a Member).
    fn resolve_member(&self, node_key: &str, m: &MemberDef) -> ResolvedMember {
        let inherited_field = if m.inherited {
            self.inherited_field(node_key, &m.name)
        } else {
            None
        };
        let field = inherited_field.as_ref().map(|(bk, fname)| {
            let b = self.base(bk).unwrap();
            (bk, fname, &b.fields[fname])
        });

        // rawType
        let raw_type = Some(if !m.inherited {
            m.ty.clone().unwrap_or_else(|| panic!("member {} has no type", m.name))
        } else if m.ty.is_some() {
            m.ty.clone().unwrap()
        } else {
            field.map(|(_, _, f)| f.ty.clone())
                .unwrap_or_else(|| panic!("member {} has no raw type source", m.name))
        });
        let raw_type = match raw_type {
            Some(t) => t,
            None => panic!("member {node_key}.{} has no raw type source", m.name),
        };

        // listKind
        let list_kind = if !m.inherited {
            m.list.clone()
        } else {
            field.and_then(|(_, _, f)| f.list.clone())
        };

        // optionality / visibility flags
        let optional = m
            .optional
            .or(field.and_then(|(_, _, f)| f.optional))
            .unwrap_or(false);
        let private_ = m.private_ || field.map(|(_, _, f)| f.private_).unwrap_or(false);
        let go_only = m.go_only || field.map(|(_, _, f)| f.go_only).unwrap_or(false);
        let no_go = m.no_go || field.map(|(_, _, f)| f.no_go).unwrap_or(false);
        let no_ts = go_only || m.no_ts || field.map(|(_, _, f)| f.no_ts).unwrap_or(false);
        let no_factory =
            go_only || m.no_factory || field.map(|(_, _, f)| f.no_factory).unwrap_or(false);

        let declared = self.resolve_type(&raw_type, Some(node_key), false);
        let ty = wrap_list(declared.clone(), list_kind.as_deref());

        // The optionality of the actual struct storage. Inherited members take
        // the base field's optionality (the member entry may narrow it for
        // factory/TS purposes — e.g. FunctionExpression.Body sets
        // `optional: false` while BodyBase.Body stays optional — but Go's
        // struct storage comes from the base field).
        let storage_optional = if m.inherited {
            field
                .and_then(|(_, _, f)| f.optional)
                .unwrap_or_else(|| m.optional.unwrap_or(false))
        } else {
            m.optional.unwrap_or(false)
        };

        let kind_param = (m.name == "Kind" || m.name == "kind")
            && base_kind(&declared) == BaseKind::Kind;

        // VisitEachChild routing: the member's own `visit` hint, else the
        // inherited base field's.
        let visit = m
            .visit
            .clone()
            .or_else(|| field.and_then(|(_, _, f)| f.visit.clone()));

        ResolvedMember {
            name: m.name.clone(),
            inherited: m.inherited,
            list_kind,
            optional,
            storage_optional,
            private_,
            go_only,
            no_go,
            no_ts,
            no_factory,
            bitmask: m
                .bitmask
                .clone()
                .or_else(|| field.and_then(|(_, _, f)| f.bitmask.clone())),
            kind_param,
            declared,
            ty,
            visit,
        }
    }

    /// Resolve a base field (MemberInfo over a BaseField).
    fn resolve_base_field(&self, _base_key: &str, name: &str, f: &FieldDef) -> ResolvedMember {
        let declared = self.resolve_type(&f.ty, None, false);
        let ty = wrap_list(declared.clone(), f.list.as_deref());
        ResolvedMember {
            name: name.to_string(),
            inherited: false,
            list_kind: f.list.clone(),
            optional: f.optional.unwrap_or(false),
            storage_optional: f.optional.unwrap_or(false),
            private_: f.private_,
            go_only: f.go_only,
            no_go: f.no_go,
            no_ts: f.no_ts,
            no_factory: f.no_factory,
            bitmask: f.bitmask.clone(),
            kind_param: false,
            declared,
            ty,
            visit: f.visit.clone(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_type(
    json: &AstJson,
    kind_aliases: &[KindAliasInfo],
    kind_alias_names: &HashSet<String>,
    kind_element_names: &[String],
    syntax_kind_to_node: &HashMap<String, (Option<String>, String)>,
    instantiation_alias: &HashMap<String, String>,
    spec: &TypeSpec,
    node_ctx: Option<&str>,
    as_node: bool,
) -> Type {
    match spec {
        TypeSpec::Many(list) => Type::Union(
            list.iter()
                .map(|t| {
                    resolve_type(
                        json,
                        kind_aliases,
                        kind_alias_names,
                        kind_element_names,
                        syntax_kind_to_node,
                        instantiation_alias,
                        &TypeSpec::One(t.clone()),
                        node_ctx,
                        as_node,
                    )
                })
                .collect(),
        ),
        TypeSpec::One(name) => resolve_type_name(
            json,
            kind_aliases,
            kind_alias_names,
            kind_element_names,
            syntax_kind_to_node,
            instantiation_alias,
            name,
            node_ctx,
            as_node,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_type_name(
    json: &AstJson,
    kind_aliases: &[KindAliasInfo],
    kind_alias_names: &HashSet<String>,
    kind_element_names: &[String],
    syntax_kind_to_node: &HashMap<String, (Option<String>, String)>,
    instantiation_alias: &HashMap<String, String>,
    name: &str,
    node_ctx: Option<&str>,
    as_node: bool,
) -> Type {
    // 1. type parameters of the node context
    if let Some(key) = node_ctx {
        if let Some(def) = json.nodes.definitions.get(key) {
            for tp in &def.type_parameters {
                if tp.name == name {
                    let constraint = if tp.constraint == name {
                        Type::Primitive(name.to_string())
                    } else {
                        resolve_type_name(
                            json,
                            kind_aliases,
                            kind_alias_names,
                            kind_element_names,
                            syntax_kind_to_node,
                            instantiation_alias,
                            &tp.constraint,
                            Some(key),
                            false,
                        )
                    };
                    return Type::TypeParam {
                        constraint: Box::new(constraint),
                    };
                }
            }
        }
    }

    // 2. kind unions
    if kind_alias_names.contains(name) {
        let members = expand_kind_alias_members(kind_aliases, kind_alias_names, kind_element_names, name);
        if as_node {
            let types = members
                .iter()
                .map(|k| {
                    let kname = kind_name_of(k);
                    match syntax_kind_to_node.get(kname) {
                        Some((alias_name, node_name)) => match alias_name {
                            Some(a) => Type::Alias {
                                name: a.clone(),
                                resolved: Box::new(Type::Node(node_name.clone())),
                            },
                            None => Type::Node(node_name.clone()),
                        },
                        None => panic!("kind alias member \"{kname}\" (from \"{name}\") does not resolve to a node type"),
                    }
                })
                .collect();
            return Type::Union(types);
        }
        return Type::Alias {
            name: name.to_string(),
            resolved: Box::new(Type::Union(
                members.into_iter().map(Type::Kind).collect(),
            )),
        };
    }

    // 3. raw kind references
    if name == "Kind" || name.starts_with("SyntaxKind.") {
        return Type::Kind(name.to_string());
    }

    // 4. the Node type
    if name == "Node" {
        return Type::Node("Node".to_string());
    }

    // 5. primitives
    if PRIMITIVES.contains(&name) {
        return Type::Primitive(name.to_string());
    }

    // 6. NodeList aliases
    if let Some(target) = json.nodes.list_aliases.get(name) {
        let inner = resolve_type_name(
            json,
            kind_aliases,
            kind_alias_names,
            kind_element_names,
            syntax_kind_to_node,
            instantiation_alias,
            target,
            node_ctx,
            false,
        );
        return Type::Alias {
            name: name.to_string(),
            resolved: Box::new(Type::List {
                element: Box::new(inner),
                list_kind: "NodeList".to_string(),
            }),
        };
    }

    // 7. instantiation aliases (Token instantiations)
    if let Some(node) = instantiation_alias.get(name) {
        return Type::Alias {
            name: name.to_string(),
            resolved: Box::new(Type::Node(node.clone())),
        };
    }

    // 8. node union aliases / base category aliases
    if let Some(alias) = json.nodes.aliases.get(name) {
        return match alias {
            NodeAliasValue::Union(members) => {
                let types = members
                    .iter()
                    .map(|m| {
                        resolve_type_name(
                            json,
                            kind_aliases,
                            kind_alias_names,
                            kind_element_names,
                            syntax_kind_to_node,
                            instantiation_alias,
                            m,
                            node_ctx,
                            as_node,
                        )
                    })
                    .collect();
                Type::Alias {
                    name: name.to_string(),
                    resolved: Box::new(Type::Union(types)),
                }
            }
            NodeAliasValue::Base { base } => Type::Alias {
                name: name.to_string(),
                resolved: Box::new(Type::Node(base.clone())),
            },
        };
    }

    // 9. concrete nodes
    if json.nodes.definitions.contains_key(name) {
        return Type::Node(name.to_string());
    }

    // 10. bases
    if json.bases.contains_key(name) {
        return Type::Node(name.to_string());
    }

    // 11. raw Go type strings (e.g. "*FlowNode", "atomic.Uint32", "SymbolTable")
    Type::Primitive(name.to_string())
}

fn expand_kind_alias_members(
    kind_aliases: &[KindAliasInfo],
    kind_alias_names: &HashSet<String>,
    _kind_element_names: &[String],
    name: &str,
) -> Vec<String> {
    match kind_aliases.iter().find(|a| a.name == name) {
        None => vec![format!("SyntaxKind.{name}")],
        Some(alias) => {
            let mut result = Vec::new();
            for m in &alias.members {
                if kind_alias_names.contains(m) {
                    result.extend(expand_kind_alias_members(
                        kind_aliases,
                        kind_alias_names,
                        _kind_element_names,
                        m,
                    ));
                } else {
                    result.push(format!("SyntaxKind.{m}"));
                }
            }
            result
        }
    }
}

fn resolve_marker_value<'a>(json: &'a AstJson, name: &'a str, _marker_names: &HashSet<String>) -> &'a str {
    if let Some(k) = json.kinds.as_ref() {
        if let Some(m) = k.markers.iter().find(|m| m.name == name) {
            return resolve_marker_value(json, &m.value, _marker_names);
        }
    }
    name
}

fn collect_kind_types(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Kind(v) => {
            if v != "Kind" {
                out.push(v.clone());
            }
        }
        Type::Union(types) => {
            for t in types {
                collect_kind_types(t, out);
            }
        }
        Type::Alias { resolved, .. } => collect_kind_types(resolved, out),
        Type::TypeParam { constraint } => collect_kind_types(constraint, out),
        _ => {}
    }
}

fn wrap_list(declared: Type, list_kind: Option<&str>) -> Type {
    match list_kind {
        Some(lk) => Type::List {
            element: Box::new(declared),
            list_kind: lk.to_string(),
        },
        None => declared,
    }
}

fn type_cache_key(t: &Type) -> String {
    match t {
        Type::Primitive(p) => format!("primitive:{p}"),
        Type::Kind(v) => format!("kind:{v}"),
        Type::Node(n) => format!("node:{n}"),
        Type::TypeParam { constraint } => format!("typeParameter:{}", type_cache_key(constraint)),
        Type::Alias { name, resolved } => format!("alias:{name}:{}", type_cache_key(resolved)),
        Type::List { element, list_kind } => {
            format!("list:{list_kind}:{}", type_cache_key(element))
        }
        Type::Union(types) => {
            format!("union:{}", types.iter().map(type_cache_key).collect::<Vec<_>>().join("|"))
        }
    }
}

#[derive(Clone)]
struct ResolvedMember {
    name: String,
    inherited: bool,
    list_kind: Option<String>,
    optional: bool,
    /// Optionality of the struct storage (base field's for inherited members).
    storage_optional: bool,
    private_: bool,
    go_only: bool,
    no_go: bool,
    no_ts: bool,
    no_factory: bool,
    /// ast.json `bitmask` — the member's own value or the inherited base
    /// field's (used by the factory to mask flag members, mirroring
    /// generate-go-ast's `m.bitmask`).
    bitmask: Option<String>,
    kind_param: bool,
    declared: Type,
    ty: Type,
    /// VisitEachChild routing hint (ast.json `visit` attribute); member's own
    /// value or the inherited base field's.
    visit: Option<String>,
}

impl ResolvedMember {
    fn is_child(&self) -> bool {
        matches!(base_kind(&self.ty), BaseKind::List | BaseKind::Node)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Rust-side type mapping (mirror of formatGoReference, adapted to the arena
// model of SPEC §5.1: Go *Node children become NodeId; lists become inline
// Option<NodeList>/Option<ModifierList>)
// ────────────────────────────────────────────────────────────────────────────

fn concrete_node_name(s: &Schema, t: &Type) -> Option<String> {
    match t {
        Type::Node(n) => {
            if s.json.nodes.definitions.contains_key(n) {
                Some(n.clone())
            } else {
                None
            }
        }
        Type::Alias { name, .. } => s.instantiation_alias.get(name).cloned(),
        Type::TypeParam { constraint } => concrete_node_name(s, constraint),
        _ => None,
    }
}

/// Whether a generated struct field's storage type is `Copy` (the rebuild and
/// Clone emissions copy such fields directly instead of `.clone()`, keeping
/// clippy::clone_on_copy out of generated code).
fn is_copy_storage(ty: &str) -> bool {
    match ty {
        "bool" | "i32" | "u32" | "Kind" | "NodeId" | "TokenFlags" | "NodeFlags"
        | "ModifierFlags" => true,
        _ => {
            ty.starts_with("Option<")
                && matches!(
                    ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')),
                    Some("NodeId" | "FlowNodeId" | "SymbolId" | "Kind")
                )
        }
    }
}

fn wrap_opt(ty: &str, optional: bool) -> String {
    if optional {
        format!("Option<{ty}>")
    } else {
        ty.to_string()
    }
}

fn storage_type(s: &Schema, t: &Type, optional: bool) -> String {
    match t {
        Type::Primitive(p) => match p.as_str() {
            "bool" | "boolean" => wrap_opt("bool", optional),
            "int" => wrap_opt("i32", optional),
            "string" => wrap_opt("Box<str>", optional),
            "any" => wrap_opt("std::sync::Arc<dyn std::any::Any + Send + Sync>", optional),
            "NodeFlags" => wrap_opt("NodeFlags", optional),
            "TokenFlags" => wrap_opt("TokenFlags", optional),
            "ModifierFlags" => wrap_opt("ModifierFlags", optional),
            // Go-only link fields (raw Go type strings from ast.json).
            // These are nullable in Go and assigned after construction, so
            // they become Option<...Id>. SPEC §5.1/§5.3: flow/symbol links are
            // binder-era state; the ids keep the layout mirrorable until the
            // side-table port decides otherwise.
            "*FlowNode" => "Option<FlowNodeId>".to_string(),
            "*Symbol" => "Option<SymbolId>".to_string(),
            "*Node" => "Option<NodeId>".to_string(),
            "SymbolTable" => "SymbolTable".to_string(),
            "atomic.Uint32" => "u32".to_string(),
            other => panic!("unmapped primitive type: {other}"),
        },
        Type::Kind(_) => wrap_opt("Kind", optional),
        Type::Node(_) => wrap_opt("NodeId", optional),
        Type::TypeParam { constraint } => storage_type(s, constraint, optional),
        Type::Alias { resolved, .. } => storage_type(s, resolved, optional),
        Type::List { element, list_kind } => {
            if list_kind == "raw" {
                if base_kind(element) == BaseKind::Node {
                    "Vec<NodeId>".to_string()
                } else {
                    match element.as_ref() {
                        Type::Primitive(p) if p == "string" => "Vec<Box<str>>".to_string(),
                        other => format!("Vec<{}>", storage_type(s, other, false)),
                    }
                }
            } else if list_kind == "ModifierList" {
                "Option<ModifierList>".to_string()
            } else if list_kind == "NodeList" {
                "Option<NodeList>".to_string()
            } else {
                panic!("unknown list kind {list_kind}");
            }
        }
        Type::Union(types) => {
            // Mirror of UnionType.formatGoReference, in storage form.
            let mut names: Vec<String> = Vec::new();
            let mut all_concrete = true;
            for t in types {
                match concrete_node_name(s, t) {
                    Some(n) => {
                        if !names.contains(&n) {
                            names.push(n);
                        }
                    }
                    None => {
                        all_concrete = false;
                        break;
                    }
                }
            }
            if all_concrete && names.len() == 1 {
                return wrap_opt("NodeId", optional);
            }
            if types
                .iter()
                .any(|t| matches!(base_kind(t), BaseKind::List | BaseKind::Node))
            {
                return wrap_opt("NodeId", optional);
            }
            if types.iter().all(|t| base_kind(t) == BaseKind::Kind) {
                return wrap_opt("Kind", optional);
            }
            let mut refs: Vec<String> = Vec::new();
            for t in types {
                let r = storage_type(s, t, false);
                if !refs.contains(&r) {
                    refs.push(r);
                }
            }
            if refs.len() == 1 {
                return wrap_opt(&refs[0], optional);
            }
            panic!(
                "cannot resolve Rust storage form for union: {}",
                refs.join(" | ")
            );
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Naming helpers
// ────────────────────────────────────────────────────────────────────────────

/// Go PascalCase identifier → snake_case. "JSDoc" is one word in Go naming
/// (schema.ts's uncapitalize maps it to "jsdoc"), so it becomes a single
/// `jsdoc` token rather than `js_doc`.
fn to_snake(s: &str) -> String {
    if s.contains("JSDoc") {
        let mut parts = s.split("JSDoc");
        let first = parts.next().unwrap_or("");
        let mut out = String::new();
        if !first.is_empty() {
            out.push_str(&snake_word(first));
            out.push('_');
        }
        out.push_str("jsdoc");
        for rest in parts {
            if rest.is_empty() {
                continue;
            }
            out.push('_');
            out.push_str(&snake_word(rest));
        }
        return out;
    }
    snake_word(s)
}

fn snake_word(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let acronym_end = i > 0
                && chars[i - 1].is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && (prev_lower || acronym_end) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

/// snake_case field name with Rust keyword avoidance.
fn rust_field_name(go_name: &str) -> String {
    let s = to_snake(go_name);
    const KEYWORDS: [&str; 5] = ["type", "fn", "impl", "trait", "box"];
    if KEYWORDS.contains(&s.as_str()) {
        format!("{s}_")
    } else {
        s
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Output writer
// ────────────────────────────────────────────────────────────────────────────

struct Out {
    buf: String,
}

impl Out {
    fn new() -> Out {
        Out { buf: String::new() }
    }
    fn line(&mut self, s: impl AsRef<str>) {
        self.buf.push_str(s.as_ref());
        self.buf.push('\n');
    }
    fn finish(self) -> String {
        self.buf
    }
}

// ────────────────────────────────────────────────────────────────────────────
// kind_generated.rs
// ────────────────────────────────────────────────────────────────────────────

fn generate_kind(s: &Schema) -> String {
    let mut o = Out::new();
    o.line("// Code generated by rust/tools/gen-ast from tools/scripts/tsc/ast.json. DO NOT EDIT.");
    o.line("// Rust mirror of tsc/internal/ast/kind_generated.go + kind_stringer_generated.go.");
    o.line("// Variant order mirrors the Go `Kind` iota ordinals exactly (`Kind::Count` is the");
    o.line("// count sentinel, exactly as in Go).");
    o.line("");
    o.line("use std::fmt;");
    o.line("");
    o.line("/// Go: `type Kind int16` — the SyntaxKind enum. Node kinds are stored on");
    o.line("/// [`super::Node`] (SPEC §5.1).");
    o.line("#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]");
    o.line("#[repr(u16)]");
    o.line("pub enum Kind {");
    let mut first = true;
    for (name, comment) in &s.kind_elements {
        let Some(name) = name else {
            if let Some(c) = comment {
                o.line(format!("// {c}"));
            }
            continue;
        };
        match comment {
            Some(c) if first => {
                o.line(format!("{name}, // {c} // Go: Kind{name} Kind = iota"));
                first = false;
            }
            Some(c) => o.line(format!("{name}, // {c}")),
            None if first => {
                o.line(format!("{name}, // Go: Kind{name} Kind = iota"));
                first = false;
            }
            None => o.line(format!("{name},")),
        }
    }
    o.line("/// Go: `KindCount` — count sentinel (ordinal = number of kinds).");
    o.line("Count,");
    o.line("}");
    o.line("");

    // Marker consts (Go: `KindFirstAssignment = KindEqualsToken` etc.).
    // Names stay Go-exact per SPEC §4.1 (grep-ability against the Go source).
    o.line("#[allow(non_upper_case_globals)]");
    o.line("impl Kind {");
    if let Some(k) = s.json.kinds.as_ref() {
        for m in &k.markers {
            o.line(format!(
                "    /// Go: `Kind{} = Kind{}`\n    pub const {}: Kind = Kind::{};",
                m.name, m.value, m.name, m.value,
            ));
        }
    }
    o.line("}");
    o.line("");

    // Kind-union type aliases (Go: `type BinaryOperator = Kind // members...`)
    o.line("// Kind-union type aliases (Go: `type X = Kind // KindA | KindB | ...`).");
    o.line("// Membership is checked with the guard functions below, mirroring the Go");
    o.line("// Is* kind guards.");
    for alias in &s.kind_aliases {
        let expanded = expand_alias_members(s, &alias.name);
        let list = expanded
            .iter()
            .map(|k| format!("Kind::{}", kind_name_of(k)))
            .collect::<Vec<_>>()
            .join(" | ");
        o.line(format!("/// Go: `type {} = Kind // {}`", alias.name, list.replace("Kind::", "Kind")));
        o.line(format!("pub type {} = Kind;", alias.name));
    }
    o.line("");

    // Kind guards (mirror of generateKindAliasGuards)
    o.line("// Kind alias guards (Go: IsTokenKind etc.; IsJSDocKind is skipped exactly as in");
    o.line("// generate-go-ast.ts — the hand-written IsJSDocKind in utilities.go has different");
    o.line("// semantics).");
    for alias in &s.kind_aliases {
        let ts_guard = format!("is{}", alias.name.replace("Syntax", ""));
        // kindGuardName → "isTokenKind"; goKindGuardName capitalizes it → "IsTokenKind".
        let go_guard = format!("I{}", &ts_guard[1..]);
        if go_guard == "IsJSDocKind" {
            continue;
        }
        let rust_name = to_snake(&go_guard);
        match &alias.range {
            Some((first, last)) => {
                o.line(format!(
                    "/// Go: `func {go_guard}(kind Kind) bool`\npub fn {rust_name}(kind: Kind) -> bool {{\n    kind >= Kind::{first} && kind <= Kind::{last}\n}}",
                ));
            }
            None => {
                let expanded = expand_alias_members(s, &alias.name);
                let arms = expanded
                    .iter()
                    .map(|k| format!("Kind::{}", kind_name_of(k)))
                    .collect::<Vec<_>>()
                    .join(" | ");
                o.line(format!(
                    "/// Go: `func {go_guard}(kind Kind) bool`\npub fn {rust_name}(kind: Kind) -> bool {{\n    matches!(kind, {arms})\n}}",
                ));
            }
        }
        o.line("");
    }

    // name() — replaces kind_stringer_generated.go; returns the exact Go
    // String() text ("KindUnknown", ...) for baseline parity.
    o.line("impl Kind {");
    o.line("    /// The Go `Kind.String()` text (e.g. `KindUnknown`), replacing");
    o.line("    /// kind_stringer_generated.go. Byte-identical to Go so baselines match.");
    o.line("    pub fn name(self) -> &'static str {");
    o.line("        match self {");
    for (name, _) in &s.kind_elements {
        let Some(name) = name else { continue };
        o.line(format!("            Kind::{name} => \"Kind{name}\","));
    }
    o.line("            Kind::Count => \"KindCount\",");
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.line("");
    o.line("impl fmt::Display for Kind {");
    o.line("    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {");
    o.line("        f.write_str(self.name())");
    o.line("    }");
    o.line("}");
    o.finish()
}

/// Fully expand a kind alias to concrete kind values (schema.ts expandMembers
/// in generateKind / expandKindAliasMembers in SchemaAPI).
fn expand_alias_members(s: &Schema, name: &str) -> Vec<String> {
    fn expand(s: &Schema, members: &[String], out: &mut Vec<String>) {
        for m in members {
            if let Some(alias) = s.get_kind_alias(m) {
                expand(s, &alias.members, out);
            } else {
                out.push(format!("SyntaxKind.{m}"));
            }
        }
    }
    let mut out = Vec::new();
    if let Some(alias) = s.get_kind_alias(name) {
        expand(s, &alias.members, &mut out);
    } else {
        out.push(format!("SyntaxKind.{name}"));
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// ast_generated.rs
// ────────────────────────────────────────────────────────────────────────────

enum FieldLine {
    Comment(String),
    Field {
        name: String,
        ty: String,
        optional: bool,
    },
}

fn emit_base_into(s: &Schema, key: &str, lines: &mut Vec<FieldLine>) {
    if key == "NodeBase" {
        lines.push(FieldLine::Comment(
            "// NodeBase (kind/flags/loc/id/parent) — stored on Node, not NodeData (SPEC §5.1)."
                .to_string(),
        ));
        return;
    }
    if s.is_zero_size_base(key) {
        lines.push(FieldLine::Comment(format!(
            "// {key} (marker base; no fields)"
        )));
        return;
    }
    for e in s.base_go_embeds(key) {
        emit_base_into(s, &e, lines);
    }
    let base = s.base(key).unwrap();
    for (fname, fdef) in &base.fields {
        if fdef.no_go {
            continue;
        }
        let r = s.resolve_base_field(key, fname, fdef);
        lines.push(FieldLine::Field {
            name: rust_field_name(fname),
            ty: storage_type(s, &r.ty, r.optional),
            optional: r.optional,
        });
    }
}

fn node_struct_lines(s: &Schema, key: &str) -> Vec<FieldLine> {
    let def = s.node_def(key);
    let mut lines = Vec::new();
    for e in s.go_embeds(&def.extends) {
        emit_base_into(s, &e, &mut lines);
    }
    for m in &def.members {
        if m.inherited {
            continue;
        }
        let r = s.resolve_member(key, m);
        if r.kind_param || r.no_go {
            continue;
        }
        lines.push(FieldLine::Field {
            name: rust_field_name(&r.name),
            ty: storage_type(s, &r.ty, r.optional),
            optional: r.optional,
        });
    }
    lines
}

/// The generated per-struct for_each_child body lines (mirror of
/// generateForEachChild: children in member order; visitor returns true to
/// stop, exactly like Go's Visitor).
fn for_each_child_lines(s: &Schema, key: &str) -> Option<Vec<String>> {
    let def = s.node_def(key);
    let mut body = Vec::new();
    for m in &def.members {
        if m.no_factory {
            continue;
        }
        let r = s.resolve_member(key, m);
        if !r.is_child() {
            continue;
        }
        let f = rust_field_name(&r.name);
        match (r.list_kind.as_deref(), base_kind(&r.ty)) {
            (Some("raw"), BaseKind::List) => {
                body.push(format!("for &id in &self.{f} {{"));
                body.push("    if visit(id) {".into());
                body.push("        return true;".into());
                body.push("    }".into());
                body.push("}".into());
            }
            (Some("ModifierList"), _) | (Some("NodeList"), _) => {
                body.push(format!("if let Some(list) = &self.{f} {{"));
                body.push("    for &id in &list.nodes {".into());
                body.push("        if visit(id) {".into());
                body.push("            return true;".into());
                body.push("        }".into());
                body.push("    }".into());
                body.push("}".into());
            }
            (None, BaseKind::Node) => {
                if r.storage_optional {
                    body.push(format!("if let Some(id) = &self.{f} {{"));
                    body.push("    if visit(*id) {".into());
                    body.push("        return true;".into());
                    body.push("    }".into());
                    body.push("}".into());
                } else {
                    // Go's ForEachChild emits every slot through the visit()
                    // helper, which filters nil before calling the Visitor
                    // (ast.go). Mandatory NodeId slots carry NodeId::NONE
                    // for Go's nil (factory mapping), so mirror the filter.
                    body.push(format!(
                        "if self.{f} != NodeId::NONE && visit(self.{f}) {{"
                    ));
                    body.push("    return true;".into());
                    body.push("}".into());
                }
            }
            _ => {}
        }
    }
    if body.is_empty() {
        return None;
    }
    body.push("false".into());
    Some(body)
}

/// How a child member routes through the NodeVisitor in the generated
/// VisitEachChild (mirror of the v.visitX call the Go generator emits).
#[derive(Clone, Copy, PartialEq)]
enum ChildRoute {
    /// `v.visit_x(Option<NodeId>) -> Option<NodeId>` (visit_node / visit_token /
    /// visit_embedded_statement / visit_iteration_body / visit_function_body).
    Node(&'static str),
    /// `v.visit_x(Option<&NodeList>) -> Option<NodeList>`
    /// (visit_nodes / visit_parameters / visit_top_level_statements).
    Nodes(&'static str),
    /// `v.visit_modifiers(Option<&ModifierList>) -> Option<ModifierList>`.
    Modifiers,
    /// Go `core.SameMap(children, v.visitNode)` over a raw `[]*Node` member
    /// (SyntaxList.Children, JSDocTypeLiteral.JSDocPropertyTags); dropped
    /// elements become `NodeId::NONE` (Go keeps nil in the slice).
    RawNodeList,
}

fn child_route(key: &str, r: &ResolvedMember) -> ChildRoute {
    let _ = key;
    if !r.is_child() {
        panic!("child_route on non-child member {}.{}", key, r.name);
    }
    match r.list_kind.as_deref() {
        Some("raw") => {
            // Raw node slices are visited element-wise via visitNode (Go SameMap).
            assert_eq!(base_kind(&r.ty), BaseKind::List);
            return ChildRoute::RawNodeList;
        }
        Some("ModifierList") => return ChildRoute::Modifiers,
        Some("NodeList") => {
            return ChildRoute::Nodes(match r.visit.as_deref() {
                Some("parameters") => "visit_parameters",
                Some("topLevelStatements") => "visit_top_level_statements",
                None => "visit_nodes",
                Some(other) => panic!("unmapped NodeList visit route: {other}"),
            });
        }
        _ => {}
    }
    assert_eq!(base_kind(&r.ty), BaseKind::Node);
    ChildRoute::Node(match r.visit.as_deref() {
        Some("embeddedStatement") => "visit_embedded_statement",
        Some("iterationBody") => "visit_iteration_body",
        Some("functionBody") => "visit_function_body",
        Some("token") => "visit_token",
        None => "visit_node",
        Some(other) => panic!("unmapped node visit route: {other}"),
    })
}

/// The generated per-struct visit_each_child body lines (mirror of Go's
/// generated `VisitEachChild` + the factory `Update*` rebuild merged: all
/// children are visited; if none changed the method returns None, mirroring
/// Update*'s "return the original node" identity check).
fn visit_each_child_lines(s: &Schema, key: &str) -> Option<Vec<String>> {
    // JSDocParameterOrPropertyTag's visitor is hand-written in Go
    // (visitEachChild_JSDocParameterOrPropertyTag, ast.go); the ast.json member
    // order (tag_name, comment, name, ...) differs from Go's hand-written
    // argument order, so the body is emitted literally.
    if key == "JSDocParameterOrPropertyTag" {
        return Some(vec![
            "let tag_name = v.visit_node(Some(self.tag_name));".into(),
            "let name = v.visit_node(Some(self.name));".into(),
            "let type_expression = v.visit_node(self.type_expression);".into(),
            "let comment = v.visit_nodes(self.comment.as_ref());".into(),
            "if tag_name == Some(self.tag_name)".into(),
            "    && name == Some(self.name)".into(),
            "    && type_expression == self.type_expression".into(),
            "    && comment == self.comment".into(),
            "{".into(),
            "    return None;".into(),
            "}".into(),
            "Some(Self {".into(),
            "    tag_name: tag_name.unwrap_or(NodeId::NONE),".into(),
            "    comment,".into(),
            "    name: name.unwrap_or(NodeId::NONE),".into(),
            "    is_bracketed: self.is_bracketed,".into(),
            "    type_expression,".into(),
            "    is_name_first: self.is_name_first,".into(),
            "})".into(),
        ]);
    }
    let def = s.node_def(key);
    // Visited children in ast.json member order.
    let mut visited: Vec<(String, ChildRoute, bool)> = Vec::new(); // (field, route, storage-optional)
    for m in &def.members {
        if m.no_factory {
            continue;
        }
        let r = s.resolve_member(key, m);
        if !r.is_child() {
            continue;
        }
        let route = child_route(key, &r);
        // Storage optionality mirrors the emitted struct field (base field's
        // for inherited members — see resolve_member's storage_optional note).
        let storage_optional = if r.inherited { r.storage_optional } else { r.optional };
        visited.push((rust_field_name(&r.name), route, storage_optional));
    }
    if visited.is_empty() {
        return None;
    }

    let mut body = Vec::new();
    let mut checks = Vec::new();
    for (f, route, optional) in &visited {
        match route {
            ChildRoute::Node(disp) => {
                if *optional {
                    body.push(format!("let {f} = v.{disp}(self.{f});"));
                    checks.push(format!("{f} == self.{f}"));
                } else {
                    body.push(format!("let {f} = v.{disp}(Some(self.{f}));"));
                    checks.push(format!("{f} == Some(self.{f})"));
                }
            }
            ChildRoute::Nodes(disp) => {
                body.push(format!("let {f} = v.{disp}(self.{f}.as_ref());"));
                checks.push(format!("{f} == self.{f}"));
            }
            ChildRoute::Modifiers => {
                body.push(format!("let {f} = v.visit_modifiers(self.{f}.as_ref());"));
                checks.push(format!("{f} == self.{f}"));
            }
            ChildRoute::RawNodeList => {
                body.push(format!(
                    "let {f}: Vec<NodeId> = self.{f}.iter().map(|&c| v.visit_node(Some(c)).unwrap_or(NodeId::NONE)).collect();"
                ));
                checks.push(format!("{f} == self.{f}"));
            }
        }
    }
    body.push(format!(
        "if {} {{",
        checks.join("\n            && ")
    ));
    body.push("    return None;".into());
    body.push("}".into());
    body.push("Some(Self {".into());
    // Every struct field gets a value: visited children take the visited
    // result, everything else is copied from the original (Go's Update* passes
    // non-child members straight through).
    let route_of: std::collections::HashMap<&String, (ChildRoute, bool)> = visited
        .iter()
        .map(|(f, route, opt)| (f, (*route, *opt)))
        .collect();
    for line in node_struct_lines(s, key) {
        if let FieldLine::Field { name, ty, .. } = line {
            if let Some((route, optional)) = route_of.get(&name) {
                match route {
                    ChildRoute::Node(_) => {
                        if *optional {
                            body.push(format!("    {name},"));
                        } else {
                            body.push(format!("    {name}: {name}.unwrap_or(NodeId::NONE),"));
                        }
                    }
                    _ => body.push(format!("    {name},")),
                }
            } else if is_copy_storage(&ty) {
                body.push(format!("    {name}: self.{name},"));
            } else {
                body.push(format!("    {name}: self.{name}.clone(),"));
            }
        }
    }
    body.push("})".into());
    Some(body)
}

fn generate_ast(s: &Schema) -> String {
    let mut o = Out::new();
    o.line("// Code generated by rust/tools/gen-ast from tools/scripts/tsc/ast.json. DO NOT EDIT.");
    o.line("// Rust mirror of tsc/internal/ast/ast_generated.go:");
    o.line("//   - node structs with base-struct composition FLATTENED (Go embedding order");
    o.line("//     preserved; NodeBase's kind/flags/loc/id/parent live on Node per SPEC §5.1)");
    o.line("//   - `NodeData` — one variant per concrete node struct (SPEC §5.1); every");
    o.line("//     payload is Boxed so `Node` stays a fixed 48-byte arena slot (SPEC §12.1:");
    o.line("//     ≤64B target; box-everything beats per-variant sizing because enum size");
    o.line("//     is the max variant size, and Go's nodeData interface is one indirection");
    o.line("//     anyway)");
    o.line("//   - `as_x` accessors (Go AsX; 192) and `is_x` predicates (Go IsX)");
    o.line("//   - `for_each_child` (Go ForEachChild; children in ast.json member order;");
    o.line("//     JSDocParameterOrPropertyTag ports the hand-written ast.go visitor)");
    o.line("//   - `visit_each_child` (Go VisitEachChild; returns Option — None = every");
    o.line("//     child unchanged, mirroring Go Update* returning the original node;");
    o.line("//     SPEC §5.6) and the Go ast.go `Node.Modifiers()`/`Node.Name()` dispatches");
    o.line("// Go *Node children are NodeId handles into the arena; Go *NodeList/*ModifierList");
    o.line("// become inline Option<NodeList>/Option<ModifierList> (SPEC §5.1).");
    o.line("");
    o.line("use std::cell::Cell;");
    o.line("");
    o.line("use super::*;");
    o.line("");

    // NodeList aliases (Go: `type StatementList = NodeList`)
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// NodeList type aliases");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    let mut used_names: HashSet<String> = HashSet::new();
    for key in s.json.bases.keys() {
        used_names.insert(key.clone());
    }
    for key in s.json.nodes.definitions.keys() {
        used_names.insert(key.clone());
    }
    for (alias, element) in &s.json.nodes.list_aliases {
        o.line(format!("/// Go: `type {alias} = NodeList // NodeList[*{element}]`"));
        o.line(format!("pub type {alias} = NodeList;"));
        used_names.insert(alias.clone());
    }
    o.line("");

    // Node union aliases + instantiation aliases (Go: `type X = Node`)
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// Node union aliases (Go: `type X = Node`; children are NodeId here)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    for (name, value) in &s.json.nodes.aliases {
        if used_names.contains(name) {
            continue;
        }
        match value {
            NodeAliasValue::Base { base } => {
                o.line(format!("/// Go: `type {name} = Node // Node with {base}`"));
            }
            NodeAliasValue::Union(members) => {
                o.line(format!("/// Go: `type {name} = Node // {}`", members.join(" | ")));
            }
        }
        o.line(format!("pub type {name} = NodeId;"));
        used_names.insert(name.clone());
    }
    for (key, def) in &s.json.nodes.definitions {
        for (alias, type_arg) in &def.instantiation_aliases {
            if used_names.contains(alias) {
                continue;
            }
            o.line(format!(
                "/// Go: `type {alias} = Node` (instantiation of {key} with {type_arg})"
            ));
            o.line(format!("pub type {alias} = NodeId;"));
            used_names.insert(alias.clone());
        }
    }
    o.line("");

    // Struct definitions (flattened)
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// Node structs (base-struct composition flattened)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    for (key, def) in s.nodes() {
        if def.hand_written {
            o.line(format!(
                "// {key}: struct hand-written in tsc/internal/ast/ast.go; the node-data payload"
            ));
            o.line(format!(
                "// ({key}NodeData) and the full owning struct live in the hand-written core"
            ));
            o.line("");
            continue;
        }
        let field_lines = node_struct_lines(s, key);
        let embeds = s.go_embeds(&def.extends);
        o.line(format!(
            "/// Go: `type {key} struct {{ {} }}` (embeds flattened in Go embedding order)",
            embeds.join("; ")
        ));
        // `Arc<dyn Any + Send + Sync>` members (Go `Type any`) are Clone, so
        // every struct derives Clone (Go copies the interface value on clone;
        // the Arc shares the payload).
        o.line("#[derive(Clone)]");
        o.line(format!("pub struct {key} {{"));
        for line in &field_lines {
            match line {
                FieldLine::Comment(c) => o.line(format!("    {c}")),
                FieldLine::Field { name, ty, optional } => {
                    if *optional {
                        o.line(format!("    pub {name}: {ty}, // Optional"));
                    } else {
                        o.line(format!("    pub {name}: {ty},"));
                    }
                }
            }
        }
        o.line("}");
        o.line("");
    }

    // per-struct for_each_child
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// ForEachChild (children in ast.json member order; the visitor returns");
    o.line("// true to stop, mirroring Go's Visitor; the method returns whether the");
    o.line("// traversal was stopped early)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    for (key, def) in s.nodes() {
        if def.hand_written {
            continue; // SourceFile: impl lives with the hand-written core
        }
        let Some(body) = for_each_child_lines(s, key) else {
            continue;
        };
        o.line(format!("impl {key} {{"));
        o.line("    pub fn for_each_child(&self, visit: &mut dyn FnMut(NodeId) -> bool) -> bool {");
        if def.hand_written_visitor {
            // Ported from forEachChild_JSDocParameterOrPropertyTag in
            // tsc/internal/ast/ast.go (runtime-dependent child ordering — Go's
            // generated code delegates to the hand-written function, so the
            // generator emits the equivalent here):
            //   visit(v, node.TagName) ||
            //     (node.IsNameFirst && (visit(v, node.name) || visit(v, node.TypeExpression))) ||
            //     (!node.IsNameFirst && (visit(v, node.TypeExpression) || visit(v, node.name))) ||
            //     visitNodeList(v, node.Comment)
            assert_eq!(key, "JSDocParameterOrPropertyTag");
            let _ = body;
            o.line("        if self.tag_name != NodeId::NONE && visit(self.tag_name) {");
            o.line("            return true;");
            o.line("        }");
            o.line("        if self.is_name_first {");
            o.line("            if self.name != NodeId::NONE && visit(self.name) {");
            o.line("                return true;");
            o.line("            }");
            o.line("            if let Some(id) = self.type_expression {");
            o.line("                if visit(id) {");
            o.line("                    return true;");
            o.line("                }");
            o.line("            }");
            o.line("        } else {");
            o.line("            if let Some(id) = self.type_expression {");
            o.line("                if visit(id) {");
            o.line("                    return true;");
            o.line("                }");
            o.line("            }");
            o.line("            if self.name != NodeId::NONE && visit(self.name) {");
            o.line("                return true;");
            o.line("            }");
            o.line("        }");
            o.line("        if let Some(list) = &self.comment {");
            o.line("            for &id in &list.nodes {");
            o.line("                if visit(id) {");
            o.line("                    return true;");
            o.line("                }");
            o.line("            }");
            o.line("        }");
            o.line("        false");
        } else {
            for l in &body {
                o.line(format!("        {l}"));
            }
        }
        o.line("    }");
        o.line("}");
        o.line("");
    }

    // Per-struct VisitEachChild
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// VisitEachChild (Go VisitEachChild + the Update* rebuild, merged: children");
    o.line("// are visited through the NodeVisitor dispatchers in ast.json member");
    o.line("// order, and the node is rebuilt only when a child changed — Go's Update*");
    o.line("// returns the original node in that case, which becomes Option::None,");
    o.line("// SPEC §5.6. Mandatory child slots dropped by the visitor become");
    o.line("// NodeId::NONE (Go stores a nil *Node).)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    for (key, def) in s.nodes() {
        if def.hand_written {
            continue; // SourceFile: SourceFileNodeData::visit_each_child lives in the core
        }
        let body = visit_each_child_lines(s, key);
        o.line(format!("impl {key} {{"));
        if let Some(lines) = &body {
            o.line(format!(
                "    /// Go: `func (node *{key}) VisitEachChild(v *NodeVisitor) *Node` (ast_generated.go)."
            ));
            o.line("    pub fn visit_each_child(&self, v: &mut dyn NodeVisitor) -> Option<Self> {");
            for l in lines {
                o.line(format!("        {l}"));
            }
        } else {
            o.line(format!(
                "    /// Go: `func (node *{key}) VisitEachChild(v *NodeVisitor) *Node` (ast_generated.go)"
            ));
            o.line("    /// — the node has no children, so nothing can change.");
            o.line("    pub fn visit_each_child(&self, _v: &mut dyn NodeVisitor) -> Option<Self> {");
            o.line("        None");
        }
        o.line("    }");
        o.line("}");
        o.line("");
    }

    // NodeData
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// NodeData — one variant per concrete node struct (SPEC §5.1); every");
    o.line("// payload is Boxed (see the file header) and Clone (Go's Clone copies");
    o.line("// the struct value via the factory; the port's NodeStore::clone_node");
    o.line("// needs a plain value copy).)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    o.line("#[derive(Clone)]");
    o.line("pub enum NodeData {");
    for (key, def) in s.nodes() {
        if def.hand_written {
            // The SourceFile node's data payload is hand-written in the core
            // (the owning SourceFile struct cannot live inside its own arena).
            o.line(format!(
                "    {key}(Box<{key}NodeData>), // payload hand-written in the core"
            ));
        } else {
            o.line(format!("    {key}(Box<{key}>),"));
        }
    }
    o.line("}");
    o.line("");

    // Node::for_each_child dispatch
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// ForEachChild dispatch (Kind switch → concrete struct, mirroring the Go");
    o.line("// dispatch that avoids an interface call; kinds without children return false)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    o.line("impl Node {");
    o.line("    pub fn for_each_child(&self, visit: &mut dyn FnMut(NodeId) -> bool) -> bool {");
    o.line("        match self.kind {");
    let mut seen_kinds: HashSet<String> = HashSet::new();
    for (key, def) in s.nodes() {
        let has_impl = def.hand_written
            || def.hand_written_visitor
            || s.node_def(key)
                .members
                .iter()
                .filter(|m| !m.no_factory)
                .any(|m| s.resolve_member(key, m).is_child());
        if !has_impl {
            continue;
        }
        let mut kinds = Vec::new();
        for k in s.all_kinds(key) {
            if seen_kinds.insert(k.clone()) {
                kinds.push(k);
            }
        }
        if kinds.is_empty() {
            continue;
        }
        let arms = kinds
            .iter()
            .map(|k| format!("Kind::{}", kind_name_of(k)))
            .collect::<Vec<_>>()
            .join(" | ");
        o.line(format!("            {arms} => match &self.data {{"));
        o.line(format!("                NodeData::{key}(d) => d.for_each_child(visit),"));
        o.line(format!(
            "                _ => panic!(\"kind {{:?}} does not carry {key} data\", self.kind),"
        ));
        o.line("            },");
    }
    o.line("            _ => false,");
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.line("");

    // As* casts
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// As*() cast methods (Go returns *X unchecked; the Rust port returns");
    o.line("// Option — PORT: panics on wrong-kind casts become None at the accessor,");
    o.line("// callers use it after the matching is_x predicate. Payloads are Boxed,");
    o.line("// hence the `&**d`.)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    o.line("impl Node {");
    for (key, def) in s.nodes() {
        let payload = if def.hand_written {
            format!("{key}NodeData")
        } else {
            key.clone()
        };
        o.line(format!(
            "    /// Go: `func (n *Node) As{key}() *{key}`",
        ));
        o.line(format!(
            "    pub fn as_{}(&self) -> Option<&{payload}> {{",
            to_snake(key)
        ));
        o.line("        match &self.data {");
        o.line(format!("            NodeData::{key}(d) => Some(&**d),"));
        o.line("            _ => None,");
        o.line("        }");
        o.line("    }");
        o.line("");
    }
    o.line("}");
    o.line("");

    // Node::visit_each_child dispatch + Modifiers()/ModifierFlags()/Name()
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// VisitEachChild dispatch (Go `Node.VisitEachChild` → nodeData.VisitEachChild;");
    o.line("// dispatches on the data variant, so multi-kind structs like Token and");
    o.line("// CaseOrDefaultClause need no Kind switch). The rebuilt node carries a");
    o.line("// fresh id/parent, exactly like Go's factory New* inside Update*.");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    o.line("impl Node {");
    o.line("    /// Go: `func (n *Node) VisitEachChild(v *NodeVisitor) *Node` — Rust returns");
    o.line("    /// `Option<Node>`: None = every child unchanged (Go returns the same node).");
    o.line("    pub fn visit_each_child(&self, v: &mut dyn NodeVisitor) -> Option<Node> {");
    o.line("        let data = match &self.data {");
    for (key, _def) in s.nodes() {
        o.line(format!(
            "            NodeData::{key}(d) => d.visit_each_child(v).map(|x| NodeData::{key}(Box::new(x))),"
        ));
    }
    o.line("        }?;");
    o.line("        Some(Node {");
    o.line("            kind: self.kind,");
    o.line("            flags: self.flags,");
    o.line("            loc: self.loc,");
    o.line("            id: Cell::new(0),");
    o.line("            parent: Cell::new(NodeId::NONE),");
    o.line("            data,");
    o.line("        })");
    o.line("    }");
    o.line("");
    o.line("    /// Go: `func (n *Node) Modifiers() *ModifierList` (ast.go — dispatches");
    o.line("    /// nodeData.Modifiers(); the flattened per-struct fields replace the");
    o.line("    /// Go interface method).");
    o.line("    pub fn modifiers(&self) -> Option<&ModifierList> {");
    o.line("        match &self.data {");
    for (key, def) in s.nodes() {
        if def.hand_written {
            continue;
        }
        let has_modifiers = node_struct_lines(s, key).iter().any(|l| {
            matches!(l, FieldLine::Field { name, ty, .. } if name == "modifiers" && ty == "Option<ModifierList>")
        });
        if has_modifiers {
            o.line(format!("            NodeData::{key}(d) => d.modifiers.as_ref(),"));
        }
    }
    o.line("            _ => None,");
    o.line("        }");
    o.line("    }");
    o.line("");
    o.line("    /// Go: `func (n *Node) ModifierFlags() ModifierFlags` (ast.go).");
    o.line("    pub fn modifier_flags(&self) -> ModifierFlags {");
    o.line("        self.modifiers().map_or(ModifierFlags::NONE, |m| m.modifier_flags)");
    o.line("    }");
    o.line("");
    o.line("    /// Go: `func (n *Node) Name() *DeclarationName` (ast.go — dispatches");
    o.line("    /// nodeData.Name(); NodeDefault returns nil, mirrored by the `_` arm).");
    o.line("    pub fn name(&self) -> Option<NodeId> {");
    o.line("        match &self.data {");
    for (key, def) in s.nodes() {
        if def.hand_written {
            continue;
        }
        for l in node_struct_lines(s, key) {
            if let FieldLine::Field { name, ty, .. } = l {
                if name == "name" {
                    if ty == "Option<NodeId>" {
                        o.line(format!("            NodeData::{key}(d) => d.name,"));
                    } else if ty == "NodeId" {
                        o.line(format!("            NodeData::{key}(d) => Some(d.name),"));
                    }
                }
            }
        }
    }
    o.line("            _ => None,");
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.line("");

    // Is* functions
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("// Is*() predicates (mirror of generateIsFunction)");
    o.line("// ──────────────────────────────────────────────────────────────────────");
    o.line("");
    for (key, _def) in s.nodes() {
        let kind_types = s.kind_types(key);
        if s.kind_type_is_type_parameter(key) {
            let arms = kind_types
                .iter()
                .map(|k| format!("Kind::{}", kind_name_of(k)))
                .collect::<Vec<_>>()
                .join(" | ");
            o.line(format!(
                "/// Go: `func Is{key}(node *Node) bool`\npub fn {}(node: &Node) -> bool {{\n    matches!(node.kind, {arms})\n}}",
                to_snake(&format!("Is{key}"))
            ));
            o.line("");
            continue;
        }
        if kind_types.len() > 1 {
            for k in &kind_types {
                let name = kind_name_of(k);
                o.line(format!(
                    "/// Go: `func Is{name}(node *Node) bool`\npub fn {}(node: &Node) -> bool {{\n    matches!(node.kind, Kind::{name})\n}}",
                    to_snake(&format!("Is{name}"))
                ));
                o.line("");
            }
            continue;
        }
        let primary = s.syntax_kind_name(key);
        o.line(format!(
            "/// Go: `func Is{key}(node *Node) bool`\npub fn {}(node: &Node) -> bool {{\n    matches!(node.kind, Kind::{primary})\n}}",
            to_snake(&format!("Is{key}"))
        ));
        o.line("");
        for alias in s.kind_aliases_of_node(key) {
            o.line(format!(
                "/// Go: `func Is{alias}(node *Node) bool`\npub fn {}(node: &Node) -> bool {{\n    matches!(node.kind, Kind::{alias})\n}}",
                to_snake(&format!("Is{alias}"))
            ));
            o.line("");
        }
    }

    o.finish()
}

// ────────────────────────────────────────────────────────────────────────────
// factory_generated.rs (NodeFactory New*/Update* — mirror of generate-go-ast.ts's
// emitNewFactory/generateUpdateFactory, over the NodeStore arena seam)
// ────────────────────────────────────────────────────────────────────────────

/// A resolved factory member (mirror of generate-go-ast.ts's `schemaMembers`).
struct FactoryMember {
    r: ResolvedMember,
    /// Rust param name (Go's `goParamName` snake-cased via the field naming,
    /// so inherited members and struct fields stay in sync).
    param: String,
    kind_param: bool,
    /// Go `isNodeFlagsMember` — the member sets `Node.Flags`, not a data field.
    node_flags: bool,
    /// Go `isTextContentType` — the constructor bumps `f.textCount`.
    text_content: bool,
}

fn factory_members(s: &Schema, key: &str) -> Vec<FactoryMember> {
    s.node_def(key)
        .members
        .iter()
        .map(|m| {
            let r = s.resolve_member(key, m);
            FactoryMember {
                param: rust_field_name(&r.name),
                kind_param: r.kind_param,
                node_flags: matches!(&r.declared, Type::Primitive(p) if p == "NodeFlags"),
                text_content: matches!(&r.ty, Type::Primitive(p) if p == "string")
                    || matches!(
                        &r.ty,
                        Type::List { element, list_kind }
                            if list_kind == "raw"
                                && matches!(element.as_ref(), Type::Primitive(p) if p == "string")
                    ),
                r,
            }
        })
        // Go schema.ts: `noFactory = goOnly || field.noFactory || member.noFactory`
        // — the resolved flag, not the raw member's (CaseOrDefaultClause's
        // goOnly FallthroughFlowNode is excluded from New* params).
        .filter(|fm| !fm.r.no_factory)
        .collect()
}

/// The struct-storage optionality of a member (the base field's for inherited
/// members — the same formula the generated struct fields and VisitEachChild
/// use, so factory params always have the field's exact type).
fn storage_optional_of(r: &ResolvedMember) -> bool {
    if r.inherited {
        r.storage_optional
    } else {
        r.optional
    }
}

fn factory_param_type(s: &Schema, m: &FactoryMember) -> String {
    if m.kind_param {
        return "Kind".to_string();
    }
    let storage = storage_type(s, &m.r.ty, storage_optional_of(&m.r));
    match storage.as_str() {
        // Go string params become &str (the payload owns the copy, PORT).
        "Box<str>" => "&str".to_string(),
        "Option<Box<str>>" => "Option<&str>".to_string(),
        _ => storage,
    }
}

/// ast.json `bitmask` → the Rust flags const (e.g. "TokenFlagsStringLiteralFlags"
/// → "TokenFlags::STRING_LITERAL_FLAGS"; the same bit values as Go, per the
/// hand-written flags modules).
fn bitmask_const(bitmask: &str) -> String {
    if let Some(rest) = bitmask.strip_prefix("NodeFlags") {
        format!("NodeFlags::{}", to_snake(rest).to_uppercase())
    } else if let Some(rest) = bitmask.strip_prefix("TokenFlags") {
        format!("TokenFlags::{}", to_snake(rest).to_uppercase())
    } else {
        panic!("unmapped factory bitmask: {bitmask}")
    }
}

/// The zero value for a struct field no factory member covers (Go: the New*
/// constructors leave unassigned fields zero — binder/flow/symbol links,
/// facts, and the noFactory modifierFlags).
fn zero_value(key: &str, field: &str, ty: &str) -> String {
    match ty {
        t if t.starts_with("Option<") => "None".to_string(),
        "bool" => "false".to_string(),
        "i32" | "u32" => "0".to_string(),
        "Vec<NodeId>" | "Vec<Box<str>>" => "Vec::new()".to_string(),
        "SymbolTable" => "SymbolTable::default()".to_string(),
        "ModifierFlags" => "ModifierFlags::NONE".to_string(),
        "TokenFlags" => "TokenFlags::NONE".to_string(),
        "Box<str>" => "\"\".into()".to_string(),
        "std::sync::Arc<dyn std::any::Any + Send + Sync>" => "std::sync::Arc::new(())".to_string(),
        other => panic!("no zero value for uncovered factory field {key}.{field}: {other}"),
    }
}

/// The struct-literal value expression for a covered factory field.
fn field_value_expr(s: &Schema, m: &FactoryMember) -> String {
    let storage = storage_type(s, &m.r.ty, storage_optional_of(&m.r));
    match storage.as_str() {
        "Box<str>" => format!("{}.into()", m.param),
        "Option<Box<str>>" => format!("{}.map(Into::into)", m.param),
        _ => match &m.r.bitmask {
            Some(b) => format!("{} & {}", m.param, bitmask_const(b)),
            None => m.param.clone(),
        },
    }
}

/// The `param != node.Field` comparison for the generated Update* changed-check
/// (Go compares every update member; raw slices use core.Same → `!=` here).
fn update_compare_expr(s: &Schema, m: &FactoryMember) -> String {
    if m.node_flags {
        return format!("{} != n.flags", m.param);
    }
    let field = rust_field_name(&m.r.name);
    let storage = storage_type(s, &m.r.ty, storage_optional_of(&m.r));
    match storage.as_str() {
        // Go compares the `any` interface value; pointer identity is the only
        // sound Arc comparison (PORT — transformer-era revisit for deep-equal
        // dynamic types, which Go would panic on anyway if non-comparable).
        t if t.starts_with("std::sync::Arc<") => {
            format!("!std::sync::Arc::ptr_eq(&{}, &d.{field})", m.param)
        }
        "Box<str>" => format!("{} != &*d.{field}", m.param),
        "Option<Box<str>>" => format!("{} != d.{field}.as_deref()", m.param),
        _ => format!("{} != d.{field}", m.param),
    }
}

/// Mirror of generate-go-ast.ts's `emitNewFactory` — one New* method for
/// `ctor_name` (the node's primary name or a kind alias), with members in
/// ast.json order (Go's exact argument order).
fn emit_new_factory(o: &mut Out, s: &Schema, key: &str, ctor_name: &str, kind_expr: &str) {
    const I: &str = "    "; // impl-block indent
    let members = factory_members(s, key);
    let params = members
        .iter()
        .map(|m| format!("{}: {}", m.param, factory_param_type(s, m)))
        .collect::<Vec<_>>();
    o.line(format!(
        "{I}/// Go: `func (f *NodeFactory) {ctor_name}({}) *Node` (ast_generated.go).",
        members
            .iter()
            .map(|m| m.r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    o.line(format!(
        "{I}pub fn {}(&mut self{}) -> NodeId {{",
        to_snake(ctor_name),
        if params.is_empty() {
            String::new()
        } else {
            format!(", {}", params.join(", "))
        }
    ));
    if members.iter().any(|m| m.text_content) {
        o.line(format!("{I}    self.text_count += 1;"));
    }
    let flags_members: Vec<&FactoryMember> = members.iter().filter(|m| m.node_flags).collect();
    if flags_members.len() == 1 {
        // Single flags member (the only shape ast.json has): one binding,
        // mirroring Go's `node.Flags = param` / `|= param & bitmask` from a
        // zero Flags.
        let m = flags_members[0];
        let expr = match &m.r.bitmask {
            // Go: `node.Flags |= param & m.bitmask`.
            Some(b) => format!("{} & {}", m.param, bitmask_const(b)),
            // Go: `node.Flags = param`.
            None => m.param.clone(),
        };
        o.line(format!("{I}    let node_flags = {expr};"));
    } else if !flags_members.is_empty() {
        // Multiple flags members (no current node has these): Go's sequential
        // assignment/merge, starting from the zero Flags.
        o.line(format!("{I}    let mut node_flags = NodeFlags::NONE;"));
        for m in &flags_members {
            match &m.r.bitmask {
                Some(b) => o.line(format!(
                    "{I}    node_flags |= {} & {};",
                    m.param,
                    bitmask_const(b)
                )),
                None => o.line(format!("{I}    node_flags = {};", m.param)),
            }
        }
    }
    o.line(format!("{I}    let data = {key} {{"));
    for line in node_struct_lines(s, key) {
        if let FieldLine::Field { name, ty, .. } = line {
            let value = match members
                .iter()
                .find(|m| !m.kind_param && !m.node_flags && rust_field_name(&m.r.name) == name)
            {
                Some(m) => field_value_expr(s, m),
                None => zero_value(key, &name, &ty),
            };
            // Field-init shorthand when the value is the plain param
            // (`name: name` → `name`), mirroring idiomatic Rust.
            if value == name {
                o.line(format!("{I}        {name},"));
            } else {
                o.line(format!("{I}        {name}: {value},"));
            }
        }
    }
    o.line(format!("{I}    }};"));
    let flags_arg = if flags_members.is_empty() {
        "NodeFlags::NONE"
    } else {
        "node_flags"
    };
    o.line(format!(
        "{I}    self.new_node({kind_expr}, {flags_arg}, NodeData::{key}(Box::new(data)))"
    ));
    o.line(format!("{I}}}"));
    o.line("");
}

/// Mirror of generate-go-ast.ts's `generateUpdateFactory` — Update{Node} with
/// every non-Kind member, returning the original handle when nothing changed
/// (Go returns the same `*Node`), else a fresh node with Flags/Loc copied
/// from the original (Go `updateNode`). Returns whether a method was emitted.
fn emit_update_factory(o: &mut Out, s: &Schema, key: &str) -> bool {
    let members = factory_members(s, key);
    let update_members: Vec<&FactoryMember> = members.iter().filter(|m| !m.kind_param).collect();
    // Go: no Update when there are no non-Kind members, or none of them is a
    // child (leaf/token nodes never rebuild).
    if update_members.is_empty() || !update_members.iter().any(|m| m.r.is_child()) {
        return false;
    }
    let has_kind_member = members.iter().any(|m| m.kind_param);
    let aliases = s.kind_aliases_of_node(key);
    // `kind` is consumed by the rebuild (New's kind param, or the alias-kind
    // dispatch); name it `_kind` when unused to keep the build warning-free.
    let kind_used = has_kind_member || !aliases.is_empty();
    let ctor_name = format!("Update{key}");
    let mut params = vec!["node: NodeId".to_string()];
    for m in &update_members {
        params.push(format!("{}: {}", m.param, factory_param_type(s, m)));
    }
    const I: &str = "    "; // impl-block indent
    o.line(format!(
        "{I}/// Go: `func (f *NodeFactory) Update{key}(node *{key}, ...) *Node` (ast_generated.go) —",
    ));
    o.line(format!("{I}/// returns `node` unchanged when every member matches (Go returns the"));
    o.line(format!("{I}/// original `*Node`), else a fresh node with Flags/Loc copied from the"));
    o.line(format!("{I}/// original (Go `updateNode`)."));
    o.line(format!(
        "{I}pub fn {}(&mut self, {}) -> NodeId {{",
        to_snake(&ctor_name),
        params.join(", ")
    ));
    o.line(format!(
        "{I}    let (flags, loc, {}, changed) = {{",
        if kind_used { "kind" } else { "_kind" }
    ));
    o.line(format!("{I}        let n = self.store.node(node);"));
    o.line(format!(
        "{I}        let d = n.as_{}().unwrap_or_else(|| panic!(\"{ctor_name}: node {{node}} does not carry {key} data\"));",
        to_snake(key)
    ));
    o.line(format!("{I}        ("));
    o.line(format!("{I}            n.flags,"));
    o.line(format!("{I}            n.loc,"));
    o.line(format!("{I}            n.kind,"));
    let comparisons = update_members
        .iter()
        .map(|m| update_compare_expr(s, m))
        .collect::<Vec<_>>();
    o.line(format!("{I}            {},", comparisons.join("\n                || ")));
    o.line(format!("{I}        )"));
    o.line(format!("{I}    }};"));
    o.line(format!("{I}    if !changed {{"));
    o.line(format!("{I}        return node;"));
    o.line(format!("{I}    }}"));
    // Rebuild through New* with the Kind from the original node (Go: `node.Kind`).
    let new_args = members
        .iter()
        .map(|m| {
            if m.kind_param {
                "kind".to_string()
            } else {
                m.param.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if has_kind_member {
        o.line(format!(
            "{I}    let updated = self.{}({new_args});",
            to_snake(&format!("New{key}"))
        ));
    } else if !aliases.is_empty() {
        // Go: switch on node.Kind to call New{alias} per kind, panic otherwise.
        o.line(format!("{I}    let updated = match kind {{"));
        o.line(format!(
            "{I}        Kind::{} => self.{}({new_args}),",
            s.syntax_kind_name(key),
            to_snake(&format!("New{}", s.syntax_kind_name(key)))
        ));
        for alias in &aliases {
            o.line(format!(
                "{I}        Kind::{alias} => self.{}({new_args}),",
                to_snake(&format!("New{alias}"))
            ));
        }
        o.line(format!(
            "{I}        _ => panic!(\"unexpected kind in {ctor_name}: {{}}\", kind.name()),"
        ));
        o.line(format!("{I}    }};"));
    } else {
        o.line(format!(
            "{I}    let updated = self.{}({new_args});",
            to_snake(&format!("New{key}"))
        ));
    }
    o.line(format!("{I}    let u = self.store.node_mut(updated);"));
    o.line(format!("{I}    u.flags = flags;"));
    o.line(format!("{I}    u.loc = loc;"));
    o.line(format!("{I}    updated"));
    o.line(format!("{I}}}"));
    o.line("");
    true
}

fn generate_factory(s: &Schema) -> String {
    let mut o = Out::new();
    o.line("// Code generated by rust/tools/gen-ast from tools/scripts/tsc/ast.json. DO NOT EDIT.");
    o.line("// Rust mirror of tsc/internal/ast/ast_generated.go's NodeFactory surface — the");
    o.line("// per-kind New*() constructors (incl. the kind-alias constructors Go emits for");
    o.line("// nodes with multi-kind defs: NewJSTypeAliasDeclaration, NewJSImportDeclaration)");
    o.line("// and Update*() methods, in ast.json definition order (identical to Go's file");
    o.line("// order). The NodeFactory core (newNode/NewNodeList/NewModifierList/NewModifier/");
    o.line("// NewSourceFile/UpdateSourceFile + the token cache) is hand-written in");
    o.line("// factory.rs; Go ast.go's NewCommentRange lives in source_file.rs.");
    o.line("//");
    o.line("// Mapping (SPEC §5.1): Go `*Node` children → `NodeId` handles (mandatory");
    o.line("// slots; Go nil becomes `NodeId::NONE`), Go `*NodeList`/`*ModifierList` params →");
    o.line("// `Option<NodeList>`/`Option<ModifierList>`, Go string params → `&str` (the node");
    o.line("// payload owns the copy — Go slices share source text; PORT, see factory.rs),");
    o.line("// Go `[]string` (raw) → `Vec<Box<str>>`. `Kind`-typed members named Kind/kind are");
    o.line("// the constructor's `kind: Kind` argument (Go `isKindParam`); NodeFlags members");
    o.line("// set `Node.Flags` (Go `isNodeFlagsMember`, masked by ast.json `bitmask`); other");
    o.line("// bitmask members are masked into the data field exactly as in Go. Uncovered");
    o.line("// struct fields (binder/flow links, facts, noFactory fields) take zero values,");
    o.line("// matching Go's fresh-struct semantics. Nodes count into the factory's");
    o.line("// textCount when any member is string-typed (Go `hasTextContent`).");
    o.line("");
    o.line("#![allow(clippy::too_many_arguments)]");
    o.line("");
    o.line("use super::*;");
    o.line("");
    o.line("impl NodeFactory<'_> {");
    let mut new_inventory: Vec<String> = Vec::new();
    let mut update_inventory: Vec<String> = Vec::new();
    for (key, def) in s.nodes() {
        if def.hand_written {
            continue; // SourceFile: NewSourceFile/UpdateSourceFile are hand-written (factory.rs)
        }
        let members = factory_members(s, key);
        let kind_expr = if members.iter().any(|m| m.kind_param) {
            "kind".to_string()
        } else {
            format!("Kind::{}", s.syntax_kind_name(key))
        };
        let ctor = format!("New{key}");
        emit_new_factory(&mut o, s, key, &ctor, &kind_expr);
        new_inventory.push(ctor);
        for alias in s.kind_aliases_of_node(key) {
            // Go: emitNewFactory re-emits per kind alias with Kind{alias}.
            let alias_ctor = format!("New{alias}");
            let alias_kind_expr = if members.iter().any(|m| m.kind_param) {
                "kind".to_string()
            } else {
                format!("Kind::{alias}")
            };
            emit_new_factory(&mut o, s, key, &alias_ctor, &alias_kind_expr);
            new_inventory.push(alias_ctor);
        }
        if emit_update_factory(&mut o, s, key) {
            update_inventory.push(format!("Update{key}"));
        }
    }
    o.line("}");
    o.line("");

    // The mechanical parity anchor: the full constructor inventory in Go's
    // file order. The ast-crate tests assert this equals the hand-extracted
    // Go inventory (same pattern as the kind-ordinal mirror test).
    o.line("/// Every generated New* constructor, in Go ast_generated.go's file order");
    o.line("/// (= ast.json definition order). Parity anchor for the test suite.");
    o.line("pub const FACTORY_NEW_INVENTORY: &[&str] = &[");
    for name in &new_inventory {
        o.line(format!("    \"{name}\","));
    }
    o.line("];");
    o.line("");
    o.line("/// Every generated Update* method, in Go ast_generated.go's file order.");
    o.line("/// Parity anchor for the test suite.");
    o.line("pub const FACTORY_UPDATE_INVENTORY: &[&str] = &[");
    for name in &update_inventory {
        o.line(format!("    \"{name}\","));
    }
    o.line("];");
    o.line("");

    // Compile-time existence probe: every inventory entry is addressable as an
    // inherent method (a skipped emission would fail the build here, keeping
    // the inventory and the emitted surface in lockstep).
    o.line("#[cfg(test)]");
    o.line("mod surface_probe {");
    o.line("    use super::*;");
    o.line("");
    o.line("    /// Every generated factory method is addressable (fn-item references;");
    o.line("    /// the behavioral tests live in factory.rs).");
    o.line("    #[test]");
    o.line("    fn generated_factory_methods_are_addressable() {");
    for name in new_inventory.iter().chain(update_inventory.iter()) {
        o.line(format!("        let _ = NodeFactory::{};", to_snake(name)));
    }
    o.line("    }");
    o.line("}");
    o.finish()
}

// ────────────────────────────────────────────────────────────────────────────
// main
// ────────────────────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("..")
        .canonicalize()
        .expect("cannot locate repo root from CARGO_MANIFEST_DIR")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let root = repo_root();
    let ast_json_path = root.join("tools/scripts/tsc/ast.json");
    let out_dir = root.join("rust/crates/ast/src");

    let json_str = std::fs::read_to_string(&ast_json_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", ast_json_path.display()));
    let json: AstJson = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", ast_json_path.display()));
    let s = Schema::load(json);

    let kind_src = generate_kind(&s);
    let ast_src = generate_ast(&s);
    let factory_src = generate_factory(&s);

    let n_kinds = s.kind_elements.iter().filter(|(n, _)| n.is_some()).count();
    let n_structs = s.json.nodes.definitions.len()
        - s
            .json
            .nodes
            .definitions
            .values()
            .filter(|d| d.hand_written)
            .count();
    let n_as = s.json.nodes.definitions.len();
    eprintln!(
        "gen-ast: {} kinds (+ Count), {} node structs, {} NodeData variants, {} as_x accessors",
        n_kinds, n_structs, n_as, n_as
    );
    eprintln!(
        "gen-ast: kind_generated.rs {} lines, ast_generated.rs {} lines",
        kind_src.lines().count(),
        ast_src.lines().count()
    );
    let n_new = factory_src
        .lines()
        .filter(|l| l.starts_with("    \"New"))
        .count();
    let n_update = factory_src
        .lines()
        .filter(|l| l.starts_with("    \"Update"))
        .count();
    eprintln!(
        "gen-ast: factory_generated.rs {} lines, {} New* constructors, {} Update* methods (Go: 193 New + 166 Update)",
        factory_src.lines().count(),
        n_new,
        n_update,
    );

    let files = [
        ("kind_generated.rs", kind_src),
        ("ast_generated.rs", ast_src),
        ("factory_generated.rs", factory_src),
    ];
    let mut stale = 0;
    for (name, src) in &files {
        let path = out_dir.join(name);
        let existing = std::fs::read_to_string(&path).ok();
        match (&existing, check) {
            (Some(cur), true) => {
                if cur == src {
                    println!("{name}: OK (current)");
                } else {
                    let diff_line = cur
                        .lines()
                        .zip(src.lines())
                        .position(|(a, b)| a != b)
                        .map(|i| i + 1)
                        .unwrap_or_else(|| cur.lines().count().max(src.lines().count()) + 1);
                    println!(
                        "{name}: STALE — first difference at line {diff_line} (checked-in {} lines, expected {} lines)",
                        cur.lines().count(),
                        src.lines().count()
                    );
                    stale += 1;
                }
            }
            (None, true) => {
                println!("{name}: MISSING (expected at {})", path.display());
                stale += 1;
            }
            (_, false) => {
                std::fs::write(&path, src)
                    .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
                println!("wrote {} ({} lines)", path.display(), src.lines().count());
            }
        }
    }
    if check && stale > 0 {
        println!("{stale} generated file(s) stale; re-run `cargo run -p gen-ast` and commit");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
