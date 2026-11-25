use super::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Origin<'a> {
    borrowed: Option<&'a str>,
    members: Option<&'a str>,
    kind: Option<&'a str>,
}

fn mutators(k: &str) -> &'static [&'static str] {
    match k {
        "builtins.list" => &[
            "append", "extend", "insert", "remove", "pop", "clear", "sort", "reverse",
        ],
        "builtins.dict" => &["clear", "pop", "popitem", "setdefault", "update"],
        "builtins.set" => &[
            "add",
            "clear",
            "discard",
            "pop",
            "remove",
            "update",
            "difference_update",
            "intersection_update",
            "symmetric_difference_update",
        ],
        "collections.deque" => &[
            "append",
            "appendleft",
            "clear",
            "extend",
            "extendleft",
            "insert",
            "pop",
            "popleft",
            "remove",
            "reverse",
            "rotate",
        ],
        _ => &[],
    }
}

fn any_mutator(attr: &str) -> bool {
    [
        "builtins.list",
        "builtins.dict",
        "builtins.set",
        "collections.deque",
    ]
    .iter()
    .any(|k| mutators(k).contains(&attr))
}

const SHALLOW_COPIES: &[&str] = &["copy.copy", "dataclasses.replace"];
const ELEMENT_VIEWS: &[&str] = &[
    "builtins.enumerate",
    "builtins.filter",
    "builtins.iter",
    "builtins.reversed",
    "builtins.sorted",
    "builtins.zip",
];

fn for_each_stored<'a>(n: &'a Node, f: &mut impl FnMut(&'a str)) {
    if kind(n) == "Name" {
        if kind(child(n, "ctx")) == "Store" {
            f(s(n, "id"));
        }
        return;
    }
    for_each_child(n, |c| for_each_stored(c, f));
}

struct Visitor<'a, 'f> {
    file: &'f FileContext<'a>,
    aliases: HashMap<&'a str, Origin<'a>>,
    undo: Vec<(&'a str, Option<Origin<'a>>)>,
    record_depth: u32,
    out: &'f mut Vec<Finding>,
}

impl<'a> Visitor<'a, '_> {
    #[inline]
    fn set_alias(&mut self, name: &'a str, val: Origin<'a>) {
        let prev = self.aliases.insert(name, val);
        if self.record_depth > 0 {
            self.undo.push((name, prev));
        }
    }

    fn origin(&self, n: &'a Node) -> Origin<'a> {
        match kind(n) {
            "Name" => self.aliases.get(s(n, "id")).copied().unwrap_or_default(),
            "Attribute" | "Subscript" => {
                let b = self.origin(child(n, "value"));
                let root = b.borrowed.or(b.members);
                if kind(n) == "Subscript" && kind(child(n, "slice")) == "Slice" {
                    Origin {
                        members: root,
                        kind: b.kind,
                        ..Origin::default()
                    }
                } else {
                    Origin {
                        borrowed: root,
                        ..Origin::default()
                    }
                }
            }
            "List" | "Dict" | "Set" => {
                let k = kind(n);
                let field = if k == "Dict" { "values" } else { "elts" };
                let mut single: Option<&'a str> = None;
                let mut multiple = false;
                for v in items(n, field) {
                    if let Some(root) = self.origin(v).borrowed {
                        match single {
                            None => single = Some(root),
                            Some(prev) if prev == root => {}
                            Some(_) => {
                                multiple = true;
                                break;
                            }
                        }
                    }
                }
                let kind_name = match k {
                    "List" => "builtins.list",
                    "Dict" => "builtins.dict",
                    _ => "builtins.set",
                };
                Origin {
                    members: if multiple { None } else { single },
                    kind: Some(kind_name),
                    ..Origin::default()
                }
            }
            "Call" => {
                let f = child(n, "func");
                let name = self.file.symbols.only_str(f).unwrap_or("");
                let first = items(n, "args").first();
                if !mutators(name).is_empty() {
                    let source = first.map(|n| self.origin(n)).unwrap_or_default();
                    let kind_static = match name {
                        "builtins.list" => "builtins.list",
                        "builtins.dict" => "builtins.dict",
                        "builtins.set" => "builtins.set",
                        _ => "collections.deque",
                    };
                    Origin {
                        members: source.borrowed.or(source.members),
                        kind: Some(kind_static),
                        ..Origin::default()
                    }
                } else if SHALLOW_COPIES.contains(&name)
                    && let Some(first) = first
                {
                    let source = self.origin(first);
                    Origin {
                        members: source.borrowed.or(source.members),
                        kind: source.kind,
                        ..Origin::default()
                    }
                } else if ELEMENT_VIEWS.contains(&name) {
                    let mut single: Option<&'a str> = None;
                    let mut multiple = false;
                    for a in items(n, "args") {
                        let source = self.origin(a);
                        if let Some(root) = source.borrowed.or(source.members) {
                            match single {
                                None => single = Some(root),
                                Some(prev) if prev == root => {}
                                Some(_) => {
                                    multiple = true;
                                    break;
                                }
                            }
                        }
                    }
                    Origin {
                        members: if multiple { None } else { single },
                        ..Origin::default()
                    }
                } else if kind(f) == "Attribute" {
                    let source = self.origin(child(f, "value"));
                    let known = source.kind.is_some_and(|k| !mutators(k).is_empty());
                    let root = source.borrowed.or(source.members);
                    match s(f, "attr") {
                        "copy" => Origin {
                            members: root,
                            kind: if known { source.kind } else { None },
                            ..Origin::default()
                        },
                        "items" | "values" => Origin {
                            members: root,
                            ..Origin::default()
                        },
                        "get" => Origin {
                            borrowed: root,
                            ..Origin::default()
                        },
                        _ => Origin::default(),
                    }
                } else {
                    Origin::default()
                }
            }
            _ => Origin::default(),
        }
    }

    fn report(&mut self, n: &Node, o: Origin<'a>) {
        if let Some(b) = o.borrowed {
            add(
                self.out,
                self.file,
                n,
                "LIT204",
                format!("mutation of caller-owned '{b}'; build and return a new value"),
            );
        }
    }

    fn write(&mut self, n: &'a Node) {
        match kind(n) {
            "Attribute" | "Subscript" => self.report(n, self.origin(child(n, "value"))),
            "Tuple" | "List" => {
                for e in items(n, "elts") {
                    self.write(e);
                }
            }
            _ => (),
        }
    }

    fn bind(&mut self, n: &'a Node, o: Origin<'a>) {
        if kind(n) == "Name" {
            if kind(child(n, "ctx")) == "Store" {
                self.set_alias(s(n, "id"), o);
            }
            return;
        }
        let derived = Origin {
            borrowed: o.borrowed.or(o.members),
            ..Origin::default()
        };
        let mut names = Vec::new();
        for_each_stored(n, &mut |name| names.push(name));
        for name in names {
            self.set_alias(name, derived);
        }
    }

    fn body(&mut self, b: &'a [Node]) {
        for n in b {
            self.visit(n);
        }
    }

    fn branches(&mut self, branches: Vec<Vec<&'a Node>>) {
        self.record_depth += 1;
        let mut deltas: Vec<HashMap<&'a str, Origin<'a>>> = Vec::with_capacity(branches.len());
        let mut changed_keys: Vec<&'a str> = Vec::new();
        for body in branches {
            let mark = self.undo.len();
            for n in body {
                self.visit(n);
            }
            let mut delta = HashMap::new();
            for &(k, _) in &self.undo[mark..] {
                if !delta.contains_key(k) {
                    if let Some(&val) = self.aliases.get(k) {
                        delta.insert(k, val);
                    }
                    if !changed_keys.contains(&k) {
                        changed_keys.push(k);
                    }
                }
            }
            while self.undo.len() > mark {
                let (k, prev) = self.undo.pop().unwrap();
                match prev {
                    Some(v) => {
                        self.aliases.insert(k, v);
                    }
                    None => {
                        self.aliases.remove(k);
                    }
                }
            }
            deltas.push(delta);
        }
        self.record_depth -= 1;

        for name in changed_keys {
            let pre = self.aliases.get(name).copied().unwrap_or_default();
            let first = deltas[0].get(name).copied().unwrap_or(pre);
            let merged = if deltas[1..]
                .iter()
                .all(|d| d.get(name).copied().unwrap_or(pre) == first)
            {
                first
            } else {
                Origin::default()
            };
            self.set_alias(name, merged);
        }
    }

    fn visit(&mut self, n: &'a Node) {
        match kind(n) {
            "FunctionDef" | "AsyncFunctionDef" | "ClassDef" | "Lambda" => (),
            "Assign" => {
                self.visit(child(n, "value"));
                let o = self.origin(child(n, "value"));
                for t in items(n, "targets") {
                    self.write(t);
                    self.bind(t, o);
                }
            }
            "AnnAssign" => {
                if !child(n, "value").is_null() {
                    self.visit(child(n, "value"));
                    let t = child(n, "target");
                    self.write(t);
                    self.bind(t, self.origin(child(n, "value")));
                }
            }
            "NamedExpr" => {
                self.visit(child(n, "value"));
                self.bind(child(n, "target"), self.origin(child(n, "value")));
            }
            "AugAssign" => {
                self.visit(child(n, "value"));
                let t = child(n, "target");
                if kind(t) == "Name" {
                    let o = self.origin(t);
                    if o.kind.is_some_and(|k| !mutators(k).is_empty()) {
                        self.report(n, o);
                    }
                } else {
                    self.write(t);
                }
            }
            "Delete" => {
                for t in items(n, "targets") {
                    self.write(t);
                    self.bind(t, Origin::default());
                }
            }
            "Expr" => {
                let call = child(n, "value");
                let f = child(call, "func");
                if kind(call) == "Call" && kind(f) == "Attribute" {
                    let o = self.origin(child(f, "value"));
                    if o.kind.is_none() && any_mutator(s(f, "attr")) {
                        self.report(call, o);
                    }
                }
                for_each_child(n, |c| self.visit(c));
            }
            "Call" => {
                let f = child(n, "func");
                if kind(f) == "Attribute" {
                    let o = self.origin(child(f, "value"));
                    if mutators(o.kind.unwrap_or("")).contains(&s(f, "attr")) {
                        self.report(n, o);
                    }
                }
                if self.file.symbols.only_str(f).is_some_and(|name| {
                    matches!(
                        name,
                        "builtins.setattr" | "builtins.delattr" | "builtins.object.__setattr__"
                    )
                }) && let Some(a) = items(n, "args").first()
                {
                    self.report(n, self.origin(a));
                }
                for_each_child(n, |c| self.visit(c));
            }
            "For" => {
                self.visit(child(n, "iter"));
                let o = self.origin(child(n, "iter"));
                self.bind(
                    child(n, "target"),
                    Origin {
                        borrowed: o.borrowed.or(o.members),
                        ..Origin::default()
                    },
                );
                self.branches(vec![vec![], items(n, "body").iter().collect()]);
                self.body(items(n, "orelse"));
            }
            "If" => {
                self.visit(child(n, "test"));
                self.branches(vec![
                    items(n, "body").iter().collect(),
                    items(n, "orelse").iter().collect(),
                ]);
            }
            "While" => {
                self.visit(child(n, "test"));
                self.branches(vec![vec![], items(n, "body").iter().collect()]);
                self.body(items(n, "orelse"));
            }
            "Try" => {
                let mut branches =
                    vec![items(n, "body").iter().chain(items(n, "orelse")).collect()];
                branches.extend(
                    items(n, "handlers")
                        .iter()
                        .map(|h| items(h, "body").iter().collect()),
                );
                self.branches(branches);
                self.body(items(n, "finalbody"));
            }
            "ListComp" | "SetComp" | "DictComp" | "GeneratorExp" => {
                self.record_depth += 1;
                let mark = self.undo.len();
                for g in items(n, "generators") {
                    self.visit(child(g, "iter"));
                    let o = self.origin(child(g, "iter"));
                    self.bind(
                        child(g, "target"),
                        Origin {
                            borrowed: o.borrowed.or(o.members),
                            ..Origin::default()
                        },
                    );
                    self.body(items(g, "ifs"));
                }
                if kind(n) == "DictComp" {
                    self.visit(child(n, "key"));
                    self.visit(child(n, "value"));
                } else {
                    self.visit(child(n, "elt"));
                }
                while self.undo.len() > mark {
                    let (k, prev) = self.undo.pop().unwrap();
                    match prev {
                        Some(v) => {
                            self.aliases.insert(k, v);
                        }
                        None => {
                            self.aliases.remove(k);
                        }
                    }
                }
                self.record_depth -= 1;
            }
            _ => {
                for_each_child(n, |c| self.visit(c));
            }
        }
    }
}

pub(super) fn check(file: &FileContext<'_>, out: &mut Vec<Finding>) {
    if !file.has_layer(&["core", "port"]) {
        return;
    }
    for &n in file.nodes {
        let k = kind(n);
        if k != "FunctionDef" && k != "AsyncFunctionDef" {
            continue;
        }
        let mut aliases: HashMap<&str, Origin<'_>> = HashMap::new();
        for a in parameters(n) {
            let arg_name = s(a, "arg");
            if arg_name == "self" || arg_name == "cls" {
                continue;
            }
            let ann = child(a, "annotation");
            let k = if !ann.is_null() {
                file.symbols.only_str(ann).map(|name| match name {
                    "builtins.list"
                    | "typing.List"
                    | "typing.MutableSequence"
                    | "collections.abc.MutableSequence" => "builtins.list",
                    "builtins.dict"
                    | "typing.Dict"
                    | "typing.MutableMapping"
                    | "collections.abc.MutableMapping"
                    | "collections.Counter"
                    | "collections.OrderedDict"
                    | "collections.defaultdict"
                    | "typing.Counter"
                    | "typing.DefaultDict"
                    | "typing.OrderedDict" => "builtins.dict",
                    "builtins.set"
                    | "typing.Set"
                    | "typing.MutableSet"
                    | "collections.abc.MutableSet" => "builtins.set",
                    "collections.deque" | "typing.Deque" => "collections.deque",
                    other => other,
                })
            } else {
                None
            };
            aliases.insert(
                arg_name,
                Origin {
                    borrowed: Some(arg_name),
                    kind: k,
                    ..Origin::default()
                },
            );
        }
        if aliases.is_empty() {
            continue;
        }
        let mut v = Visitor {
            file,
            aliases,
            undo: Vec::new(),
            record_depth: 0,
            out,
        };
        v.body(items(n, "body"));
    }
}
