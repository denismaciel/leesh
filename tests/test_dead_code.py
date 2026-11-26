from __future__ import annotations

import json

import pytest
from conftest import ManifestError, Project

CORE = "src/example/domains/orders/core.py"
MODEL = "src/example/domains/orders/model.py"
PORTS = "src/example/domains/orders/ports.py"
SERVICE = "src/example/domains/orders/service.py"
ADAPTER = "src/example/domains/orders/persistence/repository.py"
ROUTER = "src/example/app/routers/orders.py"
MAIN = "src/example/app/main.py"
SCHEMA = "src/example/app/schemas.py"
UNIT_TEST = "tests/test_orders.py"


def _enable_dead_code(project: Project, extra: str = "") -> None:
    project.manifest(f"[workspace]\ndead_code = true\n{extra}")


def test_dead_code_is_opt_in_by_default(project: Project) -> None:
    project.write({CORE: "def unused(x: int) -> int:\n    return x\n"})
    assert project.codes(CORE) == []

    cli = project.run("check", "--dead-code", "--format", "json")
    assert cli.returncode == 1
    codes = [f["code"] for f in json.loads(cli.stdout)["findings"]]
    assert "LIT510" in codes


def test_lit501_forbids_opaque_dispatch_outside_boundary_and_test(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.service import run\n"
                "def start() -> object:\n"
                "    return run(object(), 'x')\n"
                "start()\n"
            ),
            SERVICE: (
                "import importlib\n"
                "def run(obj: object, name: str) -> object:\n"
                "    _ = importlib.import_module(name)\n"
                "    _ = obj.__dict__\n"
                "    return getattr(obj, name)\n"
                "def __getattr__(name: str) -> object:\n"
                "    return name\n"
            ),
        }
    )
    codes = project.codes(SERVICE)
    assert codes.count("LIT501") == 4

    # Literal getattr is allowed, and boundary/test modules are exempt from LIT501.
    project.write(
        {
            MAIN: (
                "import importlib\n"
                "from example.domains.orders.service import run\n"
                "def boot(name: str) -> object:\n"
                "    return importlib.import_module(name)\n"
                "run(boot('math'))\n"
            ),
            SERVICE: "def run(obj: object) -> object:\n    return getattr(obj, 'pi')\n",
        }
    )
    assert "LIT501" not in project.codes(MAIN)
    assert "LIT501" not in project.codes(SERVICE)


def test_lit502_forbids_star_imports_and_dynamic_all(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.core import *\n__all__ = ['a']\n__all__ += ['b']\n"
            ),
            CORE: "__all__ = [x for x in ['a']]\n",
        }
    )
    assert project.codes(MAIN).count("LIT502") == 2
    assert "LIT502" in project.codes(CORE)

    # Static literal __all__ is clean.
    project.write(
        {
            MAIN: (
                "from example.domains.orders.core import helper\n__all__ = ['helper']\nhelper()\n"
            ),
            CORE: "__all__ = ('helper',)\ndef helper() -> int:\n    return 1\n",
        }
    )
    assert "LIT502" not in project.codes(MAIN)
    assert "LIT502" not in project.codes(CORE)


def test_lit503_flags_untyped_receiver_in_shell_adapter_router(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: ("from example.domains.orders.service import run\nrun(object(), 1)\n"),
            SERVICE: (
                "def run(repo, count) -> object:\n    _ = count + 1\n    return repo.save()\n"
            ),
        }
    )
    findings = [f for f in project.findings(SERVICE) if f.code == "LIT503"]
    assert len(findings) == 1
    assert "'repo'" in findings[0].message


def test_lit510_and_lit511_two_pass_prod_vs_test_and_dead_islands(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: ("from example.domains.orders.core import live_fn\nlive_fn(1)\n"),
            CORE: (
                "def live_fn(x: int) -> int:\n"
                "    return x + 1\n"
                "def island_a(x: int) -> int:\n"
                "    return island_b(x)\n"
                "def island_b(x: int) -> int:\n"
                "    return island_a(x)\n"
                "def test_only_fn(x: int) -> int:\n"
                "    return x * 2\n"
            ),
            "src/example/domains/orders/policy.py": (
                "def only_tested(x: int) -> int:\n    return x\n"
            ),
            "src/example/domains/orders/event.py": (
                "def totally_dead(x: int) -> int:\n    return x\n"
            ),
            "tests/helpers_unused.py": "def never_used() -> int:\n    return 1\n",
            UNIT_TEST: (
                "from example.domains.orders.core import test_only_fn\n"
                "from example.domains.orders.policy import only_tested\n"
                "def test_stuff() -> None:\n"
                "    assert test_only_fn(2) == 4\n"
                "    assert only_tested(3) == 3\n"
            ),
        }
    )
    findings = project.findings()
    by_file = {(f.filename, f.code): f.message for f in findings if f.code.startswith("LIT5")}

    assert "only imported by tests" in by_file[("src/example/domains/orders/policy.py", "LIT510")]
    assert (
        "unreachable from production entrypoints"
        in by_file[("src/example/domains/orders/event.py", "LIT510")]
    )
    assert "unreachable from any test" in by_file[("tests/helpers_unused.py", "LIT510")]

    core_msgs = [f.message for f in findings if f.filename == CORE and f.code == "LIT511"]
    assert any("'island_a'" in m for m in core_msgs)
    assert any("'island_b'" in m for m in core_msgs)
    assert any("'test_only_fn' (only referenced by tests)" in m for m in core_msgs)
    assert not any("'live_fn'" in m for m in core_msgs)


def test_lit511_server_reexport_vs_library_root_init(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: "from example.domains.orders import used_fn\nused_fn(1)\n",
            "src/example/domains/orders/__init__.py": (
                "from example.domains.orders.core import unused_fn, used_fn\n"
                "__all__ = ['used_fn', 'unused_fn']\n"
            ),
            CORE: (
                "def used_fn(x: int) -> int:\n    return x\n"
                "def unused_fn(x: int) -> int:\n    return x\n"
            ),
        }
    )
    init_msgs = [
        f.message
        for f in project.findings("src/example/domains/orders/__init__.py")
        if f.code == "LIT511"
    ]
    assert any("unused re-export 'unused_fn'" in m for m in init_msgs)
    core_msgs = [f.message for f in project.findings(CORE) if f.code == "LIT511"]
    assert any("unused function 'unused_fn'" in m for m in core_msgs)

    # In a library package, root __init__.py __all__ keeps exported symbols alive.
    project.manifest('[workspace]\nkind = "library"\ndead_code = true\n')
    project.write(
        {
            "src/example/__init__.py": (
                "from example.domains.orders.core import used_fn\n__all__ = ['used_fn']\n"
            ),
            CORE: "def used_fn(x: int) -> int:\n    return x\n",
        }
    )
    assert [f for f in project.findings(CORE) if f.code.startswith("LIT5")] == []


def test_lit511_registration_vs_transparent_decorators(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: "from example.app.routers import orders\n_ = orders.router\n",
            ROUTER: (
                "from functools import lru_cache\n"
                "class Router:\n"
                "    def get(self, path: str):\n"
                "        return lambda fn: fn\n"
                "router = Router()\n"
                "@router.get('/orders')\n"
                "def list_orders() -> list[int]:\n"
                "    return []\n"
                "@lru_cache(maxsize=8)\n"
                "def dead_cached(x: int) -> int:\n"
                "    return x\n"
            ),
        }
    )
    router_511 = [f.message for f in project.findings(ROUTER) if f.code == "LIT511"]
    assert any("'dead_cached'" in m for m in router_511)
    assert not any("'list_orders'" in m for m in router_511)


def test_lit512_structural_protocol_dispatch_without_subclassing(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.persistence.repository import PgOrderRepo\n"
                "from example.domains.orders.service import OrderService\n"
                "svc = OrderService(PgOrderRepo())\n"
                "svc.place('o1')\n"
            ),
            PORTS: (
                "from typing import Protocol\n"
                "class OrderRepo(Protocol):\n"
                "    def save(self, order_id: str) -> None: ...\n"
                "    def archive(self, order_id: str) -> None: ...\n"
            ),
            SERVICE: (
                "from example.domains.orders.ports import OrderRepo\n"
                "class OrderService:\n"
                "    def __init__(self, repo: OrderRepo) -> None:\n"
                "        self._repo = repo\n"
                "    def place(self, order_id: str) -> None:\n"
                "        self._repo.save(order_id)\n"
            ),
            # Note: PgOrderRepo does NOT subclass OrderRepo!
            ADAPTER: (
                "class PgOrderRepo:\n"
                "    def save(self, order_id: str) -> None:\n"
                "        _ = order_id\n"
                "    def archive(self, order_id: str) -> None:\n"
                "        _ = order_id\n"
                "    def stale_helper(self) -> int:\n"
                "        return 42\n"
            ),
        }
    )
    port_512 = [f.message for f in project.findings(PORTS) if f.code == "LIT512"]
    adapter_512 = [f.message for f in project.findings(ADAPTER) if f.code == "LIT512"]

    # `save` is called via OrderRepo -> live on OrderRepo and PgOrderRepo (no subclassing).
    assert not any("save" in m for m in port_512)
    assert not any("save" in m for m in adapter_512)
    # `archive` is never called -> dead on BOTH OrderRepo and PgOrderRepo!
    assert any("OrderRepo.archive" in m for m in port_512)
    assert any("PgOrderRepo.archive" in m for m in adapter_512)
    # Extra uncalled helper on adapter is dead!
    assert any("PgOrderRepo.stale_helper" in m for m in adapter_512)


def test_lit512_chained_async_context_manager_protocol_dispatch(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.service import run\n"
                "from example.domains.orders.persistence.repository import PgUow\n"
                "run(PgUow())\n"
            ),
            PORTS: (
                "from contextlib import AbstractAsyncContextManager\n"
                "from typing import Protocol\n"
                "class Store(Protocol):\n"
                "    async def fetch(self, key: str) -> str: ...\n"
                "class Tx(Protocol):\n"
                "    @property\n"
                "    def store(self) -> Store: ...\n"
                "class Uow(Protocol):\n"
                "    def transaction(self) -> AbstractAsyncContextManager[Tx]: ...\n"
            ),
            SERVICE: (
                "from example.domains.orders.ports import Uow\n"
                "async def run(uow: Uow) -> str:\n"
                "    async with uow.transaction() as tx:\n"
                "        return await tx.store.fetch('k')\n"
            ),
            ADAPTER: (
                "from contextlib import asynccontextmanager\n"
                "from collections.abc import AsyncIterator\n"
                "class PgStore:\n"
                "    async def fetch(self, key: str) -> str:\n"
                "        return key\n"
                "class PgTx:\n"
                "    @property\n"
                "    def store(self) -> PgStore:\n"
                "        return PgStore()\n"
                "class PgUow:\n"
                "    @asynccontextmanager\n"
                "    async def transaction(self) -> AsyncIterator[PgTx]:\n"
                "        yield PgTx()\n"
            ),
        }
    )
    assert [f for f in project.findings(PORTS) if f.code == "LIT512"] == []
    assert [f for f in project.findings(ADAPTER) if f.code == "LIT512"] == []


def test_lit512_pydantic_exempt_vs_pure_core_dataclass_fields(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.app.schemas import WirePayload\n"
                "from example.domains.orders.model import InternalOrder, ExportedOrder\n"
                "from example.domains.orders.core import summarize\n"
                "p = WirePayload(id='1', external_extra='ok')\n"
                "_ = p.id\n"
                "summarize(InternalOrder(id='1', unread_flag=2))\n"
                "def api_handler() -> ExportedOrder:\n"
                "    return ExportedOrder(id='1', serialized_only='yes')\n"
                "api_handler()\n"
            ),
            SCHEMA: (
                "from pydantic import BaseModel, field_validator\n"
                "class BaseSchema(BaseModel):\n"
                "    pass\n"
                "class WirePayload(BaseSchema):\n"
                "    id: str\n"
                "    external_extra: str\n"
                "    @field_validator('id')\n"
                "    @classmethod\n"
                "    def check_id(cls, v: str) -> str:\n"
                "        return v\n"
            ),
            MODEL: (
                "from dataclasses import dataclass\n"
                "@dataclass(frozen=True)\n"
                "class InternalOrder:\n"
                "    id: str\n"
                "    unread_flag: int\n"
                "@dataclass(frozen=True)\n"
                "class ExportedOrder:\n"
                "    id: str\n"
                "    serialized_only: str\n"
            ),
            CORE: (
                "from example.domains.orders.model import InternalOrder\n"
                "def summarize(order: InternalOrder) -> str:\n"
                "    return order.id\n"
            ),
        }
    )
    # Pydantic transitive subclass fields & validators are exempt!
    assert [f for f in project.findings(SCHEMA) if f.code == "LIT512"] == []
    # Pure internal core dataclass write-only field is caught; boundary-returned dataclass escapes!
    model_512 = [f.message for f in project.findings(MODEL) if f.code == "LIT512"]
    assert any("InternalOrder.unread_flag" in m for m in model_512)
    assert not any("InternalOrder.id" in m for m in model_512)
    assert not any("ExportedOrder.serialized_only" in m for m in model_512)


def test_lit513_unreachable_code_and_with_suppress_exemption(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.core import dead_after_return, safe_suppress\n"
                "dead_after_return(1)\n"
                "safe_suppress('1')\n"
            ),
            CORE: (
                "from contextlib import suppress\n"
                "def dead_after_return(x: int) -> int:\n"
                "    if False:\n"
                "        return 0\n"
                "    return x\n"
                "    return x + 1\n"
                "def safe_suppress(raw: str) -> int | None:\n"
                "    with suppress(ValueError):\n"
                "        return int(raw)\n"
                "    return None\n"
            ),
        }
    )
    msgs = [f.message for f in project.findings(CORE) if f.code == "LIT513"]
    assert len(msgs) == 2
    assert any("unsatisfiable 'if'" in m for m in msgs)
    assert any("after 'return'" in m for m in msgs)


def test_lit514_unused_parameter_strict_contract_exemptions(project: Project) -> None:
    _enable_dead_code(project)
    project.write(
        {
            MAIN: (
                "from example.domains.orders.core import direct_fn, callback_fn, prefixed_fn\n"
                "direct_fn(1, 2)\n"
                "prefixed_fn(1, 2)\n"
                "cb = callback_fn\n"
                "cb(1, 2)\n"
            ),
            CORE: (
                "def direct_fn(used: int, forgotten: int) -> int:\n"
                "    return used + 1\n"
                "def prefixed_fn(used: int, _ignored: int) -> int:\n"
                "    return used + 1\n"
                "def callback_fn(used: int, required_by_caller: int) -> int:\n"
                "    return used + 1\n"
            ),
        }
    )
    msgs = [f.message for f in project.findings(CORE) if f.code == "LIT514"]
    assert msgs == ["unused parameter 'forgotten' in 'direct_fn'"]


def test_dead_code_entrypoints_keep_and_stale_manifest_checks(project: Project) -> None:
    project.write(
        {
            "pyproject.toml": (
                '[project]\nname = "example"\n'
                '[project.scripts]\nexample = "example.domains.orders.service:cli_entry"\n'
            ),
            SERVICE: (
                "def cli_entry() -> int:\n    return 1\ndef cron_hook() -> int:\n    return 2\n"
            ),
        }
    )
    _enable_dead_code(
        project,
        extra=(
            "\n[dead_code]\n"
            'entrypoints = ["example.domains.orders.service:missing_ep"]\n'
            "[[dead_code.keep]]\n"
            'symbols = ["example.domains.orders.service:cron_hook"]\n'
            'reason = "Invoked by systemd timer"\n'
            'review_by = "2020-01-01"\n'
        ),
    )
    findings = project.findings()
    # Both pyproject script `cli_entry` and kept `cron_hook` are live (no LIT511).
    assert [f for f in findings if f.code == "LIT511"] == []
    # Stale entrypoint + expired review_by produce LIT003.
    lit003 = [f.message for f in findings if f.code == "LIT003"]
    assert any("missing_ep" in m for m in lit003)
    assert any("expired" in m for m in lit003)

    # Invalid entrypoint spec is rejected at manifest load.
    _enable_dead_code(project, extra='\n[dead_code]\nentrypoints = ["bad spec!"]\n')
    with pytest.raises(ManifestError, match="invalid module spec"):
        project.load()


def test_dead_code_generator_stub_literal_enum_and_pep695_bounds(project: Project) -> None:
    _enable_dead_code(
        project,
        extra='\n[dead_code]\nentrypoints = ["example.domains.orders.service:_wire"]\n',
    )
    project.write(
        {
            SERVICE: (
                "from collections.abc import AsyncIterator\n"
                "from enum import Enum\n"
                "from typing import Literal\n"
                "from example.domains.orders.core import run_all\n"
                "def main() -> int:\n"
                "    return run_all()\n"
                "async def never_stream() -> AsyncIterator[str]:\n"
                "    raise AssertionError('not called')\n"
                "    yield ''\n"
                "class Mode(Enum):\n"
                "    FAST = 'fast'\n"
                "def takes_mode(mode: Literal[Mode.FAST]) -> str:\n"
                "    return mode.value\n"
                "def _wire() -> object:\n"
                "    return (main, never_stream, takes_mode)\n"
            ),
            CORE: (
                "from typing import Protocol\n"
                "class _Item(Protocol):\n"
                "    @property\n"
                "    def value(self) -> int: ...\n"
                "def pick[T: _Item](items: list[T]) -> int:\n"
                "    return sum(i.value for i in items)\n"
                "def run_all() -> int:\n"
                "    return pick([])\n"
            ),
        }
    )
    findings = project.findings()
    assert [f for f in findings if f.code == "LIT513"] == []
    unused_syms = {f.target for f in findings if f.code == "LIT511"}
    assert "Mode" not in unused_syms
    assert "_Item" not in unused_syms
