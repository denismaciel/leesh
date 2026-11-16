# Releasing Leesh

The Python distribution is `leesh`, with primary CLI `leesh` and compatibility
alias `lint-it`. The import package remains `lint_it`; config filenames, native
executable/asset names and repository remain unchanged.

Distribution uses PyPI plus GitHub Git tags and attached Python wheels/sdists. Installing
from Git or an sdist uses `build_backend.py` to fetch a release executable and verify
its SHA-256 against the committed `native-artifacts.json`. Wheel installations
need no download. No runtime fallback or runtime network access is used.

1. Use a numeric `X.Y.Z` version shared by Python and Cargo. Update `pyproject.toml`, `rust/Cargo.toml`, `rust/Cargo.lock`,
   `uv.lock`, and the changelog. Build Rust locally and use
   `LINT_IT_NATIVE_BINARY=/absolute/path/to/lint-it-rust uv sync` for development.
2. Run the **Native release artifacts** workflow from the canonical repository’s `main` branch on the release commit. All five
   targets must build and pass native/Python packaging smoke tests. Download the
   artifacts; Linux executables are statically linked musl builds.
3. Run `make release-prepare RUN_ID=<run-id>` to generate `native-artifacts.json`
   with the version and SHA-256 of each executable. The helper requires a successful
   completed `workflow_dispatch` native run from this repository’s `main`,
   with its source commit still an ancestor of canonical `main` and unchanged committed
   build inputs. Fork, pull-request, other-branch, dirty-input and divergent runs fail.
   Do not rebuild Rust after this step without replacing the manifest and artifacts.
   Regenerate `THIRD_PARTY_NOTICES.txt` with `uv run python scripts/generate_notices.py`
   when changing dependencies or the Rust toolchain, updating pinned runtime notices
   when needed.
4. Build each wheel from the final checkout using `LINT_IT_NATIVE_BINARY` and
   `LINT_IT_NATIVE_TARGET` for the matching downloaded executable. Build the sdist
   from that same checkout. Verify metadata, embedded executables, notices, and
   clean wheel/sdist installations. Run the complete differential suite and CI.
5. Commit the checksum manifest and metadata, merge, and tag `v<VERSION>`. The
   Release workflow runs CI on that commit, verifies the pinned workflow inputs
   and binary checksums, then builds final wheels and the sdist without rebuilding
   Rust. It publishes all five executables, wheels, sdist, `SHA256SUMS`, and a
   hashed `index.html` through a draft release. Existing assets are never overwritten.
   A manual dispatch stages artifacts only.
   The manifest pins executable bytes; retain these assets for pinned Git clients.
6. Verify a fresh Git-tag install and the pre-commit hook after publication, without
   `LINT_IT_NATIVE_BINARY` or a Rust toolchain in the client's environment.

`LINT_IT_NATIVE_BINARY` is an explicit development/release-build override; normal
client builds always use the version-pinned, checksum-verified release assets.
`LINT_IT_NATIVE_TARGET` supports packaging a prebuilt executable for another host;
it must match the supplied binary. Release builders are responsible for verifying
that pairing for macOS/Windows. Linux wheel builds inspect ELF bytes directly,
including when cross-packaging ARM on x86: executable ELF64 architecture must match,
with an executable entry point and no interpreter or shared-library dependencies.
Malformed ELF headers/segments fail closed. Static PIE relocations are permitted.
Never publish a locally built Nix/glibc binary as the portable Linux musl artifact.
Editable installs keep generic host Linux tags so dynamic development binaries work.

The same verified static musl executable works on glibc and musl distributions;
there is no libc runtime dependency. Linux wheels advertise both
`manylinux_2_28_ARCH` and `musllinux_1_2_ARCH` in a compressed filename, with separate
WHEEL metadata tags. These conservative platform baselines cover glibc 2.28+ and
musl 1.2+ Python installations. The
[platform tag specification](https://packaging.python.org/en/latest/specifications/platform-compatibility-tags/)
explicitly covers native executables invoked as subprocesses; the
[manylinux compatibility promise](https://peps.python.org/pep-0600/) applies to the
complete wheel. Native CI tests execute the binary and installed wheel on each
architecture; static ELF verification protects the absence of runtime dependencies.


## Build engine and local commands

All Python environments and builds use uv 0.12.4. `uv_build==0.12.4` owns
metadata, Python file selection, and sdists. Since uv_build cannot create native
wheels itself, the in-tree `build_backend` PEP 517/660 adapter adds the verified
executable, platform tag, executable mode and wheel RECORD. Hatch is no longer
required. Git/sdist installs retain checksum-verified binary downloads; wheel
installs remain offline. Editable installs use the same verified binary.

`make dev`, `make check`, `make test`, `make build`, and `make release-check`
provide the shared package commands. `release-check` installs both distributions
in separate clean environments outside the checkout and exercises the CLI.
For an unreleased version, first compile Rust and export `LINT_IT_NATIVE_BINARY`
to that executable; do not publish this local build as a portable artifact.

A failed publication can leave a draft. Inspect its assets and checksums before
manually finishing it; a rerun deliberately fails instead of replacing assets.
Native workflow artifacts must still be retained when the tag workflow stages
its release. Published assets remain available independently of Actions retention.

## PyPI publication security

The `leesh` PyPI trusted publisher is bound to `denismaciel/lint-it`, `release.yml`, and
GitHub environment `pypi`. That environment accepts only `v*` tags and requires
Denis's approval; administrative approval bypass is disabled. Repository rules
restrict tag creation to admins and prohibit updating or deleting release tags.
Default-branch changes require reviewed pull requests for non-admin contributors;
CODEOWNERS assigns review to Denis. Published GitHub releases are immutable.

Release CI must pass before publication. GitHub publication and its subsequent
Git-install smoke test run in separate jobs: the smoke test has read-only access.
The PyPI job executes no project code, uses only the current run's verified
wheel/sdist artifacts, and receives short-lived OIDC credentials. The official
PyPA upload action is pinned to a commit and produces PyPI attestations. No
long-lived PyPI token is stored. PR jobs have no publishing credentials.

After a failed PyPI upload, inspect which files reached PyPI before retrying.
Never replace a published file, disable verification, or overwrite a release.
If only PyPI failed, rerun that job rather than the successful GitHub publisher.
Account-owner compromise remains outside these controls: keep GitHub and PyPI
2FA and recovery methods secure, and review the commit before approving a release.
