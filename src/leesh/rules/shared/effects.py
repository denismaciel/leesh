from __future__ import annotations

from leesh.policy import IO_PACKAGES

WALL_CLOCK = frozenset(
    {
        "datetime.datetime.now",
        "datetime.datetime.today",
        "datetime.datetime.utcnow",
        "datetime.date.today",
        "time.time",
        "time.time_ns",
    }
)
ENVIRONMENT = frozenset({"os.environ", "os.environb", "os.getenv", "os.getenvb"})
NONDETERMINISTIC = frozenset(
    {
        "datetime.date.today",
        "datetime.datetime.now",
        "datetime.datetime.today",
        "datetime.datetime.utcnow",
        "time.time",
        "time.time_ns",
        "time.monotonic",
        "time.monotonic_ns",
        "time.perf_counter",
        "time.perf_counter_ns",
        "uuid.uuid1",
        "uuid.uuid4",
        "uuid.uuid6",
        "uuid.uuid7",
        "os.urandom",
    }
)
RANDOM_APIS = frozenset(
    {
        "random",
        "randint",
        "randrange",
        "choice",
        "choices",
        "shuffle",
        "sample",
        "uniform",
        "gauss",
        "getrandbits",
        "randbytes",
        "seed",
        "Random",
        "SystemRandom",
        "normalvariate",
        "triangular",
        "expovariate",
    }
)
SECRETS_APIS = frozenset(
    {"choice", "randbelow", "randbits", "token_bytes", "token_hex", "token_urlsafe", "SystemRandom"}
)
PATH_EFFECTS = frozenset(
    {
        "open",
        "read_text",
        "read_bytes",
        "write_text",
        "write_bytes",
        "exists",
        "is_file",
        "is_dir",
        "is_symlink",
        "stat",
        "lstat",
        "resolve",
        "absolute",
        "iterdir",
        "glob",
        "rglob",
        "walk",
        "mkdir",
        "rmdir",
        "unlink",
        "rename",
        "replace",
        "touch",
        "chmod",
        "owner",
        "group",
        "readlink",
        "symlink_to",
        "hardlink_to",
        "cwd",
        "home",
        "expanduser",
    }
)
LOG_METHODS = frozenset(
    {
        "debug",
        "info",
        "warning",
        "warn",
        "error",
        "exception",
        "critical",
        "fatal",
        "log",
        "basicConfig",
        "getLogger",
        "get_logger",
        "addHandler",
        "removeHandler",
        "setLevel",
    }
)
ASYNC_APIS = frozenset(
    {
        "run",
        "create_task",
        "ensure_future",
        "gather",
        "sleep",
        "wait",
        "wait_for",
        "to_thread",
        "TaskGroup",
        "get_event_loop",
        "get_running_loop",
    }
)
AMBIENT = frozenset(
    {
        "os.getcwd",
        "os.getcwdb",
        "os.chdir",
        "os.putenv",
        "os.unsetenv",
        "locale.setlocale",
        "locale.getlocale",
        "decimal.getcontext",
        "decimal.setcontext",
        "contextvars.copy_context",
    }
)
# Everything else under `os` and `sys` reaches the process or the filesystem.
PURE_OS = (
    "os.PathLike",
    "os.altsep",
    "os.curdir",
    "os.devnull",
    "os.extsep",
    "os.fsdecode",
    "os.fsencode",
    "os.fspath",
    "os.linesep",
    "os.name",
    "os.pardir",
    "os.pathsep",
    "os.sep",
)
PURE_OS_PATH = frozenset(
    {
        "altsep",
        "basename",
        "commonpath",
        "commonprefix",
        "curdir",
        "devnull",
        "dirname",
        "extsep",
        "isabs",
        "join",
        "normcase",
        "normpath",
        "pardir",
        "pathsep",
        "sep",
        "split",
        "splitdrive",
        "splitext",
        "splitroot",
    }
)
PURE_SYS = (
    "sys.byteorder",
    "sys.exc_info",
    "sys.float_info",
    "sys.hash_info",
    "sys.implementation",
    "sys.int_info",
    "sys.intern",
    "sys.maxsize",
    "sys.maxunicode",
    "sys.platform",
    "sys.version",
    "sys.version_info",
)
DYNAMIC = frozenset(
    {
        "builtins.__import__",
        "builtins.breakpoint",
        "builtins.compile",
        "builtins.eval",
        "builtins.exec",
        "builtins.globals",
        "builtins.locals",
        "importlib.__import__",
        "importlib.import_module",
        "importlib.reload",
    }
)
DYNAMIC_MODULES = ("marshal.", "pickle.", "runpy.", "shelve.")


def _under(name: str, roots: tuple[str, ...]) -> bool:
    return any(name == root or name.startswith(f"{root}.") for root in roots)


def is_nondeterministic(name: str) -> bool:
    module, _, member = name.rpartition(".")
    return (
        name in NONDETERMINISTIC
        or (module == "random" and member in RANDOM_APIS)
        or (module == "secrets" and member in SECRETS_APIS)
    )


def is_impure_in_core(name: str) -> bool:
    """Core and port code may not touch `time` at all: sleeping and local time included."""
    return is_nondeterministic(name) or (
        name.startswith("time.") and not _under(name, ("time.struct_time",))
    )


def is_dynamic(name: str) -> bool:
    return name in DYNAMIC or name.startswith(DYNAMIC_MODULES)


def is_environment(name: str) -> bool:
    return name in ENVIRONMENT or name.startswith(("os.environ.", "os.environb."))


def is_ambient(name: str) -> bool:
    return name in AMBIENT or name in {
        "contextvars.ContextVar.get",
        "contextvars.ContextVar.set",
        "contextvars.ContextVar.reset",
    }


def is_io(name: str) -> bool:
    module, _, member = name.rpartition(".")
    return (
        name.split(".")[0] in IO_PACKAGES
        or name
        in {
            "builtins.open",
            "builtins.print",
            "builtins.input",
            "io.open",
            "io.FileIO",
            "io.open_code",
        }
        or (
            module in {"pathlib.Path", "pathlib.PosixPath", "pathlib.WindowsPath"}
            and member in PATH_EFFECTS
        )
        or (
            module
            in {
                "logging",
                "logging.Logger",
                "logging.LoggerAdapter",
                "structlog",
                "structlog.BoundLogger",
            }
            and member in LOG_METHODS
        )
        or name.startswith(("urllib.request.", "shutil.", "tempfile."))
        or _impure_os(name)
        or (name.startswith("sys.") and not _under(name, PURE_SYS))
    )


def _impure_os(name: str) -> bool:
    """Environment, ambient state and randomness under `os` belong to other rules."""
    if not name.startswith("os.") or is_environment(name) or is_ambient(name):
        return False
    if name == "os.urandom" or _under(name, PURE_OS):
        return False
    parts = name.split(".")
    return parts[1] != "path" or (len(parts) > 2 and parts[2] not in PURE_OS_PATH)


def is_async_effect(name: str) -> bool:
    return (name.startswith("asyncio.") and name.rsplit(".", 1)[-1] in ASYNC_APIS) or name in {
        "threading.Thread",
        "multiprocessing.Process",
        "concurrent.futures.ThreadPoolExecutor",
        "concurrent.futures.ProcessPoolExecutor",
    }
