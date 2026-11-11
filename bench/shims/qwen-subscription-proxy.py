#!/usr/bin/env python3
"""Локальный прокси к подписке Qwen (qwen.ai) для mark2.

У подписки Qwen нет API-ключа: Qwen Code (CLI `qwen`) входит через OAuth и
держит токен в ~/.qwen/subscription_credentials.json, access_token живёт
15 минут. Прокси слушает 127.0.0.1, принимает OpenAI-совместимые запросы
`/v1/<путь>`, подставляет `Authorization: Bearer <access_token>` и
пробрасывает их в `<upstream>/<путь>`; ответ (в том числе SSE) отдаётся
клиенту по мере поступления.

    python3 bench/shims/qwen-subscription-proxy.py --port 8092
    mark2 --base-url http://127.0.0.1:8092/v1 --model qwen3.8-max ...

Обновление токена повторяет протокол Qwen Code 0.27.2 (QwenSubscriptionSession
в chunks/chunk-N3AKLLXG.js), чтобы параллельно работающий `qwen` не разлогинил
пользователя и не было двух refresh одним refresh_token:

- блокировка — каталог `<realpath(~/.qwen)>/subscription_credentials.json.lock`
  (proper-lockfile: mkdir; stale 60 с; держатель обновляет mtime каталога);
- под блокировкой файл перечитывается: если токен уже обновил кто-то другой,
  берём его, повторно не обновляем;
- запись — tmp-файл рядом + rename, права 0600, компактный JSON;
- `revision` — идентификатор сессии входа (uuid попытки sign-in), а не версия
  токена: Qwen Code при refresh его не меняет и сравнивает при смене файла
  (getModels → session_changed, «Run /auth to sign in again»). Поэтому прокси
  revision сохраняет как есть, как и все прочие поля;
- `retryAfter.refresh` (мс epoch) — кулдаун после 429 от сервера токенов.

Только стандартная библиотека. Значения токенов, заголовок Authorization и
содержимое файла токенов никогда не попадают в лог и в ответы клиенту.
"""

import argparse
import http.client
import ipaddress
import json
import math
import os
import platform
import secrets
import signal
import ssl
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DEFAULT_CREDS = "~/.qwen/subscription_credentials.json"
DEFAULT_UPSTREAM = "https://api.qwen.ai/compatible-mode/v1"

# Обновлять токен, если до истечения осталось меньше этого (Qwen Code — 30 с).
REFRESH_MARGIN_S = 60
# Параметры блокировки — как у Qwen Code: proper-lockfile с stale 60 с,
# 350 попыток через 100 мс (~35 с ожидания).
LOCK_STALE_S = 60
LOCK_RETRY_DELAY_S = 0.1
LOCK_MAX_ATTEMPTS = 350
# Держатель блокировки обновляет mtime каталога (proper-lockfile — раз в
# stale/2 = 30 с); мы чаще, с запасом.
LOCK_HEARTBEAT_S = 15
# Таймаут запроса к серверу токенов — как у Qwen Code.
OAUTH_TIMEOUT_S = 30
# Версия Qwen Code, под которую снят протокол; идёт в User-Agent.
QWEN_CODE_VERSION = "0.27.2"

# Заголовки одного соединения (RFC 9110 §7.6.1) — не пробрасываются.
HOP_BY_HOP = {
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "trailers",
    "transfer-encoding",
    "upgrade",
}
# Заголовки клиента, которые прокси ставит сам или выбрасывает.
CLIENT_SKIP = HOP_BY_HOP | {
    "host",
    "authorization",
    "content-length",
    "user-agent",
    "expect",
}

_log_lock = threading.Lock()


def log(message):
    """Одна строка в stderr. Сюда никогда не передаются токены."""
    line = f"{time.strftime('%Y-%m-%d %H:%M:%S')} {message}\n"
    with _log_lock:
        sys.stderr.write(line)
        sys.stderr.flush()


def now_ms():
    return time.time_ns() // 1_000_000


def default_user_agent():
    """User-Agent, который Qwen Code шлёт на api.qwen.ai (process.platform/arch Node)."""
    arch = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64"}.get(
        platform.machine().lower(), platform.machine().lower()
    )
    return f"QwenCode/{QWEN_CODE_VERSION} ({sys.platform}; {arch})"


class ProxyError(Exception):
    """Ошибка прокси; текст безопасен для лога и ответа клиенту."""


class CredsLock:
    """Межпроцессная блокировка файла токенов, совместимая с Qwen Code.

    Qwen Code берёт proper-lockfile на путь `realpath(dirname) / basename` с
    `realpath: false, stale: 60000, retries: 0` и сам повторяет попытку
    350 раз через 100 мс. proper-lockfile создаёт каталог `<файл>.lock`
    (mkdir атомарен), считает его протухшим, если mtime старше stale, и
    держатель периодически обновляет mtime; если mtime поменял кто-то
    другой — блокировка «скомпрометирована» и коммитить нельзя.
    """

    def __init__(self, creds_path):
        directory = os.path.realpath(os.path.dirname(creds_path))
        self.path = os.path.join(directory, os.path.basename(creds_path)) + ".lock"
        self._mtime_ns = None
        self._compromised = False
        self._stop = threading.Event()
        self._state = threading.Lock()
        self._heartbeat_thread = None

    def __enter__(self):
        self._acquire()
        return self

    def __exit__(self, *exc):
        self._release()
        return False

    def _try_mkdir(self, check_stale):
        try:
            os.mkdir(self.path)
            return True
        except FileExistsError:
            if not check_stale:
                return False
        try:
            st = os.stat(self.path)
        except FileNotFoundError:
            # Держатель только что отпустил — одна попытка без проверки stale,
            # как acquireLock(..., {stale: 0}) в proper-lockfile.
            return self._try_mkdir(False)
        if st.st_mtime >= time.time() - LOCK_STALE_S:
            return False
        # Протухшая блокировка (держатель умер): удалить и попробовать один раз.
        try:
            os.rmdir(self.path)
        except FileNotFoundError:
            pass
        log("stale credentials lock removed")
        return self._try_mkdir(False)

    def _acquire(self):
        for attempt in range(LOCK_MAX_ATTEMPTS + 1):
            if self._try_mkdir(True):
                break
            if attempt == LOCK_MAX_ATTEMPTS:
                raise ProxyError(
                    "credentials lock is busy (another qwen process holds it too long)"
                )
            time.sleep(LOCK_RETRY_DELAY_S)
        self._touch()
        self._heartbeat_thread = threading.Thread(
            target=self._heartbeat, name="creds-lock-heartbeat", daemon=True
        )
        self._heartbeat_thread.start()

    def _touch(self):
        # Точность mtime — миллисекунды, как у proper-lockfile на APFS/ext4.
        stamp = now_ms() * 1_000_000
        os.utime(self.path, ns=(stamp, stamp))
        self._mtime_ns = os.stat(self.path).st_mtime_ns

    def _is_ours(self):
        try:
            return os.stat(self.path).st_mtime_ns == self._mtime_ns
        except FileNotFoundError:
            return False

    def _heartbeat(self):
        while not self._stop.wait(LOCK_HEARTBEAT_S):
            with self._state:
                if self._stop.is_set():
                    return
                if not self._is_ours():
                    self._compromised = True
                    return
                try:
                    self._touch()
                except OSError:
                    self._compromised = True
                    return

    def assert_can_commit(self):
        """Аналог assertCanCommit Qwen Code: перед записью блокировка ещё наша."""
        with self._state:
            if self._compromised or not self._is_ours():
                raise ProxyError("credentials lock was lost; refusing to write tokens")

    def _release(self):
        self._stop.set()
        if self._heartbeat_thread is not None:
            self._heartbeat_thread.join()
        with self._state:
            if self._is_ours():
                try:
                    os.rmdir(self.path)
                except FileNotFoundError:
                    pass


class Credentials:
    """Чтение, обновление и атомарная запись ~/.qwen/subscription_credentials.json."""

    def __init__(self, path):
        self.path = path
        # Внутри процесса refresh выполняет один поток; остальные ждут и
        # перечитывают файл — там уже свежий токен.
        self.mutex = threading.Lock()
        self._ssl = ssl.create_default_context()

    def read(self):
        try:
            fd = os.open(self.path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        except FileNotFoundError:
            raise ProxyError(
                "Qwen Subscription credentials not found: sign in with `qwen` (/auth)"
            ) from None
        except OSError as e:
            raise ProxyError(f"cannot open credentials file: {e.strerror}") from None
        with os.fdopen(fd, "rb") as f:
            raw = f.read()
        try:
            data = json.loads(raw.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            # Текст исключения может содержать фрагмент файла — не показываем.
            raise ProxyError("credentials file is not valid JSON") from None
        if not isinstance(data, dict):
            raise ProxyError("credentials file is not a JSON object")
        return data

    @staticmethod
    def _tokens(creds):
        tokens = creds.get("tokens")
        if not isinstance(tokens, dict):
            return None
        access = tokens.get("access_token")
        if not isinstance(access, str) or not access:
            return None
        return tokens

    @staticmethod
    def _is_fresh(tokens):
        expires_at = tokens.get("expiresAt")
        if not isinstance(expires_at, (int, float)) or isinstance(expires_at, bool):
            return False
        return expires_at - now_ms() >= REFRESH_MARGIN_S * 1000

    def access_token(self, rejected=None):
        """Действующий access_token; при необходимости — refresh под блокировкой.

        rejected — токен, на который апстрим ответил 401: обновлять, только
        если в файле всё ещё он (getAccessToken(signal, rejectedToken) у Qwen Code).
        """
        if rejected is None:
            tokens = self._tokens(self.read())
            if tokens is not None and self._is_fresh(tokens):
                return tokens["access_token"]
        with self.mutex:
            with CredsLock(self.path) as lock:
                creds = self.read()
                tokens = self._tokens(creds)
                if tokens is None:
                    raise ProxyError(
                        "Qwen Subscription is not signed in: run `qwen` and /auth"
                    )
                if self._is_fresh(tokens) and tokens["access_token"] != rejected:
                    # Уже обновил кто-то другой (Qwen Code или соседний поток).
                    return tokens["access_token"]
                return self._refresh(creds, lock)

    def _refresh(self, creds, lock):
        base_url = creds.get("baseUrl")
        client_id = creds.get("clientId")
        refresh_token = creds["tokens"].get("refresh_token")
        if not isinstance(base_url, str) or not base_url.startswith("https://"):
            raise ProxyError("credentials file has no valid baseUrl")
        if not isinstance(client_id, str) or not client_id:
            raise ProxyError("credentials file has no clientId")
        if not isinstance(refresh_token, str) or not refresh_token:
            raise ProxyError("credentials file has no refresh_token: sign in again")

        retry_after = creds.get("retryAfter")
        cooldown = retry_after.get("refresh") if isinstance(retry_after, dict) else None
        if isinstance(cooldown, (int, float)) and cooldown > now_ms():
            wait_s = math.ceil((cooldown - now_ms()) / 1000)
            raise ProxyError(f"token refresh is rate-limited for another {wait_s} s")

        form = urllib.parse.urlencode(
            {
                "grant_type": "refresh_token",
                "client_id": client_id,
                "refresh_token": refresh_token,
            }
        ).encode("ascii")
        request = urllib.request.Request(
            base_url.rstrip("/") + "/api/oauth/token",
            data=form,
            method="POST",
            headers={
                "Accept": "application/json",
                "Content-Type": "application/x-www-form-urlencoded",
                # Qwen Code ходит сюда через fetch Node — у него UA «node».
                "User-Agent": "node",
            },
        )
        try:
            with urllib.request.urlopen(
                request, timeout=OAUTH_TIMEOUT_S, context=self._ssl
            ) as response:
                payload = response.read()
        except urllib.error.HTTPError as e:
            code, description = _oauth_error(e)
            if e.code == 429:
                delay_ms = _retry_after_ms(e.headers.get("Retry-After"))
                if delay_ms:
                    # Как requestWithCooldown: запомнить кулдаун в файле.
                    cooldowns = creds.get("retryAfter")
                    if not isinstance(cooldowns, dict):
                        cooldowns = {}
                    cooldowns["refresh"] = now_ms() + delay_ms
                    creds["retryAfter"] = cooldowns
                    self._write(creds, lock)
            hint = (
                " — refresh token is no longer valid, sign in again with `qwen` (/auth)"
                if code == "invalid_grant"
                else ""
            )
            detail = f": {description}" if description else ""
            log(f"token refresh failed: HTTP {e.code} {code}{hint}")
            raise ProxyError(
                f"token refresh failed: HTTP {e.code} {code}{detail}{hint}"
            ) from None
        except (urllib.error.URLError, OSError) as e:
            reason = getattr(e, "reason", None) or e
            log(f"token refresh failed: {type(e).__name__}: {reason}")
            raise ProxyError(f"token refresh failed: {reason}") from None

        try:
            data = json.loads(payload.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            data = None
        if not isinstance(data, dict):
            raise ProxyError("token refresh failed: invalid response")
        access = data.get("access_token")
        token_type = data.get("token_type")
        expires_in = data.get("expires_in")
        new_refresh = data.get("refresh_token") or refresh_token
        scope = data.get("scope", creds["tokens"].get("scope", ""))
        # Схема Qwen Code: token_type — строго "Bearer", expires_in > 0. Файл,
        # который не пройдёт её проверку, Qwen Code сочтёт битым — не пишем такой.
        if (
            not isinstance(access, str)
            or not access
            or not isinstance(token_type, str)
            or token_type.lower() != "bearer"
            or not isinstance(expires_in, (int, float))
            or isinstance(expires_in, bool)
            or not math.isfinite(expires_in)
            or expires_in <= 0
            or not isinstance(new_refresh, str)
            or not isinstance(scope, str)
        ):
            raise ProxyError("token refresh failed: unexpected response shape")

        expires_at = now_ms() + expires_in * 1000
        if isinstance(expires_at, float) and expires_at.is_integer():
            expires_at = int(expires_at)
        # Порядок ключей — как пишет Qwen Code: {...tokens, expiresAt}.
        creds["tokens"] = {
            "access_token": access,
            "refresh_token": new_refresh,
            "token_type": "Bearer",
            "expires_in": expires_in,
            "scope": scope,
            "expiresAt": expires_at,
        }
        self._write(creds, lock)
        log(f"token refreshed, expires in {int(expires_in)} s")
        return access

    def _write(self, creds, lock):
        """Атомарная запись: tmp рядом (fsync, 0600) + rename под нашей блокировкой.

        Все поля верхнего уровня (revision, signInAttempt, models,
        modelMetadata, retryAfter и незнакомые) сохраняются как были.
        JSON компактный — так пишет Qwen Code (JSON.stringify без отступов).
        """
        data = json.dumps(creds, ensure_ascii=False, separators=(",", ":")).encode(
            "utf-8"
        )
        tmp = f"{self.path}.{secrets.token_hex(6)}.tmp"
        fd = os.open(
            tmp,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
        try:
            with os.fdopen(fd, "wb") as f:
                os.fchmod(f.fileno(), 0o600)
                f.write(data)
                f.flush()
                os.fsync(f.fileno())
            lock.assert_can_commit()
            os.replace(tmp, self.path)
        except BaseException:
            try:
                os.unlink(tmp)
            except FileNotFoundError:
                pass
            raise


def _oauth_error(e):
    """(error, error_description) из JSON-ответа сервера токенов."""
    try:
        body = json.loads(e.read().decode("utf-8"))
    except Exception:
        return "invalid_response", None
    finally:
        e.close()
    if not isinstance(body, dict) or not isinstance(body.get("error"), str):
        return "invalid_response", None
    description = body.get("error_description")
    return body["error"], description if isinstance(description, str) else None


def _retry_after_ms(value):
    """Retry-After (секунды или HTTP-дата) → мс; None, если не разобрать."""
    if not value:
        return None
    try:
        return max(0, int(float(value) * 1000))
    except OverflowError:
        return None
    except ValueError:
        pass
    try:
        from email.utils import parsedate_to_datetime

        return max(0, int(parsedate_to_datetime(value).timestamp() * 1000) - now_ms())
    except (TypeError, ValueError):
        return None


class Upstream:
    def __init__(self, url, timeout, user_agent):
        parts = urllib.parse.urlsplit(url)
        if parts.scheme not in ("http", "https") or not parts.hostname:
            raise SystemExit(f"--upstream must be an http(s) URL, got {url!r}")
        self.url = url.rstrip("/")
        self.https = parts.scheme == "https"
        self.host = parts.hostname
        self.port = parts.port
        self.base_path = parts.path.rstrip("/")
        self.timeout = timeout
        self.user_agent = user_agent
        self._ssl = ssl.create_default_context() if self.https else None

    def connect(self):
        if self.https:
            return http.client.HTTPSConnection(
                self.host, self.port, timeout=self.timeout, context=self._ssl
            )
        return http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)


class ProxyHandler(BaseHTTPRequestHandler):
    # HTTP/1.0 + Connection: close: тело ответа отдаётся как есть до закрытия
    # соединения — SSE идёт клиенту без перекодирования в chunked.
    protocol_version = "HTTP/1.0"
    server_version = "qwen-subscription-proxy"
    sys_version = ""

    def do_GET(self):
        self._proxy()

    def do_POST(self):
        self._proxy()

    def log_message(self, format, *args):
        # Стандартный лог BaseHTTPRequestHandler заменён своим (см. _proxy).
        pass

    def log_error(self, format, *args):
        log(f"client error: {format % args}")

    def _proxy(self):
        started = self._started = time.monotonic()
        self._headers_sent = False
        path = urllib.parse.urlsplit(self.path).path
        status = None
        notes = []
        try:
            status = self._handle(path, notes)
        except ProxyError as e:
            status = self._send_error_json(502, str(e))
        except ValueError as e:
            status = self._send_error_json(
                400, f"bad request: {e}", "invalid_request_error"
            )
        except (OSError, http.client.HTTPException) as e:
            status = self._send_error_json(
                502, f"upstream request failed: {type(e).__name__}: {e}"
            )
        finally:
            elapsed = round((time.monotonic() - started) * 1000)
            suffix = f" ({'; '.join(notes)})" if notes else ""
            log(f"{self.command} {path} → {status}, {elapsed} ms{suffix}")

    def _handle(self, path, notes):
        if path != "/v1" and not path.startswith("/v1/"):
            return self._send_error_json(
                404, f"unknown path {path}: expected /v1/...", "invalid_request_error"
            )
        upstream = self.server.upstream
        query = urllib.parse.urlsplit(self.path).query
        target = upstream.base_path + path[len("/v1") :] + (f"?{query}" if query else "")
        body = self._read_body()
        headers = self._client_headers()

        creds = self.server.creds
        token = creds.access_token()
        response, conn = self._send_upstream(target, body, headers, token)
        if response.status == 401:
            # Тело буферизовано — повтор безопасен. Один принудительный refresh.
            response.read()
            conn.close()
            token = creds.access_token(rejected=token)
            notes.append("retried after 401")
            response, conn = self._send_upstream(target, body, headers, token)
        return self._relay(response, conn, notes)

    def _read_body(self):
        if "chunked" in self.headers.get("Transfer-Encoding", "").lower():
            parts = []
            while True:
                line = self.rfile.readline(65537)
                size = int(line.split(b";", 1)[0].strip() or b"x", 16)
                if size == 0:
                    while self.rfile.readline(65537) not in (b"\r\n", b"\n", b""):
                        pass
                    break
                parts.append(self.rfile.read(size))
                self.rfile.readline(65537)
            return b"".join(parts)
        length = self.headers.get("Content-Length")
        if length is None:
            return b"" if self.command == "POST" else None
        n = int(length)
        if n < 0:
            raise ValueError("negative Content-Length")
        return self.rfile.read(n)

    def _client_headers(self):
        listed = {
            t.strip().lower()
            for t in self.headers.get("Connection", "").split(",")
            if t.strip()
        }
        out = [
            (k, v)
            for k, v in self.headers.items()
            if k.lower() not in CLIENT_SKIP and k.lower() not in listed
        ]
        out.append(("User-Agent", self.server.upstream.user_agent))
        return out

    def _send_upstream(self, target, body, headers, token):
        conn = self.server.upstream.connect()
        try:
            has_accept_encoding = any(k.lower() == "accept-encoding" for k, _ in headers)
            conn.putrequest(
                self.command, target, skip_accept_encoding=has_accept_encoding
            )
            for k, v in headers:
                conn.putheader(k, v)
            conn.putheader("Authorization", f"Bearer {token}")
            if body is not None:
                conn.putheader("Content-Length", str(len(body)))
            conn.endheaders(body)
            response = conn.getresponse()
        except BaseException:
            conn.close()
            raise
        return response, conn

    def _relay(self, response, conn, notes):
        """Статус, заголовки и тело апстрима — клиенту, кусками по мере прихода."""
        try:
            listed = {
                t.strip().lower()
                for t in (response.getheader("Connection") or "").split(",")
                if t.strip()
            }
            self.send_response_only(response.status, response.reason)
            for k, v in response.getheaders():
                if k.lower() in HOP_BY_HOP or k.lower() in listed:
                    continue
                self.send_header(k, v)
            self.send_header("Connection", "close")
            self.end_headers()
            self._headers_sent = True
            first = None
            while True:
                try:
                    # read1 возвращает то, что уже пришло (для chunked — до
                    # конца текущего чанка), не дожидаясь всего тела.
                    chunk = response.read1(65536)
                except (OSError, http.client.HTTPException) as e:
                    notes.append(f"upstream stream aborted: {type(e).__name__}")
                    break
                if not chunk:
                    break
                if first is None:
                    first = round((time.monotonic() - self._started) * 1000)
                try:
                    # wfile небуферизованный (wbufsize = 0) — сразу в сокет.
                    self.wfile.write(chunk)
                    self.wfile.flush()
                except OSError:
                    notes.append("client disconnected")
                    break
            if first is not None and "text/event-stream" in (
                response.getheader("Content-Type") or ""
            ):
                notes.append(f"stream, first byte {first} ms")
            return response.status
        finally:
            conn.close()

    def _send_error_json(self, code, message, kind="proxy_error"):
        if self._headers_sent:
            # Заголовки уже ушли — остаётся только оборвать соединение.
            return f"{code} after headers"
        body = json.dumps({"error": {"message": message, "type": kind}}).encode("utf-8")
        try:
            self.send_response_only(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self._headers_sent = True
            self.wfile.write(body)
        except OSError:
            pass
        return code


class ProxyHTTPD(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def parse_args(argv=None):
    p = argparse.ArgumentParser(
        description="Local OpenAI-compatible proxy to the Qwen subscription (qwen.ai)."
    )
    p.add_argument("--port", type=int, default=8092)
    p.add_argument("--host", default="127.0.0.1", help="loopback address only")
    p.add_argument("--creds", default=DEFAULT_CREDS, help="Qwen Code credentials file")
    p.add_argument("--upstream", default=DEFAULT_UPSTREAM)
    p.add_argument(
        "--timeout",
        type=float,
        default=600,
        help="upstream socket timeout, s (applies to each read, so long streams are fine)",
    )
    p.add_argument("--user-agent", default=default_user_agent())
    return p.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    try:
        if not ipaddress.IPv4Address(args.host).is_loopback:
            raise ValueError
    except ValueError:
        # Прокси подставляет чужую подписку — наружу его открывать нельзя.
        raise SystemExit(f"--host must be a loopback IPv4 address, got {args.host!r}")

    creds = Credentials(os.path.expanduser(args.creds))
    upstream = Upstream(args.upstream, args.timeout, args.user_agent)
    httpd = ProxyHTTPD((args.host, args.port), ProxyHandler)
    httpd.creds = creds
    httpd.upstream = upstream

    stop = threading.Event()

    def on_signal(signum, _frame):
        stop.set()

    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)

    serving = threading.Thread(
        target=httpd.serve_forever, kwargs={"poll_interval": 0.5}, daemon=True
    )
    serving.start()
    log(
        f"listening on http://{args.host}:{args.port}/v1 → {upstream.url} "
        f"(creds {creds.path}, pid {os.getpid()})"
    )
    while not stop.is_set():
        stop.wait(0.5)

    log("shutting down")
    httpd.shutdown()
    httpd.server_close()
    # Если сейчас идёт refresh — дать ему дописать файл и снять блокировку.
    if creds.mutex.acquire(timeout=LOCK_MAX_ATTEMPTS * LOCK_RETRY_DELAY_S + OAUTH_TIMEOUT_S):
        creds.mutex.release()
    log("stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
