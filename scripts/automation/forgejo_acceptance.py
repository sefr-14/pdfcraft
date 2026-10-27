#!/usr/bin/env python3
"""Controlled acceptance test of the Forgejo bot account's permissions.

    FORGEJO_USER=pdfcraft-sync FORGEJO_TOKEN=... python3 -I scripts/automation/forgejo_acceptance.py
    ... --confirm-writes        # only after the read-only pass, and with approval

Read-only pass (default):
  1. The repository reports ``permissions.push = true`` and ``admin = false`` for the token.
  2. Branch protections cannot be read (403/404): the bot is not an administrator.
  3. ``git ls-remote``, sending the token, finds ``Main``; its SHA is printed.

The token's account identity is not verified. ``GET /user`` needs the ``read:user`` scope, which
the bot's repository-restricted token deliberately lacks, and no repository-scoped endpoint
names the token's owner. ``FORGEJO_USER`` is only used as the user name in Git's HTTP
authentication and in the test commit's e-mail address.

Write pass (``--confirm-writes``), each step in order, stopping at the first failure:
  4. ``PATCH`` the repository with an empty body: must be refused (403). With an admin token it
     would be a no-op, so it is harmless either way.
  5. Make one harmless commit on top of Main that adds
     ``scripts/automation/acceptance/ACCEPTANCE.txt`` (a timestamp), in a throwaway repository,
     and push it without force to ``github-pr/0``. Refuses to run if that branch exists.
  6. Open a PR from ``github-pr/0`` into Main titled "do not merge", comment on it, edit its title
     and close it. It is never merged.
  7. Open an issue, comment on it and close it.

It never pushes to Main and never deletes anything: delete ``github-pr/0`` by hand afterwards.
Whether the bot can push to Main is checked by an administrator reading Main's branch
protection, never by trying.
"""

from __future__ import annotations

import argparse
import datetime
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from common import (  # noqa: E402
    REDACTOR,
    Git,
    GitAuth,
    SyncError,
    env_str,
    git_identity,
    printable,
    validate_branch_name,
    validate_https_url,
    validate_repo_slug,
    validate_sha,
)
from forgejo_client import ApiError, ForbiddenError, ForgejoClient, NotFoundError  # noqa: E402

TEST_BRANCH = "github-pr/0"
TEST_FILE = "scripts/automation/acceptance/ACCEPTANCE.txt"


class Acceptance:
    def __init__(self, client: ForgejoClient, git: Git, git_url: str, base: str, out=None):
        self.client = client
        self.git = git
        self.git_url = git_url
        self.base = base
        self.out = out if out is not None else sys.stdout
        self.created: list[str] = []

    def say(self, status: str, text: str) -> None:
        self.out.write(REDACTOR.redact(f"{status:5} {text}") + "\n")

    def read_checks(self) -> bool:
        ok = True
        self.say("note", "token owner not verified: GET /user needs read:user, which this token is not meant to have")
        perms = self.client.get_repo().get("permissions") or {}
        if perms.get("push") is True and perms.get("admin") is False:
            self.say("ok", "repository permissions: push yes, admin no")
        else:
            self.say("FAIL", f"repository permissions {printable(perms)}: need push=true and admin=false")
            ok = False
        try:
            self.client.list_branch_protections()
            self.say("FAIL", "the bot can read branch protections: it has administrator rights")
            ok = False
        except (ForbiddenError, NotFoundError):
            self.say("ok", "branch protections are not readable by the bot (not an administrator)")
        main_sha = self.git.ls_remote(self.git_url, f"refs/heads/{self.base}")
        if main_sha:
            self.say("ok", f"{self.base} is at {main_sha}")
        else:
            self.say("FAIL", f"{self.base} not found")
            ok = False
        return ok

    def write_checks(self) -> bool:
        try:
            self.client.edit_repo({})
            self.say("FAIL", "PATCH on the repository settings was accepted: the token can change settings")
            return False
        except ForbiddenError:
            self.say("ok", "repository settings cannot be changed (403)")

        if self.git.ls_remote(self.git_url, f"refs/heads/{TEST_BRANCH}") is not None:
            self.say("FAIL", f"{TEST_BRANCH} already exists; delete it by hand and run again")
            return False
        main_sha = self.git.fetch(self.git_url, f"refs/heads/{self.base}", "refs/sync/main")
        commit = self._harmless_commit(main_sha)
        self.git.push(self.git_url, commit, f"refs/heads/{TEST_BRANCH}")
        if self.git.ls_remote(self.git_url, f"refs/heads/{TEST_BRANCH}") != commit:
            self.say("FAIL", f"{TEST_BRANCH} does not show the pushed commit")
            return False
        self.created.append(f"branch {TEST_BRANCH} at {commit}")
        self.say("ok", f"pushed {commit} (adds {TEST_FILE}) to {TEST_BRANCH} without force")

        pr = self.client.create_pull(
            head=TEST_BRANCH, base=self.base,
            title="[acceptance] bot permission test — do not merge",
            body="Created by `scripts/automation/forgejo_acceptance.py` to check the bot's permissions. "
                 f"It adds `{TEST_FILE}` only. It is closed by the test and must never be merged; "
                 f"delete `{TEST_BRANCH}` afterwards.",
        )
        index = int(pr.get("number") or 0)
        self.created.append(f"pull request #{index}")
        self.say("ok", f"opened PR #{index}")
        self.client.create_comment(index, "Acceptance test comment.")
        self.client.edit_pull(index, title="[acceptance] bot permission test — do not merge (edited)")
        self.client.edit_pull(index, state="closed")
        check = self.client.get_pull(index)
        if check.get("merged") or check.get("state") != "closed":
            self.say("FAIL", f"PR #{index} is not closed and unmerged")
            return False
        if (check.get("head") or {}).get("sha") != commit:
            self.say("FAIL", f"PR #{index} does not show the pushed commit")
            return False
        self.say("ok", f"commented on, edited and closed PR #{index} (unmerged)")

        issue = self.client.create_issue(title="[acceptance] bot permission test",
                                         body="Created and closed by `scripts/automation/forgejo_acceptance.py`.")
        issue_index = int(issue.get("number") or 0)
        self.created.append(f"issue #{issue_index}")
        self.client.create_comment(issue_index, "Acceptance test comment.")
        self.client.edit_issue(issue_index, state="closed")
        self.say("ok", f"opened, commented on and closed issue #{issue_index}")
        if self.git.ls_remote(self.git_url, f"refs/heads/{self.base}") != main_sha:
            self.say("FAIL", f"{self.base} moved during the test")
            return False
        self.say("ok", f"{self.base} unchanged at {main_sha}")
        return True

    def _harmless_commit(self, main_sha: str) -> str:
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        content = f"PdfCraft sync bot acceptance test, {stamp}. Safe to delete with branch {TEST_BRANCH}.\n"
        blob = self.git.run(["hash-object", "-w", "--stdin"], input_bytes=content.encode()).decode().strip()
        index_file = os.path.join(tempfile.mkdtemp(prefix="pdfcraft-acceptance-"), "index")
        saved = dict(self.git.identity)
        self.git.identity = {**saved, "GIT_INDEX_FILE": index_file}
        try:
            self.git.run(["read-tree", validate_sha(main_sha)])
            self.git.run(["update-index", "--add", "--cacheinfo", f"100644,{validate_sha(blob, 'blob id')},{TEST_FILE}"])
            tree = self.git.run(["write-tree"]).decode().strip()
        finally:
            self.git.identity = saved
        message = b"Acceptance test: harmless file for the bot permission check (do not merge)\n"
        return validate_sha(self.git.run(["commit-tree", validate_sha(tree, "tree id"), "-p", main_sha, "-F", "-"],
                                         input_bytes=message).decode().strip())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Forgejo bot permission acceptance test")
    parser.add_argument("--confirm-writes", action="store_true", help="also run the write checks")
    parser.add_argument("--allow-local", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    git = None
    try:
        url = validate_https_url(env_str("FORGEJO_URL", "https://git.ferrive.me"), "Forgejo URL", args.allow_local)
        repo = validate_repo_slug(env_str("FORGEJO_REPO", "sefr/pdfcraft"))
        user = validate_branch_name(env_str("FORGEJO_USER", "pdfcraft-sync"), "user name")
        token = env_str("FORGEJO_TOKEN", required=True)
        REDACTOR.add_credential(user, token)
        git_url = validate_https_url(env_str("FORGEJO_GIT_URL", f"{url}/{repo}.git"), "Forgejo Git URL", args.allow_local)
        git = Git.temporary(auth=GitAuth(url, user, token),
                            identity=git_identity("PdfCraft sync acceptance test", f"{user}@noreply.invalid"))
        test = Acceptance(ForgejoClient(url, repo, token), git, git_url, env_str("BASE_BRANCH", "Main"))
        ok = test.read_checks()
        if ok and args.confirm_writes:
            ok = test.write_checks()
        elif not args.confirm_writes:
            test.say("note", "read-only pass only; run with --confirm-writes for the write checks")
        if test.created:
            test.say("note", "created: " + "; ".join(test.created) + f". Delete {TEST_BRANCH} by hand.")
        return 0 if ok else 1
    except (SyncError, ApiError) as exc:
        sys.stdout.write(REDACTOR.redact(f"FAIL  {exc}") + "\n")
        return 1
    finally:
        if git is not None:
            git.cleanup()


if __name__ == "__main__":
    sys.exit(main())
