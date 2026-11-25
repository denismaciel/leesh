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
