use crate::{
    model::{Finding, ImportStmt, Source, finding_at},
    workspace::Workspace,
};
use std::collections::{BTreeSet, HashSet};

fn for_each_edge<'a>(stmt: &'a ImportStmt, mut f: impl FnMut(&'a str, Option<&'a str>)) {
    if !stmt.is_from {
        for name in &stmt.names {
            f(name, None);
        }
    } else if let Some(base) = stmt.resolved_module.as_deref() {
        for name in &stmt.names {
            f(base, Some(name));
        }
    }
}

fn targets(
    module: &str,
    name: Option<&str>,
    ws: &Workspace,
    seen: &mut HashSet<(String, Option<String>)>,
    out: &mut Vec<usize>,
) {
    if !seen.insert((module.to_owned(), name.map(str::to_owned))) {
        return;
    }
    if let Some(n) = name {
        let mut qualified = String::with_capacity(module.len() + 1 + n.len());
        qualified.push_str(module);
        qualified.push('.');
        qualified.push_str(n);
        if let Some(idx) = ws.module_index(&qualified) {
            out.push(idx);
            return;
        }
    }
    let Some(idx) = ws.module_index(module) else {
        return;
    };
    let f = &ws.sources[idx];
    if let Some(name) = name {
        let before = out.len();
        for imp in &f.top_imports {
            if imp.is_from
                && let Some(sym) = imp.member.as_deref()
                && (&*imp.local == name || name == "*" || sym == "*")
            {
                targets(&imp.module, Some(sym), ws, seen, out);
            }
        }
        if out.len() > before {
            return;
        }
    }
    out.push(idx);
}

fn push_ancestors(mod_name: &str, ws: &Workspace, out: &mut Vec<usize>, seen: &mut HashSet<usize>) {
    for (dot, _) in mod_name.match_indices('.') {
        let prefix = &mod_name[..dot];
        if let Some(idx) = ws.module_index(prefix)
            && ws.sources[idx].is_marker_file
            && seen.insert(idx)
        {
            out.push(idx);
        }
    }
    if let Some(idx) = ws.module_index(mod_name)
        && seen.insert(idx)
    {
        out.push(idx);
    }
}

fn collect_direct_step(
    module: &str,
    name: Option<&str>,
    ws: &Workspace,
    target_seen: &mut HashSet<(String, Option<String>)>,
    out: &mut Vec<usize>,
    step_seen: &mut HashSet<usize>,
) {
    out.clear();
    step_seen.clear();
    target_seen.clear();
    targets(module, name, ws, target_seen, out);
    // Deduplicate targets in place while recording them in step_seen.
    out.retain(|&idx| step_seen.insert(idx));
    let target_count = out.len();
    push_ancestors(module, ws, out, step_seen);
    for i in 0..target_count {
        let idx = out[i];
        if let Some(tmod) = ws.sources[idx].module.as_deref() {
            push_ancestors(tmod, ws, out, step_seen);
        }
    }
}

fn compute_init_closure(start_idx: usize, ws: &Workspace) -> Box<[usize]> {
    let start = &ws.sources[start_idx];
    if start.init_imports.is_empty() {
        return Box::new([]);
    }
    let mut pending: Vec<(&str, Option<&str>)> = start
        .init_imports
        .iter()
        .map(|(m, n)| (&**m, n.as_deref()))
        .collect();
    let mut seen_imports: HashSet<(&str, Option<&str>)> = HashSet::new();
    let mut expanded: HashSet<usize> = HashSet::from([start_idx]);
    let mut emitted: HashSet<usize> = HashSet::new();
    let mut result: Vec<usize> = Vec::new();
    let mut target_seen: HashSet<(String, Option<String>)> = HashSet::new();
    let mut step: Vec<usize> = Vec::new();
    let mut step_seen: HashSet<usize> = HashSet::new();

    while let Some((m, n)) = pending.pop() {
        if !seen_imports.insert((m, n)) {
            continue;
        }
        collect_direct_step(m, n, ws, &mut target_seen, &mut step, &mut step_seen);
        for &idx in &step {
            if emitted.insert(idx) {
                result.push(idx);
            }
            if expanded.insert(idx) {
                for (im, sym) in &ws.sources[idx].init_imports {
                    pending.push((im, sym.as_deref()));
                }
            }
        }
    }
    result.into_boxed_slice()
}

fn reexported(
    module: &str,
    symbol: Option<&str>,
    ws: &Workspace,
    forbidden: &BTreeSet<String>,
    seen: &mut HashSet<(String, Option<String>)>,
    reached: &mut BTreeSet<String>,
) {
    if !seen.insert((module.to_owned(), symbol.map(str::to_owned))) {
        return;
    }
    let root = module.split('.').next().unwrap_or("");
    if forbidden.contains(root) {
        reached.insert(root.to_owned());
        return;
    }
    let (Some(f), Some(symbol)) = (ws.module_source(module), symbol) else {
        return;
    };
    for imp in &f.top_imports {
        let exported = if imp.is_from {
            let orig = imp.member.as_deref().unwrap_or("");
            &*imp.local == symbol || symbol == "*" || orig == "*"
        } else {
            &*imp.local == symbol
        };
        if exported {
            reexported(
                &imp.module,
                imp.member.as_deref(),
                ws,
                forbidden,
                seen,
                reached,
            );
        }
    }
}

fn domain(module: &str) -> Option<(&str, &str)> {
    let mut p = module.split('.');
    let root = p.next()?;
    let second = p.next()?;
    let dom = p.next()?;
    if second == "domains" {
        Some((root, dom))
    } else {
        None
    }
}

#[inline]
fn resolve_direct_target(imported: &str, symbol: Option<&str>, ws: &Workspace) -> Option<usize> {
    if let Some(sym) = symbol {
        let mut qualified = String::with_capacity(imported.len() + 1 + sym.len());
        qualified.push_str(imported);
        qualified.push('.');
        qualified.push_str(sym);
        if let Some(idx) = ws.module_index(&qualified) {
            return Some(idx);
        }
    }
    ws.module_index(imported)
}

fn compute_sccs(ws: &Workspace) -> Vec<Option<u32>> {
    let n = ws.sources.len();
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut active_nodes: Vec<usize> = Vec::new();

    for (idx, source) in ws.sources.iter().enumerate() {
        let Some(src_mod) = source.module.as_deref() else {
            continue;
        };
        if ws.module_index(src_mod) != Some(idx) {
            continue;
        }
        active_nodes.push(idx);
        let mut neighbors = Vec::new();
        for stmt in &source.all_imports {
            for_each_edge(stmt, |imported, sym| {
                if let Some(dest_idx) = resolve_direct_target(imported, sym, ws)
                    && dest_idx != idx
                    && !ws.dependency_overridden(source, &ws.sources[dest_idx])
                {
                    neighbors.push(dest_idx);
                }
            });
        }
        neighbors.sort_unstable();
        neighbors.dedup();
        adjacency[idx] = neighbors;
    }

    const UNVISITED: u32 = u32::MAX;
    let mut index_counter = 0u32;
    let mut indices = vec![UNVISITED; n];
    let mut lowlink = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut components: Vec<Option<u32>> = vec![None; n];
    let mut component_id = 0u32;
    let mut call_stack: Vec<(usize, usize)> = Vec::new();

    for root in active_nodes {
        if indices[root] != UNVISITED {
            continue;
        }
        indices[root] = index_counter;
        lowlink[root] = index_counter;
        index_counter += 1;
        stack.push(root);
        on_stack[root] = true;
        call_stack.push((root, 0));

        while let Some((node, pos)) = call_stack.last_mut() {
            let node = *node;
            if *pos < adjacency[node].len() {
                let nxt = adjacency[node][*pos];
                *pos += 1;
                if indices[nxt] == UNVISITED {
                    indices[nxt] = index_counter;
                    lowlink[nxt] = index_counter;
                    index_counter += 1;
                    stack.push(nxt);
                    on_stack[nxt] = true;
                    call_stack.push((nxt, 0));
                } else if on_stack[nxt] && indices[nxt] < lowlink[node] {
                    lowlink[node] = indices[nxt];
                }
            } else {
                call_stack.pop();
                let finished_low = lowlink[node];
                if let Some(&(parent, _)) = call_stack.last()
                    && finished_low < lowlink[parent]
                {
                    lowlink[parent] = finished_low;
                }
                if finished_low == indices[node] {
                    let start = stack.iter().rposition(|&m| m == node).unwrap();
                    let scc_len = stack.len() - start;
                    if scc_len >= 2 {
                        for &member in &stack[start..] {
                            on_stack[member] = false;
                            components[member] = Some(component_id);
                        }
                        component_id += 1;
                    } else {
                        on_stack[node] = false;
                    }
                    stack.truncate(start);
                }
            }
        }
    }
    components
}

pub fn precompute_sccs(ws: &Workspace) {
    ws.cyclic_components(|| compute_sccs(ws));
}

pub fn check(source_idx: usize, file: &Source, ws: &Workspace) -> Vec<Finding> {
    let _span = crate::profile::span("dependencies");
    let mut out = Vec::new();
    if file.classification.is_none() {
        let message = format!(
            "file matches no layer in its package layout ({}); move it, extend the layout, or add an override",
            ws.packages[file.owner].layout
        );
        out.push(finding_at(&file.filename, 1, 0, "LIT002", message, None));
    }
    if file.all_imports.is_empty() {
        return out;
    }

    let layer = file.layer_name();
    let allowed = ws.policy.layer_dependencies.get(layer);
    let check_layer = !layer.is_empty()
        && file.package.is_some()
        && !(allowed.is_none() && file.layer() == "test");
    let owner = file.module.as_deref().and_then(domain);
    let check_unit_test = ws.strict && file.layer_name() == "test";

    if check_layer || owner.is_some() || check_unit_test {
        let mut checked_targets: HashSet<usize> = HashSet::new();
        checked_targets.insert(source_idx);
        let mut expanded_markers: HashSet<usize> = HashSet::new();
        let mut target_seen: HashSet<(String, Option<String>)> = HashSet::new();
        let mut step: Vec<usize> = Vec::new();
        let mut step_seen: HashSet<usize> = HashSet::new();

        let check_target = |target_idx: usize,
                            stmt: &ImportStmt,
                            checked_targets: &mut HashSet<usize>,
                            out: &mut Vec<Finding>| {
            if !checked_targets.insert(target_idx) {
                return;
            }
            let target = &ws.sources[target_idx];
            let module = target.module.as_deref().unwrap_or("");
            if check_unit_test
                && target.layer() == "adapter"
                && !ws.policy.allows_dependency(layer, module)
                && !ws.dependency_overridden(file, target)
            {
                out.push(finding_at(
                    &file.filename,
                    stmt.line as usize,
                    stmt.column as usize,
                    "LIT309",
                    format!(
                        "unit test imports adapter module {module}; pass fakes through ports instead"
                    ),
                    Some(module.into()),
                ));
            }
            if check_layer
                && let Some(c) = &target.classification
                && c.layer != "marker"
            {
                let permitted = allowed.map_or(c.layer != "test", |a| ws.policy.is_a(&c.name, a));
                if !permitted
                    && !ws.policy.allows_dependency(layer, module)
                    && !ws.dependency_overridden(file, target)
                {
                    out.push(finding_at(
                        &file.filename,
                        stmt.line as usize,
                        stmt.column as usize,
                        "LIT101",
                        format!("{layer} cannot import {} module {module}", c.name),
                        Some(module.into()),
                    ));
                }
            }
            if let (Some(owner), Some(to)) = (owner, domain(module)) {
                if to.0 != owner.0
                    || to == owner
                    || target.layer() == "marker"
                    || ws.dependency_overridden(file, target)
                {
                    return;
                }
                let declared = ws
                    .policy
                    .domain_dependencies
                    .get(owner.1)
                    .is_some_and(|d| d.contains(to.1));
                let message = if !declared {
                    Some(format!(
                        "{} domain cannot import {module}: {} is not in domains.{}",
                        owner.1, to.1, owner.1
                    ))
                } else if !["core", "shell"].contains(&target.layer()) {
                    Some(format!(
                        "{} domain cannot import {module}: only the core and shell modules of {} are public",
                        owner.1, to.1
                    ))
                } else {
                    None
                };
                if let Some(message) = message {
                    out.push(finding_at(
                        &file.filename,
                        stmt.line as usize,
                        stmt.column as usize,
                        "LIT104",
                        message,
                        Some(module.into()),
                    ));
                }
            }
        };

        for stmt in &file.all_imports {
            for_each_edge(stmt, |m, n| {
                collect_direct_step(m, n, ws, &mut target_seen, &mut step, &mut step_seen);
                for &target_idx in &step {
                    check_target(target_idx, stmt, &mut checked_targets, &mut out);
                    if ws.sources[target_idx].is_marker_file && expanded_markers.insert(target_idx)
                    {
                        let closure =
                            ws.init_closure(target_idx, || compute_init_closure(target_idx, ws));
                        for &reached_idx in closure {
                            check_target(reached_idx, stmt, &mut checked_targets, &mut out);
                        }
                    }
                }
            });
        }
    }

    if let Some(forbidden) = ws
        .policy
        .forbidden_imports
        .get(layer)
        .filter(|f| !f.is_empty())
    {
        let mut seen = HashSet::new();
        let mut reached = BTreeSet::new();
        for stmt in &file.all_imports {
            reached.clear();
            for_each_edge(stmt, |m, n| {
                seen.clear();
                reexported(m, n, ws, forbidden, &mut seen, &mut reached);
            });
            if !reached.is_empty() {
                let target = reached.iter().cloned().collect::<Vec<_>>().join(", ");
                out.push(finding_at(
                    &file.filename,
                    stmt.line as usize,
                    stmt.column as usize,
                    "LIT102",
                    format!("{layer} modules cannot import {target}"),
                    Some(target),
                ));
            }
        }
    }

    if ws.packages[file.owner].kind == "client" {
        let servers = ws.server_roots();
        for stmt in &file.all_imports {
            if stmt.is_from
                && let Some(m) = stmt.raw_module.as_deref()
            {
                if servers.contains(m.split('.').next().unwrap_or("")) {
                    out.push(finding_at(
                        &file.filename,
                        stmt.line as usize,
                        stmt.column as usize,
                        "LIT103",
                        format!("client packages cannot import server module {m}"),
                        Some(m.into()),
                    ));
                }
            } else {
                for m in &stmt.names {
                    if servers.contains(m.split('.').next().unwrap_or("")) {
                        out.push(finding_at(
                            &file.filename,
                            stmt.line as usize,
                            stmt.column as usize,
                            "LIT103",
                            format!("client packages cannot import server module {m}"),
                            Some(m.to_string()),
                        ));
                    }
                }
            }
        }
    }

    if let Some(src_mod) = file.module.as_deref()
        && !file.layer().is_empty()
        && let Some(src_winner) = ws.module_index(src_mod)
    {
        let components = ws.cyclic_components(|| compute_sccs(ws));
        if let Some(group) = components[src_winner] {
            let mut seen_cycle: HashSet<usize> = HashSet::new();
            for stmt in &file.all_imports {
                for_each_edge(stmt, |m, n| {
                    if let Some(dest_idx) = resolve_direct_target(m, n, ws)
                        && dest_idx != src_winner
                        && components[dest_idx] == Some(group)
                        && !seen_cycle.contains(&dest_idx)
                    {
                        let dest = &ws.sources[dest_idx];
                        if !ws.dependency_overridden(file, dest) {
                            seen_cycle.insert(dest_idx);
                            let target = dest.module.as_deref().unwrap_or("");
                            out.push(finding_at(
                                &file.filename,
                                stmt.line as usize,
                                stmt.column as usize,
                                "LIT105",
                                format!(
                                    "import cycle: {src_mod} and {target} depend on each other"
                                ),
                                Some(target.to_owned()),
                            ));
                        }
                    }
                });
            }
        }
    }
    out
}
