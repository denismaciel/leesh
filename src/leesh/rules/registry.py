from __future__ import annotations

from collections.abc import Mapping
from types import MappingProxyType

from leesh.core import Rule
from leesh.rules.ambient_state import AmbientStateRule
from leesh.rules.async_core import AsyncCoreRule
from leesh.rules.borrowed_mutation import BorrowedMutationRule
from leesh.rules.capabilities import EffectfulCapabilityRule
from leesh.rules.client_imports import ClientImportRule
from leesh.rules.defaults import ImmutableDefaultRule
from leesh.rules.domains import DomainDependencyRule
from leesh.rules.environment import EnvironmentAccessRule
from leesh.rules.forbidden_calls import ForbiddenCallRule
from leesh.rules.forbidden_imports import ForbiddenImportRule
from leesh.rules.frozen_values import FrozenValueRule
from leesh.rules.io_calls import IoCallRule
from leesh.rules.layer_dependencies import LayerDependencyRule
from leesh.rules.meta import ManifestRule, ParseErrorRule, UnclassifiedFileRule
from leesh.rules.mocking import MockingRule
from leesh.rules.nondeterminism import NondeterminismRule
from leesh.rules.outcomes import ExpectedOutcomeRule
from leesh.rules.policy_types import BooleanPolicyRule
from leesh.rules.routers import RouterConstructionRule
from leesh.rules.subclassing import SubclassingRule
from leesh.rules.transactions import TransactionBoundaryRule

PARSE_ERROR = ParseErrorRule()
MANIFEST = ManifestRule()

ALL_RULES: tuple[Rule, ...] = (
    PARSE_ERROR,
    UnclassifiedFileRule(),
    MANIFEST,
    LayerDependencyRule(),
    ForbiddenImportRule(),
    ClientImportRule(),
    DomainDependencyRule(),
    NondeterminismRule(),
    IoCallRule(),
    AmbientStateRule(),
    BorrowedMutationRule(),
    FrozenValueRule(),
    AsyncCoreRule(),
    EffectfulCapabilityRule(),
    EnvironmentAccessRule(),
    TransactionBoundaryRule(),
    RouterConstructionRule(),
    MockingRule(),
    ImmutableDefaultRule(),
    ForbiddenCallRule(),
    BooleanPolicyRule(),
    ExpectedOutcomeRule(),
    SubclassingRule(),
)

RULES_BY_CODE: Mapping[str, Rule] = MappingProxyType({rule.code: rule for rule in ALL_RULES})
