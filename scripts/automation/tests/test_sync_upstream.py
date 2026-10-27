"""Workflow B: the daily upstream trial merge, against a fake Forgejo and real Git remotes."""

from __future__ import annotations

import base64
import contextlib
import io
import os
import subprocess
import unittest

from fakes import CF_ID, CF_SECRET, FORGEJO_TOKEN, Fault, SyncTestCase, git, no_proxy_opener, quiet_reporter

import sync_upstream

from common import GitError
from forgejo_client import ForgejoClient
from sync_upstream import TRAILER, Config, MergeExecutionError, UpstreamSync, parse_merge_tree

BRANCH = "refs/heads/upstream-sync/main"


class UpstreamTestCase(SyncTestCase):
    def setUp(self) -> None:
        super().setUp()
        r = self.remotes
        self.c0 = r.commit(None, {"shared.txt": "line 1\nline 2\nline 3\n", "README": "base\n"}, "base")
        self.f1 = r.commit(self.c0, {"fork.txt": "fork feature\n"}, "fork: overlay")
        self.u1 = r.commit(self.c0, {"upstream.txt": "upstream feature\n"}, "upstream: feature")
        r.set_ref("forgejo", "refs/heads/Main", self.f1)
        r.set_ref("upstream", "refs/heads/main", self.u1)

    def config(self, **overrides) -> Config:
        values = dict(
            forgejo_url=self.forgejo.url, forgejo_repo="sefr/pdfcraft", forgejo_git_url=self.remotes.url("forgejo"),
            forgejo_user="pdfcraft-sync", forgejo_token=FORGEJO_TOKEN, upstream_git_url=self.remotes.url("upstream"),
            upstream_branch="main", upstream_name="storytold/pdfcraft", base_branch="Main", branch_version=None,
            committer_name="PdfCraft upstream sync", committer_email="sync@example.invalid",
        )
        values.update(overrides)
        return Config(**values)

    def sync(self, git_hook=None, **overrides) -> UpstreamSync:
        cfg = self.config(**overrides)
        self.reporter = quiet_reporter()
        g = self.new_git(identity={"GIT_AUTHOR_NAME": "sync", "GIT_AUTHOR_EMAIL": "s@example.invalid",
                                   "GIT_COMMITTER_NAME": "sync", "GIT_COMMITTER_EMAIL": "s@example.invalid"})
        if git_hook:
            git_hook(g)
        return UpstreamSync(cfg, ForgejoClient(cfg.forgejo_url, cfg.forgejo_repo, cfg.forgejo_token,
                                               opener=no_proxy_opener(), sleep=lambda s: None), g, self.reporter)

    def run_sync(self, **kwargs):
        return self.sync(**kwargs).run()

    def head(self, ref: str = BRANCH) -> str | None:
        return self.remotes.ref("forgejo", ref)

    def parents(self, sha: str) -> list[str]:
        return git(self.remotes.bare["forgejo"], "rev-list", "--parents", "-n", "1", sha).split()[1:]

    def is_ancestor(self, a: str, b: str) -> bool:
        return subprocess.run(["git", f"--git-dir={self.remotes.bare['forgejo']}", "merge-base", "--is-ancestor", a, b]).returncode == 0

    def main_is(self, sha: str) -> None:
        self.assertEqual(self.remotes.ref("forgejo", "refs/heads/Main"), sha)


class CleanIntegration(UpstreamTestCase):
    def test_up_to_date_does_nothing(self):
        self.remotes.set_ref("upstream", "refs/heads/main", self.c0)
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("up-to-date", 0))
        self.assertIsNone(self.head())
        self.assertEqual(self.forgejo.pulls, {})

    def test_clean_merge_creates_branch_and_unmerged_pr(self):
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("created", 0), self.reporter.stream.getvalue())
        head = self.head()
        self.assertEqual(self.parents(head), [self.f1, self.u1])
        self.assertIn(TRAILER, git(self.remotes.bare["forgejo"], "show", "-s", "--format=%B", head))
        pr = self.forgejo.pulls[1]
        self.assertEqual((pr["head"], pr["base"], pr["state"], pr["merged"]), ("upstream-sync/main", "Main", "open", False))
        first = pr["body"].split("\n", 1)[0]
        self.assertEqual(first, f"<!-- pdfcraft-upstream-sync:v1 branch=upstream-sync/main upstream={self.u1} main={self.f1} head={head} -->")
        for item in ("Six-step Overlay wizard", "Thin Lines", "Distance, perimeter and area", "Persistent measurement annotations",
                     "Rulers and guides", "optional-content layers", "Windows portable builds", "English and Spanish",
                     "not proof of functional compatibility"):
            self.assertIn(item, pr["body"])
        self.main_is(self.f1)

    def test_repeated_daily_runs_are_noops(self):
        self.run_sync()
        for _ in range(2):
            out = self.run_sync()
            self.assertEqual((out.status, out.exit_code), ("no-change", 0))
        self.assertEqual(len(self.forgejo.pulls), 1)
        self.assertEqual(self.forgejo.comments[1], [])

    def test_conflict_marker_text_in_a_clean_merge_is_not_a_conflict(self):
        # Correction 3: Git's result decides, not text that looks like a marker.
        markers = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n"
        u = self.remotes.commit(self.u1, {"docs/merge-markers.md": markers}, "doc showing markers")
        self.remotes.set_ref("upstream", "refs/heads/main", u)
        out = self.run_sync()
        self.assertEqual(out.status, "created")
        self.assertEqual(self.forgejo.open_issues(), [])

    def test_no_content_change_pushes_nothing(self):
        u = self.remotes.commit(self.c0, {"fork.txt": "fork feature\n"}, "upstream adds the same file")
        self.remotes.set_ref("upstream", "refs/heads/main", u)
        out = self.run_sync()
        self.assertEqual(out.status, "no-content-change")
        self.assertIsNone(self.head())

    def test_dry_run_writes_nothing(self):
        out = self.run_sync(dry_run=True)
        self.assertEqual(out.status, "dry-run")
        self.assertIsNone(self.head())
        self.assertEqual(self.forgejo.pulls, {})


class Lifecycle(UpstreamTestCase):
    def test_upstream_advance_appends_and_updates_same_pr(self):
        self.run_sync()
        h1 = self.head()
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        out = self.run_sync()
        self.assertEqual(out.status, "updated")
        h2 = self.head()
        self.assertEqual(self.parents(h2), [h1, u2])
        self.assertEqual(len(self.forgejo.pulls), 1)
        self.assertIn(f"upstream={u2}", self.forgejo.pulls[1]["body"].split("\n", 1)[0])
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        self.main_is(self.f1)

    def test_main_advance_is_merged_in_first(self):
        self.run_sync()
        h1 = self.head()
        f2 = self.remotes.commit(self.f1, {"fork2.txt": "fork more\n"})
        self.remotes.set_ref("forgejo", "refs/heads/Main", f2)
        out = self.run_sync()
        self.assertEqual(out.status, "updated")
        h2 = self.head()
        self.assertEqual(self.parents(h2), [h1, f2])
        self.assertTrue(self.is_ancestor(h1, h2))

    def test_both_advance_during_review(self):
        self.run_sync()
        h1 = self.head()
        f2 = self.remotes.commit(self.f1, {"fork2.txt": "x\n"})
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "y\n"})
        self.remotes.set_ref("forgejo", "refs/heads/Main", f2)
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        self.assertEqual(self.run_sync().status, "updated")
        h3 = self.head()
        for sha in (h1, f2, u2):
            self.assertTrue(self.is_ancestor(sha, h3))
        self.assertEqual(len(self.forgejo.pulls), 1)

    def test_human_commit_pauses_updates(self):
        self.run_sync()
        h1 = self.head()
        fix = self.remotes.commit(h1, {"fork.txt": "fixed by a person\n"}, "manual fix-up")
        self.remotes.set_ref("forgejo", BRANCH, fix)
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("paused", 0))
        self.assertEqual(self.head(), fix)
        self.assertEqual(len(self.forgejo.comments[1]), 1)
        self.run_sync()
        self.assertEqual(len(self.forgejo.comments[1]), 1)

    def test_merged_pr_then_new_upstream_opens_a_new_pr_fast_forward(self):
        self.run_sync()
        h1 = self.head()
        self.forgejo.merge_pull(1)
        self.main_is(h1)
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        out = self.run_sync()
        self.assertEqual(out.status, "created")
        self.assertTrue(self.is_ancestor(h1, self.head()))
        self.assertEqual(len(self.forgejo.pulls), 2)
        self.assertEqual(len(self.forgejo.open_pulls()), 1)

    def test_branch_deleted_after_merge_is_recreated(self):
        self.run_sync()
        self.forgejo.merge_pull(1)
        self.remotes.delete_ref("forgejo", BRANCH)
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        self.assertEqual(self.run_sync().status, "created")
        self.assertIsNotNone(self.head())

    def test_closed_pr_freezes_branch_until_versioned_dispatch(self):
        self.run_sync()
        h1 = self.head()
        self.forgejo.close_pull(1)
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("frozen", 0))
        self.assertEqual(self.head(), h1)
        issues = self.forgejo.open_issues()
        self.assertEqual(len(issues), 1)
        self.assertIn("branch_version", issues[0]["body"])
        self.assertIn("`2`", issues[0]["body"])
        self.assertEqual(self.forgejo.pulls[1]["state"], "closed")  # never reopened
        out = self.run_sync(branch_version=2)
        self.assertEqual(out.status, "created")
        self.assertIsNotNone(self.head("refs/heads/upstream-sync/main-v2"))
        self.assertEqual(self.head(), h1)
        self.assertEqual([p["head"] for p in self.forgejo.open_pulls()], ["upstream-sync/main-v2"])
        self.assertEqual(self.forgejo.open_issues(), [])  # resolved by the clean PR
        # A later scheduled run follows the open v2 PR.
        self.assertEqual(self.run_sync().status, "no-change")

    def test_two_open_sync_prs_change_nothing(self):
        self.run_sync()
        self.forgejo.add_pull("upstream-sync/main", "Main", "copy", self.forgejo.pulls[1]["body"])
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("attention", 1))

    def test_upstream_rewrite_is_reported(self):
        self.run_sync()
        rewritten = self.remotes.commit(self.c0, {"upstream.txt": "rewritten\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", rewritten)
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("upstream-rewritten", 1))

    def test_push_rejected_when_branch_moves_is_not_forced(self):
        self.run_sync()
        h1 = self.head()
        other = self.remotes.commit(h1, {"x.txt": f"concurrent\n{TRAILER}\n"}, f"concurrent\n\n{TRAILER}")
        u2 = self.remotes.commit(self.u1, {"upstream2.txt": "more\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)

        def hook(g):
            real_push = g.push

            def racing_push(url, sha, ref):
                self.remotes.set_ref("forgejo", ref, other)
                return real_push(url, sha, ref)

            g.push = racing_push

        out = self.run_sync(git_hook=hook)
        self.assertEqual((out.status, out.exit_code), ("cannot-fast-forward", 1))
        self.assertIn("branch_version=2", out.detail)
        self.assertEqual(self.head(), other)
        self.assertIn(f"upstream={self.u1}", self.forgejo.pulls[1]["body"].split("\n", 1)[0])

    def test_unrelated_branch_with_our_name_is_a_collision(self):
        human = self.remotes.commit(self.f1, {"h.txt": "mine\n"}, "a person's branch")
        self.remotes.set_ref("forgejo", BRANCH, human)
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("collision", 1))
        self.assertEqual(self.head(), human)

    def test_interrupted_run_is_adopted(self):
        sync = self.sync()

        def die(**kwargs):
            raise GitError("simulated crash after push", 1)

        sync.forgejo.create_pull = die
        with self.assertRaises(GitError):
            sync.run()
        h1 = self.head()
        self.assertIsNotNone(h1)
        out = self.run_sync()
        self.assertEqual(out.status, "created")
        self.assertEqual(self.head(), h1)
        self.assertEqual(len(self.forgejo.pulls), 1)


class Conflicts(UpstreamTestCase):
    def make_conflict(self, text: str = "upstream change\n") -> str:
        f = self.remotes.commit(self.f1, {"shared.txt": "fork change\n"})
        u = self.remotes.commit(self.u1, {"shared.txt": text})
        self.remotes.set_ref("forgejo", "refs/heads/Main", f)
        self.remotes.set_ref("upstream", "refs/heads/main", u)
        self.f_conflict = f
        return u

    def test_conflict_opens_one_tracking_issue_and_pushes_nothing(self):
        u = self.make_conflict()
        out = self.run_sync()
        self.assertEqual((out.status, out.exit_code), ("conflict", 0))
        self.assertIsNone(self.head())
        self.assertEqual(self.forgejo.pulls, {})
        issues = self.forgejo.open_issues()
        self.assertEqual(len(issues), 1)
        body = issues[0]["body"]
        self.assertTrue(body.startswith(f"<!-- pdfcraft-upstream-sync-conflict:v1 upstream={u} main={self.f_conflict} -->"))
        self.assertIn("`shared.txt`", body)
        self.assertIn("CONFLICT (content)", body)
        self.main_is(self.f_conflict)

    def test_repeated_conflict_runs_do_not_duplicate(self):
        self.make_conflict()
        self.run_sync()
        self.run_sync()
        issues = self.forgejo.open_issues()
        self.assertEqual(len(issues), 1)
        self.assertEqual(self.forgejo.comments[issues[0]["number"]], [])

    def test_new_conflicting_upstream_updates_the_issue_with_one_comment(self):
        u = self.make_conflict()
        self.run_sync()
        u2 = self.remotes.commit(u, {"shared.txt": "upstream change again\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u2)
        self.run_sync()
        issue = self.forgejo.open_issues()[0]
        self.assertIn(f"upstream={u2}", issue["body"].split("\n", 1)[0])
        self.assertEqual(len(self.forgejo.comments[issue["number"]]), 1)

    def test_clean_merge_later_closes_the_tracking_issue(self):
        self.make_conflict()
        self.run_sync()
        u3 = self.remotes.commit(self.u1, {"other.txt": "no conflict\n"})
        self.remotes.set_ref("upstream", "refs/heads/main", u3)
        self.assertEqual(self.run_sync().status, "created")
        self.assertEqual(self.forgejo.open_issues(), [])


class MergeTreeHandling(UpstreamTestCase):
    def fake_merge_tree(self, returncode: int, stdout: bytes, stderr: bytes = b""):
        def hook(g):
            real = g.run_raw

            def run_raw(args, **kwargs):
                if args and args[0] == "merge-tree":
                    return subprocess.CompletedProcess(args, returncode, stdout, stderr)
                return real(args, **kwargs)

            g.run_raw = run_raw

        return hook

    def assert_execution_failure(self, hook):
        with self.assertRaises(MergeExecutionError):
            self.run_sync(git_hook=hook)
        self.assertEqual(self.forgejo.issues, {})
        self.assertIsNone(self.head())

    def test_exit_2_is_an_execution_failure_not_a_conflict(self):
        self.assert_execution_failure(self.fake_merge_tree(2, b"", b"fatal: something broke"))

    def test_exit_2_with_clean_looking_output_is_not_a_clean_merge(self):
        tree = git(self.remotes.work, "rev-parse", f"{self.f1}^{{tree}}")
        def hook(g):
            g.run(["fetch", "--quiet", self.remotes.url("forgejo"), "refs/heads/Main:refs/sync/x"])
            self.fake_merge_tree(2, tree.encode() + b"\x00\x00", b"fatal: interrupted")(g)
        self.assert_execution_failure(hook)

    def test_exit_2_with_conflict_looking_output_is_not_a_conflict(self):
        self.assert_execution_failure(self.fake_merge_tree(2, b"a" * 40 + b"\x00f\x00\x00", b"fatal"))

    def test_exit_128_is_an_execution_failure(self):
        self.assert_execution_failure(self.fake_merge_tree(128, b"", b"fatal: bad object"))

    def test_exit_1_without_a_merge_result_is_an_execution_failure(self):
        # git merge-tree exits 1 for an unknown object too, with nothing on stdout.
        self.assert_execution_failure(self.fake_merge_tree(1, b"", b"merge-tree: x - not something we can merge"))

    def test_exit_1_without_conflicted_paths_is_an_execution_failure(self):
        self.assert_execution_failure(self.fake_merge_tree(1, b"a" * 40 + b"\x00\x00"))

    def test_exit_0_with_garbage_is_an_execution_failure(self):
        self.assert_execution_failure(self.fake_merge_tree(0, b"not a tree id\x00"))

    def test_exit_0_naming_a_non_tree_is_an_execution_failure(self):
        blob = git(self.remotes.work, "rev-parse", f"{self.c0}:README")
        def hook(g):
            g.run(["fetch", "--quiet", self.remotes.url("forgejo"), "refs/heads/Main:refs/sync/x"])
            self.fake_merge_tree(0, blob.encode() + b"\x00\x00")(g)
        self.assert_execution_failure(hook)

    def test_wrong_parents_are_rejected(self):
        def hook(g):
            g.parents = lambda sha: ["0" * 40]
        self.assert_execution_failure(hook)

    def test_real_unknown_object_is_execution_failure(self):
        sync = self.sync()
        with self.assertRaises(MergeExecutionError):
            sync.merge("test", "1" * 40, "2" * 40, "m")

    def test_parse_merge_tree_layout(self):
        tree = "a" * 40
        out = (tree + "\x00f\x00\x001\x00f\x00Auto-merging\x00Auto-merging f\n\x00"
               "1\x00f\x00CONFLICT (contents)\x00CONFLICT (content): Merge conflict in f\n\x00").encode()
        parsed_tree, paths, messages = parse_merge_tree(out)
        self.assertEqual((parsed_tree, paths), (tree, ["f"]))
        self.assertEqual(len(messages), 2)
        self.assertEqual(parse_merge_tree((tree + "\x00\x00").encode()), (tree, [], []))
        for bad in (b"", b"xyz\x00", (tree + "\x00f").encode(), (tree + "\x00\x00zz\x00").encode(),
                    (tree + "\x00\x009\x00f\x00").encode()):
            with self.subTest(bad=bad), self.assertRaises(MergeExecutionError):
                parse_merge_tree(bad)


class MainEntryPoint(UpstreamTestCase):
    """``sync_upstream.main()`` as the workflow runs it, with and without Cloudflare Access."""

    def main_env(self, **extra) -> str:
        summary = os.path.join(self.remotes.root, "summary.md")
        os.environ.update({
            "SYNC_TESTING": "1", "FORGEJO_URL": self.forgejo.url, "FORGEJO_GIT_URL": self.remotes.url("forgejo"),
            "FORGEJO_USER": "pdfcraft-sync", "FORGEJO_TOKEN": FORGEJO_TOKEN,
            "UPSTREAM_GIT_URL": self.remotes.url("upstream"), "SYNC_COMMITTER_EMAIL": "sync@example.invalid",
            "GITHUB_ACTIONS": "true", "GITHUB_STEP_SUMMARY": summary,
            "no_proxy": "127.0.0.1", "NO_PROXY": "127.0.0.1", **extra,
        })
        return summary

    def run_main(self) -> tuple[int, str, str]:
        sync_upstream.REDACTOR.__init__()
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = sync_upstream.main()
        return code, out.getvalue(), err.getvalue()

    def test_with_cloudflare_creates_the_pr_and_masks_first(self):
        self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET)
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        self.assertEqual(len(self.forgejo.open_pulls()), 1)
        for headers in self.forgejo.headers:
            self.assertEqual(headers["cf-access-client-id"], CF_ID)
            self.assertEqual(headers["authorization"], f"token {FORGEJO_TOKEN}")
        lines = stdout.splitlines()
        first_other = next(i for i, line in enumerate(lines) if not line.startswith("::add-mask::"))
        masked = {line[len("::add-mask::"):] for line in lines[:first_other]}
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        self.assertLessEqual({FORGEJO_TOKEN, b64, CF_ID, CF_SECRET}, masked)
        self.main_is(self.f1)

    def test_without_cloudflare_is_unchanged(self):
        self.main_env()
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 0, stdout)
        self.assertEqual(len(self.forgejo.open_pulls()), 1)
        self.assertNotIn("cf-access-client-id", self.forgejo.headers[0])

    def test_half_configured_cloudflare_fails_before_any_request(self):
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
                self.assertIsNone(self.head())

    def test_failure_redacts_every_credential(self):
        summary = self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET=CF_SECRET)
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        self.forgejo.faults.append(Fault("GET", r"/pulls$", 403, times=10, body={
            "message": f"echo {FORGEJO_TOKEN} CF-Access-Client-Id: {CF_ID} {CF_SECRET}"}))
        code, stdout, stderr = self.run_main()
        self.assertEqual(code, 1)
        with open(summary, encoding="utf-8") as handle:
            summary_text = handle.read()
        logged = "\n".join(line for line in stdout.splitlines() if not line.startswith("::add-mask::"))
        b64 = base64.b64encode(f"pdfcraft-sync:{FORGEJO_TOKEN}".encode()).decode()
        self.assertIn("HTTP 403", logged)
        for text in (logged, stderr, summary_text):
            for secret in (FORGEJO_TOKEN, b64, CF_ID, CF_SECRET):
                self.assertNotIn(secret, text)

    def test_cloudflare_rejection_reports_a_refused_redirect(self):
        self.main_env(CF_ACCESS_CLIENT_ID=CF_ID, CF_ACCESS_CLIENT_SECRET="cf_rotated_secret_value")
        self.forgejo.require_cf = (CF_ID, CF_SECRET)
        code, stdout, _ = self.run_main()
        self.assertEqual(code, 1)
        self.assertIn("redirect refused", stdout)
        self.assertNotIn("invalid JSON", stdout)
        self.assertIsNone(self.head())


if __name__ == "__main__":
    unittest.main()
