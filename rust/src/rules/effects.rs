pub const WALL_CLOCK: &[&str] = &[
    "datetime.date.today",
    "datetime.datetime.now",
    "datetime.datetime.today",
    "datetime.datetime.utcnow",
    "time.time",
    "time.time_ns",
];
pub const ENVIRONMENT: &[&str] = &["os.environ", "os.environb", "os.getenv", "os.getenvb"];
pub const NONDETERMINISTIC: &[&str] = &[
    "datetime.date.today",
    "datetime.datetime.now",
    "datetime.datetime.today",
    "datetime.datetime.utcnow",
    "os.urandom",
    "time.monotonic",
    "time.monotonic_ns",
    "time.perf_counter",
    "time.perf_counter_ns",
    "time.time",
    "time.time_ns",
    "uuid.uuid1",
    "uuid.uuid4",
    "uuid.uuid6",
    "uuid.uuid7",
];
pub const RANDOM_APIS: &[&str] = &[
    "Random",
    "SystemRandom",
    "choice",
    "choices",
    "expovariate",
    "gauss",
    "getrandbits",
    "normalvariate",
    "randbytes",
    "randint",
    "random",
    "randrange",
    "sample",
    "seed",
    "shuffle",
    "triangular",
    "uniform",
];
pub const SECRETS_APIS: &[&str] = &[
    "SystemRandom",
    "choice",
    "randbelow",
    "randbits",
    "token_bytes",
    "token_hex",
    "token_urlsafe",
];
pub const PATH_EFFECTS: &[&str] = &[
    "absolute",
    "chmod",
    "cwd",
    "exists",
    "expanduser",
    "glob",
    "group",
    "hardlink_to",
    "home",
    "is_dir",
    "is_file",
    "is_symlink",
    "iterdir",
    "lstat",
    "mkdir",
    "open",
    "owner",
    "read_bytes",
    "read_text",
    "readlink",
    "rename",
    "replace",
    "resolve",
    "rglob",
    "rmdir",
    "stat",
    "symlink_to",
    "touch",
    "unlink",
    "walk",
    "write_bytes",
    "write_text",
];
pub const LOG_METHODS: &[&str] = &[
    "addHandler",
    "basicConfig",
    "critical",
    "debug",
    "error",
    "exception",
    "fatal",
    "getLogger",
    "get_logger",
    "info",
    "log",
    "removeHandler",
    "setLevel",
    "warn",
    "warning",
];
pub const ASYNC_APIS: &[&str] = &[
    "TaskGroup",
    "create_task",
    "ensure_future",
    "gather",
    "get_event_loop",
    "get_running_loop",
    "run",
    "sleep",
    "to_thread",
    "wait",
    "wait_for",
];
pub const AMBIENT: &[&str] = &[
    "contextvars.copy_context",
    "decimal.getcontext",
    "decimal.setcontext",
    "locale.getlocale",
    "locale.setlocale",
    "os.chdir",
    "os.getcwd",
    "os.getcwdb",
    "os.putenv",
    "os.unsetenv",
];
pub const IO_PACKAGES: &[&str] = &[
    "aiohttp",
    "aiosqlite",
    "asyncpg",
    "boto3",
    "botocore",
    "httpx",
    "playwright",
    "psycopg",
    "psycopg2",
    "redis",
    "requests",
    "socket",
    "sqlite3",
    "subprocess",
];
pub const TRANSACTION_PACKAGES: &[&str] = &[
    "aiomysql",
    "aiosqlite",
    "asyncpg",
    "django",
    "duckdb",
    "MySQLdb",
    "peewee",
    "psycopg",
    "psycopg2",
    "psycopg_pool",
    "pymongo",
    "pymysql",
    "sqlalchemy",
    "sqlite3",
    "sqlmodel",
    "tortoise",
];
pub const PURE_OS: &[&str] = &[
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
];
pub const PURE_OS_PATH: &[&str] = &[
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
];
pub const PURE_SYS: &[&str] = &[
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
];
pub const DYNAMIC: &[&str] = &[
    "builtins.__import__",
    "builtins.breakpoint",
    "builtins.compile",
    "builtins.delattr",
    "builtins.eval",
    "builtins.exec",
    "builtins.globals",
    "builtins.locals",
    "builtins.setattr",
    "builtins.vars",
    "importlib.__import__",
    "importlib.import_module",
    "importlib.reload",
];
fn under(name: &str, roots: &[&str]) -> bool {
    roots.iter().any(|root| {
        name.strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}
/// Core and port code may not touch `time` at all: sleeping and local time included.
pub fn impure_in_core(name: &str) -> bool {
    nondeterministic(name) || name.starts_with("time.") && !under(name, &["time.struct_time"])
}
pub fn dynamic(name: &str) -> bool {
    DYNAMIC.contains(&name)
        || ["marshal.", "pickle.", "runpy.", "shelve."]
            .iter()
            .any(|p| name.starts_with(p))
}
/// Environment, ambient state and randomness under `os` belong to other rules.
fn impure_os(name: &str) -> bool {
    if !name.starts_with("os.") || environment(name) || ambient(name) {
        return false;
    }
    if name == "os.urandom" || under(name, PURE_OS) {
        return false;
    }
    let mut parts = name.split('.').skip(1);
    parts.next() != Some("path") || parts.next().is_some_and(|p| !PURE_OS_PATH.contains(&p))
}
pub fn nondeterministic(name: &str) -> bool {
    let (module, member) = name.rsplit_once('.').unwrap_or(("", name));
    NONDETERMINISTIC.contains(&name)
        || module == "random" && RANDOM_APIS.contains(&member)
        || module == "secrets" && SECRETS_APIS.contains(&member)
}
pub fn environment(name: &str) -> bool {
    ENVIRONMENT.contains(&name)
        || name.starts_with("os.environ.")
        || name.starts_with("os.environb.")
}
pub fn ambient(name: &str) -> bool {
    AMBIENT.contains(&name)
        || [
            "contextvars.ContextVar.get",
            "contextvars.ContextVar.set",
            "contextvars.ContextVar.reset",
        ]
        .contains(&name)
}
pub fn asynchronous(name: &str) -> bool {
    name.starts_with("asyncio.") && ASYNC_APIS.contains(&name.rsplit('.').next().unwrap_or(""))
        || [
            "threading.Thread",
            "multiprocessing.Process",
            "concurrent.futures.ThreadPoolExecutor",
            "concurrent.futures.ProcessPoolExecutor",
        ]
        .contains(&name)
}
pub fn io(name: &str) -> bool {
    let (module, member) = name.rsplit_once('.').unwrap_or(("", name));
    IO_PACKAGES.contains(&name.split('.').next().unwrap_or(""))
        || [
            "builtins.open",
            "builtins.print",
            "builtins.input",
            "io.open",
            "io.FileIO",
            "io.open_code",
        ]
        .contains(&name)
        || ["pathlib.Path", "pathlib.PosixPath", "pathlib.WindowsPath"].contains(&module)
            && PATH_EFFECTS.contains(&member)
        || [
            "logging",
            "logging.Logger",
            "logging.LoggerAdapter",
            "structlog",
            "structlog.BoundLogger",
        ]
        .contains(&module)
            && LOG_METHODS.contains(&member)
        || ["urllib.request.", "shutil.", "tempfile."]
            .iter()
            .any(|p| name.starts_with(p))
        || impure_os(name)
        || name.starts_with("sys.") && !under(name, PURE_SYS)
}
