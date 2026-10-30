"""Workflow A: the GitHub -> Forgejo PR bridge, against fake servers and real Git remotes."""

from __future__ import annotations

import contextlib
import io
import os
import unittest

from fakes import (
    CF_ID,
    CF_SECRET,
    FORGEJO_TOKEN,
    GITHUB_TOKEN,
    Fault,
    FakeGitHub,
    SyncTestCase,
    github_pr,
    no_proxy_opener,
    quiet_reporter,
)

import sync_claude_pr
from forgejo_client import AuthError, ForgejoClient
from github_client import GitHubClient
from sync_claude_pr import Bridge, Config

N = 5
BRANCH = "refs/heads/github-pr/5"


class BridgeTestCase(SyncTestCase):
    def setUp(self) -> None:
        super().setUp()
        self.github = FakeGitHub()
        self.addCleanup(self.github.stop)
        r = self.remotes
        self.c0 = r.commit(None, {"README": "base\n"}, "base")
        for remote in ("github", "forgejo"):
            r.set_ref(remote, "refs/heads/Main", self.c0)
        self.c1 = r.commit(self.c0, {"feature.txt": "one\n"}, "feature 1")
        self.publish(self.c1)

    def publish(self, sha: str, **kwargs) -> None:
        """The GitHub PR now has head ``sha`` (both the API and refs/pull/N/head)."""
        self.remotes.set_ref("github", f"refs/pull/{N}/head", sha)
        self.github.pulls[("sefr-14/pdfcraft", N)] = github_pr(N, sha, **kwargs)

    def config(self, **overrides) -> Config:
        values = dict(
            source_repo="sefr-14/pdfcraft", pr_number=N, base_branch="Main",
            github_api_url=self.github.url, source_git_url=self.remotes.url("github"),
            github_server_url="https://github.com", github_token=GITHUB_TOKEN,
            forgejo_url=self.forgejo.url, forgejo_repo="sefr/pdfcraft",
            forgejo_git_url=self.remotes.url("forgejo"), forgejo_user="pdfcraft-sync",
            forgejo_token=FORGEJO_TOKEN,
        )
        values.update(overrides)
        return Config(**values)

    def run_bridge(self, **overrides):
        cfg = self.config(**overrides)
        self.reporter = quiet_reporter()
        bridge = Bridge(
            cfg,
            GitHubClient(cfg.github_api_url, cfg.github_token, opener=no_proxy_opener(), sleep=lambda s: None),
            ForgejoClient(cfg.forgejo_url, cfg.forgejo_repo, cfg.forgejo_token, opener=no_proxy_opener(), sleep=lambda s: None),
            self.new_git(),
            self.reporter,
        )
        return bridge.run()

    def only_pr(self) -> dict:
        self.assertEqual(len(self.forgejo.pulls), 1, self.forgejo.pulls)
        return next(iter(self.forgejo.pulls.values()))

    def main_unchanged(self) -> None:
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/Main"), self.c0)


class CreateAndUpdate(BridgeTestCase):
    def test_creates_branch_and_pr(self):
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("created", 0), self.reporter.stream.getvalue())
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)
        pr = self.only_pr()
        self.assertEqual((pr["head"], pr["base"], pr["state"]), ("github-pr/5", "Main", "open"))
        self.assertEqual(pr["title"], "[GH #5] Add a feature")
        self.assertTrue(pr["body"].startswith(
            f"<!-- pdfcraft-sync:v1 source=github:sefr-14/pdfcraft#5 synced_sha={self.c1} -->\n"))
        self.assertIn("https://github.com/sefr-14/pdfcraft/pull/5", pr["body"])
        self.main_unchanged()

    def test_rerun_is_idempotent(self):
        self.run_bridge()
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("unchanged", 0))
        self.only_pr()
        self.assertEqual(self.forgejo.comments[1], [])

    def test_new_commit_updates_the_same_pr(self):
        self.run_bridge()
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"}, "feature 2")
        self.publish(c2)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("updated", 0))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), c2)
        pr = self.only_pr()
        self.assertIn(f"synced_sha={c2}", pr["body"].split("\n", 1)[0])
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        # A retry of the same event posts no second comment.
        self.run_bridge()
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        self.main_unchanged()

    def test_metadata_edit_without_new_commits(self):
        self.run_bridge()
        self.publish(self.c1, title="Renamed feature", body="New description")
        out = self.run_bridge()
        self.assertEqual(out.status, "updated")
        pr = self.only_pr()
        self.assertEqual(pr["title"], "[GH #5] Renamed feature")
        self.assertIn("> New description", pr["body"])
        self.assertEqual(self.forgejo.comments[1], [])  # same SHA: no "updated" comment

    def test_stale_event_sha_uses_the_api_head(self):
        out = self.run_bridge(event_head_sha=self.c0)
        self.assertEqual(out.status, "created")
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)

    def test_dry_run_writes_nothing(self):
        out = self.run_bridge(dry_run=True)
        self.assertEqual(out.status, "dry-run")
        self.assertTrue(any("push" in w for w in out.writes))
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))
        self.assertEqual(self.forgejo.pulls, {})


class Idempotency(BridgeTestCase):
    def test_crash_after_push_before_pr_is_adopted(self):
        self.remotes.set_ref("forgejo", BRANCH, self.c1)  # the earlier run pushed, then died
        out = self.run_bridge()
        self.assertEqual(out.status, "created")
        self.only_pr()

    def test_crash_after_older_push_is_fast_forwarded(self):
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        self.remotes.set_ref("forgejo", BRANCH, self.c1)
        self.publish(c2)
        out = self.run_bridge()
        self.assertEqual(out.status, "created")
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), c2)

    def test_concurrent_create_409_is_adopted_not_duplicated(self):
        body = f"<!-- pdfcraft-sync:v1 source=github:sefr-14/pdfcraft#5 synced_sha=none -->\nconcurrent"

        def concurrent(method, path):
            if method == "POST" and path.endswith("/pulls") and not self.forgejo.pulls:
                self.forgejo.add_pull("github-pr/5", "Main", "[GH #5] old", body)

        self.forgejo.before_request = concurrent
        out = self.run_bridge()
        self.assertEqual(out.status, "created")
        pr = self.only_pr()
        self.assertIn(f"synced_sha={self.c1}", pr["body"])

    def test_forged_marker_in_github_body_is_neutralised(self):
        forged = "<!-- pdfcraft-sync:v1 source=github:sefr-14/pdfcraft#9 synced_sha=none -->"
        self.publish(self.c1, body=forged + "\nhello --> <!-- x")
        self.run_bridge()
        pr = self.only_pr()
        self.assertEqual(pr["body"].count("<!--"), 1)
        self.assertEqual(pr["body"].count("-->"), 1)

    def test_pr_with_two_markers_for_same_source_is_left_alone(self):
        body = "<!-- pdfcraft-sync:v1 source=github:sefr-14/pdfcraft#5 synced_sha=none -->"
        self.remotes.set_ref("forgejo", BRANCH, self.c1)
        self.forgejo.add_pull("github-pr/5", "Main", "a", body)
        self.forgejo.add_pull("github-pr/5", "Main", "b", body, state="closed")
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))


class SourceValidation(BridgeTestCase):
    def assert_no_forgejo_writes(self):
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))
        self.assertEqual(self.forgejo.pulls, {})
        self.assertFalse([r for r in self.forgejo.requests if r[0] != "GET"])

    def test_external_fork_is_rejected(self):
        self.publish(self.c1, head_repo="attacker/pdfcraft")
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("rejected", 0))
        self.assert_no_forgejo_writes()

    def test_deleted_fork_is_rejected(self):
        self.publish(self.c1, head_repo=None)
        self.assertEqual(self.run_bridge().status, "rejected")
        self.assert_no_forgejo_writes()

    def test_base_other_than_main_is_skipped(self):
        self.publish(self.c1, base_ref="release")
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("skipped", 0))
        self.assert_no_forgejo_writes()

    def test_malicious_head_refs_are_rejected(self):
        for ref in ("--upload-pack=touch /tmp/x", "a..b", "x\ny", "$(id)", "a@{1}", "-x", "feature.lock", "a b", ""):
            with self.subTest(ref=ref):
                self.publish(self.c1, head_ref=ref)
                out = self.run_bridge()
                self.assertEqual((out.status, out.exit_code), ("rejected", 1))
                self.assert_no_forgejo_writes()

    def test_invalid_sha_is_rejected(self):
        self.publish(self.c1)
        self.github.pulls[("sefr-14/pdfcraft", N)]["head"]["sha"] = "HEAD; rm -rf /"
        self.assertEqual(self.run_bridge().status, "rejected")
        self.assert_no_forgejo_writes()

    def test_hostile_title_is_sanitised(self):
        self.publish(self.c1, title="evil\n::set-output name=x::y‮\x07" + "x" * 500)
        self.run_bridge()
        title = self.only_pr()["title"]
        self.assertNotIn("\n", title)
        self.assertNotIn("‮", title)
        self.assertLessEqual(len(title), 200)


class Races(BridgeTestCase):
    def test_fetched_head_differs_from_api_is_superseded(self):
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        self.github.pulls[("sefr-14/pdfcraft", N)] = github_pr(N, c2)  # API ahead of refs/pull
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("superseded", 0))
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))

    def test_head_moves_during_revalidation_is_superseded(self):
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})

        def move(count):
            if count == 2:  # the revalidation just before the push
                self.github.pulls[("sefr-14/pdfcraft", N)] = github_pr(N, c2)

        self.github.on_get = move
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("superseded", 0))
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))
        self.assertEqual(self.forgejo.pulls, {})

    def test_forgejo_pr_merged_during_sync_stops_before_push(self):
        self.run_bridge()
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        self.publish(c2)

        def merge_now(method, path):
            if method == "GET" and path.endswith("/pulls/1") and not self.forgejo.pulls[1].get("merged"):
                self.forgejo.pulls[1].update(state="closed", merged=True, merged_head=self.c1)

        self.forgejo.before_request = merge_now
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)

    def test_concurrent_push_is_rejected_and_state_kept(self):
        self.run_bridge()
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        other = self.remotes.commit(self.c1, {"other.txt": "someone else\n"})
        self.publish(c2)

        def concurrent(method, path):
            if method == "GET" and path.endswith("/pulls/1") and self.remotes.ref("forgejo", BRANCH) == self.c1:
                self.remotes.set_ref("forgejo", BRANCH, other)

        self.forgejo.before_request = concurrent
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("push-rejected", 1))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), other)
        self.assertIn(f"synced_sha={self.c1}", self.only_pr()["body"].split("\n", 1)[0])


class NonFastForward(BridgeTestCase):
    def test_rewritten_github_branch_is_reported_not_forced(self):
        self.run_bridge()
        rewritten = self.remotes.commit(self.c0, {"feature.txt": "rewritten\n"})
        self.publish(rewritten)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("non-fast-forward", 1))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)
        self.assertIn(f"synced_sha={self.c1}", self.only_pr()["body"])
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        self.run_bridge()
        self.assertEqual(len(self.forgejo.comments[1]), 1)  # reported once per SHA

    def test_human_branch_with_our_name_is_a_collision(self):
        human = self.remotes.commit(self.c0, {"human.txt": "mine\n"})
        self.remotes.set_ref("forgejo", BRANCH, human)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("collision", 1))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), human)
        self.assertEqual(self.forgejo.pulls, {})

    def test_pr_on_our_branch_without_marker_is_left_alone(self):
        self.remotes.set_ref("forgejo", BRANCH, self.c1)
        self.forgejo.add_pull("github-pr/5", "Main", "made by hand", "no marker")
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))
        self.assertEqual(len(self.forgejo.pulls), 1)


class ForgejoSideStates(BridgeTestCase):
    def test_merged_forgejo_pr_is_not_reopened_or_duplicated(self):
        self.run_bridge()
        self.forgejo.merge_pull(1)
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        self.publish(c2)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)
        self.only_pr()
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        self.run_bridge()
        self.assertEqual(len(self.forgejo.comments[1]), 1)

    def test_merged_forgejo_pr_without_new_commits_is_up_to_date(self):
        self.run_bridge()
        self.forgejo.merge_pull(1)
        self.assertEqual(self.run_bridge().status, "up-to-date")

    def test_closed_forgejo_pr_is_respected(self):
        self.run_bridge()
        self.forgejo.close_pull(1)
        c2 = self.remotes.commit(self.c1, {"feature.txt": "two\n"})
        self.publish(c2)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("respected-close", 0))
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)
        self.assertEqual(self.only_pr()["state"], "closed")

    def test_already_in_main_creates_nothing(self):
        self.remotes.set_ref("forgejo", "refs/heads/Main", self.c1)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("already-in-main", 0))
        self.assertEqual(self.forgejo.pulls, {})
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))

    def test_deleted_branch_of_open_pr_is_not_recreated(self):
        self.run_bridge()
        self.remotes.delete_ref("forgejo", BRANCH)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))
        self.assertIsNone(self.remotes.ref("forgejo", BRANCH))


class GitHubClosure(BridgeTestCase):
    def test_closed_unmerged_records_and_keeps_forgejo(self):
        self.run_bridge()
        self.publish(self.c1, state="closed")
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("github-closed", 0))
        pr = self.only_pr()
        self.assertEqual(pr["state"], "open")
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), self.c1)
        self.assertEqual(len(self.forgejo.comments[1]), 1)

    def test_merged_after_forgejo_merge_is_landed_via_mirror(self):
        self.run_bridge()
        self.forgejo.merge_pull(1)
        self.publish(self.c1, state="closed", merged=True)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("landed-via-mirror", 0))
        self.assertEqual(self.forgejo.comments[1], [])

    def test_github_side_merge_is_a_violation_even_if_reachable_from_main(self):
        # Correction 1: closure is handled before the "already in Main" shortcut.
        self.run_bridge()
        self.remotes.set_ref("forgejo", "refs/heads/Main", self.c1)
        self.publish(self.c1, state="closed", merged=True)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("policy-violation", 1))
        pr = self.only_pr()
        self.assertEqual(pr["state"], "open")
        self.assertIn("Policy violation", self.forgejo.comments[1][0]["body"])

    def test_github_side_merge_without_forgejo_pr_is_a_violation(self):
        self.remotes.set_ref("forgejo", "refs/heads/Main", self.c1)
        self.publish(self.c1, state="closed", merged=True)
        out = self.run_bridge()
        self.assertEqual((out.status, out.exit_code), ("policy-violation", 1))
        self.assertEqual(self.forgejo.pulls, {})


class ErrorsAndRedaction(BridgeTestCase):
    def test_forgejo_auth_failure_raises_typed_redacted_error(self):
        self.forgejo.token = "a-different-token"
        with self.assertRaises(AuthError) as ctx:
            self.run_bridge()
        self.assertNotIn(FORGEJO_TOKEN, str(ctx.exception))

    def test_transient_forgejo_errors_are_retried(self):
        self.forgejo.faults.append(Fault("GET", r"/pulls$", 503, times=2))
        self.assertEqual(self.run_bridge().status, "created")

    def main_env(self, **extra) -> str:
        summary = os.path.join(self.remotes.root, "summary.md")
        os.environ.update({
            "SYNC_TESTING": "1", "GITHUB_REPOSITORY": "sefr-14/pdfcraft", "PR_NUMBER": str(N),
            "GITHUB_API_URL": self.github.url, "GITHUB_SERVER_URL": "https://github.com",
            "SOURCE_GIT_URL": self.remotes.url("github"), "GITHUB_TOKEN": GITHUB_TOKEN,
            "FORGEJO_URL": self.forgejo.url, "FORGEJO_GIT_URL": self.remotes.url("forgejo"),
            "FORGEJO_USER": "pdfcraft-sync", "FORGEJO_TOKEN": FORGEJO_TOKEN,
            "GITHUB_ACTIONS": "true", "GITHUB_STEP_SUMMARY": summary, "no_proxy": "127.0.0.1",
            "NO_PROXY": "127.0.0.1", **extra,
        })
        return summary

    def run_main(self) -> tuple[int, str, str]:
        sync_claude_pr.REDACTOR.__init__()
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = sync_claude_pr.main()
        return code, out.getvalue(), err.getvalue()

    def test_main_redacts_tokens_everywhere(self):
        self.forgejo.faults.append(Fault("GET", r"/pulls$", 401, body={
            "message": f"bad token {FORGEJO_TOKEN} CF-Access-Client-Secret: {CF_SECRET} id {CF_ID}"}))
        summary = self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET)
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        code, stdout, stderr = self.run_main()
        self.assertEqual(code, 1)
        with open(summary, encoding="utf-8") as handle:
            summary_text = handle.read()
        import base64
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        # The ::add-mask:: lines are the one place the secret may appear: they are how
        # GitHub learns to mask it. Everything else must be clean.
        logged = "\n".join(line for line in stdout.splitlines() if not line.startswith("::add-mask::"))
        for text in (logged, stderr, summary_text):
            for secret in (FORGEJO_TOKEN, GITHUB_TOKEN, b64, CF_ID, CF_SECRET):
                self.assertNotIn(secret, text)
        self.assertIn("HTTP 401", logged)
        self.assertIn("::stop-commands::", stdout)

    def test_main_masks_every_credential_before_anything_else_is_printed(self):
        import base64
        self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET)
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        lines = stdout.splitlines()
        first_other = next(i for i, line in enumerate(lines) if not line.startswith("::add-mask::"))
        self.assertTrue(lines[first_other].startswith("::stop-commands::"))
        masked = {line[len("::add-mask::"):] for line in lines[:first_other]}
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        self.assertLessEqual({FORGEJO_TOKEN, GITHUB_TOKEN, b64, CF_ID, CF_SECRET}, masked)
        self.assertFalse([l for l in lines[first_other:] if l.startswith("::add-mask::")])

    def test_source_auth_is_scoped_to_the_exact_source_repository(self):
        url = "https://github.com/sefr-14/pdfcraft.git"
        auth = sync_claude_pr.source_auth(self.config(source_git_url=url))
        self.assertIsNotNone(auth)
        self.assertEqual((auth.url_prefix, auth.user, auth.token, auth.cf_access), (url, "x-access-token", GITHUB_TOKEN, None))
        keys = {key for key, _ in auth.entries()}
        self.assertEqual(keys, {f"http.{url}/.extraHeader", f"http.{url}/.followRedirects"})
        self.assertIn((f"http.{url}/.followRedirects", "false"), auth.entries())
        # No token (local and dry runs): anonymous as before. file:// test remotes: none.
        self.assertIsNone(sync_claude_pr.source_auth(self.config(source_git_url=url, github_token="")))
        self.assertIsNone(sync_claude_pr.source_auth(self.config()))

    def test_main_gives_git_the_source_credentials_and_fails_safely(self):
        import base64
        from unittest import mock
        # A source that can't be reached: the bridge must fail, never fall back or leak.
        self.main_env(SOURCE_GIT_URL="http://127.0.0.1:9/github.git")
        seen = {}
        original = sync_claude_pr.Git.temporary

        def capture(*args, **kwargs):
            seen.update(kwargs)
            return original(*args, **kwargs)

        with mock.patch.object(sync_claude_pr.Git, "temporary", side_effect=capture):
            code, stdout, stderr = self.run_main()
        self.assertEqual(code, 1)
        (extra,) = seen["extra_auth"]
        self.assertEqual((extra.url_prefix, extra.user), ("http://127.0.0.1:9/github.git", "x-access-token"))
        self.assertEqual(seen["auth"].url_prefix, self.forgejo.url, "Forgejo keeps its own credentials")
        self.assertEqual(self.remotes.ref("forgejo", BRANCH), None, "nothing was pushed")
        b64 = base64.b64encode(f"x-access-token:{GITHUB_TOKEN}".encode()).decode()
        lines = stdout.splitlines()
        self.assertIn(f"::add-mask::{b64}", lines)
        logged = "\n".join(l for l in lines if not l.startswith("::add-mask::"))
        for text in (logged, stderr):
            for secret in (GITHUB_TOKEN, b64, FORGEJO_TOKEN):
                self.assertNotIn(secret, text)

    def test_main_refuses_a_github_token_that_could_split_a_header(self):
        self.main_env(GITHUB_TOKEN="ghs_abc\r\nX-Injected: 1")
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 1)
        self.assertIn("GITHUB_TOKEN", stdout)
        self.assertNotIn("X-Injected", stdout)

    def test_main_registers_basic_base64_for_the_default_user(self):
        import base64
        self.main_env(FORGEJO_USER="")
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        self.assertIn(f"::add-mask::{b64}", stdout)

    def test_main_with_cloudflare_reaches_forgejo(self):
        self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET)
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        self.assertEqual(len(self.forgejo.open_pulls()), 1)
        for headers in self.forgejo.headers:
            self.assertEqual(headers["cf-access-client-secret"], CF_SECRET)
            self.assertEqual(headers["authorization"], f"token {FORGEJO_TOKEN}")
        for headers in self.github.headers:
            self.assertNotIn("cf-access-client-id", headers)
            self.assertNotIn("cf-access-client-secret", headers)

    def test_main_without_cloudflare_is_unchanged(self):
        self.main_env()
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        self.assertEqual(len(self.forgejo.open_pulls()), 1)
        self.assertNotIn("cf-access-client-id", self.forgejo.headers[0])

    def test_main_refuses_half_configured_cloudflare_before_any_request(self):
        for name, value in (("CF_ACCESS_CLIENT_ID", CF_ID), ("CF_ACCESS_CLIENT_SECRET", CF_SECRET)):
            with self.subTest(only=name):
                os.environ.pop("CF_ACCESS_CLIENT_ID", None)
                os.environ.pop("CF_ACCESS_CLIENT_SECRET", None)
                self.main_env(**{name: value})
                code, stdout, stderr = self.run_main()
                self.assertEqual(code, 1)
                self.assertIn("half configured", stdout)
                self.assertNotIn(value, stdout + stderr)
                self.assertEqual(self.forgejo.requests, [])
                self.assertEqual(self.github.gets, 0)
                self.assertEqual(self.github.headers, [])

    def test_main_refuses_a_token_that_would_split_a_header(self):
        self.main_env(FORGEJO_TOKEN=FORGEJO_TOKEN + "\r\nX-Evil: 1")
        code, stdout, stderr = self.run_main()
        self.assertEqual(code, 1)
        self.assertIn("FORGEJO_TOKEN contains characters not allowed", stdout)
        self.assertNotIn(FORGEJO_TOKEN, stdout + stderr)
        self.assertEqual(self.forgejo.requests, [])


if __name__ == "__main__":
    unittest.main()
