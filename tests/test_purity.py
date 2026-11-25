from __future__ import annotations

import pytest
from conftest import Project

CORE = "src/example/domains/orders/core.py"
MODEL = "src/example/domains/orders/model.py"
PORTS = "src/example/domains/orders/ports.py"
SERVICE = "src/example/domains/orders/service.py"


@pytest.mark.parametrize(
    "source",
    [
        "from uuid import uuid4 as make_id\ndef calculate():\n    return make_id()",
        "import datetime as dt\ndef calculate():\n    return dt.datetime.now(dt.UTC)",
        "import random\nvalue = random.choice(('a', 'b'))",
        "import secrets\ntoken = secrets.token_hex()",
        "import time\nstart = time.monotonic()",
        "import os\nseed = os.urandom(8)",
        """\
        import uuid
        from dataclasses import dataclass, field
        @dataclass(frozen=True)
        class Value:
            id: uuid.UUID = field(default_factory=uuid.uuid4)
        """,
        """\
        from datetime import datetime
        from dataclasses import dataclass, field
        @dataclass(frozen=True)
        class Created:
            at: datetime = field(default_factory=datetime.now)
        """,
    ],
)
def test_lit201_rejects_nondeterminism_in_core(project: Project, source: str) -> None:
    [finding] = [
        item for item in project.write({CORE: source}).findings(CORE) if item.code == "LIT201"
    ]
    assert finding.code == "LIT201"
    assert "pass the observed" in finding.message


@pytest.mark.parametrize(
    "source",
    [
        "import time\ntime.sleep(1)",
        "import time\nnow = time.localtime()",
        "from time import strftime\nyear = strftime('%Y')",
        "import time\nread = getattr(time, 'time')",
    ],
)
def test_lit201_keeps_the_time_module_out_of_core(project: Project, source: str) -> None:
    assert project.lint(CORE, source) == ["LIT201"]
    assert project.lint(SERVICE, source.replace("'time'", "'sleep'")) == []


def test_lit201_allows_time_values_in_core(project: Project) -> None:
    source = "import time\ndef year(value: time.struct_time) -> int:\n    return value.tm_year"
    assert project.lint(CORE, source) == []


def test_lit201_allows_deterministic_ids(project: Project) -> None:
    source = (
        "import uuid\n"
        "def calculate(namespace: uuid.UUID, key: str) -> uuid.UUID:\n"
        "    return uuid.uuid5(namespace, key)"
    )
    assert project.lint(CORE, source) == []


@pytest.mark.parametrize(
    "source",
    [
        "from datetime import datetime as D\nnow = D.now()",
        "import datetime as dt\nnow = dt.datetime.utcnow()",
        "from datetime import date\nnow = date.today()",
        "import time as t\nnow = t.time_ns()",
        "from time import time as read\nnow = read()",
        "from datetime import datetime\nread = datetime.now",
    ],
)
@pytest.mark.parametrize("path", [SERVICE, "src/example/app/main.py", "scripts/report.py"])
def test_lit201_requires_the_clock_outside_core(project: Project, path: str, source: str) -> None:
    [finding] = project.write({path: source}).findings(path)
    assert finding.code == "LIT201"
    assert "workspace.clock_modules" in finding.message


@pytest.mark.parametrize(
    "source",
    [
        "import time\nstart = time.monotonic()",
        "import time\nstart = time.perf_counter_ns()",
        "import uuid\nvalue = uuid.uuid4()",
        "class Clock:\n    def now(self): ...\nnow = Clock().now()",
    ],
)
def test_lit201_allows_other_sources_outside_core(project: Project, source: str) -> None:
    assert project.lint(SERVICE, source) == []


def test_lit201_exempts_clock_modules_and_tests_but_not_core(project: Project) -> None:
    project.manifest(
        '[workspace]\nclock_modules = ["example.integrations.clock", "example.domains.orders.core"]'
    )
    read = "from datetime import datetime\nnow = datetime.now()\n"
    assert project.lint("src/example/integrations/clock.py", read) == []
    assert project.lint("src/example/integrations/other.py", read) == ["LIT201"]
    assert project.lint("tests/test_clock.py", read) == []
    assert project.lint(CORE, read) == ["LIT201"]


@pytest.mark.parametrize(
    "source",
    [
        "from pathlib import Path\ndef read(path: Path):\n    return path.read_text()",
        'from pathlib import Path as P\ndef read():\n    return P("x").exists()',
        'import pathlib as p\ndef read():\n    return p.Path("x").read_bytes()',
        'import logging as log\ndef decide():\n    log.warning("x")',
        'import logging\ndef decide():\n    logging.getLogger(__name__).info("x")',
        'from builtins import open as read\ndef decide():\n    return read("x")',
        'import sys\ndef decide():\n    sys.stderr.write("x")',
        'from pathlib import Path\nREADER = Path("x").read_text',
        "print('x')",
        "import shutil\nshutil.rmtree('x')",
        "import os\nos.rmdir('x')",
        "import os\npid = os.getpid()",
        "import os\nfound = os.path.exists('x')",
        "import os.path as p\nsize = p.getsize('x')",
        "import sys\nargs = sys.argv",
        "import sys\nsys.path.append('x')",
        "import sys\nsys.modules['x'] = None",
        "import json\nimport sys\ndata = json.load(sys.stdin)",
        "import builtins\nreader = getattr(builtins, 'open')",
        "import sys\ngetattr(sys, 'stdout').write('x')",
    ],
)
@pytest.mark.parametrize("path", [CORE, PORTS])
def test_lit202_rejects_io_in_core(project: Project, path: str, source: str) -> None:
    assert "LIT202" in project.lint(path, source)


@pytest.mark.parametrize(
    "source",
    [
        "def calculate(open):\n    return open(1)",
        "import logging\ndef calculate(logging):\n    return logging.warning(1)",
        'from pathlib import PurePosixPath\ndef calculate():\n    return PurePosixPath("x").name',
        "def calculate(value):\n    return value.read_text()",
        "def other():\n    import logging\ndef calculate(logging):\n    return logging.info(1)",
        "import logging\ndef calculate():\n    logging = object()\n    return logging.info(1)",
        "import logging\nvalues = [logging.info(1) for logging in ()]",
        """\
        import logging
        def calculate(flag, other):
            if flag:
                target = logging
            else:
                target = other
            return target.info(1)
        """,
        "import logging\ndef calculate():\n    return logging.info(1)\nlogging = object()",
        """\
        import logging
        def calculate(value):
            match value:
                case logging:
                    return logging.info(1)
        """,
        """\
        import logging
        def calculate():
            try:
                work()
            except ValueError as logging:
                return logging.info(1)
        """,
    ],
)
def test_lit202_does_not_invent_effects_for_shadowed_names(project: Project, source: str) -> None:
    assert "LIT202" not in project.lint(CORE, source)


@pytest.mark.parametrize(
    "source",
    [
        "import os\nname = os.path.join('a', os.sep, os.path.basename('b'))",
        "import os\ndef calculate(path: os.PathLike[str]) -> str:\n    return os.fspath(path)",
        "import sys\nlimit = sys.maxsize if sys.version_info >= (3, 12) else 0",
        "def calculate(order: object) -> object:\n    return getattr(order, 'status')",
    ],
)
def test_lit202_allows_pure_os_and_sys(project: Project, source: str) -> None:
    assert project.lint(CORE, source) == []


def test_lit202_allows_io_in_the_shell(project: Project) -> None:
    assert project.lint(SERVICE, "print('x')\nopen('x')\n") == []


@pytest.mark.parametrize(
    "source",
    [
        "STATE = []\ndef calculate():\n    return len(STATE)",
        "count = 0\ndef calculate():\n    global count\n    count += 1",
        """\
        import contextvars
        request = contextvars.ContextVar("request")
        def calculate():
            return request.get()
        """,
        "from decimal import getcontext\ndef calculate():\n    return getcontext().prec",
        "import os\ndef calculate():\n    return os.getcwd()",
        "import itertools\nIDS = itertools.count()\ndef calculate():\n    return next(IDS)",
        "from collections import Counter\nSEEN = Counter()\ndef calculate():\n    return SEEN",
        "from collections import deque\nQUEUE = deque()\ndef calculate():\n    return len(QUEUE)",
    ],
)
def test_lit203_rejects_ambient_state(project: Project, source: str) -> None:
    assert "LIT203" in project.lint(CORE, source)


@pytest.mark.parametrize(
    "source",
    [
        "STATE = []\ndef calculate(STATE: tuple[int, ...]) -> int:\n    return len(STATE)",
        """\
        STATE = []
        def calculate() -> int:
            STATE = (1, 2)
            def count() -> int:
                return len(STATE)
            return count()
        """,
    ],
)
def test_lit203_respects_shadowing(project: Project, source: str) -> None:
    assert project.lint(CORE, source) == []


def test_lit203_leaves_environment_to_lit301(project: Project) -> None:
    assert project.lint(CORE, "import os\nvalue = os.getenv('X')\n") == ["LIT301"]


@pytest.mark.parametrize(
    "source",
    [
        "def calculate(items: list[int]):\n    items.append(1)",
        "def calculate(items: list[int]):\n    alias = items\n    alias.clear()",
        'def calculate(items: dict[str, int]):\n    items["a"] = 1',
        'def calculate(order):\n    order.status = "sent"',
        "def calculate(order):\n    del order.status",
        'def calculate(order):\n    setattr(order, "status", "sent")',
        "def calculate(items: list[int]):\n    items += [1]",
        'def calculate(items: list[object]):\n    copied = list(items)\n    copied[0].status = "x"',
        'def calculate(items: list[object]):\n    for item in items:\n        item.status = "x"',
        'def calculate(items: list[object]):\n    copied = items[:]\n    copied[0].status = "x"',
        "def calculate(items):\n    items.append(1)",
        "def calculate(order):\n    order.lines.append(1)",
        "def calculate(index: dict):\n    for row in index.values():\n        row['x'] = 1",
        "def calculate(index):\n    for key, row in index.items():\n        row.update(a=1)",
        "def calculate(rows):\n    for i, row in enumerate(rows):\n        row['x'] = i",
        "def calculate(rows):\n    for row in sorted(rows):\n        row.x = 1",
        "def calculate(a, b):\n    for x, y in zip(a, a):\n        x.done = True",
        "def calculate(index):\n    index.get('a').append(1)",
        "def calculate(index):\n    found = index.get('a')\n    found['x'] = 1",
        "import copy\ndef calculate(order):\n    own = copy.copy(order)\n    own.lines[0].x = 1",
        """\
        from dataclasses import replace
        def calculate(order):
            own = replace(order, status="x")
            own.lines.append(1)
        """,
        "def calculate(order):\n    own = order.copy()\n    own['lines'].append(1)",
        "from collections import deque\ndef calculate(queue: deque):\n    return queue.popleft()",
        "from collections import Counter\ndef calculate(index: Counter):\n    index.clear()",
    ],
)
def test_lit204_rejects_borrowed_mutation(project: Project, source: str) -> None:
    assert "LIT204" in project.lint(CORE, source)


@pytest.mark.parametrize(
    "source",
    [
        """\
        def calculate(items: tuple[int, ...]):
            result = []
            for item in items:
                result.append(item)
            return tuple(result)
        """,
        "def calculate(items: list[int]):\n    copied = list(items)\n    copied.sort()",
        "def calculate(items: list[int]):\n    alias = items\n    alias = []\n    alias.append(1)",
        "def calculate(number: int):\n    number += 1\n    return number",
        "def calculate(items: list):\n    def nested():\n        own = []\n        own.clear()",
        "def calculate(order):\n    copied = order.copy()\n    copied.status = 'x'",
        "import copy\ndef calculate(order):\n    own = copy.deepcopy(order)\n    own.a.x = 1",
        "def calculate(order):\n    return order.total.add(1)",
        "def calculate(order: Order):\n    order.append(1)",
        "def calculate(index):\n    found = index.get('a')\n    return found.append",
        "def calculate(items: list[int]):\n    return [items.clear() for items in [[], []]]",
    ],
)
def test_lit204_allows_local_mutation(project: Project, source: str) -> None:
    assert "LIT204" not in project.lint(CORE, source)


@pytest.mark.parametrize(
    "source",
    [
        "from dataclasses import dataclass\n@dataclass\nclass Order:\n    id: str",
        "from dataclasses import dataclass as dc\n@dc(frozen=True)\nclass Order:\n    i: list[str]",
        """\
        from dataclasses import dataclass
        @dataclass(frozen=True)
        class Order:
            ids: tuple[dict[str, int], ...]
        """,
    ],
)
@pytest.mark.parametrize("path", [MODEL, PORTS])
def test_lit205_rejects_mutable_records(project: Project, path: str, source: str) -> None:
    assert project.lint(path, source) == ["LIT205"]


@pytest.mark.parametrize(
    "field",
    [
        'extra: "dict[str, int] | None"',
        'extra: "tuple[list[int], ...]"',
        "extra: defaultdict",
        "extra: deque[int]",
        "extra: bytearray",
        "extra: object = field(default_factory=list)",
        "extra: object = field(default_factory=lambda: {})",
    ],
)
def test_lit205_rejects_hidden_mutable_fields(project: Project, field: str) -> None:
    source = f"""\
        from collections import defaultdict, deque
        from dataclasses import dataclass, field
        @dataclass(frozen=True)
        class Order:
            {field}
        """
    assert "LIT205" in project.lint(MODEL, source)


def test_lit205_checks_named_tuple_fields(project: Project) -> None:
    source = "from typing import NamedTuple\nclass Row(NamedTuple):\n    cells: list[str]"
    assert project.lint(MODEL, source) == ["LIT205"]
    assert project.lint(MODEL, source.replace("list[str]", "tuple[str, ...]")) == []


def test_lit205_rejects_writes_to_frozen_records(project: Project) -> None:
    source = """\
        from dataclasses import dataclass, field
        @dataclass(frozen=True)
        class Order:
            id: str
            key: str = field(init=False)
            def __post_init__(self) -> None:
                object.__setattr__(self, "key", self.id.lower())
            def _rename(self, id: str) -> None:
                object.__setattr__(self, "id", id)
        """
    [finding] = project.write({MODEL: source}).findings(MODEL)
    assert (finding.code, finding.line) == ("LIT205", 9)


def test_lit205_reads_types_not_literal_values(project: Project) -> None:
    source = """\
        from dataclasses import dataclass, field
        from typing import Annotated, Literal
        @dataclass(frozen=True)
        class Order:
            kind: Literal["list", "dict"]
            note: Annotated[str, "a list of words"]
            tags: tuple[str, ...] = field(default_factory=tuple)
        """
    assert "LIT205" not in project.lint(MODEL, source)


@pytest.mark.parametrize(
    "source",
    [
        "result = eval('1 + 1')",
        "exec('x = 1')",
        "code = compile('1', 'x', 'eval')",
        "module = __import__('json')",
        "import importlib\nmodule = importlib.import_module('json')",
        "from importlib import import_module as load\nmodule = load('json')",
        "def calculate():\n    return globals()",
        "def calculate():\n    return locals()",
        "def calculate():\n    breakpoint()",
        "import pickle\ndef calculate(blob):\n    return pickle.loads(blob)",
        "import builtins\nrun = getattr(builtins, 'eval')",
    ],
)
@pytest.mark.parametrize("path", [CORE, PORTS])
def test_lit208_rejects_dynamic_code(project: Project, path: str, source: str) -> None:
    assert "LIT208" in project.lint(path, source)


@pytest.mark.parametrize(
    "source",
    [
        (
            "from collections.abc import Callable\n"
            "def calculate(eval: Callable[[int], int]) -> int:\n"
            "    return eval(1)"
        ),
        "import re\nPATTERN = re.compile('x')",
        "def compile(value: int) -> int:\n    return value\nresult = compile(1)",
        "import json\ndef calculate(text: str) -> object:\n    return json.loads(text)",
    ],
)
def test_lit208_does_not_guess(project: Project, source: str) -> None:
    assert project.lint(CORE, source) == []


def test_lit208_allows_dynamic_code_outside_core(project: Project) -> None:
    assert project.lint(SERVICE, "import importlib\nmodule = importlib.import_module('x')") == []


def test_laundered_environment_reads_are_seen(project: Project) -> None:
    assert project.lint(CORE, "import os\nvalue = getattr(os, 'environ')['X']\n") == ["LIT301"]


def test_lit205_allows_frozen_records(project: Project) -> None:
    source = """\
        from dataclasses import dataclass
        from typing import ClassVar
        @dataclass(frozen=True)
        class Order:
            ids: tuple[str, ...]
            registry: ClassVar[dict[str, int]] = {}
    """
    assert project.lint(MODEL, source) == []


@pytest.mark.parametrize(
    "source",
    [
        "async def calculate(repo: object) -> object:\n    return await repo.load()",
        "import asyncio\ndef calculate() -> None:\n    asyncio.create_task(work())",
        (
            "async def calculate(stream: object) -> None:\n"
            "    async for value in stream:\n"
            "        pass"
        ),
        "async def calculate(lock: object) -> None:\n    async with lock:\n        pass",
        "import threading\nworker = threading.Thread(target=int)",
    ],
)
def test_lit206_keeps_async_out_of_core(project: Project, source: str) -> None:
    assert project.lint(CORE, source) == ["LIT206"]
    assert "LIT206" not in project.lint(SERVICE, source)
    assert "LIT206" not in project.lint(PORTS, source)


@pytest.mark.parametrize(
    "annotation",
    [
        "S",
        "S | None",
        '"S | None"',
        "Optional[S]",
        'Optional["S"]',
        "Annotated[S, 'database']",
        '"Annotated[S, metadata]"',
        "list[S]",
    ],
)
def test_lit207_resolves_wrapped_and_forward_annotations(project: Project, annotation: str) -> None:
    project.write(
        {
            CORE: "from sqlalchemy.orm import Session as S\n"
            "from typing import Annotated, Optional\n"
            f"def decide(session: {annotation}): ...\n"
        }
    )
    [finding] = [item for item in project.findings(CORE) if item.code == "LIT207"]
    assert "sqlalchemy.orm.Session" in finding.message


@pytest.mark.parametrize(
    "signature",
    [
        "def decide(session: S, /): ...",
        "def decide(*, session: S): ...",
        "def decide(*session: S): ...",
        "def decide(**session: S): ...",
        "class Decision:\n    def __init__(self, session: S): ...",
        "async def decide(session: S): ...",
    ],
)
def test_lit207_checks_every_parameter_position(project: Project, signature: str) -> None:
    source = "from httpx import Client as S\n" + signature
    assert project.write({CORE: source}).codes(CORE).count("LIT207") == 1


@pytest.mark.parametrize(
    "type_name",
    [
        "httpx.AsyncClient",
        "requests.Session",
        "aiohttp.ClientSession",
        "io.TextIOBase",
        "typing.BinaryIO",
        "fsspec.AbstractFileSystem",
        "sqlite3.Connection",
    ],
)
def test_lit207_knows_library_capabilities(project: Project, type_name: str) -> None:
    module, _, name = type_name.rpartition(".")
    source = f"from {module} import {name}\ndef decide(value: {name}): ...\n"
    assert "LIT207" in project.lint(CORE, source)


@pytest.mark.parametrize(
    "source",
    [
        "from pathlib import Path\ndef decide(path: Path): ...",
        "class Repository: ...\ndef decide(repository: Repository): ...",
        "from unknown import Session\ndef decide(session: Session): ...",
        "from typing import Callable\ndef decide(fn: Callable[[int], int]): ...",
        "from typing import Any\ndef decide(session: Any): ...",
        "def decide(session): ...",
        "from sqlalchemy.orm import Session\nclass Session: ...\ndef decide(s: Session): ...",
        "from typing import Literal\nfrom io import IOBase\ndef f(x: Literal['IOBase']): ...",
        "from typing import Annotated as A\nfrom io import IOBase\ndef f(x: A[int, IOBase]): ...",
    ],
)
def test_lit207_does_not_guess(project: Project, source: str) -> None:
    assert "LIT207" not in project.lint(CORE, source)


@pytest.mark.parametrize("path", [SERVICE, PORTS])
def test_lit207_allows_capabilities_outside_core(project: Project, path: str) -> None:
    assert "LIT207" not in project.lint(path, "from io import IOBase\ndef run(f: IOBase): ...\n")


def test_lit207_registry_resolves_relative_imports(project: Project) -> None:
    project.manifest(
        '[workspace]\neffectful_types = ["example.domains.orders.ports.Clock"]\n'
        '[layer_dependencies]\ncore = ["core", "port"]\n'
    )
    project.write(
        {
            PORTS: "class Clock: ...\n",
            CORE: "from .ports import Clock as C\ndef f(c: C) -> None: ...\n",
        }
    )
    [finding] = project.findings(CORE)
    assert "example.domains.orders.ports.Clock" in finding.message


def test_lit207_follows_assignment_aliases(project: Project) -> None:
    source = "from io import IOBase\nStream = IOBase\ndef decide(stream: Stream) -> None: ...\n"
    assert project.lint(MODEL, source) == ["LIT207"]


def test_strings_that_cannot_be_annotations_do_not_break_resolution(project: Project) -> None:
    source = 'SURROGATE = "\\ud800"\nOCTAL = "1_a"\nPROSE = """line\nline"""\n'

    assert project.lint("src/example/domains/orders/core.py", source) == []
