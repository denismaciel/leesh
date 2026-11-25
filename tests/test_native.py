"""Stateful end-to-end CLI checks against the native Rust executable."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest
from conftest import binary_path

BINARY = binary_path()
pytestmark = pytest.mark.skipif(not BINARY.is_file(), reason="build the native executable first")


@pytest.mark.parametrize("output", ["text", "json", "github"])
def test_native_stateful_cli_workflow(tmp_path: Path, output: str) -> None:
    (tmp_path / "pyproject.toml").write_text('[project]\nname = "example"\n')
    source = tmp_path / "src/example/core.py"
    source.parent.mkdir(parents=True)
    source.write_text("import httpx\nfrom .adapters.store import VALUE\nVALUE = []\n")
    adapter = tmp_path / "src/example/adapters/store.py"
    adapter.parent.mkdir()
    adapter.write_text("VALUE = 1\n")

    def run(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(BINARY), *args],
            cwd=tmp_path,
            text=True,
            capture_output=True,
            check=False,
        )

    assert run("rules").returncode == 0
    assert run("rules", "--format", "markdown").stdout.startswith("# Rules\n")
    assert run("init", "--layout", "flat", "--kind", "library").returncode == 0
    assert run("init", "--layout", "flat").returncode == 2
    with (tmp_path / "leesh.toml").open("a") as manifest:
        manifest.write('baseline = "baseline.json"\n')

    initial = run("check", "--format", output, "--jobs", "1")
    assert initial.returncode == 1
    assert run("baseline", "--jobs", "1").returncode == 0
    baseline_data = json.loads((tmp_path / "baseline.json").read_text())
    assert baseline_data["version"] == 1
    assert "src/example/core.py" in baseline_data["findings"]

    assert run("check", "--format", output, "--jobs", "1").returncode == 0
    assert run("check", "--format", output, "--no-baseline", "--jobs", "1").returncode == 1

    source.write_text("VALUE = 1\n")
    stale = run("check", "--format", output, "--jobs", "1")
    assert stale.returncode == 1
    assert "LIT003" in stale.stdout

    assert run("baseline", "--prune", "--jobs", "1").returncode == 0
    assert json.loads((tmp_path / "baseline.json").read_text())["findings"] == {}
    assert run("check", "--format", output, "--jobs", "1").returncode == 0
