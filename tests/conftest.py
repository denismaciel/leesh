from __future__ import annotations

import json
import os
import re
import subprocess
import textwrap
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

import pytest

REPOSITORY = Path(__file__).resolve().parents[1]


def binary_path() -> Path:
    for env in ("LEESH_RUST_BINARY", "LEESH_NATIVE_BINARY"):
        value = os.environ.get(env)
        if value:
            return Path(value).resolve()
    release = REPOSITORY / "rust/target/release/leesh-rust"
    if release.is_file():
        return release
    bundled = REPOSITORY / "src/leesh/_bin/leesh-rust"
    if bundled.is_file():
        return bundled
    return release


class ManifestError(Exception):
    """Raised when `leesh` rejects the workspace manifest (exit status 2)."""


@dataclass(frozen=True)
class Finding:
    filename: str
    line: int
    column: int
    code: str
    message: str
    target: str | None = None


@dataclass(frozen=True)
class Classification:
    name: str
    layer: str
    origin: str
    glob: str
    raw: str


_CLASSIFY_RE = re.compile(
    r"^(?P<path>.+?): (?P<name>[a-z0-9_-]+)(?: \(a (?P<base>[a-z0-9_-]+)\))? "
    r"\((?P<origin>override|.+? layout), (?P<glob>.+)\)$"
)


def run_cli(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary_path()), *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )


@dataclass(frozen=True)
class Project:
    """A throwaway workspace: one root package named ``example`` unless told otherwise."""

    root: Path

    def write(self, files: Mapping[str, str]) -> Project:
        for relative, source in files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(source), encoding="utf-8")
        return self

    def manifest(self, text: str) -> Project:
        return self.write({"leesh.toml": text})

    def package(self, path: str, name: str) -> Project:
        return self.write({f"{path}/pyproject.toml": f'[project]\nname = "{name}"\n'})

    def run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return run_cli(*args, "--root", str(self.root))

    def load(self) -> dict[str, object]:
        process = run_cli("check", "--root", str(self.root), "--format", "json", "--jobs", "1")
        if process.returncode == 2:
            if process.stdout.strip():
                payload = json.loads(process.stdout)
                if payload.get("findings"):
                    raise ManifestError(payload["findings"][0]["message"])
            raise ManifestError(process.stderr.strip() or "manifest error")
        return json.loads(process.stdout)

    def classify(self, path: str) -> Classification | None:
        process = run_cli("classify", str(self.root / path), "--root", str(self.root))
        if process.returncode == 2:
            raise ManifestError(process.stderr.strip())
        line = process.stdout.strip().splitlines()[-1]
        if ": unclassified (" in line or ": outside every package" in line:
            return None
        match = _CLASSIFY_RE.match(line)
        assert match is not None, f"unexpected classify output: {line!r}"
        name = match.group("name")
        base = match.group("base") or name
        origin = "override" if match.group("origin") == "override" else "layout"
        return Classification(
            name=name,
            layer=base,
            origin=origin,
            glob=match.group("glob"),
            raw=line,
        )

    def findings(self, *paths: str) -> list[Finding]:
        args = ["check", "--root", str(self.root), "--format", "json", "--jobs", "1"]
        args.extend(str(self.root / path) for path in paths)
        process = run_cli(*args)
        if process.returncode == 2:
            if process.stdout.strip():
                payload = json.loads(process.stdout)
                if payload.get("findings"):
                    raise ManifestError(payload["findings"][0]["message"])
            raise ManifestError(process.stderr.strip() or "manifest error")
        payload = json.loads(process.stdout)
        return [
            Finding(
                filename=item["filename"],
                line=item["line"],
                column=item["column"],
                code=item["code"],
                message=item["message"],
                target=item.get("target"),
            )
            for item in payload["findings"]
        ]

    def codes(self, path: str) -> list[str]:
        return [item.code for item in self.findings(path) if item.filename == path]

    def lint(self, path: str, source: str) -> list[str]:
        """Codes reported for ``path`` after writing ``source`` to it."""
        return self.write({path: source}).codes(path)


@pytest.fixture
def project(tmp_path: Path) -> Project:
    return Project(tmp_path).write({"pyproject.toml": '[project]\nname = "example"\n'})
