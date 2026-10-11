#!/usr/bin/env python3
"""Adversarial RC checks for restart persistence gaps.

These tests intentionally exercise behavior not covered by rc_blackbox.py:
persisted audit history exposed through the API after restart, and both current
and legacy traffic-bucket migration behavior.
"""

from __future__ import annotations

import json
import time
import unittest

import rc_blackbox as blackbox


class RcAdversarial(unittest.TestCase):
    def test_audit_api_restores_persisted_history_after_restart(self) -> None:
        h = blackbox.RcBlackBox
        h.setUpClass()
        try:
            mac = "02:aa:bb:cc:dd:ee"
            status, _, body = h.request(
                "POST",
                f"/api/v1/clients/{mac}/whitelist",
                cookie=h.cookie,
                payload={"on": True},
                csrf=True,
            )
            self.assertEqual(status, 200, body)

            before = json.loads(h.request("GET", "/api/v1/audit", cookie=h.cookie)[2])
            self.assertGreaterEqual(len(before), 1)
            self.assertTrue(h.audit_path.exists())
            disk_before = h.audit_path.read_text().splitlines()
            self.assertGreaterEqual(len(disk_before), 1)

            h.stop_server()
            h.start_server()
            h.cookie = h.login()

            status, _, body = h.request("GET", "/api/v1/audit", cookie=h.cookie)
            self.assertEqual(status, 200)
            after = json.loads(body)
            disk_after = h.audit_path.read_text().splitlines()
            restored_action = next(
                (
                    entry
                    for entry in after
                    if entry["action"] == "whitelist"
                    and entry["target"] == f"{mac} -> on"
                    and entry["result"] == "ok"
                ),
                None,
            )
            self.assertIsNotNone(
                restored_action,
                f"whitelist audit entry missing after restart; disk has {len(disk_after)} rows",
            )
        finally:
            h.tearDownClass()

    def _assert_traffic_restart_policy(self, *, legacy: bool) -> None:
        h = blackbox.RcBlackBox
        h.setUpClass()
        try:
            h.stop_server()

            seed_count = 100
            seed_rx = 1_000
            seed_tx = 2_000
            now = int(time.time())
            bucket_24h = now - now % 1800
            bucket_7d = now - now % (3 * 3600)
            if legacy:
                persisted = {
                    "24h": [{"ts": bucket_24h, "rx_bps": seed_rx, "tx_bps": seed_tx}],
                    "7d": [{"ts": bucket_7d, "rx_bps": seed_rx, "tx_bps": seed_tx}],
                }
            else:
                persisted = {
                    "24h": [],
                    "7d": [],
                    "active_24h": {
                        "start": bucket_24h,
                        "rx_sum": seed_rx * seed_count,
                        "tx_sum": seed_tx * seed_count,
                        "samples": seed_count,
                    },
                    "active_7d": {
                        "start": bucket_7d,
                        "rx_sum": seed_rx * seed_count,
                        "tx_sum": seed_tx * seed_count,
                        "samples": seed_count,
                    },
                }
            h.mock_state_paths[0].write_text(json.dumps(persisted))

            h.start_server()
            h.cookie = h.login()

            # Catch the first non-zero post-restart WAN sample, before another
            # poll can change the rates used to verify the bucket average.
            deadline = time.monotonic() + 8
            observed = None
            while time.monotonic() < deadline:
                status, _, body = h.request("GET", "/api/v1/snapshot", cookie=h.cookie)
                self.assertEqual(status, 200)
                snap = json.loads(body)
                new_rx = snap["wan"]["rx_bps"]
                new_tx = snap["wan"]["tx_bps"]
                if new_rx > 0 or new_tx > 0:
                    points_by_range = {}
                    for range_name, bucket in (("24h", bucket_24h), ("7d", bucket_7d)):
                        status, _, body = h.request(
                            "GET", f"/api/v1/history?range={range_name}", cookie=h.cookie
                        )
                        self.assertEqual(status, 200)
                        points = json.loads(body)
                        points_by_range[range_name] = [p for p in points if p["ts"] == bucket]
                    observed = (points_by_range, new_rx, new_tx)
                    break
                time.sleep(0.05)

            self.assertIsNotNone(observed, "no non-zero post-restart WAN sample observed")
            points_by_range, new_rx, new_tx = observed
            for range_name, points in points_by_range.items():
                with self.subTest(range=range_name, legacy=legacy):
                    self.assertEqual(len(points), 1, points)
                    point = points[0]
                    if legacy:
                        expected = (new_rx, new_tx)
                    else:
                        expected = (
                            (seed_rx * seed_count + new_rx) // (seed_count + 1),
                            (seed_tx * seed_count + new_tx) // (seed_count + 1),
                        )
                    self.assertEqual(
                        (point["rx_bps"], point["tx_bps"]),
                        expected,
                        f"{range_name} restart policy mismatch: point={point}, "
                        f"expected={expected}, new=({new_rx}, {new_tx})",
                    )
        finally:
            h.tearDownClass()

    def test_current_schema_active_bucket_preserves_exact_weight(self) -> None:
        self._assert_traffic_restart_policy(legacy=False)

    def test_legacy_average_bucket_discards_unknown_partial_weight(self) -> None:
        self._assert_traffic_restart_policy(legacy=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
