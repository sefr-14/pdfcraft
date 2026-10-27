"""Test doubles: an in-process fake Forgejo and fake GitHub API, and real temporary Git remotes.

Nothing here talks to the network beyond 127.0.0.1. Git remotes are bare repositories on disk
reached over file://; the fake Forgejo remote refuses non-fast-forward pushes and any push to
Main, as the real server's branch protection is meant to.
"""

from __future__ import annotations

import io
import json
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
AUTOMATION = os.path.dirname(HERE)
if AUTOMATION not in os.sys.path:
    os.sys.path.insert(0, AUTOMATION)

from common import Git, Redactor, Reporter  # noqa: E402

FORGEJO_TOKEN = "fj_test_token_5f3a9c1e7b2d4a6f8e0c"
GITHUB_TOKEN = "ghs_testtokenABCDEFGHIJKLMNOPQRSTUV0123"
# Fake Cloudflare Access service token; real ones never appear in tests.
CF_ID = "cf_test_id_0123456789abcdef.access"
CF_SECRET = "cf_test_secret_9f8e7d6c5b4a39281706"
IDENTITY = {
    "GIT_AUTHOR_NAME": "Test Author",
    "GIT_AUTHOR_EMAIL": "author@example.invalid",
    "GIT_COMMITTER_NAME": "Test Author",
    "GIT_COMMITTER_EMAIL": "author@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
}


def no_proxy_opener() -> urllib.request.OpenerDirector:
    return urllib.request.build_opener(urllib.request.ProxyHandler({}))


def git(cwd: str, *args: str, input_bytes: bytes | None = None, check: bool = True) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(IDENTITY)
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull, "LC_ALL": "C"})
    proc = subprocess.run(["git", *args], cwd=cwd, env=env, input=input_bytes,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if check and proc.returncode != 0:
        raise AssertionError(f"git {' '.join(args)} failed: {proc.stderr.decode(errors='replace')}")
    return proc.stdout.decode("utf-8", errors="replace").strip()


PRE_RECEIVE = """#!/bin/sh
# Like Forgejo branch protection on Main: no direct pushes, ever.
while read old new ref; do
  case "$ref" in
    refs/heads/Main|refs/heads/main) echo "protected branch $ref" >&2; exit 1 ;;
  esac
done
exit 0
"""


class Remotes:
    """A work repository for authoring commits plus bare 'github', 'upstream' and 'forgejo' remotes."""

    def __init__(self) -> None:
        self.root = tempfile.mkdtemp(prefix="sync-test-")
        self.work = os.path.join(self.root, "work")
        os.makedirs(self.work)
        git(self.work, "init", "-q", "-b", "Main")
        self.bare = {}
        for name in ("github", "upstream", "forgejo"):
            path = os.path.join(self.root, f"{name}.git")
            git(self.root, "init", "-q", "--bare", path)
            self.bare[name] = path
        git(self.bare["forgejo"], "config", "receive.denyNonFastForwards", "true")
        git(self.bare["forgejo"], "config", "receive.denyDeletes", "true")
        hook = os.path.join(self.bare["forgejo"], "hooks", "pre-receive")
        with open(hook, "w", encoding="utf-8") as handle:
            handle.write(PRE_RECEIVE)
        os.chmod(hook, 0o755)
        self._n = 0

    def url(self, name: str) -> str:
        return "file://" + self.bare[name]

    def commit(self, parent: str | None, files: dict[str, str], message: str = "change") -> str:
        """Creates a commit on top of ``parent`` (None for a root) without touching branches."""
        self._n += 1
        if parent and not git(self.work, "cat-file", "-t", parent, check=False) == "commit":
            for path in self.bare.values():  # a commit the scripts made on a remote
                git(self.work, "fetch", "-q", path, "+refs/*:refs/fetched/*", check=False)
        if parent:
            git(self.work, "checkout", "-q", "--detach", parent)
        else:
            git(self.work, "checkout", "-q", "--orphan", f"tmp{self._n}")
        if not parent:
            git(self.work, "rm", "-rfq", "--cached", ".", check=False)
        for path, content in files.items():
            full = os.path.join(self.work, path)
            os.makedirs(os.path.dirname(full), exist_ok=True)
            with open(full, "w", encoding="utf-8") as handle:
                handle.write(content)
            git(self.work, "add", path)
        git(self.work, "commit", "-q", "--allow-empty", "-m", message)
        return git(self.work, "rev-parse", "HEAD")

    def set_ref(self, remote: str, ref: str, sha: str) -> None:
        """Sets a ref on a bare remote directly, the way the server itself (or a merge) would."""
        self._n += 1
        git(self.work, "push", "-q", "--no-verify", self.bare[remote], f"+{sha}:refs/sync-objects/{self._n}")
        git(self.bare[remote], "update-ref", ref, sha)

    def ref(self, remote: str, ref: str) -> str | None:
        out = git(self.bare[remote], "rev-parse", "--verify", "--quiet", ref, check=False)
        return out or None

    def delete_ref(self, remote: str, ref: str) -> None:
        git(self.bare[remote], "update-ref", "-d", ref)

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


class _Server:
    def __init__(self, handler_factory):
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler_factory)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    @property
    def url(self) -> str:
        host, port = self.httpd.server_address[:2]
        return f"http://{host}:{port}"

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()


class Fault:
    def __init__(self, method: str, path_re: str, status: int, body: object = None,
                 headers: dict | None = None, times: int = 1, delay: float = 0.0):
        self.method = method
        self.path_re = re.compile(path_re)
        self.status = status
        self.body = body if body is not None else {"message": "injected fault"}
        self.headers = headers or {}
        self.times = times
        self.delay = delay


class FakeForgejo:
    """Enough of the Forgejo 15 API for the scripts, backed by a real bare repository."""

    def __init__(self, git_dir: str, token: str = FORGEJO_TOKEN, owner: str = "sefr", repo: str = "pdfcraft"):
        self.git_dir = git_dir
        self.token = token
        self.prefix = f"/api/v1/repos/{owner}/{repo}"
        self.pulls: dict[int, dict] = {}
        self.issues: dict[int, dict] = {}
        self.comments: dict[int, list] = {}
        self.next_index = 1
        self.faults: list[Fault] = []
        self.requests: list[tuple[str, str]] = []
        self.headers: list[dict[str, str]] = []  # each request's headers, names lower-cased
        # (client id, secret): when set, a request without both answers like Cloudflare Access,
        # with a 302 to ``access_redirect`` (a login page), before Forgejo sees it.
        self.require_cf: tuple[str, str] | None = None
        self.access_redirect = "http://127.0.0.1:9/cdn-cgi/access/login"
        self.before_request = None  # callable(method, path) -> None
        self.swagger = None  # served unauthenticated at /swagger.v1.json when set
        self.lock = threading.RLock()
        self.server = _Server(self._handler())

    @property
    def url(self) -> str:
        return self.server.url

    def stop(self) -> None:
        self.server.stop()

    def _tip(self, branch: str) -> str | None:
        out = git(self.git_dir, "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}", check=False)
        return out or None

    def pull_json(self, pr: dict) -> dict:
        head_sha = pr["merged_head"] if pr.get("merged") else (self._tip(pr["head"]) or pr.get("last_head"))
        base_sha = self._tip(pr["base"])
        merge_base = None
        if head_sha and base_sha:
            merge_base = git(self.git_dir, "merge-base", head_sha, base_sha, check=False) or None
        return {
            "number": pr["number"],
            "title": pr["title"],
            "body": pr["body"],
            "state": pr["state"],
            "merged": pr.get("merged", False),
            "head": {"ref": pr["head"], "label": pr["head"], "sha": head_sha},
            "base": {"ref": pr["base"], "label": pr["base"], "sha": base_sha},
            "merge_base": merge_base,
            "html_url": f"https://git.example.invalid/sefr/pdfcraft/pulls/{pr['number']}",
        }

    # -- test controls ---------------------------------------------------------------------------

    def add_pull(self, head: str, base: str, title: str, body: str, state: str = "open") -> int:
        with self.lock:
            index = self.next_index
            self.next_index += 1
            self.pulls[index] = {"number": index, "head": head, "base": base, "title": title,
                                 "body": body, "state": state, "merged": False, "last_head": self._tip(head)}
            self.comments[index] = []
            return index

    def merge_pull(self, index: int) -> None:
        """Merges like a fast-forward/merge commit on the server: Main moves to the head."""
        with self.lock:
            pr = self.pulls[index]
            head = self._tip(pr["head"])
            base = self._tip(pr["base"])
            if base and not subprocess.run(["git", f"--git-dir={self.git_dir}", "merge-base", "--is-ancestor", base, head]).returncode == 0:
                raise AssertionError("test merge would not be a fast-forward")
            git(self.git_dir, "update-ref", f"refs/heads/{pr['base']}", head)
            pr.update(state="closed", merged=True, merged_head=head)

    def close_pull(self, index: int) -> None:
        with self.lock:
            self.pulls[index]["state"] = "closed"
            self.pulls[index]["last_head"] = self._tip(self.pulls[index]["head"])

    def open_pulls(self) -> list[dict]:
        return [p for p in self.pulls.values() if p["state"] == "open"]

    def open_issues(self) -> list[dict]:
        return [i for i in self.issues.values() if i["state"] == "open"]

    # -- HTTP --------------------------------------------------------------------------------------

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # keep test output quiet
                pass

            def _send(self, status, payload, headers=None):
                if isinstance(payload, bytes):
                    data = payload
                else:
                    data = json.dumps(payload).encode("utf-8") if payload is not None else b""
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                for key, value in (headers or {}).items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(data)

            def _body(self):
                length = int(self.headers.get("Content-Length") or 0)
                if not length:
                    return {}
                return json.loads(self.rfile.read(length).decode("utf-8"))

            def _dispatch(self, method):
                parsed = urllib.parse.urlparse(self.path)
                path, query = parsed.path, dict(urllib.parse.parse_qsl(parsed.query))
                body = self._body() if method in ("POST", "PATCH") else {}
                hit = None
                with fake.lock:
                    fake.requests.append((method, path))
                    fake.headers.append({k.lower(): v for k, v in self.headers.items()})
                    if fake.require_cf is not None and (
                        self.headers.get("CF-Access-Client-Id"), self.headers.get("CF-Access-Client-Secret")
                    ) != fake.require_cf:
                        return self._send(302, b"<html>Sign in</html>", {"Location": fake.access_redirect})
                    if fake.before_request:
                        fake.before_request(method, path)
                    for fault in list(fake.faults):
                        if fault.method == method and fault.path_re.search(path) and fault.times > 0:
                            fault.times -= 1
                            hit = fault
                            break
                if hit is not None:
                    if hit.delay:
                        time.sleep(hit.delay)  # outside the lock: retries must not queue behind it
                    try:
                        return self._send(hit.status, hit.body, hit.headers)
                    except (BrokenPipeError, ConnectionResetError):
                        return None
                with fake.lock:
                    if path == "/swagger.v1.json" and fake.swagger is not None:
                        return self._send(200, fake.swagger)
                    if path == "/api/v1/version":
                        return self._send(200, {"version": "15.0.0+gitea-1.22.0"})
                    if self.headers.get("Authorization") != f"token {fake.token}":
                        return self._send(401, {"message": "token is required"})
                    return fake.route(self, method, path, query, body)

            def do_GET(self):
                self._dispatch("GET")

            def do_POST(self):
                self._dispatch("POST")

            def do_PATCH(self):
                self._dispatch("PATCH")

        return Handler

    def route(self, h, method, path, query, body):
        if path == "/api/v1/user" and method == "GET":
            return h._send(200, {"login": "pdfcraft-sync", "id": 7})
        if not path.startswith(self.prefix):
            return h._send(404, {"message": "not found"})
        rest = path[len(self.prefix):]
        if rest == "" and method == "GET":
            return h._send(200, {"full_name": "sefr/pdfcraft", "permissions": {"admin": False, "push": True, "pull": True}})
        if rest == "" and method == "PATCH":
            return h._send(403, {"message": "user should be an owner or a collaborator with admin write"})
        if rest == "/branch_protections":
            return h._send(403, {"message": "admin access required"})
        match = re.fullmatch(r"/branches/(.+)", rest)
        if match and method == "GET":
            name = urllib.parse.unquote(match.group(1))
            tip = self._tip(name)
            return h._send(200, {"name": name, "commit": {"id": tip}}) if tip else h._send(404, {"message": "branch not found"})
        if rest == "/pulls" and method == "GET":
            items = [self.pull_json(p) for p in sorted(self.pulls.values(), key=lambda p: p["number"])
                     if query.get("state", "open") in ("all", p["state"])]
            return h._send(200, self._page(items, query))
        if rest == "/pulls" and method == "POST":
            head, base = body.get("head"), body.get("base")
            if not self._tip(head or "") or not self._tip(base or ""):
                return h._send(404, {"message": "branch does not exist"})
            for p in self.pulls.values():
                if p["state"] == "open" and p["head"] == head and p["base"] == base:
                    return h._send(409, {"message": f"pull request already exists for these targets [id: {p['number']}]"})
            index = self.add_pull(head, base, body.get("title", ""), body.get("body", ""))
            return h._send(201, self.pull_json(self.pulls[index]))
        match = re.fullmatch(r"/pulls/([0-9]+)", rest)
        if match:
            pr = self.pulls.get(int(match.group(1)))
            if pr is None:
                return h._send(404, {"message": "not found"})
            if method == "PATCH":
                for key in ("title", "body", "state"):
                    if key in body:
                        pr[key] = body[key]
            return h._send(200, self.pull_json(pr))
        if rest == "/issues" and method == "GET":
            items = [dict(i) for i in sorted(self.issues.values(), key=lambda i: i["number"])
                     if query.get("state", "open") in ("all", i["state"])]
            return h._send(200, self._page(items, query))
        if rest == "/issues" and method == "POST":
            with self.lock:
                index = self.next_index
                self.next_index += 1
                self.issues[index] = {"number": index, "title": body.get("title", ""), "body": body.get("body", ""), "state": "open"}
                self.comments[index] = []
            return h._send(201, dict(self.issues[index]))
        match = re.fullmatch(r"/issues/([0-9]+)", rest)
        if match and method == "PATCH":
            issue = self.issues.get(int(match.group(1)))
            if issue is None:
                return h._send(404, {"message": "not found"})
            for key in ("title", "body", "state"):
                if key in body:
                    issue[key] = body[key]
            return h._send(201, dict(issue))
        match = re.fullmatch(r"/issues/([0-9]+)/comments", rest)
        if match:
            index = int(match.group(1))
            if index not in self.comments:
                return h._send(404, {"message": "not found"})
            if method == "POST":
                self.comments[index].append({"id": len(self.comments[index]) + 1, "body": body.get("body", "")})
                return h._send(201, self.comments[index][-1])
            return h._send(200, list(self.comments[index]))
        return h._send(404, {"message": "not found"})

    @staticmethod
    def _page(items, query):
        page = max(int(query.get("page", 1)), 1)
        limit = max(min(int(query.get("limit", 30)), 50), 1)
        return items[(page - 1) * limit: page * limit]


class FakeGitHub:
    """GET /repos/{owner}/{repo}/pulls/{n}; ``on_get`` lets a test change the PR mid-run."""

    def __init__(self, token: str = GITHUB_TOKEN):
        self.token = token
        self.pulls: dict[tuple[str, int], dict] = {}
        self.gets = 0
        self.headers: list[dict[str, str]] = []  # each request's headers, names lower-cased
        self.on_get = None  # callable(count) -> None, before answering
        self.server = _Server(self._handler())

    @property
    def url(self) -> str:
        return self.server.url

    def stop(self) -> None:
        self.server.stop()

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                match = re.fullmatch(r"/repos/([^/]+)/([^/]+)/pulls/([0-9]+)", urllib.parse.urlparse(self.path).path)
                fake.headers.append({k.lower(): v for k, v in self.headers.items()})
                if self.headers.get("Authorization") != f"Bearer {fake.token}":
                    status, payload = 401, {"message": "Bad credentials"}
                elif not match:
                    status, payload = 404, {"message": "Not Found"}
                else:
                    fake.gets += 1
                    if fake.on_get:
                        fake.on_get(fake.gets)
                    key = (f"{match.group(1)}/{match.group(2)}", int(match.group(3)))
                    pr = fake.pulls.get(key)
                    status, payload = (200, pr) if pr else (404, {"message": "Not Found"})
                data = json.dumps(payload).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        return Handler


class RecordingServer:
    """Answers every request with 200 ``{}`` and records its headers: a redirect target or a
    stand-in for a host that must never receive credentials."""

    def __init__(self) -> None:
        self.headers: list[dict[str, str]] = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _answer(self):
                fake.headers.append({k.lower(): v for k, v in self.headers.items()})
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")

            do_GET = do_POST = do_PATCH = _answer

        self.server = _Server(Handler)

    @property
    def url(self) -> str:
        return self.server.url

    def stop(self) -> None:
        self.server.stop()


class GitHttpServer:
    """Git's smart HTTP protocol over 127.0.0.1, served by ``git http-backend`` as CGI.

    Every request's headers are recorded. ``require`` maps header names to the values a request
    must carry: a missing Cloudflare header answers 302 to ``redirect_to`` (like Access), a wrong
    or missing ``Authorization`` answers 401. ``fail_status`` makes every request fail that way
    after it was recorded.
    """

    def __init__(self, project_root: str, require: dict[str, str] | None = None,
                 redirect_to: str = "http://127.0.0.1:9/") -> None:
        self.project_root = project_root
        self.require = dict(require or {})
        self.redirect_to = redirect_to
        self.fail_status: int | None = None
        self.headers: list[dict[str, str]] = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _plain(self, status, text, extra=None):
                data = text.encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "text/plain")
                self.send_header("Content-Length", str(len(data)))
                for key, value in (extra or {}).items():
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(data)

            def _cgi(self):
                fake.headers.append({k.lower(): v for k, v in self.headers.items()})
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length) if length else b""
                for name, value in fake.require.items():
                    if self.headers.get(name) != value:
                        if name.lower().startswith("cf-access-"):
                            return self._plain(302, "Sign in", {"Location": fake.redirect_to})
                        return self._plain(401, "Unauthorized", {"WWW-Authenticate": 'Basic realm="x"'})
                if fake.fail_status:
                    return self._plain(fake.fail_status, "injected failure")
                parsed = urllib.parse.urlparse(self.path)
                env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
                env.update({
                    "GIT_PROJECT_ROOT": fake.project_root, "GIT_HTTP_EXPORT_ALL": "1",
                    "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                    "REQUEST_METHOD": self.command, "PATH_INFO": parsed.path,
                    "QUERY_STRING": parsed.query, "CONTENT_TYPE": self.headers.get("Content-Type", ""),
                    "CONTENT_LENGTH": str(len(body)), "REMOTE_USER": "pdfcraft-sync", "REMOTE_ADDR": "127.0.0.1",
                })
                if self.headers.get("Content-Encoding"):
                    env["HTTP_CONTENT_ENCODING"] = self.headers["Content-Encoding"]
                if self.headers.get("Git-Protocol"):
                    env["GIT_PROTOCOL"] = self.headers["Git-Protocol"]
                proc = subprocess.run(["git", "http-backend"], input=body, env=env,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
                head, _, payload = proc.stdout.partition(b"\r\n\r\n")
                if not _:
                    head, _, payload = proc.stdout.partition(b"\n\n")
                status, headers = 200, []
                for line in head.decode("latin-1").splitlines():
                    key, _, value = line.partition(":")
                    if key.lower() == "status":
                        status = int(value.strip().split()[0])
                    elif key:
                        headers.append((key, value.strip()))
                self.send_response(status)
                for key, value in headers:
                    self.send_header(key, value)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            do_GET = do_POST = _cgi

        self.server = _Server(Handler)

    @property
    def url(self) -> str:
        return self.server.url

    def stop(self) -> None:
        self.server.stop()


def github_pr(number: int, sha: str, *, head_ref: str = "claude/feature-x", head_repo: str = "sefr-14/pdfcraft",
              base_ref: str = "Main", state: str = "open", merged: bool = False, title: str = "Add a feature",
              body: str = "Implements the feature.") -> dict:
    return {
        "number": number,
        "state": state,
        "merged": merged,
        "merged_at": "2026-01-02T00:00:00Z" if merged else None,
        "merge_commit_sha": sha if merged else None,
        "title": title,
        "body": body,
        "html_url": f"https://github.com/sefr-14/pdfcraft/pull/{number}",
        "base": {"ref": base_ref, "repo": {"full_name": "sefr-14/pdfcraft"}},
        "head": {"ref": head_ref, "sha": sha, "repo": {"full_name": head_repo} if head_repo else None},
    }


def quiet_reporter(redactor: Redactor | None = None, actions: bool = False) -> Reporter:
    return Reporter(redactor or Redactor(), stream=io.StringIO(), actions=actions)


class SyncTestCase(unittest.TestCase):
    """Sets up remotes and fake servers; allows file:// for the scripts' Git."""

    def setUp(self) -> None:
        self._env = dict(os.environ)
        os.environ["SYNC_ALLOW_FILE_PROTOCOL"] = "always"
        os.environ.pop("GITHUB_ACTIONS", None)
        os.environ.pop("GITHUB_STEP_SUMMARY", None)
        os.environ.pop("CF_ACCESS_CLIENT_ID", None)  # hermetic: tests opt in to Cloudflare Access
        os.environ.pop("CF_ACCESS_CLIENT_SECRET", None)
        self.remotes = Remotes()
        self.forgejo = FakeForgejo(self.remotes.bare["forgejo"])
        self.addCleanup(self.forgejo.stop)
        self.addCleanup(self.remotes.cleanup)
        self.gits: list[Git] = []

    def tearDown(self) -> None:
        for g in self.gits:
            g.cleanup()
        os.environ.clear()
        os.environ.update(self._env)

    def new_git(self, **kwargs) -> Git:
        g = Git.temporary(**kwargs)
        self.gits.append(g)
        return g
