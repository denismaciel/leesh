from __future__ import annotations

from collections.abc import Iterable

from leesh.core import Finding, SourceFile, finding, loaded_names
from leesh.policy import INNER_LAYERS
from leesh.rules.shared.effects import is_dynamic


class DynamicCodeRule:
    """Core or port code runs code or loads modules chosen at run time.

    `eval`, `exec`, `compile`, `__import__`, `importlib.import_module`,
    `globals`, `locals`, `breakpoint`, and `pickle`, `marshal`, `shelve` and
    `runpy` execute or reach code no other rule can see, so one call undoes
    every purity rule. Import the module and call the function by name; parse
    serialized data at a boundary. `getattr` with a literal name is read as the
    attribute it spells, so `getattr(os, "environ")` is reported like
    `os.environ`.
    """

    code = "LIT208"
    name = "dynamic-code"

    def check(self, file: SourceFile) -> Iterable[Finding]:
        if not file.has_layer(*INNER_LAYERS):
            return []
        return [
            finding(
                self,
                file,
                node,
                f"{', '.join(sorted(names))} runs code chosen at run time; "
                "import and call what the core needs by name",
                target=", ".join(sorted(names)),
            )
            for node, names in loaded_names(file)
            if all(is_dynamic(name) for name in names)
        ]
