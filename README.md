<p align="center"><img src="docs/icon.svg" width="96" height="96" alt=""></p>

# Leesh

**A leash for LLM-written Python.**

## Why?

Python written by LLMs sucks.

Leesh is the remedy for that. It's based on a handful of good ideas:

- [Functional Core, Imperative Shell](https://www.destroyallsoftware.com/screencasts/catalog/functional-core-imperative-shell).
- [Parse, don't validate](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/).
- [The Zen of Python](https://peps.python.org/pep-0020/): "There should be one, and preferably only one, obvious way to do it."

- Separate business logic from code that talks to the outside world (data you don't control).
- Isolate side effects.
- Build on pure functions and values.
- Don't rely on global state.
- Inject your dependencies; it makes testing easier.
- Parse outside data with Pydantic, convert it to dataclasses, then run the business logic.
- Model domain data as well-structured dataclasses.
- Prefer functions to methods.
- You rarely need subclassing.
- Raise exceptions only in specific places.
- No default arguments or default values. Be explicit: you're not the one writing the code, the LLM is.

## How it works

Every file belongs to a layer, decided by its path: `core`, `port`, `shell`,
`adapter`, `router`, `boundary`, `transaction`, `test`. Each layer has rules
about what it may import and which effects it may perform. Imports are followed
across packages, re-exports and `__init__.py` files, so a dependency can't be
laundered through a re-export.

## Install

Install Leesh from PyPI. The CLI runs a bundled Rust executable; clients do not
need Cargo or a Rust toolchain.

```bash
uv tool install leesh
leesh check
```

Run it without installing a persistent tool:

```bash
uvx leesh check
```

For a project's development dependencies, use `uv add --dev leesh` and
`uv run leesh check`. To pin the Git source instead:

```bash
uv add --dev 'leesh @ git+https://github.com/denismaciel/leesh.git@v0.22.0'
```

Or run it with pre-commit:

```yaml
repos:
  - repo: https://github.com/denismaciel/leesh
    rev: v0.22.0
    hooks:
      - id: leesh
```

Git and source-distribution installs download the matching executable from that
release and verify its pinned SHA-256. Installed packages run offline. Prebuilt
wheels are also attached to the release. Supported platforms: Linux x86-64 and
ARM64 (including musl), macOS Intel (10.13+) and Apple Silicon (11+), and Windows
x86-64. Python 3.12+ remains required.

The Python import, CLI, configuration file (`leesh.toml`), and native executable
(`leesh-rust`) use the Leesh name. The old module and command aliases are removed.

## Use

```bash
leesh init --layout domains   # write a starter leesh.toml
leesh check                   # check the whole workspace
leesh check src/app/orders    # report findings under these paths only
leesh report                  # rank findings by rule, layer edge, target and area
leesh baseline                # record current findings so only new ones fail
leesh classify src            # show each file's layer and the pattern that matched
leesh explain LIT101          # show a rule's documentation
leesh rules                   # list every rule
```

`check` finds the workspace root from the nearest `leesh.toml` (or
`pyproject.toml` if there is none), discovers every package with a
`pyproject.toml` below it, and always analyzes the whole workspace so that
imports are followed across files and packages. Output formats are `text`,
`json` (`{"findings": [...], "summary": {...}}`) and `github` (workflow
annotations). Exit status is 0 when clean, 1 when there are findings, and 2 for
usage errors or a manifest that cannot be loaded. Files are analyzed with native worker threads; `--jobs 1` uses one worker.

### Adopting Leesh on an existing codebase

Describe the code you have instead of moving it first: a custom layout maps
today's directories to layers, and custom layers name the roles that don't fit
the built-in ones (a Temporal workflow is a deterministic router). Then:

1. `leesh report` shows which rules, layer edges and imported modules produce
   the findings, so you can tell policy questions from real debt.
2. Settle policy in the manifest: `[[allowed_dependencies]]` for modules that
   act as ports, `[[ignore]]` for a rule you are not adopting yet in a given
   place, `[conventions]` for your framework bases. Each needs a reason.
3. Set `workspace.baseline` and run `leesh baseline`. `check` now fails only
   on new findings, and on baseline entries that no longer match until you run
   `leesh baseline --prune`, so the baseline can only shrink.

## Layers

| Layer | Holds | May import (default) |
| --- | --- | --- |
| `core` | decisions, values, policies | core |
| `port` | interfaces the core needs | core, port |
| `shell` | orchestration of core and ports | core, port, shell |
| `adapter` | implementations of ports | core, port, adapter |
| `router` | HTTP handlers | shell, boundary, router, transaction |
| `boundary` | entry points, configuration, schemas | anything |
| `transaction` | units of work, migrations | anything |
| `test` | tests and fixtures | anything |
| `marker` | `__init__.py` files | anything (followed, never a target) |

A package's `layout` maps paths to layers. The built-in layouts are `domains`
(`src/<pkg>/domains/<domain>/{core,model,policy,ports,service}.py`, `app/`,
`db/`, `integrations/`), `hexagonal` (`domain/`, `application/`, `adapters/`, `web/`),
`flat` (everything is shell, except `model.py`, `core.py`, `policy.py`,
`ports.py`, `adapters/` and `cli.py`), `boundary`, and `test-support`. A
conventional module may grow into a package of the same name: the files of
`domains/orders/core/` are core like `domains/orders/core.py`. A
package's `kind` (`server`, `client` or `library`) controls whether other
packages may depend on it. Custom layers (`[layers.<name>]`) follow a built-in
layer's rules with their own dependency policy.

See [docs/rules.md](docs/rules.md) for every rule and
[docs/configuration.md](docs/configuration.md) for the manifest.

## Rust implementation

Leesh is implemented in Rust and packaged with a thin Python launcher shim for
PyPI and `uvx` distribution. To build Rust locally:

```bash
cargo build --locked --release --manifest-path rust/Cargo.toml
rust/target/release/leesh-rust check
```

It reuses Ruff's parser, parallelizes analysis, and exposes Leesh's rules and
CLI commands. See [build and verification instructions](rust/README.md) and the
[performance benchmarks](benchmarks/README.md).

## Development

```bash
cargo build --locked --release --manifest-path rust/Cargo.toml
export LEESH_NATIVE_BINARY="$PWD/rust/target/release/leesh-rust"
make dev check test
make build release-check
uv run leesh rules --format markdown > docs/rules.md  # after changing rust/src/registry.json
```

Python packaging uses uv and uv_build with a small native-artifact adapter.
See [release instructions](docs/releasing.md) for preparation and tagged releases.

Rules live in `rust/src/rules/` and are documented in `rust/src/registry.json`.

## License

MIT. See [LICENSE](LICENSE).
