from __future__ import annotations

import ast
from collections.abc import Iterable, Mapping

from leesh.core import Finding, SourceFile, finding
from leesh.policy import INNER_LAYERS
from leesh.rules.shared.annotations import (
    annotation_nodes,
    has_string_annotations,
    parsed_annotations,
)

MUTABLE_TYPES = frozenset(
    {
        "builtins.bytearray",
        "builtins.dict",
        "builtins.list",
        "builtins.set",
        "collections.ChainMap",
        "collections.Counter",
        "collections.OrderedDict",
        "collections.abc.MutableMapping",
        "collections.abc.MutableSequence",
        "collections.abc.MutableSet",
        "collections.defaultdict",
        "collections.deque",
        "typing.Counter",
        "typing.DefaultDict",
        "typing.Deque",
        "typing.Dict",
        "typing.List",
        "typing.MutableMapping",
        "typing.MutableSequence",
        "typing.MutableSet",
        "typing.OrderedDict",
        "typing.Set",
    }
)
SETATTR = (
    frozenset({"builtins.object.__setattr__"}),
    frozenset({"builtins.object.__delattr__"}),
)
MUTABLE_FIELD = (
    "frozen records must not expose mutable collection fields; "
    "use tuples, frozensets, or a read-only collection interface"
)


def _mutable_factory(value: ast.expr | None, symbols: Mapping[ast.expr, frozenset[str]]) -> bool:
    """`field(default_factory=list)` builds a mutable value whatever the annotation says."""
    if not isinstance(value, ast.Call) or symbols.get(value.func) != frozenset(
        {"dataclasses.field"}
    ):
        return False
    return any(
        keyword.arg == "default_factory"
        and (
            bool(symbols.get(keyword.value, frozenset()) & MUTABLE_TYPES)
            or (
                isinstance(keyword.value, ast.Lambda)
                and isinstance(
                    keyword.value.body,
                    (ast.List, ast.Dict, ast.Set, ast.ListComp, ast.DictComp, ast.SetComp),
                )
            )
        )
        for keyword in value.keywords
    )


class FrozenValueRule:
    """A core or port record is mutable.

    Dataclasses must be declared with `frozen=True`. Their fields, and those
    of a `NamedTuple`, may not be annotated with mutable collections (`list`,
    `dict`, `set`, `bytearray`, `deque`, `defaultdict`, `OrderedDict`,
    `Counter`, `ChainMap`, or the `typing` and `collections.abc` mutable
    protocols), in string annotations too, or built by a `default_factory`
    that returns one. `object.__setattr__(self, ...)` writes to a frozen record
    and is reported outside `__post_init__`, where it sets derived fields.
    `ClassVar` fields are exempt. Use tuples, frozensets, or read-only
    `Mapping`/`Sequence` interfaces.
    """

    code = "LIT205"
    name = "frozen-values"

    def check(self, file: SourceFile) -> Iterable[Finding]:
        if not file.has_layer(*INNER_LAYERS):
            return []
        if has_string_annotations(file.tree, ast.AnnAssign):
            tree, symbols = parsed_annotations(file, ast.AnnAssign)
        else:
            tree, symbols = file.tree, file.symbols
        findings: list[Finding] = []
        initializing = {
            id(part)
            for node in ast.walk(tree)
            if isinstance(node, ast.FunctionDef) and node.name == "__post_init__"
            for part in ast.walk(node)
        }
        for node in ast.walk(tree):
            if (
                isinstance(node, ast.Call)
                and id(node) not in initializing
                and symbols.get(node.func) in SETATTR
                and node.args
                and isinstance(node.args[0], ast.Name)
                and node.args[0].id == "self"
            ):
                findings.append(
                    finding(
                        self,
                        file,
                        node,
                        "object.__setattr__ writes to a frozen record; "
                        "return a replacement value instead",
                    )
                )
            if not isinstance(node, ast.ClassDef):
                continue
            decorators = [
                decorator
                for decorator in node.decorator_list
                if symbols.get(decorator.func if isinstance(decorator, ast.Call) else decorator)
                == frozenset({"dataclasses.dataclass"})
            ]
            named_tuple = any(
                symbols.get(base) == frozenset({"typing.NamedTuple"}) for base in node.bases
            )
            if not decorators and not named_tuple:
                continue
            if decorators and not any(
                isinstance(decorator, ast.Call)
                and any(
                    keyword.arg == "frozen"
                    and isinstance(keyword.value, ast.Constant)
                    and keyword.value.value is True
                    for keyword in decorator.keywords
                )
                for decorator in decorators
            ):
                findings.append(
                    finding(
                        self,
                        file,
                        node,
                        (
                            "core dataclasses must be frozen; "
                            "return a replacement value for transitions"
                        ),
                    )
                )
            for statement in node.body:
                if not isinstance(statement, ast.AnnAssign):
                    continue
                annotation = statement.annotation
                if isinstance(annotation, ast.Subscript) and symbols.get(
                    annotation.value
                ) == frozenset({"typing.ClassVar"}):
                    continue
                if any(
                    symbols.get(item, frozenset()) & MUTABLE_TYPES
                    for item in annotation_nodes(annotation, symbols)
                ) or _mutable_factory(statement.value, symbols):
                    findings.append(finding(self, file, statement, MUTABLE_FIELD))
        return findings
