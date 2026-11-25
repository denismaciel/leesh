from __future__ import annotations

import pytest
from conftest import ManifestError, Project


def layer_of(project: Project, path: str) -> str | None:
    project.write({path: ""})
    classification = project.classify(path)
    return None if classification is None else classification.layer


@pytest.mark.parametrize(
    ("path", "layer"),
    [
        ("src/example/__init__.py", "marker"),
        ("src/example/__main__.py", "boundary"),
        ("src/example/app/main.py", "boundary"),
        ("src/example/app/config.py", "boundary"),
        ("src/example/app/routers/orders.py", "router"),
        ("src/example/app/router/orders.py", "router"),
        ("src/example/app/api/orders_router.py", "router"),
        ("src/example/app/router.py", "router"),
        ("src/example/app/dependencies.py", "transaction"),
        ("src/example/domains/orders/core.py", "core"),
        ("src/example/domains/orders/model.py", "core"),
        ("src/example/domains/orders/approval/policy.py", "core"),
        ("src/example/domains/orders/ports.py", "port"),
        ("src/example/domains/orders/service.py", "shell"),
        ("src/example/domains/orders/boundary.py", "boundary"),
        ("src/example/domains/orders/persistence/repository.py", "adapter"),
        ("src/example/domains/orders/core/pricing.py", "core"),
        ("src/example/domains/orders/core/renewal/terms.py", "core"),
        ("src/example/domains/orders/core/ports.py", "core"),
        ("src/example/domains/orders/model/order.py", "core"),
        ("src/example/domains/orders/approval/policy/limits.py", "core"),
        ("src/example/domains/orders/event/placed.py", "core"),
        ("src/example/domains/orders/ports/payments.py", "port"),
        ("src/example/domains/orders/service/checkout.py", "shell"),
        ("src/example/domains/orders/boundary/schemas.py", "boundary"),
        ("src/example/domains/orders/core/persistence/tables.py", "adapter"),
        ("src/example/domains/policy/service.py", "shell"),
        ("src/example/domains/event/sourcing.py", None),
        ("src/example/projections/summary/model.py", "core"),
        ("src/example/projections/summary/model/totals.py", "core"),
        ("src/example/projections/summary/service/refresh.py", "shell"),
        ("src/example/shared/core/money.py", "core"),
        ("src/example/shared/ports/clock.py", "port"),
        ("src/example/projections/summary/persistence/queries.py", "adapter"),
        ("src/example/shared/model.py", "core"),
        ("src/example/db/base.py", "adapter"),
        ("src/example/db/orders/repo.py", "adapter"),
        ("src/example/db/unit_of_work.py", "transaction"),
        ("src/example/db/unit_of_work/sql.py", "transaction"),
        ("src/example/integrations/payments.py", "adapter"),
        ("src/example/domains/orders/fakes.py", "test"),
        ("src/example/shared/fakes.py", "test"),
        ("src/example/db/fakes.py", "test"),
        ("src/example/integrations/gmail/fake.py", "test"),
        ("src/example/integrations/fakes/gmail.py", "test"),
        ("src/example/integrations/unit_of_work/sql.py", "transaction"),
        ("tests/domains/orders/test_core.py", "test"),
        ("conftest.py", "test"),
        ("scripts/backfill.py", "boundary"),
        ("examples/usage.py", "boundary"),
        ("migrations/versions/0001.py", "transaction"),
        ("src/example/orders.py", None),
        ("src/example/domains/orders/repository.py", None),
        ("setup.py", None),
    ],
)
def test_default_manifest_uses_the_domains_layout(
    project: Project, path: str, layer: str | None
) -> None:
    assert layer_of(project, path) == layer


@pytest.mark.parametrize(
    ("layout", "path", "layer"),
    [
        ("hexagonal", "src/example/rules.py", "core"),
        ("hexagonal", "src/example/domain/orders/ports.py", "port"),
        ("hexagonal", "src/example/application/orders.py", "shell"),
        ("hexagonal", "src/example/application/orders_errors.py", "core"),
        ("hexagonal", "src/example/adapters/sql/database.py", "transaction"),
        ("hexagonal", "src/example/web/routes.py", "boundary"),
        ("hexagonal", "src/example/development.py", "boundary"),
        ("flat", "src/example/anything/at/all.py", "shell"),
        ("flat", "src/example/model.py", "core"),
        ("flat", "src/example/ports.py", "port"),
        ("flat", "src/example/core/pricing.py", "core"),
        ("flat", "src/example/model/order.py", "core"),
        ("flat", "src/example/ports/clock.py", "port"),
        ("flat", "src/example/cli/commands.py", "boundary"),
        ("flat", "src/example/adapters/core/http.py", "adapter"),
        ("flat", "src/example/adapters/http.py", "adapter"),
        ("flat", "src/example/cli.py", "boundary"),
        ("flat", "examples/usage.py", "boundary"),
        ("boundary", "src/example/schemas.py", "boundary"),
        ("test-support", "src/example/fakes.py", "test"),
    ],
)
def test_built_in_layouts(project: Project, layout: str, path: str, layer: str) -> None:
    project.manifest(f'[workspace]\nlayout = "{layout}"\n')
    assert layer_of(project, path) == layer


def test_packages_select_kind_and_layout(project: Project) -> None:
    project.package("packages/sdk", "sdk").package("packages/tools", "tools").manifest(
        """
        [workspace]
        kind = "server"

        [[packages]]
        path = "packages/sdk"
        kind = "client"
        layout = "boundary"

        [[packages]]
        path = "packages/tools"
        kind = "library"
        layout = "flat"
        """
    )
    project.write(
        {
            "src/example/domains/orders/core.py": "VALUE = 1\n",
            "packages/sdk/src/sdk/client.py": "from example.domains.orders.core import VALUE\n",
            "packages/tools/src/tools/helper.py": "VALUE = 1\n",
        }
    )
    sdk = project.classify("packages/sdk/src/sdk/client.py")
    tools = project.classify("packages/tools/src/tools/helper.py")
    assert sdk is not None and sdk.layer == "boundary"
    assert tools is not None and tools.layer == "shell"
    assert project.codes("packages/sdk/src/sdk/client.py") == ["LIT103"]


def test_nested_packages_own_their_files(project: Project) -> None:
    project.package("packages/support", "support").write(
        {"packages/support/src/support/domains/orders/core.py": "VALUE = 1\n"}
    )
    classification = project.classify("packages/support/src/support/domains/orders/core.py")

    assert classification is not None
    assert classification.layer == "core"
    assert classification.glob == "src/*/domains/**/core.py"


def test_custom_layout_extends_a_built_in(project: Project) -> None:
    project.manifest(
        """
        [workspace]
        layout = "orders"

        [layouts.orders]
        extends = "hexagonal"
        layers = [{ glob = "src/*/clock.py", layer = "adapter" }]
        """
    )
    assert layer_of(project, "src/example/clock.py") == "adapter"
    assert layer_of(project, "src/example/web/app.py") == "boundary"
    assert layer_of(project, "tests/test_clock.py") == "test"


def test_custom_layout_without_base_only_knows_its_globs(project: Project) -> None:
    project.manifest(
        """
        [workspace]
        layout = "mine"

        [layouts.mine]
        layers = [{ glob = "src/*/decisions.py", layer = "core" }]
        """
    )
    assert layer_of(project, "src/example/decisions.py") == "core"
    assert layer_of(project, "src/example/domains/orders/core.py") is None


def test_classification_records_the_matching_pattern(project: Project) -> None:
    project.write({"src/example/domains/orders/core.py": ""})
    classification = project.classify("src/example/domains/orders/core.py")

    assert classification is not None
    assert classification.origin == "layout"
    assert classification.glob == "src/*/domains/**/core.py"


OVERRIDE = """
[[overrides]]
glob = "{glob}"
layer = "{layer}"
reason = "Legacy module waiting for the orders split"
issue = "EX-1"
"""


def test_override_reclassifies_one_file(project: Project) -> None:
    project.manifest(
        "[workspace]\noverride_budget = 1\n"
        + OVERRIDE.format(glob="src/example/orders.py", layer="shell")
    )
    project.write({"src/example/orders.py": ""})
    classification = project.classify("src/example/orders.py")

    assert classification is not None
    assert (classification.layer, classification.origin) == ("shell", "override")
    assert project.findings() == []


def test_override_budget_counts_files(project: Project) -> None:
    project.manifest(
        "[workspace]\noverride_budget = 1\n"
        + OVERRIDE.format(glob="src/example/legacy/*.py", layer="shell")
    )
    project.write({"src/example/legacy/first.py": "", "src/example/legacy/second.py": ""})
    [problem] = project.findings()

    assert (problem.filename, problem.line, problem.code) == ("leesh.toml", 1, "LIT003")
    assert "override budget exceeded: 2 files use overrides, budget is 1" in problem.message


def test_stale_and_overlapping_overrides_are_findings(project: Project) -> None:
    project.manifest(
        "[workspace]\noverride_budget = 5\n"
        + OVERRIDE.format(glob="src/example/orders.py", layer="shell")
        + OVERRIDE.format(glob="src/example/orders.py", layer="boundary")
        + OVERRIDE.format(glob="src/example/missing.py", layer="boundary")
    )
    project.write({"src/example/orders.py": ""})
    messages = [item.message for item in project.findings() if item.code == "LIT003"]

    assert project.classify("src/example/orders.py") is None
    assert any("overlapping layout overrides for src/example/orders.py" in m for m in messages)
    assert any("stale layout override in .: src/example/missing.py" in m for m in messages)
    assert project.codes("src/example/orders.py") == ["LIT002"]


def test_dependency_overrides_are_validated(project: Project) -> None:
    project.manifest(
        """
        [[dependency_overrides]]
        importer = "src/example/domains/orders/core.py"
        imported = "src/example/app/missing.py"
        reason = "Temporary inversion"
        issue = "EX-1"
        """
    )
    project.write({"src/example/domains/orders/core.py": ""})
    messages = [item.message for item in project.findings()]

    assert any("stale dependency override imported" in message for message in messages)
    assert any("dependency override budget exceeded" in message for message in messages)


@pytest.mark.parametrize(
    ("manifest", "message"),
    [
        ('version = "2"', "unknown manifest field: version"),
        ("[workspace]\noverride_buget = 2", "unknown workspace field: override_buget"),
        ('[workspace]\nkind = "service"', "workspace.kind must be one of: server, client"),
        ('[workspace]\nlayout = "web"', "workspace.layout must be one of: domains, hexagonal"),
        ('[[packages]]\npath = "packages/missing"', "package has no pyproject.toml"),
        ("[[packages]]\npath = '../elsewhere'", "packages.path must be a relative path"),
        ("[layouts.flat]\nlayers = []", "cannot redefine a built-in"),
        ("[layouts.mine]\nlayers = []", "layouts.mine.layers must be a non-empty array"),
        ('[layouts.mine]\nextends = "web"\nlayers = [1]', "layouts.mine.extends must be one of"),
        (
            "[layouts.mine]\nlayers = [{ glob = 'src/a.py', layer = 'functional-core' }]",
            "layouts.mine.layers.layer must be one of: core, port, shell",
        ),
        (
            "[layouts.mine]\nlayers = [{ glob = 'src/a.txt', layer = 'core' }]",
            "must select Python files",
        ),
        (
            OVERRIDE.format(glob="src/**/*.py", layer="shell"),
            "override.glob is too broad",
        ),
        (
            OVERRIDE.format(glob="src/example/a.py", layer="shell").replace("EX-1", ""),
            "overrides.issue must be a non-empty string",
        ),
        ('[workspace]\neffectful_types = ["x.*"]', "workspace.effectful_types must contain"),
        ("[workspace]\nclock_modules = [1]", "workspace.clock_modules must be an array"),
        ('[layer_dependencies]\ncore = ["kernel"]', "layer_dependencies.core must be one of"),
        ('[forbidden_imports]\nwidget = ["x"]', "forbidden_imports key must be one of"),
        (
            '[extend_forbidden_imports]\nwidget = ["x"]',
            "extend_forbidden_imports key must be one of",
        ),
        ("[conventions]\nprefixes = []", "unknown conventions field: prefixes"),
        ("[workspace\n", "Expected ']'"),
    ],
)
def test_invalid_manifests_name_the_problem(project: Project, manifest: str, message: str) -> None:
    project.manifest(manifest)
    with pytest.raises(ManifestError, match=message.replace("[", r"\[").replace("*", r"\*")):
        project.load()


def test_virtual_environments_are_skipped_whatever_their_name(project: Project) -> None:
    site = ".venv315/lib/python3.15/site-packages/dep"
    project.write(
        {
            "src/example/core.py": "",
            ".venv315/pyvenv.cfg": "home = /usr/bin\n",
            f"{site}/__init__.py": "",
            f"{site}/pyproject.toml": "[project]\nname = 'dep'\n",
        }
    )
    payload = project.load()

    assert payload["summary"] == {
        "findings": 1,
        "packages": 1,
        "files": 1,
        "baselined": 0,
        "ignored": 0,
    }


def test_workspace_without_projects_is_a_manifest_error(tmp_path) -> None:
    with pytest.raises(ManifestError, match=r"no pyproject\.toml"):
        Project(tmp_path).load()


def test_policy_sections_replace_built_in_entries(project: Project) -> None:
    project.manifest(
        """
        [layer_dependencies]
        shell = "*"
        core = ["core"]

        [forbidden_imports]
        router = []
        shell = ["boto3"]

        [conventions]
        predicate_prefixes = ["is_"]
        outcome_suffixes = ["Missing"]
        """
    )
    project.write({"src/example/domains/orders/persistence/repository.py": "VALUE = 1\n"})
    assert (
        project.lint(
            "src/example/domains/orders/service.py",
            "from example.domains.orders.persistence.repository import VALUE\n",
        )
        == []
    )
    assert project.lint("src/example/app/routers/orders.py", "import sqlalchemy\n") == []
    assert project.lint("src/example/domains/orders/service.py", "import boto3\n") == ["LIT102"]
    assert project.lint("src/example/domains/orders/core.py", "import httpx\n") == ["LIT102"]
    assert project.lint(
        "src/example/domains/orders/core.py", "def has_flag() -> bool:\n    return True\n"
    ) == ["LIT401"]
    assert project.lint(
        "src/example/domains/orders/core.py", "def f() -> None:\n    raise OrderMissing()\n"
    ) == ["LIT402"]


def test_extend_forbidden_imports_adds_to_every_named_layer(project: Project) -> None:
    project.manifest(
        """
        [forbidden_imports]
        shell = ["boto3"]

        [extend_forbidden_imports]
        "*" = ["legacy"]
        shell = ["redis"]
        """
    )
    assert project.lint("src/example/domains/orders/service.py", "import boto3\n") == ["LIT102"]
    assert project.lint("src/example/domains/orders/service.py", "import legacy\n") == ["LIT102"]
    assert project.lint("src/example/domains/orders/service.py", "import redis\n") == ["LIT102"]
    assert project.lint("src/example/domains/orders/core.py", "import httpx\n") == ["LIT102"]
    assert project.lint("src/example/domains/orders/core.py", "import legacy\n") == ["LIT102"]
    assert project.lint("src/example/app/routers/orders.py", "import legacy\n") == ["LIT102"]
