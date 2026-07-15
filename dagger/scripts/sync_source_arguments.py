#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

ROOT = Path(
    os.environ.get("CANARY_CI_SOURCE_ROOT", Path(__file__).resolve().parents[2])
)
INDEX_PATH = ROOT / "dagger/src/index.ts"

COMMON_IGNORE = [
    "_build",
    "deps",
    "cover",
    "clients/typescript/node_modules",
    "clients/typescript/dist",
    "clients/typescript/coverage",
    "dagger/node_modules",
    "target",
]

SOURCE_POLICIES = {
    "without_git": [".git", *COMMON_IGNORE],
    "with_git": COMMON_IGNORE,
}

FUNCTION_POLICIES = {
    "fast": "without_git",
    "advisories": "without_git",
    "strict": "with_git",
    "codexAgentRoles": "without_git",
    "deterministic": "without_git",
    "ciContract": "without_git",
    "openapiContract": "without_git",
    "typescriptQuality": "without_git",
    "rustQuality": "without_git",
    "rustCoverage": "without_git",
    "productionImageSmoke": "without_git",
    "changeScope": "without_git",
    "productionImageAlertPlaneRehearsal": "without_git",
    "rustAdvisories": "without_git",
    "secrets": "with_git",
    "secretsHistory": "with_git",
}

AUXILIARY_ARGUMENT_POLICIES = {
    ("strict", "base"): "without_git",
    ("deterministic", "policy"): "without_git",
    ("rustCoverage", "policy"): "without_git",
    ("changeScope", "base"): "without_git",
}

ARGUMENT_BLOCK_PATTERN = re.compile(
    r'(async\s+(?P<name>\w+)\(\s+@argument\(\{\s+defaultPath: "/",\s+ignore: \[\n)(?P<body>.*?)(?P<suffix>\n(?:\s*\n)*\s+\],\s+\}\)\s+source\?: Directory,)',
    re.DOTALL,
)
ARGUMENT_SUFFIX = "\n      ],\n    })\n    source?: Directory,"


def render_ignore_block(policy_name: str) -> str:
    entries = SOURCE_POLICIES[policy_name]
    return "\n".join(f'        "{entry}",' for entry in entries)


def render_auxiliary_argument(argument_name: str, policy_name: str) -> str:
    return "\n".join(
        [
            "    @argument({",
            "      ignore: [",
            render_ignore_block(policy_name),
            "      ],",
            "    })",
            f"    {argument_name}?: Directory,",
        ]
    )


def method_parameters(source_text: str, method_name: str) -> str:
    match = re.search(rf"\basync\s+{re.escape(method_name)}\s*\(", source_text)
    if match is None:
        raise RuntimeError(f"{INDEX_PATH}: missing async {method_name}()")
    start = match.end() - 1
    depth = 0
    for index in range(start, len(source_text)):
        character = source_text[index]
        if character == "(":
            depth += 1
        elif character == ")":
            depth -= 1
            if depth == 0:
                return source_text[start + 1 : index]
    raise RuntimeError(f"{INDEX_PATH}: unterminated parameters for async {method_name}()")


def validate_auxiliary_arguments(source_text: str) -> None:
    for (method_name, argument_name), policy_name in AUXILIARY_ARGUMENT_POLICIES.items():
        expected = render_auxiliary_argument(argument_name, policy_name)
        parameters = method_parameters(source_text, method_name)
        if expected not in parameters:
            raise RuntimeError(
                f"{INDEX_PATH}: {method_name}() {argument_name} Directory argument "
                f"must use the canonical {policy_name} ignore policy"
            )


def sync_source_arguments(source_text: str) -> str:
    seen: list[str] = []

    def replace(match: re.Match[str]) -> str:
        name = match.group("name")
        seen.append(name)

        try:
            policy_name = FUNCTION_POLICIES[name]
        except KeyError as exc:
            raise RuntimeError(
                f"{INDEX_PATH}: no source policy configured for async {name}()",
            ) from exc

        return "".join(
            [
                match.group(1),
                render_ignore_block(policy_name),
                ARGUMENT_SUFFIX,
            ]
        )

    updated = ARGUMENT_BLOCK_PATTERN.sub(replace, source_text)
    seen_set = set(seen)
    expected_set = set(FUNCTION_POLICIES)
    missing = sorted(expected_set - seen_set)
    unexpected = sorted(seen_set - expected_set)

    if missing or unexpected:
        problems: list[str] = []
        if missing:
            problems.append(f"missing functions in dagger/src/index.ts: {', '.join(missing)}")
        if unexpected:
            problems.append(f"unmapped functions in dagger/src/index.ts: {', '.join(unexpected)}")
        raise RuntimeError("; ".join(problems))

    validate_auxiliary_arguments(updated)

    return updated


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Sync Dagger Directory @argument ignore literals from the canonical "
            "policy table in this script."
        )
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check",
        action="store_true",
        help="Fail if dagger/src/index.ts is out of sync.",
    )
    mode.add_argument(
        "--write",
        action="store_true",
        help="Rewrite dagger/src/index.ts in place.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    original = INDEX_PATH.read_text()
    try:
        updated = sync_source_arguments(original)
    except RuntimeError as error:
        print(error, file=sys.stderr)
        return 1

    if args.write:
        if updated != original:
            INDEX_PATH.write_text(updated)
            print(f"updated {INDEX_PATH.relative_to(ROOT)}")
        return 0

    if updated != original:
        print(
            f"{INDEX_PATH.relative_to(ROOT)} is out of sync with dagger/scripts/sync_source_arguments.py",
            file=sys.stderr,
        )
        return 1

    if args.check:
        print(f"{INDEX_PATH.relative_to(ROOT)} is in sync")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
