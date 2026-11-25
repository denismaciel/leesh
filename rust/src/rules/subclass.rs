use super::*;
use crate::model::{BaseRef, ClassBaseCheck, TopDefKind};

const DECLARATIVE: &[&str] = &[
    "sqlalchemy.ext.declarative.declarative_base",
    "sqlalchemy.orm.DeclarativeBase",
    "sqlalchemy.orm.DeclarativeBaseNoMeta",
    "sqlalchemy.orm.declarative_base",
];

#[inline]
fn matches_allowed(name: &str, allowed: &BTreeSet<String>) -> bool {
    allowed.contains(name)
        || allowed
            .iter()
            .any(|p| p.ends_with(".*") && name.starts_with(&p[..p.len() - 1]))
}

#[inline]
fn all_in_allowed(resolved: &[Box<str>], allowed: &BTreeSet<String>) -> bool {
    !resolved.is_empty()
        && resolved
            .iter()
            .all(|n| &**n != "?" && matches_allowed(n, allowed))
}

#[inline]
fn all_in_static(resolved: &[Box<str>], allowed: &[&str]) -> bool {
    !resolved.is_empty() && resolved.iter().all(|n| allowed.contains(&&**n))
}

#[inline]
fn is_only_object(resolved: &[Box<str>]) -> bool {
    resolved.len() == 1 && &*resolved[0] == "builtins.object"
}

fn local_target_name(base: &Node) -> Option<Box<str>> {
    let target = if kind(base) == "Subscript" {
        child(base, "value")
    } else {
        base
    };
    (kind(target) == "Name").then(|| s(target, "id").into())
}

pub(super) fn extract_summary(
    file: &FileContext<'_>,
    policy: &Policy,
) -> (Box<[TopDef]>, Box<[ClassCheck]>) {
    let mut top_defs = Vec::new();
    for st in items(file.tree, "body") {
        match kind(st) {
            "ClassDef" => {
                let bases: Box<[BaseRef]> = items(st, "bases")
                    .iter()
                    .map(|b| {
                        let resolved = file.symbols.resolved_box(b);
                        let local_target = if resolved.is_empty() {
                            local_target_name(b)
                        } else {
                            None
                        };
                        BaseRef {
                            resolved,
                            local_target,
                        }
                    })
                    .collect();
                top_defs.push(TopDef {
                    name: s(st, "name").into(),
                    kind: TopDefKind::Class {
                        id: ast::id(st) as u32,
                        bases,
                    },
                });
            }
            "Assign" => {
                let value = child(st, "value");
                if !value.is_null() {
                    let mut cached: Option<Box<[Box<str>]>> = None;
                    let val_id = ast::id(value) as u32;
                    for t in items(st, "targets") {
                        if kind(t) == "Name" {
                            let resolved = cached
                                .get_or_insert_with(|| file.symbols.resolved_box(value))
                                .clone();
                            top_defs.push(TopDef {
                                name: s(t, "id").into(),
                                kind: TopDefKind::Value {
                                    id: val_id,
                                    resolved,
                                },
                            });
                        }
                    }
                }
            }
            "AnnAssign" => {
                let target = child(st, "target");
                let value = child(st, "value");
                if kind(target) == "Name" && !value.is_null() {
                    top_defs.push(TopDef {
                        name: s(target, "id").into(),
                        kind: TopDefKind::Value {
                            id: ast::id(value) as u32,
                            resolved: file.symbols.resolved_box(value),
                        },
                    });
                }
            }
            "ImportFrom" => {
                if let Some(imported) = symbols::absolute_module(st, file.package) {
                    for a in items(st, "names") {
                        let asname = s(a, "asname");
                        let orig = s(a, "name");
                        let local = if asname.is_empty() { orig } else { asname };
                        top_defs.push(TopDef {
                            name: local.into(),
                            kind: TopDefKind::Import {
                                target: format!("{imported}.{orig}").into(),
                            },
                        });
                    }
                }
            }
            _ => {}
        }
    }

    let mut classes = Vec::new();
    if !file.layer().is_empty() {
        let allowed = &policy.allowed_bases;
        for &n in file.nodes {
            if kind(n) != "ClassDef" {
                continue;
            }
            let bases = items(n, "bases");
            if bases.is_empty() {
                continue;
            }
            let mut has_declarative_base = false;
            let mut unchecked_bases = Vec::new();
            for base in bases {
                if !file.symbols.is_unknown(base)
                    && file.symbols.all(base, |v| DECLARATIVE.contains(&v))
                {
                    has_declarative_base = true;
                }
                if !file.symbols.is_unknown(base)
                    && file
                        .symbols
                        .all(base, |v| v != "?" && matches_allowed(v, allowed))
                {
                    continue;
                }
                let resolved = file.symbols.resolved_box(base);
                let local_target = if resolved.is_empty() {
                    local_target_name(base)
                } else {
                    None
                };
                let (line, col) = base.position().unwrap_or((1, 0));
                unchecked_bases.push(ClassBaseCheck {
                    line: line as u32,
                    column: col as u32,
                    resolved,
                    local_unparsed: defaults::unparse(base).into_boxed_str(),
                    local_target,
                });
            }
            if !unchecked_bases.is_empty() {
                classes.push(ClassCheck {
                    name: s(n, "name").into(),
                    id: ast::id(n) as u32,
                    has_declarative_base,
                    bases: unchecked_bases.into_boxed_slice(),
                });
            }
        }
    }
    (top_defs.into_boxed_slice(), classes.into_boxed_slice())
}

fn def_id(def: &TopDef) -> u32 {
    match &def.kind {
        TopDefKind::Class { id, .. } | TopDefKind::Value { id, .. } => *id,
        TopDefKind::Import { .. } => 0,
    }
}

fn definition<'a>(
    name: &str,
    w: &'a Workspace,
    seen: &mut HashSet<String>,
) -> Option<(&'a TopDef, usize)> {
    if !seen.insert(name.into()) {
        return None;
    }
    let (module, attribute) = name.rsplit_once('.')?;
    let source_idx = w.module_index(module)?;
    let source = &w.sources[source_idx];
    for def in &source.top_defs {
        if &*def.name != attribute {
            continue;
        }
        match &def.kind {
            TopDefKind::Class { .. } | TopDefKind::Value { .. } => {
                return Some((def, source_idx));
            }
            TopDefKind::Import { target } => {
                return definition(target, w, seen);
            }
        }
    }
    None
}

fn resolve_bases<'a>(
    resolved: &[Box<str>],
    local_target: Option<&str>,
    scope_idx: usize,
    w: &'a Workspace,
) -> Option<Vec<(&'a TopDef, usize)>> {
    if resolved.is_empty() {
        let target = local_target?;
        let scope = &w.sources[scope_idx];
        return scope
            .top_defs
            .iter()
            .find(|d| matches!(d.kind, TopDefKind::Class { .. }) && &*d.name == target)
            .map(|d| vec![(d, scope_idx)]);
    }
    let mut out = Vec::with_capacity(resolved.len());
    for n in resolved {
        if &**n == "?" {
            return None;
        }
        out.push(definition(n, w, &mut HashSet::new())?);
    }
    Some(out)
}

fn extendable(def: &TopDef, allowed: &BTreeSet<String>) -> bool {
    match &def.kind {
        TopDefKind::Class { bases, .. } => {
            bases.iter().any(|b| !is_only_object(&b.resolved))
                && bases.iter().all(|b| all_in_allowed(&b.resolved, allowed))
        }
        TopDefKind::Value { resolved, .. } => all_in_allowed(resolved, allowed),
        TopDefKind::Import { .. } => false,
    }
}

fn declarative(def: &TopDef) -> bool {
    match &def.kind {
        TopDefKind::Class { bases, .. } => bases
            .iter()
            .any(|b| all_in_static(&b.resolved, DECLARATIVE)),
        TopDefKind::Value { resolved, .. } => all_in_static(resolved, DECLARATIVE),
        TopDefKind::Import { .. } => false,
    }
}

fn plain(def: &TopDef) -> bool {
    match &def.kind {
        TopDefKind::Class { bases, .. } => bases.iter().all(|b| is_only_object(&b.resolved)),
        _ => false,
    }
}

fn exception(
    def: &TopDef,
    scope_idx: usize,
    w: &Workspace,
    seen: &mut HashSet<(usize, u32)>,
) -> bool {
    let TopDefKind::Class { id, bases } = &def.kind else {
        return false;
    };
    if !seen.insert((scope_idx, *id)) {
        return false;
    }
    for b in bases {
        if !b.resolved.is_empty()
            && b.resolved
                .iter()
                .all(|n| BUILTIN_EXCEPTIONS.contains(&&**n))
        {
            return true;
        }
        if let Some(ds) = resolve_bases(&b.resolved, b.local_target.as_deref(), scope_idx, w)
            && ds
                .into_iter()
                .any(|(d, s_idx)| exception(d, s_idx, w, &mut seen.clone()))
        {
            return true;
        }
    }
    false
}

pub(super) fn check(file_idx: usize, file: &Source, w: &Workspace, out: &mut Vec<Finding>) {
    if file.classes.is_empty() {
        return;
    }
    let allowed = &w.policy.allowed_bases;
    for cls in &file.classes {
        let mut rejected: Vec<(&ClassBaseCheck, String)> = Vec::new();
        let mut mixins: Vec<(&ClassBaseCheck, String)> = Vec::new();
        let mut is_declarative = cls.has_declarative_base;
        for base in &cls.bases {
            let unknown = base.resolved.is_empty();
            let mut described = if unknown {
                format!("'{}', a workspace class", base.local_unparsed)
            } else {
                base.resolved.join(", ")
            };
            let ds = resolve_bases(&base.resolved, base.local_target.as_deref(), file_idx, w);
            if ds.is_none()
                || ds.as_ref().is_some_and(|ds| {
                    ds.iter()
                        .any(|(d, s_idx)| *s_idx == file_idx && def_id(d) == cls.id)
                })
            {
                if unknown {
                    described = format!("'{}', which leesh cannot resolve", base.local_unparsed);
                }
                rejected.push((base, described));
                continue;
            }
            let ds = ds.unwrap();
            if ds.iter().all(|(d, _)| declarative(d)) {
                is_declarative = true;
            }
            if ds.iter().all(|(d, s_idx)| {
                extendable(d, allowed) || exception(d, *s_idx, w, &mut HashSet::new())
            }) {
                continue;
            }
            if ds.iter().all(|(d, _)| plain(d)) {
                mixins.push((base, described));
            } else {
                rejected.push((base, described));
            }
        }
        if !is_declarative {
            rejected.extend(mixins);
        }
        for (base, description) in rejected {
            let target = if !base.resolved.is_empty() {
                base.resolved.join(", ")
            } else if let Some(module) = &file.module {
                format!("{module}.{}", base.local_unparsed)
            } else {
                base.local_unparsed.to_string()
            };
            out.push(finding_at(
                &file.filename,
                base.line as usize,
                base.column as usize,
                "LIT403",
                format!(
                    "'{}' extends {description}, which is not an allowed base; compose, or implement a Protocol (extend conventions.allowed_bases for framework bases)",
                    cls.name
                ),
                Some(target),
            ));
        }
    }
}

const BUILTIN_EXCEPTIONS: &[&str] = &[
    "builtins.ArithmeticError",
    "builtins.AssertionError",
    "builtins.AttributeError",
    "builtins.BaseException",
    "builtins.BaseExceptionGroup",
    "builtins.BlockingIOError",
    "builtins.BrokenPipeError",
    "builtins.BufferError",
    "builtins.BytesWarning",
    "builtins.ChildProcessError",
    "builtins.ConnectionAbortedError",
    "builtins.ConnectionError",
    "builtins.ConnectionRefusedError",
    "builtins.ConnectionResetError",
    "builtins.DeprecationWarning",
    "builtins.EOFError",
    "builtins.EncodingWarning",
    "builtins.EnvironmentError",
    "builtins.Exception",
    "builtins.ExceptionGroup",
    "builtins.FileExistsError",
    "builtins.FileNotFoundError",
    "builtins.FloatingPointError",
    "builtins.FutureWarning",
    "builtins.GeneratorExit",
    "builtins.IOError",
    "builtins.ImportError",
    "builtins.ImportWarning",
    "builtins.IndentationError",
    "builtins.IndexError",
    "builtins.InterruptedError",
    "builtins.IsADirectoryError",
    "builtins.KeyError",
    "builtins.KeyboardInterrupt",
    "builtins.LookupError",
    "builtins.MemoryError",
    "builtins.ModuleNotFoundError",
    "builtins.NameError",
    "builtins.NotADirectoryError",
    "builtins.NotImplementedError",
    "builtins.OSError",
    "builtins.OverflowError",
    "builtins.PendingDeprecationWarning",
    "builtins.PermissionError",
    "builtins.ProcessLookupError",
    "builtins.PythonFinalizationError",
    "builtins.RecursionError",
    "builtins.ReferenceError",
    "builtins.ResourceWarning",
    "builtins.RuntimeError",
    "builtins.RuntimeWarning",
    "builtins.StopAsyncIteration",
    "builtins.StopIteration",
    "builtins.SyntaxError",
    "builtins.SyntaxWarning",
    "builtins.SystemError",
    "builtins.SystemExit",
    "builtins.TabError",
    "builtins.TimeoutError",
    "builtins.TypeError",
    "builtins.UnboundLocalError",
    "builtins.UnicodeDecodeError",
    "builtins.UnicodeEncodeError",
    "builtins.UnicodeError",
    "builtins.UnicodeTranslateError",
    "builtins.UnicodeWarning",
    "builtins.UserWarning",
    "builtins.ValueError",
    "builtins.Warning",
    "builtins.ZeroDivisionError",
    "builtins._IncompleteInputError",
];
