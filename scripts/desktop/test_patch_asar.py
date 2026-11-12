import hashlib
import importlib.util
import json
import struct
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('desktop_patch', Path(__file__).with_name('patch-asar.py'))
desktop_patch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop_patch)


def fixture(app, compatible=True):
    resources = app / 'resources'
    resources.mkdir(parents=True)
    composer = b'c=n("zTeN");f=n("zd8w");eX=n("Oj9r");' + desktop_patch.COMPOSER + b' x ' + desktop_patch.INSERTION + b' x ' + desktop_patch.RAIL
    if not compatible:
        composer = composer.replace(desktop_patch.INSERTION, b'changed-upstream')
    members = {
        '.vite/build/bootstrap.js': b'before' + desktop_patch.OLD_GUID + b'after',
        'webview/assets/composer.js': composer,
        'webview/assets/state.js': desktop_patch.STATE_MARKER + b' x broadcastPatchesToFollowers:!1',
        'webview/assets/terminal.js': b'__rspack_esm_ids=[335708];xiIo(e,t,n){ LocalConversationBackgroundTerminalTab',
    }
    root = {'files': {}}
    offset = 0
    for path, data in members.items():
        node = root
        parts = path.split('/')
        for part in parts[:-1]:
            node = node['files'].setdefault(part, {'files': {}})
        checksum = hashlib.sha256(data).hexdigest()
        node['files'][parts[-1]] = {'size': len(data), 'offset': str(offset), 'integrity': {'algorithm': 'SHA256', 'hash': checksum, 'blockSize': 4194304, 'blocks': [checksum]}}
        offset += len(data)
    header = json.dumps(root, separators=(',', ':')).encode()
    padding = (-len(header)) % 4
    size = 8 + len(header) + padding
    (resources / 'app.asar').write_bytes(struct.pack('<IIII', 4, size, size - 4, len(header)) + header + b'\0' * padding + b''.join(members.values()))
    (app / 'ChatGPT.exe').write_bytes(b'prefix' + hashlib.sha256(header).hexdigest().encode() + b'suffix')


class DesktopPatchTests(unittest.TestCase):
    def test_candidate_updates_header_marker_and_preserves_unrelated_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / 'app'
            fixture(app)
            result = desktop_patch.patch(app)
            archive = app / 'resources' / 'app.asar'
            with archive.open('rb') as f:
                _, size, _, json_size = struct.unpack('<IIII', f.read(16))
                raw = f.read(json_size)
                self.assertIn(hashlib.sha256(raw).hexdigest().encode(), (app / 'ChatGPT.exe').read_bytes())
                self.assertEqual(result['composer'], 'webview/assets/composer.js')
                self.assertEqual(result['output_state'], 'webview/assets/state.js')
                header = json.loads(raw)
                entry = header['files']['webview']['files']['assets']['files']['state.js']
                f.seek(8 + size + int(entry['offset']))
                data = f.read(entry['size'])
                self.assertIn(b'broadcastPatchesToFollowers:!0', data)
                self.assertEqual(hashlib.sha256(data).hexdigest(), entry['integrity']['hash'])
            self.assertTrue((app / 'ChatGPT.original.exe').exists())

    def test_incompatible_candidate_is_not_modified(self):
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / 'app'
            fixture(app, compatible=False)
            archive = app / 'resources' / 'app.asar'
            before = archive.read_bytes(), (app / 'ChatGPT.exe').read_bytes()
            with self.assertRaisesRegex(RuntimeError, 'manual adaptation required'):
                desktop_patch.patch(app)
            self.assertEqual(before, (archive.read_bytes(), (app / 'ChatGPT.exe').read_bytes()))


if __name__ == '__main__':
    unittest.main()
