//! Conservative, flow-sensitive bindings matching the Python implementation.
use crate::ast::{self, Node, child, for_each_child, id, items, kind, s};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

/// Resolved names per expression id, sorted by id.
/// Unresolved expressions are absent (they represent `"?"`).
#[derive(Debug, Clone, Default)]
pub struct Symbols(Box<[(u32, ResolvedNames)]>);

#[derive(Debug, Clone)]
enum ResolvedNames {
    Static(&'static str),
    One(Box<str>),
    Many(Box<[Box<str>]>),
}

static UNKNOWN_SLICE: &[&str] = &["?"];

impl Symbols {
    fn from_entries(mut entries: Vec<(u32, SymSet)>) -> Self {
        entries.sort_unstable_by_key(|(id, _)| *id);
        // Keep last entry per id if ever duplicated (matches HashMap::insert overwrite).
        let mut compacted: Vec<(u32, ResolvedNames)> = Vec::with_capacity(entries.len());
        for (node_id, syms) in entries {
            let resolved = match syms {
                SymSet::Unknown => continue,
                SymSet::One(Sym::Static(s)) => ResolvedNames::Static(s),
                SymSet::One(Sym::Owned(s)) => ResolvedNames::One(s.as_ref().into()),
                SymSet::Many(list) => {
                    ResolvedNames::Many(list.iter().map(|s| s.as_str().into()).collect())
                }
            };
            if let Some(last) = compacted.last_mut()
                && last.0 == node_id
            {
                last.1 = resolved;
            } else {
                compacted.push((node_id, resolved));
            }
        }
        Self(compacted.into_boxed_slice())
    }

    #[inline]
    fn find(&self, node_id: u32) -> Option<&ResolvedNames> {
        let index = self.0.binary_search_by_key(&node_id, |(id, _)| *id).ok()?;
        Some(&self.0[index].1)
    }

    #[inline]
    pub fn is_unknown(&self, node: &Node) -> bool {
        self.find(id(node) as u32).is_none()
    }

    #[inline]
    pub fn only(&self, node: &Node, expected: &str) -> bool {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => *s == expected,
            Some(ResolvedNames::One(s)) => &**s == expected,
            Some(ResolvedNames::Many(list)) => list.len() == 1 && &*list[0] == expected,
            None => expected == "?",
        }
    }

    #[inline]
    pub fn only_str(&self, node: &Node) -> Option<&str> {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => Some(*s),
            Some(ResolvedNames::One(s)) => Some(s),
            Some(ResolvedNames::Many(list)) if list.len() == 1 => Some(&list[0]),
            Some(ResolvedNames::Many(_)) => None,
            None => Some("?"),
        }
    }

    #[inline]
    pub fn contains(&self, node: &Node, expected: &str) -> bool {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => *s == expected,
            Some(ResolvedNames::One(s)) => &**s == expected,
            Some(ResolvedNames::Many(list)) => list.iter().any(|s| &**s == expected),
            None => expected == "?",
        }
    }

    #[inline]
    pub fn any(&self, node: &Node, mut f: impl FnMut(&str) -> bool) -> bool {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => f(s),
            Some(ResolvedNames::One(s)) => f(s),
            Some(ResolvedNames::Many(list)) => list.iter().any(|s| f(s)),
            None => f("?"),
        }
    }

    #[inline]
    pub fn all(&self, node: &Node, mut f: impl FnMut(&str) -> bool) -> bool {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => f(s),
            Some(ResolvedNames::One(s)) => f(s),
            Some(ResolvedNames::Many(list)) => !list.is_empty() && list.iter().all(|s| f(s)),
            None => f("?"),
        }
    }

    #[inline]
    pub fn for_each(&self, node: &Node, mut f: impl FnMut(&str)) {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => f(s),
            Some(ResolvedNames::One(s)) => f(s),
            Some(ResolvedNames::Many(list)) => {
                for s in list {
                    f(s);
                }
            }
            None => {
                for s in UNKNOWN_SLICE {
                    f(s);
                }
            }
        }
    }

    pub fn joined(&self, node: &Node) -> String {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => (*s).to_owned(),
            Some(ResolvedNames::One(s)) => s.to_string(),
            Some(ResolvedNames::Many(list)) => list.join(", "),
            None => "?".to_owned(),
        }
    }

    /// Returns resolved names as boxed slice, or empty slice if unknown (`"?"`).
    pub fn resolved_box(&self, node: &Node) -> Box<[Box<str>]> {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => Box::new([(*s).into()]),
            Some(ResolvedNames::One(s)) => Box::new([s.clone()]),
            Some(ResolvedNames::Many(list)) => list.clone(),
            None => Box::new([]),
        }
    }

    pub fn to_set(&self, node: &Node) -> BTreeSet<String> {
        match self.find(id(node) as u32) {
            Some(ResolvedNames::Static(s)) => BTreeSet::from([(*s).to_owned()]),
            Some(ResolvedNames::One(s)) => BTreeSet::from([s.to_string()]),
            Some(ResolvedNames::Many(list)) => list.iter().map(|s| s.to_string()).collect(),
            None => BTreeSet::from(["?".to_owned()]),
        }
    }
}

pub fn names(symbols: &Symbols, node: &Node) -> BTreeSet<String> {
    symbols.to_set(node)
}

#[derive(Clone, Debug)]
enum Sym {
    Static(&'static str),
    Owned(Rc<str>),
}

impl Sym {
    #[inline]
    fn as_str(&self) -> &str {
        match self {
            Sym::Static(s) => s,
            Sym::Owned(s) => s,
        }
    }
}

impl PartialEq for Sym {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for Sym {}
impl PartialOrd for Sym {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Sym {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SymSet {
    Unknown,
    One(Sym),
    Many(Rc<[Sym]>),
}

impl SymSet {
    #[inline]
    fn is_unknown(&self) -> bool {
        matches!(self, SymSet::Unknown)
    }

    #[inline]
    fn one_owned(s: String) -> Self {
        if s == "?" {
            SymSet::Unknown
        } else {
            SymSet::One(Sym::Owned(Rc::from(s)))
        }
    }

    #[inline]
    fn one_str(s: &str) -> Self {
        if s == "?" {
            SymSet::Unknown
        } else {
            SymSet::One(Sym::Owned(Rc::from(s)))
        }
    }

    fn append_suffix(&self, suffix: &str) -> Self {
        match self {
            SymSet::Unknown => SymSet::Unknown,
            SymSet::One(sym) => {
                let s = sym.as_str();
                if s == "?" {
                    SymSet::Unknown
                } else {
                    let mut out = String::with_capacity(s.len() + 1 + suffix.len());
                    out.push_str(s);
                    out.push('.');
                    out.push_str(suffix);
                    SymSet::One(Sym::Owned(Rc::from(out)))
                }
            }
            SymSet::Many(list) => {
                let v: Vec<Sym> = list
                    .iter()
                    .map(|sym| {
                        let s = sym.as_str();
                        if s == "?" {
                            Sym::Static("?")
                        } else {
                            let mut out = String::with_capacity(s.len() + 1 + suffix.len());
                            out.push_str(s);
                            out.push('.');
                            out.push_str(suffix);
                            Sym::Owned(Rc::from(out))
                        }
                    })
                    .collect();
                Self::from_vec(v)
            }
        }
    }

    fn from_vec(mut v: Vec<Sym>) -> Self {
        v.sort();
        v.dedup();
        match v.len() {
            0 => SymSet::Unknown,
            1 => {
                let only = v.pop().unwrap();
                if only.as_str() == "?" {
                    SymSet::Unknown
                } else {
                    SymSet::One(only)
                }
            }
            _ => SymSet::Many(Rc::from(v)),
        }
    }

    fn push_into(&self, out: &mut Vec<Sym>) {
        match self {
            SymSet::Unknown => out.push(Sym::Static("?")),
            SymSet::One(sym) => out.push(sym.clone()),
            SymSet::Many(list) => out.extend(list.iter().cloned()),
        }
    }
}

type Bindings<'a> = HashMap<&'a str, SymSet>;

pub fn absolute_module(node: &Node, package: Option<&str>) -> Option<String> {
    let level = child(node, "level").as_u64().unwrap_or(0) as usize;
    if level == 0 {
        return child(node, "module").as_str().map(str::to_string);
    }
    let package = package?;
    let parts: Vec<_> = if package.is_empty() {
        vec![]
    } else {
        package.split('.').collect()
    };
    if level > parts.len() {
        return None;
    }
    let mut prefix = parts[..parts.len() - level + 1].to_vec();
    let m = s(node, "module");
    if !m.is_empty() {
        prefix.push(m);
    }
    Some(prefix.join("."))
}

fn for_each_stored_name<'a>(node: &'a Node, f: &mut impl FnMut(&'a str)) {
    if kind(node) == "Name" {
        if kind(child(node, "ctx")) == "Store" {
            f(s(node, "id"));
        }
        return;
    }
    for_each_child(node, |c| for_each_stored_name(c, f));
}

fn for_each_local<'a>(node: &'a Node, f: &mut impl FnMut(&'a str)) {
    match kind(node) {
        "Name" => {
            if kind(child(node, "ctx")) == "Store" {
                f(s(node, "id"));
            }
        }
        "Import" | "ImportFrom" => {
            let is_import = kind(node) == "Import";
            for alias in items(node, "names") {
                let asname = s(alias, "asname");
                let bound = if !asname.is_empty() {
                    asname
                } else {
                    let name = s(alias, "name");
                    if is_import {
                        name.split('.').next().unwrap_or(name)
                    } else {
                        name
                    }
                };
                f(bound);
            }
        }
        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" => {
            f(s(node, "name"));
        }
        "Lambda" | "ListComp" | "SetComp" | "DictComp" | "GeneratorExp" => {}
        _ => {
            for_each_child(node, |c| for_each_local(c, f));
        }
    }
}

fn for_each_match_capture<'a>(node: &'a Node, f: &mut impl FnMut(&'a str)) {
    let name = match kind(node) {
        "MatchAs" | "MatchStar" => s(node, "name"),
        "MatchMapping" => s(node, "rest"),
        _ => "",
    };
    if !name.is_empty() {
        f(name);
    }
    for_each_child(node, |c| for_each_match_capture(c, f));
}

#[inline]
fn is_ascii_ident(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

#[inline]
fn is_python_keyword(s: &str) -> bool {
    matches!(
        s,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

struct Resolver<'a, 'r> {
    bindings: Bindings<'a>,
    base: Option<Rc<Bindings<'a>>>,
    function_parent: Option<Rc<Bindings<'a>>>,
    references: &'r mut Vec<(u32, SymSet)>,
    undo: Vec<(&'a str, Option<SymSet>)>,
    record_depth: u32,
    package: Option<&'a str>,
}

impl<'a, 'r> Resolver<'a, 'r> {
    fn new(
        base: Option<Rc<Bindings<'a>>>,
        references: &'r mut Vec<(u32, SymSet)>,
        package: Option<&'a str>,
    ) -> Self {
        Self {
            bindings: HashMap::new(),
            base,
            function_parent: None,
            references,
            undo: Vec::new(),
            record_depth: 0,
            package,
        }
    }

    #[inline]
    fn set_binding(&mut self, name: &'a str, val: SymSet) {
        let prev = self.bindings.insert(name, val);
        if self.record_depth > 0 {
            self.undo.push((name, prev));
        }
    }

    #[inline]
    fn lookup(&self, name: &str) -> SymSet {
        if let Some(v) = self.bindings.get(name) {
            return v.clone();
        }
        if let Some(base) = &self.base
            && let Some(v) = base.get(name)
        {
            return v.clone();
        }
        if let Some(b) = lookup_builtin(name) {
            return SymSet::One(Sym::Static(b));
        }
        SymSet::Unknown
    }

    fn effective_bindings(&self) -> Rc<Bindings<'a>> {
        if self.bindings.is_empty()
            && let Some(base) = &self.base
        {
            return Rc::clone(base);
        }
        let mut merged = match &self.base {
            Some(base) => (**base).clone(),
            None => HashMap::with_capacity(self.bindings.len()),
        };
        for (&k, v) in &self.bindings {
            merged.insert(k, v.clone());
        }
        Rc::new(merged)
    }

    #[inline]
    fn names(&self, node: &Node) -> SymSet {
        self.names_depth(node, 0)
    }

    fn names_depth(&self, node: &Node, depth: usize) -> SymSet {
        if depth > 64 {
            return SymSet::Unknown;
        }
        match kind(node) {
            "Name" => self.lookup(s(node, "id")),
            "Attribute" => {
                let base = self.names_depth(child(node, "value"), depth + 1);
                if base.is_unknown() {
                    SymSet::Unknown
                } else {
                    base.append_suffix(s(node, "attr"))
                }
            }
            "Call" => {
                let function = self.names_depth(child(node, "func"), depth + 1);
                if function.is_unknown() {
                    return SymSet::Unknown;
                }
                if let SymSet::One(ref f) = function
                    && f.as_str() == "builtins.getattr"
                    && let [object, attribute, ..] = items(node, "args")
                    && kind(attribute) == "Constant"
                    && let Some(attr) = child(attribute, "value").as_str()
                {
                    let obj = self.names_depth(object, depth + 1);
                    return if obj.is_unknown() {
                        SymSet::Unknown
                    } else {
                        obj.append_suffix(attr)
                    };
                }
                match function {
                    SymSet::Unknown => SymSet::Unknown,
                    SymSet::One(ref f) => match f.as_str() {
                        "logging.getLogger" => SymSet::One(Sym::Static("logging.Logger")),
                        "structlog.get_logger" => SymSet::One(Sym::Static("structlog.BoundLogger")),
                        "pathlib.Path.home" | "pathlib.Path.cwd" => {
                            SymSet::One(Sym::Static("pathlib.Path"))
                        }
                        _ => function,
                    },
                    SymSet::Many(ref list) => {
                        let mapped: Vec<Sym> = list
                            .iter()
                            .map(|f| match f.as_str() {
                                "logging.getLogger" => Sym::Static("logging.Logger"),
                                "structlog.get_logger" => Sym::Static("structlog.BoundLogger"),
                                "pathlib.Path.home" | "pathlib.Path.cwd" => {
                                    Sym::Static("pathlib.Path")
                                }
                                _ => f.clone(),
                            })
                            .collect();
                        SymSet::from_vec(mapped)
                    }
                }
            }
            "Subscript" => self.names_depth(child(node, "value"), depth + 1),
            "BinOp" if kind(child(node, "op")) == "Div" => {
                self.names_depth(child(node, "left"), depth + 1)
            }
            "Constant" => {
                let Some(text) = child(node, "value").as_str() else {
                    return SymSet::Unknown;
                };
                if text.is_empty()
                    || text.contains('\n')
                    || (text.len() > 200 && text.chars().count() > 200)
                {
                    return SymSet::Unknown;
                }
                // Fast path 1: simple ASCII identifier.
                if is_ascii_ident(text) {
                    if depth >= 64 {
                        return SymSet::Unknown;
                    }
                    return self.lookup(text);
                }
                // Fast path 2: ASCII dotted attribute chain like `pathlib.Path`.
                if let Some((first, rest)) = text.split_once('.')
                    && is_ascii_ident(first)
                    && rest.split('.').all(is_ascii_ident)
                {
                    let segs = rest.bytes().filter(|&b| b == b'.').count() + 2;
                    if depth + segs > 64 {
                        return SymSet::Unknown;
                    }
                    let base = self.lookup(first);
                    if base.is_unknown() {
                        return SymSet::Unknown;
                    }
                    if is_python_keyword(first) || rest.split('.').any(is_python_keyword) {
                        return SymSet::Unknown;
                    }
                    return base.append_suffix(rest);
                }
                // Fast path 3: reject strings whose first non-whitespace char cannot start
                // a Name, Attribute, Call, Subscript, BinOp(Div), Constant(str), or `(`.
                let trimmed = text.trim_start_matches([' ', '\t', '\r', '\x0c']);
                let Some(&first_byte) = trimmed.as_bytes().first() else {
                    return SymSet::Unknown;
                };
                if first_byte.is_ascii()
                    && !(first_byte.is_ascii_alphabetic()
                        || first_byte == b'_'
                        || first_byte == b'('
                        || first_byte == b'\''
                        || first_byte == b'"')
                {
                    return SymSet::Unknown;
                }
                if let Some(expr) = ast::parse_expression(text) {
                    return self.names_depth(&expr, depth + 1);
                }
                SymSet::Unknown
            }
            _ => SymSet::Unknown,
        }
    }

    fn bind(&mut self, target: &'a Node, names: SymSet) {
        if kind(target) == "Name" {
            if kind(child(target, "ctx")) == "Store" {
                self.set_binding(s(target, "id"), names);
            }
            return;
        }
        let mut stored = Vec::new();
        for_each_stored_name(target, &mut |name| stored.push(name));
        for name in stored {
            self.set_binding(name, SymSet::Unknown);
        }
    }

    fn visit_body(&mut self, body: &'a [Node]) {
        for node in body {
            self.visit(node);
        }
    }

    fn generic(&mut self, node: &'a Node) {
        for_each_child(node, |c| self.visit(c));
    }

    fn function(&mut self, node: &'a Node) {
        let is_lambda = kind(node) == "Lambda";
        let args = child(node, "args");
        for default in items(args, "defaults")
            .iter()
            .chain(items(args, "kw_defaults"))
        {
            if !default.is_null() {
                self.visit(default);
            }
        }
        if !is_lambda {
            self.visit_body(items(node, "decorator_list"));
            self.set_binding(s(node, "name"), SymSet::Unknown);
        }
        let posonly = items(args, "posonlyargs");
        let regular = items(args, "args");
        let kwonly = items(args, "kwonlyargs");
        let vararg = child(args, "vararg");
        let kwarg = child(args, "kwarg");
        let mut param_annotations: Vec<(&'a str, SymSet)> = Vec::with_capacity(
            posonly.len()
                + regular.len()
                + kwonly.len()
                + usize::from(!vararg.is_null())
                + usize::from(!kwarg.is_null()),
        );
        for parameter in posonly.iter().chain(regular).chain(kwonly) {
            let ann = child(parameter, "annotation");
            if !ann.is_null() {
                self.visit(ann);
            }
            param_annotations.push((s(parameter, "arg"), self.names(ann)));
        }
        for variadic in [vararg, kwarg] {
            if !variadic.is_null() {
                let ann = child(variadic, "annotation");
                if !ann.is_null() {
                    self.visit(ann);
                }
                param_annotations.push((s(variadic, "arg"), self.names(ann)));
            }
        }
        let parent_rc = self
            .function_parent
            .clone()
            .unwrap_or_else(|| self.effective_bindings());
        let mut child_resolver = Resolver::new(Some(parent_rc), self.references, self.package);
        let body = if is_lambda {
            &[][..]
        } else {
            items(node, "body")
        };
        for stmt in body {
            for_each_local(stmt, &mut |name| {
                child_resolver.bindings.insert(name, SymSet::Unknown);
            });
        }
        for (name, sym) in param_annotations {
            child_resolver.bindings.insert(name, sym);
        }
        for variadic in [vararg, kwarg] {
            if !variadic.is_null() {
                child_resolver
                    .bindings
                    .insert(s(variadic, "arg"), SymSet::Unknown);
            }
        }
        if is_lambda {
            child_resolver.visit(child(node, "body"));
        } else {
            child_resolver.visit_body(body);
        }
    }

    fn branches(&mut self, branches: Vec<Vec<&'a Node>>) {
        self.record_depth += 1;
        let mut deltas: Vec<HashMap<&'a str, SymSet>> = Vec::with_capacity(branches.len());
        let mut changed_keys: Vec<&'a str> = Vec::new();
        for body in branches {
            let mark = self.undo.len();
            for statement in body {
                self.visit(statement);
            }
            let mut delta = HashMap::new();
            for &(k, _) in &self.undo[mark..] {
                if !delta.contains_key(k) {
                    if let Some(val) = self.bindings.get(k) {
                        delta.insert(k, val.clone());
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
                        self.bindings.insert(k, v);
                    }
                    None => {
                        self.bindings.remove(k);
                    }
                }
            }
            deltas.push(delta);
        }
        self.record_depth -= 1;

        if changed_keys.is_empty() {
            return;
        }
        let mut buf: Vec<Sym> = Vec::new();
        for key in changed_keys {
            let pre = self.lookup(key);
            buf.clear();
            let mut all_same = true;
            let mut first: Option<&SymSet> = None;
            for delta in &deltas {
                let val = delta.get(key).unwrap_or(&pre);
                if let Some(f) = first {
                    if f != val {
                        all_same = false;
                    }
                } else {
                    first = Some(val);
                }
            }
            let merged = if all_same {
                first.unwrap().clone()
            } else {
                for delta in &deltas {
                    delta.get(key).unwrap_or(&pre).push_into(&mut buf);
                }
                SymSet::from_vec(std::mem::take(&mut buf))
            };
            self.set_binding(key, merged);
        }
    }

    fn comprehension(&mut self, node: &'a Node) {
        self.record_depth += 1;
        let mark = self.undo.len();
        for generator in items(node, "generators") {
            self.visit(child(generator, "iter"));
            self.bind(child(generator, "target"), SymSet::Unknown);
            self.visit_body(items(generator, "ifs"));
        }
        if kind(node) == "DictComp" {
            self.visit(child(node, "key"));
            self.visit(child(node, "value"));
        } else {
            self.visit(child(node, "elt"));
        }
        while self.undo.len() > mark {
            let (k, prev) = self.undo.pop().unwrap();
            match prev {
                Some(v) => {
                    self.bindings.insert(k, v);
                }
                None => {
                    self.bindings.remove(k);
                }
            }
        }
        self.record_depth -= 1;
    }

    fn visit(&mut self, node: &'a Node) {
        if node.is_null() {
            return;
        }
        let k = kind(node);
        // Only these 6 expression kinds can ever resolve to a non-unknown symbol.
        if matches!(k, "Name" | "Attribute" | "Call" | "Subscript")
            || (k == "BinOp" && kind(child(node, "op")) == "Div")
            || (k == "Constant" && child(node, "value").is_string())
        {
            let names = self.names(node);
            if !names.is_unknown() {
                self.references.push((id(node) as u32, names));
            }
        }
        match k {
            "Module" => {
                let mut discard = Vec::new();
                let mut summary = Resolver::new(None, &mut discard, self.package);
                for statement in items(node, "body") {
                    match kind(statement) {
                        "Import" | "ImportFrom" => summary.visit(statement),
                        "Assign" => {
                            let names = summary.names(child(statement, "value"));
                            for target in items(statement, "targets") {
                                summary.bind(target, names.clone());
                            }
                        }
                        "AnnAssign" => {
                            let names = summary.names(child(statement, "value"));
                            summary.bind(child(statement, "target"), names);
                        }
                        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" => {
                            summary
                                .bindings
                                .insert(s(statement, "name"), SymSet::Unknown);
                        }
                        _ => {
                            for_each_local(statement, &mut |name| {
                                summary.bindings.insert(name, SymSet::Unknown);
                            });
                        }
                    }
                }
                self.function_parent = Some(Rc::new(summary.bindings));
                self.visit_body(items(node, "body"));
            }
            "Import" => {
                for alias in items(node, "names") {
                    let name = s(alias, "name");
                    let alias_name = s(alias, "asname");
                    let root = name.split('.').next().unwrap_or(name);
                    let (key, target) = if alias_name.is_empty() {
                        (root, root)
                    } else {
                        (alias_name, name)
                    };
                    self.set_binding(key, SymSet::one_str(target));
                }
            }
            "ImportFrom" => {
                if let Some(module) = absolute_module(node, self.package)
                    && !module.is_empty()
                {
                    for alias in items(node, "names") {
                        let name = s(alias, "name");
                        if name != "*" {
                            let alias_name = s(alias, "asname");
                            let key = if alias_name.is_empty() {
                                name
                            } else {
                                alias_name
                            };
                            let mut qualified =
                                String::with_capacity(module.len() + 1 + name.len());
                            qualified.push_str(&module);
                            qualified.push('.');
                            qualified.push_str(name);
                            self.set_binding(key, SymSet::one_owned(qualified));
                        }
                    }
                }
            }
            "Assign" => {
                self.visit(child(node, "value"));
                let names = self.names(child(node, "value"));
                for target in items(node, "targets") {
                    self.visit(target);
                    self.bind(target, names.clone());
                }
            }
            "AnnAssign" => {
                self.visit(child(node, "annotation"));
                self.visit(child(node, "value"));
                self.visit(child(node, "target"));
                let val_node = child(node, "value");
                let names = self.names(if val_node.is_null() {
                    child(node, "annotation")
                } else {
                    val_node
                });
                self.bind(child(node, "target"), names);
            }
            "NamedExpr" => {
                self.visit(child(node, "value"));
                let names = self.names(child(node, "value"));
                self.bind(child(node, "target"), names);
            }
            "AugAssign" => {
                self.generic(node);
                self.bind(child(node, "target"), SymSet::Unknown);
            }
            "Delete" => {
                for target in items(node, "targets") {
                    self.visit(target);
                    self.bind(target, SymSet::Unknown);
                }
            }
            "FunctionDef" | "AsyncFunctionDef" | "Lambda" => self.function(node),
            "ClassDef" => {
                for expression in items(node, "bases")
                    .iter()
                    .chain(items(node, "decorator_list"))
                {
                    self.visit(expression);
                }
                for keyword in items(node, "keywords") {
                    self.visit(child(keyword, "value"));
                }
                let saved_fp = self.function_parent.clone();
                if self.function_parent.is_none() {
                    self.function_parent = Some(self.effective_bindings());
                }
                self.record_depth += 1;
                let mark = self.undo.len();
                self.visit_body(items(node, "body"));
                while self.undo.len() > mark {
                    let (k, prev) = self.undo.pop().unwrap();
                    match prev {
                        Some(v) => {
                            self.bindings.insert(k, v);
                        }
                        None => {
                            self.bindings.remove(k);
                        }
                    }
                }
                self.record_depth -= 1;
                self.function_parent = saved_fp;
                self.set_binding(s(node, "name"), SymSet::Unknown);
            }
            "If" => {
                self.visit(child(node, "test"));
                self.branches(vec![
                    items(node, "body").iter().collect(),
                    items(node, "orelse").iter().collect(),
                ]);
            }
            "For" | "AsyncFor" => {
                self.visit(child(node, "iter"));
                self.bind(child(node, "target"), SymSet::Unknown);
                self.branches(vec![vec![], items(node, "body").iter().collect()]);
                self.visit_body(items(node, "orelse"));
            }
            "While" => {
                self.visit(child(node, "test"));
                self.branches(vec![vec![], items(node, "body").iter().collect()]);
                self.visit_body(items(node, "orelse"));
            }
            "Try" | "TryStar" => {
                let mut locals_in_body = Vec::new();
                for stmt in items(node, "body") {
                    for_each_local(stmt, &mut |name| locals_in_body.push(name));
                }
                for name in locals_in_body {
                    self.set_binding(name, SymSet::Unknown);
                }
                for handler in items(node, "handlers") {
                    if !child(handler, "name").is_null() {
                        self.set_binding(s(handler, "name"), SymSet::Unknown);
                    }
                }
                let mut branches = vec![
                    items(node, "body")
                        .iter()
                        .chain(items(node, "orelse"))
                        .collect(),
                ];
                branches.extend(
                    items(node, "handlers")
                        .iter()
                        .map(|h| items(h, "body").iter().collect()),
                );
                self.branches(branches);
                self.visit_body(items(node, "finalbody"));
            }
            "Match" => {
                self.visit(child(node, "subject"));
                for case in items(node, "cases") {
                    let mut captures = Vec::new();
                    for_each_match_capture(child(case, "pattern"), &mut |name| {
                        captures.push(name);
                    });
                    for name in captures {
                        self.set_binding(name, SymSet::Unknown);
                    }
                    self.visit(child(case, "guard"));
                }
                let mut branches = vec![vec![]];
                branches.extend(
                    items(node, "cases")
                        .iter()
                        .map(|c| items(c, "body").iter().collect()),
                );
                self.branches(branches);
            }
            "ListComp" | "SetComp" | "DictComp" | "GeneratorExp" => self.comprehension(node),
            _ => self.generic(node),
        }
    }
}

/// Sorted table of builtin names -> `"builtins.<name>"` for O(log N) zero-alloc lookup.
const BUILTINS_MAP: &[(&str, &str)] = &[
    ("ArithmeticError", "builtins.ArithmeticError"),
    ("AssertionError", "builtins.AssertionError"),
    ("AttributeError", "builtins.AttributeError"),
    ("BaseException", "builtins.BaseException"),
    ("BaseExceptionGroup", "builtins.BaseExceptionGroup"),
    ("BlockingIOError", "builtins.BlockingIOError"),
    ("BrokenPipeError", "builtins.BrokenPipeError"),
    ("BufferError", "builtins.BufferError"),
    ("BytesWarning", "builtins.BytesWarning"),
    ("ChildProcessError", "builtins.ChildProcessError"),
    ("ConnectionAbortedError", "builtins.ConnectionAbortedError"),
    ("ConnectionError", "builtins.ConnectionError"),
    ("ConnectionRefusedError", "builtins.ConnectionRefusedError"),
    ("ConnectionResetError", "builtins.ConnectionResetError"),
    ("DeprecationWarning", "builtins.DeprecationWarning"),
    ("EOFError", "builtins.EOFError"),
    ("EncodingWarning", "builtins.EncodingWarning"),
    ("EnvironmentError", "builtins.EnvironmentError"),
    ("Exception", "builtins.Exception"),
    ("ExceptionGroup", "builtins.ExceptionGroup"),
    ("FileExistsError", "builtins.FileExistsError"),
    ("FileNotFoundError", "builtins.FileNotFoundError"),
    ("FloatingPointError", "builtins.FloatingPointError"),
    ("FutureWarning", "builtins.FutureWarning"),
    ("GeneratorExit", "builtins.GeneratorExit"),
    ("IOError", "builtins.IOError"),
    ("ImportError", "builtins.ImportError"),
    ("ImportWarning", "builtins.ImportWarning"),
    ("IndentationError", "builtins.IndentationError"),
    ("IndexError", "builtins.IndexError"),
    ("InterruptedError", "builtins.InterruptedError"),
    ("IsADirectoryError", "builtins.IsADirectoryError"),
    ("KeyError", "builtins.KeyError"),
    ("KeyboardInterrupt", "builtins.KeyboardInterrupt"),
    ("LookupError", "builtins.LookupError"),
    ("MemoryError", "builtins.MemoryError"),
    ("ModuleNotFoundError", "builtins.ModuleNotFoundError"),
    ("NameError", "builtins.NameError"),
    ("NotADirectoryError", "builtins.NotADirectoryError"),
    ("NotImplementedError", "builtins.NotImplementedError"),
    ("OSError", "builtins.OSError"),
    ("OverflowError", "builtins.OverflowError"),
    (
        "PendingDeprecationWarning",
        "builtins.PendingDeprecationWarning",
    ),
    ("PermissionError", "builtins.PermissionError"),
    ("ProcessLookupError", "builtins.ProcessLookupError"),
    (
        "PythonFinalizationError",
        "builtins.PythonFinalizationError",
    ),
    ("RecursionError", "builtins.RecursionError"),
    ("ReferenceError", "builtins.ReferenceError"),
    ("ResourceWarning", "builtins.ResourceWarning"),
    ("RuntimeError", "builtins.RuntimeError"),
    ("RuntimeWarning", "builtins.RuntimeWarning"),
    ("StopAsyncIteration", "builtins.StopAsyncIteration"),
    ("StopIteration", "builtins.StopIteration"),
    ("SyntaxError", "builtins.SyntaxError"),
    ("SyntaxWarning", "builtins.SyntaxWarning"),
    ("SystemError", "builtins.SystemError"),
    ("SystemExit", "builtins.SystemExit"),
    ("TabError", "builtins.TabError"),
    ("TimeoutError", "builtins.TimeoutError"),
    ("TypeError", "builtins.TypeError"),
    ("UnboundLocalError", "builtins.UnboundLocalError"),
    ("UnicodeDecodeError", "builtins.UnicodeDecodeError"),
    ("UnicodeEncodeError", "builtins.UnicodeEncodeError"),
    ("UnicodeError", "builtins.UnicodeError"),
    ("UnicodeTranslateError", "builtins.UnicodeTranslateError"),
    ("UnicodeWarning", "builtins.UnicodeWarning"),
    ("UserWarning", "builtins.UserWarning"),
    ("ValueError", "builtins.ValueError"),
    ("Warning", "builtins.Warning"),
    ("ZeroDivisionError", "builtins.ZeroDivisionError"),
    ("_IncompleteInputError", "builtins._IncompleteInputError"),
    ("__import__", "builtins.__import__"),
    ("__loader__", "builtins.__loader__"),
    ("bool", "builtins.bool"),
    ("breakpoint", "builtins.breakpoint"),
    ("bytearray", "builtins.bytearray"),
    ("bytes", "builtins.bytes"),
    ("classmethod", "builtins.classmethod"),
    ("compile", "builtins.compile"),
    ("complex", "builtins.complex"),
    ("delattr", "builtins.delattr"),
    ("dict", "builtins.dict"),
    ("enumerate", "builtins.enumerate"),
    ("eval", "builtins.eval"),
    ("exec", "builtins.exec"),
    ("filter", "builtins.filter"),
    ("float", "builtins.float"),
    ("frozenset", "builtins.frozenset"),
    ("getattr", "builtins.getattr"),
    ("globals", "builtins.globals"),
    ("hasattr", "builtins.hasattr"),
    ("input", "builtins.input"),
    ("int", "builtins.int"),
    ("iter", "builtins.iter"),
    ("list", "builtins.list"),
    ("locals", "builtins.locals"),
    ("map", "builtins.map"),
    ("memoryview", "builtins.memoryview"),
    ("object", "builtins.object"),
    ("open", "builtins.open"),
    ("print", "builtins.print"),
    ("property", "builtins.property"),
    ("range", "builtins.range"),
    ("reversed", "builtins.reversed"),
    ("set", "builtins.set"),
    ("setattr", "builtins.setattr"),
    ("slice", "builtins.slice"),
    ("sorted", "builtins.sorted"),
    ("staticmethod", "builtins.staticmethod"),
    ("str", "builtins.str"),
    ("super", "builtins.super"),
    ("tuple", "builtins.tuple"),
    ("type", "builtins.type"),
    ("vars", "builtins.vars"),
    ("zip", "builtins.zip"),
];

#[inline]
fn lookup_builtin(name: &str) -> Option<&'static str> {
    BUILTINS_MAP
        .binary_search_by_key(&name, |(k, _)| *k)
        .ok()
        .map(|i| BUILTINS_MAP[i].1)
}

pub fn resolve(tree: &Node, package: Option<&str>) -> Symbols {
    let _span = crate::profile::span("symbols");
    let mut references = Vec::with_capacity(64);
    let mut resolver = Resolver::new(None, &mut references, package);
    resolver.visit(tree);
    Symbols::from_entries(references)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::walk;
    fn unknown() -> BTreeSet<String> {
        BTreeSet::from(["?".into()])
    }
    fn singleton(s: impl Into<String>) -> BTreeSet<String> {
        BTreeSet::from([s.into()])
    }
    fn calls(source: &str) -> Vec<BTreeSet<String>> {
        let tree = ast::parse(source).unwrap();
        let symbols = resolve(&tree, Some("example.domain"));
        let mut calls: Vec<_> = walk(&tree)
            .into_iter()
            .filter(|n| kind(n) == "Call")
            .collect();
        calls.sort_by_key(|n| n.position());
        calls
            .into_iter()
            .map(|n| names(&symbols, child(n, "func")))
            .collect()
    }
    #[test]
    fn builtins_table_is_sorted() {
        assert!(BUILTINS_MAP.windows(2).all(|w| w[0].0 < w[1].0));
    }
    #[test]
    fn branches_keep_all_possible_aliases() {
        assert_eq!(
            calls(
                "import random\nimport time\nif condition:\n    clock = time.time\nelse:\n    clock = random.random\nclock()\n"
            ),
            vec![BTreeSet::from(["random.random".into(), "time.time".into()])]
        );
    }
    #[test]
    fn function_globals_see_later_rebinding_and_local_shadowing() {
        assert_eq!(
            calls("import time\ndef first():\n    time.time()\ntime = None\n"),
            vec![unknown()]
        );
        assert_eq!(
            calls("import time\ndef second():\n    time.time()\n    time = None\n"),
            vec![unknown()]
        );
    }
    #[test]
    fn classes_do_not_leak_bindings_to_methods() {
        assert_eq!(
            calls(
                "import time\nclass C:\n    time = None\n    def method(self):\n        time.time()\n"
            ),
            vec![singleton("time.time")]
        );
    }
    #[test]
    fn annotations_factories_and_comprehensions() {
        assert_eq!(
            calls(
                "from pathlib import Path\ndef f(path: 'Path'):\n    path.read_text()\n[Path() for Path in values]\nPath()\n"
            ),
            vec![
                singleton("pathlib.Path.read_text"),
                unknown(),
                singleton("pathlib.Path")
            ]
        );
        assert_eq!(
            calls("from .clock import now\nnow()\n"),
            vec![singleton("example.domain.clock.now")]
        );
    }
}
