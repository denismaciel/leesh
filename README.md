<p align="center"><img src="docs/icon.svg" width="96" height="96" alt=""></p>

# Leesh

**A leash for LLM-written Python.** Strict architecture linting for functional cores and imperative shells, powered by Ruff's Rust parser.

## Why

Python written by LLMs tends to leak I/O into business logic, pass mutable state everywhere, and invent ad-hoc abstractions. Leesh enforces a small set of architectural boundaries mechanically:

- [Functional Core, Imperative Shell](https://www.destroyallsoftware.com/screencasts/catalog/functional-core-imperative-shell) — separate business logic from code that talks to the outside world.
- [Parse, don't validate](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/) — parse untrusted input at the boundary (e.g. with Pydantic), convert to dataclasses, then run pure logic.
- [The Zen of Python](https://peps.python.org/pep-0020/) — one obvious way to structure modules, dependencies, and data:
  - Isolate side effects and avoid global state.
  - Inject dependencies explicitly so domain logic is trivial to test.
  - Model domain data as well-structured dataclasses; prefer functions over methods and avoid unnecessary subclassing.
  - Raise exceptions only at defined boundaries; avoid hidden defaults.

## Install

Requires Python 3.12+. Prebuilt wheels bundle the native Rust binary for Linux (x86-64, ARM64, glibc and musl), macOS (Intel 10.13+, Apple Silicon 11+), and Windows (x86-64) — no Rust toolchain required.

```bash
uv tool install leesh
```

Or run without installing persistently:

```bash
uvx leesh check
```

To add it to a project's development dependencies:

```bash
uv add --dev leesh
```

With [pre-commit](https://pre-commit.com/):

```yaml
repos:
  - repo: https://github.com/denismaciel/leesh
    rev: v0.23.0
    hooks:
      - id: leesh
```

## Quick start

```bash
leesh init --layout domains   # write a starter leesh.toml
leesh check                   # analyze the workspace
```

Common commands:

```bash
leesh check src/app/orders    # report findings under selected paths only
leesh report                  # rank findings by rule, layer edge, target, and area
leesh baseline                # record current findings so only new ones fail
leesh classify src            # show each file's layer and matching glob
leesh explain LIT101          # print a rule's documentation
leesh rules                   # list all rules
```

## How it works

Every Python file belongs to a layer determined by its path. Each layer defines which other layers it may import and which side effects or language constructs it may use. Imports are resolved across packages, re-exports, and `__init__.py` files, so a forbidden dependency cannot be laundered through a re-export.

`leesh check` locates the workspace root from the nearest `leesh.toml` (falling back to `pyproject.toml`), discovers every package containing a `pyproject.toml`, and analyzes the whole workspace in parallel.

### Layers

| Layer | Holds | May import (default) |
| --- | --- | --- |
| `core` | decisions, values, policies | `core` |
| `port` | interfaces the core needs | `core`, `port` |
| `shell` | orchestration of core and ports | `core`, `port`, `shell` |
| `adapter` | implementations of ports | `core`, `port`, `adapter` |
| `router` | HTTP handlers | `shell`, `boundary`, `router`, `transaction` |
| `boundary` | entry points, configuration, schemas | anything |
| `transaction` | units of work, migrations | anything |
| `test` | tests and fixtures | anything |
| `marker` | `__init__.py` files | anything (followed, never a target) |

### Layouts and packages

A package's `layout` maps paths to layers. Built-in layouts are:

- `domains` — `src/<pkg>/domains/<domain>/{core,model,policy,ports,service}.py`, `app/`, `db/`, `integrations/`
- `hexagonal` — `domain/`, `application/`, `adapters/`, `web/`
- `flat` — shell by default, with `model.py`, `core.py`, `policy.py`, `ports.py`, `adapters/`, and `cli.py`
- `boundary` — every source file is a boundary
- `test-support` — every source file is test code

Conventional modules can grow into packages of the same name (`domains/orders/core/` is `core`, like `domains/orders/core.py`). A package's `kind` (`server`, `client`, or `library`) restricts cross-package dependencies. Custom layouts and custom layers (`[layers.<name>]`) let you match your repository's structure.

### CLI options and exit codes

- `--format text|json|github` — human-readable output, JSON (`{"findings": [...], "summary": {...}}`), or GitHub Actions annotations (`check`; `report` supports `text` and `json`).
- `--jobs N` — override worker thread count.
- `--dead-code` — enable static analyzability and dead-code rules (`LIT501`–`LIT514`), optionally with `--entrypoint`.
- `--no-baseline` — ignore the configured baseline file.
- Exit status: `0` when clean, `1` when findings are reported, `2` on usage errors or invalid manifests.

### Adopting on an existing codebase

Describe your current layout before moving files: define a custom layout or custom layers in `leesh.toml`, then:

1. Run `leesh report` to see which rules, layer edges, and imported modules drive findings, separating policy choices from real debt.
2. Record intentional exceptions in `leesh.toml` (`[[allowed_dependencies]]`, `[[ignore]]`, `[conventions]`), each with a documented `reason`.
3. Set `workspace.baseline` and run `leesh baseline`. `leesh check` then fails only on new findings and on stale baseline entries until you run `leesh baseline --prune`, so the baseline only shrinks.

## Reference

- [Rules reference](docs/rules.md)
- [Configuration reference](docs/configuration.md)
- [Changelog](CHANGELOG.md)

## Development

See [docs/development.md](docs/development.md) for local setup, building the Rust binary, running tests, and release procedures.

## License

MIT. See [LICENSE](LICENSE).
