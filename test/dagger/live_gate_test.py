#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "dagger" / "scripts" / "live_gate.py"
SPEC = importlib.util.spec_from_file_location("live_gate", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {MODULE_PATH}")
LIVE_GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LIVE_GATE)


def sample(kind: str, status: int, latency_ms: int) -> dict[str, object]:
    return {
        "kind": kind,
        "status": status,
        "latency_ms": latency_ms,
        "error": None,
    }


class LiveGatePolicyTest(unittest.TestCase):
    def evaluate(
        self,
        samples: list[dict[str, object]],
        retention_deleted: int = 20_000,
    ) -> dict[str, object]:
        return LIVE_GATE.evaluate_receipt(
            samples=samples,
            query_budget_ms=500,
            report_budget_ms=750,
            seeded_errors=50_000,
            seeded_services=200,
            expected_retention_deletes=20_000,
            retention_deleted=retention_deleted,
        )

    def test_fast_concurrent_surface_passes(self) -> None:
        receipt = self.evaluate(
            [
                *[sample("query", 200, value) for value in range(20, 32)],
                *[sample("report", 200, value) for value in range(40, 52)],
                *[sample("ingest", 201, value) for value in range(10, 22)],
                *[sample("readyz", 200, value) for value in range(5, 17)],
            ]
        )
        self.assertEqual(receipt["status"], "ok")
        self.assertEqual(receipt["http_5xx"], 0)
        self.assertEqual(receipt["query"]["p95_ms"], 31)
        self.assertEqual(receipt["report"]["p95_ms"], 51)
        self.assertEqual(receipt["retention_prune"]["observed"], True)

    def test_one_tail_blip_does_not_redefine_p95(self) -> None:
        receipt = self.evaluate(
            [
                *[sample("query", 200, 100) for _ in range(19)],
                sample("query", 200, 900),
                *[sample("report", 200, 100) for _ in range(20)],
            ]
        )
        self.assertEqual(receipt["status"], "ok")

    def test_slow_p95_fails_the_latency_oracle(self) -> None:
        receipt = self.evaluate(
            [
                *[sample("query", 200, 100) for _ in range(18)],
                sample("query", 200, 900),
                sample("query", 200, 901),
                *[sample("report", 200, 100) for _ in range(20)],
            ]
        )
        self.assertEqual(receipt["status"], "failed")
        self.assertIn("query_p95_exceeded", receipt["failures"])

    def test_any_5xx_or_missing_retention_proof_fails(self) -> None:
        receipt = self.evaluate(
            [sample("query", 500, 10), sample("report", 200, 10)],
            retention_deleted=0,
        )
        self.assertEqual(receipt["status"], "failed")
        self.assertEqual(receipt["http_5xx"], 1)
        self.assertIn("http_5xx", receipt["failures"])
        self.assertIn("retention_prune_not_observed", receipt["failures"])

    def test_workload_interleaves_reads_writes_and_readiness(self) -> None:
        work = LIVE_GATE.interleaved_work(
            {"query": 2, "report": 2, "ingest": 3, "readyz": 2}
        )
        self.assertEqual(
            work[:8],
            [
                ("query", 0),
                ("report", 0),
                ("ingest", 0),
                ("readyz", 0),
                ("query", 1),
                ("report", 1),
                ("ingest", 1),
                ("readyz", 1),
            ],
        )
        self.assertEqual(work[-1], ("ingest", 2))

    def test_service_wait_requires_health_and_readiness(self) -> None:
        with mock.patch.object(
                LIVE_GATE,
                "read_json",
                side_effect=[
                    {"status": "ok"},
                    {"status": "not_ready"},
                    {"status": "ok"},
                    {"status": "ready"},
                ],
            ) as read_json:
            with mock.patch.object(LIVE_GATE.time, "sleep") as sleep:
                LIVE_GATE.wait_for_service("http://canary")

        self.assertEqual(read_json.call_count, 4)
        sleep.assert_called_once_with(0.25)


if __name__ == "__main__":
    unittest.main()
