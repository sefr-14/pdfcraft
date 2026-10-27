"""Git over HTTP with real Git: the Forgejo Basic header and the Cloudflare Access headers.

A local smart-HTTP server (``git http-backend``) records every request's headers and behaves like
Cloudflare Access in front of Forgejo: no service token, a 302 to a login page; no or a wrong
Basic header, a 401. These tests prove what Git actually sends, not what the environment says.
They cannot prove Cloudflare's own behaviour; that is checked on the first live run.
"""

from __future__ import annotations

import base64
import contextlib
import io
import os
import shutil
import subprocess
import unittest

from fakes import CF_ID, CF_SECRET, FORGEJO_TOKEN, GitHttpServer, RecordingServer, SyncTestCase, git

from common import CloudflareAccess, GitAuth, GitError, PushRefused, Redactor, Reporter

USER = "pdfcraft-sync"
BASIC = "Basic " + base64.b64encode(f"{USER}:{FORGEJO_TOKEN}".encode()).decode()
CF = CloudflareAccess(CF_ID, CF_SECRET)
SECRETS = (FORGEJO_TOKEN, CF_ID, CF_SECRET, BASIC.split()[1])


def _has_http_backend() -> bool:
    proc = subprocess.run(["git", "http-backend"], env={"REQUEST_METHOD": "GET", "PATH": os.environ.get("PATH", "")},
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    return shutil.which("git") is not None and proc.returncode != 127


@unittest.skipUnless(_has_http_backend(), "git http-backend is not available")
class GitHttpAuth(SyncTestCase):
    def setUp(self) -> None:
        super().setUp()
        for name in ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"):
            os.environ.pop(name, None)  # the servers are on 127.0.0.1; restored by tearDown
        r = self.remotes
        self.c0 = r.commit(None, {"README": "base\n"}, "base")
        r.set_ref("forgejo", "refs/heads/Main", self.c0)
        r.set_ref("github", "refs/heads/Main", self.c0)
        self.c1 = r.commit(self.c0, {"feature.txt": "one\n"}, "feature")
        git(self.remotes.bare["forgejo"], "config", "http.receivepack", "true")
        self.login = RecordingServer()  # where "Access" redirects to
        self.addCleanup(self.login.stop)
        self.other = RecordingServer()
        self.addCleanup(self.other.stop)

    def serve(self, name: str, require: dict[str, str] | None = None) -> GitHttpServer:
        server = GitHttpServer(self.remotes.root, require, redirect_to=self.login.url + "/cdn-cgi/access/login")
        self.addCleanup(server.stop)
        server.repo_url = f"{server.url}/{name}.git"
        return server

    def forgejo_http(self, *, cf: bool = True) -> GitHttpServer:
        require = {"Authorization": BASIC}
        if cf:
            require.update({"CF-Access-Client-Id": CF_ID, "CF-Access-Client-Secret": CF_SECRET})
        return self.serve("forgejo", require)

    def new_http_git(self, server: GitHttpServer, cf: CloudflareAccess | None = CF):
        redactor = Redactor()
        redactor.add_credential(USER, FORGEJO_TOKEN)
        if cf is not None:
            cf.register(redactor)
        g = self.new_git(auth=GitAuth(server.url, USER, FORGEJO_TOKEN, cf), redactor=redactor)
        g.run(["config", "protocol.http.allow", "always"])  # tests only: production allows https
        return g

    def assertAllAuthenticated(self, server: GitHttpServer, cf: bool = True) -> None:
        self.assertTrue(server.headers)
        for headers in server.headers:
            self.assertEqual(headers.get("authorization"), BASIC)
            if cf:
                self.assertEqual(headers.get("cf-access-client-id"), CF_ID)
                self.assertEqual(headers.get("cf-access-client-secret"), CF_SECRET)
            else:
                self.assertNotIn("cf-access-client-id", headers)
                self.assertNotIn("cf-access-client-secret", headers)

    def assertNoCredentials(self, headers_list) -> None:
        for headers in headers_list:
            self.assertNotIn("authorization", headers)
            self.assertNotIn("cf-access-client-id", headers)
            self.assertNotIn("cf-access-client-secret", headers)

    # -- both credentials reach Forgejo on every request ------------------------------------------

    def test_ls_remote_fetch_and_push_send_basic_and_both_cloudflare_headers(self):
        server = self.forgejo_http()
        g = self.new_http_git(server)
        self.assertEqual(g.ls_remote(server.repo_url, "refs/heads/Main"), self.c0)
        self.assertEqual(g.fetch(server.repo_url, "refs/heads/Main", "refs/sync/main"), self.c0)
        g.add_alternate(os.path.join(self.remotes.work, ".git", "objects"))
        g.push(server.repo_url, self.c1, "refs/heads/github-pr/1")
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/github-pr/1"), self.c1)
        methods = {h.get("content-type", "") for h in server.headers}
        self.assertTrue(any("receive-pack" in m for m in methods), "the push itself was recorded")
        self.assertAllAuthenticated(server)

    def test_without_cloudflare_basic_auth_still_works(self):
        server = self.forgejo_http(cf=False)
        g = self.new_http_git(server, cf=None)
        self.assertEqual(g.fetch(server.repo_url, "refs/heads/Main", "refs/sync/main"), self.c0)
        g.add_alternate(os.path.join(self.remotes.work, ".git", "objects"))
        g.push(server.repo_url, self.c1, "refs/heads/github-pr/2")
        self.assertAllAuthenticated(server, cf=False)

    def test_the_cloudflare_headers_are_required_by_the_fake(self):
        # Without the service token the "Access" front answers 302, and Git must not follow it.
        server = self.forgejo_http()
        g = self.new_http_git(server, cf=None)
        with self.assertRaises(GitError):
            g.ls_remote(server.repo_url, "refs/heads/Main")
        self.assertEqual(self.login.headers, [])

    # -- credentials never leave the Forgejo prefix ----------------------------------------------

    def test_credentials_are_not_sent_to_other_hosts(self):
        forgejo = self.forgejo_http()
        github = self.serve("github")  # GitHub/upstream stand-in: another port, so another prefix
        g = self.new_http_git(forgejo)
        self.assertEqual(g.fetch(github.repo_url, "refs/heads/Main", "refs/sync/github"), self.c0)
        self.assertEqual(g.fetch(forgejo.repo_url, "refs/heads/Main", "refs/sync/main"), self.c0)
        self.assertTrue(github.headers)
        self.assertNoCredentials(github.headers)
        self.assertAllAuthenticated(forgejo)

    def test_redirects_are_refused_with_and_without_cloudflare(self):
        for cf in (CF, None):
            with self.subTest(cloudflare=cf is not None):
                self.login.headers.clear()
                # Every request is redirected, as Access does when it rejects the service token.
                server = self.serve("forgejo", {"CF-Access-Client-Id": "a-token-nobody-has"})
                g = self.new_http_git(server, cf=cf)
                with self.assertRaises(GitError):
                    g.fetch(server.repo_url, "refs/heads/Main", "refs/sync/main")
                self.assertTrue(server.headers)
                self.assertEqual(self.login.headers, [], "Git followed the redirect")

    def test_nothing_is_written_to_repository_config(self):
        server = self.forgejo_http()
        g = self.new_http_git(server)
        g.fetch(server.repo_url, "refs/heads/Main", "refs/sync/main")
        with open(os.path.join(g.git_dir, "config"), encoding="utf-8") as handle:
            config = handle.read()
        for secret in SECRETS:
            self.assertNotIn(secret, config)
        self.assertNotIn("extraheader", config.lower())
        self.assertNotIn("followredirects", config.lower())

    # -- the push guard and the server's checks are unchanged -------------------------------------

    def test_push_guard_and_non_fast_forward_still_refused(self):
        server = self.forgejo_http()
        g = self.new_http_git(server)
        g.add_alternate(os.path.join(self.remotes.work, ".git", "objects"))
        with self.assertRaises(PushRefused):
            g.push(server.repo_url, self.c1, "refs/heads/Main")
        g.push(server.repo_url, self.c1, "refs/heads/github-pr/3")
        other = self.remotes.commit(self.c0, {"other.txt": "x\n"}, "unrelated")
        git(self.remotes.bare["forgejo"], "config", "receive.denyNonFastForwards", "true")
        with self.assertRaises(GitError):
            g.push(server.repo_url, other, "refs/heads/github-pr/3")
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/github-pr/3"), self.c1)

    # -- nothing user-visible carries a credential -------------------------------------------------

    def test_git_failures_are_redacted_in_errors_logs_and_summaries(self):
        summary = os.path.join(self.remotes.root, "summary.md")
        os.environ["GITHUB_STEP_SUMMARY"] = summary
        for status in (401, 500):
            with self.subTest(status=status):
                server = self.forgejo_http()
                server.fail_status = status
                g = self.new_http_git(server)
                with self.assertRaises(GitError) as ctx:
                    g.fetch(server.repo_url, "refs/heads/Main", "refs/sync/main")
                self.assertAllAuthenticated(server)  # the headers really were sent
                out = io.StringIO()
                rep = Reporter(g.redactor, stream=out, actions=True)
                rep.error(f"sync failed: {ctx.exception}")
                rep.add_summary(f"**failed** — {ctx.exception} {ctx.exception.stderr}")
                rep.finish()
                with open(summary, encoding="utf-8") as handle:
                    summary_text = handle.read()
                logged = "\n".join(l for l in out.getvalue().splitlines() if not l.startswith("::add-mask::"))
                visible = (str(ctx.exception), repr(ctx.exception), ctx.exception.stderr, logged, summary_text,
                           repr(g), repr(g.auth))
                for text in visible:
                    for secret in SECRETS:
                        self.assertNotIn(secret, text)

    def test_git_trace_cannot_be_enabled_from_the_environment(self):
        server = self.forgejo_http()
        g = self.new_http_git(server)
        server.fail_status = 500
        with mock_env(GIT_TRACE="1", GIT_CURL_VERBOSE="1", GIT_TRACE_CURL="1", GIT_TRACE_PACKET="1"):
            proc = g.run_raw(["ls-remote", "--end-of-options", server.repo_url, "refs/heads/Main"])
        raw = (proc.stdout + proc.stderr).decode("utf-8", "replace")
        self.assertNotEqual(proc.returncode, 0)
        for secret in SECRETS:
            self.assertNotIn(secret, raw)


@contextlib.contextmanager
def mock_env(**values):
    saved = {k: os.environ.get(k) for k in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


if __name__ == "__main__":
    unittest.main()
