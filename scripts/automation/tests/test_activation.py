"""Activation tooling: the read-only API check and the bot acceptance test, against the fake."""

from __future__ import annotations

import contextlib
import io
import os
import unittest

from fakes import FORGEJO_TOKEN, SyncTestCase, git

import forgejo_acceptance
import forgejo_api_check
from forgejo_client import REQUIRED_OPERATIONS


def swagger(missing: tuple[str, str] | None = None) -> dict:
    paths: dict = {}
    for method, path in REQUIRED_OPERATIONS:
        if (method, path) != missing:
            paths.setdefault(path, {})[method] = {}
    return {"basePath": "/api/v1", "paths": paths}


class ApiCheck(unittest.TestCase):
    def test_all_present_on_15(self):
        out = io.StringIO()
        self.assertTrue(forgejo_api_check.check("15.0.1+gitea-1.22.0", swagger(), out))
        self.assertNotIn("FAIL", out.getvalue())

    def test_missing_operation_fails(self):
        out = io.StringIO()
        self.assertFalse(forgejo_api_check.check("15.0.1", swagger(("post", "/repos/{owner}/{repo}/pulls")), out))
        self.assertIn("FAIL  POST  /repos/{owner}/{repo}/pulls is missing", out.getvalue())

    def test_other_major_version_fails(self):
        self.assertFalse(forgejo_api_check.check("14.0.0", swagger(), io.StringIO()))
        self.assertFalse(forgejo_api_check.check("garbage", swagger(), io.StringIO()))


class Acceptance(SyncTestCase):
    def setUp(self):
        super().setUp()
        self.main = self.remotes.commit(None, {"README": "base\n"}, "base")
        self.remotes.set_ref("forgejo", "refs/heads/Main", self.main)
        self.forgejo.swagger = swagger()
        os.environ.update({
            "FORGEJO_URL": self.forgejo.url, "FORGEJO_GIT_URL": self.remotes.url("forgejo"),
            "FORGEJO_USER": "pdfcraft-sync", "FORGEJO_TOKEN": FORGEJO_TOKEN,
            "no_proxy": "127.0.0.1", "NO_PROXY": "127.0.0.1",
        })

    def run_main(self, *args: str) -> tuple[int, str]:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = forgejo_acceptance.main(["--allow-local", *args])
        return code, out.getvalue()

    def test_read_only_pass_writes_nothing(self):
        code, out = self.run_main()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.forgejo.pulls, {})
        self.assertIsNone(self.remotes.ref("forgejo", "refs/heads/github-pr/0"))
        self.assertFalse([r for r in self.forgejo.requests if r[0] != "GET"])
        self.assertNotIn(FORGEJO_TOKEN, out)

    def test_write_pass_makes_a_real_change_and_never_merges(self):
        code, out = self.run_main("--confirm-writes")
        self.assertEqual(code, 0, out)
        branch = self.remotes.ref("forgejo", "refs/heads/github-pr/0")
        self.assertIsNotNone(branch)
        # Correction 2: the branch carries a genuine diff against Main.
        changed = git(self.remotes.bare["forgejo"], "diff", "--name-only", self.main, branch)
        self.assertEqual(changed, "scripts/automation/acceptance/ACCEPTANCE.txt")
        self.assertEqual(git(self.remotes.bare["forgejo"], "rev-parse", f"{branch}^"), self.main)
        pr = self.forgejo.pulls[1]
        self.assertEqual((pr["state"], pr["merged"]), ("closed", False))
        self.assertIn("(edited)", pr["title"])
        issue = next(iter(self.forgejo.issues.values()))
        self.assertEqual(issue["state"], "closed")
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/Main"), self.main)
        self.assertNotIn(FORGEJO_TOKEN, out)

    def test_existing_test_branch_is_not_reused(self):
        self.remotes.set_ref("forgejo", "refs/heads/github-pr/0", self.main)
        code, out = self.run_main("--confirm-writes")
        self.assertEqual(code, 1)
        self.assertIn("already exists", out)
        self.assertEqual(self.forgejo.pulls, {})

    def test_admin_token_fails_the_read_pass(self):
        real_route = self.forgejo.route

        def route(h, method, path, query, body):
            if path.endswith("/pdfcraft") and method == "GET":
                return h._send(200, {"permissions": {"admin": True, "push": True}})
            return real_route(h, method, path, query, body)

        self.forgejo.route = route
        code, out = self.run_main()
        self.assertEqual(code, 1)
        self.assertIn("need push=true and admin=false", out)


if __name__ == "__main__":
    unittest.main()
