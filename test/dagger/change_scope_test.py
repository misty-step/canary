#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CLASSIFIER = ROOT / "dagger" / "scripts" / "change_scope.py"


class ChangeScopeTest(unittest.TestCase):
    def classify(self, files: dict[str, str | None]) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base"
            candidate = Path(tmp) / "candidate"
            base.mkdir()
            candidate.mkdir()
            for relative, candidate_contents in files.items():
                base_path = base / relative
                base_path.parent.mkdir(parents=True, exist_ok=True)
                base_path.write_text("base\n")
                if candidate_contents is not None:
                    candidate_path = candidate / relative
                    candidate_path.parent.mkdir(parents=True, exist_ok=True)
                    candidate_path.write_text(candidate_contents)

            completed = subprocess.run(
                [
                    "python3",
                    str(CLASSIFIER),
                    "--base",
                    str(base),
                    "--candidate",
                    str(candidate),
                ],
                check=True,
                text=True,
                capture_output=True,
            )
            return json.loads(completed.stdout)

    def test_docs_only_diff_skips_runtime_lanes(self) -> None:
        result = self.classify(
            {
                "README.md": "candidate\n",
                "docs/operations.md": "candidate\n",
            }
        )
        self.assertEqual(result["schema"], "canary.ci-change-scope.v1")
        self.assertEqual(result["runtime_required"], False)
        self.assertEqual(
            result["changed_paths"], ["README.md", "docs/operations.md"]
        )

    def test_runtime_and_mixed_diffs_run_every_lane(self) -> None:
        for changed in (
            {"crates/canary-server/src/lib.rs": "candidate\n"},
            {"Dockerfile": "candidate\n"},
            {
                "docs/operations.md": "candidate\n",
                "crates/canary-store/src/lib.rs": "candidate\n",
            },
            {"docs/operations.md": None},
            {"crates/canary-server/src/lib.rs": None},
        ):
            with self.subTest(changed=changed):
                self.assertEqual(self.classify(changed)["runtime_required"], True)

    def test_unknown_or_empty_diff_fails_closed(self) -> None:
        self.assertEqual(self.classify({})["runtime_required"], True)
        self.assertEqual(
            self.classify({"config/example.toml": "candidate\n"})[
                "runtime_required"
            ],
            True,
        )


if __name__ == "__main__":
    unittest.main()
