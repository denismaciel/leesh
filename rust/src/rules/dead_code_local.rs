use super::*;
use crate::model::{
    AttrRead, DeadCodeSummary, DeadDef, DeadDefKind, DeadRef, InitFieldType, ParamTypeEntry,
    TypeFlow, def_flags,
};

const OPAQUE_DISPATCH: &[&str] = &[
    "builtins.__import__",
    "builtins.compile",
    "builtins.delattr",
    "builtins.eval",
    "builtins.exec",
    "builtins.globals",
    "builtins.locals",
    "builtins.setattr",
    "builtins.vars",
    "importlib.__import__",
    "importlib.import_module",
    "importlib.reload",
    "inspect.getmembers",
    "inspect.getmembers_static",
];

/// Decorators that only transform/annotate a definition without registering it externally.
const TRANSPARENT_DECORATORS: &[&str] = &[
    "abc.abstractmethod",
    "builtins.classmethod",
    "builtins.property",
    "builtins.staticmethod",
    "contextlib.asynccontextmanager",
    "contextlib.contextmanager",
    "dataclasses.dataclass",
    "functools.cache",
    "functools.cached_property",
    "functools.lru_cache",
    "functools.total_ordering",
    "functools.wraps",
    "typing.final",
    "typing.no_type_check",
    "typing.overload",
    "typing.override",
    "typing.runtime_checkable",
    "typing_extensions.deprecated",
    "typing_extensions.final",
    "typing_extensions.overload",
    "typing_extensions.override",
    "typing_extensions.runtime_checkable",
    "warnings.deprecated",
];

const PYDANTIC_BASES: &[&str] = &[
    "pydantic.BaseModel",
    "pydantic.RootModel",
    "pydantic.main.BaseModel",
    "pydantic.root_model.RootModel",
    "pydantic_settings.BaseSettings",
    "pydantic_settings.main.BaseSettings",
];

const PYDANTIC_HOOK_DECORATORS: &[&str] = &[
    "pydantic.computed_field",
    "pydantic.field_serializer",
    "pydantic.field_validator",
    "pydantic.model_serializer",
    "pydantic.model_validator",
    "pydantic.root_validator",
    "pydantic.validator",
    "pydantic.functional_serializers.field_serializer",
    "pydantic.functional_serializers.model_serializer",
    "pydantic.functional_validators.field_validator",
    "pydantic.functional_validators.model_validator",
    "pydantic.fields.computed_field",
];

const ORM_BASES: &[&str] = &[
    "sqlalchemy.ext.declarative.declarative_base",
    "sqlalchemy.orm.DeclarativeBase",
    "sqlalchemy.orm.DeclarativeBaseNoMeta",
    "sqlalchemy.orm.MappedAsDataclass",
    "sqlalchemy.orm.declarative_base",
];

const ORM_HOOK_DECORATORS: &[&str] = &[
    "sqlalchemy.ext.hybrid.hybrid_method",
    "sqlalchemy.ext.hybrid.hybrid_property",
    "sqlalchemy.orm.reconstructor",
    "sqlalchemy.orm.validates",
];

const ENUM_BASES: &[&str] = &[
    "enum.Enum",
    "enum.Flag",
    "enum.IntEnum",
    "enum.IntFlag",
    "enum.StrEnum",
];

const SERIALIZE_FUNCS: &[&str] = &[
    "dataclasses.asdict",
    "dataclasses.astuple",
    "dataclasses.fields",
    "pydantic.TypeAdapter",
];

const WRAPPER_TYPES: &[&str] = &[
    "builtins.frozenset",
    "builtins.list",
    "builtins.set",
    "builtins.tuple",
    "collections.abc.AsyncGenerator",
    "collections.abc.AsyncIterable",
    "collections.abc.AsyncIterator",
    "collections.abc.Awaitable",
    "collections.abc.Collection",
    "collections.abc.Coroutine",
    "collections.abc.Generator",
    "collections.abc.Iterable",
    "collections.abc.Iterator",
    "collections.abc.MutableSequence",
    "collections.abc.MutableSet",
    "collections.abc.Sequence",
    "collections.abc.Set",
    "typing.Annotated",
    "typing.AsyncGenerator",
    "typing.AsyncIterable",
    "typing.AsyncIterator",
    "typing.Awaitable",
    "typing.ClassVar",
    "typing.Collection",
    "typing.Coroutine",
    "typing.Final",
    "typing.FrozenSet",
    "typing.Generator",
    "typing.Iterable",
    "typing.Iterator",
    "typing.List",
    "typing.MutableSequence",
    "typing.MutableSet",
    "typing.Optional",
    "typing.Sequence",
    "typing.Set",
    "typing.Tuple",
    "typing.Union",
    "typing_extensions.Annotated",
    "typing_extensions.Final",
    "typing_extensions.NotRequired",
    "typing_extensions.Optional",
    "typing_extensions.Required",
    "typing_extensions.Union",
];

fn is_opaque_dispatch(name: &str) -> bool {
    OPAQUE_DISPATCH.contains(&name) || name.starts_with("runpy.")
}

fn decorator_target(dec: &Node) -> &Node {
    if kind(dec) == "Call" {
        child(dec, "func")
    } else {
        dec
    }
}

fn decorator_is_transparent(dec: &Node, syms: &symbols::Symbols) -> bool {
    let target = decorator_target(dec);
    if !syms.is_unknown(target) && syms.all(target, |v| TRANSPARENT_DECORATORS.contains(&v)) {
        return true;
    }
    // Unresolved @x.setter / @x.deleter / @x.getter are property accessors, not registrations.
    if kind(target) == "Attribute" && matches!(s(target, "attr"), "setter" | "deleter" | "getter") {
        return true;
    }
    // Single-identifier transparent names when imported via alias or builtins.
    if kind(target) == "Name"
        && matches!(
            s(target, "id"),
            "property" | "staticmethod" | "classmethod" | "dataclass" | "overload" | "override"
        )
    {
        return true;
    }
    false
}

fn decorator_matches_any(
    dec: &Node,
    syms: &symbols::Symbols,
    list: &[&str],
    short: &[&str],
) -> bool {
    let target = decorator_target(dec);
    if !syms.is_unknown(target) && syms.any(target, |v| list.contains(&v)) {
        return true;
    }
    let name = if kind(target) == "Name" {
        s(target, "id")
    } else if kind(target) == "Attribute" {
        s(target, "attr")
    } else {
        ""
    };
    short.contains(&name)
}

fn is_pytest_fixture(dec: &Node, syms: &symbols::Symbols) -> bool {
    let target = decorator_target(dec);
    if !syms.is_unknown(target)
        && syms.any(target, |v| {
            v == "pytest.fixture" || v == "pytest_asyncio.fixture" || v.ends_with(".fixture")
        })
    {
        return true;
    }
    (kind(target) == "Attribute" && s(target, "attr") == "fixture")
        || (kind(target) == "Name" && s(target, "id") == "fixture")
}

fn is_static_string_sequence(node: &Node) -> Option<Box<[Box<str>]>> {
    if !matches!(kind(node), "List" | "Tuple") {
        return None;
    }
    let mut out = Vec::new();
    for elt in items(node, "elts") {
        if kind(elt) == "Constant"
            && let Some(text) = elt["value"].as_str()
        {
            out.push(text.into());
        } else {
            return None;
        }
    }
    Some(out.into_boxed_slice())
}

fn is_all_target(node: &Node) -> bool {
    kind(node) == "Name" && s(node, "id") == "__all__"
}

/// Check LIT501 (opaque-dispatch), LIT502 (opaque-exports), LIT503 (untyped-receiver),
/// and LIT513 (unreachable-code), and extract compact `DeadCodeSummary`.
pub(super) fn check_and_extract(
    file: &FileContext<'_>,
    parsed_all: Option<&(Node, symbols::Symbols)>,
    out: &mut Vec<Finding>,
) -> DeadCodeSummary {
    let _span = crate::profile::span("dead_code_local");

    // 1. LIT501: opaque-dispatch in shell, adapter, router, transaction.
    let check_opaque = file.has_layer(&["shell", "adapter", "router", "transaction"]);
    if check_opaque {
        for &node in file.nodes {
            if loaded(file, node)
                && !file.symbols.is_unknown(node)
                && file.symbols.all(node, is_opaque_dispatch)
            {
                let text = file.symbols.joined(node);
                out.push(finding(
                    file,
                    node,
                    "LIT501",
                    format!(
                        "{text} defeats static reachability; move dynamic dispatch to a boundary module"
                    ),
                    Some(text),
                ));
            }
            let k = kind(node);
            if k == "Attribute" && s(node, "attr") == "__dict__" {
                out.push(finding(
                    file,
                    node,
                    "LIT501",
                    "__dict__ defeats static reachability; access attributes explicitly",
                    Some("__dict__".into()),
                ));
            }
            if k == "Call"
                && file.symbols.only(child(node, "func"), "builtins.getattr")
                && items(node, "args")
                    .get(1)
                    .is_none_or(|a| kind(a) != "Constant" || !a["value"].is_string())
            {
                out.push(finding(
                    file,
                    node,
                    "LIT501",
                    "dynamic getattr defeats static reachability; access attributes explicitly or move to a boundary",
                    Some("builtins.getattr".into()),
                ));
            }
        }
        for stmt in items(file.tree, "body") {
            if matches!(kind(stmt), "FunctionDef" | "AsyncFunctionDef")
                && matches!(s(stmt, "name"), "__getattr__" | "__dir__")
            {
                add(
                    out,
                    file,
                    stmt,
                    "LIT501",
                    format!(
                        "module-level {} defeats static reachability; export names explicitly",
                        s(stmt, "name")
                    ),
                );
            }
        }
    }

    // 2. LIT502: opaque-exports across all layers (including marker).
    let mut static_all: Option<Box<[Box<str>]>> = None;
    for &node in file.nodes {
        if kind(node) == "ImportFrom" && items(node, "names").iter().any(|a| s(a, "name") == "*") {
            add(
                out,
                file,
                node,
                "LIT502",
                "wildcard import (`import *`) hides symbol references; import names explicitly",
            );
        }
    }
    for stmt in items(file.tree, "body") {
        match kind(stmt) {
            "Assign" => {
                if items(stmt, "targets").iter().any(is_all_target) {
                    if let Some(names) = is_static_string_sequence(child(stmt, "value")) {
                        static_all = Some(names);
                    } else {
                        add(
                            out,
                            file,
                            stmt,
                            "LIT502",
                            "__all__ must be a static list or tuple of string literals",
                        );
                    }
                }
            }
            "AnnAssign" => {
                if is_all_target(child(stmt, "target")) {
                    let val = child(stmt, "value");
                    if !val.is_null() {
                        if let Some(names) = is_static_string_sequence(val) {
                            static_all = Some(names);
                        } else {
                            add(
                                out,
                                file,
                                stmt,
                                "LIT502",
                                "__all__ must be a static list or tuple of string literals",
                            );
                        }
                    }
                }
            }
            "AugAssign" => {
                if is_all_target(child(stmt, "target")) {
                    add(
                        out,
                        file,
                        stmt,
                        "LIT502",
                        "__all__ must be assigned as a single static list or tuple of string literals",
                    );
                }
            }
            "Expr" => {
                let call = child(stmt, "value");
                if kind(call) == "Call" {
                    let func = child(call, "func");
                    if kind(func) == "Attribute"
                        && is_all_target(child(func, "value"))
                        && matches!(s(func, "attr"), "append" | "extend" | "insert" | "remove")
                    {
                        add(
                            out,
                            file,
                            stmt,
                            "LIT502",
                            "__all__ must be a static list or tuple of string literals; do not mutate it",
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // 3. LIT503: untyped-receiver in shell, adapter, router.
    if file.has_layer(&["shell", "adapter", "router"]) {
        check_untyped_receivers(file, out);
    }

    // 4. LIT513: intra-procedural unreachable code (all non-empty layers).
    if !file.layer().is_empty() {
        check_unreachable_block(file, items(file.tree, "body"), out);
    }

    // 5. Extract compact DeadCodeSummary using parsed string annotations if present.
    let owned_parsed = if parsed_all.is_none() {
        capabilities::parsed_all_annotations(file.tree, file.nodes, file.package)
    } else {
        None
    };
    let (tree, syms): (&Node, &symbols::Symbols) = match parsed_all.or(owned_parsed.as_ref()) {
        Some((t, s)) => (t, s),
        None => (file.tree, file.symbols),
    };

    extract_summary(file, tree, syms, static_all)
}

fn check_untyped_receivers(file: &FileContext<'_>, out: &mut Vec<Finding>) {
    for &node in file.nodes {
        if !matches!(kind(node), "FunctionDef" | "AsyncFunctionDef") {
            continue;
        }
        let mut unannotated: Vec<(&str, &Node)> = Vec::new();
        for param in parameters(node) {
            let name = s(param, "arg");
            if matches!(name, "self" | "cls") || name.starts_with('_') {
                continue;
            }
            if child(param, "annotation").is_null() {
                unannotated.push((name, param));
            }
        }
        if unannotated.is_empty() {
            continue;
        }
        let mut accessed: HashSet<&str> = HashSet::new();
        let wanted: HashSet<&str> = unannotated.iter().map(|(n, _)| *n).collect();
        collect_attr_base_names(items(node, "body"), &wanted, &mut accessed);
        for (name, param_node) in unannotated {
            if accessed.contains(name) {
                add(
                    out,
                    file,
                    param_node,
                    "LIT503",
                    format!(
                        "parameter '{name}' has attributes accessed without a type annotation; annotate it so method dispatch resolves statically"
                    ),
                );
            }
        }
    }
}

fn collect_attr_base_names<'a>(
    stmts: &'a [Node],
    wanted: &HashSet<&str>,
    accessed: &mut HashSet<&'a str>,
) {
    for stmt in stmts {
        visit_attr_bases(stmt, wanted, accessed);
    }
}

fn visit_attr_bases<'a>(node: &'a Node, wanted: &HashSet<&str>, accessed: &mut HashSet<&'a str>) {
    match kind(node) {
        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" | "Lambda" => return,
        "Attribute" => {
            let val = child(node, "value");
            if kind(val) == "Name"
                && kind(child(val, "ctx")) == "Load"
                && wanted.contains(s(val, "id"))
            {
                accessed.insert(s(val, "id"));
            }
        }
        _ => {}
    }
    for_each_child(node, |c| visit_attr_bases(c, wanted, accessed));
}

// ---------------------------------------------------------------------------
// LIT513: Intra-procedural unreachable code (Vulture-style fall-through analysis)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConstBool {
    True,
    False,
    Unknown,
}

fn eval_const_bool(node: &Node) -> ConstBool {
    match kind(node) {
        "Constant" => match &node["value"] {
            Node::Bool(true) => ConstBool::True,
            Node::Bool(false) | Node::Null => ConstBool::False,
            Node::Int(i) => {
                if *i == 0 {
                    ConstBool::False
                } else {
                    ConstBool::True
                }
            }
            Node::Float(f) => {
                if *f == 0.0 {
                    ConstBool::False
                } else {
                    ConstBool::True
                }
            }
            Node::Str(s) => {
                if s.is_empty() {
                    ConstBool::False
                } else {
                    ConstBool::True
                }
            }
            _ => ConstBool::Unknown,
        },
        "UnaryOp" if kind(child(node, "op")) == "Not" => {
            match eval_const_bool(child(node, "operand")) {
                ConstBool::True => ConstBool::False,
                ConstBool::False => ConstBool::True,
                ConstBool::Unknown => ConstBool::Unknown,
            }
        }
        "BoolOp" => {
            let is_and = kind(child(node, "op")) == "And";
            let vals = items(node, "values");
            let mut all_known = true;
            for v in vals {
                match (is_and, eval_const_bool(v)) {
                    (true, ConstBool::False) => return ConstBool::False,
                    (false, ConstBool::True) => return ConstBool::True,
                    (_, ConstBool::Unknown) => all_known = false,
                    _ => {}
                }
            }
            if all_known && !vals.is_empty() {
                if is_and {
                    ConstBool::True
                } else {
                    ConstBool::False
                }
            } else {
                ConstBool::Unknown
            }
        }
        _ => ConstBool::Unknown,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FallThrough {
    Yes,
    /// Terminated exclusively by `raise` (a surrounding `with` could suppress it).
    Raised,
    /// Terminated by `return`/`break`/`continue`/infinite loop.
    Stopped,
}

impl FallThrough {
    fn can_fall(self) -> bool {
        self == FallThrough::Yes
    }
    fn join(a: Self, b: Self) -> Self {
        match (a, b) {
            (Self::Yes, _) | (_, Self::Yes) => Self::Yes,
            (Self::Raised, _) | (_, Self::Raised) => Self::Raised,
            (Self::Stopped, Self::Stopped) => Self::Stopped,
        }
    }
}

fn loop_has_break(stmts: &[Node]) -> bool {
    for stmt in stmts {
        match kind(stmt) {
            "Break" => return true,
            "While" | "For" | "AsyncFor" | "FunctionDef" | "AsyncFunctionDef" | "ClassDef" => {}
            _ => {
                let mut found = false;
                for_each_child(stmt, |c| {
                    if !found {
                        if kind(c) == "Break" {
                            found = true;
                        } else if !matches!(
                            kind(c),
                            "While"
                                | "For"
                                | "AsyncFor"
                                | "FunctionDef"
                                | "AsyncFunctionDef"
                                | "ClassDef"
                        ) {
                            found = loop_has_break(std::slice::from_ref(c));
                        }
                    }
                });
                if found {
                    return true;
                }
            }
        }
    }
    false
}

fn check_unreachable_block(
    file: &FileContext<'_>,
    stmts: &[Node],
    out: &mut Vec<Finding>,
) -> FallThrough {
    for (idx, stmt) in stmts.iter().enumerate() {
        let ft = check_unreachable_stmt(file, stmt, out);
        if !ft.can_fall() {
            if let Some(next) = stmts.get(idx + 1) {
                // `raise ...; yield ...` is the standard Python idiom to force a
                // stub generator / async generator function to compile as a generator.
                let is_generator_marker = kind(stmt) == "Raise"
                    && kind(next) == "Expr"
                    && matches!(kind(child(next, "value")), "Yield" | "YieldFrom");
                if is_generator_marker {
                    return ft;
                }
                let reason = match kind(stmt) {
                    "Return" => "after 'return'",
                    "Raise" => "after 'raise'",
                    "Break" => "after 'break'",
                    "Continue" => "after 'continue'",
                    _ => "after terminating statement",
                };
                add(
                    out,
                    file,
                    next,
                    "LIT513",
                    format!("unreachable code {reason}"),
                );
            }
            return ft;
        }
    }
    FallThrough::Yes
}

fn check_unreachable_stmt(
    file: &FileContext<'_>,
    stmt: &Node,
    out: &mut Vec<Finding>,
) -> FallThrough {
    match kind(stmt) {
        "Return" | "Break" | "Continue" => FallThrough::Stopped,
        "Raise" => FallThrough::Raised,
        "FunctionDef" | "AsyncFunctionDef" | "ClassDef" => {
            check_unreachable_block(file, items(stmt, "body"), out);
            FallThrough::Yes
        }
        "With" | "AsyncWith" => {
            // A context manager's `__exit__` can suppress exceptions raised anywhere
            // inside the block (including inside a `return expr` expression such as
            // `with suppress(ValueError): return UUID(v)`), so execution after `with`
            // is always reachable even if the body ends in `return` or `raise`.
            check_unreachable_block(file, items(stmt, "body"), out);
            FallThrough::Yes
        }
        "For" | "AsyncFor" => {
            check_unreachable_block(file, items(stmt, "body"), out);
            check_unreachable_block(file, items(stmt, "orelse"), out);
            FallThrough::Yes
        }
        "While" => {
            let body = items(stmt, "body");
            let orelse = items(stmt, "orelse");
            match eval_const_bool(child(stmt, "test")) {
                ConstBool::False => {
                    add(
                        out,
                        file,
                        stmt,
                        "LIT513",
                        "unsatisfiable 'while' condition; loop body is unreachable",
                    );
                    check_unreachable_block(file, orelse, out)
                }
                ConstBool::True => {
                    check_unreachable_block(file, body, out);
                    let has_break = loop_has_break(body);
                    if !orelse.is_empty() && !has_break {
                        add(
                            out,
                            file,
                            &orelse[0],
                            "LIT513",
                            "unreachable 'else' block after infinite 'while' loop",
                        );
                    } else {
                        check_unreachable_block(file, orelse, out);
                    }
                    if has_break {
                        FallThrough::Yes
                    } else {
                        FallThrough::Stopped
                    }
                }
                ConstBool::Unknown => {
                    check_unreachable_block(file, body, out);
                    check_unreachable_block(file, orelse, out);
                    FallThrough::Yes
                }
            }
        }
        "If" => {
            let body = items(stmt, "body");
            let orelse = items(stmt, "orelse");
            match eval_const_bool(child(stmt, "test")) {
                ConstBool::False => {
                    add(
                        out,
                        file,
                        stmt,
                        "LIT513",
                        "unsatisfiable 'if' condition; branch body is unreachable",
                    );
                    if orelse.is_empty() {
                        FallThrough::Yes
                    } else {
                        check_unreachable_block(file, orelse, out)
                    }
                }
                ConstBool::True => {
                    let body_ft = check_unreachable_block(file, body, out);
                    if !orelse.is_empty() {
                        add(
                            out,
                            file,
                            &orelse[0],
                            "LIT513",
                            "unreachable 'else' block after always-true 'if' condition",
                        );
                    }
                    body_ft
                }
                ConstBool::Unknown => {
                    let body_ft = check_unreachable_block(file, body, out);
                    let else_ft = if orelse.is_empty() {
                        FallThrough::Yes
                    } else {
                        check_unreachable_block(file, orelse, out)
                    };
                    FallThrough::join(body_ft, else_ft)
                }
            }
        }
        "Try" | "TryStar" => {
            let body_ft = check_unreachable_block(file, items(stmt, "body"), out);
            let orelse = items(stmt, "orelse");
            let else_ft = if !orelse.is_empty() {
                if !body_ft.can_fall() {
                    add(
                        out,
                        file,
                        &orelse[0],
                        "LIT513",
                        "unreachable 'else' block after terminating 'try' body",
                    );
                    body_ft
                } else {
                    check_unreachable_block(file, orelse, out)
                }
            } else {
                body_ft
            };
            let mut combined = else_ft;
            for h in items(stmt, "handlers") {
                let h_ft = check_unreachable_block(file, items(h, "body"), out);
                combined = FallThrough::join(combined, h_ft);
            }
            let final_ft = check_unreachable_block(file, items(stmt, "finalbody"), out);
            if !final_ft.can_fall() {
                final_ft
            } else {
                combined
            }
        }
        "Match" => {
            let mut has_wildcard = false;
            let mut combined = FallThrough::Stopped;
            let cases = items(stmt, "cases");
            for c in cases {
                let pat = child(c, "pattern");
                if child(c, "guard").is_null()
                    && kind(pat) == "MatchAs"
                    && child(pat, "pattern").is_null()
                {
                    has_wildcard = true;
                }
                let c_ft = check_unreachable_block(file, items(c, "body"), out);
                combined = FallThrough::join(combined, c_ft);
            }
            if has_wildcard && !cases.is_empty() {
                combined
            } else {
                FallThrough::Yes
            }
        }
        _ => FallThrough::Yes,
    }
}

// ---------------------------------------------------------------------------
// Compact DeadCodeSummary extraction (runs while AST & Symbols are in memory)
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LocalTypeInfo {
    /// Direct type(s) of a variable: `mod.Class`, `@LocalClass`, `>mod.func`, `>@local_func`,
    /// or `field:Class.attr` (resolved in Phase 2).
    direct: Vec<Box<str>>,
    /// Element type(s) when iterating (`for x in var:`).
    elem: Vec<Box<str>>,
}

struct Extractor<'a> {
    file: &'a FileContext<'a>,
    syms: &'a symbols::Symbols,
    next_id: u32,
    defs: Vec<DeadDef>,
    refs: Vec<DeadRef>,
    attr_reads: Vec<AttrRead>,
    type_flows: Vec<TypeFlow>,
    init_fields: Vec<InitFieldType>,
    escaped_types: BTreeSet<Box<str>>,
    read_params: BTreeSet<u32>,
    top_level_names: HashSet<&'a str>,
    /// Fallback map of module-scope imports (including inside top-level `with`/`if`/`try` blocks
    /// such as Temporal's `with workflow.unsafe.imports_passed_through():`).
    module_imports: HashMap<&'a str, Box<str>>,
    /// Class local name -> (field_name -> LocalTypeInfo) for `self.<field>` resolution.
    class_field_types: HashMap<&'a str, HashMap<String, LocalTypeInfo>>,
}

impl<'a> Extractor<'a> {
    fn alloc_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn resolve_expr_types(&self, node: &Node, env: &HashMap<&str, LocalTypeInfo>) -> LocalTypeInfo {
        self.resolve_expr_types_depth(node, env, 0)
    }

    fn resolve_expr_types_depth(
        &self,
        node: &Node,
        env: &HashMap<&str, LocalTypeInfo>,
        depth: usize,
    ) -> LocalTypeInfo {
        if node.is_null() || depth > 12 {
            return LocalTypeInfo::default();
        }
        match kind(node) {
            "Name" => {
                let id = s(node, "id");
                if let Some(local) = env.get(id)
                    && (!local.direct.is_empty() || !local.elem.is_empty())
                {
                    return local.clone();
                }
                if self.top_level_names.contains(id) {
                    return LocalTypeInfo {
                        direct: vec![format!("@{id}").into()],
                        elem: vec![],
                    };
                }
                let resolved = self.syms.resolved_box(node);
                if !resolved.is_empty() {
                    return LocalTypeInfo {
                        direct: resolved.into_vec(),
                        elem: vec![],
                    };
                }
                if let Some(imp) = self.module_imports.get(id) {
                    return LocalTypeInfo {
                        direct: vec![imp.clone()],
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            "Attribute" => {
                let attr = s(node, "attr");
                let base_node = child(node, "value");
                let base_info = self.resolve_expr_types_depth(base_node, env, depth + 1);
                if !base_info.direct.is_empty() {
                    let mut direct = Vec::new();
                    let mut elem = Vec::new();
                    for t in &base_info.direct {
                        if let Some(cls_name) = t.strip_prefix('@')
                            && !cls_name.contains('.')
                            && let Some(fields) = self.class_field_types.get(cls_name)
                            && let Some(finfo) = fields.get(attr)
                        {
                            direct.extend(finfo.direct.iter().cloned());
                            elem.extend(finfo.elem.iter().cloned());
                            continue;
                        }
                        // Encode chained attribute access `.<receiver>.<attr>` for Phase 2 resolution
                        // (handles `self._repos.agent`, `uow.deals`, `mod.CONST`, etc.).
                        direct.push(format!(".{t}.{attr}").into());
                    }
                    if !direct.is_empty() || !elem.is_empty() {
                        return LocalTypeInfo { direct, elem };
                    }
                }
                let resolved = self.syms.resolved_box(node);
                if !resolved.is_empty() {
                    return LocalTypeInfo {
                        direct: resolved.into_vec(),
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            "Subscript" => {
                let val = child(node, "value");
                let base_info = self.resolve_expr_types_depth(val, env, depth + 1);
                if !base_info.elem.is_empty() {
                    return LocalTypeInfo {
                        direct: base_info.elem,
                        elem: vec![],
                    };
                }
                if !base_info.direct.is_empty() {
                    let direct = base_info
                        .direct
                        .into_iter()
                        .map(|t| format!("[{t}]").into())
                        .collect();
                    return LocalTypeInfo {
                        direct,
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            "Await" => self.resolve_expr_types_depth(child(node, "value"), env, depth + 1),
            "Call" => {
                let func = child(node, "func");
                // Mapping/container lookup helpers: `d.get(k)`, `d.pop(k)`, `d.setdefault(k, v)`, `d.values()`.
                if kind(func) == "Attribute"
                    && matches!(s(func, "attr"), "get" | "pop" | "setdefault" | "values")
                {
                    let container =
                        self.resolve_expr_types_depth(child(func, "value"), env, depth + 1);
                    if !container.elem.is_empty() {
                        return if s(func, "attr") == "values" {
                            LocalTypeInfo {
                                direct: vec![],
                                elem: container.elem,
                            }
                        } else {
                            LocalTypeInfo {
                                direct: container.elem,
                                elem: vec![],
                            }
                        };
                    }
                    if !container.direct.is_empty() {
                        let mapped: Vec<Box<str>> = container
                            .direct
                            .into_iter()
                            .map(|t| format!("[{t}]").into())
                            .collect();
                        return if s(func, "attr") == "values" {
                            LocalTypeInfo {
                                direct: vec![],
                                elem: mapped,
                            }
                        } else {
                            LocalTypeInfo {
                                direct: mapped,
                                elem: vec![],
                            }
                        };
                    }
                }
                // Direct constructor or function/method call: mark as `><callee>` so Phase 2 resolves
                // either to the Class itself or to the function/method's return type(s).
                let callee = self.resolve_expr_types_depth(func, env, depth + 1);
                if !callee.direct.is_empty() {
                    let mut out = Vec::with_capacity(callee.direct.len());
                    for c in callee.direct {
                        out.push(format!(">{c}").into());
                    }
                    return LocalTypeInfo {
                        direct: out,
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            _ => LocalTypeInfo::default(),
        }
    }

    fn resolve_annotation_types(&self, ann: &Node, owning_class: Option<&str>) -> LocalTypeInfo {
        if ann.is_null() {
            return LocalTypeInfo::default();
        }
        match kind(ann) {
            "BinOp" if kind(child(ann, "op")) == "BitOr" => {
                let mut left = self.resolve_annotation_types(child(ann, "left"), owning_class);
                let right = self.resolve_annotation_types(child(ann, "right"), owning_class);
                left.direct.extend(right.direct);
                left.elem.extend(right.elem);
                left
            }
            "Subscript" => {
                let val = child(ann, "value");
                let sl = child(ann, "slice");
                let is_literal = self.syms.any(val, |v| {
                    v == "typing.Literal" || v == "typing_extensions.Literal"
                });
                if is_literal {
                    return LocalTypeInfo::default();
                }
                let is_annotated = self.syms.any(val, |v| {
                    v == "typing.Annotated" || v == "typing_extensions.Annotated"
                });
                if is_annotated {
                    let first = if kind(sl) == "Tuple" {
                        items(sl, "elts").first().unwrap_or(sl)
                    } else {
                        sl
                    };
                    return self.resolve_annotation_types(first, owning_class);
                }
                // Unwrap transparent wrappers: Optional, Union, ClassVar, Final, Awaitable,
                // type[T], ContextManager[T], AsyncContextManager[T], Generator[T, ...], AsyncGenerator[T, ...].
                let is_first_arg_wrapper = self.syms.any(val, |v| {
                    matches!(
                        v,
                        "builtins.type"
                            | "typing.Type"
                            | "contextlib.AbstractAsyncContextManager"
                            | "contextlib.AbstractContextManager"
                            | "typing.AsyncContextManager"
                            | "typing.ContextManager"
                            | "typing.AsyncGenerator"
                            | "typing.Generator"
                            | "collections.abc.AsyncGenerator"
                            | "collections.abc.Generator"
                    )
                });
                if is_first_arg_wrapper {
                    let first = if kind(sl) == "Tuple" {
                        items(sl, "elts").first().unwrap_or(sl)
                    } else {
                        sl
                    };
                    return self.resolve_annotation_types(first, owning_class);
                }
                let is_union_or_opt = self.syms.any(val, |v| {
                    matches!(
                        v,
                        "typing.Optional"
                            | "typing.Union"
                            | "typing.ClassVar"
                            | "typing.Final"
                            | "typing.Awaitable"
                            | "typing_extensions.Optional"
                            | "typing_extensions.Union"
                            | "typing_extensions.Final"
                            | "typing_extensions.Required"
                            | "typing_extensions.NotRequired"
                            | "collections.abc.Awaitable"
                    )
                });
                if is_union_or_opt {
                    let mut out = LocalTypeInfo::default();
                    if kind(sl) == "Tuple" {
                        for elt in items(sl, "elts") {
                            let r = self.resolve_annotation_types(elt, owning_class);
                            out.direct.extend(r.direct);
                            out.elem.extend(r.elem);
                        }
                    } else {
                        out = self.resolve_annotation_types(sl, owning_class);
                    }
                    return out;
                }
                let is_sequence = self.syms.any(val, |v| {
                    matches!(
                        v,
                        "builtins.list"
                            | "builtins.tuple"
                            | "builtins.set"
                            | "builtins.frozenset"
                            | "typing.List"
                            | "typing.Tuple"
                            | "typing.Set"
                            | "typing.FrozenSet"
                            | "typing.Sequence"
                            | "typing.MutableSequence"
                            | "typing.Iterable"
                            | "typing.Iterator"
                            | "typing.AsyncIterable"
                            | "typing.AsyncIterator"
                            | "typing.Collection"
                            | "collections.abc.Sequence"
                            | "collections.abc.MutableSequence"
                            | "collections.abc.Iterable"
                            | "collections.abc.Iterator"
                            | "collections.abc.AsyncIterable"
                            | "collections.abc.AsyncIterator"
                            | "collections.abc.Collection"
                            | "collections.abc.Set"
                            | "collections.abc.MutableSet"
                    )
                });
                if is_sequence {
                    let elem_node = if kind(sl) == "Tuple" {
                        items(sl, "elts").first().unwrap_or(sl)
                    } else {
                        sl
                    };
                    let elem = self.resolve_annotation_types(elem_node, owning_class);
                    return LocalTypeInfo {
                        direct: vec![],
                        elem: elem.direct,
                    };
                }
                let is_mapping = self.syms.any(val, |v| {
                    matches!(
                        v,
                        "builtins.dict"
                            | "typing.Dict"
                            | "typing.Mapping"
                            | "typing.MutableMapping"
                            | "collections.abc.Mapping"
                            | "collections.abc.MutableMapping"
                    )
                });
                if is_mapping {
                    if kind(sl) == "Tuple"
                        && let Some(v_node) = items(sl, "elts").get(1)
                    {
                        let val_ty = self.resolve_annotation_types(v_node, owning_class);
                        return LocalTypeInfo {
                            direct: vec![],
                            elem: val_ty.direct,
                        };
                    }
                    return LocalTypeInfo::default();
                }
                let is_callable = self.syms.any(val, |v| {
                    matches!(
                        v,
                        "collections.abc.Callable"
                            | "typing.Callable"
                            | "typing_extensions.Callable"
                    )
                });
                if is_callable {
                    if kind(sl) == "Tuple"
                        && let Some(ret_node) = items(sl, "elts").get(1)
                    {
                        return self.resolve_annotation_types(ret_node, owning_class);
                    }
                    return LocalTypeInfo::default();
                }
                // Generic workspace class `MyGeneric[T]` -> resolves to `MyGeneric`!
                self.resolve_annotation_types(val, owning_class)
            }
            "Name" => {
                let id = s(ann, "id");
                if id == "None" {
                    return LocalTypeInfo::default();
                }
                if (id == "Self"
                    || self
                        .syms
                        .any(ann, |v| v == "typing.Self" || v == "typing_extensions.Self"))
                    && let Some(cls) = owning_class
                {
                    return LocalTypeInfo {
                        direct: vec![format!("@{cls}").into()],
                        elem: vec![],
                    };
                }
                if self.top_level_names.contains(id) {
                    return LocalTypeInfo {
                        direct: vec![format!("@{id}").into()],
                        elem: vec![],
                    };
                }
                let resolved = self.syms.resolved_box(ann);
                if !resolved.is_empty() {
                    let filtered: Vec<Box<str>> = resolved
                        .into_vec()
                        .into_iter()
                        .filter(|s| !WRAPPER_TYPES.contains(&&**s) && &**s != "builtins.None")
                        .collect();
                    return LocalTypeInfo {
                        direct: filtered,
                        elem: vec![],
                    };
                }
                if let Some(imp) = self.module_imports.get(id)
                    && !WRAPPER_TYPES.contains(&&**imp)
                {
                    return LocalTypeInfo {
                        direct: vec![imp.clone()],
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            "Attribute" => {
                let resolved = self.syms.resolved_box(ann);
                if !resolved.is_empty() {
                    return LocalTypeInfo {
                        direct: resolved.into_vec(),
                        elem: vec![],
                    };
                }
                LocalTypeInfo::default()
            }
            _ => LocalTypeInfo::default(),
        }
    }

    fn record_type_param_refs(&mut self, owner_id: u32, node: &Node) {
        for tp in items(node, "type_params") {
            self.record_annotation_refs(owner_id, child(tp, "bound"));
            self.record_annotation_refs(owner_id, child(tp, "default_value"));
        }
    }

    fn record_literal_refs(&mut self, owner_id: u32, node: &Node) {
        if node.is_null() {
            return;
        }
        match kind(node) {
            // String/number/bool constants inside `Literal[...]` are values, never forward refs.
            "Constant" => {}
            "Name" | "Attribute" => self.record_annotation_refs(owner_id, node),
            _ => for_each_child(node, |c| self.record_literal_refs(owner_id, c)),
        }
    }

    fn record_annotation_refs(&mut self, owner_id: u32, ann: &Node) {
        if ann.is_null() {
            return;
        }
        match kind(ann) {
            "Subscript" => {
                let val = child(ann, "value");
                self.record_annotation_refs(owner_id, val);
                if self.syms.any(val, |v| {
                    v == "typing.Literal" || v == "typing_extensions.Literal"
                }) {
                    self.record_literal_refs(owner_id, child(ann, "slice"));
                    return;
                }
                self.record_annotation_refs(owner_id, child(ann, "slice"));
                return;
            }
            "Name" => {
                let id = s(ann, "id");
                if self.top_level_names.contains(id) {
                    self.refs.push(DeadRef {
                        owner_id,
                        target: format!("@{id}").into(),
                        as_value: false,
                    });
                    return;
                }
                if !self.syms.is_unknown(ann) {
                    self.syms.for_each(ann, |r| {
                        if r != "?" && !r.starts_with("builtins.") {
                            self.refs.push(DeadRef {
                                owner_id,
                                target: r.into(),
                                as_value: false,
                            });
                        }
                    });
                } else if let Some(imp) = self.module_imports.get(id) {
                    self.refs.push(DeadRef {
                        owner_id,
                        target: imp.clone(),
                        as_value: false,
                    });
                }
                return;
            }
            "Attribute" if !self.syms.is_unknown(ann) => {
                self.syms.for_each(ann, |r| {
                    if r != "?" && !r.starts_with("builtins.") {
                        self.refs.push(DeadRef {
                            owner_id,
                            target: r.into(),
                            as_value: false,
                        });
                    }
                });
                return;
            }
            _ => {}
        }
        for_each_child(ann, |c| self.record_annotation_refs(owner_id, c));
    }

    fn is_stub_body(body: &[Node]) -> bool {
        let mut stmts = body;
        if let Some(first) = stmts.first()
            && kind(first) == "Expr"
            && kind(child(first, "value")) == "Constant"
            && child(first, "value")["value"].is_string()
        {
            stmts = &stmts[1..];
        }
        if stmts.is_empty() {
            return true;
        }
        if stmts.len() != 1 {
            return false;
        }
        match kind(&stmts[0]) {
            "Pass" => true,
            "Expr" => {
                let v = child(&stmts[0], "value");
                kind(v) == "Constant" && matches!(v["value"], Node::Ellipsis)
            }
            "Return" => {
                let v = child(&stmts[0], "value");
                v.is_null()
                    || kind(v) == "Constant"
                    || (matches!(kind(v), "List" | "Tuple" | "Set") && items(v, "elts").is_empty())
                    || (kind(v) == "Dict" && items(v, "keys").is_empty())
            }
            "Raise" => {
                let exc = child(&stmts[0], "exc");
                let target = if kind(exc) == "Call" {
                    child(exc, "func")
                } else {
                    exc
                };
                (kind(target) == "Name" && s(target, "id") == "NotImplementedError")
                    || (kind(target) == "Attribute" && s(target, "attr") == "NotImplementedError")
            }
            _ => false,
        }
    }

    fn classify_class_flags(&self, cls: &Node) -> (u16, Box<[Box<str>]>) {
        let mut flags = 0u16;
        for dec in items(cls, "decorator_list") {
            let target = decorator_target(dec);
            if self.syms.only(target, "dataclasses.dataclass")
                || (kind(target) == "Name" && s(target, "id") == "dataclass")
            {
                flags |= def_flags::IS_DATACLASS;
            }
            if !decorator_is_transparent(dec, self.syms) {
                flags |= def_flags::HAS_REG_DECORATOR;
            }
        }
        let mut bases = Vec::new();
        for base in items(cls, "bases") {
            let target = if kind(base) == "Subscript" {
                child(base, "value")
            } else {
                base
            };
            let resolved = self.syms.resolved_box(target);
            if resolved.is_empty() {
                if kind(target) == "Name" {
                    let name = s(target, "id");
                    if self.top_level_names.contains(name) {
                        bases.push(format!("@{name}").into());
                    } else {
                        bases.push(name.into());
                    }
                    if name == "Protocol" {
                        flags |= def_flags::IS_PROTOCOL;
                    } else if name == "NamedTuple" {
                        flags |= def_flags::IS_NAMED_TUPLE;
                    } else if name == "TypedDict" {
                        flags |= def_flags::IS_TYPED_DICT;
                    } else if name.ends_with("Error") || name.ends_with("Exception") {
                        flags |= def_flags::IS_EXCEPTION;
                    }
                }
            } else {
                for r in resolved.iter() {
                    bases.push(r.clone());
                    if matches!(&**r, "typing.Protocol" | "typing_extensions.Protocol") {
                        flags |= def_flags::IS_PROTOCOL;
                    } else if matches!(&**r, "typing.NamedTuple" | "typing_extensions.NamedTuple") {
                        flags |= def_flags::IS_NAMED_TUPLE;
                    } else if matches!(&**r, "typing.TypedDict" | "typing_extensions.TypedDict") {
                        flags |= def_flags::IS_TYPED_DICT;
                    } else if ENUM_BASES.contains(&&**r) {
                        flags |= def_flags::IS_ENUM;
                    } else if PYDANTIC_BASES.contains(&&**r) {
                        flags |= def_flags::IS_PYDANTIC;
                    } else if ORM_BASES.contains(&&**r) {
                        flags |= def_flags::IS_ORM;
                    } else if matches!(&**r, "builtins.Exception" | "builtins.BaseException") {
                        flags |= def_flags::IS_EXCEPTION;
                    }
                }
            }
        }
        (flags, bases.into_boxed_slice())
    }

    fn visit_expr(
        &mut self,
        owner_id: u32,
        node: &Node,
        env: &HashMap<&str, LocalTypeInfo>,
        local_scopes: &[HashSet<&str>],
        param_map: &HashMap<&str, u32>,
        in_call_func: bool,
    ) {
        if node.is_null() {
            return;
        }
        let k = kind(node);
        match k {
            "Name" => {
                let ctx = kind(child(node, "ctx"));
                if matches!(ctx, "Load" | "Del") {
                    let id = s(node, "id");
                    if let Some(&pid) = param_map.get(id) {
                        self.read_params.insert(pid);
                    }
                    let shadowed = local_scopes.iter().any(|sc| sc.contains(id));
                    if !shadowed {
                        if self.top_level_names.contains(id) {
                            self.refs.push(DeadRef {
                                owner_id,
                                target: format!("@{id}").into(),
                                as_value: !in_call_func,
                            });
                        } else if !self.syms.is_unknown(node) {
                            self.syms.for_each(node, |r| {
                                if r != "?" && !r.starts_with("builtins.") {
                                    self.refs.push(DeadRef {
                                        owner_id,
                                        target: r.into(),
                                        as_value: !in_call_func,
                                    });
                                }
                            });
                        } else if let Some(imp) = self.module_imports.get(id) {
                            self.refs.push(DeadRef {
                                owner_id,
                                target: imp.clone(),
                                as_value: !in_call_func,
                            });
                        }
                    }
                }
                return;
            }
            "Attribute" => {
                let ctx = kind(child(node, "ctx"));
                let val = child(node, "value");
                let attr = s(node, "attr");
                // Visit receiver expression first (it's loaded even when setting `self.x = 1`).
                self.visit_expr(owner_id, val, env, local_scopes, param_map, false);
                if matches!(ctx, "Load" | "Del") {
                    // Direct module/symbol reference (e.g., `mod.func` or `MyClass.method`).
                    if !self.syms.is_unknown(node) {
                        self.syms.for_each(node, |r| {
                            if r != "?" && !r.starts_with("builtins.") {
                                self.refs.push(DeadRef {
                                    owner_id,
                                    target: r.into(),
                                    as_value: !in_call_func,
                                });
                            }
                        });
                    }
                    // Check for `super().method`
                    let is_super =
                        kind(val) == "Call" && self.syms.only(child(val, "func"), "builtins.super");
                    if is_super {
                        let recv = env
                            .get("self")
                            .or_else(|| env.get("cls"))
                            .map(|t| t.direct.clone().into_boxed_slice())
                            .unwrap_or_default();
                        self.attr_reads.push(AttrRead {
                            owner_id,
                            receiver_types: recv,
                            attr: attr.into(),
                            is_super: true,
                        });
                    } else {
                        let recv_info = self.resolve_expr_types(val, env);
                        self.attr_reads.push(AttrRead {
                            owner_id,
                            receiver_types: recv_info.direct.into_boxed_slice(),
                            attr: attr.into(),
                            is_super: false,
                        });
                    }
                }
                return;
            }
            "Call" => {
                let func = child(node, "func");
                self.visit_expr(owner_id, func, env, local_scopes, param_map, true);

                // Check literal getattr(obj, "attr") as an attribute read.
                if self.syms.only(func, "builtins.getattr")
                    && let [obj, attr_node, ..] = items(node, "args")
                    && kind(attr_node) == "Constant"
                    && let Some(attr) = attr_node["value"].as_str()
                {
                    let recv = self.resolve_expr_types(obj, env);
                    self.attr_reads.push(AttrRead {
                        owner_id,
                        receiver_types: recv.direct.into_boxed_slice(),
                        attr: attr.into(),
                        is_super: false,
                    });
                }

                // Serialization escape tracking: dataclasses.asdict(x), pydantic.TypeAdapter(X), etc.
                if !self.syms.is_unknown(func)
                    && self.syms.any(func, |v| SERIALIZE_FUNCS.contains(&v))
                    && let Some(first_arg) = items(node, "args").first()
                {
                    let ty = self.resolve_expr_types(first_arg, env);
                    for t in ty.direct {
                        self.escaped_types.insert(t);
                    }
                    for t in ty.elem {
                        self.escaped_types.insert(t);
                    }
                }
                // `model_validate(x, from_attributes=True)` or template `.render(...)`
                if kind(func) == "Attribute"
                    && matches!(s(func, "attr"), "model_validate" | "from_orm")
                    && let Some(first_arg) = items(node, "args").first()
                {
                    let ty = self.resolve_expr_types(first_arg, env);
                    for t in ty.direct {
                        self.escaped_types.insert(t);
                    }
                }
                if kind(func) == "Attribute" && s(func, "attr") == "render" {
                    for arg in items(node, "args") {
                        let ty = self.resolve_expr_types(arg, env);
                        for t in ty.direct.into_iter().chain(ty.elem) {
                            self.escaped_types.insert(t);
                        }
                    }
                    for kw in items(node, "keywords") {
                        let ty = self.resolve_expr_types(child(kw, "value"), env);
                        for t in ty.direct.into_iter().chain(ty.elem) {
                            self.escaped_types.insert(t);
                        }
                    }
                }

                // Record argument type flows for Protocol wiring: `callee(arg)` -> callee's param type.
                let callee_names = self.resolve_expr_types(func, env).direct;
                for (pos, arg) in items(node, "args").iter().enumerate() {
                    self.visit_expr(owner_id, arg, env, local_scopes, param_map, false);
                    let arg_types = self.resolve_expr_types(arg, env).direct;
                    for ct in &callee_names {
                        for at in &arg_types {
                            self.type_flows.push(TypeFlow {
                                owner_id,
                                from_type: at.clone(),
                                to_type: format!("param:{ct}#{pos}").into(),
                            });
                        }
                    }
                }
                for kw in items(node, "keywords") {
                    let kw_val = child(kw, "value");
                    self.visit_expr(owner_id, kw_val, env, local_scopes, param_map, false);
                    let kw_name = s(kw, "arg");
                    if !kw_name.is_empty() {
                        let arg_types = self.resolve_expr_types(kw_val, env).direct;
                        for ct in &callee_names {
                            for at in &arg_types {
                                self.type_flows.push(TypeFlow {
                                    owner_id,
                                    from_type: at.clone(),
                                    to_type: format!("kwparam:{ct}.{kw_name}").into(),
                                });
                            }
                        }
                    }
                }
                return;
            }
            "Lambda" => {
                let mut inner_scopes = local_scopes.to_vec();
                let mut lambda_locals = HashSet::new();
                for p in parameters(node) {
                    lambda_locals.insert(s(p, "arg"));
                }
                let args = child(node, "args");
                for def in items(args, "defaults")
                    .iter()
                    .chain(items(args, "kw_defaults"))
                {
                    self.visit_expr(owner_id, def, env, local_scopes, param_map, false);
                }
                inner_scopes.push(lambda_locals);
                self.visit_expr(
                    owner_id,
                    child(node, "body"),
                    env,
                    &inner_scopes,
                    param_map,
                    false,
                );
                return;
            }
            "ListComp" | "SetComp" | "DictComp" | "GeneratorExp" => {
                let mut inner_env = env.clone();
                let mut inner_scopes = local_scopes.to_vec();
                let mut comp_locals = HashSet::new();
                for generator in items(node, "generators") {
                    let iter_node = child(generator, "iter");
                    self.visit_expr(
                        owner_id,
                        iter_node,
                        &inner_env,
                        &inner_scopes,
                        param_map,
                        false,
                    );
                    let iter_ty = self.resolve_expr_types(iter_node, &inner_env);
                    let target = child(generator, "target");
                    collect_pattern_names(target, &mut comp_locals);
                    if kind(target) == "Name" && !iter_ty.elem.is_empty() {
                        inner_env.insert(
                            s(target, "id"),
                            LocalTypeInfo {
                                direct: iter_ty.elem,
                                elem: vec![],
                            },
                        );
                    }
                    inner_scopes.push(comp_locals.clone());
                    for cond in items(generator, "ifs") {
                        self.visit_expr(
                            owner_id,
                            cond,
                            &inner_env,
                            &inner_scopes,
                            param_map,
                            false,
                        );
                    }
                }
                if k == "DictComp" {
                    self.visit_expr(
                        owner_id,
                        child(node, "key"),
                        &inner_env,
                        &inner_scopes,
                        param_map,
                        false,
                    );
                    self.visit_expr(
                        owner_id,
                        child(node, "value"),
                        &inner_env,
                        &inner_scopes,
                        param_map,
                        false,
                    );
                } else {
                    self.visit_expr(
                        owner_id,
                        child(node, "elt"),
                        &inner_env,
                        &inner_scopes,
                        param_map,
                        false,
                    );
                }
                return;
            }
            _ => {}
        }
        for_each_child(node, |c| {
            self.visit_expr(owner_id, c, env, local_scopes, param_map, false)
        });
    }

    fn visit_stmts(
        &mut self,
        owner_id: u32,
        stmts: &'a [Node],
        env: &mut HashMap<&'a str, LocalTypeInfo>,
        local_scopes: &mut Vec<HashSet<&'a str>>,
        param_map: &HashMap<&'a str, u32>,
        in_boundary_return: bool,
    ) {
        for stmt in stmts {
            match kind(stmt) {
                "Assign" => {
                    let val = child(stmt, "value");
                    self.visit_expr(owner_id, val, env, local_scopes, param_map, false);
                    let val_ty = self.resolve_expr_types(val, env);
                    for target in items(stmt, "targets") {
                        self.visit_expr(owner_id, target, env, local_scopes, param_map, false);
                        if kind(target) == "Name" {
                            let name = s(target, "id");
                            if let Some(top) = local_scopes.last_mut() {
                                top.insert(name);
                            }
                            if !val_ty.direct.is_empty() || !val_ty.elem.is_empty() {
                                env.insert(name, val_ty.clone());
                            }
                        }
                    }
                }
                "AnnAssign" => {
                    let ann = child(stmt, "annotation");
                    self.record_annotation_refs(owner_id, ann);
                    let val = child(stmt, "value");
                    if !val.is_null() {
                        self.visit_expr(owner_id, val, env, local_scopes, param_map, false);
                    }
                    let target = child(stmt, "target");
                    self.visit_expr(owner_id, target, env, local_scopes, param_map, false);
                    let ann_ty = self.resolve_annotation_types(ann, None);
                    let val_ty = if !val.is_null() {
                        self.resolve_expr_types(val, env)
                    } else {
                        LocalTypeInfo::default()
                    };
                    // Record wiring flow if concrete value is assigned to annotated variable (e.g. `repo: OrderRepo = PgOrderRepo()`).
                    for ft in &val_ty.direct {
                        for tt in &ann_ty.direct {
                            self.type_flows.push(TypeFlow {
                                owner_id,
                                from_type: ft.clone(),
                                to_type: tt.clone(),
                            });
                        }
                    }
                    if kind(target) == "Name" {
                        let name = s(target, "id");
                        if let Some(top) = local_scopes.last_mut() {
                            top.insert(name);
                        }
                        if !ann_ty.direct.is_empty() || !ann_ty.elem.is_empty() {
                            env.insert(name, ann_ty);
                        } else if !val_ty.direct.is_empty() || !val_ty.elem.is_empty() {
                            env.insert(name, val_ty);
                        }
                    }
                }
                "Return" => {
                    let val = child(stmt, "value");
                    if !val.is_null() {
                        self.visit_expr(owner_id, val, env, local_scopes, param_map, false);
                        if in_boundary_return {
                            let ty = self.resolve_expr_types(val, env);
                            for t in ty.direct.into_iter().chain(ty.elem) {
                                self.escaped_types.insert(t);
                            }
                        }
                    }
                }
                "For" | "AsyncFor" => {
                    let iter_node = child(stmt, "iter");
                    self.visit_expr(owner_id, iter_node, env, local_scopes, param_map, false);
                    let iter_ty = self.resolve_expr_types(iter_node, env);
                    let target = child(stmt, "target");
                    if let Some(top) = local_scopes.last_mut() {
                        collect_pattern_names(target, top);
                    }
                    if kind(target) == "Name" && !iter_ty.elem.is_empty() {
                        env.insert(
                            s(target, "id"),
                            LocalTypeInfo {
                                direct: iter_ty.elem,
                                elem: vec![],
                            },
                        );
                    }
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "orelse"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "With" | "AsyncWith" => {
                    let is_async = kind(stmt) == "AsyncWith";
                    for item in items(stmt, "items") {
                        let ctx_expr = child(item, "context_expr");
                        self.visit_expr(owner_id, ctx_expr, env, local_scopes, param_map, false);
                        let opt_var = child(item, "optional_vars");
                        if !opt_var.is_null() {
                            if let Some(top) = local_scopes.last_mut() {
                                collect_pattern_names(opt_var, top);
                            }
                            if kind(opt_var) == "Name" {
                                let mut ctx_ty = self.resolve_expr_types(ctx_expr, env);
                                // Also include `__aenter__` / `__enter__` return type on the context manager.
                                let enter_method =
                                    if is_async { "__aenter__" } else { "__enter__" };
                                let mut extra: Vec<Box<str>> = Vec::new();
                                for t in &ctx_ty.direct {
                                    extra.push(format!(">.{t}.{enter_method}").into());
                                }
                                ctx_ty.direct.extend(extra);
                                if !ctx_ty.direct.is_empty() || !ctx_ty.elem.is_empty() {
                                    env.insert(s(opt_var, "id"), ctx_ty);
                                }
                            }
                        }
                    }
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "If" => {
                    self.visit_expr(
                        owner_id,
                        child(stmt, "test"),
                        env,
                        local_scopes,
                        param_map,
                        false,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "orelse"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "While" => {
                    self.visit_expr(
                        owner_id,
                        child(stmt, "test"),
                        env,
                        local_scopes,
                        param_map,
                        false,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "orelse"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "Try" | "TryStar" => {
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    for h in items(stmt, "handlers") {
                        self.visit_expr(
                            owner_id,
                            child(h, "type"),
                            env,
                            local_scopes,
                            param_map,
                            false,
                        );
                        if !child(h, "name").is_null()
                            && let Some(top) = local_scopes.last_mut()
                        {
                            top.insert(s(h, "name"));
                        }
                        self.visit_stmts(
                            owner_id,
                            items(h, "body"),
                            env,
                            local_scopes,
                            param_map,
                            in_boundary_return,
                        );
                    }
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "orelse"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "finalbody"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "Match" => {
                    let subj = child(stmt, "subject");
                    self.visit_expr(owner_id, subj, env, local_scopes, param_map, false);
                    for c in items(stmt, "cases") {
                        self.visit_match_pattern(
                            owner_id,
                            child(c, "pattern"),
                            env,
                            local_scopes,
                            param_map,
                        );
                        self.visit_expr(
                            owner_id,
                            child(c, "guard"),
                            env,
                            local_scopes,
                            param_map,
                            false,
                        );
                        self.visit_stmts(
                            owner_id,
                            items(c, "body"),
                            env,
                            local_scopes,
                            param_map,
                            in_boundary_return,
                        );
                    }
                }
                "FunctionDef" | "AsyncFunctionDef" => {
                    // Nested helper function: attribute all its references to the enclosing owner_id!
                    let name = s(stmt, "name");
                    if let Some(top) = local_scopes.last_mut() {
                        top.insert(name);
                    }
                    for dec in items(stmt, "decorator_list") {
                        self.visit_expr(owner_id, dec, env, local_scopes, param_map, false);
                    }
                    let args = child(stmt, "args");
                    for def in items(args, "defaults")
                        .iter()
                        .chain(items(args, "kw_defaults"))
                    {
                        self.visit_expr(owner_id, def, env, local_scopes, param_map, false);
                    }
                    let mut nested_locals = HashSet::new();
                    let mut nested_env = env.clone();
                    for p in parameters(stmt) {
                        let pname = s(p, "arg");
                        nested_locals.insert(pname);
                        let ann = child(p, "annotation");
                        self.record_annotation_refs(owner_id, ann);
                        let pty = self.resolve_annotation_types(ann, None);
                        if !pty.direct.is_empty() || !pty.elem.is_empty() {
                            nested_env.insert(pname, pty);
                        }
                    }
                    self.record_type_param_refs(owner_id, stmt);
                    self.record_annotation_refs(owner_id, child(stmt, "returns"));
                    local_scopes.push(nested_locals);
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        &mut nested_env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                    local_scopes.pop();
                }
                "ClassDef" => {
                    let name = s(stmt, "name");
                    if let Some(top) = local_scopes.last_mut() {
                        top.insert(name);
                    }
                    for b in items(stmt, "bases")
                        .iter()
                        .chain(items(stmt, "decorator_list"))
                    {
                        self.visit_expr(owner_id, b, env, local_scopes, param_map, false);
                    }
                    self.visit_stmts(
                        owner_id,
                        items(stmt, "body"),
                        env,
                        local_scopes,
                        param_map,
                        in_boundary_return,
                    );
                }
                "Import" | "ImportFrom" => {
                    // Local import inside a function (e.g., boundary/test): add to local scope so
                    // it doesn't accidentally shadow or miss refs, and record the module/symbol ref.
                    if kind(stmt) == "ImportFrom"
                        && let Some(module) = symbols::absolute_module(stmt, self.file.package)
                    {
                        for a in items(stmt, "names") {
                            let orig = s(a, "name");
                            if orig != "*" {
                                self.refs.push(DeadRef {
                                    owner_id,
                                    target: format!("{module}.{orig}").into(),
                                    as_value: false,
                                });
                            }
                        }
                    } else if kind(stmt) == "Import" {
                        for a in items(stmt, "names") {
                            self.refs.push(DeadRef {
                                owner_id,
                                target: s(a, "name").into(),
                                as_value: false,
                            });
                        }
                    }
                }
                _ => {
                    for_each_child(stmt, |c| {
                        self.visit_expr(owner_id, c, env, local_scopes, param_map, false)
                    });
                }
            }
        }
    }

    fn visit_match_pattern(
        &mut self,
        owner_id: u32,
        pat: &'a Node,
        env: &HashMap<&str, LocalTypeInfo>,
        local_scopes: &mut Vec<HashSet<&'a str>>,
        param_map: &HashMap<&str, u32>,
    ) {
        if pat.is_null() {
            return;
        }
        match kind(pat) {
            "MatchClass" => {
                let cls = child(pat, "cls");
                self.visit_expr(owner_id, cls, env, local_scopes, param_map, false);
                let cls_ty = self.resolve_expr_types(cls, env).direct;
                for kw in items(pat, "kwd_attrs") {
                    if let Some(attr) = kw.as_str() {
                        self.attr_reads.push(AttrRead {
                            owner_id,
                            receiver_types: cls_ty.clone().into_boxed_slice(),
                            attr: attr.into(),
                            is_super: false,
                        });
                    }
                }
                // Positional match patterns (`case Order(x, y):`) read fields -> mark class escaped
                // so we never falsely flag positionally-matched fields.
                if !items(pat, "patterns").is_empty() {
                    for t in cls_ty {
                        self.escaped_types.insert(t);
                    }
                }
            }
            "MatchAs" | "MatchStar" => {
                let n = s(pat, "name");
                if !n.is_empty()
                    && let Some(top) = local_scopes.last_mut()
                {
                    top.insert(n);
                }
            }
            "MatchMapping" => {
                let n = s(pat, "rest");
                if !n.is_empty()
                    && let Some(top) = local_scopes.last_mut()
                {
                    top.insert(n);
                }
            }
            "MatchValue" => {
                self.visit_expr(
                    owner_id,
                    child(pat, "value"),
                    env,
                    local_scopes,
                    param_map,
                    false,
                );
                return;
            }
            _ => {}
        }
        for_each_child(pat, |c| {
            self.visit_match_pattern(owner_id, c, env, local_scopes, param_map)
        });
    }

    fn process_function(
        &mut self,
        node: &'a Node,
        parent_id: u32,
        owning_class: Option<&'a str>,
        kind_override: DeadDefKind,
    ) {
        let name = s(node, "name");
        let (line, col) = node.position().unwrap_or((1, 0));
        let mut flags = 0u16;
        let mut is_property = false;

        for dec in items(node, "decorator_list") {
            let target = decorator_target(dec);
            if self.syms.only(target, "builtins.property")
                || self.syms.only(target, "functools.cached_property")
                || (kind(target) == "Name"
                    && matches!(s(target, "id"), "property" | "cached_property"))
            {
                is_property = true;
            }
            if self.syms.only(target, "builtins.staticmethod")
                || self.syms.only(target, "builtins.classmethod")
                || (kind(target) == "Name"
                    && matches!(s(target, "id"), "staticmethod" | "classmethod"))
            {
                flags |= def_flags::IS_STATIC_OR_CLASSMETHOD;
            }
            if decorator_matches_any(
                dec,
                self.syms,
                &["typing.override", "typing_extensions.override"],
                &["override"],
            ) {
                flags |= def_flags::IS_OVERRIDE;
            }
            if decorator_matches_any(
                dec,
                self.syms,
                &["typing.overload", "typing_extensions.overload"],
                &["overload"],
            ) {
                flags |= def_flags::IS_OVERLOAD;
            }
            if is_pytest_fixture(dec, self.syms) {
                flags |= def_flags::IS_PYTEST_FIXTURE;
            }
            if decorator_matches_any(
                dec,
                self.syms,
                PYDANTIC_HOOK_DECORATORS,
                &[
                    "field_validator",
                    "model_validator",
                    "computed_field",
                    "field_serializer",
                    "model_serializer",
                    "validator",
                    "root_validator",
                ],
            ) {
                flags |= def_flags::IS_PYDANTIC_HOOK;
            }
            if decorator_matches_any(
                dec,
                self.syms,
                ORM_HOOK_DECORATORS,
                &[
                    "validates",
                    "hybrid_property",
                    "hybrid_method",
                    "reconstructor",
                ],
            ) {
                flags |= def_flags::IS_ORM_HOOK;
            }
            if !decorator_is_transparent(dec, self.syms) {
                flags |= def_flags::HAS_REG_DECORATOR;
            }
        }
        if Self::is_stub_body(items(node, "body")) {
            flags |= def_flags::IS_STUB_BODY;
        }

        let def_kind = if is_property && owning_class.is_some() {
            DeadDefKind::Property
        } else {
            kind_override
        };

        let def_id = self.alloc_id();

        // Decorators execute at definition time (when enclosing module/class runs).
        let empty_env = HashMap::new();
        let empty_params = HashMap::new();
        for dec in items(node, "decorator_list") {
            self.visit_expr(parent_id, dec, &empty_env, &[], &empty_params, false);
        }

        let args = child(node, "args");
        for default in items(args, "defaults")
            .iter()
            .chain(items(args, "kw_defaults"))
        {
            self.visit_expr(parent_id, default, &empty_env, &[], &empty_params, false);
        }

        let mut env: HashMap<&'a str, LocalTypeInfo> = HashMap::new();
        let mut param_map: HashMap<&'a str, u32> = HashMap::new();
        let mut param_types: Vec<ParamTypeEntry> = Vec::new();
        let mut local_names: HashSet<&'a str> = HashSet::new();

        if let Some(cls_name) = owning_class {
            env.insert(
                "self",
                LocalTypeInfo {
                    direct: vec![format!("@{cls_name}").into()],
                    elem: vec![],
                },
            );
            env.insert(
                "cls",
                LocalTypeInfo {
                    direct: vec![format!("@{cls_name}").into()],
                    elem: vec![],
                },
            );
        }

        for param in parameters(node) {
            let pname = s(param, "arg");
            local_names.insert(pname);
            let ann = child(param, "annotation");
            self.record_annotation_refs(def_id, ann);
            let pty = self.resolve_annotation_types(ann, owning_class);
            if !matches!(pname, "self" | "cls") {
                param_types.push((pname.into(), pty.direct.clone().into_boxed_slice()));
                let (pline, pcol) = param.position().unwrap_or((line, col));
                let pid = self.alloc_id();
                self.defs.push(DeadDef {
                    local_id: pid,
                    parent_id: def_id,
                    name: pname.into(),
                    kind: DeadDefKind::Param,
                    line: pline as u32,
                    column: pcol as u32,
                    flags: 0,
                    reexport_target: None,
                    return_types: Box::new([]),
                    value_types: Box::new([]),
                    elem_types: Box::new([]),
                    bases: Box::new([]),
                    param_types: Box::new([]),
                });
                param_map.insert(pname, pid);
            }
            if !pty.direct.is_empty() || !pty.elem.is_empty() {
                env.insert(pname, pty);
            }
        }
        for variadic in [child(args, "vararg"), child(args, "kwarg")] {
            if !variadic.is_null() {
                local_names.insert(s(variadic, "arg"));
                self.record_annotation_refs(def_id, child(variadic, "annotation"));
            }
        }

        self.record_type_param_refs(def_id, node);
        let ret = child(node, "returns");
        self.record_annotation_refs(def_id, ret);
        let ret_ty = self.resolve_annotation_types(ret, owning_class);
        let in_boundary_return = self.file.has_layer(&["boundary", "router"]);
        if in_boundary_return {
            for t in ret_ty.direct.iter().chain(&ret_ty.elem) {
                self.escaped_types.insert(t.clone());
            }
        }

        self.defs.push(DeadDef {
            local_id: def_id,
            parent_id,
            name: name.into(),
            kind: def_kind,
            line: line as u32,
            column: col as u32,
            flags,
            reexport_target: None,
            return_types: ret_ty.direct.into_boxed_slice(),
            value_types: Box::new([]),
            elem_types: ret_ty.elem.into_boxed_slice(),
            bases: Box::new([]),
            param_types: param_types.into_boxed_slice(),
        });

        let mut local_scopes = vec![local_names];
        self.visit_stmts(
            def_id,
            items(node, "body"),
            &mut env,
            &mut local_scopes,
            &param_map,
            in_boundary_return,
        );
    }

    fn process_class(&mut self, node: &'a Node) {
        let name = s(node, "name");
        let (line, col) = node.position().unwrap_or((1, 0));
        let (flags, bases) = self.classify_class_flags(node);
        let cls_id = self.alloc_id();

        // Pre-scan class body & `__init__` for `self.<field>` types so methods can resolve `self._repo.save()`.
        let mut field_map: HashMap<String, LocalTypeInfo> = HashMap::new();
        for stmt in items(node, "body") {
            if kind(stmt) == "AnnAssign" && kind(child(stmt, "target")) == "Name" {
                let fname = s(child(stmt, "target"), "id");
                let fty = self.resolve_annotation_types(child(stmt, "annotation"), Some(name));
                if !fty.direct.is_empty() || !fty.elem.is_empty() {
                    field_map.insert(fname.to_owned(), fty);
                }
            } else if matches!(kind(stmt), "FunctionDef" | "AsyncFunctionDef")
                && s(stmt, "name") == "__init__"
            {
                let mut init_params: HashMap<&str, LocalTypeInfo> = HashMap::new();
                for p in parameters(stmt) {
                    let pname = s(p, "arg");
                    let pty = self.resolve_annotation_types(child(p, "annotation"), Some(name));
                    if !pty.direct.is_empty() || !pty.elem.is_empty() {
                        init_params.insert(pname, pty);
                    }
                }
                for bstmt in items(stmt, "body") {
                    if kind(bstmt) == "AnnAssign" {
                        let t = child(bstmt, "target");
                        if kind(t) == "Attribute"
                            && kind(child(t, "value")) == "Name"
                            && s(child(t, "value"), "id") == "self"
                        {
                            let fty = self
                                .resolve_annotation_types(child(bstmt, "annotation"), Some(name));
                            if !fty.direct.is_empty() || !fty.elem.is_empty() {
                                field_map.insert(s(t, "attr").to_owned(), fty);
                            }
                        }
                    } else if kind(bstmt) == "Assign" {
                        let val = child(bstmt, "value");
                        let val_ty = self.resolve_expr_types(val, &init_params);
                        if !val_ty.direct.is_empty() || !val_ty.elem.is_empty() {
                            for t in items(bstmt, "targets") {
                                if kind(t) == "Attribute"
                                    && kind(child(t, "value")) == "Name"
                                    && s(child(t, "value"), "id") == "self"
                                {
                                    field_map.insert(s(t, "attr").to_owned(), val_ty.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        if !field_map.is_empty() {
            for (attr, info) in &field_map {
                self.init_fields.push(InitFieldType {
                    class_local_id: cls_id,
                    attr: attr.as_str().into(),
                    direct_types: info.direct.clone().into_boxed_slice(),
                    elem_types: info.elem.clone().into_boxed_slice(),
                });
            }
            self.class_field_types.insert(name, field_map);
        }

        // Dataclasses defined in boundary layer are wire payloads -> fields escape.
        if (flags & (def_flags::IS_DATACLASS | def_flags::IS_NAMED_TUPLE)) != 0
            && self.file.has_layer(&["boundary", "router"])
        {
            self.escaped_types.insert(format!("@{name}").into());
        }

        self.defs.push(DeadDef {
            local_id: cls_id,
            parent_id: 0,
            name: name.into(),
            kind: DeadDefKind::Class,
            line: line as u32,
            column: col as u32,
            flags,
            reexport_target: None,
            return_types: Box::new([]),
            value_types: Box::new([]),
            elem_types: Box::new([]),
            bases,
            param_types: Box::new([]),
        });

        let empty_env = HashMap::new();
        let empty_params = HashMap::new();
        // Class decorators execute at module init; base classes belong to the class itself.
        for dec in items(node, "decorator_list") {
            self.visit_expr(0, dec, &empty_env, &[], &empty_params, false);
        }
        self.record_type_param_refs(cls_id, node);
        for base in items(node, "bases") {
            self.record_annotation_refs(cls_id, base);
        }
        for kw in items(node, "keywords") {
            self.visit_expr(
                cls_id,
                child(kw, "value"),
                &empty_env,
                &[],
                &empty_params,
                false,
            );
        }

        for stmt in items(node, "body") {
            match kind(stmt) {
                "FunctionDef" | "AsyncFunctionDef" => {
                    self.process_function(stmt, cls_id, Some(name), DeadDefKind::Method);
                }
                "AnnAssign" => {
                    let target = child(stmt, "target");
                    let ann = child(stmt, "annotation");
                    self.record_annotation_refs(cls_id, ann);
                    let val = child(stmt, "value");
                    if !val.is_null() {
                        self.visit_expr(cls_id, val, &empty_env, &[], &empty_params, false);
                    }
                    if kind(target) == "Name" {
                        let fname = s(target, "id");
                        let is_classvar = kind(ann) == "Subscript"
                            && self.syms.only(child(ann, "value"), "typing.ClassVar");
                        if !is_classvar {
                            let (fline, fcol) = stmt.position().unwrap_or((line, col));
                            let fty = self.resolve_annotation_types(ann, Some(name));
                            let fid = self.alloc_id();
                            self.defs.push(DeadDef {
                                local_id: fid,
                                parent_id: cls_id,
                                name: fname.into(),
                                kind: DeadDefKind::Field,
                                line: fline as u32,
                                column: fcol as u32,
                                flags: 0,
                                reexport_target: None,
                                return_types: Box::new([]),
                                value_types: fty.direct.into_boxed_slice(),
                                elem_types: fty.elem.into_boxed_slice(),
                                bases: Box::new([]),
                                param_types: Box::new([]),
                            });
                        }
                    }
                }
                "Assign" => {
                    let val = child(stmt, "value");
                    self.visit_expr(cls_id, val, &empty_env, &[], &empty_params, false);
                    // Only track class-body attributes for dataclasses/NamedTuples; Pydantic/ORM/Enum
                    // class attributes are schema/enum definitions handled via class flags.
                }
                "ClassDef" => {
                    // Nested class (e.g. `class Config:` inside Pydantic model): attribute refs to outer class.
                    for b in items(stmt, "bases")
                        .iter()
                        .chain(items(stmt, "decorator_list"))
                    {
                        self.visit_expr(cls_id, b, &empty_env, &[], &empty_params, false);
                    }
                    let mut env = HashMap::new();
                    let mut scopes = Vec::new();
                    self.visit_stmts(
                        cls_id,
                        items(stmt, "body"),
                        &mut env,
                        &mut scopes,
                        &empty_params,
                        false,
                    );
                }
                _ => {
                    let mut env = HashMap::new();
                    let mut scopes = Vec::new();
                    self.visit_stmts(
                        cls_id,
                        std::slice::from_ref(stmt),
                        &mut env,
                        &mut scopes,
                        &empty_params,
                        false,
                    );
                }
            }
        }
    }
}

fn collect_pattern_names<'a>(node: &'a Node, out: &mut HashSet<&'a str>) {
    if kind(node) == "Name" {
        out.insert(s(node, "id"));
        return;
    }
    for_each_child(node, |c| collect_pattern_names(c, out));
}

fn is_main_guard(node: &Node) -> bool {
    if kind(node) != "If" {
        return false;
    }
    let test = child(node, "test");
    if kind(test) != "Compare" {
        return false;
    }
    let ops = items(test, "ops");
    let comps = items(test, "comparators");
    if ops.len() != 1 || kind(&ops[0]) != "Eq" || comps.len() != 1 {
        return false;
    }
    let left = child(test, "left");
    let right = &comps[0];
    let is_name_dunder = |n: &Node| kind(n) == "Name" && s(n, "id") == "__name__";
    let is_main_str = |n: &Node| kind(n) == "Constant" && n["value"].as_str() == Some("__main__");
    (is_name_dunder(left) && is_main_str(right)) || (is_main_str(left) && is_name_dunder(right))
}

fn collect_module_scope_imports<'a>(
    stmts: &'a [Node],
    package: Option<&str>,
    out: &mut HashMap<&'a str, Box<str>>,
) {
    for stmt in stmts {
        match kind(stmt) {
            "Import" => {
                for alias in items(stmt, "names") {
                    let orig = s(alias, "name");
                    let asname = s(alias, "asname");
                    let (key, target) = if asname.is_empty() {
                        let root = orig.split('.').next().unwrap_or(orig);
                        (root, root)
                    } else {
                        (asname, orig)
                    };
                    out.entry(key).or_insert_with(|| target.into());
                }
            }
            "ImportFrom" => {
                if let Some(module) = symbols::absolute_module(stmt, package)
                    && !module.is_empty()
                {
                    for alias in items(stmt, "names") {
                        let orig = s(alias, "name");
                        if orig == "*" {
                            continue;
                        }
                        let asname = s(alias, "asname");
                        let key = if asname.is_empty() { orig } else { asname };
                        out.entry(key)
                            .or_insert_with(|| format!("{module}.{orig}").into());
                    }
                }
            }
            "With" | "AsyncWith" => {
                collect_module_scope_imports(items(stmt, "body"), package, out);
            }
            "If" => {
                collect_module_scope_imports(items(stmt, "body"), package, out);
                collect_module_scope_imports(items(stmt, "orelse"), package, out);
            }
            "Try" | "TryStar" => {
                collect_module_scope_imports(items(stmt, "body"), package, out);
                for h in items(stmt, "handlers") {
                    collect_module_scope_imports(items(h, "body"), package, out);
                }
                collect_module_scope_imports(items(stmt, "orelse"), package, out);
                collect_module_scope_imports(items(stmt, "finalbody"), package, out);
            }
            _ => {}
        }
    }
}

fn extract_summary(
    file: &FileContext<'_>,
    tree: &Node,
    syms: &symbols::Symbols,
    static_all: Option<Box<[Box<str>]>>,
) -> DeadCodeSummary {
    let body = items(tree, "body");
    let mut top_level_names = HashSet::new();
    let mut module_imports = HashMap::new();
    collect_module_scope_imports(body, file.package, &mut module_imports);
    let mut pytest_plugins: Vec<Box<str>> = Vec::new();
    for stmt in body {
        match kind(stmt) {
            "FunctionDef" | "AsyncFunctionDef" => {
                top_level_names.insert(s(stmt, "name"));
            }
            "ClassDef" => {
                top_level_names.insert(s(stmt, "name"));
            }
            "TypeAlias" => {
                let n = child(stmt, "name");
                if kind(n) == "Name" {
                    top_level_names.insert(s(n, "id"));
                }
            }
            "Assign" => {
                for t in items(stmt, "targets") {
                    if kind(t) == "Name" {
                        let name = s(t, "id");
                        top_level_names.insert(name);
                        if name == "pytest_plugins" {
                            let val = child(stmt, "value");
                            if kind(val) == "Constant"
                                && let Some(s) = val["value"].as_str()
                            {
                                pytest_plugins.push(s.into());
                            } else if let Some(seq) = is_static_string_sequence(val) {
                                pytest_plugins.extend(seq);
                            }
                        }
                    }
                }
            }
            "AnnAssign" => {
                let t = child(stmt, "target");
                if kind(t) == "Name" {
                    top_level_names.insert(s(t, "id"));
                }
            }
            _ => {}
        }
    }

    let mut ex = Extractor {
        file,
        syms,
        next_id: 0,
        defs: Vec::new(),
        refs: Vec::new(),
        attr_reads: Vec::new(),
        type_flows: Vec::new(),
        init_fields: Vec::new(),
        escaped_types: BTreeSet::new(),
        read_params: BTreeSet::new(),
        top_level_names,
        module_imports,
        class_field_types: HashMap::new(),
    };

    let is_marker = file.path.file_name().is_some_and(|n| n == "__init__.py");
    let mut has_main_guard = false;
    let mut module_env: HashMap<&str, LocalTypeInfo> = HashMap::new();
    let mut module_scopes: Vec<HashSet<&str>> = Vec::new();
    let empty_params = HashMap::new();
    let in_boundary = file.has_layer(&["boundary", "router"]);

    for stmt in body {
        match kind(stmt) {
            "FunctionDef" | "AsyncFunctionDef" => {
                ex.process_function(stmt, 0, None, DeadDefKind::Function);
            }
            "ClassDef" => {
                ex.process_class(stmt);
            }
            "TypeAlias" => {
                let name_node = child(stmt, "name");
                if kind(name_node) == "Name" {
                    let (line, col) = stmt.position().unwrap_or((1, 0));
                    let id = ex.alloc_id();
                    let val_ty = ex.resolve_annotation_types(child(stmt, "value"), None);
                    ex.defs.push(DeadDef {
                        local_id: id,
                        parent_id: 0,
                        name: s(name_node, "id").into(),
                        kind: DeadDefKind::TypeAlias,
                        line: line as u32,
                        column: col as u32,
                        flags: 0,
                        reexport_target: None,
                        return_types: Box::new([]),
                        value_types: val_ty.direct.into_boxed_slice(),
                        elem_types: val_ty.elem.into_boxed_slice(),
                        bases: Box::new([]),
                        param_types: Box::new([]),
                    });
                    ex.record_type_param_refs(id, stmt);
                    ex.record_annotation_refs(id, child(stmt, "value"));
                }
            }
            "Assign" => {
                let val = child(stmt, "value");
                let targets = items(stmt, "targets");
                if targets.len() == 1 && kind(&targets[0]) == "Name" {
                    let vname = s(&targets[0], "id");
                    if vname == "__all__" {
                        continue;
                    }
                    let (line, col) = stmt.position().unwrap_or((1, 0));
                    let vid = ex.alloc_id();
                    let vty = ex.resolve_expr_types(val, &module_env);
                    let ann_alias_ty = ex.resolve_annotation_types(val, None);
                    let mut direct = vty.direct.clone();
                    for t in ann_alias_ty.direct {
                        if !direct.contains(&t) {
                            direct.push(t);
                        }
                    }
                    let mut elem = vty.elem.clone();
                    for t in ann_alias_ty.elem {
                        if !elem.contains(&t) {
                            elem.push(t);
                        }
                    }
                    ex.defs.push(DeadDef {
                        local_id: vid,
                        parent_id: 0,
                        name: vname.into(),
                        kind: DeadDefKind::Variable,
                        line: line as u32,
                        column: col as u32,
                        flags: 0,
                        reexport_target: None,
                        return_types: Box::new([]),
                        value_types: direct.clone().into_boxed_slice(),
                        elem_types: elem.clone().into_boxed_slice(),
                        bases: Box::new([]),
                        param_types: Box::new([]),
                    });
                    ex.visit_expr(vid, val, &module_env, &[], &empty_params, false);
                    if !direct.is_empty() || !elem.is_empty() {
                        module_env.insert(vname, LocalTypeInfo { direct, elem });
                    }
                } else {
                    ex.visit_expr(0, val, &module_env, &[], &empty_params, false);
                }
            }
            "AnnAssign" => {
                let target = child(stmt, "target");
                let ann = child(stmt, "annotation");
                let val = child(stmt, "value");
                if kind(target) == "Name" {
                    let vname = s(target, "id");
                    if vname == "__all__" {
                        continue;
                    }
                    let (line, col) = stmt.position().unwrap_or((1, 0));
                    let vid = ex.alloc_id();
                    let ann_ty = ex.resolve_annotation_types(ann, None);
                    let val_ty = if !val.is_null() {
                        ex.resolve_expr_types(val, &module_env)
                    } else {
                        LocalTypeInfo::default()
                    };
                    let (direct, elem) = if !ann_ty.direct.is_empty() || !ann_ty.elem.is_empty() {
                        (ann_ty.direct.clone(), ann_ty.elem.clone())
                    } else {
                        (val_ty.direct.clone(), val_ty.elem.clone())
                    };
                    ex.defs.push(DeadDef {
                        local_id: vid,
                        parent_id: 0,
                        name: vname.into(),
                        kind: DeadDefKind::Variable,
                        line: line as u32,
                        column: col as u32,
                        flags: 0,
                        reexport_target: None,
                        return_types: Box::new([]),
                        value_types: direct.clone().into_boxed_slice(),
                        elem_types: elem.clone().into_boxed_slice(),
                        bases: Box::new([]),
                        param_types: Box::new([]),
                    });
                    ex.record_annotation_refs(vid, ann);
                    if !val.is_null() {
                        ex.visit_expr(vid, val, &module_env, &[], &empty_params, false);
                    }
                    for ft in &val_ty.direct {
                        for tt in &ann_ty.direct {
                            ex.type_flows.push(TypeFlow {
                                owner_id: vid,
                                from_type: ft.clone(),
                                to_type: tt.clone(),
                            });
                        }
                    }
                    if !direct.is_empty() || !elem.is_empty() {
                        module_env.insert(vname, LocalTypeInfo { direct, elem });
                    }
                }
            }
            "ImportFrom" => {
                if is_marker && let Some(module) = symbols::absolute_module(stmt, file.package) {
                    let (line, col) = stmt.position().unwrap_or((1, 0));
                    for alias in items(stmt, "names") {
                        let orig = s(alias, "name");
                        if orig == "*" {
                            continue;
                        }
                        let asname = s(alias, "asname");
                        let local = if asname.is_empty() { orig } else { asname };
                        let rid = ex.alloc_id();
                        ex.defs.push(DeadDef {
                            local_id: rid,
                            parent_id: 0,
                            name: local.into(),
                            kind: DeadDefKind::Reexport,
                            line: line as u32,
                            column: col as u32,
                            flags: 0,
                            reexport_target: Some(format!("{module}.{orig}").into()),
                            return_types: Box::new([]),
                            value_types: Box::new([]),
                            elem_types: Box::new([]),
                            bases: Box::new([]),
                            param_types: Box::new([]),
                        });
                    }
                }
            }
            "Import" => {}
            "If" => {
                if is_main_guard(stmt) {
                    has_main_guard = true;
                }
                ex.visit_stmts(
                    0,
                    std::slice::from_ref(stmt),
                    &mut module_env,
                    &mut module_scopes,
                    &empty_params,
                    in_boundary,
                );
            }
            _ => {
                ex.visit_stmts(
                    0,
                    std::slice::from_ref(stmt),
                    &mut module_env,
                    &mut module_scopes,
                    &empty_params,
                    in_boundary,
                );
            }
        }
    }

    let is_conftest = file.path.file_name().is_some_and(|n| n == "conftest.py");

    DeadCodeSummary {
        defs: ex.defs.into_boxed_slice(),
        refs: ex.refs.into_boxed_slice(),
        attr_reads: ex.attr_reads.into_boxed_slice(),
        type_flows: ex.type_flows.into_boxed_slice(),
        init_fields: ex.init_fields.into_boxed_slice(),
        escaped_types: ex.escaped_types.into_iter().collect(),
        read_params: ex.read_params.into_iter().collect(),
        static_all,
        pytest_plugins: pytest_plugins.into_boxed_slice(),
        has_main_guard,
        is_conftest,
    }
}
