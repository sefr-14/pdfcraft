#!/usr/bin/env python3
"""Read-only check that a Forgejo server offers every API operation the sync scripts use.

    python3 -I scripts/automation/forgejo_api_check.py [--url https://git.ferrive.me] [--swagger FILE]

Fetches ``/api/v1/version`` and ``/swagger.v1.json`` without credentials and checks each entry of
``forgejo_client.REQUIRED_OPERATIONS``. Writes nothing anywhere. Exit status 0 when everything is
present on a Forgejo 15.x server, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from common import SyncError, printable, validate_https_url  # noqa: E402
from forgejo_client import REQUIRED_OPERATIONS, HttpClient  # noqa: E402

EXPECTED_MAJOR = 15


def check(version: str, swagger: dict, out=None) -> bool:
    out = out if out is not None else sys.stdout
    ok = True
    match = re.match(r"^v?([0-9]+)\.", version or "")
    major = int(match.group(1)) if match else None
    if major != EXPECTED_MAJOR:
        out.write(f"FAIL  version {printable(version)!r}: expected Forgejo {EXPECTED_MAJOR}.x\n")
        ok = False
    else:
        out.write(f"ok    version {printable(version)}\n")
    base = swagger.get("basePath")
    if base not in ("/api/v1", None):
        out.write(f"FAIL  swagger basePath {printable(base)!r}, expected /api/v1\n")
        ok = False
    paths = swagger.get("paths") if isinstance(swagger.get("paths"), dict) else {}
    for method, path in REQUIRED_OPERATIONS:
        item = paths.get(path)
        if isinstance(item, dict) and method in item:
            out.write(f"ok    {method.upper():5} {path}\n")
        else:
            out.write(f"FAIL  {method.upper():5} {path} is missing\n")
            ok = False
    return ok


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--url", default=os.environ.get("FORGEJO_URL", "https://git.ferrive.me"))
    parser.add_argument("--swagger", help="read the swagger document from a file instead")
    parser.add_argument("--version-string", help="use this version instead of asking the server")
    parser.add_argument("--allow-local", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        url = validate_https_url(args.url, "Forgejo URL", allow_local=args.allow_local)
        client = HttpClient(url, None, name="Forgejo")
        if args.swagger:
            with open(args.swagger, encoding="utf-8") as handle:
                swagger = json.load(handle)
        else:
            swagger = client.request("GET", "/swagger.v1.json") or {}
        version = args.version_string or str((client.request("GET", "/api/v1/version") or {}).get("version", ""))
    except (SyncError, OSError, ValueError) as exc:
        sys.stdout.write(f"FAIL  {printable(exc, 300)}\n")
        return 1
    return 0 if check(version, swagger if isinstance(swagger, dict) else {}) else 1


if __name__ == "__main__":
    sys.exit(main())
