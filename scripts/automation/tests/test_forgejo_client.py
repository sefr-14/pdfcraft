"""The Forgejo/GitHub HTTP client: typed errors, retries, pagination and redaction."""

from __future__ import annotations

import unittest

from fakes import FORGEJO_TOKEN, Fault, FakeForgejo, Remotes, no_proxy_opener

from common import Redactor
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

    def test_required_operations_cover_the_client(self):
        self.assertIn(("post", "/repos/{owner}/{repo}/pulls"), REQUIRED_OPERATIONS)
        self.assertEqual(len(set(REQUIRED_OPERATIONS)), len(REQUIRED_OPERATIONS))


if __name__ == "__main__":
    unittest.main()
