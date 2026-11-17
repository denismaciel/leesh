use super::*;
fn non_transactional(file: &Source, w: &Workspace, receiver: &Node) -> bool {
    let ns = names(file, receiver);
    !ns.is_empty()
        && !ns.contains("?")
        && ns.iter().all(|n| {
            let root = n.split('.').next().unwrap_or(n);
            !effects::TRANSACTION_PACKAGES.contains(&root) && !w.defines_package(root)
        })
}
pub(super) fn check(file: &Source, w: &Workspace, out: &mut Vec<Finding>) {
    let inner = file.has_layer(&["core", "port"]);
    let core = file.has_layer(&["core"]);
    for n in walk(&file.tree) {
        let k = kind(n);
        let f = child(n, "func");
        if core && ["Await", "AsyncFor", "AsyncWith"].contains(&k) {
            add(
                out,
                file,
                n,
                "LIT206",
                "async execution belongs in the shell; the core computes decisions from values",
            );
        }
        if core && loaded(file, n) {
            let ns = names(file, n);
            if !ns.is_empty() && ns.iter().all(|n| effects::asynchronous(n)) {
                add(
                    out,
                    file,
                    n,
                    "LIT206",
                    "task and thread scheduling belongs in the shell",
                );
            }
        }
        if !file.layer().is_empty()
            && !file.has_layer(&["transaction", "test"])
            && k == "Call"
            && kind(f) == "Attribute"
            && ["commit", "rollback"].contains(&s(f, "attr"))
            && !non_transactional(file, w, child(f, "value"))
        {
            add(
                out,
                file,
                n,
                "LIT302",
                format!(
                    "{}() is only allowed at transaction boundaries",
                    s(f, "attr")
                ),
            );
        }
        if file.has_layer(&["router"]) && k == "Call" {
            let ns = names(file, f);
            let mut short = ns
                .iter()
                .map(|n| n.rsplit('.').next().unwrap_or(n).to_owned())
                .collect::<BTreeSet<_>>();
            if short.is_empty() || ns == BTreeSet::from(["?".into()]) {
                short.clear();
                let v = if kind(f) == "Name" {
                    s(f, "id")
                } else if kind(f) == "Attribute" {
                    s(f, "attr")
                } else {
                    ""
                };
                if !v.is_empty() {
                    short.insert(v.into());
                }
            }
            if !short.is_empty()
                && short.iter().all(|n| {
                    ["Repository", "Client", "Engine"]
                        .iter()
                        .any(|v| n.ends_with(v))
                        || ["create_async_engine", "create_engine", "load_settings"]
                            .contains(&n.as_str())
                })
            {
                let text = joined(&short);
                out.push(finding(
                    file,
                    n,
                    "LIT303",
                    format!("routers cannot construct {text}"),
                    Some(text),
                ));
            }
        }
        if file.has_layer(&["test"]) {
            if k == "Import"
                && items(n, "names").iter().any(|a| {
                    s(a, "name") == "unittest.mock" || s(a, "name").starts_with("unittest.mock.")
                })
                || k == "ImportFrom"
                    && (s(n, "module") == "unittest.mock"
                        || s(n, "module") == "unittest"
                            && items(n, "names").iter().any(|a| s(a, "name") == "mock"))
            {
                add(
                    out,
                    file,
                    n,
                    "LIT304",
                    "do not import unittest.mock in tests",
                );
            }
            if ["FunctionDef", "AsyncFunctionDef"].contains(&k) {
                for a in parameters(n) {
                    if ["mocker", "class_mocker", "module_mocker", "package_mocker"]
                        .contains(&s(a, "arg"))
                    {
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
            if k == "Call"
                && kind(f) == "Attribute"
                && kind(child(f, "value")) == "Name"
                && s(child(f, "value"), "id") == "monkeypatch"
                && [
                    "chdir",
                    "context",
                    "delattr",
                    "delenv",
                    "delitem",
                    "setattr",
                    "setenv",
                    "setitem",
                    "syspath_prepend",
                ]
                .contains(&s(f, "attr"))
            {
                add(
                    out,
                    file,
                    n,
                    "LIT304",
                    "do not use pytest monkeypatch in tests",
                );
            }
        }
        if core
            && ["FunctionDef", "AsyncFunctionDef"].contains(&k)
            && !s(n, "name").starts_with('_')
        {
            for a in parameters(n) {
                if !["self", "cls"].contains(&s(a, "arg")) && has_bool(child(a, "annotation")) {
                    add(
                        out,
                        file,
                        a,
                        "LIT401",
                        format!(
                            "policy input '{}' is boolean; use a named enum or value type",
                            s(a, "arg")
                        ),
                    );
                }
            }
            if has_bool(child(n, "returns"))
                && !w
                    .policy
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
        if core && ["Raise", "ExceptHandler"].contains(&k) {
            let e = child(n, if k == "Raise" { "exc" } else { "type" });
            if exception_names(e)
                .iter()
                .any(|n| w.policy.outcome_suffixes.iter().any(|v| n.ends_with(v)))
            {
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
    if inner {
        frozen_values(file, out);
    }
}
fn has_bool(n: &Node) -> bool {
    match kind(n) {
        "Name" => s(n, "id") == "bool",
        "BinOp" if kind(child(n, "op")) == "BitOr" => {
            has_bool(child(n, "left")) || has_bool(child(n, "right"))
        }
        "Subscript" => {
            let v = child(n, "value");
            if ["Union", "Optional"].contains(&if kind(v) == "Name" {
                s(v, "id")
            } else {
                s(v, "attr")
            }) {
                let sl = child(n, "slice");
                if kind(sl) == "Tuple" {
                    items(sl, "elts").iter().any(has_bool)
                } else {
                    has_bool(sl)
                }
            } else {
                false
            }
        }
        _ => false,
    }
}
fn exception_names(n: &Node) -> Vec<&str> {
    match kind(n) {
        "Tuple" => items(n, "elts").iter().flat_map(exception_names).collect(),
        "Call" => exception_names(child(n, "func")),
        "Name" => vec![s(n, "id")],
        "Attribute" => vec![s(n, "attr")],
        _ => vec![],
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
fn mutable(syms: &symbols::Symbols, n: &Node) -> bool {
    symbols::names(syms, n)
        .iter()
        .any(|v| MUTABLE_TYPES.contains(&v.as_str()))
}
fn only(syms: &symbols::Symbols, n: &Node, name: &str) -> bool {
    let ns = symbols::names(syms, n);
    ns.len() == 1 && ns.contains(name)
}
/// `field(default_factory=list)` builds a mutable value whatever the annotation says.
fn mutable_factory(syms: &symbols::Symbols, v: &Node) -> bool {
    kind(v) == "Call"
        && only(syms, child(v, "func"), "dataclasses.field")
        && items(v, "keywords").iter().any(|kw| {
            let f = child(kw, "value");
            s(kw, "arg") == "default_factory"
                && (mutable(syms, f)
                    || kind(f) == "Lambda"
                        && ["List", "Dict", "Set", "ListComp", "DictComp", "SetComp"]
                            .contains(&kind(child(f, "body"))))
        })
}
fn frozen_values(file: &Source, out: &mut Vec<Finding>) {
    let parsed = capabilities::parsed_annotations(file, "AnnAssign");
    let (tree, syms) = match &parsed {
        Some((tree, syms)) => (tree, syms),
        None => (&file.tree, &file.symbols),
    };
    let nodes = walk(tree);
    let initializing = nodes
        .iter()
        .filter(|n| kind(n) == "FunctionDef" && s(n, "name") == "__post_init__")
        .flat_map(|n| walk(n))
        .map(ast::id)
        .collect::<HashSet<_>>();
    for n in nodes {
        if kind(n) == "Call"
            && !initializing.contains(&ast::id(n))
            && (only(syms, child(n, "func"), "builtins.object.__setattr__")
                || only(syms, child(n, "func"), "builtins.object.__delattr__"))
            && items(n, "args")
                .first()
                .is_some_and(|a| kind(a) == "Name" && s(a, "id") == "self")
        {
            add(
                out,
                file,
                n,
                "LIT205",
                "object.__setattr__ writes to a frozen record; return a replacement value instead",
            );
        }
        if kind(n) == "ClassDef" {
            frozen(file, syms, n, out);
        }
    }
}
fn frozen(file: &Source, syms: &symbols::Symbols, n: &Node, out: &mut Vec<Finding>) {
    let ds = items(n, "decorator_list")
        .iter()
        .filter(|d| {
            only(
                syms,
                if kind(d) == "Call" {
                    child(d, "func")
                } else {
                    d
                },
                "dataclasses.dataclass",
            )
        })
        .collect::<Vec<_>>();
    let named_tuple = items(n, "bases")
        .iter()
        .any(|b| only(syms, b, "typing.NamedTuple"));
    if ds.is_empty() && !named_tuple {
        return;
    }
    if !ds.is_empty()
        && !ds.iter().any(|d| {
            kind(d) == "Call"
                && items(d, "keywords").iter().any(|kw| {
                    s(kw, "arg") == "frozen"
                        && kind(child(kw, "value")) == "Constant"
                        && child(kw, "value").get("value").and_then(Node::as_bool) == Some(true)
                })
        })
    {
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
        if kind(a) == "Subscript" && only(syms, child(a, "value"), "typing.ClassVar") {
            continue;
        }
        if capabilities::annotation_nodes(a, syms)
            .iter()
            .any(|a| mutable(syms, a))
            || mutable_factory(syms, child(st, "value"))
        {
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
