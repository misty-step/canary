#!/usr/bin/env python3
"""Seed and exercise Canary's production image under representative load."""

from __future__ import annotations

import argparse
import concurrent.futures
import itertools
import json
import math
import os
import sqlite3
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def percentile(values: list[int], percentile_value: int) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, math.ceil((percentile_value / 100) * len(ordered)) - 1)
    return ordered[index]


def evaluate_receipt(
    *,
    samples: list[dict[str, object]],
    query_budget_ms: int,
    report_budget_ms: int,
    seeded_errors: int,
    seeded_services: int,
    expected_retention_deletes: int,
    retention_deleted: int,
) -> dict[str, object]:
    by_kind: dict[str, list[dict[str, object]]] = {}
    for sample in samples:
        by_kind.setdefault(str(sample["kind"]), []).append(sample)

    def latency_summary(kind: str, budget_ms: int) -> dict[str, object]:
        latencies = [int(sample["latency_ms"]) for sample in by_kind.get(kind, [])]
        return {
            "samples": len(latencies),
            "p95_ms": percentile(latencies, 95),
            "max_ms": max(latencies) if latencies else None,
            "budget_ms": budget_ms,
        }

    query = latency_summary("query", query_budget_ms)
    report = latency_summary("report", report_budget_ms)
    http_5xx = sum(1 for sample in samples if int(sample["status"]) >= 500)
    http_errors = sum(
        1
        for sample in samples
        if sample.get("error") is not None
        or int(sample["status"]) < 200
        or int(sample["status"]) >= 300
    )
    failures: list[str] = []
    if query["samples"] == 0 or int(query["p95_ms"] or 0) > query_budget_ms:
        failures.append("query_p95_exceeded")
    if report["samples"] == 0 or int(report["p95_ms"] or 0) > report_budget_ms:
        failures.append("report_p95_exceeded")
    if http_5xx:
        failures.append("http_5xx")
    if http_errors:
        failures.append("http_errors")
    retention_observed = retention_deleted >= expected_retention_deletes
    if not retention_observed:
        failures.append("retention_prune_not_observed")

    return {
        "schema": "canary.live-gate.v1",
        "status": "ok" if not failures else "failed",
        "seed": {
            "current_errors": seeded_errors,
            "services": seeded_services,
            "expired_errors": expected_retention_deletes,
        },
        "query": query,
        "report": report,
        "http_5xx": http_5xx,
        "http_errors": http_errors,
        "retention_prune": {
            "expected_minimum_deleted": expected_retention_deletes,
            "deleted": retention_deleted,
            "observed": retention_observed,
        },
        "failures": failures,
        "samples": samples,
    }


def seed_database(
    database: Path,
    current_errors: int,
    expired_errors: int,
    services: int,
) -> dict[str, object]:
    now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    expired_at = "2000-01-01T00:00:00Z"
    connection = sqlite3.connect(str(database))
    try:
        connection.execute("PRAGMA synchronous=OFF")
        rows = []
        groups = []
        total = current_errors + expired_errors
        for index in range(total):
            service = f"gate-seed-{index % services:03d}"
            group_hash = f"sha256:{index:064x}"
            error_id = f"ERR-GATE{index:012d}"
            created_at = now if index < current_errors else expired_at
            error_class = f"GateError{index % 250:03d}"
            message = f"production-shaped gate error {index}"
            groups.append(
                (
                    group_hash,
                    service,
                    error_class,
                    message,
                    "error",
                    created_at,
                    created_at,
                    error_id,
                )
            )
            rows.append(
                (
                    error_id,
                    service,
                    error_class,
                    message,
                    message,
                    group_hash,
                    created_at,
                )
            )
        with connection:
            connection.executemany(
                """
                INSERT INTO error_groups (
                    group_hash, service, error_class, message_template, severity,
                    first_seen_at, last_seen_at, last_error_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                groups,
            )
            connection.executemany(
                """
                INSERT INTO errors (
                    id, service, error_class, message, message_template,
                    group_hash, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                rows,
            )
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchall()
    finally:
        connection.close()
    return {
        "schema": "canary.live-gate-seed.v1",
        "database": str(database),
        "current_errors": current_errors,
        "expired_errors": expired_errors,
        "services": services,
    }


def request_sample(
    endpoint: str,
    api_keys: dict[str, str],
    kind: str,
    ordinal: int,
    seeded_services: int,
) -> dict[str, object]:
    api_key = api_keys.get(kind, api_keys["default"])
    headers = {"Authorization": f"Bearer {api_key}"}
    method = "GET"
    body = None
    if kind == "query":
        path = (
            "/api/v1/query?service="
            f"gate-seed-{ordinal % seeded_services:03d}&window=1h"
        )
    elif kind == "report":
        path = "/api/v1/report?window=1h&limit=50"
    elif kind == "readyz":
        path = "/readyz"
        headers = {}
    elif kind == "ingest":
        path = "/api/v1/errors"
        method = "POST"
        headers["Content-Type"] = "application/json"
        body = json.dumps(
            {
                "service": f"gate-live-{ordinal % 8:02d}",
                "error_class": "GateConcurrentError",
                "message": f"concurrent gate request {ordinal}",
                "severity": "error",
                "fingerprint": ["canary-live-gate", str(ordinal)],
            }
        ).encode()
    else:
        raise ValueError(f"unknown sample kind: {kind}")

    started = time.monotonic()
    status = 0
    error = None
    try:
        request = urllib.request.Request(
            f"{endpoint}{path}", data=body, headers=headers, method=method
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            status = response.status
            response.read()
    except urllib.error.HTTPError as exc:
        status = exc.code
        error = f"HTTP {exc.code}"
        exc.read()
    except Exception as exc:  # pragma: no cover - exercised by live failures
        error = str(exc)
    latency_ms = max(1, math.ceil((time.monotonic() - started) * 1000))
    return {
        "kind": kind,
        "status": status,
        "latency_ms": latency_ms,
        "error": error,
    }


def read_json(endpoint: str, path: str, timeout: float = 10) -> dict[str, Any]:
    with urllib.request.urlopen(f"{endpoint}{path}", timeout=timeout) as response:
        return json.load(response)


def wait_for_service(endpoint: str) -> None:
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        try:
            healthy = read_json(endpoint, "/healthz", 2).get("status") == "ok"
            ready = read_json(endpoint, "/readyz", 2).get("status") == "ready"
            if healthy and ready:
                return
        except Exception:
            pass
        time.sleep(0.25)
    raise RuntimeError("timed out waiting for production image readiness")


def interleaved_work(sample_counts: dict[str, int]) -> list[tuple[str, int]]:
    lanes = [
        [(kind, index) for index in range(count)]
        for kind, count in sample_counts.items()
    ]
    return [
        item
        for group in itertools.zip_longest(*lanes)
        for item in group
        if item is not None
    ]


def retention_deleted(endpoint: str, expected: int) -> int:
    deadline = time.monotonic() + 90
    observed = 0
    while time.monotonic() < deadline:
        try:
            ready = read_json(endpoint, "/readyz", 2)
            workers = ready.get("checks", {}).get("workers", [])
            worker = next(
                item for item in workers if item.get("name") == "retention_prune"
            )
            observed = int(worker.get("due_count", 0))
            if worker.get("last_success_at") and observed >= expected:
                return observed
        except Exception:
            pass
        time.sleep(0.25)
    return observed


def run_live_gate(args: argparse.Namespace) -> dict[str, object]:
    api_key = os.environ.get("CANARY_API_KEY", "")
    if not api_key:
        raise RuntimeError("CANARY_API_KEY is required")
    api_keys = {
        "default": api_key,
        "report": os.environ.get("CANARY_REPORT_API_KEY", api_key),
    }
    endpoint = args.endpoint.rstrip("/")
    wait_for_service(endpoint)
    work = interleaved_work(
        {
            "query": args.query_samples,
            "report": args.report_samples,
            "ingest": args.ingest_samples,
            "readyz": args.readyz_samples,
        }
    )
    samples: list[dict[str, object]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = [
            pool.submit(
                request_sample,
                endpoint,
                api_keys,
                kind,
                ordinal,
                args.seeded_services,
            )
            for kind, ordinal in work
        ]
        for future in concurrent.futures.as_completed(futures):
            samples.append(future.result())
    deleted = retention_deleted(endpoint, args.expected_retention_deletes)
    return evaluate_receipt(
        samples=samples,
        query_budget_ms=args.query_budget_ms,
        report_budget_ms=args.report_budget_ms,
        seeded_errors=args.seeded_errors,
        seeded_services=args.seeded_services,
        expected_retention_deletes=args.expected_retention_deletes,
        retention_deleted=deleted,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    seed = commands.add_parser("seed")
    seed.add_argument("--database", type=Path, required=True)
    seed.add_argument("--current-errors", type=int, default=50_000)
    seed.add_argument("--expired-errors", type=int, default=20_000)
    seed.add_argument("--services", type=int, default=200)
    run = commands.add_parser("run")
    run.add_argument("--endpoint", required=True)
    run.add_argument("--query-samples", type=int, default=20)
    run.add_argument("--report-samples", type=int, default=20)
    run.add_argument("--ingest-samples", type=int, default=48)
    run.add_argument("--readyz-samples", type=int, default=24)
    run.add_argument("--concurrency", type=int, default=16)
    run.add_argument("--query-budget-ms", type=int, default=2_000)
    run.add_argument("--report-budget-ms", type=int, default=4_000)
    run.add_argument("--seeded-errors", type=int, default=50_000)
    run.add_argument("--seeded-services", type=int, default=200)
    run.add_argument("--expected-retention-deletes", type=int, default=20_000)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.command == "seed":
        receipt = seed_database(
            args.database, args.current_errors, args.expired_errors, args.services
        )
    else:
        receipt = run_live_gate(args)
    print(json.dumps(receipt, sort_keys=True))
    return 0 if receipt.get("status", "ok") == "ok" else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"canary live gate: {error}", file=sys.stderr)
        raise SystemExit(2)
