#!/usr/bin/env python3
"""Workflow A: bridge a same-repository GitHub pull request into a Forgejo pull request.

Run by ``.github/workflows/forgejo-pr-sync.yml`` from the trusted ``Main`` checkout. The pull
request's code is never checked out or executed: its commits are fetched as Git objects and
pushed on to Forgejo as data.

State is never stored here. Each run derives it from three places:

- the GitHub API (the PR's current head SHA, state, base and head repository),
- the Forgejo branch ``github-pr/<N>`` (its tip, read with ``git ls-remote``),
- the Forgejo pull request whose first body line is the automation-owned marker
  ``<!-- pdfcraft-sync:v1 source=github:<repo>#<N> synced_sha=<sha> -->``.

``synced_sha`` is only written after a verified push, so a run that fails anywhere leaves a
consistent state behind and the next run repairs it. See README.md for the full state model.

Exit status: 0 when the run did what it should (including "nothing to do", "superseded" and
"external fork rejected"), 1 when a person has to act (non-fast-forward, collision, policy
violation, API or Git failure).
"""

from __future__ import annotations

import os
import re
import sys
from dataclasses import dataclass, field

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from common import (  # noqa: E402
    REDACTOR,
    Git,
    GitAuth,
    GitError,
    Reporter,
    SyncError,
    ValidationError,
    env_flag,
    env_str,
    install_redacting_excepthook,
    md_code,
    printable,
    quote_untrusted,
    sanitize_title,
    validate_branch_name,
    validate_https_url,
    validate_repo_slug,
    validate_sha,
)
from forgejo_client import ApiError, ConflictError, ForgejoClient  # noqa: E402
from github_client import GitHubClient  # noqa: E402

MARKER_RE = re.compile(
    r"^<!-- pdfcraft-sync:v1 source=github:(?P<repo>[A-Za-z0-9._-]+/[A-Za-z0-9._-]+)#(?P<num>[0-9]{1,10})"
    r" synced_sha=(?P<sha>[0-9a-f]{40}|none) -->$"
)
EVENT_MARKER = "<!-- pdfcraft-sync-event:{kind}:{key} -->"
BRANCH_PREFIX = "github-pr/"


@dataclass
class Config:
    source_repo: str
    pr_number: int
    base_branch: str
    github_api_url: str
    source_git_url: str
    github_server_url: str
    github_token: str
    forgejo_url: str
    forgejo_repo: str
    forgejo_git_url: str
    forgejo_user: str
    forgejo_token: str
    event_action: str = ""
    event_head_sha: str = ""
    dry_run: bool = False
    object_cache: str = ""

    @classmethod
    def from_env(cls) -> "Config":
        allow_local = env_flag("SYNC_TESTING")
        source_repo = validate_repo_slug(env_str("SOURCE_REPO", env_str("GITHUB_REPOSITORY", required=True)))
        raw_number = env_str("PR_NUMBER", required=True).strip()
        if not raw_number.isdigit() or not 0 < int(raw_number) < 10**9:
            raise ValidationError(f"invalid PR_NUMBER {printable(raw_number)!r}")
        server = validate_https_url(env_str("GITHUB_SERVER_URL", "https://github.com"), "GitHub URL", allow_local)
        forgejo_url = validate_https_url(env_str("FORGEJO_URL", "https://git.ferrive.me"), "Forgejo URL", allow_local)
        forgejo_repo = validate_repo_slug(env_str("FORGEJO_REPO", "sefr/pdfcraft"), "Forgejo repository")
        return cls(
            source_repo=source_repo,
            pr_number=int(raw_number),
            base_branch=validate_branch_name(env_str("BASE_BRANCH", "Main")),
            github_api_url=validate_https_url(env_str("GITHUB_API_URL", "https://api.github.com"), "GitHub API URL", allow_local),
            source_git_url=validate_https_url(env_str("SOURCE_GIT_URL", f"{server}/{source_repo}.git"), "source Git URL", allow_local),
            github_server_url=server,
            github_token=env_str("GITHUB_TOKEN"),
            forgejo_url=forgejo_url,
            forgejo_repo=forgejo_repo,
            forgejo_git_url=validate_https_url(env_str("FORGEJO_GIT_URL", f"{forgejo_url}/{forgejo_repo}.git"), "Forgejo Git URL", allow_local),
            forgejo_user=env_str("FORGEJO_USER"),
            forgejo_token=env_str("FORGEJO_TOKEN"),
            event_action=printable(env_str("EVENT_ACTION"), limit=40),
            event_head_sha=env_str("EVENT_HEAD_SHA"),
            dry_run=env_flag("DRY_RUN"),
            object_cache=env_str("SYNC_OBJECT_CACHE"),
        )


@dataclass
class Outcome:
    status: str
    exit_code: int
    detail: str = ""
    writes: list[str] = field(default_factory=list)


class Bridge:
    def __init__(self, cfg: Config, github: GitHubClient, forgejo: ForgejoClient, git: Git,
                 reporter: Reporter):
        self.cfg = cfg
        self.github = github
        self.forgejo = forgejo
        self.git = git
        self.rep = reporter
        self.n = cfg.pr_number
        self.branch = f"{BRANCH_PREFIX}{self.n}"
        self.branch_ref = f"refs/heads/{self.branch}"
        self.writes: list[str] = []

    # -- helpers --------------------------------------------------------------------------------

    def _done(self, status: str, exit_code: int, detail: str) -> Outcome:
        (self.rep.error if exit_code else self.rep.log)(f"{status}: {detail}")
        self.rep.add_summary(f"### Forgejo PR sync — GitHub #{self.n}\n\n**{status}** — {detail}")
        return Outcome(status, exit_code, detail, self.writes)

    def _write(self, description: str) -> bool:
        """Records a remote write; returns False in dry-run mode, where it is only printed."""
        self.writes.append(description)
        if self.cfg.dry_run:
            self.rep.log(f"[dry-run] would {description}")
            return False
        return True

    def _validate_source(self, pr: dict) -> tuple[str, str] | Outcome:
        """Returns (head SHA, head ref) for a valid same-repo PR into Main, or a final Outcome."""
        base = pr.get("base") or {}
        head = pr.get("head") or {}
        base_repo = (base.get("repo") or {}).get("full_name")
        head_repo = (head.get("repo") or {}).get("full_name")
        if base_repo != self.cfg.source_repo:
            return self._done("rejected", 1, f"base repository is {md_code(base_repo)}, expected {md_code(self.cfg.source_repo)}")
        if head_repo != self.cfg.source_repo:
            return self._done("rejected", 0, f"external fork {md_code(head_repo)} — only branches of {md_code(self.cfg.source_repo)} are bridged")
        if base.get("ref") != self.cfg.base_branch:
            return self._done("skipped", 0, f"targets {md_code(base.get('ref'))}, not {md_code(self.cfg.base_branch)}")
        try:
            sha = validate_sha(head.get("sha"), "head SHA")
            ref = validate_branch_name(head.get("ref"), "head branch")
        except ValidationError as exc:
            return self._done("rejected", 1, str(exc))
        if int(pr.get("number") or 0) != self.n:
            return self._done("rejected", 1, "GitHub returned a different pull request")
        return sha, ref

    def _find_forgejo_pr(self) -> dict | None | Outcome:
        """The Forgejo PR for this GitHub PR, None, or an Outcome on an ambiguous state."""
        ours, squatters = [], []
        for pr in self.forgejo.list_pulls("all"):
            head_ref = (pr.get("head") or {}).get("ref")
            first = str(pr.get("body") or "").split("\n", 1)[0].strip()
            match = MARKER_RE.match(first)
            if match and match.group("repo") == self.cfg.source_repo and int(match.group("num")) == self.n:
                if head_ref != self.branch:
                    return self._done("attention", 1, f"Forgejo PR #{pr.get('number')} carries this PR's marker but its branch is {md_code(head_ref)}")
                ours.append(pr)
            elif head_ref == self.branch:
                squatters.append(pr)
        if len(ours) > 1:
            numbers = ", ".join(f"#{p.get('number')}" for p in ours)
            return self._done("attention", 1, f"several Forgejo PRs ({numbers}) claim GitHub #{self.n}; nothing changed")
        if not ours and squatters:
            return self._done("attention", 1, f"branch {md_code(self.branch)} has a Forgejo PR the automation did not create (#{squatters[0].get('number')}); nothing changed")
        return ours[0] if ours else None

    @staticmethod
    def _synced_sha(fj_pr: dict) -> str | None:
        first = str(fj_pr.get("body") or "").split("\n", 1)[0].strip()
        match = MARKER_RE.match(first)
        if not match or match.group("sha") == "none":
            return None
        return match.group("sha")

    def _title(self, pr: dict) -> str:
        return f"[GH #{self.n}] {sanitize_title(pr.get('title'), limit=180)}"

    def _body(self, pr: dict, sha: str, head_ref: str) -> str:
        url = f"{self.cfg.github_server_url}/{self.cfg.source_repo}/pull/{self.n}"
        return "\n".join(
            [
                f"<!-- pdfcraft-sync:v1 source=github:{self.cfg.source_repo}#{self.n} synced_sha={sha} -->",
                f"Bridged from GitHub pull request {self.cfg.source_repo}#{self.n}: {url}",
                "",
                "| | |",
                "|---|---|",
                f"| Source branch | {md_code(head_ref)} |",
                f"| Source head | `{sha}` |",
                f"| Forgejo branch | `{self.branch}` |",
                "",
                "Managed by the GitHub→Forgejo PR bridge (`scripts/automation/sync_claude_pr.py`). "
                "Review and merge **here, in Forgejo**; never merge the GitHub pull request. "
                "New commits on the GitHub branch update this PR; rewritten history is reported, never forced.",
                "",
                "### Original description (from GitHub, untrusted)",
                "",
                quote_untrusted(pr.get("body")),
            ]
        )

    def _comment_once(self, fj_pr: dict | None, kind: str, key: str, text: str) -> None:
        if not fj_pr:
            return
        index = int(fj_pr.get("number") or 0)
        marker = EVENT_MARKER.format(kind=kind, key=key)
        if self._write(f"comment on Forgejo PR #{index} ({kind})"):
            self.forgejo.comment_once(index, marker, text)

    # -- main flow --------------------------------------------------------------------------------

    def run(self) -> Outcome:
        if self.cfg.event_action:
            self.rep.log(f"event: {self.cfg.event_action} for GitHub #{self.n}; state is re-read from the API")
        pr = self.github.get_pull(self.cfg.source_repo, self.n)
        validated = self._validate_source(pr)
        if isinstance(validated, Outcome):
            return validated
        sha, head_ref = validated
        if self.cfg.event_head_sha and self.cfg.event_head_sha != sha:
            self.rep.log(f"event head {printable(self.cfg.event_head_sha, 40)} is stale; GitHub's current head {sha} is used")

        fj_pr = self._find_forgejo_pr()
        if isinstance(fj_pr, Outcome):
            return fj_pr

        # 1. Closure and merge policy come first: reachability from Forgejo Main never makes a
        #    GitHub-side merge count as authorized.
        if pr.get("state") == "closed":
            return self._handle_closed(pr, sha, fj_pr)
        if pr.get("state") != "open":
            return self._done("attention", 1, f"unexpected GitHub PR state {md_code(pr.get('state'))}")

        # 2. Fetch the head as data and check it is the commit the API names.
        fetched = self.git.fetch(self.cfg.source_git_url, f"refs/pull/{self.n}/head", "refs/sync/source")
        if fetched != sha:
            return self._done("superseded", 0, f"fetched {fetched[:12]} but GitHub names {sha[:12]}; a newer event follows")
        main_sha = self.git.fetch(self.cfg.forgejo_git_url, f"refs/heads/{self.cfg.base_branch}", "refs/sync/main")

        # 3. Forgejo PR state.
        if fj_pr is not None:
            if fj_pr.get("merged"):
                if sha == self._synced_sha(fj_pr) or self.git.is_ancestor(sha, main_sha):
                    return self._done("up-to-date", 0, f"Forgejo PR #{fj_pr.get('number')} is merged and contains {sha[:12]}")
                self._comment_once(fj_pr, "after-merge", sha,
                                   f"GitHub #{self.n} gained commit `{sha}` after this PR was merged. "
                                   "Policy: completed work is never reopened or duplicated automatically. Open a new PR for the follow-up.")
                return self._done("attention", 1, f"Forgejo PR #{fj_pr.get('number')} is already merged; new commit {sha[:12]} needs a new PR (policy)")
            if fj_pr.get("state") == "closed":
                self._comment_once(fj_pr, "after-close", sha,
                                   f"GitHub #{self.n} is at `{sha}`, but this Forgejo PR was closed here. It is left closed.")
                self.rep.warning(f"Forgejo PR #{fj_pr.get('number')} was closed in Forgejo; not updated")
                return self._done("respected-close", 0, f"Forgejo PR #{fj_pr.get('number')} was closed in Forgejo and is left closed")
        elif self.git.is_ancestor(sha, main_sha):
            # 4. Already landed (open GitHub PR only — closures were handled above).
            return self._done("already-in-main", 0, f"{sha[:12]} is already in Forgejo {self.cfg.base_branch}; no Forgejo PR created")

        # 5. Destination branch.
        tip = self.git.ls_remote(self.cfg.forgejo_git_url, self.branch_ref)
        if tip is not None and tip != sha:
            self.git.fetch(self.cfg.forgejo_git_url, self.branch_ref, "refs/sync/dest")
            if not self.git.is_ancestor(tip, sha):
                kind = "non-fast-forward" if fj_pr else "collision"
                self._comment_once(fj_pr, "non-ff", sha,
                                   f"GitHub #{self.n} moved to `{sha}`, which does not descend from this branch's tip `{tip}` "
                                   "(history was rewritten). Nothing was pushed. Rewrite nothing here: either push the GitHub "
                                   "branch so it descends from the Forgejo tip, or close this PR and let a new one be opened.")
                return self._done(kind, 1, f"Forgejo {md_code(self.branch)} is at {tip[:12]}, which {sha[:12]} does not descend from; nothing pushed")
        elif tip is None and fj_pr is not None:
            return self._done("attention", 1, f"Forgejo PR #{fj_pr.get('number')} is open but {md_code(self.branch)} is gone; not recreated")

        # 6. Revalidate right before writing.
        if tip != sha:
            latest = self.github.get_pull(self.cfg.source_repo, self.n)
            if (latest.get("head") or {}).get("sha") != sha or latest.get("state") != "open":
                return self._done("superseded", 0, "the GitHub PR changed while syncing; a newer event follows")
            if fj_pr is not None:
                current = self.forgejo.get_pull(int(fj_pr.get("number") or 0))
                if current.get("merged") or current.get("state") != "open":
                    return self._done("attention", 1, f"Forgejo PR #{fj_pr.get('number')} was merged or closed while syncing; nothing pushed")
            if self._write(f"push {sha[:12]} to Forgejo {self.branch} (no force)"):
                try:
                    self.git.push(self.cfg.forgejo_git_url, sha, self.branch_ref)
                except GitError as exc:
                    now = self.git.ls_remote(self.cfg.forgejo_git_url, self.branch_ref)
                    return self._done("push-rejected", 1, f"Forgejo refused the push (tip now {(now or 'none')[:12]}): {exc}")
                now = self.git.ls_remote(self.cfg.forgejo_git_url, self.branch_ref)
                if now != sha:
                    return self._done("attention", 1, f"after the push Forgejo {md_code(self.branch)} is at {(now or 'none')[:12]}, not {sha[:12]}")

        # 7. Pull request metadata.
        title, body = self._title(pr), self._body(pr, sha, head_ref)
        if fj_pr is None:
            if not self._write(f"create Forgejo PR {self.branch} -> {self.cfg.base_branch}"):
                return self._done("dry-run", 0, "planned writes listed above")
            try:
                fj_pr = self.forgejo.create_pull(head=self.branch, base=self.cfg.base_branch, title=title, body=body)
            except ConflictError:
                adopted = self._find_forgejo_pr()
                if isinstance(adopted, Outcome):
                    return adopted
                if adopted is None:
                    raise
                fj_pr = adopted
                self.forgejo.edit_pull(int(fj_pr.get("number") or 0), title=title, body=body)
            action = "created"
        else:
            previous = self._synced_sha(fj_pr)
            action = "unchanged"
            if fj_pr.get("title") != title or fj_pr.get("body") != body:
                if self._write(f"update Forgejo PR #{fj_pr.get('number')} title/body"):
                    self.forgejo.edit_pull(int(fj_pr.get("number") or 0), title=title, body=body)
                action = "updated"
            if previous != sha:
                self._comment_once(fj_pr, "updated", sha, f"Updated to GitHub head `{sha}`.")
                action = "updated"

        if self.cfg.dry_run:
            return self._done("dry-run", 0, "planned writes listed above")

        # 8. Verify what Forgejo now shows.
        index = int(fj_pr.get("number") or 0)
        check = self.forgejo.get_pull(index)
        head_sha = (check.get("head") or {}).get("sha")
        base_ref = (check.get("base") or {}).get("ref")
        if head_sha != sha or base_ref != self.cfg.base_branch:
            return self._done("attention", 1, f"Forgejo PR #{index} shows head {printable(head_sha, 12)} into {md_code(base_ref)}, expected {sha[:12]} into {md_code(self.cfg.base_branch)}")
        url = check.get("html_url") or ""
        return self._done(action, 0, f"Forgejo PR #{index} ({printable(url, 200)}) at {sha[:12]}")

    def _handle_closed(self, pr: dict, sha: str, fj_pr: dict | None) -> Outcome:
        merged = bool(pr.get("merged") or pr.get("merged_at"))
        if not merged:
            self._comment_once(fj_pr, "github-closed", sha,
                               f"GitHub #{self.n} was closed without merging at `{sha}`. "
                               "This Forgejo PR and its branch are left as they are for review here.")
            where = f"Forgejo PR #{fj_pr.get('number')} left open" if fj_pr else "no Forgejo PR exists"
            return self._done("github-closed", 0, f"GitHub #{self.n} closed unmerged; {where}")
        if fj_pr is not None and fj_pr.get("merged"):
            return self._done("landed-via-mirror", 0, f"GitHub #{self.n} shows merged because Forgejo PR #{fj_pr.get('number')} was merged and mirrored")
        merge_sha = pr.get("merge_commit_sha") or "unknown"
        self._comment_once(fj_pr, "policy-violation", str(merge_sha)[:40],
                           f"**Policy violation:** GitHub #{self.n} was merged on GitHub (merge commit `{printable(merge_sha, 40)}`) "
                           "without this Forgejo PR being merged. Forgejo is authoritative: nothing was changed here. "
                           "Check GitHub `Main` against Forgejo `Main` before the mirror runs again.")
        where = f"Forgejo PR #{fj_pr.get('number')} is not merged" if fj_pr else "no Forgejo PR exists"
        return self._done("policy-violation", 1, f"GitHub #{self.n} was merged on GitHub but {where}; Forgejo left unchanged")


def main() -> int:
    install_redacting_excepthook()
    reporter: Reporter | None = None
    git: Git | None = None
    try:
        cfg = Config.from_env()
        REDACTOR.add_credential(cfg.forgejo_user, cfg.forgejo_token)
        REDACTOR.add_credential(None, cfg.github_token)
        reporter = Reporter(REDACTOR)
        if not cfg.forgejo_token and not cfg.dry_run:
            raise SyncError("FORGEJO_TOKEN is not set")
        auth = GitAuth(cfg.forgejo_url, cfg.forgejo_user or "pdfcraft-sync", cfg.forgejo_token) if cfg.forgejo_token else None
        git = Git.temporary(auth=auth)
        if cfg.object_cache:
            git.add_alternate(cfg.object_cache)
        github = GitHubClient(cfg.github_api_url, cfg.github_token or None)
        forgejo = ForgejoClient(cfg.forgejo_url, cfg.forgejo_repo, cfg.forgejo_token or None)
        outcome = Bridge(cfg, github, forgejo, git, reporter).run()
        return outcome.exit_code
    except (SyncError, ApiError) as exc:
        reporter = reporter or Reporter(REDACTOR)
        reporter.error(f"sync failed: {exc}")
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
