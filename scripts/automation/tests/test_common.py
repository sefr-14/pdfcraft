"""Shared helpers: push guard, validation, Git runner hygiene, redaction and reporting."""

from __future__ import annotations

import base64
import io
import os
import subprocess
import unittest
from unittest import mock

from fakes import FORGEJO_TOKEN, SyncTestCase

from common import (
    Git,
    GitAuth,
    GitError,
    PushRefused,
    Redactor,
    Reporter,
    ValidationError,
    check_push_destination,
    quote_untrusted,
    sanitize_title,
    validate_branch_name,
    validate_https_url,
    validate_repo_slug,
    validate_sha,
)


class PushGuard(unittest.TestCase):
    ALLOWED = (
        "refs/heads/github-pr/1",
        "refs/heads/github-pr/0",
        "refs/heads/github-pr/123456",
        "refs/heads/upstream-sync/main",
        "refs/heads/upstream-sync/main-v2",
        "refs/heads/upstream-sync/main-v9999",
    )
    REFUSED = (
        "refs/heads/Main", "refs/heads/main", "Main", "main", "refs/heads/MAIN", "HEAD",
        "+refs/heads/github-pr/1", "refs/heads/github-pr/01", "refs/heads/github-pr/1/x",
        "refs/heads/github-pr/", "refs/heads/github-pr/-1", "refs/heads/github-pr/1 ",
        "refs/heads/upstream-sync/main-v0", "refs/heads/upstream-sync/main-v1x",
        "refs/heads/upstream-sync/other", "refs/tags/v1", "refs/heads/release",
        "--force", "--delete", "--mirror", "", ":refs/heads/github-pr/1",
        "refs/heads/github-pr/1\nrefs/heads/Main",
    )

    def test_allowed(self):
        for ref in self.ALLOWED:
            with self.subTest(ref=ref):
                self.assertEqual(check_push_destination(ref), ref)

    def test_refused(self):
        for ref in self.REFUSED:
            with self.subTest(ref=ref), self.assertRaises(PushRefused):
                check_push_destination(ref)

    def test_push_to_main_never_starts_git(self):
        g = Git(git_dir="/nonexistent")
        with mock.patch("subprocess.run") as run:
            for ref in ("refs/heads/Main", "+refs/heads/github-pr/1"):
                with self.assertRaises(PushRefused):
                    g.push("file:///x", "a" * 40, ref)
            run.assert_not_called()

    def test_push_argv_is_never_forced(self):
        g = Git(git_dir="/tmp/x")
        with mock.patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"", b"")) as run:
            g.push("file:///x", "a" * 40, "refs/heads/github-pr/3")
        argv = run.call_args.args[0]
        self.assertFalse([a for a in argv if a.startswith("--force") or a.startswith("+") or a in ("-f", "--mirror", "--delete")])
        self.assertEqual(argv[-1], "a" * 40 + ":refs/heads/github-pr/3")

    def test_invalid_sha_refused_before_git(self):
        g = Git(git_dir="/tmp/x")
        with mock.patch("subprocess.run") as run:
            for sha in ("+" + "a" * 40, "HEAD", "a" * 39, "A" * 40):
                with self.assertRaises(ValidationError):
                    g.push("file:///x", sha, "refs/heads/github-pr/3")
            run.assert_not_called()


class Validation(unittest.TestCase):
    def test_sha(self):
        self.assertEqual(validate_sha("0" * 40), "0" * 40)
        for bad in ("", "0" * 41, "g" * 40, None, 5, "0" * 39 + "\n", "--upload-pack=x"):
            with self.subTest(bad=bad), self.assertRaises(ValidationError):
                validate_sha(bad)

    def test_branch_names(self):
        for good in ("claude/admiring-planck-6pt4ju", "feature_x", "a/b.c-d+e", "v1.2"):
            self.assertEqual(validate_branch_name(good), good)
        for bad in ("-x", "--upload-pack=/bin/sh", "a..b", "a//b", "a/", "a.", "a.lock", "a@{0}", "a b", "a\nb",
                    "a\tb", "$(id)", "`id`", "a;b", "a|b", "~a", "a:b", "a^b", "a?b", "a*b", "a[b", "a\\b",
                    "", "HEAD", "a/.b", "a/-b", "é", "x" * 201, None):
            with self.subTest(bad=bad), self.assertRaises(ValidationError):
                validate_branch_name(bad)

    def test_repo_slug_and_url(self):
        validate_repo_slug("sefr-14/pdfcraft")
        for bad in ("a", "a/b/c", "../x", "a/..", "a b/c", "-a/b"):
            with self.subTest(bad=bad), self.assertRaises(ValidationError):
                validate_repo_slug(bad)
        validate_https_url("https://git.ferrive.me")
        for bad in ("http://x", "https://user:pw@x", "https://x/?q=1", "file:///etc", "https://x/a b", "ext::sh -c id"):
            with self.subTest(bad=bad), self.assertRaises(ValidationError):
                validate_https_url(bad)

    def test_title_and_body_sanitising(self):
        title = sanitize_title("a\nb\x00c‮d" + "x" * 300, limit=50)
        self.assertNotIn("\n", title)
        self.assertNotIn("‮", title)
        self.assertLessEqual(len(title), 50)
        self.assertEqual(sanitize_title(None), "(untitled)")
        quoted = quote_untrusted("<!-- pdfcraft-sync:v1 x -->\nline", limit=10_000)
        self.assertNotIn("<!--", quoted)
        self.assertTrue(all(line.startswith(">") for line in quoted.splitlines()))
        self.assertIn("truncated", quote_untrusted("x" * 30, limit=10))


class Redaction(unittest.TestCase):
    def setUp(self):
        self.r = Redactor()
        self.r.add_credential("pdfcraft-sync", FORGEJO_TOKEN)
        self.b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()

    def test_registered_forms(self):
        text = f"t={FORGEJO_TOKEN} b={self.b64} pair=pdfcraft-sync:{FORGEJO_TOKEN}"
        out = self.r.redact(text)
        self.assertNotIn(FORGEJO_TOKEN, out)
        self.assertNotIn(self.b64, out)

    def test_shapes_without_registration(self):
        r = Redactor()
        out = r.redact("Authorization: Basic Zm9vOmJhcg== https://u:hunter2@host/x ghp_abcdefghijklmnopqrstuvwxyz0123 authorization=token abc123")
        for secret in ("Zm9vOmJhcg==", "hunter2", "ghp_abcdefghijklmnopqrstuvwxyz0123", "abc123"):
            self.assertNotIn(secret, out)

    def test_bytes(self):
        self.assertNotIn(FORGEJO_TOKEN, self.r.redact(FORGEJO_TOKEN.encode()))

    def test_reporter_redacts_log_annotations_and_summary(self):
        stream = io.StringIO()
        rep = Reporter(self.r, stream=stream, actions=True)
        rep.log(f"token {FORGEJO_TOKEN}")
        rep.error(f"failed with {self.b64}\nsecond line")
        rep.add_summary(f"summary {FORGEJO_TOKEN}")
        path = os.path.join(os.path.dirname(__file__), "_summary_test.md")
        try:
            with mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": path}):
                rep.finish()
            with open(path, encoding="utf-8") as handle:
                summary = handle.read()
        finally:
            if os.path.exists(path):
                os.remove(path)
        logged = "\n".join(l for l in stream.getvalue().splitlines() if not l.startswith("::add-mask::"))
        self.assertNotIn(FORGEJO_TOKEN, logged + summary)
        self.assertNotIn(self.b64, logged + summary)
        self.assertIn("%0A", logged)  # annotation newlines escaped

    def test_untrusted_workflow_commands_are_inert(self):
        stream = io.StringIO()
        rep = Reporter(Redactor(), stream=stream, actions=True)
        rep.log("::add-mask::x\n::error::forged")
        lines = stream.getvalue().splitlines()
        start = next(i for i, l in enumerate(lines) if l.startswith("::stop-commands::"))
        token = lines[start].split("::")[2]
        forged = lines.index("::error::forged")
        resume = [i for i, l in enumerate(lines) if l == f"::{token}::"]
        self.assertGreater(forged, start)
        self.assertFalse([i for i in resume if start < i < forged])  # commands still stopped


class GitRunner(SyncTestCase):
    def test_credentials_never_on_argv_and_scoped_to_forgejo(self):
        g = Git(git_dir="/tmp/x", auth=GitAuth("https://git.ferrive.me", "pdfcraft-sync", FORGEJO_TOKEN))
        with mock.patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"", b"")) as run:
            g.run(["ls-remote", "https://git.ferrive.me/sefr/pdfcraft.git", "refs/heads/Main"])
        argv, env = run.call_args.args[0], run.call_args.kwargs["env"]
        self.assertFalse([a for a in argv if FORGEJO_TOKEN in a or "extraHeader" in a])
        self.assertEqual(env["GIT_CONFIG_KEY_0"], "http.https://git.ferrive.me/.extraHeader")
        self.assertEqual(env["GIT_TERMINAL_PROMPT"], "0")

    def test_trace_variables_are_dropped(self):
        g = Git(git_dir="/tmp/x")
        with mock.patch.dict(os.environ, {"GIT_TRACE": "1", "GIT_CURL_VERBOSE": "1", "GIT_TRACE_CURL": "1",
                                          "GIT_ASKPASS": "/bin/echo", "GIT_CONFIG_PARAMETERS": "'x.y'='z'"}):
            env = g._env()
        for key in ("GIT_TRACE", "GIT_CURL_VERBOSE", "GIT_TRACE_CURL", "GIT_ASKPASS", "GIT_CONFIG_PARAMETERS"):
            self.assertNotIn(key, env)

    def test_git_failure_messages_are_redacted(self):
        g = self.new_git()
        g.redactor = Redactor()
        g.redactor.add(FORGEJO_TOKEN)
        with self.assertRaises(GitError) as ctx:
            g.fetch(f"https://pdfcraft-sync:{FORGEJO_TOKEN}@127.0.0.1:9/x.git", "refs/heads/Main", "refs/sync/x")
        text = f"{ctx.exception} {ctx.exception.stderr}"
        self.assertNotIn(FORGEJO_TOKEN, text)

    def test_nul_in_argument_is_refused(self):
        g = Git(git_dir="/tmp/x")
        with self.assertRaises(ValidationError):
            g.run(["rev-parse", "a\x00b"])

    def test_fetch_namespace_is_enforced(self):
        g = Git(git_dir="/tmp/x")
        with self.assertRaises(ValidationError):
            g.fetch("file:///x", "refs/heads/Main", "refs/heads/Main")


if __name__ == "__main__":
    unittest.main()
