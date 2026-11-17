from __future__ import annotations

import ast
from collections.abc import Iterable, Mapping
from copy import deepcopy

from leesh.core import SourceFile
from leesh.symbols import resolved_symbols


class _AnnotationStrings(ast.NodeTransformer):
    def visit_Constant(self, node: ast.Constant) -> ast.AST:
        if isinstance(node.value, str):
            try:
                parsed = ast.parse(node.value, mode="eval").body
            except SyntaxError:
                return node
            # Avoid recursion on quoted string literals.
            if not isinstance(parsed, ast.Constant):
                rewritten = self.visit(parsed)
                if isinstance(rewritten, ast.AST):
                    return ast.copy_location(rewritten, node)
        return node


def parsed_annotations(
    file: SourceFile, owner: type[ast.arg] | type[ast.AnnAssign]
) -> tuple[ast.Module, Mapping[ast.expr, frozenset[str]]]:
    """A private tree with forward annotations parsed; snapshot ASTs stay untouched."""
    tree = deepcopy(file.tree)
    for node in ast.walk(tree):
        if isinstance(node, owner) and node.annotation is not None:
            node.annotation = _AnnotationStrings().visit(node.annotation)
    return tree, resolved_symbols(tree, file.package)


def has_string_annotations(tree: ast.Module, owner: type[ast.arg] | type[ast.AnnAssign]) -> bool:
    return any(
        isinstance(part, ast.Constant) and isinstance(part.value, str)
        for node in ast.walk(tree)
        if isinstance(node, owner) and node.annotation is not None
        for part in ast.walk(node.annotation)
    )


def annotation_nodes(
    node: ast.AST, symbols: Mapping[ast.expr, frozenset[str]]
) -> Iterable[ast.expr]:
    """Type expressions in an annotation: not `Literal` values or `Annotated` metadata."""
    if isinstance(node, ast.expr):
        yield node
    if isinstance(node, ast.Subscript):
        names = symbols.get(node.value, frozenset())
        if names & {"typing.Literal", "typing_extensions.Literal"}:
            return
        if names & {"typing.Annotated", "typing_extensions.Annotated"}:
            first = node.slice.elts[0] if isinstance(node.slice, ast.Tuple) else node.slice
            yield from annotation_nodes(first, symbols)
            return
    for child in ast.iter_child_nodes(node):
        yield from annotation_nodes(child, symbols)
