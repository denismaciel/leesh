from __future__ import annotations

import pytest
from conftest import ManifestError, Project

CORE = "src/example/domains/orders/core.py"
MODEL = "src/example/domains/orders/model.py"
PORTS = "src/example/domains/orders/ports.py"
SERVICE = "src/example/domains/orders/service.py"
ADAPTER = "src/example/domains/orders/persistence/repository.py"
ROUTER = "src/example/app/routers/orders.py"
BOUNDARY = "src/example/app/main.py"
UNIT_TEST = "tests/test_orders.py"
INTEGRATION_TEST = "tests/integration/test_orders.py"


def test_lit101_forbids_core_importing_port_by_default(project: Project) -> None:
    project.write(
        {
            PORTS: (
                "from typing import Protocol\n"
                "class Store(Protocol):\n"
                "    def save(self) -> None: ...\n"
            ),
            CORE: "from example.domains.orders.ports import Store\n",
        }
    )
    assert project.codes(CORE) == ["LIT101"]


def test_lit105_detects_module_import_cycles(project: Project) -> None:
    project.write(
        {
            CORE: "from example.domains.orders.model import Order\n",
            MODEL: "from example.domains.orders.core import calculate\n",
        }
    )
    assert project.codes(CORE) == ["LIT105"]
    assert project.codes(MODEL) == ["LIT105"]


def test_lit105_allows_acyclic_imports(project: Project) -> None:
    project.write(
        {
            MODEL: "VALUE = 1\n",
            CORE: "from example.domains.orders.model import VALUE\n",
        }
    )
    assert project.codes(CORE) == []
    assert project.codes(MODEL) == []


def test_lit106_rejects_local_imports_and_type_checking_in_inner_layers(project: Project) -> None:
    local = "def run() -> int:\n    import json\n    return len(json.dumps({}))\n"
    assert "LIT106" in project.lint(CORE, local)
    assert "LIT106" in project.lint(SERVICE, local)
    assert project.lint(BOUNDARY, local) == []
    assert project.lint(UNIT_TEST, local) == []

    tc = "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    import math\n"
    assert project.lint(CORE, tc) == ["LIT106"]
    assert project.lint(PORTS, tc) == ["LIT106"]
    assert project.lint(SERVICE, tc) == []


def test_lit203_extends_to_shell_and_catches_caches_and_singletons(project: Project) -> None:
    cache_src = (
        "from functools import lru_cache\n"
        "@lru_cache(maxsize=8)\n"
        "def f(x: int) -> int:\n"
        "    return x\n"
    )
    # Pure core functions may memoize; impure layers may not cache process-wide state.
    assert project.lint(CORE, cache_src) == []
    assert project.lint(SERVICE, cache_src) == ["LIT203"]
    assert project.lint(ADAPTER, cache_src) == ["LIT203"]
    assert project.lint(BOUNDARY, cache_src) == []

    singleton = "import threading\nLOCK = threading.Lock()\ndef run() -> object:\n    return LOCK\n"
    assert project.lint(SERVICE, singleton) == ["LIT203"]
    assert project.lint(ADAPTER, singleton) == ["LIT203"]
    assert project.lint(BOUNDARY, singleton) == []

    # nonlocal is banned in core/port, but allowed for method-local closures in adapter/shell.
    nonlocal_src = (
        "def run() -> int:\n"
        "    count = 0\n"
        "    def bump() -> int:\n"
        "        nonlocal count\n"
        "        count += 1\n"
        "        return count\n"
        "    return bump()\n"
    )
    assert project.lint(CORE, nonlocal_src) == ["LIT203"]
    assert project.lint(ADAPTER, nonlocal_src) == []
    assert project.lint(SERVICE, nonlocal_src) == []


def test_lit205_rejects_object_setattr_on_non_self(project: Project) -> None:
    source = """\
        from dataclasses import dataclass
        @dataclass(frozen=True)
        class Order:
            id: str
            def __post_init__(self) -> None:
                object.__setattr__(self, "id", self.id)
        def mutate() -> Order:
            order = Order("1")
            object.__setattr__(order, "id", "x")
            return order
        """
    assert project.lint(MODEL, source) == ["LIT205"]


def test_lit208_closes_reflection_backdoors(project: Project) -> None:
    assert "LIT208" in project.lint(CORE, "def f(x: object) -> object:\n    return vars(x)\n")
    assert "LIT208" in project.lint(CORE, "def f(x: object) -> object:\n    return x.__dict__\n")
    assert "LIT208" in project.lint(
        CORE, "def f(x: object, k: str) -> object:\n    return getattr(x, k)\n"
    )
    assert "LIT208" in project.lint(CORE, "def f(x: object) -> None:\n    delattr(x, 'id')\n")
    assert project.lint(CORE, "def f(x: object) -> object:\n    return getattr(x, 'id')\n") == []


def test_lit209_requires_annotations_and_forbids_any_bare_containers_and_typeddict(
    project: Project,
) -> None:
    assert "LIT209" in project.lint(CORE, "def f(x) -> int:\n    return 1\n")
    assert "LIT209" in project.lint(CORE, "def f(x: int):\n    return x\n")
    assert "LIT209" in project.lint(
        CORE, "from typing import Any\ndef f(x: Any) -> int:\n    return 1\n"
    )
    assert "LIT209" in project.lint(CORE, "def f(x: dict) -> int:\n    return len(x)\n")
    assert "LIT209" in project.lint(CORE, "def f(x: tuple) -> int:\n    return len(x)\n")
    assert "LIT209" in project.lint(CORE, "def f(x: frozenset) -> int:\n    return len(x)\n")
    assert "LIT209" in project.lint(
        MODEL, "from typing import TypedDict\nclass Row(TypedDict):\n    id: str\n"
    )
    assert project.lint(CORE, "def f(x: dict[str, int]) -> int:\n    return len(x)\n") == []
    assert project.lint(CORE, "def f(x: tuple[int, ...]) -> int:\n    return len(x)\n") == []
    assert project.lint(CORE, "def f(x: frozenset[str]) -> int:\n    return len(x)\n") == []
    assert (
        project.lint(
            CORE,
            "Key = tuple[str, int]\ndef f(x: Key) -> int:\n    return len(x)\n",
        )
        == []
    )
    assert "LIT209" in project.lint(CORE, "Raw = dict\ndef f(x: Raw) -> int:\n    return len(x)\n")
    assert project.lint(SERVICE, "from typing import Any\ndef f(x: Any):\n    return x\n") == []


def test_lit303_extends_to_shell_and_io_packages_without_flagging_method_calls(
    project: Project,
) -> None:
    assert project.lint(SERVICE, "import httpx\ndef run():\n    return httpx.AsyncClient()\n") == [
        "LIT303"
    ]
    assert project.lint(
        SERVICE, "import subprocess\ndef run():\n    return subprocess.run(['ls'])\n"
    ) == ["LIT303"]
    # Calling a method on an injected client is allowed.
    assert (
        project.lint(
            ROUTER,
            "import httpx\nasync def endpoint(client: httpx.AsyncClient):\n    return await client.get('https://x')\n",
        )
        == []
    )


def test_lit304_catches_patching_libraries_and_module_attribute_writes(project: Project) -> None:
    assert project.lint(UNIT_TEST, "import freezegun\n") == ["LIT304"]
    assert project.lint(UNIT_TEST, "from respx import mock\n") == ["LIT304"]
    assert project.lint(UNIT_TEST, "from pytest import MonkeyPatch\n") == ["LIT304"]
    assert project.lint(UNIT_TEST, "import math\nmath.sqrt = lambda x: x\n") == ["LIT304"]
    assert project.lint(UNIT_TEST, "import math\nsetattr(math, 'sqrt', lambda x: x)\n") == [
        "LIT304"
    ]
    assert project.lint(UNIT_TEST, "import math\ndel math.sqrt\n") == ["LIT304"]
    # Shadowing an imported module name with a local variable or parameter is not patching.
    assert (
        project.lint(
            UNIT_TEST,
            "import math\n"
            "class Fake:\n    pass\n"
            "def test_shadow():\n    math = Fake()\n    math.sqrt = 1\n",
        )
        == []
    )
    assert (
        project.lint(
            UNIT_TEST,
            "import math\ndef test_param(math):\n    math.sqrt = 1\n",
        )
        == []
    )
    # Mutating a local fake object in tests is still allowed.
    assert project.lint(UNIT_TEST, "class Fake:\n    pass\nf = Fake()\nf.calls = 1\n") == []


def test_lit307_rejects_broad_except_swallowing_outside_edges(project: Project) -> None:
    swallow = "def run():\n    try:\n        work()\n    except Exception:\n        return None\n"
    reraise = (
        "def run():\n"
        "    try:\n"
        "        work()\n"
        "    except Exception:\n"
        "        cleanup()\n"
        "        raise\n"
    )
    translate = (
        "class PortError(Exception):\n"
        "    pass\n"
        "def run():\n"
        "    try:\n"
        "        work()\n"
        "    except Exception as exc:\n"
        "        raise PortError('boom') from exc\n"
    )
    assert project.lint(SERVICE, swallow) == ["LIT307"]
    assert project.lint(ADAPTER, swallow) == ["LIT307"]
    assert project.lint(SERVICE, reraise) == []
    assert project.lint(ADAPTER, translate) == []
    assert project.lint(SERVICE, translate) == []
    assert project.lint(ROUTER, swallow) == []
    assert project.lint(BOUNDARY, swallow) == []


def test_lit308_rejects_defensive_duck_typing_in_core_and_shell(project: Project) -> None:
    assert project.lint(CORE, "def f(x: object) -> bool:\n    return hasattr(x, 'id')\n") == [
        "LIT401",
        "LIT308",
    ] or set(
        project.lint(CORE, "def is_ok(x: object) -> bool:\n    return hasattr(x, 'id')\n")
    ) == {"LIT308"}
    assert project.lint(
        CORE, "def f(x: object) -> object:\n    return getattr(x, 'id', None)\n"
    ) == ["LIT308"]
    # Dynamic 3-arg getattr in core reports only LIT208, not both LIT208 and LIT308.
    assert project.lint(
        CORE, "def f(x: object, key: str) -> object:\n    return getattr(x, key, None)\n"
    ) == ["LIT208"]
    assert project.lint(SERVICE, "def f(x):\n    return getattr(x, 'id', None)\n") == ["LIT308"]
    assert project.lint(SERVICE, "def f(x, key):\n    return getattr(x, key, None)\n") == ["LIT308"]
    assert project.lint(
        CORE, "from typing import cast\ndef f(x: object) -> int:\n    return cast(int, x)\n"
    ) == ["LIT308"]
    # typing.cast is allowed in shell (interfacing with untyped third-party code).
    assert (
        project.lint(SERVICE, "from typing import cast\ndef f(x):\n    return cast(int, x)\n") == []
    )
    assert project.lint(BOUNDARY, "def f(x):\n    return hasattr(x, 'id')\n") == []


def test_lit309_checks_test_sleep_and_unit_test_adapter_imports(project: Project) -> None:
    assert project.lint(UNIT_TEST, "import time\ndef test_slow():\n    time.sleep(0.1)\n") == [
        "LIT309"
    ]
    assert project.lint(
        INTEGRATION_TEST, "import time\ndef test_slow():\n    time.sleep(0.1)\n"
    ) == ["LIT309"]
    project.write({ADAPTER: "VALUE = 1\n"})
    # Unit-test adapter import check is opt-in via workspace.strict.
    assert (
        project.lint(UNIT_TEST, "from example.domains.orders.persistence.repository import VALUE\n")
        == []
    )
    project.manifest("[workspace]\nstrict = true\n")
    assert project.lint(
        UNIT_TEST, "from example.domains.orders.persistence.repository import VALUE\n"
    ) == ["LIT309"]
    assert (
        project.lint(
            INTEGRATION_TEST, "from example.domains.orders.persistence.repository import VALUE\n"
        )
        == []
    )
    assert (
        project.lint(
            "tests/adapters/test_repository.py",
            "from example.domains.orders.persistence.repository import VALUE\n",
        )
        == []
    )
    assert (
        project.lint(
            "tests/app/orders_integration_test.py",
            "from example.domains.orders.persistence.repository import VALUE\n",
        )
        == []
    )
    assert (
        project.lint(
            "tests/integrations/test_db.py",
            "from example.domains.orders.persistence.repository import VALUE\n",
        )
        == []
    )


def test_lit309_uses_layout_integration_test_layer(project: Project) -> None:
    project.manifest(
        """
        [workspace]
        layout = "custom"
        strict = true

        [layouts.custom]
        extends = "domains"
        layers = [{ glob = "tests/db/**/*.py", layer = "integration_test" }]
        """
    )
    project.write({ADAPTER: "VALUE = 1\n"})
    assert (
        project.lint(
            "tests/db/test_repo.py",
            "from example.domains.orders.persistence.repository import VALUE\n",
        )
        == []
    )


def test_lit401_resolves_string_annotations_and_annotated(project: Project) -> None:
    source = (
        "from typing import Annotated\n"
        'def plan(urgent: "bool") -> int:\n    return 1\n'
        'def decide(flag: Annotated[bool, "meta"]) -> int:\n    return 1\n'
    )
    assert project.lint(CORE, source) == ["LIT401", "LIT401"]


def test_lit404_forbids_plain_classes_always_and_record_methods_in_strict(project: Project) -> None:
    plain = "class Calculator:\n    pass\n"
    assert project.lint(CORE, plain) == ["LIT404"]

    method = """\
        from dataclasses import dataclass
        @dataclass(frozen=True)
        class Order:
            total: int
            def apply_discount(self, pct: int) -> int:
                return self.total - pct
            @property
            def cents(self) -> int:
                return self.total * 100
        """
    # Default mode allows pure methods on frozen records.
    assert project.lint(MODEL, method) == []
    # Strict mode forbids public instance methods on core records.
    project.manifest("[workspace]\nstrict = true\n")
    assert project.lint(MODEL, method) == ["LIT404"]


def test_lit405_forbids_raises_outside_post_init_in_strict(project: Project) -> None:
    source = """\
        def parse(value: int) -> int:
            if value < 0:
                raise ValueError("negative")
            return value
        """
    # Off by default.
    assert project.lint(CORE, source) == []

    project.manifest("[workspace]\nstrict = true\n")
    assert project.lint(CORE, source) == ["LIT405"]

    # Narrow stdlib try/except parsing and guard exceptions are allowed even in strict mode.
    try_src = """\
        def parse(value: str) -> int:
            try:
                return int(value)
            except ValueError:
                return 0
        """
    assert project.lint(CORE, try_src) == []

    guard = """\
        def unreachable(value: int) -> int:
            if value >= 0:
                return value
            raise AssertionError("unreachable")
        """
    assert project.lint(CORE, guard) == []

    post_init = """\
        from dataclasses import dataclass
        @dataclass(frozen=True)
        class Positive:
            raw: str
            def __post_init__(self) -> None:
                try:
                    value = int(self.raw)
                except ValueError as exc:
                    raise ValueError("not an int") from exc
                if value <= 0:
                    raise ValueError("must be positive")
        """
    assert project.lint(MODEL, post_init) == []

    top_level_post_init = """\
        def __post_init__() -> None:
            raise ValueError("not a class method")
        """
    assert project.lint(CORE, top_level_post_init) == ["LIT405"]


def test_lit406_requires_explicit_public_core_port_args_and_fields_in_strict(
    project: Project,
) -> None:
    fn = "def calculate(limit: int = 10) -> int:\n    return limit\n"
    # Off by default.
    assert project.lint(CORE, fn) == []

    project.manifest("[workspace]\nstrict = true\n")
    assert project.lint(CORE, fn) == ["LIT406"]
    # Shell, private helpers, and lambdas are exempt even in strict mode.
    assert project.lint(SERVICE, fn) == []
    assert project.lint(ROUTER, fn) == []
    assert project.lint(CORE, "def _helper(limit: int = 10) -> int:\n    return limit\n") == []
    assert project.lint(CORE, "FN = lambda x=1: x\n") == []

    record = """\
        from dataclasses import dataclass, field
        @dataclass(frozen=True)
        class Config:
            limit: int = 10
            derived: int = field(init=False)
        """
    assert project.lint(MODEL, record) == ["LIT406"]

    generic_and_aliased_protocols = """\
        from typing import Protocol, Protocol as P, TypeVar
        T = TypeVar("T")
        class Repo(Protocol[T]):
            def fetch(self, limit: int = 10) -> T: ...
        class Store(P):
            def save(self, value: str = "") -> None: ...
        """
    assert project.lint(PORTS, generic_and_aliased_protocols) == []


def test_manifest_review_by_and_max_count(project: Project) -> None:
    project.manifest(
        """
        [[ignore]]
        rules = ["LIT308"]
        paths = ["src/example/domains/orders/service.py"]
        reason = "Temporary hasattr"
        review_by = "2020-01-01"
        max_count = 1
        """
    )
    project.write({SERVICE: ("def run(x):\n    return hasattr(x, 'a') and hasattr(x, 'b')\n")})
    messages = [item.message for item in project.findings()]
    assert any("max_count exceeded" in msg for msg in messages)
    assert any("review_by 2020-01-01 has expired" in msg for msg in messages)


def test_manifest_rejects_invalid_review_by(project: Project) -> None:
    project.manifest(
        """
        [[ignore]]
        rules = ["LIT308"]
        reason = "Bad date"
        review_by = "2026-02-30"
        """
    )
    with pytest.raises(ManifestError, match="review_by"):
        project.load()


def test_manifest_rejects_non_bool_strict(project: Project) -> None:
    project.manifest('[workspace]\nstrict = "yes"\n')
    with pytest.raises(ManifestError, match=r"workspace\.strict"):
        project.load()
