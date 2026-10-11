#!/usr/bin/env python3
"""Emit deterministic synthetic vectors for owner-module accounting tests.

Usage: python3 qa/rc_accounting_vectors.py [output.json]
The output is nlbwmon's columnar `mac` grouping plus exact untruncated totals and
local-midnight boundary cases. No production code or router is involved.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import sys


rows = []
for i in range(120):
    rows.append([f"02:00:00:00:{i // 256:02x}:{i % 256:02x}", i + 1, 1_000 + i, 0,
                 2_000 + 2 * i, 0])

cases = []
local_date = "2026-09-29"
for offset in (0, 19_800, -28_800):  # UTC, UTC+05:30, UTC-08:00
    local_midnight_as_utc = datetime(2026, 9, 29, tzinfo=timezone.utc) - timedelta(seconds=offset)
    for seconds, expected in ((-1, "2026-09-28"), (0, local_date), (1, local_date)):
        ts = int((local_midnight_as_utc + timedelta(seconds=seconds)).timestamp())
        cases.append({"offset_secs": offset, "timestamp": ts, "expected_day": expected})

vector = {
    "description": "120 clients; all-client totals must be independent of the 50-row display cap",
    "nlbw_mac": {
        "columns": ["mac", "conns", "rx_bytes", "rx_pkts", "tx_bytes", "tx_pkts"],
        "data": rows,
    },
    "expected": {
        "client_count": 120,
        "display_limit": 50,
        "rx_bytes": sum(row[2] for row in rows),
        "tx_bytes": sum(row[4] for row in rows),
        "rx_bytes_visible_top_50": sum(row[2] for row in sorted(rows, key=lambda r: r[2] + r[4], reverse=True)[:50]),
        "tx_bytes_visible_top_50": sum(row[4] for row in sorted(rows, key=lambda r: r[2] + r[4], reverse=True)[:50]),
    },
    "local_midnight": cases,
    "wan_counter_reset_cases": [
        {"previous": {"rx": 1000, "tx": 2000}, "current": {"rx": 100, "tx": 2300}, "expected_delta": {"rx": 0, "tx": 300}},
        {"previous": {"rx": 1000, "tx": 2000}, "current": {"rx": 1300, "tx": 100}, "expected_delta": {"rx": 300, "tx": 0}},
        {"previous": {"rx": 1000, "tx": 2000}, "current": {"rx": 100, "tx": 100}, "expected_delta": {"rx": 0, "tx": 0}},
        {"previous": {"rx": 1000, "tx": 2000}, "current": {"rx": 1300, "tx": 2300}, "expected_delta": {"rx": 300, "tx": 300}},
    ],
}

encoded = json.dumps(vector, indent=2) + "\n"
if len(sys.argv) > 1:
    Path(sys.argv[1]).write_text(encoded)
else:
    sys.stdout.write(encoded)
