//! Deterministic, layer-aware cross-file dead-code analysis (`LIT510`, `LIT511`, `LIT512`, `LIT514`).
use crate::{
    model::{
        DeadCodeSummary, DeadDef, DeadDefKind, Finding, Source, def_flags, finding_at,
        relative_name,
    },
    workspace::Workspace,
};
use std::collections::{HashMap, HashSet, VecDeque};

const SERVER_ENTRY_BASENAMES: &[&str] = &[
    "__main__.py",
    "app.py",
    "asgi.py",
    "cli.py",
    "main.py",
    "scheduler.py",
    "server.py",
    "worker.py",
    "wsgi.py",
];

const PYTEST_HOOK_PREFIX: &str = "pytest_";
const PYTEST_MAGIC_GLOBALS: &[&str] = &[
    "pytestmark",
    "pytest_plugins",
    "collect_ignore",
    "collect_ignore_glob",
];
const PYTEST_LIFECYCLE_METHODS: &[&str] = &[
    "setup_class",
    "teardown_class",
    "setup_method",
    "teardown_method",
    "setUp",
    "tearDown",
    "setUpClass",
    "tearDownClass",
    "asyncSetUp",
    "asyncTearDown",
];
const PYTEST_LIFECYCLE_FUNCS: &[&str] = &[
    "setup_module",
    "teardown_module",
    "setup_function",
    "teardown_function",
];

const NON_THIRD_PARTY_HOOK_BASES: &[&str] = &[
    "builtins.object",
    "builtins.dict",
    "builtins.list",
    "builtins.set",
    "builtins.tuple",
    "builtins.str",
    "builtins.int",
    "builtins.float",
    "builtins.bytes",
    "builtins.Exception",
    "builtins.BaseException",
    "typing.Generic",
    "typing.Protocol",
    "typing.NamedTuple",
    "typing.TypedDict",
    "typing_extensions.Generic",
    "typing_extensions.Protocol",
    "typing_extensions.NamedTuple",
    "typing_extensions.TypedDict",
    "enum.Enum",
    "enum.Flag",
    "enum.IntEnum",
    "enum.IntFlag",
    "enum.StrEnum",
    "pydantic.BaseModel",
    "pydantic.RootModel",
    "pydantic.main.BaseModel",
    "pydantic.root_model.RootModel",
    "pydantic_settings.BaseSettings",
    "pydantic_settings.main.BaseSettings",
    "sqlalchemy.ext.declarative.declarative_base",
    "sqlalchemy.orm.DeclarativeBase",
    "sqlalchemy.orm.DeclarativeBaseNoMeta",
    "sqlalchemy.orm.MappedAsDataclass",
    "sqlalchemy.orm.declarative_base",
];

#[derive(Clone, Copy, Debug)]
struct NodeMeta {
    source_idx: usize,
    /// `None` for the synthetic `MODULE_INIT` node, `Some(idx)` into `summary.defs`.
    def_idx: Option<usize>,
}

type InitFieldMap = HashMap<String, (Vec<Box<str>>, Vec<Box<str>>)>;

#[derive(Default)]
struct ClassInfo {
    source_idx: usize,
    workspace_bases: Vec<usize>,
    is_protocol: bool,
    is_pydantic: bool,
    is_orm: bool,
    is_dataclass: bool,
    is_named_tuple: bool,
    is_enum: bool,
    is_typed_dict: bool,
    is_exception: bool,
    has_third_party_base: bool,
    /// Direct member name -> member `NodeId`s (Method, Property, Field).
    members: HashMap<String, Vec<usize>>,
    /// Instance attributes initialized in `__init__`: attr -> `(direct_types, elem_types)`.
    init_fields: InitFieldMap,
    /// Direct non-dunder method & property names (used for Protocol structural conformance).
    method_names: HashSet<String>,
    /// All non-dunder method & property names including workspace bases.
    all_method_names: HashSet<String>,
    /// Required members if this class is a Protocol (transitively including base Protocols).
    protocol_members: HashSet<String>,
    /// All member names (methods + properties + fields) including workspace bases.
    all_member_names: HashSet<String>,
}

fn is_dunder(name: &str) -> bool {
    name.len() >= 4 && name.starts_with("__") && name.ends_with("__")
}

fn is_test_source(s: &Source) -> bool {
    s.layer() == "test"
}

fn is_pytest_test_file(s: &Source) -> bool {
    if !is_test_source(s) {
        return false;
    }
    let Some(name) = s.path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name == "conftest.py"
        || name == "__main__.py"
        || name.starts_with("test_")
        || name.ends_with("_test.py")
}

fn is_migration_file(s: &Source) -> bool {
    let p = s.filename.as_str();
    p.starts_with("migrations/") || p.contains("/migrations/")
}

fn is_script_or_example(s: &Source) -> bool {
    let p = s.filename.as_str();
    p.starts_with("scripts/")
        || p.contains("/scripts/")
        || p.starts_with("examples/")
        || p.contains("/examples/")
}

fn is_executable_boundary(s: &Source, summary: &DeadCodeSummary) -> bool {
    if s.layer() != "boundary" {
        return false;
    }
    if summary.has_main_guard || is_script_or_example(s) {
        return true;
    }
    s.path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| SERVER_ENTRY_BASENAMES.contains(&n))
}

fn is_library_root_init(s: &Source, w: &Workspace) -> bool {
    if !s.is_marker_file {
        return false;
    }
    let kind = w.packages[s.owner].kind.as_str();
    if !matches!(kind, "library" | "client") {
        return false;
    }
    s.module
        .as_deref()
        .is_some_and(|m| !m.is_empty() && !m.contains('.'))
}

fn is_library_boundary_module(s: &Source, w: &Workspace) -> bool {
    let pkg = &w.packages[s.owner];
    matches!(pkg.kind.as_str(), "library" | "client")
        && pkg.layout == "boundary"
        && !is_test_source(s)
}

fn summary_of(s: &Source) -> Option<&DeadCodeSummary> {
    s.dead_code.as_deref()
}

/// Build unified module lookup covering both `src/` modules and package-root-relative
/// modules (`tests.helpers.spend`, `scripts.ability_evals.fixtures`, etc.).
fn build_module_lookup(w: &Workspace) -> (HashMap<String, usize>, Vec<Option<String>>) {
    let mut map: HashMap<String, usize> = HashMap::with_capacity(w.sources.len());
    let mut names: Vec<Option<String>> = Vec::with_capacity(w.sources.len());
    for (idx, src) in w.sources.iter().enumerate() {
        if let Some(m) = &src.module {
            map.insert(m.clone(), idx);
            names.push(Some(m.clone()));
            continue;
        }
        let pkg_root = &w.packages[src.owner].root;
        let derived = src.path.strip_prefix(pkg_root).ok().map(|p| {
            let mut s = p
                .with_extension("")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, ".");
            if s == "__init__" {
                s.clear();
            } else if s.ends_with(".__init__") {
                s.truncate(s.len() - 9);
            }
            s
        });
        if let Some(m) = derived.filter(|m| !m.is_empty()) {
            map.entry(m.clone()).or_insert(idx);
            // Also index without leading `scripts.` or `tests.` if tests add `tests/` to sys.path.
            if let Some(rest) = m.strip_prefix("tests.") {
                map.entry(rest.to_owned()).or_insert(idx);
            }
            if let Some(rest) = m.strip_prefix("scripts.") {
                map.entry(rest.to_owned()).or_insert(idx);
            }
            names.push(Some(m));
        } else {
            names.push(None);
        }
    }
    (map, names)
}

pub fn check(w: &Workspace) -> (Vec<Finding>, Vec<Finding>) {
    let _span = crate::profile::span("dead_code_cross_file");
    if !w.dead_code {
        return (Vec::new(), Vec::new());
    }

    let manifest_name = w
        .manifest
        .as_ref()
        .map_or("leesh.toml".to_owned(), |p| relative_name(p, &w.root));
    let mut manifest_problems: Vec<Finding> = Vec::new();
    let (mod_lookup, source_mod_names) = build_module_lookup(w);

    // 1. Allocate NodeIds for every MODULE_INIT and every DeadDef across all sources.
    let mut nodes: Vec<NodeMeta> = Vec::new();
    let mut mod_init_node: Vec<usize> = vec![usize::MAX; w.sources.len()];
    let mut local_to_node: Vec<HashMap<u32, usize>> = Vec::with_capacity(w.sources.len());
    let mut top_defs: HashMap<(usize, String), Vec<usize>> = HashMap::new();
    let mut class_nodes: Vec<usize> = Vec::new();

    for (src_idx, src) in w.sources.iter().enumerate() {
        let mut lmap = HashMap::new();
        let init_id = nodes.len();
        nodes.push(NodeMeta {
            source_idx: src_idx,
            def_idx: None,
        });
        mod_init_node[src_idx] = init_id;
        lmap.insert(0, init_id);

        if let Some(sum) = summary_of(src) {
            for (d_idx, def) in sum.defs.iter().enumerate() {
                let nid = nodes.len();
                nodes.push(NodeMeta {
                    source_idx: src_idx,
                    def_idx: Some(d_idx),
                });
                lmap.insert(def.local_id, nid);
                if def.parent_id == 0 {
                    top_defs
                        .entry((src_idx, def.name.to_string()))
                        .or_default()
                        .push(nid);
                }
                if def.kind == DeadDefKind::Class {
                    class_nodes.push(nid);
                }
            }
        }
        local_to_node.push(lmap);
    }

    let get_def = |nid: usize| -> Option<&DeadDef> {
        let meta = nodes[nid];
        let d_idx = meta.def_idx?;
        summary_of(&w.sources[meta.source_idx]).map(|s| &s.defs[d_idx])
    };

    let resolve_symbol = |from_src: usize, target: &str| -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        resolve_symbol_rec(
            w,
            &mod_lookup,
            &mod_init_node,
            &top_defs,
            from_src,
            target,
            &mut seen,
            &mut out,
        );
        out
    };

    // 2. Build ClassInfo table, members, init_fields, workspace inheritance, and transitive class roles.
    let mut classes: HashMap<usize, ClassInfo> = HashMap::with_capacity(class_nodes.len());
    for &cid in &class_nodes {
        let meta = nodes[cid];
        let def = get_def(cid).unwrap();
        let mut info = ClassInfo {
            source_idx: meta.source_idx,
            is_protocol: (def.flags & def_flags::IS_PROTOCOL) != 0,
            is_pydantic: (def.flags & def_flags::IS_PYDANTIC) != 0,
            is_orm: (def.flags & def_flags::IS_ORM) != 0,
            is_dataclass: (def.flags & def_flags::IS_DATACLASS) != 0,
            is_named_tuple: (def.flags & def_flags::IS_NAMED_TUPLE) != 0,
            is_enum: (def.flags & def_flags::IS_ENUM) != 0,
            is_typed_dict: (def.flags & def_flags::IS_TYPED_DICT) != 0,
            is_exception: (def.flags & def_flags::IS_EXCEPTION) != 0,
            ..Default::default()
        };
        for base in &def.bases {
            let resolved = resolve_symbol(meta.source_idx, base);
            let mut matched_ws_class = false;
            for tid in resolved {
                if get_def(tid).is_some_and(|d| d.kind == DeadDefKind::Class) && tid != cid {
                    info.workspace_bases.push(tid);
                    matched_ws_class = true;
                }
            }
            if !matched_ws_class
                && !base.starts_with('@')
                && !NON_THIRD_PARTY_HOOK_BASES.contains(&&**base)
                && !base.starts_with("builtins.")
                && !base.ends_with("Error")
                && !base.ends_with("Exception")
            {
                info.has_third_party_base = true;
            }
        }
        classes.insert(cid, info);
    }

    // Populate direct class members and `__init__` instance attribute types.
    for (src_idx, src) in w.sources.iter().enumerate() {
        let Some(sum) = summary_of(src) else {
            continue;
        };
        for ifield in &sum.init_fields {
            if let Some(&cls_nid) = local_to_node[src_idx].get(&ifield.class_local_id)
                && let Some(cinfo) = classes.get_mut(&cls_nid)
            {
                let entry = cinfo
                    .init_fields
                    .entry(ifield.attr.to_string())
                    .or_default();
                entry.0.extend(ifield.direct_types.iter().cloned());
                entry.1.extend(ifield.elem_types.iter().cloned());
                cinfo.all_member_names.insert(ifield.attr.to_string());
            }
        }
    }
    for (nid, meta) in nodes.iter().enumerate() {
        let Some(d_idx) = meta.def_idx else {
            continue;
        };
        let def = &summary_of(&w.sources[meta.source_idx]).unwrap().defs[d_idx];
        if matches!(
            def.kind,
            DeadDefKind::Method | DeadDefKind::Property | DeadDefKind::Field
        ) && let Some(&parent_nid) = local_to_node[meta.source_idx].get(&def.parent_id)
            && let Some(cinfo) = classes.get_mut(&parent_nid)
        {
            cinfo
                .members
                .entry(def.name.to_string())
                .or_default()
                .push(nid);
            cinfo.all_member_names.insert(def.name.to_string());
            if matches!(def.kind, DeadDefKind::Method | DeadDefKind::Property)
                && !is_dunder(&def.name)
            {
                cinfo.method_names.insert(def.name.to_string());
                cinfo.all_method_names.insert(def.name.to_string());
                if cinfo.is_protocol {
                    cinfo.protocol_members.insert(def.name.to_string());
                }
            }
        }
    }

    // Transitive propagation of class roles, member sets, and Protocol members.
    let mut changed = true;
    while changed {
        changed = false;
        for &cid in &class_nodes {
            let bases = classes[&cid].workspace_bases.clone();
            for bid in bases {
                let Some(base_info) = classes.get(&bid) else {
                    continue;
                };
                let b_pyd = base_info.is_pydantic;
                let b_orm = base_info.is_orm;
                let b_proto = base_info.is_protocol;
                let b_enum = base_info.is_enum;
                let b_exc = base_info.is_exception;
                let b_3p = base_info.has_third_party_base;
                let b_methods: Vec<String> = base_info.all_method_names.iter().cloned().collect();
                let b_members: Vec<String> = base_info.all_member_names.iter().cloned().collect();
                let b_pmems: Vec<String> = base_info.protocol_members.iter().cloned().collect();

                let cinfo = classes.get_mut(&cid).unwrap();
                if b_pyd && !cinfo.is_pydantic {
                    cinfo.is_pydantic = true;
                    changed = true;
                }
                if b_orm && !cinfo.is_orm {
                    cinfo.is_orm = true;
                    changed = true;
                }
                if b_proto && !cinfo.is_protocol {
                    cinfo.is_protocol = true;
                    changed = true;
                }
                if b_enum && !cinfo.is_enum {
                    cinfo.is_enum = true;
                    changed = true;
                }
                if b_exc && !cinfo.is_exception {
                    cinfo.is_exception = true;
                    changed = true;
                }
                if b_3p && !cinfo.has_third_party_base {
                    cinfo.has_third_party_base = true;
                    changed = true;
                }
                for m in b_methods {
                    if cinfo.all_method_names.insert(m) {
                        changed = true;
                    }
                }
                for m in b_members {
                    if cinfo.all_member_names.insert(m) {
                        changed = true;
                    }
                }
                if cinfo.is_protocol {
                    for m in b_pmems {
                        if cinfo.protocol_members.insert(m) {
                            changed = true;
                        }
                    }
                    let own: Vec<String> = cinfo.method_names.iter().cloned().collect();
                    for m in own {
                        if cinfo.protocol_members.insert(m) {
                            changed = true;
                        }
                    }
                }
            }
        }
    }

    let mut subclasses_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&cid, info) in &classes {
        for &bid in &info.workspace_bases {
            subclasses_of.entry(bid).or_default().push(cid);
        }
    }

    let resolve_type_to_classes = |from_src: usize, raw: &str| -> Vec<usize> {
        let mut out = Vec::new();
        let mut visited: HashSet<(usize, String)> = HashSet::new();
        resolve_type_rec(
            w,
            &mod_lookup,
            &nodes,
            &mod_init_node,
            &top_defs,
            &classes,
            from_src,
            raw,
            0,
            &mut visited,
            &mut out,
        );
        out
    };

    // 3. Build directed edges & collect callback escapes, serialization escapes, and Protocol wiring.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut value_escaped_defs: HashSet<usize> = HashSet::new();
    let mut escaped_classes: HashSet<usize> = HashSet::new();
    let mut wired_protocols: HashMap<usize, HashSet<usize>> = HashMap::new();
    let mut owner_unknown_attrs: HashMap<usize, Vec<String>> = HashMap::new();

    for (src_idx, src) in w.sources.iter().enumerate() {
        let Some(sum) = summary_of(src) else {
            continue;
        };
        let init_nid = mod_init_node[src_idx];

        // Module imports execute imported modules (and ancestor __init__.py markers).
        for stmt in &src.all_imports {
            if !stmt.is_from {
                for name in &stmt.names {
                    link_module_and_ancestors(
                        w,
                        &mod_lookup,
                        &mod_init_node,
                        init_nid,
                        name,
                        &mut adj,
                    );
                }
            } else if let Some(base) = stmt.resolved_module.as_deref() {
                link_module_and_ancestors(w, &mod_lookup, &mod_init_node, init_nid, base, &mut adj);
                for sym in &stmt.names {
                    if &**sym != "*" {
                        let qualified = format!("{base}.{sym}");
                        if mod_lookup.contains_key(&qualified) {
                            link_module_and_ancestors(
                                w,
                                &mod_lookup,
                                &mod_init_node,
                                init_nid,
                                &qualified,
                                &mut adj,
                            );
                        }
                    }
                }
            } else if let Some(raw_mod) = stmt.raw_module.as_deref() {
                // Non-src relative/local import fallback (e.g., inside tests/ or scripts/).
                link_module_and_ancestors(
                    w,
                    &mod_lookup,
                    &mod_init_node,
                    init_nid,
                    raw_mod,
                    &mut adj,
                );
                for sym in &stmt.names {
                    if &**sym != "*" {
                        let qualified = format!("{raw_mod}.{sym}");
                        if mod_lookup.contains_key(&qualified) {
                            link_module_and_ancestors(
                                w,
                                &mod_lookup,
                                &mod_init_node,
                                init_nid,
                                &qualified,
                                &mut adj,
                            );
                        }
                    }
                }
            }
        }

        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for def in &sum.defs {
            if def.parent_id == 0 {
                let nid = local_to_node[src_idx][&def.local_id];
                by_name.entry(&def.name).or_default().push(nid);

                if (def.flags & def_flags::HAS_REG_DECORATOR) != 0 {
                    adj[init_nid].push(nid);
                }
                if def.kind == DeadDefKind::Reexport
                    && let Some(target) = def.reexport_target.as_deref()
                {
                    for tid in resolve_symbol(src_idx, target) {
                        adj[nid].push(tid);
                        link_target_module(
                            w,
                            &mod_lookup,
                            &source_mod_names,
                            &nodes,
                            &mod_init_node,
                            nid,
                            tid,
                            &mut adj,
                        );
                    }
                }
            } else {
                let nid = local_to_node[src_idx][&def.local_id];
                let parent_nid = local_to_node[src_idx][&def.parent_id];
                if let Some(cinfo) = classes.get(&parent_nid) {
                    let keep_with_class = is_dunder(&def.name)
                        || (def.flags & def_flags::HAS_REG_DECORATOR) != 0
                        || (def.flags & def_flags::IS_PYDANTIC_HOOK) != 0
                        || (def.flags & def_flags::IS_ORM_HOOK) != 0
                        || (cinfo.is_pydantic && def.kind == DeadDefKind::Field)
                        || (cinfo.is_orm && def.kind == DeadDefKind::Field)
                        || (cinfo.is_typed_dict && def.kind == DeadDefKind::Field)
                        || (cinfo.is_enum && def.kind == DeadDefKind::Field)
                        || (cinfo.has_third_party_base
                            && matches!(def.kind, DeadDefKind::Method | DeadDefKind::Property)
                            && !def.name.starts_with('_'));
                    if keep_with_class {
                        adj[parent_nid].push(nid);
                    }
                }
            }
        }
        for group in by_name.values() {
            if group.len() > 1 {
                for &a in group {
                    for &b in group {
                        if a != b {
                            adj[a].push(b);
                        }
                    }
                }
            }
        }

        for (&cid, cinfo) in &classes {
            if cinfo.source_idx == src_idx {
                for &bid in &cinfo.workspace_bases {
                    adj[cid].push(bid);
                    link_target_module(
                        w,
                        &mod_lookup,
                        &source_mod_names,
                        &nodes,
                        &mod_init_node,
                        cid,
                        bid,
                        &mut adj,
                    );
                }
            }
        }

        for r in &sum.refs {
            let owner_nid = local_to_node[src_idx][&r.owner_id];
            let targets = resolve_symbol(src_idx, &r.target);
            for tid in targets {
                if tid != owner_nid {
                    adj[owner_nid].push(tid);
                    link_target_module(
                        w,
                        &mod_lookup,
                        &source_mod_names,
                        &nodes,
                        &mod_init_node,
                        owner_nid,
                        tid,
                        &mut adj,
                    );
                }
                if r.as_value {
                    value_escaped_defs.insert(tid);
                }
            }
            if !r.target.starts_with('@')
                && let Some((cls_path, member)) = r.target.rsplit_once('.')
            {
                for cid in resolve_symbol(src_idx, cls_path) {
                    if classes.contains_key(&cid) {
                        adj[owner_nid].push(cid);
                        link_class_member(&classes, owner_nid, cid, member, &mut adj);
                        if r.as_value {
                            for mid in lookup_class_member(&classes, cid, member) {
                                value_escaped_defs.insert(mid);
                            }
                        }
                    }
                }
            }
        }

        for ar in &sum.attr_reads {
            let owner_nid = local_to_node[src_idx][&ar.owner_id];
            if ar.receiver_types.is_empty() {
                owner_unknown_attrs
                    .entry(owner_nid)
                    .or_default()
                    .push(ar.attr.to_string());
                continue;
            }
            let mut matched_any_class = false;
            for raw_ty in &ar.receiver_types {
                let cids = resolve_type_to_classes(src_idx, raw_ty);
                for cid in cids {
                    matched_any_class = true;
                    if ar.is_super {
                        if let Some(cinfo) = classes.get(&cid) {
                            for &bid in &cinfo.workspace_bases {
                                link_class_member(&classes, owner_nid, bid, &ar.attr, &mut adj);
                            }
                        }
                    } else {
                        link_class_member(&classes, owner_nid, cid, &ar.attr, &mut adj);
                    }
                }
            }
            if !matched_any_class {
                owner_unknown_attrs
                    .entry(owner_nid)
                    .or_default()
                    .push(ar.attr.to_string());
            }
        }

        for raw_ty in &sum.escaped_types {
            for cid in resolve_type_to_classes(src_idx, raw_ty) {
                escaped_classes.insert(cid);
            }
        }

        for tf in &sum.type_flows {
            let from_cids = resolve_type_to_classes(src_idx, &tf.from_type);
            if from_cids.is_empty() {
                continue;
            }
            let to_cids: Vec<usize> = if let Some(rest) = tf.to_type.strip_prefix("param:") {
                if let Some((callee_raw, pos_str)) = rest.rsplit_once('#')
                    && let Ok(pos) = pos_str.parse::<usize>()
                {
                    resolve_callee_param_classes(
                        w,
                        &mod_lookup,
                        &nodes,
                        &mod_init_node,
                        &top_defs,
                        &classes,
                        src_idx,
                        callee_raw,
                        Some(pos),
                        None,
                    )
                } else {
                    Vec::new()
                }
            } else if let Some(rest) = tf.to_type.strip_prefix("kwparam:") {
                if let Some((callee_raw, kw_name)) = rest.rsplit_once('.') {
                    resolve_callee_param_classes(
                        w,
                        &mod_lookup,
                        &nodes,
                        &mod_init_node,
                        &top_defs,
                        &classes,
                        src_idx,
                        callee_raw,
                        None,
                        Some(kw_name),
                    )
                } else {
                    Vec::new()
                }
            } else {
                resolve_type_to_classes(src_idx, &tf.to_type)
            };
            for &tcid in &to_cids {
                if classes.get(&tcid).is_some_and(|c| c.is_protocol) {
                    for &fcid in &from_cids {
                        if fcid != tcid && classes.get(&fcid).is_some_and(|c| !c.is_protocol) {
                            wired_protocols.entry(tcid).or_default().insert(fcid);
                        }
                    }
                }
            }
        }
    }

    // Transitive serialization escape: any dataclass nested inside a Pydantic model or
    // an already-escaped dataclass (returned from boundary/router/workflow or serialized)
    // also escapes to serialization!
    {
        let mut escape_queue: VecDeque<usize> = VecDeque::new();
        for (&cid, cinfo) in &classes {
            if cinfo.is_pydantic || escaped_classes.contains(&cid) {
                escape_queue.push_back(cid);
            }
        }
        let mut expanded: HashSet<usize> = HashSet::new();
        while let Some(cid) = escape_queue.pop_front() {
            if !expanded.insert(cid) {
                continue;
            }
            let Some(cinfo) = classes.get(&cid) else {
                continue;
            };
            let c_src = cinfo.source_idx;
            for mids in cinfo.members.values() {
                for &mid in mids {
                    let Some(def) = get_def(mid) else {
                        continue;
                    };
                    if def.kind != DeadDefKind::Field {
                        continue;
                    }
                    for raw_t in def.value_types.iter().chain(&def.elem_types) {
                        for nested_cid in resolve_type_to_classes(c_src, raw_t) {
                            if escaped_classes.insert(nested_cid) {
                                escape_queue.push_back(nested_cid);
                            }
                        }
                    }
                }
            }
        }
    }

    // 4. Compute Production Roots and Test Roots (plus validate `[dead_code]` manifest specs).
    let mut prod_roots: Vec<usize> = Vec::new();
    let mut test_roots: Vec<usize> = Vec::new();
    let mut keep_nodes: HashSet<usize> = HashSet::new();

    // Collect pytest plugin module indices from `pytest.ini` / `pyproject.toml` and `conftest.py`.
    let mut pytest_plugin_srcs: HashSet<usize> = HashSet::new();
    for plugin in &w.pytest_plugins {
        if let Some(&idx) = mod_lookup.get(plugin) {
            pytest_plugin_srcs.insert(idx);
        }
    }
    for src in &w.sources {
        if let Some(sum) = summary_of(src) {
            for plugin in &sum.pytest_plugins {
                if let Some(&idx) = mod_lookup.get(&**plugin) {
                    pytest_plugin_srcs.insert(idx);
                }
            }
        }
    }

    for (src_idx, src) in w.sources.iter().enumerate() {
        let Some(sum) = summary_of(src) else {
            continue;
        };
        let init_nid = mod_init_node[src_idx];

        if is_test_source(src) {
            let is_plugin = pytest_plugin_srcs.contains(&src_idx);
            if is_pytest_test_file(src) || is_plugin || sum.has_main_guard {
                test_roots.push(init_nid);
                for def in &sum.defs {
                    let nid = local_to_node[src_idx][&def.local_id];
                    let is_root = match def.kind {
                        DeadDefKind::Function if def.parent_id == 0 => {
                            def.name.starts_with("test_")
                                || PYTEST_LIFECYCLE_FUNCS.contains(&&*def.name)
                                || (def.flags & def_flags::IS_PYTEST_FIXTURE) != 0
                                || ((sum.is_conftest || is_plugin)
                                    && def.name.starts_with(PYTEST_HOOK_PREFIX))
                        }
                        DeadDefKind::Class if def.parent_id == 0 => def.name.starts_with("Test"),
                        DeadDefKind::Method => {
                            let parent_nid = local_to_node[src_idx][&def.parent_id];
                            let parent_is_test_cls =
                                get_def(parent_nid).is_some_and(|d| d.name.starts_with("Test"));
                            parent_is_test_cls
                                && (def.name.starts_with("test_")
                                    || PYTEST_LIFECYCLE_METHODS.contains(&&*def.name))
                        }
                        _ => false,
                    };
                    if is_root {
                        test_roots.push(nid);
                    }
                }
            }
            continue;
        }

        if is_migration_file(src) {
            prod_roots.push(init_nid);
            for def in &sum.defs {
                if def.parent_id == 0 {
                    prod_roots.push(local_to_node[src_idx][&def.local_id]);
                }
            }
        } else if is_library_root_init(src, w) || is_library_boundary_module(src, w) {
            prod_roots.push(init_nid);
            if let Some(exported) = &sum.static_all {
                for name in exported {
                    for &nid in top_defs
                        .get(&(src_idx, name.to_string()))
                        .into_iter()
                        .flatten()
                    {
                        prod_roots.push(nid);
                    }
                }
            } else {
                for def in &sum.defs {
                    if def.parent_id == 0 && !def.name.starts_with('_') {
                        prod_roots.push(local_to_node[src_idx][&def.local_id]);
                    }
                }
            }
        } else if is_executable_boundary(src, sum) {
            prod_roots.push(init_nid);
            if is_script_or_example(src) {
                for def in &sum.defs {
                    if def.parent_id == 0 && &*def.name == "main" {
                        prod_roots.push(local_to_node[src_idx][&def.local_id]);
                    }
                }
            }
        } else if src
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "__main__.py")
        {
            prod_roots.push(init_nid);
        }
    }

    for ep in &w.pyproject_entrypoints {
        prod_roots.extend(resolve_entrypoint_spec(
            w,
            &mod_lookup,
            &mod_init_node,
            &top_defs,
            &classes,
            ep,
        ));
    }

    for ep in &w.dead_code_entrypoints {
        let matched =
            resolve_entrypoint_spec(w, &mod_lookup, &mod_init_node, &top_defs, &classes, ep);
        if matched.is_empty() {
            manifest_problems.push(finding_at(
                &manifest_name,
                1,
                0,
                "LIT003",
                format!("dead_code.entrypoints matches no workspace symbol or module: {ep}"),
                None,
            ));
        } else {
            prod_roots.extend(matched);
        }
    }

    for k in &w.dead_code_keep {
        for sym in &k.symbols {
            let matched =
                resolve_entrypoint_spec(w, &mod_lookup, &mod_init_node, &top_defs, &classes, sym);
            if matched.is_empty() {
                manifest_problems.push(finding_at(
                    &manifest_name,
                    1,
                    0,
                    "LIT003",
                    format!("dead_code.keep matches no workspace symbol or module: {sym}"),
                    None,
                ));
            } else {
                for nid in matched {
                    keep_nodes.insert(nid);
                    prod_roots.push(nid);
                }
            }
        }
    }

    // 5. Run Two-Pass Reachability with dynamic Protocol & Subclass dispatch fixed-point.
    let (prod_live, prod_proto_constrained) = run_reachability(
        w,
        &nodes,
        &mod_init_node,
        &adj,
        &classes,
        &subclasses_of,
        &wired_protocols,
        &owner_unknown_attrs,
        &prod_roots,
        true,
    );

    let mut all_roots = prod_roots.clone();
    all_roots.extend(&test_roots);
    let (all_live, all_proto_constrained) = run_reachability(
        w,
        &nodes,
        &mod_init_node,
        &adj,
        &classes,
        &subclasses_of,
        &wired_protocols,
        &owner_unknown_attrs,
        &all_roots,
        false,
    );

    let mut unknown_attr_reads: HashSet<String> = HashSet::new();
    for (&owner, attrs) in &owner_unknown_attrs {
        if all_live[owner] {
            unknown_attr_reads.extend(attrs.iter().cloned());
        }
    }

    // 6. Emit deterministic findings (`LIT510`, `LIT511`, `LIT512`, `LIT514`).
    let mut findings: Vec<Finding> = Vec::new();

    for (src_idx, src) in w.sources.iter().enumerate() {
        let Some(sum) = summary_of(src) else {
            continue;
        };
        if src.layer().is_empty() {
            continue;
        }
        let init_nid = mod_init_node[src_idx];
        let is_test = is_test_source(src);

        if !src.is_marker_file {
            if !is_test {
                if !prod_live[init_nid] {
                    let mod_label = source_mod_names[src_idx]
                        .as_deref()
                        .unwrap_or(&src.filename);
                    let msg = if all_live[init_nid] {
                        format!(
                            "module '{mod_label}' is unreachable from production entrypoints (only imported by tests)"
                        )
                    } else {
                        format!("module '{mod_label}' is unreachable from production entrypoints")
                    };
                    findings.push(finding_at(
                        &src.filename,
                        1,
                        0,
                        "LIT510",
                        msg,
                        Some(mod_label.to_owned()),
                    ));
                    continue;
                }
            } else if !all_live[init_nid] {
                let mod_label = source_mod_names[src_idx]
                    .as_deref()
                    .unwrap_or(&src.filename);
                findings.push(finding_at(
                    &src.filename,
                    1,
                    0,
                    "LIT510",
                    format!("test helper module '{mod_label}' is unreachable from any test"),
                    Some(mod_label.to_owned()),
                ));
                continue;
            }
        }

        let read_params_set: HashSet<u32> = sum.read_params.iter().copied().collect();
        let live_slice = if is_test { &all_live } else { &prod_live };
        let proto_constrained = if is_test {
            &all_proto_constrained
        } else {
            &prod_proto_constrained
        };

        for def in &sum.defs {
            let nid = local_to_node[src_idx][&def.local_id];
            if keep_nodes.contains(&nid) {
                continue;
            }
            match def.kind {
                DeadDefKind::Function
                | DeadDefKind::Class
                | DeadDefKind::Variable
                | DeadDefKind::TypeAlias
                | DeadDefKind::Reexport => {
                    if is_dunder(&def.name) || &*def.name == "_" {
                        continue;
                    }
                    if is_test && PYTEST_MAGIC_GLOBALS.contains(&&*def.name) {
                        continue;
                    }
                    if def.kind == DeadDefKind::Reexport && is_library_root_init(src, w) {
                        continue;
                    }
                    if !live_slice[nid] {
                        let kind_label = match def.kind {
                            DeadDefKind::Function => "function",
                            DeadDefKind::Class => "class",
                            DeadDefKind::Variable => "variable",
                            DeadDefKind::TypeAlias => "type alias",
                            DeadDefKind::Reexport => "re-export",
                            _ => "symbol",
                        };
                        let suffix = if !is_test && all_live[nid] {
                            " (only referenced by tests)"
                        } else {
                            ""
                        };
                        findings.push(finding_at(
                            &src.filename,
                            def.line as usize,
                            def.column as usize,
                            "LIT511",
                            format!("unused {kind_label} '{}'{}", def.name, suffix),
                            Some(def.name.to_string()),
                        ));
                    }
                }
                DeadDefKind::Method | DeadDefKind::Property | DeadDefKind::Field => {
                    let parent_nid = local_to_node[src_idx][&def.parent_id];
                    if !live_slice[parent_nid] {
                        continue;
                    }
                    if is_dunder(&def.name) || &*def.name == "_" {
                        continue;
                    }
                    let Some(cinfo) = classes.get(&parent_nid) else {
                        continue;
                    };
                    if def.kind == DeadDefKind::Field
                        && (src.layer() != "core"
                            || !(cinfo.is_dataclass || cinfo.is_named_tuple)
                            || cinfo.is_pydantic
                            || cinfo.is_orm
                            || cinfo.is_typed_dict
                            || cinfo.is_enum
                            || escaped_classes.contains(&parent_nid)
                            || unknown_attr_reads.contains(&*def.name))
                    {
                        continue;
                    }
                    if !live_slice[nid] {
                        let cls_name = get_def(parent_nid).map_or("", |d| &*d.name);
                        let kind_label = match def.kind {
                            DeadDefKind::Method if cinfo.is_protocol => "protocol method",
                            DeadDefKind::Method => "method",
                            DeadDefKind::Property => "property",
                            DeadDefKind::Field => "dataclass field",
                            _ => "member",
                        };
                        let suffix = if !is_test && all_live[nid] {
                            " (only referenced by tests)"
                        } else {
                            ""
                        };
                        findings.push(finding_at(
                            &src.filename,
                            def.line as usize,
                            def.column as usize,
                            "LIT512",
                            format!("unused {kind_label} '{cls_name}.{}'{}", def.name, suffix),
                            Some(format!("{cls_name}.{}", def.name)),
                        ));
                    }
                }
                DeadDefKind::Param => {
                    if def.name.starts_with('_') || matches!(&*def.name, "self" | "cls") {
                        continue;
                    }
                    if read_params_set.contains(&def.local_id) {
                        continue;
                    }
                    let fn_nid = local_to_node[src_idx][&def.parent_id];
                    if !live_slice[fn_nid] {
                        continue;
                    }
                    let Some(fn_def) = get_def(fn_nid) else {
                        continue;
                    };
                    if (fn_def.flags
                        & (def_flags::IS_STUB_BODY
                            | def_flags::IS_OVERRIDE
                            | def_flags::IS_OVERLOAD
                            | def_flags::HAS_REG_DECORATOR
                            | def_flags::IS_PYTEST_FIXTURE
                            | def_flags::IS_PYDANTIC_HOOK
                            | def_flags::IS_ORM_HOOK))
                        != 0
                    {
                        continue;
                    }
                    if is_dunder(&fn_def.name)
                        || value_escaped_defs.contains(&fn_nid)
                        || proto_constrained.contains(&fn_nid)
                    {
                        continue;
                    }
                    if is_test
                        && (fn_def.parent_id != 0
                            || fn_def.name.starts_with("test_")
                            || fn_def.name.starts_with(PYTEST_HOOK_PREFIX))
                    {
                        continue;
                    }
                    if fn_def.parent_id != 0 {
                        let cls_nid = local_to_node[src_idx][&fn_def.parent_id];
                        if let Some(cinfo) = classes.get(&cls_nid)
                            && (cinfo.is_protocol
                                || cinfo.has_third_party_base
                                || cinfo.is_pydantic
                                || cinfo.is_orm
                                || !cinfo.workspace_bases.is_empty()
                                || subclasses_of.contains_key(&cls_nid))
                        {
                            continue;
                        }
                    }
                    findings.push(finding_at(
                        &src.filename,
                        def.line as usize,
                        def.column as usize,
                        "LIT514",
                        format!("unused parameter '{}' in '{}'", def.name, fn_def.name),
                        Some(def.name.to_string()),
                    ));
                }
            }
        }
    }

    (findings, manifest_problems)
}

#[allow(clippy::too_many_arguments)]
fn run_reachability(
    w: &Workspace,
    nodes: &[NodeMeta],
    mod_init_node: &[usize],
    adj: &[Vec<usize>],
    classes: &HashMap<usize, ClassInfo>,
    subclasses_of: &HashMap<usize, Vec<usize>>,
    wired_protocols: &HashMap<usize, HashSet<usize>>,
    owner_unknown_attrs: &HashMap<usize, Vec<String>>,
    roots: &[usize],
    prod_only: bool,
) -> (Vec<bool>, HashSet<usize>) {
    let mut live = vec![false; nodes.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut proto_constrained: HashSet<usize> = HashSet::new();

    let enqueue = |nid: usize, live: &mut [bool], queue: &mut VecDeque<usize>| {
        if nid < live.len() && !live[nid] {
            live[nid] = true;
            queue.push_back(nid);
        }
    };

    for &r in roots {
        if prod_only && is_test_source(&w.sources[nodes[r].source_idx]) {
            continue;
        }
        enqueue(r, &mut live, &mut queue);
    }

    let protocols: Vec<usize> = classes
        .iter()
        .filter(|(_, c)| c.is_protocol && !c.protocol_members.is_empty())
        .map(|(&cid, _)| cid)
        .collect();

    let mut expanded_unknown_attrs: HashSet<String> = HashSet::new();
    loop {
        let mut progress = false;
        while let Some(u) = queue.pop_front() {
            let u_src = nodes[u].source_idx;
            if prod_only && is_test_source(&w.sources[u_src]) {
                continue;
            }
            let m_init = mod_init_node[u_src];
            if !live[m_init] {
                enqueue(m_init, &mut live, &mut queue);
                progress = true;
            }
            for &v in &adj[u] {
                if !live[v] {
                    enqueue(v, &mut live, &mut queue);
                    progress = true;
                }
            }
            if let Some(attrs) = owner_unknown_attrs.get(&u) {
                for attr in attrs {
                    if expanded_unknown_attrs.insert(attr.clone()) {
                        progress = true;
                    }
                }
            }
        }

        // Propagate workspace base -> live subclass overrides.
        for (&base_cid, children) in subclasses_of {
            let Some(base_info) = classes.get(&base_cid) else {
                continue;
            };
            for (mname, base_mids) in &base_info.members {
                if base_mids.iter().any(|&mid| live[mid]) {
                    for &child_cid in children {
                        if !live[child_cid] {
                            continue;
                        }
                        if prod_only && is_test_source(&w.sources[nodes[child_cid].source_idx]) {
                            continue;
                        }
                        if let Some(child_mids) = classes[&child_cid].members.get(mname) {
                            for &cmid in child_mids {
                                if !live[cmid] {
                                    enqueue(cmid, &mut live, &mut queue);
                                    progress = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Propagate Protocol -> conforming live concrete classes (wired OR structural, NO subclassing required).
        for &pid in &protocols {
            if !live[pid] {
                continue;
            }
            let pinfo = &classes[&pid];
            let live_proto_members: Vec<&str> = pinfo
                .protocol_members
                .iter()
                .filter(|m| is_member_live_in_hierarchy(classes, &live, pid, m))
                .map(String::as_str)
                .collect();
            if live_proto_members.is_empty() {
                continue;
            }
            let wired = wired_protocols.get(&pid);
            for (&cid, cinfo) in classes {
                if cinfo.is_protocol || !live[cid] {
                    continue;
                }
                if prod_only && is_test_source(&w.sources[cinfo.source_idx]) {
                    continue;
                }
                let conforms = wired.is_some_and(|s| s.contains(&cid))
                    || pinfo
                        .protocol_members
                        .iter()
                        .all(|req| cinfo.all_member_names.contains(req));
                if !conforms {
                    continue;
                }
                for mname in &live_proto_members {
                    for cmid in lookup_class_member(classes, cid, mname) {
                        proto_constrained.insert(cmid);
                        if !live[cmid] {
                            enqueue(cmid, &mut live, &mut queue);
                            progress = true;
                        }
                    }
                }
            }
        }

        if !expanded_unknown_attrs.is_empty() {
            for (&cid, cinfo) in classes {
                if !live[cid] || cinfo.is_protocol {
                    continue;
                }
                if prod_only && is_test_source(&w.sources[cinfo.source_idx]) {
                    continue;
                }
                for (mname, mids) in &cinfo.members {
                    if expanded_unknown_attrs.contains(mname) {
                        for &mid in mids {
                            if !live[mid] {
                                enqueue(mid, &mut live, &mut queue);
                                progress = true;
                            }
                        }
                    }
                }
            }
        }

        if !progress && queue.is_empty() {
            break;
        }
    }

    (live, proto_constrained)
}

fn is_member_live_in_hierarchy(
    classes: &HashMap<usize, ClassInfo>,
    live: &[bool],
    cid: usize,
    member: &str,
) -> bool {
    let mut stack = vec![cid];
    let mut seen = HashSet::new();
    while let Some(curr) = stack.pop() {
        if !seen.insert(curr) {
            continue;
        }
        let Some(info) = classes.get(&curr) else {
            continue;
        };
        if let Some(mids) = info.members.get(member)
            && mids.iter().any(|&id| live[id])
        {
            return true;
        }
        stack.extend(&info.workspace_bases);
    }
    false
}

fn lookup_class_member(
    classes: &HashMap<usize, ClassInfo>,
    cid: usize,
    member: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![cid];
    let mut seen = HashSet::new();
    while let Some(curr) = stack.pop() {
        if !seen.insert(curr) {
            continue;
        }
        let Some(info) = classes.get(&curr) else {
            continue;
        };
        if let Some(mids) = info.members.get(member) {
            out.extend(mids.iter().copied());
        }
        stack.extend(&info.workspace_bases);
    }
    out
}

fn lookup_init_field_types(
    classes: &HashMap<usize, ClassInfo>,
    cid: usize,
    attr: &str,
) -> Vec<(usize, Box<str>, bool)> {
    // Returns `(source_idx, raw_type, is_elem)`.
    let mut out = Vec::new();
    let mut stack = vec![cid];
    let mut seen = HashSet::new();
    while let Some(curr) = stack.pop() {
        if !seen.insert(curr) {
            continue;
        }
        let Some(info) = classes.get(&curr) else {
            continue;
        };
        if let Some((direct, elem)) = info.init_fields.get(attr) {
            for t in direct {
                out.push((info.source_idx, t.clone(), false));
            }
            for t in elem {
                out.push((info.source_idx, t.clone(), true));
            }
        }
        stack.extend(&info.workspace_bases);
    }
    out
}

fn link_class_member(
    classes: &HashMap<usize, ClassInfo>,
    owner_nid: usize,
    cid: usize,
    member: &str,
    adj: &mut [Vec<usize>],
) {
    for mid in lookup_class_member(classes, cid, member) {
        if mid != owner_nid {
            adj[owner_nid].push(mid);
        }
    }
}

fn link_module_and_ancestors(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    mod_init_node: &[usize],
    owner_nid: usize,
    mod_name: &str,
    adj: &mut [Vec<usize>],
) {
    for (dot, _) in mod_name.match_indices('.') {
        let prefix = &mod_name[..dot];
        if let Some(&idx) = mod_lookup.get(prefix)
            && w.sources[idx].is_marker_file
        {
            let target = mod_init_node[idx];
            if target != owner_nid {
                adj[owner_nid].push(target);
            }
        }
    }
    if let Some(&idx) = mod_lookup.get(mod_name) {
        let target = mod_init_node[idx];
        if target != owner_nid {
            adj[owner_nid].push(target);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn link_target_module(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    source_mod_names: &[Option<String>],
    nodes: &[NodeMeta],
    mod_init_node: &[usize],
    owner_nid: usize,
    target_nid: usize,
    adj: &mut [Vec<usize>],
) {
    let target_src = nodes[target_nid].source_idx;
    if target_src != nodes[owner_nid].source_idx {
        let t_init = mod_init_node[target_src];
        if t_init != owner_nid {
            adj[owner_nid].push(t_init);
        }
        if let Some(tmod) = source_mod_names[target_src].as_deref() {
            link_module_and_ancestors(w, mod_lookup, mod_init_node, owner_nid, tmod, adj);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_symbol_rec(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    mod_init_node: &[usize],
    top_defs: &HashMap<(usize, String), Vec<usize>>,
    from_src: usize,
    target: &str,
    seen: &mut HashSet<(usize, String)>,
    out: &mut Vec<usize>,
) {
    if !seen.insert((from_src, target.to_owned())) {
        return;
    }
    if let Some(local) = target.strip_prefix('@') {
        if let Some(nids) = top_defs.get(&(from_src, local.to_owned())) {
            out.extend(nids.iter().copied());
        }
        return;
    }
    if let Some(&midx) = mod_lookup.get(target) {
        out.push(mod_init_node[midx]);
        return;
    }
    if let Some((mod_part, sym_part)) = target.rsplit_once('.')
        && let Some(&midx) = mod_lookup.get(mod_part)
    {
        if let Some(nids) = top_defs.get(&(midx, sym_part.to_owned())) {
            out.extend(nids.iter().copied());
            return;
        }
        let src = &w.sources[midx];
        for imp in &src.top_imports {
            if &*imp.local == sym_part {
                if imp.is_from
                    && let Some(member) = imp.member.as_deref()
                {
                    let next = format!("{}.{member}", imp.module);
                    resolve_symbol_rec(
                        w,
                        mod_lookup,
                        mod_init_node,
                        top_defs,
                        midx,
                        &next,
                        seen,
                        out,
                    );
                } else {
                    resolve_symbol_rec(
                        w,
                        mod_lookup,
                        mod_init_node,
                        top_defs,
                        midx,
                        &imp.module,
                        seen,
                        out,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_type_rec(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    nodes: &[NodeMeta],
    mod_init_node: &[usize],
    top_defs: &HashMap<(usize, String), Vec<usize>>,
    classes: &HashMap<usize, ClassInfo>,
    from_src: usize,
    raw: &str,
    depth: usize,
    visited: &mut HashSet<(usize, String)>,
    out: &mut Vec<usize>,
) {
    if depth > 8 || !visited.insert((from_src, raw.to_owned())) {
        return;
    }
    // 1. Subscript element extraction: `[expr]` -> elem_types of the target variable/field/function.
    if let Some(inner) = raw.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        // Check `.base.attr` field/property elem_types first.
        if let Some(attr_expr) = inner.strip_prefix('.')
            && let Some((base_raw, attr)) = attr_expr.rsplit_once('.')
        {
            let mut base_classes = Vec::new();
            resolve_type_rec(
                w,
                mod_lookup,
                nodes,
                mod_init_node,
                top_defs,
                classes,
                from_src,
                base_raw,
                depth + 1,
                visited,
                &mut base_classes,
            );
            for bcid in base_classes {
                for (fsrc, fty, is_elem) in lookup_init_field_types(classes, bcid, attr) {
                    if is_elem {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            fsrc,
                            &fty,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                }
                for mid in lookup_class_member(classes, bcid, attr) {
                    let meta = nodes[mid];
                    if let Some(d_idx) = meta.def_idx
                        && let Some(sum) = summary_of(&w.sources[meta.source_idx])
                    {
                        for et in &sum.defs[d_idx].elem_types {
                            resolve_type_rec(
                                w,
                                mod_lookup,
                                nodes,
                                mod_init_node,
                                top_defs,
                                classes,
                                meta.source_idx,
                                et,
                                depth + 1,
                                visited,
                                out,
                            );
                        }
                    }
                }
            }
            return;
        }
        let mut sym_seen = HashSet::new();
        let mut targets = Vec::new();
        resolve_symbol_rec(
            w,
            mod_lookup,
            mod_init_node,
            top_defs,
            from_src,
            inner,
            &mut sym_seen,
            &mut targets,
        );
        for tid in targets {
            let meta = nodes[tid];
            if let Some(d_idx) = meta.def_idx
                && let Some(sum) = summary_of(&w.sources[meta.source_idx])
            {
                let def = &sum.defs[d_idx];
                if def.kind == DeadDefKind::Reexport
                    && let Some(rt) = def.reexport_target.as_deref()
                {
                    resolve_type_rec(
                        w,
                        mod_lookup,
                        nodes,
                        mod_init_node,
                        top_defs,
                        classes,
                        meta.source_idx,
                        &format!("[{rt}]"),
                        depth + 1,
                        visited,
                        out,
                    );
                } else {
                    for et in &def.elem_types {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            meta.source_idx,
                            et,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                }
            }
        }
        return;
    }

    // 2. Chained attribute / property / field type lookup: `.<base>.<attr>`.
    if let Some(rest) = raw.strip_prefix('.')
        && let Some((base_raw, attr)) = rest.rsplit_once('.')
    {
        // First try resolving `base_raw` as a class type (e.g., `self._repos.agent`).
        let mut base_classes = Vec::new();
        resolve_type_rec(
            w,
            mod_lookup,
            nodes,
            mod_init_node,
            top_defs,
            classes,
            from_src,
            base_raw,
            depth + 1,
            visited,
            &mut base_classes,
        );
        for bcid in base_classes {
            for (fsrc, fty, is_elem) in lookup_init_field_types(classes, bcid, attr) {
                if !is_elem {
                    resolve_type_rec(
                        w,
                        mod_lookup,
                        nodes,
                        mod_init_node,
                        top_defs,
                        classes,
                        fsrc,
                        &fty,
                        depth + 1,
                        visited,
                        out,
                    );
                }
            }
            for mid in lookup_class_member(classes, bcid, attr) {
                let meta = nodes[mid];
                if let Some(d_idx) = meta.def_idx
                    && let Some(sum) = summary_of(&w.sources[meta.source_idx])
                {
                    let def = &sum.defs[d_idx];
                    let types = match def.kind {
                        DeadDefKind::Field => &def.value_types,
                        DeadDefKind::Property | DeadDefKind::Method => &def.return_types,
                        _ => &def.value_types,
                    };
                    for t in types {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            meta.source_idx,
                            t,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                }
            }
        }
        // Also try as a module attribute (`mod.Symbol`) if `base_raw` is a module!
        if !base_raw.starts_with(['.', '>', '[', '@']) {
            let qualified = format!("{base_raw}.{attr}");
            resolve_type_rec(
                w,
                mod_lookup,
                nodes,
                mod_init_node,
                top_defs,
                classes,
                from_src,
                &qualified,
                depth + 1,
                visited,
                out,
            );
        }
        return;
    }

    // 3. Call result: `><callee>` (class constructor, function/method return type, or `[container]()`).
    if let Some(callee) = raw.strip_prefix('>') {
        // Call on a subscripted class map: `>[WORKBOOK_FORM_SERVICES]` -> resolve `[WORKBOOK_FORM_SERVICES]`.
        if callee.starts_with('[') {
            resolve_type_rec(
                w,
                mod_lookup,
                nodes,
                mod_init_node,
                top_defs,
                classes,
                from_src,
                callee,
                depth + 1,
                visited,
                out,
            );
            return;
        }
        // Method call on a receiver expression: `>.<recv>.<method>`.
        if let Some(rest) = callee.strip_prefix('.')
            && let Some((recv_raw, method_name)) = rest.rsplit_once('.')
        {
            let mut recv_classes = Vec::new();
            resolve_type_rec(
                w,
                mod_lookup,
                nodes,
                mod_init_node,
                top_defs,
                classes,
                from_src,
                recv_raw,
                depth + 1,
                visited,
                &mut recv_classes,
            );
            for rcid in recv_classes {
                for mid in lookup_class_member(classes, rcid, method_name) {
                    let meta = nodes[mid];
                    if let Some(d_idx) = meta.def_idx
                        && let Some(sum) = summary_of(&w.sources[meta.source_idx])
                    {
                        for ret in &sum.defs[d_idx].return_types {
                            resolve_type_rec(
                                w,
                                mod_lookup,
                                nodes,
                                mod_init_node,
                                top_defs,
                                classes,
                                meta.source_idx,
                                ret,
                                depth + 1,
                                visited,
                                out,
                            );
                        }
                    }
                }
            }
            // Also handle module-qualified function calls `>.mod.func` -> `>mod.func`.
            if !recv_raw.starts_with(['.', '>', '[', '@']) {
                resolve_type_rec(
                    w,
                    mod_lookup,
                    nodes,
                    mod_init_node,
                    top_defs,
                    classes,
                    from_src,
                    &format!(">{recv_raw}.{method_name}"),
                    depth + 1,
                    visited,
                    out,
                );
            }
            return;
        }

        let mut sym_seen = HashSet::new();
        let mut targets = Vec::new();
        resolve_symbol_rec(
            w,
            mod_lookup,
            mod_init_node,
            top_defs,
            from_src,
            callee,
            &mut sym_seen,
            &mut targets,
        );
        if targets.is_empty()
            && let Some((recv_raw, method_name)) = callee.rsplit_once('.')
        {
            let mut recv_classes = Vec::new();
            resolve_type_rec(
                w,
                mod_lookup,
                nodes,
                mod_init_node,
                top_defs,
                classes,
                from_src,
                recv_raw,
                depth + 1,
                visited,
                &mut recv_classes,
            );
            for rcid in recv_classes {
                targets.extend(lookup_class_member(classes, rcid, method_name));
            }
        }
        for tid in targets {
            if classes.contains_key(&tid) {
                out.push(tid);
                continue;
            }
            let meta = nodes[tid];
            if let Some(d_idx) = meta.def_idx
                && let Some(sum) = summary_of(&w.sources[meta.source_idx])
            {
                let def = &sum.defs[d_idx];
                if def.kind == DeadDefKind::Reexport
                    && let Some(rt) = def.reexport_target.as_deref()
                {
                    resolve_type_rec(
                        w,
                        mod_lookup,
                        nodes,
                        mod_init_node,
                        top_defs,
                        classes,
                        meta.source_idx,
                        &format!(">{rt}"),
                        depth + 1,
                        visited,
                        out,
                    );
                } else {
                    for ret in &def.return_types {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            meta.source_idx,
                            ret,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                    // Callable variable / alias (e.g., `Factory = MyClass` -> `Factory()`).
                    for vt in &def.value_types {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            meta.source_idx,
                            vt,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                }
            }
        }
        return;
    }

    // 4. Direct class / alias / variable lookup.
    let mut sym_seen = HashSet::new();
    let mut targets = Vec::new();
    resolve_symbol_rec(
        w,
        mod_lookup,
        mod_init_node,
        top_defs,
        from_src,
        raw,
        &mut sym_seen,
        &mut targets,
    );
    for tid in targets {
        if classes.contains_key(&tid) {
            out.push(tid);
        } else {
            let meta = nodes[tid];
            if let Some(d_idx) = meta.def_idx
                && let Some(sum) = summary_of(&w.sources[meta.source_idx])
            {
                let def = &sum.defs[d_idx];
                if def.kind == DeadDefKind::Reexport
                    && let Some(rt) = def.reexport_target.as_deref()
                {
                    resolve_type_rec(
                        w,
                        mod_lookup,
                        nodes,
                        mod_init_node,
                        top_defs,
                        classes,
                        meta.source_idx,
                        rt,
                        depth + 1,
                        visited,
                        out,
                    );
                } else {
                    for vt in &def.value_types {
                        resolve_type_rec(
                            w,
                            mod_lookup,
                            nodes,
                            mod_init_node,
                            top_defs,
                            classes,
                            meta.source_idx,
                            vt,
                            depth + 1,
                            visited,
                            out,
                        );
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_callee_param_classes(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    nodes: &[NodeMeta],
    mod_init_node: &[usize],
    top_defs: &HashMap<(usize, String), Vec<usize>>,
    classes: &HashMap<usize, ClassInfo>,
    from_src: usize,
    callee_raw: &str,
    pos: Option<usize>,
    kw_name: Option<&str>,
) -> Vec<usize> {
    let mut sym_seen = HashSet::new();
    let mut targets = Vec::new();
    resolve_symbol_rec(
        w,
        mod_lookup,
        mod_init_node,
        top_defs,
        from_src,
        callee_raw,
        &mut sym_seen,
        &mut targets,
    );
    let mut fn_nids = Vec::new();
    for tid in targets {
        if classes.contains_key(&tid) {
            fn_nids.extend(lookup_class_member(classes, tid, "__init__"));
        } else {
            fn_nids.push(tid);
        }
    }
    let mut out = Vec::new();
    for fnid in fn_nids {
        let meta = nodes[fnid];
        let Some(d_idx) = meta.def_idx else {
            continue;
        };
        let Some(sum) = summary_of(&w.sources[meta.source_idx]) else {
            continue;
        };
        let def = &sum.defs[d_idx];
        let ptypes: Option<&[Box<str>]> = if let Some(idx) = pos {
            def.param_types.get(idx).map(|(_, t)| &**t)
        } else if let Some(kw) = kw_name {
            def.param_types
                .iter()
                .find(|(n, _)| &**n == kw)
                .map(|(_, t)| &**t)
        } else {
            None
        };
        if let Some(types) = ptypes {
            let mut visited = HashSet::new();
            for t in types {
                resolve_type_rec(
                    w,
                    mod_lookup,
                    nodes,
                    mod_init_node,
                    top_defs,
                    classes,
                    meta.source_idx,
                    t,
                    0,
                    &mut visited,
                    &mut out,
                );
            }
        }
    }
    out
}

fn resolve_entrypoint_spec(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    mod_init_node: &[usize],
    top_defs: &HashMap<(usize, String), Vec<usize>>,
    classes: &HashMap<usize, ClassInfo>,
    spec: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    let (mod_part, attr_part) = match spec.split_once(':') {
        Some((m, a)) => (m, Some(a)),
        None => (spec, None),
    };
    if let Some(prefix) = mod_part.strip_suffix(".*") {
        let dot_prefix = format!("{prefix}.");
        for (mod_name, &src_idx) in mod_lookup {
            if mod_name == prefix || mod_name.starts_with(&dot_prefix) {
                if let Some(attr) = attr_part {
                    resolve_attr_in_module(
                        w,
                        mod_lookup,
                        mod_init_node,
                        top_defs,
                        classes,
                        src_idx,
                        attr,
                        &mut out,
                    );
                } else {
                    out.push(mod_init_node[src_idx]);
                }
            }
        }
        return out;
    }
    let Some(&src_idx) = mod_lookup.get(mod_part) else {
        return out;
    };
    if let Some(attr) = attr_part {
        resolve_attr_in_module(
            w,
            mod_lookup,
            mod_init_node,
            top_defs,
            classes,
            src_idx,
            attr,
            &mut out,
        );
    } else {
        out.push(mod_init_node[src_idx]);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn resolve_attr_in_module(
    w: &Workspace,
    mod_lookup: &HashMap<String, usize>,
    mod_init_node: &[usize],
    top_defs: &HashMap<(usize, String), Vec<usize>>,
    classes: &HashMap<usize, ClassInfo>,
    src_idx: usize,
    attr: &str,
    out: &mut Vec<usize>,
) {
    if let Some((cls_name, mem_name)) = attr.split_once('.') {
        if let Some(cids) = top_defs.get(&(src_idx, cls_name.to_owned())) {
            for &cid in cids {
                if classes.contains_key(&cid) {
                    out.push(cid);
                    out.extend(lookup_class_member(classes, cid, mem_name));
                }
            }
        }
    } else if let Some(nids) = top_defs.get(&(src_idx, attr.to_owned())) {
        out.push(mod_init_node[src_idx]);
        out.extend(nids.iter().copied());
    } else {
        let mut seen = HashSet::new();
        let mut resolved = Vec::new();
        if let Some(m) = w.sources[src_idx].module.as_deref() {
            resolve_symbol_rec(
                w,
                mod_lookup,
                mod_init_node,
                top_defs,
                src_idx,
                &format!("{m}.{attr}"),
                &mut seen,
                &mut resolved,
            );
        }
        if !resolved.is_empty() {
            out.push(mod_init_node[src_idx]);
            out.extend(resolved);
        }
    }
}
