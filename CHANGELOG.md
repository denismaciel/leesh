# Changelog

## 0.23.0

### Dead-Code Analysis (Opt-In)

Added deterministic, layer-aware dead-code and static-analyzability analysis,
enabled via `[dead_code] enabled = true` in `leesh.toml` or `leesh check --dead-code`
(with optional repeatable `--entrypoint <module_or_path>`).

- **Static analyzability rules (`LIT501`–`LIT503`):**
  - **LIT501 `opaque-dynamic-dispatch`:** Disallows dynamic `getattr`/`setattr`/`delattr`,
    `eval`/`exec`/`compile`, `__import__`, `importlib.import_module`, `globals`/`locals`/`vars`,
    `inspect.getmembers`, and `runpy.*` outside `boundary`/`router`/`test`/`integration_test`.
  - **LIT502 `opaque-export`:** Disallows non-literal `__all__`, `__all__ += ...`,
    and wildcard `from x import *` imports.
  - **LIT503 `unregistered-decorator`:** Flags unknown decorators outside
    `boundary`/`router`/`test`/`integration_test` unless listed in
    `[dead_code].live_decorators` or `[dead_code].transparent_decorators`.
- **Dead-code rules (`LIT510`–`LIT514`):**
  - **LIT510 `unreachable-module`:** Flags production modules unreachable from
    configured/discovered entrypoints and test helpers unreachable from any test.
  - **LIT511 `unused-symbol`:** Flags unused top-level functions, classes, type
    aliases, and module variables (including symbols referenced only by tests),
    with PEP 695 `type_params` and `Literal[Enum.MEMBER]` tracking.
  - **LIT512 `unused-member`:** Flags unused methods, properties, and `core`
    dataclass fields using structural `Protocol` dispatch (no explicit
    subclassing required) and class-scoped type propagation; exempts Pydantic/ORM
    fields and `boundary`/`router` serialization contracts.
  - **LIT513 `unreachable-code`:** Intra-procedural fall-through analysis after
    `return`/`raise`/`break`/`continue`, unsatisfiable `if`/`while` branches,
    and unreachable `try`/`while` `else` clauses (exempting the Python
    `raise ...; yield ...` generator stub idiom).
  - **LIT514 `unused-parameter`:** Flags unused function parameters (respecting
    `_` prefixes, `Protocol`/`abstractmethod`/`@override` stubs, and lambdas).
- **Configuration & CLI:**
  - `[dead_code]` table in `leesh.toml` (`enabled`, `entrypoints`,
    `live_decorators`, `transparent_decorators`, `ignore_names`, `test_only`,
    `strict_members`).
  - Automatic entrypoint discovery from `pyproject.toml` `[project.scripts]`,
    `[project.gui-scripts]`, `[project.entry-points]`, pytest plugins, and
    `__main__.py`.
  - `leesh check --dead-code` and `--entrypoint` flags.

## 0.22.0

### Rules

- **LIT105 `import-cycle` (new).** Reports strongly connected import cycles across
  workspace modules.
- **LIT106 `deferred-import` (new).** Disallows function-local imports (outside
  `boundary`, `test`, `marker`) and `if TYPE_CHECKING:` blocks in `core`/`port`.
- **LIT208 `dynamic-code` (new).** Core and port code may not call `eval`,
  `exec`, `compile`, `__import__`, `importlib.import_module`, `globals`,
  `locals`, `breakpoint`, dynamic `getattr`, `__dict__`, or use `pickle`,
  `marshal`, `shelve` and `runpy`.
- **LIT209 `untyped-core` (new).** Core and port functions/records must annotate
  parameters and return types without `Any`, `TypedDict`, or unparameterized
  collections.
- **LIT307 `broad-except` (new).** Disallows bare `except:` and broad
  `except Exception:` / `except BaseException:` catches that do not re-raise
  (outside `boundary`, `router`, `test`, `marker`).
- **LIT308 `defensive-duck-typing` (new).** Disallows `hasattr`, 3-argument
  `getattr` with defaults (in `core`/`shell`), and `typing.cast` (in `core`).
- **LIT309 `test-hygiene` (new).** Disallows `time.sleep` in tests; when
  `workspace.strict = true`, also disallows importing `adapter` modules in unit
  tests (`test` layer, while `integration_test` may import adapters).
- **LIT404 `core-classes` (new).** Disallows plain classes in `core`; when
  `workspace.strict = true`, also flags non-dunder/non-property methods on
  `core` dataclasses and `NamedTuple`s.
- **LIT405 `core-exceptions` (new, opt-in via `workspace.strict = true`).**
  Flags `raise` statements in `core` outside class `__post_init__` (exempting
  `AssertionError`, `NotImplementedError`, and `outcome_suffixes` handled by
  `LIT402`).
- **LIT406 `explicit-arguments` (new, opt-in via `workspace.strict = true`).**
  Disallows parameter and dataclass field defaults on public `core`/`port`
  APIs.
- **Tighter defaults & purity rules:**
  - `core` may now import only `core` (not `port`).
  - `LIT203` bans `@cache`/`@lru_cache` outside `core`, scopes `nonlocal` to
    `core`/`port`, and tracks more module-level mutable state (`defaultdict`,
    `deque`, `Counter`, `OrderedDict`, `ChainMap`, `bytearray`, `iter`,
    `itertools.count`/`cycle`).
  - `LIT201` bans `time.*` in `core`/`port` (except `time.struct_time`).
  - `LIT202` treats `os` and `sys` as effectful by default (except pure path
    arithmetic and interpreter constants).
  - `LIT204` tracks element views, shallow copies, and untyped mutator calls.
  - `LIT205` inspects compound string annotations, `NamedTuple`s,
    `default_factory`, and `object.__setattr__` outside `__post_init__`.
  - `LIT303` checks both `router` and `shell` for I/O and adapter construction.
  - `LIT304` catches patching packages (`responses`, `respx`, `freezegun`,
    `time_machine`, `httpretty`, `mock`), `monkeypatch`, `pytest-mock`, and
    module attribute patching.
  - `LIT305` checks lambda defaults.
  - `getattr` with a literal attribute name is resolved like attribute access.

### Layouts & Configuration

- **Built-in `integration_test` layer** (`base = "test"`): automatically
  classifies `conftest.py` and integration/e2e/persistence/contract/smoke/
  fixture/helper/fake test paths so unit-test strictness (`LIT309`) does not
  flag integration test setup.
- **`[workspace] strict = false`** (opt-in) enables strict checks (`LIT309`
  unit-test adapter imports, `LIT404` record methods, `LIT405`, `LIT406`).
- Optional `review_by = "YYYY-MM-DD"` expiry dates on `[[overrides]]`,
  `[[dependency_overrides]]`, `[[allowed_dependencies]]`, and `[[ignore]]`, plus
  `max_count` on `[[ignore]]`.

### Architecture & Performance

- **Rust-only engine.** Removed the legacy Python reference engine; the PyPI
  package now ships only the thin Python launcher shim (`src/leesh/_native.py`)
  and the native `leesh-rust` executable.
- **4.6×–51× faster and 4×–8.6× less memory.** Streaming per-file AST and symbol
  lifecycles drop syntax trees and symbol tables immediately after local checks,
  paired with an undo-log symbol resolver, lock-free per-marker import closures,
  O(1) package/override lookups, and streaming JSON output:
  - Django (2,818 files): `0.65 s → 0.14 s` multi-core (`3.15 s → 0.69 s` single-core);
    peak RSS `362 MB → 95 MB` multi-core (`322 MB → 44 MB` single-core).
  - Apache Airflow (5,371 files, 102 packages): `12.25 s → 0.24 s` multi-core
    (`29.85 s → 1.19 s` single-core); peak RSS `490 MB → 97 MB` multi-core
    (`456 MB → 53 MB` single-core).

### Upgrading

- Every new or tightened rule can add findings. Run `leesh report`, then fix
  them or run `leesh baseline` to record them.

## 0.21.0

- Complete the Leesh rename: `leesh` Python module, CLI, native executable,
  environment variables, configuration (`leesh.toml`), and baseline filenames.
- Remove the old Python module, CLI alias, and pre-commit hook alias.


## 0.20.2

- Publish as **Leesh** on PyPI, with `leesh` as the primary command and
  `leesh` retained as an alias. Existing configuration and Python imports work.
- Build Python distributions with uv and uv_build; remove Hatch.
- Ship verified static Linux binaries in manylinux/musllinux wheels.
- Publish tested release artifacts with protected GitHub Actions Trusted
  Publishing, short-lived credentials, and PyPI attestations.

## 0.20.1

### Fixes

- **Virtual environments are skipped whatever their name.** File discovery now
  prunes any directory containing `pyvenv.cfg`, not only one named `.venv`. A
  checkout with `.venv315/` or `env/` beside `pyproject.toml` no longer parses
  every installed package, which could exhaust memory and get the process killed.

### Performance

- **A tenth of the memory, four times faster.** Syntax trees are compact typed
  nodes instead of JSON objects: kind, id and start position are struct fields,
  field names are static, and null or empty fields and end positions are not
  stored. Symbol tables keep only resolved expressions and share each qualified
  name across the workspace. Peak memory falls from 3.9 GB to 0.33 GB on Django
  and from 4.6 GB to 0.38 GB on a 2,600-file service, and a full check takes
  0.6 s instead of 2.8 s on Django. Findings are unchanged.

## 0.20.0

### Rules

- **LIT302 ignores third-party receivers outside databases.** A `commit()` or
  `rollback()` whose receiver resolves only to third-party packages other than
  database drivers and ORMs (`sqlalchemy`, `asyncpg`, `psycopg`, `sqlite3`,
  `django`, ...) is no longer reported, so building a search index with
  `tantivy.Index(...).writer().commit()` stays legal in a service. Unresolved,
  first-party and database receivers are still reported.

### Upgrading

- `leesh baseline --prune` drops LIT302 entries for such receivers.

## 0.19.0

- The existing Python package now runs the Rust implementation, using Ruff's parser.
  Package name, Git-tag installation, pre-commit hook, CLI commands and output
  formats remain unchanged. No Rust toolchain is needed on supported clients.
- Ship verified native binaries and platform wheels for Linux x86-64/ARM64,
  macOS Intel/Apple Silicon and Windows x86-64. Git/sdist builds download the
  matching release binary and verify its pinned SHA-256; runtime is offline.
- Preserve the Python implementation as the equivalence reference. The port
  passes the existing regression suite and full Requests/Flask diagnostic checks.
- Native parsing, worker threads, moved AST subtrees and cached import traversals
  improve measured single-worker runtime by roughly 3.2–3.3× versus Python.


## 0.18.0

### Layouts

- **Fakes next to production code.** In `domains`, `fakes.py`, `fake.py` and
  `fakes/` anywhere under the package are `test`, so an in-memory repository
  can sit beside the port it implements.

### Rules

- **LIT101 keeps test code out of production.** Layers without a dependency
  table, such as boundary and transaction, may still import anything except
  test code; only test modules may import test modules. A composition root
  can no longer wire a fake into production.

### Upgrading

- Boundary or transaction modules that import a `test` or `test-support`
  package get `LIT101` findings. Record deliberate edges with
  `[[dependency_overrides]]` or `leesh baseline`.

## 0.17.0

### Layouts

- **`domains` classifies a top-level `db/` package.** `src/<pkg>/db/**` is an
  adapter, so tables and repositories can live outside the domains, and
  `db/unit_of_work.py` or `db/unit_of_work/` is a transaction. Files there
  were previously unclassified.

## 0.16.0

### Rules

- **LIT104 `cross-domain-import`.** A module under `<package>/domains/<name>/`
  may import another domain only when `[domains]` declares the dependency,
  and then only that domain's core and shell modules; its ports,
  `persistence/` and boundaries stay private. Imports are followed through
  package initializers and re-exports, and `[[dependency_overrides]]` exempts
  one edge.

### Manifest

- **`[domains]`** maps each domain to the domains it may import. The graph is
  validated on load: every named domain needs its own entry, and
  self-dependencies and cycles are manifest errors that report the cycle. A
  declared domain without a `domains/<name>/` package is a `LIT003` finding.

### Upgrading

- Workspaces whose domains already import each other get `LIT104` findings
  until they declare `[domains]`, or record them with `leesh baseline`.

## 0.15.0

### Layouts

- A conventional module can grow into a package of the same name. In
  `domains`, `projections` and `shared`, `core/`, `model/`, `policy/`,
  `event/`, `ports/`, `service/` and `boundary/` packages take the layer of
  the matching `.py` module, as do `model/`, `core/`, `policy/`, `ports/` and
  `cli/` in `flat`. Files inside such a package take its layer whatever their
  own name; `persistence/` still wins below it. Previously these files were
  unclassified (`LIT002`) in `domains`, and shell in `flat`.

## 0.14.0

### Rules

- **LIT305** accepts immutable references as parameter defaults: `UPPER_CASE`
  constants, enum members such as `Status.OPEN`, tuples of those, and
  `frozenset`, `tuple`, `Decimal`, `Fraction` and `timedelta` calls over
  immutable arguments. A same-module constant bound to a mutable value, any
  other call, and lambdas are still reported.

## 0.13.0

Adopting leesh on an existing codebase, and flexibility for layouts that do
not match the built-in ones.

### Adoption

- **Baseline.** `workspace.baseline` names a JSON file of known findings,
  keyed by file, rule and message rather than line. `leesh baseline` writes
  it; `check` reports only findings beyond it, plus a `LIT003` finding for
  entries that no longer match; `leesh baseline --prune` only ever removes
  entries. `check --no-baseline` shows everything.
- **`[[ignore]]`** switches rules off by path glob and/or layer, with a reason.
  Unknown rules, `LIT001`/`LIT003`, and entries that suppress nothing in a full
  check are `LIT003` findings.
- **`leesh report`** ranks findings (new and baselined) by rule, `LIT101`
  layer edge, target and directory, as text or JSON; `--rule` focuses it.
- Findings carry a `target`: the module, package, base, call or name they are
  about. JSON output includes it, and the summary adds `baselined` and
  `ignored` counts.

### Flexibility

- **Custom layers.** `[layers.<name>]` with a built-in `base` follows that
  layer's rules under its own name. It inherits the base's dependency, import
  and call policy unless the manifest names it, and counts as its base when
  imported. `deterministic = true` holds it to core's `LIT201` standard.
- **`LIT306 forbidden-call`** and `[forbidden_calls]`: per-layer bans on
  resolved call names, `module.*` for a whole module. Nothing by default.
- **`[[allowed_dependencies]]`** lets listed importer layers use named
  workspace modules (`module.*` for a tree) regardless of their layer, with a
  reason. Entries that match no module are `LIT003` findings.
- **Layout precedence.** A custom layout's globs now come after the
  conventional `tests/`, `scripts/`, `examples/` and `migrations/` patterns,
  so they can reclassify them. `src/**/__init__.py` markers still come last.

### Speed

- A 3,000-file workspace checks in about 7s instead of 72s with identical
  findings: top-level imports, initialization imports and resolved symbols are
  computed once per module, cyclic garbage collection is off during a run, and
  rule checks fan out to forked workers (`--jobs`, default one per CPU; falls
  back to in-process where `fork` is unavailable).

### Fixes

- A string constant holding a lone surrogate no longer crashes symbol
  resolution, and parsing strings as forward references no longer leaks
  `SyntaxWarning`s.
- Findings at the same position are ordered by message, so output is stable.

### Python API

- `Policy.layer_dependencies`, `forbidden_imports` and the new
  `forbidden_calls` are keyed by layer name. `Classification` gains `name`
  (the configured layer; `layer` is the base layer whose rules apply).
  `engine.run` returns new, baselined and ignored findings; `engine.check`
  still returns the findings to report.

## 0.12.0

- `LIT403 restricted-subclassing`: a class may only extend an allowlisted base
  (builtins, `typing`/`enum` bases, Pydantic models and settings, SQLAlchemy
  declarative bases and `TypeDecorator`) or a workspace class that directly
  extends one. Exception hierarchies may nest to any depth, and tables may mix
  in plain workspace classes next to a declarative base. Configure with `conventions.allowed_bases` and
  `conventions.extend_allowed_bases`.

## 0.11.0

- `[extend_forbidden_imports]` adds packages to a layer's forbidden list
  instead of replacing it; the key `"*"` adds them to every layer. Use it to
  keep a legacy package out of a new one without copying the defaults.

## 0.10.0

A redesign for the first public release. Nothing is backward compatible with
0.9: the manifest schema, rule codes, CLI and Python API have all changed.

### Workspace model

- A manifest is optional. Without `leesh.toml`, every package is a `server`
  with the `domains` layout. There is no filename-guessing mode: every rule
  uses the layer of the file it checks.
- Packages have a `kind` (`server`, `client`, `library`) and a `layout`,
  replacing the eight package roles.
- Nine layers: `core`, `port`, `shell`, `adapter`, `router`, `boundary`,
  `transaction`, `test`, `marker`. Migrations are `transaction`; tests and
  fixtures are `test`.
- Built-in layouts `domains`, `hexagonal`, `flat`, `boundary` and
  `test-support`. Custom `[layouts.<name>]` may `extends` a built-in.
- Policy is data: `[layer_dependencies]`, `[forbidden_imports]` and
  `[conventions]` replace the built-in entries they name.
- Manifest problems are findings (`LIT003`). A manifest that cannot be loaded
  exits with status 2.

### Rules

Rules are renumbered by family and use the `LIT` prefix (0.9 used `PAL`). Each
has a name and a docstring, and `docs/rules.md` is generated from them.

| New | Replaces |
| --- | --- |
| LIT001 parse-error | PAL000 |
| LIT002 unclassified-file | PAL016 |
| LIT003 manifest | CLI coverage and override errors |
| LIT101 layer-dependency | PAL003, PAL010, PAL017, boundary imports in PAL024 |
| LIT102 forbidden-import | PAL007, PAL009, PAL012, imports in PAL014, PAL024 frameworks |
| LIT103 client-imports-server | PAL008 |
| LIT201 nondeterminism | PAL011 |
| LIT202 io-in-core | calls in PAL014 |
| LIT203 ambient-state | PAL020, without environment access |
| LIT204 borrowed-mutation | PAL019 |
| LIT205 frozen-values | PAL021 |
| LIT206 async-in-core | PAL022 |
| LIT207 effectful-capability | PAL023 |
| LIT301 environment-access | PAL004 |
| LIT302 transaction-boundary | PAL005 |
| LIT303 router-construction | PAL006 |
| LIT304 mocking | PAL002 |
| LIT305 immutable-defaults | PAL001, without the FastAPI exemptions |
| LIT401 boolean-policy | PAL013 |
| LIT402 expected-outcome | PAL015 |

- PAL018 (runtime settings naming) is removed.
- LIT101 checks every restricted layer, not only core and port: shells may not
  import adapters or edges, adapters may not import shells, and routers may not
  import core or adapters directly.
- LIT301 now allows environment access only in boundary, transaction and test
  modules; routers are no longer exempt.
- Identical findings are reported once.

### CLI

- Subcommands: `check`, `classify`, `explain`, `init`, `rules`.
- `check --format text|json|github`. JSON is `{"findings": [...], "summary": {...}}`.
  Filenames are relative to the workspace root.
- `check PATH...` reports findings under those paths but always analyzes the
  whole workspace.
- `python -m leesh` runs the CLI.

### Removed

- The Flake8 adapter.
- `leesh-advisory` (PLC001–006) and `leesh-packages` (PKG001–003).

### Distribution

- Python 3.12 and later. CI tests 3.12, 3.13 and 3.14.
- A pre-commit hook (`.pre-commit-hooks.yaml`).

## 0.9.1

- Standalone release extracted from the existing 0.9.0 package.

## 0.9.0

Previously distributed through a private package index.
