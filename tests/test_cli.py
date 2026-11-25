from __future__ import annotations

import json
import subprocess
import sys

import pytest
from conftest import REPOSITORY, Project, run_cli

CORE = "src/example/domains/orders/core.py"


def test_clean_workspace_exits_zero(project: Project) -> None:
    project.write({CORE: "VALUE = 1\n"})

    result = project.run("check")
    assert result.returncode == 0
    assert result.stdout == "No findings (1 file in 1 package checked)\n"


def test_findings_exit_one_with_relative_paths(project: Project) -> None:
    project.write({CORE: "import httpx\n", "src/example/orders.py": ""})

    result = project.run("check")
    assert result.returncode == 1
    assert result.stdout.splitlines() == [
        "src/example/domains/orders/core.py:1:1: LIT102 core modules cannot import httpx",
        "src/example/orders.py:1:1: LIT002 file matches no layer in its package layout "
        "(domains); move it, extend the layout, or add an override",
        "2 findings in 2 files (2 files in 1 package checked)",
    ]


def test_paths_limit_reported_findings(project: Project) -> None:
    project.write({CORE: "import httpx\n", "src/example/orders.py": ""})

    result = project.run("check", str(project.root / "src/example/domains"))
    assert result.returncode == 1
    assert "orders.py" not in result.stdout


def test_json_output_is_findings_and_summary(project: Project) -> None:
    project.write({CORE: "import httpx\n"})

    result = project.run("check", "--format", "json")
    assert result.returncode == 1
    assert json.loads(result.stdout) == {
        "findings": [
            {
                "filename": CORE,
                "line": 1,
                "column": 0,
                "code": "LIT102",
                "message": "core modules cannot import httpx",
                "target": "httpx",
            }
        ],
        "summary": {"findings": 1, "packages": 1, "files": 1, "baselined": 0, "ignored": 0},
    }


def test_github_output_escapes_annotations(project: Project) -> None:
    project.write({CORE: "import httpx, redis\n"})

    result = project.run("check", "--format", "github")
    assert result.returncode == 1
    assert result.stdout == (
        f"::error file={CORE},line=1,col=1,title=LIT102::core modules cannot import httpx, redis\n"
    )


def test_syntax_errors_are_findings(project: Project) -> None:
    project.write({CORE: "def broken(:\n", "src/example/domains/orders/model.py": "VALUE = 1\n"})

    result = project.run("check")
    assert result.returncode == 1
    assert result.stdout.splitlines()[0].startswith(f"{CORE}:1:12: LIT001 file cannot be parsed:")


@pytest.mark.parametrize("output", ["text", "json"])
def test_invalid_manifest_exits_two(project: Project, output: str) -> None:
    project.manifest('[workspace]\nlayout = "web"\n')

    result = project.run("check", "--format", output)
    assert result.returncode == 2
    if output == "json":
        [problem] = json.loads(result.stdout)["findings"]
        assert (problem["filename"], problem["code"]) == ("leesh.toml", "LIT003")
    else:
        assert result.stderr.startswith("leesh.toml:1:1: LIT003 workspace.layout must be one of")


def test_stale_manifest_entries_are_findings(project: Project) -> None:
    project.manifest(
        '[[overrides]]\nglob = "src/example/gone.py"\nlayer = "shell"\nreason = "x"\nissue = "y"\n'
    )

    result = project.run("check")
    assert result.returncode == 1
    assert "leesh.toml:1:1: LIT003 stale layout override" in result.stdout


def test_paths_outside_the_workspace_are_usage_errors(
    project: Project, tmp_path_factory: pytest.TempPathFactory
) -> None:
    elsewhere = tmp_path_factory.mktemp("elsewhere")
    assert project.run("check", str(elsewhere)).returncode == 2
    assert project.run("check", str(project.root / "missing")).returncode == 2


def test_classify_explains_layers(project: Project) -> None:
    project.package("packages/sdk", "sdk").manifest(
        "[workspace]\noverride_budget = 1\n"
        '[[overrides]]\nglob = "src/example/legacy.py"\nlayer = "shell"\n'
        'reason = "Old"\nissue = "EX-1"\n'
    )
    project.write(
        {
            CORE: "",
            "src/example/legacy.py": "",
            "src/example/orders.py": "",
            "packages/sdk/src/sdk/client.py": "",
            "tools/run.py": "",
        }
    )

    result = project.run("classify", str(project.root / "src"), str(project.root / "tools"))
    assert result.returncode == 0
    assert result.stdout.splitlines() == [
        f"{CORE}: core (domains layout, src/*/domains/**/core.py)",
        "src/example/legacy.py: shell (override, src/example/legacy.py)",
        "src/example/orders.py: unclassified (domains layout)",
        "tools/run.py: unclassified (domains layout)",
    ]


def test_classify_reports_files_outside_packages(project: Project) -> None:
    project.package("packages/sdk", "sdk").write({"loose/run.py": ""})
    (project.root / "pyproject.toml").unlink()

    result = project.run("classify", str(project.root / "loose/run.py"))
    assert result.returncode == 0
    assert result.stdout == "loose/run.py: outside every package\n"


def test_init_writes_a_starter_manifest(project: Project) -> None:
    assert project.run("init", "--layout", "flat", "--kind", "library").returncode == 0
    manifest = (project.root / "leesh.toml").read_text()

    assert 'kind = "library"\nlayout = "flat"\n' in manifest
    assert project.findings() == []
    assert project.run("init").returncode == 2


def test_explain_accepts_codes_and_names() -> None:
    by_code = run_cli("explain", "lit101")
    assert by_code.returncode == 0
    assert by_code.stdout.startswith("LIT101 layer-dependency\n\nA module imports")
    assert run_cli("explain", "mocking").returncode == 0
    assert run_cli("explain", "LIT999").returncode == 2


def test_rule_reference_is_up_to_date() -> None:
    reference = (REPOSITORY / "docs/rules.md").read_text()
    generated = run_cli("rules", "--format", "markdown")
    assert generated.returncode == 0
    assert reference == generated.stdout, (
        "regenerate with: uv run leesh rules --format markdown > docs/rules.md"
    )


def test_module_entry_point_runs_the_cli(project: Project) -> None:
    project.write({CORE: "import httpx\n"})
    process = subprocess.run(
        [sys.executable, "-m", "leesh", "check", "--format", "json"],
        cwd=project.root / "src",
        capture_output=True,
        text=True,
        check=False,
    )

    assert process.returncode == 1, process.stderr
    assert [item["code"] for item in json.loads(process.stdout)["findings"]] == ["LIT102"]


def test_leesh_passes_its_own_rules() -> None:
    result = run_cli("check", "--root", str(REPOSITORY))
    assert result.returncode == 0, result.stdout + result.stderr


def test_parallel_workers_report_the_serial_findings(project: Project) -> None:
    project.write(
        {f"src/example/domains/orders{index}/core.py": "import httpx\n" for index in range(8)}
    )
    serial = project.run("check", "--format", "json", "--jobs", "1")
    parallel = project.run("check", "--format", "json", "--jobs", "3")

    assert serial.returncode == 1
    assert parallel.returncode == 1
    assert json.loads(parallel.stdout) == json.loads(serial.stdout)
    assert len(json.loads(serial.stdout)["findings"]) == 8


def test_jobs_option_checks_in_process(project: Project) -> None:
    project.write({CORE: "import httpx\n"})

    assert project.run("check", "--jobs", "1").returncode == 1
