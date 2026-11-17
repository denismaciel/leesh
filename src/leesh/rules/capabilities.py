from __future__ import annotations

import ast
from collections.abc import Iterable

from leesh.core import Finding, SourceFile, finding
from leesh.policy import Layer
from leesh.rules.shared.annotations import annotation_nodes, parsed_annotations

# Exact public type identities, never suffix or parameter-name heuristics.
EFFECTFUL_TYPES = frozenset(
    {
        "sqlalchemy.orm.Session",
        "sqlalchemy.orm.session.Session",
        "sqlalchemy.ext.asyncio.AsyncSession",
        "sqlalchemy.ext.asyncio.session.AsyncSession",
        "sqlalchemy.engine.Connection",
        "sqlalchemy.engine.Engine",
        "sqlite3.Connection",
        "sqlite3.Cursor",
        "psycopg.Connection",
        "psycopg.AsyncConnection",
        "httpx.Client",
        "httpx.AsyncClient",
        "requests.Session",
        "requests.sessions.Session",
        "aiohttp.ClientSession",
        "aiohttp.client.ClientSession",
        "io.IOBase",
        "io.RawIOBase",
        "io.BufferedIOBase",
        "io.TextIOBase",
        "io.TextIOWrapper",
        "typing.IO",
        "typing.TextIO",
        "typing.BinaryIO",
        "fsspec.AbstractFileSystem",
        "fsspec.spec.AbstractFileSystem",
    }
)


class EffectfulCapabilityRule:
    """A core function takes a parameter that can perform effects.

    Database sessions and connections, HTTP clients, file objects and
    filesystems are capabilities: a function that receives one can do I/O even
    when it imports nothing. Types are matched by exact resolved name, through
    aliases, unions, `Optional`, `Annotated` and string annotations. Add your own
    with `workspace.effectful_types`. Ports are exempt because they describe
    effectful collaborators.
    """

    code = "LIT207"
    name = "effectful-capability"

    def check(self, file: SourceFile) -> Iterable[Finding]:
        if not file.has_layer(Layer.CORE):
            return []
        tree, symbols = parsed_annotations(file, ast.arg)
        known = EFFECTFUL_TYPES | file.workspace.policy.effectful_types
        findings: list[Finding] = []
        for node in ast.walk(tree):
            if not isinstance(node, ast.arg) or node.annotation is None:
                continue
            matches = {
                name
                for part in annotation_nodes(node.annotation, symbols)
                if (names := symbols.get(part, frozenset())) and names <= known
                for name in names
            }
            if matches:
                findings.append(
                    finding(
                        self,
                        file,
                        node,
                        f"core parameter '{node.arg}' receives effectful capability "
                        f"{', '.join(sorted(matches))}; "
                        "pass observed values and execute returned decisions in the shell",
                    )
                )
        return findings
