"""A small, standard-library client for the Forgejo 15 REST API (``/api/v1``).

Only the operations the synchronization scripts need are implemented. ``forgejo_api_check.py``
checks each of them against the server's ``/swagger.v1.json`` before activation.

Error handling:

- 401 raises :class:`AuthError` (token missing, revoked or expired) and is never retried.
- 403 raises :class:`ForbiddenError`, 404 :class:`NotFoundError`, 409 and 422 :class:`ConflictError`.
- Network failures, timeouts, 429 and 5xx are retried with exponential backoff (``Retry-After``
  honoured, capped) for GET, PATCH and DELETE. POST is retried only on 429, where the server
  says it did nothing; any other ambiguous POST failure is raised, and the caller's next run
  re-lists before creating, so a retry can never duplicate a pull request, issue or comment.

Redirects are refused (:class:`ApiError`, status 3xx), never followed: urllib would replay every
request header, ``Authorization`` and the Cloudflare Access service token included, to whatever
host the redirect names, and a redirect means the request never reached Forgejo anyway. The
usual cause is Cloudflare Access rejecting or missing the service token (``common.CloudflareAccess``).

No request header is ever logged, no redirect body or location is reported, and every error
message is redacted.
"""

from __future__ import annotations

import json
import socket
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Callable, Iterator

from common import REDACTOR, CloudflareAccess, Redactor, SyncError, printable, validate_repo_slug

USER_AGENT = "pdfcraft-sync/1 (+scripts/automation)"
MAX_RESPONSE_BYTES = 32 * 1024 * 1024
MAX_PAGES = 100
PAGE_LIMIT = 50


class ApiError(SyncError):
    def __init__(self, message: str, status: int | None = None):
        super().__init__(message)
        self.status = status


class AuthError(ApiError):
    pass


class ForbiddenError(ApiError):
    pass


class NotFoundError(ApiError):
    pass


class ConflictError(ApiError):
    pass


class TransientError(ApiError):
    pass


_RETRYABLE_STATUS = {429, 500, 502, 503, 504}


class _RefuseRedirects(urllib.request.BaseHandler):
    """Turns every redirect into an error before ``HTTPRedirectHandler`` (order 500) can follow it."""

    handler_order = 100

    def _refuse(self, req, fp, code, msg, headers):
        raise urllib.error.HTTPError(req.full_url, code, msg, headers, fp)

    http_error_301 = http_error_302 = http_error_303 = http_error_307 = http_error_308 = _refuse


class HttpClient:
    """JSON over HTTPS with retries, typed errors and redaction."""

    def __init__(
        self,
        base_url: str,
        auth_header: str | None,
        *,
        redactor: Redactor = REDACTOR,
        opener: urllib.request.OpenerDirector | None = None,
        retries: int = 4,
        backoff: float = 2.0,
        max_retry_after: float = 60.0,
        timeout: float = 30.0,
        sleep: Callable[[float], None] = time.sleep,
        extra_headers: dict[str, str] | None = None,
        name: str = "API",
    ):
        self.base_url = base_url.rstrip("/")
        self._auth_header = auth_header
        self.redactor = redactor
        self.opener = opener or urllib.request.build_opener()
        self.retries = retries
        self.backoff = backoff
        self.max_retry_after = max_retry_after
        self.timeout = timeout
        self.sleep = sleep
        self.extra_headers = extra_headers or {}
        self.name = name

    def _url(self, path: str, params: dict[str, Any] | None) -> str:
        url = self.base_url + path
        if params:
            url += "?" + urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
        return url

    def request(self, method: str, path: str, *, params: dict[str, Any] | None = None,
                body: Any = None) -> Any:
        url = self._url(path, params)
        data = json.dumps(body).encode("utf-8") if body is not None else None
        attempt = 0
        while True:
            attempt += 1
            req = urllib.request.Request(url, data=data, method=method)
            req.add_header("Accept", "application/json")
            req.add_header("User-Agent", USER_AGENT)
            if data is not None:
                req.add_header("Content-Type", "application/json")
            for key, value in self.extra_headers.items():
                req.add_header(key, value)
            if self._auth_header:
                req.add_header("Authorization", self._auth_header)
            retry_after: float | None = None
            try:
                with self.opener.open(req, timeout=self.timeout) as resp:
                    raw = resp.read(MAX_RESPONSE_BYTES + 1)
                    if len(raw) > MAX_RESPONSE_BYTES:
                        raise ApiError(f"{self.name} {method} {self._safe_path(path)}: response too large")
                    if not raw.strip():
                        return None
                    try:
                        return json.loads(raw.decode("utf-8"))
                    except (UnicodeDecodeError, json.JSONDecodeError):
                        raise ApiError(f"{self.name} {method} {self._safe_path(path)}: invalid JSON") from None
            except urllib.error.HTTPError as exc:
                status = exc.code
                # A redirect's body is a login page or similar: never worth reporting.
                detail = "" if 300 <= status < 400 else self._error_detail(exc)
                if status in _RETRYABLE_STATUS and self._may_retry(method, status) and attempt <= self.retries:
                    retry_after = self._retry_after(exc)
                else:
                    raise self._typed(status, method, path, detail) from None
            except (urllib.error.URLError, socket.timeout, TimeoutError, ConnectionError) as exc:
                reason = getattr(exc, "reason", exc)
                if method in ("GET", "PATCH", "DELETE") and attempt <= self.retries:
                    retry_after = None
                else:
                    raise TransientError(
                        self.redactor.redact(
                            f"{self.name} {method} {self._safe_path(path)}: network error: {reason}"
                        )
                    ) from None
            delay = retry_after if retry_after is not None else self.backoff * (2 ** (attempt - 1))
            self.sleep(min(max(delay, 0.0), self.max_retry_after))

    @staticmethod
    def _may_retry(method: str, status: int) -> bool:
        if method in ("GET", "PATCH", "DELETE"):
            return True
        return status == 429  # the server refused before doing anything

    def _retry_after(self, exc: urllib.error.HTTPError) -> float | None:
        value = exc.headers.get("Retry-After") if exc.headers else None
        if value and value.strip().isdigit():
            return min(float(value.strip()), self.max_retry_after)
        return None

    def _error_detail(self, exc: urllib.error.HTTPError) -> str:
        try:
            raw = exc.read(4096)
        except Exception:  # noqa: BLE001 - a broken error body must not hide the status
            return ""
        try:
            parsed = json.loads(raw.decode("utf-8", errors="replace"))
            if isinstance(parsed, dict) and isinstance(parsed.get("message"), str):
                return printable(self.redactor.redact(parsed["message"]), limit=300)
        except (json.JSONDecodeError, ValueError):
            pass
        return printable(self.redactor.redact(raw), limit=300)

    @staticmethod
    def _safe_path(path: str) -> str:
        return printable(path.split("?", 1)[0], limit=120)

    def _typed(self, status: int, method: str, path: str, detail: str) -> ApiError:
        msg = self.redactor.redact(f"{self.name} {method} {self._safe_path(path)}: HTTP {status} {detail}".strip())
        if 300 <= status < 400:
            return ApiError(msg + " redirect refused (Cloudflare Access or a proxy intercepted the request;"
                            " check CF_ACCESS_CLIENT_ID and CF_ACCESS_CLIENT_SECRET)", status)
        if status == 401:
            return AuthError(msg + " (token missing, revoked or expired)", status)
        if status == 403:
            return ForbiddenError(msg, status)
        if status == 404:
            return NotFoundError(msg, status)
        if status in (409, 422):
            return ConflictError(msg, status)
        if status in _RETRYABLE_STATUS:
            return TransientError(msg, status)
        return ApiError(msg, status)


def _q(value: str) -> str:
    return urllib.parse.quote(value, safe="")


class ForgejoClient(HttpClient):
    """The Forgejo operations the automation uses, on one repository.

    ``cf_access`` adds the Cloudflare Access service-token headers to this client's requests
    only; other clients (GitHub) never see them. Redirects are always refused.
    """

    def __init__(self, base_url: str, repo: str, token: str | None, *,
                 cf_access: CloudflareAccess | None = None, **kwargs):
        opener = kwargs.pop("opener", None) or urllib.request.build_opener()
        opener.add_handler(_RefuseRedirects())
        extra_headers = dict(kwargs.pop("extra_headers", None) or {})
        if cf_access is not None:
            extra_headers.update(cf_access.headers())
        super().__init__(
            base_url.rstrip("/") + "/api/v1",
            f"token {token}" if token else None,
            name="Forgejo",
            opener=opener,
            extra_headers=extra_headers,
            **kwargs,
        )
        owner, name = validate_repo_slug(repo, "Forgejo repository").split("/", 1)
        self.repo_path = f"/repos/{_q(owner)}/{_q(name)}"

    # -- identity and repository ---------------------------------------------------------------

    def version(self) -> dict:
        return self.request("GET", "/version") or {}

    def current_user(self) -> dict:
        return self.request("GET", "/user") or {}

    def get_repo(self) -> dict:
        return self.request("GET", self.repo_path) or {}

    def edit_repo(self, fields: dict) -> dict:
        return self.request("PATCH", self.repo_path, body=fields) or {}

    def list_branch_protections(self) -> list:
        return self.request("GET", self.repo_path + "/branch_protections") or []

    def get_branch(self, name: str) -> dict | None:
        try:
            return self.request("GET", f"{self.repo_path}/branches/{_q(name)}")
        except NotFoundError:
            return None

    # -- pull requests --------------------------------------------------------------------------

    def _paginate(self, path: str, params: dict[str, Any]) -> Iterator[dict]:
        for page in range(1, MAX_PAGES + 1):
            items = self.request("GET", path, params={**params, "page": page, "limit": PAGE_LIMIT})
            if not isinstance(items, list):
                raise ApiError(f"Forgejo GET {path}: expected a list")
            for item in items:
                if isinstance(item, dict):
                    yield item
            if len(items) < PAGE_LIMIT:
                return
        raise ApiError(f"Forgejo GET {path}: more than {MAX_PAGES} pages")

    def list_pulls(self, state: str = "all") -> list[dict]:
        return list(self._paginate(self.repo_path + "/pulls", {"state": state, "sort": "oldest"}))

    def get_pull(self, index: int) -> dict:
        return self.request("GET", f"{self.repo_path}/pulls/{int(index)}") or {}

    def create_pull(self, *, head: str, base: str, title: str, body: str) -> dict:
        return self.request(
            "POST", self.repo_path + "/pulls",
            body={"head": head, "base": base, "title": title, "body": body},
        ) or {}

    def edit_pull(self, index: int, **fields: Any) -> dict:
        allowed = {k: v for k, v in fields.items() if k in ("title", "body", "state")}
        return self.request("PATCH", f"{self.repo_path}/pulls/{int(index)}", body=allowed) or {}

    # -- issues and comments --------------------------------------------------------------------

    def list_issues(self, state: str = "open") -> list[dict]:
        return list(self._paginate(self.repo_path + "/issues", {"state": state, "type": "issues"}))

    def create_issue(self, *, title: str, body: str) -> dict:
        return self.request("POST", self.repo_path + "/issues", body={"title": title, "body": body}) or {}

    def edit_issue(self, index: int, **fields: Any) -> dict:
        allowed = {k: v for k, v in fields.items() if k in ("title", "body", "state")}
        return self.request("PATCH", f"{self.repo_path}/issues/{int(index)}", body=allowed) or {}

    def list_comments(self, index: int) -> list[dict]:
        items = self.request("GET", f"{self.repo_path}/issues/{int(index)}/comments")
        return [c for c in items if isinstance(c, dict)] if isinstance(items, list) else []

    def create_comment(self, index: int, body: str) -> dict:
        return self.request("POST", f"{self.repo_path}/issues/{int(index)}/comments", body={"body": body}) or {}

    def comment_once(self, index: int, marker: str, body: str) -> bool:
        """Posts ``body`` unless a comment carrying ``marker`` exists. Returns True if posted."""
        for comment in self.list_comments(index):
            if marker in str(comment.get("body") or ""):
                return False
        self.create_comment(index, f"{marker}\n{body}")
        return True


# The operations above, as (method, swagger path). forgejo_api_check.py verifies each one.
REQUIRED_OPERATIONS = (
    ("get", "/version"),
    ("get", "/user"),
    ("get", "/repos/{owner}/{repo}"),
    ("patch", "/repos/{owner}/{repo}"),
    ("get", "/repos/{owner}/{repo}/branch_protections"),
    ("get", "/repos/{owner}/{repo}/branches/{branch}"),
    ("get", "/repos/{owner}/{repo}/pulls"),
    ("post", "/repos/{owner}/{repo}/pulls"),
    ("get", "/repos/{owner}/{repo}/pulls/{index}"),
    ("patch", "/repos/{owner}/{repo}/pulls/{index}"),
    ("get", "/repos/{owner}/{repo}/issues"),
    ("post", "/repos/{owner}/{repo}/issues"),
    ("patch", "/repos/{owner}/{repo}/issues/{index}"),
    ("get", "/repos/{owner}/{repo}/issues/{index}/comments"),
    ("post", "/repos/{owner}/{repo}/issues/{index}/comments"),
)
