use super::*;

const PATCHING_PACKAGES: &[&str] = &[
    "freezegun",
    "httpretty",
    "mock",
    "responses",
    "respx",
    "time_machine",
];

fn is_io_call(syms: &symbols::Symbols, f: &Node) -> bool {
    !syms.is_unknown(f)
        && syms.all(f, |n| {
            let mut parts = n.split('.');
            let Some(first) = parts.next() else {
                return false;
            };
            if !effects::IO_PACKAGES.contains(&first) {
                return false;
            }
            let Some(second) = parts.next() else {
                return false;
            };
            let mut last = second;
            let mut count = 2usize;
            for part in parts {
                last = part;
                count += 1;
            }
            count == 2 || last.chars().next().is_some_and(char::is_uppercase)
        })
}

fn could_be_adapter_construction(
    file: &FileContext<'_>,
    syms: &symbols::Symbols,
    f: &Node,
) -> bool {
    if syms.is_unknown(f) || file.layer_name().is_empty() {
        return false;
    }
    syms.all(f, |name| {
        if name == "?" {
            return false;
        }
        let short = name.rsplit('.').next().unwrap_or(name);
        if short.ends_with("Error") || short.ends_with("Exception") {
            return false;
        }
        name.contains('.')
    })
}

pub(super) fn check(
    file: &FileContext<'_>,
    policy: &Policy,
    parsed_all: Option<&(Node, symbols::Symbols)>,
    out: &mut Vec<Finding>,
    deferred: &mut DeferredLocal,
) {
    let inner = file.has_layer(&["core", "port"]);
    let core = file.has_layer(&["core"]);
    let is_test = file.has_layer(&["test"]);
    let check_commit = !file.layer().is_empty() && !file.has_layer(&["transaction", "test"]);
    let check_router_shell = file.has_layer(&["router", "shell"]);

    // For test files, separate unconditional `Import` prefixes from `ImportFrom` candidates
    // (which require `w.module_source(&qualified).is_some()` in the cross-file pass).
    let mut import_prefixes: HashSet<String> = HashSet::new();
    let mut from_candidates: HashSet<String> = HashSet::new();
    if is_test {
        for &node in file.nodes {
            let k = kind(node);
            if k == "Import" {
                for alias in items(node, "names") {
                    let full = s(alias, "name");
                    if full.is_empty() {
                        continue;
                    }
                    import_prefixes.insert(full.to_owned());
                    for (idx, _) in full.match_indices('.') {
                        if idx > 0 {
                            import_prefixes.insert(full[..idx].to_owned());
                        }
                    }
                }
            } else if k == "ImportFrom"
                && let Some(base) = symbols::absolute_module(node, file.package)
            {
                for alias in items(node, "names") {
                    let sym = s(alias, "name");
                    if sym != "*" {
                        from_candidates.insert(format!("{base}.{sym}"));
                    }
                }
            }
        }
    }

    let subject = if file.has_layer(&["router"]) {
        "routers"
    } else {
        file.layer_name()
    };

    for &n in file.nodes {
        let k = kind(n);
        if core && matches!(k, "Await" | "AsyncFor" | "AsyncWith") {
            add(
                out,
                file,
                n,
                "LIT206",
                "async execution belongs in the shell; the core computes decisions from values",
            );
        }
        if core
            && loaded(file, n)
            && !file.symbols.is_unknown(n)
            && file.symbols.all(n, effects::asynchronous)
        {
            add(
                out,
                file,
                n,
                "LIT206",
                "task and thread scheduling belongs in the shell",
            );
        }
        if check_commit && k == "Call" {
            let f = child(n, "func");
            if kind(f) == "Attribute" {
                let attr = s(f, "attr");
                if attr == "commit" || attr == "rollback" {
                    let receiver = child(f, "value");
                    // non_transactional is true iff resolved names are non-empty, no "?",
                    // and every root is neither in TRANSACTION_PACKAGES nor defines_package.
                    if file.symbols.is_unknown(receiver)
                        || file.symbols.any(receiver, |name| {
                            name == "?"
                                || effects::TRANSACTION_PACKAGES
                                    .contains(&name.split('.').next().unwrap_or(name))
                        })
                    {
                        add(
                            out,
                            file,
                            n,
                            "LIT302",
                            format!("{attr}() is only allowed at transaction boundaries"),
                        );
                    } else {
                        let mut roots: Vec<Box<str>> = Vec::new();
                        file.symbols.for_each(receiver, |name| {
                            let root = name.split('.').next().unwrap_or(name);
                            if !roots.iter().any(|r| &**r == root) {
                                roots.push(root.into());
                            }
                        });
                        let (line, col) = n.position().unwrap_or((1, 0));
                        deferred.lit302.push((
                            line as u32,
                            col as u32,
                            attr.into(),
                            roots.into_boxed_slice(),
                        ));
                    }
                }
            }
        }
        if check_router_shell && k == "Call" {
            let f = child(n, "func");
            let is_bad_short = |short: &str| {
                short.ends_with("Repository")
                    || short.ends_with("Client")
                    || short.ends_with("Engine")
                    || matches!(
                        short,
                        "create_async_engine" | "create_engine" | "load_settings"
                    )
            };
            let matched_short: Option<String> = if file.symbols.is_unknown(f) {
                let v = if kind(f) == "Name" {
                    s(f, "id")
                } else if kind(f) == "Attribute" {
                    s(f, "attr")
                } else {
                    ""
                };
                if !v.is_empty() && is_bad_short(v) {
                    Some(v.to_owned())
                } else {
                    None
                }
            } else if file.symbols.all(f, |name| {
                is_bad_short(name.rsplit('.').next().unwrap_or(name))
            }) {
                let mut shorts = BTreeSet::new();
                file.symbols.for_each(f, |name| {
                    shorts.insert(name.rsplit('.').next().unwrap_or(name).to_owned());
                });
                Some(shorts.into_iter().collect::<Vec<_>>().join(", "))
            } else {
                None
            };

            if let Some(text) = matched_short {
                out.push(finding(
                    file,
                    n,
                    "LIT303",
                    format!("{subject} cannot construct {text}"),
                    Some(text),
                ));
            } else if is_io_call(file.symbols, f) {
                let text = file.symbols.joined(f);
                out.push(finding(
                    file,
                    n,
                    "LIT303",
                    format!("{subject} cannot construct {text}"),
                    Some(text),
                ));
            } else if could_be_adapter_construction(file, file.symbols, f) {
                let (line, col) = n.position().unwrap_or((1, 0));
                deferred
                    .lit303
                    .push((line as u32, col as u32, file.symbols.resolved_box(f)));
            }
        }
        if is_test {
            if k == "Import" {
                if items(n, "names").iter().any(|a| {
                    let name = s(a, "name");
                    name == "unittest.mock" || name.starts_with("unittest.mock.")
                }) {
                    add(
                        out,
                        file,
                        n,
                        "LIT304",
                        "do not import unittest.mock in tests",
                    );
                }
                for a in items(n, "names") {
                    let root = s(a, "name").split('.').next().unwrap_or("");
                    if PATCHING_PACKAGES.contains(&root) {
                        add(
                            out,
                            file,
                            n,
                            "LIT304",
                            format!("do not import {root} in tests"),
                        );
                    }
                }
            } else if k == "ImportFrom" {
                let m = s(n, "module");
                let root = m.split('.').next().unwrap_or("");
                if m == "unittest.mock"
                    || (m == "unittest" && items(n, "names").iter().any(|a| s(a, "name") == "mock"))
                {
                    add(
                        out,
                        file,
                        n,
                        "LIT304",
                        "do not import unittest.mock in tests",
                    );
                } else if PATCHING_PACKAGES.contains(&root) {
                    add(
                        out,
                        file,
                        n,
                        "LIT304",
                        format!("do not import {root} in tests"),
                    );
                } else if matches!(m, "pytest" | "_pytest.monkeypatch")
                    && items(n, "names")
                        .iter()
                        .any(|a| s(a, "name") == "MonkeyPatch")
                {
                    add(
                        out,
                        file,
                        n,
                        "LIT304",
                        "do not use pytest monkeypatch in tests",
                    );
                }
            } else if matches!(k, "FunctionDef" | "AsyncFunctionDef") {
                for a in parameters(n) {
                    if matches!(
                        s(a, "arg"),
                        "mocker" | "class_mocker" | "module_mocker" | "package_mocker"
                    ) {
                        add(
                            out,
                            file,
                            a,
                            "LIT304",
                            "do not use pytest-mock fixtures in tests",
                        );
                    }
                }
            }
            let check_module_expr =
                |expr: &Node, out: &mut Vec<Finding>, deferred: &mut DeferredLocal| {
                    if file.symbols.is_unknown(expr) {
                        return;
                    }
                    if file.symbols.all(expr, |v| import_prefixes.contains(v)) {
                        add(
                            out,
                            file,
                            n,
                            "LIT304",
                            "do not patch module attributes in tests; pass fakes through ports",
                        );
                    } else if !from_candidates.is_empty()
                        && file.symbols.all(expr, |v| {
                            import_prefixes.contains(v) || from_candidates.contains(v)
                        })
                    {
                        let mut needed = Vec::new();
                        file.symbols.for_each(expr, |v| {
                            if !import_prefixes.contains(v) {
                                needed.push(v.into());
                            }
                        });
                        let (line, col) = n.position().unwrap_or((1, 0));
                        deferred
                            .lit304
                            .push((line as u32, col as u32, needed.into_boxed_slice()));
                    }
                };
            if k == "Assign" || k == "Delete" {
                for target in items(n, "targets") {
                    if kind(target) == "Attribute" {
                        check_module_expr(child(target, "value"), out, deferred);
                    }
                }
            } else if k == "AnnAssign" || k == "AugAssign" {
                let target = child(n, "target");
                if kind(target) == "Attribute" {
                    check_module_expr(child(target, "value"), out, deferred);
                }
            } else if k == "Call" {
                let f = child(n, "func");
                let is_monkeypatch = (kind(f) == "Attribute"
                    && kind(child(f, "value")) == "Name"
                    && s(child(f, "value"), "id") == "monkeypatch"
                    && matches!(
                        s(f, "attr"),
                        "chdir"
                            | "context"
                            | "delattr"
                            | "delenv"
                            | "delitem"
                            | "setattr"
                            | "setenv"
                            | "setitem"
                            | "syspath_prepend"
                    ))
                    || (!file.symbols.is_unknown(f)
                        && file.symbols.all(f, |v| {
                            v == "pytest.MonkeyPatch"
                                || v == "_pytest.monkeypatch.MonkeyPatch"
                                || v.starts_with("pytest.MonkeyPatch.")
                                || v.starts_with("_pytest.monkeypatch.MonkeyPatch.")
                        }));
                if is_monkeypatch {
                    add(
                        out,
                        file,
                        n,
                        "LIT304",
                        "do not use pytest monkeypatch in tests",
                    );
                } else if (file.symbols.only(f, "builtins.setattr")
                    || file.symbols.only(f, "builtins.delattr"))
                    && let Some(first) = items(n, "args").first()
                {
                    check_module_expr(first, out, deferred);
                }
            }
        }
        if core && matches!(k, "Raise" | "ExceptHandler") {
            let e = child(n, if k == "Raise" { "exc" } else { "type" });
            if has_outcome_exception(e, &policy.outcome_suffixes) {
                add(
                    out,
                    file,
                    n,
                    "LIT402",
                    if k == "Raise" {
                        "expected outcomes must be returned as named results, not raised"
                    } else {
                        "expected outcomes must be matched as named results, not caught"
                    },
                );
            }
        }
    }
    if core {
        boolean_policy(file, policy, parsed_all, out);
    }
    if inner {
        frozen_values(file, out);
    }
}

pub(super) fn check_deferred(file: &Source, w: &Workspace, out: &mut Vec<Finding>) {
    let d = &file.deferred_local;
    for (line, col, attr, roots) in &d.lit302 {
        if roots.iter().any(|r| w.defines_package(r)) {
            out.push(finding_at(
                &file.filename,
                *line as usize,
                *col as usize,
                "LIT302",
                format!("{attr}() is only allowed at transaction boundaries"),
                None,
            ));
        }
    }
    if !d.lit303.is_empty() {
        let subject = if file.has_layer(&["router"]) {
            "routers"
        } else {
            file.layer_name()
        };
        for (line, col, resolved) in &d.lit303 {
            let is_adapter = resolved.iter().all(|name| {
                let Some((module, _)) = name.rsplit_once('.') else {
                    return false;
                };
                let Some(source) = w.module_source(module) else {
                    return false;
                };
                source.layer() == "adapter"
                    && !w.policy.allows_dependency(file.layer_name(), module)
            });
            if is_adapter {
                let text = resolved.join(", ");
                out.push(finding_at(
                    &file.filename,
                    *line as usize,
                    *col as usize,
                    "LIT303",
                    format!("{subject} cannot construct {text}"),
                    Some(text),
                ));
            }
        }
    }
    for (line, col, needed) in &d.lit304 {
        if needed.iter().all(|m| w.module_source(m).is_some()) {
            out.push(finding_at(
                &file.filename,
                *line as usize,
                *col as usize,
                "LIT304",
                "do not patch module attributes in tests; pass fakes through ports",
                None,
            ));
        }
    }
}

fn boolean_policy(
    file: &FileContext<'_>,
    policy: &Policy,
    parsed_all: Option<&(Node, symbols::Symbols)>,
    out: &mut Vec<Finding>,
) {
    let owned_nodes;
    let (nodes, syms): (&[&Node], &symbols::Symbols) = match parsed_all {
        Some((tree, syms)) => {
            owned_nodes = walk(tree);
            (&owned_nodes, syms)
        }
        None => (file.nodes, file.symbols),
    };
    for &n in nodes {
        let k = kind(n);
        if (k != "FunctionDef" && k != "AsyncFunctionDef") || s(n, "name").starts_with('_') {
            continue;
        }
        for a in parameters(n) {
            let arg_name = s(a, "arg");
            if arg_name != "self" && arg_name != "cls" && has_bool(child(a, "annotation"), syms) {
                add(
                    out,
                    file,
                    a,
                    "LIT401",
                    format!("policy input '{arg_name}' is boolean; use a named enum or value type"),
                );
            }
        }
        if has_bool(child(n, "returns"), syms)
            && !policy
                .predicate_prefixes
                .iter()
                .any(|p| s(n, "name").starts_with(p))
        {
            add(
                out,
                file,
                n,
                "LIT401",
                format!(
                    "decision '{}' returns bool; return a named result or enum",
                    s(n, "name")
                ),
            );
        }
    }
}

fn has_bool(n: &Node, syms: &symbols::Symbols) -> bool {
    if n.is_null() {
        return false;
    }
    match kind(n) {
        "BinOp" if kind(child(n, "op")) == "BitOr" => {
            has_bool(child(n, "left"), syms) || has_bool(child(n, "right"), syms)
        }
        "Subscript" => {
            let v = child(n, "value");
            let ident = if kind(v) == "Name" {
                s(v, "id")
            } else {
                s(v, "attr")
            };
            if ident == "Annotated"
                || syms.any(v, |r| {
                    r == "typing.Annotated" || r == "typing_extensions.Annotated"
                })
            {
                let sl = child(n, "slice");
                let first = if kind(sl) == "Tuple" {
                    items(sl, "elts").first().unwrap_or(sl)
                } else {
                    sl
                };
                return has_bool(first, syms);
            }
            if matches!(ident, "Union" | "Optional")
                || syms.any(v, |r| {
                    matches!(
                        r,
                        "typing.Union"
                            | "typing.Optional"
                            | "typing_extensions.Union"
                            | "typing_extensions.Optional"
                    )
                })
            {
                let sl = child(n, "slice");
                if kind(sl) == "Tuple" {
                    items(sl, "elts").iter().any(|e| has_bool(e, syms))
                } else {
                    has_bool(sl, syms)
                }
            } else {
                false
            }
        }
        _ => {
            if syms.contains(n, "builtins.bool") {
                return true;
            }
            kind(n) == "Name" && s(n, "id") == "bool"
        }
    }
}

pub(super) fn has_outcome_exception(n: &Node, suffixes: &[String]) -> bool {
    match kind(n) {
        "Tuple" => items(n, "elts")
            .iter()
            .any(|e| has_outcome_exception(e, suffixes)),
        "Call" => has_outcome_exception(child(n, "func"), suffixes),
        "Name" => {
            let name = s(n, "id");
            suffixes.iter().any(|v| name.ends_with(v))
        }
        "Attribute" => {
            let name = s(n, "attr");
            suffixes.iter().any(|v| name.ends_with(v))
        }
        _ => false,
    }
}

pub(super) fn has_exempt_exception(n: &Node, suffixes: &[String]) -> bool {
    match kind(n) {
        "Tuple" => items(n, "elts")
            .iter()
            .any(|e| has_exempt_exception(e, suffixes)),
        "Call" => has_exempt_exception(child(n, "func"), suffixes),
        "Name" => {
            let name = s(n, "id");
            matches!(name, "AssertionError" | "NotImplementedError")
                || suffixes.iter().any(|v| name.ends_with(v))
        }
        "Attribute" => {
            let name = s(n, "attr");
            matches!(name, "AssertionError" | "NotImplementedError")
                || suffixes.iter().any(|v| name.ends_with(v))
        }
        _ => false,
    }
}

const MUTABLE_TYPES: &[&str] = &[
    "builtins.bytearray",
    "builtins.dict",
    "builtins.list",
    "builtins.set",
    "collections.ChainMap",
    "collections.Counter",
    "collections.OrderedDict",
    "collections.abc.MutableMapping",
    "collections.abc.MutableSequence",
    "collections.abc.MutableSet",
    "collections.defaultdict",
    "collections.deque",
    "typing.Counter",
    "typing.DefaultDict",
    "typing.Deque",
    "typing.Dict",
    "typing.List",
    "typing.MutableMapping",
    "typing.MutableSequence",
    "typing.MutableSet",
    "typing.OrderedDict",
    "typing.Set",
];

#[inline]
fn mutable(syms: &symbols::Symbols, n: &Node) -> bool {
    syms.any(n, |v| MUTABLE_TYPES.contains(&v))
}

fn mutable_factory(syms: &symbols::Symbols, v: &Node) -> bool {
    kind(v) == "Call"
        && syms.only(child(v, "func"), "dataclasses.field")
        && items(v, "keywords").iter().any(|kw| {
            let f = child(kw, "value");
            s(kw, "arg") == "default_factory"
                && (mutable(syms, f)
                    || (kind(f) == "Lambda"
                        && matches!(
                            kind(child(f, "body")),
                            "List" | "Dict" | "Set" | "ListComp" | "DictComp" | "SetComp"
                        )))
        })
}

fn collect_ids(n: &Node, out: &mut HashSet<usize>) {
    out.insert(ast::id(n));
    for_each_child(n, |c| collect_ids(c, out));
}

fn frozen_values(file: &FileContext<'_>, out: &mut Vec<Finding>) {
    let parsed = capabilities::parsed_annotations(file.tree, file.nodes, file.package, "AnnAssign");
    let owned_nodes;
    let (nodes, syms): (&[&Node], &symbols::Symbols) = match &parsed {
        Some((tree, syms)) => {
            owned_nodes = walk(tree);
            (&owned_nodes, syms)
        }
        None => (file.nodes, file.symbols),
    };
    let mut initializing: Option<HashSet<usize>> = None;
    for &n in nodes {
        let k = kind(n);
        if k == "Call" {
            let func = child(n, "func");
            let is_setattr = syms.only(func, "builtins.object.__setattr__");
            if is_setattr || syms.only(func, "builtins.object.__delattr__") {
                let init_set = initializing.get_or_insert_with(|| {
                    let mut set = HashSet::new();
                    for &m in nodes {
                        if kind(m) == "FunctionDef" && s(m, "name") == "__post_init__" {
                            collect_ids(m, &mut set);
                        }
                    }
                    set
                });
                let is_self_init = is_setattr
                    && init_set.contains(&ast::id(n))
                    && items(n, "args")
                        .first()
                        .is_some_and(|a| kind(a) == "Name" && s(a, "id") == "self");
                if !is_self_init {
                    add(
                        out,
                        file,
                        n,
                        "LIT205",
                        "object.__setattr__ writes to a frozen record; return a replacement value instead",
                    );
                }
            }
        } else if k == "ClassDef" {
            frozen(file, syms, n, out);
        }
    }
}

fn frozen(file: &FileContext<'_>, syms: &symbols::Symbols, n: &Node, out: &mut Vec<Finding>) {
    let mut has_dataclass = false;
    let mut is_frozen = false;
    for d in items(n, "decorator_list") {
        let target = if kind(d) == "Call" {
            child(d, "func")
        } else {
            d
        };
        if syms.only(target, "dataclasses.dataclass") {
            has_dataclass = true;
            if kind(d) == "Call"
                && items(d, "keywords").iter().any(|kw| {
                    s(kw, "arg") == "frozen"
                        && kind(child(kw, "value")) == "Constant"
                        && child(kw, "value").get("value").and_then(Node::as_bool) == Some(true)
                })
            {
                is_frozen = true;
            }
        }
    }
    let named_tuple = items(n, "bases")
        .iter()
        .any(|b| syms.only(b, "typing.NamedTuple"));
    if !has_dataclass && !named_tuple {
        return;
    }
    if has_dataclass && !is_frozen {
        add(
            out,
            file,
            n,
            "LIT205",
            "core dataclasses must be frozen; return a replacement value for transitions",
        );
    }
    for st in items(n, "body") {
        if kind(st) != "AnnAssign" {
            continue;
        }
        let a = child(st, "annotation");
        if kind(a) == "Subscript" && syms.only(child(a, "value"), "typing.ClassVar") {
            continue;
        }
        let mut has_mut = false;
        capabilities::visit_annotation_nodes(a, syms, &mut |node| {
            if !has_mut && mutable(syms, node) {
                has_mut = true;
            }
        });
        if has_mut || mutable_factory(syms, child(st, "value")) {
            add(
                out,
                file,
                st,
                "LIT205",
                "frozen records must not expose mutable collection fields; use tuples, frozensets, or a read-only collection interface",
            );
        }
    }
}
