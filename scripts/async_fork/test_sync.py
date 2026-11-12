import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sync


class SyncTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        # Exercise GitHub reporting without writing into the real workflow's files.
        reporting = patch.dict(
            os.environ,
            {
                "GITHUB_STEP_SUMMARY": str(self.root / "step-summary.md"),
                "GITHUB_OUTPUT": str(self.root / "step-output"),
            },
        )
        reporting.start()
        self.addCleanup(reporting.stop)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        sync.git(self.repo, "init", "-b", "main")
        self.write("source.txt", "official v1\n")
        self.write(".github/workflows/official.yml", "official CI v1\n")
        self.write(".github/workflows/support.sh", "official helper v1\n")
        self.commit("official base")
        self.base = sync.oid(self.repo, "HEAD")
        sync.git(self.repo, "switch", "-c", "async-main")
        self.write(
            sync.CONFIG,
            json.dumps(
                {
                    "repository": "thisisme112/codex-async",
                    "target_branch": "async-main",
                    "source_branch": "upstream/source",
                    "upstream_url": "unused",
                    "upstream_branch": "main",
                }
            ),
        )
        for name, markers in sync.REQUIRED.items():
            self.write(name, "\n".join(markers))
        sync.git(self.repo, "rm", ".github/workflows/official.yml")
        self.write(sync.WORKFLOW, "our trusted workflow\n")
        self.commit("async features")
        self.ours = sync.oid(self.repo, "HEAD")
        self.output = self.root / "output"

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, message):
        sync.git(self.repo, "add", "-A")
        sync.git(self.repo, "commit", "-m", message)

    def upstream(self, protected=False):
        sync.git(self.repo, "switch", "-c", "official", self.base)
        self.write("source.txt", "official v2\n")
        self.write(".github/workflows/official.yml", "official CI v2\n")
        self.write(".github/workflows/support.sh", "official helper v2\n")
        if protected:
            self.write("scripts/async_fork/injected.py", "unexpected control change")
        self.commit("upstream update")
        upstream = sync.oid(self.repo, "HEAD")
        sync.git(self.repo, "switch", "async-main")
        return upstream

    def remote(self):
        path = self.root / "remote.git"
        subprocess.run(
            ["git", "init", "--bare", str(path)], check=True, capture_output=True
        )
        sync.git(self.repo, "remote", "add", "origin", str(path))
        sync.git(self.repo, "push", "origin", "HEAD:refs/heads/async-main")
        return path

    def test_clean_merge_preserves_features_and_excludes_inherited_ci(self):
        result = sync.prepare(self.repo, self.output, self.upstream())
        self.assertEqual(result["status"], "candidate")
        self.assertEqual(sync.oid(self.repo, "HEAD"), self.ours)
        self.assertEqual(sync.git(self.repo, "status", "--porcelain").stdout, "")
        self.assertEqual(
            sync.git(self.repo, "show", f"{result['candidate']}:source.txt").stdout,
            "official v2\n",
        )
        workflows = sync.git(
            self.repo,
            "ls-tree",
            "-r",
            "--name-only",
            result["candidate"],
            ".github/workflows",
        ).stdout.splitlines()
        self.assertEqual(workflows, [sync.WORKFLOW, ".github/workflows/support.sh"])
        self.assertEqual(
            sync.git(
                self.repo,
                "show",
                f"{result['candidate']}:./.github/workflows/support.sh",
            ).stdout,
            "official helper v2\n",
        )
        sync.git(self.repo, "bundle", "verify", str(self.output / "candidate.bundle"))

    def reviewed_conflict(self):
        self.write('source.txt', 'fork source\n')
        self.commit('fork source change')
        ours = sync.oid(self.repo, 'HEAD')
        upstream = self.upstream()
        record = {
            'path': 'source.txt',
            'base_blob': sync.git(self.repo, 'rev-parse', f'{self.base}:source.txt').stdout.strip(),
            'ours_blob': sync.git(self.repo, 'rev-parse', f'{ours}:source.txt').stdout.strip(),
            'theirs_blob': sync.git(self.repo, 'rev-parse', f'{upstream}:source.txt').stdout.strip(),
            'ours': 'fork source\n',
            'theirs': 'official v2\n',
            'resolved': 'official v2 with fork source\n',
        }
        return upstream, record

    def test_reviewed_exact_conflict_is_resolved_without_changing_usable_checkout(self):
        upstream, record = self.reviewed_conflict()
        self.write(sync.RESOLUTIONS, json.dumps([record]))
        self.commit('record reviewed conflict')
        base = sync.oid(self.repo, 'HEAD')
        result = sync.prepare(self.repo, self.output, upstream)
        self.assertEqual(result['status'], 'candidate')
        self.assertEqual(result['resolved_conflicts'], ['source.txt'])
        self.assertEqual(sync.oid(self.repo, 'HEAD'), base)
        self.assertEqual(sync.git(self.repo, 'show', f'{result["candidate"]}:source.txt').stdout, record['resolved'])
        self.assertEqual(sync.git(self.repo, 'status', '--porcelain').stdout, '')

    def test_unreviewed_blob_version_still_blocks_merge(self):
        upstream, record = self.reviewed_conflict()
        record['theirs_blob'] = '0' * 40
        self.write(sync.RESOLUTIONS, json.dumps([record]))
        self.commit('record different version')
        result = sync.prepare(self.repo, self.output, upstream)
        self.assertEqual(result['status'], 'conflict')
        self.assertEqual(result['conflicts'], ['source.txt'])

    def test_unreviewed_conflict_text_still_blocks_merge(self):
        upstream, record = self.reviewed_conflict()
        record['ours'] = 'unreviewed source\n'
        self.write(sync.RESOLUTIONS, json.dumps([record]))
        self.commit('record different conflict text')
        result = sync.prepare(self.repo, self.output, upstream)
        self.assertEqual(result['status'], 'conflict')
        self.assertEqual(result['conflicts'], ['source.txt'])
    def test_conflict_keeps_usable_branch_and_records_paths(self):
        self.write("source.txt", "our incompatible edit\n")
        self.commit("local source edit")
        before = sync.oid(self.repo, "HEAD")
        result = sync.prepare(self.repo, self.output, self.upstream())
        self.assertEqual(result["status"], "conflict")
        self.assertEqual(result["conflicts"], ["source.txt"])
        self.assertEqual(sync.oid(self.repo, "HEAD"), before)
        self.assertEqual(sync.git(self.repo, "status", "--porcelain").stdout, "")
        self.assertFalse((self.output / "candidate.bundle").exists())

    def test_unexpected_upstream_control_change_is_blocked(self):
        with self.assertRaisesRegex(RuntimeError, "control files"):
            sync.prepare(self.repo, self.output, self.upstream(protected=True))
        self.assertEqual(sync.oid(self.repo, "HEAD"), self.ours)
        self.assertEqual(sync.git(self.repo, "status", "--porcelain").stdout, "")

    def test_promotion_advances_both_refs_and_next_run_is_noop(self):
        remote = self.remote()
        upstream = self.upstream()
        result = sync.prepare(self.repo, self.output, upstream)
        sync.promote(self.repo, self.output)
        self.assertEqual(sync.oid(remote, "refs/heads/async-main"), result["candidate"])
        self.assertEqual(
            sync.oid(remote, "refs/heads/upstream/source"), result["source"]
        )
        sync.git(self.repo, "switch", "--detach", result["candidate"])
        next_result = sync.prepare(self.repo, self.root / "second", upstream)
        self.assertEqual(next_result["status"], "up_to_date")

    def test_stale_validation_cannot_overwrite_a_newer_usable_branch(self):
        remote = self.remote()
        sync.prepare(self.repo, self.output, self.upstream())
        self.write("owner-change.txt", "new work after validation started\n")
        self.commit("owner advances usable branch")
        newer = sync.oid(self.repo, "HEAD")
        sync.git(self.repo, "push", "origin", "HEAD:refs/heads/async-main")
        with self.assertRaisesRegex(RuntimeError, "changed during validation"):
            sync.promote(self.repo, self.output)
        self.assertEqual(sync.oid(remote, "refs/heads/async-main"), newer)


if __name__ == "__main__":
    unittest.main()
