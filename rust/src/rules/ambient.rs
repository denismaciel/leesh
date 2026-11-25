use super::*;

const STATEFUL_LAYERS: &[&str] = &["core", "port", "shell", "adapter", "router"];
const STATEFUL_CONSTRUCTORS: &[&str] = &[
    "aiohttp.ClientSession",
    "aiohttp.client.ClientSession",
    "asyncio.Event",
    "asyncio.Lock",
    "asyncio.Queue",
    "asyncio.Semaphore",
    "boto3.client",
    "boto3.resource",
    "builtins.bytearray",
    "builtins.dict",
    "builtins.iter",
    "builtins.list",
    "builtins.set",
    "collections.ChainMap",
    "collections.Counter",
    "collections.OrderedDict",
    "collections.defaultdict",
    "collections.deque",
    "httpx.AsyncClient",
    "httpx.Client",
    "itertools.count",
    "itertools.cycle",
    "queue.LifoQueue",
    "queue.PriorityQueue",
    "queue.Queue",
    "redis.Redis",
    "redis.asyncio.Redis",
    "requests.Session",
    "requests.sessions.Session",
    "threading.BoundedSemaphore",
    "threading.Condition",
    "threading.Event",
    "threading.Lock",
    "threading.RLock",
    "threading.Semaphore",
    "types.SimpleNamespace",
];
const CACHE_DECORATORS: &[&str] = &["functools.cache", "functools.lru_cache"];

fn collect_scope<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    out.push(n);
    if !matches!(
        kind(n),
        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" | "Lambda"
    ) {
        for_each_child(n, |c| collect_scope(c, out));
    }
}

fn collect_locals<'a>(n: &'a Node, result: &mut HashSet<&'a str>) {
    match kind(n) {
        "Name" if kind(child(n, "ctx")) == "Store" => {
            result.insert(s(n, "id"));
        }
        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" => {
            result.insert(s(n, "name"));
            return;
        }
        "Lambda" | "ListComp" | "SetComp" | "DictComp" | "GeneratorExp" => return,
        "Import" | "ImportFrom" => {
            let is_import = kind(n) == "Import";
            for a in items(n, "names") {
                let asname = s(a, "asname");
                let name = if !asname.is_empty() {
                    asname
                } else if is_import {
                    s(a, "name").split('.').next().unwrap_or("")
                } else {
                    s(a, "name")
                };
                result.insert(name);
            }
            return;
        }
        _ => (),
    }
    for_each_child(n, |c| collect_locals(c, result));
}

fn locals_in_body<'a>(body: &'a [Node], result: &mut HashSet<&'a str>) {
    for n in body {
        collect_locals(n, result);
    }
}

pub(super) fn check(file: &FileContext<'_>, out: &mut Vec<Finding>) {
    if !file.has_layer(STATEFUL_LAYERS) {
        return;
    }
    let inner = file.has_layer(&["core", "port"]);
    let is_core = file.has_layer(&["core"]);

    let mut globals: HashSet<&str> = HashSet::new();
    for st in items(file.tree, "body") {
        let sk = kind(st);
        if sk != "Assign" && sk != "AnnAssign" {
            continue;
        }
        let v = child(st, "value");
        let vk = kind(v);
        let is_stateful = matches!(
            vk,
            "List" | "Dict" | "Set" | "ListComp" | "DictComp" | "SetComp"
        ) || (vk == "Call"
            && file
                .symbols
                .only_str(child(v, "func"))
                .is_some_and(|n| STATEFUL_CONSTRUCTORS.contains(&n)));
        if is_stateful {
            if sk == "Assign" {
                for t in items(st, "targets") {
                    if kind(t) == "Name" {
                        globals.insert(s(t, "id"));
                    }
                }
            } else {
                let t = child(st, "target");
                if kind(t) == "Name" {
                    globals.insert(s(t, "id"));
                }
            }
        }
    }

    // Only build parent map when mutable module globals exist.
    let parents: Option<HashMap<usize, &Node>> = if !globals.is_empty() {
        let mut map = HashMap::with_capacity(file.nodes.len());
        for &n in file.nodes {
            for_each_child(n, |c| {
                map.insert(ast::id(c), n);
            });
        }
        Some(map)
    } else {
        None
    };

    let mut localnodes: Vec<&Node> = Vec::new();
    let mut external: HashSet<&str> = HashSet::new();
    let mut shadowed: HashSet<&str> = HashSet::new();

    for &n in file.nodes {
        if inner
            && loaded(file, n)
            && !file.symbols.is_unknown(n)
            && file.symbols.all(n, effects::ambient)
        {
            let text = file.symbols.joined(n);
            out.push(finding(
                file,
                n,
                "LIT203",
                format!(
                    "{text} uses ambient state; pass an explicit value or policy into the core"
                ),
                Some(text),
            ));
        }
        let k = kind(n);
        if k != "FunctionDef" && k != "AsyncFunctionDef" {
            continue;
        }
        if !is_core {
            for dec in items(n, "decorator_list") {
                let target = if kind(dec) == "Call" {
                    child(dec, "func")
                } else {
                    dec
                };
                if !file.symbols.is_unknown(target)
                    && file.symbols.all(target, |v| CACHE_DECORATORS.contains(&v))
                {
                    let text = file.symbols.joined(target);
                    out.push(finding(
                        file,
                        dec,
                        "LIT203",
                        format!(
                            "{text} caches process-wide state; pass explicit state or cache at a boundary"
                        ),
                        Some(text),
                    ));
                }
            }
        }
        localnodes.clear();
        for stmt in items(n, "body") {
            collect_scope(stmt, &mut localnodes);
        }
        external.clear();
        for &ln in &localnodes {
            let lk = kind(ln);
            if lk == "Global" || (inner && lk == "Nonlocal") {
                for name_node in items(ln, "names") {
                    if let Some(name) = name_node.as_str() {
                        external.insert(name);
                    }
                }
            }
        }
        let check_globals = !globals.is_empty();
        if !check_globals && external.is_empty() {
            continue;
        }
        if check_globals {
            shadowed.clear();
            locals_in_body(items(n, "body"), &mut shadowed);
            let args = child(n, "args");
            for a in items(args, "posonlyargs")
                .iter()
                .chain(items(args, "args"))
                .chain(items(args, "kwonlyargs"))
            {
                shadowed.insert(s(a, "arg"));
            }
            for variadic in [child(args, "vararg"), child(args, "kwarg")] {
                if !variadic.is_null() {
                    shadowed.insert(s(variadic, "arg"));
                }
            }
            if let Some(ref parents) = parents {
                let mut parent = parents.get(&ast::id(n));
                while let Some(&p) = parent {
                    let pk = kind(p);
                    if matches!(pk, "FunctionDef" | "AsyncFunctionDef" | "Lambda") {
                        let pa = child(p, "args");
                        for a in items(pa, "posonlyargs")
                            .iter()
                            .chain(items(pa, "args"))
                            .chain(items(pa, "kwonlyargs"))
                        {
                            shadowed.insert(s(a, "arg"));
                        }
                        if pk != "Lambda" {
                            locals_in_body(items(p, "body"), &mut shadowed);
                        }
                    }
                    parent = parents.get(&ast::id(p));
                }
            }
            if !external.is_empty() {
                shadowed.retain(|name| !external.contains(name));
            }
        }
        for &c in &localnodes {
            if kind(c) != "Name" {
                continue;
            }
            let name = s(c, "id");
            let ctx = kind(child(c, "ctx"));
            if (ctx == "Store" || ctx == "Del") && external.contains(name) {
                add(
                    out,
                    file,
                    c,
                    "LIT203",
                    format!("write to global/nonlocal '{name}'; return the updated state instead"),
                );
            } else if check_globals
                && ctx == "Load"
                && globals.contains(name)
                && !shadowed.contains(name)
            {
                add(
                    out,
                    file,
                    c,
                    "LIT203",
                    format!(
                        "mutable module state '{name}'; use an immutable constant or pass a snapshot"
                    ),
                );
            }
        }
    }
}
