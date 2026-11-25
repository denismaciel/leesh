use super::*;

const UNPARAMETERIZED: &[&str] = &[
    "builtins.dict",
    "builtins.frozenset",
    "builtins.list",
    "builtins.set",
    "builtins.tuple",
    "collections.abc.Mapping",
    "collections.abc.MutableMapping",
    "collections.abc.MutableSequence",
    "collections.abc.Sequence",
    "typing.Dict",
    "typing.FrozenSet",
    "typing.List",
    "typing.Mapping",
    "typing.Sequence",
    "typing.Set",
    "typing.Tuple",
];
const PLAIN_BASES: &[&str] = &[
    "builtins.object",
    "typing.Generic",
    "typing_extensions.Generic",
];
const EXEMPT_CLASS_BASES: &[&str] = &[
    "enum.Enum",
    "enum.Flag",
    "enum.IntEnum",
    "enum.IntFlag",
    "enum.StrEnum",
    "typing.NamedTuple",
    "typing.Protocol",
    "typing.TypedDict",
    "typing_extensions.NamedTuple",
    "typing_extensions.Protocol",
    "typing_extensions.TypedDict",
];
const ALLOWED_METHOD_DECORATORS: &[&str] = &[
    "builtins.classmethod",
    "builtins.property",
    "builtins.staticmethod",
    "functools.cached_property",
];

fn collect_ids(n: &Node, out: &mut HashSet<usize>) {
    out.insert(ast::id(n));
    for_each_child(n, |c| collect_ids(c, out));
}

pub(super) fn check(
    file: &FileContext<'_>,
    policy: &Policy,
    strict_mode: bool,
    parsed_all: Option<&(Node, symbols::Symbols)>,
    out: &mut Vec<Finding>,
) {
    if file.layer().is_empty() {
        return;
    }
    let inner = file.has_layer(&["core", "port"]);
    let core = file.has_layer(&["core"]);

    // LIT106: deferred-import
    let check_local = !file.has_layer(&["boundary", "test", "marker"]);
    if check_local || inner {
        visit_deferred(file, file.tree, check_local, inner, 0, out);
    }

    // LIT209: untyped-core
    if inner {
        check_untyped_core(file, parsed_all, out);
    }

    let check_broad_except = !file.has_layer(&["boundary", "router", "test", "marker"]);
    let check_duck_typing = file.has_layer(&["core", "shell"]);
    let check_test_sleep = file.has_layer(&["test"]);
    let check_core_raises = strict_mode && core;

    let mut post_init: Option<HashSet<usize>> = None;

    // Single pass over file.nodes for LIT307, LIT308, LIT309(sleep), LIT404, LIT405.
    if check_broad_except || check_duck_typing || check_test_sleep || core {
        for &n in file.nodes {
            let k = kind(n);

            // LIT307: broad-except
            if check_broad_except
                && k == "ExceptHandler"
                && is_broad_except(child(n, "type"), file.symbols)
                && !ends_with_raise(items(n, "body"))
            {
                add(
                    out,
                    file,
                    n,
                    "LIT307",
                    "broad exception catch swallows failures; catch specific exceptions or handle at a boundary",
                );
            }

            // LIT308: defensive-duck-typing
            if check_duck_typing && k == "Call" {
                let f = child(n, "func");
                if file.symbols.only(f, "builtins.hasattr") {
                    out.push(finding(
                        file,
                        n,
                        "LIT308",
                        "hasattr hides duck-typing; narrow with typed dataclasses or Protocols",
                        Some("builtins.hasattr".into()),
                    ));
                } else if file.symbols.only(f, "builtins.getattr") && items(n, "args").len() >= 3 {
                    let literal_name = items(n, "args")
                        .get(1)
                        .is_some_and(|a| kind(a) == "Constant" && a["value"].is_string());
                    if !(core && !literal_name) {
                        out.push(finding(
                            file,
                            n,
                            "LIT308",
                            "getattr with a default hides missing attributes; use typed dataclasses",
                            Some("builtins.getattr".into()),
                        ));
                    }
                } else if core
                    && !file.symbols.is_unknown(f)
                    && file
                        .symbols
                        .all(f, |v| v == "typing.cast" || v == "typing_extensions.cast")
                {
                    let text = file.symbols.joined(f);
                    out.push(finding(
                        file,
                        n,
                        "LIT308",
                        "typing.cast bypasses type safety; parse or narrow values explicitly",
                        Some(text),
                    ));
                }
            }

            // LIT309: test-hygiene (time.sleep part)
            if check_test_sleep && loaded(file, n) && file.symbols.only(n, "time.sleep") {
                out.push(finding(
                    file,
                    n,
                    "LIT309",
                    "time.sleep makes tests slow and flaky; inject a clock or synchronize explicitly",
                    Some("time.sleep".into()),
                ));
            }

            // LIT404: core-classes
            if core && k == "ClassDef" {
                if is_plain_class(n, file.symbols) {
                    add(
                        out,
                        file,
                        n,
                        "LIT404",
                        format!(
                            "'{}' is a plain class in core; use frozen dataclasses and module-level functions",
                            s(n, "name")
                        ),
                    );
                } else if strict_mode
                    && (is_dataclass(n, file.symbols) || is_named_tuple(n, file.symbols))
                {
                    for stmt in items(n, "body") {
                        if matches!(kind(stmt), "FunctionDef" | "AsyncFunctionDef")
                            && !allowed_method(stmt, file.symbols)
                        {
                            add(
                                out,
                                file,
                                stmt,
                                "LIT404",
                                format!(
                                    "method '{}.{}' belongs as a module-level function taking {}",
                                    s(n, "name"),
                                    s(stmt, "name"),
                                    s(n, "name")
                                ),
                            );
                        }
                    }
                }
            }

            // LIT405: core-exceptions (opt-in via workspace.strict)
            if check_core_raises && k == "Raise" {
                let exc = child(n, "exc");
                if simple::has_exempt_exception(exc, &policy.outcome_suffixes) {
                    continue;
                }
                let post_init_set = post_init.get_or_insert_with(|| {
                    let mut set = HashSet::new();
                    for &m in file.nodes {
                        if kind(m) == "ClassDef" {
                            for stmt in items(m, "body") {
                                if kind(stmt) == "FunctionDef" && s(stmt, "name") == "__post_init__"
                                {
                                    collect_ids(stmt, &mut set);
                                }
                            }
                        }
                    }
                    set
                });
                if !post_init_set.contains(&ast::id(n)) {
                    add(
                        out,
                        file,
                        n,
                        "LIT405",
                        "core code should return named results instead of raising exceptions",
                    );
                }
            }
        }
    }

    // LIT406: explicit-arguments (opt-in via workspace.strict)
    if strict_mode && inner {
        visit_explicit(file, file.tree, false, false, out);
    }
}

fn visit_deferred(
    file: &FileContext<'_>,
    n: &Node,
    check_local: bool,
    check_tc: bool,
    depth: usize,
    out: &mut Vec<Finding>,
) {
    let k = kind(n);
    match k {
        "Import" | "ImportFrom" => {
            if check_local && depth > 0 {
                add(
                    out,
                    file,
                    n,
                    "LIT106",
                    "imports belong at module top level; local imports hide dependencies or cycles",
                );
            }
        }
        "If" => {
            if check_tc && is_type_checking(file, child(n, "test")) {
                add(
                    out,
                    file,
                    n,
                    "LIT106",
                    "TYPE_CHECKING blocks hide import cycles or layer leaks; restructure types instead",
                );
            }
            for stmt in items(n, "body") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
            for stmt in items(n, "orelse") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
        }
        "FunctionDef" | "AsyncFunctionDef" => {
            for stmt in items(n, "body") {
                visit_deferred(file, stmt, check_local, check_tc, depth + 1, out);
            }
        }
        "Module" | "ClassDef" | "With" | "AsyncWith" => {
            for stmt in items(n, "body") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
        }
        "For" | "AsyncFor" | "While" => {
            for stmt in items(n, "body") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
            for stmt in items(n, "orelse") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
        }
        "Try" | "TryStar" => {
            for stmt in items(n, "body") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
            for h in items(n, "handlers") {
                for stmt in items(h, "body") {
                    visit_deferred(file, stmt, check_local, check_tc, depth, out);
                }
            }
            for stmt in items(n, "orelse") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
            for stmt in items(n, "finalbody") {
                visit_deferred(file, stmt, check_local, check_tc, depth, out);
            }
        }
        "Match" => {
            for c in items(n, "cases") {
                for stmt in items(c, "body") {
                    visit_deferred(file, stmt, check_local, check_tc, depth, out);
                }
            }
        }
        _ => {}
    }
}

fn is_type_checking(file: &FileContext<'_>, test: &Node) -> bool {
    if !file.symbols.is_unknown(test)
        && file.symbols.all(test, |v| {
            v == "typing.TYPE_CHECKING" || v == "typing_extensions.TYPE_CHECKING"
        })
    {
        return true;
    }
    kind(test) == "Name" && s(test, "id") == "TYPE_CHECKING"
}

fn parameterized_aliases(tree: &Node) -> HashSet<&str> {
    let mut aliases = HashSet::new();
    let body = items(tree, "body");
    for stmt in body {
        let k = kind(stmt);
        if k == "Assign" && kind(child(stmt, "value")) == "Subscript" {
            for target in items(stmt, "targets") {
                if kind(target) == "Name" {
                    aliases.insert(s(target, "id"));
                }
            }
        } else if k == "AnnAssign"
            && kind(child(stmt, "target")) == "Name"
            && kind(child(stmt, "value")) == "Subscript"
        {
            aliases.insert(s(child(stmt, "target"), "id"));
        }
    }
    if aliases.is_empty() {
        return aliases;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in body {
            let k = kind(stmt);
            let value = child(stmt, "value");
            if k == "Assign" && kind(value) == "Name" && aliases.contains(s(value, "id")) {
                for target in items(stmt, "targets") {
                    if kind(target) == "Name" && aliases.insert(s(target, "id")) {
                        changed = true;
                    }
                }
            } else if k == "AnnAssign"
                && kind(child(stmt, "target")) == "Name"
                && kind(value) == "Name"
                && aliases.contains(s(value, "id"))
                && aliases.insert(s(child(stmt, "target"), "id"))
            {
                changed = true;
            }
        }
    }
    aliases
}

fn scan_annotation(
    node: &Node,
    syms: &symbols::Symbols,
    param_aliases: &HashSet<&str>,
) -> (bool, bool) {
    if node.is_null() {
        return (false, false);
    }
    if kind(node) == "Subscript" {
        let val = child(node, "value");
        if syms.any(val, |v| {
            v == "typing.Literal" || v == "typing_extensions.Literal"
        }) {
            return (false, false);
        }
        if syms.any(val, |v| {
            v == "typing.Annotated" || v == "typing_extensions.Annotated"
        }) {
            let sl = child(node, "slice");
            let first = if kind(sl) == "Tuple" {
                items(sl, "elts").first().unwrap_or(sl)
            } else {
                sl
            };
            return scan_annotation(first, syms, param_aliases);
        }
        return scan_annotation(child(node, "slice"), syms, param_aliases);
    }
    let mut has_any = syms.any(node, |v| v == "typing.Any" || v == "typing_extensions.Any");
    let is_param_alias = kind(node) == "Name" && param_aliases.contains(s(node, "id"));
    let mut has_bare = !is_param_alias && syms.any(node, |v| UNPARAMETERIZED.contains(&v));
    for_each_child(node, |c| {
        if !(has_any && has_bare) {
            let (ca, cb) = scan_annotation(c, syms, param_aliases);
            has_any |= ca;
            has_bare |= cb;
        }
    });
    (has_any, has_bare)
}

fn check_untyped_core(
    file: &FileContext<'_>,
    parsed_all: Option<&(Node, symbols::Symbols)>,
    out: &mut Vec<Finding>,
) {
    let owned_nodes;
    let (tree, nodes, syms): (&Node, &[&Node], &symbols::Symbols) = match parsed_all {
        Some((tree, syms)) => {
            owned_nodes = walk(tree);
            (tree, &owned_nodes, syms)
        }
        None => (file.tree, file.nodes, file.symbols),
    };
    let param_aliases = parameterized_aliases(tree);
    let layer = if file.layer_name().is_empty() {
        "core"
    } else {
        file.layer_name()
    };
    let check_expr = |owner: &Node, expr: &Node, out: &mut Vec<Finding>| {
        let (has_any, has_bare) = scan_annotation(expr, syms, &param_aliases);
        if has_any {
            add(
                out,
                file,
                owner,
                "LIT209",
                "Any is forbidden in core and port; model data with typed frozen dataclasses",
            );
        }
        if has_bare {
            add(
                out,
                file,
                owner,
                "LIT209",
                "unparameterized collection in core or port; specify element types or use a dataclass",
            );
        }
    };
    for &node in nodes {
        let k = kind(node);
        if k == "ClassDef" {
            if items(node, "bases").iter().any(|b| {
                syms.any(b, |v| {
                    v == "typing.TypedDict" || v == "typing_extensions.TypedDict"
                })
            }) {
                add(
                    out,
                    file,
                    node,
                    "LIT209",
                    format!(
                        "'{}' uses TypedDict in {layer}; model domain data as a frozen dataclass",
                        s(node, "name")
                    ),
                );
            }
            if is_dataclass(node, syms) || is_named_tuple(node, syms) {
                for stmt in items(node, "body") {
                    if kind(stmt) != "AnnAssign" {
                        continue;
                    }
                    let ann = child(stmt, "annotation");
                    if kind(ann) == "Subscript" && syms.only(child(ann, "value"), "typing.ClassVar")
                    {
                        continue;
                    }
                    check_expr(stmt, ann, out);
                }
            }
        } else if k == "FunctionDef" || k == "AsyncFunctionDef" {
            let args = child(node, "args");
            let vararg = child(args, "vararg");
            let kwarg = child(args, "kwarg");
            for arg in items(args, "posonlyargs")
                .iter()
                .chain(items(args, "args"))
                .chain(items(args, "kwonlyargs"))
                .chain(std::iter::once(vararg).filter(|n| !n.is_null()))
                .chain(std::iter::once(kwarg).filter(|n| !n.is_null()))
            {
                let name = s(arg, "arg");
                if name == "self" || name == "cls" {
                    continue;
                }
                let ann = child(arg, "annotation");
                if ann.is_null() {
                    add(
                        out,
                        file,
                        arg,
                        "LIT209",
                        format!(
                            "core parameter '{name}' is unannotated; annotate inputs explicitly"
                        ),
                    );
                } else {
                    check_expr(arg, ann, out);
                }
            }
            let ret = child(node, "returns");
            if ret.is_null() {
                add(
                    out,
                    file,
                    node,
                    "LIT209",
                    format!(
                        "core function '{}' has no return annotation; annotate outputs explicitly",
                        s(node, "name")
                    ),
                );
            } else {
                check_expr(node, ret, out);
            }
        }
    }
}

fn is_broad_except(handler_type: &Node, syms: &symbols::Symbols) -> bool {
    if handler_type.is_null() {
        return true;
    }
    if kind(handler_type) == "Tuple" {
        return items(handler_type, "elts")
            .iter()
            .any(|e| is_broad_except(e, syms));
    }
    if !syms.is_unknown(handler_type)
        && syms.all(handler_type, |v| {
            v == "builtins.Exception" || v == "builtins.BaseException"
        })
    {
        return true;
    }
    kind(handler_type) == "Name" && matches!(s(handler_type, "id"), "Exception" | "BaseException")
}

fn ends_with_raise(body: &[Node]) -> bool {
    body.last().is_some_and(|st| kind(st) == "Raise")
}

fn is_dataclass(node: &Node, syms: &symbols::Symbols) -> bool {
    items(node, "decorator_list").iter().any(|dec| {
        syms.only(
            if kind(dec) == "Call" {
                child(dec, "func")
            } else {
                dec
            },
            "dataclasses.dataclass",
        )
    })
}

fn is_named_tuple(node: &Node, syms: &symbols::Symbols) -> bool {
    items(node, "bases").iter().any(|b| {
        syms.any(b, |v| {
            v == "typing.NamedTuple" || v == "typing_extensions.NamedTuple"
        })
    })
}

fn is_plain_class(node: &Node, syms: &symbols::Symbols) -> bool {
    if is_dataclass(node, syms) {
        return false;
    }
    let bases = items(node, "bases");
    if bases
        .iter()
        .any(|b| syms.any(b, |v| EXEMPT_CLASS_BASES.contains(&v)))
    {
        return false;
    }
    bases
        .iter()
        .all(|b| !syms.is_unknown(b) && syms.all(b, |v| PLAIN_BASES.contains(&v)))
}

fn allowed_method(node: &Node, syms: &symbols::Symbols) -> bool {
    if s(node, "name").starts_with('_') {
        return true;
    }
    items(node, "decorator_list").iter().any(|dec| {
        let target = if kind(dec) == "Call" {
            child(dec, "func")
        } else {
            dec
        };
        !syms.is_unknown(target) && syms.all(target, |v| ALLOWED_METHOD_DECORATORS.contains(&v))
    })
}

fn field_has_default(value: &Node, syms: &symbols::Symbols) -> bool {
    if value.is_null() {
        return false;
    }
    if kind(value) == "Call" && syms.only(child(value, "func"), "dataclasses.field") {
        for kw in items(value, "keywords") {
            if s(kw, "arg") == "init"
                && kind(child(kw, "value")) == "Constant"
                && child(kw, "value").get("value").and_then(Node::as_bool) == Some(false)
            {
                return false;
            }
        }
        return items(value, "keywords")
            .iter()
            .any(|kw| matches!(s(kw, "arg"), "default" | "default_factory"));
    }
    true
}

fn base_target(base: &Node) -> &Node {
    if kind(base) == "Subscript" {
        child(base, "value")
    } else {
        base
    }
}

fn is_protocol_base(base: &Node, syms: &symbols::Symbols) -> bool {
    let target = base_target(base);
    if syms.any(target, |v| {
        v == "typing.Protocol" || v == "typing_extensions.Protocol"
    }) {
        return true;
    }
    (kind(target) == "Name" && s(target, "id") == "Protocol")
        || (kind(target) == "Attribute" && s(target, "attr") == "Protocol")
}

fn is_exception_base(base: &Node, syms: &symbols::Symbols) -> bool {
    let target = base_target(base);
    if syms.any(target, |v| {
        v == "builtins.Exception" || v == "builtins.BaseException"
    }) {
        return true;
    }
    (kind(target) == "Name" && matches!(s(target, "id"), "Exception" | "BaseException"))
        || (kind(target) == "Attribute"
            && matches!(s(target, "attr"), "Exception" | "BaseException"))
}

fn visit_explicit(
    file: &FileContext<'_>,
    n: &Node,
    protocol: bool,
    exception: bool,
    out: &mut Vec<Finding>,
) {
    let mut p = protocol;
    let mut e = exception;
    let k = kind(n);
    match k {
        "ClassDef" => {
            p |= items(n, "bases")
                .iter()
                .any(|b| is_protocol_base(b, file.symbols));
            e |= s(n, "name").ends_with("Error")
                || s(n, "name").ends_with("Exception")
                || items(n, "bases")
                    .iter()
                    .any(|b| is_exception_base(b, file.symbols));
            if is_dataclass(n, file.symbols) || is_named_tuple(n, file.symbols) {
                for stmt in items(n, "body") {
                    if kind(stmt) != "AnnAssign" {
                        continue;
                    }
                    let ann = child(stmt, "annotation");
                    if kind(ann) == "Subscript"
                        && file.symbols.only(child(ann, "value"), "typing.ClassVar")
                    {
                        continue;
                    }
                    if field_has_default(child(stmt, "value"), file.symbols) {
                        add(
                            out,
                            file,
                            stmt,
                            "LIT406",
                            "explicit fields required; remove dataclass field default so callers construct values explicitly",
                        );
                    }
                }
            }
            for stmt in items(n, "body") {
                visit_explicit(file, stmt, p, e, out);
            }
        }
        "FunctionDef" | "AsyncFunctionDef" => {
            if !s(n, "name").starts_with('_') && !p && !(e && s(n, "name") == "__init__") {
                let a = child(n, "args");
                if !items(a, "defaults").is_empty()
                    || items(a, "kw_defaults").iter().any(|v| !v.is_null())
                {
                    add(
                        out,
                        file,
                        n,
                        "LIT406",
                        "explicit arguments required; remove parameter defaults so callers pass values explicitly",
                    );
                }
            }
            for stmt in items(n, "body") {
                visit_explicit(file, stmt, p, e, out);
            }
        }
        "Module" | "With" | "AsyncWith" => {
            for stmt in items(n, "body") {
                visit_explicit(file, stmt, p, e, out);
            }
        }
        "If" | "For" | "AsyncFor" | "While" => {
            for stmt in items(n, "body") {
                visit_explicit(file, stmt, p, e, out);
            }
            for stmt in items(n, "orelse") {
                visit_explicit(file, stmt, p, e, out);
            }
        }
        "Try" | "TryStar" => {
            for stmt in items(n, "body") {
                visit_explicit(file, stmt, p, e, out);
            }
            for h in items(n, "handlers") {
                for stmt in items(h, "body") {
                    visit_explicit(file, stmt, p, e, out);
                }
            }
            for stmt in items(n, "orelse") {
                visit_explicit(file, stmt, p, e, out);
            }
            for stmt in items(n, "finalbody") {
                visit_explicit(file, stmt, p, e, out);
            }
        }
        "Match" => {
            for c in items(n, "cases") {
                for stmt in items(c, "body") {
                    visit_explicit(file, stmt, p, e, out);
                }
            }
        }
        _ => {}
    }
}
