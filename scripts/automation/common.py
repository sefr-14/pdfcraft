"""Shared helpers for the GitHub -> Forgejo synchronization scripts.

Everything here treats its inputs as untrusted: ref names, SHAs, titles and bodies come from
GitHub pull requests or from Forgejo, and every one of them can be hostile. The rules:

- Git runs as a list of arguments, never through a shell, with credentials passed only through
  the environment (``GIT_CONFIG_*``), never on the command line, in a URL or in a remote.
- Every push goes through :func:`check_push_destination`, which only accepts the automation's own
  branch namespaces. ``Main`` and force pushes are refused before Git is even started.
- Every line that reaches a log, an annotation, an exception message or the job summary passes
  through :class:`Redactor` first.

Only the Python standard library is used.
"""

from __future__ import annotations

import base64
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from typing import Iterable, Mapping, Sequence

SHA_RE = re.compile(r"^[0-9a-f]{40}$")

# Branches the automation may push to. Nothing else, and in particular never Main.
#   github-pr/<N>               the Claude PR bridge (github-pr/0 is the bot acceptance test)
#   upstream-sync/main[-v<N>]   the upstream integration branch and its versioned successors
PUSH_DESTINATION_RE = re.compile(
    r"^refs/heads/(?:github-pr/(?:0|[1-9][0-9]{0,9})|upstream-sync/main(?:-v[1-9][0-9]{0,3})?)$"
)

# A conservative allowlist for branch names read from GitHub or Forgejo. Git accepts far more,
# but nothing this automation handles needs it.
BRANCH_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/+-]{0,199}$")

# Environment variables that make Git print requests, headers or credentials.
_GIT_ENV_DROP_PREFIXES = ("GIT_TRACE", "GIT_CURL_VERBOSE", "GIT_REDACT_COOKIES")
_GIT_ENV_DROP = {
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_EXEC_PATH",
    "GIT_NAMESPACE",
}

GIT_TIMEOUT_SECONDS = 900
MAX_CAPTURE_BYTES = 64 * 1024 * 1024


class SyncError(Exception):
    """A failure the run reports and stops on. The message is always redacted."""


class ValidationError(SyncError):
    """Untrusted input failed validation."""


class PushRefused(SyncError):
    """The push guard refused a destination before Git was started."""


class GitError(SyncError):
    """A Git subprocess failed. ``stderr`` is redacted."""

    def __init__(self, message: str, returncode: int, stdout: bytes = b"", stderr: str = ""):
        super().__init__(message)
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr


# ---------------------------------------------------------------------------------------------
# Redaction


class Redactor:
    """Replaces every registered secret, and the obvious credential shapes, with ``***``."""

    _PATTERNS = (
        # Authorization headers, whatever their scheme.
        re.compile(r"(?i)(authorization\s*[:=]\s*)(?:(?:token|bearer|basic)\s+)?[^\s\"',;]+"),
        # Credentials embedded in URLs: https://user:secret@host
        re.compile(r"(?i)(\b[a-z][a-z0-9+.-]*://[^/\s:@]+:)[^@\s/]+(@)"),
        # GitHub token shapes.
        re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b"),
    )

    def __init__(self) -> None:
        self._secrets: list[str] = []

    def add(self, value: str | None) -> None:
        if not value or len(value) < 4:
            # Too short to redact without destroying ordinary text; such a value is no secret.
            return
        if value not in self._secrets:
            self._secrets.append(value)
            self._secrets.sort(key=len, reverse=True)

    def add_credential(self, user: str | None, token: str | None) -> None:
        """Registers a token together with every form Git or HTTP may print it in."""
        self.add(token)
        if token:
            self.add(f"token {token}")
            self.add(f"Bearer {token}")
            if user:
                pair = f"{user}:{token}".encode("utf-8")
                self.add(base64.b64encode(pair).decode("ascii"))
                self.add(f"{user}:{token}")

    def secrets(self) -> Sequence[str]:
        return tuple(self._secrets)

    def redact(self, text: object) -> str:
        if isinstance(text, bytes):
            out = text.decode("utf-8", errors="replace")
        else:
            out = str(text)
        for value in self._secrets:
            out = out.replace(value, "***")
        out = self._PATTERNS[0].sub(lambda m: m.group(1) + "***", out)
        out = self._PATTERNS[1].sub(lambda m: m.group(1) + "***" + m.group(2), out)
        out = self._PATTERNS[2].sub("***", out)
        return out


REDACTOR = Redactor()


def install_redacting_excepthook(redactor: Redactor = REDACTOR) -> None:
    """Uncaught exceptions print one redacted line, never a traceback with locals."""

    def hook(exc_type, exc, _tb):  # pragma: no cover - exercised through main() wrappers
        sys.stderr.write(redactor.redact(f"{exc_type.__name__}: {exc}") + "\n")

    sys.excepthook = hook


# ---------------------------------------------------------------------------------------------
# Validation and sanitising


def validate_sha(value: object, what: str = "commit SHA") -> str:
    if not isinstance(value, str) or not SHA_RE.match(value):
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    return value


def validate_branch_name(value: object, what: str = "branch name") -> str:
    """Validates a branch name without asking Git, so it is safe before any subprocess runs."""
    if not isinstance(value, str) or not BRANCH_NAME_RE.match(value):
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    bad = (
        ".." in value
        or "//" in value
        or "@{" in value
        or value.endswith("/")
        or value.endswith(".")
        or value.endswith(".lock")
        or "/." in value
        or "/-" in value
        or value == "HEAD"
    )
    if bad:
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    return value


def check_push_destination(ref: str) -> str:
    """The push guard. Returns ``ref`` if the automation may push to it, raises otherwise."""
    if not isinstance(ref, str) or not PUSH_DESTINATION_RE.match(ref):
        raise PushRefused(f"push to {printable(ref)!r} refused: outside the automation's branches")
    if ref.lower() in ("refs/heads/main",):  # belt and braces; the regex already excludes it
        raise PushRefused("push to Main refused")
    return ref


_CONTROL_RE = re.compile(r"[\x00-\x08\x0b-\x1f\x7f​-‏‪-‮⁦-⁩]")


def printable(value: object, limit: int = 200) -> str:
    """One line, no control or bidi characters, capped. For logging untrusted values."""
    text = str(value)
    text = _CONTROL_RE.sub("?", text).replace("\n", " ").replace("\r", " ")
    if len(text) > limit:
        text = text[:limit] + "..."
    return text


def sanitize_title(value: object, limit: int = 200) -> str:
    text = value if isinstance(value, str) else ""
    text = _CONTROL_RE.sub(" ", text).replace("\n", " ").replace("\r", " ")
    text = " ".join(text.split())
    if len(text) > limit:
        text = text[: limit - 1].rstrip() + "…"
    return text or "(untitled)"


def neutralize_markers(text: str) -> str:
    """Keeps untrusted text from carrying an HTML comment, and so a forged automation marker."""
    return text.replace("<!--", "&lt;!--").replace("-->", "--&gt;")


def quote_untrusted(text: object, limit: int = 20000) -> str:
    """Renders an untrusted Markdown body as a quote block, markers neutralised, size capped."""
    body = text if isinstance(text, str) else ""
    body = _CONTROL_RE.sub("", body.replace("\r\n", "\n"))
    truncated = len(body) > limit
    if truncated:
        body = body[:limit]
    body = neutralize_markers(body)
    lines = body.split("\n") if body.strip() else ["(no description)"]
    quoted = "\n".join("> " + line for line in lines)
    if truncated:
        quoted += "\n>\n> *(truncated)*"
    return quoted


def md_code(value: object) -> str:
    """Untrusted text as inline Markdown code, safe for tables and summaries."""
    text = printable(value).replace("`", "'").replace("|", "\\|")
    return f"`{text}`"


# ---------------------------------------------------------------------------------------------
# Reporting


class Reporter:
    """Logs, annotations and the job summary, all redacted.

    On GitHub Actions, untrusted text in the log could otherwise smuggle in workflow commands
    (``::add-mask::``, ``::error::``…), so workflow-command processing is switched off with
    ``::stop-commands::`` for the whole run and switched on only around our own annotations.
    """

    def __init__(self, redactor: Redactor = REDACTOR, stream=None, actions: bool | None = None):
        self.redactor = redactor
        self.stream = stream if stream is not None else sys.stdout
        self.actions = (os.environ.get("GITHUB_ACTIONS") == "true") if actions is None else actions
        self.summary: list[str] = []
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self._stop_token = secrets.token_hex(16)
        if self.actions:
            # Mask every derived credential form (GitHub masks the raw secret only), then stop
            # command processing so that nothing printed later can issue a workflow command.
            for value in redactor.secrets():
                self._write(f"::add-mask::{value}")
            self._write(f"::stop-commands::{self._stop_token}")

    def _write(self, line: str) -> None:
        self.stream.write(line + "\n")
        self.stream.flush()

    def log(self, message: object) -> None:
        for line in self.redactor.redact(message).splitlines() or [""]:
            self._write(printable(line, limit=2000))

    def _annotate(self, level: str, message: str) -> None:
        text = self.redactor.redact(message)
        if self.actions:
            escaped = text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
            self._write(f"::{self._stop_token}::")
            self._write(f"::{level}::{escaped}")
            self._write(f"::stop-commands::{self._stop_token}")
        else:
            self._write(f"{level.upper()}: {printable(text, limit=2000)}")

    def error(self, message: str) -> None:
        self.errors.append(self.redactor.redact(message))
        self._annotate("error", message)

    def warning(self, message: str) -> None:
        self.warnings.append(self.redactor.redact(message))
        self._annotate("warning", message)

    def add_summary(self, markdown: str) -> None:
        self.summary.append(self.redactor.redact(markdown))

    def finish(self) -> None:
        """Writes the job summary (redacted) and re-enables workflow commands."""
        path = os.environ.get("GITHUB_STEP_SUMMARY")
        if path and self.summary:
            try:
                with open(path, "a", encoding="utf-8") as handle:
                    handle.write(self.redactor.redact("\n\n".join(self.summary)) + "\n")
            except OSError as exc:
                self._write(printable(f"could not write the job summary: {exc}"))
        if self.actions:
            self._write(f"::{self._stop_token}::")


# ---------------------------------------------------------------------------------------------
# Git


@dataclass
class GitAuth:
    """HTTP credentials for one URL prefix, handed to Git through the environment only."""

    url_prefix: str
    user: str
    token: str

    def env(self) -> dict[str, str]:
        pair = base64.b64encode(f"{self.user}:{self.token}".encode("utf-8")).decode("ascii")
        prefix = self.url_prefix.rstrip("/") + "/"
        return {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": f"http.{prefix}.extraHeader",
            "GIT_CONFIG_VALUE_0": f"Authorization: Basic {pair}",
        }


@dataclass
class Git:
    """A Git runner bound to one private bare repository."""

    git_dir: str
    redactor: Redactor = field(default_factory=lambda: REDACTOR)
    auth: GitAuth | None = None
    identity: Mapping[str, str] = field(default_factory=dict)
    git_bin: str = "git"

    @classmethod
    def temporary(cls, prefix: str = "pdfcraft-sync-", **kwargs) -> "Git":
        path = tempfile.mkdtemp(prefix=prefix)
        git = cls(git_dir=path, **kwargs)
        git.run(["init", "--quiet", "--bare", path], bare_dir=False)
        # The repository is private and throwaway: no hooks, no gc, no object guessing.
        for key, value in (
            ("gc.auto", "0"),
            ("core.hooksPath", os.devnull),
            ("fetch.fsckObjects", "true"),
            ("transfer.fsckObjects", "true"),
            ("protocol.allow", "never"),
            ("protocol.https.allow", "always"),
            ("protocol.file.allow", os.environ.get("SYNC_ALLOW_FILE_PROTOCOL", "never")),
        ):
            git.run(["config", key, value])
        return git

    def cleanup(self) -> None:
        shutil.rmtree(self.git_dir, ignore_errors=True)

    def add_alternate(self, objects_dir: str) -> None:
        """Borrows objects from a trusted local repository (the workflow's Main checkout)."""
        if not os.path.isdir(objects_dir):
            return
        info = os.path.join(self.git_dir, "objects", "info")
        os.makedirs(info, exist_ok=True)
        with open(os.path.join(info, "alternates"), "a", encoding="utf-8") as handle:
            handle.write(os.path.abspath(objects_dir) + "\n")

    def _env(self) -> dict[str, str]:
        env = {
            key: value
            for key, value in os.environ.items()
            if key not in _GIT_ENV_DROP
            and not key.startswith(_GIT_ENV_DROP_PREFIXES)
            and not key.startswith("GIT_CONFIG_KEY_")
            and not key.startswith("GIT_CONFIG_VALUE_")
        }
        env.update(
            {
                "GIT_TERMINAL_PROMPT": "0",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "LC_ALL": "C",
                "LANG": "C",
            }
        )
        env.update(self.identity)
        if self.auth is not None:
            env.update(self.auth.env())
        return env

    def run_raw(
        self,
        args: Sequence[str],
        *,
        input_bytes: bytes | None = None,
        bare_dir: bool = True,
        timeout: int = GIT_TIMEOUT_SECONDS,
    ) -> subprocess.CompletedProcess:
        """Runs Git and returns the completed process whatever its exit status."""
        for arg in args:
            if not isinstance(arg, str) or "\x00" in arg:
                raise ValidationError("invalid Git argument")
        argv = [self.git_bin]
        if bare_dir:
            argv.append(f"--git-dir={self.git_dir}")
        argv.extend(args)
        try:
            proc = subprocess.run(
                argv,
                input=input_bytes,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=self._env(),
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as exc:
            raise GitError(self._describe(args, f"timed out after {timeout}s"), -1) from None
        except OSError as exc:
            raise GitError(self._describe(args, f"could not start Git: {exc}"), -1) from None
        if len(proc.stdout) > MAX_CAPTURE_BYTES:
            raise GitError(self._describe(args, "produced too much output"), proc.returncode)
        return proc

    def run(self, args: Sequence[str], *, input_bytes: bytes | None = None, bare_dir: bool = True,
            timeout: int = GIT_TIMEOUT_SECONDS) -> bytes:
        proc = self.run_raw(args, input_bytes=input_bytes, bare_dir=bare_dir, timeout=timeout)
        if proc.returncode != 0:
            stderr = self.redactor.redact(proc.stderr)[-4000:]
            raise GitError(
                self._describe(args, f"exit {proc.returncode}: {stderr.strip()}"),
                proc.returncode,
                proc.stdout,
                stderr,
            )
        return proc.stdout

    def _describe(self, args: Sequence[str], what: str) -> str:
        # Only the subcommand and its options are named; URLs and refspecs could carry data.
        shown = [a for a in args[:3] if a.startswith("-") or a.isalpha()]
        return self.redactor.redact(f"git {' '.join(shown) or '?'} failed: {what}")

    # -- queries ------------------------------------------------------------------------------

    def rev_parse_commit(self, rev: str) -> str | None:
        proc = self.run_raw(["rev-parse", "--verify", "--quiet", "--end-of-options", f"{rev}^{{commit}}"])
        if proc.returncode != 0:
            return None
        sha = proc.stdout.decode("ascii", errors="replace").strip()
        return sha if SHA_RE.match(sha) else None

    def has_commit(self, sha: str) -> bool:
        validate_sha(sha)
        return self.rev_parse_commit(sha) == sha

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        validate_sha(ancestor)
        validate_sha(descendant)
        proc = self.run_raw(["merge-base", "--is-ancestor", ancestor, descendant])
        if proc.returncode == 0:
            return True
        if proc.returncode == 1:
            return False
        raise GitError(
            self._describe(["merge-base", "--is-ancestor"], self.redactor.redact(proc.stderr).strip()),
            proc.returncode,
        )

    def merge_base(self, a: str, b: str) -> str | None:
        proc = self.run_raw(["merge-base", validate_sha(a), validate_sha(b)])
        sha = proc.stdout.decode("ascii", errors="replace").strip()
        return sha if proc.returncode == 0 and SHA_RE.match(sha) else None

    def tree_of(self, sha: str) -> str:
        out = self.run(["rev-parse", "--verify", f"{validate_sha(sha)}^{{tree}}"]).decode().strip()
        return validate_sha(out, "tree id")

    def object_type(self, oid: str) -> str:
        proc = self.run_raw(["cat-file", "-t", validate_sha(oid, "object id")])
        return proc.stdout.decode("ascii", errors="replace").strip() if proc.returncode == 0 else ""

    def parents(self, sha: str) -> list[str]:
        out = self.run(["rev-list", "--parents", "-n", "1", validate_sha(sha)]).decode().split()
        return [validate_sha(p) for p in out[1:]]

    def commits_in(self, include: str, exclude: Iterable[str], limit: int = 5000) -> list[str]:
        args = ["rev-list", f"--max-count={limit}", validate_sha(include)]
        args += [f"^{validate_sha(sha)}" for sha in exclude]
        return [validate_sha(s) for s in self.run(args).decode().split()]

    def count_left_right(self, left: str, right: str) -> tuple[int, int]:
        out = self.run(
            ["rev-list", "--left-right", "--count", f"{validate_sha(left)}...{validate_sha(right)}"]
        ).decode().split()
        if len(out) != 2 or not all(part.isdigit() for part in out):
            raise GitError("git rev-list --count returned unexpected output", 0)
        return int(out[0]), int(out[1])

    def commit_message(self, sha: str) -> str:
        return self.run(["show", "-s", "--format=%B", validate_sha(sha)]).decode("utf-8", "replace")

    # -- remotes ------------------------------------------------------------------------------

    def ls_remote(self, url: str, ref: str) -> str | None:
        """The SHA ``ref`` points to on ``url``, or None if it does not exist."""
        out = self.run(["ls-remote", "--end-of-options", url, ref]).decode("ascii", "replace")
        for line in out.splitlines():
            parts = line.split("\t")
            if len(parts) == 2 and parts[1] == ref:
                return validate_sha(parts[0], f"SHA of {ref}")
        return None

    def fetch(self, url: str, remote_ref: str, local_ref: str) -> str:
        """Fetches ``remote_ref`` into ``local_ref`` (non-forced, no tags) and returns its SHA."""
        if not remote_ref.startswith("refs/") or not local_ref.startswith("refs/sync/"):
            raise ValidationError("fetch refspec outside the expected namespaces")
        self.run_raw(["update-ref", "-d", local_ref])
        self.run(
            ["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--no-recurse-submodules",
             "--end-of-options", url, f"{remote_ref}:{local_ref}"]
        )
        sha = self.rev_parse_commit(local_ref)
        if sha is None:
            raise GitError(f"fetched ref {printable(remote_ref)} is not a commit", 0)
        return sha

    def push(self, url: str, sha: str, dest_ref: str) -> None:
        """Pushes one commit to one branch. Never forced; the guard runs before Git starts."""
        check_push_destination(dest_ref)
        validate_sha(sha)
        # "<sha>:<ref>" with a validated 40-hex SHA can't start with "+", so it is never forced.
        self.run(["push", "--porcelain", "--no-verify", "--end-of-options", url, f"{sha}:{dest_ref}"])


def git_identity(name: str, email: str) -> dict[str, str]:
    return {
        "GIT_AUTHOR_NAME": name,
        "GIT_AUTHOR_EMAIL": email,
        "GIT_COMMITTER_NAME": name,
        "GIT_COMMITTER_EMAIL": email,
    }


# ---------------------------------------------------------------------------------------------
# Environment


def env_str(name: str, default: str | None = None, *, required: bool = False) -> str:
    value = os.environ.get(name)
    if value is None or value == "":
        if required:
            raise SyncError(f"missing required environment variable {name}")
        return default or ""
    return value


def env_flag(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in ("1", "true", "yes")


def validate_repo_slug(value: str, what: str = "repository") -> str:
    if not re.match(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,99}/[A-Za-z0-9][A-Za-z0-9._-]{0,99}$", value or ""):
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    if ".." in value:
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    return value


def validate_https_url(value: str, what: str = "URL", allow_local: bool = False) -> str:
    """Accepts https:// URLs without credentials or query; file:// and http://127.0.0.1 in tests."""
    if allow_local and (value.startswith("file://") or value.startswith("http://127.0.0.1:")):
        return value.rstrip("/")
    if not re.match(r"^https://[A-Za-z0-9.-]+(?::[0-9]{1,5})?(?:/[A-Za-z0-9._~/-]*)?$", value or ""):
        raise ValidationError(f"invalid {what}: {printable(value)!r}")
    return value.rstrip("/")
