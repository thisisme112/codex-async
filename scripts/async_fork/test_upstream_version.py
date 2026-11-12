import json
import tempfile
import tomllib
import unittest
from pathlib import Path

import upstream_version as version


class OfficialVersionTests(unittest.TestCase):
    def test_semver_order_preserves_official_prerelease(self):
        values = ['0.170.0-alpha.10', '0.167.2', '0.170.0', '0.170.0-alpha.2']
        self.assertEqual(sorted(values, key=version.version_key), ['0.167.2', '0.170.0-alpha.2', '0.170.0-alpha.10', '0.170.0'])

    def test_newer_release_is_not_used_before_its_source_is_synced(self):
        releases = [{'tagName': 'rust-v0.170.0-alpha.2', 'isDraft': False}, {'tagName': 'rust-v0.171.0-alpha.1', 'isDraft': False}]
        def api(path):
            if '/git/ref/tags/' in path:
                return {'object': {'type': 'commit', 'sha': path.rsplit('/', 1)[1]}}
            if '/commits/' in path:
                tag = path.rsplit('/', 1)[1]
                return {'parents': [{'sha': 'future' if '0.171' in tag else 'included'}], 'files': [{'filename': 'codex-rs/Cargo.toml', 'patch': '-version = "0.0.0"\n+version = "' + tag[6:] + '"'}]}
            return {'status': 'behind' if '/future...' in path else 'ahead'}
        result = version.resolve('synced-source', releases, api)
        self.assertEqual(result['version'], '0.170.0-alpha.2')
        self.assertEqual(result['release_source'], 'included')
        self.assertEqual(result['official_tag'], 'rust-v0.170.0-alpha.2')

    def test_code_changes_on_a_release_commit_are_not_ignored(self):
        releases = [{'tagName': 'rust-v0.170.0', 'isDraft': False}]
        def api(path):
            if '/git/ref/tags/' in path:
                return {'object': {'type': 'commit', 'sha': 'release-with-code'}}
            if '/commits/' in path:
                return {'parents': [{'sha': 'already-included'}], 'files': [{'filename': 'codex-rs/Cargo.toml'}, {'filename': 'codex-rs/core/src/lib.rs'}]}
            self.assertIn('/release-with-code...', path)
            return {'status': 'behind'}
        with self.assertRaisesRegex(RuntimeError, 'No official release'):
            version.resolve('old-source', releases, api)

    def test_annotated_official_tag_is_dereferenced(self):
        def api(path):
            if '/git/ref/tags/' in path:
                return {'object': {'type': 'tag', 'sha': 'annotation'}}
            if '/git/tags/' in path:
                return {'object': {'type': 'commit', 'sha': 'release-commit'}}
            if '/commits/' in path:
                return {'parents': [{'sha': 'main-source'}], 'files': [{'filename': 'codex-rs/Cargo.toml', 'patch': '-version = "0.0.0"\n+version = "0.170.0-alpha.2"'}]}
            return {'status': 'identical'}
        record = version.resolve('main-source', [{'tagName': 'rust-v0.170.0-alpha.2', 'isDraft': False}], api)
        self.assertEqual(record['release_commit'], 'release-commit')
        self.assertEqual(record['release_source'], 'main-source')


class VersionStampTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        rs = self.root / 'codex-rs'
        rs.mkdir()
        (rs / 'Cargo.toml').write_text('[workspace]\nmembers = ["a", "b", "fixed"]\n[workspace.package]\nversion = "0.0.0"\n')
        for name in ('a', 'b', 'fixed'):
            (rs / name).mkdir()
            value = '"0.0.0"' if name == 'fixed' else '{ workspace = true }'
            (rs / name / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = {value}\n')
        (rs / 'Cargo.lock').write_text('''version = 4
[[package]]
name = "a"
version = "0.0.0"
dependencies = ["b 0.0.0", "b 0.0.0 (registry+https://example.invalid)"]
[[package]]
name = "b"
version = "0.0.0"
[[package]]
name = "fixed"
version = "0.0.0"
[[package]]
name = "b"
version = "0.0.0"
source = "registry+https://example.invalid"
checksum = "preserved"
''')
        self.record = {'version': '0.170.0-alpha.2'}

    def test_stamp_preserves_registry_dependencies_and_fixed_packages(self):
        version.stamp_workspace(self.root, self.record)
        rs = self.root / 'codex-rs'
        self.assertEqual(tomllib.loads((rs / 'Cargo.toml').read_text())['workspace']['package']['version'], self.record['version'])
        packages = tomllib.loads((rs / 'Cargo.lock').read_text())['package']
        self.assertEqual([p['version'] for p in packages], [self.record['version'], self.record['version'], '0.0.0', '0.0.0'])
        self.assertEqual(packages[0]['dependencies'], ['b ' + self.record['version'], 'b 0.0.0 (registry+https://example.invalid)'])
        self.assertEqual(packages[-1]['checksum'], 'preserved')

    def test_repeated_stamp_is_identical(self):
        version.stamp_workspace(self.root, self.record)
        files = [self.root / 'codex-rs' / name for name in ('Cargo.toml', 'Cargo.lock')]
        first = [p.read_bytes() for p in files]
        version.stamp_workspace(self.root, self.record)
        self.assertEqual(first, [p.read_bytes() for p in files])

    def test_unexpected_existing_release_version_is_rejected(self):
        path = self.root / 'codex-rs/Cargo.toml'
        path.write_text(path.read_text().replace('0.0.0', '0.168.0'))
        with self.assertRaisesRegex(ValueError, 'conflicts with official'):
            version.stamp_workspace(self.root, self.record)

    def test_record_cannot_label_a_different_upstream(self):
        (self.root / '.github').mkdir()
        (self.root / '.github/async-fork-state.json').write_text(json.dumps({'upstream': 'current'}))
        path = self.root / 'version.json'
        path.write_text(json.dumps({'version': '0.170.0-alpha.2', 'official_tag': 'rust-v0.170.0-alpha.2', 'upstream': 'different'}))
        with self.assertRaisesRegex(ValueError, 'does not match'):
            version.load_record(self.root, path)


if __name__ == '__main__':
    unittest.main()
