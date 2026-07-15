#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "dagger" / "scripts" / "sync_source_arguments.py"
SPEC = importlib.util.spec_from_file_location("sync_source_arguments", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
SYNC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SYNC)


class DirectoryArgumentPolicyTest(unittest.TestCase):
    def test_live_dagger_arguments_match_declared_policies(self) -> None:
        source = (ROOT / "dagger" / "src" / "index.ts").read_text()
        self.assertEqual(SYNC.sync_source_arguments(source), source)

    def test_new_directory_authorities_cannot_drop_upload_exclusions(self) -> None:
        source = (ROOT / "dagger" / "src" / "index.ts").read_text()
        block = SYNC.render_auxiliary_argument("base", "without_git")
        mutant = source.replace(block, "    base?: Directory,", 1)
        with self.assertRaisesRegex(
            RuntimeError,
            r"strict\(\) base Directory argument must use the canonical without_git",
        ):
            SYNC.sync_source_arguments(mutant)


if __name__ == "__main__":
    unittest.main()
