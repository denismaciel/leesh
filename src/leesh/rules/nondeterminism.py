from __future__ import annotations

from collections.abc import Iterable

from leesh.core import Finding, SourceFile, finding, loaded_names
from leesh.policy import INNER_LAYERS, Layer
from leesh.rules.shared.effects import WALL_CLOCK, is_impure_in_core, is_nondeterministic


class NondeterminismRule:
    """Code reads the clock, randomness, or fresh IDs directly.

    Core and port modules may not read any nondeterministic source
    (`datetime.now`, `uuid4`, `random`, `secrets`, `os.urandom`) or use the
    `time` module at all, `time.sleep` and `time.localtime` included; pass the
    observed value in. Elsewhere only wall-clock reads are reported, so time
    flows through one clock you can control in tests. List that clock's modules
    in `workspace.clock_modules`; test modules are exempt. A custom layer with
    `deterministic = true`, such as Temporal workflow code, is held to the core
    standard.
    """

    code = "LIT201"
    name = "nondeterminism"

    def check(self, file: SourceFile) -> Iterable[Finding]:
        inner = file.has_layer(*INNER_LAYERS) or file.is_deterministic
        hidden = is_impure_in_core if file.has_layer(*INNER_LAYERS) else is_nondeterministic
        if not inner and (
            file.layer in (None, Layer.TEST) or file.module in file.workspace.policy.clock_modules
        ):
            return []
        return [
            finding(
                self,
                file,
                node,
                f"{', '.join(sorted(names))} hides nondeterminism; "
                f"{file.layer_name} modules must be deterministic"
                if file.is_deterministic
                else f"{', '.join(sorted(names))} hides nondeterminism; "
                "pass the observed time, ID, or random value into the core"
                if inner
                else f"{', '.join(sorted(names))} reads wall time directly; "
                "use the clock listed in workspace.clock_modules",
                target=", ".join(sorted(names)),
            )
            for node, names in loaded_names(file)
            if all(hidden(name) if inner else name in WALL_CLOCK for name in names)
        ]
