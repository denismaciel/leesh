use super::*;

const EFFECTFUL: &[&str] = &[
    "aiohttp.ClientSession",
    "aiohttp.client.ClientSession",
    "fsspec.AbstractFileSystem",
    "fsspec.spec.AbstractFileSystem",
    "httpx.AsyncClient",
    "httpx.Client",
    "io.BufferedIOBase",
    "io.IOBase",
    "io.RawIOBase",
    "io.TextIOBase",
    "io.TextIOWrapper",
    "psycopg.AsyncConnection",
    "psycopg.Connection",
    "requests.Session",
    "requests.sessions.Session",
    "sqlalchemy.engine.Connection",
    "sqlalchemy.engine.Engine",
    "sqlalchemy.ext.asyncio.AsyncSession",
    "sqlalchemy.ext.asyncio.session.AsyncSession",
    "sqlalchemy.orm.Session",
    "sqlalchemy.orm.session.Session",
    "sqlite3.Connection",
    "sqlite3.Cursor",
    "typing.BinaryIO",
    "typing.IO",
    "typing.TextIO",
];

fn for_each_nested_mut(n: &mut Node, f: &mut impl FnMut(&mut Node)) {
    for v in n.values_mut() {
        match v {
            Node::List(values) => {
                for item in values.iter_mut() {
                    f(item);
                }
            }
            other => f(other),
        }
    }
}

fn normalize(n: &mut Node) {
    if kind(n) == "Constant"
        && let Some(text) = n["value"].as_str()
        && let Some(mut parsed) = ast::parse_expression(text)
        && kind(&parsed) != "Constant"
    {
        normalize(&mut parsed);
        if let Some((line, column)) = n.position() {
            parsed.set_position(line, column);
        }
        *n = parsed;
        return;
    }
    for_each_nested_mut(n, &mut normalize);
}

fn rewrite(n: &mut Node, owner: &str) {
    if kind(n) == owner
        && let Some(a) = n.get_mut("annotation")
    {
        normalize(a);
    }
    for_each_nested_mut(n, &mut |child| rewrite(child, owner));
}

fn reid(n: &mut Node, c: &mut usize) {
    if n.is_node() {
        n.set_id(*c);
        *c += 1;
    }
    for_each_nested_mut(n, &mut |child| reid(child, c));
}

fn rewrite_all(n: &mut Node) {
    if matches!(kind(n), "arg" | "AnnAssign")
        && let Some(a) = n.get_mut("annotation")
    {
        normalize(a);
    } else if matches!(kind(n), "FunctionDef" | "AsyncFunctionDef")
        && let Some(r) = n.get_mut("returns")
    {
        normalize(r);
    }
    for_each_nested_mut(n, &mut rewrite_all);
}

fn has_string_constant(n: &Node) -> bool {
    if kind(n) == "Constant" && n["value"].is_string() {
        return true;
    }
    let mut found = false;
    for_each_child(n, |c| {
        if !found && has_string_constant(c) {
            found = true;
        }
    });
    found
}

pub(super) fn parsed_all_annotations(
    tree: &Node,
    nodes: &[&Node],
    package: Option<&str>,
) -> Option<(Node, symbols::Symbols)> {
    let has_strings = nodes.iter().any(|&n| {
        let ann = match kind(n) {
            "arg" | "AnnAssign" => child(n, "annotation"),
            "FunctionDef" | "AsyncFunctionDef" => child(n, "returns"),
            _ => return false,
        };
        !ann.is_null() && has_string_constant(ann)
    });
    if !has_strings {
        return None;
    }
    let mut cloned = tree.clone();
    rewrite_all(&mut cloned);
    reid(&mut cloned, &mut 1);
    let resolved = symbols::resolve(&cloned, package);
    Some((cloned, resolved))
}

/// A private tree with the forward annotations of `owner` nodes parsed, if there are any.
pub(super) fn parsed_annotations(
    tree: &Node,
    nodes: &[&Node],
    package: Option<&str>,
    owner: &str,
) -> Option<(Node, symbols::Symbols)> {
    let has_strings = nodes.iter().any(|&n| {
        kind(n) == owner && {
            let ann = child(n, "annotation");
            !ann.is_null() && has_string_constant(ann)
        }
    });
    if !has_strings {
        return None;
    }
    let mut cloned = tree.clone();
    rewrite(&mut cloned, owner);
    reid(&mut cloned, &mut 1);
    let resolved = symbols::resolve(&cloned, package);
    Some((cloned, resolved))
}

/// Visits type expressions in an annotation: skips `Literal` values and `Annotated` metadata.
pub(super) fn visit_annotation_nodes<'a>(
    n: &'a Node,
    symbols: &symbols::Symbols,
    f: &mut impl FnMut(&'a Node),
) {
    f(n);
    if kind(n) == "Subscript" {
        let val = child(n, "value");
        if symbols.any(val, |s| {
            s == "typing.Literal" || s == "typing_extensions.Literal"
        }) {
            return;
        }
        if symbols.any(val, |s| {
            s == "typing.Annotated" || s == "typing_extensions.Annotated"
        }) {
            let sl = child(n, "slice");
            let first = if kind(sl) == "Tuple" {
                items(sl, "elts").first().unwrap_or(sl)
            } else {
                sl
            };
            visit_annotation_nodes(first, symbols, f);
            return;
        }
    }
    for_each_child(n, |c| visit_annotation_nodes(c, symbols, f));
}

pub(super) fn check(file: &FileContext<'_>, policy: &Policy, out: &mut Vec<Finding>) {
    if !file.has_layer(&["core"]) {
        return;
    }
    let parsed = parsed_annotations(file.tree, file.nodes, file.package, "arg");
    let owned_nodes;
    let (nodes, syms): (&[&Node], &symbols::Symbols) = match &parsed {
        Some((tree, syms)) => {
            owned_nodes = walk(tree);
            (&owned_nodes, syms)
        }
        None => (file.nodes, file.symbols),
    };
    let mut matched = BTreeSet::new();
    for &a in nodes {
        if kind(a) != "arg" {
            continue;
        }
        let ann = child(a, "annotation");
        if ann.is_null() {
            continue;
        }
        matched.clear();
        visit_annotation_nodes(ann, syms, &mut |n| {
            if !syms.is_unknown(n)
                && syms.all(n, |s| {
                    EFFECTFUL.contains(&s) || policy.effectful_types.contains(s)
                })
            {
                syms.for_each(n, |s| {
                    matched.insert(s.to_owned());
                });
            }
        });
        if !matched.is_empty() {
            let joined = matched.iter().cloned().collect::<Vec<_>>().join(", ");
            add(
                out,
                file,
                a,
                "LIT207",
                format!(
                    "core parameter '{}' receives effectful capability {joined}; pass observed values and execute returned decisions in the shell",
                    s(a, "arg"),
                ),
            );
        }
    }
}
