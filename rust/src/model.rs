use crate::{
    ast::Node,
    symbols::Symbols,
    workspace::{Policy, Workspace},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Finding {
    pub filename: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub message: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParseFailure {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub package: usize,
    pub layer: String,
    pub name: String,
    pub origin: String,
    pub glob: String,
}

/// Top-level `Import` / `ImportFrom` alias entry used by cross-file dependency resolution.
#[derive(Debug, Clone)]
pub struct TopImport {
    pub is_from: bool,
    pub module: Box<str>,
    pub member: Option<Box<str>>,
    pub local: Box<str>,
}

/// An `Import` or `ImportFrom` statement in BFS (`ast::walk`) order.
#[derive(Debug, Clone)]
pub struct ImportStmt {
    pub line: u32,
    pub column: u32,
    pub is_from: bool,
    pub raw_module: Option<Box<str>>,
    pub resolved_module: Option<Box<str>>,
    pub names: Box<[Box<str>]>,
}

/// Resolved base expression on a top-level `ClassDef` for `LIT403`.
#[derive(Debug, Clone)]
pub struct BaseRef {
    /// Empty means unresolved (`"?"`).
    pub resolved: Box<[Box<str>]>,
    /// Local class name if unresolved and `target` is a `Name`.
    pub local_target: Option<Box<str>>,
}

#[derive(Debug, Clone)]
pub enum TopDefKind {
    Class {
        id: u32,
        bases: Box<[BaseRef]>,
    },
    Value {
        id: u32,
        /// Empty means unresolved (`"?"`).
        resolved: Box<[Box<str>]>,
    },
    Import {
        target: Box<str>,
    },
}

#[derive(Debug, Clone)]
pub struct TopDef {
    pub name: Box<str>,
    pub kind: TopDefKind,
}

#[derive(Debug, Clone)]
pub struct ClassBaseCheck {
    pub line: u32,
    pub column: u32,
    /// Empty means unresolved (`"?"`).
    pub resolved: Box<[Box<str>]>,
    pub local_unparsed: Box<str>,
    pub local_target: Option<Box<str>>,
}

#[derive(Debug, Clone)]
pub struct ClassCheck {
    pub name: Box<str>,
    pub id: u32,
    pub has_declarative_base: bool,
    pub bases: Box<[ClassBaseCheck]>,
}

pub type InitImport = (Box<str>, Option<Box<str>>);
pub type Lit302Candidate = (u32, u32, Box<str>, Box<[Box<str>]>);
pub type Lit303Candidate = (u32, u32, Box<[Box<str>]>);
pub type Lit304Candidate = (u32, u32, Box<[Box<str>]>);

/// Rare local-rule candidates that depend on the final `Workspace.module_index`
/// (which excludes files that failed to parse).
#[derive(Debug, Clone, Default)]
pub struct DeferredLocal {
    /// `(line, col, attr, receiver_root_packages)` for `LIT302`.
    pub lit302: Vec<Lit302Candidate>,
    /// `(line, col, resolved_names)` for `LIT303` adapter construction.
    pub lit303: Vec<Lit303Candidate>,
    /// `(line, col, needed_from_import_modules)` for `LIT304` module patching.
    pub lit304: Vec<Lit304Candidate>,
}

/// Kind of definition tracked by deterministic dead-code analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadDefKind {
    Function,
    Class,
    Variable,
    TypeAlias,
    Reexport,
    Method,
    Property,
    Field,
    Param,
}

/// Bitflags describing semantic roles of a definition for dead-code analysis.
pub mod def_flags {
    pub const HAS_REG_DECORATOR: u16 = 1 << 0;
    pub const IS_PROTOCOL: u16 = 1 << 1;
    pub const IS_DATACLASS: u16 = 1 << 2;
    pub const IS_NAMED_TUPLE: u16 = 1 << 3;
    pub const IS_ENUM: u16 = 1 << 4;
    pub const IS_TYPED_DICT: u16 = 1 << 5;
    pub const IS_PYDANTIC: u16 = 1 << 6;
    pub const IS_ORM: u16 = 1 << 7;
    pub const IS_EXCEPTION: u16 = 1 << 8;
    pub const IS_STUB_BODY: u16 = 1 << 9;
    pub const IS_OVERRIDE: u16 = 1 << 10;
    pub const IS_OVERLOAD: u16 = 1 << 11;
    pub const IS_STATIC_OR_CLASSMETHOD: u16 = 1 << 12;
    pub const IS_PYTEST_FIXTURE: u16 = 1 << 13;
    pub const IS_PYDANTIC_HOOK: u16 = 1 << 14;
    pub const IS_ORM_HOOK: u16 = 1 << 15;
}

pub type ParamTypeEntry = (Box<str>, Box<[Box<str>]>);

/// Compact summary of one definition inside a file.
/// Local index `0` is always reserved for `MODULE_INIT` (top-level execution).
#[derive(Debug, Clone)]
pub struct DeadDef {
    pub local_id: u32,
    pub parent_id: u32,
    pub name: Box<str>,
    pub kind: DeadDefKind,
    pub line: u32,
    pub column: u32,
    pub flags: u16,
    /// For `Reexport`, the qualified target `module.symbol` (or `module`).
    pub reexport_target: Option<Box<str>>,
    /// For `Function` / `Method` / `Property`, the resolved return type(s) if annotated.
    pub return_types: Box<[Box<str>]>,
    /// For `Variable` / `TypeAlias` / `Field`, resolved direct value/annotation type(s).
    pub value_types: Box<[Box<str>]>,
    /// For `Variable` / `Field` / `Function` / `Method` / `Property`, container element type(s) (e.g. `dict[K, V]` -> `V`, `list[T]` -> `T`).
    pub elem_types: Box<[Box<str>]>,
    /// For `Class`, resolved base class qualified names (or local class names prefixed with `@`).
    pub bases: Box<[Box<str>]>,
    /// For `Function` / `Method`, parameter `(name, type_names)` in positional order (excluding self/cls).
    pub param_types: Box<[ParamTypeEntry]>,
}

/// A directed reference edge from an owning definition (`owner_id`, `0` = module init).
#[derive(Debug, Clone)]
pub struct DeadRef {
    pub owner_id: u32,
    /// Resolved qualified name (`mod.sym`) or `@local_name` for same-module top-level symbols.
    pub target: Box<str>,
    /// True if referenced as a bare value (outside `Call.func`, base class, or annotation),
    /// meaning it may be passed as a callback whose signature is externally constrained.
    pub as_value: bool,
}

/// An attribute/method access on a receiver inside `owner_id`.
#[derive(Debug, Clone)]
pub struct AttrRead {
    pub owner_id: u32,
    /// Resolved receiver type(s) (`mod.Class` or `@LocalClass`), empty if receiver type is unknown.
    pub receiver_types: Box<[Box<str>]>,
    pub attr: Box<str>,
    pub is_super: bool,
}

/// A wiring edge observed at a call or assignment site: concrete type flows into target type.
#[derive(Debug, Clone)]
pub struct TypeFlow {
    pub owner_id: u32,
    pub from_type: Box<str>,
    pub to_type: Box<str>,
}

/// Instance attribute type recorded on `self.<attr>` inside `__init__`.
#[derive(Debug, Clone)]
pub struct InitFieldType {
    pub class_local_id: u32,
    pub attr: Box<str>,
    pub direct_types: Box<[Box<str>]>,
    pub elem_types: Box<[Box<str>]>,
}

/// Compact per-file dead-code summary extracted while AST & Symbols are alive.
#[derive(Debug, Clone, Default)]
pub struct DeadCodeSummary {
    pub defs: Box<[DeadDef]>,
    pub refs: Box<[DeadRef]>,
    pub attr_reads: Box<[AttrRead]>,
    pub type_flows: Box<[TypeFlow]>,
    pub init_fields: Box<[InitFieldType]>,
    /// Types whose fields escape to serialization (`asdict`, `TypeAdapter`, boundary return, etc.).
    pub escaped_types: Box<[Box<str>]>,
    /// Local parameter `local_id`s that were read at least once in their owning function body.
    pub read_params: Box<[u32]>,
    /// Static `__all__` entries if defined as a literal list/tuple of strings.
    pub static_all: Option<Box<[Box<str>]>>,
    /// Static `pytest_plugins` module names declared in `conftest.py`.
    pub pytest_plugins: Box<[Box<str>]>,
    /// True if module has `if __name__ == "__main__":`.
    pub has_main_guard: bool,
    /// True if module has any top-level `Import` / `ImportFrom` with `pytest` or fixture usage.
    pub is_conftest: bool,
}

/// Lightweight per-file summary retained in `Workspace` after dropping full AST & Symbols.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub filename: String,
    pub owner: usize,
    pub module: Option<String>,
    pub package: Option<String>,
    pub classification: Option<Classification>,
    pub is_marker_file: bool,
    pub top_imports: Box<[TopImport]>,
    pub init_imports: Box<[InitImport]>,
    pub all_imports: Box<[ImportStmt]>,
    pub top_defs: Box<[TopDef]>,
    pub classes: Box<[ClassCheck]>,
    pub deferred_local: DeferredLocal,
    pub local_findings: Vec<Finding>,
    pub dead_code: Option<Box<DeadCodeSummary>>,
}

impl Source {
    pub fn layer(&self) -> &str {
        self.classification
            .as_ref()
            .map_or("", |c| c.layer.as_str())
    }
    pub fn layer_name(&self) -> &str {
        self.classification.as_ref().map_or("", |c| c.name.as_str())
    }
    pub fn has_layer(&self, layers: &[&str]) -> bool {
        layers.contains(&self.layer())
    }
    pub fn deterministic(&self, w: &Workspace) -> bool {
        w.policy
            .custom_layers
            .get(self.layer_name())
            .is_some_and(|c| c.deterministic)
    }
}

/// Ephemeral per-file AST + Symbols context live only while checking one file.
pub struct FileContext<'a> {
    pub path: &'a Path,
    pub filename: &'a str,
    pub owner: usize,
    pub module: Option<&'a str>,
    pub package: Option<&'a str>,
    pub classification: Option<&'a Classification>,
    pub tree: &'a Node,
    pub nodes: &'a [&'a Node],
    pub symbols: &'a Symbols,
}

impl<'a> FileContext<'a> {
    #[inline]
    pub fn layer(&self) -> &str {
        self.classification
            .as_ref()
            .map_or("", |c| c.layer.as_str())
    }
    #[inline]
    pub fn layer_name(&self) -> &str {
        self.classification.as_ref().map_or("", |c| c.name.as_str())
    }
    #[inline]
    pub fn has_layer(&self, layers: &[&str]) -> bool {
        layers.contains(&self.layer())
    }
    #[inline]
    pub fn deterministic(&self, policy: &Policy) -> bool {
        policy
            .custom_layers
            .get(self.layer_name())
            .is_some_and(|c| c.deterministic)
    }
}

#[inline]
pub fn finding_at(
    filename: &str,
    line: usize,
    column: usize,
    code: &str,
    message: impl Into<String>,
    target: Option<String>,
) -> Finding {
    Finding {
        filename: filename.to_owned(),
        line,
        column,
        code: code.to_owned(),
        message: message.into(),
        target,
    }
}

#[inline]
pub fn finding(
    file: &FileContext<'_>,
    node: &Node,
    code: &str,
    message: impl Into<String>,
    target: Option<String>,
) -> Finding {
    let (line, column) = node.position().unwrap_or((1, 0));
    finding_at(file.filename, line, column, code, message, target)
}

pub fn relative_name(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
