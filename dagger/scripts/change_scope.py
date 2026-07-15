#!/usr/bin/env python3
"""Classify a candidate tree against a trusted base for strict CI.

The classifier is intentionally fail-closed: only an all-documentation diff may
skip runtime and production-image lanes. Unknown paths, deletions, mixed diffs,
and an empty diff all run the full gate.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


IGNORED_PARTS = {
    ".git",
    "node_modules",
    "target",
    "coverage",
    "dist",
    "_build",
    "deps",
    "cover",
}
ROOT_DOCS = {
    "AGENTS.md",
    "ARCHITECTURE.md",
    "CHANGELOG.md",
    "CLAUDE.md",
    "CODE_OF_CONDUCT.md",
    "CONTRIBUTING.md",
    "LICENSE",
    "README.md",
    "SECURITY.md",
    "VISION.md",
}


def file_digests(root: Path) -> dict[str, str]:
    digests: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(root)
        if any(part in IGNORED_PARTS for part in relative.parts):
            continue
        digests[relative.as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return digests


def documentation_path(path: str) -> bool:
    if path in ROOT_DOCS:
        return True
    return path.startswith("docs/")


def classify(base: Path, candidate: Path) -> dict[str, object]:
    base_files = file_digests(base)
    candidate_files = file_digests(candidate)
    changed = sorted(
        path
        for path in base_files.keys() | candidate_files.keys()
        if base_files.get(path) != candidate_files.get(path)
    )
    deleted = base_files.keys() - candidate_files.keys()
    docs_only = (
        bool(changed)
        and not deleted
        and all(documentation_path(path) for path in changed)
    )
    runtime_required = not docs_only
    reason = "documentation_only" if docs_only else "full_gate_fail_closed"
    return {
        "schema": "canary.ci-change-scope.v1",
        "changed_paths": changed,
        "runtime_required": runtime_required,
        "reason": reason,
        "skipped_lanes": (
            []
            if runtime_required
            else ["rust_quality", "rust_coverage", "rust_advisories", "production_image"]
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    args = parser.parse_args()
    if not args.base.is_dir() or not args.candidate.is_dir():
        parser.error("--base and --candidate must name directories")
    print(json.dumps(classify(args.base, args.candidate), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
