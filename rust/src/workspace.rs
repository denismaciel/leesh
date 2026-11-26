use crate::{
    ast::{self, ParseFailure},
    model::{Classification, FileContext, ImportStmt, InitImport, Source, TopImport},
    rules, symbols,
};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
};

#[derive(Debug, Clone)]
struct CompiledGlob {
    prefix: Box<str>,
    suffix: Box<str>,
    regex: Arc<regex::Regex>,
}
impl CompiledGlob {
    fn new(pattern: &str) -> Self {
        let prefix_end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
        let suffix_start = pattern.rfind(['*', '?', ']']).map_or(0, |i| i + 1);
        Self {
            prefix: pattern[..prefix_end].into(),
            suffix: pattern[suffix_start..].into(),
            regex: compile_glob(pattern),
        }
    }
    #[inline]
    fn is_match(&self, path: &str) -> bool {
        if !self.prefix.is_empty() && !path.starts_with(&*self.prefix) {
            return false;
        }
        if !self.suffix.is_empty() && !path.ends_with(&*self.suffix) {
            return false;
        }
        self.regex.is_match(path)
    }
}

#[derive(Debug, Clone)]
pub struct CustomLayer {
    pub name: String,
    pub base: String,
    pub deterministic: bool,
}
#[derive(Debug, Clone)]
pub struct AllowedDependency {
    pub modules: Vec<String>,
    pub layers: BTreeSet<String>,
    pub reason: String,
    pub review_by: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Policy {
    pub layer_dependencies: BTreeMap<String, BTreeSet<String>>,
    pub forbidden_imports: BTreeMap<String, BTreeSet<String>>,
    pub forbidden_calls: BTreeMap<String, BTreeSet<String>>,
    pub custom_layers: BTreeMap<String, CustomLayer>,
    pub allowed_dependencies: Vec<AllowedDependency>,
    pub predicate_prefixes: Vec<String>,
    pub outcome_suffixes: Vec<String>,
    pub allowed_bases: BTreeSet<String>,
    pub effectful_types: BTreeSet<String>,
    pub clock_modules: BTreeSet<String>,
    pub domain_dependencies: BTreeMap<String, BTreeSet<String>>,
}
impl Policy {
    pub fn base<'a>(&'a self, layer: &'a str) -> &'a str {
        self.custom_layers
            .get(layer)
            .map_or(layer, |c| c.base.as_str())
    }
    pub fn is_a(&self, layer: &str, targets: &BTreeSet<String>) -> bool {
        targets.contains(layer) || targets.contains(self.base(layer))
    }
    pub fn allows_dependency(&self, importer: &str, module: &str) -> bool {
        self.allowed_dependencies.iter().any(|a| {
            (a.layers.is_empty() || self.is_a(importer, &a.layers))
                && matches_name(module, &a.modules)
        })
    }
}
#[derive(Debug, Clone)]
pub struct Package {
    pub root: PathBuf,
    pub kind: String,
    pub layout: String,
    pub layers: Vec<(String, String)>,
    pub overrides: Vec<Value>,
    pub dependency_overrides: Vec<Value>,
    compiled_layers: Vec<CompiledGlob>,
    compiled_overrides: Vec<CompiledGlob>,
    dep_override_pairs: HashSet<(String, String)>,
}
#[derive(Debug, Clone)]
pub struct Ignore {
    pub rules: Vec<String>,
    pub paths: Vec<String>,
    pub layers: BTreeSet<String>,
    pub reason: String,
    pub review_by: Option<String>,
    pub max_count: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct DeadCodeKeep {
    pub symbols: Vec<String>,
    pub reason: String,
    pub review_by: Option<String>,
}
pub struct Workspace {
    init_closures: Box<[OnceLock<Box<[usize]>>]>,
    scc_cache: OnceLock<Vec<Option<u32>>>,
    server_roots_cache: OnceLock<HashSet<String>>,
    module_index: HashMap<String, usize>,
    defined_packages: HashSet<String>,
    packages_by_depth: Vec<usize>,
    has_dependency_overrides: bool,
    pub root: PathBuf,
    pub manifest: Option<PathBuf>,
    pub packages: Vec<Package>,
    pub sources: Vec<Source>,
    pub parse_failures: Vec<(PathBuf, ParseFailure)>,
    pub policy: Policy,
    pub ignores: Vec<Ignore>,
    pub baseline_path: Option<PathBuf>,
    pub baseline: BTreeMap<(String, String, String), usize>,
    pub override_budget: usize,
    pub dependency_override_budget: usize,
    pub strict: bool,
    pub dead_code: bool,
    pub dead_code_entrypoints: Vec<String>,
    pub dead_code_keep: Vec<DeadCodeKeep>,
    pub pyproject_entrypoints: Vec<String>,
    pub pytest_plugins: Vec<String>,
    pub today: String,
}
pub fn matches_name<I, S>(name: &str, patterns: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    patterns.into_iter().any(|p| {
        let p = p.as_ref();
        name == p || (p.ends_with(".*") && name.starts_with(&p[..p.len() - 1]))
    })
}
fn compile_glob(pattern: &str) -> Arc<regex::Regex> {
    static CACHE: OnceLock<RwLock<HashMap<String, Arc<regex::Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(re) = cache.read().unwrap().get(pattern) {
        return Arc::clone(re);
    }
    let parts: Vec<_> = pattern.split('/').collect();
    let mut r = String::from("\\A");
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if *p == "**" {
            r.push_str(if last {
                "(?:[^/]+/)*[^/]+"
            } else {
                "(?:[^/]+/)*"
            });
            continue;
        }
        let chars: Vec<_> = p.chars().collect();
        let mut j = 0;
        while j < chars.len() {
            match chars[j] {
                '*' => r.push_str("[^/]*"),
                '?' => r.push_str("[^/]"),
                '[' => {
                    if let Some(end) = (j + 2..chars.len()).find(|&k| chars[k] == ']') {
                        r.push('[');
                        let mut start = j + 1;
                        if chars[start] == '!' {
                            r.push('^');
                            start += 1;
                        }
                        for c in &chars[start..end] {
                            if *c == '-' {
                                r.push('-')
                            } else {
                                r.push_str(&regex::escape(&c.to_string()))
                            }
                        }
                        r.push(']');
                        j = end;
                    } else {
                        r.push_str("\\[")
                    }
                }
                c => r.push_str(&regex::escape(&c.to_string())),
            }
            j += 1;
        }
        if !last {
            r.push('/')
        }
    }
    r.push_str("\\z");
    let re = Arc::new(regex::Regex::new(&r).unwrap_or_else(|_| regex::Regex::new("a^").unwrap()));
    let mut guard = cache.write().unwrap();
    Arc::clone(guard.entry(pattern.into()).or_insert(re))
}
pub fn matches_glob(path: &str, pattern: &str) -> bool {
    compile_glob(pattern).is_match(path)
}
const LAYERS: &[&str] = &[
    "core",
    "port",
    "shell",
    "adapter",
    "router",
    "boundary",
    "transaction",
    "test",
    "marker",
];
fn st(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn string(v: &Value, f: &str) -> Result<String, String> {
    v.as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| format!("{f} must be a non-empty string"))
}
fn strings(v: &Value, f: &str) -> Result<Vec<String>, String> {
    v.as_array()
        .filter(|a| a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty())))
        .map(|a| a.iter().map(|v| st(v).into()).collect())
        .ok_or_else(|| format!("{f} must be an array of non-empty strings"))
}
fn list(v: &Value, f: &str) -> Result<Vec<String>, String> {
    if v.is_null() {
        Ok(vec![])
    } else {
        strings(v, f)
    }
}
fn table<'a>(v: &'a Value, f: &str) -> Result<Vec<(&'a String, &'a Value)>, String> {
    if v.is_null() {
        Ok(vec![])
    } else {
        v.as_object()
            .map(|o| o.iter().collect())
            .ok_or_else(|| format!("{f} must be a table"))
    }
}
fn tables<'a>(v: &'a Value, f: &str) -> Result<Vec<&'a Value>, String> {
    if v.is_null() {
        Ok(vec![])
    } else {
        v.as_array()
            .filter(|a| a.iter().all(Value::is_object))
            .map(|a| a.iter().collect())
            .ok_or_else(|| format!("{f} must be an array of tables"))
    }
}
fn keys(v: &Value, allowed: &[&str], field: &str) -> Result<(), String> {
    let mut bad: Vec<_> = table(v, field)?
        .into_iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !allowed.contains(k))
        .collect();
    bad.sort();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("unknown {field} field: {}", bad.join(", ")))
    }
}
fn choice(v: &Value, f: &str, choices: &[String]) -> Result<String, String> {
    let s = string(v, f)?;
    if choices.contains(&s) {
        Ok(s)
    } else {
        Err(format!(
            "{f} must be one of: {}; got '{s}'",
            choices.join(", ")
        ))
    }
}
fn rel(v: &Value, f: &str) -> Result<String, String> {
    let s = v
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{f} must be a non-empty relative path"))?;
    if s.starts_with('/') || s.split('/').any(|s| s == "..") {
        return Err(format!("{f} must be a relative path inside the workspace"));
    }
    let parts: Vec<_> = s
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    Ok(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
}
fn pyglob(v: &Value, f: &str) -> Result<String, String> {
    let s = rel(v, f)?;
    if !s.ends_with(".py") {
        Err(format!("{f} must select Python files: {s}"))
    } else {
        Ok(s)
    }
}
fn qualified(v: &Value, f: &str, wild: bool) -> Result<BTreeSet<String>, String> {
    let names = list(v, f)?;
    let bad: Vec<_> = names
        .iter()
        .filter(|n| {
            !n.strip_suffix(if wild { ".*" } else { "\0" })
                .unwrap_or(n)
                .split('.')
                .all(identifier)
        })
        .cloned()
        .collect();
    if !bad.is_empty() {
        return Err(format!(
            "{f} must contain qualified Python names{}: {}",
            if wild { " or module.*" } else { "" },
            bad.join(", ")
        ));
    }
    Ok(names.into_iter().collect())
}
fn identifier(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && c.all(|c| c == '_' || c.is_alphanumeric())
}
fn integer(v: &Value, f: &str) -> Result<usize, String> {
    if v.is_null() {
        Ok(0)
    } else {
        v.as_u64()
            .map(|v| v as usize)
            .ok_or_else(|| format!("{f} must be a non-negative integer"))
    }
}
fn opt_integer(v: &Value, f: &str) -> Result<Option<usize>, String> {
    if v.is_null() {
        Ok(None)
    } else {
        v.as_u64()
            .map(|v| Some(v as usize))
            .ok_or_else(|| format!("{f} must be a non-negative integer"))
    }
}
fn iso_date(v: &Value, f: &str) -> Result<Option<String>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let err = || format!("{f} must be an ISO date (YYYY-MM-DD)");
    let s = v.as_str().ok_or_else(err)?;
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return Err(err());
    }
    let year: u32 = s[0..4].parse().map_err(|_| err())?;
    let month: u32 = s[5..7].parse().map_err(|_| err())?;
    let day: u32 = s[8..10].parse().map_err(|_| err())?;
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || day == 0 {
        return Err(err());
    }
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return Err(err()),
    };
    if day > max_day {
        return Err(err());
    }
    Ok(Some(s.to_owned()))
}
fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}")
}
fn default<'a>(v: &'a Value, key: &str, d: &'a Value) -> &'a Value {
    v.get(key).unwrap_or(d)
}
fn policy(raw: &Value, w: &Value, d: &Value) -> Result<Policy, String> {
    let mut custom = BTreeMap::from([(
        "integration_test".into(),
        CustomLayer {
            name: "integration_test".into(),
            base: "test".into(),
            deterministic: false,
        },
    )]);
    for (n, v) in table(&raw["layers"], "layers")? {
        let f = format!("layers.{n}");
        if LAYERS.contains(&n.as_str())
            || custom.contains_key(n.as_str())
            || n.replace(['_', '-'], "").is_empty()
            || !n
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "{f} must be a new layer name of letters, digits, - and _"
            ));
        }
        keys(v, &["base", "deterministic"], &f)?;
        let base = choice(
            &v["base"],
            &format!("{f}.base"),
            &LAYERS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )?;
        if base == "marker" {
            return Err(format!("{f}.base cannot be marker"));
        }
        let deterministic = v.get("deterministic").map_or(Ok(false), |v| {
            v.as_bool()
                .ok_or_else(|| format!("{f}.deterministic must be true or false"))
        })?;
        custom.insert(
            n.clone(),
            CustomLayer {
                name: n.clone(),
                base,
                deterministic,
            },
        );
    }
    let names: Vec<String> = LAYERS
        .iter()
        .map(|s| s.to_string())
        .chain(custom.keys().cloned())
        .collect();
    let mut maps = Vec::new();
    for key in ["layer_dependencies", "forbidden_imports", "forbidden_calls"] {
        let mut map: BTreeMap<String, BTreeSet<String>> = if d[key].is_null() {
            BTreeMap::new()
        } else {
            serde_json::from_value(d[key].clone()).unwrap()
        };
        let mut explicit = BTreeSet::new();
        for (n, v) in table(&raw[key], key)? {
            choice(&json!(n), &format!("{key} key"), &names)?;
            explicit.insert(n.clone());
            if key == "layer_dependencies" && v == "*" {
                map.remove(n);
                continue;
            }
            let values = if key == "layer_dependencies" {
                strings(v, &format!("{key}.{n}"))?
                    .iter()
                    .map(|s| choice(&json!(s), &format!("{key}.{n}"), &names))
                    .collect::<Result<BTreeSet<_>, _>>()?
            } else {
                qualified(v, &format!("{key}.{n}"), key == "forbidden_calls")?
            };
            map.insert(n.clone(), values);
        }
        let mut ext = BTreeMap::new();
        if key == "forbidden_imports" {
            for (n, v) in table(&raw["extend_forbidden_imports"], "extend_forbidden_imports")? {
                if n != "*" {
                    choice(&json!(n), "extend_forbidden_imports key", &names)?;
                }
                ext.insert(
                    n.clone(),
                    qualified(v, &format!("extend_forbidden_imports.{n}"), false)?,
                );
            }
            for n in names
                .iter()
                .filter(|n| !custom.contains_key(*n) || explicit.contains(*n))
            {
                let extra: BTreeSet<_> = ext
                    .get("*")
                    .into_iter()
                    .flatten()
                    .chain(ext.get(n).into_iter().flatten())
                    .cloned()
                    .collect();
                if !extra.is_empty() {
                    map.entry(n.clone()).or_default().extend(extra);
                }
            }
        }
        for (n, c) in &custom {
            if !explicit.contains(n) {
                if let Some(v) = map.get(&c.base).cloned() {
                    map.insert(n.clone(), v);
                }
                if let Some(v) = ext.get(n) {
                    map.entry(n.clone()).or_default().extend(v.clone());
                }
            }
        }
        maps.push(map);
    }
    let mut allowed = vec![];
    for v in tables(&raw["allowed_dependencies"], "allowed_dependencies")? {
        keys(
            v,
            &["modules", "layers", "reason", "review_by"],
            "allowed_dependencies",
        )?;
        let modules = qualified(&v["modules"], "allowed_dependencies.modules", true)?;
        if modules.is_empty() {
            return Err("allowed_dependencies.modules must name at least one module".into());
        }
        let layers = list(&v["layers"], "allowed_dependencies.layers")?
            .iter()
            .map(|s| choice(&json!(s), "allowed_dependencies.layers", &names))
            .collect::<Result<_, _>>()?;
        allowed.push(AllowedDependency {
            modules: modules.into_iter().collect(),
            layers,
            reason: string(&v["reason"], "allowed_dependencies.reason")?,
            review_by: iso_date(&v["review_by"], "allowed_dependencies.review_by")?,
        });
    }
    let c = &raw["conventions"];
    keys(
        c,
        &[
            "predicate_prefixes",
            "outcome_suffixes",
            "allowed_bases",
            "extend_allowed_bases",
        ],
        "conventions",
    )?;
    let mut bases = qualified(
        default(c, "allowed_bases", &d["allowed_bases"]),
        "conventions.allowed_bases",
        true,
    )?;
    bases.extend(qualified(
        &c["extend_allowed_bases"],
        "conventions.extend_allowed_bases",
        true,
    )?);
    let mut domains = BTreeMap::new();
    for (n, v) in table(&raw["domains"], "domains")? {
        if !identifier(n) {
            return Err(format!(
                "domains key must be a Python identifier; got '{n}'"
            ));
        }
        let deps: BTreeSet<_> = strings(v, &format!("domains.{n}"))?.into_iter().collect();
        if deps.contains(n) {
            return Err(format!("domains.{n} cannot depend on itself"));
        }
        domains.insert(n.clone(), deps);
    }
    for (n, deps) in &domains {
        let bad: Vec<_> = deps
            .iter()
            .filter(|s| !domains.contains_key(*s))
            .cloned()
            .collect();
        if !bad.is_empty() {
            return Err(format!(
                "domains.{n} names undeclared domains: {}; give every domain its own entry",
                bad.join(", ")
            ));
        }
    }
    fn visit(
        n: &str,
        g: &BTreeMap<String, BTreeSet<String>>,
        path: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if let Some(i) = path.iter().position(|s| s == n) {
            let mut p = path[i..].to_vec();
            p.push(n.into());
            return Err(format!("domains form a cycle: {}", p.join(" -> ")));
        }
        if done.contains(n) {
            return Ok(());
        }
        path.push(n.into());
        for t in &g[n] {
            visit(t, g, path, done)?
        }
        path.pop();
        done.insert(n.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for n in domains.keys() {
        visit(n, &domains, &mut vec![], &mut done)?
    }
    Ok(Policy {
        layer_dependencies: maps.remove(0),
        forbidden_imports: maps.remove(0),
        forbidden_calls: maps.remove(0),
        custom_layers: custom,
        allowed_dependencies: allowed,
        predicate_prefixes: strings(
            default(c, "predicate_prefixes", &d["predicate_prefixes"]),
            "conventions.predicate_prefixes",
        )?,
        outcome_suffixes: strings(
            default(c, "outcome_suffixes", &d["outcome_suffixes"]),
            "conventions.outcome_suffixes",
        )?,
        allowed_bases: bases,
        effectful_types: qualified(&w["effectful_types"], "workspace.effectful_types", false)?,
        clock_modules: qualified(&w["clock_modules"], "workspace.clock_modules", false)?,
        domain_dependencies: domains,
    })
}
const DEFAULTS: &str = r#"{"layer_dependencies": {"core": ["core"], "port": ["core", "port"], "shell": ["core", "port", "shell"], "adapter": ["adapter", "core", "port"], "router": ["boundary", "router", "shell", "transaction"]}, "forbidden_imports": {"core": ["aiohttp", "aiosqlite", "asyncpg", "boto3", "botocore", "fastapi", "httpx", "playwright", "psycopg", "psycopg2", "pydantic", "pydantic_settings", "redis", "requests", "socket", "sqlalchemy", "sqlite3", "starlette", "subprocess"], "port": ["aiohttp", "aiosqlite", "asyncpg", "boto3", "botocore", "fastapi", "httpx", "playwright", "psycopg", "psycopg2", "pydantic", "pydantic_settings", "redis", "requests", "socket", "sqlalchemy", "sqlite3", "starlette", "subprocess"], "shell": ["fastapi", "pydantic", "pydantic_settings", "sqlalchemy", "starlette"], "router": ["sqlalchemy"]}, "predicate_prefixes": ["allows_", "can_", "contains_", "has_", "is_", "matches_", "should_"], "outcome_suffixes": ["AlreadyExists", "Conflict", "Denied", "Expired", "Forbidden", "NotFound", "Rejected", "Unauthorized", "Unavailable"], "allowed_bases": ["builtins.*", "enum.Enum", "enum.Flag", "enum.IntEnum", "enum.IntFlag", "enum.StrEnum", "pydantic.BaseModel", "pydantic.RootModel", "pydantic_settings.BaseSettings", "sqlalchemy.TypeDecorator", "sqlalchemy.ext.declarative.declarative_base", "sqlalchemy.orm.DeclarativeBase", "sqlalchemy.orm.DeclarativeBaseNoMeta", "sqlalchemy.orm.MappedAsDataclass", "sqlalchemy.orm.declarative_base", "sqlalchemy.types.TypeDecorator", "typing.Generic", "typing.NamedTuple", "typing.Protocol", "typing.TypedDict", "typing_extensions.Generic", "typing_extensions.NamedTuple", "typing_extensions.Protocol", "typing_extensions.TypedDict"], "layouts": {"domains": [["src/*/domains/**/boundary.py", "boundary"], ["src/*/domains/**/model.py", "core"], ["src/*/domains/**/core.py", "core"], ["src/*/domains/**/policy.py", "core"], ["src/*/domains/**/event.py", "core"], ["src/*/domains/**/ports.py", "port"], ["src/*/domains/**/service.py", "shell"], ["src/*/domains/*/**/boundary/**/*.py", "boundary"], ["src/*/domains/*/**/model/**/*.py", "core"], ["src/*/domains/*/**/core/**/*.py", "core"], ["src/*/domains/*/**/policy/**/*.py", "core"], ["src/*/domains/*/**/event/**/*.py", "core"], ["src/*/domains/*/**/ports/**/*.py", "port"], ["src/*/domains/*/**/service/**/*.py", "shell"], ["src/*/domains/**/persistence/**/*.py", "adapter"], ["src/*/projections/**/model.py", "core"], ["src/*/projections/**/ports.py", "port"], ["src/*/projections/**/service.py", "shell"], ["src/*/projections/*/**/model/**/*.py", "core"], ["src/*/projections/*/**/ports/**/*.py", "port"], ["src/*/projections/*/**/service/**/*.py", "shell"], ["src/*/projections/**/persistence/**/*.py", "adapter"], ["src/*/shared/model.py", "core"], ["src/*/shared/core.py", "core"], ["src/*/shared/policy.py", "core"], ["src/*/shared/ports.py", "port"], ["src/*/shared/model/**/*.py", "core"], ["src/*/shared/core/**/*.py", "core"], ["src/*/shared/policy/**/*.py", "core"], ["src/*/shared/ports/**/*.py", "port"], ["src/*/shared/persistence/**/*.py", "adapter"], ["src/*/__main__.py", "boundary"], ["src/*/app/**/*.py", "boundary"], ["src/*/app/**/router.py", "router"], ["src/*/app/**/*_router.py", "router"], ["src/*/app/router/**/*.py", "router"], ["src/*/app/routers/**/*.py", "router"], ["src/*/app/dependencies.py", "transaction"], ["src/*/db/**/*.py", "adapter"], ["src/*/db/unit_of_work.py", "transaction"], ["src/*/db/unit_of_work/**/*.py", "transaction"], ["src/*/integrations/**/*.py", "adapter"], ["src/*/integrations/unit_of_work/**/*.py", "transaction"], ["src/*/**/fakes.py", "test"], ["src/*/**/fake.py", "test"], ["src/*/**/fakes/**/*.py", "test"]], "hexagonal": [["src/*/*.py", "core"], ["src/*/domain/**/*.py", "core"], ["src/*/domain/**/ports.py", "port"], ["src/*/application/**/*.py", "shell"], ["src/*/application/**/ports.py", "port"], ["src/*/application/**/errors.py", "core"], ["src/*/application/**/*_errors.py", "core"], ["src/*/adapters/**/*.py", "adapter"], ["src/*/adapters/**/database.py", "transaction"], ["src/*/web/**/*.py", "boundary"], ["src/*/app/**/*.py", "boundary"], ["src/*/config.py", "boundary"], ["src/*/development.py", "boundary"], ["src/*/main.py", "boundary"], ["src/*/__main__.py", "boundary"], ["src/*/worker.py", "boundary"]], "flat": [["src/**/*.py", "shell"], ["src/*/model.py", "core"], ["src/*/core.py", "core"], ["src/*/policy.py", "core"], ["src/*/ports.py", "port"], ["src/*/model/**/*.py", "core"], ["src/*/core/**/*.py", "core"], ["src/*/policy/**/*.py", "core"], ["src/*/ports/**/*.py", "port"], ["src/*/adapters/**/*.py", "adapter"], ["src/*/cli.py", "boundary"], ["src/*/cli/**/*.py", "boundary"], ["src/*/__main__.py", "boundary"], ["src/*/development.py", "boundary"]], "boundary": [["src/**/*.py", "boundary"]], "test-support": [["src/**/*.py", "test"]]}}"#;
const IGNORED: &[&str] = &[
    ".direnv",
    ".git",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "target",
];
/// Every virtual environment has a `pyvenv.cfg`, whatever the directory is called
/// (`.venv314`, `env`, `venv-prod`). Its site-packages is never workspace code.
fn is_virtualenv(e: &walkdir::DirEntry) -> bool {
    e.file_type().is_dir() && e.path().join("pyvenv.cfg").is_file()
}
fn extract_top_imports(tree: &ast::Node, package: Option<&str>) -> Box<[TopImport]> {
    let mut result = Vec::new();
    for node in ast::items(tree, "body") {
        match ast::kind(node) {
            "Import" => {
                for alias in ast::items(node, "names") {
                    let orig = ast::s(alias, "name");
                    let asname = ast::s(alias, "asname");
                    let local = if asname.is_empty() {
                        orig.split('.').next().unwrap_or("")
                    } else {
                        asname
                    };
                    result.push(TopImport {
                        is_from: false,
                        module: orig.into(),
                        member: None,
                        local: local.into(),
                    });
                }
            }
            "ImportFrom" => {
                if let Some(module) = symbols::absolute_module(node, package) {
                    for alias in ast::items(node, "names") {
                        let orig = ast::s(alias, "name");
                        let asname = ast::s(alias, "asname");
                        let local = if asname.is_empty() { orig } else { asname };
                        result.push(TopImport {
                            is_from: true,
                            module: module.as_str().into(),
                            member: Some(orig.into()),
                            local: local.into(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    result.into_boxed_slice()
}
fn extract_init_imports(tree: &ast::Node, package: Option<&str>) -> Box<[InitImport]> {
    let mut pending: Vec<&ast::Node> = ast::items(tree, "body").iter().collect();
    let mut result = Vec::new();
    while let Some(node) = pending.pop() {
        match ast::kind(node) {
            "Import" => {
                for alias in ast::items(node, "names") {
                    result.push((ast::s(alias, "name").into(), None));
                }
            }
            "ImportFrom" => {
                if let Some(module) = symbols::absolute_module(node, package) {
                    for alias in ast::items(node, "names") {
                        result.push((module.as_str().into(), Some(ast::s(alias, "name").into())));
                    }
                }
            }
            "ClassDef" | "With" | "AsyncWith" => {
                pending.extend(ast::items(node, "body"));
            }
            "If" | "For" | "AsyncFor" | "While" => {
                pending.extend(ast::items(node, "body"));
                pending.extend(ast::items(node, "orelse"));
            }
            "Try" | "TryStar" => {
                pending.extend(ast::items(node, "body"));
                for h in ast::items(node, "handlers") {
                    pending.extend(ast::items(h, "body"));
                }
                pending.extend(ast::items(node, "orelse"));
                pending.extend(ast::items(node, "finalbody"));
            }
            "Match" => {
                for c in ast::items(node, "cases") {
                    pending.extend(ast::items(c, "body"));
                }
            }
            _ => {}
        }
    }
    result.into_boxed_slice()
}
fn extract_all_imports(nodes: &[&ast::Node], package: Option<&str>) -> Box<[ImportStmt]> {
    let mut result = Vec::new();
    for &node in nodes {
        let k = ast::kind(node);
        if k == "Import" {
            let (line, col) = node.position().unwrap_or((1, 0));
            let names: Box<[Box<str>]> = ast::items(node, "names")
                .iter()
                .map(|a| ast::s(a, "name").into())
                .collect();
            result.push(ImportStmt {
                line: line as u32,
                column: col as u32,
                is_from: false,
                raw_module: None,
                resolved_module: None,
                names,
            });
        } else if k == "ImportFrom" {
            let (line, col) = node.position().unwrap_or((1, 0));
            let raw_module = (!node["module"].is_null()).then(|| ast::s(node, "module").into());
            let resolved_module = symbols::absolute_module(node, package).map(Into::into);
            let names: Box<[Box<str>]> = ast::items(node, "names")
                .iter()
                .map(|a| ast::s(a, "name").into())
                .collect();
            result.push(ImportStmt {
                line: line as u32,
                column: col as u32,
                is_from: true,
                raw_module,
                resolved_module,
                names,
            });
        }
    }
    result.into_boxed_slice()
}
fn toml_error(error: toml::de::Error, text: &str) -> String {
    if error.message().contains("unclosed table") && error.message().contains("expected `]`") {
        let offset = error
            .span()
            .map_or(text.len(), |span| span.start)
            .min(text.len());
        let before = &text[..offset];
        let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("Expected ']' at the end of a table declaration (at line {line}, column {column})")
    } else {
        error.to_string()
    }
}
pub fn find_root(start: &Path) -> PathBuf {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    for marker in ["leesh.toml", "pyproject.toml"] {
        for p in start.ancestors() {
            if p.join(marker).is_file() {
                return p.to_path_buf();
            }
        }
    }
    start
}
fn relative(path: &Path, root: &Path) -> String {
    let s = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    if s.is_empty() {
        ".".into()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}
impl Workspace {
    #[inline]
    pub fn owner(&self, path: &Path) -> Option<usize> {
        self.packages_by_depth
            .iter()
            .copied()
            .find(|&i| path.starts_with(&self.packages[i].root))
    }
    fn classify_in(&self, path: &Path, owner: usize) -> Option<Classification> {
        let p = &self.packages[owner];
        let name = relative(path, &p.root);
        let mut matched_override: Option<usize> = None;
        for (idx, re) in p.compiled_overrides.iter().enumerate() {
            if re.is_match(&name) {
                if matched_override.is_some() {
                    return None;
                }
                matched_override = Some(idx);
            }
        }
        let (glob, layer, origin) = if let Some(idx) = matched_override {
            let o = &p.overrides[idx];
            (st(&o["glob"]), st(&o["layer"]), "override")
        } else {
            let (idx, _) = p
                .compiled_layers
                .iter()
                .enumerate()
                .rev()
                .find(|(_, re)| re.is_match(&name))?;
            let (g, l) = &p.layers[idx];
            (g.as_str(), l.as_str(), "layout")
        };
        Some(Classification {
            package: owner,
            layer: self.policy.base(layer).into(),
            name: layer.into(),
            origin: origin.into(),
            glob: glob.into(),
        })
    }
    pub fn classify(&self, path: &Path) -> Option<Classification> {
        let owned;
        let path = if path.is_absolute() {
            path
        } else {
            owned = self.root.join(path);
            &owned
        };
        let i = self.owner(path)?;
        self.classify_in(path, i)
    }
    pub(crate) fn init_closure(
        &self,
        source_idx: usize,
        compute: impl FnOnce() -> Box<[usize]>,
    ) -> &[usize] {
        self.init_closures[source_idx].get_or_init(compute)
    }
    pub(crate) fn cyclic_components(
        &self,
        compute: impl FnOnce() -> Vec<Option<u32>>,
    ) -> &[Option<u32>] {
        self.scc_cache.get_or_init(compute)
    }
    pub(crate) fn server_roots(&self) -> &HashSet<String> {
        self.server_roots_cache.get_or_init(|| {
            self.sources
                .iter()
                .enumerate()
                .filter(|(idx, f)| {
                    self.packages[f.owner].kind == "server"
                        && f.module
                            .as_deref()
                            .and_then(|m| self.module_index(m))
                            .is_some_and(|winner| winner == *idx)
                })
                .filter_map(|(_, f)| f.module.as_deref().and_then(|m| m.split('.').next()))
                .map(str::to_owned)
                .collect()
        })
    }
    #[inline]
    pub fn module_index(&self, module: &str) -> Option<usize> {
        self.module_index.get(module).copied()
    }
    #[inline]
    pub fn module_source(&self, module: &str) -> Option<&Source> {
        self.module_index.get(module).map(|&i| &self.sources[i])
    }
    #[inline]
    pub fn defines_package(&self, root: &str) -> bool {
        self.defined_packages.contains(root)
    }
    #[inline]
    pub fn dependency_overridden(&self, importer: &Source, imported: &Source) -> bool {
        if !self.has_dependency_overrides {
            return false;
        }
        let p = &self.packages[importer.owner];
        if p.dep_override_pairs.is_empty() || !imported.path.starts_with(&p.root) {
            return false;
        }
        p.dep_override_pairs.contains(&(
            relative(&importer.path, &p.root),
            relative(&imported.path, &p.root),
        ))
    }
    pub fn configuration_errors(&self) -> Vec<String> {
        let mut errors = vec![];
        let mut overridden = BTreeSet::new();
        // Bucket all files by owning package in a single O(F) pass.
        let mut by_pkg: Vec<BTreeMap<String, &PathBuf>> =
            (0..self.packages.len()).map(|_| BTreeMap::new()).collect();
        for s in &self.sources {
            let p = &self.packages[s.owner];
            by_pkg[s.owner].insert(relative(&s.path, &p.root), &s.path);
        }
        for (path, _) in &self.parse_failures {
            if let Some(owner) = self.owner(path) {
                let p = &self.packages[owner];
                by_pkg[owner].insert(relative(path, &p.root), path);
            }
        }
        for (i, p) in self.packages.iter().enumerate() {
            let location = relative(&p.root, &self.root);
            let rel_map = &by_pkg[i];
            let mut pairs = BTreeSet::new();
            for o in &p.dependency_overrides {
                let a = st(&o["importer"]);
                let b = st(&o["imported"]);
                if !pairs.insert((a, b)) {
                    errors.push(format!(
                        "duplicate dependency override in {location}: {a} -> {b}"
                    ))
                }
                for (role, t) in [("importer", a), ("imported", b)] {
                    if !rel_map.contains_key(t) {
                        errors.push(format!(
                            "stale dependency override {role} in {location}: {t}"
                        ))
                    }
                }
                if let Some(review_by) = o.get("review_by").and_then(Value::as_str)
                    && review_by < self.today.as_str()
                {
                    errors.push(format!(
                        "dependency override review_by {review_by} has expired in {location}: {a} -> {b}"
                    ));
                }
            }
            if !p.overrides.is_empty() {
                for (o, re) in p.overrides.iter().zip(&p.compiled_overrides) {
                    let g = st(&o["glob"]);
                    let matches: Vec<_> = rel_map
                        .iter()
                        .filter(|(n, _)| re.is_match(n))
                        .map(|(_, path)| *path)
                        .collect();
                    if matches.is_empty() {
                        errors.push(format!("stale layout override in {location}: {g}"))
                    }
                    if let Some(review_by) = o.get("review_by").and_then(Value::as_str)
                        && review_by < self.today.as_str()
                    {
                        errors.push(format!(
                            "layout override review_by {review_by} has expired in {location}: {g}"
                        ));
                    }
                    overridden.extend(matches);
                }
                if p.overrides.len() > 1 {
                    for name in rel_map.keys() {
                        let globs: Vec<_> = p
                            .overrides
                            .iter()
                            .zip(&p.compiled_overrides)
                            .filter(|(_, re)| re.is_match(name))
                            .map(|(o, _)| st(&o["glob"]))
                            .collect();
                        if globs.len() > 1 {
                            errors.push(format!(
                                "overlapping layout overrides for {name}: {}",
                                globs.join(", ")
                            ))
                        }
                    }
                }
            }
        }
        if overridden.len() > self.override_budget {
            errors.push(format!(
                "layout override budget exceeded: {} files use overrides, budget is {}",
                overridden.len(),
                self.override_budget
            ))
        }
        if !self.policy.allowed_dependencies.is_empty()
            || !self.policy.domain_dependencies.is_empty()
        {
            let modules: Vec<_> = self
                .sources
                .iter()
                .filter_map(|s| s.module.as_deref())
                .collect();
            for a in &self.policy.allowed_dependencies {
                for p in &a.modules {
                    if !modules.iter().any(|m| matches_name(m, [p])) {
                        errors.push(format!(
                            "allowed dependency matches no workspace module: {p}"
                        ))
                    }
                }
                if let Some(review_by) = &a.review_by
                    && review_by.as_str() < self.today.as_str()
                {
                    errors.push(format!(
                        "allowed dependency review_by {review_by} has expired: {}",
                        a.modules.join(", ")
                    ));
                }
            }
            for n in self.policy.domain_dependencies.keys() {
                if !modules.iter().any(|m| {
                    let mut p = m.split('.');
                    p.next().is_some()
                        && p.next() == Some("domains")
                        && p.next() == Some(n.as_str())
                }) {
                    errors.push(format!("declared domain matches no domains/ package: {n}"))
                }
            }
        }
        let count: usize = self
            .packages
            .iter()
            .map(|p| p.dependency_overrides.len())
            .sum();
        if count > self.dependency_override_budget {
            errors.push(format!(
                "dependency override budget exceeded: {count} edges use overrides, budget is {}",
                self.dependency_override_budget
            ))
        }
        for k in &self.dead_code_keep {
            if let Some(review_by) = &k.review_by
                && review_by.as_str() < self.today.as_str()
            {
                errors.push(format!(
                    "dead_code.keep review_by {review_by} has expired: {}",
                    k.symbols.join(", ")
                ));
            }
        }
        errors.sort();
        errors
    }
}

fn extract_pyproject_entrypoints(pkg_root: &Path) -> Vec<String> {
    let path = pkg_root.join("pyproject.toml");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = toml::from_str::<toml::Value>(&text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(bs) = doc.get("build-system").and_then(toml::Value::as_table)
        && bs.get("backend-path").is_some()
        && let Some(backend) = bs.get("build-backend").and_then(toml::Value::as_str)
        && !backend.trim().is_empty()
    {
        let b = backend.trim();
        out.push(b.to_owned());
        if !b.contains(':') {
            for hook in [
                "build_wheel",
                "build_editable",
                "build_sdist",
                "get_requires_for_build_wheel",
                "get_requires_for_build_editable",
                "get_requires_for_build_sdist",
                "prepare_metadata_for_build_wheel",
                "prepare_metadata_for_build_editable",
            ] {
                out.push(format!("{b}:{hook}"));
            }
        }
    }
    let Some(project) = doc.get("project").and_then(toml::Value::as_table) else {
        return out;
    };
    for section in ["scripts", "gui-scripts"] {
        if let Some(tbl) = project.get(section).and_then(toml::Value::as_table) {
            for val in tbl.values() {
                if let Some(s) = val.as_str()
                    && !s.trim().is_empty()
                {
                    out.push(s.trim().to_owned());
                }
            }
        }
    }
    if let Some(eps) = project.get("entry-points").and_then(toml::Value::as_table) {
        for group in eps.values() {
            if let Some(tbl) = group.as_table() {
                for val in tbl.values() {
                    if let Some(s) = val.as_str()
                        && !s.trim().is_empty()
                    {
                        out.push(s.trim().to_owned());
                    }
                }
            }
        }
    }
    out
}

fn extract_pytest_plugins(pkg_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut parse_addopts = |raw: &str| {
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i] == "-p"
                && let Some(next) = tokens.get(i + 1)
                && !next.starts_with("no:")
            {
                out.push((*next).to_owned());
                i += 2;
                continue;
            }
            if let Some(rest) = tokens[i].strip_prefix("-p")
                && !rest.is_empty()
                && !rest.starts_with("no:")
            {
                out.push(rest.to_owned());
            }
            i += 1;
        }
    };
    if let Ok(text) = std::fs::read_to_string(pkg_root.join("pytest.ini")) {
        for line in text.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("addopts")
                && let Some((_, val)) = rest.split_once('=')
            {
                parse_addopts(val);
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(pkg_root.join("pyproject.toml"))
        && let Ok(doc) = toml::from_str::<toml::Value>(&text)
        && let Some(ini) = doc
            .get("tool")
            .and_then(|t| t.get("pytest"))
            .and_then(|p| p.get("ini_options"))
        && let Some(addopts) = ini.get("addopts")
    {
        if let Some(s) = addopts.as_str() {
            parse_addopts(s);
        } else if let Some(arr) = addopts.as_array() {
            let joined = arr
                .iter()
                .filter_map(toml::Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            parse_addopts(&joined);
        }
    }
    out
}

fn validate_entrypoint_spec(spec: &str, field: &str) -> Result<String, String> {
    let s = spec.trim();
    if s.is_empty() {
        return Err(format!("{field} must be a non-empty entrypoint spec"));
    }
    let (module_part, attr_part) = match s.split_once(':') {
        Some((m, a)) => (m, Some(a)),
        None => (s, None),
    };
    let mod_check = module_part.strip_suffix(".*").unwrap_or(module_part);
    if mod_check.is_empty() || !mod_check.split('.').all(identifier) {
        return Err(format!(
            "{field} has invalid module spec '{s}'; expected module, module.*, or module:symbol"
        ));
    }
    if let Some(a) = attr_part
        && (a.is_empty() || !a.split('.').all(identifier))
    {
        return Err(format!(
            "{field} has invalid symbol spec '{s}'; expected module:symbol"
        ));
    }
    Ok(s.to_owned())
}

fn load_impl(
    root: &Path,
    parse_files: bool,
    dead_code_override: Option<bool>,
    extra_entrypoints: &[String],
) -> Result<Workspace, String> {
    let _span = crate::profile::span("workspace_load");
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let manifest = root.join("leesh.toml");
    let raw: Value = if manifest.is_file() {
        let text = std::fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
        let v: toml::Value = toml::from_str(&text).map_err(|e| toml_error(e, &text))?;
        serde_json::to_value(v).map_err(|e| e.to_string())?
    } else {
        json!({})
    };
    keys(
        &raw,
        &[
            "workspace",
            "packages",
            "layers",
            "layouts",
            "layer_dependencies",
            "forbidden_imports",
            "extend_forbidden_imports",
            "forbidden_calls",
            "allowed_dependencies",
            "ignore",
            "conventions",
            "overrides",
            "dependency_overrides",
            "domains",
            "dead_code",
        ],
        "manifest",
    )?;
    let w = &raw["workspace"];
    keys(
        w,
        &[
            "kind",
            "layout",
            "effectful_types",
            "clock_modules",
            "override_budget",
            "dependency_override_budget",
            "baseline",
            "strict",
            "dead_code",
        ],
        "workspace",
    )?;
    let strict = w.get("strict").map_or(Ok(false), |v| {
        v.as_bool()
            .ok_or_else(|| "workspace.strict must be true or false".to_owned())
    })?;
    let manifest_dead_code = w.get("dead_code").map_or(Ok(false), |v| {
        v.as_bool()
            .ok_or_else(|| "workspace.dead_code must be true or false".to_owned())
    })?;
    let dead_code = dead_code_override.unwrap_or(manifest_dead_code);
    let dc = &raw["dead_code"];
    keys(dc, &["entrypoints", "keep"], "dead_code")?;
    let mut dead_code_entrypoints = Vec::new();
    for ep in list(&dc["entrypoints"], "dead_code.entrypoints")? {
        dead_code_entrypoints.push(validate_entrypoint_spec(&ep, "dead_code.entrypoints")?);
    }
    for ep in extra_entrypoints {
        dead_code_entrypoints.push(validate_entrypoint_spec(ep, "--entrypoint")?);
    }
    let mut dead_code_keep = Vec::new();
    for entry in tables(&dc["keep"], "dead_code.keep")? {
        keys(entry, &["symbols", "reason", "review_by"], "dead_code.keep")?;
        let raw_syms = strings(&entry["symbols"], "dead_code.keep.symbols")?;
        if raw_syms.is_empty() {
            return Err("dead_code.keep.symbols must name at least one symbol".into());
        }
        let mut symbols = Vec::with_capacity(raw_syms.len());
        for sym in raw_syms {
            symbols.push(validate_entrypoint_spec(&sym, "dead_code.keep.symbols")?);
        }
        dead_code_keep.push(DeadCodeKeep {
            symbols,
            reason: string(&entry["reason"], "dead_code.keep.reason")?,
            review_by: iso_date(&entry["review_by"], "dead_code.keep.review_by")?,
        });
    }
    let mut projects = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for entry in walkdir::WalkDir::new(&root).into_iter().filter_entry(|e| {
        e.depth() == 0
            || !(IGNORED.contains(&e.file_name().to_string_lossy().as_ref()) || is_virtualenv(e))
    }) {
        let e = entry.map_err(|e| e.to_string())?;
        if e.file_type().is_file() {
            if e.file_name() == "pyproject.toml" {
                projects.insert(e.path().parent().unwrap().to_path_buf());
            }
            if e.path().extension().is_some_and(|s| s == "py") {
                paths.insert(e.path().to_path_buf());
            }
        }
    }
    if projects.is_empty() {
        return Err(format!("no pyproject.toml found under {}", root.display()));
    }
    let d: Value = serde_json::from_str(DEFAULTS).unwrap();
    let policy = policy(&raw, w, &d)?;
    let names: Vec<String> = LAYERS
        .iter()
        .map(|s| s.to_string())
        .chain(policy.custom_layers.keys().cloned())
        .collect();
    let mut layouts: BTreeMap<String, Vec<(String, String)>> =
        serde_json::from_value(d["layouts"].clone()).unwrap();
    let builtin = vec![
        "domains".into(),
        "hexagonal".into(),
        "flat".into(),
        "boundary".into(),
        "test-support".into(),
    ];
    let conventions = vec![
        ("conftest.py".into(), "integration_test".into()),
        ("tests/**/*.py".into(), "test".into()),
        ("tests/**/conftest.py".into(), "integration_test".into()),
        (
            "tests/**/integration/**/*.py".into(),
            "integration_test".into(),
        ),
        (
            "tests/**/integrations/**/*.py".into(),
            "integration_test".into(),
        ),
        (
            "tests/**/persistence/**/*.py".into(),
            "integration_test".into(),
        ),
        ("tests/**/e2e/**/*.py".into(), "integration_test".into()),
        (
            "tests/**/contract/**/*.py".into(),
            "integration_test".into(),
        ),
        ("tests/**/smoke/**/*.py".into(), "integration_test".into()),
        (
            "tests/**/adapters/**/*.py".into(),
            "integration_test".into(),
        ),
        (
            "tests/**/fixtures/**/*.py".into(),
            "integration_test".into(),
        ),
        ("tests/**/helpers/**/*.py".into(), "integration_test".into()),
        ("tests/**/fakes/**/*.py".into(), "integration_test".into()),
        ("tests/**/testing/**/*.py".into(), "integration_test".into()),
        (
            "tests/**/*_integration_test.py".into(),
            "integration_test".into(),
        ),
        ("tests/**/*_e2e_test.py".into(), "integration_test".into()),
        (
            "tests/**/test_*_integration.py".into(),
            "integration_test".into(),
        ),
        ("tests/**/test_*_e2e.py".into(), "integration_test".into()),
        ("scripts/**/*.py".into(), "boundary".into()),
        ("examples/**/*.py".into(), "boundary".into()),
        ("migrations/**/*.py".into(), "transaction".into()),
    ];
    for pats in layouts.values_mut() {
        pats.extend(conventions.iter().cloned());
    }
    let mut layout_names = builtin.clone();
    for (n, v) in table(&raw["layouts"], "layouts")? {
        if layouts.contains_key(n) || !v.is_object() {
            return Err(format!(
                "layout {n} must be a table and cannot redefine a built-in"
            ));
        }
        keys(v, &["extends", "layers"], &format!("layouts.{n}"))?;
        let mut pats = if let Some(x) = v.get("extends") {
            let p = choice(x, &format!("layouts.{n}.extends"), &builtin)?;
            layouts[&p].clone()
        } else {
            conventions.clone()
        };
        let entries = v["layers"]
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| format!("layouts.{n}.layers must be a non-empty array"))?;
        for e in entries {
            let f = format!("layouts.{n}.layers");
            if !e.is_object() {
                return Err(format!("{f} entries must be tables"));
            }
            keys(e, &["glob", "layer"], &f)?;
            pats.push((
                pyglob(&e["glob"], &format!("{f}.glob"))?,
                choice(&e["layer"], &format!("{f}.layer"), &names)?,
            ));
        }
        layouts.insert(n.clone(), pats);
        layout_names.push(n.clone());
    }
    for pats in layouts.values_mut() {
        pats.push(("src/**/__init__.py".into(), "marker".into()));
    }
    let kinds = vec!["server".into(), "client".into(), "library".into()];
    let dk = choice(
        default(w, "kind", &json!("server")),
        "workspace.kind",
        &kinds,
    )?;
    let dl = choice(
        default(w, "layout", &json!("domains")),
        "workspace.layout",
        &layout_names,
    )?;
    let mut configured = BTreeMap::new();
    for e in tables(&raw["packages"], "packages")? {
        keys(e, &["path", "kind", "layout"], "packages")?;
        let p = root.join(rel(&e["path"], "packages.path")?);
        if configured.contains_key(&p) {
            return Err(format!("duplicate package entry: {}", relative(&p, &root)));
        }
        if !projects.contains(&p) {
            return Err(format!(
                "package has no pyproject.toml: {}",
                relative(&p, &root)
            ));
        }
        configured.insert(
            p,
            (
                choice(default(e, "kind", &json!(dk)), "packages.kind", &kinds)?,
                choice(
                    default(e, "layout", &json!(dl)),
                    "packages.layout",
                    &layout_names,
                )?,
            ),
        );
    }
    let mut overrides: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
    let mut deps: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
    for (key, map) in [
        ("overrides", &mut overrides),
        ("dependency_overrides", &mut deps),
    ] {
        for e in tables(&raw[key], key)? {
            keys(
                e,
                if key == "overrides" {
                    &["package", "glob", "layer", "reason", "issue", "review_by"]
                } else {
                    &[
                        "package",
                        "importer",
                        "imported",
                        "reason",
                        "issue",
                        "review_by",
                    ]
                },
                key,
            )?;
            let p = root.join(rel(
                default(e, "package", &json!(".")),
                &format!("{key}.package"),
            )?);
            if !projects.contains(&p) {
                return Err(format!(
                    "{key}.package has no pyproject.toml: {}",
                    relative(&p, &root)
                ));
            }
            let mut e = e.clone();
            if key == "overrides" {
                let g = pyglob(&e["glob"], "override.glob")?;
                if let Some(i) = g.find(['*', '?', '['])
                    && g[..i]
                        .trim_end_matches('/')
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .count()
                        < 3
                {
                    return Err(format!(
                        "override.glob is too broad; scope it below a package area: {g}"
                    ));
                }
                e["glob"] = json!(g);
                e["layer"] = json!(choice(&e["layer"], "overrides.layer", &names)?);
            } else {
                for f in ["importer", "imported"] {
                    e[f] = json!(pyglob(&e[f], &format!("{key}.{f}"))?);
                }
            }
            for f in ["reason", "issue"] {
                e[f] = json!(string(&e[f], &format!("{key}.{f}"))?);
            }
            if let Some(review_by) = iso_date(&e["review_by"], &format!("{key}.review_by"))? {
                e["review_by"] = json!(review_by);
            }
            map.entry(p).or_default().push(e);
        }
    }
    let packages: Vec<Package> = projects
        .into_iter()
        .map(|p| {
            let (k, l) = configured.remove(&p).unwrap_or((dk.clone(), dl.clone()));
            let pkg_layers = layouts[&l].clone();
            let pkg_overrides = overrides.remove(&p).unwrap_or_default();
            let pkg_deps = deps.remove(&p).unwrap_or_default();
            let compiled_layers = pkg_layers
                .iter()
                .map(|(g, _)| CompiledGlob::new(g))
                .collect();
            let compiled_overrides = pkg_overrides
                .iter()
                .map(|o| CompiledGlob::new(st(&o["glob"])))
                .collect();
            let dep_override_pairs = pkg_deps
                .iter()
                .map(|o| (st(&o["importer"]).to_owned(), st(&o["imported"]).to_owned()))
                .collect();
            Package {
                layers: pkg_layers,
                kind: k,
                layout: l,
                overrides: pkg_overrides,
                dependency_overrides: pkg_deps,
                compiled_layers,
                compiled_overrides,
                dep_override_pairs,
                root: p,
            }
        })
        .collect();
    let has_dependency_overrides = packages.iter().any(|p| !p.dep_override_pairs.is_empty());
    let mut packages_by_depth: Vec<usize> = (0..packages.len()).collect();
    packages_by_depth.sort_by_key(|&i| std::cmp::Reverse(packages[i].root.components().count()));

    let mut ignores = vec![];
    for e in tables(&raw["ignore"], "ignore")? {
        keys(
            e,
            &[
                "rules",
                "paths",
                "layers",
                "reason",
                "review_by",
                "max_count",
            ],
            "ignore",
        )?;
        let rules = strings(&e["rules"], "ignore.rules")?;
        if rules.is_empty() {
            return Err("ignore.rules must name at least one rule".into());
        }
        ignores.push(Ignore {
            rules,
            paths: list(&e["paths"], "ignore.paths")?
                .iter()
                .map(|s| rel(&json!(s), "ignore.paths"))
                .collect::<Result<_, _>>()?,
            layers: list(&e["layers"], "ignore.layers")?
                .iter()
                .map(|s| choice(&json!(s), "ignore.layers", &names))
                .collect::<Result<_, _>>()?,
            reason: string(&e["reason"], "ignore.reason")?,
            review_by: iso_date(&e["review_by"], "ignore.review_by")?,
            max_count: opt_integer(&e["max_count"], "ignore.max_count")?,
        });
    }
    let baseline_path = w
        .get("baseline")
        .map(|v| rel(v, "workspace.baseline").map(|s| root.join(s)))
        .transpose()?;
    let mut baseline = BTreeMap::new();
    if let Some(p) = baseline_path.as_ref().filter(|p| p.is_file()) {
        let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        let b: Value = serde_json::from_str(&text).map_err(|e| baseline_json_error(&text, e))?;
        if !b.is_object() || b["version"] != 1 {
            return Err("baseline must be an object with version 1".into());
        }
        let files = b["findings"]
            .as_object()
            .ok_or("baseline findings must be an object keyed by file")?;
        for (f, entries) in files {
            for e in entries
                .as_array()
                .ok_or_else(|| format!("baseline entries for {f} must be an array"))?
            {
                let count = e
                    .get("count")
                    .map_or(Some(1), Value::as_u64)
                    .filter(|c| *c > 0);
                if !e["code"].is_string() || !e["message"].is_string() || count.is_none() {
                    return Err(format!(
                        "baseline entry for {f} needs code, message and count"
                    ));
                }
                *baseline
                    .entry((f.clone(), st(&e["code"]).into(), st(&e["message"]).into()))
                    .or_insert(0) += count.unwrap() as usize;
            }
        }
    }
    let pyproject_entrypoints: Vec<String> = if dead_code {
        packages
            .iter()
            .flat_map(|p| extract_pyproject_entrypoints(&p.root))
            .collect()
    } else {
        Vec::new()
    };
    let pytest_plugins: Vec<String> = if dead_code {
        packages
            .iter()
            .flat_map(|p| extract_pytest_plugins(&p.root))
            .collect()
    } else {
        Vec::new()
    };
    let mut ws = Workspace {
        module_index: HashMap::new(),
        defined_packages: HashSet::new(),
        init_closures: Box::new([]),
        scc_cache: OnceLock::new(),
        server_roots_cache: OnceLock::new(),
        packages_by_depth,
        has_dependency_overrides,
        root,
        manifest: manifest.is_file().then_some(manifest),
        packages,
        sources: vec![],
        parse_failures: vec![],
        policy,
        ignores,
        baseline_path,
        baseline,
        override_budget: integer(&w["override_budget"], "workspace.override_budget")?,
        dependency_override_budget: integer(
            &w["dependency_override_budget"],
            "workspace.dependency_override_budget",
        )?,
        strict,
        dead_code,
        dead_code_entrypoints,
        dead_code_keep,
        pyproject_entrypoints,
        pytest_plugins,
        today: today_utc(),
    };
    let owned_paths: Vec<(PathBuf, usize)> = paths
        .into_iter()
        .filter_map(|p| ws.owner(&p).map(|owner| (p, owner)))
        .collect();

    if !parse_files {
        ws.sources = owned_paths
            .into_iter()
            .map(|(path, owner)| Source {
                filename: relative(&path, &ws.root),
                classification: ws.classify_in(&path, owner),
                is_marker_file: path.file_name().is_some_and(|s| s == "__init__.py"),
                path,
                owner,
                module: None,
                package: None,
                top_imports: Box::new([]),
                init_imports: Box::new([]),
                all_imports: Box::new([]),
                top_defs: Box::new([]),
                classes: Box::new([]),
                deferred_local: Default::default(),
                local_findings: Vec::new(),
                dead_code: None,
            })
            .collect();
        return Ok(ws);
    }

    // Stream each file: parse -> resolve symbols -> run local rules & extract compact summary ->
    // immediately drop source text, AST, and Symbols so peak RSS stays O(threads).
    let parsed: Vec<_> = owned_paths
        .par_iter()
        .map(|(path, owner)| {
            let owner = *owner;
            let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let tree = match ast::parse(&src) {
                Ok(tree) => tree,
                Err(error) => return Ok(Err((path.clone(), error))),
            };
            drop(src);
            let is_marker_file = path.file_name().is_some_and(|s| s == "__init__.py");
            let module = path
                .strip_prefix(ws.packages[owner].root.join("src"))
                .ok()
                .map(|p| {
                    let mut s = p
                        .with_extension("")
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, ".");
                    if s == "__init__" {
                        s.clear()
                    } else if s.ends_with(".__init__") {
                        s.truncate(s.len() - 9);
                    }
                    s
                });
            let package = module.as_ref().map(|m| {
                if is_marker_file {
                    m.clone()
                } else {
                    m.rsplit_once('.').map_or("", |(p, _)| p).into()
                }
            });
            let filename = relative(path, &ws.root);
            let classification = ws.classify_in(path, owner);
            let symbols = symbols::resolve(&tree, package.as_deref());
            let nodes = ast::walk(&tree);

            let top_imports = extract_top_imports(&tree, package.as_deref());
            let init_imports = extract_init_imports(&tree, package.as_deref());
            let all_imports = extract_all_imports(&nodes, package.as_deref());

            let ctx = FileContext {
                path,
                filename: &filename,
                owner,
                module: module.as_deref(),
                package: package.as_deref(),
                classification: classification.as_ref(),
                tree: &tree,
                nodes: &nodes,
                symbols: &symbols,
            };
            let local = rules::check_local(&ctx, &ws.policy, ws.strict, ws.dead_code);
            // `tree`, `nodes`, and `symbols` drop right here at the end of the closure.
            Ok(Ok(Source {
                path: path.clone(),
                filename,
                owner,
                module,
                package,
                classification,
                is_marker_file,
                top_imports,
                init_imports,
                all_imports,
                top_defs: local.top_defs,
                classes: local.classes,
                deferred_local: local.deferred,
                local_findings: local.findings,
                dead_code: local.dead_code,
            }))
        })
        .collect::<Result<_, String>>()?;

    for result in parsed {
        match result {
            Ok(source) => ws.sources.push(source),
            Err(failure) => ws.parse_failures.push(failure),
        }
    }
    ws.module_index = ws
        .sources
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.module.clone().map(|m| (m, i)))
        .collect();
    ws.defined_packages = ws
        .module_index
        .keys()
        .map(|m| m.split('.').next().unwrap_or(m).to_owned())
        .collect();
    ws.init_closures = (0..ws.sources.len())
        .map(|_| OnceLock::new())
        .collect::<Vec<_>>()
        .into_boxed_slice();
    Ok(ws)
}

pub fn load(root: &Path) -> Result<Workspace, String> {
    load_impl(root, true, None, &[])
}

pub fn load_with_options(
    root: &Path,
    dead_code_override: Option<bool>,
    extra_entrypoints: &[String],
) -> Result<Workspace, String> {
    load_impl(root, true, dead_code_override, extra_entrypoints)
}

pub fn load_metadata(root: &Path) -> Result<Workspace, String> {
    load_impl(root, false, None, &[])
}

fn baseline_json_error(text: &str, error: serde_json::Error) -> String {
    let message = error.to_string();
    if message.starts_with("EOF while parsing an object")
        || message.starts_with("key must be a string")
    {
        let offset = if error.is_eof() {
            text.len()
        } else {
            text.split_inclusive('\n')
                .take(error.line().saturating_sub(1))
                .map(str::len)
                .sum::<usize>()
                + error.column().saturating_sub(1)
        };
        let before = &text[..offset.min(text.len())];
        let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        return format!(
            "baseline is not valid JSON: Expecting property name enclosed in double quotes: line {line} column {column} (char {})",
            before.chars().count()
        );
    }
    format!("baseline is not valid JSON: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn glob_segments_and_recursive_zero_depth() {
        for (path, glob) in [
            ("src/example/core.py", "src/*/core.py"),
            ("src/example/core.py", "src/**/*.py"),
            ("src/example/a/b/core.py", "src/**/*.py"),
            (
                "src/example/domains/orders/core.py",
                "src/*/domains/**/core.py",
            ),
            ("src/example/domains/core.py", "src/*/domains/**/core.py"),
            ("tests/test_x.py", "tests/**"),
            ("tests/a/b.py", "tests/**"),
            ("conftest.py", "conftest.py"),
            ("src/example/v2.py", "src/example/v[0-9].py"),
            ("src/example/a.py", "src/example/?.py"),
            ("src/app/core.py", "src/**/core.py"),
            ("core.py", "**/core.py"),
            ("tests/test_a.py", "tests/test_[a-z].py"),
        ] {
            assert!(matches_glob(path, glob), "{path} should match {glob}");
        }
        for (path, glob) in [
            ("src/example/a/core.py", "src/*/core.py"),
            ("tests/conftest.py", "conftest.py"),
            ("src/core.py", "src/*/domains/**/core.py"),
            ("src/example/core.pyi", "src/**/*.py"),
            ("src/example/ab.py", "src/example/?.py"),
            ("src/example.py", "src/**/x.py"),
            ("src/app/deep/core.py", "src/*/core.py"),
            ("tests/test_a.py", "tests/test_[!a-z].py"),
        ] {
            assert!(!matches_glob(path, glob), "{path} should not match {glob}");
        }
    }
    #[test]
    fn unclosed_toml_table_matches_python_diagnostic() {
        let text = "[workspace\n";
        let error = toml::from_str::<toml::Value>(text).unwrap_err();
        assert_eq!(
            toml_error(error, text),
            "Expected ']' at the end of a table declaration (at line 1, column 11)"
        );
    }
    #[test]
    fn virtual_environments_are_skipped_whatever_their_name() {
        let root = std::env::temp_dir().join(format!("leesh-native-venv-{}", std::process::id()));
        let site = root.join(".venv315/lib/python3.15/site-packages/dep");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::create_dir_all(root.join("src/example")).unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname='example'\n").unwrap();
        std::fs::write(root.join("src/example/core.py"), "").unwrap();
        std::fs::write(root.join(".venv315/pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        std::fs::write(site.join("__init__.py"), "").unwrap();
        std::fs::write(site.join("pyproject.toml"), "[project]\nname='dep'\n").unwrap();
        let ws = load(&root).unwrap();
        assert_eq!(ws.sources.len(), 1);
        assert!(!ws.sources[0].filename.contains(".venv315"));
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn native_workspace_classification_and_policy() {
        let root =
            std::env::temp_dir().join(format!("leesh-native-workspace-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/example/domains/orders")).unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname='example'\n").unwrap();
        std::fs::write(
            root.join("src/example/domains/orders/core.py"),
            "import httpx\n",
        )
        .unwrap();
        std::fs::write(root.join("src/example/__init__.py"), "").unwrap();
        let ws = load(&root).unwrap();
        assert_eq!(ws.sources.len(), 2);
        assert_eq!(
            ws.classify(Path::new("src/example/domains/orders/core.py"))
                .unwrap()
                .layer,
            "core"
        );
        assert_eq!(
            ws.classify(Path::new("src/example/__init__.py"))
                .unwrap()
                .layer,
            "marker"
        );
        assert!(ws.policy.forbidden_imports["core"].contains("httpx"));
        assert!(ws.module_source("example.domains.orders.core").is_some());
        assert!(ws.configuration_errors().is_empty());
        assert!(
            ws.sources
                .windows(2)
                .all(|pair| pair[0].path < pair[1].path)
        );
        let core_idx = ws.module_index("example.domains.orders.core").unwrap();
        let core = &ws.sources[core_idx];
        assert_eq!(core.filename, "src/example/domains/orders/core.py");
        assert_eq!(core.package.as_deref(), Some("example.domains.orders"));
        assert_eq!(&*core.top_imports[0].module, "httpx");
        assert_eq!(&*core.init_imports, &[("httpx".into(), None)]);
        let findings = crate::dependencies::check(core_idx, core, &ws);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "LIT102");
        std::fs::remove_dir_all(root).unwrap();
    }
}
