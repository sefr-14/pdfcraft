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

from fakes import CF_ID, CF_SECRET, FORGEJO_TOKEN, GITHUB_TOKEN, GitHttpServer, RecordingServer, SyncTestCase, git

from common import CloudflareAccess, GitAuth, GitError, PushRefused, Redactor, Reporter
from sync_claude_pr import GITHUB_TOKEN_USER

USER = "pdfcraft-sync"
BASIC = "Basic " + base64.b64encode(f"{USER}:{FORGEJO_TOKEN}".encode()).decode()
CF = CloudflareAccess(CF_ID, CF_SECRET)
SECRETS = (FORGEJO_TOKEN, CF_ID, CF_SECRET, BASIC.split()[1])


def _has_http_backend() -> bool:
    proc = subprocess.run(["git", "http-backend"], env={"REQUEST_METHOD": "GET", "PATH": os.environ.get("PATH", "")},
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    return shutil.which("git") is not None and proc.returncode != 127


class _HttpCase(SyncTestCase):
    """The servers and helpers both suites share (no tests of its own)."""

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



@unittest.skipUnless(_has_http_backend(), "git http-backend is not available")
class GitHttpAuth(_HttpCase):
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


GH_BASIC = "Basic " + base64.b64encode(f"{GITHUB_TOKEN_USER}:{GITHUB_TOKEN}".encode()).decode()
GH_SECRETS = (GITHUB_TOKEN, GH_BASIC.split()[1], f"{GITHUB_TOKEN_USER}:{GITHUB_TOKEN}")


@unittest.skipUnless(_has_http_backend(), "git http-backend is not available")
class GitHubSourceAuth(_HttpCase):
    """The bridge's fetch of a PR head from its own GitHub repository once that is private.

    One "GitHub" host serves the private source (``github.git``) beside a public repository on
    the same host (``upstream.git``); Forgejo is another host, behind its own credentials.
    """

    def setUp(self) -> None:
        super().setUp()
        self.remotes.set_ref("github", "refs/pull/7/head", self.c1)
        self.remotes.set_ref("upstream", "refs/heads/main", self.c0)

    def github_http(self, *, private: bool = True, redirect: bool = False) -> GitHttpServer:
        repo_require = {}
        if private:
            repo_require["github.git"] = {"Authorization": GH_BASIC}
        if redirect:
            # A header nobody has makes the fake answer 302 for that repository, as a sign-in would.
            repo_require["github.git"] = {"CF-Access-Client-Id": "nobody-has-this"}
        server = GitHttpServer(self.remotes.root, None, redirect_to=self.login.url + "/login", repo_require=repo_require)
        self.addCleanup(server.stop)
        server.repo_url = f"{server.url}/github.git"
        server.upstream_url = f"{server.url}/upstream.git"
        return server

    def bridge_git(self, github: GitHttpServer, forgejo: GitHttpServer | None, *, token: bool = True):
        """A Git runner set up as ``sync_claude_pr.main`` sets it up."""
        redactor = Redactor()
        redactor.add_credential(USER, FORGEJO_TOKEN)
        CF.register(redactor)
        redactor.add_credential(GITHUB_TOKEN_USER, GITHUB_TOKEN)
        auth = GitAuth(forgejo.url, USER, FORGEJO_TOKEN, CF) if forgejo is not None else None
        extra = (GitAuth(github.repo_url, GITHUB_TOKEN_USER, GITHUB_TOKEN),) if token else ()
        g = self.new_git(auth=auth, extra_auth=extra, redactor=redactor)
        g.run(["config", "protocol.http.allow", "always"])  # tests only: production allows https
        return g

    def assertGitHubOnly(self, headers_list) -> None:
        self.assertTrue(headers_list)
        for headers in headers_list:
            self.assertEqual(headers.get("authorization"), GH_BASIC)
            self.assertNotIn("cf-access-client-id", headers)
            self.assertNotIn("cf-access-client-secret", headers)

    def test_a_private_source_is_fetched_with_the_actions_token(self):
        github = self.github_http()
        g = self.bridge_git(github, None)
        self.assertEqual(g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source"), self.c1)
        self.assertGitHubOnly(github.headers)

    def test_without_the_token_a_private_source_fails_and_nothing_is_fetched(self):
        github = self.github_http()
        g = self.bridge_git(github, None, token=False)
        with self.assertRaises(GitError):
            g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source")
        self.assertIsNone(g.rev_parse_commit("refs/sync/source"))
        self.assertTrue(github.headers)
        self.assertNoCredentials(github.headers)

    def test_a_rejected_token_fails_without_an_anonymous_retry(self):
        github = self.github_http()
        g = self.new_git(extra_auth=(GitAuth(github.repo_url, GITHUB_TOKEN_USER, "ghs_wrongwrongwrong"),))
        g.run(["config", "protocol.http.allow", "always"])
        with self.assertRaises(GitError):
            g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source")
        for headers in github.headers:
            self.assertIn("authorization", headers, "an anonymous request was made")

    def test_each_credential_reaches_only_its_own_repository(self):
        github = self.github_http()
        forgejo = self.forgejo_http()
        g = self.bridge_git(github, forgejo)
        self.assertEqual(g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source"), self.c1)
        self.assertEqual(g.fetch(forgejo.repo_url, "refs/heads/Main", "refs/sync/main"), self.c0)
        # Upstream on the same host as the source: a public repository gets nothing.
        self.assertEqual(g.fetch(github.upstream_url, "refs/heads/main", "refs/sync/upstream"), self.c0)
        by_repo: dict[str, list] = {}
        for repo, headers in github.requests:
            by_repo.setdefault(repo, []).append(headers)
        self.assertGitHubOnly(by_repo["github.git"])
        self.assertNoCredentials(by_repo["upstream.git"])
        # Forgejo gets its own Basic and Cloudflare headers, never the GitHub token.
        self.assertAllAuthenticated(forgejo)
        for headers in forgejo.headers:
            self.assertNotEqual(headers.get("authorization"), GH_BASIC)

    def test_a_redirect_from_the_source_is_not_followed(self):
        github = self.github_http(redirect=True)
        g = self.bridge_git(github, None)
        with self.assertRaises(GitError):
            g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source")
        self.assertTrue(github.headers)
        self.assertEqual(self.login.headers, [], "Git followed the redirect")

    def test_a_public_source_still_works_with_or_without_the_token(self):
        for token in (True, False):
            with self.subTest(token=token):
                github = self.github_http(private=False)
                g = self.bridge_git(github, None, token=token)
                self.assertEqual(g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source"), self.c1)
                if not token:
                    self.assertNoCredentials(github.headers)

    def test_the_push_guard_holds_with_both_credentials(self):
        github = self.github_http()
        forgejo = self.forgejo_http()
        g = self.bridge_git(github, forgejo)
        g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source")
        with self.assertRaises(PushRefused):
            g.push(forgejo.repo_url, self.c1, "refs/heads/Main")
        g.push(forgejo.repo_url, self.c1, "refs/heads/github-pr/7")
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/github-pr/7"), self.c1)
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/Main"), self.c0)
        for repo, headers in github.requests:
            self.assertNotIn("receive-pack", headers.get("content-type", ""), "nothing is pushed to GitHub")

    def test_no_token_appears_in_config_errors_logs_or_summaries(self):
        summary = os.path.join(self.remotes.root, "summary-gh.md")
        os.environ["GITHUB_STEP_SUMMARY"] = summary
        github = self.github_http()
        forgejo = self.forgejo_http()
        g = self.bridge_git(github, forgejo)
        g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/source")
        with open(os.path.join(g.git_dir, "config"), encoding="utf-8") as handle:
            config = handle.read()
        github.fail_status = 401
        with mock_env(GIT_TRACE="1", GIT_CURL_VERBOSE="1", GIT_TRACE_CURL="1", GIT_TRACE_PACKET="1"):
            with self.assertRaises(GitError) as ctx:
                g.fetch(github.repo_url, "refs/pull/7/head", "refs/sync/again")
            raw = g.run_raw(["ls-remote", "--end-of-options", github.repo_url, "refs/pull/7/head"])
        self.assertGitHubOnly(github.headers)  # the token really was sent
        out = io.StringIO()
        rep = Reporter(g.redactor, stream=out, actions=True)
        rep.error(f"sync failed: {ctx.exception}")
        rep.add_summary(f"**failed** — {ctx.exception} {ctx.exception.stderr}")
        rep.finish()
        with open(summary, encoding="utf-8") as handle:
            summary_text = handle.read()
        logged = "\n".join(l for l in out.getvalue().splitlines() if not l.startswith("::add-mask::"))
        visible = (config, str(ctx.exception), repr(ctx.exception), ctx.exception.stderr, logged, summary_text,
                   (raw.stdout + raw.stderr).decode("utf-8", "replace"), repr(g), repr(g.extra_auth))
        for text in visible:
            for secret in GH_SECRETS + SECRETS:
                self.assertNotIn(secret, text)
        self.assertNotIn("extraheader", config.lower())


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
