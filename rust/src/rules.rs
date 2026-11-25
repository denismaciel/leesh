use crate::{
    ast::{self, Node, child, for_each_child, items, kind, s, walk},
    model::{ClassCheck, DeferredLocal, FileContext, Finding, Source, TopDef, finding, finding_at},
    symbols,
    workspace::{Policy, Workspace},
};
use std::collections::{BTreeSet, HashMap, HashSet};

mod ambient;
mod capabilities;
mod defaults;
mod effects;
mod mutation;
mod simple;
mod strict;
mod subclass;

#[inline]
fn add(
    out: &mut Vec<Finding>,
    file: &FileContext<'_>,
    node: &Node,
    code: &str,
    msg: impl Into<String>,
) {
    out.push(finding(file, node, code, msg, None));
}

/// A name the file reads: a loaded name or attribute, or `getattr` spelling one.
#[inline]
fn loaded(file: &FileContext<'_>, node: &Node) -> bool {
    match kind(node) {
        "Name" | "Attribute" => kind(child(node, "ctx")) == "Load",
        "Call" => file.symbols.only(child(node, "func"), "builtins.getattr"),
        _ => false,
    }
}

fn parameters(node: &Node) -> Vec<&Node> {
    let args = child(node, "args");
    let posonly = items(args, "posonlyargs");
    let regular = items(args, "args");
    let kwonly = items(args, "kwonlyargs");
    let mut out = Vec::with_capacity(posonly.len() + regular.len() + kwonly.len());
    out.extend(posonly);
    out.extend(regular);
    out.extend(kwonly);
    out
}

pub struct LocalCheckOutput {
    pub findings: Vec<Finding>,
    pub deferred: DeferredLocal,
    pub top_defs: Box<[TopDef]>,
    pub classes: Box<[ClassCheck]>,
}

pub fn check_local(file: &FileContext<'_>, policy: &Policy, strict_mode: bool) -> LocalCheckOutput {
    let _span = crate::profile::span("local_rules");
    let mut out = Vec::new();
    let mut deferred = DeferredLocal::default();
    let inner = file.has_layer(&["core", "port"]);
    let deterministic = file.deterministic(policy);
    let check_wall_clock = !file.layer().is_empty()
        && !file.has_layer(&["test"])
        && !file
            .module
            .is_some_and(|m| policy.clock_modules.contains(m));
    let check_nondet = inner || deterministic || check_wall_clock;
    let check_env =
        !file.layer().is_empty() && !file.has_layer(&["boundary", "transaction", "test"]);
    let forbidden_calls = policy.forbidden_calls.get(file.layer_name());

    for &node in file.nodes {
        if loaded(file, node) && !file.symbols.is_unknown(node) {
            if check_nondet
                && file.symbols.all(node, |n| {
                    if inner {
                        effects::impure_in_core(n)
                    } else if deterministic {
                        effects::nondeterministic(n)
                    } else {
                        effects::WALL_CLOCK.contains(&n)
                    }
                })
            {
                let text = file.symbols.joined(node);
                let msg = if deterministic {
                    format!(
                        "{text} hides nondeterminism; {} modules must be deterministic",
                        file.layer_name()
                    )
                } else if inner {
                    format!(
                        "{text} hides nondeterminism; pass the observed time, ID, or random value into the core"
                    )
                } else {
                    format!(
                        "{text} reads wall time directly; use the clock listed in workspace.clock_modules"
                    )
                };
                out.push(finding(file, node, "LIT201", msg, Some(text)));
            }
            if inner && file.symbols.all(node, effects::io) {
                let text = file.symbols.joined(node);
                out.push(finding(
                    file,
                    node,
                    "LIT202",
                    format!("{text} performs I/O; pass its result into the core"),
                    Some(text),
                ));
            }
            if inner && file.symbols.all(node, effects::dynamic) {
                let text = file.symbols.joined(node);
                out.push(finding(
                    file,
                    node,
                    "LIT208",
                    format!(
                        "{text} runs code chosen at run time; import and call what the core needs by name"
                    ),
                    Some(text),
                ));
            }
            if check_env && file.symbols.all(node, effects::environment) {
                add(
                    &mut out,
                    file,
                    node,
                    "LIT301",
                    format!(
                        "{} modules cannot read the environment; read configuration at a boundary and pass it in",
                        file.layer_name()
                    ),
                );
            }
        }
        let k = kind(node);
        if inner && k == "Attribute" && s(node, "attr") == "__dict__" {
            out.push(finding(
                file,
                node,
                "LIT208",
                "__dict__ reaches raw object state; use explicit attributes on frozen values",
                Some("__dict__".into()),
            ));
        }
        if inner
            && k == "Call"
            && file.symbols.only(child(node, "func"), "builtins.getattr")
            && items(node, "args")
                .get(1)
                .is_none_or(|a| kind(a) != "Constant" || !a["value"].is_string())
        {
            out.push(finding(
                file,
                node,
                "LIT208",
                "builtins.getattr with a dynamic name runs code chosen at run time; access attributes by name",
                Some("builtins.getattr".into()),
            ));
        }
        if k == "Call"
            && let Some(forbidden) = forbidden_calls
            && file.symbols.any(child(node, "func"), |n| {
                crate::workspace::matches_name(n, forbidden)
            })
        {
            let mut matched = BTreeSet::new();
            file.symbols.for_each(child(node, "func"), |n| {
                if crate::workspace::matches_name(n, forbidden) {
                    matched.insert(n.to_owned());
                }
            });
            if !matched.is_empty() {
                let text = matched.into_iter().collect::<Vec<_>>().join(", ");
                out.push(finding(
                    file,
                    node,
                    "LIT306",
                    format!("{} modules cannot call {text}", file.layer_name()),
                    Some(text),
                ));
            }
        }
    }

    // Share one parsed-annotations tree between boolean_policy (LIT401) and check_untyped_core (LIT209).
    let parsed_all = if inner {
        capabilities::parsed_all_annotations(file.tree, file.nodes, file.package)
    } else {
        None
    };

    {
        let _span = crate::profile::span("rule_simple");
        simple::check(file, policy, parsed_all.as_ref(), &mut out, &mut deferred);
    }
    {
        let _span = crate::profile::span("rule_defaults");
        defaults::check(file, &mut out);
    }
    {
        let _span = crate::profile::span("rule_ambient");
        ambient::check(file, &mut out);
    }
    {
        let _span = crate::profile::span("rule_mutation");
        mutation::check(file, &mut out);
    }
    {
        let _span = crate::profile::span("rule_capabilities");
        capabilities::check(file, policy, &mut out);
    }
    let (top_defs, classes) = {
        let _span = crate::profile::span("rule_subclass");
        subclass::extract_summary(file, policy)
    };
    {
        let _span = crate::profile::span("rule_strict");
        strict::check(file, policy, strict_mode, parsed_all.as_ref(), &mut out);
    }
    LocalCheckOutput {
        findings: out,
        deferred,
        top_defs,
        classes,
    }
}

pub fn check_cross_file(
    source_idx: usize,
    file: &Source,
    workspace: &Workspace,
    out: &mut Vec<Finding>,
) {
    simple::check_deferred(file, workspace, out);
    {
        let _span = crate::profile::span("rule_subclass");
        subclass::check(source_idx, file, workspace, out);
    }
}
