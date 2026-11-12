import importlib.util
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('desktop_v8', Path(__file__).with_name('prepare-v8.py'))
desktop_v8 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop_v8)


class PrepareV8Tests(unittest.TestCase):
    def setup_fixture(self, root):
        workspace, checkout = root / 'workspace', root / 'checkout'
        workspace.mkdir()
        checkout.mkdir()
        rev = 'a' * 40
        url = 'https://github.com/openai-oss-forks/rusty_v8.git'
        (workspace / 'Cargo.toml').write_text(f'[patch.crates-io]\nv8 = {{ git = "{url}", rev = "{rev}" }}\n')
        (workspace / 'Cargo.lock').write_text(f'[[package]]\nname = "v8"\nversion = "152.2.0"\nsource = "git+{url}?rev={rev}#{rev}"\n\n[[package]]\nname = "other"\nversion = "1.0.0"\n')
        (checkout / 'Cargo.toml').write_text('[package]\nname = "v8"\nversion = "152.2.0"\n')
        return workspace, checkout, rev

    def test_pinned_source_preserves_every_other_lock_entry(self):
        with tempfile.TemporaryDirectory() as tmp:
            workspace, checkout, rev = self.setup_fixture(Path(tmp))
            before = tomllib.loads((workspace / 'Cargo.lock').read_text())
            with patch.object(desktop_v8, 'run', return_value=rev):
                desktop_v8.prepare(workspace, checkout)
            after = tomllib.loads((workspace / 'Cargo.lock').read_text())
            before['package'][0].pop('source')
            self.assertEqual(before, after)
            self.assertEqual(tomllib.loads((workspace / 'Cargo.toml').read_text())['patch']['crates-io']['v8']['path'], checkout.as_posix())

    def test_wrong_checkout_is_rejected_without_modifying_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            workspace, checkout, _ = self.setup_fixture(Path(tmp))
            before = [(workspace / name).read_bytes() for name in ('Cargo.toml', 'Cargo.lock')]
            with patch.object(desktop_v8, 'run', return_value='b' * 40):
                with self.assertRaisesRegex(RuntimeError, 'pinned commit'):
                    desktop_v8.prepare(workspace, checkout)
            self.assertEqual(before, [(workspace / name).read_bytes() for name in ('Cargo.toml', 'Cargo.lock')])


if __name__ == '__main__':
    unittest.main()
