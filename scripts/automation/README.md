# GitHub → Forgejo synchronization (fork automation)

Status: bootstrapped into `Main` (Forgejo PR #1). The bot token exists and its acceptance
test is in progress; the workflows are not yet relied on. Cloudflare Access service-token
authentication added 2026-10-27. Last updated 2026-10-27.

This fork (`sefr-14/pdfcraft` on GitHub) is integrated on Forgejo (`sefr/pdfcraft` on
`git.ferrive.me`). Forgejo `Main` is authoritative and push-mirrors `Main` to GitHub. Claude Code
Web works through GitHub branches and pull requests. Two GitHub Actions workflows carry that
work to Forgejo, and neither ever merges, force-pushes or writes to `Main`:

| Workflow | Script | What it does |
|---|---|---|
| `.github/workflows/forgejo-pr-sync.yml` | `sync_claude_pr.py` | Bridges each same-repo GitHub PR into `Main` to a Forgejo branch `github-pr/<N>` and a Forgejo PR |
| `.github/workflows/forgejo-upstream-sync.yml` | `sync_upstream.py` | Every day, trial-merges `storytold/pdfcraft` `main` into an unmerged review PR, or keeps one conflict issue up to date |
| `.github/workflows/automation-tests.yml` | `tests/` | Runs these tests on PRs that touch them; no secrets |

Helpers: `common.py` (validation, the push guard, the Git runner, redaction, reporting),
`forgejo_client.py` / `github_client.py` (standard-library HTTP clients),
`forgejo_api_check.py` (read-only API compatibility check) and `forgejo_acceptance.py`
(the bot permission acceptance test). Only the Python standard library and Git are used.
The scripts are CI tooling, like the Python already inline in `ci.yml`; CLAUDE.md's
"Rust only" rule covers the product and its build.

Run the tests with `python3 -I -m unittest discover -s scripts/automation/tests -v`. They use
temporary Git repositories and in-process fake Forgejo and GitHub servers on 127.0.0.1, and
never reach either real service.

## Security model

- **Event:** the PR bridge uses `pull_request_target`. Since December 2025 GitHub always runs
  that event's workflow file from the default branch. A PR therefore cannot change the code that
  runs with the Forgejo secret, as it could under `pull_request` (which runs the PR's own copy
  of a workflow, with secrets, for same-repository branches).
- **No PR code runs.** The job checks out `Main` only (`ref: Main`, `persist-credentials: false`).
  The PR's commits are fetched as Git objects (`refs/pull/<N>/head`, with `fetch.fsckObjects`) and
  pushed on to Forgejo as data. Nothing from the PR is checked out, built or executed.
- **Untrusted text** (titles, bodies, branch names) never reaches a shell.
  - Only the PR number and head SHA pass through `${{ }}`, into `env:`. The script re-reads the
    PR from the API.
  - Branch names and SHAs are validated against allowlists before any Git call. Git runs with
    list arguments and `--end-of-options`.
  - Bodies are quoted, with HTML comments neutralised, so a forged automation marker is inert.
  - Workflow commands in the log are switched off (`::stop-commands::`) except around the
    script's own annotations.
- **Push guard** (`common.check_push_destination`): the only destinations are
  `refs/heads/github-pr/<N>`, `refs/heads/upstream-sync/main` and `upstream-sync/main-v<N>`.
  - `Main` (any case), `+` refspecs, deletes and mirrors are refused before Git starts.
  - Pushes are plain `<sha>:<ref>`, so the server's fast-forward check always applies.
- **Credentials.**
  - The Forgejo token reaches Git only through `GIT_CONFIG_*` environment variables, as an
    `http.<forgejo>/.extraHeader` scoped to the Forgejo URL. It never appears in argv, a URL, a
    remote or a config file. The Cloudflare Access service token travels the same way (see
    [Cloudflare Access](#cloudflare-access)).
  - Requests carrying credentials never follow a redirect: `http.<forgejo>/.followRedirects=false`
    for Git, and the Forgejo API client refuses 3xx answers. Otherwise urllib would replay
    `Authorization` and curl would replay the Cloudflare headers to whatever host a redirect names.
  - `GIT_TRACE*`, `GIT_CURL_VERBOSE` and `GIT_ASKPASS` are removed from Git's environment.
  - Every log line, annotation, exception message and job-summary line passes through
    `Redactor`. It knows the token, `base64(user:token)`, `user:token`, the Cloudflare client
    id and secret, Authorization and `CF-Access-Client-*` headers, URL credentials and GitHub
    token shapes.
  - Derived forms are registered with `::add-mask::`.
  - Uncaught errors print one redacted line, never a traceback.
- **Permissions.**
  - GitHub token: `contents: read` (plus `pull-requests: read` for the bridge). The scripts
    never write to GitHub; reports go to the job summary and to Forgejo.
  - Forgejo: a dedicated bot account with Write access to `sefr/pdfcraft` only. Forgejo token
    scopes are per user, so repository scope comes from membership.
- **Concurrency.** One bridge run per PR and one upstream run at a time, never cancelled half way
  (`cancel-in-progress: false`). State is re-read on every run, so a dropped intermediate event
  loses nothing.
- **HTTP retries.**
  - GET, PATCH and DELETE retry on network errors, 429 and 5xx, honouring `Retry-After`
    (capped at 60 s).
  - POST retries on 429 only, so a retry can never duplicate a PR, issue or comment.
  - 401 is reported as an invalid or expired token and not retried.

## Workflow A: PR bridge state model

State is never stored by the scripts. Each run derives it from three sources:

- the GitHub API;
- the Forgejo branch `github-pr/<N>`;
- the Forgejo PR whose first body line is
  `<!-- pdfcraft-sync:v1 source=github:sefr-14/pdfcraft#<N> synced_sha=<sha> -->`.

`synced_sha` is written only after a verified push.

**Steps, in order:**

1. **Validate the source.**
   - Base and head repository must both be `sefr-14/pdfcraft`. A fork PR is rejected; the job
     `if:` already filters forks out.
   - Base must be `Main`. Head SHA and branch must be valid.
   - The API's head SHA is authoritative; a stale event SHA is only logged.
2. **Find the Forgejo PR by its marker.**
   - Two PRs claiming one GitHub PR, or a PR on `github-pr/<N>` without the marker, are
     reported. Nothing is changed.
3. **Closed GitHub PR. Handled before anything else**, so reachability from Forgejo `Main`
   never authorizes a GitHub-side merge.
   - *Closed unmerged:* comment on the Forgejo PR. Nothing is closed or deleted.
   - *Merged, and the Forgejo PR is merged:* the mirror did it. Recorded.
   - *Merged otherwise:* **policy violation**. Comment and fail the run; Forgejo is not changed.
4. **Fetch the head.** Fetch `refs/pull/<N>/head` and require it to equal the API SHA.
   Otherwise the run is *superseded* and exits 0; a newer event follows.
5. **Forgejo PR merged.**
   - With new GitHub commits: report once. It is never reopened or duplicated; a follow-up
     needs a new PR.
   - Without new commits: up to date.
6. **Forgejo PR closed** (by a person, unmerged): respected and not updated.
7. **No Forgejo PR, and the head is already in Forgejo `Main`:** nothing is created.
8. **Destination branch.**
   - An existing tip must be an ancestor of the new head. An interrupted earlier run is adopted.
   - Anything else is reported, never forced: *non-fast-forward* when a Forgejo PR exists,
     *collision* when the branch belongs to someone else.
   - An open PR whose branch was deleted is reported, not recreated.
9. **Revalidate right before pushing.**
   - The GitHub head must be unchanged. Otherwise superseded, no push.
   - The Forgejo PR must still be open. Otherwise stop.
10. **Push.**
    - Plain push. If the server refuses it (a concurrent push), report it; the marker keeps the
      old SHA.
    - Afterwards, check that `ls-remote` shows the new SHA.
11. **Create or update the PR.**
    - Create `[GH #<N>] <title>`; a 409 "already exists" is adopted, never duplicated.
    - Or update its title and body, with one "updated" comment per new SHA.
12. **Verify** that the Forgejo PR shows the head SHA, into `Main`.

## Workflow B: upstream branch lifecycle

Trial merges run with `git merge-tree --write-tree` in a throwaway bare repository, so no
working tree and no conflict marker ever exists.

**Merge results:** Git's exit status and structured output decide.

| `merge-tree` result | Treated as | Consequence |
|---|---|---|
| exit 0 with a tree id and no conflicted paths | **clean** | Commit with `commit-tree` (trailer `Pdfcraft-Sync: upstream-merge`) and validate it |
| exit 1 with a parsable result listing conflicted paths | **conflict** | Normal conflict tracking |
| exit 1 with no result (for example an unknown object), any other exit code, or unparsable output | **execution failure** | Run fails with redacted diagnostics; the tracking issue is not touched |

**Validating a clean merge:**

- the tree id names a tree;
- the commit's parents are exactly the two sides;
- the commit descends from both sides, from Forgejo `Main` and from the branch's previous tip;
- the result changes some file against `Main`.

Text that merely looks like a conflict marker in a legitimate file is not a conflict.

**Sync state** comes from Forgejo:

- PRs whose first body line is
  `<!-- pdfcraft-upstream-sync:v1 branch=<b> upstream=<U> main=<F> head=<H> -->`;
- the branch tips;
- one open issue whose body starts with `<!-- pdfcraft-upstream-sync-conflict:v1 upstream=<U> main=<F> -->`.

**Active branch:**

- the branch of the one open sync PR;
- otherwise `upstream-sync/main-v<branch_version>` when the workflow is run by hand with
  `branch_version`;
- otherwise the branch of the newest sync PR, or `upstream-sync/main`.

Two open sync PRs are reported and neither is changed.

| State of the integration branch | What a run does |
|---|---|
| Upstream already in `Main` | Nothing. A comment on an open sync PR says it may be redundant (a person closes it). The tracking issue is resolved. |
| Absent (never made, or deleted after a merge) | Build from `Main`, create it with a plain push, open a new PR |
| Open PR, only automation commits | Merge `Main`, then upstream, on top of the current tip. Always fast-forward. Update the PR markers; one comment per new (upstream, `Main`) pair. |
| Open PR, nothing new | No-op (repeat runs post nothing) |
| Open PR, a person has committed to it | **Paused:** no push. One comment per new pair, with the exact `git merge` commands. |
| PR merged (branch ⊂ `Main`) | When upstream moves again, build from `Main` (a fast-forward of the old branch) and open a **new** PR |
| PR closed unmerged | **Frozen:** never reopened or pushed. The tracking issue proposes `branch_version = <next>`. Scheduled runs wait. |
| Branch exists with someone else's commits and no sync PR | Reported as a collision; nothing changed |
| Push refused (branch moved meanwhile) | Reported as "cannot fast-forward", with a proposed `branch_version`; nothing forced |
| Upstream history rewritten (recorded upstream not an ancestor) | Reported; nothing updated |
| Conflict | No commit, no push. One tracking issue: created once, body replaced with the latest report, one comment only when (upstream, `Main`) changes. Closed with a comment by the next clean run. |

**The review PR** lists:

- the upstream, `Main` and merge-base SHAs;
- the fork-only and upstream-only commit counts;
- the upstream log;
- changed files in sensitive areas;
- a checklist of the fork's regression-sensitive features:
  - the six-step Overlay wizard and alignment;
  - Thin Lines;
  - distance, perimeter and area measurements;
  - persistent measurement annotations;
  - rulers and guides;
  - PDF rendering, resources and optional-content layers;
  - Windows portable builds;
  - English and Spanish localization.

A clean merge is not proof of compatibility.

## Cloudflare Access

`git.ferrive.me` sits behind a Cloudflare Access application. Requests from outside its bypass
rule (GitHub-hosted runners included) are answered by Access, not Forgejo: an HTTP 302 to the
`*.cloudflareaccess.com` login page. That is how the first bridge run failed (GitHub PR
sefr-14/pdfcraft#19): the client followed the 302 and reported the HTML login page as
`Forgejo GET /repos/sefr/pdfcraft/pulls: invalid JSON`. The fix is machine authentication with an
Access **service token**, allowed by the application's Service Auth policy for these workflows.

**Two credentials, two jobs.** Both are needed on every request to Forgejo:

| Secret (environment `forgejo-sync`) | Checked by | Purpose |
|---|---|---|
| `CF_ACCESS_CLIENT_ID`, `CF_ACCESS_CLIENT_SECRET` | Cloudflare's edge | Proves the request comes from this automation, so Access lets it through to Forgejo. Grants nothing inside Forgejo. |
| `FORGEJO_USER`, `FORGEJO_TOKEN` | Forgejo | Authorizes the bot `pdfcraft-sync` (Write on `sefr/pdfcraft` only, no admin) |

**Both API and Git traffic carry them.**
- API: `ForgejoClient` sends `Authorization: token …`, `CF-Access-Client-Id` and
  `CF-Access-Client-Secret`. The headers live on that client only; the GitHub client never has
  them.
- Git over HTTPS (fetch, `ls-remote` and push): `GitAuth` sets three `http.<forgejo>/.extraHeader`
  entries (`Authorization: Basic …` and the two Cloudflare headers) in `GIT_CONFIG_COUNT` /
  `GIT_CONFIG_KEY_n` / `GIT_CONFIG_VALUE_n`. `http.extraHeader` is multi-valued, so Git sends every
  entry, and command-scope configuration from the environment is never written to `.git/config` or
  a global file. The URL prefix limits the headers to Forgejo: GitHub and upstream fetches carry
  none. `tests/test_git_http_auth.py` checks this with real Git against a local smart-HTTP
  server.

**Redirects are refused** in every mode, with or without Cloudflare: a redirect means the request
never reached Forgejo. The run fails with `HTTP 302 redirect refused (Cloudflare Access or a proxy
intercepted the request; check CF_ACCESS_CLIENT_ID and CF_ACCESS_CLIENT_SECRET)`. The page body
and the redirect target are not reported.

**Configuration rules** (checked before any network or Git call):
- Both variables set: Cloudflare authentication is on.
- Neither set, or both empty or whitespace-only: behaviour as before, for hosts inside an Access
  bypass (the TrueNAS host running `forgejo_acceptance.py`) and for local use.
- Exactly one set: the run stops with `Cloudflare Access is half configured: CF_ACCESS_CLIENT_ID is
  set but CF_ACCESS_CLIENT_SECRET is not; set both or neither` (or the reverse).
- Surrounding whitespace is stripped. A value with control characters, inner spaces or non-ASCII
  is refused, naming the variable but never the value. The same check applies to `FORGEJO_TOKEN`.

**Other tools.**
- `forgejo_acceptance.py` reads the same two variables, optionally.
- `forgejo_api_check.py` reads the public swagger without credentials. It is meant to run from a
  host inside the Access bypass, and is not supported from GitHub-hosted runners.

**Deployment.**
1. Merge the change through one Forgejo PR into `Main`, then let the mirror carry it to GitHub.
2. Check that GitHub `Main` equals Forgejo `Main`, and that both workflows pass the
   `CF_ACCESS_*` secrets.
3. Reconcile a single PR by hand (Actions › forgejo pr sync › Run workflow, with its number).
   Check the job summary, `github-pr/<N>` and the Forgejo PR. Neither `Main` may move.
4. Run the upstream sync once by hand, separately and only with approval.

**Rollback.** Open a corrective Forgejo PR that reverts the change, and merge it. Never
force-push or reset `Main`. Meanwhile, the kill switches above still apply: disable the workflows,
or remove the `forgejo-sync` secrets. Do not disable Access or widen its bypass to work around a
failure.

## Assumptions not verifiable from the development environment

- **Forgejo 15 API.** `git.ferrive.me` was unreachable from the development container.
  - The client targets the v1 API. The user confirmed that `/api/v1/version` and
    `/swagger.v1.json` answer.
  - `forgejo_api_check.py` checks every operation used against the live swagger.
  - The fake follows Gitea/Forgejo behaviour as documented, with these unverified details:
    - a duplicate PR answers 409;
    - `merge_base` is present on a PR;
    - `head.sha` survives a closed PR whose branch was deleted;
    - `type=issues` excludes PRs from the issue list.
- **Bot permissions.** Unverified until `forgejo_acceptance.py` runs.
- **Token owner.** Not verified by any script. `GET /user` needs the `read:user` scope, which the
  repository-restricted bot token deliberately lacks, and no repository-scoped endpoint names the
  token's owner. Neither sync workflow needs that endpoint.
- **Main protection.** That the bot cannot push to `Main` is checked by an administrator, never
  by trying.
- **Event policy.** GitHub's per-workflow-file execution policies (confirmed available by the
  user) must be visible in this repository's settings.
- **Mirror.** The Forgejo push mirror force-pushes `Main` to GitHub. It is out of scope and not
  changed; GitHub `Main` protection must keep allowing it.
- **Merged-PR detection.** When the mirror lands a bridged PR's commits, GitHub marks the
  GitHub PR merged. Forgejo is expected to have marked its PR merged first.
- **Inactive repositories.** GitHub disables scheduled workflows after 60 days without
  repository activity.

## Activation

Every step that writes anything needs separate approval. In order:

1. **Read-only checks.**
   - `python3 -I scripts/automation/forgejo_api_check.py --url https://git.ferrive.me`: must
     report Forgejo 15.x and no `FAIL`.
   - As a Forgejo administrator, read `Main`'s branch protection: direct pushes blocked, PRs
     required, the bot not on the push or merge allowlists.
   - Confirm the push mirror is limited to `Main`.
   - On GitHub, confirm `Main`'s protection still lets the mirror push.
   - On GitHub, open Settings › Actions › **Policies**. Check for existing event policies and,
     in Insights or evaluate mode, for runs `pull_request_target` would block. Confirm that
     execution policies can target a single workflow file.
2. **Provision.**
   - Create the Forgejo user `pdfcraft-sync` and add it as a **Write** collaborator on
     `sefr/pdfcraft` only.
   - Create a token with `write:repository` and `write:issue`.
   - On GitHub, create the environment `forgejo-sync` with deployments allowed from `Main` only.
     Add the secrets `FORGEJO_TOKEN` and `FORGEJO_USER`, and optionally the variables
     `FORGEJO_URL` and `FORGEJO_REPO`. Where Cloudflare Access protects Forgejo, also add
     `CF_ACCESS_CLIENT_ID` and `CF_ACCESS_CLIENT_SECRET` ([Cloudflare Access](#cloudflare-access)).
3. **Acceptance test** (from a trusted local checkout):
   `FORGEJO_USER=pdfcraft-sync FORGEJO_TOKEN=… python3 -I scripts/automation/forgejo_acceptance.py`,
   then the same with `--confirm-writes`.
   - The read-only pass checks the token's repository permissions (push, not admin), that branch
     protections are refused to it, and that `git ls-remote` finds `Main` with the token sent.
     It does not check which account owns the token: that needs `read:user`, which the token is
     not meant to have. Confirm the owner in Forgejo's token settings when you create the token.
   - The write pass pushes one harmless commit (adding
     `scripts/automation/acceptance/ACCEPTANCE.txt`) to `github-pr/0`.
   - It then opens, comments on, edits and closes a PR (never merged), and opens, comments on
     and closes an issue.
   - Delete `github-pr/0` by hand afterwards.
4. **Event policy.** Allow `pull_request_target` **for `.github/workflows/forgejo-pr-sync.yml`
   only**: evaluate mode first, then active, before GitHub's default block on 2 December 2026.
   - If per-file targeting turns out to be unavailable, stop and decide. Do not fall back to a
     repository-wide permission.
   - `tests/test_workflows.py` keeps every other workflow off `pull_request_target`.
5. **Bootstrap** the implementation (below).
6. **First live runs.**
   - Open a trivial GitHub test PR into `Main`. Check `github-pr/<N>` and the Forgejo PR, then
     push a second commit and check the update.
   - Run "forgejo upstream sync" once by hand and review its PR or issue.
   - Leave the daily schedule on.

**Kill switches:** disable either workflow in the Actions tab, or remove the `forgejo-sync`
secrets (runs then fail harmlessly).

## Bootstrap (first PR)

The workflows do not exist on GitHub `Main` until this implementation is merged in Forgejo and
mirrored, so the bridge cannot carry its own PR. The bootstrap therefore uses **no GitHub PR**
and **exactly one Forgejo PR**:

1. Push the implementation branch to GitHub (`claude/admiring-planck-6pt4ju`).
   **Do not open a GitHub PR for it.**
2. With your own Forgejo credentials, from any trusted clone:

   ```sh
   git fetch https://github.com/sefr-14/pdfcraft.git claude/admiring-planck-6pt4ju
   git rev-parse FETCH_HEAD        # must equal the SHA reported for the branch
   git push https://git.ferrive.me/sefr/pdfcraft.git FETCH_HEAD:refs/heads/bootstrap/forgejo-sync
   ```

   `bootstrap/*` is outside the bridge's `github-pr/*` namespace, so no later run adopts or
   duplicates it.
3. In Forgejo, open one PR from `bootstrap/forgejo-sync` into `Main`, review it and merge it
   there. The mirror then brings `Main`, and the workflows, to GitHub.
4. Check that GitHub `Main` equals Forgejo `Main`, and that the three workflows appear under
   Actions.

If a GitHub PR for that branch is opened anyway, the bridge creates nothing once the commits
are in Forgejo `Main`. If it is merged on GitHub, the bridge reports a policy violation; it is
never treated as authorized.

**To reverse:**

- Before the merge: close the Forgejo PR, then delete `bootstrap/forgejo-sync` and the GitHub
  branch by hand.
- After the merge: revert through one Forgejo PR, or use the kill switches above.
