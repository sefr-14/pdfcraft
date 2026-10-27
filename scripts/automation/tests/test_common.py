"""Shared helpers: push guard, validation, Git runner hygiene, redaction and reporting."""

from __future__ import annotations

import base64
import io
import os
import subprocess
import unittest
from unittest import mock

from fakes import CF_ID, CF_SECRET, FORGEJO_TOKEN, SyncTestCase

from common import (
    CloudflareAccess,
    Git,
    GitAuth,
    GitError,
    PushRefused,
    Redactor,
    Reporter,
    SyncError,
    ValidationError,
    check_push_destination,
    quote_untrusted,
    sanitize_title,
    validate_branch_name,
    validate_header_value,
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

    def test_basic_authorization_base64_is_redacted(self):
        for text in (f"Authorization: Basic {self.b64}", f"http.extraHeader=Authorization: Basic {self.b64}",
                     f"credentials {self.b64} end", f"{self.b64}"):
            with self.subTest(text=text[:30]):
                self.assertNotIn(self.b64, self.r.redact(text))
        # Unregistered: the header shape alone is enough.
        self.assertNotIn(self.b64, Redactor().redact(f"Authorization: Basic {self.b64}"))

    def test_cloudflare_credentials_registered_and_header_shapes(self):
        CloudflareAccess(CF_ID, CF_SECRET).register(self.r)
        out = self.r.redact(f"id={CF_ID} secret={CF_SECRET}")
        self.assertNotIn(CF_ID, out)
        self.assertNotIn(CF_SECRET, out)
        out = Redactor().redact(f"CF-Access-Client-Id: {CF_ID}\ncf-access-client-secret={CF_SECRET}")
        self.assertNotIn(CF_ID, out)
        self.assertNotIn(CF_SECRET, out)

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


class CloudflareConfig(unittest.TestCase):
    def env(self, **values):
        clean = {k: v for k, v in os.environ.items() if not k.startswith("CF_ACCESS_")}
        clean.update(values)
        return mock.patch.dict(os.environ, clean, clear=True)

    def test_both_absent_is_none(self):
        with self.env():
            self.assertIsNone(CloudflareAccess.from_env())

    def test_both_present(self):
        with self.env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET):
            cf = CloudflareAccess.from_env()
        self.assertEqual(cf.headers(), {"CF-Access-Client-Id": CF_ID, "CF-Access-Client-Secret": CF_SECRET})

    def test_exactly_one_present_fails_without_printing_it(self):
        for values, missing in (({"CF_ACCESS_CLIENT_ID": CF_ID}, "CF_ACCESS_CLIENT_SECRET is not"),
                                ({"CF_ACCESS_CLIENT_SECRET": CF_SECRET}, "CF_ACCESS_CLIENT_ID is not"),
                                ({"CF_ACCESS_CLIENT_ID": CF_ID, "CF_ACCESS_CLIENT_SECRET": "  \n"}, "SECRET is not"),
                                ({"CF_ACCESS_CLIENT_ID": "", "CF_ACCESS_CLIENT_SECRET": CF_SECRET}, "ID is not")):
            with self.subTest(values=sorted(values)), self.env(**values):
                with self.assertRaises(SyncError) as ctx:
                    CloudflareAccess.from_env()
                self.assertIn("half configured", str(ctx.exception))
                self.assertIn(missing, str(ctx.exception))
                self.assertNotIn(CF_ID, str(ctx.exception))
                self.assertNotIn(CF_SECRET, str(ctx.exception))

    def test_empty_and_whitespace_only_count_as_absent(self):
        for value in ("", " ", "\t\n "):
            with self.subTest(value=value), self.env(CF_ACCESS_CLIENT_ID=value, CF_ACCESS_CLIENT_SECRET=value):
                self.assertIsNone(CloudflareAccess.from_env())

    def test_surrounding_whitespace_is_stripped(self):
        with self.env(CF_ACCESS_CLIENT_ID=f" {CF_ID}\n", CF_ACCESS_CLIENT_SECRET=f"{CF_SECRET}\r\n"):
            cf = CloudflareAccess.from_env()
        self.assertEqual((cf.client_id, cf.client_secret), (CF_ID, CF_SECRET))

    def test_header_injection_is_refused(self):
        for bad in (f"{CF_SECRET}\r\nX-Evil: 1", f"{CF_SECRET}\nX-Evil: 1",
                    f"{CF_SECRET} x", f"{CF_SECRET}\x7f", f"{CF_SECRET}é", "x" * 5000):
            with self.subTest(bad=bad[:40]), self.env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=bad):
                with self.assertRaises(ValidationError) as ctx:
                    CloudflareAccess.from_env()
                self.assertIn("CF_ACCESS_CLIENT_SECRET", str(ctx.exception))
                self.assertNotIn(CF_SECRET, str(ctx.exception))

    def test_validate_header_value(self):
        self.assertEqual(validate_header_value(FORGEJO_TOKEN, "FORGEJO_TOKEN"), FORGEJO_TOKEN)
        for bad in ("", "a b", "a\nb", "a\rb", "tab\there", "nul\x00", None):  # NUL cannot reach os.environ
            with self.subTest(bad=bad), self.assertRaises(ValidationError) as ctx:
                validate_header_value(bad, "FORGEJO_TOKEN")
            self.assertIn("FORGEJO_TOKEN", str(ctx.exception))


class GitRunner(SyncTestCase):
    def test_credentials_never_on_argv_and_scoped_to_forgejo(self):
        for cf in (None, CloudflareAccess(CF_ID, CF_SECRET)):
            with self.subTest(cloudflare=cf is not None):
                g = Git(git_dir="/tmp/x", auth=GitAuth("https://git.ferrive.me", "pdfcraft-sync", FORGEJO_TOKEN, cf))
                with mock.patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, b"", b"")) as run:
                    g.run(["ls-remote", "https://git.ferrive.me/sefr/pdfcraft.git", "refs/heads/Main"])
                argv, env = run.call_args.args[0], run.call_args.kwargs["env"]
                self.assertFalse([a for a in argv if FORGEJO_TOKEN in a or CF_ID in a or CF_SECRET in a
                                  or "extraHeader" in a or "followRedirects" in a])
                self.assertEqual(env["GIT_CONFIG_KEY_0"], "http.https://git.ferrive.me/.extraHeader")
                self.assertEqual(env["GIT_TERMINAL_PROMPT"], "0")

    # The raw, internal Git environment: it must carry the credentials (and only for the Forgejo
    # prefix). Whether they ever become visible is tested separately, in test_git_http_auth.

    def test_git_auth_env_without_cloudflare_keeps_basic_and_refuses_redirects(self):
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        self.assertEqual(GitAuth("https://git.ferrive.me/", "pdfcraft-sync", FORGEJO_TOKEN).env(), {
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": "http.https://git.ferrive.me/.extraHeader",
            "GIT_CONFIG_VALUE_0": f"Authorization: Basic {b64}",
            "GIT_CONFIG_KEY_1": "http.https://git.ferrive.me/.followRedirects",
            "GIT_CONFIG_VALUE_1": "false",
        })

    def test_git_auth_env_with_cloudflare_has_three_headers_for_the_same_prefix(self):
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        env = GitAuth("https://git.ferrive.me", "pdfcraft-sync", FORGEJO_TOKEN,
                      CloudflareAccess(CF_ID, CF_SECRET)).env()
        self.assertEqual(env, {
            "GIT_CONFIG_COUNT": "4",
            "GIT_CONFIG_KEY_0": "http.https://git.ferrive.me/.extraHeader",
            "GIT_CONFIG_VALUE_0": f"Authorization: Basic {b64}",
            "GIT_CONFIG_KEY_1": "http.https://git.ferrive.me/.extraHeader",
            "GIT_CONFIG_VALUE_1": f"CF-Access-Client-Id: {CF_ID}",
            "GIT_CONFIG_KEY_2": "http.https://git.ferrive.me/.extraHeader",
            "GIT_CONFIG_VALUE_2": f"CF-Access-Client-Secret: {CF_SECRET}",
            "GIT_CONFIG_KEY_3": "http.https://git.ferrive.me/.followRedirects",
            "GIT_CONFIG_VALUE_3": "false",
        })

    def test_inherited_git_config_entries_are_replaced_not_merged(self):
        g = Git(git_dir="/tmp/x", auth=GitAuth("https://git.ferrive.me", "u", FORGEJO_TOKEN))
        with mock.patch.dict(os.environ, {"GIT_CONFIG_COUNT": "5", "GIT_CONFIG_KEY_4": "http.extraHeader",
                                          "GIT_CONFIG_VALUE_4": "X-Leak: 1"}):
            env = g._env()
        self.assertEqual(env["GIT_CONFIG_COUNT"], "2")
        self.assertNotIn("GIT_CONFIG_KEY_4", env)

    def test_reprs_hide_credentials(self):
        cf = CloudflareAccess(CF_ID, CF_SECRET)
        auth = GitAuth("https://git.ferrive.me", "pdfcraft-sync", FORGEJO_TOKEN, cf)
        for text in (repr(cf), str(cf), repr(auth), repr(Git(git_dir="/tmp/x", auth=auth))):
            for secret in (CF_ID, CF_SECRET, FORGEJO_TOKEN):
                self.assertNotIn(secret, text)

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
