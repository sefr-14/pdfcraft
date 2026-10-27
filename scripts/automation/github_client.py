"""Read-only GitHub REST client for the PR bridge. It never writes to GitHub."""

from __future__ import annotations

import urllib.parse

from common import validate_repo_slug
from forgejo_client import HttpClient


class GitHubClient(HttpClient):
    def __init__(self, api_url: str, token: str | None, **kwargs):
        super().__init__(
            api_url,
            f"Bearer {token}" if token else None,
            name="GitHub",
            extra_headers={"X-GitHub-Api-Version": "2022-11-28", "Accept": "application/vnd.github+json"},
            **kwargs,
        )

    def get_pull(self, repo: str, number: int) -> dict:
        owner, name = validate_repo_slug(repo, "GitHub repository").split("/", 1)
        path = f"/repos/{urllib.parse.quote(owner, safe='')}/{urllib.parse.quote(name, safe='')}/pulls/{int(number)}"
        return self.request("GET", path) or {}
