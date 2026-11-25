from __future__ import annotations

import json

import pytest
from conftest import ManifestError, Project

CORE = "src/example/domains/orders/core.py"
SERVICE = "src/example/domains/orders/service.py"
REPOSITORY = "src/example/domains/orders/persistence/repository.py"
TEST = "tests/test_orders.py"
BASELINE = '[workspace]\nbaseline = "leesh-baseline.json"\n'


def recorded(project: Project) -> dict[str, object]:
    return json.loads((project.root / "leesh-baseline.json").read_text(encoding="utf-8"))


def test_ignore_by_layer_drops_only_that_rule(project: Project) -> None:
    project.manifest(
        """
        [[ignore]]
        rules = ["mocking"]
        layers = ["test"]
        reason = "Tests move off unittest.mock separately"
        """
    )
    project.write({TEST: "from unittest import mock\n", CORE: "import httpx\n"})
    result = project.findings()

    assert [item.code for item in result] == ["LIT102"]


def test_ignore_by_path_glob(project: Project) -> None:
    project.manifest(
        """
        [[ignore]]
        rules = ["LIT102"]
        paths = ["src/example/domains/legacy/**"]
        reason = "Legacy domain is being rewritten"
        """
    )
    project.write({"src/example/domains/legacy/core.py": "import httpx\n", CORE: "import httpx\n"})

    assert [item.filename for item in project.findings()] == [CORE]


def test_unused_and_invalid_ignores_are_manifest_findings(project: Project) -> None:
    project.manifest(
        """
        [[ignore]]
        rules = ["LIT102"]
        paths = ["src/example/gone/**"]
        reason = "Nothing left here"

        [[ignore]]
        rules = ["no-such-rule", "LIT003"]
        reason = "Typos"
        """
    )
    project.write({CORE: ""})

    assert [item.message for item in project.findings()] == [
        "LIT003 cannot be ignored",
        "ignore names unknown rule no-such-rule",
        "ignore suppresses nothing: LIT102 in src/example/gone/**",
    ]


def test_unused_ignores_are_not_reported_for_a_path_selection(project: Project) -> None:
    project.manifest('[[ignore]]\nrules = ["LIT102"]\nreason = "Later"\n')
    project.write({CORE: ""})

    assert project.findings(CORE) == []


@pytest.mark.parametrize(
    ("manifest", "message"),
    [
        ('[[ignore]]\nrules = []\nreason = "x"', "ignore.rules must name at least one rule"),
        ('[[ignore]]\nrules = ["LIT102"]', "ignore.reason must be a non-empty string"),
        (
            '[[ignore]]\nrules = ["LIT102"]\nlayers = ["web"]\nreason = "x"',
            "ignore.layers must be one of",
        ),
        ('[[ignore]]\nrules = ["LIT102"]\nwhy = "x"', "unknown ignore field: why"),
    ],
)
def test_invalid_ignores_name_the_problem(project: Project, manifest: str, message: str) -> None:
    project.manifest(manifest)
    with pytest.raises(ManifestError, match=message):
        project.load()


def test_baseline_records_findings_without_lines(project: Project) -> None:
    project.manifest(BASELINE).write({CORE: "import httpx\nimport httpx as client\n"})

    result = project.run("baseline")
    assert result.returncode == 0
    assert recorded(project) == {
        "version": 1,
        "findings": {
            CORE: [{"code": "LIT102", "message": "core modules cannot import httpx", "count": 2}]
        },
    }
    assert result.stdout == "wrote leesh-baseline.json: 2 findings in 1 files (was 0)\n"


def test_check_reports_only_findings_beyond_the_baseline(project: Project) -> None:
    project.manifest(BASELINE).write({CORE: "import httpx\n"})
    project.run("baseline")
    project.write({CORE: "\n\nimport httpx\n", SERVICE: "import fastapi\n"})

    checked = project.run("check")
    assert checked.returncode == 1
    assert checked.stdout.splitlines() == [
        f"{SERVICE}:1:1: LIT102 shell modules cannot import fastapi",
        "1 finding in 1 file (2 files in 1 package checked; 1 baselined)",
    ]
    unfiltered = project.run("check", "--no-baseline")
    assert unfiltered.returncode == 1
    assert "2 findings" in unfiltered.stdout


def test_fixed_findings_make_the_baseline_stale_until_pruned(project: Project) -> None:
    project.manifest(BASELINE).write({CORE: "import httpx\n", SERVICE: "import fastapi\n"})
    project.run("baseline")
    project.write({CORE: "", "src/example/domains/orders/model.py": "import redis\n"})

    checked = project.run("check")
    assert checked.returncode == 1
    assert checked.stdout.splitlines()[:2] == [
        "leesh-baseline.json:1:1: LIT003 1 baseline entry no longer matches a finding; "
        "run `leesh baseline --prune` to shrink the baseline",
        "src/example/domains/orders/model.py:1:1: LIT102 core modules cannot import redis",
    ]
    assert project.run("baseline", "--prune").returncode == 0
    assert recorded(project) == {
        "version": 1,
        "findings": {
            SERVICE: [
                {
                    "code": "LIT102",
                    "message": "shell modules cannot import fastapi",
                    "count": 1,
                }
            ]
        },
    }


def test_baseline_needs_a_configured_path(project: Project) -> None:
    project.write({CORE: ""})

    result = project.run("baseline")
    assert result.returncode == 2
    assert "set workspace.baseline" in result.stderr


@pytest.mark.parametrize(
    ("text", "message"),
    [
        ("[]", "baseline must be an object with version 1"),
        ('{"version": 1, "findings": []}', "keyed by file"),
        ('{"version": 1, "findings": {"a.py": [{"code": "LIT1"}]}}', "needs code, message"),
        ("{", "baseline is not valid JSON"),
    ],
)
def test_corrupt_baselines_are_manifest_errors(project: Project, text: str, message: str) -> None:
    project.manifest(BASELINE).write({"leesh-baseline.json": text})
    with pytest.raises(ManifestError, match=message):
        project.load()


def test_report_ranks_rules_edges_targets_and_areas(project: Project) -> None:
    project.manifest(BASELINE).write(
        {
            REPOSITORY: "VALUE = 1\n",
            SERVICE: "from example.domains.orders.persistence.repository import VALUE\n",
            CORE: "import httpx\n",
        }
    )
    project.run("baseline")
    project.write(
        {
            "src/example/projections/summary/service.py": (
                "from example.domains.orders.persistence.repository import VALUE\n"
            )
        }
    )

    result = project.run("report", "--depth", "4", "--format", "json")
    assert result.returncode == 0
    stats = json.loads(result.stdout)

    assert (stats["findings"], stats["new"], stats["baselined"]) == (3, 1, 2)
    assert stats["rules"][0] == {
        "code": "LIT101",
        "name": "layer-dependency",
        "findings": 2,
        "new": 1,
        "files": 2,
    }
    assert stats["edges"] == [{"label": "shell -> adapter", "findings": 2}]
    assert stats["targets"][0] == {
        "label": "LIT101 example.domains.orders.persistence.repository",
        "findings": 2,
    }
    assert {"label": "src/example/domains/orders", "findings": 2} in stats["areas"]


def test_report_text_lists_each_section(project: Project) -> None:
    project.write({CORE: "import httpx\n"})

    result = project.run("report")
    assert result.returncode == 0
    output = result.stdout
    assert output.startswith("1 finding: 1 new, 0 baselined (0 ignored)\n\nBy rule\n  LIT102")
    assert "Top targets\n       1  LIT102 httpx" in output
    assert "Top areas\n       1  src/example/domains" in output


def test_report_can_focus_on_one_rule(project: Project) -> None:
    project.write(
        {
            CORE: "import httpx\n",
            "src/example/domains/orders/model.py": "class Base(dict): ...\nclass Mid(Base): ...\n"
            "class Leaf(Mid): ...\n",
        }
    )

    result = project.run("report", "--rule", "restricted-subclassing", "--format", "json")
    assert result.returncode == 0
    stats = json.loads(result.stdout)
    assert [item["code"] for item in stats["rules"]] == ["LIT403"]
    assert stats["targets"] == [{"label": "LIT403 example.domains.orders.model.Mid", "findings": 1}]
    assert project.run("report", "--rule", "LIT999").returncode == 2
