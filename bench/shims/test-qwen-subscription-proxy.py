#!/usr/bin/env python3
"""Офлайн-тесты bench/shims/qwen-subscription-proxy.py.

    python3 bench/shims/test-qwen-subscription-proxy.py

Сеть не нужна: сервер токенов подменён (urllib.request.urlopen), апстрим —
локальный http.server, файл токенов — во временном каталоге (настоящий
~/.qwen не трогается). Токены здесь фиктивные.
"""

import email.message
import http.client
import importlib.util
import io
import json
import os
import stat
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "qwen_subscription_proxy", os.path.join(HERE, "qwen-subscription-proxy.py")
)
proxy = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(proxy)
proxy.log = lambda message: None  # тесты не шумят в stderr


def now_ms():
    return time.time_ns() // 1_000_000


class FakeOAuth:
    """Подмена urlopen для POST /api/oauth/token."""

    def __init__(self, delay=0.0, error=None):
        self.calls = []
        self.delay = delay
        self.error = error
        self._lock = threading.Lock()

    def __call__(self, request, timeout=None, context=None):
        form = urllib.parse.parse_qs(request.data.decode("ascii"))
        with self._lock:
            self.calls.append((request.full_url, request.get_method(), form))
            n = len(self.calls)
        time.sleep(self.delay)
        if self.error is not None:
            raise self.error(request.full_url)
        body = json.dumps(
            {
                "access_token": f"access-new-{n}",
                "refresh_token": f"refresh-new-{n}",
                "token_type": "Bearer",
                "expires_in": 900,
                "scope": "model:invoke profile:read",
            }
        ).encode()
        return _Response(body)


class _Response(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def http_error(code, body, retry_after=None):
    def make(url):
        headers = email.message.Message()
        if retry_after is not None:
            headers["Retry-After"] = retry_after
        return urllib.error.HTTPError(
            url, code, "error", headers, io.BytesIO(json.dumps(body).encode())
        )

    return make


class CredsCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.tmp.name, "subscription_credentials.json")
        self.lock_dir = os.path.join(
            os.path.realpath(self.tmp.name), "subscription_credentials.json.lock"
        )

    def tearDown(self):
        self.tmp.cleanup()

    def write_creds(self, access="access-old", expires_in_ms=-1000, **extra):
        data = {
            "revision": "rev-1",
            "baseUrl": "https://auth.example",
            "clientId": "qwen-code-cli",
            "models": ["qwen3.8-max", "qwen3.8-flash"],
            "modelMetadata": [{"id": "qwen3.8-max", "contextWindowSize": 1000000}],
            "futureField": {"keep": [1, 2, 3]},
            "tokens": {
                "access_token": access,
                "refresh_token": "refresh-old",
                "token_type": "Bearer",
                "scope": "model:invoke profile:read",
                "expires_in": 900,
                "expiresAt": now_ms() + expires_in_ms,
            },
        }
        data.update(extra)
        with open(self.path, "w") as f:
            json.dump(data, f)
        os.chmod(self.path, 0o600)
        return data

    def read_creds(self):
        with open(self.path) as f:
            return json.load(f)


class RefreshTest(CredsCase):
    def test_refresh_preserves_fields_and_revision(self):
        before = self.write_creds()
        oauth = FakeOAuth()
        with mock.patch("urllib.request.urlopen", oauth):
            token = proxy.Credentials(self.path).access_token()
        self.assertEqual(token, "access-new-1")
        self.assertEqual(len(oauth.calls), 1)
        url, method, form = oauth.calls[0]
        self.assertEqual(url, "https://auth.example/api/oauth/token")
        self.assertEqual(method, "POST")
        self.assertEqual(
            form,
            {
                "grant_type": ["refresh_token"],
                "client_id": ["qwen-code-cli"],
                "refresh_token": ["refresh-old"],
            },
        )
        after = self.read_creds()
        for key in ("revision", "baseUrl", "clientId", "models", "modelMetadata", "futureField"):
            self.assertEqual(after[key], before[key], key)
        tokens = after["tokens"]
        self.assertEqual(tokens["access_token"], "access-new-1")
        self.assertEqual(tokens["refresh_token"], "refresh-new-1")
        self.assertEqual(tokens["token_type"], "Bearer")
        self.assertEqual(tokens["expires_in"], 900)
        self.assertIsInstance(tokens["expiresAt"], int)
        self.assertGreater(tokens["expiresAt"], now_ms() + 890_000)
        self.assertEqual(stat.S_IMODE(os.stat(self.path).st_mode), 0o600)
        with open(self.path) as f:
            self.assertNotIn(": ", f.read())  # компактный JSON, как у Qwen Code
        self.assertEqual(os.listdir(self.tmp.name), ["subscription_credentials.json"])

    def test_fresh_token_is_used_without_refresh(self):
        self.write_creds(access="access-fresh", expires_in_ms=600_000)
        oauth = FakeOAuth()
        with mock.patch("urllib.request.urlopen", oauth):
            self.assertEqual(proxy.Credentials(self.path).access_token(), "access-fresh")
        self.assertEqual(oauth.calls, [])

    def test_refresh_inside_margin(self):
        self.write_creds(expires_in_ms=(proxy.REFRESH_MARGIN_S - 5) * 1000)
        oauth = FakeOAuth()
        with mock.patch("urllib.request.urlopen", oauth):
            self.assertEqual(proxy.Credentials(self.path).access_token(), "access-new-1")

    def test_concurrent_requests_refresh_once(self):
        self.write_creds()
        oauth = FakeOAuth(delay=0.3)
        creds = proxy.Credentials(self.path)
        results = []
        with mock.patch("urllib.request.urlopen", oauth):
            threads = [
                threading.Thread(target=lambda: results.append(creds.access_token()))
                for _ in range(8)
            ]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
        self.assertEqual(len(oauth.calls), 1)
        self.assertEqual(results, ["access-new-1"] * 8)

    def test_two_processes_refresh_once(self):
        # Два независимых Credentials — как два процесса: синхронизирует только
        # файловая блокировка.
        self.write_creds()
        oauth = FakeOAuth(delay=0.3)
        a, b = proxy.Credentials(self.path), proxy.Credentials(self.path)
        results = []
        with mock.patch("urllib.request.urlopen", oauth):
            threads = [
                threading.Thread(target=lambda c=c: results.append(c.access_token()))
                for c in (a, b)
            ]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
        self.assertEqual(len(oauth.calls), 1)
        self.assertEqual(results, ["access-new-1", "access-new-1"])

    def test_rejected_token(self):
        self.write_creds(access="access-fresh", expires_in_ms=600_000)
        oauth = FakeOAuth()
        creds = proxy.Credentials(self.path)
        with mock.patch("urllib.request.urlopen", oauth):
            # В файле уже другой токен — кто-то обновил, refresh не нужен.
            self.assertEqual(creds.access_token(rejected="access-stale"), "access-fresh")
            self.assertEqual(oauth.calls, [])
            # Апстрим отверг именно токен из файла — принудительный refresh.
            self.assertEqual(creds.access_token(rejected="access-fresh"), "access-new-1")
        self.assertEqual(len(oauth.calls), 1)

    def test_waits_for_foreign_lock_and_reuses_its_token(self):
        self.write_creds()
        os.mkdir(self.lock_dir)  # «Qwen Code» держит блокировку и обновляет токен

        def foreign_refresh():
            time.sleep(0.5)
            data = self.read_creds()
            data["tokens"]["access_token"] = "access-by-qwen"
            data["tokens"]["expiresAt"] = now_ms() + 900_000
            with open(self.path, "w") as f:
                json.dump(data, f)
            os.rmdir(self.lock_dir)

        oauth = FakeOAuth()
        t = threading.Thread(target=foreign_refresh)
        started = time.monotonic()
        with mock.patch("urllib.request.urlopen", oauth):
            t.start()
            token = proxy.Credentials(self.path).access_token()
        t.join()
        self.assertEqual(token, "access-by-qwen")
        self.assertEqual(oauth.calls, [])
        self.assertGreaterEqual(time.monotonic() - started, 0.45)
        self.assertFalse(os.path.exists(self.lock_dir))

    def test_stale_lock_is_taken_over(self):
        self.write_creds()
        os.mkdir(self.lock_dir)
        old = time.time() - proxy.LOCK_STALE_S - 30
        os.utime(self.lock_dir, (old, old))
        oauth = FakeOAuth()
        started = time.monotonic()
        with mock.patch("urllib.request.urlopen", oauth):
            self.assertEqual(proxy.Credentials(self.path).access_token(), "access-new-1")
        self.assertLess(time.monotonic() - started, 1.0)
        self.assertFalse(os.path.exists(self.lock_dir))

    def test_lost_lock_blocks_commit(self):
        lock = proxy.CredsLock(self.path)
        with lock:
            lock.assert_can_commit()
            os.utime(self.lock_dir, (1, 1))  # чужой процесс перехватил блокировку
            with self.assertRaises(proxy.ProxyError):
                lock.assert_can_commit()
        # Чужую блокировку не удаляем.
        self.assertTrue(os.path.isdir(self.lock_dir))

    def test_429_sets_cooldown(self):
        self.write_creds()
        oauth = FakeOAuth(error=http_error(429, {"error": "rate_limited"}, retry_after="30"))
        creds = proxy.Credentials(self.path)
        with mock.patch("urllib.request.urlopen", oauth):
            with self.assertRaises(proxy.ProxyError):
                creds.access_token()
            cooldown = self.read_creds()["retryAfter"]["refresh"]
            self.assertAlmostEqual(cooldown, now_ms() + 30_000, delta=2_000)
            with self.assertRaisesRegex(proxy.ProxyError, "rate-limited"):
                creds.access_token()
        self.assertEqual(len(oauth.calls), 1)
        self.assertEqual(self.read_creds()["revision"], "rev-1")

    def test_invalid_grant_leaves_file_untouched(self):
        self.write_creds()
        with open(self.path, "rb") as f:
            before = f.read()
        oauth = FakeOAuth(error=http_error(400, {"error": "invalid_grant"}))
        with mock.patch("urllib.request.urlopen", oauth):
            with self.assertRaisesRegex(proxy.ProxyError, "invalid_grant"):
                proxy.Credentials(self.path).access_token()
        with open(self.path, "rb") as f:
            self.assertEqual(f.read(), before)
        self.assertFalse(os.path.exists(self.lock_dir))

    def test_missing_file(self):
        with self.assertRaisesRegex(proxy.ProxyError, "not found"):
            proxy.Credentials(self.path).access_token()

    def test_error_messages_never_contain_tokens(self):
        self.write_creds()
        oauth = FakeOAuth(error=http_error(400, {"error": "invalid_grant"}))
        with mock.patch("urllib.request.urlopen", oauth):
            try:
                proxy.Credentials(self.path).access_token()
            except proxy.ProxyError as e:
                self.assertNotIn("refresh-old", str(e))
                self.assertNotIn("access-old", str(e))


class FakeUpstream(BaseHTTPRequestHandler):
    """Апстрим: /compatible-mode/v1/models, /chat/completions (SSE), 401 для access-old."""

    protocol_version = "HTTP/1.1"
    seen = []

    def log_message(self, *args):
        pass

    def _record(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        FakeUpstream.seen.append((self.command, self.path, dict(self.headers), body))
        return body

    def do_GET(self):
        self._record()
        if self.headers.get("Authorization") == "Bearer access-old":
            return self._json(401, {"error": {"message": "expired", "code": "invalid_token"}})
        self._json(200, {"object": "list", "data": [{"id": "qwen3.8-max"}]})

    def do_POST(self):
        self._record()
        if self.headers.get("Authorization") == "Bearer access-old":
            return self._json(401, {"error": {"message": "expired", "code": "invalid_token"}})
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        for i in range(3):
            data = f"data: {json.dumps({'i': i})}\n\n".encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
            self.wfile.flush()
            time.sleep(0.4)
        self.wfile.write(b"0\r\n\r\n")

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def serve(httpd):
    t = threading.Thread(target=httpd.serve_forever, daemon=True)
    t.start()
    return t


class ProxyTest(CredsCase):
    def setUp(self):
        super().setUp()
        FakeUpstream.seen = []
        self.upstream = ThreadingHTTPServer(("127.0.0.1", 0), FakeUpstream)
        serve(self.upstream)
        up_port = self.upstream.server_address[1]
        self.httpd = proxy.ProxyHTTPD(("127.0.0.1", 0), proxy.ProxyHandler)
        self.httpd.creds = proxy.Credentials(self.path)
        self.httpd.upstream = proxy.Upstream(
            f"http://127.0.0.1:{up_port}/compatible-mode/v1", 10, "QwenCode/test (darwin; arm64)"
        )
        serve(self.httpd)
        self.port = self.httpd.server_address[1]

    def tearDown(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.upstream.shutdown()
        self.upstream.server_close()
        super().tearDown()

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        conn.request(method, path, body=body, headers=headers or {})
        return conn, conn.getresponse()

    def test_models_injects_auth_and_user_agent(self):
        self.write_creds(access="access-fresh", expires_in_ms=600_000)
        conn, resp = self.request(
            "GET", "/v1/models?x=1", headers={"Authorization": "Bearer client-key", "X-Extra": "1"}
        )
        self.assertEqual(resp.status, 200)
        self.assertEqual(json.loads(resp.read())["data"][0]["id"], "qwen3.8-max")
        conn.close()
        method, path, headers, _ = FakeUpstream.seen[-1]
        self.assertEqual((method, path), ("GET", "/compatible-mode/v1/models?x=1"))
        self.assertEqual(headers["Authorization"], "Bearer access-fresh")
        self.assertEqual(headers["User-Agent"], "QwenCode/test (darwin; arm64)")
        self.assertEqual(headers["X-Extra"], "1")

    def test_stream_arrives_incrementally(self):
        self.write_creds(access="access-fresh", expires_in_ms=600_000)
        body = json.dumps({"model": "qwen3.8-max", "stream": True, "messages": []}).encode()
        conn, resp = self.request(
            "POST", "/v1/chat/completions", body=body, headers={"Content-Type": "application/json"}
        )
        self.assertEqual(resp.status, 200)
        self.assertEqual(resp.getheader("Content-Type"), "text/event-stream")
        arrivals, lines = [], []
        while True:
            line = resp.readline()
            if not line:
                break
            if line.startswith(b"data: "):
                arrivals.append(time.monotonic())
                lines.append(line)
        conn.close()
        self.assertEqual(len(lines), 3)
        # Первый чанк пришёл заметно раньше последнего — нет буферизации тела.
        self.assertGreater(arrivals[-1] - arrivals[0], 0.6)
        self.assertEqual(FakeUpstream.seen[-1][3], body)

    def test_401_refreshes_and_retries_once(self):
        self.write_creds(access="access-old", expires_in_ms=600_000)
        oauth = FakeOAuth()
        body = json.dumps({"model": "qwen3.8-max", "stream": True, "messages": []}).encode()
        with mock.patch("urllib.request.urlopen", oauth):
            conn, resp = self.request("POST", "/v1/chat/completions", body=body)
            data = resp.read()
            conn.close()
        self.assertEqual(resp.status, 200)
        self.assertEqual(data.count(b"data: "), 3)
        self.assertEqual(len(oauth.calls), 1)
        auths = [h["Authorization"] for _, _, h, _ in FakeUpstream.seen]
        self.assertEqual(auths, ["Bearer access-old", "Bearer access-new-1"])
        self.assertEqual([b for *_, b in FakeUpstream.seen], [body, body])

    def test_upstream_down_is_502(self):
        self.write_creds(access="access-fresh", expires_in_ms=600_000)
        self.httpd.upstream = proxy.Upstream("http://127.0.0.1:9/v1", 2, "ua")
        conn, resp = self.request("GET", "/v1/models")
        self.assertEqual(resp.status, 502)
        self.assertEqual(json.loads(resp.read())["error"]["type"], "proxy_error")
        conn.close()

    def test_refresh_failure_is_502(self):
        self.write_creds()
        oauth = FakeOAuth(error=http_error(400, {"error": "invalid_grant"}))
        with mock.patch("urllib.request.urlopen", oauth):
            conn, resp = self.request("GET", "/v1/models")
            error = json.loads(resp.read())["error"]
            conn.close()
        self.assertEqual(resp.status, 502)
        self.assertEqual(error["type"], "proxy_error")
        self.assertIn("invalid_grant", error["message"])
        self.assertEqual(FakeUpstream.seen, [])

    def test_unknown_path_is_404(self):
        conn, resp = self.request("GET", "/models")
        self.assertEqual(resp.status, 404)
        conn.close()


class CliTest(unittest.TestCase):
    def test_rejects_non_loopback_host(self):
        for host in ("0.0.0.0", "192.168.1.10", "localhost"):
            with self.assertRaises(SystemExit):
                proxy.main(["--host", host, "--port", "0"])

    def test_default_user_agent_mimics_qwen_code(self):
        self.assertRegex(proxy.default_user_agent(), r"^QwenCode/\d+\.\d+\.\d+ \(\w+; \w+\)$")


if __name__ == "__main__":
    unittest.main(verbosity=2)
