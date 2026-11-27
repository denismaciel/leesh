# Development

Leesh is implemented in Rust (`rust/`) and packaged with a thin Python launcher shim (`src/leesh/`) for PyPI and `uvx` distribution.

## Environment

Enter the Nix development shell for the pinned Rust toolchain (`1.100.0` via `rust-toolchain.toml`), Python, and `uv`:

```bash
nix develop
```

Without Nix, install Rust matching `rust-toolchain.toml`, a C compiler and `make` (for jemalloc on Unix), Python 3.12+, and `uv`.

## Building and testing

Build the native binary locally and point the Python launcher at it during development:

```bash
cargo build --locked --release --manifest-path rust/Cargo.toml
export LEESH_NATIVE_BINARY="$PWD/rust/target/release/leesh-rust"
make dev
make check
make test
```

Makefile targets:

| Target | What it runs |
| --- | --- |
| `make dev` | `uv sync --locked` |
| `make check` | `ruff format --check`, `ruff check`, `ty check` |
| `make test` | `pytest` and `leesh check` |
| `make build` | `uv build --no-sources` |
| `make release-check` | Verify built wheel and sdist in clean environments |
| `make release-prepare RUN_ID=<id>` | Generate `native-artifacts.json` from a native workflow run |

To run Rust checks directly:

```bash
cargo test --locked --release --manifest-path rust/Cargo.toml
cargo clippy --locked --release --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path rust/Cargo.toml --check
```

## Rules and documentation

Rules live in `rust/src/rules/` and are documented in `rust/src/registry.json`. After editing `rust/src/registry.json`, rebuild the binary and regenerate the rule reference:

```bash
uv run leesh rules --format markdown > docs/rules.md
```

## Installing from Git or source

Git and source-distribution installs download the matching release binary and verify its pinned SHA-256 against `native-artifacts.json`; installed packages run offline:

```bash
uv add --dev 'leesh @ git+https://github.com/denismaciel/leesh.git@v0.23.0'
```

## Further reading

- [Native Rust implementation and profiling](../rust/README.md)
- [Performance benchmarks](../benchmarks/README.md)
- [Packaging and release process](releasing.md)
