"""Static checks on the workflows: the event policy, permissions and untrusted-input rules."""

from __future__ import annotations

import os
import re
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")
NEW = ("forgejo-pr-sync.yml", "forgejo-upstream-sync.yml", "automation-tests.yml")
SYNC = ("forgejo-pr-sync.yml", "forgejo-upstream-sync.yml")

try:
    import yaml  # not in the standard library; the structural checks skip without it
except ImportError:  # pragma: no cover
    yaml = None


def read(name: str) -> str:
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        return handle.read()


def all_workflows() -> list[str]:
    return sorted(n for n in os.listdir(WORKFLOWS) if n.endswith((".yml", ".yaml")))


def code(text: str) -> str:
    """The workflow without comment lines."""
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))


class EventPolicy(unittest.TestCase):
    def test_only_the_bridge_uses_pull_request_target(self):
        # The repository event policy allows pull_request_target for forgejo-pr-sync.yml only.
        users = [n for n in all_workflows() if "pull_request_target" in code(read(n))]
        self.assertEqual(users, ["forgejo-pr-sync.yml"])

    def test_no_workflow_run_triggers_in_new_workflows(self):
        for name in NEW:
            self.assertNotIn("workflow_run", code(read(name)), name)

    def test_forgejo_secrets_only_in_the_sync_workflows(self):
        users = [n for n in all_workflows() if re.search(r"secrets\.FORGEJO", read(n))]
        self.assertEqual(sorted(users), sorted(SYNC))

    def test_tests_workflow_has_no_secrets(self):
        self.assertNotIn("secrets.", read("automation-tests.yml"))


class Hygiene(unittest.TestCase):
    def test_actions_are_pinned_by_sha(self):
        for name in NEW:
            for line in read(name).splitlines():
                match = re.search(r"uses:\s*(\S+)", line)
                if match:
                    self.assertRegex(match.group(1), r"@[0-9a-f]{40}$", f"{name}: {line.strip()}")

    def test_no_write_permissions(self):
        for name in NEW:
            self.assertNotRegex(code(read(name)), r":\s*write\b", name)

    def test_checkouts_do_not_persist_credentials(self):
        for name in NEW:
            text = read(name)
            self.assertEqual(text.count("actions/checkout@"), text.count("persist-credentials: false"), name)

    def test_no_expressions_inside_run_blocks(self):
        for name in NEW:
            lines = read(name).splitlines()
            for i, line in enumerate(lines):
                if re.match(r"^\s*(?:-\s+)?run:", line):
                    block = [line]
                    indent = len(line) - len(line.lstrip())
                    for nxt in lines[i + 1:]:
                        if nxt.strip() and len(nxt) - len(nxt.lstrip()) <= indent:
                            break
                        block.append(nxt)
                    self.assertNotIn("${{", "\n".join(block), f"{name}: an expression in a run: block")

    def test_no_untrusted_pr_text_in_expressions(self):
        untrusted = r"github\.event\.pull_request\.(?:title|body|head\.ref|head\.label)|github\.head_ref"
        for name in NEW:
            self.assertNotRegex(read(name), untrusted, name)

    def test_sync_workflows_check_out_main_and_serialise(self):
        for name in SYNC:
            text = read(name)
            self.assertIn("ref: Main", text, name)
            self.assertIn("cancel-in-progress: false", text, name)
            self.assertIn("environment: forgejo-sync", text, name)
            self.assertIn("github.repository == 'sefr-14/pdfcraft'", text, name)

    def test_bridge_rejects_fork_prs_before_the_job_starts(self):
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", read("forgejo-pr-sync.yml"))

    @unittest.skipIf(yaml is None, "PyYAML not installed")
    def test_structure(self):
        for name in NEW:
            doc = yaml.safe_load(read(name))
            triggers = doc.get("on", doc.get(True))
            self.assertIsInstance(doc.get("permissions"), dict, name)
            for job in doc["jobs"].values():
                self.assertIn("timeout-minutes", job, name)
            if name == "forgejo-pr-sync.yml":
                self.assertEqual(triggers["pull_request_target"]["branches"], ["Main"])
                self.assertEqual(set(triggers["pull_request_target"]["types"]),
                                 {"opened", "synchronize", "reopened", "edited", "closed"})
                self.assertEqual(doc["permissions"], {"contents": "read", "pull-requests": "read"})
            if name == "forgejo-upstream-sync.yml":
                self.assertIn("schedule", triggers)
                self.assertIn("workflow_dispatch", triggers)
                self.assertEqual(doc["permissions"], {"contents": "read"})


if __name__ == "__main__":
    unittest.main()
