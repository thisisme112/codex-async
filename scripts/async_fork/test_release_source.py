import subprocess
import tempfile
import unittest
from pathlib import Path

import release_source


class ReleaseSourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.run_git('init', '-b', 'async-main')
        self.run_git('config', 'user.name', 'Test')
        self.run_git('config', 'user.email', 'test@example.invalid')
        for name in release_source.PACKAGE_INPUTS:
            path = self.root / name
            if name in ('codex-rs', 'scripts/codex_package', 'third_party/v8'):
                path = path / 'input.txt'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('original\n')
        self.commit()
        self.run_git('tag', 'async-original')
        self.releases = [{'tagName': 'async-original', 'isDraft': False}]

    def run_git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, check=True, capture_output=True)

    def commit(self):
        self.run_git('add', '-A')
        self.run_git('commit', '-m', 'test state')

    def match(self):
        return release_source.matching_release(self.root, self.releases, fetch=False)

    def test_cache_workflow_and_prose_changes_reuse_existing_package(self):
        for name in ('README_ASYNC.md', '.github/workflows/async-linux-release.yml', '.github/actions/async-rust-cache/action.yml'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('cache-only configuration\n')
        self.commit()
        self.assertEqual(self.match(), 'async-original')

    def test_rust_change_requires_a_new_build(self):
        (self.root / 'codex-rs/input.txt').write_text('new Rust inputs\n')
        self.commit()
        self.assertIsNone(self.match())

    def test_installer_and_packaging_inputs_require_a_new_build(self):
        for name in ('scripts/async_fork/install-package.sh', 'scripts/codex_package/input.txt', '.github/async-fork-state.json'):
            with self.subTest(name=name):
                path = self.root / name
                original = path.read_text()
                path.write_text('changed\n')
                self.commit()
                self.assertIsNone(self.match())
                path.write_text(original)
                self.commit()

    def test_draft_does_not_suppress_build(self):
        self.releases[0]['isDraft'] = True
        self.assertIsNone(self.match())

    def test_older_incomplete_release_does_not_suppress_build(self):
        self.run_git('rm', 'scripts/async_fork/package.sh')
        self.commit()
        self.run_git('tag', 'async-incomplete')
        self.run_git('reset', '--hard', 'async-original')
        self.releases = [{'tagName': 'async-incomplete', 'isDraft': False}]
        self.assertIsNone(self.match())


if __name__ == '__main__':
    unittest.main()
