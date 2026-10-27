"""The Forgejo/GitHub HTTP client: typed errors, retries, pagination and redaction."""

from __future__ import annotations

import os
import unittest
from unittest import mock

from fakes import (
    CF_ID,
    CF_SECRET,
    FORGEJO_TOKEN,
    GITHUB_TOKEN,
    Fault,
    FakeForgejo,
    FakeGitHub,
    RecordingServer,
    Remotes,
    no_proxy_opener,
)

from common import CloudflareAccess, Redactor
from github_client import GitHubClient
from forgejo_client import (
    REQUIRED_OPERATIONS,
    ApiError,
    AuthError,
    ConflictError,
    ForbiddenError,
    ForgejoClient,
    NotFoundError,
    TransientError,
)


class ClientTests(unittest.TestCase):
    def setUp(self):
        self.remotes = Remotes()
        self.addCleanup(self.remotes.cleanup)
        self.fake = FakeForgejo(self.remotes.bare["forgejo"])
        self.addCleanup(self.fake.stop)
        self.sleeps: list[float] = []
        self.redactor = Redactor()
        self.redactor.add_credential("pdfcraft-sync", FORGEJO_TOKEN)

    def client(self, token=FORGEJO_TOKEN, **kwargs) -> ForgejoClient:
        return ForgejoClient(self.fake.url, "sefr/pdfcraft", token, opener=no_proxy_opener(),
                             sleep=self.sleeps.append, redactor=self.redactor, **kwargs)

    def attempts(self, method: str, suffix: str) -> int:
        return len([r for r in self.fake.requests if r[0] == method and r[1].endswith(suffix)])

    def test_success(self):
        self.assertEqual(self.client().current_user()["login"], "pdfcraft-sync")
        self.assertTrue(self.client().get_repo()["permissions"]["push"])

    def test_401_is_auth_error_not_retried_and_redacted(self):
        self.fake.faults.append(Fault("GET", r"/user$", 401, body={"message": f"invalid token {FORGEJO_TOKEN}"}, times=5))
        with self.assertRaises(AuthError) as ctx:
            self.client().current_user()
        self.assertEqual(self.attempts("GET", "/user"), 1)
        self.assertNotIn(FORGEJO_TOKEN, str(ctx.exception))
        self.assertIn("expired", str(ctx.exception))

    def test_wrong_token(self):
        with self.assertRaises(AuthError):
            self.client(token="nope-nope").current_user()

    def test_typed_errors(self):
        for status, exc in ((403, ForbiddenError), (404, NotFoundError), (409, ConflictError), (422, ConflictError), (418, ApiError)):
            with self.subTest(status=status):
                self.fake.faults.append(Fault("GET", r"/user$", status))
                with self.assertRaises(exc):
                    self.client().current_user()

    def test_get_retries_5xx_then_succeeds(self):
        self.fake.faults.append(Fault("GET", r"/user$", 503, times=2))
        self.assertEqual(self.client().current_user()["login"], "pdfcraft-sync")
        self.assertEqual(self.attempts("GET", "/user"), 3)
        self.assertEqual(self.sleeps, [2.0, 4.0])

    def test_retry_exhaustion(self):
        self.fake.faults.append(Fault("GET", r"/user$", 502, times=99))
        with self.assertRaises(TransientError):
            self.client(retries=3).current_user()
        self.assertEqual(self.attempts("GET", "/user"), 4)

    def test_retry_after_is_honoured_and_capped(self):
        self.fake.faults.append(Fault("GET", r"/user$", 429, headers={"Retry-After": "7"}))
        self.fake.faults.append(Fault("GET", r"/user$", 429, headers={"Retry-After": "86400"}))
        self.client().current_user()
        self.assertEqual(self.sleeps, [7.0, 60.0])

    def test_post_is_not_retried_on_ambiguous_failure(self):
        self.fake.faults.append(Fault("POST", r"/issues$", 500, times=5))
        with self.assertRaises(TransientError):
            self.client().create_issue(title="t", body="b")
        self.assertEqual(self.attempts("POST", "/issues"), 1)
        self.assertEqual(self.fake.issues, {})

    def test_post_is_retried_on_429(self):
        self.fake.faults.append(Fault("POST", r"/issues$", 429))
        self.client().create_issue(title="t", body="b")
        self.assertEqual(len(self.fake.issues), 1)

    def test_timeout_is_retried_for_get(self):
        self.fake.faults.append(Fault("GET", r"/user$", 200, delay=1.0))
        self.assertEqual(self.client(timeout=0.3).current_user()["login"], "pdfcraft-sync")
        self.assertEqual(len(self.sleeps), 1)

    def test_network_failure_is_transient_and_redacted(self):
        client = ForgejoClient("http://127.0.0.1:9", "sefr/pdfcraft", FORGEJO_TOKEN, opener=no_proxy_opener(),
                               sleep=self.sleeps.append, redactor=self.redactor, retries=2)
        with self.assertRaises(TransientError) as ctx:
            client.current_user()
        self.assertNotIn(FORGEJO_TOKEN, str(ctx.exception))
        self.assertEqual(len(self.sleeps), 2)

    def test_pagination(self):
        sha = self.remotes.commit(None, {"a": "a"})
        self.remotes.set_ref("forgejo", "refs/heads/Main", sha)
        for i in range(120):
            self.fake.add_pull("Main", "Main", f"pr {i}", "")
        pulls = self.client().list_pulls("all")
        self.assertEqual(len(pulls), 120)
        self.assertEqual(len({p["number"] for p in pulls}), 120)

    def test_invalid_json(self):
        self.fake.faults.append(Fault("GET", r"/user$", 200, body=b"<html>not json</html>"))
        with self.assertRaises(ApiError):
            self.client().current_user()

    def test_comment_once(self):
        index = self.client().create_issue(title="t", body="b")["number"]
        self.assertTrue(self.client().comment_once(index, "<!-- m -->", "hello"))
        self.assertFalse(self.client().comment_once(index, "<!-- m -->", "hello"))
        self.assertEqual(len(self.fake.comments[index]), 1)

    # -- Cloudflare Access and redirects -----------------------------------------------------------

    def test_cloudflare_headers_sent_with_the_forgejo_token(self):
        self.fake.require_cf = (CF_ID, CF_SECRET)
        client = self.client(cf_access=CloudflareAccess(CF_ID, CF_SECRET))
        self.assertTrue(client.get_repo()["permissions"]["push"])
        client.create_issue(title="t", body="b")
        self.assertEqual(len(self.fake.headers), 2)
        for headers in self.fake.headers:
            self.assertEqual(headers["authorization"], f"token {FORGEJO_TOKEN}")
            self.assertEqual(headers["cf-access-client-id"], CF_ID)
            self.assertEqual(headers["cf-access-client-secret"], CF_SECRET)
        self.assertNotIn(CF_SECRET, client.base_url)

    def test_without_cloudflare_no_cloudflare_headers(self):
        self.client().get_repo()
        self.assertNotIn("cf-access-client-id", self.fake.headers[0])
        self.assertNotIn("cf-access-client-secret", self.fake.headers[0])

    def test_github_client_never_gets_cloudflare_headers(self):
        github = FakeGitHub()
        self.addCleanup(github.stop)
        self.client(cf_access=CloudflareAccess(CF_ID, CF_SECRET)).get_repo()
        client = GitHubClient(github.url, GITHUB_TOKEN, opener=no_proxy_opener(), sleep=lambda s: None)
        with self.assertRaises(NotFoundError):
            client.get_pull("sefr-14/pdfcraft", 1)
        self.assertTrue(github.headers)
        for headers in github.headers:
            self.assertNotIn("cf-access-client-id", headers)
            self.assertNotIn("cf-access-client-secret", headers)

    def test_access_redirect_is_refused_and_nothing_reaches_the_redirect_host(self):
        login = RecordingServer()
        self.addCleanup(login.stop)
        self.fake.require_cf = (CF_ID, CF_SECRET)
        self.fake.access_redirect = login.url + "/cdn-cgi/access/login"
        cases = (("no service token", None), ("wrong service token", CloudflareAccess(CF_ID, "wrong-secret-value")))
        for name, cf in cases:
            with self.subTest(name):
                self.fake.requests.clear()
                self.redactor.add("wrong-secret-value")
                with self.assertRaises(ApiError) as ctx:
                    self.client(cf_access=cf).list_pulls()
                text = str(ctx.exception)
                self.assertIn("HTTP 302", text)
                self.assertIn("redirect refused", text)
                self.assertNotIn("invalid JSON", text)
                self.assertNotIn("Sign in", text)  # no body
                self.assertNotIn(login.url, text)  # no location
                self.assertNotIn(FORGEJO_TOKEN, text)
                self.assertEqual(self.attempts("GET", "/pulls"), 1)  # not retried
        # Before the fix urllib followed the 302 and replayed Authorization (and any CF header).
        self.assertEqual(login.headers, [])

    def test_redirect_refused_on_the_built_in_opener_too(self):
        login = RecordingServer()
        self.addCleanup(login.stop)
        self.fake.faults.append(Fault("GET", r"/version$", 307, headers={"Location": login.url + "/x"}))
        with mock.patch.dict(os.environ, {"no_proxy": "127.0.0.1", "NO_PROXY": "127.0.0.1"}):
            client = ForgejoClient(self.fake.url, "sefr/pdfcraft", FORGEJO_TOKEN, sleep=self.sleeps.append)
            with self.assertRaises(ApiError):
                client.version()
        self.assertEqual(login.headers, [])

    def test_required_operations_cover_the_client(self):
        self.assertIn(("post", "/repos/{owner}/{repo}/pulls"), REQUIRED_OPERATIONS)
        self.assertEqual(len(set(REQUIRED_OPERATIONS)), len(REQUIRED_OPERATIONS))


if __name__ == "__main__":
    unittest.main()
