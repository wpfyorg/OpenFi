#!/usr/bin/env python3
"""Black-box RC checks against an isolated fixture-mode OpenWrtFi process.

Run after `cargo build` with `python3 qa/rc_blackbox.py`. Uses only Python's standard
library. It retains the API extractor contract failure until its patch lands and verifies
restart history through the externally visible API. The mock backend's temporary state is
isolated under a per-run TMPDIR.
"""

from __future__ import annotations

import concurrent.futures
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("OPENWRTFI_BIN", ROOT / "target/debug/openwrtfi"))
FIXTURES = ROOT / "fixtures"


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class RcBlackBox(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if not BIN.is_file():
            raise RuntimeError(f"binary missing: {BIN}; run `cargo build` first")
        cls.tmp = tempfile.TemporaryDirectory(prefix="openwrtfi-rc-")
        cls.tmp_path = Path(cls.tmp.name)
        cls.audit_path = cls.tmp_path / "audit.jsonl"
        cls.config_path = cls.tmp_path / "config.json"
        cls.system_tmp = cls.tmp_path / "system-tmp"
        cls.system_tmp.mkdir()
        cls.mock_state_paths = (
            cls.system_tmp / "openwrtfi-mock-history.json",
            cls.system_tmp / "openwrtfi-mock-daily.json",
        )
        cls.config_path.write_text(json.dumps({"audit_log": str(cls.audit_path)}))
        cls.port = free_port()
        cls.base = f"http://127.0.0.1:{cls.port}"
        cls.proc: subprocess.Popen[bytes] | None = None
        cls.original_mock_state = {}
        for path in cls.mock_state_paths:
            cls.original_mock_state[path] = path.read_bytes() if path.exists() else None
            path.unlink(missing_ok=True)
        cls.start_server()
        cls.cookie = cls.login()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.stop_server()
        for path, content in cls.original_mock_state.items():
            if content is None:
                path.unlink(missing_ok=True)
            else:
                path.write_bytes(content)
        cls.tmp.cleanup()

    @classmethod
    def start_server(cls) -> None:
        cls.proc = subprocess.Popen(
            [str(BIN), "--mock", str(FIXTURES), "--config", str(cls.config_path),
             "--listen", f"127.0.0.1:{cls.port}"],
            cwd=ROOT,
            env={**os.environ, "TMPDIR": str(cls.system_tmp)},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if cls.proc.poll() is not None:
                raise RuntimeError(f"OpenWrtFi exited with status {cls.proc.returncode}")
            try:
                status, _, body = cls.request("GET", "/api/v1/health")
                if status == 200 and body == b"ok":
                    return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.1)
        raise RuntimeError("OpenWrtFi did not become healthy within 12 seconds")

    @classmethod
    def stop_server(cls) -> None:
        proc = getattr(cls, "proc", None)
        if proc is None or proc.poll() is not None:
            return
        proc.send_signal(signal.SIGINT)
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=3)
        cls.proc = None

    @classmethod
    def request(cls, method: str, path: str, *, cookie: str | None = None,
                payload: object = None, csrf: bool = False,
                content_type: str | None = "application/json"):
        headers = {}
        if cookie:
            headers["Cookie"] = cookie
        if csrf:
            headers["X-OpenWrtFi"] = "1"
        data = None
        if payload is not None:
            data = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        if content_type:
            headers["Content-Type"] = content_type
        req = urllib.request.Request(cls.base + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=12) as response:
                return response.status, {k.lower(): v for k, v in response.headers.items()}, response.read()
        except urllib.error.HTTPError as e:
            try:
                return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()
            finally:
                e.close()

    @classmethod
    def login(cls) -> str:
        status, headers, body = cls.request(
            "POST", "/api/v1/auth/login", payload={"username": "root", "password": "mock"},
            csrf=True,
        )
        if status != 200:
            raise RuntimeError(f"mock login failed: HTTP {status}: {body!r}")
        raw_cookie = headers.get("set-cookie")
        if not raw_cookie:
            raise RuntimeError("login did not set a session cookie")
        return raw_cookie.split(";", 1)[0]

    def test_audit_concurrent_successes_persist_to_disk(self) -> None:
        n = 160

        def write(i: int) -> int:
            mac = f"02:00:{(i >> 16) & 255:02x}:{(i >> 8) & 255:02x}:{i & 255:02x}:01"
            status, _, _ = self.request(
                "POST", f"/api/v1/clients/{mac}/whitelist", cookie=self.cookie,
                payload={"on": bool(i & 1)}, csrf=True,
            )
            return status

        with concurrent.futures.ThreadPoolExecutor(max_workers=32) as pool:
            statuses = list(pool.map(write, range(n)))
        self.assertEqual(statuses, [200] * n)
        api_rows = json.loads(self.request("GET", "/api/v1/audit", cookie=self.cookie)[2])
        disk_rows = [json.loads(line) for line in self.audit_path.read_text().splitlines()]
        targets = {row["target"].split(" -> ", 1)[0] for row in disk_rows}
        self.assertGreaterEqual(len(api_rows), n)
        self.assertEqual(len(targets), n, f"only {len(targets)}/{n} unique audit entries persisted")

    def test_api_extractor_rejections_are_json_errors(self) -> None:
        cases = (
            (b"{", "application/json", 400),
            (b'{"tool":"ping","host":123}', "application/json", 422),
            (b'{"tool":"ping","host":"127.0.0.1"}', None, 415),
        )
        for body, content_type, expected_status in cases:
            with self.subTest(body=body, content_type=content_type):
                status, headers, response = self.request(
                    "POST", "/api/v1/diag", cookie=self.cookie, payload=body,
                    csrf=True, content_type=content_type,
                )
                self.assertEqual(status, expected_status)
                self.assertIn("application/json", headers.get("content-type", ""))
                parsed = json.loads(response)
                self.assertIsInstance(parsed.get("error"), str)

    def test_auth_csrf_and_protected_routes(self) -> None:
        self.assertEqual(self.request("GET", "/api/v1/snapshot")[0], 401)
        self.assertEqual(self.request("GET", "/api/v1/auth/me", cookie=self.cookie)[0], 200)
        self.assertEqual(self.request(
            "POST", "/api/v1/nodes", cookie=self.cookie, payload={}, csrf=False
        )[0], 403)
        self.assertEqual(self.request(
            "GET", "/api/v1/no-such-route", cookie=self.cookie
        )[0], 404)
        status, headers, body = self.request("GET", "/api/v1/no-such-route", cookie=self.cookie)
        self.assertEqual(status, 404)
        self.assertIn("application/json", headers.get("content-type", ""))
        self.assertEqual(json.loads(body).get("error"), "not found")

    def test_logout_revokes_session(self) -> None:
        cookie = self.login()
        status, _, _ = self.request("POST", "/api/v1/auth/logout", cookie=cookie, csrf=True)
        self.assertEqual(status, 204)
        self.assertEqual(self.request("GET", "/api/v1/auth/me", cookie=cookie)[0], 401)

    def test_sse_emits_authenticated_snapshot_event(self) -> None:
        req = urllib.request.Request(
            self.base + "/api/v1/stream", headers={"Cookie": self.cookie}
        )
        with urllib.request.urlopen(req, timeout=10) as response:
            self.assertEqual(response.status, 200)
            self.assertIn("text/event-stream", response.headers.get("Content-Type", ""))
            lines = []
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline and not any(line.startswith(b"data:") for line in lines):
                line = response.readline()
                if line:
                    lines.append(line)
            self.assertTrue(any(b"event: snapshot" in line for line in lines), lines)
            payload = next(line[5:].strip() for line in lines if line.startswith(b"data:"))
            self.assertIsInstance(json.loads(payload), dict)

    def test_wifi_rollout_success_and_injected_failure(self) -> None:
        status, _, body = self.request("GET", "/api/v1/wifi/networks", cookie=self.cookie)
        self.assertEqual(status, 200)
        networks = json.loads(body)
        self.assertTrue(networks)
        network_id = networks[0]["id"]
        for ssid, expected_state in (("RC QA success", "done"), ("fail-ap-2", "failed")):
            status, _, body = self.request(
                "POST", f"/api/v1/wifi/networks/{network_id}/plan", cookie=self.cookie,
                payload={"ssid": ssid}, csrf=True,
            )
            self.assertEqual(status, 200, body)
            plan_id = json.loads(body)["id"]
            status, _, body = self.request(
                "POST", f"/api/v1/wifi/plans/{plan_id}/apply", cookie=self.cookie,
                payload={}, csrf=True,
            )
            self.assertEqual(status, 202, body)
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                status, _, body = self.request(
                    "GET", f"/api/v1/wifi/plans/{plan_id}", cookie=self.cookie
                )
                self.assertEqual(status, 200)
                state = json.loads(body)
                if state["state"] in ("done", "failed"):
                    break
                time.sleep(0.2)
            self.assertEqual(state["state"], expected_state, state)
            if expected_state == "failed":
                self.assertTrue(any(step["status"] == "rolled_back" for step in state["steps"]), state)
                self.assertTrue(any(step["status"] == "skipped" for step in state["steps"]), state)

    def test_traffic_history_restart_has_no_duplicate_buckets(self) -> None:
        duplicate_sets: list[list[int]] = []
        for _ in range(3):
            time.sleep(2.2)  # allow the 2-second mock collector to sample at least once
            status, _, body = self.request("GET", "/api/v1/history?range=24h", cookie=self.cookie)
            self.assertEqual(status, 200)
            timestamps = [point["ts"] for point in json.loads(body)]
            duplicate_sets.append(sorted(ts for ts in set(timestamps) if timestamps.count(ts) > 1))
            self.stop_server()
            persisted = json.loads(self.mock_state_paths[0].read_text())
            persisted_timestamps = [point["ts"] for point in persisted.get("24h", [])]
            duplicate_sets.append(sorted(
                ts for ts in set(persisted_timestamps) if persisted_timestamps.count(ts) > 1
            ))
            self.start_server()
            type(self).cookie = self.login()
        self.assertFalse(duplicate_sets[-1], f"duplicate timestamps in persisted history: {duplicate_sets[-1]}; per-phase={duplicate_sets}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
