#!/usr/bin/env python3
"""Workflow B: daily trial integration of upstream PdfCraft into a Forgejo review PR.

Run by ``.github/workflows/forgejo-upstream-sync.yml``. It never merges into ``Main`` and never
force-pushes. Everything happens in a private, throwaway bare repository: the trial merge is
``git merge-tree --write-tree``, so no working tree, and so no conflict marker, ever exists.

State is derived each run, never stored here:

- Forgejo PRs whose first body line is
  ``<!-- pdfcraft-upstream-sync:v1 branch=<b> upstream=<U> main=<F> head=<H> -->``,
- the tip of the integration branch ``upstream-sync/main`` (or ``upstream-sync/main-v<k>``),
- the open tracking issue whose first body line starts with
  ``<!-- pdfcraft-upstream-sync-conflict:v1``.

Automation merge commits carry the trailer ``Pdfcraft-Sync: upstream-merge``. A commit on the
integration branch without it is a person's work: automatic updates then pause, so manual work
is never overwritten or disturbed. See README.md for the full branch lifecycle.

Exit status: 0 for every expected state (up to date, no-op, clean update, conflict tracked,
paused, frozen), 1 for execution failures and states a person must untangle.
"""

from __future__ import annotations

import os
import re
import sys
from dataclasses import dataclass, field

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from common import (  # noqa: E402
    REDACTOR,
    CloudflareAccess,
    Git,
    GitAuth,
    GitError,
    Reporter,
    SyncError,
    ValidationError,
    env_flag,
    env_str,
    git_identity,
    install_redacting_excepthook,
    md_code,
    printable,
    validate_branch_name,
    validate_header_value,
    validate_https_url,
    validate_repo_slug,
    validate_sha,
)
from forgejo_client import ApiError, ConflictError, ForgejoClient  # noqa: E402

PR_MARKER_RE = re.compile(
    r"^<!-- pdfcraft-upstream-sync:v1 branch=(?P<branch>upstream-sync/main(?:-v[1-9][0-9]{0,3})?)"
    r" upstream=(?P<u>[0-9a-f]{40}) main=(?P<f>[0-9a-f]{40}) head=(?P<h>[0-9a-f]{40}) -->$"
)
ISSUE_MARKER_PREFIX = "<!-- pdfcraft-upstream-sync-conflict:v1"
ISSUE_MARKER_RE = re.compile(
    r"^<!-- pdfcraft-upstream-sync-conflict:v1 upstream=(?P<u>[0-9a-f]{40}) main=(?P<f>[0-9a-f]{40}) -->$"
)
EVENT_MARKER = "<!-- pdfcraft-upstream-sync-event:{kind}:{key} -->"
TRAILER = "Pdfcraft-Sync: upstream-merge"
DEFAULT_BRANCH = "upstream-sync/main"

# Paths whose changes deserve a reviewer's eye, mapped to the fork feature they can break.
SENSITIVE_PATHS = (
    ("crates/overlay/", "Six-step Overlay wizard and alignment"),
    ("crates/measure/", "Distance, perimeter and area measurements; persistent measurement annotations"),
    ("crates/annot/", "Persistent measurement annotations"),
    ("crates/render/", "PDF rendering, resources and optional-content layers"),
    ("crates/model/", "PDF rendering, resources and optional-content layers"),
    ("crates/cos/", "PDF rendering, resources and optional-content layers"),
    ("crates/ui-egui/src/i18n/", "Fork English and Spanish localization"),
    ("crates/ui-egui/", "Thin Lines, rulers and guides, Overlay wizard UI"),
    ("packaging/windows", "Windows portable builds"),
    (".github/workflows/", "Windows portable builds and CI"),
    ("Cargo.toml", "Dependencies (egui 0.37 migration)"),
    ("Cargo.lock", "Dependencies (egui 0.37 migration)"),
)

REGRESSION_CHECKLIST = (
    "Six-step Overlay wizard and alignment",
    "Thin Lines",
    "Distance, perimeter and area measurements",
    "Persistent measurement annotations",
    "Rulers and guides",
    "PDF rendering, resources and optional-content layers",
    "Windows portable builds (run the `overlay windows build` workflow by hand on this branch's merge result)",
    "Fork-specific English and Spanish localization",
)


@dataclass
class Config:
    forgejo_url: str
    forgejo_repo: str
    forgejo_git_url: str
    forgejo_user: str
    forgejo_token: str
    upstream_git_url: str
    upstream_branch: str
    upstream_name: str
    base_branch: str
    branch_version: int | None
    committer_name: str
    committer_email: str
    dry_run: bool = False
    object_cache: str = ""
    # The Cloudflare Access service token for the Forgejo host; None where Access lets the
    # runner through without one (local and bypassed deployments).
    cf_access: CloudflareAccess | None = None

    @classmethod
    def from_env(cls) -> "Config":
        allow_local = env_flag("SYNC_TESTING")
        forgejo_url = validate_https_url(env_str("FORGEJO_URL", "https://git.ferrive.me"), "Forgejo URL", allow_local)
        forgejo_repo = validate_repo_slug(env_str("FORGEJO_REPO", "sefr/pdfcraft"), "Forgejo repository")
        raw_version = env_str("BRANCH_VERSION").strip()
        version = None
        if raw_version:
            if not raw_version.isdigit() or not 2 <= int(raw_version) <= 9999:
                raise ValidationError(f"BRANCH_VERSION must be an integer from 2 to 9999, not {printable(raw_version)!r}")
            version = int(raw_version)
        email = env_str("SYNC_COMMITTER_EMAIL", "pdfcraft-sync@noreply.git.ferrive.me")
        if not re.match(r"^[A-Za-z0-9._+-]+@[A-Za-z0-9.-]+$", email):
            raise ValidationError("invalid SYNC_COMMITTER_EMAIL")
        return cls(
            forgejo_url=forgejo_url,
            forgejo_repo=forgejo_repo,
            forgejo_git_url=validate_https_url(env_str("FORGEJO_GIT_URL", f"{forgejo_url}/{forgejo_repo}.git"), "Forgejo Git URL", allow_local),
            forgejo_user=env_str("FORGEJO_USER"),
            forgejo_token=env_str("FORGEJO_TOKEN"),
            upstream_git_url=validate_https_url(env_str("UPSTREAM_GIT_URL", "https://github.com/storytold/pdfcraft.git"), "upstream URL", allow_local),
            upstream_branch=validate_branch_name(env_str("UPSTREAM_BRANCH", "main")),
            upstream_name=validate_repo_slug(env_str("UPSTREAM_NAME", "storytold/pdfcraft")),
            base_branch=validate_branch_name(env_str("BASE_BRANCH", "Main")),
            branch_version=version,
            committer_name=printable(env_str("SYNC_COMMITTER_NAME", "PdfCraft upstream sync"), limit=80),
            committer_email=email,
            dry_run=env_flag("DRY_RUN"),
            object_cache=env_str("SYNC_OBJECT_CACHE"),
            cf_access=CloudflareAccess.from_env(),
        )


@dataclass
class Outcome:
    status: str
    exit_code: int
    detail: str = ""
    writes: list[str] = field(default_factory=list)


class MergeConflict(Exception):
    def __init__(self, stage: str, ours: str, theirs: str, paths: list[str], messages: list[str]):
        super().__init__(stage)
        self.stage = stage
        self.ours = ours
        self.theirs = theirs
        self.paths = paths
        self.messages = messages


class MergeExecutionError(SyncError):
    """merge-tree failed to run or produced output that is not a merge result."""


def parse_merge_tree(stdout: bytes) -> tuple[str, list[str], list[str]]:
    """Parses ``git merge-tree --write-tree -z --name-only --messages`` output.

    Layout: ``<tree>\\0`` then conflicted paths each ``\\0``-terminated, an empty field, then
    informational messages as ``<n>\\0<path>\\0…<type>\\0<message>\\0``.
    """
    fields = stdout.split(b"\x00")
    if not fields or not re.match(rb"^[0-9a-f]{40}$", fields[0]):
        raise MergeExecutionError("merge-tree output does not start with a tree id")
    tree = fields[0].decode("ascii")
    paths: list[str] = []
    i = 1
    while i < len(fields) and fields[i] != b"":
        paths.append(fields[i].decode("utf-8", errors="replace"))
        i += 1
    if i >= len(fields):
        raise MergeExecutionError("merge-tree output is truncated")
    i += 1  # the empty field ending the conflicted-file section
    messages: list[str] = []
    while i < len(fields):
        count = fields[i]
        if count == b"":
            i += 1
            continue
        if not count.isdigit():
            raise MergeExecutionError("merge-tree message section is malformed")
        n = int(count)
        if n > 1000 or i + n + 2 >= len(fields):
            raise MergeExecutionError("merge-tree message section is malformed")
        msg_paths = [p.decode("utf-8", errors="replace") for p in fields[i + 1:i + 1 + n]]
        kind = fields[i + 1 + n].decode("utf-8", errors="replace")
        text = fields[i + 2 + n].decode("utf-8", errors="replace")
        messages.append(f"{kind}: {text.strip()} ({', '.join(msg_paths)})")
        i += n + 3
    return tree, paths, messages


class UpstreamSync:
    def __init__(self, cfg: Config, forgejo: ForgejoClient, git: Git, reporter: Reporter):
        self.cfg = cfg
        self.forgejo = forgejo
        self.git = git
        self.rep = reporter
        self.writes: list[str] = []

    # -- helpers --------------------------------------------------------------------------------

    def _done(self, status: str, exit_code: int, detail: str, extra: str = "") -> Outcome:
        (self.rep.error if exit_code else self.rep.log)(f"{status}: {detail}")
        self.rep.add_summary(f"### Upstream sync\n\n**{status}** — {detail}" + (f"\n\n{extra}" if extra else ""))
        return Outcome(status, exit_code, detail, self.writes)

    def _write(self, description: str) -> bool:
        self.writes.append(description)
        if self.cfg.dry_run:
            self.rep.log(f"[dry-run] would {description}")
            return False
        return True

    def _comment_once(self, index: int, kind: str, key: str, text: str) -> None:
        if self._write(f"comment on Forgejo #{index} ({kind})"):
            self.forgejo.comment_once(index, EVENT_MARKER.format(kind=kind, key=key), text)

    def merge(self, stage: str, ours: str, theirs: str, message: str) -> str:
        """Trial-merges ``theirs`` into ``ours``. Returns the validated merge commit.

        Git's exit status and structured output alone decide: 0 is clean, 1 *with a parsable
        merge result* is a conflict, anything else is an execution failure.
        """
        proc = self.git.run_raw(
            ["merge-tree", "--write-tree", "-z", "--name-only", "--messages", "--end-of-options",
             validate_sha(ours), validate_sha(theirs)]
        )
        if proc.returncode not in (0, 1):
            raise MergeExecutionError(
                f"git merge-tree ({stage}) exited {proc.returncode}: {self.rep.redactor.redact(proc.stderr).strip()[-2000:]}"
            )
        try:
            tree, paths, messages = parse_merge_tree(proc.stdout)
        except MergeExecutionError as exc:
            raise MergeExecutionError(
                f"git merge-tree ({stage}) exited {proc.returncode} without a merge result: {exc}; "
                f"{self.rep.redactor.redact(proc.stderr).strip()[-2000:]}"
            ) from None
        if proc.returncode == 1:
            if not paths:
                raise MergeExecutionError(f"git merge-tree ({stage}) reported a conflict without conflicted paths")
            raise MergeConflict(stage, ours, theirs, paths, messages)
        if paths:
            raise MergeExecutionError(f"git merge-tree ({stage}) exited 0 but listed conflicted paths")
        if self.git.object_type(tree) != "tree":
            raise MergeExecutionError(f"git merge-tree ({stage}) named {tree}, which is not a tree")
        body = f"{message}\n\n{TRAILER}\n".encode("utf-8")
        commit = self.git.run(["commit-tree", tree, "-p", ours, "-p", theirs, "-F", "-"], input_bytes=body)
        sha = validate_sha(commit.decode("ascii", errors="replace").strip(), "merge commit")
        if self.git.parents(sha) != [ours, theirs]:
            raise MergeExecutionError(f"merge commit {sha} does not have the parents {ours[:12]}, {theirs[:12]}")
        if not (self.git.is_ancestor(ours, sha) and self.git.is_ancestor(theirs, sha)):
            raise MergeExecutionError(f"merge commit {sha} does not descend from both sides")
        return sha

    def _sync_prs(self) -> list[tuple[dict, re.Match]]:
        found = []
        for pr in self.forgejo.list_pulls("all"):
            first = str(pr.get("body") or "").split("\n", 1)[0].strip()
            match = PR_MARKER_RE.match(first)
            if match and (pr.get("head") or {}).get("ref") == match.group("branch"):
                found.append((pr, match))
        found.sort(key=lambda item: int(item[0].get("number") or 0))
        return found

    def _conflict_issue(self) -> dict | None:
        for issue in self.forgejo.list_issues("open"):
            if str(issue.get("body") or "").startswith(ISSUE_MARKER_PREFIX):
                return issue
        return None

    def _resolve_issue(self, reason: str) -> None:
        issue = self._conflict_issue()
        if issue is None:
            return
        index = int(issue.get("number") or 0)
        if self._write(f"comment on and close tracking issue #{index}"):
            self.forgejo.create_comment(index, f"Resolved: {reason}")
            self.forgejo.edit_issue(index, state="closed")

    def _human_commits(self, tip: str, main_sha: str, upstream_sha: str) -> list[str]:
        """Commits on the integration branch that are neither the automation's nor F's nor U's."""
        return [
            sha for sha in self.git.commits_in(tip, [main_sha, upstream_sha], limit=2000)
            if TRAILER not in self.git.commit_message(sha)
        ]

    def _sensitive(self, base: str, head: str) -> list[str]:
        out = self.git.run(["diff", "--name-only", "-z", "--no-renames", validate_sha(base), validate_sha(head)])
        changed = [p.decode("utf-8", "replace") for p in out.split(b"\x00") if p]
        hits: dict[str, list[str]] = {}
        for path in changed:
            for prefix, feature in SENSITIVE_PATHS:
                if path == prefix or path.startswith(prefix):
                    hits.setdefault(feature, []).append(path)
                    break
        lines = [f"Files changed against Forgejo `{self.cfg.base_branch}`: **{len(changed)}**", ""]
        for feature, paths in hits.items():
            shown = ", ".join(md_code(p) for p in paths[:8]) + (f" … (+{len(paths) - 8})" if len(paths) > 8 else "")
            lines.append(f"- **{feature}** — {len(paths)} file(s): {shown}")
        return lines

    def _upstream_log(self, main_sha: str, upstream_sha: str, limit: int = 60) -> list[str]:
        out = self.git.run(
            ["log", f"--max-count={limit}", "--no-merges", "--format=%h %s", validate_sha(upstream_sha), f"^{validate_sha(main_sha)}"]
        ).decode("utf-8", "replace")
        return [f"- {md_code(line)}" for line in out.splitlines()]

    # -- main flow --------------------------------------------------------------------------------

    def run(self) -> Outcome:
        cfg = self.cfg
        main_sha = self.git.fetch(cfg.forgejo_git_url, f"refs/heads/{cfg.base_branch}", "refs/sync/main")
        upstream_sha = self.git.fetch(cfg.upstream_git_url, f"refs/heads/{cfg.upstream_branch}", "refs/sync/upstream")
        base = self.git.merge_base(main_sha, upstream_sha)
        if base is None:
            return self._done("attention", 1, "Forgejo Main and upstream share no history")
        fork_only, upstream_only = self.git.count_left_right(main_sha, upstream_sha)
        facts = (
            f"| | |\n|---|---|\n| Upstream `{cfg.upstream_name}` `{cfg.upstream_branch}` | `{upstream_sha}` |\n"
            f"| Forgejo `{cfg.base_branch}` | `{main_sha}` |\n| Merge base | `{base}` |\n"
            f"| Fork-only commits | {fork_only} |\n| Upstream-only commits | {upstream_only} |"
        )
        self.rep.log(f"Forgejo {cfg.base_branch} {main_sha}, upstream {upstream_sha}, base {base}, {fork_only} fork-only / {upstream_only} upstream-only")

        prs = self._sync_prs()
        open_prs = [(pr, m) for pr, m in prs if pr.get("state") == "open"]
        if len(open_prs) > 1:
            numbers = ", ".join(f"#{pr.get('number')}" for pr, _ in open_prs)
            return self._done("attention", 1, f"several upstream sync PRs are open ({numbers}); none changed", facts)

        # Up to date: upstream is fully in Main.
        if self.git.is_ancestor(upstream_sha, main_sha):
            if open_prs:
                pr, _ = open_prs[0]
                self._comment_once(int(pr.get("number") or 0), "redundant", f"{upstream_sha}-{main_sha}",
                                   f"Upstream `{upstream_sha}` is already fully contained in Forgejo `{cfg.base_branch}` `{main_sha}`. "
                                   "This PR may be redundant; it is left for a person to close.")
            self._resolve_issue(f"upstream `{upstream_sha}` is contained in Forgejo `{cfg.base_branch}` `{main_sha}`.")
            return self._done("up-to-date", 0, f"upstream {upstream_sha[:12]} is already in Forgejo {cfg.base_branch}", facts)

        # Which integration branch is active.
        active_pr, active_match = open_prs[0] if open_prs else (None, None)
        if active_pr is not None:
            branch = active_match.group("branch")
            if cfg.branch_version is not None and branch != f"{DEFAULT_BRANCH}-v{cfg.branch_version}":
                self.rep.warning(f"branch_version ignored: sync PR #{active_pr.get('number')} on {branch} is open")
        elif cfg.branch_version is not None:
            branch = f"{DEFAULT_BRANCH}-v{cfg.branch_version}"
        elif prs:
            branch = prs[-1][1].group("branch")
        else:
            branch = DEFAULT_BRANCH
        validate_branch_name(branch)
        branch_ref = f"refs/heads/{branch}"
        branch_prs = [(pr, m) for pr, m in prs if m.group("branch") == branch]
        latest = branch_prs[-1] if branch_prs else None

        # Frozen: the latest PR on this branch was closed without merging.
        if active_pr is None and latest is not None and latest[0].get("state") == "closed" and not latest[0].get("merged"):
            versions = [int(m.group("branch").rsplit("-v", 1)[1]) for _, m in prs if "-v" in m.group("branch")]
            proposal = max(versions + [1]) + 1
            self._track_state(upstream_sha, main_sha, facts,
                              f"The upstream sync PR #{latest[0].get('number')} on `{branch}` was closed without merging, so "
                              f"`{branch}` is frozen and is not reused. To start again, run the workflow by hand with "
                              f"`branch_version` = `{proposal}` (branch `{DEFAULT_BRANCH}-v{proposal}`).")
            self.rep.warning(f"{branch} is frozen; dispatch with branch_version={proposal} to continue")
            return self._done("frozen", 0, f"{md_code(branch)} is frozen (PR #{latest[0].get('number')} closed unmerged); proposed `branch_version={proposal}`", facts)

        # Upstream rewritten since the open PR was made.
        if active_match is not None:
            recorded_u = active_match.group("u")
            if not self.git.has_commit(recorded_u):
                tip_now = self.git.ls_remote(cfg.forgejo_git_url, branch_ref)
                if tip_now:
                    self.git.fetch(cfg.forgejo_git_url, branch_ref, "refs/sync/branch")
            if not self.git.has_commit(recorded_u) or not self.git.is_ancestor(recorded_u, upstream_sha):
                self._comment_once(int(active_pr.get("number") or 0), "upstream-rewritten", upstream_sha,
                                   f"Upstream `{cfg.upstream_branch}` is now `{upstream_sha}`, which does not descend from the "
                                   f"recorded `{recorded_u}`: upstream history was rewritten. Nothing was updated.")
                return self._done("upstream-rewritten", 1, f"upstream {upstream_sha[:12]} does not descend from recorded {recorded_u[:12]}", facts)

        # The integration branch.
        tip = self.git.ls_remote(cfg.forgejo_git_url, branch_ref)
        if tip is not None:
            fetched_tip = self.git.fetch(cfg.forgejo_git_url, branch_ref, "refs/sync/branch")
            if fetched_tip != tip:
                return self._done("attention", 1, f"{md_code(branch)} moved while fetching; try again", facts)
        if tip is None and active_pr is not None:
            return self._done("attention", 1, f"sync PR #{active_pr.get('number')} is open but {md_code(branch)} is gone; not recreated", facts)

        if tip is not None and not self.git.is_ancestor(tip, main_sha):
            human = self._human_commits(tip, main_sha, upstream_sha)
            if human:
                if active_pr is None:
                    return self._done("collision", 1, f"{md_code(branch)} exists with commits the automation did not make and has no sync PR; nothing changed", facts)
                if not (active_match.group("u") == upstream_sha and active_match.group("f") == main_sha):
                    self._comment_once(int(active_pr.get("number") or 0), "paused", f"{upstream_sha}-{main_sha}",
                                       "Automatic updates are **paused** because this branch has commits made by a person "
                                       f"({', '.join('`' + h[:12] + '`' for h in human[:10])}). Nothing is pushed over manual work.\n\n"
                                       f"Upstream is now `{upstream_sha}` and Forgejo `{cfg.base_branch}` is `{main_sha}`. To bring them in by hand:\n\n"
                                       f"```\ngit fetch <forgejo> {branch} {cfg.base_branch}\ngit fetch {cfg.upstream_git_url} {cfg.upstream_branch}\n"
                                       f"git switch {branch}\ngit merge {main_sha}\ngit merge {upstream_sha}\ngit push <forgejo> {branch}\n```")
                self.rep.warning(f"{branch} has manual commits; automatic updates paused")
                return self._done("paused", 0, f"{md_code(branch)} has {len(human)} manual commit(s); automatic updates paused", facts)
            start = tip  # our own commits only (an interrupted earlier run, or an open PR)
        else:
            start = main_sha  # absent, or fully merged into Main: B ⊂ F, so building on F is a fast-forward

        # Repeat run with nothing new.
        if active_match is not None and tip is not None and active_match.group("u") == upstream_sha \
                and active_match.group("f") == main_sha and active_match.group("h") == tip:
            return self._done("no-change", 0, f"sync PR #{active_pr.get('number')} is current", facts)

        # Trial merges, appending to the branch's history.
        head = start
        try:
            if not self.git.is_ancestor(main_sha, head):
                head = self.merge("Forgejo Main into the integration branch", head, main_sha,
                                  f"Merge Forgejo {cfg.base_branch} {main_sha[:12]} into {branch}")
            if not self.git.is_ancestor(upstream_sha, head):
                head = self.merge("upstream into the integration branch", head, upstream_sha,
                                  f"Merge {cfg.upstream_name} {cfg.upstream_branch} {upstream_sha[:12]} into {branch}")
        except MergeConflict as conflict:
            return self._conflict(conflict, upstream_sha, main_sha, base, facts, branch)

        # Validate the result before anything leaves this repository.
        for name, sha in (("Forgejo Main", main_sha), ("upstream", upstream_sha)) + ((("branch tip", tip),) if tip else ()):
            if not self.git.is_ancestor(sha, head):
                raise MergeExecutionError(f"integration result {head[:12]} does not contain {name} {sha[:12]}")
        if self.git.tree_of(head) == self.git.tree_of(main_sha):
            self._resolve_issue(f"upstream `{upstream_sha}` adds no content to Forgejo `{cfg.base_branch}` `{main_sha}`.")
            return self._done("no-content-change", 0, "merging upstream changes no files; nothing pushed", facts)

        if head != tip:
            if not self._write(f"push {head[:12]} to Forgejo {branch} (no force)"):
                return self._done("dry-run", 0, "planned writes listed above", facts)
            try:
                self.git.push(cfg.forgejo_git_url, head, branch_ref)
            except GitError as exc:
                now = self.git.ls_remote(cfg.forgejo_git_url, branch_ref)
                successor = max([int(m.group("branch").rsplit("-v", 1)[1]) for _, m in prs if "-v" in m.group("branch")] + [1]) + 1
                return self._done("cannot-fast-forward", 1,
                                  f"Forgejo refused the push to {md_code(branch)} (tip now {(now or 'none')[:12]}); nothing forced. "
                                  f"If the branch was rewritten on purpose, run by hand with `branch_version={successor}`. ({exc})", facts)
            if self.git.ls_remote(cfg.forgejo_git_url, branch_ref) != head:
                return self._done("attention", 1, f"after the push {md_code(branch)} is not at {head[:12]}", facts)

        marker = f"<!-- pdfcraft-upstream-sync:v1 branch={branch} upstream={upstream_sha} main={main_sha} head={head} -->"
        body = "\n".join([marker, self._pr_body(branch, upstream_sha, main_sha, base, head, facts)])
        title = f"Upstream sync: {cfg.upstream_name} {cfg.upstream_branch} @ {upstream_sha[:12]} into {cfg.base_branch} (review, do not auto-merge)"
        if active_pr is None:
            if not self._write(f"create Forgejo PR {branch} -> {cfg.base_branch}"):
                return self._done("dry-run", 0, "planned writes listed above", facts)
            try:
                pr = self.forgejo.create_pull(head=branch, base=cfg.base_branch, title=title, body=body)
            except ConflictError:
                again = [p for p, m in self._sync_prs() if p.get("state") == "open" and m.group("branch") == branch]
                if not again:
                    raise
                pr = again[0]
                self.forgejo.edit_pull(int(pr.get("number") or 0), title=title, body=body)
            action = "created"
        else:
            pr = active_pr
            if self._write(f"update sync PR #{pr.get('number')}"):
                self.forgejo.edit_pull(int(pr.get("number") or 0), title=title, body=body)
            self._comment_once(int(pr.get("number") or 0), "updated", f"{upstream_sha}-{main_sha}",
                               f"Updated: upstream `{upstream_sha}`, Forgejo `{cfg.base_branch}` `{main_sha}`, head `{head}`. "
                               "Re-run the regression checklist.")
            action = "updated"
        if self.cfg.dry_run:
            return self._done("dry-run", 0, "planned writes listed above", facts)

        index = int(pr.get("number") or 0)
        check = self.forgejo.get_pull(index)
        if (check.get("head") or {}).get("sha") != head or (check.get("base") or {}).get("ref") != cfg.base_branch:
            return self._done("attention", 1, f"sync PR #{index} does not show head {head[:12]} into {md_code(cfg.base_branch)}", facts)
        merge_base = check.get("merge_base")
        if merge_base and merge_base != main_sha:
            self.rep.warning(f"sync PR #{index} reports merge base {printable(merge_base, 40)}, expected {main_sha}")
        self._resolve_issue(f"a clean trial merge is in sync PR #{index} (`{head}`).")
        return self._done(action, 0, f"sync PR #{index} at {head[:12]} — a clean merge is not proof of compatibility; review it", facts)

    def _pr_body(self, branch: str, upstream_sha: str, main_sha: str, base: str, head: str, facts: str) -> str:
        cfg = self.cfg
        checklist = "\n".join(f"- [ ] {item}" for item in REGRESSION_CHECKLIST)
        return "\n".join(
            [
                f"Trial integration of `{cfg.upstream_name}` `{cfg.upstream_branch}` into Forgejo `{cfg.base_branch}`, prepared by "
                "`scripts/automation/sync_upstream.py`. **Unmerged on purpose: review before merging.**",
                "",
                facts,
                f"| Integration branch | `{branch}` |",
                f"| Integration head | `{head}` |",
                "",
                "> [!WARNING]",
                "> A clean Git merge is not proof of functional compatibility. This is high-risk integration work: "
                "upstream has restructured the engine and UI and moved to egui 0.37. Build it, run the tests and check every item below.",
                "",
                "### Regression-sensitive features",
                "",
                checklist,
                "",
                "### Changes in sensitive areas",
                "",
                *self._sensitive(main_sha, head),
                "",
                "### Upstream commits (newest first, capped)",
                "",
                *self._upstream_log(main_sha, upstream_sha),
                "",
                f"Each run appends merges to `{branch}` without rewriting it. Commits a person adds here pause automatic updates.",
            ]
        )

    def _track_state(self, upstream_sha: str, main_sha: str, facts: str, report: str) -> None:
        """Creates or updates the one tracking issue, commenting only when (U, F) changes."""
        marker = f"{ISSUE_MARKER_PREFIX} upstream={upstream_sha} main={main_sha} -->"
        body = "\n".join([marker, f"Upstream sync needs attention.\n\n{facts}\n\n{report}"])
        issue = self._conflict_issue()
        title = f"Upstream sync blocked: {self.cfg.upstream_name} {self.cfg.upstream_branch} into {self.cfg.base_branch}"
        if issue is None:
            if self._write("create the upstream sync tracking issue"):
                self.forgejo.create_issue(title=title, body=body)
            return
        index = int(issue.get("number") or 0)
        match = ISSUE_MARKER_RE.match(str(issue.get("body") or "").split("\n", 1)[0].strip())
        if match and match.group("u") == upstream_sha and match.group("f") == main_sha:
            return
        if self._write(f"update tracking issue #{index}"):
            self.forgejo.edit_issue(index, body=body)
            self.forgejo.create_comment(index, f"Updated for upstream `{upstream_sha}` and Forgejo `{self.cfg.base_branch}` `{main_sha}`.")

    def _conflict(self, conflict: MergeConflict, upstream_sha: str, main_sha: str, base: str, facts: str, branch: str) -> Outcome:
        paths = "\n".join(f"- {md_code(p)}" for p in conflict.paths[:300])
        if len(conflict.paths) > 300:
            paths += f"\n- … and {len(conflict.paths) - 300} more"
        messages = "\n".join(f"- {md_code(m)}" for m in conflict.messages[:100])
        report = "\n".join(
            [
                f"Trial merge **{conflict.stage}** conflicts: `{conflict.theirs}` into `{conflict.ours}` (integration branch `{branch}`).",
                "Nothing was committed or pushed, and Forgejo `Main` is untouched.",
                "",
                f"#### Conflicted paths ({len(conflict.paths)})",
                "",
                paths,
                "",
                "#### Git diagnostics (`git merge-tree --write-tree --messages`)",
                "",
                messages or "- (none)",
                "",
                "Reproduce locally with:",
                "",
                f"```\ngit merge-tree --write-tree --name-only {conflict.ours} {conflict.theirs}\n```",
            ]
        )
        self._track_state(upstream_sha, main_sha, facts, report)
        self.rep.warning(f"upstream sync conflicts in {len(conflict.paths)} path(s); see the tracking issue")
        return self._done("conflict", 0, f"{len(conflict.paths)} conflicted path(s) in {conflict.stage}; tracked in one Forgejo issue", report)


def main() -> int:
    install_redacting_excepthook()
    reporter: Reporter | None = None
    git: Git | None = None
    try:
        cfg = Config.from_env()
        if cfg.forgejo_token:
            # Before anything registers or prints it: a CR/LF would split a header or a mask line.
            validate_header_value(cfg.forgejo_token, "FORGEJO_TOKEN")
        # The same user name GitAuth falls back to, so base64(user:token) is always registered.
        REDACTOR.add_credential(cfg.forgejo_user or "pdfcraft-sync", cfg.forgejo_token)
        if cfg.cf_access is not None:
            cfg.cf_access.register(REDACTOR)
        reporter = Reporter(REDACTOR)
        if not cfg.forgejo_token and not cfg.dry_run:
            raise SyncError("FORGEJO_TOKEN is not set")
        auth = (GitAuth(cfg.forgejo_url, cfg.forgejo_user or "pdfcraft-sync", cfg.forgejo_token, cfg.cf_access)
                if cfg.forgejo_token else None)
        git = Git.temporary(auth=auth, identity=git_identity(cfg.committer_name, cfg.committer_email))
        if cfg.object_cache:
            git.add_alternate(cfg.object_cache)
        forgejo = ForgejoClient(cfg.forgejo_url, cfg.forgejo_repo, cfg.forgejo_token or None, cf_access=cfg.cf_access)
        return UpstreamSync(cfg, forgejo, git, reporter).run().exit_code
    except (SyncError, ApiError) as exc:
        reporter = reporter or Reporter(REDACTOR)
        reporter.error(f"upstream sync failed: {exc}")
        reporter.add_summary(f"**failed** — {exc}")
        return 1
    except Exception as exc:  # noqa: BLE001 - last line of defence: one redacted line, no traceback
        reporter = reporter or Reporter(REDACTOR)
        reporter.error(f"unexpected {type(exc).__name__}: {exc}")
        reporter.add_summary(f"**failed** — unexpected {type(exc).__name__}: {exc}")
        return 1
    finally:
        if git is not None:
            git.cleanup()
        if reporter is not None:
            reporter.finish()


if __name__ == "__main__":
    sys.exit(main())
